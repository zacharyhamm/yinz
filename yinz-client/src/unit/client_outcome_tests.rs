use crate::test_support::FakeClientOsApi;
use crate::{start_client, ClientInfo};
use std::io::ErrorKind;
use yinz_ipc::ipc::{ClientToServerMsg, ExitReason, ServerToClientMsg};
use yinz_types::data::SessionConnection;

#[test]
fn start_client_returns_terminal_failure_after_cleanup() {
    check_outcome(Some(ErrorKind::BrokenPipe), None);
}

#[test]
fn start_client_resets_output_abandonment_after_teardown() {
    use crate::terminal_escapes::{stalled_output_abandoned, ClientTeardown};

    const CHILD: &str = "YINZ_TEST_RESET_OUTPUT_ABANDONMENT";
    if std::env::var_os(CHILD).is_some() {
        let os_input = FakeClientOsApi::new().capturing_stdout();
        ClientTeardown::new(24, false).restore(&os_input, "switching sessions");
        assert!(stalled_output_abandoned());
        check_outcome(Some(ErrorKind::TimedOut), None);
        assert!(
            stalled_output_abandoned(),
            "teardown must enable abandonment again"
        );
        return;
    }
    // Cancellation is process-global; isolate it from other tests under libtest too.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "unit::client_outcome_tests::start_client_resets_output_abandonment_after_teardown",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn start_client_returns_normal_shutdown_after_cleanup() {
    check_outcome(None, None);
}

#[test]
fn start_client_returns_reconnection_without_restoring_the_terminal() {
    check_outcome(
        None,
        Some(SessionConnection {
            name: Some("next-session".into()),
            ..Default::default()
        }),
    );
}

#[test]
fn legacy_reconnect_retains_options_when_configuration_on_disk_is_invalid() {
    if yinz_test_support::reexec_test() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.kdl");
    std::fs::write(&config_path, "scroll_buffer_size 1000\nmouse_mode false\n").unwrap();
    let args = yinz_cli::args::CliArgs {
        config: Some(config_path.clone()),
        config_dir: Some(dir.path().into()),
        ..Default::default()
    };
    let (config, mut options) = yinz_cli::setup::Setup::config_without_layout(&args).unwrap();
    options.scroll_buffer_size = Some(42);
    std::fs::write(&config_path, "{").unwrap();
    let os = FakeClientOsApi::new().capturing_stdout();
    os.queue_from_server(ServerToClientMsg::Render {
        content: "retained config attached".into(),
    });
    os.queue_from_server(ServerToClientMsg::Exit {
        exit_reason: ExitReason::Normal,
    });
    let outcome = start_client(
        Box::new(os.clone()),
        args,
        config,
        options.clone(),
        ClientInfo::Attach("legacy-reconnect".into(), options.clone()),
        None,
        None,
        true,
        false,
    );
    assert_eq!(outcome.exit_status, 0, "{}", outcome.exit_msg);
    assert!(os.sent_messages().iter().any(|msg| matches!(msg,
        ClientToServerMsg::AttachClient { cli_assets, .. }
        if cli_assets.configuration_options.as_ref() == Some(&options)
    )));
    assert!(String::from_utf8(os.stdout_bytes())
        .unwrap()
        .contains("retained config attached"));
}

