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
    for daemon_runtime in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let launcher_sockets = dir.path().join("launcher");
        let listener = listener(&launcher_sockets);
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
                    "YINZ_SESSION_NAME":"report-test", "YINZ_PANE_ID":"15",
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
            .args(["hocus", "codex-report"])
            .env("YINZ_SOCKET_DIR", dir.path().join("daemon"))
            .env("TMPDIR", dir.path())
            .env_remove("XDG_RUNTIME_DIR")
            .env("YINZ_PANE_REF", "aaaaaaaaaaaa")
            .env("YINZ_SESSION_NAME", "old-session")
            .env("YINZ_PANE_ID", "20")
            .env("YINZ_REMOTE_PANE_TOKEN", "old-remote-pane")
            .env("YINZ_SWAMP_OWNER", "old-workflow")
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
                "hook_event_name":"UserPromptSubmit", "session_id": thread,
                "prompt":"PRIVATE PROMPT"
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
        let event = report.event.unwrap();
        assert_eq!(event.session_id.as_deref(), Some(thread.as_str()));
        assert_eq!(event.kind, "codex");
        assert!(
            serde_json::from_str::<serde_json::Value>(&event.raw).unwrap()["_yinz_workflow_owner"]
                .is_null()
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
            .args(["hocus", "codex", "--command"])
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
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("{}\n", args.join("\n"))
        );
    }
}

#[test]
fn standalone_codex_hooks_require_the_owning_in_process_pane() {
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
                .args(["hocus", "codex-report"])
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
                "YINZ_SWAMP_OWNER",
            ] {
                command.env_remove(key);
            }
            command
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
        "detachable_panes false\nshow_release_notes false\nload_plugins clear-defaults=true\n",
    )
    .unwrap();
    let layout = root.join("layout.kdl");
    std::fs::write(&layout, format!(
        "layout {{\n pane command={:?} {{\n args \"--exact\" \"standalone_codex_hooks_require_the_owning_in_process_pane\"\n }}\n}}\n",
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
        assert_eq!(pane["agent"]["session_id"], "accepted", "{case}: {pane}");
        assert_eq!(pane["agent"]["kind"], "codex");
        assert_eq!(pane["agent"]["state"], "Busy");
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
        .args(["hocus", "codex-report"])
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
    let output = Command::new(env!("CARGO_BIN_EXE_yinz"))
        .args(["hocus", "hooks", "install"])
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
    for (agent, expected) in [(".claude", ""), (".codex", "{}\n")] {
        let started = Instant::now();
        let mut child = Command::new("sh")
            .arg(dir.path().join(agent).join("hocus/hocus-hook.sh"))
            .env("YINZ_SESSION_NAME", "missing")
            .env("YINZ_PANE_ID", "7")
            .env("YINZ_SOCKET_DIR", dir.path())
            .env("TMPDIR", dir.path())
            .env_remove("YINZ_PANE_REF")
            .env_remove("YINZ_REMOTE_PANE_TOKEN")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        write_event(&mut child, "not valid JSON");
        let output = child.wait_with_output().unwrap();
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
