//! Remote startup regressions with isolated servers and interruptible SSH stubs.
use super::*;
use interprocess::local_socket::traits::Stream;
use nix::sys::signal::{kill, killpg, Signal};
use nix::unistd::Pid;
use std::io::Read;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use yinz_test_support::{default_timeout, Watched};

fn discovery_command(session: &Session, names: &[&str]) -> Command {
    let mut command = fake_ssh_command(session, names);
    command.args(["attach", "--remote", "user@example"]);
    command
}

/// A client whose SSH lists `names` and fails any session bootstrap after recording it.
fn fake_ssh_command(session: &Session, names: &[&str]) -> Command {
    let root = session.0.path();
    yinz_test_support::write_warm_script(
        &root.join("ssh"),
        r#"printf '%s\n' "$@" >> "$YINZ_TEST_SSH_ARGS"
for command do :; done
case "$command" in
    *remote-sessions*) printf '%s\n' "$YINZ_TEST_SESSIONS" ;;
    *) echo 'selected-session-bootstrap' >&2; exit 1 ;;
esac"#,
    );
    let mut path = vec![root.to_path_buf()];
    path.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let sessions = names
        .iter()
        .map(|name| {
            (
                name,
                Duration::ZERO,
                yinz_cli::sessions::SessionListing::Running,
            )
        })
        .collect::<Vec<_>>();
    let mut command = session.command();
    command
        .env("PATH", std::env::join_paths(path).unwrap())
        .env("YINZ_TEST_SSH_ARGS", root.join("ssh-args"))
        .env(
            "YINZ_TEST_SESSIONS",
            serde_json::json!({
                "yinz_version": yinz_runtime::consts::VERSION,
                "contract_version": yinz_runtime::consts::CLIENT_SERVER_CONTRACT_VERSION,
                "bootstrap_version": yinz_runtime::consts::REMOTE_BOOTSTRAP_VERSION,
                "sessions": sessions,
            })
            .to_string(),
        );
    command
}

#[test]
fn remote_attach_without_a_name_discovers_before_selecting_or_reporting_sessions() {
    for (names, expected) in [
        (
            vec![],
            "No active yinz sessions found on user@example. Start one with `yinz --remote user@example`.",
        ),
        (vec!["remote work"], "selected-session-bootstrap"),
        (
            vec!["one", "two"],
            "yinz attach --remote user@example SESSION",
        ),
    ] {
        let session = Session::new();
        let output = discovery_command(&session, &names).output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(stderr.contains(expected), "{stderr}");
        let args = fs::read_to_string(session.0.path().join("ssh-args")).unwrap();
        assert!(
            args.contains("user@example\n'yinz' 'remote-sessions'"),
            "{args}"
        );
        assert_eq!(
            args.matches("BatchMode=yes").count(),
            if names.len() == 1 { 2 } else { 1 }
        );
        if names.len() == 1 {
            assert!(args.contains("'--' 'remote work'"), "{args}");
        } else {
            assert!(!args.contains("'remote-session'"), "{args}");
        }
    }
}

#[test]
fn remote_attach_discovery_picker_selects_only_from_the_requested_host() {
    use nix::sys::termios::{self, LocalFlags};
    use std::io::Write;

    let session = Session::new();
    let pty = nix::pty::openpty(
        Some(&nix::pty::Winsize {
            ws_row: 24,
            ws_col: 100,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }),
        None,
    )
    .unwrap();
    let slave = fs::File::from(pty.slave);
    let original = termios::tcgetattr(&slave).unwrap();
    let mut master = fs::File::from(pty.master);
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
    let child = discovery_command(&session, &["one", "two"])
        .stdin(slave.try_clone().unwrap())
        .stdout(slave.try_clone().unwrap())
        .stderr(slave.try_clone().unwrap())
        .process_group(0)
        .spawn()
        .unwrap();
    let client = Client {
        pid: Pid::from_raw(child.id() as i32),
        output: Watched::new(None),
    };
    let completed = client.output.clone();
    let waiter = std::thread::spawn(move || {
        *completed.lock() = Some(child.wait_with_output().unwrap());
    });
    screen.wait_until("remote session choices", default_timeout(), |bytes| {
        let text = String::from_utf8_lossy(bytes);
        (text.contains("user@example / one") && text.contains("user@example / two")).then_some(())
    });
    master.write_all(b"two\r").unwrap();
    client
        .output
        .wait_until("selected remote bootstrap", default_timeout(), |output| {
            output
                .as_ref()
                .map(|output| assert_eq!(output.status.code(), Some(1), "{output:?}"))
        });
    waiter.join().unwrap();
    // XNU sets PENDIN when canonical mode returns; it is kernel status, not part
    // of the restored settings.
    let restored = termios::tcgetattr(&slave).unwrap();
    assert_eq!(restored.input_flags, original.input_flags);
    assert_eq!(restored.output_flags, original.output_flags);
    assert_eq!(restored.control_flags, original.control_flags);
    assert_eq!(
        restored.local_flags - LocalFlags::PENDIN,
        original.local_flags - LocalFlags::PENDIN,
    );
    assert_eq!(restored.control_chars, original.control_chars);
    let args = fs::read_to_string(session.0.path().join("ssh-args")).unwrap();
    assert_eq!(args.matches("remote-sessions").count(), 1, "{args}");
    assert!(args.contains("'--' 'two'"), "{args}");
    drop(slave);
    drop(master);
    capture.join().unwrap();
}