fn check_outcome(error: Option<ErrorKind>, reconnect: Option<SessionConnection>) {
    let mut os_input = FakeClientOsApi::new().capturing_stdout();
    if let Some(error) = error {
        os_input = os_input.with_stdout_error_on(error, b"frame".to_vec());
    }
    os_input.queue_from_server(ServerToClientMsg::Render {
        content: "frame".into(),
    });
    if let Some(connect_to_session) = reconnect.clone() {
        os_input.queue_from_server(ServerToClientMsg::SwitchSession { connect_to_session });
    }
    // The server finishes the stream after the client leaves, including a session switch.
    os_input.queue_from_server(ServerToClientMsg::Exit {
        exit_reason: ExitReason::Normal,
    });
    let observed = os_input.clone();
    let (done, completion) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let outcome = start_client(
            Box::new(os_input),
            Default::default(),
            Default::default(),
            Default::default(),
            ClientInfo::Attach("client-outcome-test".into(), Default::default()),
            None,
            None,
            true, // Reconnects write immediately, so the queued exit cannot bypass the frame.
            false,
        );
        done.send(outcome).unwrap();
    });
    let outcome = completion
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("start_client must finish cleanup and return");
    worker.join().unwrap();
    assert!(!String::from_utf8(observed.stdout_bytes())
        .unwrap()
        .contains("░██░"));
    assert_eq!(outcome.exit_status, i32::from(error.is_some()));
    assert_eq!(outcome.reconnect_to_session, reconnect);
    assert_eq!(observed.raw_mode(), reconnect.is_some());
    assert!(observed
        .sent_messages()
        .iter()
        .any(|msg| matches!(msg, ClientToServerMsg::ClientExited)));
    if error.is_some() {
        assert!(
            outcome.exit_msg.contains("Could not write to the terminal"),
            "{}",
            outcome.exit_msg
        );
    } else if reconnect.is_none() {
        assert_eq!(outcome.exit_msg, ExitReason::Normal.to_string());
        assert!(String::from_utf8(observed.stdout_bytes())
            .unwrap()
            .contains(&outcome.exit_msg));
    }
}

#[test]
fn interactive_startup_respects_splash_option_and_detached_startup_stays_quiet() {
    for disabled in [None, Some(false), Some(true)] {
        for info in [
            ClientInfo::New("splash-new".into(), None, None),
            ClientInfo::Resurrect("splash-resurrect".into(), Default::default(), false, None),
            ClientInfo::Attach("splash-attach".into(), Default::default()),
            ClientInfo::Watch("splash-watch".into(), Default::default()),
        ] {
            let os = FakeClientOsApi::new().capturing_stdout();
            os.queue_from_server(ServerToClientMsg::Exit {
                exit_reason: ExitReason::Normal,
            });
            let outcome = start_client(
                Box::new(os.clone()),
                Default::default(),
                Default::default(),
                yinz_types::input::options::Options {
                    disable_splash_screen: disabled,
                    ..Default::default()
                },
                info,
                None,
                None,
                false,
                false,
            );
            assert_eq!(outcome.exit_status, 0);
            assert_eq!(
                String::from_utf8(os.stdout_bytes())
                    .unwrap()
                    .contains("░██░"),
                disabled != Some(true)
            );
            assert!(!os.raw_mode());
        }
    }
    let os = FakeClientOsApi::new().capturing_stdout();
    let outcome = start_client(
        Box::new(os.clone()),
        Default::default(),
        Default::default(),
        Default::default(),
        ClientInfo::New("splash-detached".into(), None, None),
        None,
        None,
        false,
        true,
    );
    assert_eq!(outcome.exit_status, 0);
    assert!(os.stdout_bytes().is_empty());
    assert!(!os.raw_mode());
    assert!(os
        .sent_messages()
        .iter()
        .any(|msg| matches!(msg, ClientToServerMsg::FirstClientConnected { .. })));
}

#[test]
fn remote_attach_sends_only_terminal_assets_and_restores_before_switching() {
    let local_config = tempfile::tempdir().unwrap();
    let config_path = local_config.path().join("config.kdl");
    std::fs::write(&config_path, "").unwrap();
    let os = FakeClientOsApi::new().capturing_stdout();
    os.queue_from_server(ServerToClientMsg::SwitchSession {
        connect_to_session: SessionConnection {
            name: Some("next-remote".into()),
            ..Default::default()
        },
    });
    os.queue_from_server(ServerToClientMsg::Exit {
        exit_reason: ExitReason::Normal,
    });
    let opts = yinz_cli::args::CliArgs {
        config: Some(config_path),
        config_dir: Some(local_config.path().into()),
        data_dir: Some("/local/data".into()),
        layout: Some("/local/layout.kdl".into()),
        ..Default::default()
    };
    let outcome = start_client(
        Box::new(os.clone()),
        opts,
        Default::default(),
        Default::default(),
        ClientInfo::Remote {
            host: "test-host".to_owned(),
            name: "same-name-as-local".into(),
            socket: "/forwarded/socket".into(),
            cancelled: Default::default(),
        },
        None,
        None,
        true,
        false,
    );
    assert_eq!(outcome.exit_status, 0);
    assert!(outcome.reconnect_to_session.is_some());
    assert!(!os.raw_mode());
    let messages = os.sent_messages();
    let assets = messages
        .iter()
        .find_map(|msg| match msg {
            ClientToServerMsg::AttachClient { cli_assets, .. } => Some(cli_assets),
            _ => None,
        })
        .unwrap();
    assert!(assets.config_file_path.is_none());
    assert!(assets.config_dir.is_none());
    assert!(assets.data_dir.is_none());
    assert!(assets.layout.is_none());
    assert!(assets.configuration_options.is_none());
    assert!(assets.cwd.is_none());
    assert!(!messages
        .iter()
        .any(|msg| matches!(msg, ClientToServerMsg::FirstClientConnected { .. })));
}

