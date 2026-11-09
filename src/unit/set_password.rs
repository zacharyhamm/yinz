use super::*;
#[test]
fn set_password_stdin_validation() {
    for value in [
        b"short\n".as_slice(),
        b"twelve\0characters\n",
        &[0xff; 20],
        &[b'a'; 1025],
    ] {
        assert!(read_password(value).is_err());
    }
    assert_eq!(
        read_password("twelve words long\r\nignored".as_bytes())
            .unwrap()
            .as_str(),
        "twelve words long"
    );
    assert!(read_password("🐈🐈🐈🐈🐈🐈🐈🐈🐈🐈🐈🐈\n".as_bytes()).is_ok());
    assert!(read_password(&[b'a'; 1024][..]).is_ok());
}
#[test]
fn set_password_clear_and_revoke_explicit_directory() {
    let temp = tempfile::tempdir().unwrap();
    assert!(run(temp.path(), false, false, true, "unused").is_err());
    yinz_web::auth::password::write(temp.path(), &Zeroizing::new("twelve words long".into()))
        .unwrap();
    run(temp.path(), false, false, true, "unused").unwrap();
    run(temp.path(), false, true, false, "unused").unwrap();
    assert_eq!(
        yinz_web::auth::password::read_state(temp.path()),
        yinz_web::auth::password::PasswordState::Missing
    );
}

fn binary() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug/yinz")
}
#[test]
fn set_password_command_entry_stdin_local_errors_clear_revoke() {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let temp = tempfile::tempdir().unwrap();
    let mut command = Command::new(binary());
    command
        .args(["--remote", "must-not-be-contacted.invalid", "--data-dir"])
        .arg(temp.path())
        .args(["set-password", "--password-stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"entrypoint password\r\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("entrypoint password"));
    for mode in ["--revoke-sessions", "--clear"] {
        assert!(Command::new(binary())
            .arg("--data-dir")
            .arg(temp.path())
            .args(["set-password", mode])
            .stdin(Stdio::null())
            .status()
            .unwrap()
            .success());
    }
    assert!(!Command::new(binary())
        .arg("--data-dir")
        .arg(temp.path())
        .args(["set-password", "--password-stdin"])
        .stdin(Stdio::null())
        .output()
        .unwrap()
        .status
        .success());
    assert!(!Command::new(binary())
        .arg("--data-dir")
        .arg(temp.path())
        .arg("set-password")
        .stdin(Stdio::null())
        .output()
        .unwrap()
        .status
        .success());
}

struct Prompt {
    child: std::process::Child,
    slave: Option<std::fs::File>,
    master: std::fs::File,
    output: yinz_test_support::Watched<Vec<u8>>,
    reader: Option<std::thread::JoinHandle<()>>,
    original: nix::sys::termios::Termios,
}
impl Prompt {
    fn new(dir: &Path) -> Self {
        use std::{
            io::Read,
            os::unix::process::CommandExt,
            process::{Command, Stdio},
        };
        let pty = nix::pty::openpty(None, None).unwrap();
        let slave = std::fs::File::from(pty.slave);
        let original = nix::sys::termios::tcgetattr(&slave).unwrap();
        let mut cmd = Command::new(binary());
        cmd.arg("--data-dir")
            .arg(dir)
            .arg("set-password")
            .env("TERM", "xterm-256color")
            .stdin(slave.try_clone().unwrap())
            .stdout(slave.try_clone().unwrap())
            .stderr(slave.try_clone().unwrap());
        // SAFETY: only async-signal-safe session/terminal syscalls run after fork.
        unsafe {
            cmd.pre_exec(|| {
                if nix::libc::setsid() < 0 || nix::libc::ioctl(0, nix::libc::TIOCSCTTY as _, 0) < 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn().unwrap();
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let master = std::fs::File::from(pty.master);
        let mut read = master.try_clone().unwrap();
        let output = yinz_test_support::Watched::new(Vec::new());
        let captured = output.clone();
        let reader = std::thread::spawn(move || {
            let mut buf = [0; 4096];
            loop {
                match read.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => captured.lock().extend_from_slice(&buf[..n]),
                    Err(e) if e.raw_os_error() == Some(nix::libc::EIO) => break,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => panic!("password PTY: {e}"),
                }
            }
        });
        Self {
            child,
            slave: Some(slave),
            master,
            output,
            reader: Some(reader),
            original,
        }
    }
    fn wait(&self, text: &str) {
        self.output.wait_until(
            "password prompt output",
            yinz_test_support::default_timeout(),
            |bytes| String::from_utf8_lossy(bytes).contains(text).then_some(()),
        );
    }
    fn input(&mut self, text: &[u8]) {
        use std::io::Write;
        // Seeing the prompt precedes console's ECHO change; wait for the actual termios
        // state, not a guessed delay, before entering a secret.
        let slave = self.slave.as_ref().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if !nix::sys::termios::tcgetattr(slave)
                .unwrap()
                .local_flags
                .contains(nix::sys::termios::LocalFlags::ECHO)
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "password reader never disabled echo"
            );
            std::thread::yield_now();
        }
        self.master.write_all(text).unwrap();
    }
    fn restored(&self) {
        assert_eq!(
            nix::sys::termios::tcgetattr(self.slave.as_ref().unwrap()).unwrap(),
            self.original
        );
    }
}
impl Drop for Prompt {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        drop(self.slave.take());
        self.reader.take().unwrap().join().unwrap();
    }
}
#[test]
fn set_password_pty_confirmation_no_echo_and_ctrl_c_restores_terminal() {
    let temp = tempfile::tempdir().unwrap();
    {
        let mut prompt = Prompt::new(temp.path());
        prompt.wait("Web password");
        prompt.input(b"never echoed secret\n");
        prompt.wait("Confirm web password");
        assert_eq!(
            yinz_web::auth::password::read_state(temp.path()),
            yinz_web::auth::password::PasswordState::Missing
        );
        prompt.input(b"never echoed secret\n");
        prompt.wait("Web password saved");
        assert!(prompt.child.wait().unwrap().success());
        prompt.restored();
        assert!(!String::from_utf8_lossy(&prompt.output.get()).contains("never echoed secret"));
    }
    let previous = yinz_web::auth::password::read_state(temp.path());
    {
        let mut prompt = Prompt::new(temp.path());
        prompt.wait("Web password");
        prompt.input(b"\x03");
        prompt.wait("Password entry cancelled");
        assert_eq!(prompt.child.wait().unwrap().code(), Some(130));
        prompt.restored();
    }
    assert_eq!(yinz_web::auth::password::read_state(temp.path()), previous);
}

