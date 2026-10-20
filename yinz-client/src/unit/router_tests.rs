//! What the router does when the socket stops producing messages.
//!
//! The distinction under test is the one that produced "Received empty unknown
//! from server": a server that exited closed its socket, and the router treated
//! that identically to a corrupt frame — retrying it a thousand times with no
//! delay and then reporting an empty message rather than a departed server.

use super::super::router::route;
use crate::render_budget::{RenderBudget, RENDER_BUDGET_BYTES};
use crate::test_support::FakeClientOsApi;
use crate::ClientInstruction;
use yinz_runtime::channels::{self, SenderWithContext, Shutdown};
use {yinz_ipc::ipc::ExitReason, yinz_ipc::ipc::ServerToClientMsg};

fn run_router(os_input: FakeClientOsApi) -> Vec<ClientInstruction> {
    let (sender, receiver) = channels::unbounded();
    let sender = SenderWithContext::new(sender);
    let router = std::thread::spawn(move || {
        let render_budget = RenderBudget::new(RENDER_BUDGET_BYTES);
        route(Box::new(os_input), sender, &render_budget)
    });
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
        errors[0].contains("connection to the yinz server closed"),
        "the error should name the closed server connection, got: {}",
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

#[test]
fn a_router_blocked_on_a_full_channel_stops_once_the_main_loop_is_gone() {
    // The main loop has exited, after failing to write to the terminal, and the server
    // keeps sending frames. With nobody draining the channel, the router blocks on the
    // first send that does not fit; dropping the receiver is what lets it go.
    for malformed in [false, true] {
        let os_input = FakeClientOsApi::default();
        for _ in 0..5 {
            if malformed {
                os_input.queue_malformed_from_server();
            } else {
                os_input.queue_from_server(ServerToClientMsg::Render {
                    content: "frame".to_string(),
                });
            }
        }
        let (sender, receiver) = channels::bounded(1);
        let shutdown = Shutdown::default();
        let sender = shutdown.sender(sender);
        let (finished, router_finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let render_budget = RenderBudget::new(RENDER_BUDGET_BYTES);
            route(Box::new(os_input), sender, &render_budget);
            let _ = finished.send(());
        });
        // Nothing else stops the router: the fake server never hangs up.
        assert!(router_finished
            .recv_timeout(std::time::Duration::from_millis(200))
            .is_err());
        // Production begins shutdown before dropping the receiver.
        shutdown.begin();
        drop(receiver);
        router_finished
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("the router should stop once nobody reads its channel");
    }
}

