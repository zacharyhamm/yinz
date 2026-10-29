//! Real reporter processes and fake peers. No sleeps: sockets and gates synchronize the tests.
use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions};
use interprocess::TryClone;
use std::{
    io::Write,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use {
    yinz_ipc::ipc::ClientToServerMsg, yinz_ipc::ipc::IpcReceiverWithContext,
    yinz_ipc::ipc::IpcRecv, yinz_ipc::ipc::ServerToClientMsg,
    yinz_types::agent::report::AgentReportState,
};

fn command(dir: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_yinz"));
    cmd.args([
        "--session",
        "report-test",
        "action",
        "agent-report",
        "--pane-id",
        "7",
    ])
    .env("YINZ_SOCKET_DIR", dir)
    .env("TMPDIR", dir)
    .env("HOME", dir)
    .env("XDG_CONFIG_HOME", dir)
    .env("YINZ_HOOK_TRACE", "1")
    .env_remove("YINZ_PANE_REF")
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    cmd
}

/// Pay macOS's first-exec cost before a test arms its clock.
///
/// macOS validates a freshly built binary's code signature on its first exec and caches it per
/// file: measured here at 5.4s for the 138MB debug `yinz`, against 50ms for every later exec.
/// A test whose subject is a deadline must not be timing that. Same trap, and same remedy, as
/// `yinz_test_support::write_warm_script`.
fn warm_binary() {
    static WARM: std::sync::Once = std::sync::Once::new();
    WARM.call_once(|| {
        Command::new(env!("CARGO_BIN_EXE_yinz"))
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("failed to warm the yinz binary");
    });
}

fn listener(dir: &std::path::Path) -> interprocess::local_socket::Listener {
    let socket_dir = dir.join(&*yinz_runtime::consts::CLIENT_SERVER_CONTRACT_DIR);
    std::fs::create_dir_all(&socket_dir).unwrap();
    ListenerOptions::new()
        .name(
            socket_dir
                .join("report-test")
                .to_fs_name::<GenericFilePath>()
                .unwrap(),
        )
        .create_sync()
        .unwrap()
}

fn agent_command(dir: &std::path::Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_yinz"));
    command
        .args(["--session", "report-test", "agent"])
        .args(args)
        .env("YINZ_SOCKET_DIR", dir)
        .env("TMPDIR", dir)
        .env("HOME", dir)
        .env("XDG_CONFIG_HOME", dir)
        .env_remove("YINZ_PANE_REF")
        .env_remove("YINZ_PANE_ID")
        .env_remove("YINZ_SESSION_NAME")
        .env_remove("YINZ_REMOTE_PANE_TOKEN")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn write_event(child: &mut Child, json: &str) {
    child
        .stdin
        .take()
        .unwrap()
        .write_all(json.as_bytes())
        .unwrap();
}

#[test]
fn daemon_hooks_use_the_registered_conversation_and_ignore_inherited_targets() {
    warm_binary();
    for (daemon_runtime, hook) in [
        (false, "UserPromptSubmit"),
        (true, "UserPromptSubmit"),
        (false, "StopFailure"),
        (true, "StopFailure"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let launcher_sockets = dir.path().join("launcher");
        let listener = listener(&launcher_sockets);
        let _supervisor = pane_routing::Supervisor::new(&launcher_sockets, "report-test", 15);
        let registry = std::path::Path::new("/tmp")
            .join(format!("yinz-{}", nix::unistd::Uid::current()))
            .join("codex");
        std::fs::create_dir_all(&registry).unwrap();
        let registration = tempfile::tempdir_in(registry).unwrap();
        let lease = std::fs::File::create(registration.path().join("lease")).unwrap();
        lease.lock().unwrap();
        let thread = uuid::Uuid::new_v4().to_string();
        let binding = serde_json::json!({
                "thread_id": thread,
                "env": {
                    "YINZ_PANE_REF":"abcdef012345",
                    "YINZ_SOCKET_DIR": launcher_sockets
                }
        });
        std::fs::write(
            registration.path().join("binding.json"),
            binding.to_string(),
        )
        .unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_yinz"));
        command
            .args(["agent", "codex-report"])
            .env("YINZ_SOCKET_DIR", dir.path().join("daemon"))
            .env("TMPDIR", dir.path())
            .env_remove("XDG_RUNTIME_DIR")
            .env("YINZ_PANE_REF", "aaaaaaaaaaaa")
            .env("YINZ_SESSION_NAME", "old-session")
            .env("YINZ_PANE_ID", "20")
            .env("YINZ_REMOTE_PANE_TOKEN", "old-remote-pane")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if daemon_runtime {
            command.env("XDG_RUNTIME_DIR", dir.path().join("daemon-runtime"));
        }
        let mut child = command.spawn().unwrap();
        write_event(
            &mut child,
            &serde_json::json!({
                "hook_event_name":hook, "session_id": thread, "turn_id":"turn",
                "prompt":"PRIVATE PROMPT", "last_assistant_message":"HTTP 400: unsupported model"
            })
            .to_string(),
        );
        let mut receiver =
            IpcReceiverWithContext::<ClientToServerMsg>::new(listener.accept().unwrap());
        let report = match receiver.recv_client_msg() {
            IpcRecv::Msg(ClientToServerMsg::AgentReport(report), _) => report,
            _ => panic!("expected mapped report"),
        };
        assert_eq!(
            report.pane_id,
            yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(15))
        );
        if hook == "StopFailure" {
            assert!(matches!(report.state, AgentReportState::Set(ref agent)
                if agent.state == yinz_types::agent::AgentState::Idle));
        }
        let event = report.event.unwrap();
        assert_eq!(event.session_id.as_deref(), Some(thread.as_str()));
        assert_eq!(event.kind, "codex");
        assert_eq!(event.hook, hook);
        let raw: yinz_types::agent::HookEvent = serde_json::from_str(&event.raw).unwrap();
        assert_eq!(raw.turn_id.as_deref(), Some("turn"));
        assert_eq!(
            event.last_message.as_deref(),
            Some("HTTP 400: unsupported model")
        );
        receiver
            .get_sender::<ServerToClientMsg>()
            .unwrap()
            .send_server_msg(ServerToClientMsg::AgentReportAccepted {
                report_id: report.report_id,
            })
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
        assert!(!diagnostics(dir.path()).contains("PRIVATE PROMPT"));
    }
}

#[test]
fn bypassed_codex_launches_preserve_arguments_and_socket_config_is_not_pane_identity() {
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("codex");
    yinz_test_support::write_warm_script(&program, "printf '%s\\n' \"$@\"\nexit 23\n");
    let cases: &[(&[&str], bool)] = &[
        (&["prompt without a pane"], false),
        (&["-c", "model=example", "exec", "prompt with spaces"], true),
        (&["--profile=work", "a", "task"], true),
        (&["--add-dir", "a directory", "prompt"], true),
        (
            &[
                "-c",
                "sandbox_workspace_write.writable_roots=['/work']",
                "prompt",
            ],
            true,
        ),
        (
            &[
                "--config",
                " sandbox_workspace_write.writable_roots = []",
                "prompt",
            ],
            true,
        ),
        (
            &["-csandbox_workspace_write.writable_roots=['/work with spaces']"],
            true,
        ),
        (&["-c=sandbox_workspace_write.writable_roots=[]"], true),
        (
            &["--config=sandbox_workspace_write.writable_roots=[]"],
            true,
        ),
        (&["--remote=unix:///custom", "resume", "--last"], true),
        (&["--new-option", "value"], true),
    ];
    for (args, pane) in cases {
        let mut command = Command::new(env!("CARGO_BIN_EXE_yinz"));
        command
            .args(["agent", "codex", "--command"])
            .arg(&program)
            .arg("--")
            .args(*args)
            .env("YINZ_SOCKET_DIR", dir.path())
            .env("TMPDIR", dir.path())
            .env("YINZ_SESSION_NAME", "session-without-a-pane")
            .env_remove("YINZ_PANE_REF")
            .env_remove("YINZ_PANE_ID")
            .env_remove("YINZ_REMOTE_PANE_TOKEN");
        if *pane {
            command.env("YINZ_PANE_ID", "15");
        }
        let output = command.output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(23),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert_eq!(stdout, format!("{}\n", args.join("\n")));
    }
}

#[test]
fn standalone_codex_hooks_reject_reference_less_panes() {
    use std::io::{BufRead, BufReader};
    use std::os::unix::{net::UnixStream, process::CommandExt};

    const CHILD_ENV: &str = "YINZ_TEST_CODEX_PANE";
    if let Some(root) = std::env::var_os(CHILD_ENV) {
        let root = std::path::Path::new(&root);
        let mut peer = UnixStream::connect(root.join("test.sock")).unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        let mut replies = BufReader::new(peer.try_clone().unwrap());
        assert!(std::env::var_os("YINZ_PANE_REF").is_none());
        let pane = std::env::var("YINZ_PANE_ID").unwrap();
        for case in [
            "accepted",
            "missing-pane",
            "invalid-pane",
            "missing-session",
            "invalid-session",
            "mismatched-session",
            "invalid-reference",
            "empty-reference",
            "remote-token",
            "empty-remote-token",
            "detached-daemon",
        ] {
            let mut reporter = Command::new(root.join("yinz"));
            reporter
                .args(["agent", "codex-report"])
                .env("YINZ_SOCKET_DIR", root.join("socket-alias"))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            match case {
                "missing-pane" => {
                    reporter.env_remove("YINZ_PANE_ID");
                },
                "invalid-pane" => {
                    reporter.env("YINZ_PANE_ID", "invalid");
                },
                "missing-session" => {
                    reporter.env_remove("YINZ_SESSION_NAME");
                },
                "invalid-session" => {
                    reporter.env("YINZ_SESSION_NAME", "..");
                },
                "mismatched-session" => {
                    reporter.env("YINZ_SESSION_NAME", "other-session");
                },
                "invalid-reference" => {
                    reporter.env("YINZ_PANE_REF", "invalid");
                },
                "empty-reference" => {
                    reporter.env("YINZ_PANE_REF", "");
                },
                "remote-token" => {
                    reporter.env("YINZ_REMOTE_PANE_TOKEN", "stale-token");
                },
                "empty-remote-token" => {
                    reporter.env("YINZ_REMOTE_PANE_TOKEN", "");
                },
                "detached-daemon" => unsafe {
                    // Keep the live pane and its environment, but leave its Unix session.
                    reporter.pre_exec(|| nix::unistd::setsid().map(|_| ()).map_err(Into::into));
                },
                _ => {},
            }
            let mut child = reporter.spawn().unwrap();
            write_event(
                &mut child,
                &serde_json::json!({
                    "hook_event_name": "UserPromptSubmit", "session_id": case,
                    "cwd": "/work/repo", "prompt": "private prompt"
                })
                .to_string(),
            );
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{case}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.stdout.is_empty());
            writeln!(peer, "{pane} {case}").unwrap();
            let mut ack = String::new();
            assert!(replies.read_line(&mut ack).unwrap() > 0);
        }
        return;
    }

    // The real server owns the PTY. Re-exec this test inside it so the hook and the
    // detached process start with exactly the environment an ordinary pane inherits.
    struct Session(tempfile::TempDir);
    impl Session {
        fn command(&self) -> Command {
            let mut command = Command::new(env!("CARGO_BIN_EXE_yinz"));
            for key in [
                "HOME",
                "XDG_CONFIG_HOME",
                "XDG_DATA_HOME",
                "XDG_CACHE_HOME",
                "TMPDIR",
                "YINZ_SOCKET_DIR",
            ] {
                command.env(key, self.0.path());
            }
            for key in [
                "YINZ",
                "YINZ_PANE_ID",
                "YINZ_PANE_REF",
                "YINZ_SESSION_NAME",
                "YINZ_REMOTE_PANE_TOKEN",
            ] {
                command.env_remove(key);
            }
            command
                .arg("--config-dir")
                .arg(self.0.path())
                .env(CHILD_ENV, self.0.path())
                .env("SHELL", "/bin/sh")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            command
        }
    }
    impl Drop for Session {
        fn drop(&mut self) {
            let _ = self
                .command()
                .args(["kill-session", "report-test"])
                .output();
        }
    }
    warm_binary();
    let session = Session(tempfile::tempdir().unwrap());
    let root = session.0.path();
    std::os::unix::fs::symlink(root, root.join("socket-alias")).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_yinz"), root.join("yinz")).unwrap();
    let socket_dir = root.join(&*yinz_runtime::consts::CLIENT_SERVER_CONTRACT_DIR);
    std::fs::create_dir_all(&socket_dir).unwrap();
    let _other_session =
        std::os::unix::net::UnixListener::bind(socket_dir.join("other-session")).unwrap();
    let listener = std::os::unix::net::UnixListener::bind(root.join("test.sock")).unwrap();
    let (connected, connection) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = connected.send(listener.accept());
    });
    let config = root.join("config.kdl");
    std::fs::write(
        &config,
        "detachable_panes false\nload_plugins clear-defaults=true\n",
    )
    .unwrap();
    let layout = root.join("layout.kdl");
    std::fs::write(&layout, format!(
        "layout {{\n pane command={:?} {{\n args \"--exact\" \"standalone_codex_hooks_reject_reference_less_panes\"\n }}\n}}\n",
        std::env::current_exe().unwrap().to_str().unwrap()
    )).unwrap();
    let output = session
        .command()
        .arg("--config")
        .arg(&config)
        .arg("--layout")
        .arg(&layout)
        .args(["attach", "--create-background", "report-test"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (mut peer, _) = connection
        .recv_timeout(Duration::from_secs(30))
        .unwrap()
        .unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    let mut reports = BufReader::new(peer.try_clone().unwrap());
    let mut count = 0;
    loop {
        let mut line = String::new();
        if reports.read_line(&mut line).unwrap() == 0 {
            break;
        }
        let (pane, case) = line.trim().split_once(' ').unwrap();
        let output = session
            .command()
            .args(["--session", "report-test", "action", "list-panes", "--json"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{case}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let panes: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let pane = panes
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| {
                entry["id"] == pane.parse::<u32>().unwrap() && entry["is_plugin"] == false
            })
            .unwrap();
        assert!(pane["agent"].is_null(), "{case}: {pane}");
        count += 1;
        writeln!(peer, "ok").unwrap();
    }
    assert_eq!(count, 11, "the pane must complete every ownership case");
}

fn diagnostics(dir: &std::path::Path) -> String {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| {
            let path = entry.ok()?.path().join("yinz-log/yinz.log");
            std::fs::read_to_string(path).ok()
        })
        .collect()
}

#[test]
fn reporter_requires_its_exact_acceptance_and_bounds_stalled_peers() {
    warm_binary();
    for response in [
        "accepted",
        "mismatch",
        "generic",
        "rejected",
        "malformed",
        "eof",
        "stalled",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let listener = listener(dir.path());
        let started = Instant::now();
        let mut child = command(dir.path()).arg("--hook-json").spawn().unwrap();
        write_event(
            &mut child,
            r#"{"hook_event_name":"Stop","prompt":"PRIVATE PROMPT"}"#,
        );
        let peer = listener.accept().unwrap();
        let mut raw = if response == "malformed" {
            Some(peer.try_clone().unwrap())
        } else {
            None
        };
        let mut receiver = IpcReceiverWithContext::<ClientToServerMsg>::new(peer);
        let report = match receiver.recv_client_msg() {
            IpcRecv::Msg(ClientToServerMsg::AgentReport(report), _) => report,
            _ => panic!("expected a dedicated report"),
        };
        assert!(matches!(report.state, AgentReportState::Set(_)));
        let mut sender = receiver.get_sender::<ServerToClientMsg>().unwrap();
        match response {
            "accepted" => sender
                .send_server_msg(ServerToClientMsg::AgentReportAccepted {
                    report_id: report.report_id,
                })
                .unwrap(),
            "mismatch" => sender
                .send_server_msg(ServerToClientMsg::AgentReportAccepted {
                    report_id: "another-report".into(),
                })
                .unwrap(),
            "generic" => sender
                .send_server_msg(ServerToClientMsg::UnblockInputThread)
                .unwrap(),
            "rejected" => sender
                .send_server_msg(ServerToClientMsg::LogError {
                    lines: vec!["rejected".into()],
                })
                .unwrap(),
            "malformed" => {
                let raw = raw.as_mut().unwrap();
                raw.write_all(&1u32.to_le_bytes()).unwrap();
                raw.write_all(&[0xff]).unwrap();
            },
            "eof" => {
                drop(sender);
                drop(receiver);
            },
            "stalled" => {},
            _ => unreachable!(),
        }
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.success(),
            response == "accepted",
            "{response}"
        );
        assert!(output.stdout.is_empty());
        assert!(
            started.elapsed() < Duration::from_millis(1600),
            "{response}: {:?}",
            started.elapsed()
        );
        let log = diagnostics(dir.path());
        assert!(log.contains("stage=acknowledgement"), "{response}: {log}");
        assert!(!log.contains("PRIVATE PROMPT"));
    }
}

