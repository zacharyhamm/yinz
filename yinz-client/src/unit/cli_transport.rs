use super::*;
use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions};
use yinz_ipc::ipc::IpcSenderWithContext;

#[test]
fn requests_are_correlated_and_a_lost_mutation_is_not_retried() {
    for mode in ["reply", "disconnect", "timeout"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cli.sock");
        let listener = ListenerOptions::new()
            .name(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
            .create_sync()
            .unwrap();
        let server = std::thread::spawn(move || {
            let mut sender =
                IpcSenderWithContext::<ServerToClientMsg>::new(listener.accept().unwrap());
            let mut receiver = sender.get_receiver::<ClientToServerMsg>().unwrap();
            assert!(matches!(
                receiver.recv_client_msg(),
                IpcRecv::Msg(ClientToServerMsg::DataRequest { .. }, _)
            ));
            if mode == "disconnect" {
                return;
            }
            if mode == "reply" {
                for id in ["another-request", "wanted"] {
                    sender
                        .send_server_msg(ServerToClientMsg::DataResponse {
                            request_id: id.into(),
                            result: Ok(serde_json::json!({"id":id}).to_string()),
                        })
                        .unwrap();
                }
            }
            receiver
                .set_recv_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            assert!(
                matches!(
                    receiver.recv_client_msg(),
                    IpcRecv::Msg(ClientToServerMsg::ClientExited, _)
                ),
                "the only follow-up is disconnect, never another mutation"
            );
        });
        let result = exchange_at(
            &path,
            &SessionName::new("fixture"),
            Duration::from_millis(100),
            ClientToServerMsg::DataRequest {
                request_id: "wanted".into(),
                request: yinz_types::cli::DataRequest::Apply {
                    namespace: "shared:test".into(),
                    mutations: vec![],
                },
            },
            |message| match message {
                ServerToClientMsg::DataResponse { request_id, result }
                    if request_id == "wanted" =>
                {
                    Some(Ok(serde_json::from_str(&result.unwrap()).unwrap()))
                },
                _ => None,
            },
        );
        match mode {
            "reply" => assert_eq!(result.unwrap()["id"], "wanted"),
            "disconnect" => assert!(result.unwrap_err().to_string().contains("disconnected")),
            _ => assert!(result.unwrap_err().to_string().contains("not retried")),
        }
        server.join().unwrap();
    }
}
