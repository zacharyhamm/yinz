use super::*;

#[path = "client_update_real.rs"]
mod client_update_real;
#[path = "live_update_journal.rs"]
mod journal_tests;

#[test]
fn session_preflight_ignores_pty_broker_and_checks_session_capabilities() {
    use interprocess::local_socket::traits::Listener;
    use std::os::unix::net::UnixListener;
    use yinz_ipc::ipc::{ClientToServerMsg, IpcReceiverWithContext, IpcRecv, ServerToClientMsg};
    for protocol in [yinz_types::session_update::PROTOCOL_VERSION, 0] {
        let directory = tempfile::tempdir().unwrap();
        let broker = UnixListener::bind(
            directory
                .path()
                .join(yinz_ipc::consts::pty_broker_socket_name(1)),
        )
        .unwrap();
        broker.set_nonblocking(true).unwrap();
        let session = yinz_ipc::consts::ipc_bind(&directory.path().join("session")).unwrap();
        let probed = yinz_test_support::Watched::new(false);
        std::thread::scope(|scope| {
            let probed = &probed;
            scope.spawn(move || {
                let stream = session.accept().unwrap();
                let mut receiver = IpcReceiverWithContext::<ClientToServerMsg>::new(stream);
                receiver
                    .set_recv_timeout(Some(yinz_test_support::default_timeout()))
                    .unwrap();
                assert!(matches!(
                    receiver.recv_client_msg(),
                    IpcRecv::Msg(ClientToServerMsg::ConnStatus, _)
                ));
                receiver
                    .get_sender::<ServerToClientMsg>()
                    .unwrap()
                    .send_server_msg(ServerToClientMsg::Connected {
                        warp_companions: false,
                        warp_held_terminals: false,
                        warp_agent_tracking: false,
                        warp_terminal_state: 0,
                        signal_quit: false,
                        update_protocol: protocol,
                    })
                    .unwrap();
                *probed.lock() = true;
            });
            let result = transport::preflight_sessions_in(directory.path());
            if protocol == 0 {
                assert!(result
                    .unwrap_err()
                    .to_string()
                    .contains("session session uses an older server"));
            } else {
                result.unwrap();
            }
            probed.wait_until_described(
                "session capability probe",
                yinz_test_support::default_timeout(),
                |seen| seen.then_some(()),
                |seen| format!("probed={seen}"),
            );
        });
        assert_eq!(
            broker.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

#[test]
fn occupied_private_update_endpoint_refuses_before_starting_an_executable() {
    use std::io::Read;
    use std::os::unix::net::UnixListener;
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("occupied.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let marker = directory.path().join("started");
    let executable = directory.path().join("yinz");
    yinz_test_support::write_warm_script(
        &executable,
        &format!(
            "#!/bin/sh\n[ \"$1\" != --server ] || : > '{}'\nexit 0\n",
            marker.display()
        ),
    );
    let error = coordinator::start_server_at(&executable, &socket)
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("already occupied"));
    assert!(
        !marker.exists(),
        "the existing owner must be checked before exec"
    );
    let (mut connection, _) = listener.accept().unwrap();
    let mut bytes = Vec::new();
    connection.read_to_end(&mut bytes).unwrap();
    assert!(
        bytes.is_empty(),
        "an unrelated listener must receive no initialization or protocol commands"
    );
    assert!(socket.exists());
}

#[test]
fn manual_client_commit_requires_exact_installation_and_current_running_image() {
    let expected = Installation {
        host: "host".into(),
        executable: "/installed/yinz".into(),
        data_dir: "/installed/data".into(),
        running_version: "1.0.0".into(),
        installed_version: "2.0.0".into(),
        target: "x86_64-unknown-linux-gnu".into(),
        capabilities: capabilities(),
    };
    let target = update::Release {
        tag_name: "2.0.0".into(),
        assets: vec![],
        source: None,
    };
    assert!(coordinator::manual_commit_matches(&target, &expected, &expected, false).is_err());
    let mut current = expected.clone();
    current.running_version = "2.0.0".into();
    assert!(coordinator::manual_commit_matches(&target, &expected, &current, false).unwrap());
    current.host = "different-host".into();
    assert!(!coordinator::manual_commit_matches(&target, &expected, &current, false).unwrap());
    current.host = expected.host.clone();
    current.data_dir = "/another/data".into();
    assert!(!coordinator::manual_commit_matches(&target, &expected, &current, false).unwrap());
    current.data_dir = expected.data_dir.clone();
    current.installed_version = "1.0.0".into();
    assert!(coordinator::manual_commit_matches(&target, &expected, &current, false).is_err());
}

#[test]
fn staged_update_refuses_an_old_executable_without_capabilities() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("yinz");
    yinz_test_support::write_warm_script(&executable, "#!/bin/sh\nexit 1\n");
    assert!(validate_capabilities(&executable)
        .unwrap_err()
        .to_string()
        .contains("does not support"));
}

#[test]
fn staged_update_requires_each_contract_independently() {
    for field in ["client_server", "ssh_bootstrap", "update", "migration"] {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("yinz");
        let mut peer = serde_json::to_value(capabilities()).unwrap();
        peer[field] = serde_json::json!(999);
        let script = format!("#!/bin/sh\nprintf '%s\\n' '{}'\n", peer);
        yinz_test_support::write_warm_script(&executable, &script);
        assert!(validate_capabilities(&executable).is_err(), "{field}");
    }
}

#[test]
fn staged_update_contracts_allow_different_release_labels() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("yinz");
    let mut peer = capabilities();
    peer.build_version = "a-different-release".into();
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' '{}'\n",
        serde_json::to_string(&peer).unwrap()
    );
    yinz_test_support::write_warm_script(&executable, &script);
    validate_capabilities(&executable).unwrap();
}

