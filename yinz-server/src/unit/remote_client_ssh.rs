//! Real interactive clients on the local PTY, using the suite's SSH fixture.
use super::*;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, ExitStatus};
use std::sync::atomic::AtomicBool;
use yinz_panes::pane::TerminalPaneIo;
use yinz_test_support::Watched;

#[derive(Default)]
struct Output {
    bytes: Vec<u8>,
    screen: String,
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
    cache: PathBuf,
    local_session: Option<String>,
}
impl Client {
    fn start(host: &str, name: &str, create: bool) -> Self {
        let mut args = vec!["--remote", host, name];
        if create {
            args.push("--create");
        }
        Self::start_args(&args, None, None)
    }
    fn picker(cache: &std::path::Path) -> Self {
        Self::start_args(&[], Some(cache), None)
    }
    fn start_args(args: &[&str], cache: Option<&Path>, sockets: Option<&Path>) -> Self {
        test_env();
        let root = tempfile::Builder::new()
            .prefix("yrc-")
            .tempdir_in("/tmp")
            .unwrap();
        let config = root.path().join("config.kdl");
        // A nonexistent local default layout must not affect remote attachment or discovery.
        std::fs::write(&config, "default_layout \"/no/local/layout.kdl\"\n").unwrap();
        if args.contains(&"--create") && !args.contains(&"--remote") {
            let layout = root.path().join("local.kdl");
            std::fs::write(&layout, r#"layout { pane command="/bin/sh" { args "-c" "printf 'LOCAL_READY\\n'; exec /bin/sh"; }; }"#).unwrap();
            std::fs::write(&config, format!("show_startup_tips false\nload_plugins clear-defaults=true {{ \"session-palette\"; }}\ndefault_layout {:?}\n", layout)).unwrap();
        }
        let builtins = root.path().join("data/yinz/plugins/builtins");
        std::fs::create_dir_all(&builtins).unwrap();
        std::fs::copy(
            repo_root().join("yinz-steel-builtins/assets/plugins/session-palette.scm"),
            builtins.join("session-palette.scm"),
        )
        .unwrap();
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
        let cache = cache
            .map(Path::to_path_buf)
            .unwrap_or_else(|| root.path().join("cache"));
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
            .env(
                "YINZ_SOCKET_DIR",
                sockets
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| root.path().join("s")),
            )
            .env("XDG_CONFIG_HOME", root.path())
            .env("XDG_DATA_HOME", root.path().join("data"))
            .env("YINZ_TEST_CACHE_DIR", &cache)
            .env("TERM", "xterm-256color")
            .arg("--config")
            .arg(config)
            .arg("attach")
            .stdin(slave.try_clone().unwrap())
            .stdout(slave.try_clone().unwrap())
            .stderr(slave.try_clone().unwrap());
        command.args(args);
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
            use yinz_panes::{pane::Pane, panes::TerminalPane};
            let mut size = yinz_types::pane_size::PaneGeom::default();
            size.rows.set_inner(30);
            size.cols.set_inner(100);
            let mut terminal = TerminalPane::new(
                yinz_types::internal_ids::TerminalId::new(0),
                size,
                Default::default(),
                0,
                String::new(),
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                None,
                None,
                false,
                true,
                true,
                true,
                false,
                None,
            );
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
                        Ok(n) => {
                            terminal.handle_pty_bytes(bytes[..n].to_vec());
                            while !terminal.drain_forwarded_queries().is_empty() {
                                let pending = terminal.drain_pending_pty_input();
                                terminal.handle_pty_bytes(pending);
                            }
                            let mut output = recorded.lock();
                            output.bytes.extend_from_slice(&bytes[..n]);
                            output.screen = terminal.dump_screen(false, None);
                        },
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
            cache,
            local_session: (!args.contains(&"--remote"))
                .then(|| args.last().map(|name| (*name).to_owned()))
                .flatten(),
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
    fn see_screen(&self, text: &str) {
        self.output.wait_until_described(
            text,
            Duration::from_secs(45),
            |out| {
                assert!(out.status.is_none(), "client exited: {}", out.screen);
                out.screen.contains(text).then_some(())
            },
            |out| out.screen.clone(),
        );
    }
    fn switch_picker(&mut self, label: &str) {
        self.output.lock().bytes.clear();
        self.send(b"\x1b[112;7u");
        self.send(b"Switch session");
        self.see_screen("› Switch session");
        self.output.lock().bytes.clear();
        self.send(b"\r");
        self.see_screen("Current session");
        self.send(label.as_bytes());
        let selected = format!("› {label}");
        let already_selected = self.output.wait_until_described(
            "filtered session rows",
            Duration::from_secs(45),
            |out| {
                (out.screen.contains(&format!("> {label}"))
                    && out.screen.contains(&format!("{label} —"))
                    && out
                        .screen
                        .lines()
                        .filter(|line| line.contains(" / ") && !line.contains("│>"))
                        .all(|line| line.contains(label)))
                .then(|| out.screen.contains(&selected))
            },
            |out| out.screen.clone(),
        );
        if !already_selected {
            self.send(b"\x1b[B");
        }
        self.see_screen(&selected);
        self.output.lock().bytes.clear();
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
        if let Some(name) = &self.local_session {
            let _ = Command::new(repo_root().join("target/debug/yinz"))
                .env("YINZ_SOCKET_DIR", self.root.path().join("s"))
                .args(["kill-session", name])
                .output();
        }
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
    assert!(yinz_runtime::remote_hosts::load(&client.cache)
        .unwrap()
        .is_empty());
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

#[test]
#[ignore = "requires Docker SSH fixture"]
fn remote_client_remembers_host_and_picker_discovers_and_attaches() {
    let session = Session::new();
    let mut first = Client::start(HOST, &session.0, true);
    first.see(&session.0);
    first.marker("DISCOVERED");
    first.detach();
    let cache = &first.cache;
    assert_eq!(yinz_runtime::remote_hosts::load(cache).unwrap(), [HOST]);
    let response = remote_exec(&[
        "yinz",
        "remote-sessions",
        "--client-version",
        yinz_runtime::consts::VERSION,
        "--contract-version",
        &yinz_runtime::consts::CLIENT_SERVER_CONTRACT_VERSION.to_string(),
        "--bootstrap-version",
        &yinz_runtime::consts::REMOTE_BOOTSTRAP_VERSION.to_string(),
    ]);
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert!(response["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row[0] == session.0));
    let mut picker = Client::picker(cache);
    picker.see(&format!("{HOST} / {}", session.0));
    picker.send(format!("{}\r", session.0).as_bytes());
    picker.see("YINZ_CLIENT_DISCOVERED");
    picker.marker("PICKED");
    picker.detach();
    assert_eq!(yinz_runtime::remote_hosts::load(cache).unwrap(), [HOST]);
    let mut cancelled = Client::picker(cache);
    cancelled.see("Session:");
    cancelled.send(b"\x1b");
    cancelled.exit(true);
    let interrupted = Client::picker(cache);
    interrupted.see("Session:");
    let pid = std::fs::read_to_string(interrupted.root.path().join("pid"))
        .unwrap()
        .parse()
        .unwrap();
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGTERM,
    )
    .unwrap();
    interrupted.exit(false);
    let local_name = format!("local-{}", session.0);
    let mut local = Client::start_args(&["--create", &local_name], Some(cache), None);
    local.see("LOCAL_READY");
    local.marker("LOCAL");
    let mut invalid_layout =
        Client::start_args(&[], Some(cache), Some(&local.root.path().join("s")));
    invalid_layout.see(&local_name);
    invalid_layout.send(format!("{local_name}\r").as_bytes());
    assert!(invalid_layout.exit(false).contains("/no/local/layout.kdl"));
    let binary =
        yinz_process::ssh::shell_argument(repo_root().join("target/debug/yinz").to_str().unwrap())
            .unwrap();
    let config =
        yinz_process::ssh::shell_argument(local.root.path().join("config.kdl").to_str().unwrap())
            .unwrap();
    local.send(format!("{binary} --config {config} attach\r").as_bytes());
    local.see("Enter switch");
    local.see(&format!("{HOST} / {}", session.0));
    local.send(format!("{}\r", session.0).as_bytes());
    local.see("Nested Yinz session detected:");
    local.send(b"1");
    local.see("YINZ_CLIENT_PICKED");
    local.marker("NESTED");
    session.action(&["detach"]);
    local.marker("RETURNED_LOCAL");
    local.detach();
    session.alive();
}

#[test]
#[ignore = "requires Docker SSH fixture"]
fn attached_palette_switches_machines_with_identical_names_and_survives_failed_handoff() {
    let session = Session::new();
    let mut remote = Client::start(HOST, &session.0, true);
    remote.see(&session.0);
    remote.marker("DESTINATION");
    remote.detach();
    let second = Session::new();
    let alternate_host = format!("test@{HOST}");
    let mut another = Client::start_args(
        &["--remote", &alternate_host, &second.0, "--create"],
        Some(&remote.cache),
        None,
    );
    another.see(&second.0);
    another.marker("SECOND");
    another.detach();
    let mut local = Client::start_args(&["--create", &session.0], Some(&remote.cache), None);
    local.see("LOCAL_READY");
    local.marker("SOURCE");
    let label = format!("{HOST} / {}", session.0);
    local.switch_picker(&label);
    proxy_set_enabled(false);
    local.send(b"\r");
    local.see("SSH");
    proxy_set_enabled(true);
    local.send(b"\x1b[112;7u");
    local.marker("AFTER_FAILURE");
    local.switch_picker(&label);
    local.send(b"\r");
    local.see("YINZ_CLIENT_DESTINATION");
    local.marker("ON_REMOTE");
    // Both a same-host session change and a different remembered SSH endpoint use the coordinator.
    local.switch_picker(&format!("{HOST} / {}", second.0));
    local.send(b"\r");
    local.see("YINZ_CLIENT_SECOND");
    local.marker("SAME_HOST");
    local.switch_picker(&format!("{alternate_host} / {}", session.0));
    local.send(b"\r");
    local.see("YINZ_CLIENT_ON_REMOTE");
    local.marker("OTHER_ENDPOINT");
    local.switch_picker(&format!("Local / {}", session.0));
    local.send(b"\r");
    local.see("YINZ_CLIENT_AFTER_FAILURE");
    local.marker("BACK_LOCAL");
    local.detach();
    session.alive();
}

#[test]
#[ignore = "requires Docker SSH fixture"]
fn attached_palette_resurrects_a_saved_session_on_the_current_remote_host() {
    let source = Session::new();
    let saved = Session::new();
    let mut original = Client::start(HOST, &saved.0, true);
    original.see(&saved.0);
    original.marker("BEFORE_SAVE");
    saved.action(&["save-session"]);
    original.detach();
    remote_exec(&["yinz", "kill-session", &saved.0]);
    let mut client = Client::start(HOST, &source.0, true);
    client.see(&source.0);
    client.marker("SOURCE");
    client.switch_picker(&format!("{HOST} / {}", saved.0));
    client.see_screen("Saved");
    client.send(b"\r");
    client.see_screen(&format!("({})", saved.0));
    client.marker("RESURRECTED");
    client.detach();
    source.alive();
    saved.alive();
}
