//! The thread that turns server IPC messages into client instructions.
//!
//! It is the only place that calls `recv_from_server`, and it does nothing
//! with what it receives beyond translating and forwarding it, so that the
//! main loop never blocks on the socket.

use crate::os_input_output::ClientOsApi;
use crate::render_budget::RenderBudget;
use crate::ClientInstruction;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use yinz_runtime::channels::SenderWithContext;
use yinz_runtime::errors::prelude::*;
use yinz_types::diagnostics::ErrorContext;
use {yinz_ipc::ipc::IpcRecv, yinz_ipc::ipc::ServerToClientMsg};

/// How many unreadable messages in a row we tolerate before concluding the
/// stream is not going to recover and tearing the client down. A single
/// failure is survivable — a run of them means the stream has desynchronised.
///
/// This applies to malformed frames only. A hangup is not a malformed frame:
/// it is terminal and returns instantly, so retrying it a thousand times is a
/// busy loop that reports the wrong cause.
const MAX_CONSECUTIVE_MALFORMED_MESSAGES: usize = 1000;

/// Between malformed frames, so that a stream that produces them without ever
/// blocking cannot spin a core on the way to giving up.
const MALFORMED_MESSAGE_BACKOFF: Duration = Duration::from_millis(1);

// A peer may keep producing complete frames after our writer fails. Limit both
// ready frames and transport read time, excluding render/queue backpressure.
const MAX_DRAIN_FRAMES: usize = 256;

/// What the user is told when the server's socket closes without an `Exit`.
/// The server sends one on every shutdown path, so reaching this means it went
/// away without saying goodbye.
const SERVER_HUNG_UP: &str =
    "The connection to the yinz server closed unexpectedly.\nFor a local session, run `yinz ls`.";

#[derive(Default)]
pub(crate) struct SlowWarnings {
    last_logged: Option<Instant>,
    suppressed: usize,
    max_suppressed: Duration,
}

impl SlowWarnings {
    pub(crate) fn record(&mut self, now: Instant, elapsed: Duration) -> Option<(usize, Duration)> {
        if elapsed < Duration::from_millis(100) {
            return None;
        }
        if self
            .last_logged
            .is_some_and(|last| now.duration_since(last) < Duration::from_secs(1))
        {
            self.suppressed += 1;
            self.max_suppressed = self.max_suppressed.max(elapsed);
            return None;
        }
        self.last_logged = Some(now);
        let summary = (self.suppressed, self.max_suppressed);
        self.suppressed = 0;
        self.max_suppressed = Duration::ZERO;
        Some(summary)
    }

    pub(crate) fn finish(&self) -> Option<(usize, Duration)> {
        (self.suppressed > 0).then_some((self.suppressed, self.max_suppressed))
    }
}

/// Spawn the router thread. It runs until the server says `Exit` or the
/// socket gives up, and the caller joins it during shutdown.
#[cfg(test)]
pub(crate) fn spawn(
    os_input: Box<dyn ClientOsApi>,
    send_client_instructions: SenderWithContext<ClientInstruction>,
    render_budget: Arc<RenderBudget>,
) -> thread::JoinHandle<()> {
    spawn_with_initial(
        os_input,
        send_client_instructions,
        render_budget,
        Default::default(),
    )
}

pub(crate) fn spawn_with_initial(
    os_input: Box<dyn ClientOsApi>,
    send_client_instructions: SenderWithContext<ClientInstruction>,
    render_budget: Arc<RenderBudget>,
    initial: std::collections::VecDeque<(ServerToClientMsg, ErrorContext)>,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("router".to_string())
        .spawn(worker_with_initial(
            os_input,
            send_client_instructions,
            render_budget,
            initial,
        ))
        .unwrap()
}

