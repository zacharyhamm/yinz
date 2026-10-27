use super::*;
use crate::{
    command_is_executing::CommandIsExecuting, os_input_output::ServerConnection,
    remote_recovery::RecoveryControl, test_support::FakeClientOsApi,
};
use yinz_ipc::ipc::ExitReason;
use yinz_runtime::channels;

#[test]
fn continuously_ready_frames_cannot_prevent_recovery_draining_from_finishing() {
    let os = FakeClientOsApi::new();
    let mut recovery =
        crate::remote_recovery::Recovery::new("host".into(), CommandIsExecuting::new());
    os.set_remote_recovery(Some(recovery.control.clone()))
        .unwrap();
    for _ in 0..MAX_DRAIN_FRAMES + 1 {
        os.queue_from_server(ServerToClientMsg::UnblockInputThread);
    }
    recovery.control.lost(0, "write failed".into());
    let (tx, rx) = channels::unbounded();
    route(
        Box::new(os.clone()),
        SenderWithContext::new(tx),
        &RenderBudget::new(100),
    );
    let instructions: Vec<_> = rx.iter().map(|(instruction, _)| instruction).collect();
    assert!(instructions.len() <= MAX_DRAIN_FRAMES + 1);
    assert!(
        matches!(instructions.last(), Some(ClientInstruction::FromServer(0, instruction))
        if matches!(**instruction, ClientInstruction::ServerReaderDrained(0)))
    );
    // The reader stopped with complete frames still available, without EOF/Idle.
    assert!(matches!(os.recv_from_server(), IpcRecv::Msg(..)));
    let now = Instant::now();
    assert!(recovery.poll(&os, "work", now).unwrap().is_none());
    recovery.control.reader_drained(0);
    // Before an initial render, poll reports the failure instead of retrying.
    // This proves that the marker releases the recovery gate.
    assert!(recovery
        .poll(&os, "work", now)
        .err()
        .unwrap()
        .to_string()
        .contains("write failed"));
}

#[test]
fn delayed_router_keeps_its_original_generation_and_leaves_the_new_socket_alone() {
    let os = FakeClientOsApi::new();
    let control = RecoveryControl::new(CommandIsExecuting::new());
    os.set_remote_recovery(Some(control.clone())).unwrap();
    let (tx, rx) = channels::unbounded();
    let worker = worker_with_initial(
        Box::new(os.clone()),
        SenderWithContext::new(tx),
        Arc::new(RenderBudget::new(100)),
        [(
            ServerToClientMsg::Render {
                content: "STALE".into(),
            },
            ErrorContext::default(),
        )]
        .into(),
    );
    // Run the captured worker only after two replacements, without relying on scheduling.
    control.replacing();
    control.replacing();
    control.repainted();
    os.queue_from_server(ServerToClientMsg::Exit {
        exit_reason: ExitReason::Normal,
    });
    worker();
    assert!(rx.is_empty());
    assert!(matches!(
        os.recv_from_server(),
        IpcRecv::Msg(ServerToClientMsg::Exit { .. }, _)
    ));
}

#[test]
fn write_failure_drains_pending_detach_and_session_switch_before_stopping() {
    for message in [
        ServerToClientMsg::Exit {
            exit_reason: ExitReason::NormalDetached,
        },
        ServerToClientMsg::SwitchSession {
            connect_to_session: Default::default(),
        },
    ] {
        let os = FakeClientOsApi::new();
        let control = RecoveryControl::new(CommandIsExecuting::new());
        os.set_remote_recovery(Some(control.clone())).unwrap();
        // An unrelated pending frame must not hide the explicit message behind it.
        os.queue_from_server(ServerToClientMsg::UnblockInputThread);
        os.queue_from_server(message);
        control.lost(0, "write failed".into());
        let (tx, rx) = channels::unbounded();
        route(
            Box::new(os),
            SenderWithContext::new(tx),
            &RenderBudget::new(100),
        );
        let instructions: Vec<_> = rx.iter().map(|(instruction, _)| instruction).collect();
        assert_eq!(instructions.len(), 2, "{instructions:?}");
        assert!(
            matches!(&instructions[1], ClientInstruction::FromServer(0, instruction)
            if matches!(**instruction, ClientInstruction::Exit(_) | ClientInstruction::SwitchSession(_)))
        );
    }
}

#[test]
fn write_failure_without_pending_messages_reports_draining_finished() {
    let os = FakeClientOsApi::new();
    let control = RecoveryControl::new(CommandIsExecuting::new());
    os.set_remote_recovery(Some(control.clone())).unwrap();
    control.lost(0, "write failed".into());
    let (tx, rx) = channels::unbounded();
    let worker = spawn(
        Box::new(os),
        SenderWithContext::new(tx),
        Arc::new(RenderBudget::new(100)),
    );
    let (instruction, _) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(
        matches!(instruction, ClientInstruction::FromServer(0, instruction)
        if matches!(*instruction, ClientInstruction::ServerReaderDrained(0)))
    );
    worker.join().unwrap();
    assert!(rx.is_empty());
}

#[test]
fn output_pressure_does_not_expire_draining_before_a_pending_exit() {
    let os = FakeClientOsApi::new();
    let control = RecoveryControl::new(CommandIsExecuting::new());
    os.set_remote_recovery(Some(control.clone())).unwrap();
    for content in ["first", "second"] {
        os.queue_from_server(ServerToClientMsg::Render {
            content: content.into(),
        });
    }
    os.queue_from_server(ServerToClientMsg::Exit {
        exit_reason: ExitReason::NormalDetached,
    });
    control.lost(0, "write failed".into());
    // Rendezvous sends prevent the router from reaching the exit until both
    // renders are consumed, regardless of scheduling.
    let (tx, rx) = channels::bounded(0);
    let (done, finished) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || {
        route(
            Box::new(os),
            SenderWithContext::new(tx),
            &RenderBudget::new(100),
        );
        done.send(()).unwrap();
    });
    let (instruction, _) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(
        matches!(instruction, ClientInstruction::FromServer(0, instruction)
        if matches!(*instruction, ClientInstruction::Render(_)))
    );
    // Timing is the behavior under test: draining must survive a consumer stall
    // longer than the socket's idle deadline, once the router has started draining.
    assert!(matches!(
        finished.recv_timeout(crate::remote_recovery::READ_POLL * 2),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    let (instruction, _) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(
        matches!(instruction, ClientInstruction::FromServer(0, instruction)
        if matches!(*instruction, ClientInstruction::Render(_)))
    );
    let (instruction, _) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(
        matches!(instruction, ClientInstruction::FromServer(0, instruction)
        if matches!(*instruction, ClientInstruction::Exit(ExitReason::NormalDetached)))
    );
    worker.join().unwrap();
}