#[test]
fn reporter_bounds_unclosed_stdin_even_when_diagnostics_cannot_be_written() {
    warm_binary();
    for broken_log in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut cmd = command(dir.path());
        cmd.arg("--hook-json");
        if broken_log {
            cmd.env("TMPDIR", "/dev/null");
        }
        let started = Instant::now();
        let mut child = cmd.spawn().unwrap();
        let stdin = child.stdin.take().unwrap(); // Kept open: read_to_string must be killed.
        let output = child.wait_with_output().unwrap();
        drop(stdin);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(started.elapsed() < Duration::from_millis(1600));
        if !broken_log {
            assert!(diagnostics(dir.path()).contains("stage=stdin_parse"));
        }
    }
}

#[test]
fn codex_router_bounds_unclosed_stdin_without_configuration_or_logging() {
    warm_binary();
    let started = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_yinz"))
        .args(["agent", "codex-report"])
        .env("TMPDIR", "/dev/null")
        .env("XDG_CONFIG_HOME", "/dev/null")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let output = child.wait_with_output().unwrap();
    drop(stdin);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert!(started.elapsed() < Duration::from_millis(1600));
}

#[test]
fn oversized_reports_use_the_existing_bounded_payload() {
    let dir = tempfile::tempdir().unwrap();
    let listener = listener(dir.path());
    let mut child = command(dir.path()).arg("--hook-json").spawn().unwrap();
    let json =
        serde_json::json!({"hook_event_name":"PostToolUse", "tool_response":"x".repeat(150_000)})
            .to_string();
    write_event(&mut child, &json);
    let mut receiver = IpcReceiverWithContext::<ClientToServerMsg>::new(listener.accept().unwrap());
    let report = match receiver.recv_client_msg() {
        IpcRecv::Msg(ClientToServerMsg::AgentReport(report), _) => report,
        _ => panic!("expected a report"),
    };
    assert!(report.event.unwrap().raw.len() <= yinz_types::agent::HOOK_BROADCAST_MAX_BYTES);
    receiver
        .get_sender::<ServerToClientMsg>()
        .unwrap()
        .send_server_msg(ServerToClientMsg::AgentReportAccepted {
            report_id: report.report_id,
        })
        .unwrap();
    assert!(child.wait().unwrap().success());
}

