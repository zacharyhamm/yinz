//! Disposable process-level regressions for deletion and same-name resurrection.
use interprocess::local_socket::traits::Listener;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::mpsc,
    time::Duration,
};
use yinz_ipc::ipc::{
    ClientToServerMsg, ExitReason, IpcRecv, IpcSenderWithContext, ServerToClientMsg,
};
use yinz_runtime::session_checkpoint::{write_checkpoint, SessionCheckpoint};

const NAME: &str = "recreation";
const MARKER: &str = "floating-plugin-marker";

#[path = "remote_startup.rs"]
mod remote_startup;

struct Session(tempfile::TempDir);
impl Session {
    fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("ysr-")
            .tempdir_in("/tmp")
            .unwrap();
        for dir in [
            "home", "config", "data", "cache", "tmp", "sockets", "runtime",
        ] {
            fs::create_dir(root.path().join(dir)).unwrap();
        }
        let session = Self(root);
        fs::create_dir_all(session.socket().parent().unwrap()).unwrap();
        fs::write(
            session.0.path().join("config.kdl"),
            "detachable_panes false\nload_plugins clear-defaults=true\n",
        )
        .unwrap();
        session
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_yinz"));
        cmd.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default());
        for (key, dir) in [
            ("HOME", "home"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_CACHE_HOME", "cache"),
            ("TMPDIR", "tmp"),
            ("YINZ_SOCKET_DIR", "sockets"),
            ("XDG_RUNTIME_DIR", "runtime"),
        ] {
            cmd.env(key, self.0.path().join(dir));
        }
        cmd.current_dir(self.0.path().join("home"))
            .arg("--config-dir")
            .arg(self.0.path().join("config"));
        cmd.env("SHELL", "/bin/sh")
            .env("TERM", "xterm-256color")
            .arg("--config")
            .arg(self.0.path().join("config.kdl"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }
    fn run(&self, args: &[&str]) -> Output {
        let mut cmd = self.command();
        cmd.args(args);
        eprintln!("argv/environment: {cmd:?}");
        let output = cmd.output().unwrap();
        eprintln!(
            "status={} stdout={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
    fn success(&self, args: &[&str]) -> Output {
        let out = self.run(args);
        assert!(out.status.success());
        out
    }
    fn wait_for_panes(&self, floating_plugin: bool) {
        let state = yinz_test_support::Watched::new(serde_json::Value::Null);
        let timeout = Duration::from_secs(30);
        let ready = |panes: &serde_json::Value| {
            panes.as_array().is_some_and(|panes| {
                panes.iter().any(|pane| pane["is_plugin"] == false)
                    && (panes.iter().any(|pane| {
                        pane["is_floating"] == true && pane.to_string().contains(MARKER)
                    }) == floating_plugin)
            })
        };
        std::thread::scope(|scope| {
            let state = &state;
            let ready = &ready;
            scope.spawn(move || {
                let deadline = std::time::Instant::now() + timeout;
                while std::time::Instant::now() < deadline {
                    let out = self
                        .command()
                        .args(["--session", NAME, "action", "list-panes", "--json"])
                        .output()
                        .unwrap();
                    if let Ok(panes) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                        let complete = ready(&panes);
                        *state.lock() = panes;
                        if complete {
                            return;
                        }
                    }
                }
            });
            state.wait_until("all layout panes to finish loading", timeout, |panes| {
                ready(panes).then_some(())
            });
        });
        eprintln!("ready panes: {:?}", *state.lock());
    }
    fn socket(&self) -> PathBuf {
        self.0
            .path()
            .join("sockets")
            .join(&*yinz_runtime::consts::CLIENT_SERVER_CONTRACT_DIR)
            .join(NAME)
    }
    fn folder(&self) -> PathBuf {
        self.0
            .path()
            .join(if cfg!(target_os = "macos") {
                "home/Library/Caches/org.Yinz-Contributors.Yinz"
            } else {
                "cache/yinz"
            })
            .join(&*yinz_runtime::consts::CLIENT_SERVER_CONTRACT_DIR)
            .join("session_info")
            .join(NAME)
    }
    fn checkpoint(&self) -> String {
        let folder = self.folder();
        let checkpoint = SessionCheckpoint::read(&folder).unwrap();
        let layout = fs::read_to_string(checkpoint.layout_path()).unwrap();
        eprintln!(
            "current={:?}\nmetadata={}\nlayout={layout}",
            fs::read_link(folder.join("current")),
            fs::read_to_string(checkpoint.metadata_path()).unwrap()
        );
        layout
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        if std::thread::panicking() {
            for entry in fs::read_dir(self.0.path().join("tmp")).unwrap().flatten() {
                if let Ok(log) = fs::read_to_string(entry.path().join("yinz-log/yinz.log")) {
                    eprintln!("server log: {log}");
                }
            }
        }
        if self.socket().exists() {
            let _ = self.run(&["kill-session", NAME]);
        }
    }
}

#[test]
fn startup_traces_identify_processes_and_stages_without_config_values() {
    let session = Session::new();
    fs::write(
        session.0.path().join("config.kdl"),
        "detachable_panes false\nload_plugins clear-defaults=true\nenv { STARTUP_PRIVATE \"startup-config-secret\"; }\n",
    )
    .unwrap();
    session.success(&[
        "--layout-string",
        "layout { pane; } // startup-argument-secret",
        "attach",
        "--create-background",
        NAME,
    ]);
    session.wait_for_panes(false);
    let log = fs::read_to_string(session.0.path().join(format!(
        "tmp/yinz-{}/yinz-log/yinz.log",
        nix::unistd::Uid::current()
    )))
    .unwrap();
    for stage in [
        "cli.create_directories",
        "cli.initial_config",
        "cli.config",
        "cli.layout",
        "cli.themes",
        "client.server_spawn",
        "client.server_wait",
        "client.send_first_message",
        "server.session_ownership",
        "server.bind_socket",
        "server.config_and_layout",
        "server.host_identity",
        "server.init_session",
    ] {
        assert!(
            log.contains(&format!("stage={stage} event=begin")),
            "{stage}: {log}"
        );
        assert!(
            log.contains(&format!("stage={stage} event=end elapsed_us=")),
            "{stage}: {log}"
        );
    }
    for marker in [
        "process kind=attach ppid=",
        "process kind=server ppid=",
        "exe=Some(",
        "pre_logger_us=",
        "logger_us=",
        "kind=new detached=true",
        "IPC FirstClientConnected write finished",
        "first client dispatch complete",
    ] {
        assert!(log.contains(marker), "missing {marker}: {log}");
    }
    for line in log.lines().filter(|line| line.contains("startup trace:")) {
        let (_, pid) = line.split_once("[pid=").expect("startup line has a PID");
        assert!(pid.split_once(']').unwrap().0.parse::<u32>().unwrap() > 0);
    }
    assert!(
        !log.contains("startup-config-secret"),
        "config leaked: {log}"
    );
    assert!(
        !log.contains("startup-argument-secret"),
        "arguments leaked: {log}"
    );
}

#[cfg(target_os = "linux")]
fn server_pid(session: &Session) -> Option<i32> {
    use nix::libc;
    use std::os::fd::AsRawFd;
    let socket = std::os::unix::net::UnixStream::connect(session.socket()).unwrap();
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut size = std::mem::size_of_val(&credentials) as libc::socklen_t;
    // Both pointers refer to initialized, correctly sized values for SO_PEERCRED.
    assert_eq!(
        unsafe {
            libc::getsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut credentials as *mut libc::ucred).cast(),
                &mut size,
            )
        },
        0
    );
    Some(credentials.pid)
}
#[cfg(not(target_os = "linux"))]
fn server_pid(_session: &Session) -> Option<i32> {
    None
}