#[test]
fn the_client_loop_releases_render_pressure_on_delivery_and_terminal_failure() {
    use crate::client_loop::ClientLoop;
    use crate::command_is_executing::CommandIsExecuting;
    use crate::nested_reannounce::NestedReannounce;
    use crate::stdin_ansi_parser::StdinAnsiParser;
    use crate::terminal_escapes::ClientTeardown;
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::Duration;
    use yinz_ipc::ipc::ClientToServerMsg;
    use yinz_types::internal_ids::SessionName;

    const CHILD: &str = "YINZ_TEST_QUITTING_RENDER_PRESSURE";
    let quitting = std::env::var_os(CHILD).is_some();
    if quitting {
        crate::terminal_escapes::abandon_stalled_output();
    } else {
        // Cancellation is process-global; isolate it from other tests under libtest too.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "unit::router_tests::the_client_loop_releases_render_pressure_on_delivery_and_terminal_failure",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
    for error in [
        None,
        Some(std::io::ErrorKind::BrokenPipe),
        Some(std::io::ErrorKind::TimedOut),
    ] {
        let mut os_input = FakeClientOsApi::default().capturing_stdout();
        if let Some(error) = error {
            os_input = os_input.with_stdout_error(error);
        }
        for content in ["first", "second"] {
            os_input.queue_from_server(ServerToClientMsg::Render {
                content: content.into(),
            });
        }
        os_input.queue_from_server(ServerToClientMsg::Exit {
            exit_reason: ExitReason::Normal,
        });
        let observed = os_input.clone();
        let (finished, completion) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let (sender, receiver) = channels::bounded(1);
            let shutdown = Shutdown::default();
            // The second frame must wait until the first is delivered; it is also
            // larger than the budget, exercising the oversized-frame escape hatch.
            let budget = Arc::new(RenderBudget::new(5));
            let router = crate::router::spawn(
                Box::new(os_input.clone()),
                shutdown.sender(sender),
                budget.clone(),
            );
            let nested_reannounce = NestedReannounce::spawn(
                Box::new(FakeClientOsApi::default()),
                SessionName::from("render-pressure-test"),
            );
            nested_reannounce.stop();
            let (input_sender, _input_receiver) = channels::unbounded();
            let outcome = ClientLoop {
                os_input: &os_input,
                command_is_executing: &mut CommandIsExecuting::new(),
                stdin_ansi_parser: &Arc::new(Mutex::new(StdinAnsiParser::new())),
                send_input_instructions: &SenderWithContext::new(input_sender),
                nested_reannounce: &nested_reannounce,
                teardown: ClientTeardown::new(24, false),
                synchronised_output: None,
                render_budget: &budget,
                startup_started: std::time::Instant::now(),
                splash_deadline: None,
            }
            .run(&receiver);
            // Mirror start_client's shutdown: neither a reservation nor a send may
            // keep its router alive once the terminal loop has left.
            budget.close().unwrap();
            shutdown.begin();
            drop(receiver);
            router.join().unwrap();
            finished.send(outcome).unwrap();
        });
        let outcome = completion
            .recv_timeout(Duration::from_secs(30))
            .expect("the terminal loop and router must both finish");
        worker.join().unwrap();
        assert!(observed
            .sent_messages()
            .iter()
            .any(|message| matches!(message, ClientToServerMsg::ClientExited)));
        if error.is_some() && !(quitting && error == Some(std::io::ErrorKind::TimedOut)) {
            assert_eq!(outcome.exit_status, 1);
            assert!(
                outcome.exit_msg.contains("Could not write to the terminal"),
                "{}",
                outcome.exit_msg
            );
            assert!(observed.stdout_bytes().is_empty());
        } else {
            assert_eq!(outcome.exit_status, 0);
            assert_eq!(outcome.exit_msg, ExitReason::Normal.to_string());
            let expected = if error.is_some() {
                &b""[..]
            } else {
                &b"firstsecond"[..]
            };
            assert_eq!(observed.stdout_bytes(), expected);
        }
    }
}

#[test]
fn late_terminal_replies_do_not_panic_after_the_client_loop_exits() {
    use crate::command_is_executing::CommandIsExecuting;
    use crate::input_handler::input_loop;
    use crate::nested_reannounce::NestedReannounce;
    use crate::stdin_ansi_parser::{AnsiStdinInstruction, SyncOutput};
    use crate::InputInstruction;
    use yinz_ipc::nested_session::{encode_payload, NestedSessionMessage};
    use yinz_types::internal_ids::SessionName;

    for late_reply in [
        InputInstruction::AnsiStdinInstructions(vec![AnsiStdinInstruction::SynchronizedOutput(
            Some(SyncOutput::Csi),
        )]),
        InputInstruction::NestedSessionFrameFromHost(encode_payload(&NestedSessionMessage::Ping)),
    ] {
        let (client_sender, client_receiver) = channels::unbounded();
        // start_client drops this before telling the input thread to exit. Host
        // replies already queued for the input thread can still arrive first.
        drop(client_receiver);
        let (input_sender, input_receiver) = channels::unbounded();
        let input_sender = SenderWithContext::new(input_sender);
        input_sender.send(late_reply).unwrap();
        input_sender.send(InputInstruction::Exit).unwrap();
        let nested_reannounce = NestedReannounce::spawn(
            Box::new(FakeClientOsApi::default()),
            SessionName::from("late-terminal-reply"),
        );
        nested_reannounce.stop();
        input_loop(
            Box::new(FakeClientOsApi::default()),
            Default::default(),
            CommandIsExecuting::new(),
            SenderWithContext::new(client_sender),
            input_receiver,
            nested_reannounce,
        );
    }
}