#[test]
fn installed_scripts_keep_neutral_output_after_reporter_failure() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    yinz_steel_builtins::install(&yinz_steel_builtins::asset_dir(), &data).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_yinz"))
        .arg("--data-dir")
        .arg(&data)
        .args(["hooks", "install"])
        .env("HOME", dir.path())
        .env("CODEX_HOME", dir.path().join(".codex"))
        .env("XDG_CONFIG_HOME", dir.path())
        .env("XDG_CACHE_HOME", dir.path())
        .env("TMPDIR", dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let input = dir.path().join("hook-input");
    std::fs::write(&input, "not valid JSON").unwrap();
    for (agent, expected) in [(".claude", ""), (".codex", "{}\n")] {
        let started = Instant::now();
        let output = Command::new("sh")
            .arg(dir.path().join(agent).join("yinz/yinz-hook.sh"))
            .env("YINZ_SESSION_NAME", "missing")
            .env("YINZ_PANE_ID", "7")
            .env("YINZ_SOCKET_DIR", dir.path())
            .env("TMPDIR", dir.path())
            // The installed hook only invokes the reporter for a stable pane reference.
            .env("YINZ_PANE_REF", "abcdef012345")
            .env_remove("YINZ_CLAUDE_INLINE_HOOKS")
            .env_remove("YINZ_REMOTE_PANE_TOKEN")
            .stdin(std::fs::File::open(&input).unwrap())
            .stdout(Stdio::piped())
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, expected.as_bytes());
        assert!(started.elapsed() < Duration::from_millis(1600));
    }
}