fn recreation(delete: bool, kill_first: bool) {
    let session = Session::new();
    let plugin = session.0.path().join(format!("{MARKER}.scm"));
    fs::write(
        &plugin,
        "(yinz-declare-pane) (define (render rows columns) \"floating-plugin-marker\")\n",
    )
    .unwrap();
    let layout = session.0.path().join("layout.kdl");
    fs::write(&layout, format!("layout {{\n pane\n floating_panes {{\n pane name=\"{MARKER}\" {{ plugin location=\"file:{}\"; }}\n }}\n}}\n", plugin.display())).unwrap();
    session.success(&[
        "--layout",
        layout.to_str().unwrap(),
        "attach",
        "--create-background",
        NAME,
    ]);
    session.wait_for_panes(true);
    let old_pid = server_pid(&session);
    eprintln!(
        "old server pid={old_pid:?}, socket={:?}",
        fs::metadata(session.socket()).unwrap()
    );
    session.success(&["--session", NAME, "action", "save-session"]);
    assert!(session.checkpoint().contains(MARKER));
    if kill_first {
        // The save completed against this live server; this kill must use completion.
        assert!(session.socket().exists());
        session.success(&["kill-session", NAME]);
        assert!(session.checkpoint().contains(MARKER));
    }
    if delete {
        session.success(&["delete-session", "--force", NAME]);
        assert!(
            !session.folder().exists(),
            "successful deletion left persistence"
        );
    }
    fs::write(&layout, "layout { pane; }\n").unwrap();
    session.success(&[
        "--layout",
        layout.to_str().unwrap(),
        "attach",
        "--create-background",
        NAME,
    ]);
    session.wait_for_panes(!delete);
    let new_pid = server_pid(&session);
    eprintln!(
        "new server pid={new_pid:?}, socket={:?}",
        fs::metadata(session.socket()).unwrap()
    );
    if old_pid.is_some() {
        assert_ne!(old_pid, new_pid);
    }
    let panes = session.success(&["--session", NAME, "action", "list-panes", "--json"]);
    assert_eq!(
        String::from_utf8_lossy(&panes.stdout).contains(MARKER),
        !delete
    );
    session.success(&["--session", NAME, "action", "save-session"]);
    assert_eq!(session.checkpoint().contains(MARKER), !delete);
    session.success(&["kill-session", NAME]);
}
#[test]
fn kill_delete_recreate_does_not_restore_floating_plugin() {
    recreation(true, true);
}
#[test]
fn force_delete_recreate_does_not_restore_floating_plugin() {
    recreation(true, false);
}
#[test]
fn kill_without_delete_resurrects_floating_plugin() {
    recreation(false, true);
}

