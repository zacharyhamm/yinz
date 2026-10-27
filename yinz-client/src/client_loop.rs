//! The client's main event loop.
//!
//! Every instruction the client acts on — whether it came from the server via
//! the router thread, or from the input thread — arrives on one channel and
//! is dispatched here. The loop owns the terminal: it is the only place that
//! paints output, and the only place that decides the client is done.

use crate::os_input_output::ClientOsApi;
use crate::render_budget::RenderBudget;
use crate::router::SlowWarnings;
use crate::stdin_ansi_parser::{ForwardKind, StdinAnsiParser, SyncOutput};
use crate::terminal_escapes::{stalled_output_abandoned, write_synchronised, ClientTeardown};
use crate::{
    command_is_executing::CommandIsExecuting, nested_reannounce::NestedReannounce,
    stdin_ansi_parser, ClientInstruction, InputInstruction,
};
use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use yinz_runtime::channels::SenderWithContext;
use yinz_runtime::errors::prelude::*;
use yinz_types::diagnostics::ContextType;
use {
    yinz_ipc::ipc::ClientToServerMsg, yinz_ipc::ipc::ExitReason, yinz_ipc::nested_session,
    yinz_types::data::SessionConnection,
};

/// Why the loop stopped, and what the caller should do next.
pub struct ClientOutcome {
    /// The message to show as the client hands the terminal back.
    pub exit_msg: String,
    /// The process status to use after orderly cleanup.
    pub exit_status: i32,
    /// Set when the client should reconnect to a different session rather
    /// than exiting.
    pub reconnect_to_session: Option<SessionConnection>,
    pub(crate) prepared_connection: Option<crate::session_switch::PreparedConnection>,
}

pub(crate) struct ClientLoop<'a, 'config> {
    pub recovery: &'a mut Option<crate::remote_recovery::Recovery>,
    pub send_client_instructions: Option<&'a SenderWithContext<ClientInstruction>>,
    pub sessions: &'a mut crate::session_switch::Sessions<'config>,
    pub remote_host: Option<&'a str>,
    pub os_input: &'a dyn ClientOsApi,
    pub command_is_executing: &'a mut CommandIsExecuting,
    pub stdin_ansi_parser: &'a Arc<Mutex<StdinAnsiParser>>,
    pub send_input_instructions: &'a SenderWithContext<InputInstruction>,
    /// Re-announces this client to its Yinz host, and knows whether one has
    /// ever answered — which is how the client learns it is nested at all.
    pub nested_reannounce: &'a NestedReannounce,
    /// How to hand the terminal back for fatal instructions that exit immediately.
    /// Terminal write failures return through `ClientOutcome` for orderly cleanup.
    pub teardown: ClientTeardown,
    /// The host's synchronised-output support, which the server can change
    /// at runtime once it learns what the terminal advertised.
    pub synchronised_output: Option<SyncOutput>,
    /// Released as each frame reaches the terminal, so the router reads the next.
    pub render_budget: &'a Arc<RenderBudget>,
    pub startup_started: Instant,
    pub splash_deadline: Option<Instant>,
}

