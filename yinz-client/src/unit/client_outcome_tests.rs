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
fn interactive_startup_shows_the_splash_and_detached_startup_does_not() {
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
            Default::default(),
            info,
            None,
            None,
            false,
            false,
        );
        assert_eq!(outcome.exit_status, 0);
        assert!(String::from_utf8(os.stdout_bytes())
            .unwrap()
            .contains("░██░"));
        assert!(!os.raw_mode());
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