/// A serialized session names a floating plugin that has since dropped its pane declaration.
/// Resurrection restores the rest of the session but not that pane: the plugin's slot is
/// removed, the plugin is not left running, and the next save no longer records it.
#[test]
fn resurrecting_an_undeclared_floating_plugin_removes_its_pane() {
    const TERMINAL_NAME: &str = "resurrected-terminal";
    let session = Session::new();
    let plugin = session.0.path().join(format!("{MARKER}.scm"));
    fs::write(
        &plugin,
        "(yinz-declare-pane) (define (render rows columns) \"floating-plugin-marker\")\n",
    )
    .unwrap();
    let layout = session.0.path().join("layout.kdl");
    fs::write(&layout, format!("layout {{\n pane name=\"{TERMINAL_NAME}\"\n floating_panes {{\n pane name=\"{MARKER}\" {{ plugin location=\"file:{}\"; }}\n }}\n}}\n", plugin.display())).unwrap();
    session.success(&[
        "--layout",
        layout.to_str().unwrap(),
        "attach",
        "--create-background",
        NAME,
    ]);
    session.wait_for_panes(true);
    session.success(&["--session", NAME, "action", "save-session"]);
    let saved = session.checkpoint();
    assert!(saved.contains(MARKER) && saved.contains(TERMINAL_NAME));
    session.success(&["kill-session", NAME]);
    assert!(session.checkpoint().contains(MARKER));

    // The plugin no longer declares a pane; the session comes back from its checkpoint.
    fs::write(
        &plugin,
        "(define (load configuration) #t)\n(define (pipe message) #f)\n",
    )
    .unwrap();
    fs::write(&layout, "layout { pane; }\n").unwrap();
    session.success(&[
        "--layout",
        layout.to_str().unwrap(),
        "attach",
        "--create-background",
        NAME,
    ]);
    // The named terminal shows the checkpoint, not the fresh layout, was restored.
    let state = yinz_test_support::Watched::new(serde_json::Value::Null);
    let timeout = Duration::from_secs(30);
    let restored_without_plugin = |panes: &serde_json::Value| {
        panes.as_array().is_some_and(|panes| {
            panes
                .iter()
                .any(|pane| pane["is_plugin"] == false && pane.to_string().contains(TERMINAL_NAME))
                && !panes.iter().any(|pane| pane.to_string().contains(MARKER))
        })
    };
    std::thread::scope(|scope| {
        let state = &state;
        let restored_without_plugin = &restored_without_plugin;
        let session = &session;
        scope.spawn(move || {
            let deadline = std::time::Instant::now() + timeout;
            while std::time::Instant::now() < deadline {
                let out = session
                    .command()
                    .args(["--session", NAME, "action", "list-panes", "--json"])
                    .output()
                    .unwrap();
                if let Ok(panes) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                    let done = restored_without_plugin(&panes);
                    *state.lock() = panes;
                    if done {
                        return;
                    }
                }
            }
        });
        state.wait_until(
            "the checkpoint to be restored without the undeclared plugin's pane",
            timeout,
            |panes| restored_without_plugin(panes).then_some(()),
        );
    });
    let panes = session.success(&["--session", NAME, "action", "list-panes", "--json"]);
    let panes: serde_json::Value = serde_json::from_slice(&panes.stdout).unwrap();
    assert!(
        panes
            .as_array()
            .unwrap()
            .iter()
            .all(|pane| pane["is_plugin"] == false),
        "{panes}"
    );
    let plugins = session.success(&["--session", NAME, "plugins", "list", "--running", "--json"]);
    assert!(
        !String::from_utf8_lossy(&plugins.stdout).contains(MARKER),
        "the aborted plugin must not be running"
    );
    session.success(&["--session", NAME, "action", "save-session"]);
    let saved = session.checkpoint();
    // The tabs record live panes. `new_tab_template` is the layout new tabs are made from, kept
    // from the session's first layout; a tab made from it drops the slot the same way.
    let tabs = saved.split("new_tab_template").next().unwrap();
    assert!(
        tabs.contains(TERMINAL_NAME) && !tabs.contains(MARKER),
        "{saved}"
    );
    session.success(&["kill-session", NAME]);
}

