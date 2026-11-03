//! Real processes, supervisors, and sockets exercise the complete ownership handoff.
use super::*;
use std::{path::Path, time::Instant};
use yinz_ipc::ipc::IpcReceiverWithContext;
use yinz_types::{
    session_migration::{SessionMigrationInventory, SessionMigrationMap},
    session_update::{Command as UpdateCommand, Inventory, Request},
};
#[path = "update_previous_release.rs"]
mod previous_release;

struct Peer {
    receiver: IpcReceiverWithContext<ServerToClientMsg>,
    sender: IpcSenderWithContext<ClientToServerMsg>,
}
impl Peer {
    fn connect(path: &Path) -> Self {
        let stream = yinz_ipc::consts::ipc_connect(path).unwrap();
        let receiver = IpcReceiverWithContext::new(stream);
        let sender = receiver.get_sender().unwrap();
        Self { receiver, sender }
    }
    fn request(
        &mut self,
        operation: &str,
        command: UpdateCommand,
    ) -> Result<serde_json::Value, String> {
        let token = uuid::Uuid::new_v4().to_string();
        self.sender
            .send_client_msg(ClientToServerMsg::SessionUpdate(Request {
                token: token.clone(),
                operation: operation.into(),
                command,
            }))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            match self.receiver.recv_server_msg_until(deadline) {
                IpcRecv::Msg(ServerToClientMsg::SessionUpdateReply(reply), _)
                    if reply.token == token =>
                {
                    return reply.result
                },
                IpcRecv::Msg(_, _) => {},
                IpcRecv::Eof => panic!("update server disconnected before replying"),
                IpcRecv::Idle => panic!("update server reply timed out"),
                IpcRecv::Malformed => panic!("malformed update server reply"),
            }
        }
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.sender.send_client_msg(ClientToServerMsg::ClientExited);
    }
}

fn setup() -> (Session, Vec<i32>) {
    setup_with_configuration("detachable_panes true\nload_plugins clear-defaults=true\n")
}

fn setup_with_configuration(configuration: &str) -> (Session, Vec<i32>) {
    let session = Session::new();
    yinz_steel_builtins::install(
        &yinz_steel_builtins::asset_dir(),
        &session.0.path().join("data/yinz"),
    )
    .unwrap();
    fs::write(session.0.path().join("config.kdl"), configuration).unwrap();
    let script = session.0.path().join("terminal");
    yinz_test_support::write_warm_script(&script,"#!/bin/sh\n[ \"$1\" = run ] || exit 0\necho $$ > \"$2\"\nprintf '\\033[?2004hMIGRATION-CONTENT\\n'\nexec /bin/cat\n");
    let layout=format!("layout {{ tab name=\"one\" {{ pane command=\"{}\" {{ args \"run\" \"{}\"; }}; }}; tab name=\"two\" {{ pane command=\"{}\" {{ args \"run\" \"{}\"; }}; }}; }}",script.display(),session.0.path().join("pid1").display(),script.display(),session.0.path().join("pid2").display());
    session.success(&[
        "--layout-string",
        &layout,
        "attach",
        "--create-background",
        NAME,
    ]);
    session.wait_for_panes(false);
    let observed = yinz_test_support::Watched::new((
        [
            Err(String::from("not read yet")),
            Err(String::from("not read yet")),
        ],
        false,
    ));
    let pids = std::thread::scope(|scope| {
        let observed = &observed;
        let root = session.0.path();
        scope.spawn(move || {
            // The external shell stubs publish PID files, not in-process events.
            // Portable file notifications would add a separate watcher mechanism;
            // retain bounded polling and report each file's actual contents/error.
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let pids = ["pid1", "pid2"].map(|name| {
                    fs::read_to_string(root.join(name))
                        .map_err(|error| format!("{name}: {error}"))
                        .and_then(|contents| {
                            contents
                                .trim()
                                .parse::<i32>()
                                .map_err(|error| format!("{name}: {contents:?}: {error}"))
                                .and_then(|pid| {
                                    if pid > 0 {
                                        Ok(pid)
                                    } else {
                                        Err(format!("{name}: invalid PID {pid}"))
                                    }
                                })
                        })
                });
                let ready = pids.iter().all(Result::is_ok);
                let expired = Instant::now() >= deadline;
                *observed.lock() = (pids, expired);
                if ready || expired {
                    return;
                }
                std::thread::park_timeout(Duration::from_millis(10));
            }
        });
        let (pids, _) = observed.wait_until_described(
            "both terminal PID files to contain valid process IDs",
            Duration::from_secs(30),
            |state| (state.0.iter().all(Result::is_ok) || state.1).then(|| state.clone()),
            |state| {
                format!(
                    "pid1: {:?}; pid2: {:?}; deadline expired: {}",
                    state.0[0], state.0[1], state.1
                )
            },
        );
        assert!(
            pids.iter().all(Result::is_ok),
            "timed out after 30s waiting for terminal PID files: {pids:?}"
        );
        pids.into_iter().map(Result::unwrap).collect()
    });
    (session, pids)
}