#[test]
fn bare_remote_start_asks_the_host_for_a_new_session() {
    for (extra, expected) in [
        (&[][..], "'remote-session' '--client-version'"),
        (
            &["--session", "remote work"][..],
            "'--new' '--' 'remote work'",
        ),
    ] {
        let session = Session::new();
        let output = fake_ssh_command(&session, &[])
            .args(["--remote", "user@example"])
            .args(extra)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(stderr.contains("selected-session-bootstrap"), "{stderr}");
        let args = fs::read_to_string(session.0.path().join("ssh-args")).unwrap();
        assert!(!args.contains("remote-sessions"), "{args}");
        assert!(args.contains(expected), "{args}");
        let bootstrap = args.lines().last().unwrap();
        assert!(bootstrap.contains("'--new'"), "{bootstrap}");
        assert!(!bootstrap.contains("'--create'"), "{bootstrap}");
        assert_eq!(bootstrap.contains("'--'"), !extra.is_empty(), "{bootstrap}");
    }
    let session = Session::new();
    let output = fake_ssh_command(&session, &[])
        .args(["--remote", "user@example", "attach", "work"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("selected-session-bootstrap"),
        "{output:?}"
    );
    assert!(session.0.path().join("ssh-args").exists());
}

#[test]
fn remote_attach_offers_a_new_session_when_the_host_has_none() {
    use std::io::Write;

    for (answer, code) in [(b"y", 1), (b"n", 0)] {
        let session = Session::new();
        let pty = nix::pty::openpty(None, None).unwrap();
        let slave = fs::File::from(pty.slave);
        let mut master = fs::File::from(pty.master);
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
        let child = discovery_command(&session, &[])
            .stdin(slave.try_clone().unwrap())
            .stdout(slave.try_clone().unwrap())
            .stderr(slave.try_clone().unwrap())
            .process_group(0)
            .spawn()
            .unwrap();
        let output = Watched::new(None);
        let completed = output.clone();
        let waiter = std::thread::spawn(move || {
            *completed.lock() = Some(child.wait_with_output().unwrap());
        });
        screen.wait_until("new session question", default_timeout(), |bytes| {
            String::from_utf8_lossy(bytes)
                .contains("No active yinz sessions found on user@example. Create a new session?")
                .then_some(())
        });
        master.write_all(answer).unwrap();
        output.wait_until("remote attach exit", default_timeout(), |output| {
            output
                .as_ref()
                .map(|output| assert_eq!(output.status.code(), Some(code), "{output:?}"))
        });
        waiter.join().unwrap();
        let args = fs::read_to_string(session.0.path().join("ssh-args")).unwrap();
        assert_eq!(args.matches("remote-sessions").count(), 1, "{args}");
        let bootstrap = args.lines().find(|line| line.contains("'remote-session'"));
        if answer == b"y" {
            let bootstrap = bootstrap.unwrap();
            assert!(
                bootstrap.contains("'--new'") && !bootstrap.contains("'--'"),
                "{bootstrap}"
            );
        } else {
            assert_eq!(bootstrap, None, "{args}");
        }
        drop(slave);
        drop(master);
        capture.join().unwrap();
    }
}

#[test]
fn remote_creation_and_resurrection_preserve_or_default_term() {
    for (inherited, configured, expected) in [
        (None, None, "xterm-256color"),
        (Some(""), None, "xterm-256color"),
        (Some("vt100"), None, "vt100"),
        (Some("vt100"), Some("xterm"), "xterm"),
        (Some("vt100"), Some(""), "xterm-256color"),
    ] {
        let session = Session::new();
        session.remote_layout();
        if let Some(term) = configured {
            use std::io::Write;
            writeln!(
                fs::OpenOptions::new()
                    .append(true)
                    .open(session.0.path().join("config.kdl"))
                    .unwrap(),
                "env {{ TERM {term:?}; }}"
            )
            .unwrap();
        }
        let fifo = session.0.path().join("term");
        nix::unistd::mkfifo(
            &fifo,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .unwrap();
        for resurrect in [false, true] {
            let mut command = session.remote_bootstrap(true);
            command.env_remove("TERM");
            if let Some(term) = inherited {
                command.env("TERM", term);
            }
            let output = command.output().unwrap();
            assert!(output.status.success(), "{output:?}");
            session.wait_for_panes(false);
            let term = Watched::new(None);
            let recorded = term.clone();
            let path = fifo.clone();
            let reader = std::thread::spawn(move || {
                let term = fs::read_to_string(path).unwrap();
                *recorded.lock() = Some(term);
            });
            let panes = session.success(&["--session", NAME, "action", "list-panes", "--json"]);
            let panes: serde_json::Value = serde_json::from_slice(&panes.stdout).unwrap();
            let pane = panes
                .as_array()
                .unwrap()
                .iter()
                .find(|pane| pane["is_plugin"] == false)
                .unwrap();
            let pane_id = pane["id"].as_u64().unwrap().to_string();
            session.success(&[
                "--session",
                NAME,
                "action",
                "write-chars",
                "--pane-id",
                &pane_id,
                &format!("printf '%s' \"$TERM\" > '{}'\n", fifo.display()),
            ]);
            let actual = term.wait_until("pane TERM", default_timeout(), Clone::clone);
            reader.join().unwrap();
            assert_eq!(
                actual, expected,
                "inherited={inherited:?}, configured={configured:?}, resurrect={resurrect}"
            );
            session.success(&["--session", NAME, "action", "save-session"]);
            session.success(&["kill-session", NAME]);
        }
    }
}

#[derive(Debug, serde::Deserialize)]
struct Event {
    stage: String,
    pid: i32,
    socket: Option<PathBuf>,
}

struct Client {
    pid: Pid,
    output: Watched<Option<Output>>,
}

#[test]
fn local_resurrection_preserves_terminal_settings_geometry_pane_limit_and_debug() {
    use nix::sys::termios::{tcgetattr, LocalFlags};
    use yinz_types::session_destination::{
        ClientReply, ClientRequest, Destination, Request, SwitchRequest, SwitchTarget,
    };

    let session = Session::new();
    session.remote_layout();
    fs::write(
        session.0.path().join("remote.kdl"),
        "layout { pane name=\"fixed\" size=30; pane; }",
    )
    .unwrap();
    let output = session
        .remote_bootstrap(true)
        .args(["--terminal-window-size", r#"{"rows":60,"cols":120}"#])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    session.wait_for_panes(false);
    session.success(&["--session", NAME, "action", "save-session"]);
    session.success(&["kill-session", NAME]);

    let listener = yinz_ipc::consts::ipc_bind(&session.socket().with_file_name("source")).unwrap();
    let pty = nix::pty::openpty(
        Some(&nix::pty::Winsize {
            ws_row: 60,
            ws_col: 120,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }),
        None,
    )
    .unwrap();
    let slave = fs::File::from(pty.slave);
    assert!(Command::new("stty")
        .args(["intr", "^X"])
        .stdin(slave.try_clone().unwrap())
        .status()
        .unwrap()
        .success());
    let original = tcgetattr(&slave).unwrap();
    let mut command = session.command();
    command
        .args(["--max-panes", "2", "--debug", "attach", "source"])
        .stdin(slave.try_clone().unwrap())
        .stdout(slave.try_clone().unwrap());
    // Detach from the test runner's terminal so size queries use this PTY.
    unsafe {
        command.pre_exec(|| {
            nix::unistd::setsid()
                .map(|_| ())
                .map_err(std::io::Error::from)
        });
    }
    let child = command.spawn().unwrap();
    let client = Client {
        pid: Pid::from_raw(child.id() as i32),
        output: Watched::new(None),
    };
    let completed = client.output.clone();
    let waiter = std::thread::spawn(move || {
        let output = child.wait_with_output().unwrap();
        *completed.lock() = Some(output);
    });
    let (mut sender, mut receiver) = loop {
        let stream = listener.accept().unwrap();
        stream.set_recv_timeout(Some(default_timeout())).unwrap();
        let mut sender = IpcSenderWithContext::<ServerToClientMsg>::new(stream);
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
            IpcRecv::Msg(ClientToServerMsg::AttachClient { .. }, _) => break (sender, receiver),
            _ => panic!("expected source probe or attachment"),
        }
    };
    sender
        .send_server_msg(ServerToClientMsg::Render {
            content: "source ready".into(),
        })
        .unwrap();
    let mut request = |request| {
        sender
            .send_server_msg(ServerToClientMsg::SessionRequest(Request {
                token: "termios".into(),
                request: ClientRequest::Switch(request),
            }))
            .unwrap();
        loop {
            match receiver.recv_client_msg() {
                IpcRecv::Msg(ClientToServerMsg::SessionReply(reply), _) => {
                    return reply.result.unwrap()
                },
                IpcRecv::Msg(_, _) => {},
                _ => panic!("client disconnected during resurrection"),
            }
        }
    };
    assert_eq!(
        request(SwitchRequest::Prepare {
            id: "termios".into(),
            target: SwitchTarget {
                destination: Destination::CurrentServer,
                name: NAME.into(),
                saved: true
            },
        }),
        ClientReply::Pending
    );
    assert!(!tcgetattr(&slave)
        .unwrap()
        .local_flags
        .intersects(LocalFlags::ICANON | LocalFlags::ISIG));
    let deadline = std::time::Instant::now() + default_timeout();
    while request(SwitchRequest::Status("termios".into())) == ClientReply::Pending {
        assert!(
            std::time::Instant::now() < deadline,
            "local resurrection did not finish"
        );
    }
    assert_eq!(
        request(SwitchRequest::Cancel("termios".into())),
        ClientReply::Complete
    );
    sender
        .send_server_msg(ServerToClientMsg::Exit {
            exit_reason: ExitReason::Normal,
        })
        .unwrap();
    client
        .output
        .wait_until("source client exit", default_timeout(), |out| {
            out.as_ref()
                .map(|out| assert!(out.status.success(), "{out:?}"))
        });
    waiter.join().unwrap();
    let restored = tcgetattr(&slave).unwrap();
    assert_eq!(restored.control_chars, original.control_chars);
    assert_eq!(
        restored.local_flags - LocalFlags::PENDIN,
        original.local_flags - LocalFlags::PENDIN
    );

    session.wait_for_panes(false);
    let fifo = session.0.path().join("termios");
    nix::unistd::mkfifo(
        &fifo,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .unwrap();
    let settings = Watched::new(None);
    let recorded = settings.clone();
    let path = fifo.clone();
    let reader = std::thread::spawn(move || {
        let settings = fs::read_to_string(path).unwrap();
        *recorded.lock() = Some(settings);
    });
    let panes = session.success(&["--session", NAME, "action", "list-panes", "--json"]);
    let panes: serde_json::Value = serde_json::from_slice(&panes.stdout).unwrap();
    let panes: Vec<_> = panes
        .as_array()
        .unwrap()
        .iter()
        .filter(|pane| pane["is_plugin"] == false)
        .collect();
    assert_eq!(panes.len(), 2, "{panes:?}");
    assert_eq!(
        panes.iter().find(|pane| pane["title"] == "fixed").unwrap()["pane_rows"],
        30
    );
    assert_eq!(
        panes
            .iter()
            .map(|pane| pane["pane_rows"].as_u64().unwrap())
            .sum::<u64>(),
        60
    );
    assert!(panes.iter().all(|pane| pane["pane_columns"] == 120));
    if let Some(pid) = server_pid(&session) {
        let args = fs::read(format!("/proc/{pid}/cmdline")).unwrap();
        assert!(args.split(|byte| *byte == 0).any(|arg| arg == b"--debug"));
    }
    let pane_id = panes
        .iter()
        .find(|pane| pane["is_plugin"] == false)
        .unwrap()["id"]
        .as_u64()
        .unwrap()
        .to_string();
    session.success(&[
        "--session",
        NAME,
        "action",
        "write-chars",
        "--pane-id",
        &pane_id,
        &format!("stty -a > '{}'\n", fifo.display()),
    ]);
    let actual = settings.wait_until(
        "resurrected pane terminal settings",
        default_timeout(),
        Clone::clone,
    );
    reader.join().unwrap();
    assert!(actual.contains("intr = ^X"), "{actual}");
    for flag in ["icanon", "isig", "echo", "icrnl", "opost"] {
        assert!(
            actual
                .split_whitespace()
                .any(|word| word.trim_end_matches(';') == flag),
            "missing {flag}: {actual}"
        );
    }
    let raw_log = session.0.path().join(format!(
        "tmp/yinz-{}/yinz-log/yinz-{pane_id}.log",
        nix::unistd::Uid::current()
    ));
    let output = Watched::new(String::new());
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let deadline = std::time::Instant::now() + default_timeout();
            while std::time::Instant::now() < deadline {
                let text = fs::read_to_string(&raw_log).unwrap_or_default();
                let done = text.contains("stty -a");
                *output.lock() = text;
                if done {
                    return;
                }
            }
        });
        output.wait_until("raw PTY debug output", default_timeout(), |text| {
            text.contains("stty -a").then_some(())
        });
    });
    session.success(&["--session", NAME, "action", "new-pane", "--name", "added"]);
    let panes = session.success(&["--session", NAME, "action", "list-panes", "--json"]);
    let panes: serde_json::Value = serde_json::from_slice(&panes.stdout).unwrap();
    let panes: Vec<_> = panes
        .as_array()
        .unwrap()
        .iter()
        .filter(|pane| pane["is_plugin"] == false)
        .collect();
    assert_eq!(panes.len(), 2, "{panes:?}");
    assert!(panes.iter().any(|pane| pane["title"] == "added"));
}