#[test]
fn delete_session_reports_removal_failure_and_missing_persistence() {
    let session = Session::new();
    let folder = session.folder();
    fs::create_dir_all(folder.parent().unwrap()).unwrap();
    fs::write(&folder, "invalid directory").unwrap();
    let out = session.run(&["delete-session", "--force", NAME]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("Failed to remove session"));
    assert_eq!(fs::read_to_string(&folder).unwrap(), "invalid directory");
    fs::remove_file(&folder).unwrap();
    assert_eq!(
        session
            .run(&["delete-session", "--force", NAME])
            .status
            .code(),
        Some(2)
    );
    write_checkpoint(&folder, "metadata", &(MARKER.into(), Default::default())).unwrap();
    session.success(&["delete-session", "--force", NAME]);
    assert!(!folder.exists());
}

#[test]
fn force_delete_waits_for_completion_and_preserves_checkpoint_on_failure() {
    for (reply, timeout) in [
        (
            Some(ServerToClientMsg::KillSessionComplete { error: None }),
            false,
        ),
        (
            Some(ServerToClientMsg::KillSessionComplete {
                error: Some("cleanup failed".into()),
            }),
            false,
        ),
        (
            Some(ServerToClientMsg::Exit {
                exit_reason: ExitReason::Normal,
            }),
            false,
        ),
        (None, false),
        (None, true),
    ] {
        let session = Session::new();
        let folder = session.folder();
        write_checkpoint(&folder, "metadata", &(MARKER.into(), Default::default())).unwrap();
        let before = session.checkpoint();
        let listener = yinz_ipc::consts::ipc_bind(&session.socket()).unwrap();
        let (started, received) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let success = matches!(
            reply,
            Some(ServerToClientMsg::KillSessionComplete { error: None })
        );
        let peer = std::thread::spawn(move || loop {
            let mut sender =
                IpcSenderWithContext::<ServerToClientMsg>::new(listener.accept().unwrap());
            let mut receiver = sender.get_receiver::<ClientToServerMsg>().unwrap();
            match receiver.recv_client_msg() {
                IpcRecv::Msg(ClientToServerMsg::ConnStatus, _) => {
                    sender
                        .send_server_msg(ServerToClientMsg::Connected {
                            warp_companions: false,
                            warp_held_terminals: false,
                        })
                        .unwrap();
                },
                IpcRecv::Msg(
                    ClientToServerMsg::KillSessionWithCompletion {
                        report_progress: true,
                    },
                    _,
                ) => {
                    started.send(()).unwrap();
                    released.recv().unwrap();
                    // Completion must not race the client checking that the listener is closed.
                    drop(listener);
                    if let Some(reply) = reply {
                        if success {
                            write_checkpoint(
                                &folder,
                                "final",
                                &(MARKER.into(), Default::default()),
                            )
                            .unwrap();
                        }
                        sender.send_server_msg(reply).unwrap();
                    }
                    break;
                },
                _ => panic!("unexpected request"),
            }
        });
        let mut child = session
            .command()
            .args(["delete-session", "--force", NAME])
            .spawn()
            .unwrap();
        received.recv_timeout(Duration::from_secs(30)).unwrap();
        assert!(child.try_wait().unwrap().is_none());
        assert_eq!(session.checkpoint(), before);
        let output = if timeout {
            let output = child.wait_with_output().unwrap();
            release.send(()).unwrap();
            output
        } else {
            release.send(()).unwrap();
            child.wait_with_output().unwrap()
        };
        peer.join().unwrap();
        eprintln!("delete result: {output:?}");
        assert_eq!(output.status.success(), success);
        if success {
            assert!(!session.folder().exists());
        } else {
            assert_eq!(session.checkpoint(), before);
        }
    }
}