#[test]
fn manual_reports_preserve_explicit_targets_and_do_not_read_stdin() {
    for clear in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let listener = listener(dir.path());
        let mut cmd = command(dir.path());
        if clear {
            cmd.arg("--clear");
        } else {
            cmd.args(["--state", "busy", "--kind", "codex"]);
        }
        cmd.env(
            "YINZ_PANE_REF",
            "invalid-reference-that-must-not-be-resolved",
        )
        .env("YINZ_SESSION_NAME", "stale-session")
        .env("YINZ_PANE_ID", "999");
        let mut child = cmd.spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let mut receiver =
            IpcReceiverWithContext::<ClientToServerMsg>::new(listener.accept().unwrap());
        let report = match receiver.recv_client_msg() {
            IpcRecv::Msg(ClientToServerMsg::AgentReport(report), _) => report,
            _ => panic!("expected a manual report"),
        };
        assert_eq!(
            report.pane_id,
            yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7))
        );
        assert!(report.event.is_none());
        assert_eq!(matches!(report.state, AgentReportState::Clear), clear);
        receiver
            .get_sender::<ServerToClientMsg>()
            .unwrap()
            .send_server_msg(ServerToClientMsg::AgentReportAccepted {
                report_id: report.report_id,
            })
            .unwrap();
        assert!(child.wait().unwrap().success());
        drop(stdin);
    }
}

