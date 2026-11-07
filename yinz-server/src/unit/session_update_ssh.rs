//! Whole-session migration through both real SSH topologies.
use super::*;
use yinz_ipc::ipc::{
    ClientToServerMsg, IpcReceiverWithContext, IpcRecv, IpcSenderWithContext, ServerToClientMsg,
};
use yinz_types::session_migration::{SessionMigrationInventory, SessionMigrationMap};
use yinz_types::session_update::{Command as Update, Inventory, Request};
use yinz_types::update_client::{
    ClientReply, ClientRequest, ParticipantRequest, Request as ClientUpdateRequest,
};

// Stage exactly the production transaction without release downloads or a
// production-only fixture switch. The candidate is this worktree's own binary.
#[path = "../../../src/update/transaction.rs"]
mod update_transaction;

struct Peer {
    reader: IpcReceiverWithContext<ServerToClientMsg>,
    writer: IpcSenderWithContext<ClientToServerMsg>,
    _forward: Option<yinz_process::ssh::HostForward>,
}
impl Peer {
    fn connect(socket: &Path, far: bool, root: &Path) -> Self {
        let local = root.join(format!("peer-{}.sock", uuid::Uuid::new_v4().simple()));
        let forward = far.then(|| {
            yinz_process::ssh::HostForward::start(
                HOST,
                &Default::default(),
                &local,
                socket.to_str().unwrap(),
                None,
            )
            .unwrap()
        });
        let stream = yinz_ipc::consts::ipc_connect(if far { &local } else { socket }).unwrap();
        let reader = IpcReceiverWithContext::new(stream);
        let writer = reader.get_sender().unwrap();
        Self {
            reader,
            writer,
            _forward: forward,
        }
    }
    fn request(&mut self, operation: &str, command: Update) -> serde_json::Value {
        let token = uuid::Uuid::new_v4().to_string();
        self.writer
            .send_client_msg(ClientToServerMsg::SessionUpdate(Request {
                token: token.clone(),
                operation: operation.into(),
                command,
            }))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            match self.reader.recv_server_msg_until(deadline) {
                IpcRecv::Msg(ServerToClientMsg::SessionUpdateReply(reply), _)
                    if reply.token == token =>
                {
                    return reply.result.unwrap()
                },
                IpcRecv::Msg(_, _) => {},
                IpcRecv::Idle => panic!("migration peer reply timed out"),
                IpcRecv::Eof => panic!("migration peer disconnected"),
                IpcRecv::Malformed => panic!("migration peer sent an invalid reply"),
            }
        }
    }
    fn client(
        &mut self,
        operation: &str,
        client: yinz_types::data::ClientId,
        request: ClientRequest,
    ) -> ClientReply {
        serde_json::from_value(self.request(
            operation,
            Update::Client {
                client,
                request: ClientUpdateRequest {
                    token: uuid::Uuid::new_v4().to_string(),
                    request,
                },
            },
        ))
        .unwrap()
    }
    fn participant(
        &mut self,
        operation: &str,
        client: yinz_types::data::ClientId,
        request: ParticipantRequest,
    ) -> serde_json::Value {
        let token = uuid::Uuid::new_v4().to_string();
        let mut request = ClientUpdateRequest {
            token: token.clone(),
            request: ClientRequest::Participant(request),
        };
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            match serde_json::from_value(
                self.request(operation, Update::Client { client, request }),
            )
            .unwrap()
            {
                ClientReply::Participant(value) => return value,
                ClientReply::Pending => {},
                other => panic!("unexpected participant reply: {other:?}"),
            }
            assert!(
                Instant::now() < deadline,
                "SSH client participant did not finish"
            );
            request = ClientUpdateRequest {
                token: uuid::Uuid::new_v4().to_string(),
                request: ClientRequest::Poll {
                    token: token.clone(),
                },
            };
        }
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.writer.send_client_msg(ClientToServerMsg::ClientExited);
    }
}
struct Session {
    root: tempfile::TempDir,
    remote_root: String,
    name: String,
    far: bool,
    config: PathBuf,
    socket: PathBuf,
    names: Vec<String>,
}
impl Session {
    fn command(&self, arguments: &[&str]) -> String {
        if self.far {
            let mut args = vec!["yinz", "--config", self.config.to_str().unwrap()];
            args.extend_from_slice(arguments);
            remote_exec(&args)
        } else {
            let output = Command::new(repo_root().join("target/debug/yinz"))
                .env("YINZ_SOCKET_DIR", self.root.path().join("s"))
                .env("XDG_CONFIG_HOME", self.root.path().join("config"))
                .env("XDG_DATA_HOME", self.root.path().join("data"))
                .env("YINZ_TEST_CACHE_DIR", self.root.path().join("cache"))
                .env_remove("YINZ")
                .env_remove("YINZ_SESSION_NAME")
                .env_remove("YINZ_PANE_ID")
                .env_remove("YINZ_PANE_REF")
                .arg("--config")
                .arg(&self.config)
                .arg("--data-dir")
                .arg(remote_test_dir().join("data"))
                .args(arguments)
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{arguments:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        }
    }
    fn new(far: bool) -> Self {
        let _hosts = start();
        let root = tempfile::Builder::new()
            .prefix("ym-")
            .tempdir_in("/tmp")
            .unwrap();
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let name = format!("migration-{}", &suffix[..12]);
        let remote_root = format!("/tmp/yinz-migration-{suffix}");
        remote_sh(&format!("mkdir -p {}", shell_quote(&remote_root)));
        let config = if far {
            PathBuf::from(&remote_root).join("config.kdl")
        } else {
            root.path().join("config.kdl")
        };
        let text = "detachable_panes true\nload_plugins clear-defaults=true\n";
        if far {
            remote_sh(&format!(
                "printf %s {} > {}",
                shell_quote(text),
                shell_quote(config.to_str().unwrap())
            ));
        } else {
            std::fs::write(&config, text).unwrap();
        }
        let mut session = Self {
            root,
            remote_root,
            name: name.clone(),
            far,
            config,
            socket: PathBuf::new(),
            names: vec![name],
        };
        let script = format!("echo $$ > {}/pid; trap 'printf DETACHED_REPLAY_MARKER\\\\n' USR1; printf MIGRATION_SSH_READY\\\\n; while :; do IFS= read -r line || continue; printf 'MIGRATION_ECHO:%s\\n' \"$line\"; done", session.remote_root);
        let pane = format!(
            "pane command=\"/bin/sh\" {} {{ args \"-c\" {}; }}",
            if far {
                String::new()
            } else {
                format!("ssh={HOST:?}")
            },
            serde_json::to_string(&script).unwrap()
        );
        let layout = format!("layout {{ tab name=\"ssh-preserved\" {{ {pane}; }}; }}");
        session.command(&[
            "--layout-string",
            &layout,
            "attach",
            "--create-background",
            &session.name,
        ]);
        session.socket = if far {
            let result = remote_exec(&[
                "yinz",
                "remote-session",
                "--client-version",
                yinz_runtime::consts::VERSION,
                "--contract-version",
                &yinz_runtime::consts::CLIENT_SERVER_CONTRACT_VERSION.to_string(),
                "--bootstrap-version",
                &yinz_runtime::consts::REMOTE_BOOTSTRAP_VERSION.to_string(),
                "--",
                &session.name,
            ]);
            let endpoint: serde_json::Value = serde_json::from_str(&result).unwrap();
            PathBuf::from(endpoint["socket"].as_str().unwrap())
        } else {
            session
                .root
                .path()
                .join("s")
                .join(&*yinz_runtime::consts::CLIENT_SERVER_CONTRACT_DIR)
                .join(&session.name)
        };
        session.wait_for_text("MIGRATION_SSH_READY");
        session
    }
    fn wait_for_text(&self, text: &str) {
        let observed = yinz_test_support::Watched::new(String::new());
        std::thread::scope(|scope| {
            let observed = &observed;
            scope.spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(60);
                while Instant::now() < deadline {
                    let output = self.command(&[
                        "--session",
                        &self.name,
                        "action",
                        "dump-screen",
                        "--pane-id",
                        "terminal_0",
                    ]);
                    let ready = output.contains(text);
                    *observed.lock() = output;
                    if ready {
                        return;
                    }
                    std::thread::park_timeout(Duration::from_millis(30));
                }
            });
            observed.wait_until_described(
                text,
                Duration::from_secs(60),
                |output| output.contains(text).then_some(()),
                Clone::clone,
            );
        });
    }
    fn pid(&self) -> String {
        remote_sh(&format!("cat {}/pid", shell_quote(&self.remote_root)))
            .trim()
            .to_owned()
    }
    fn assert_pid(&self, pid: &str) {
        assert_eq!(self.pid(), pid);
        remote_exec(&["kill", "-0", pid]);
    }
    fn replacement(&mut self, source: &Inventory, operation: &str) -> Peer {
        let name = format!(".update-{}", &operation[..12]);
        self.names.push(name.clone());
        let socket = self.socket.with_file_name(name);
        self.command(&["--server", socket.to_str().unwrap()]);
        let mut peer = Peer::connect(&socket, self.far, self.root.path());
        let mut assets = source.assets.clone();
        assets.update_target = Some(operation.into());
        assets.update_source_session = Some(source.session.clone().into());
        assets.update_configuration = Some(source.configuration.as_str().unwrap().into());
        assets.update_themes = source.themes.clone();
        assets.configuration_options = None;
        assets.layout = None;
        peer.writer
            .send_client_msg(ClientToServerMsg::FirstClientConnected { cli_assets: assets })
            .unwrap();
        peer.request(operation, Update::Inventory);
        peer
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        for name in &self.names {
            // Each name belongs to this test. A removed private endpoint is expected.
            if self.far {
                let _ = remote_sh(&format!(
                    "yinz --config {} kill-session {} >/dev/null 2>&1 || true",
                    shell_quote(self.config.to_str().unwrap()),
                    shell_quote(name)
                ));
            } else {
                let _ = Command::new(repo_root().join("target/debug/yinz"))
                    .env("YINZ_SOCKET_DIR", self.root.path().join("s"))
                    .args(["kill-session", name])
                    .output();
            }
        }
        remote_sh(&format!("rm -rf -- {}", shell_quote(&self.remote_root)));
    }
}