impl Session {
    fn remote_bootstrap(&self, create: bool) -> Command {
        let mut command = self.remote_bootstrap_without_name();
        if create {
            command.arg("--create");
        }
        command.arg(NAME);
        command
    }
    fn remote_bootstrap_without_name(&self) -> Command {
        let mut command = self.command();
        command.env("YINZ_CONFIG_FILE", self.0.path().join("config.kdl"));
        command.env("YINZ_CONFIG_DIR", self.0.path().join("config"));
        command.args([
            "remote-session",
            "--client-version",
            yinz_runtime::consts::VERSION,
            "--contract-version",
            &yinz_runtime::consts::CLIENT_SERVER_CONTRACT_VERSION.to_string(),
            "--bootstrap-version",
            &yinz_runtime::consts::REMOTE_BOOTSTRAP_VERSION.to_string(),
        ]);
        command
    }
    fn remote_endpoint(&self, create: bool) -> serde_json::Value {
        let out = self.remote_bootstrap(create).output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let endpoint: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(endpoint["session_name"], NAME);
        assert_eq!(
            PathBuf::from(endpoint["socket"].as_str().unwrap()),
            self.socket().canonicalize().unwrap()
        );
        endpoint
    }
    fn remote_layout(&self) {
        let layout = self.0.path().join("remote.kdl");
        fs::write(
            &layout,
            format!(
                "layout {{ pane cwd=\"{}\"; }}",
                self.0.path().join("home").display()
            ),
        )
        .unwrap();
        fs::write(
            self.0.path().join("config.kdl"),
            format!(
                "detachable_panes false\nload_plugins clear-defaults=true\ndefault_layout \"{}\"\n",
                layout.display()
            ),
        )
        .unwrap();
    }
}

#[test]
fn remote_bootstrap_missing_existing_create_and_resurrection() {
    let session = Session::new();
    session.remote_layout();
    let missing = session.remote_bootstrap(false).output().unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("does not exist"));
    session.remote_endpoint(true);
    session.wait_for_panes(false);
    let panes = session.success(&["--session", NAME, "action", "list-panes", "--json"]);
    assert!(String::from_utf8_lossy(&panes.stdout)
        .contains(session.0.path().join("home").to_str().unwrap()));
    let panes: serde_json::Value = serde_json::from_slice(&panes.stdout).unwrap();
    let pane = panes
        .as_array()
        .unwrap()
        .iter()
        .find(|pane| pane["is_plugin"] == false)
        .unwrap();
    assert_eq!(pane["pane_rows"], 24);
    assert_eq!(pane["pane_columns"], 80);
    let pid = server_pid(&session);
    if let Some(pid) = pid {
        let args = fs::read(format!("/proc/{pid}/cmdline")).unwrap();
        assert!(!args.split(|byte| *byte == 0).any(|arg| arg == b"--debug"));
    }
    session.remote_endpoint(false);
    session.remote_endpoint(true);
    assert_eq!(pid, server_pid(&session));
    session.success(&["--session", NAME, "action", "save-session"]);
    session.success(&["kill-session", NAME]);
    assert!(!session
        .remote_bootstrap(false)
        .output()
        .unwrap()
        .status
        .success());
    session.remote_endpoint(true);
    session.wait_for_panes(false);
    assert!(session.checkpoint().contains("pane"));
    for _ in 0..2 {
        session.success(&["--session", NAME, "action", "new-pane"]);
    }
    let panes = session.success(&["--session", NAME, "action", "list-panes", "--json"]);
    let panes: serde_json::Value = serde_json::from_slice(&panes.stdout).unwrap();
    assert_eq!(
        panes
            .as_array()
            .unwrap()
            .iter()
            .filter(|pane| pane["is_plugin"] == false)
            .count(),
        3,
        "{panes}"
    );
    let log_dir = session
        .0
        .path()
        .join(format!("tmp/yinz-{}/yinz-log", nix::unistd::Uid::current()));
    assert!(!fs::read_dir(log_dir).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("yinz-")
    }));
}

#[test]
fn remote_bootstrap_new_names_a_session_and_refuses_taken_names() {
    let session = Session::new();
    session.remote_layout();
    let new = |name: Option<&str>| {
        let mut command = session.remote_bootstrap_without_name();
        command.arg("--new");
        if let Some(name) = name {
            command.args(["--", name]);
        }
        command.output().unwrap()
    };
    let refused = |out: Output| {
        assert!(!out.status.success(), "{out:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("YINZ_REMOTE_ERROR \"session_exists\"")
                && stderr.contains(&format!("remote session {NAME:?} already exists")),
            "{stderr}"
        );
    };

    let out = new(Some(NAME));
    assert!(out.status.success(), "{out:?}");
    let endpoint: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(endpoint["session_name"], NAME);
    session.wait_for_panes(false);
    let pid = server_pid(&session);
    refused(new(Some(NAME)));
    assert_eq!(pid, server_pid(&session));

    let out = new(None);
    assert!(out.status.success(), "{out:?}");
    let endpoint: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let generated = endpoint["session_name"].as_str().unwrap().to_owned();
    // Only NAME is cleaned up on drop, so stop the generated session before asserting.
    session.success(&["kill-session", &generated]);
    assert_ne!(generated, NAME);
    assert!(yinz_client::remote_sessions::validate_name(&generated).is_ok());

    // A resurrectable session's name is taken too.
    session.success(&["--session", NAME, "action", "save-session"]);
    session.success(&["kill-session", NAME]);
    refused(new(Some(NAME)));
    assert!(!session.socket().exists());
}