impl Drop for Client {
    fn drop(&mut self) {
        if self.output.lock().is_none() {
            let _ = killpg(self.pid, Signal::SIGKILL);
            self.output
                .wait_until("failed client cleanup", default_timeout(), |out| {
                    out.is_some().then_some(())
                });
        }
    }
}

fn interrupted_startup(stage: &str, signal: Signal) {
    use std::io::Write;

    let session = Session::new();
    let root = session.0.path();
    let events_path = root.join("events");
    let listener = UnixListener::bind(&events_path).unwrap();
    let events = Watched::new(Vec::<Event>::new());
    let recorded = events.clone();
    let event_reader = std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut line = String::new();
            stream.unwrap().read_to_string(&mut line).unwrap();
            if line.is_empty() {
                break;
            }
            recorded.lock().push(serde_json::from_str(&line).unwrap());
        }
    });
    let peer_path = root.join("peer");
    let peer = yinz_ipc::consts::ipc_bind(&peer_path).unwrap();
    let watching = stage.starts_with("watch-");
    let (resume_config, config_resumed) = mpsc::channel();
    let mut peer_worker = if watching {
        let root = root.to_owned();
        let events = events.clone();
        let before_guard = stage == "watch-config";
        Some(std::thread::spawn(move || {
            let stream = peer.accept().unwrap();
            stream.set_recv_timeout(Some(default_timeout())).unwrap();
            let mut sender = IpcSenderWithContext::<ServerToClientMsg>::new(stream);
            let mut receiver = sender.get_receiver::<ClientToServerMsg>().unwrap();
            assert!(matches!(
                receiver.recv_client_msg(),
                IpcRecv::Msg(ClientToServerMsg::ConnStatus, _)
            ));
            let (pid, socket) = events
                .lock()
                .iter()
                .find_map(|event| {
                    event
                        .socket
                        .as_ref()
                        .map(|socket| (event.pid, socket.clone()))
                })
                .unwrap();
            if before_guard {
                // Dispatch has read the configuration already. Hold the watcher's next read
                // after dispatch returns, before start_client can arm its terminal guard.
                fs::remove_file(root.join("config.kdl")).unwrap();
                nix::unistd::mkfifo(
                    &root.join("config.kdl"),
                    nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
                )
                .unwrap();
            } else {
                // The startup guard must handle signals while the client retries connecting.
                fs::remove_file(socket).unwrap();
            }
            sender
                .send_server_msg(ServerToClientMsg::Connected {
                    warp_companions: false,
                    warp_held_terminals: false,
                })
                .unwrap();
            if before_guard {
                let mut config = fs::File::create(root.join("config.kdl")).unwrap();
                events.lock().push(Event {
                    stage: "blocked".into(),
                    pid,
                    socket: None,
                });
                config_resumed.recv_timeout(default_timeout()).unwrap();
                config.write_all(b"show_startup_splash false\n").unwrap();
                // start_client reloads local configuration once more before arming its guard.
                fs::remove_file(root.join("config.kdl")).unwrap();
                fs::write(root.join("config.kdl"), "show_startup_splash false\n").unwrap();
            }
        }))
    } else if stage == "switch" {
        Some(std::thread::spawn(move || {
            for probe in [true, false] {
                let stream = peer.accept().unwrap();
                stream.set_recv_timeout(Some(default_timeout())).unwrap();
                let mut sender = IpcSenderWithContext::<ServerToClientMsg>::new(stream);
                let mut receiver = sender.get_receiver::<ClientToServerMsg>().unwrap();
                if probe {
                    assert!(matches!(
                        receiver.recv_client_msg(),
                        IpcRecv::Msg(ClientToServerMsg::ConnStatus, _)
                    ));
                    sender
                        .send_server_msg(ServerToClientMsg::Connected {
                            warp_companions: false,
                            warp_held_terminals: false,
                        })
                        .unwrap();
                } else {
                    assert!(matches!(
                        receiver.recv_client_msg(),
                        IpcRecv::Msg(ClientToServerMsg::AttachClient { .. }, _)
                    ));
                    sender
                        .send_server_msg(ServerToClientMsg::SwitchSession {
                            connect_to_session: yinz_types::data::SessionConnection {
                                name: Some("next".into()),
                                ..Default::default()
                            },
                        })
                        .unwrap();
                    loop {
                        match receiver.recv_client_msg() {
                            IpcRecv::Msg(ClientToServerMsg::ClientExited, _) => break,
                            IpcRecv::Msg(_, _) => {},
                            _ => panic!("client did not complete switch"),
                        }
                    }
                    sender
                        .send_server_msg(ServerToClientMsg::Exit {
                            exit_reason: ExitReason::Normal,
                        })
                        .unwrap();
                }
            }
        }))
    } else {
        None
    };
    fs::write(root.join("ssh.py"), SSH_STUB).unwrap();
    // Resolve before isolating HOME so version-manager shims keep their configured interpreter.
    let python = Command::new("python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()
        .unwrap();
    assert!(python.status.success(), "{python:?}");
    let python = String::from_utf8(python.stdout).unwrap();
    yinz_test_support::write_warm_script(
        &root.join("ssh"),
        "exec \"$YINZ_TEST_PYTHON\" \"$YINZ_TEST_SSH_STUB\" \"$@\"",
    );
    let pty = nix::pty::openpty(None, None).unwrap();
    let slave = fs::File::from(pty.slave);
    let original = nix::sys::termios::tcgetattr(&slave).unwrap();
    let mut path = vec![root.to_path_buf()];
    path.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let mut child = session
        .command()
        .env("PATH", std::env::join_paths(path).unwrap())
        .env("YINZ_TEST_PYTHON", python.trim())
        .env("YINZ_TEST_SSH_STUB", root.join("ssh.py"))
        .env("YINZ_TEST_EVENTS", &events_path)
        .env("YINZ_TEST_PEER", &peer_path)
        .env("YINZ_TEST_STAGE", stage)
        .env(
            "YINZ_TEST_ENDPOINT",
            serde_json::json!({
                "session_name": NAME,
                "socket": "/remote/session",
                "yinz_version": yinz_runtime::consts::VERSION,
                "contract_version": yinz_runtime::consts::CLIENT_SERVER_CONTRACT_VERSION,
                "bootstrap_version": yinz_runtime::consts::REMOTE_BOOTSTRAP_VERSION,
            })
            .to_string(),
        )
        .args([
            if watching { "watch" } else { "attach" },
            "--remote",
            "example",
            NAME,
        ])
        .stdin(slave.try_clone().unwrap())
        .process_group(0)
        .spawn()
        .unwrap();
    let screen = Watched::new(Vec::new());
    let captured = screen.clone();
    let mut stdout = child.stdout.take().unwrap();
    let capture = std::thread::spawn(move || {
        let mut buffer = [0; 4096];
        while let Ok(count) = stdout.read(&mut buffer) {
            if count == 0 {
                break;
            }
            captured.lock().extend_from_slice(&buffer[..count]);
        }
    });
    let client = Client {
        pid: Pid::from_raw(child.id() as i32),
        output: Watched::new(None),
    };
    let completed = client.output.clone();
    let changed = events.clone();
    let waiter = std::thread::spawn(move || {
        let output = child.wait_with_output().unwrap();
        *completed.lock() = Some(output);
        drop(changed.lock());
    });
    if stage == "watch-connect" {
        screen.wait_until(
            "watcher to claim the terminal",
            default_timeout(),
            |bytes| {
                bytes
                    .windows(b"\x1b[?1049h".len())
                    .any(|window| window == b"\x1b[?1049h")
                    .then_some(())
            },
        );
    } else {
        events.wait_until_described(
            "SSH stalled at the requested stage",
            default_timeout(),
            |events| {
                if let Some(output) = &*client.output.lock() {
                    panic!("client exited before {stage}: {output:?}");
                }
                events
                    .iter()
                    .any(|event| event.stage == "blocked")
                    .then_some(())
            },
            |events| format!("{events:?}"),
        );
    }
    if stage == "watch-config" {
        // Queue SIGINT while stopped, then make the configuration readable. On resume the
        // signal is delivered before userspace can continue into the client's guard.
        kill(client.pid, Signal::SIGSTOP).unwrap();
        assert!(matches!(
            nix::sys::wait::waitpid(client.pid, Some(nix::sys::wait::WaitPidFlag::WUNTRACED))
                .unwrap(),
            nix::sys::wait::WaitStatus::Stopped(_, Signal::SIGSTOP)
        ));
    }
    kill(client.pid, signal).unwrap();
    if stage == "watch-config" {
        resume_config.send(()).unwrap();
        peer_worker.take().unwrap().join().unwrap();
        kill(client.pid, Signal::SIGCONT).unwrap();
    }
    client
        .output
        .wait_until("interrupted client exit", default_timeout(), |out| {
            out.as_ref().map(|out| {
                assert_eq!(out.status.code(), Some(130), "{out:?}");
            })
        });
    waiter.join().unwrap();
    capture.join().unwrap();
    if let Some(worker) = peer_worker {
        worker.join().unwrap();
    }
    drop(UnixStream::connect(&events_path).unwrap());
    event_reader.join().unwrap();
    assert_eq!(nix::sys::termios::tcgetattr(&slave).unwrap(), original);
    for event in events.lock().iter() {
        assert_eq!(
            kill(Pid::from_raw(event.pid), None),
            Err(nix::errno::Errno::ESRCH),
            "SSH was not reaped: {event:?}"
        );
        if let Some(path) = &event.socket {
            assert!(!path.exists(), "forward socket survived: {path:?}");
        }
    }
    assert_eq!(
        fs::read_dir(root.join("sockets/client"))
            .map(|entries| entries.count())
            .unwrap_or(0),
        0
    );
}

