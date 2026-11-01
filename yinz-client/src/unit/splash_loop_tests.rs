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
        deadline.map(|deadline| crate::splash::test_splash(deadline - Duration::from_secs(3)));
    let outcome = run(&os, &budget, splash, &rx, input_tx);
    producer.join().unwrap();
    assert_eq!(outcome.exit_status, 0);
    let clear = if deadline.is_some() {
        crate::splash::CLEAR
    } else {
        ""
    };
    let sync = SyncOutput::Csi;
    let expected = [format!("{clear}first"), "second".into(), "third".into()]
        .into_iter()
        .flat_map(|frame| [sync.start_seq(), frame.as_bytes(), sync.end_seq()].concat())
        .collect::<Vec<_>>();
    let bytes = os.stdout_bytes();
    assert!(bytes.ends_with(&expected));
    let text = String::from_utf8(bytes).unwrap();
    assert_eq!(text.matches("first").count(), 1);
    assert_eq!(text.matches("second").count(), 1);
    assert_eq!(text.matches("third").count(), 1);
    if deadline.is_none() {
        assert_eq!(text.as_bytes(), expected);
    } else {
        // At least one animation frame was painted before handing off. The number
        // depends on scheduling, so only assert visible Braille content.
        let before_session = text.split("first").next().unwrap();
        assert!(before_session
            .chars()
            .any(|c| ('\u{2801}'..='\u{28ff}').contains(&c)));
    }
    // Both slots must be free after all three frames reached the terminal.
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
    let nested = NestedReannounce::spawn(Box::new(FakeClientOsApi::new()), "splash-test".into());
    nested.stop();
    ClientLoop {
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
    }
    .run(rx)
}
