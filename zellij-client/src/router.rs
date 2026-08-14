//! The thread that turns server IPC messages into client instructions.
//!
//! It is the only place that calls `recv_from_server`, and it does nothing
//! with what it receives beyond translating and forwarding it, so that the
//! main loop never blocks on the socket.

use crate::os_input_output::ClientOsApi;
use crate::ClientInstruction;
use std::thread;
use zellij_utils::{channels::SenderWithContext, errors::ErrorContext, ipc::ServerToClientMsg};

/// How many unreadable messages in a row we tolerate before concluding the
/// socket is not going to recover and tearing the client down. A single
/// failure is survivable — a run of them means the server is gone or the
/// stream has desynchronised.
const MAX_CONSECUTIVE_UNKNOWN_MESSAGES: usize = 1000;

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

fn route(
    os_input: Box<dyn ClientOsApi>,
    send_client_instructions: SenderWithContext<ClientInstruction>,
) {
    let mut consecutive_unknown_messages_received = 0;
    loop {
        match os_input.recv_from_server() {
            Some((instruction, err_ctx)) => {
                consecutive_unknown_messages_received = 0;
                if forward(&send_client_instructions, instruction, err_ctx) {
                    break;
                }
            },
            None => {
                consecutive_unknown_messages_received += 1;
                // Unblock the input thread so it doesn't sit waiting on a
                // reply that is never going to arrive.
                send_client_instructions
                    .send(ClientInstruction::UnblockInputThread)
                    .unwrap();
                log::error!("Received unknown message from server");
                if consecutive_unknown_messages_received >= MAX_CONSECUTIVE_UNKNOWN_MESSAGES {
                    send_client_instructions
                        .send(ClientInstruction::Error(
                            "Received empty unknown from server".to_string(),
                        ))
                        .unwrap();
                    break;
                }
            },
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