#[test]
fn interrupt_before_remote_watch_client_guard_reaps_ssh_and_removes_socket() {
    interrupted_startup("watch-config", Signal::SIGINT);
}

#[test]
fn interrupt_after_remote_watch_client_guard_reaps_ssh_and_restores_terminal() {
    interrupted_startup("watch-connect", Signal::SIGINT);
}

#[test]
fn interrupts_during_remote_bootstrap_reap_ssh() {
    for signal in [
        Signal::SIGINT,
        Signal::SIGTERM,
        Signal::SIGQUIT,
        Signal::SIGHUP,
    ] {
        interrupted_startup("bootstrap", signal);
    }
}

#[test]
fn interrupt_during_forward_readiness_reaps_ssh_and_removes_socket() {
    interrupted_startup("forward", Signal::SIGTERM);
}

#[test]
fn interrupt_during_final_probe_reaps_ssh_and_removes_socket() {
    interrupted_startup("probe", Signal::SIGQUIT);
}

#[test]
fn interrupt_during_named_switch_reaps_ssh_and_removes_socket() {
    interrupted_startup("switch", Signal::SIGHUP);
}

const SSH_STUB: &str = r#"
import json, os, select, signal, socket, sys

stage = os.environ['YINZ_TEST_STAGE']
path = None
def report(stage):
    with socket.socket(socket.AF_UNIX) as events:
        events.connect(os.environ['YINZ_TEST_EVENTS'])
        events.sendall(json.dumps(dict(stage=stage, pid=os.getpid(), socket=path)).encode())

