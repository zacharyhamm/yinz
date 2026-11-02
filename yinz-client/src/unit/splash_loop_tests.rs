use crate::client_loop::ClientLoop;
use crate::command_is_executing::CommandIsExecuting;
use crate::nested_reannounce::NestedReannounce;
use crate::render_budget::RenderBudget;
use crate::stdin_ansi_parser::{StdinAnsiParser, SyncOutput};
use crate::terminal_escapes::ClientTeardown;
use crate::test_support::FakeClientOsApi;
use crate::{ClientInstruction, InputInstruction};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use yinz_ipc::ipc::ExitReason;
use yinz_runtime::channels::{self, SenderWithContext};

#[test]
fn early_frames_keep_their_budget_and_replay_in_order_after_the_deadline() {
    check_frames(Some(Instant::now() + Duration::from_millis(200)));
}

#[test]
fn late_frames_clear_once_and_sessions_without_a_splash_do_not_clear() {
    check_frames(Some(Instant::now() - Duration::from_secs(1)));
    check_frames(None);
}

fn check_frames(deadline: Option<Instant>) {
    let os = FakeClientOsApi::new().capturing_stdout();
    let budget = Arc::new(RenderBudget::new(100));
    let (tx, rx) = channels::unbounded();
    let tx = SenderWithContext::new(tx);
    for frame in ["first", "second"] {
        budget.reserve(frame.len()).unwrap();
        tx.send(ClientInstruction::Render(frame.into())).unwrap();
    }
    let (input_tx, input_rx): channels::ChannelWithContext<InputInstruction> =
        channels::unbounded();
    let producer = std::thread::spawn({
        let budget = budget.clone();
        move || {
            // Both slots are reserved; the third cannot fit until a frame is written.
            budget.reserve(5).unwrap();
            if let Some(deadline) = deadline {
                assert!(Instant::now() >= deadline);
            }
            tx.send(ClientInstruction::Render("third".into())).unwrap();
            tx.send(ClientInstruction::SetMouseMode(true)).unwrap();
            assert!(matches!(
                input_rx.recv_timeout(Duration::from_secs(30)).unwrap().0,
                InputInstruction::SetMouseMode(true)
            ));
            tx.send(ClientInstruction::Exit(ExitReason::Normal))
                .unwrap();
        }
    });
    let splash =
        deadline.map(|deadline| crate::splash::test_splash(deadline - Duration::from_millis(1000)));
    let outcome = run(&os, &budget, splash, &rx, input_tx);
    producer.join().unwrap();
    assert_eq!(outcome.exit_status, 0);
    let sync = SyncOutput::Csi;
    let mut expected = Vec::new();
    if deadline.is_some() {
        expected.extend(
            [
                sync.start_seq(),
                crate::splash::CLEAR.as_bytes(),
                sync.end_seq(),
            ]
            .concat(),
        );
    }
    for frame in ["first", "second", "third"] {
        expected.extend([sync.start_seq(), frame.as_bytes(), sync.end_seq()].concat());
    }
    let bytes = os.stdout_bytes();
    assert!(bytes.ends_with(&expected));
    let text = String::from_utf8(bytes).unwrap();
    assert_eq!(text.matches("first").count(), 1);
    assert_eq!(text.matches("second").count(), 1);
    assert_eq!(text.matches("third").count(), 1);
    if deadline.is_none() {
        assert_eq!(text.as_bytes(), expected);
    } else {
        assert_eq!(
            os.stdout_attempts
                .get()
                .iter()
                .filter(|write| write.as_slice() == crate::splash::CLEAR.as_bytes())
                .count(),
            1
        );
    }
    // Both slots must be free after all three frames reached the terminal.
    budget.reserve(100).unwrap();
    budget.release(100).unwrap();
}