#[test]
fn claude_observer_outlives_the_hook_and_reports_missing_evidence() {
    warm_binary();
    let dir = tempfile::tempdir().unwrap();
    let listener = listener(dir.path());
    let mut child = command(dir.path()).arg("--hook-json").spawn().unwrap();
    write_event(
        &mut child,
        &serde_json::json!({
            "hook_event_name":"UserPromptSubmit", "session_id":"conversation", "prompt_id":"prompt",
            "prompt":"review", "transcript_path":dir.path().join("missing.jsonl")
        })
        .to_string(),
    );
    let mut receiver = IpcReceiverWithContext::<ClientToServerMsg>::new(listener.accept().unwrap());
    let IpcRecv::Msg(ClientToServerMsg::AgentReport(report), _) = receiver.recv_client_msg() else {
        panic!("expected submit");
    };
    receiver
        .get_sender::<ServerToClientMsg>()
        .unwrap()
        .send_server_msg(ServerToClientMsg::AgentReportAccepted {
            report_id: report.report_id,
        })
        .unwrap();
    // EOF on both hook output pipes proves the observer did not retain them.
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut receiver = IpcReceiverWithContext::<ClientToServerMsg>::new(listener.accept().unwrap());
    let IpcRecv::Msg(ClientToServerMsg::AgentReport(report), _) = receiver.recv_client_msg() else {
        panic!("expected confirmation");
    };
    let hook: yinz_types::agent::HookEvent =
        serde_json::from_str(&report.event.unwrap().raw).unwrap();
    assert_eq!(hook.claude_turn_complete, Some(false));
    assert_eq!(hook.prompt_id.as_deref(), Some("prompt"));
    assert_eq!(hook.last_assistant_message, None);
    receiver
        .get_sender::<ServerToClientMsg>()
        .unwrap()
        .send_server_msg(ServerToClientMsg::AgentReportAccepted {
            report_id: report.report_id,
        })
        .unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn claude_observer_delivers_confirmation_after_its_binary_is_replaced() {
    warm_binary();
    let dir = tempfile::tempdir().unwrap();
    let listener = listener(dir.path());
    // Debug images can exceed /tmp's capacity. Keep the replaceable copy on
    // this worktree's build filesystem, with the short socket path still in /tmp.
    let images = tempfile::tempdir_in(
        std::path::Path::new(env!("CARGO_BIN_EXE_yinz"))
            .parent()
            .unwrap(),
    )
    .unwrap();
    let binary = images.path().join("yinz");
    std::fs::copy(env!("CARGO_BIN_EXE_yinz"), &binary).unwrap();
    let mut child = Command::new(&binary)
        .arg("--claude-turn-observer")
        .env("YINZ_SOCKET_DIR", dir.path())
        .env("TMPDIR", dir.path())
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .env_remove("YINZ_PANE_REF")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // spawn() has exec'd the observer, and stdin gates its observation. Replace
    // its image before releasing it, exactly as an install during a turn does.
    let replacement = images.path().join("replacement");
    yinz_test_support::write_warm_script(&replacement, "exit 97");
    std::fs::rename(&replacement, &binary).unwrap();
    write_event(
        &mut child,
        &serde_json::json!({
            "kind":"claude",
            "hook":{"hook_event_name":"UserPromptSubmit", "session_id":"conversation", "prompt_id":"prompt"},
            "route":{"Local":{"session":"report-test","pane":"terminal_7"}},
            // Missing process evidence makes the observer immediately send an
            // unavailable confirmation; its delivery must still survive install.
            "process":null
        })
        .to_string(),
    );
    let socket = dir
        .path()
        .join(&*yinz_runtime::consts::CLIENT_SERVER_CONTRACT_DIR)
        .join("report-test");
    let waiter = std::thread::spawn(move || {
        let output = child.wait_with_output().unwrap();
        // If the observer cannot spawn its reporter, wake accept() with EOF so
        // this regression fails on its exit status rather than timing out.
        let _ = std::os::unix::net::UnixStream::connect(socket);
        output
    });
    let mut receiver = IpcReceiverWithContext::<ClientToServerMsg>::new(listener.accept().unwrap());
    receiver
        .set_recv_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    let received = match receiver.recv_client_msg() {
        IpcRecv::Msg(ClientToServerMsg::AgentReport(report), _) => {
            let hook: yinz_types::agent::HookEvent =
                serde_json::from_str(&report.event.unwrap().raw).unwrap();
            assert_eq!(hook.claude_turn_complete, Some(false));
            assert_eq!(hook.session_id, "conversation");
            assert_eq!(hook.prompt_id.as_deref(), Some("prompt"));
            receiver
                .get_sender::<ServerToClientMsg>()
                .unwrap()
                .send_server_msg(ServerToClientMsg::AgentReportAccepted {
                    report_id: report.report_id,
                })
                .unwrap();
            true
        },
        _ => false,
    };
    let output = waiter.join().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(received, "observer lost the turn confirmation");
}

#[test]
fn commands_in_an_adopted_pane_reach_the_adopting_session() {
    use yinz_server::pty_host::protocol::{
        read_msg, write_msg, HostStatus, HostToServerMsg, ServerToHostMsg,
    };
    warm_binary();
    let dir = tempfile::tempdir().unwrap();
    let sessions = listener(dir.path());
    let supervisors = dir
        .path()
        .join(&*yinz_runtime::consts::CLIENT_SERVER_CONTRACT_DIR)
        .join("pty");
    std::fs::create_dir_all(&supervisors).unwrap();
    let supervisor =
        std::os::unix::net::UnixListener::bind(supervisors.join("abcdef012345.sock")).unwrap();
    let answered = std::thread::spawn(move || {
        let (mut stream, _) = supervisor.accept().unwrap();
        let query: ServerToHostMsg = read_msg(&mut stream).unwrap().unwrap();
        assert!(matches!(query, ServerToHostMsg::Query), "{query:?}");
        write_msg(
            &mut stream,
            &HostToServerMsg::Status(HostStatus {
                host_id: yinz_types::internal_ids::SupervisorId::new("abcdef012345"),
                session_name: Some("report-test".to_owned().into()),
                terminal_id: 2,
                command: "claude".to_owned(),
                cwd: None,
                child_pid: None,
                host_pid: yinz_types::internal_ids::ProcessId::new(1),
                started_at_unix: yinz_types::units::UnixSeconds::new(0),
                attached: true,
                supports_relabel: true,
                child_exit_status: None,
                child_group_terminated: false,
                buffered_bytes: 0,
            }),
        )
        .unwrap();
    });
    // The pane was born as pane 23 of a session that no longer exists.
    let child = Command::new(env!("CARGO_BIN_EXE_yinz"))
        .args(["agent", "list", "--json"])
        .env("YINZ_SOCKET_DIR", dir.path())
        .env("TMPDIR", dir.path())
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .env("YINZ_SESSION_NAME", "chatty-orange")
        .env("YINZ_PANE_ID", "23")
        .env("YINZ_PANE_REF", "abcdef012345")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    answered.join().unwrap();
    let mut receiver = IpcReceiverWithContext::<ClientToServerMsg>::new(sessions.accept().unwrap());
    match receiver.recv_client_msg() {
        IpcRecv::Msg(ClientToServerMsg::AgentControl(request), _) => assert!(
            matches!(
                request,
                yinz_types::agent::control::AgentRequest::List { .. }
            ),
            "{request:?}"
        ),
        _ => panic!("expected an agent request"),
    }
    receiver
        .get_sender::<ServerToClientMsg>()
        .unwrap()
        .send_server_msg(ServerToClientMsg::AgentControl(
            yinz_types::agent::control::AgentResponse::new(
                yinz_types::agent::control::AgentOutcome::Ok,
            ),
        ))
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"outcome\":\"ok\""));
}