#[test]
fn custom_store_guidance_retains_quoted_data_and_config_through_provisioning() {
    use std::{
        io::Write,
        os::unix::fs::{DirBuilderExt, OpenOptionsExt},
        process::{Command, Stdio},
    };
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data's $value `literal`");
    let config_dir = temp.path().join("config's directory");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config = config_dir.join("chosen's config.kdl");
    std::fs::write(&config, "web { port 0; }\n").unwrap();
    let command = || {
        let mut cmd = Command::new(binary());
        cmd.env("PATH", "/nonexistent")
            .args(["--remote", "must-not-contact.invalid", "--data-dir"])
            .arg(&data)
            .arg("--config")
            .arg(&config)
            .arg("--config-dir")
            .arg(&config_dir);
        cmd
    };
    let quoted = |p: &Path| format!("'{}'", p.to_str().unwrap().replace('\'', "'\\''"));
    let prefix = format!(
        "yinz --data-dir {} --config {} --config-dir {}",
        quoted(&data),
        quoted(&config),
        quoted(&config_dir)
    );
    let status = || {
        let output = command().args(["web", "status"]).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let missing = status();
    assert!(
        missing.contains(&format!("Data directory: {}", data.display())),
        "{missing}"
    );
    assert!(
        missing.contains(&format!(
            "Password: Missing; enabled listeners wait for {prefix} set-password"
        )),
        "{missing}"
    );
    std::fs::create_dir_all(&data).unwrap();
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(data.join("web"))
        .unwrap();
    let mut invalid = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(data.join("web/password"))
        .unwrap();
    invalid.write_all(b"invalid").unwrap();
    drop(invalid);
    let invalid = status();
    assert!(
        invalid.contains(&format!(
            "Password: Invalid; repair with {prefix} set-password"
        )),
        "{invalid}"
    );
    let mut child = command()
        .args(["set-password", "--password-stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"custom store secret\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let saved = String::from_utf8(output.stdout).unwrap();
    assert!(
        saved.contains(&format!("Use {prefix} web status for listener")),
        "{saved}"
    );
    assert!(!saved.contains("custom store secret"));
    assert!(!saved.contains("must-not-contact"));
    assert!(status().contains("Password: Ready"));
    let mut sessions = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(data.join("web/sessions.json"))
        .unwrap();
    sessions.write_all(b"invalid").unwrap();
    drop(sessions);
    let broken = status();
    assert!(
        broken.contains(&format!(
            "recover with {prefix} set-password --revoke-sessions"
        )),
        "{broken}"
    );
    let output = command()
        .args(["set-password", "--revoke-sessions"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let repaired = status();
    assert!(repaired.contains("Password: Ready"));
    assert!(repaired.contains("Active browser sessions: 0"));
}