#[test]
#[ignore = "requires the Docker SSH fixture"]
fn local_session_remote_pane_update_and_rollback_keep_pid_and_output() {
    let mut session = Session::new(false);
    let operation = uuid::Uuid::new_v4().to_string();
    let mut source = Peer::connect(&session.socket, false, session.root.path());
    let inventory: Inventory =
        serde_json::from_value(source.request(&operation, Update::Inventory)).unwrap();
    let before: SessionMigrationInventory =
        serde_json::from_value(source.request(&operation, Update::PrepareMigration)).unwrap();
    assert_eq!(before.remote_hosts, vec![HOST]);
    let pid = session.pid();
    let path = session.root.path().join("snapshot.json");
    source.request(
        &operation,
        Update::ReleaseMigration {
            snapshot: path.clone(),
        },
    );
    remote_exec(&["kill", "-USR1", &pid]);
    let mut destination = session.replacement(&inventory, &operation);
    destination.request(
        &operation,
        Update::AdoptMigration {
            snapshot: path.clone(),
        },
    );
    destination.request(&operation, Update::AbortMigration);
    source.request(&operation, Update::AdoptMigration { snapshot: path });
    source.request(&operation, Update::FinishMigration);
    session.wait_for_text("DETACHED_REPLAY_MARKER");
    session.assert_pid(&pid);
}