fn worker_with_initial(
    os_input: Box<dyn ClientOsApi>,
    send_client_instructions: SenderWithContext<ClientInstruction>,
    render_budget: Arc<RenderBudget>,
    initial: std::collections::VecDeque<(ServerToClientMsg, ErrorContext)>,
) -> impl FnOnce() + Send {
    // Capture before spawning: initial frames and the socket belong to this
    // generation even if the worker starts after a later connection is installed.
    let recovery = os_input.remote_recovery().map(|control| {
        let generation = control.stamp().0;
        (control, generation)
    });
    move || {
        route_with_initial(
            os_input,
            send_client_instructions,
            &render_budget,
            initial,
            recovery,
        )
    }
}

#[cfg(test)]
pub(crate) fn route(
    os_input: Box<dyn ClientOsApi>,
    send_client_instructions: SenderWithContext<ClientInstruction>,
    render_budget: &RenderBudget,
) {
    let recovery = os_input.remote_recovery().map(|control| {
        let generation = control.stamp().0;
        (control, generation)
    });
    route_with_initial(
        os_input,
        send_client_instructions,
        render_budget,
        Default::default(),
        recovery,
    );
}
fn route_with_initial(
    os_input: Box<dyn ClientOsApi>,
    send_client_instructions: SenderWithContext<ClientInstruction>,
    render_budget: &RenderBudget,
    mut initial: std::collections::VecDeque<(ServerToClientMsg, ErrorContext)>,
    recovery: Option<(crate::remote_recovery::RecoveryControl, u64)>,
) {
    let control = recovery.as_ref().map(|(control, _)| control);
    let generation = recovery.as_ref().map(|(_, generation)| *generation);
    let send_client_instructions = RouterSender {
        sender: send_client_instructions,
        generation,
    };
    let mut consecutive_malformed_messages_received = 0;
    let started_at = Instant::now();
    let mut waiting_for_first_message = true;
    let mut waiting_for_first_render = true;
    let mut slow_warnings = SlowWarnings::default();
    let mut budget_warnings = SlowWarnings::default();
    let mut drain_frames = 0;
    let mut drain_read_time = Duration::ZERO;
    log::info!(
        "attach trace: client pid={} router started; waiting for first server message",
        std::process::id()
    );
    loop {
        if let Some((control, generation)) = &recovery
            && !control.can_read(*generation)
        {
            break;
        }
        let draining = control.is_some_and(|control| !control.can_write());
        if let Some(generation) = generation
            && draining
            && (drain_frames >= MAX_DRAIN_FRAMES
                || drain_read_time >= crate::remote_recovery::READ_POLL)
        {
            send_client_instructions.reader_drained(generation);
            break;
        }
        let read_started = Instant::now();
        let received = initial
            .pop_front()
            .map(|(message, context)| IpcRecv::Msg(message, context))
            .unwrap_or_else(|| match generation {
                Some(generation) => os_input.recv_from_server_generation(generation),
                None => os_input.recv_from_server(),
            });
        if control.is_some_and(|control| !control.can_write()) {
            if matches!(&received, IpcRecv::Msg(..) | IpcRecv::Malformed) {
                drain_frames += 1;
            }
            if draining {
                drain_read_time += read_started.elapsed();
            }
        }
        match received {
            IpcRecv::Msg(instruction, err_ctx) => {
                consecutive_malformed_messages_received = 0;
                let tracing_first_message = waiting_for_first_message;
                let tracing_first_render = waiting_for_first_render
                    && matches!(&instruction, ServerToClientMsg::Render { .. });
                if waiting_for_first_message {
                    let kind = match &instruction {
                        ServerToClientMsg::Render { content } => {
                            format!("Render(bytes={})", content.len())
                        },
                        ServerToClientMsg::Connected { .. } => "Connected".to_owned(),
                        ServerToClientMsg::Exit { .. } => "Exit".to_owned(),
                        _ => "other".to_owned(),
                    };
                    log::info!(
                        "attach trace: client pid={} first server message={kind} after {:?}",
                        std::process::id(),
                        started_at.elapsed()
                    );
                    waiting_for_first_message = false;
                }
                if tracing_first_render && !tracing_first_message {
                    let bytes = match &instruction {
                        ServerToClientMsg::Render { content } => content.len(),
                        _ => 0,
                    };
                    log::info!(
                        "attach trace: client pid={} first Render arrived from server bytes={bytes} after {:?}",
                        std::process::id(),
                        started_at.elapsed()
                    );
                }
                if tracing_first_message || tracing_first_render {
                    log::info!(
                        "attach trace: client pid={} forwarding {} to client loop",
                        std::process::id(),
                        if tracing_first_render {
                            "first Render"
                        } else {
                            "first server message"
                        }
                    );
                }
                let (kind, render_bytes) = match &instruction {
                    ServerToClientMsg::Render { content } => ("Render", content.len()),
                    ServerToClientMsg::Exit { .. } => ("Exit", 0),
                    ServerToClientMsg::Connected { .. } => ("Connected", 0),
                    _ => ("other", 0),
                };
                if let ServerToClientMsg::Render { content } = &instruction {
                    // Stop reading while the terminal is behind; see `RenderBudget`.
                    let waiting_since = Instant::now();
                    if let Err(error) = render_budget.reserve(content.len()) {
                        // Other producers still own client-channel senders, so returning
                        // alone would leave the client loop waiting for another instruction.
                        send_client_instructions
                            .send(ClientInstruction::Error(format!(
                                "Could not reserve terminal render capacity: {error:#}"
                            )))
                            .to_anyhow()
                            .context("failed to report a render-budget failure")
                            .non_fatal();
                        Err::<(), _>(error)
                            .context("router stopped after a render-budget failure")
                            .non_fatal();
                        break;
                    }
                    let waited = waiting_since.elapsed();
                    if let Some((suppressed, max_suppressed)) =
                        budget_warnings.record(Instant::now(), waited)
                    {
                        log::warn!("output pressure: client pid={} stage=render_budget kind={kind} render_bytes={render_bytes} elapsed_ms={} suppressed={suppressed} max_suppressed_ms={}", std::process::id(), waited.as_millis(), max_suppressed.as_millis());
                    }
                }
                let started = Instant::now();
                let should_exit = forward(&send_client_instructions, instruction, err_ctx);
                let elapsed = started.elapsed();
                if let Some((suppressed, max_suppressed)) =
                    slow_warnings.record(Instant::now(), elapsed)
                {
                    log::warn!("output pressure: client pid={} stage=router_enqueue kind={kind} render_bytes={render_bytes} elapsed_ms={} suppressed={suppressed} max_suppressed_ms={}", std::process::id(), elapsed.as_millis(), max_suppressed.as_millis());
                }
                if tracing_first_message || tracing_first_render {
                    log::info!(
                        "attach trace: client pid={} {} reached client-loop queue after {:?}",
                        std::process::id(),
                        if tracing_first_render {
                            "first Render"
                        } else {
                            "first server message"
                        },
                        started_at.elapsed()
                    );
                }
                if tracing_first_render {
                    waiting_for_first_render = false;
                }
                if should_exit {
                    break;
                }
            },
            IpcRecv::Eof => {
                if waiting_for_first_message {
                    log::warn!(
                        "attach trace: client pid={} server connection closed before first message after {:?}",
                        std::process::id(),
                        started_at.elapsed()
                    );
                }
                // Terminal: every further read returns this immediately. Say so
                // once and leave, rather than spinning until a counter runs out
                // and then blaming an empty message.
                log::error!("The server closed the connection without sending Exit");
                if let (Some(control), Some(generation)) = (&control, generation) {
                    control.lost(generation, SERVER_HUNG_UP.to_owned());
                    send_client_instructions.reader_drained(generation);
                    break;
                }
                send_client_instructions
                    .send(ClientInstruction::Error(SERVER_HUNG_UP.to_string()))
                    .to_anyhow()
                    .context("failed to report that the server hung up")
                    .non_fatal();
                break;
            },
            IpcRecv::Malformed => {
                consecutive_malformed_messages_received += 1;
                // Unblock the input thread so it doesn't sit waiting on a
                // reply that is never going to arrive.
                if send_client_instructions
                    .send_required(ClientInstruction::UnblockInputThread)
                    .is_err()
                {
                    break;
                }
                if consecutive_malformed_messages_received == 1 {
                    log::error!("Received an unreadable message from the server");
                }
                if consecutive_malformed_messages_received >= MAX_CONSECUTIVE_MALFORMED_MESSAGES {
                    log::error!(
                        "Received {} unreadable messages in a row from the server, giving up",
                        consecutive_malformed_messages_received
                    );
                    send_client_instructions
                        .send(ClientInstruction::Error(
                            "The connection to the yinz server desynchronised.".to_string(),
                        ))
                        .to_anyhow()
                        .context("failed to report a desynchronised server connection")
                        .non_fatal();
                    break;
                }
                thread::sleep(MALFORMED_MESSAGE_BACKOFF);
            },
            IpcRecv::Idle => {
                // Writes stop immediately on failure, but explicit detach/handoff
                // frames may still be pending. EOF/idle finish draining early;
                // otherwise the transport budget bounds a continuously busy peer.
                if let Some((control, generation)) = &recovery
                    && !control.can_write()
                {
                    send_client_instructions.reader_drained(*generation);
                    break;
                }
            },
        }
    }
    if let Some((suppressed, max_suppressed)) = slow_warnings.finish() {
        log::warn!("output pressure: client pid={} stage=router_enqueue final suppressed={suppressed} max_suppressed_ms={}", std::process::id(), max_suppressed.as_millis());
    }
    if let Some((suppressed, max_suppressed)) = budget_warnings.finish() {
        log::warn!("output pressure: client pid={} stage=render_budget final suppressed={suppressed} max_suppressed_ms={}", std::process::id(), max_suppressed.as_millis());
    }
}

