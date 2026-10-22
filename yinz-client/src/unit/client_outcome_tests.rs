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
        config: Some("/local/config.kdl".into()),
        config_dir: Some("/local/config".into()),
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
