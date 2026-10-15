use super::*;
use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions};
use serde_json::json;

#[test]
fn session_repl_correlates_replies_and_keeps_one_instance_until_close() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("repl.sock");
    let listener = ListenerOptions::new()
        .name(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
        .create_sync()
        .unwrap();
    let server = std::thread::spawn(move || {
        let mut sender = IpcSenderWithContext::<ServerToClientMsg>::new(listener.accept().unwrap());
        let mut receiver = sender.get_receiver::<ClientToServerMsg>().unwrap();
        receiver
            .set_recv_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut identity = None;
        let mut pipe = None;
        for (name, source) in [
            ("__yinz_repl_eval", Some("(define answer 42)")),
            ("__yinz_repl_eval", Some("answer")),
            ("__yinz_repl_close", None),
        ] {
            let IpcRecv::Msg(
                ClientToServerMsg::Action {
                    action:
                        Action::CliPipe {
                            pipe_id,
                            name: actual_name,
                            payload,
                            plugin,
                            configuration,
                            focus_plugin,
                            ..
                        },
                    terminal_id,
                    is_cli_client: true,
                    ..
                },
                _,
            ) = receiver.recv_client_msg()
            else {
                panic!("expected REPL pipe")
            };
            assert_eq!(actual_name.as_deref(), Some(name));
            assert_eq!(payload.as_deref(), source);
            assert_eq!(plugin.as_deref(), Some("yinz:repl"));
            assert_eq!(terminal_id, Some(TerminalId::new(7)));
            assert!(!focus_plugin);
            if let Some(expected) = &identity {
                assert_eq!(&configuration, expected);
            } else {
                identity = Some(configuration);
            }
            if let Some(expected) = &pipe {
                assert_eq!(&pipe_id, expected);
            } else {
                pipe = Some(pipe_id.clone());
            }
            sender
                .send_server_msg(ServerToClientMsg::CliPipeOutput {
                    pipe_name: "unrelated".into(),
                    output: "invalid JSON".into(),
                })
                .unwrap();
            sender
                .send_server_msg(ServerToClientMsg::UnblockCliPipeInput {
                    pipe_name: "unrelated".into(),
                })
                .unwrap();
            sender
                .send_server_msg(ServerToClientMsg::CliPipeOutput {
                    pipe_name: pipe_id.clone(),
                    output: json!({"values":["42"],"output":""}).to_string(),
                })
                .unwrap();
            sender
                .send_server_msg(ServerToClientMsg::UnblockCliPipeInput { pipe_name: pipe_id })
                .unwrap();
        }
        assert!(matches!(
            receiver.recv_client_msg(),
            IpcRecv::Msg(ClientToServerMsg::ClientExited, _)
        ));
    });
    let mut client = Client::connect_at(&path, Some(TerminalId::new(7))).unwrap();
    client.eval("(define answer 42)").unwrap();
    assert_eq!(client.eval("answer").unwrap()["values"], json!(["42"]));
    client.close().unwrap();
    drop(client);
    server.join().unwrap();
}

#[test]
fn session_repl_does_not_retry_an_expression_after_the_pipe_fails() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("repl.sock");
    let listener = ListenerOptions::new()
        .name(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
        .create_sync()
        .unwrap();
    let server = std::thread::spawn(move || {
        let mut sender = IpcSenderWithContext::<ServerToClientMsg>::new(listener.accept().unwrap());
        let mut receiver = sender.get_receiver::<ClientToServerMsg>().unwrap();
        receiver
            .set_recv_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let IpcRecv::Msg(
            ClientToServerMsg::Action {
                action: Action::CliPipe { pipe_id, .. },
                ..
            },
            _,
        ) = receiver.recv_client_msg()
        else {
            panic!("expected expression")
        };
        sender
            .send_server_msg(ServerToClientMsg::UnblockCliPipeInput { pipe_name: pipe_id })
            .unwrap();
        let IpcRecv::Msg(
            ClientToServerMsg::Action {
                action: Action::CliPipe { name, .. },
                ..
            },
            _,
        ) = receiver.recv_client_msg()
        else {
            panic!("expected cleanup")
        };
        assert_eq!(name.as_deref(), Some("__yinz_repl_close"));
        assert!(matches!(
            receiver.recv_client_msg(),
            IpcRecv::Msg(ClientToServerMsg::ClientExited, _)
        ));
    });
    let mut client = Client::connect_at(&path, None).unwrap();
    assert!(client
        .eval("(yinz-command \"NewPane\")")
        .unwrap_err()
        .to_string()
        .contains("without a reply"));
    assert!(client
        .eval("(+ 1 2)")
        .unwrap_err()
        .to_string()
        .contains("cannot continue"));
    drop(client);
    server.join().unwrap();
}
