use super::*;
use std::io::{BufRead, BufReader, Write};

fn command(root: &Path, repo: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_yinz"));
    for key in [
        "HOME",
        "XDG_DATA_HOME",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "YINZ_SOCKET_DIR",
    ] {
        cmd.env(key, root);
    }
    cmd.env_remove("YINZ_CONFIG_DIR")
        .env_remove("CODEX_HOME")
        .env("YINZ_SESSION_NAME", "owner")
        .env("YINZ_PANE_REF", "abcdef012345")
        .env("YINZ_PANE_ID", "7")
        .env_remove("YINZ_REMOTE_PANE_TOKEN")
        .current_dir(repo)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

fn ready(stream: &std::os::unix::net::UnixStream) {
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    assert_eq!(line, "ready\n");
}

#[test]
#[ignore = "requires Linux Bubblewrap user namespaces"]
fn bubblewrap_cli_control_reporting_concurrent_leases_and_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let repo = root.join("repo");
    std::fs::create_dir(&repo).unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .arg(&repo)
        .status()
        .unwrap()
        .success());
    std::fs::create_dir(repo.join(".yinz")).unwrap();
    std::fs::write(repo.join(".yinz/bubblewrap.kdl"), "").unwrap();
    let session = Session::new(root, "owner");
    yinz_runtime::shared::set_permissions(session.path.parent().unwrap(), 0o700).unwrap();
    let supervisor = Supervisor::new(root, "owner", 7);
    let metadata = supervisor.path.with_extension("json");
    std::fs::write(&metadata, "supervisor metadata").unwrap();
    let output = command(root, &repo)
        .args(["bubblewrap", "prepare"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let listener = std::os::unix::net::UnixListener::bind(repo.join("gate.sock")).unwrap();
    let script = r#"
import errno, os, pathlib, socket, subprocess
control = next(pathlib.Path('/run/yinz-control').iterdir())
metadata = control / 'pty/abcdef012345.json'
assert metadata.read_text() == 'supervisor metadata'
for operation in [lambda: (control / 'new-file').touch(), metadata.unlink]:
    try:
        operation()
    except OSError as error:
        assert error.errno == errno.EROFS, error
    else:
        raise AssertionError('control directory must be read-only')
def report():
    subprocess.run(['yinz', 'action', 'agent-report', '--hook-json'], input=b'{"hook_event_name":"Stop","session_id":"sandbox"}', check=True, timeout=20)
subprocess.run(['yinz', 'agent', 'list', '--json'], check=True, timeout=20)
report()
s = socket.socket(socket.AF_UNIX)
s.settimeout(30)
s.connect('gate.sock')
s.sendall(b'ready\n')
while s.recv(1) == b'r':
    subprocess.run(['yinz', 'agent', 'list', '--json'], check=True, timeout=20)
    subprocess.run(['yinz', 'action', 'write-chars', 'transferred'], check=True, timeout=20)
    report()
    s.sendall(b'ready\n')
"#;
    let mut children = Vec::new();
    let mut gates = Vec::new();
    for _ in 0..2 {
        let child = command(root, &repo)
            .arg("--bubblewrap-run")
            .arg(&repo)
            .args(["--", "/usr/bin/python3", "-c", script])
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        // Socket gates prove both agents have finished startup while retaining their leases.
        let gate = yinz_test_support::Watched::new(None);
        let connected = gate.clone();
        let listen = listener.try_clone().unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listen.accept().unwrap();
            *connected.lock() = Some(stream);
        });
        let stream = gate.wait_until("sandbox connects", Duration::from_secs(30), |value| {
            value.as_ref().map(|stream| stream.try_clone().unwrap())
        });
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        ready(&stream);
        assert!(matches!(
            session.receive(),
            ClientToServerMsg::AgentControl(_)
        ));
        assert!(matches!(
            session.receive(),
            ClientToServerMsg::AgentReport(_)
        ));
        children.push(child);
        gates.push(stream);
    }
    let busy = command(root, &repo)
        .args(["bubblewrap", "prepare"])
        .output()
        .unwrap();
    assert!(!busy.status.success());
    assert!(String::from_utf8_lossy(&busy.stderr).contains("busy"));
    // The destination did not exist when either sandbox mounted the control directory.
    let mut destination = Session::new(root, "destination");
    {
        let mut status = supervisor.status.lock().unwrap();
        status.session_name = Some("destination".into());
        status.terminal_id = 42;
    }
    for restart in [false, true] {
        if restart {
            drop(destination);
            destination = Session::new(root, "destination");
        }
        for gate in &mut gates {
            gate.write_all(b"r").unwrap();
            ready(gate);
            assert!(matches!(
                destination.receive(),
                ClientToServerMsg::AgentControl(_)
            ));
            assert!(
                matches!(destination.receive(), ClientToServerMsg::Action { terminal_id: Some(id), .. } if id == TerminalId::new(42))
            );
            assert!(
                matches!(destination.receive(), ClientToServerMsg::AgentReport(report) if report.pane_id == PaneId::Terminal(TerminalId::new(42)))
            );
        }
        assert!(session.messages.try_recv().is_err());
        assert_eq!(
            std::fs::read_to_string(&metadata).unwrap(),
            "supervisor metadata"
        );
        assert!(!metadata
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("new-file")
            .exists());
    }
    for (index, (mut child, mut gate)) in children.into_iter().zip(gates).enumerate() {
        if index == 0 {
            gate.write_all(b"x").unwrap();
        } else {
            child.kill().unwrap();
        }
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.success(),
            index == 0,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = command(root, &repo)
        .args(["bubblewrap", "prepare"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