fn target(session: &Session, source: &Inventory, operation: &str) -> (Peer, PathBuf) {
    let socket = session.socket().with_file_name("private-update");
    let mut cmd = session.command();
    assert!(cmd.arg("--server").arg(&socket).status().unwrap().success());
    let mut peer = Peer::connect(&socket);
    let mut assets = source.assets.clone();
    assets.update_target = Some(operation.into());
    assets.update_source_session = Some(source.session.clone().into());
    assets.update_configuration = Some(source.configuration.as_str().unwrap().to_owned());
    assets.configuration_options = None;
    assets.layout = None;
    peer.sender
        .send_client_msg(ClientToServerMsg::FirstClientConnected { cli_assets: assets })
        .unwrap();
    peer.request(operation, UpdateCommand::Inventory).unwrap();
    (peer, socket)
}

fn assert_alive(pids: &[i32]) {
    for pid in pids {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(*pid), None)
            .expect("terminal process identity must survive migration");
    }
}

fn assert_server_exited(pid: u32) {
    let exited = yinz_test_support::Watched::new(Ok(false));
    std::thread::scope(|scope| {
        let exited = &exited;
        scope.spawn(move || {
            // This server is not our child, so waitpid cannot observe its exit.
            // There is no portable exit notification for an arbitrary PID; poll
            // liveness within the existing deadline and publish the actual result.
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let state = yinz_process::liveness::has_exited(pid)
                    .map_err(|error| format!("server PID {}: {error}", pid));
                let done = !matches!(state, Ok(false));
                *exited.lock() = state;
                if done {
                    return;
                }
                if Instant::now() >= deadline {
                    *exited.lock() = Err(format!(
                        "timed out waiting for server PID {} to exit; last observed alive",
                        pid
                    ));
                    return;
                }
                std::thread::park_timeout(Duration::from_millis(10));
            }
        });
        exited
            .wait_until_described(
                "retired migration server to exit",
                Duration::from_secs(30),
                |state| (!matches!(state, Ok(false))).then(|| state.clone()),
                |state| format!("server PID {}: {state:?}", pid),
            )
            .expect("retired migration server to exit");
    });
}

/// These are real builtin-owned SQLite rows. An empty intermediate pane/tab
/// manifest would make agent-service prune them; checking only terminal state
/// cannot detect that loss.
struct PersistentBindings {
    host: String,
    latch: yinz_types::plugin_data::DataRows,
    assignment: yinz_types::plugin_data::DataRows,
}

impl PersistentBindings {
    fn context(session: &Session) -> serde_json::Value {
        serde_json::from_slice(
            &session
                .success(&["--session", NAME, "action", "context", "--pane-id", "0"])
                .stdout,
        )
        .unwrap()
    }

    fn rows(
        session: &Session,
        host: &str,
        entity: &str,
        attributes: &[&str],
    ) -> yinz_types::plugin_data::DataRows {
        use yinz_types::plugin_data::{FetchRequest, Pred};
        yinz_storage::plugin_data_store::PluginDataStore::new(&session.0.path().join("data/yinz"))
            .fetch(
                "shared:yinz",
                &FetchRequest::entity(entity)
                    .filter(Pred::eq("host_id", host).and(Pred::eq("yinz_session_name", NAME)))
                    .select(attributes.iter().copied()),
            )
            .unwrap()
    }

