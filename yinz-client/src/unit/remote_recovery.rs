use super::*;

#[test]
fn backoff_has_bounded_jitter_and_resets_only_after_a_minute() {
    for sample in [0, 200, 400] {
        let mut backoff = Backoff::default();
        for nominal in [0, 1, 2, 4, 8, 16, 30, 30, 30] {
            let delay = backoff.delay(sample);
            assert!(delay <= Duration::from_secs(30));
            assert!(delay.as_secs_f64() >= f64::from(nominal) * 0.8);
            assert!(delay.as_secs_f64() <= (f64::from(nominal) * 1.2).min(30.));
        }
        backoff.healthy(Duration::from_secs(59));
        assert!(backoff.delay(sample) >= Duration::from_secs(24));
        backoff.healthy(Duration::from_secs(60));
        assert_eq!(backoff.delay(sample), Duration::ZERO);
    }
}

#[test]
fn failed_transport_unblocks_input_and_generations_reject_stale_failures() {
    let mut command = CommandIsExecuting::new();
    let control = RecoveryControl::new(command.clone());
    command.blocking_input_thread();
    control.lost(0, "write failed".into());
    let (done, rx) = mpsc::channel();
    std::thread::spawn(move || {
        command.wait_until_input_thread_is_unblocked();
        done.send(()).unwrap();
    });
    rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(control.phase(), Phase::Reconnecting);
    assert!(!control.can_write());
    control.replacing();
    assert!(!control.accepts(0));
    assert_eq!(control.stamp(), (1, false));
    control.lost(0, "late EOF".into());
    assert_eq!(control.phase(), Phase::Repainting);
    assert!(control.repainted());
    control.lost(1, "second outage".into());
    assert_eq!(control.phase(), Phase::Reconnecting);
    control.stop();
    control.lost(1, "late write failure".into());
    assert_eq!(control.phase(), Phase::Stopped);
}

#[test]
fn replay_contains_capabilities_and_current_size_but_never_keyboard_input() {
    let control = RecoveryControl::new(CommandIsExecuting::new());
    control.cache(&ClientToServerMsg::KittyGraphicsSupport { supported: false });
    control.cache(&ClientToServerMsg::KittyGraphicsSupport { supported: true });
    control.cache(&ClientToServerMsg::SixelSupport { supported: true });
    control.cache(&ClientToServerMsg::Paste("OFFLINE".into()));
    let os = crate::test_support::FakeClientOsApi::new();
    control.replay(&os);
    let messages = os.sent_messages();
    assert_eq!(messages.len(), 4);
    assert!(matches!(
        messages[0],
        ClientToServerMsg::TerminalResize { .. }
    ));
    assert!(matches!(
        messages[1],
        ClientToServerMsg::KittyGraphicsSupport { supported: true }
    ));
    assert!(messages
        .iter()
        .all(|message| !matches!(message, ClientToServerMsg::Paste(_))));
}

#[test]
fn recovery_deadline_is_shared_with_bootstrap_and_forwarding_and_cancellation_reaps_children() {
    if yinz_test_support::reexec_test() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let args = dir.path().join("args");
    let endpoint = serde_json::json!({ "session_name": "work", "socket": "/remote/work",
        "yinz_version": yinz_runtime::consts::VERSION,
        "contract_version": yinz_runtime::consts::CLIENT_SERVER_CONTRACT_VERSION,
        "bootstrap_version": yinz_runtime::consts::REMOTE_BOOTSTRAP_VERSION,
    });
    yinz_test_support::write_warm_script(
        &dir.path().join("ssh"),
        r#"
printf '%s\n' "$*" >> "$YINZ_TEST_SSH_ARGS"
case "$*" in
    *remote-session*) printf '%s\n' "$YINZ_TEST_ENDPOINT" ;;
    *) printf '%s\n' "$$" > "$YINZ_TEST_FORWARD_PID"; read -r blocked ;;