if '-L' not in sys.argv:
    if stage == 'bootstrap' or (stage == 'switch' and "'next'" in sys.argv[-1]):
        report('blocked')
        signal.pause()
    report('bootstrap')
    print(os.environ['YINZ_TEST_ENDPOINT'])
    sys.exit(0)

path = sys.argv[sys.argv.index('-L') + 1].split(':')[0]
listener = socket.socket(socket.AF_UNIX)
listener.bind(path)
report('forward')
if stage == 'forward':
    report('blocked')
    signal.pause()
listener.listen()
ready, _ = listener.accept()
ready.close()
while True:
    client, _ = listener.accept()
    with client:
        if stage == 'probe':
            assert client.recv(1)
            report('blocked')
            signal.pause()
        with socket.socket(socket.AF_UNIX) as remote:
            remote.connect(os.environ['YINZ_TEST_PEER'])
            connected = True
            while connected:
                for source in select.select([client, remote], [], [], 30)[0]:
                    data = source.recv(65536)
                    if not data:
                        connected = False
                        break
                    (remote if source is client else client).sendall(data)
"#;

/// Two separate session namespaces connected through an SSH-shaped Unix-socket relay.
/// The CLI still runs locally, so local-only input paths cannot be read by the peer.
#[test]
fn remote_cli_uses_forwarded_sessions_and_local_inputs() {
    let remote = Session::new();
    remote.remote_layout();
    yinz_steel_builtins::install(
        &yinz_steel_builtins::asset_dir(),
        &remote.0.path().join("data/yinz"),
    )
    .unwrap();
    let config_dir = remote.0.path().join("config/yinz");
    fs::create_dir_all(&config_dir).unwrap();
    fs::copy(
        remote.0.path().join("config.kdl"),
        config_dir.join("config.kdl"),
    )
    .unwrap();
    remote.remote_endpoint(true);
    remote.wait_for_panes(false);
    let local = Session::new();
    // A local default layout means nothing to a remote session, and layout names resolve here.
    let local_layouts = local.0.path().join("local-layouts");
    fs::create_dir(&local_layouts).unwrap();
    fs::write(local_layouts.join("only-local.kdl"), "layout { pane; }\n").unwrap();
    fs::write(
        local.0.path().join("config.kdl"),
        format!(
            "load_plugins clear-defaults=true\ndefault_layout \"/no/local/layout.kdl\"\nlayout_dir \"{}\"\n",
            local_layouts.display()
        ),
    )
    .unwrap();
    let stub = local.0.path().join("ssh.py");
    fs::write(&stub, CLI_SSH_STUB).unwrap();
    yinz_test_support::write_warm_script(
        &local.0.path().join("ssh"),
        "exec python3 \"$YINZ_TEST_RELAY\" \"$@\"",
    );
    let mut env: std::collections::BTreeMap<_, _> = remote
        .command()
        .get_envs()
        .filter_map(|(k, v)| {
            v.map(|v| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
        })
        .collect();
    // Exercise SSH configurations that forward the invoking shell's Yinz variables.
    env.insert("YINZ_PANE_REF".into(), "invalid-forwarded-pane".into());
    let mut path = vec![local.0.path().to_path_buf()];
    path.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let path = std::env::join_paths(path).unwrap();
    let command = |args: &[&str]| {
        let mut cmd = local.command();
        cmd.env("PATH", &path)
            .env("YINZ_TEST_RELAY", &stub)
            .env("YINZ_TEST_REMOTE_ENV", serde_json::to_string(&env).unwrap())
            .env("YINZ_TEST_BINARY", env!("CARGO_BIN_EXE_yinz"))
            .env("YINZ_TEST_FORWARD_LOG", local.0.path().join("forwards"))
            // Invalid inherited local context must have no bearing on remote selection.
            .env("YINZ_PANE_REF", "invalid-local-pane")
            .args(["--remote", "fixture"])
            .args(args);
        cmd
    };
    let call = |args: &[&str]| {
        let output = command(args).output().unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
        output
    };
    let out = call(&["action", "list-panes", "--json"]);
    let panes: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let pane = panes
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["is_plugin"] == false)
        .unwrap()["id"]
        .as_u64()
        .unwrap_or(1)
        .to_string();
    call(&["agent", "list", "--json"]);
    // Nothing was sent, which `--json` callers read like any other rejection.
    let rejected = command(&["--session", "../bad", "agent", "list", "--json"])
        .output()
        .unwrap();
    assert!(!rejected.status.success(), "{rejected:?}");
    let response: serde_json::Value = serde_json::from_slice(&rejected.stdout).unwrap();
    assert_eq!(response["outcome"], "rejected", "{rejected:?}");
    // The server would otherwise start an agent with no directory in `/`.
    let spawn = command(&["agent", "spawn", "--kind", "codex"])
        .output()
        .unwrap();
    assert!(!spawn.status.success(), "{spawn:?}");
    assert!(
        String::from_utf8_lossy(&spawn.stderr).contains("--cwd or --project"),
        "{spawn:?}"
    );
    call(&["data", "catalog"]);
    let fragment = local.0.path().join("fragment.kdl");
    fs::write(&fragment, "pane_frames false\n").unwrap();
    call(&[
        "action",
        "reconfigure",
        "--file",
        fragment.to_str().unwrap(),
    ]);
    let out = call(&["repl", "--eval", "(+ 20 22)"]);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("42"),
        "{out:?}"
    );
    call(&["plugins", "list", "--running", "--json"]);
    call(&[
        "action",
        "agent-report",
        "--pane-id",
        &pane,
        "--kind",
        "claude",
        "--state",
        "idle",
    ]);
    // SSH gives the host no terminal, so the question is asked here; nobody can answer this one.
    let refused = command(&["kill-all-sessions"]).output().unwrap();
    assert!(!refused.status.success(), "{refused:?}");
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("--yes"),
        "{refused:?}"
    );
    let out = call(&["list-sessions", "--short"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains(NAME));
    let created = call(&[
        "run",
        "--",
        "/bin/sh",
        "-c",
        "printf remote-stream-marker; read ignored",
    ]);
    let created = String::from_utf8(created.stdout).unwrap();
    let pane_id = created.lines().last().unwrap().trim();
    for args in [
        vec!["subscribe", "--pane-id", pane_id],
        vec!["events", "--type", "agent_lifecycle"],
    ] {
        let mut child = command(&args).spawn().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let content = Watched::new(Vec::<u8>::new());
        let captured = content.clone();
        let reader = std::thread::spawn(move || {
            let mut buffer = [0; 4096];
            while let Ok(count) = stdout.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                captured.lock().extend_from_slice(&buffer[..count]);
            }
        });
        content.wait_until(
            &format!("remote stream content for {args:?}"),
            default_timeout(),
            |bytes| {
                let text = String::from_utf8_lossy(bytes);
                (text.contains("remote-stream-marker") || text.contains("agent_lifecycle"))
                    .then_some(())
            },
        );
        kill(Pid::from_raw(child.id() as i32), Signal::SIGTERM).unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(128 + signal_hook::consts::SIGTERM),
            "{output:?}"
        );
        reader.join().unwrap();
    }
    // Watchers mirror an interactive client; a background-only session has no view to follow.
    let socket = yinz_ipc::consts::ipc_connect(&remote.socket()).unwrap();
    socket.set_recv_timeout(Some(default_timeout())).unwrap();
    let mut source = IpcSenderWithContext::<ClientToServerMsg>::new(socket);
    let mut source_messages = source.get_receiver::<ServerToClientMsg>().unwrap();
    source
        .send_client_msg(ClientToServerMsg::AttachClient {
            cli_assets: yinz_types::input::cli_assets::CliAssets {
                config_file_path: Some(remote.0.path().join("config.kdl")),
                config_dir: Some(remote.0.path().join("config")),
                data_dir: Some(remote.0.path().join("data/yinz")),
                terminal_window_size: yinz_types::pane_size::Size { rows: 24, cols: 80 },
                ..Default::default()
            },
            tab_number_to_focus: None,
            pane_to_focus: None,
        })
        .unwrap();
    let source_reader = std::thread::spawn(move || {
        while let IpcRecv::Msg(message, _) = source_messages.recv_server_msg() {
            if matches!(message, ServerToClientMsg::Exit { .. }) {
                break;
            }
        }
    });
    let pty = nix::pty::openpty(
        Some(&nix::pty::Winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }),
        None,
    )
    .unwrap();
    let slave = fs::File::from(pty.slave);
    let mut master = fs::File::from(pty.master);
    let mut watcher = command(&["watch"])
        .stdin(slave.try_clone().unwrap())
        .stdout(slave.try_clone().unwrap())
        .stderr(slave.try_clone().unwrap())
        .process_group(0)
        .spawn()
        .unwrap();
    drop(slave);
    let content = Watched::new(Vec::<u8>::new());
    let captured = content.clone();
    let reader = std::thread::spawn(move || {
        let mut buffer = [0; 4096];
        while let Ok(count) = master.read(&mut buffer) {
            if count == 0 {
                break;
            }
            captured.lock().extend_from_slice(&buffer[..count]);
        }
    });
    content.wait_until_described(
        "remote watcher render",
        default_timeout(),
        |bytes| {
            String::from_utf8_lossy(bytes)
                .contains("remote-stream-marker")
                .then_some(())
        },
        |bytes| String::from_utf8_lossy(bytes).into_owned(),
    );
    killpg(Pid::from_raw(watcher.id() as i32), Signal::SIGTERM).unwrap();
    watcher.wait().unwrap();
    reader.join().unwrap();
    call(&["action", "new-tab", "--no-focus", "--layout", "only-local"]);
    source
        .send_client_msg(ClientToServerMsg::ClientExited)
        .unwrap();
    drop(source);
    source_reader.join().unwrap();
    call(&["--session", NAME, "kill-session", NAME]);
    assert!(!remote.socket().exists());
    for socket in fs::read_to_string(local.0.path().join("forwards"))
        .unwrap()
        .lines()
    {
        assert!(
            !std::path::Path::new(socket).exists(),
            "forward leaked: {socket}"
        );
    }
    assert!(!local.socket().exists());
}

const CLI_SSH_STUB: &str = r#"
import json, os, select, shlex, socket, sys, threading
if '-L' not in sys.argv:
    args = shlex.split(sys.argv[-1])
    binary = os.environ['YINZ_TEST_BINARY']
    os.execve(binary, [binary] + args[1:], json.loads(os.environ['YINZ_TEST_REMOTE_ENV']))
path, peer = sys.argv[sys.argv.index('-L') + 1].split(':', 1)
with open(os.environ['YINZ_TEST_FORWARD_LOG'], 'a') as log:
    log.write(path + '\n')
listener = socket.socket(socket.AF_UNIX)
listener.bind(path)
listener.listen()
def relay(client):
    with client, socket.socket(socket.AF_UNIX) as remote:
        remote.connect(peer)
        while True:
            for source in select.select([client, remote], [], [])[0]:
                data = source.recv(65536)
                if not data:
                    return
                (remote if source is client else client).sendall(data)
while True:
    client, _ = listener.accept()
    threading.Thread(target=relay, args=(client,), daemon=True).start()
"#;