#[test]
fn remote_bootstrap_resurrects_without_loading_the_default_layout() {
    for invalid in [false, true] {
        let session = Session::new();
        session.remote_layout();
        let layout = session.0.path().join("remote.kdl");
        if invalid {
            fs::write(&layout, "layout { invalid").unwrap();
        } else {
            fs::remove_file(&layout).unwrap();
        }
        let out = session.remote_bootstrap(true).output().unwrap();
        assert!(!out.status.success(), "{out:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("load remote configuration and layout"),
            "{out:?}"
        );
        assert!(!session.socket().exists());

        session.success(&[
            "--layout-string",
            "layout { pane name=\"saved checkpoint\"; }",
            "attach",
            "--create-background",
            NAME,
        ]);
        session.wait_for_panes(false);
        session.success(&["--session", NAME, "action", "save-session"]);
        session.success(&["kill-session", NAME]);
        session.remote_endpoint(true);
        session.wait_for_panes(false);
        let panes = session.success(&["--session", NAME, "action", "list-panes", "--json"]);
        assert!(String::from_utf8_lossy(&panes.stdout).contains("saved checkpoint"));
        session.success(&["kill-session", NAME]);

        write_checkpoint(
            &session.folder(),
            "metadata",
            &("layout { invalid".into(), Default::default()),
        )
        .unwrap();
        let out = session.remote_bootstrap(true).output().unwrap();
        assert!(!out.status.success(), "{out:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("read remote session checkpoint"),
            "{out:?}"
        );
        assert!(!session.socket().exists());
    }
}

#[test]
fn local_bootstrap_preserves_selected_paths_options_and_checkpoint_layout() {
    let session = Session::new();
    let config = session.0.path().join("selected config.kdl");
    let config_dir = session.0.path().join("selected config directory");
    let data_dir = session.0.path().join("selected data directory");
    yinz_steel_builtins::install(&yinz_steel_builtins::asset_dir(), &data_dir).unwrap();
    fs::create_dir_all(config_dir.join("layouts")).unwrap();
    fs::write(
        &config,
        "detachable_panes false\nload_plugins clear-defaults=true\ndefault_layout \"selected\"\nscroll_buffer_size 1000\n",
    )
    .unwrap();
    let layout = config_dir.join("layouts/selected.kdl");
    fs::write(&layout, "layout { pane name=\"selected configuration\"; }").unwrap();
    for resurrect in [false, true] {
        let out = session
            .command()
            .arg("--config")
            .arg(&config)
            .arg("--config-dir")
            .arg(&config_dir)
            .arg("--data-dir")
            .arg(&data_dir)
            .args([
                "remote-session",
                "--create",
                "--client-version",
                yinz_runtime::consts::VERSION,
                "--contract-version",
                &yinz_runtime::consts::CLIENT_SERVER_CONTRACT_VERSION.to_string(),
                "--bootstrap-version",
                &yinz_runtime::consts::REMOTE_BOOTSTRAP_VERSION.to_string(),
                "--configuration-options",
                &serde_json::to_string(&yinz_types::input::options::Options {
                    scroll_buffer_size: Some(32),
                    ..Default::default()
                })
                .unwrap(),
                "--",
                NAME,
            ])
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        session.wait_for_panes(false);
        let panes = session.success(&["--session", NAME, "action", "list-panes", "--json"]);
        let panes = String::from_utf8(panes.stdout).unwrap();
        assert!(
            panes.contains("selected configuration"),
            "resurrect={resurrect}: {panes}"
        );
        let panes: serde_json::Value = serde_json::from_str(&panes).unwrap();
        let pane_id = panes
            .as_array()
            .unwrap()
            .iter()
            .find(|pane| pane["is_plugin"] == false)
            .unwrap()["id"]
            .as_u64()
            .unwrap()
            .to_string();
        session.success(&["--session", NAME, "data", "catalog"]);
        assert!(data_dir.join("plugin-data.db").is_file());
        assert!(data_dir.join("host-id").is_file());
        let marker = format!("buffer-complete-{resurrect}");
        session.success(&[
            "--session", NAME, "action", "write-chars", "--pane-id", &pane_id,
            &format!("i=0; while [ \"$i\" -lt 200 ]; do printf 'buffer-line-%03d\\n' \"$i\"; i=$((i+1)); done; printf '{marker}\\n'\n"),
        ]);
        let screen = yinz_test_support::Watched::new(String::new());
        let timeout = Duration::from_secs(30);
        let complete = |text: &str| text.lines().any(|line| line.trim() == marker);
        let dump = std::thread::scope(|scope| {
            scope.spawn(|| {
                let deadline = std::time::Instant::now() + timeout;
                while std::time::Instant::now() < deadline {
                    let out = session
                        .command()
                        .args([
                            "--session",
                            NAME,
                            "action",
                            "dump-screen",
                            "--full",
                            "--pane-id",
                            &pane_id,
                        ])
                        .output()
                        .unwrap();
                    assert!(out.status.success(), "{out:?}");
                    let text = String::from_utf8(out.stdout).unwrap();
                    let done = complete(&text);
                    *screen.lock() = text;
                    if done {
                        return;
                    }
                }
            });
            screen.wait_until("scrollback output", timeout, |text| {
                complete(text).then(|| text.clone())
            })
        });
        assert!(dump.contains("buffer-line-199"), "{dump}");
        // The process-wide scrollback limit is fixed at server initialization.
        assert!(
            !dump.contains("buffer-line-000"),
            "resurrect={resurrect}: {dump}"
        );
        session.success(&["--session", NAME, "action", "save-session"]);
        session.success(&["kill-session", NAME]);
        fs::write(&layout, "layout { pane name=\"changed default\"; }").unwrap();
    }
}

