//! The thread that turns server IPC messages into client instructions.
//!
//! It is the only place that calls `recv_from_server`, and it does nothing
//! with what it receives beyond translating and forwarding it, so that the
//! main loop never blocks on the socket.

use crate::os_input_output::ClientOsApi;
use crate::ClientInstruction;
use std::thread;
use std::time::{Duration, Instant};
use zellij_utils::{
    channels::SenderWithContext, errors::ErrorContext, ipc::IpcRecv, ipc::ServerToClientMsg,
};

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

/// What the user is told when the server's socket closes without an `Exit`.
/// The server sends one on every shutdown path, so reaching this means it went
/// away without saying goodbye.
const SERVER_HUNG_UP: &str =
    "The zellij server exited without notifying this client.\nRun `zellij ls` to see whether the session is still there.";

/// Spawn the router thread. It runs until the server says `Exit` or the
/// socket gives up, and the caller joins it during shutdown.
pub(crate) fn spawn(
    os_input: Box<dyn ClientOsApi>,
    send_client_instructions: SenderWithContext<ClientInstruction>,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("router".to_string())
        .spawn(move || route(os_input, send_client_instructions))
        .unwrap()
}

pub(crate) fn route(
    os_input: Box<dyn ClientOsApi>,
    send_client_instructions: SenderWithContext<ClientInstruction>,
) {
    let mut consecutive_malformed_messages_received = 0;
    let started_at = Instant::now();
    let mut waiting_for_first_message = true;
    let mut waiting_for_first_render = true;
    log::info!(
        "attach trace: client pid={} router started; waiting for first server message",
        std::process::id()
    );
    loop {
        match os_input.recv_from_server() {
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
                        ServerToClientMsg::Connected => "Connected".to_owned(),
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
                let should_exit = forward(&send_client_instructions, instruction, err_ctx);
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
                send_client_instructions
                    .send(ClientInstruction::Error(SERVER_HUNG_UP.to_string()))
                    .unwrap();
                break;
            },
            IpcRecv::Malformed => {
                consecutive_malformed_messages_received += 1;
                // Unblock the input thread so it doesn't sit waiting on a
                // reply that is never going to arrive.
                send_client_instructions
                    .send(ClientInstruction::UnblockInputThread)
                    .unwrap();
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
                            "The connection to the zellij server desynchronised.".to_string(),
                        ))
                        .unwrap();
                    break;
                }
                thread::sleep(MALFORMED_MESSAGE_BACKOFF);
            },
            IpcRecv::Idle => {},
        }
    }
}

/// Forward one message to the main loop. Returns whether this was the last
/// one — `Exit` is still delivered before the thread stops.
fn forward(
    send_client_instructions: &SenderWithContext<ClientInstruction>,
    instruction: ServerToClientMsg,
    err_ctx: ErrorContext,
) -> bool {
    err_ctx.update_thread_ctx();
    let is_exit = matches!(instruction, ServerToClientMsg::Exit { .. });
    send_client_instructions.send(instruction.into()).unwrap();
    is_exit
}