#[test]
fn agent_validation_errors_answer_in_json_when_asked() {
    warm_binary();
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_yinz"))
            .args(["--session", "json-errors"])
            .args(args)
            .env("YINZ_SOCKET_DIR", dir.path())
            .env("TMPDIR", dir.path())
            .env("HOME", dir.path())
            .env("XDG_CONFIG_HOME", dir.path())
            .env_remove("YINZ_PANE_REF")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    for (args, message) in [
        (
            &["agent", "prompt", "terminal_7", " ", "--json"][..],
            "prompt must not be empty",
        ),
        (
            &["agent", "prompt", "7", "text", "--file", "-", "--json"][..],
            "not both",
        ),
    ] {
        let output = run(args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        let response: serde_json::Value =
            serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
                panic!(
                    "{args:?}: {error}: {}",
                    String::from_utf8_lossy(&output.stdout)
                )
            });
        assert_eq!(response["outcome"], "rejected", "{args:?}");
        assert!(
            response["message"].as_str().unwrap().contains(message),
            "{response}"
        );
    }
    // Without --json the error stays on stderr.
    let output = run(&["agent", "prompt", "terminal_7", " "]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("prompt must not be empty"));
}

#[test]
fn delivered_agent_prompts_keep_their_submission_id_after_transport_disconnect() {
    use yinz_types::agent::control::AgentRequest;

    warm_binary();
    for chosen_id in [None, Some("orchestrator-review-7")] {
        let dir = tempfile::tempdir().unwrap();
        let listener = listener(dir.path());
        let mut args = vec![
            "prompt",
            "terminal_7",
            "Review local changes",
            "--conversation",
            "conversation-a",
            "--timeout",
            "5s",
            "--json",
        ];
        if let Some(id) = chosen_id {
            args.extend(["--submission", id]);
        }
        let child = agent_command(dir.path(), &args).spawn().unwrap();
        let mut receiver =
            IpcReceiverWithContext::<ClientToServerMsg>::new(listener.accept().unwrap());
        receiver
            .set_recv_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        let IpcRecv::Msg(
            ClientToServerMsg::AgentControl(AgentRequest::Prompt {
                preserve_draft: false,
                pane_id,
                expected_session_id,
                submission_id,
                prompt,
                wait,
                timeout_ms,
            }),
            _,
        ) = receiver.recv_client_msg()
        else {
            panic!("expected a delivered agent prompt");
        };
        assert_eq!(pane_id.to_string(), "terminal_7");
        assert_eq!(expected_session_id.as_deref(), Some("conversation-a"));
        assert_eq!(prompt, "Review local changes");
        assert!(!wait);
        assert_eq!(timeout_ms, 5000);
        let delivered_id = submission_id.expect("the CLI must assign an ID before sending");
        if let Some(chosen_id) = chosen_id {
            assert_eq!(delivered_id, chosen_id);
        } else {
            assert!(uuid::Uuid::parse_str(&delivered_id).is_ok());
        }
        // The server received the mutation, but its acceptance/result was lost.
        drop(receiver);
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["outcome"], "failed");
        assert_eq!(response["submission_id"], delivered_id);
        assert!(
            response["message"]
                .as_str()
                .unwrap()
                .contains("disconnected"),
            "{response}"
        );
    }
}