#[test]
fn cancelled_remote_handoff_does_not_claim_the_terminal_or_connect() {
    let os = FakeClientOsApi::new().capturing_stdout();
    let outcome = start_client(
        Box::new(os.clone()),
        Default::default(),
        Default::default(),
        Default::default(),
        ClientInfo::Remote {
            host: "test-host".to_owned(),
            name: "cancelled-remote".into(),
            socket: "/forwarded/socket".into(),
            cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        },
        None,
        None,
        false,
        false,
    );
    assert_eq!(outcome.exit_status, 130);
    assert!(outcome.reconnect_to_session.is_none());
    assert!(!os.raw_mode());
    assert!(os.stdout_bytes().is_empty());
    assert!(os.sent_messages().is_empty());
}

#[test]
fn remote_hosts_are_remembered_only_after_a_successful_render() {
    if yinz_test_support::reexec_test() {
        return;
    }
    let cache = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("YINZ_TEST_CACHE_ROOT", cache.path());
    }
    for (host, render, fail) in [
        ("no-render", false, false),
        ("failed-render", true, true),
        ("user@alias", true, false),
    ] {
        let mut os = FakeClientOsApi::new().capturing_stdout();
        if fail {
            os = os.with_stdout_error_on(ErrorKind::BrokenPipe, b"frame".to_vec());
        }
        if render {
            os.queue_from_server(ServerToClientMsg::Render {
                content: "frame".into(),
            });
        }
        os.queue_from_server(ServerToClientMsg::Exit {
            exit_reason: ExitReason::Normal,
        });
        let outcome = start_client(
            Box::new(os),
            Default::default(),
            Default::default(),
            Default::default(),
            ClientInfo::Remote {
                host: host.into(),
                name: "remote".into(),
                socket: "/forwarded/socket".into(),
                cancelled: Default::default(),
            },
            None,
            None,
            true,
            false,
        );
        assert_eq!(outcome.exit_status, i32::from(fail));
    }
    assert_eq!(
        yinz_runtime::remote_hosts::load(&yinz_runtime::consts::YINZ_CACHE_DIR).unwrap(),
        ["user@alias"]
    );
}

#[test]
fn refused_destination_keeps_rendering_the_source_until_it_exits() {
    use yinz_types::session_destination::*;
    let os = FakeClientOsApi::new().capturing_stdout();
    os.queue_from_server(ServerToClientMsg::SessionRequest(Request {
        token: "refused".into(),
        request: ClientRequest::Switch(SwitchRequest::Prepare {
            id: "switch".into(),
            target: SwitchTarget {
                destination: Destination::Remote("-invalid-host".into()),
                name: "same".into(),
                saved: false,
            },
        }),
    }));
    os.queue_from_server(ServerToClientMsg::Render {
        content: "SOURCE STILL USABLE".into(),
    });
    os.queue_from_server(ServerToClientMsg::Exit {
        exit_reason: ExitReason::Normal,
    });
    let outcome = start_client(
        Box::new(os.clone()),
        Default::default(),
        Default::default(),
        Default::default(),
        ClientInfo::Attach("source".into(), Default::default()),
        None,
        None,
        true,
        false,
    );
    assert_eq!(outcome.exit_status, 0);
    assert!(outcome.reconnect_to_session.is_none());
    assert!(String::from_utf8(os.stdout_bytes())
        .unwrap()
        .contains("SOURCE STILL USABLE"));
    let sent = os.sent_messages();
    let refusal = sent.iter().position(|message| matches!(message, ClientToServerMsg::SessionReply(Reply { token, result: Err(_) }) if token == "refused")).unwrap();
    let exits = sent
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            matches!(message, ClientToServerMsg::ClientExited).then_some(index)
        })
        .collect::<Vec<_>>();
    assert_eq!(exits.len(), 1);
    assert!(exits[0] > refusal);
}