#[test]
fn expiry_clears_without_a_server_render_and_does_not_clear_again_for_a_late_frame() {
    let os = FakeClientOsApi::new().capturing_stdout();
    let budget = Arc::new(RenderBudget::new(100));
    let (tx, rx) = channels::unbounded();
    let tx = SenderWithContext::new(tx);
    let (input_tx, input_rx): channels::ChannelWithContext<InputInstruction> =
        channels::unbounded();
    let started = Instant::now();
    let producer = std::thread::spawn({
        let os = os.clone();
        let budget = budget.clone();
        move || {
            os.stdout_attempts.wait_until(
                "splash clear before any server render",
                yinz_test_support::default_timeout(),
                |writes| {
                    writes
                        .iter()
                        .any(|write| write == crate::splash::CLEAR.as_bytes())
                        .then_some(())
                },
            );
            assert!(Instant::now() >= started + Duration::from_millis(1000));
            // A control round trip confirms the clear finished and the loop is
            // still responsive while the session's first render is pending.
            tx.send(ClientInstruction::SetMouseMode(true)).unwrap();
            assert!(matches!(
                input_rx.recv_timeout(Duration::from_secs(30)).unwrap().0,
                InputInstruction::SetMouseMode(true)
            ));
            budget.reserve(4).unwrap();
            tx.send(ClientInstruction::Render("late".into())).unwrap();
            tx.send(ClientInstruction::SetMouseMode(false)).unwrap();
            assert!(matches!(
                input_rx.recv_timeout(Duration::from_secs(30)).unwrap().0,
                InputInstruction::SetMouseMode(false)
            ));
            tx.send(ClientInstruction::Exit(ExitReason::Normal))
                .unwrap();
        }
    });
    let outcome = run(
        &os,
        &budget,
        Some(crate::splash::test_splash(started)),
        &rx,
        input_tx,
    );
    producer.join().unwrap();
    assert_eq!(outcome.exit_status, 0);
    let sync = SyncOutput::Csi;
    assert!(os.stdout_bytes().ends_with(
        &[
            sync.start_seq(),
            crate::splash::CLEAR.as_bytes(),
            sync.end_seq(),
            sync.start_seq(),
            b"late",
            sync.end_seq(),
        ]
        .concat()
    ));
    assert_eq!(
        os.stdout_attempts
            .get()
            .iter()
            .filter(|write| write.as_slice() == crate::splash::CLEAR.as_bytes())
            .count(),
        1
    );
    budget.reserve(100).unwrap();
    budget.release(100).unwrap();
}

#[test]
fn animation_write_failure_returns_a_client_error() {
    let os = FakeClientOsApi::new().with_stdout_error(std::io::ErrorKind::BrokenPipe);
    let budget = Arc::new(RenderBudget::new(100));
    let (tx, rx) = channels::unbounded();
    let (input_tx, _) = channels::unbounded();
    // Keep the receiver connected while it waits for the timed redraw.
    let outcome = run(
        &os,
        &budget,
        Some(crate::splash::test_splash(Instant::now())),
        &rx,
        input_tx,
    );
    drop(tx);
    assert_eq!(outcome.exit_status, 1);
    assert!(outcome.exit_msg.contains("Could not write to the terminal"));
    budget.reserve(100).unwrap();
    budget.release(100).unwrap();
}

#[test]
fn controls_and_exit_do_not_wait_for_the_splash_or_write_deferred_frames() {
    let os = FakeClientOsApi::new().capturing_stdout();
    let budget = Arc::new(RenderBudget::new(100));
    let (tx, rx) = channels::unbounded();
    let tx = SenderWithContext::new(tx);
    budget.reserve(5).unwrap();
    tx.send(ClientInstruction::Render("first".into())).unwrap();
    tx.send(ClientInstruction::SetMouseMode(true)).unwrap();
    tx.send(ClientInstruction::Exit(ExitReason::Normal))
        .unwrap();
    let (input_tx, input_rx) = channels::unbounded();
    let outcome = run(
        &os,
        &budget,
        Some(crate::splash::test_splash(
            Instant::now() + Duration::from_secs(3600),
        )),
        &rx,
        input_tx,
    );
    assert_eq!(outcome.exit_status, 0);
    assert!(matches!(
        input_rx.try_recv().unwrap().0,
        InputInstruction::SetMouseMode(true)
    ));
    assert!(os.stdout_bytes().is_empty());
    budget.reserve(100).unwrap();
    budget.release(100).unwrap();
}

fn run(
    os: &FakeClientOsApi,
    budget: &Arc<RenderBudget>,
    splash: Option<crate::splash::Splash>,
    rx: &channels::Receiver<(ClientInstruction, yinz_types::diagnostics::ErrorContext)>,
    input_tx: channels::Sender<(InputInstruction, yinz_types::diagnostics::ErrorContext)>,
) -> crate::ClientOutcome {
    run_with_saver(os, budget, splash, rx, input_tx, None)
}

fn run_with_saver(
    os: &FakeClientOsApi,
    budget: &Arc<RenderBudget>,
    splash: Option<crate::splash::Splash>,
    rx: &channels::Receiver<(ClientInstruction, yinz_types::diagnostics::ErrorContext)>,
    input_tx: channels::Sender<(InputInstruction, yinz_types::diagnostics::ErrorContext)>,
    screensaver: Option<crate::screensaver::Screensaver>,
) -> crate::ClientOutcome {
    let nested = NestedReannounce::spawn(Box::new(FakeClientOsApi::new()), "splash-test".into());
    nested.stop();
    ClientLoop {
        signal_quit_supported: Default::default(),
        recovery: &mut None,
        send_client_instructions: None,
        sessions: &mut crate::session_switch::Sessions::new(
            None,
            "test".into(),
            &Default::default(),
            &mut Default::default(),
        ),
        remote_host: None,
        os_input: os,
        command_is_executing: &mut CommandIsExecuting::new(),
        stdin_ansi_parser: &Arc::new(Mutex::new(StdinAnsiParser::new())),
        send_input_instructions: &SenderWithContext::new(input_tx),
        nested_reannounce: &nested,
        teardown: ClientTeardown::new(24, false),
        synchronised_output: Some(SyncOutput::Csi),
        render_budget: budget,
        startup_started: Instant::now(),
        splash,
        screensaver,
    }
    .run(rx)
}