struct RouterSender {
    sender: SenderWithContext<ClientInstruction>,
    generation: Option<u64>,
}
impl RouterSender {
    fn reader_drained(&self, generation: u64) {
        self.send_required(ClientInstruction::ServerReaderDrained(generation))
            .to_anyhow()
            .context("failed to report completion of server message draining")
            .non_fatal();
    }
    fn tagged(&self, instruction: ClientInstruction) -> ClientInstruction {
        match self.generation {
            Some(generation) => ClientInstruction::FromServer(generation, Box::new(instruction)),
            None => instruction,
        }
    }
    fn send(
        &self,
        instruction: ClientInstruction,
    ) -> Result<(), yinz_runtime::channels::SendError<(ClientInstruction, ErrorContext)>> {
        self.sender.send(self.tagged(instruction))
    }
    fn send_required(
        &self,
        instruction: ClientInstruction,
    ) -> Result<(), yinz_runtime::channels::SendError<(ClientInstruction, ErrorContext)>> {
        self.sender.send_required(self.tagged(instruction))
    }
}

#[cfg(test)]
#[path = "unit/router_recovery_tests.rs"]
mod recovery_tests;

/// Forward one message to the main loop. Returns whether this was the last
/// one — `Exit` is still delivered before the thread stops — or the main loop
/// has already exited, say after failing to write to the terminal.
fn forward(
    send_client_instructions: &RouterSender,
    instruction: ServerToClientMsg,
    err_ctx: ErrorContext,
) -> bool {
    yinz_runtime::errors::update_thread_ctx(&err_ctx);
    let is_exit = matches!(
        instruction,
        ServerToClientMsg::Exit { .. } | ServerToClientMsg::SwitchSession { .. }
    );
    // A disconnected receiver is the normal way shutdown stops the router.
    send_client_instructions
        .send_required(instruction.into())
        .is_err()
        || is_exit
}

#[cfg(test)]
#[path = "unit/router_slow_warning_tests.rs"]
mod slow_warning_tests;