#[test]
fn reconnect_clear_output_failure_preserves_the_session_switch() {
    if yinz_test_support::reexec_test() {
        return;
    }
    use crate::terminal_escapes::CLEAR_SCREEN;
    use yinz_test_support::default_timeout;

    for error in [
        ErrorKind::WouldBlock,
        ErrorKind::TimedOut,
        ErrorKind::BrokenPipe,
    ] {
        let os = FakeClientOsApi::new()
            .capturing_stdout()
            .with_stdout_error_on(error, CLEAR_SCREEN.as_bytes().to_vec());
        let destination = SessionConnection {
            name: Some("next-session".into()),
            ..Default::default()
        };
        os.queue_from_server(ServerToClientMsg::SwitchSession {
            connect_to_session: destination.clone(),
        });
        os.queue_from_server(ServerToClientMsg::Exit {
            exit_reason: ExitReason::Normal,
        });
        let (done, completion) = std::sync::mpsc::channel();
        let worker = std::thread::spawn({
            let os = os.clone();
            move || done.send(run_client(os, Default::default())).unwrap()
        });
        let outcome = completion
            .recv_timeout(default_timeout())
            .expect("a failed screen clear must not prevent reconnecting");
        worker.join().unwrap();
        assert_eq!(outcome.exit_status, 0);
        assert_eq!(outcome.reconnect_to_session, Some(destination));
        assert!(os.raw_mode());
        assert!(os
            .stdout_attempts
            .get()
            .iter()
            .any(|bytes| bytes == CLEAR_SCREEN.as_bytes()));
    }
}

#[test]
fn signal_setup_failure_restores_the_terminal() {
    let os = FakeClientOsApi::new()
        .capturing_stdout()
        .with_signal_setup_error();
    let outcome = run_client(os.clone(), Default::default());
    assert_eq!(outcome.exit_status, 1);
    assert!(outcome.reconnect_to_session.is_none());
    assert!(outcome.exit_msg.contains("signal registration failed"));
    assert!(!os.raw_mode());
    assert!(os
        .sent_messages()
        .iter()
        .any(|msg| matches!(msg, ClientToServerMsg::ClientExited)));
}

fn run_client(
    os: FakeClientOsApi,
    options: yinz_types::input::options::Options,
) -> crate::ClientOutcome {
    start_client(
        Box::new(os),
        Default::default(),
        Default::default(),
        options,
        ClientInfo::Attach("worker-cleanup-test".into(), Default::default()),
        None,
        None,
        true,
        false,
    )
}

#[test]
fn repeated_reconnects_only_query_from_the_current_listener() {
    if yinz_test_support::reexec_test() {
        return;
    }
    use crate::os_input_output::SignalEvent;
    use crate::stdin_handler::PIXEL_SIZE_QUERY;
    use yinz_test_support::default_timeout;

    let mut previous: Vec<(FakeClientOsApi, yinz_runtime::channels::Sender<SignalEvent>)> =
        Vec::new();
    for _ in 0..4 {
        let (signals, rx) = yinz_runtime::channels::bounded(8);
        let os = FakeClientOsApi::new().capturing_stdout().with_signals(rx);
        let (done, completion) = std::sync::mpsc::channel();
        let worker = std::thread::spawn({
            let os = os.clone();
            move || done.send(run_client(os, Default::default())).unwrap()
        });
        for (_, old_signals) in &previous {
            old_signals.send(SignalEvent::Resize).unwrap();
        }
        signals.send(SignalEvent::Resize).unwrap();
        os.stdout_attempts.wait_until(
            "current connection's pixel query",
            default_timeout(),
            |writes| {
                writes
                    .iter()
                    .any(|bytes| bytes == PIXEL_SIZE_QUERY.as_bytes())
                    .then_some(())
            },
        );
        os.queue_from_server(ServerToClientMsg::SwitchSession {
            connect_to_session: SessionConnection {
                name: Some("next-session".into()),
                ..Default::default()
            },
        });
        os.queue_from_server(ServerToClientMsg::Exit {
            exit_reason: ExitReason::Normal,
        });
        let outcome = completion.recv_timeout(default_timeout()).unwrap();
        worker.join().unwrap();
        assert_eq!(outcome.exit_status, 0);
        assert!(outcome.reconnect_to_session.is_some());
        previous.push((os, signals));
        for (old, _) in &previous {
            assert_eq!(
                old.sent_messages()
                    .iter()
                    .filter(|msg| matches!(msg, ClientToServerMsg::TerminalResize { .. }))
                    .count(),
                1
            );
            assert_eq!(
                old.stdout_attempts
                    .get()
                    .iter()
                    .filter(|bytes| bytes.as_slice() == PIXEL_SIZE_QUERY.as_bytes())
                    .count(),
                1
            );
        }
    }
}