#[test]
fn bootstrap_rejects_malformed_configuration_options_before_startup() {
    for json in ["{", r#"{"scroll_buffer_size":"invalid"}"#, "null"] {
        let session = Session::new();
        fs::remove_dir(session.socket().parent().unwrap()).unwrap();
        let out = session
            .remote_bootstrap(true)
            .args(["--configuration-options", json])
            .output()
            .unwrap();
        assert!(!out.status.success(), "{out:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("decode configuration options"),
            "{out:?}"
        );
        assert!(!session.socket().parent().unwrap().exists());
        assert!(!session.folder().exists());
    }
}

#[test]
fn bootstrap_rejects_malformed_terminal_settings_before_startup() {
    for json in ["{", "{}", "42", r#"{"iflag":"invalid"}"#] {
        let session = Session::new();
        fs::remove_dir(session.socket().parent().unwrap()).unwrap();
        let out = session
            .remote_bootstrap(true)
            .args(["--client-termios", json])
            .output()
            .unwrap();
        assert!(!out.status.success(), "{out:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("decode client terminal settings"),
            "{out:?}"
        );
        assert!(!session.socket().parent().unwrap().exists());
        assert!(!session.folder().exists());
    }
}

#[test]
fn bootstrap_rejects_malformed_terminal_window_size_before_startup() {
    for json in ["{", "{}", "null", r#"{"rows":"invalid","cols":120}"#] {
        let session = Session::new();
        fs::remove_dir(session.socket().parent().unwrap()).unwrap();
        let out = session
            .remote_bootstrap(true)
            .args(["--terminal-window-size", json])
            .output()
            .unwrap();
        assert!(!out.status.success(), "{out:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("decode terminal window size"),
            "{out:?}"
        );
        assert!(!session.socket().parent().unwrap().exists());
        assert!(!session.folder().exists());
        assert!(fs::read_dir(session.0.path().join("tmp"))
            .unwrap()
            .next()
            .is_none());
    }
}

#[test]
fn simultaneous_remote_bootstraps_share_one_session() {
    let session = Session::new();
    session.remote_layout();
    std::thread::scope(|scope| {
        let first = scope.spawn(|| session.remote_endpoint(true));
        let second = scope.spawn(|| session.remote_endpoint(true));
        assert_eq!(first.join().unwrap(), second.join().unwrap());
    });
    session.wait_for_panes(false);
}

#[test]
fn simultaneous_new_bootstraps_create_one_session() {
    let session = Session::new();
    session.remote_layout();
    let outputs = std::thread::scope(|scope| {
        let bootstraps = [(); 2].map(|_| {
            scope.spawn(|| {
                session
                    .remote_bootstrap_without_name()
                    .args(["--new", NAME])
                    .output()
                    .unwrap()
            })
        });
        bootstraps.map(|bootstrap| bootstrap.join().unwrap())
    });
    let (created, refused): (Vec<_>, Vec<_>) = outputs.iter().partition(|out| out.status.success());
    assert_eq!((created.len(), refused.len()), (1, 1), "{outputs:?}");
    let endpoint: serde_json::Value = serde_json::from_slice(&created[0].stdout).unwrap();
    assert_eq!(endpoint["session_name"], NAME);
    let error = String::from_utf8_lossy(&refused[0].stderr);
    assert!(error.contains("already exists"), "{error}");
    session.wait_for_panes(false);
}

