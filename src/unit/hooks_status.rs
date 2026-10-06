//! Exercise stdout failure in a real CLI process, without racing `head`.
#[test]
fn hooks_status_exits_quietly_when_stdout_is_closed() {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::process::{Command, Stdio};

    let dir = tempfile::tempdir().unwrap();
    let (output, reader) = UnixStream::pair().unwrap();
    drop(reader);
    let output = Command::new(env!("CARGO_BIN_EXE_yinz"))
        .args(["hooks", "status", "--agent", "codex"])
        .env("HOME", dir.path())
        .env("CODEX_HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .env("TMPDIR", dir.path())
        .stdout(Stdio::from(OwnedFd::from(output)))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