esac
"#,
    );
    unsafe {
        std::env::set_var("PATH", dir.path());
        std::env::set_var("YINZ_SOCKET_DIR", dir.path().join("sockets"));
        std::env::set_var("YINZ_TEST_SSH_ARGS", &args);
        std::env::set_var("YINZ_TEST_ENDPOINT", endpoint.to_string());
        std::env::set_var("YINZ_TEST_FORWARD_PID", dir.path().join("pid"));
    }
    for cancel in [false, true] {
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        let started = Instant::now();
        let (done, result) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            done.send(session_switch::prepare_recovery(
                "host".into(),
                "work".into(),
                Default::default(),
                &cancelled,
                Instant::now() + Duration::from_millis(500),
            ))
            .unwrap();
        });
        if cancel {
            stop.store(true, Ordering::Release);
        }
        assert!(result
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .is_err());
        worker.join().unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the forward must use the attempt's deadline"
        );
        if let Ok(pid) = std::fs::read_to_string(dir.path().join("pid")) {
            assert_eq!(
                nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(pid.trim().parse().unwrap()),
                    None
                ),
                Err(nix::errno::Errno::ESRCH)
            );
        }
    }
    let calls = std::fs::read_to_string(args).unwrap();
    assert!(calls.contains("remote-session"));
    assert!(!calls.contains("--create"));
    assert_eq!(
        std::fs::read_dir(dir.path().join("sockets/client"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn queued_stale_frames_and_session_switches_do_not_affect_the_replacement() {
    use crate::{
        client_loop::ClientLoop, nested_reannounce::NestedReannounce, render_budget::RenderBudget,
        stdin_ansi_parser::StdinAnsiParser, terminal_escapes::ClientTeardown, ClientInstruction,
    };
    use yinz_runtime::channels::{self, SenderWithContext};
    let os = crate::test_support::FakeClientOsApi::new().capturing_stdout();
    let mut command = CommandIsExecuting::new();
    let mut recovery = Recovery::new("host".into(), command.clone());
    recovery.rendered = true;
    recovery.control.replacing();
    recovery.control.repainted();
    let (tx, rx) = channels::unbounded();
    let tx = SenderWithContext::new(tx);
    let budget = Arc::new(RenderBudget::new(100));
    budget.reserve(5).unwrap();
    for instruction in [
        ClientInstruction::FromServer(0, Box::new(ClientInstruction::Render("STALE".into()))),
        ClientInstruction::FromServer(
            0,
            Box::new(ClientInstruction::SwitchSession(Default::default())),
        ),
        ClientInstruction::FromServer(
            1,
            Box::new(ClientInstruction::Exit(yinz_ipc::ipc::ExitReason::Normal)),
        ),
    ] {
        tx.send(instruction).unwrap();
    }
    let (input_tx, _input_rx) = channels::unbounded();
    let nested = NestedReannounce::spawn(Box::new(os.clone()), "generation-test".into());
    nested.stop();
    let args = Default::default();
    let mut config = Default::default();
    let mut sessions =
        session_switch::Sessions::new(Some("host".into()), "work".into(), &args, &mut config);
    let outcome = ClientLoop {
        sessions: &mut sessions,
        recovery: &mut Some(recovery),
        send_client_instructions: Some(&tx),
        remote_host: Some("host"),
        os_input: &os,
        command_is_executing: &mut command,
        stdin_ansi_parser: &Arc::new(Mutex::new(StdinAnsiParser::new())),
        send_input_instructions: &SenderWithContext::new(input_tx),
        nested_reannounce: &nested,
        teardown: ClientTeardown::new(24, false),
        synchronised_output: None,
        render_budget: &budget,
        startup_started: Instant::now(),
        splash_deadline: None,
    }
    .run(&rx);
    assert_eq!(outcome.exit_status, 0);
    assert!(outcome.reconnect_to_session.is_none());
    assert!(!String::from_utf8(os.stdout_bytes())
        .unwrap()
        .contains("STALE"));
    budget.reserve(100).unwrap();
    budget.release(100).unwrap();
}

#[test]
fn eof_starts_remote_recovery_but_explicit_detach_does_not() {
    use crate::{
        os_input_output::ServerConnection, render_budget::RenderBudget, ClientInstruction,
    };
    use yinz_ipc::ipc::{ExitReason, ServerToClientMsg};
    use yinz_runtime::channels::{self, SenderWithContext};
    for explicit in [false, true] {
        let os = crate::test_support::FakeClientOsApi::new();
        let control = RecoveryControl::new(CommandIsExecuting::new());
        os.set_remote_recovery(Some(control.clone())).unwrap();
        if explicit {
            os.queue_from_server(ServerToClientMsg::Exit {
                exit_reason: ExitReason::NormalDetached,
            });
        }
        os.queue_server_hangup();
        let (tx, rx) = channels::unbounded();
        crate::router::route(
            Box::new(os),
            SenderWithContext::new(tx),
            &RenderBudget::new(100),
        );
        let instructions: Vec<_> = rx.iter().map(|(instruction, _)| instruction).collect();
        if explicit {
            assert_eq!(control.phase(), Phase::Connected);
            assert!(
                matches!(&instructions[..], [ClientInstruction::FromServer(0, instruction)] if matches!(**instruction, ClientInstruction::Exit(ExitReason::NormalDetached)))
            );
        } else {
            assert_eq!(control.phase(), Phase::Reconnecting);
            assert!(instructions.is_empty());
        }
    }
}

#[test]
fn recovery_status_uses_current_terminal_geometry_and_permanent_failures_do_not_spawn_attempts() {
    let os = crate::test_support::FakeClientOsApi::new().capturing_stdout();
    let now = Instant::now();
    let mut recovery = Recovery::new("host".into(), CommandIsExecuting::new());
    recovery.rendered = true;
    recovery.control.lost(0, "EOF".into());
    recovery.next_retry = Some(now + Duration::from_secs(10));
    recovery.paint_status(&os, "work", now).unwrap();
    let resized = os.clone().with_terminal_size(Size {
        rows: 40,
        cols: 120,
    });
    recovery.paint_status(&resized, "renamed", now).unwrap();
    let output = String::from_utf8(os.stdout_bytes()).unwrap();
    assert!(output.contains("\x1b[24;1H"));
    assert!(output.contains("\x1b[40;1H"));
    assert!(output.contains("host / renamed"));
    assert!(output.contains("Ctrl-C to cancel"));
    let narrow = crate::test_support::FakeClientOsApi::new()
        .capturing_stdout()
        .with_terminal_size(Size { rows: 12, cols: 40 });
    recovery.paint_status(&narrow, "work", now).unwrap();
    let output = String::from_utf8(narrow.stdout_bytes()).unwrap();
    assert!(output.contains("host / work"));
    assert!(output.contains("Ctrl-C to cancel"));
    recovery.control.state.lock().unwrap().failure = Some("Host key verification failed".into());
    let error = recovery.poll(&os, "renamed", now).err().unwrap();
    assert!(error.to_string().contains("Host key verification failed"));
    assert!(recovery.attempt.is_none());
}
