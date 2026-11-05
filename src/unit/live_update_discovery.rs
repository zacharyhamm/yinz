use super::*;
use interprocess::local_socket::traits::Listener;
use yinz_ipc::ipc::{ClientToServerMsg, IpcReceiverWithContext, IpcRecv, ServerToClientMsg};

struct Discovery {
    root: tempfile::TempDir,
    operation: String,
    socket: std::path::PathBuf,
}

impl Discovery {
    fn new(release: serde_json::Value) -> Self {
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("session");
        let operation = uuid::Uuid::new_v4().to_string();
        let mut store = Store::open(&socket, root.path(), &operation).unwrap();
        store.journal.channel = serde_json::from_value(
            release
                .get("channel")
                .cloned()
                .unwrap_or(serde_json::json!("Stable")),
        )
        .unwrap();
        store.journal.release = Some(release);
        store.save().unwrap();
        Self {
            root,
            operation,
            socket,
        }
    }

    fn journal(&self) -> Journal {
        serde_json::from_value(
            yinz_runtime::session_update::read_active(self.root.path(), &self.socket)
                .unwrap()
                .unwrap(),
        )
        .unwrap()
    }

    fn run(&self, clients: Vec<yinz_types::session_update::Client>) -> Result<()> {
        let listener = yinz_ipc::consts::ipc_bind(&self.socket).unwrap();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let mut probe =
                    IpcReceiverWithContext::<ClientToServerMsg>::new(listener.accept().unwrap());
                probe
                    .set_recv_timeout(Some(yinz_test_support::default_timeout()))
                    .unwrap();
                assert!(matches!(
                    probe.recv_client_msg(),
                    IpcRecv::Msg(ClientToServerMsg::ConnStatus, _)
                ));
                probe
                    .get_sender::<ServerToClientMsg>()
                    .unwrap()
                    .send_server_msg(ServerToClientMsg::Connected {
                        warp_companions: false,
                        warp_held_terminals: false,
                        warp_agent_tracking: false,
                        warp_terminal_state: 0,
                        signal_quit: false,
                        update_protocol: yinz_types::session_update::PROTOCOL_VERSION,
                    })
                    .unwrap();
                let mut peer =
                    IpcReceiverWithContext::<ClientToServerMsg>::new(listener.accept().unwrap());
                peer.set_recv_timeout(Some(yinz_test_support::default_timeout()))
                    .unwrap();
                let mut sender = peer.get_sender::<ServerToClientMsg>().unwrap();
                loop {
                    match peer.recv_client_msg() {
                        IpcRecv::Msg(ClientToServerMsg::SessionUpdate(request), _) => {
                            let value = match request.command {
                                Command::Inventory => serde_json::to_value(Inventory {
                                    session: "session".into(),
                                    server_pid: std::process::id(),
                                    server_instance: "source".into(),
                                    executable: "/usr/bin/true".into(),
                                    running_version: "0.1.2-alpha.1".into(),
                                    running_binary_sha256: None,
                                    assets: Default::default(),
                                    configuration: serde_json::json!(""),
                                    clients: clients.clone(),
                                })
                                .unwrap(),
                                Command::Notify { .. } => serde_json::Value::Null,
                                other => panic!(
                                    "discovery must not prepare or mutate the session: {other:?}"
                                ),
                            };
                            sender
                                .send_server_msg(ServerToClientMsg::SessionUpdateReply(
                                    yinz_types::session_update::Reply {
                                        token: request.token,
                                        operation: request.operation,
                                        result: Ok(value),
                                    },
                                ))
                                .unwrap();
                        },
                        IpcRecv::Msg(ClientToServerMsg::ClientExited, _) | IpcRecv::Eof => break,
                        IpcRecv::Msg(other, _) => {
                            panic!("unexpected coordinator request: {other:?}")
                        },
                        IpcRecv::Idle => panic!("coordinator request timed out"),
                        IpcRecv::Malformed => panic!("malformed coordinator request"),
                    }
                }
            });
            run_with_channel(
                &self.socket,
                self.root.path(),
                &self.operation,
                false,
                self.journal().channel,
            )
        })
    }
}

#[test]
fn downgrade_refusal_fails_and_a_prerelease_start_gets_a_fresh_operation() {
    let fixture = Discovery::new(serde_json::json!({"tag_name":"v0.1.1", "assets":[]}));
    let error = fixture.run(vec![]).unwrap_err();
    assert!(error.to_string().contains("source server is newer"));
    let failed = fixture.journal();
    assert_eq!(failed.phase, Phase::Failed);
    assert_eq!(failed.channel, ReleaseChannel::Stable);
    assert_eq!(failed.errors.len(), 1);
    assert!(failed.participants.is_empty());
    let next = uuid::Uuid::new_v4().to_string();
    let store = Store::open_with_channel(
        &fixture.socket,
        fixture.root.path(),
        &next,
        false,
        ReleaseChannel::Prerelease,
    )
    .unwrap();
    assert_eq!(store.journal.operation, next);
    assert_eq!(store.journal.channel, ReleaseChannel::Prerelease);
    assert_eq!(store.journal.phase, Phase::Discover);
    assert!(store.journal.release.is_none());
}

#[test]
fn malformed_pinned_release_fails_before_preparation() {
    let fixture = Discovery::new(serde_json::json!({"tag_name":42}));
    assert!(fixture
        .run(vec![])
        .unwrap_err()
        .to_string()
        .contains("decode pinned update release"));
    assert_eq!(fixture.journal().phase, Phase::Failed);
}

#[test]
fn discovery_network_failure_is_terminal_and_can_be_retried_fresh() {
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("session");
    let operation = uuid::Uuid::new_v4().to_string();
    let mut store = Store::open(&socket, root.path(), &operation).unwrap();
    let error =
        resolve_release(&mut store, &mut |_, _| Err(anyhow!("network unavailable"))).unwrap_err();
    record_discovery_failure(&mut store, &error).unwrap();
    assert_eq!(store.journal.phase, Phase::Failed);
    drop(store);
    let next = uuid::Uuid::new_v4().to_string();
    let store = Store::open(&socket, root.path(), &next).unwrap();
    assert_eq!(store.journal.operation, next);
    assert!(store.journal.release.is_none());
}

#[test]
fn prerelease_capability_refusal_is_terminal_before_preparation() {
    let fixture = Discovery::new(
        serde_json::json!({"channel":"Prerelease", "tag_name":"v0.1.3-alpha.1", "assets":[]}),
    );
    let error = fixture
        .run(vec![yinz_types::session_update::Client {
            id: 1.into(),
            supports_update: false,
            supports_source_update: false,
            resumed: None,
            configuration: String::new(),
            mode: Default::default(),
        }])
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("does not support prerelease updates"));
    assert_eq!(fixture.journal().phase, Phase::Failed);
    assert!(fixture.journal().participants.is_empty());
}