impl ClientLoop<'_, '_> {
    /// Run until the server tells us to exit or to switch sessions.
    pub fn run(
        mut self,
        receive_client_instructions: &yinz_runtime::channels::Receiver<(
            ClientInstruction,
            yinz_types::diagnostics::ErrorContext,
        )>,
    ) -> ClientOutcome {
        let mut outcome = ClientOutcome {
            exit_msg: String::new(),
            exit_status: 0,
            reconnect_to_session: None,
            prepared_connection: None,
        };
        let started_at = Instant::now();
        let mut waiting_for_first_render = true;
        let mut slow_warnings = SlowWarnings::default();
        let mut deferred = VecDeque::new();

        self.announce_synchronised_output();

        loop {
            if receive_client_instructions.is_empty()
                && deferred.is_empty()
                && self.poll_recovery(&mut outcome)
            {
                break;
            }
            let waiting_for_splash = self.splash_deadline.filter(|end| *end > Instant::now());
            let next = if let Some(deadline) = waiting_for_splash {
                match receive_client_instructions.recv_deadline(deadline) {
                    Ok(instruction) => instruction,
                    Err(yinz_runtime::channels::RecvTimeoutError::Timeout) => continue,
                    Err(e) => panic!("failed to receive app instruction on channel: {e}"),
                }
            } else if let Some(instruction) = deferred.pop_front() {
                instruction
            } else if self.recovery.is_some() {
                match receive_client_instructions.recv_timeout(crate::remote_recovery::READ_POLL) {
                    Ok(instruction) => instruction,
                    Err(yinz_runtime::channels::RecvTimeoutError::Timeout) => continue,
                    Err(error) => panic!("failed to receive app instruction: {error}"),
                }
            } else {
                receive_client_instructions
                    .recv()
                    .expect("failed to receive app instruction on channel")
            };
            let next = match next {
                (ClientInstruction::FromServer(generation, instruction), context) => {
                    if self
                        .recovery
                        .as_ref()
                        .is_some_and(|recovery| !recovery.control.accepts(generation))
                    {
                        if let ClientInstruction::Render(output) = *instruction {
                            self.render_budget.release(output.len()).non_fatal();
                        }
                        continue;
                    }
                    (*instruction, context)
                },
                next => next,
            };
            if waiting_for_splash.is_some() && matches!(next.0, ClientInstruction::Render(_)) {
                // Frames may be incremental; keep every frame and its budget reservation.
                deferred.push_back(next);
                continue;
            }
            let (client_instruction, mut err_ctx) = next;

            yinz_runtime::errors::add_call(
                &mut err_ctx,
                ContextType::Client((&client_instruction).into()),
            );

            let first_render_bytes = if waiting_for_first_render {
                match &client_instruction {
                    ClientInstruction::Render(output) => Some(output.len()),
                    _ => None,
                }
            } else {
                None
            };
            if let Some(bytes) = first_render_bytes {
                log::info!(
                    "startup trace: client pid={} client loop received first Render bytes={bytes} after {:?}; terminal write begin",
                    std::process::id(),
                    started_at.elapsed()
                );
            }
            let should_exit = self.handle(client_instruction, &mut outcome, &mut slow_warnings);
            if first_render_bytes.is_some() {
                let result = if outcome.exit_status != 0 {
                    "error"
                } else if stalled_output_abandoned() {
                    "abandoned"
                } else {
                    "ok"
                };
                log::info!(
                    "startup trace: client pid={} first Render terminal write finished outcome={result} client_startup_us={} loop_elapsed_us={}",
                    std::process::id(),
                    self.startup_started.elapsed().as_micros(),
                    started_at.elapsed().as_micros()
                );
                if result == "ok"
                    && let Some(host) = self.remote_host
                {
                    yinz_runtime::remote_hosts::remember(
                        &yinz_runtime::consts::YINZ_CACHE_DIR,
                        host,
                    )
                    .non_fatal();
                }
                waiting_for_first_render = false;
            }
            if should_exit {
                break;
            }
        }

        for (instruction, _) in deferred {
            if let ClientInstruction::Render(output) = instruction {
                self.render_budget.release(output.len()).non_fatal();
            }
        }

        if waiting_for_first_render {
            log::warn!("startup trace: client exited before first render exit_status={} client_startup_us={}", outcome.exit_status, self.startup_started.elapsed().as_micros());
        }

        if let Some((suppressed, max_suppressed)) = slow_warnings.finish() {
            log::warn!("output pressure: client pid={} stage=terminal_write_flush final suppressed={suppressed} max_suppressed_ms={}", std::process::id(), max_suppressed.as_millis());
        }

        if let Some(recovery) = self.recovery {
            recovery.shutdown();
        }
        self.nested_reannounce.stop();

        outcome
    }

    fn poll_recovery(&mut self, outcome: &mut ClientOutcome) -> bool {
        let Some(recovery) = self.recovery else {
            return false;
        };
        if recovery.control.phase() == crate::remote_recovery::Phase::Reconnecting {
            self.sessions.cancel();
            let mut parser = self.stdin_ansi_parser.lock().unwrap();
            parser.take_active(ForwardKind::Barrier);
            parser.take_active(ForwardKind::Clipboard);
        }
        let result = (|| -> Result<()> {
            if let Some(mut prepared) =
                recovery.poll(self.os_input, self.sessions.name(), Instant::now())?
            {
                recovery.install(self.os_input, &mut prepared)?;
                // The router owns a shared budget clone, supplied by the startup path.
                let router = crate::router::spawn_with_initial(
                    self.os_input.box_clone(),
                    self.send_client_instructions
                        .context("recovery instruction sender is missing")?
                        .clone(),
                    self.render_budget.clone(),
                    std::mem::take(&mut prepared.buffered),
                );
                recovery.routers.push(router);
            }
            recovery.paint_status(self.os_input, self.sessions.name(), Instant::now())
        })();
        if let Err(error) = result {
            outcome.exit_msg = format!("Remote attachment failed: {error:#}");
            outcome.exit_status = if recovery.interrupted() {
                crate::INTERRUPTED_EXIT_STATUS
            } else {
                1
            };
            return true;
        }
        false
    }

