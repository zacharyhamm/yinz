//! Real interactive clients on the local PTY, using the suite's SSH fixture.
use super::*;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, ExitStatus};
use std::sync::atomic::AtomicBool;
use yinz_test_support::Watched;

#[derive(Default)]
struct Output {
    bytes: Vec<u8>,
    status: Option<ExitStatus>,
}
struct Client {
    child: Arc<Mutex<Child>>,
    master: File,
    slave: File,
    original: nix::sys::termios::Termios,
    output: Watched<Output>,
    stop: Arc<AtomicBool>,
    reader: Option<std::thread::JoinHandle<()>>,
    root: tempfile::TempDir,
}
impl Client {
    fn start(host: &str, name: &str, create: bool) -> Self {
        test_env();
        let root = tempfile::Builder::new()
            .prefix("yrc-")
            .tempdir_in("/tmp")
            .unwrap();
        let config = root.path().join("config.kdl");
        // A nonexistent local default layout must not affect a remote attachment.
        std::fs::write(&config, "default_layout \"/no/local/layout.kdl\"\n").unwrap();
        let pty = nix::pty::openpty(
            Some(&nix::pty::Winsize {
                ws_row: 30,
                ws_col: 100,
                ws_xpixel: 0,
                ws_ypixel: 0,
            }),
            None,
        )
        .unwrap();
        for fd in [&pty.master, &pty.slave] {
            nix::fcntl::fcntl(
                fd,
                nix::fcntl::FcntlArg::F_SETFD(nix::fcntl::FdFlag::FD_CLOEXEC),
            )
            .unwrap();
        }
        let original = nix::sys::termios::tcgetattr(&pty.slave).unwrap();
        let slave = File::from(pty.slave);
        let master = File::from(pty.master);
        let status_path = root.path().join("status");
        let hold = root.path().join("hold");
        nix::unistd::mkfifo(&hold, nix::sys::stat::Mode::from_bits_truncate(0o600)).unwrap();
        // Keep a shell as the session leader: macOS revokes the slave when that leader exits.
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                r#"/bin/sh -c 'printf "%s" "$$" > "$YINZ_TEST_CLIENT_PID"; exec "$@"' client "$@"
printf "%s" "$?" > "$YINZ_TEST_CLIENT_STATUS"
read -r hold < "$YINZ_TEST_CLIENT_HOLD""#,
                "client",
            ])
            .arg(repo_root().join("target/debug/yinz"));
        command
            .env("YINZ_TEST_CLIENT_PID", root.path().join("pid"))
            .env("YINZ_TEST_CLIENT_STATUS", &status_path)
            .env("YINZ_TEST_CLIENT_HOLD", hold)
            .env_remove("YINZ")
            .env_remove("YINZ_SESSION_NAME")
            .env_remove("YINZ_PANE_ID")
            .env_remove("YINZ_PANE_REF")
            .env_remove("YINZ_CONFIG_DIR")
            .env_remove("YINZ_CONFIG_FILE")
            .env("YINZ_SOCKET_DIR", root.path().join("s"))
            .env("XDG_CONFIG_HOME", root.path())
            .env("XDG_CACHE_HOME", root.path())
            .env("TERM", "xterm-256color")
            .arg("--config")
            .arg(config)
            .args(["attach", "--remote", host, name])
            .stdin(slave.try_clone().unwrap())
            .stdout(slave.try_clone().unwrap())
            .stderr(slave.try_clone().unwrap());
        if create {
            command.arg("--create");
        }
        // The shell owns the controlling terminal while Yinz runs in its foreground group.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = Arc::new(Mutex::new(command.spawn().unwrap()));
        drop(command);
        let mut read = master.try_clone().unwrap();
        let flags = nix::fcntl::fcntl(&read, nix::fcntl::FcntlArg::F_GETFL).unwrap();
        nix::fcntl::fcntl(
            &read,
            nix::fcntl::FcntlArg::F_SETFL(
                nix::fcntl::OFlag::from_bits_truncate(flags) | nix::fcntl::OFlag::O_NONBLOCK,
            ),
        )
        .unwrap();
        let output = Watched::new(Output::default());
        let recorded = output.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let observed_child = child.clone();
        let reader = std::thread::spawn(move || {
            use std::os::fd::AsFd;
            let mut bytes = [0; 65536];
            while !stopping.load(Ordering::Acquire) {
                let mut polls = [nix::poll::PollFd::new(
                    read.as_fd(),
                    nix::poll::PollFlags::POLLIN,
                )];
                let _ = nix::poll::poll(&mut polls, 50u16);
                loop {
                    match read.read(&mut bytes) {
                        Ok(0) => break,
                        Ok(n) => recorded.lock().bytes.extend_from_slice(&bytes[..n]),
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
                let status = std::fs::read_to_string(&status_path)
                    .ok()
                    .and_then(|s| s.parse::<i32>().ok())
                    .map(|code| ExitStatus::from_raw(code << 8))
                    .or_else(|| observed_child.lock().unwrap().try_wait().unwrap());
                if let Some(status) = status {
                    recorded.lock().status = Some(status);
                }
            }
        });
        Self {
            child,
            master,
            slave,
            original,
            output,
            stop,
            reader: Some(reader),
            root,
        }
    }
    fn send(&mut self, bytes: &[u8]) {
        self.master.write_all(bytes).unwrap();
    }
    fn see(&self, text: &str) {
        self.output
            .wait_until(text, Duration::from_secs(45), |out| {
                assert!(
                    out.status.is_none(),
                    "client exited before {text}: {}",
                    String::from_utf8_lossy(&out.bytes)
                );
                out.bytes
                    .windows(text.len())
                    .any(|part| part == text.as_bytes())
                    .then_some(())
            });
    }
    fn marker(&mut self, suffix: &str) {
        self.send(format!("printf 'YINZ_CLIENT_%s\\n' {suffix}\r").as_bytes());
        self.see(&format!("YINZ_CLIENT_{suffix}"));
    }
    fn exit(&self, success: bool) -> String {
        let status = self
            .output
            .wait_until("client exit", Duration::from_secs(60), |out| out.status);
        assert_eq!(
            status.success(),
            success,
            "{}",
            String::from_utf8_lossy(&self.output.lock().bytes)
        );
        let current = nix::sys::termios::tcgetattr(&self.slave).unwrap();
        assert_eq!(
            current, self.original,
            "client must restore the local terminal"
        );
        let forwards = self.root.path().join("s").join("client");
        assert_eq!(
            std::fs::read_dir(forwards)
                .map(|entries| entries.count())
                .unwrap_or(0),
            0
        );
        let processes = Command::new("ps").args(["-Ao", "args="]).output().unwrap();
        assert!(
            !String::from_utf8_lossy(&processes.stdout)
                .lines()
                .any(|line| line.contains(" -L ")
                    && line.contains(self.root.path().to_str().unwrap())),
            "owned SSH forward survived client exit"
        );
        String::from_utf8_lossy(&self.output.lock().bytes).into_owned()
    }
    fn detach(&self) {
        let pid = std::fs::read_to_string(self.root.path().join("pid"))
            .unwrap()
            .parse()
            .unwrap();
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid),
            nix::sys::signal::Signal::SIGTERM,
        )
        .unwrap();
        self.exit(true);
    }
    fn resize(&self, rows: u16, cols: u16) {
        let size = nix::pty::Winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert_eq!(
            unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ as _, &size) },
            0
        );
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let mut child = self.child.lock().unwrap();
        // A failed assertion must also terminate the client's SSH children.
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(child.id() as i32),
            nix::sys::signal::Signal::SIGKILL,
        );
        let _ = child.wait();
        drop(child);
        self.stop.store(true, Ordering::Release);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if std::thread::panicking() {
            eprintln!(
                "client transcript: {}",
                String::from_utf8_lossy(&self.output.lock().bytes)
            );
        }
    }
}
struct Session(String);
impl Session {
    fn new() -> Self {
        test_env();
        proxy_set_enabled(true);
        // Provision a remote config so the first-run wizard cannot consume shell input.
        remote_sh("mkdir -p ~/.config/yinz; printf 'show_startup_tips false\\n' > ~/.config/yinz/config.kdl");
        Self(format!(
            "client-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        ))
    }
    fn action(&self, args: &[&str]) -> String {
        let mut argv = vec!["yinz", "--session", &self.0, "action"];
        argv.extend_from_slice(args);
        remote_exec(&argv)
    }
    fn alive(&self) {
        let sessions = remote_exec(&["yinz", "list-sessions", "--short", "--no-formatting"]);
        assert!(sessions.lines().any(|name| name == self.0), "{sessions}");
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        remote_exec(&["yinz", "delete-session", "--force", &self.0]);
    }
}

#[test]
#[ignore = "requires Docker SSH fixture"]
fn remote_client_input_resize_detach_concurrent_and_named_switch() {
    let mut session = Session::new();
    // Both the remote shell command and OpenSSH's forwarding parser must keep the name literal.
    session.0.push_str(" 'q:${HOME}%h'");
    let mut first = Client::start(HOST, &session.0, true);
    first.see(&session.0);
    first.marker("ONE");
    first.send(b"printf 'REMOTE_TERM=%s\\n' \"$TERM\"; tput cols >/dev/null && printf 'REMOTE_TPUT_%s\\n' OK\r");
    first.see("REMOTE_TERM=xterm-256color");
    first.see("REMOTE_TPUT_OK");
    first.resize(40, 120);
    wait_for_remote("remote pane resize", || {
        let panes: serde_json::Value =
            serde_json::from_str(&session.action(&["list-panes", "--json"])).unwrap();
        panes.as_array().unwrap().iter().any(|p| {
            p["is_plugin"] == false && p["pane_columns"].as_u64().is_some_and(|n| n >= 118)
        })
    });
    let mut second = Client::start(HOST, &session.0, false);
    second.see("YINZ_CLIENT_ONE");
    second.marker("TWO");
    first.see("YINZ_CLIENT_TWO");
    second.detach();
    first.detach();
    session.alive();
    let mut reattached = Client::start(HOST, &session.0, false);
    reattached.see("YINZ_CLIENT_TWO");
    let next = Session::new();
    session.action(&["switch-session", &next.0]);
    reattached.see(&next.0);
    reattached.marker("SWITCHED");
    next.alive();
    reattached.detach();
    session.alive();
}

#[test]
#[ignore = "requires Docker SSH fixture"]
fn remote_client_network_cut_restores_terminal_and_preserves_server() {
    let session = Session::new();
    let mut client = Client::start(HOST, &session.0, true);
    client.see(&session.0);
    client.marker("BEFORE_CUT");
    with_link_down(|| {
        let message = client.exit(false);
        assert!(
            message.contains("connection") || message.contains("Connection"),
            "{message}"
        );
        session.alive();
    });
    let mut reattached = Client::start(HOST, &session.0, false);
    reattached.see("YINZ_CLIENT_BEFORE_CUT");
    reattached.marker("AFTER_CUT");
    reattached.detach();
}

fn root_sh(script: &str) {
    let out = Command::new("docker")
        .args([
            "compose",
            "-p",
            COMPOSE_PROJECT,
            "-f",
            COMPOSE_FILE,
            "exec",
            "-T",
            "--user",
            "root",
            "remote",
            "sh",
            "-c",
            script,
        ])
        .current_dir(repo_root())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
struct RestoreFixture(&'static str);
impl Drop for RestoreFixture {
    fn drop(&mut self) {
        root_sh(self.0);
    }
}

#[test]
#[ignore = "requires Docker SSH fixture"]
fn remote_client_bootstrap_errors_and_denied_forwarding_leave_no_tunnel() {
    let session = Session::new();
    let client = Client::start(HOST, &session.0, false);
    assert!(client.exit(false).contains("does not exist"));
    let client = Client::start(&format!("no-such-user@{HOST}"), &session.0, false);
    assert!(client.exit(false).contains("Permission denied"));
    {
        root_sh("mv /usr/local/bin/yinz /usr/local/bin/yinz.saved");
        let _restore = RestoreFixture("mv /usr/local/bin/yinz.saved /usr/local/bin/yinz");
        let client = Client::start(HOST, &session.0, false);
        assert!(client.exit(false).contains("not found"));
    }
    {
        root_sh(
            r#"mv /usr/local/bin/yinz /usr/local/bin/yinz.saved
cat > /usr/local/bin/yinz <<'SH'
#!/bin/sh
printf '%s\n' '{"session_name":"unused","socket":"/tmp/unused","yinz_version":"incompatible","contract_version":0}'
SH
chmod +x /usr/local/bin/yinz"#,
        );
        let _restore = RestoreFixture("mv /usr/local/bin/yinz.saved /usr/local/bin/yinz");
        let client = Client::start(HOST, &session.0, true);
        let error = client.exit(false);
        assert!(
            error.contains("compatibility mismatch") && error.contains("incompatible"),
            "{error}"
        );
    }
    {
        root_sh("sed -i 's/AllowStreamLocalForwarding yes/AllowStreamLocalForwarding no/' /etc/ssh/sshd_config; kill -HUP 1");
        let _restore = RestoreFixture("sed -i 's/AllowStreamLocalForwarding no/AllowStreamLocalForwarding yes/' /etc/ssh/sshd_config; kill -HUP 1");
        let client = Client::start(HOST, &session.0, true);
        let error = client.exit(false);
        assert!(
            error.contains("forwarding") || error.contains("prohibited"),
            "{error}"
        );
        session.alive();
    }
}
