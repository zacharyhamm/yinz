//! Exercise a real attached client's terminal teardown and exec, without release-network fixtures.
use crate::update::transaction::Transaction;
use std::{
    fs::{self, File},
    io::{Read, Write},
    os::unix::{fs::MetadataExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use yinz_ipc::ipc::{
    ClientToServerMsg, IpcReceiverWithContext, IpcRecv, IpcSenderWithContext, ServerToClientMsg,
};
use yinz_test_support::Watched;
use yinz_types::{
    data::{ClientId, PaneInfo, SessionConnection},
    internal_ids::TabNumber,
    session_update::{
        Command as SessionCommand, Installation, Inventory, Request as SessionRequest,
    },
    update_client::{ClientReply, ClientRequest, ParticipantRequest, Request},
};

const SESSION: &str = "client-update";
const TIMEOUT: Duration = Duration::from_secs(60);

#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[path = "source_build_real.rs"]
mod source_build_real;

#[path = "client_update_cold.rs"]
mod cold;

#[path = "client_update_retry.rs"]
mod retry;

#[path = "client_update_plugin_recovery.rs"]
mod plugin_recovery;

#[path = "real_fixture.rs"]
mod real_fixture;

#[cfg(target_os = "linux")]
#[path = "sequential_update_ssh.rs"]
mod sequential_ssh;

struct Fixture {
    root: real_fixture::RealFixture,
    binary: PathBuf,
    data: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        Self::with_root(real_fixture::RealFixture::new("ycu-"), None)
    }
    #[cfg(target_os = "linux")]
    fn new_in(temporary: &Path, installation: Option<&Path>) -> Self {
        Self::with_root(
            real_fixture::RealFixture::new_in("ycu-", temporary),
            installation,
        )
    }
    fn with_root(root: real_fixture::RealFixture, installation: Option<&Path>) -> Self {
        for name in ["home", "config", "data", "cache", "tmp", "runtime"] {
            fs::create_dir(root.path().join(name)).unwrap();
        }
        // cargo x test builds this checkout's production binary before testing.
        let built = std::env::var_os("YINZ_COMPILATION_WORKER")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug/yinz"));
        assert!(
            built.is_file(),
            "build the worktree binary before running this test"
        );
        let binary = installation.unwrap_or(root.path()).join("yinz");
        fs::copy(built, &binary).unwrap();
        let data = installation
            .map(|directory| directory.join("data/yinz"))
            .unwrap_or_else(|| root.path().join("data"));
        yinz_steel_builtins::install(&yinz_steel_builtins::asset_dir(), &data).unwrap();
        fs::write(
            root.path().join("config.kdl"),
            "detachable_panes false\nload_plugins clear-defaults=true\ndefault_mode \"locked\"\n",
        )
        .unwrap();
        Self { root, binary, data }
    }
    fn command(&self) -> Command {
        self.command_for(&self.binary, &self.data)
    }
    fn command_for(&self, binary: &Path, data: &Path) -> Command {
        let mut command = Command::new(binary);
        self.root.inherit(&mut command);
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default());
        for (key, directory) in [
            ("HOME", "home"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_CACHE_HOME", "cache"),
            ("TMPDIR", "tmp"),
            ("XDG_RUNTIME_DIR", "runtime"),
        ] {
            command.env(key, self.root.path().join(directory));
        }
        command
            .env("YINZ_SOCKET_DIR", self.root.socket_dir())
            .env("SHELL", "/bin/sh")
            .env("TERM", "xterm-256color")
            .current_dir(self.root.path().join("home"))
            .arg("--config")
            .arg(self.root.path().join("config.kdl"))
            .arg("--config-dir")
            .arg(self.root.path().join("config"))
            .arg("--data-dir")
            .arg(data)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
    fn success(&self, args: &[&str]) -> Vec<u8> {
        let output = self.command().args(args).output().unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
        output.stdout
    }
    fn socket(&self) -> PathBuf {
        self.root
            .socket_dir()
            .join(&*yinz_runtime::consts::CLIENT_SERVER_CONTRACT_DIR)
            .join(SESSION)
    }
    fn prepare(&self, operation: &str) {
        let release = self.root.path().join("release");
        fs::create_dir(&release).unwrap();
        fs::copy(&self.binary, release.join("yinz")).unwrap();
        yinz_steel_builtins::install(&yinz_steel_builtins::asset_dir(), &release).unwrap();
        // Drop process locks while retaining the durable prepared installation.
        drop(Transaction::prepare(&release, &self.binary, &self.data, operation).unwrap());
    }
    fn start(&self) {
        let script = self.root.path().join("pane");
        yinz_test_support::write_warm_script(&script, "#!/bin/sh\n[ \"$1\" = run ] || exit 0\nshift\nprintf '%s\\n' \"$$\" > \"$2.pid\"\nprintf 'READY-%s\\n' \"$1\"\nwhile IFS= read -r line; do printf '%s\\n' \"$line\" >> \"$2\"; printf 'ECHO-%s:%s\\n' \"$1\" \"$line\"; done\n");
        let layout = format!("layout {{ tab name=\"first\" {{ pane name=\"first\" command=\"{}\" {{ args \"run\" \"first\" \"{}\"; }}; }}; tab name=\"second\" {{ pane name=\"second\" command=\"{}\" {{ args \"run\" \"second\" \"{}\"; }}; }}; }}", script.display(), self.root.path().join("first-input").display(), script.display(), self.root.path().join("second-input").display());
        self.success(&[
            "--layout-string",
            &layout,
            "attach",
            "--create-background",
            SESSION,
        ]);
        // The background command returns after sending startup assets, before
        // the server has finished initializing. An immediate interactive attach
        // can exhaust its short session probe and exit without ever attaching.
        // Wait for the actual main-loop reply, then verify initialized inventory.
        let mut source =
            crate::live_update::transport::Connection::connect(&self.socket()).unwrap();
        source
            .request(&uuid::Uuid::new_v4().to_string(), SessionCommand::Inventory)
            .unwrap();
    }
}

struct Peer {
    receiver: IpcReceiverWithContext<ServerToClientMsg>,
    sender: IpcSenderWithContext<ClientToServerMsg>,
}
impl Peer {
    fn connect(path: &Path) -> Self {
        let receiver = IpcReceiverWithContext::new(yinz_ipc::consts::ipc_connect(path).unwrap());
        let sender = receiver.get_sender().unwrap();
        Self { receiver, sender }
    }
    fn request(
        &mut self,
        operation: &str,
        command: SessionCommand,
    ) -> Result<serde_json::Value, String> {
        let token = uuid::Uuid::new_v4().to_string();
        self.sender
            .send_client_msg(ClientToServerMsg::SessionUpdate(SessionRequest {
                token: token.clone(),
                operation: operation.into(),
                command,
            }))
            .unwrap();
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match self.receiver.recv_server_msg_until(deadline) {
                IpcRecv::Msg(ServerToClientMsg::SessionUpdateReply(reply), _)
                    if reply.token == token =>
                {
                    return reply.result
                },
                IpcRecv::Msg(ServerToClientMsg::UpdateRequest(request), _) => panic!(
                    "legacy/command peer received an unsupported update request: {request:?}"
                ),
                IpcRecv::Msg(_, _) => {},
                _ => panic!("session update reply did not arrive"),
            }
        }
    }
    fn inventory(&mut self, operation: &str) -> Inventory {
        serde_json::from_value(self.request(operation, SessionCommand::Inventory).unwrap()).unwrap()
    }
    fn client(&mut self, operation: &str, client: ClientId, request: ClientRequest) -> ClientReply {
        serde_json::from_value(
            self.request(
                operation,
                SessionCommand::Client {
                    client,
                    request: Request {
                        token: uuid::Uuid::new_v4().to_string(),
                        request,
                    },
                },
            )
            .unwrap(),
        )
        .unwrap()
    }
    fn participant(
        &mut self,
        operation: &str,
        client: ClientId,
        request: ParticipantRequest,
    ) -> serde_json::Value {
        let token = uuid::Uuid::new_v4().to_string();
        let mut request = Request {
            token: token.clone(),
            request: ClientRequest::Participant(request),
        };
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let response: ClientReply = serde_json::from_value(
                self.request(operation, SessionCommand::Client { client, request })
                    .unwrap(),
            )
            .unwrap();
            match response {
                ClientReply::Participant(value) => return value,
                ClientReply::Pending => {},
                other => panic!("unexpected participant reply: {other:?}"),
            }
            assert!(Instant::now() < deadline, "participant did not finish");
            request = Request {
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
        let _ = self.sender.send_client_msg(ClientToServerMsg::ClientExited);
    }
}

struct Attached {
    child: Child,
    master: File,
    slave: Option<File>,
    screen: Watched<Vec<u8>>,
    capture: Option<std::thread::JoinHandle<()>>,
    original: nix::sys::termios::Termios,
}
impl Attached {
    fn start(fixture: &Fixture) -> Self {
        let mut command = fixture.command();
        command.args(["attach", SESSION]);
        Self::start_command(command)
    }
    fn start_command(mut command: Command) -> Self {
        use nix::fcntl::{fcntl, FcntlArg, FdFlag};
        let pty = nix::pty::openpty(
            Some(&nix::pty::Winsize {
                ws_row: 24,
                ws_col: 90,
                ws_xpixel: 0,
                ws_ypixel: 0,
            }),
            None,
        )
        .unwrap();
        for fd in [&pty.master, &pty.slave] {
            fcntl(fd, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC)).unwrap();
        }
        let slave = File::from(pty.slave);
        let original = nix::sys::termios::tcgetattr(&slave).unwrap();
        let master = File::from(pty.master);
        let mut reader = master.try_clone().unwrap();
        let screen = Watched::new(Vec::new());
        let captured = screen.clone();
        let capture = std::thread::spawn(move || {
            let mut buffer = [0; 4096];
            while let Ok(count) = reader.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                captured.lock().extend_from_slice(&buffer[..count]);
            }
        });
        command
            .stdin(slave.try_clone().unwrap())
            .stdout(slave.try_clone().unwrap())
            .stderr(slave.try_clone().unwrap());
        // SAFETY: setsid is async-signal-safe and touches no allocation or shared state.
        unsafe {
            command.pre_exec(|| {
                nix::unistd::setsid()
                    .map(|_| ())
                    .map_err(std::io::Error::from)
            });
        }
        let child = command.spawn().unwrap();
        Self {
            child,
            master,
            slave: Some(slave),
            screen,
            capture: Some(capture),
            original,
        }
    }
    fn wait_for(&self, expected: &str) {
        self.screen.wait_until(expected, TIMEOUT, |screen| {
            String::from_utf8_lossy(screen)
                .contains(expected)
                .then_some(())
        });
    }
}
impl Drop for Attached {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.slave.take();
        if let Some(capture) = self.capture.take() {
            let _ = capture.join();
        }
        if std::thread::panicking() {
            let screen = self.screen.lock();
            let tail = &screen[screen.len().saturating_sub(16 * 1024)..];
            eprintln!(
                "terminal tail ({} total bytes): {}",
                screen.len(),
                String::from_utf8_lossy(tail)
            );
        }
    }
}