#[test]
fn pixel_query_failure_is_reported_after_worker_cleanup() {
    check_query_cleanup(ErrorKind::BrokenPipe, false, false);
}

#[test]
fn switching_sessions_abandons_a_stalled_pixel_query_before_joining() {
    check_query_cleanup(ErrorKind::WouldBlock, true, false);
}

#[test]
fn exiting_abandons_a_stalled_pixel_query_before_joining() {
    check_query_cleanup(ErrorKind::WouldBlock, false, false);
}

#[test]
fn quit_signals_preserve_force_close_and_release_stalled_workers() {
    check_query_cleanup(ErrorKind::WouldBlock, false, true);
}

fn check_query_cleanup(error: ErrorKind, reconnect: bool, quit: bool) {
    if yinz_test_support::reexec_test() {
        return;
    }
    use crate::os_input_output::SignalEvent;
    use crate::stdin_handler::PIXEL_SIZE_QUERY;
    use yinz_test_support::default_timeout;
    use yinz_types::input::{actions::Action, options::OnForceClose};

    for force_close in [OnForceClose::Detach, OnForceClose::Quit] {
        let (signals, rx) = yinz_runtime::channels::bounded(2);
        let os = FakeClientOsApi::new()
            .capturing_stdout()
            .with_signals(rx)
            .with_stdout_error_on(error, PIXEL_SIZE_QUERY.as_bytes().to_vec());
        let (done, completion) = std::sync::mpsc::channel();
        let worker = std::thread::spawn({
            let os = os.clone();
            move || {
                done.send(run_client(
                    os,
                    yinz_types::input::options::Options {
                        on_force_close: Some(force_close),
                        ..Default::default()
                    },
                ))
                .unwrap()
            }
        });
        signals.send(SignalEvent::Resize).unwrap();
        os.stdout_attempts
            .wait_until("pixel query write attempt", default_timeout(), |writes| {
                writes
                    .iter()
                    .any(|bytes| bytes == PIXEL_SIZE_QUERY.as_bytes())
                    .then_some(())
            });
        if quit {
            signals.send(SignalEvent::Quit).unwrap();
            let expected: Action = force_close.into();
            os.sent_to_server.wait_until("force-close action", default_timeout(), |messages| {
                messages.iter().any(|msg| matches!(msg, ClientToServerMsg::Action { action, .. } if action == &expected)).then_some(())
            });
        }
        if reconnect {
            os.queue_from_server(ServerToClientMsg::SwitchSession {
                connect_to_session: SessionConnection {
                    name: Some("next-session".into()),
                    ..Default::default()
                },
            });
        }
        os.queue_from_server(ServerToClientMsg::Exit {
            exit_reason: ExitReason::Normal,
        });
        let outcome = completion
            .recv_timeout(default_timeout())
            .expect("cleanup must release stalled workers");
        worker.join().unwrap();
        let failed = error == ErrorKind::BrokenPipe;
        assert_eq!(outcome.exit_status, i32::from(failed));
        assert_eq!(outcome.reconnect_to_session.is_some(), reconnect && !failed);
        assert_eq!(os.raw_mode(), reconnect && !failed);
        if failed {
            assert!(
                outcome
                    .exit_msg
                    .contains("failed to query the host terminal's pixel size"),
                "{}",
                outcome.exit_msg
            );
        }
    }
}
