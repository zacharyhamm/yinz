//! What the router does when the socket stops producing messages.
//!
//! The distinction under test is the one that produced "Received empty unknown
//! from server": a server that exited closed its socket, and the router treated
//! that identically to a corrupt frame — retrying it a thousand times with no
//! delay and then reporting an empty message rather than a departed server.

use super::super::router::route;
use crate::test_support::FakeClientOsApi;
use crate::ClientInstruction;
use yinz_runtime::channels::{self, SenderWithContext};
use {yinz_ipc::ipc::ExitReason, yinz_ipc::ipc::ServerToClientMsg};

fn run_router(os_input: FakeClientOsApi) -> Vec<ClientInstruction> {
    let (sender, receiver) = channels::unbounded();
    let sender = SenderWithContext::new(sender);
    let router = std::thread::spawn(move || route(Box::new(os_input), sender));
    router.join().expect("router thread panicked");
    // The sender is dropped with the router thread, so this drains and stops.
    receiver
        .iter()
        .map(|(instruction, _)| instruction)
        .collect()
}

#[test]
fn a_server_hangup_is_reported_once_and_stops_the_router() {
    let os_input = FakeClientOsApi::default();
    os_input.queue_server_hangup();

    let instructions = run_router(os_input);

    let errors: Vec<&String> = instructions
        .iter()
        .filter_map(|instruction| match instruction {
            ClientInstruction::Error(message) => Some(message),
            _ => None,
        })
        .collect();
    assert_eq!(
        errors.len(),
        1,
        "a hangup should produce exactly one error, got: {instructions:?}"
    );
    assert!(
        errors[0].contains("server exited"),
        "the error should name the departed server, got: {}",
        errors[0]
    );
    assert!(
        !instructions
            .iter()
            .any(|i| matches!(i, ClientInstruction::UnblockInputThread)),
        "a hangup unblocks nothing - there is no reply coming: {instructions:?}"
    );
}

#[test]
fn a_hangup_does_not_spin_through_the_malformed_budget() {
    let os_input = FakeClientOsApi::default();
    // More hangups than the router would ever consume if it stopped at the
    // first one, and far fewer than the malformed budget of 1000.
    for _ in 0..10 {
        os_input.queue_server_hangup();
    }

    let instructions = run_router(os_input);

    assert_eq!(
        instructions.len(),
        1,
        "the router should act on the first hangup and leave: {instructions:?}"
    );
}

#[test]
fn an_exit_still_reaches_the_main_loop() {
    let os_input = FakeClientOsApi::default();
    os_input.queue_from_server(ServerToClientMsg::Exit {
        exit_reason: ExitReason::Normal,
    });
    // Never read: the router stops at the Exit above.
    os_input.queue_server_hangup();

    let instructions = run_router(os_input);

    assert!(
        matches!(
            instructions.as_slice(),
            [ClientInstruction::Exit(ExitReason::Normal)]
        ),
        "a normal exit should be forwarded and nothing else: {instructions:?}"
    );
}

#[test]
fn malformed_frames_are_survivable() {
    let os_input = FakeClientOsApi::default();
    os_input.queue_malformed_from_server();
    os_input.queue_from_server(ServerToClientMsg::Exit {
        exit_reason: ExitReason::Normal,
    });

    let instructions = run_router(os_input);

    assert!(
        matches!(
            instructions.as_slice(),
            [
                ClientInstruction::UnblockInputThread,
                ClientInstruction::Exit(ExitReason::Normal)
            ]
        ),
        "a single bad frame should unblock the input thread and then carry on: {instructions:?}"
    );
}