#[test]
fn remote_bootstrap_refuses_unresponsive_session_without_replacing_socket() {
    use std::os::unix::fs::MetadataExt;
    let session = Session::new();
    let _listener = std::os::unix::net::UnixListener::bind(session.socket()).unwrap();
    let inode = fs::metadata(session.socket()).unwrap().ino();
    let out = session.remote_bootstrap(true).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unresponsive"));
    assert_eq!(inode, fs::metadata(session.socket()).unwrap().ino());
    fs::remove_file(session.socket()).unwrap();
}

#[test]
fn bootstrap_from_a_client_without_bootstrap_versioning_cannot_create_a_session() {
    let session = Session::new();
    let mut command = session.command();
    command.env("YINZ_CONFIG_FILE", session.0.path().join("config.kdl"));
    command.env("YINZ_CONFIG_DIR", session.0.path().join("config"));
    // The command line an older client sends: matching versions, no bootstrap version.
    let out = command
        .args([
            "remote-session",
            "--client-version",
            yinz_runtime::consts::VERSION,
            "--contract-version",
            &yinz_runtime::consts::CLIENT_SERVER_CONTRACT_VERSION.to_string(),
            "--create",
            NAME,
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(
        error.contains("Yinz compatibility mismatch")
            && error.contains("no remote bootstrap version"),
        "{error}"
    );
    assert!(!session.socket().exists());
}

#[test]
fn remote_bootstrap_version_mismatch_cannot_create_a_session() {
    let session = Session::new();
    let out = session
        .remote_bootstrap(true)
        .args(["--client-version", "incompatible"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(error.contains("incompatible") && error.contains(yinz_runtime::consts::VERSION));
    assert!(!session.socket().exists());
}

#[test]
fn live_kill_session_confirms_a_sweep_longer_than_ten_seconds() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use yinz_process::pty_host::protocol::{self, HostStatus, HostToServerMsg, ServerToHostMsg};

    let session = Session::new();
    session.success(&[
        "--layout-string",
        "layout { pane; }",
        "attach",
        "--create-background",
        NAME,
    ]);
    session.wait_for_panes(false);
    let dir = session.socket().parent().unwrap().join("pty");
    fs::create_dir_all(&dir).unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let mut supervisors = tokio::task::JoinSet::new();
            for i in 0..129 {
                let path = dir.join(format!("delayed-{i}.sock"));
                let listener = tokio::net::UnixListener::bind(&path).unwrap();
                supervisors.spawn(async move {
                    for terminate in [false, true] {
                        let (mut stream, _) = listener.accept().await.unwrap();
                        let len = stream.read_u32_le().await.unwrap();
                        assert!(len < 4096, "unexpectedly large supervisor request");
                        let mut body = vec![0; len as usize];
                        stream.read_exact(&mut body).await.unwrap();
                        let request: ServerToHostMsg = serde_json::from_slice(&body).unwrap();
                        let reply = if terminate {
                            assert!(matches!(request, ServerToHostMsg::TerminateDetached { expected_session } if expected_session.as_str() == NAME));
                            // Five batches at the 32-operation limit must exceed ten seconds.
                            tokio::time::sleep(Duration::from_secs(2)).await;
                            HostToServerMsg::TerminationComplete { terminated: true, error: None }
                        } else {
                            assert!(matches!(request, ServerToHostMsg::Query));
                            let status: HostStatus = serde_json::from_value(serde_json::json!({
                                "host_id": format!("delayed-{i}"), "session_name": NAME,
                                "terminal_id": 0, "command": "sleep", "cwd": null,
                                "child_pid": null, "host_pid": 1, "started_at_unix": 0,
                                "attached": false, "child_exit_status": null, "buffered_bytes": 0
                            })).unwrap();
                            HostToServerMsg::Status(status)
                        };
                        let mut bytes = Vec::new();
                        protocol::write_msg(&mut bytes, &reply).unwrap();
                        stream.write_all(&bytes).await.unwrap();
                    }
                    i
                });
            }
            let start = std::time::Instant::now();
            let output = tokio::time::timeout(
                Duration::from_secs(60),
                tokio::process::Command::from(session.command()).args(["kill-session", NAME]).output(),
            ).await.unwrap().unwrap();
            assert!(output.status.success(), "{output:?}");
            assert!(start.elapsed() > Duration::from_secs(10));
            let mut terminated = std::collections::BTreeSet::new();
            while let Some(result) = supervisors.join_next().await {
                assert!(terminated.insert(result.unwrap()), "duplicate termination request");
            }
            assert_eq!(terminated.len(), 129);
            // A live kill succeeds only on completion; fallback orphan cleanup cannot return success.
            assert!(std::os::unix::net::UnixStream::connect(session.socket()).is_err());
        });
}