    fn seed(session: &Session) -> Self {
        let context = Self::context(session);
        let host = context["host_id"].as_str().unwrap().to_owned();
        let tab = context["tab_id"].as_u64().unwrap().to_string();
        session.success(&[
            "--session",
            NAME,
            "action",
            "project-remember",
            "migration-project",
            "--root",
            session.0.path().to_str().unwrap(),
        ]);
        session.success(&[
            "--session",
            NAME,
            "action",
            "project-assign",
            "migration-project",
            "--tab-id",
            &tab,
        ]);
        session.success(&["--session", NAME, "agent", "register", "0", "reviewer"]);
        let latch = Self::rows(session, &host, "agent_latch", &["uid", "pane_id", "kind"]);
        let assignment = Self::rows(
            session,
            &host,
            "agent_assignment",
            &["uid", "tab_id", "project_id", "project"],
        );
        assert_eq!(latch.len(), 1);
        assert_eq!(assignment.len(), 1);
        let bindings = Self {
            host,
            latch,
            assignment,
        };
        bindings.assert_preserved(session);
        bindings
    }

    fn assert_preserved(&self, session: &Session) {
        // The RPC runs on the actual resumed builtin worker, after its queued
        // lifecycle events, and proves its in-memory assignment was restored.
        assert_eq!(Self::context(session)["project"], "migration-project");
        assert_eq!(
            Self::rows(
                session,
                &self.host,
                "agent_latch",
                &["uid", "pane_id", "kind"]
            ),
            self.latch
        );
        assert_eq!(
            Self::rows(
                session,
                &self.host,
                "agent_assignment",
                &["uid", "tab_id", "project_id", "project"]
            ),
            self.assignment
        );
    }
}