#[test]
fn agent_submission_wait_forwards_its_receipt_and_preserves_failed_outcome() {
    use yinz_types::agent::control::{AgentOutcome, AgentRequest, AgentResponse};

    warm_binary();
    let dir = tempfile::tempdir().unwrap();
    let listener = listener(dir.path());
    let child = agent_command(
        dir.path(),
        &[
            "wait",
            "7",
            "--submission",
            "saved-submission-7",
            "--conversation",
            "conversation-a",
            "--timeout",
            "17s",
            "--json",
        ],
    )
    .spawn()
    .unwrap();
    let mut receiver = IpcReceiverWithContext::<ClientToServerMsg>::new(listener.accept().unwrap());
    receiver
        .set_recv_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    let IpcRecv::Msg(
        ClientToServerMsg::AgentControl(AgentRequest::WaitTurn {
            pane_id,
            expected_session_id,
            submission_id,
            timeout_ms,
        }),
        _,
    ) = receiver.recv_client_msg()
    else {
        panic!("expected an exact submission wait, not a state wait");
    };
    assert_eq!(pane_id.to_string(), "terminal_7");
    assert_eq!(expected_session_id.as_deref(), Some("conversation-a"));
    assert_eq!(submission_id, "saved-submission-7");
    assert_eq!(timeout_ms, 17_000);
    let mut response = AgentResponse::new(AgentOutcome::Failed);
    response.submission_id = Some(submission_id.clone());
    response.turn_id = Some("provider-turn-7".into());
    response.acknowledged = true;
    response.message = Some("provider rejected the selected model".into());
    receiver
        .get_sender::<ServerToClientMsg>()
        .unwrap()
        .send_server_msg(ServerToClientMsg::AgentControl(response))
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["outcome"], "failed");
    assert_eq!(response["submission_id"], submission_id);
    assert_eq!(response["turn_id"], "provider-turn-7");
    assert_eq!(response["acknowledged"], true);
    assert_eq!(response["message"], "provider rejected the selected model");
}

