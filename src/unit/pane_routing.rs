//! Executable routing checks with supervisor and session peers.
use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::JoinHandle,
    time::Duration,
};
use yinz_ipc::ipc::{ClientToServerMsg, IpcReceiverWithContext, IpcRecv, ServerToClientMsg};
use yinz_process::pty_host::protocol::{
    read_msg, write_msg, HostStatus, HostToServerMsg, ServerToHostMsg,
};
use yinz_types::{
    data::PaneId,
    input::actions::Action,
    internal_ids::{ProcessId, SupervisorId, TerminalId},
};

pub(super) struct Supervisor {
    pub status: Arc<Mutex<HostStatus>>,
    stop: Arc<AtomicBool>,
    path: PathBuf,
    thread: Option<JoinHandle<()>>,
}
impl Supervisor {
    pub fn new(root: &Path, session: &str, terminal: u32) -> Self {
        let path = root
            .join(&*yinz_runtime::consts::CLIENT_SERVER_CONTRACT_DIR)
            .join("pty/abcdef012345.sock");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let status = Arc::new(Mutex::new(HostStatus {
            host_id: SupervisorId::new("abcdef012345"),
            session_name: Some(session.into()),
            terminal_id: terminal,
            command: "shell".into(),
            cwd: None,
            child_pid: None,
            host_pid: ProcessId::new(1),
            started_at_unix: yinz_types::units::UnixSeconds::new(0),
            attached: true,
            supports_relabel: true,
            child_exit_status: None,
            child_group_terminated: false,
            buffered_bytes: 0,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (state, done) = (status.clone(), stop.clone());
        let thread = std::thread::spawn(move || loop {
            let (mut stream, _) = listener.accept().unwrap();
            if done.load(Ordering::SeqCst) {
                break;
            }
            assert!(matches!(
                read_msg::<ServerToHostMsg, _>(&mut stream).unwrap(),
                Some(ServerToHostMsg::Query)
            ));
            write_msg(
                &mut stream,
                &HostToServerMsg::Status(state.lock().unwrap().clone()),
            )
            .unwrap();
        });
        Self {
            status,
            stop,
            path,
            thread: Some(thread),
        }
    }
}
impl Drop for Supervisor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = std::os::unix::net::UnixStream::connect(&self.path);
        self.thread.take().unwrap().join().unwrap();
    }
}
struct Session {
    probes: Arc<AtomicUsize>,
    messages: mpsc::Receiver<ClientToServerMsg>,
    stop: Arc<AtomicBool>,
    path: PathBuf,
    thread: Option<JoinHandle<()>>,
}
impl Session {
    fn new(root: &Path, name: &str) -> Self {
        Self::with_unresponsive_probes(root, name, 0)
    }
    fn with_unresponsive_probes(root: &Path, name: &str, unresponsive: usize) -> Self {
        let path = root
            .join(&*yinz_runtime::consts::CLIENT_SERVER_CONTRACT_DIR)
            .join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let listener = ListenerOptions::new()
            .name(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
            .create_sync()
            .unwrap();
        let (tx, messages) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let probes = Arc::new(AtomicUsize::new(0));
        let observed = probes.clone();
        let thread = std::thread::spawn(move || loop {
            let socket = listener.accept().unwrap();
            if done.load(Ordering::SeqCst) {
                break;
            }
            let mut receiver = IpcReceiverWithContext::<ClientToServerMsg>::new(socket);
            let mut sender = receiver.get_sender::<ServerToClientMsg>().unwrap();
            let IpcRecv::Msg(message, _) = receiver.recv_client_msg() else {
                continue;
            };
            if matches!(message, ClientToServerMsg::ConnStatus)
                && observed.fetch_add(1, Ordering::SeqCst) < unresponsive
            {
                // Withhold the reply until the CLI's liveness deadline closes the probe.
                assert!(matches!(receiver.recv_client_msg(), IpcRecv::Eof));
                continue;
            }
            let reply = match &message {
                ClientToServerMsg::ConnStatus => ServerToClientMsg::Connected {
                    warp_companions: true,
                    warp_held_terminals: true,
                },
                ClientToServerMsg::AgentControl(_) => {
                    ServerToClientMsg::AgentControl(yinz_types::agent::control::AgentResponse::new(
                        yinz_types::agent::control::AgentOutcome::Ok,
                    ))
                },
                ClientToServerMsg::AgentReport(report) => ServerToClientMsg::AgentReportAccepted {
                    report_id: report.report_id.clone(),
                },
                ClientToServerMsg::Action {
                    action: Action::CliPipe { pipe_id, .. },
                    ..
                } => ServerToClientMsg::CliPipeOutput {
                    pipe_name: pipe_id.clone(),
                    output: r#"{"ok":true,"data":{}}"#.into(),
                },
                _ => ServerToClientMsg::UnblockInputThread,
            };
            if !matches!(message, ClientToServerMsg::ConnStatus) {
                tx.send(message).unwrap();
            }
            sender.send_server_msg(reply).unwrap();
        });
        Self {
            probes,
            messages,
            stop,
            path,
            thread: Some(thread),
        }
    }
    fn receive(&self) -> ClientToServerMsg {
        self.messages.recv_timeout(Duration::from_secs(10)).unwrap()
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = yinz_ipc::consts::ipc_connect(&self.path);
        self.thread.take().unwrap().join().unwrap();
    }
}
fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_yinz"));
    for key in [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "YINZ_SOCKET_DIR",
        "TMPDIR",
    ] {
        command.env(key, root);
    }
    command
        .env("YINZ_SESSION_NAME", "old")
        .env("YINZ_PANE_ID", "999")
        .env("YINZ_PANE_REF", "abcdef012345")
        .env_remove("YINZ_REMOTE_PANE_TOKEN")
        .stdin(Stdio::null());
    command
}

#[test]
fn actions_reports_and_store_requests_use_the_current_owner() {
    let dir = tempfile::tempdir().unwrap();
    let owner = Session::new(dir.path(), "owner");
    let old = Session::new(dir.path(), "old");
    let _supervisor = Supervisor::new(dir.path(), "owner", 42);
    for args in [
        vec!["action", "write-chars", "hello"],
        vec!["action", "notify", "task finished"],
        vec!["action", "agent-report", "--state", "busy"],
        vec!["agent", "register", "terminal_42", "codex"],
    ] {
        let output = command(dir.path()).args(&args).output().unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        match owner.receive() {
            ClientToServerMsg::Action { terminal_id, .. } => {
                assert_eq!(terminal_id, Some(TerminalId::new(42)))
            },
            ClientToServerMsg::AgentReport(report) => {
                assert_eq!(report.pane_id, PaneId::Terminal(TerminalId::new(42)))
            },
            other => panic!("{other:?}"),
        }
        assert!(old.messages.try_recv().is_err());
    }
    // An explicit pane changes only the pane, never implicit session selection.
    assert!(command(dir.path())
        .args(["action", "agent-report", "--pane-id", "7", "--clear"])
        .output()
        .unwrap()
        .status
        .success());
    assert!(
        matches!(owner.receive(), ClientToServerMsg::AgentReport(report) if report.pane_id == PaneId::Terminal(TerminalId::new(7)))
    );
    for reference in [Some("abcdef012345"), Some("invalid"), None] {
        for args in [
            vec!["action", "write-chars", "hello"],
            vec!["agent", "register", "7", "codex"],
        ] {
            let mut explicit = command(dir.path());
            match reference {
                Some(reference) => {
                    explicit.env("YINZ_PANE_REF", reference);
                },
                None => {
                    explicit.env_remove("YINZ_PANE_REF");
                },
            }
            let output = explicit
                .args(["--session", "old"])
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(matches!(
                old.receive(),
                ClientToServerMsg::Action {
                    terminal_id: None,
                    ..
                }
            ));
        }
    }
}

#[test]
fn invalid_or_missing_owner_never_reaches_the_only_available_session() {
    let dir = tempfile::tempdir().unwrap();
    let fallback = Session::new(dir.path(), "old");
    let supervisor = Supervisor::new(dir.path(), "missing", 42);
    for case in [
        "missing-session",
        "detached",
        "missing-owner",
        "mismatch",
        "malformed",
        "absent",
        "no-reference",
    ] {
        let mut command = command(dir.path());
        match case {
            "detached" => supervisor.status.lock().unwrap().attached = false,
            "missing-owner" => {
                let mut status = supervisor.status.lock().unwrap();
                status.attached = true;
                status.session_name = None;
            },
            "mismatch" => {
                supervisor.status.lock().unwrap().host_id = SupervisorId::new("000000000000")
            },
            "malformed" => {
                command.env("YINZ_PANE_REF", "invalid");
            },
            "absent" => {
                command.env("YINZ_PANE_REF", "111111111111");
            },
            "no-reference" => {
                command.env_remove("YINZ_PANE_REF");
            },
            _ => {},
        }
        let output = command
            .args(["action", "write-chars", "hello"])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{case}");
        assert!(fallback.messages.try_recv().is_err(), "{case}");
    }
    let mut outside = command(dir.path());
    for key in ["YINZ", "YINZ_PANE_REF", "YINZ_PANE_ID", "YINZ_SESSION_NAME"] {
        outside.env_remove(key);
    }
    assert!(outside
        .args(["action", "write-chars", "hello"])
        .output()
        .unwrap()
        .status
        .success());
    assert!(matches!(
        fallback.receive(),
        ClientToServerMsg::Action {
            terminal_id: None,
            ..
        }
    ));
}

#[test]
fn delayed_observer_resolves_the_pane_again_after_it_moves() {
    use std::io::{BufRead, BufReader, Write};
    let dir = tempfile::tempdir().unwrap();
    let initial = Session::new(dir.path(), "owner");
    let destination = Session::new(dir.path(), "destination");
    let supervisor = Supervisor::new(dir.path(), "owner", 42);
    let transcript = dir.path().join("transcript.jsonl");
    std::fs::write(&transcript, "").unwrap();
    let hook = dir.path().join("hook.json");
    std::fs::write(&hook, serde_json::json!({
        "hook_event_name":"UserPromptSubmit", "session_id":"conversation", "prompt_id":"prompt", "transcript_path":transcript,
        "prompt":"review"
    }).to_string()).unwrap();
    let env = command(dir.path());
    let mut claude = Command::new("bash")
        .args(["-c", r#"exec -a claude bash -c '"$1" action agent-report --hook-json < "$2"; echo ready; read -r hold' -- "$@""#, "--"])
        .arg(env!("CARGO_BIN_EXE_yinz")).arg(&hook)
        .envs(env.get_envs().filter_map(|(key, value)| value.map(|value| (key, value))))
        .env_remove("YINZ_REMOTE_PANE_TOKEN")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut ready = String::new();
    BufReader::new(claude.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready.trim(), "ready");
    assert!(
        matches!(initial.receive(), ClientToServerMsg::AgentReport(report) if report.pane_id == PaneId::Terminal(TerminalId::new(42)))
    );
    {
        let mut status = supervisor.status.lock().unwrap();
        status.session_name = Some("destination".into());
        status.terminal_id = 8;
    }
    std::fs::write(&transcript, concat!(
        r#"{"type":"user","sessionId":"conversation","promptId":"prompt","uuid":"user"}"#, "\n",
        r#"{"type":"system","subtype":"turn_duration","sessionId":"conversation","parentUuid":"user"}"#, "\n"
    )).unwrap();
    let ClientToServerMsg::AgentReport(report) = destination.receive() else {
        panic!("expected completion");
    };
    assert_eq!(report.pane_id, PaneId::Terminal(TerminalId::new(8)));
    let hook: yinz_types::agent::HookEvent =
        serde_json::from_str(&report.event.unwrap().raw).unwrap();
    assert_eq!(hook.claude_turn_complete, Some(true));
    writeln!(claude.stdin.take().unwrap(), "exit").unwrap();
    assert!(claude.wait().unwrap().success());
    assert!(initial.messages.try_recv().is_err());
}

#[test]
fn implicit_streams_and_layouts_require_a_pane_reference() {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        vec!["events"],
        vec!["subscribe", "--pane-id", "1"],
        vec!["watch"],
        vec!["--layout-string", "layout { pane; }"],
        vec!["pty-host", "adopt", "abcdef012345"],
    ] {
        let output = command(dir.path())
            .env_remove("YINZ_PANE_REF")
            .args(&args)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{args:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("YINZ_PANE_REF"),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn named_actions_and_commands_wait_for_readiness_without_probing_other_sessions() {
    for explicit in [false, true] {
        for args in [vec!["action", "write-chars", "hello"], vec!["events"]] {
            let dir = tempfile::tempdir().unwrap();
            let owner = Session::with_unresponsive_probes(dir.path(), "owner", 2);
            let unrelated = Session::new(dir.path(), "other");
            let _supervisor = Supervisor::new(dir.path(), "owner", 42);
            let mut cmd = command(dir.path());
            if explicit {
                cmd.args(["--session", "owner"]);
            }
            let output = cmd.args(&args).output().unwrap();
            // The fake peer closes event streams after acknowledging the request.
            if args[0] == "action" {
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(matches!(owner.receive(), ClientToServerMsg::Action { .. }));
            } else {
                assert!(matches!(
                    owner.receive(),
                    ClientToServerMsg::SubscribeToEvents { .. }
                ));
            }
            assert_eq!(owner.probes.load(Ordering::SeqCst), 3);
            assert!(
                owner.messages.try_recv().is_err(),
                "dispatched more than once"
            );
            assert_eq!(unrelated.probes.load(Ordering::SeqCst), 0);
        }
    }
}

#[cfg(target_os = "linux")]
#[path = "bubblewrap_cli.rs"]
mod bubblewrap_cli;
