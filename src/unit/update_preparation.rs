use super::*;
use interprocess::local_socket::traits::Listener;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use yinz_ipc::ipc::{ClientToServerMsg, IpcReceiverWithContext, IpcRecv, ServerToClientMsg};
use yinz_types::session_migration::{MigrationPreparation, PreparationWork};

struct Server {
    socket: PathBuf,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    commands: yinz_test_support::Watched<Vec<Command>>,
}

impl Server {
    fn new(socket: PathBuf, inventory: Inventory, stall_reply: bool) -> Self {
        let listener = yinz_ipc::consts::ipc_bind(&socket).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let commands = yinz_test_support::Watched::new(Vec::new());
        let received = commands.clone();
        let worker = std::thread::spawn(move || {
            std::thread::scope(|scope| {
                loop {
                    let stream = listener.accept().unwrap();
                    if stopping.load(Ordering::Acquire) {
                        break;
                    }
                    let inventory = &inventory;
                    let received = &received;
                    scope.spawn(move || {
                        let mut receiver = IpcReceiverWithContext::<ClientToServerMsg>::new(stream);
                        receiver
                            .set_recv_timeout(Some(yinz_test_support::default_timeout()))
                            .unwrap();
                        let mut sender = receiver.get_sender::<ServerToClientMsg>().unwrap();
                        let mut preparations = 0;
                        loop {
                            match receiver.recv_client_msg() {
                                IpcRecv::Msg(ClientToServerMsg::ConnStatus, _) => {
                                    sender
                                        .send_server_msg(ServerToClientMsg::Connected {
                                            warp_companions: false,
                                            warp_held_terminals: false,
                                            warp_agent_tracking: false,
                                            warp_terminal_state: 0,
                                            signal_quit: false,
                                            update_protocol:
                                                yinz_types::session_update::PROTOCOL_VERSION,
                                            server_version: String::new(),
                                            server_host: String::new(),
                                        })
                                        .unwrap();
                                },
                                IpcRecv::Msg(ClientToServerMsg::SessionUpdate(request), _) => {
                                    received.lock().push(request.command.clone());
                                    let value = match request.command {
                                        Command::PrepareUpdate => {
                                            preparations += 1;
                                            // Withhold a later reply to check that RPC time also
                                            // counts against the preparation deadline.
                                            if stall_reply && preparations > 1 {
                                                continue;
                                            }
                                            serde_json::to_value(MigrationPreparation::Waiting(
                                                vec![
                                                    PreparationWork::PaneTransfer,
                                                    PreparationWork::HostReply,
                                                ],
                                            ))
                                            .unwrap()
                                        },
                                        Command::Identity => {
                                            serde_json::to_value(server_identity(inventory))
                                                .unwrap()
                                        },
                                        Command::Inventory => {
                                            serde_json::to_value(inventory).unwrap()
                                        },
                                        Command::Notify { .. } | Command::AbortMigration => {
                                            serde_json::Value::Null
                                        },
                                        command => panic!(
                                            "unexpected preparation/recovery command: {command:?}"
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
                                IpcRecv::Msg(ClientToServerMsg::ClientExited, _) | IpcRecv::Eof => {
                                    break
                                },
                                _ => panic!("unexpected coordinator message or transport failure"),
                            }
                        }
                    });
                }
            })
        });
        Self {
            socket,
            stop,
            worker: Some(worker),
            commands,
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = yinz_ipc::consts::ipc_connect(&self.socket);
        self.worker.take().unwrap().join().unwrap();
    }
}

#[test]
fn stalled_update_preparation_reports_pending_work_and_runs_failure_cleanup() {
    for stall_reply in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("session");
        let mut store =
            Store::open(&socket, root.path(), &uuid::Uuid::new_v4().to_string()).unwrap();
        let inventory = Inventory {
            session: "session".into(),
            server_pid: std::process::id(),
            server_instance: "source".into(),
            executable: root.path().join("yinz"),
            running_version: "1.0.0".into(),
            running_binary_sha256: None,
            assets: Default::default(),
            configuration: serde_json::json!(""),
            themes: None,
            clients: vec![],
        };
        let server = Server::new(socket.clone(), inventory.clone(), stall_reply);
        let mut source = Connection::connect(&socket).unwrap();
        let old_timeout = PREPARATION_TIMEOUT.replace(Duration::from_secs(1));
        let started = Instant::now();
        let result = execute_release(
            &mut store,
            &mut source,
            inventory,
            update::Release {
                channel: ReleaseChannel::Stable,
                tag_name: "2.0.0".into(),
                assets: vec![],
                source: None,
            },
        );
        PREPARATION_TIMEOUT.set(old_timeout);
        let error = format!("{:#}", result.unwrap_err());
        assert!(error.contains("preparation timed out"), "{error}");
        assert!(
            error.contains("pane transfers, terminal host replies"),
            "{error}"
        );
        assert!(started.elapsed() < yinz_test_support::default_timeout());
        assert_eq!(store.journal.phase, Phase::Failed);
        assert!(store.journal.source_restored);
        assert!(store
            .journal
            .errors
            .iter()
            .any(|error| error.contains("preparation timed out")));
        let commands = server.commands.lock();
        assert!(commands.iter().any(|command| matches!(command, Command::Notify { text } if text.contains("pane transfers, terminal host replies"))));
        assert!(commands
            .iter()
            .any(|command| matches!(command, Command::AbortMigration)));
        assert!(!commands
            .iter()
            .any(|command| matches!(command, Command::ReleaseMigration { .. })));
    }
}

/// An installation that another, unfinished update has already prepared, as when a remote
/// session's update publishes a client host's installation before that host's own session
/// starts updating.
struct ForeignInstallation {
    _directory: tempfile::TempDir,
    executable: PathBuf,
    data_dir: PathBuf,
}

impl ForeignInstallation {
    const OWNER: &str = "06f0f3ef-e66c-4b97-a1c1-6c00f74edb45";

    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let release = directory.path().join("release");
        std::fs::create_dir_all(release.join("plugins/builtins")).unwrap();
        std::fs::write(release.join("yinz"), b"foreign binary").unwrap();
        std::fs::write(release.join("plugins/builtins/main.scm"), b"foreign plugin").unwrap();
        let executable = directory.path().join("yinz");
        std::fs::write(&executable, b"installed binary").unwrap();
        let data_dir = directory.path().join("data");
        std::fs::create_dir_all(data_dir.join("plugins/builtins")).unwrap();
        std::fs::write(
            data_dir.join("plugins/builtins/main.scm"),
            b"installed plugin",
        )
        .unwrap();
        let mut foreign = update::transaction::Transaction::prepare(
            &release,
            &executable,
            &data_dir,
            Self::OWNER,
        )
        .unwrap();
        foreign.publish().unwrap();
        Self {
            _directory: directory,
            executable: executable.canonicalize().unwrap(),
            data_dir,
        }
    }

    fn assert_untouched(&self) {
        assert_eq!(std::fs::read(&self.executable).unwrap(), b"foreign binary");
        assert_eq!(
            std::fs::read(self.data_dir.join("plugins/builtins/main.scm")).unwrap(),
            b"foreign plugin"
        );
        let foreign =
            update::transaction::Transaction::lookup(&self.executable, &self.data_dir, Self::OWNER)
                .unwrap()
                .expect("the other update still owns its installation");
        assert_eq!(foreign.status(), update::transaction::Status::Published);
    }
}

/// Recovers a session update whose single installation another update owns, starting from the
/// journal a failed preparation leaves behind with its participant in `state`.
fn recover_installation_owned_by_another_update(state: &str) -> (Store, Result<()>) {
    let installation = ForeignInstallation::new();
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("session");
    let operation = uuid::Uuid::new_v4().to_string();
    // This is how the participant's preparation fails.
    let release = root.path().join("release");
    std::fs::create_dir_all(release.join("plugins/builtins")).unwrap();
    std::fs::write(release.join("yinz"), b"new binary").unwrap();
    let error = update::transaction::Transaction::prepare(
        &release,
        &installation.executable,
        &installation.data_dir,
        &operation,
    )
    .err()
    .expect("another unfinished update owns the installation");
    assert!(
        format!("{error:#}").contains(&format!(
            "installation belongs to unfinished update {}",
            ForeignInstallation::OWNER
        )),
        "{error:#}"
    );
    let mut store = Store::open(&socket, root.path(), &operation).unwrap();
    let inventory = Inventory {
        session: "session".into(),
        server_pid: std::process::id(),
        server_instance: "source".into(),
        executable: installation.executable.clone(),
        running_version: "1.0.0".into(),
        running_binary_sha256: None,
        assets: Default::default(),
        configuration: serde_json::json!(""),
        themes: None,
        clients: vec![],
    };
    store.journal.source = Some(inventory.clone());
    store.journal.participants.push(Participant {
        installation: Installation {
            host: "host".into(),
            executable: installation.executable.clone(),
            data_dir: installation.data_dir.clone(),
            running_version: "1.0.0".into(),
            installed_version: "1.0.0".into(),
            target: "target".into(),
            capabilities: crate::live_update::capabilities(),
        },
        route: Route::Server,
        state: state.into(),
    });
    store
        .journal
        .errors
        .push(format!("Preparation failed: {error:#}"));
    store.phase(Phase::Prepare).unwrap();
    let server = Server::new(socket.clone(), inventory, false);
    let mut source = Connection::connect(&socket).unwrap();
    let result = recover(&mut store, &mut source, None);
    installation.assert_untouched();
    if result.is_ok() {
        assert!(server
            .commands
            .lock()
            .iter()
            .any(|command| matches!(command, Command::AbortMigration)));
    }
    (store, result)
}

#[test]
fn failed_preparation_recovery_leaves_another_updates_installation_and_reopens_the_source() {
    let (store, result) = recover_installation_owned_by_another_update("preparing");
    result.unwrap();
    assert_eq!(store.journal.phase, Phase::Failed);
    assert!(store.journal.source_restored);
    assert_eq!(store.journal.participants[0].state, "rolled_back");
}

#[test]
fn confirmed_preparation_recovery_still_refuses_another_updates_installation() {
    // Only an unconfirmed preparation may conclude it staged nothing here.
    let (store, result) = recover_installation_owned_by_another_update("prepared");
    let error = format!("{:#}", result.unwrap_err());
    assert!(
        error.contains(&format!(
            "installation journal belongs to operation {}",
            ForeignInstallation::OWNER
        )),
        "{error}"
    );
    assert_eq!(store.journal.phase, Phase::RecoveryRequired);
    assert!(!store.journal.source_restored);
    assert_eq!(store.journal.participants[0].state, "prepared");
}