    /// Tell the server whether this client's host presents each frame atomically.
    ///
    /// The loop needs the flavour, to wrap each frame in the right markers; the server needs only
    /// the yes/no, so that it can stop hiding the cursor while a frame paints. Both are said from
    /// here, because a second thread announcing the same decision could be ordered against this
    /// one and leave the server holding the opposite of what the client is doing.
    fn announce_synchronised_output(&self) {
        self.os_input
            .send_to_server(ClientToServerMsg::SynchronizedOutputSupport {
                supported: self.synchronised_output.is_some(),
            });
    }

    /// Act on one instruction. Returns whether the loop should stop.
    fn handle(
        &mut self,
        instruction: ClientInstruction,
        outcome: &mut ClientOutcome,
        slow_warnings: &mut SlowWarnings,
    ) -> bool {
        match instruction {
            ClientInstruction::FromServer(_, _) => {
                unreachable!("server generation is checked before dispatch")
            },
            ClientInstruction::Exit(reason) => {
                self.os_input
                    .send_to_server(ClientToServerMsg::ClientExited);
                if let ExitReason::Error(_) = reason {
                    self.fail(reason.to_string());
                }
                outcome.exit_msg = reason.to_string();
                return true;
            },
            ClientInstruction::SessionRequest(request) => {
                use yinz_types::session_destination::{
                    ClientReply, ClientRequest, Reply, SwitchRequest,
                };
                let (result, prepared) =
                    if let ClientRequest::Switch(SwitchRequest::Commit(id)) = &request.request {
                        match self.sessions.commit(id) {
                            Ok(prepared) => (Ok(ClientReply::Complete), Some(prepared)),
                            Err(error) => (Err(error), None),
                        }
                    } else {
                        self.sessions.handle(request.request, self.os_input)
                    };
                if !request.token.is_empty() {
                    self.os_input
                        .send_to_server(ClientToServerMsg::SessionReply(Reply {
                            token: request.token,
                            result: result.map_err(|error| format!("{error:#}")),
                        }));
                }
                if let Some(prepared) = prepared {
                    outcome.prepared_connection = Some(prepared);
                    self.os_input
                        .send_to_server(ClientToServerMsg::ClientExited);
                    return true;
                }
            },
            ClientInstruction::SwitchSession(connect_to_session) => {
                outcome.reconnect_to_session = Some(connect_to_session);
                self.os_input
                    .send_to_server(ClientToServerMsg::ClientExited);
                return true;
            },
            ClientInstruction::Error(backtrace) => self.fail(backtrace),
            ClientInstruction::Render(mut output) => {
                let render_bytes = output.len();
                if self.splash_deadline.take().is_some() {
                    output.insert_str(0, crate::splash::CLEAR);
                }
                let started = Instant::now();
                let mut stdout = self.os_input.get_stdout_writer();
                let written =
                    write_synchronised(&mut *stdout, self.synchronised_output, output.as_bytes());
                self.render_budget.release(render_bytes).non_fatal();
                if matches!(&written, Err(e) if e.kind() == std::io::ErrorKind::TimedOut)
                    && stalled_output_abandoned()
                {
                    // Quitting cancelled this frame. Keep draining instructions so the
                    // server's exit reason, rather than output backpressure, decides the status.
                    return false;
                }
                if let Err(e) = written
                    .context("failed to write a frame to the terminal")
                    .to_log()
                {
                    self.os_input
                        .send_to_server(ClientToServerMsg::ClientExited);
                    outcome.exit_msg =
                        format!("Could not write to the terminal: {}", e.root_cause());
                    outcome.exit_status = 1;
                    return true;
                }
                if let Some(recovery) = self.recovery {
                    recovery.first_render(self.os_input, Instant::now());
                }
                let elapsed = started.elapsed();
                if let Some((suppressed, max_suppressed)) =
                    slow_warnings.record(Instant::now(), elapsed)
                {
                    log::warn!("output pressure: client pid={} stage=terminal_write_flush render_bytes={render_bytes} elapsed_ms={} suppressed={suppressed} max_suppressed_ms={}", std::process::id(), elapsed.as_millis(), max_suppressed.as_millis());
                }
            },
            ClientInstruction::UnblockInputThread => {
                self.command_is_executing.unblock_input_thread();
            },
            ClientInstruction::Log(lines_to_log) => {
                for line in lines_to_log {
                    log::info!("{line}");
                }
            },
            ClientInstruction::LogError(lines_to_log) => {
                for line in lines_to_log {
                    log::error!("{line}");
                }
            },
            ClientInstruction::SetSynchronizedOutput(enabled) => {
                let support_changed = enabled.is_some() != self.synchronised_output.is_some();
                self.synchronised_output = enabled;
                if support_changed {
                    self.announce_synchronised_output();
                }
            },
            ClientInstruction::QueryTerminalSize => {
                self.os_input
                    .send_to_server(ClientToServerMsg::TerminalResize {
                        new_size: self.os_input.get_terminal_size(),
                    });
            },
            ClientInstruction::ForwardQueryToHost {
                token,
                query_bytes,
                resolve_async,
            } => self.forward_query_to_host(token, query_bytes, resolve_async),
            ClientInstruction::EmitNestedSessionFrame(payload_bytes) => {
                // Only a client whose host has answered is nested; anywhere
                // else the frame would land in a terminal that ignores it.
                if self.nested_reannounce.host_contacted() {
                    let frame = nested_session::encode_frame_from_payload(&payload_bytes);
                    let mut out = self.os_input.get_stdout_writer();
                    let _ = out.write_all(&frame);
                    let _ = out.flush();
                }
            },
            // Handled entirely on the server side, or carried only for the
            // error context they add to the trace.
            ClientInstruction::SetMouseMode(enabled) => {
                self.send_input_instructions
                    .send(InputInstruction::SetMouseMode(enabled))
                    .context("updating mouse mode")
                    .non_fatal();
            },
            ClientInstruction::RenamedSession(name) => {
                self.sessions.rename(name.into_string());
            },
            ClientInstruction::Connected
            | ClientInstruction::UnblockCliPipeInput(..)
            | ClientInstruction::CliPipeOutput(..)
            | ClientInstruction::ConfigFileUpdated => {},
        }
        false
    }

