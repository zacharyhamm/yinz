//! Real reporter processes and fake peers. No sleeps: sockets and gates synchronize the tests.
use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions};
use interprocess::TryClone;
use std::{
    io::Write,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use {
    zellij_ipc::ipc::ClientToServerMsg, zellij_ipc::ipc::IpcReceiverWithContext,
    zellij_ipc::ipc::IpcRecv, zellij_ipc::ipc::ServerToClientMsg,
    zellij_types::agent::report::AgentReportState,
};

fn command(dir: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_zellij"));
    cmd.args([
        "--session",
        "report-test",
        "action",
        "agent-report",
        "--pane-id",
        "7",
    ])
    .env("ZELLIJ_SOCKET_DIR", dir)
    .env("TMPDIR", dir)
    .env("HOME", dir)
    .env("XDG_CONFIG_HOME", dir)
    .env("ZELLIJ_HOOK_TRACE", "1")
    .env_remove("ZELLIJ_PANE_REF")
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    cmd
}

fn listener(dir: &std::path::Path) -> interprocess::local_socket::Listener {
    let socket_dir = dir.join(&*zellij_runtime::consts::CLIENT_SERVER_CONTRACT_DIR);
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

fn diagnostics(dir: &std::path::Path) -> String {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| {
            let path = entry.ok()?.path().join("zellij-log/zellij.log");
            std::fs::read_to_string(path).ok()
        })
        .collect()
}

#[test]
fn reporter_requires_its_exact_acceptance_and_bounds_stalled_peers() {
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
        let mut sender = receiver.get_sender::<ServerToClientMsg>();
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
    assert!(report.event.unwrap().raw.len() <= zellij_types::agent::HOOK_BROADCAST_MAX_BYTES);
    receiver
        .get_sender::<ServerToClientMsg>()
        .send_server_msg(ServerToClientMsg::AgentReportAccepted {
            report_id: report.report_id,
        })
        .unwrap();
    assert!(child.wait().unwrap().success());
}

#[test]
fn installed_scripts_keep_neutral_output_after_reporter_failure() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_zellij"))
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
            .env("ZELLIJ_SESSION_NAME", "missing")
            .env("ZELLIJ_PANE_ID", "7")
            .env("ZELLIJ_SOCKET_DIR", dir.path())
            .env("TMPDIR", dir.path())
            .env_remove("ZELLIJ_PANE_REF")
            .env_remove("ZELLIJ_REMOTE_PANE_TOKEN")
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
            "ZELLIJ_PANE_REF",
            "invalid-reference-that-must-not-be-resolved",
        )
        .env("ZELLIJ_SESSION_NAME", "stale-session")
        .env("ZELLIJ_PANE_ID", "999");
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
            zellij_types::data::PaneId::Terminal(zellij_types::internal_ids::TerminalId(7))
        );
        assert!(report.event.is_none());
        assert_eq!(matches!(report.state, AgentReportState::Clear), clear);
        receiver
            .get_sender::<ServerToClientMsg>()
            .send_server_msg(ServerToClientMsg::AgentReportAccepted {
                report_id: report.report_id,
            })
            .unwrap();
        assert!(child.wait().unwrap().success());
        drop(stdin);
    }
}