#[test]
#[ignore = "requires the Docker SSH fixture"]
fn ssh_client_remote_session_update_keeps_terminal_and_reconnects() {
    use std::os::unix::fs::MetadataExt;
    let mut session = Session::new(true);
    let binary = session.root.path().join("client-yinz");
    std::fs::copy(repo_root().join("target/debug/yinz"), &binary).unwrap();
    let mut client = super::remote_client_ssh::Client::start_update(HOST, &session.name, &binary);
    client.see_screen("MIGRATION_SSH_READY");
    let client_pid = client.pid();
    let old_inode = std::fs::metadata(&binary).unwrap().ino();
    let old_forwards = client.forward_paths();
    assert!(!old_forwards.is_empty());
    yinz_steel_builtins::install(&yinz_steel_builtins::asset_dir(), &client.data_dir()).unwrap();
    let operation = uuid::Uuid::new_v4().to_string();
    let mut source = Peer::connect(&session.socket, true, session.root.path());
    let inventory: Inventory =
        serde_json::from_value(source.request(&operation, Update::Inventory)).unwrap();
    assert_eq!(inventory.clients.len(), 1);
    assert!(inventory.clients[0].supports_update);
    let original_client = inventory.clients[0].id;
    let installation: yinz_types::session_update::Installation = serde_json::from_value(
        source.participant(&operation, original_client, ParticipantRequest::Inventory),
    )
    .unwrap();
    assert_eq!(installation.executable, binary.canonicalize().unwrap());
    assert_eq!(
        installation.data_dir,
        client.data_dir().canonicalize().unwrap()
    );
    let candidate = session.root.path().join("client-release");
    std::fs::create_dir(&candidate).unwrap();
    std::fs::copy(&binary, candidate.join("yinz")).unwrap();
    yinz_steel_builtins::install(&yinz_steel_builtins::asset_dir(), &candidate).unwrap();
    drop(
        update_transaction::Transaction::prepare(
            &candidate,
            &binary,
            &client.data_dir(),
            &operation,
        )
        .unwrap(),
    );
    assert_eq!(
        source.participant(
            &operation,
            original_client,
            ParticipantRequest::Publish {
                operation: operation.clone()
            }
        ),
        serde_json::json!("published")
    );
    let new_inode = std::fs::metadata(&binary).unwrap().ino();
    assert_ne!(old_inode, new_inode);
    let pid = session.pid();
    let path = PathBuf::from(&session.remote_root).join("snapshot.json");
    source.request(
        &operation,
        Update::ReleaseMigration {
            snapshot: path.clone(),
        },
    );
    remote_exec(&["kill", "-USR1", &pid]);
    let mut destination = session.replacement(&inventory, &operation);
    destination.request(
        &operation,
        Update::RestoreClients {
            clients: inventory.clients.clone(),
        },
    );
    let mapping: SessionMigrationMap = serde_json::from_value(
        destination.request(&operation, Update::AdoptMigration { snapshot: path }),
    )
    .unwrap();
    let retired = format!(".retired-{}", &operation[..12]);
    session.names.push(retired.clone());
    source.request(&operation, Update::RenameSession { name: retired });
    destination.request(
        &operation,
        Update::RenameSession {
            name: session.name.clone(),
        },
    );
    destination.request(&operation, Update::FinishMigration);
    assert_eq!(
        source.client(
            &operation,
            original_client,
            ClientRequest::Restart {
                operation: operation.clone(),
                source_client: original_client,
                executable: installation.executable,
                target: yinz_types::data::SessionConnection {
                    name: Some(session.name.clone().into()),
                    pane_id: Some(mapping.panes[0].1),
                    ..Default::default()
                },
            },
        ),
        ClientReply::Restarting
    );
    let resumed = yinz_test_support::Watched::new(None);
    std::thread::scope(|scope| {
        let resumed = &resumed;
        let destination = &mut destination;
        let operation = &operation;
        scope.spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(60);
            while Instant::now() < deadline {
                let inventory: Inventory =
                    serde_json::from_value(destination.request(operation, Update::Inventory))
                        .unwrap();
                if let Some(client) = inventory
                    .clients
                    .iter()
                    .find(|client| client.resumed == Some((operation.clone(), original_client)))
                {
                    *resumed.lock() = Some(client.id);
                    return;
                }
            }
        });
        resumed.wait_until(
            "remote replacement client readiness",
            Duration::from_secs(60),
            |id| id.map(|_| ()),
        );
    });
    assert_eq!(client.pid(), client_pid);
    #[cfg(target_os = "linux")]
    assert_eq!(
        std::fs::metadata(format!("/proc/{client_pid}/exe"))
            .unwrap()
            .ino(),
        new_inode
    );
    assert_ne!(client.forward_paths(), old_forwards);
    client.see_screen("DETACHED_REPLAY_MARKER");
    client.send(b"AFTER_SSH_MIGRATION\r");
    client.see_screen("MIGRATION_ECHO:AFTER_SSH_MIGRATION");
    session.assert_pid(&pid);
    assert_eq!(
        destination.participant(
            &operation,
            resumed.lock().unwrap(),
            ParticipantRequest::Commit {
                operation: operation.clone()
            }
        ),
        serde_json::json!("committed")
    );
    client.detach();
}