fn migration(rollback: bool) {
    let (session, pids) = setup_with_configuration(
        "detachable_panes true\nplugins { project-store location=\"yinz:project-store\" { shared true; }; agent-service location=\"yinz:agent-service\" { shared true; }; }\nload_plugins clear-defaults=true { project-store; agent-service; }\n"
    );
    let bindings = PersistentBindings::seed(&session);
    let operation = uuid::Uuid::new_v4().to_string();
    let mut source = Peer::connect(&session.socket());
    let inventory: Inventory = serde_json::from_value(
        source
            .request(&operation, UpdateCommand::Inventory)
            .unwrap(),
    )
    .unwrap();
    let before: SessionMigrationInventory = serde_json::from_value(
        source
            .request(&operation, UpdateCommand::PrepareMigration)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(before.tabs.len(), 2);
    let snapshot = session.0.path().join("snapshot.json");
    source
        .request(
            &operation,
            UpdateCommand::ReleaseMigration {
                snapshot: snapshot.clone(),
            },
        )
        .unwrap();
    assert_alive(&pids);
    // The last tab has left, but the source must stay alive for rollback and client replacement.
    assert_eq!(
        source
            .request(&operation, UpdateCommand::Inventory)
            .unwrap()["server_pid"],
        inventory.server_pid
    );
    let (mut destination, socket) = target(&session, &inventory, &operation);
    let mapping: SessionMigrationMap = serde_json::from_value(
        destination
            .request(
                &operation,
                UpdateCommand::AdoptMigration {
                    snapshot: snapshot.clone(),
                },
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(mapping.tabs.len(), 2);
    assert_eq!(mapping.panes.len(), 2);
    assert_alive(&pids);
    if rollback {
        destination
            .request(&operation, UpdateCommand::AbortMigration)
            .unwrap();
        source
            .request(&operation, UpdateCommand::AdoptMigration { snapshot })
            .unwrap();
        source
            .request(&operation, UpdateCommand::FinishMigration)
            .unwrap();
        bindings.assert_preserved(&session);
        assert_alive(&pids);
        let restored: SessionMigrationInventory = serde_json::from_value(
            source
                .request(&operation, UpdateCommand::PrepareMigration)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(restored.tabs.len(), 2);
        let destination_pid = destination
            .request(&operation, UpdateCommand::Inventory)
            .unwrap()["server_pid"]
            .as_u64()
            .unwrap() as u32;
        destination
            .sender
            .send_client_msg(ClientToServerMsg::KillSession)
            .unwrap();
        assert_server_exited(destination_pid);
        bindings.assert_preserved(&session);
        drop(destination);
        let _ = socket;
    } else {
        source
            .request(
                &operation,
                UpdateCommand::RenameSession {
                    name: "retired-update".into(),
                },
            )
            .unwrap();
        destination
            .request(
                &operation,
                UpdateCommand::RenameSession { name: NAME.into() },
            )
            .unwrap();
        destination
            .request(&operation, UpdateCommand::FinishMigration)
            .unwrap();
        bindings.assert_preserved(&session);
        assert_alive(&pids);
        let after: SessionMigrationInventory = serde_json::from_value(
            destination
                .request(&operation, UpdateCommand::PrepareMigration)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            after
                .tabs
                .iter()
                .map(|t| t.title.as_str())
                .collect::<Vec<_>>(),
            vec!["one", "two"]
        );
        source
            .sender
            .send_client_msg(ClientToServerMsg::KillSession)
            .unwrap();
        assert_server_exited(inventory.server_pid);
        bindings.assert_preserved(&session);
        drop(destination);
    }
}

#[test]
fn live_session_update_preserves_terminal_processes_and_session_name() {
    migration(false);
}

#[test]
fn live_session_update_rolls_back_after_destination_adoption() {
    migration(true);
}

#[test]
fn live_update_retries_partial_source_readoption_after_transient_supervisor_failure() {
    let (session, pids) = setup();
    let operation = uuid::Uuid::new_v4().to_string();
    let mut source = Peer::connect(&session.socket());
    let path = session.0.path().join("snapshot.json");
    source
        .request(
            &operation,
            UpdateCommand::ReleaseMigration {
                snapshot: path.clone(),
            },
        )
        .unwrap();
    let snapshot: yinz_types::session_migration::SessionMigrationSnapshot =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let socket = &snapshot.tabs[1].payload.tiled_panes[0].control_socket;
    let hidden = socket.with_extension("temporarily-unreachable");
    fs::rename(socket, &hidden).unwrap();
    let failed = source.request(
        &operation,
        UpdateCommand::AdoptMigration {
            snapshot: path.clone(),
        },
    );
    fs::rename(&hidden, socket).unwrap();
    assert!(
        failed.is_err(),
        "the second supervisor must be unavailable for the first attempt"
    );
    assert_alive(&pids);
    source
        .request(&operation, UpdateCommand::AdoptMigration { snapshot: path })
        .unwrap();
    source
        .request(&operation, UpdateCommand::FinishMigration)
        .unwrap();
    assert_alive(&pids);
    let restored: SessionMigrationInventory = serde_json::from_value(
        source
            .request(&operation, UpdateCommand::PrepareMigration)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(restored.tabs.len(), 2);
}

#[test]
fn live_update_preserves_sparse_terminal_identities_and_next_allocation() {
    let (session, pids) = setup();
    session.success(&[
        "--session",
        NAME,
        "action",
        "close-pane",
        "--pane-id",
        "terminal_0",
    ]);
    session.success(&["--session", NAME, "run", "--", "/bin/cat"]);
    session.success(&[
        "--session",
        NAME,
        "action",
        "close-pane",
        "--pane-id",
        "terminal_2",
    ]);
    session.success(&["--session", NAME, "run", "--", "/bin/cat"]);
    session.wait_for_panes(false);
    let operation = uuid::Uuid::new_v4().to_string();
    let mut source = Peer::connect(&session.socket());
    let inventory: Inventory = serde_json::from_value(
        source
            .request(&operation, UpdateCommand::Inventory)
            .unwrap(),
    )
    .unwrap();
    let before: SessionMigrationInventory = serde_json::from_value(
        source
            .request(&operation, UpdateCommand::PrepareMigration)
            .unwrap(),
    )
    .unwrap();
    let panes: Vec<_> = before
        .tabs
        .iter()
        .flat_map(|tab| tab.panes.iter().copied())
        .collect();
    assert!(panes.contains(&yinz_types::pane_id::PaneId::Terminal(
        yinz_types::internal_ids::TerminalId::new(1)
    )));
    assert!(panes.contains(&yinz_types::pane_id::PaneId::Terminal(
        yinz_types::internal_ids::TerminalId::new(3)
    )));
    let snapshot = session.0.path().join("sparse.json");
    source
        .request(
            &operation,
            UpdateCommand::ReleaseMigration {
                snapshot: snapshot.clone(),
            },
        )
        .unwrap();
    let (mut destination, _) = target(&session, &inventory, &operation);
    let mapping: SessionMigrationMap = serde_json::from_value(
        destination
            .request(&operation, UpdateCommand::AdoptMigration { snapshot })
            .unwrap(),
    )
    .unwrap();
    assert!(
        mapping
            .panes
            .iter()
            .all(|(source, destination)| source == destination),
        "inherited pane IDs and durable agent bindings must stay valid: {mapping:?}"
    );
    assert!(
        mapping
            .tabs
            .iter()
            .all(|(source, destination)| source == destination),
        "durable tab assignments must retain their identities: {mapping:?}"
    );
    source
        .request(
            &operation,
            UpdateCommand::RenameSession {
                name: "retired-sparse".into(),
            },
        )
        .unwrap();
    destination
        .request(
            &operation,
            UpdateCommand::RenameSession { name: NAME.into() },
        )
        .unwrap();
    destination
        .request(&operation, UpdateCommand::FinishMigration)
        .unwrap();
    assert_alive(&pids[1..]);
    session.success(&["--session", NAME, "run", "--", "/bin/cat"]);
    let after: SessionMigrationInventory = serde_json::from_value(
        destination
            .request(&operation, UpdateCommand::PrepareMigration)
            .unwrap(),
    )
    .unwrap();
    assert!(after
        .tabs
        .iter()
        .flat_map(|tab| &tab.panes)
        .any(|pane| *pane
            == yinz_types::pane_id::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(
                4
            ))));
    source
        .sender
        .send_client_msg(ClientToServerMsg::KillSession)
        .unwrap();
    assert_server_exited(inventory.server_pid);
}

#[test]
fn retired_source_duplicate_update_returns_the_original_active_operation() {
    let (session, pids) = setup();
    let operation = uuid::Uuid::new_v4().to_string();
    let data = session.0.path().join("data/yinz");
    let directory = data.join("updates").join(&operation);
    fs::create_dir_all(&directory).unwrap();
    let active = yinz_runtime::session_update::record(&data, &session.socket()).unwrap();
    fs::write(&active, serde_json::to_vec(&operation).unwrap()).unwrap();
    fs::write(
        directory.join("journal.json"),
        serde_json::to_vec(&serde_json::json!({"operation": operation, "phase": "Transfer"}))
            .unwrap(),
    )
    .unwrap();
    // Model the independent coordinator's claim while this source moves to its
    // private endpoint. A duplicate Start must return this operation, not spawn
    // another updater keyed by the temporary session name.
    let coordinator = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(active.with_extension("lock"))
        .unwrap();
    coordinator.lock().unwrap();
    let mut source = Peer::connect(&session.socket());
    let snapshot = directory.join("snapshot.json");
    source
        .request(
            &operation,
            UpdateCommand::ReleaseMigration {
                snapshot: snapshot.clone(),
            },
        )
        .unwrap();
    let mut watcher = Peer::connect(&session.socket());
    watcher
        .sender
        .send_client_msg(ClientToServerMsg::AttachWatcherClient {
            terminal_size: yinz_types::pane_size::Size { rows: 24, cols: 80 },
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match watcher.receiver.recv_server_msg_until(deadline) {
            IpcRecv::Msg(ServerToClientMsg::Exit { exit_reason }, _) => {
                assert!(
                    exit_reason.to_string().contains("transferring terminals"),
                    "{exit_reason:?}"
                );
                break;
            },
            // The router may release its input barrier before the main thread
            // answers the attach. This is not an attachment acknowledgement.
            IpcRecv::Msg(ServerToClientMsg::UnblockInputThread, _) => {},
            IpcRecv::Msg(other, _) => {
                panic!("watcher received {other:?} instead of transfer refusal")
            },
            IpcRecv::Eof => panic!("watcher closed without transfer refusal"),
            IpcRecv::Idle => panic!("watcher transfer refusal timed out"),
            IpcRecv::Malformed => panic!("watcher received malformed refusal"),
        }
    }
    source
        .request(
            &operation,
            UpdateCommand::RenameSession {
                name: format!(".retired-{operation}"),
            },
        )
        .unwrap();
    let status = source
        .request(&uuid::Uuid::new_v4().to_string(), UpdateCommand::Start)
        .unwrap();
    assert_eq!(status["operation"], operation);
    assert_eq!(status["phase"], "Transfer");
    source
        .request(&operation, UpdateCommand::AdoptMigration { snapshot })
        .unwrap();
    source
        .request(
            &operation,
            UpdateCommand::RenameSession { name: NAME.into() },
        )
        .unwrap();
    source
        .request(&operation, UpdateCommand::FinishMigration)
        .unwrap();
    assert_alive(&pids);
}

#[test]
fn live_update_preserves_layout_defaults_for_new_tabs() {
    let session = Session::new();
    yinz_steel_builtins::install(
        &yinz_steel_builtins::asset_dir(),
        &session.0.path().join("data/yinz"),
    )
    .unwrap();
    fs::write(
        session.0.path().join("config.kdl"),
        "detachable_panes true\nload_plugins clear-defaults=true\n",
    )
    .unwrap();
    session.success(&[
        "--layout-string",
        r#"
        layout {
            default_tab_template { pane name="custom-default" command="/bin/cat"; children; }
            tab name="existing" { pane command="/bin/cat"; }
            swap_tiled_layout name="custom tiled" {
                tab max_panes=4 { pane split_direction="vertical" { pane; pane; }; }
            }
            swap_floating_layout name="custom floating" {
                floating_panes max_panes=2 { pane x=1 y=2 width=30 height=10; }
            }
        }
    "#,
        "attach",
        "--create-background",
        NAME,
    ]);
    session.wait_for_panes(false);
    let operation = uuid::Uuid::new_v4().to_string();
    let mut source = Peer::connect(&session.socket());
    let inventory: Inventory = serde_json::from_value(
        source
            .request(&operation, UpdateCommand::Inventory)
            .unwrap(),
    )
    .unwrap();
    source
        .request(&operation, UpdateCommand::PrepareMigration)
        .unwrap();
    let snapshot = session.0.path().join("layout-snapshot.json");
    source
        .request(
            &operation,
            UpdateCommand::ReleaseMigration {
                snapshot: snapshot.clone(),
            },
        )
        .unwrap();
    let (mut destination, _socket) = target(&session, &inventory, &operation);
    let restored: Inventory = serde_json::from_value(
        destination
            .request(&operation, UpdateCommand::Inventory)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        restored.assets.update_layout,
        inventory.assets.update_layout
    );
    destination
        .request(&operation, UpdateCommand::AdoptMigration { snapshot })
        .unwrap();
    source
        .request(
            &operation,
            UpdateCommand::RenameSession {
                name: "retired-update".into(),
            },
        )
        .unwrap();
    destination
        .request(
            &operation,
            UpdateCommand::RenameSession { name: NAME.into() },
        )
        .unwrap();
    destination
        .request(&operation, UpdateCommand::FinishMigration)
        .unwrap();
    session.success(&[
        "--session",
        NAME,
        "action",
        "new-tab",
        "--name",
        "after-update",
    ]);
    let panes: serde_json::Value = serde_json::from_slice(
        &session
            .success(&["--session", NAME, "action", "list-panes", "--json"])
            .stdout,
    )
    .unwrap();
    assert!(
        panes
            .as_array()
            .unwrap()
            .iter()
            .any(|pane| pane["title"] == "custom-default" && pane["tab_name"] == "after-update"),
        "{panes:#}"
    );
    source
        .sender
        .send_client_msg(ClientToServerMsg::KillSession)
        .unwrap();
    assert_server_exited(inventory.server_pid);
}