#[test]
fn publication_skips_disconnected_shared_client_but_acknowledges_connected_client() {
    use crate::live_update::{
        coordinator,
        journal::{Participant, Route, Store},
        transport::Connection,
    };
    let fixture = Fixture::new();
    fixture.start();
    let disconnected = Attached::start(&fixture);
    disconnected.wait_for("READY-first");
    let operation = uuid::Uuid::new_v4().to_string();
    let mut peer = Peer::connect(&fixture.socket());
    let before = peer.inventory(&operation);
    let departed = before
        .clients
        .iter()
        .find(|client| client.supports_update)
        .unwrap()
        .id;
    let installation: Installation = serde_json::from_value(peer.participant(
        &operation,
        departed,
        ParticipantRequest::Inventory,
    ))
    .unwrap();
    let connected = Attached::start(&fixture);
    connected.wait_for("READY-first");
    let attached = peer.inventory(&operation);
    let remaining = attached
        .clients
        .iter()
        .find(|client| client.supports_update && client.id != departed)
        .unwrap()
        .id;
    fixture.prepare(&operation);
    drop(disconnected);
    let frozen = Watched::new(None);
    std::thread::scope(|scope| {
        let frozen = &frozen;
        let peer = &mut peer;
        let operation = &operation;
        scope.spawn(move || {
            let deadline = Instant::now() + TIMEOUT;
            while Instant::now() < deadline {
                let inventory = peer.inventory(operation);
                let done = !inventory.clients.iter().any(|client| client.id == departed);
                *frozen.lock() = Some(inventory);
                if done {
                    return;
                }
            }
        });
        frozen.wait_until_described(
            "disconnected client absent from frozen inventory",
            TIMEOUT,
            |inventory| {
                inventory
                    .as_ref()
                    .filter(|inventory| {
                        !inventory.clients.iter().any(|client| client.id == departed)
                    })
                    .cloned()
            },
            |inventory| format!("inventory: {inventory:?}"),
        );
    });
    let mut store = Store::open(&fixture.socket(), &fixture.data, &operation).unwrap();
    store.journal.disconnected_clients.push(departed);
    store.journal.participants = [
        Route::Server,
        Route::Client(departed),
        Route::Client(remaining),
    ]
    .into_iter()
    .map(|route| Participant {
        installation: installation.clone(),
        route,
        state: "prepared".into(),
    })
    .collect();
    let mut source = Connection::connect(&fixture.socket()).unwrap();
    coordinator::publish_participants(&mut store, &mut source).unwrap();
    assert!(store
        .journal
        .participants
        .iter()
        .all(|participant| participant.state == "published"));
    // This validation runs after the client's publication check. It proves the
    // surviving client acknowledged Publish even though its installation was shared.
    let error = peer
        .request(
            &operation,
            SessionCommand::Client {
                client: remaining,
                request: Request {
                    token: uuid::Uuid::new_v4().to_string(),
                    request: ClientRequest::Restart {
                        operation: operation.clone(),
                        source_client: remaining,
                        executable: installation.executable.clone(),
                        target: SessionConnection::default(),
                    },
                },
            },
        )
        .unwrap_err();
    assert!(error.contains("restart target must name"), "{error}");

    // A disconnected client with its own installation still needs publication.
    // Skipping its acknowledgement must never label that installation published.
    let mut separate = installation;
    separate.data_dir = fixture.root.path().join("separate-data");
    store.journal.participants.push(Participant {
        installation: separate,
        route: Route::Client(departed),
        state: "prepared".into(),
    });
    assert!(coordinator::publish_participants(&mut store, &mut source).is_err());
    assert_eq!(
        store.journal.participants.last().unwrap().state,
        "publishing"
    );
}