#[test]
fn manual_source_commit_requires_pinned_digest_and_source_identity_capability() {
    let mut attached = Installation {
        host: "host".into(),
        executable: "/installed/yinz".into(),
        data_dir: "/installed/data".into(),
        running_version: "2.0.0".into(),
        installed_version: "2.0.0".into(),
        target: "x86_64-unknown-linux-gnu".into(),
        capabilities: capabilities(),
    };
    let expected = attached.clone();
    let release = update::Release {
        tag_name: "2.0.0".into(),
        assets: vec![],
        source: Some(update::SourceRelease {
            commit: "a".repeat(40),
            artifacts: vec![yinz_types::update_client::SourceArtifact {
                target: attached.target.clone(),
                size: 1,
                sha256: "b".repeat(64),
                binary_sha256: "c".repeat(64),
            }],
        }),
    };
    attached.capabilities.source_update = true;
    for digest in [None, Some("d".repeat(64)), Some("b".repeat(64))] {
        attached.capabilities.source_binary_sha256 = digest;
        assert!(
            coordinator::manual_commit_matches(&release, &expected, &attached, true)
                .unwrap_err()
                .to_string()
                .contains("pinned source executable")
        );
    }
    attached.capabilities.source_binary_sha256 = Some("c".repeat(64));
    assert!(coordinator::manual_commit_matches(&release, &expected, &attached, true).unwrap());
    assert!(coordinator::manual_commit_matches(&release, &expected, &attached, false).is_err());
    attached.capabilities.source_update = false;
    assert!(coordinator::manual_commit_matches(&release, &expected, &attached, true).is_err());
    attached.capabilities.source_update = true;
    attached.target = "aarch64-unknown-linux-gnu".into();
    assert!(
        coordinator::manual_commit_matches(&release, &expected, &attached, true)
            .unwrap_err()
            .to_string()
            .contains("no pinned source artifact")
    );
    attached.target = expected.target.clone();
    let client = |id: u32| yinz_types::session_update::Client {
        id: id.into(),
        supports_update: true,
        supports_source_update: true,
        resumed: None,
        configuration: String::new(),
        mode: Default::default(),
    };
    for clients in [[client(1), client(2)], [client(2), client(1)]] {
        for bad_digest in [None, Some("d".repeat(64))] {
            let candidates =
                coordinator::manual_commit_candidates(&release, &expected, &clients, |id| {
                    let mut identity = attached.clone();
                    if id == 2.into() {
                        identity.capabilities.source_binary_sha256 = bad_digest.clone();
                    }
                    Ok(identity)
                });
            assert!(
                candidates.is_err(),
                "a valid peer must never hide an old executable"
            );
        }
        assert_eq!(
            coordinator::manual_commit_candidates(&release, &expected, &clients, |_| Ok(
                attached.clone()
            ))
            .unwrap(),
            clients.iter().map(|client| client.id).collect::<Vec<_>>()
        );
    }
    attached.host = "unrelated".into();
    assert!(!coordinator::manual_commit_matches(&release, &expected, &attached, false).unwrap());
}
