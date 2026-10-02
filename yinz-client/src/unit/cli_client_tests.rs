use crate::cli_client::{start_cli_client, start_events_client, start_subscribe_client};
use crate::os_input_output::ServerConnection;
use crate::test_support::FakeClientOsApi;
use yinz_cli::args::{EventsCli, SubscribeCli, SubscribeFormat};
use yinz_ipc::ipc::{ClientToServerMsg, ExitReason, IpcRecv, ServerToClientMsg};
use yinz_types::input::actions::Action;

fn pipe_action(payload: Option<String>) -> Action {
    Action::CliPipe {
        pipe_id: "pipe".into(),
        name: None,
        payload,
        plugin: None,
        args: None,
        configuration: None,
        launch_new: false,
        skip_cache: false,
        floating: None,
        in_place: None,
        cwd: None,
        pane_title: None,
    }
}

#[test]
fn action_and_pipe_clients_fail_when_the_server_disconnects() {
    for action in [Action::NoOp, pipe_action(Some("input".into()))] {
        let os = FakeClientOsApi::default();
        os.queue_server_hangup();
        // A broken implementation reaches this successful reply instead of
        // failing on EOF, so this regression does not itself spin forever.
        os.queue_from_server(ServerToClientMsg::Log { lines: vec![] });
        assert_eq!(
            start_cli_client(Box::new(os), &"test".into(), vec![action], None),
            2
        );
    }
}

#[test]
fn streaming_clients_do_not_treat_a_malformed_message_as_eof() {
    for events in [true, false] {
        let os = FakeClientOsApi::default();
        os.queue_malformed_from_server();
        os.queue_from_server(ServerToClientMsg::Exit {
            exit_reason: ExitReason::Normal,
        });
        os.queue_server_hangup();
        if events {
            start_events_client(
                Box::new(os.clone()),
                &"test".into(),
                EventsCli { event_type: vec![] },
            );
        } else {
            start_subscribe_client(
                Box::new(os.clone()),
                &"test".into(),
                SubscribeCli {
                    pane_id: vec!["1".into()],
                    scrollback: None,
                    format: SubscribeFormat::Raw,
                    ansi: false,
                },
            );
        }
        assert!(matches!(os.recv_from_server(), IpcRecv::Eof));
    }
}

#[test]
fn invalid_pipe_input_is_an_error_not_end_of_input() {
    let os = FakeClientOsApi::default().with_stdin(vec![0xff]);
    assert_eq!(
        start_cli_client(
            Box::new(os.clone()),
            &"test".into(),
            vec![pipe_action(None)],
            None,
        ),
        2,
    );
    assert!(os.sent_messages().is_empty());
}

#[test]
fn pipe_output_accepts_closed_stdout_but_reports_other_write_errors() {
    for (error, expected) in [
        (std::io::ErrorKind::BrokenPipe, 0),
        (std::io::ErrorKind::PermissionDenied, 2),
    ] {
        let os = FakeClientOsApi::default().with_stdout_error(error);
        os.queue_from_server(ServerToClientMsg::CliPipeOutput {
            pipe_name: "pipe".into(),
            output: "result".into(),
        });
        os.queue_from_server(ServerToClientMsg::UnblockCliPipeInput {
            pipe_name: "pipe".into(),
        });
        assert_eq!(
            start_cli_client(
                Box::new(os),
                &"test".into(),
                vec![pipe_action(Some("input".into()))],
                None,
            ),
            expected,
        );
    }
}

#[test]
fn action_and_pipe_logs_accept_closed_stdout_but_report_other_write_errors() {
    for action in [Action::NoOp, pipe_action(Some("input".into()))] {
        for (error, expected) in [
            (None, 0),
            (Some(std::io::ErrorKind::BrokenPipe), 0),
            (Some(std::io::ErrorKind::PermissionDenied), 2),
        ] {
            let mut os = FakeClientOsApi::default().capturing_stdout();
            if let Some(error) = error {
                os = os.with_stdout_error(error);
            }
            os.queue_from_server(ServerToClientMsg::Log {
                lines: vec!["first".into(), "second".into()],
            });
            assert_eq!(
                start_cli_client(
                    Box::new(os.clone()),
                    &"test".into(),
                    vec![action.clone()],
                    None
                ),
                expected,
            );
            if error.is_none() {
                assert_eq!(os.stdout_bytes(), b"first\nsecond\n");
            }
        }
    }
}

/// The server answers a plugin reply too large for one frame with an error, and the plugin's
/// unblock still follows it. The error decides the exit, not the unblock.
#[test]
fn a_pipe_error_before_its_unblock_exits_non_zero() {
    let os = FakeClientOsApi::default().capturing_stdout();
    os.queue_from_server(ServerToClientMsg::LogError {
        lines: vec!["too large".into()],
    });
    os.queue_from_server(ServerToClientMsg::UnblockCliPipeInput {
        pipe_name: "pipe".into(),
    });
    assert_eq!(
        start_cli_client(
            Box::new(os.clone()),
            &"test".into(),
            vec![pipe_action(Some("input".into()))],
            None,
        ),
        2,
    );
    assert!(os.stdout_bytes().is_empty());
}

#[test]
fn pipe_input_eof_waits_for_the_final_response() {
    for input in ["", "input\n"] {
        let os = FakeClientOsApi::default()
            .with_stdin(input.as_bytes().to_vec())
            .capturing_stdout();
        let mut expected_payloads = vec![];
        if !input.is_empty() {
            os.queue_from_server(ServerToClientMsg::UnblockCliPipeInput {
                pipe_name: "pipe".into(),
            });
            expected_payloads.push(Some(input.to_owned()));
        }
        expected_payloads.push(None);
        os.queue_from_server(ServerToClientMsg::CliPipeOutput {
            pipe_name: "pipe".into(),
            output: "final result".into(),
        });
        os.queue_from_server(ServerToClientMsg::UnblockCliPipeInput {
            pipe_name: "pipe".into(),
        });
        assert_eq!(
            start_cli_client(
                Box::new(os.clone()),
                &"test".into(),
                vec![pipe_action(None)],
                None,
            ),
            0,
        );
        let payloads: Vec<_> = os
            .sent_messages()
            .into_iter()
            .filter_map(|message| match message {
                ClientToServerMsg::Action {
                    action: Action::CliPipe { payload, .. },
                    ..
                } => Some(payload),
                _ => None,
            })
            .collect();
        assert_eq!(payloads, expected_payloads);
        assert_eq!(os.stdout_bytes(), b"final result");
    }
}