#[test]
fn attached_client_update_reexecs_published_inode_preserving_input_focus_and_terminal() {
    let fixture = Fixture::new();
    fixture.start();
    let mut attached = Attached::start(&fixture);
    attached.wait_for("READY-first");
    let operation = uuid::Uuid::new_v4().to_string();
    let mut peer = Peer::connect(&fixture.socket());
    let before = peer.inventory(&operation);
    let client = before
        .clients
        .iter()
        .find(|client| client.supports_update)
        .unwrap()
        .id;
    let panes: Vec<PaneInfo> = serde_json::from_slice(&fixture.success(&[
        "--session",
        SESSION,
        "action",
        "list-panes",
        "--json",
    ]))
    .unwrap();
    let pane = panes.iter().find(|pane| pane.title == "second").unwrap().id;
    let installation: Installation =
        serde_json::from_value(peer.participant(&operation, client, ParticipantRequest::Inventory))
            .unwrap();
    assert_eq!(installation.data_dir, fixture.data.canonicalize().unwrap());
    assert_eq!(
        installation.executable,
        fixture.binary.canonicalize().unwrap()
    );
    assert_eq!(installation.running_version, yinz_runtime::consts::VERSION);
    let old_inode = fs::metadata(&fixture.binary).unwrap().ino();
    fixture.prepare(&operation);
    assert_eq!(
        peer.participant(
            &operation,
            client,
            ParticipantRequest::Publish {
                operation: operation.clone()
            }
        ),
        serde_json::json!("published")
    );
    let new_inode = fs::metadata(&fixture.binary).unwrap().ino();
    assert_ne!(old_inode, new_inode);
    #[cfg(target_os = "linux")]
    assert_eq!(
        fs::metadata(format!("/proc/{}/exe", attached.child.id()))
            .unwrap()
            .ino(),
        old_inode
    );
    // Reattachment must use the captured effective configuration even when its
    // original file is no longer readable by the replacement client.
    fs::remove_file(fixture.root.path().join("config.kdl")).unwrap();
    assert_eq!(
        peer.client(
            &operation,
            client,
            ClientRequest::Restart {
                operation: operation.clone(),
                source_client: client,
                executable: installation.executable,
                target: SessionConnection {
                    name: Some(SESSION.into()),
                    tab_position: Some(TabNumber::new(2)),
                    pane_id: Some(pane),
                    ..Default::default()
                },
            }
        ),
        ClientReply::Restarting
    );
    attached.wait_for("READY-second");
    let reattached = peer
        .inventory(&operation)
        .clients
        .into_iter()
        .find(|client| client.supports_update)
        .unwrap()
        .id;
    // This test keeps the server and its panes in place. Supply the ordered
    // post-restore barrier that the migration path emits after its final render.
    assert_eq!(
        peer.client(
            &operation,
            reattached,
            ClientRequest::Ready {
                operation: operation.clone(),
                source_client: client,
            }
        ),
        ClientReply::Resumed {
            operation: operation.clone(),
            source_client: client
        }
    );
    let observed = Watched::new(None);
    std::thread::scope(|scope| {
        let observed = &observed;
        let operation = &operation;
        let peer = &mut peer;
        scope.spawn(move || {
            let deadline = Instant::now() + TIMEOUT;
            while Instant::now() < deadline {
                let inventory = peer.inventory(operation);
                if let Some(resumed) = inventory
                    .clients
                    .iter()
                    .find(|entry| entry.resumed == Some((operation.clone(), client)))
                {
                    *observed.lock() = Some(resumed.id);
                    return;
                }
            }
        });
        observed.wait_until(
            "replacement client first-render acknowledgment",
            TIMEOUT,
            |client| client.map(|_| ()),
        );
    });
    let resumed = observed.lock().unwrap();
    #[cfg(target_os = "linux")]
    assert_eq!(
        fs::metadata(format!("/proc/{}/exe", attached.child.id()))
            .unwrap()
            .ino(),
        new_inode
    );
    attached.wait_for("READY-second");
    attached.master.write_all(b"after-reexec\n").unwrap();
    attached.wait_for("ECHO-second:after-reexec");
    assert_eq!(
        fs::read_to_string(fixture.root.path().join("second-input")).unwrap(),
        "after-reexec\n"
    );
    assert!(!fixture.root.path().join("first-input").exists());
    peer.sender
        .send_client_msg(ClientToServerMsg::DetachSession {
            client_ids: vec![resumed],
        })
        .unwrap();
    let exited = Watched::new(false);
    std::thread::scope(|scope| {
        let exited = &exited;
        let child = &mut attached.child;
        scope.spawn(move || {
            let deadline = Instant::now() + TIMEOUT;
            while Instant::now() < deadline {
                if let Some(status) = child.try_wait().unwrap() {
                    assert!(status.success());
                    *exited.lock() = true;
                    return;
                }
                std::thread::yield_now();
            }
        });
        exited.wait_until("updated client to detach cleanly", TIMEOUT, |done| {
            done.then_some(())
        });
    });
    assert_eq!(
        nix::sys::termios::tcgetattr(attached.slave.as_ref().unwrap()).unwrap(),
        attached.original
    );
    Transaction::open(&fixture.binary, &fixture.data, &operation)
        .unwrap()
        .commit()
        .unwrap();
}