    /// Restore the terminal, report `message`, and exit non-zero.
    fn fail(&mut self, message: String) -> ! {
        if let Some(recovery) = self.recovery {
            recovery.shutdown();
        }
        self.sessions.shutdown();
        self.teardown.tear_down(self.os_input, message, 1)
    }

    /// Forward a query to the host terminal and arrange for its reply to
    /// find its way back to the server.
    fn forward_query_to_host(&self, token: u32, query_bytes: Vec<u8>, resolve_async: bool) {
        let kind = if resolve_async {
            ForwardKind::Clipboard
        } else {
            ForwardKind::Barrier
        };

        // 1. Open a forwarding window on the parser so replies that arrive
        //    before the closing signal are captured. A slot may still be
        //    open: the server's backstop timeout can give up on the previous
        //    token and dispatch this one before this client's per-slot timer
        //    got to run. Hand that slot over instead of clobbering it.
        let stale_forward = {
            let mut parser = self.stdin_ansi_parser.lock().unwrap();
            let stale_forward = parser.take_active(kind);
            parser.open(kind, token);
            stale_forward
        };
        if let Some((stale_token, stale_reply_bytes)) = stale_forward {
            log::warn!(
                "{} slot for token {} was still open when token {} was dispatched \
                 ({} accumulated bytes); closing it out",
                kind.label(),
                stale_token,
                token,
                stale_reply_bytes.len(),
            );
            let _ = self.send_input_instructions.send(
                InputInstruction::ForwardedReplyFromHostComplete {
                    token: stale_token,
                    reply_bytes: stale_reply_bytes,
                },
            );
        }

        // 2. Arm a per-forward deadline on the dedicated runtime. When it
        //    fires, the task closes the slot (if it is still open for this
        //    token) and relays `ForwardedReplyFromHostComplete` so the
        //    server releases `forward_in_flight` and dispatches the next
        //    queued forward.
        let runtime = stdin_ansi_parser::forward_timeout_runtime();
        let sender_for_timer = self.send_input_instructions.clone();
        stdin_ansi_parser::schedule_timeout(
            runtime.handle(),
            self.stdin_ansi_parser.clone(),
            kind,
            token,
            kind.timeout(),
            move |token, reply_bytes| {
                let _ = sender_for_timer
                    .send(InputInstruction::ForwardedReplyFromHostComplete { token, reply_bytes });
            },
        );

        // 3. Write the query, plus whatever provokes the closing reply, in a
        //    single write_all. When that reply arrives it closes the window
        //    parser-side, and the timer's eventual wake-up finds an empty
        //    slot and no-ops.
        let mut blob = query_bytes;
        blob.extend_from_slice(kind.barrier_suffix());
        let mut out = self.os_input.get_stdout_writer();
        let _ = out.write_all(&blob);
        let _ = out.flush();
    }
}
