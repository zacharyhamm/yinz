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
    let outcome = run(&os, &budget, deadline, &rx, input_tx);
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
    assert_eq!(os.stdout_bytes(), expected);
    // Both slots must be free after all three frames reached the terminal.
    budget.reserve(100).unwrap();
    budget.release(100).unwrap();
}

#[test]
fn controls_and_exit_do_not_wait_for_the_splash_or_write_deferred_frames() {
    let os = FakeClientOsApi::new().capturing_stdout();
    let budget = RenderBudget::new(100);
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
        Some(Instant::now() + Duration::from_secs(3600)),
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
    budget: &RenderBudget,
    splash_deadline: Option<Instant>,
    rx: &channels::Receiver<(ClientInstruction, yinz_types::diagnostics::ErrorContext)>,
    input_tx: channels::Sender<(InputInstruction, yinz_types::diagnostics::ErrorContext)>,
) -> crate::ClientOutcome {
    let nested = NestedReannounce::spawn(Box::new(FakeClientOsApi::new()), "splash-test".into());
    nested.stop();
    ClientLoop {
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
        splash_deadline,
    }
    .run(rx)
}
