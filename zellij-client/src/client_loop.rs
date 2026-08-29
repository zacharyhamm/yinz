//! The client's main event loop.
//!
//! Every instruction the client acts on — whether it came from the server via
//! the router thread, or from the input thread — arrives on one channel and
//! is dispatched here. The loop owns the terminal: it is the only place that
//! paints output, and the only place that decides the client is done.

use crate::os_input_output::ClientOsApi;
use crate::stdin_ansi_parser::{ForwardKind, StdinAnsiParser, SyncOutput};
use crate::terminal_escapes::{write_synchronised, ClientTeardown};
use crate::{
    command_is_executing::CommandIsExecuting, nested_reannounce::NestedReannounce,
    spawn_web_server, stdin_ansi_parser, ClientInstruction, InputInstruction,
};
use std::io::Write;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use zellij_utils::cli::CliArgs;
use zellij_utils::shared::web_server_base_url;
use zellij_utils::{
    channels::SenderWithContext,
    data::ConnectToSession,
    errors::ContextType,
    ipc::{ClientToServerMsg, ExitReason},
    nested_session,
};

/// Where the web server would be reachable, if this client is asked to start
/// one. Resolved from config up front so the loop doesn't have to.
pub(crate) struct WebServerAddress {
    pub ip: IpAddr,
    pub port: u16,
    pub has_certificate: bool,
    pub enforce_https_for_localhost: bool,
}

impl WebServerAddress {
    fn base_url(&self) -> String {
        web_server_base_url(
            self.ip,
            self.port,
            self.has_certificate,
            self.enforce_https_for_localhost,
        )
    }
}

/// Why the loop stopped, and what the caller should do next.
pub(crate) struct LoopOutcome {
    /// The message to show as the client hands the terminal back.
    pub exit_msg: String,
    /// Set when the client should reconnect to a different session rather
    /// than exiting.
    pub reconnect_to_session: Option<ConnectToSession>,
}

pub(crate) struct ClientLoop<'a> {
    pub os_input: &'a dyn ClientOsApi,
    pub cli_args: &'a CliArgs,
    pub command_is_executing: &'a mut CommandIsExecuting,
    pub stdin_ansi_parser: &'a Arc<Mutex<StdinAnsiParser>>,
    pub send_input_instructions: &'a SenderWithContext<InputInstruction>,
    /// Re-announces this client to its Zellij host, and knows whether one has
    /// ever answered — which is how the client learns it is nested at all.
    pub nested_reannounce: &'a NestedReannounce,
    pub web_server_address: WebServerAddress,
    /// How to hand the terminal back when an instruction turns out to be
    /// fatal. Errors do not return through `LoopOutcome` — they exit.
    pub teardown: ClientTeardown,
    /// The host's synchronised-output support, which the server can change
    /// at runtime once it learns what the terminal advertised.
    pub synchronised_output: Option<SyncOutput>,
}

impl ClientLoop<'_> {
    /// Run until the server tells us to exit or to switch sessions.
    pub fn run(
        mut self,
        receive_client_instructions: &zellij_utils::channels::Receiver<(
            ClientInstruction,
            zellij_utils::errors::ErrorContext,
        )>,
    ) -> LoopOutcome {
        let mut outcome = LoopOutcome {
            exit_msg: String::new(),
            reconnect_to_session: None,
        };

        loop {
            let (client_instruction, mut err_ctx) = receive_client_instructions
                .recv()
                .expect("failed to receive app instruction on channel");

            err_ctx.add_call(ContextType::Client((&client_instruction).into()));

            if self.handle(client_instruction, &mut outcome) {
                break;
            }
        }

        self.nested_reannounce.stop();

        outcome
    }

    /// Act on one instruction. Returns whether the loop should stop.
    fn handle(&mut self, instruction: ClientInstruction, outcome: &mut LoopOutcome) -> bool {
        match instruction {
            ClientInstruction::Exit(reason) => {
                self.os_input
                    .send_to_server(ClientToServerMsg::ClientExited);
                if let ExitReason::Error(_) = reason {
                    self.fail(reason.to_string());
                }
                outcome.exit_msg = reason.to_string();
                return true;
            },
            ClientInstruction::SwitchSession(connect_to_session) => {
                outcome.reconnect_to_session = Some(connect_to_session);
                self.os_input
                    .send_to_server(ClientToServerMsg::ClientExited);
                return true;
            },
            ClientInstruction::Error(backtrace) => self.fail(backtrace),
            ClientInstruction::Render(output) => {
                let mut stdout = self.os_input.get_stdout_writer();
                write_synchronised(&mut *stdout, self.synchronised_output, output.as_bytes());
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
                self.synchronised_output = enabled;
            },
            ClientInstruction::QueryTerminalSize => {
                self.os_input
                    .send_to_server(ClientToServerMsg::TerminalResize {
                        new_size: self.os_input.get_terminal_size(),
                    });
            },
            ClientInstruction::StartWebServer => self.start_web_server(),
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
            ClientInstruction::Connected
            | ClientInstruction::UnblockCliPipeInput(..)
            | ClientInstruction::CliPipeOutput(..)
            | ClientInstruction::RenamedSession(..)
            | ClientInstruction::ConfigFileUpdated => {},
        }
        false
    }

    /// Restore the terminal, report `message`, and exit non-zero.
    fn fail(&self, message: String) -> ! {
        self.teardown.tear_down(self.os_input, message, 1)
    }

    fn start_web_server(&self) {
        match spawn_web_server(self.cli_args) {
            Ok(_) => {
                self.os_input
                    .send_to_server(ClientToServerMsg::WebServerStarted {
                        base_url: self.web_server_address.base_url(),
                    });
            },
            Err(e) => {
                log::error!("Failed to start web_server: {}", e);
                self.os_input
                    .send_to_server(ClientToServerMsg::FailedToStartWebServer { error: e });
            },
        }
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