#[test]
fn session_update_refuses_legacy_client_without_sending_unknown_messages() {
    let fixture = Fixture::new();
    fixture.start();
    let operation = uuid::Uuid::new_v4().to_string();
    let mut peer = Peer::connect(&fixture.socket());
    let before = peer.inventory(&operation);
    let mut legacy = Peer::connect(&fixture.socket());
    legacy
        .sender
        .send_client_msg(ClientToServerMsg::AttachClient {
            cli_assets: yinz_types::input::cli_assets::CliAssets {
                config_file_path: Some(fixture.root.path().join("config.kdl")),
                terminal_window_size: yinz_types::pane_size::Size { rows: 24, cols: 90 },
                ..Default::default()
            },
            tab_number_to_focus: None,
            pane_to_focus: None,
        })
        .unwrap();
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match legacy.receiver.recv_server_msg_until(deadline) {
            IpcRecv::Msg(ServerToClientMsg::Render { .. }, _) => break,
            IpcRecv::Msg(_, _) => {},
            _ => panic!("legacy client did not attach"),
        }
    }
    let after = peer.inventory(&operation);
    let client = after
        .clients
        .iter()
        .find(|client| {
            !before
                .clients
                .iter()
                .any(|previous| previous.id == client.id)
        })
        .unwrap();
    assert!(!client.supports_update);
    let refusal = peer
        .request(
            &operation,
            SessionCommand::Client {
                client: client.id,
                request: Request {
                    token: uuid::Uuid::new_v4().to_string(),
                    request: ClientRequest::Participant(ParticipantRequest::Inventory),
                },
            },
        )
        .unwrap_err();
    assert!(
        refusal.contains(yinz_types::update_client::UNSUPPORTED_CLIENT),
        "{refusal}"
    );
    // A same-connection inventory reply is an ordering fence after the refused route.
    legacy.inventory(&operation);
    peer.request(
        &operation,
        SessionCommand::DisconnectUpdateClient { client: client.id },
    )
    .unwrap();
    loop {
        match legacy
            .receiver
            .recv_server_msg_until(Instant::now() + TIMEOUT)
        {
            IpcRecv::Msg(ServerToClientMsg::Exit { .. }, _) | IpcRecv::Eof => break,
            IpcRecv::Msg(ServerToClientMsg::UpdateRequest(_), _) => {
                panic!("legacy client received an unsupported update message")
            },
            IpcRecv::Msg(_, _) => {},
            IpcRecv::Idle => panic!("legacy client did not disconnect before the deadline"),
            IpcRecv::Malformed => panic!("malformed reply during legacy client disconnection"),
        }
    }
    // Exit is sent before the server removes the client. Inventory requests use
    // another router and can overtake that cleanup, so observe removal itself.
    let removed = Watched::new(None);
    std::thread::scope(|scope| {
        let removed = &removed;
        let peer = &mut peer;
        let operation = &operation;
        let client_id = client.id;
        scope.spawn(move || {
            let deadline = Instant::now() + TIMEOUT;
            while Instant::now() < deadline {
                let inventory = peer.inventory(operation);
                let done = !inventory
                    .clients
                    .iter()
                    .any(|current| current.id == client_id);
                *removed.lock() = Some(inventory);
                if done {
                    return;
                }
            }
        });
        removed.wait_until_described(
            "legacy client removed after Exit",
            TIMEOUT,
            |inventory| {
                inventory
                    .as_ref()
                    .filter(|inventory| {
                        !inventory
                            .clients
                            .iter()
                            .any(|current| current.id == client_id)
                    })
                    .map(|_| ())
            },
            |inventory| format!("inventory: {inventory:?}"),
        );
    });
}

