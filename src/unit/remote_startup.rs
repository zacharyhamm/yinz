//! Remote startup regressions with isolated servers and interruptible SSH stubs.
use super::*;
use interprocess::local_socket::traits::Stream;
use nix::sys::signal::{kill, killpg, Signal};
use nix::unistd::Pid;
use std::io::Read;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use yinz_test_support::{default_timeout, Watched};

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
    let switching = (stage == "switch").then(|| {
        std::thread::spawn(move || {
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
        })
    });
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
    let child = session
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
            })
            .to_string(),
        )
        .args(["attach", "--remote", "example", NAME])
        .stdin(slave.try_clone().unwrap())
        .process_group(0)
        .spawn()
        .unwrap();
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
    kill(client.pid, signal).unwrap();
    client
        .output
        .wait_until("interrupted client exit", default_timeout(), |out| {
            out.as_ref().map(|out| {
                assert_eq!(out.status.code(), Some(130), "{out:?}");
            })
        });
    waiter.join().unwrap();
    if let Some(switching) = switching {
        switching.join().unwrap();
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
