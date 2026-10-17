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
        "(define (render rows columns) \"floating-plugin-marker\")\n",
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
                IpcRecv::Msg(ClientToServerMsg::KillSessionWithCompletion, _) => {
                    started.send(()).unwrap();
                    released.recv().unwrap();
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