#[test]
fn agy_observer_reports_only_after_api_idle() {
    use std::io::{BufRead, BufReader};
    warm_binary();
    let dir = tempfile::tempdir().unwrap();
    let listener = listener(dir.path());
    let (tx, rx) = std::sync::mpsc::channel();
    let peer = std::thread::spawn(move || {
        for _ in 0..5 {
            let mut receiver =
                IpcReceiverWithContext::<ClientToServerMsg>::new(listener.accept().unwrap());
            let IpcRecv::Msg(ClientToServerMsg::AgentReport(report), _) =
                receiver.recv_client_msg()
            else {
                panic!("expected AGY report");
            };
            let hook: yinz_types::agent::HookEvent =
                serde_json::from_str(&report.event.unwrap().raw).unwrap();
            receiver
                .get_sender::<ServerToClientMsg>()
                .unwrap()
                .send_server_msg(ServerToClientMsg::AgentReportAccepted {
                    report_id: report.report_id,
                })
                .unwrap();
            tx.send((report.state, hook)).unwrap();
        }
    });
    let script = dir.path().join("agy.py");
    std::fs::write(
        &script,
        include_str!("../../yinz-test-support/fixtures/agy.py"),
    )
    .unwrap();
    let mut child = Command::new("bash")
        .args([
            "-c",
            "exec -a agy bash -c 'python3 \"$@\"; exit $?' -- \"$@\"",
            "--",
        ])
        .arg(script)
        .arg(dir.path().join("transcript.jsonl"))
        .arg(env!("CARGO_BIN_EXE_yinz"))
        .args([
            "--session",
            "report-test",
            "action",
            "agent-report",
            "--pane-id",
            "7",
            "--kind",
            "agy",
            "--hook-json",
        ])
        .env("YINZ_SOCKET_DIR", dir.path())
        .env("TMPDIR", dir.path())
        .env_remove("YINZ_PANE_REF")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    for (gate, count) in [("first-stop", 2), ("continued-stop", 2)] {
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), gate);
        for _ in 0..count {
            let (state, hook) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
            assert_eq!(hook.agy_turn_complete, None);
            if hook.hook_event_name == "UserPromptSubmit" {
                assert_eq!(hook.prompt_id.as_deref(), Some("fixture-conversation:1"));
                assert!(
                    matches!(state, AgentReportState::Set(agent) if agent.state == yinz_types::agent::AgentState::Busy)
                );
            }
        }
        // A finished model response and a continued Stop must not produce confirmation.
        assert!(rx.recv_timeout(Duration::from_millis(250)).is_err());
        writeln!(child.stdin.as_mut().unwrap(), "continue").unwrap();
    }
    let (_, confirmed) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(confirmed.agy_turn_complete, Some(true));
    assert_eq!(confirmed.session_id, "fixture-conversation");
    assert_eq!(
        confirmed.prompt_id.as_deref(),
        Some("fixture-conversation:1")
    );
    assert_eq!(confirmed.turn_id.as_deref(), Some("1"));
    assert_eq!(confirmed.last_assistant_message.as_deref(), Some("SECOND"));
    writeln!(child.stdin.as_mut().unwrap(), "exit").unwrap();
    assert!(child.wait().unwrap().success());
    peer.join().unwrap();
}

#[path = "../unit/pane_routing.rs"]
mod pane_routing;

#[path = "../unit/hooks_status.rs"]
mod hooks_status;

#[path = "../unit/hooks_provider.rs"]
mod hooks_provider;

#[path = "../unit/muse_launch.rs"]
mod muse_launch;