#[test]
fn failed_client_exec_reconnects_the_old_process_without_a_false_ready_ack() {
    let fixture = Fixture::new();
    fixture.start();
    let mut attached = Attached::start(&fixture);
    attached.wait_for("READY-first");
    let operation = uuid::Uuid::new_v4().to_string();
    let mut peer = Peer::connect(&fixture.socket());
    let client = peer
        .inventory(&operation)
        .clients
        .into_iter()
        .find(|client| client.supports_update)
        .unwrap()
        .id;
    let panes: Vec<PaneInfo> = serde_json::from_slice(&fixture.success(&[
        "--session",
        SESSION,
        "action",
        "list-panes",
        "--json",
    ]))
    .unwrap();
    let pane = panes.iter().find(|pane| pane.title == "second").unwrap().id;
    let old_inode = fs::metadata(&fixture.binary).unwrap().ino();
    fixture.prepare(&operation);
    peer.participant(
        &operation,
        client,
        ParticipantRequest::Publish {
            operation: operation.clone(),
        },
    );
    // Metadata still validates as an executable. The missing interpreter fails
    // only at exec, after the old client has disconnected and restored its TTY.
    fs::write(
        &fixture.binary,
        b"#!/definitely-missing-yinz-test-interpreter\n",
    )
    .unwrap();
    assert_eq!(
        peer.client(
            &operation,
            client,
            ClientRequest::Restart {
                operation: operation.clone(),
                source_client: client,
                executable: fixture.binary.canonicalize().unwrap(),
                target: SessionConnection {
                    name: Some(SESSION.into()),
                    tab_position: Some(TabNumber::new(2)),
                    pane_id: Some(pane),
                    ..Default::default()
                },
            }
        ),
        ClientReply::Restarting
    );
    attached.wait_for("READY-second");
    assert!(attached.child.try_wait().unwrap().is_none());
    #[cfg(target_os = "linux")]
    assert_eq!(
        fs::metadata(format!("/proc/{}/exe", attached.child.id()))
            .unwrap()
            .ino(),
        old_inode
    );
    attached.master.write_all(b"after-failed-exec\n").unwrap();
    attached.wait_for("ECHO-second:after-failed-exec");
    let inventory = peer.inventory(&operation);
    assert!(inventory
        .clients
        .iter()
        .all(|client| client.resumed.is_none()));
    assert_eq!(
        fs::read_to_string(fixture.root.path().join("second-input")).unwrap(),
        "after-failed-exec\n"
    );
    assert!(!fixture.root.path().join("first-input").exists());
    // Restore the valid installation so cleanup and later worker spawns are usable.
    Transaction::open(&fixture.binary, &fixture.data, &operation)
        .unwrap()
        .rollback()
        .unwrap();
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CoordinatorFixture {
    socket: PathBuf,
    data: PathBuf,
    operation: String,
    release: crate::update::Release,
    #[serde(default)]
    stale_clients: bool,
}

#[test]
#[ignore = "subprocess fixture with an isolated session environment"]
fn coordinator_test_worker() {
    let path =
        std::env::var_os("YINZ_TEST_UPDATE_COORDINATOR").expect("coordinator fixture request");
    let request: CoordinatorFixture = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    eprintln!("coordinator stage: open journal");
    let mut store = crate::live_update::journal::Store::open_with_channel(
        &request.socket,
        &request.data,
        &request.operation,
        request.release.source.is_some(),
        request.release.channel,
    )
    .unwrap();
    eprintln!("coordinator stage: connect source");
    let mut source = crate::live_update::transport::Connection::connect(&request.socket).unwrap();
    eprintln!("coordinator stage: inventory source");
    let mut inventory: Inventory = serde_json::from_value(
        source
            .request(&request.operation, SessionCommand::Inventory)
            .unwrap(),
    )
    .unwrap();
    // The staged candidate is this worktree's actual executable. Only the test's
    // discovery label is older, so the complete same-version handoff is exercised.
    if request.release.source.is_none() {
        inventory.running_version = "0.0.0".into();
    }
    if request.stale_clients {
        // The inventory was captured before the second client attached. Its
        // installation must be discovered and staged by the same transaction.
        inventory.clients.truncate(1);
    }
    eprintln!("coordinator stage: execute release");
    crate::live_update::coordinator::execute_release(
        &mut store,
        &mut source,
        inventory,
        request.release,
    )
    .unwrap();
    eprintln!("coordinator stage: complete");
}

fn coordinator_diagnostics(fixture: &Fixture, operation: &str, log_path: &Path) -> String {
    let log = fs::read(log_path).unwrap_or_default();
    let log = String::from_utf8_lossy(&log[log.len().saturating_sub(16 * 1024)..]);
    let journal_path = fixture
        .data
        .join("updates")
        .join(operation)
        .join("journal.json");
    let journal: serde_json::Value = fs::read(journal_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let participants: Vec<_> = journal["participants"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| serde_json::json!({"route": p["route"], "state": p["state"]}))
        .collect();
    let mut transfers = Vec::new();
    for data in [
        &fixture.data,
        &fixture.root.path().join("second-client-data"),
    ] {
        let directory = data
            .join("updates")
            .join(operation)
            .join("source-artifacts");
        for entry in fs::read_dir(directory).into_iter().flatten().flatten() {
            let path = entry.path();
            if matches!(
                path.extension().and_then(|s| s.to_str()),
                Some("part" | "gz")
            ) {
                transfers.push(format!(
                    "{}: {} bytes",
                    path.display(),
                    entry.metadata().map(|m| m.len()).unwrap_or_default()
                ));
            }
        }
    }
    format!("log tail: {log}\nphase: {}; participants: {participants:?}; errors: {}; source transfers: {transfers:?}", journal["phase"], journal["errors"])
}

#[test]
fn full_update_coordinator_preserves_two_clients_focus_input_and_terminal_processes() {
    check_full_update_coordinator(false, false);
}

#[test]
#[cfg(target_os = "linux")]
fn same_version_source_update_streams_to_client_and_preserves_processes() {
    check_full_update_coordinator(true, false);
}

#[test]
fn coordinator_reconciles_client_missing_from_initial_inventory() {
    check_full_update_coordinator(false, true);
}

fn check_full_update_coordinator(from_source: bool, stale_clients: bool) {
    use sha2::{Digest, Sha256};
    let fixture = Fixture::new();
    fs::write(fixture.root.path().join("config.kdl"), "detachable_panes true\nmirror_session false\nload_plugins clear-defaults=true\ndefault_mode \"locked\"\n").unwrap();
    fixture.start();
    let mut first = Attached::start(&fixture);
    first.wait_for("READY-first");
    let operation = uuid::Uuid::new_v4().to_string();
    let mut source = Peer::connect(&fixture.socket());
    let before = source.inventory(&operation);
    let first_id = before
        .clients
        .iter()
        .find(|client| client.supports_update)
        .unwrap()
        .id;
    // Source updates must stream to a separate client installation, so this
    // client cannot satisfy preparation by reading the server's staging files.
    let second_binary = if from_source || stale_clients {
        let binary = fixture.root.path().join("second-client-yinz");
        fs::copy(&fixture.binary, &binary).unwrap();
        binary
    } else {
        fixture.binary.clone()
    };
    let second_data = if from_source || stale_clients {
        let data = fixture.root.path().join("second-client-data");
        yinz_steel_builtins::install(&yinz_steel_builtins::asset_dir(), &data).unwrap();
        data
    } else {
        fixture.data.clone()
    };
    let mut second_command = fixture.command_for(&second_binary, &second_data);
    second_command.args(["attach", SESSION]);
    let mut second = Attached::start_command(second_command);
    second.wait_for("READY-first");
    let second_id = source
        .inventory(&operation)
        .clients
        .into_iter()
        .find(|client| client.supports_update && client.id != first_id)
        .unwrap()
        .id;
    source
        .sender
        .send_client_msg(ClientToServerMsg::Action {
            action: yinz_types::input::actions::Action::GoToTab {
                index: TabNumber::new(2),
            },
            terminal_id: None,
            client_id: Some(second_id),
            is_cli_client: true,
        })
        .unwrap();
    second.wait_for("READY-second");
    let pids: Vec<i32> = ["first-input.pid", "second-input.pid"]
        .iter()
        .map(|name| {
            fs::read_to_string(fixture.root.path().join(name))
                .unwrap()
                .trim()
                .parse()
                .unwrap()
        })
        .collect();
    let old_inode = fs::metadata(&fixture.binary).unwrap().ino();
    let release = if from_source {
        source_fixture_release(&fixture, &operation)
    } else {
        fixture.prepare(&operation);
        let release = crate::update::Release {
            channel: yinz_types::session_update::ReleaseChannel::Prerelease,
            source: None,
            tag_name: yinz_runtime::consts::VERSION.into(),
            assets: vec![],
        };
        let receipts = fixture
            .data
            .join("updates")
            .join(&operation)
            .join("participants");
        fs::create_dir_all(&receipts).unwrap();
        let executable = fixture.binary.canonicalize().unwrap();
        let key = crate::update::hex(&Sha256::digest(executable.as_os_str().as_encoded_bytes()));
        fs::write(
            receipts.join(format!("{key}.json")),
            serde_json::to_vec(&release).unwrap(),
        )
        .unwrap();
        if stale_clients {
            drop(
                Transaction::prepare(
                    &fixture.root.path().join("release"),
                    &second_binary,
                    &second_data,
                    &operation,
                )
                .unwrap(),
            );
            let receipts = second_data
                .join("updates")
                .join(&operation)
                .join("participants");
            fs::create_dir_all(&receipts).unwrap();
            let executable = second_binary.canonicalize().unwrap();
            let key =
                crate::update::hex(&Sha256::digest(executable.as_os_str().as_encoded_bytes()));
            fs::write(
                receipts.join(format!("{key}.json")),
                serde_json::to_vec(&release).unwrap(),
            )
            .unwrap();
        }
        release
    };
    let source_identity = release.source.clone();
    let request_path = fixture.root.path().join("coordinator.json");
    fs::write(
        &request_path,
        serde_json::to_vec(&CoordinatorFixture {
            socket: fixture.socket(),
            data: fixture.data.clone(),
            operation: operation.clone(),
            release,
            stale_clients,
        })
        .unwrap(),
    )
    .unwrap();
    let log_path = fixture.root.path().join("coordinator.log");
    let log = File::create(&log_path).unwrap();
    let environment = fixture.command();
    let mut command = Command::new(std::env::current_exe().unwrap());
    fixture.root.inherit(&mut command);
    command.env_clear();
    for (name, value) in environment.get_envs() {
        if let Some(value) = value {
            command.env(name, value);
        }
    }
    command
        .current_dir(environment.get_current_dir().unwrap())
        .env("YINZ_TEST_UPDATE_COORDINATOR", &request_path)
        .args([
            "--exact",
            "live_update::tests::client_update_real::coordinator_test_worker",
            "--ignored",
            "--nocapture",
        ])
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log);
    let mut child = command.spawn().unwrap();
    let completed = Watched::new(None);
    // Source transfer carries the much larger debug executable through the
    // production bounded request path; this test measures correctness, not speed.
    let worker_timeout = Duration::from_secs(if from_source { 240 } else { 180 });
    let status = std::thread::scope(|scope| {
        let completed = &completed;
        let child = &mut child;
        scope.spawn(move || {
            // Child::wait has no timeout and holds the child needed for cleanup.
            // Keep bounded try_wait polling so a stuck worker can be killed and
            // reaped without leaving a scoped waiter blocked indefinitely.
            let deadline = Instant::now() + worker_timeout;
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        *completed.lock() = Some(Ok(status));
                        return;
                    },
                    Ok(None) => {},
                    Err(error) => {
                        let killed = child.kill();
                        let reaped = child.wait();
                        *completed.lock() = Some(Err(format!(
                            "failed to observe worker: {error}; kill: {killed:?}; reap: {reaped:?}"
                        )));
                        return;
                    },
                }
                if Instant::now() >= deadline {
                    let killed = child.kill();
                    let reaped = child.wait();
                    *completed.lock() = Some(Err(format!(
                        "timed out after {worker_timeout:?} waiting for full update coordinator to finish; kill: {killed:?}; reap: {reaped:?}"
                    )));
                    return;
                }
                std::thread::park_timeout(Duration::from_millis(10));
            }
        });
        completed.wait_until_described(
            "full update coordinator to finish",
            worker_timeout + Duration::from_secs(10),
            |state| state.clone(),
            |state| format!(
                "worker outcome: {state:?}; log: {}",
                coordinator_diagnostics(&fixture, &operation, &log_path)
            ),
        )
    }).unwrap_or_else(|error| panic!(
        "{error}; log: {}",
        coordinator_diagnostics(&fixture, &operation, &log_path)
    ));
    assert!(
        status.success(),
        "coordinator: {}\njournal: {}",
        fs::read_to_string(&log_path).unwrap(),
        fs::read_to_string(
            fixture
                .data
                .join("updates")
                .join(&operation)
                .join("journal.json")
        )
        .unwrap_or_default()
    );
    drop(source);
    let mut destination = Peer::connect(&fixture.socket());
    let after = destination.inventory(&operation);
    assert_ne!(after.server_pid, before.server_pid);
    assert_eq!(after.session, SESSION);
    let source_exited = Watched::new(Ok(false));
    std::thread::scope(|scope| {
        let source_exited = &source_exited;
        scope.spawn(move || {
            // This server is not our child, so waitpid cannot observe its exit.
            // There is no portable exit notification for an arbitrary PID; poll
            // liveness within the existing deadline and publish the actual result.
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let state = yinz_process::liveness::has_exited(before.server_pid)
                    .map_err(|error| format!("server PID {}: {error}", before.server_pid));
                let done = !matches!(state, Ok(false));
                *source_exited.lock() = state;
                if done {
                    return;
                }
                if Instant::now() >= deadline {
                    *source_exited.lock() = Err(format!(
                        "timed out waiting for server PID {} to exit; last observed alive",
                        before.server_pid
                    ));
                    return;
                }
                std::thread::park_timeout(Duration::from_millis(10));
            }
        });
        source_exited
            .wait_until_described(
                "retired source server to exit after completed handoff",
                Duration::from_secs(15),
                |state| (!matches!(state, Ok(false))).then(|| state.clone()),
                |state| format!("server PID {}: {state:?}", before.server_pid),
            )
            .expect("retired source server to exit after completed handoff");
    });
    for source_id in [first_id, second_id] {
        assert!(
            after
                .clients
                .iter()
                .any(|client| client.resumed == Some((operation.clone(), source_id))),
            "missing client {source_id}: {after:?}"
        );
    }
    let new_inode = fs::metadata(&fixture.binary).unwrap().ino();
    assert_ne!(new_inode, old_inode);
    #[cfg(target_os = "linux")]
    for (process, binary) in [
        (first.child.id(), &fixture.binary),
        (second.child.id(), &second_binary),
        (after.server_pid, &fixture.binary),
    ] {
        assert_eq!(
            fs::metadata(format!("/proc/{process}/exe")).unwrap().ino(),
            fs::metadata(binary).unwrap().ino()
        );
    }
    for pid in pids {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None)
            .expect("original terminal process survives the complete update");
    }
    first
        .master
        .write_all(b"after-full-update-first\n")
        .unwrap();
    second
        .master
        .write_all(b"after-full-update-second\n")
        .unwrap();
    first.wait_for("ECHO-first:after-full-update-first");
    second.wait_for("ECHO-second:after-full-update-second");
    assert_eq!(
        fs::read_to_string(fixture.root.path().join("first-input")).unwrap(),
        "after-full-update-first\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.root.path().join("second-input")).unwrap(),
        "after-full-update-second\n"
    );
    let journal: serde_json::Value = serde_json::from_slice(
        &fs::read(
            fixture
                .data
                .join("updates")
                .join(&operation)
                .join("journal.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(journal["phase"], "Complete");
    assert!(journal["restart_required"].as_array().unwrap().is_empty());
    if let Some(identity) = source_identity {
        assert_eq!(after.running_version, before.running_version);
        assert!(!fixture
            .data
            .join("updates")
            .join(&operation)
            .join("source-artifacts")
            .exists());
        assert_eq!(journal["release"]["source"]["commit"], identity.commit);
        for binary in [&fixture.binary, &second_binary] {
            assert_eq!(
                crate::live_update::artifacts::digest_file(binary).unwrap(),
                identity.artifacts[0].binary_sha256
            );
        }
        let received = crate::live_update::artifacts::archive_path(
            &second_data,
            &operation,
            &identity.artifacts[0].target,
        )
        .unwrap();
        assert_eq!(
            crate::live_update::artifacts::digest_file(&received).unwrap(),
            identity.artifacts[0].sha256
        );
    }
}

fn source_fixture_release(fixture: &Fixture, operation: &str) -> crate::update::Release {
    use crate::live_update::artifacts;
    let target = crate::update::target(std::env::consts::OS, std::env::consts::ARCH).unwrap();
    let binary = fixture.root.path().join("source-yinz");
    fs::copy(&fixture.binary, &binary).unwrap();
    // Release packages strip symbols. Keep all of this checkout's executable
    // code, but omit compiler/debug metadata from the transport fixture too.
    // Otherwise chunk count grows with debug info rather than the behavior this
    // correctness test covers, spending minutes launching per-chunk helpers.
    let stripped = Command::new("strip")
        .arg("--strip-all")
        .arg(&binary)
        .output()
        .expect("source update fixture requires binutils strip");
    assert!(stripped.status.success(), "strip fixture: {stripped:?}");
    // A changed executable with the same Cargo version makes a version-only
    // no-op or readiness check observable without building a second workspace.
    fs::OpenOptions::new()
        .append(true)
        .open(&binary)
        .unwrap()
        .write_all(b"\nsource-update-same-version-fixture\n")
        .unwrap();
    let archive = artifacts::archive_path(&fixture.data, operation, target).unwrap();
    fs::create_dir_all(archive.parent().unwrap()).unwrap();
    let encoder =
        flate2::write::GzEncoder::new(File::create(&archive).unwrap(), flate2::Compression::fast());
    let mut bundle = tar::Builder::new(encoder);
    bundle.append_path_with_name(&binary, "yinz").unwrap();
    bundle
        .append_dir_all("plugins/builtins", fixture.data.join("plugins/builtins"))
        .unwrap();
    bundle.into_inner().unwrap().finish().unwrap();
    let artifact = yinz_types::update_client::SourceArtifact {
        target: target.into(),
        sha256: artifacts::digest_file(&archive).unwrap(),
        size: fs::metadata(&archive).unwrap().len(),
        binary_sha256: artifacts::digest_file(&binary).unwrap(),
    };
    assert!(
        artifact.size > 2 * artifacts::CHUNK_SIZE as u64,
        "fixture must stream multiple bounded chunks"
    );
    eprintln!(
        "source fixture: executable={} bytes, archive={} bytes",
        fs::metadata(&binary).unwrap().len(),
        artifact.size
    );
    crate::update::Release {
        channel: Default::default(),
        source: Some(crate::update::SourceRelease {
            commit: "a".repeat(40),
            artifacts: vec![artifact],
        }),
        tag_name: yinz_runtime::consts::VERSION.into(),
        assets: vec![],
    }
}

#[test]
fn fixture_drop_stops_real_server_and_panes_before_removing_installation() {
    let fixture = Fixture::new();
    fixture.start();
    let root = fixture.root.path().to_owned();
    let sockets = fixture.root.socket_dir().to_owned();
    let mut peer = Peer::connect(&fixture.socket());
    let inventory = peer.inventory(&uuid::Uuid::new_v4().to_string());
    // Inventory and terminal content prove both pane scripts started before teardown.
    let attached = Attached::start(&fixture);
    attached.wait_for("READY-first");
    fixture.success(&["--session", SESSION, "action", "go-to-tab", "2"]);
    attached.wait_for("READY-second");
    let pids: Vec<u32> = ["first-input.pid", "second-input.pid"]
        .into_iter()
        .map(|name| {
            fs::read_to_string(root.join(name))
                .unwrap()
                .trim()
                .parse()
                .unwrap()
        })
        .collect();
    drop(attached);
    drop(peer);
    drop(fixture);
    assert!(!root.exists());
    assert!(!sockets.exists());
    for pid in std::iter::once(inventory.server_pid).chain(pids) {
        assert!(
            yinz_process::liveness::has_exited(pid).unwrap(),
            "fixture process {pid} survived cleanup"
        );
    }
}