#[test]
fn screensaver_drains_background_frames_and_restores_on_wake() {
    let os = FakeClientOsApi::new().capturing_stdout();
    let budget = Arc::new(RenderBudget::new(100));
    let saver = crate::screensaver::Screensaver::new(
        &Default::default(),
        &yinz_types::input::options::Options {
            screensaver_idle_timeout: Some(1),
            ..Default::default()
        },
    )
    .unwrap();
    let activity = saver.activity.clone();
    let (tx, rx) = channels::unbounded();
    let tx = SenderWithContext::new(tx);
    let (input_tx, _input_rx) = channels::unbounded();
    budget.reserve(7).unwrap();
    tx.send(ClientInstruction::Render("initial".into()))
        .unwrap();
    let producer = std::thread::spawn({
        let os = os.clone();
        let budget = budget.clone();
        move || {
            os.stdout_attempts.wait_until(
                "idle animation appears",
                yinz_test_support::default_timeout(),
                |writes| {
                    writes
                        .iter()
                        .any(|write| write.windows(6).any(|bytes| bytes == b"\x1b[?25l"))
                        .then_some(())
                },
            );
            let notification = "\x1b]9;background complete\x07";
            tx.send(ClientInstruction::from(
                yinz_ipc::ipc::ServerToClientMsg::DesktopNotification {
                    content: notification.into(),
                },
            ))
            .unwrap();
            os.stdout_attempts.wait_until(
                "desktop notification during screensaver",
                yinz_test_support::default_timeout(),
                |writes| {
                    writes
                        .iter()
                        .any(|write| write == notification.as_bytes())
                        .then_some(())
                },
            );
            let clipboard = "\x1b]52;c;Y29weQ==\x07";
            tx.send(ClientInstruction::from(
                yinz_ipc::ipc::ServerToClientMsg::ClipboardUpdate {
                    content: clipboard.into(),
                },
            ))
            .unwrap();
            os.stdout_attempts.wait_until(
                "clipboard update during screensaver",
                yinz_test_support::default_timeout(),
                |writes| {
                    writes
                        .iter()
                        .any(|write| write == clipboard.as_bytes())
                        .then_some(())
                },
            );
            // Each hidden frame fills the entire budget. The next reserve can finish only if
            // the loop drains and releases the preceding frame while the screensaver is up.
            for _ in 0..5 {
                budget.reserve(100).unwrap();
                tx.send(ClientInstruction::Render("H".repeat(100))).unwrap();
            }
            budget.reserve(100).unwrap();
            budget.release(100).unwrap();
            assert!(crate::screensaver::record_input(
                &activity,
                &InputInstruction::KeyEvent(
                    yinz_key_codec::vendored::termwiz::input::InputEvent::Paste("wake".into()),
                    vec![]
                ),
                Instant::now()
            )
            .unwrap());
            tx.send(ClientInstruction::UserActivity).unwrap();
            os.sent_to_server.wait_until(
                "wake requests repaint",
                yinz_test_support::default_timeout(),
                |messages| {
                    messages
                        .contains(&yinz_ipc::ipc::ClientToServerMsg::RequestRepaint)
                        .then_some(())
                },
            );
            budget.reserve(8).unwrap();
            tx.send(ClientInstruction::Render("restored".into()))
                .unwrap();
            tx.send(ClientInstruction::RepaintComplete).unwrap();
            tx.send(ClientInstruction::Exit(ExitReason::Normal))
                .unwrap();
        }
    });
    let outcome = run_with_saver(&os, &budget, None, &rx, input_tx, Some(saver));
    producer.join().unwrap();
    assert_eq!(outcome.exit_status, 0);
    let output = String::from_utf8(os.stdout_bytes()).unwrap();
    assert!(output.find("initial").unwrap() < output.find("\x1b[?25l").unwrap());
    assert!(output.contains("restored"));
    assert_eq!(output.matches("\x1b]52;c;Y29weQ==\x07").count(), 1);
    assert!(!output.contains(&"H".repeat(100)));
    budget.reserve(100).unwrap();
    budget.release(100).unwrap();
}