#[test]
#[ignore = "requires Docker SSH fixture and checksum-verified YINZ_PREVIOUS_RELEASE binary"]
fn previous_release_remote_broker_keeps_pid_through_release_resume_and_return() {
    let first = start();
    let previous = std::env::var_os("YINZ_PREVIOUS_RELEASE")
        .map(PathBuf::from)
        .expect("set YINZ_PREVIOUS_RELEASE to the checksum-verified published executable");
    let previous = previous.canonicalize().unwrap();
    let target = repo_root().join("target").canonicalize().unwrap();
    let remote_binary = Path::new("/usr/src/yinz").join(previous.strip_prefix(target).unwrap());
    for pid in remote_pids("pty-broker serve") {
        remote_exec(&["kill", &pid.to_string()]);
    }
    wait_for_remote("fixture broker to exit", || {
        remote_pids("pty-broker serve").is_empty()
    });
    let endpoint = remote_exec(&[remote_binary.to_str().unwrap(), "--pty-broker", "ensure"]);
    let endpoint: ssh::BrokerEndpoint = serde_json::from_str(&endpoint).unwrap();
    assert_eq!(endpoint.protocol_version, REMOTE_PROTOCOL_VERSION);
    let brokers = remote_pids("pty-broker serve");
    assert_eq!(brokers.len(), 1);
    let broker_pid = brokers[0];
    assert_eq!(
        remote_exec(&["readlink", &format!("/proc/{broker_pid}/exe")]),
        remote_binary.to_str().unwrap()
    );
    struct RetireBroker(u32);
    impl Drop for RetireBroker {
        fn drop(&mut self) {
            remote_exec(&["kill", &self.0.to_string()]);
        }
    }
    let _broker = RetireBroker(broker_pid);
    let marker = marker("previous-release-broker");
    let pane = open_pane(&first, &format!(
        "stty -echo; printf '{marker}-READY\\n'; while IFS= read -r line; do printf '{marker}:%s\\n' \"$line\"; done"
    ));
    pane.wait_for_output(&format!("{marker}-READY"));
    let pids = remote_pids(&marker);
    assert_eq!(pids.len(), 1);
    let (token, received) = release_for_warp(pane);
    let second = Arc::new(RemoteHosts::new());
    let (adopted, dropped) = adopt_elsewhere(&second, &token, received);
    assert_eq!(dropped, 0);
    adopted.send_line("AFTER_UPDATE");
    adopted.wait_for_output(&format!("{marker}:AFTER_UPDATE"));
    let (token, received) = release_for_warp(adopted);
    let (returned, dropped) = adopt_elsewhere(&first, &token, received);
    assert_eq!(dropped, 0);
    returned.send_line("AFTER_ROLLBACK");
    returned.wait_for_output(&format!("{marker}:AFTER_ROLLBACK"));
    assert_eq!(remote_pids(&marker), pids);
    assert_eq!(remote_pids("pty-broker serve"), vec![broker_pid]);
    returned.close();
    first.shutdown();
    second.shutdown();
}
