#![cfg(unix)]

//! Remote panes, against a real machine.
//!
//! Every other test of `pty_host::remote` stops at the same seam.
//! `pty_host_remote_ssh_tests.rs` says so in its first line — "tests for the
//! parts of the ssh path that do not need an ssh" — and the rest fake the far
//! side with a `UnixStream::pair()` and a thread. That leaves the half of the
//! feature that is *only* the far side unverified: whether `ssh` can be made to
//! forward a unix socket at all, whether a broker started over ssh survives the
//! ssh exiting, whether a replay ring on another machine really does hand back
//! the bytes an outage swallowed.
//!
//! So these tests use a real one. `docker-compose.remote-panes.yml` stands up an
//! sshd with `AllowStreamLocalForwarding yes` and the musl build of this very
//! tree on its PATH, reachable through a proxy that a test can switch off
//! mid-pane. Nothing below the test is mocked: a real `ssh` subprocess, a real
//! detached broker, a real supervisor, a real pty, real `/proc`.
//!
//! They are `#[ignore]`d, so `cargo x test` and `cargo x integration-test` never
//! run them and never need docker. To run them:
//!
//! ```sh
//! cargo x remote-test --build   # musl binary, keypair, containers
//! cargo x remote-test --test
//! ```
//!
//! **These tests share one far machine and set process-wide environment**, so
//! they must run single-threaded. `cargo x remote-test --test` passes
//! `--test-threads 1`; running them by hand without it will produce nonsense.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use zellij_server::os_input_output::AsyncReader;
use zellij_server::pty_host::protocol::WireWinsize;
use zellij_server::pty_host::remote::conn::{RemoteGitProbe, RemoteStatus};
use zellij_server::pty_host::remote::protocol::{
    self, BrokerToServerMsg, ChildSignal, ServerToBrokerMsg, REMOTE_PROTOCOL_VERSION,
};
use zellij_server::pty_host::remote::ssh::{self, RemoteHosts};
use zellij_server::pty_host::remote::{RemoteConn, RemoteReader};
use zellij_utils::input::command::RunCommand;

// ---------------------------------------------------------------------------
// Where the far machine is
// ---------------------------------------------------------------------------

/// The ssh destination. An alias defined in the generated config rather than a
/// `user@host`, because resolving an alias out of `~/.ssh/config` is part of
/// what `--remote-host` promises.
const HOST: &str = "zellij-remote-pane";
const COMPOSE_FILE: &str = "docker-compose.remote-panes.yml";
/// Toxiproxy's control API, as published by the compose file.
const PROXY_API: &str = "127.0.0.1:8474";
/// The proxy named in `docker/remote-pane-toxiproxy.json`.
const PROXY_NAME: &str = "sshd";

/// How long to wait for something the far machine has to do — spawn a process,
/// notice a signal, run its 1-second status tick. Generous because a container
/// under a loaded test run is not a fast machine.
const SETTLE: Duration = Duration::from_secs(15);

/// Overrides `RECONNECT_ATTEMPTS` for this whole process.
///
/// Five attempts is a ladder of 250 ms + 500 + 1 s + 2 s + 4 s. Long enough that
/// a two-second outage is comfortably ridden out, short enough that the test
/// which *wants* the giving-up does not spend a minute asleep to get it.
const RECONNECT_ATTEMPTS: &str = "5";

// ---------------------------------------------------------------------------
// Process environment
// ---------------------------------------------------------------------------

/// Point this process at the generated ssh shim and a short socket directory.
///
/// The shim is how the code under test is given an ssh config. It runs
/// `ssh <host>` with no `-F`, so its configuration is ordinarily the user's own,
/// which a test may not write to; and `$HOME` is no way in, because OpenSSH
/// resolves `~/.ssh/config` through `getpwuid()` rather than the environment.
/// `PATH` *is* honoured, since `Command::new("ssh")` resolves through it, so
/// `cargo x remote-test --build` writes an `ssh` that execs the real one with
/// `-F <test config>` and this puts it first. Nothing else about the invocation
/// is touched.
///
/// This is also why these tests cannot run in parallel: `PATH` is process-wide
/// state, and `set_var` is unsound with other threads live.
///
/// The socket dir is deliberately *not* under `target/`. `ZELLIJ_SOCK_DIR` ends
/// up in a `sockaddr_un`, which holds 108 bytes; a checkout path plus
/// `target/remote-test/` plus the contract-version dir plus a hashed socket name
/// blows through that, and `check_socket_path_length` would refuse every pane.
fn test_env() -> &'static TestEnv {
    static ENV: OnceLock<TestEnv> = OnceLock::new();
    ENV.get_or_init(|| {
        let shim_dir = remote_test_dir().join("bin");
        assert!(
            shim_dir.join("ssh").exists(),
            "no ssh shim at {} — run 'cargo x remote-test --build' first",
            shim_dir.join("ssh").display()
        );

        let sockets = PathBuf::from(format!("/tmp/zellij-remote-test-{}", unsafe {
            libc::getuid()
        }));
        std::fs::create_dir_all(&sockets)
            .unwrap_or_else(|e| panic!("failed to create {}: {e}", sockets.display()));

        let path = match std::env::var_os("PATH") {
            Some(existing) => {
                let mut dirs = vec![shim_dir.clone()];
                dirs.extend(std::env::split_paths(&existing));
                std::env::join_paths(dirs).expect("failed to build a PATH with the ssh shim")
            },
            None => shim_dir.clone().into_os_string(),
        };
        std::env::set_var("PATH", path);
        std::env::set_var("ZELLIJ_SOCKET_DIR", &sockets);
        std::env::set_var("ZELLIJ_REMOTE_RECONNECT_ATTEMPTS", RECONNECT_ATTEMPTS);
        // Nothing here starts a zellij session, but a stray `ZELLIJ` in the
        // developer's shell would make the binary under test think otherwise.
        std::env::remove_var("ZELLIJ");
        std::env::remove_var("ZELLIJ_SESSION_NAME");

        let env = TestEnv { sockets };
        sweep_leftover_panes();
        env
    })
}

struct TestEnv {
    sockets: PathBuf,
}

fn repo_root() -> PathBuf {
    // zellij-server/ -> the workspace root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("zellij-server has a parent directory")
        .to_path_buf()
}

fn remote_test_dir() -> PathBuf {
    repo_root().join("target/remote-test")
}

/// Everything a test needs before it can talk to the far machine.
///
/// Also puts the link back up, so a test that failed with the proxy switched off
/// does not take every test after it down with it.
fn start() -> Arc<RemoteHosts> {
    test_env();
    proxy_set_enabled(true);
    Arc::new(RemoteHosts::new())
}

// ---------------------------------------------------------------------------
// The far machine, seen from outside ssh
// ---------------------------------------------------------------------------

/// Run a shell command on the far machine *without* using ssh.
///
/// The point of going through docker rather than ssh is independence: these are
/// the assertions that check whether ssh did what it claimed, so they must not
/// depend on ssh working — and half of them run while the link is deliberately
/// cut.
///
/// As `test`, not as root, because that is who the panes run as. A fixture set
/// up by root is a fixture the pane may not be able to use: git, for one,
/// refuses to look at a repository owned by somebody else and reports it as an
/// empty probe rather than as an error.
fn remote_sh(script: &str) -> String {
    remote_exec(&["sh", "-c", script])
}

/// Run a command on the far machine directly, with no shell in between.
fn remote_exec(argv: &[&str]) -> String {
    let mut command = Command::new("docker");
    command
        .args([
            "compose",
            "-f",
            COMPOSE_FILE,
            "exec",
            "-T",
            "--user",
            "test",
            "remote",
        ])
        .args(argv)
        .current_dir(repo_root());
    let output = command
        .output()
        .unwrap_or_else(|e| panic!("failed to run docker compose exec: {e}. Is docker running?"));
    // Exit status is not checked: `pgrep` exits 1 when it matches nothing, which
    // is a perfectly ordinary answer to most of the questions asked here.
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// The pids on the far machine whose argv matches `pattern`.
///
/// Deliberately not through `remote_sh`: a `sh -c "pgrep -f <pattern>"` carries
/// the pattern in its own argv, and pgrep excludes itself but not its parent, so
/// the wrapper shell would intermittently count as a match. Running pgrep
/// directly leaves nothing holding the pattern but the processes being asked
/// about.
fn remote_pids(pattern: &str) -> Vec<u32> {
    remote_exec(&["pgrep", "-f", pattern])
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Wait for something on the far machine to become true.
fn wait_for_remote(what: &str, predicate: impl FnMut() -> bool) {
    wait_within(what, SETTLE, predicate);
}

fn wait_within(what: &str, timeout: Duration, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("timed out after {timeout:?} waiting for {what} on the far machine");
}

// ---------------------------------------------------------------------------
// The link, as something a test can cut
// ---------------------------------------------------------------------------

/// Switch the proxy in front of sshd on or off.
///
/// Disabling closes every open connection immediately, which is as close to
/// pulling the cable as a test can get: the server's forward dies mid-pane, the
/// broker keeps running, and nothing on either side was asked politely to stop.
fn proxy_set_enabled(enabled: bool) {
    let body = format!("{{\"enabled\":{enabled}}}");
    let request = format!(
        "POST /proxies/{PROXY_NAME} HTTP/1.1\r\n\
         Host: {PROXY_API}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    let mut stream = TcpStream::connect(PROXY_API).unwrap_or_else(|e| {
        panic!("could not reach the toxiproxy API at {PROXY_API}: {e}. Are the containers up?")
    });
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("failed to set a read timeout on the toxiproxy connection");
    stream
        .write_all(request.as_bytes())
        .expect("failed to write to the toxiproxy API");
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "toxiproxy refused to set enabled={enabled}: {response}"
    );
}

/// Cut the link for the duration of a closure, and put it back afterwards even
/// if the closure panics.
fn with_link_down<T>(body: impl FnOnce() -> T) -> T {
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            proxy_set_enabled(true);
        }
    }
    proxy_set_enabled(false);
    let _restore = Restore;
    body()
}

// ---------------------------------------------------------------------------
// A pane
// ---------------------------------------------------------------------------

/// A remote pane with its reader being pumped on a thread.
///
/// The pump exists because a `RemoteReader` is an async, borrow-hungry thing and
/// every interesting test wants to be doing something else — cutting the link,
/// asking docker a question — while output keeps arriving.
struct Pane {
    conn: Arc<RemoteConn>,
    output: Arc<Mutex<Vec<u8>>>,
    /// `Some` once the reader has finished: `Ok(())` for a clean EOF (which
    /// means, and may only mean, that the child exited), `Err` for a transport
    /// failure it could not recover from.
    ended: Arc<Mutex<Option<Result<(), String>>>>,
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
    pump: Option<std::thread::JoinHandle<()>>,
}

impl Pane {
    fn output(&self) -> String {
        String::from_utf8_lossy(&self.output.lock().unwrap()).into_owned()
    }

    /// Wait for `needle` to appear in the pane's output, returning everything
    /// received so far.
    fn wait_for_output(&self, needle: &str) -> String {
        self.wait_for_output_within(needle, SETTLE)
    }

    fn wait_for_output_within(&self, needle: &str, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        loop {
            let seen = self.output();
            if seen.contains(needle) {
                return seen;
            }
            if let Some(ended) = self.ended.lock().unwrap().as_ref() {
                panic!(
                    "the pane ended ({ended:?}) before {needle:?} appeared. Output so far:\n{seen}"
                );
            }
            if Instant::now() >= deadline {
                panic!(
                    "timed out after {timeout:?} waiting for {needle:?}. Output so far:\n{seen}"
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn wait_until_ended(&self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(ended) = self.ended.lock().unwrap().clone() {
                return ended;
            }
            if Instant::now() >= deadline {
                panic!(
                    "the pane was still running after {timeout:?}. Output so far:\n{}",
                    self.output()
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Write pty input, retrying the short counts `write_input` is allowed to
    /// return, exactly as `pty_writer` does.
    fn send(&self, bytes: &[u8]) {
        let deadline = Instant::now() + SETTLE;
        let mut written = 0;
        while written < bytes.len() {
            let accepted = self
                .conn
                .write_input(&bytes[written..])
                .expect("failed to write to a remote pane");
            written += accepted;
            if accepted == 0 {
                assert!(
                    Instant::now() < deadline,
                    "the remote pane never accepted the rest of {} bytes of input",
                    bytes.len()
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    fn send_line(&self, line: &str) {
        self.send(format!("{line}\n").as_bytes());
    }

    fn status(&self) -> RemoteStatus {
        self.conn.status()
    }

    fn git_probe(&self) -> Option<RemoteGitProbe> {
        self.conn.git_probe()
    }

    /// Wait for the far machine's status pump to report something we care about.
    fn wait_for_status(
        &self,
        what: &str,
        mut predicate: impl FnMut(&RemoteStatus) -> bool,
    ) -> RemoteStatus {
        let deadline = Instant::now() + SETTLE;
        loop {
            let status = self.status();
            if predicate(&status) {
                return status;
            }
            if Instant::now() >= deadline {
                panic!("timed out waiting for the far machine to report {what}; last status was {status:?}");
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Stop reading, without telling the far side anything.
    ///
    /// What a server crashing looks like from the broker's side: the connection
    /// simply ends, and the pane it was carrying is left running and unclaimed.
    fn abandon(mut self) {
        self.stop_pump();
    }

    /// Tell the far side the pane is over, then stop reading.
    fn close(mut self) {
        let _ = self.conn.terminate();
        self.stop_pump();
    }

    fn stop_pump(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        if let Some(pump) = self.pump.take() {
            let _ = pump.join();
        }
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        // A pump left running would keep trying to reconnect through whatever
        // the next test does to the link.
        self.stop_pump();
    }
}

/// Open a pane on the far machine running `/bin/sh -c <script>`.
fn open_pane(hosts: &Arc<RemoteHosts>, script: &str) -> Pane {
    open_pane_in(hosts, script, None)
}

fn open_pane_in(hosts: &Arc<RemoteHosts>, script: &str, cwd: Option<&str>) -> Pane {
    open_pane_with(hosts, script, cwd, vec![], 0)
}

/// `open_pane_in`, but *saying* what this side's environment is rather than
/// reading it. These tests share one process, so setting `TERM` for real would
/// set it for every other test in the binary at the same time.
fn open_pane_with(
    hosts: &Arc<RemoteHosts>,
    script: &str,
    cwd: Option<&str>,
    env: Vec<(String, String)>,
    terminal_id: u32,
) -> Pane {
    let cmd = RunCommand {
        command: PathBuf::from("/bin/sh"),
        args: vec!["-c".to_owned(), script.to_owned()],
        cwd: cwd.map(PathBuf::from),
        remote_host: Some(HOST.to_owned()),
        ..Default::default()
    };
    let (conn, reader_socket) = hosts
        .open_pane_with_env(HOST, &cmd, None, terminal_id, env)
        .unwrap_or_else(|e| panic!("failed to open a pane on {HOST}: {e:#}"));
    let reader = RemoteReader::new(conn.clone(), reader_socket)
        .expect("failed to build a reader for a remote pane");
    pump(conn, reader)
}

/// Open a pane with no command at all — the far machine's login shell.
///
/// Deliberately not `open_pane_in` with a shell path: naming one here would test
/// that this side can say "/bin/bash", which is exactly the thing that must not
/// happen. The empty command is the request.
fn open_default_shell_pane(hosts: &Arc<RemoteHosts>, cwd: Option<&str>) -> Pane {
    open_default_shell_pane_with(hosts, cwd, vec![], 0)
}

fn open_default_shell_pane_with(
    hosts: &Arc<RemoteHosts>,
    cwd: Option<&str>,
    env: Vec<(String, String)>,
    terminal_id: u32,
) -> Pane {
    let cmd = RunCommand {
        command: PathBuf::new(),
        remote_default_shell: true,
        cwd: cwd.map(PathBuf::from),
        remote_host: Some(HOST.to_owned()),
        ..Default::default()
    };
    let (conn, reader_socket) = hosts
        .open_pane_with_env(HOST, &cmd, None, terminal_id, env)
        .unwrap_or_else(|e| panic!("failed to open a shell pane on {HOST}: {e:#}"));
    let reader = RemoteReader::new(conn.clone(), reader_socket)
        .expect("failed to build a reader for a remote pane");
    pump(conn, reader)
}

fn pump(conn: Arc<RemoteConn>, mut reader: RemoteReader) -> Pane {
    let output = Arc::new(Mutex::new(Vec::new()));
    let ended = Arc::new(Mutex::new(None));
    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel();

    let pump_output = output.clone();
    let pump_ended = ended.clone();
    let pump = std::thread::Builder::new()
        .name("remote-pane-test-pump".to_owned())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build a runtime for the pane pump");
            runtime.block_on(async move {
                let mut buf = [0u8; 8192];
                loop {
                    tokio::select! {
                        // Cancellation drops the reader, and with it the socket.
                        // Without it a teardown would sit through the whole
                        // reconnect ladder every time a test cut the link.
                        _ = &mut cancel_rx => return,
                        read = reader.read(&mut buf) => match read {
                            Ok(0) => {
                                *pump_ended.lock().unwrap() = Some(Ok(()));
                                return;
                            },
                            Ok(n) => pump_output.lock().unwrap().extend_from_slice(&buf[..n]),
                            Err(e) => {
                                *pump_ended.lock().unwrap() = Some(Err(e.to_string()));
                                return;
                            },
                        },
                    }
                }
            });
        })
        .expect("failed to start a pane pump thread");

    Pane {
        conn,
        output,
        ended,
        cancel: Some(cancel_tx),
        pump: Some(pump),
    }
}

/// A marker no other test will produce, so `pgrep` on the far machine can find
/// exactly this test's process.
///
/// The pid is in there because the counter restarts with every run, and a pane
/// leaked by an earlier *failed* run would otherwise answer to this run's name —
/// which shows up as a mysterious second process and sends you looking for a
/// double-spawn that never happened. `MARKER_PREFIX` is what
/// `sweep_leftover_panes` uses to clear those out.
fn marker(name: &str) -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    format!(
        "{MARKER_PREFIX}{name}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

const MARKER_PREFIX: &str = "zellij-remote-test-";

/// Kill anything an earlier run left behind on the far machine.
///
/// Panes outliving their server is the feature, not a bug, so a run that failed
/// midway leaves its panes running over there until the orphan timeout — and
/// they are indistinguishable from this run's until then.
fn sweep_leftover_panes() {
    let _ = remote_exec(&["pkill", "-f", MARKER_PREFIX]);
}

/// A script that sits there, identifiable, until something kills it.
fn idle_marked(marker: &str) -> String {
    format!("echo {marker}-started; while :; do sleep 0.2; done")
}

/// How many `ssh -L` forwards this process has open.
fn local_forward_count(env: &TestEnv) -> usize {
    let output = Command::new("ps")
        .args(["-Ao", "args="])
        .output()
        .expect("failed to run ps");
    let needle = env.sockets.display().to_string();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.contains(" -L ") && line.contains(&needle))
        .count()
}

// ---------------------------------------------------------------------------
// Bootstrap and forward
// ---------------------------------------------------------------------------

/// The first thing a remote pane does, and the one step with no fallback: ask
/// the far machine where its broker listens, starting one if there is none.
#[test]
#[ignore]
fn bootstrap_starts_a_broker_on_the_far_machine_and_says_where_it_listens() {
    test_env();
    proxy_set_enabled(true);
    remote_sh("pkill -f 'pty-broker serve' || true");
    wait_for_remote("the old broker to go away", || {
        remote_pids("pty-broker serve").is_empty()
    });

    let endpoint = ssh::bootstrap(HOST).expect("failed to bootstrap a broker");

    assert_eq!(
        endpoint.protocol_version, REMOTE_PROTOCOL_VERSION,
        "the far machine speaks a different remote protocol than this build"
    );
    assert!(
        endpoint.socket.ends_with("pty-broker.sock"),
        "unexpected broker socket path: {}",
        endpoint.socket
    );
    // The path is the far machine's to decide — `ZELLIJ_SOCK_DIR` resolves
    // differently per platform, which is the whole reason it is reported back
    // rather than computed here.
    assert!(
        endpoint.socket.starts_with('/'),
        "the broker reported a relative socket path: {}",
        endpoint.socket
    );
    assert_eq!(
        remote_pids("pty-broker serve").len(),
        1,
        "bootstrap should have started exactly one broker"
    );

    // Idempotent: the second call finds the first broker rather than starting a
    // second one alongside it.
    let again = ssh::bootstrap(HOST).expect("failed to bootstrap a second time");
    assert_eq!(again.socket, endpoint.socket);
    assert_eq!(
        remote_pids("pty-broker serve").len(),
        1,
        "a second bootstrap started a second broker"
    );
}

/// The broker is started by an ssh that exits immediately afterwards. If it died
/// with its parent, every pane on that machine would die with the connection
/// that happened to start the daemon.
#[test]
#[ignore]
fn the_broker_outlives_the_ssh_that_started_it() {
    test_env();
    proxy_set_enabled(true);
    remote_sh("pkill -f 'pty-broker serve' || true");
    wait_for_remote("the old broker to go away", || {
        remote_pids("pty-broker serve").is_empty()
    });

    ssh::bootstrap(HOST).expect("failed to bootstrap a broker");
    let pid = remote_pids("pty-broker serve");
    assert_eq!(pid.len(), 1, "expected exactly one broker, got {pid:?}");

    // No ssh of ours is left running: `bootstrap` waits for the command to exit
    // before it parses the endpoint. So if the broker is still alive a moment
    // later, it is because `setsid` put it in its own session.
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(
        remote_pids("pty-broker serve"),
        pid,
        "the broker did not survive the ssh that started it"
    );

    // And its parent is init, not a dead sshd.
    let ppid = remote_sh(&format!("ps -o ppid= -p {}", pid[0]));
    assert_eq!(
        ppid.trim(),
        "1",
        "the broker was not reparented; it is still a child of {ppid}"
    );
}

/// One forward per host, shared by every pane on it. ssh opens a channel per
/// connection, so multiplexing costs nothing and needs no multiplexer.
#[test]
#[ignore]
fn two_panes_on_one_host_share_one_ssh_forward() {
    let env = test_env();
    let hosts = start();
    let before = local_forward_count(env);

    let first_marker = marker("share-one");
    let second_marker = marker("share-two");
    let first = open_pane(&hosts, &idle_marked(&first_marker));
    let after_one = local_forward_count(env);
    let second = open_pane(&hosts, &idle_marked(&second_marker));

    first.wait_for_output(&format!("{first_marker}-started"));
    second.wait_for_output(&format!("{second_marker}-started"));

    assert_eq!(
        after_one,
        before + 1,
        "opening the first pane should have started exactly one ssh forward"
    );
    assert_eq!(
        local_forward_count(env),
        after_one,
        "the second pane started a forward of its own instead of sharing"
    );
    // Two separate panes, not one pane heard twice.
    assert!(!first.output().contains(&second_marker));
    assert!(!second.output().contains(&first_marker));

    first.close();
    second.close();
}

/// Sharing is *within* a server, never between two. Two zellij sessions belong
/// to the same user and so share `ZELLIJ_SOCK_DIR`; on a host-keyed socket name
/// they would land on the same path, and the second would unlink and rebind the
/// first's socket — after which the first routes its panes through the second's
/// ssh, and the first to close deletes the other's socket.
#[test]
#[ignore]
fn two_servers_on_one_host_keep_their_forwards_to_themselves() {
    let env = test_env();
    proxy_set_enabled(true);
    let before = local_forward_count(env);

    let first_server = Arc::new(RemoteHosts::new());
    let second_server = Arc::new(RemoteHosts::new());
    assert_ne!(
        first_server.forward_socket_path(HOST),
        second_server.forward_socket_path(HOST),
        "two servers picked the same forward socket for the same host"
    );

    let first_marker = marker("two-servers-one");
    let second_marker = marker("two-servers-two");
    let first = open_pane(&first_server, &idle_marked(&first_marker));
    let second = open_pane(&second_server, &idle_marked(&second_marker));
    first.wait_for_output(&format!("{first_marker}-started"));
    second.wait_for_output(&format!("{second_marker}-started"));

    assert_eq!(
        local_forward_count(env),
        before + 2,
        "two servers should have one forward each, not one between them"
    );

    // Closing the first must leave the second's pane working: on a shared path
    // the first server's teardown unlinks the socket the second is using.
    drop(first);
    first_server.shutdown();

    second.send_line("");
    second.wait_for_output_within(&format!("{second_marker}-started"), Duration::from_secs(5));
    let third_marker = marker("two-servers-three");
    let third = open_pane(&second_server, &idle_marked(&third_marker));
    third.wait_for_output(&format!("{third_marker}-started"));

    second.close();
    third.close();
    second_server.shutdown();
}

// ---------------------------------------------------------------------------
// A pane, for real
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn a_pane_runs_on_the_far_machine_and_its_output_comes_back() {
    let hosts = start();
    let marker = marker("runs-there");
    // The output could in principle have come from anywhere, and so could a
    // process: the container's processes are visible in this machine's process
    // table too, so `ps` cannot tell the two apart. A file can — the container
    // has its own filesystem, and nothing here is shared with /tmp over there.
    let witness = format!("/tmp/{marker}");
    let pane = open_pane(
        &hosts,
        &format!("touch {witness}; {}", idle_marked(&marker)),
    );

    pane.wait_for_output(&format!("{marker}-started"));

    assert_eq!(
        remote_pids(&marker).len(),
        1,
        "expected exactly one process matching {marker} on the far machine"
    );
    wait_for_remote("the pane's file to appear over there", || {
        remote_sh(&format!("test -f {witness} && echo yes")) == "yes"
    });
    assert!(
        !Path::new(&witness).exists(),
        "a 'remote' pane ran its process on this machine: it left {witness} here"
    );

    pane.close();
    remote_sh(&format!("rm -f {witness}"));
}

#[test]
#[ignore]
fn input_written_locally_reaches_the_remote_child() {
    let hosts = start();
    let marker = marker("input");
    // A shell reading lines, so input has to arrive as pty input rather than as
    // anything the spawn itself could have carried.
    let pane = open_pane(
        &hosts,
        &format!("echo {marker}-ready; while read line; do echo got:$line; done"),
    );
    pane.wait_for_output(&format!("{marker}-ready"));

    pane.send_line("hello-from-the-server");

    let seen = pane.wait_for_output("got:hello-from-the-server");
    assert!(
        seen.contains("got:hello-from-the-server"),
        "the remote child never saw the input: {seen}"
    );

    pane.close();
}

/// A resize has to reach the pty on the other machine, or every full-screen
/// application in a remote pane draws at the wrong size forever.
#[test]
#[ignore]
fn a_resize_reaches_the_remote_pty() {
    let hosts = start();
    let marker = marker("resize");
    // 28 rather than the name: SIGWINCH is a signal number busybox ash and every
    // other POSIX shell agree on, where support for the name is patchier.
    let pane = open_pane(
        &hosts,
        &format!("trap 'stty size' 28; echo {marker}-ready; while :; do sleep 0.2; done"),
    );
    pane.wait_for_output(&format!("{marker}-ready"));

    pane.conn
        .resize(WireWinsize {
            cols: 97,
            rows: 41,
            width_in_pixels: 0,
            height_in_pixels: 0,
        })
        .expect("failed to resize a remote pane");

    let seen = pane.wait_for_output("41 97");
    assert!(
        seen.contains("41 97"),
        "the far pty never reported the new size: {seen}"
    );

    pane.close();
}

/// A window resized while the link was down has to take effect when it comes
/// back. The send fails on a broken connection and the outbound queues are
/// cleared behind it, and nothing else will ever mention the size again — it
/// only goes out when it changes — so the far pty would otherwise stay the shape
/// it was before the outage, and the application keep drawing to it.
#[test]
#[ignore]
fn a_resize_during_an_outage_reaches_the_pty_after_the_reconnect() {
    let hosts = start();
    let marker = marker("resize-outage");
    let pane = open_pane(
        &hosts,
        &format!("trap 'stty size' 28; echo {marker}-ready; while :; do sleep 0.2; done"),
    );
    pane.wait_for_output(&format!("{marker}-ready"));

    pane.conn
        .resize(WireWinsize {
            cols: 80,
            rows: 24,
            width_in_pixels: 0,
            height_in_pixels: 0,
        })
        .expect("failed to resize a remote pane");
    pane.wait_for_output("24 80");

    with_link_down(|| {
        std::thread::sleep(Duration::from_millis(500));
        // Deliberately ignored: over a dead link this cannot land, and that it
        // is remembered anyway is the whole point.
        let _ = pane.conn.resize(WireWinsize {
            cols: 120,
            rows: 40,
            width_in_pixels: 0,
            height_in_pixels: 0,
        });
    });

    let seen = pane.wait_for_output_within("40 120", Duration::from_secs(30));
    assert!(
        seen.contains("40 120"),
        "the size set during the outage never reached the far pty, so the remote \
         application is still drawing at the old one: {seen}"
    );

    pane.close();
}

/// A pid means nothing off its own machine, so signals travel symbolically and
/// the broker resolves them against the child it actually owns.
#[test]
#[ignore]
fn a_signal_is_delivered_to_the_remote_child() {
    let hosts = start();
    let marker = marker("signal");
    let pane = open_pane(
        &hosts,
        &format!(
            "trap 'echo {marker}-got-int' INT; echo {marker}-ready; while :; do sleep 0.2; done"
        ),
    );
    pane.wait_for_output(&format!("{marker}-ready"));

    pane.conn
        .signal_child(ChildSignal::Int)
        .expect("failed to signal a remote child");

    let seen = pane.wait_for_output(&format!("{marker}-got-int"));
    assert!(
        seen.contains(&format!("{marker}-got-int")),
        "the remote child never saw the signal: {seen}"
    );

    pane.close();
}

/// Closing a pane has to reach across, or every closed remote pane leaves a
/// process and a supervisor running on the far machine forever.
#[test]
#[ignore]
fn terminating_a_pane_kills_the_remote_child() {
    let hosts = start();
    let marker = marker("terminate");
    let pane = open_pane(&hosts, &idle_marked(&marker));
    pane.wait_for_output(&format!("{marker}-started"));
    assert_eq!(remote_pids(&marker).len(), 1);

    pane.close();

    wait_for_remote("the remote child to be killed", || {
        remote_pids(&marker).is_empty()
    });
}

/// `Ok(0)` from the reader means the child exited, and may mean nothing else.
/// This is the only path allowed to produce it.
#[test]
#[ignore]
fn a_remote_child_exiting_is_reported_as_a_child_exit() {
    let hosts = start();
    let marker = marker("exit");
    let pane = open_pane(&hosts, &format!("echo {marker}-bye; exit 7"));

    // The child's last words arrive before the EOF, never instead of it.
    pane.wait_for_output(&format!("{marker}-bye"));
    pane.wait_until_ended(SETTLE)
        .expect("a clean child exit should not surface as a transport error");

    assert_eq!(
        pane.conn.child_exit(),
        Some(Some(7)),
        "the exit status did not survive the trip"
    );

    pane.close();
}

/// A child can be gone before the broker has finished registering the pane it
/// belongs to. The exit has to be held and replayed rather than dropped on the
/// floor, or the pane sits on screen looking alive until the orphan timeout.
#[test]
#[ignore]
fn a_child_that_exits_immediately_is_still_reported() {
    let hosts = start();
    // Nothing to print and nothing to wait for: the fastest possible exit, which
    // is the one that races registration.
    let pane = open_pane(&hosts, "exit 3");

    pane.wait_until_ended(SETTLE)
        .expect("a child that exited before registration finished should still end cleanly");
    assert_eq!(
        pane.conn.child_exit(),
        Some(Some(3)),
        "the exit of a child that beat its own registration was lost"
    );
}

/// The exit must never overtake the output. The server stops reading at
/// `ChildExited`, so an exit reported before the pty is drained does not
/// reorder the output — it discards it.
#[test]
#[ignore]
fn everything_a_child_printed_arrives_even_when_it_exits_at_once() {
    let hosts = start();
    let marker = marker("burst");
    // Enough to outrun a single pty read, so the exit has real output to race.
    let pane = open_pane(
        &hosts,
        &format!("i=1; while [ $i -le 200 ]; do echo {marker}-$i; i=$((i+1)); done; exit 0"),
    );

    pane.wait_until_ended(SETTLE)
        .expect("a clean child exit should not surface as a transport error");

    let output = pane.output();
    for i in 1..=200 {
        assert!(
            output.contains(&format!("{marker}-{i}")),
            "line {i} of 200 never arrived — the exit was reported before the pty was drained"
        );
    }
}

// ---------------------------------------------------------------------------
// What only the far machine can see
// ---------------------------------------------------------------------------

/// A pane's cwd and running command come from inspecting the process, which is
/// a thing only the machine running it can do.
#[test]
#[ignore]
fn the_far_machine_reports_the_panes_command_and_cwd() {
    let hosts = start();
    let marker = marker("status");
    let dir = format!("/tmp/{marker}");
    remote_sh(&format!("mkdir -p {dir}"));

    let pane = open_pane_in(&hosts, &idle_marked(&marker), Some(&dir));
    pane.wait_for_output(&format!("{marker}-started"));

    let status = pane.wait_for_status("a cwd", |s| s.cwd.is_some());
    assert_eq!(
        status.cwd.as_deref(),
        Some(dir.as_str()),
        "the reported cwd is not the far machine's"
    );
    let argv = status.shell_argv.expect("no argv was reported");
    assert_eq!(
        argv.first().map(String::as_str),
        Some("/bin/sh"),
        "unexpected argv: {argv:?}"
    );
    assert!(
        argv.iter().any(|a| a.contains(&marker)),
        "the reported argv is not this pane's: {argv:?}"
    );

    pane.close();
    remote_sh(&format!("rm -rf {dir}"));
}

/// A pane opened with no command is what `ssh <host>` gives you: that machine's
/// user's shell, logged in, in that user's home directory.
///
/// The container's test user has `/bin/bash` in its passwd entry precisely so
/// this can tell a real lookup from the `/bin/sh` fallback, and the leading `-`
/// is what makes it a login shell rather than merely an interactive one.
#[test]
#[ignore]
fn a_pane_with_no_command_runs_the_remote_users_login_shell() {
    let hosts = start();
    let pane = open_default_shell_pane(&hosts, None);

    let status = pane.wait_for_status("its own argv", |s| s.shell_argv.is_some());
    let argv = status.shell_argv.expect("checked just above");
    assert_eq!(
        argv.first().map(String::as_str),
        Some("-bash"),
        "the pane should be running the far user's shell as a login shell: {argv:?}"
    );
    assert_eq!(
        status.cwd.as_deref(),
        Some("/home/test"),
        "with no cwd asked for, a pane should land where ssh lands"
    );

    pane.close();
}

/// The point of a *login* shell: the far machine gets to have its own say about
/// the pane on top of the environment composed for it, which is where a prompt
/// and any site-specific PATH come from.
#[test]
#[ignore]
fn a_default_shell_pane_sources_the_remote_profile() {
    let hosts = start();
    let pane = open_default_shell_pane(&hosts, None);
    pane.wait_for_status("a live shell", |s| s.shell_argv.is_some());

    pane.send_line("echo marker=$ZELLIJ_REMOTE_LOGIN_MARKER");

    pane.wait_for_output("marker=profile-was-sourced");

    pane.close();
}

/// An explicit cwd names a directory on the far machine and outranks the home
/// directory the login would otherwise have chosen.
#[test]
#[ignore]
fn an_explicit_cwd_wins_over_the_remote_home_directory() {
    let hosts = start();
    let marker = marker("shell-cwd");
    let dir = format!("/tmp/{marker}");
    remote_sh(&format!("mkdir -p {dir}"));

    let pane = open_default_shell_pane(&hosts, Some(&dir));

    let status = pane.wait_for_status("a cwd", |s| s.cwd.is_some());
    assert_eq!(status.cwd.as_deref(), Some(dir.as_str()));
    assert_eq!(
        status
            .shell_argv
            .as_ref()
            .and_then(|a| a.first())
            .map(String::as_str),
        Some("-bash"),
        "asking for a cwd should not stop it being a login shell"
    );

    pane.close();
    remote_sh(&format!("rm -rf {dir}"));
}

/// Agent detection is string matching over argv, so what a pane is *running*
/// right now has to be told apart from the shell it is running in — on the far
/// machine, where `tcgetpgrp` and `/proc` can answer.
#[test]
#[ignore]
fn a_foreground_process_is_reported_separately_from_the_shell() {
    let hosts = start();
    let marker = marker("foreground");
    // `set -m` turns on job control, which is what puts a foreground job in a
    // process group of its own and hands it the terminal. Without it the child
    // shares the shell's process group, `tcgetpgrp` returns the shell, and the
    // far side correctly reports no foreground command — which is true, but not
    // what this test is about. A real pane's shell is interactive and has job
    // control on already.
    let pane = open_pane(
        &hosts,
        &format!("set -m; echo {marker}-ready; while read line; do $line; done"),
    );
    pane.wait_for_output(&format!("{marker}-ready"));

    // Nothing in the foreground yet: the shell is its own process group leader,
    // and reporting it here would just be the shell again.
    let idle = pane.wait_for_status("its own argv", |s| s.shell_argv.is_some());
    assert_eq!(
        idle.foreground_argv, None,
        "a shell at its prompt should have no foreground command"
    );

    pane.send_line("sleep 300");

    let busy = pane.wait_for_status("a foreground command", |s| s.foreground_argv.is_some());
    let foreground = busy.foreground_argv.expect("checked just above");
    assert!(
        foreground.iter().any(|a| a.contains("sleep")),
        "unexpected foreground command: {foreground:?}"
    );
    assert!(
        busy.shell_argv
            .as_ref()
            .is_some_and(|argv| argv.iter().any(|a| a.contains(&marker))),
        "the shell's own argv should still be reported alongside: {:?}",
        busy.shell_argv
    );

    pane.close();
}

/// Which checkout a pane is in is decided by running git in its folder — and for
/// a remote pane that folder is on another machine.
#[test]
#[ignore]
fn a_git_checkout_on_the_far_machine_is_probed_there() {
    let hosts = start();
    let marker = marker("git");
    let dir = format!("/tmp/{marker}");
    remote_sh(&format!(
        "rm -rf {dir} && mkdir -p {dir} && cd {dir} && git init -q -b main . && \
         git config user.email t@example.com && git config user.name test"
    ));

    let pane = open_pane_in(&hosts, &idle_marked(&marker), Some(&dir));
    pane.wait_for_output(&format!("{marker}-started"));

    let deadline = Instant::now() + SETTLE;
    let probe = loop {
        if let Some(probe) = pane.git_probe() {
            break probe;
        }
        assert!(
            Instant::now() < deadline,
            "the far machine never reported a git probe for {dir}"
        );
        std::thread::sleep(Duration::from_millis(200));
    };

    assert_eq!(probe.cwd, dir, "the probe was run in the wrong directory");
    assert_eq!(
        probe.host, HOST,
        "the probe did not carry the machine it was run on, so two checkouts at \
         the same path on different hosts would merge"
    );
    let metadata = String::from_utf8_lossy(&probe.metadata);
    assert!(
        metadata
            .lines()
            .next()
            .is_some_and(|first| first.starts_with('/')),
        "the probe's first line should be an absolute path on the far machine: {metadata:?}"
    );
    assert!(
        metadata.contains(&dir),
        "the probe describes some other checkout: {metadata:?}"
    );
    // Raw and unparsed on purpose — a repository with no commits writes its
    // identity to stdout and *then* exits 128, so a non-zero code here is not a
    // failure and the exit code cannot gate the parse.
    assert!(
        !probe.metadata.is_empty(),
        "an initialised repository reported nothing"
    );

    pane.close();
    remote_sh(&format!("rm -rf {dir}"));
}

// ---------------------------------------------------------------------------
// Outages
// ---------------------------------------------------------------------------

/// The point of the whole session layer: a connection ending is routine and says
/// nothing about the process.
#[test]
#[ignore]
fn a_pane_survives_a_dropped_connection_and_replays_what_it_missed() {
    let hosts = start();
    let marker = marker("survive");
    // A counter, so a hole in the replay is visible as a hole in the sequence
    // rather than as merely "less output than expected".
    let pane = open_pane(
        &hosts,
        &format!(
            "echo {marker}-ready; i=0; while :; do i=$((i+1)); echo {marker}-$i; sleep 0.1; done"
        ),
    );
    pane.wait_for_output(&format!("{marker}-ready"));
    pane.wait_for_output(&format!("{marker}-5"));

    let pid_before = remote_pids(&marker);
    assert_eq!(pid_before.len(), 1);

    with_link_down(|| std::thread::sleep(Duration::from_secs(2)));

    pane.wait_for_output_within("── reconnected ──", Duration::from_secs(30));
    let seen = pane.output();

    assert!(
        seen.contains("── disconnected from"),
        "the user was never told the link had gone: {seen}"
    );
    assert_eq!(
        pane.conn.child_exit(),
        None,
        "a dropped connection was mistaken for the child exiting"
    );
    assert_eq!(
        remote_pids(&marker),
        pid_before,
        "the pane's process did not survive the outage"
    );

    // Nothing was lost: every counter value from 1 to the highest seen is
    // present, including the ones produced while nobody was listening.
    let highest = highest_counter(&seen, &marker);
    assert!(
        highest > 10,
        "the pane barely produced anything ({highest}); the outage may not have been ridden out"
    );
    for i in 1..=highest {
        assert!(
            seen.contains(&format!("{marker}-{i}\r\n")),
            "output line {i} of {highest} never arrived — the replay has a hole in it"
        );
    }
    assert!(
        !seen.contains("bytes of output were lost"),
        "a two-second outage should fit in the replay buffer: {seen}"
    );

    // And the pane still works afterwards.
    pane.wait_for_output(&format!("{marker}-{}", highest + 5));

    pane.close();
}

/// The same, but against a pane that is talking the whole time.
///
/// Reconnecting has a moment where the broker has worked out what to replay but
/// has not yet handed the pane over. A pane that goes quiet between prints will
/// almost never produce anything in that moment; a busy one will, and those
/// bytes belong to neither the replay nor the newly attached wire unless the
/// two are made to interlock. Several outages, because it is a race.
#[test]
#[ignore]
fn a_busy_pane_loses_nothing_across_a_reconnect() {
    let hosts = start();
    let marker = marker("busy");
    // Fast enough to be mid-sentence when the reconnect lands, slow enough that
    // a short outage still fits in the replay buffer.
    let pane = open_pane(
        &hosts,
        &format!("i=0; while :; do i=$((i+1)); echo {marker}-$i; sleep 0.005; done"),
    );
    pane.wait_for_output(&format!("{marker}-5"));

    for round in 1..=3 {
        let before = highest_counter(&pane.output(), &marker);
        with_link_down(|| std::thread::sleep(Duration::from_millis(500)));
        pane.wait_for_output_within(
            &format!("{marker}-{}", before + 20),
            Duration::from_secs(30),
        );

        let seen = pane.output();
        assert!(
            !seen.contains("bytes of output were lost"),
            "round {round}: a half-second outage should fit in the replay buffer: {seen}"
        );
        let highest = highest_counter(&seen, &marker);
        for i in 1..=highest {
            assert!(
                seen.contains(&format!("{marker}-{i}\r\n")),
                "round {round}: line {i} of {highest} is missing — output produced while the \
                 pane was being handed over went to neither the replay nor the new connection"
            );
        }
    }

    pane.close();
}

/// The replay buffer is finite, and an outage can outlast it. What matters then
/// is that the gap is *admitted* rather than papered over — a pane silently
/// missing a screenful of output is worse than one that says so.
#[test]
#[ignore]
fn an_outage_longer_than_the_ring_says_how_much_was_lost() {
    let hosts = start();
    let marker = marker("lossy");
    // Triggered by a file rather than by pty input, because the burst has to
    // happen *while the link is down* — which is exactly when there is no way to
    // type at the pane. docker can still reach the far machine when ssh cannot,
    // which is the whole reason the fixtures go through it.
    let trigger = format!("/tmp/{marker}.go");
    let pane = open_pane(
        &hosts,
        &format!(
            "echo {marker}-ready; while ! [ -f {trigger} ]; do sleep 0.1; done; \
             yes zellij-remote-pane-filler | head -c 12000000; echo {marker}-done"
        ),
    );
    pane.wait_for_output(&format!("{marker}-ready"));

    // Comfortably past the 4 MiB ring, so the oldest of it cannot survive.
    with_link_down(|| {
        remote_sh(&format!("touch {trigger}"));
        std::thread::sleep(Duration::from_secs(3));
    });

    let seen = pane.wait_for_output_within("bytes of output were lost", Duration::from_secs(60));

    assert!(
        seen.contains("\u{1b}[2J"),
        "a lossy replay must clear the screen — what is on it no longer follows \
         from what came before: {}",
        &seen[seen.len().saturating_sub(200)..]
    );
    let dropped = reported_dropped(&seen);
    assert!(
        dropped > 0,
        "the notice claimed a lossy replay but reported zero bytes lost"
    );
    assert_eq!(
        pane.conn.child_exit(),
        None,
        "an overflowing outage was mistaken for the child exiting"
    );

    pane.close();
    remote_sh(&format!("rm -f {trigger}"));
}

/// A link that never comes back is the one case where the pane really is over,
/// and the error has to say so rather than looking like a child exit.
#[test]
#[ignore]
fn a_reconnect_gives_up_after_its_attempts_are_exhausted() {
    let hosts = start();
    let marker = marker("give-up");
    let pane = open_pane(&hosts, &idle_marked(&marker));
    pane.wait_for_output(&format!("{marker}-started"));

    let error = with_link_down(|| {
        pane.wait_until_ended(Duration::from_secs(60))
            .expect_err("a permanently dead link should not look like a clean exit")
    });

    assert!(
        error.contains("gave up reconnecting"),
        "unexpected error after exhausting the reconnect attempts: {error}"
    );
    assert!(
        error.contains(HOST),
        "the error should name the machine that went away: {error}"
    );
    assert_eq!(
        pane.conn.child_exit(),
        None,
        "giving up on the transport must not be reported as the child exiting"
    );

    // The far side does not know any of this happened; its pane is simply
    // unclaimed. Reclaiming it is the reaper's job, tested below.
    assert_eq!(remote_pids(&marker).len(), 1);
    remote_sh(&format!("pkill -f {} || true", shell_quote(&marker)));
}

/// A server that crashed and never came back would otherwise leave a process and
/// a supervisor running on the far machine forever, with nothing over there
/// aware they had been abandoned.
///
/// Needs `ZELLIJ_REMOTE_ORPHAN_TIMEOUT_SECS`, set for the container in
/// `docker-compose.remote-panes.yml`; the shipped default is half an hour.
#[test]
#[ignore]
fn a_pane_nobody_comes_back_for_is_reclaimed() {
    let hosts = start();
    let marker = marker("orphan");
    let pane = open_pane(&hosts, &idle_marked(&marker));
    pane.wait_for_output(&format!("{marker}-started"));
    assert_eq!(remote_pids(&marker).len(), 1);

    let configured: u64 = remote_sh("echo $ZELLIJ_REMOTE_ORPHAN_TIMEOUT_SECS")
        .parse()
        .expect(
            "the container has no ZELLIJ_REMOTE_ORPHAN_TIMEOUT_SECS, so this test \
             would have to wait half an hour",
        );

    // Stop reading and stop writing, saying nothing. The broker sees a
    // connection that simply ended, which is exactly what a crashed server looks
    // like from over there.
    pane.abandon();
    drop(hosts);

    // Nothing happens for a while, and that is the point: the pane is *kept*
    // until the timeout, because the whole reason it outlives its connection is
    // that the connection might come back.
    std::thread::sleep(Duration::from_secs(configured / 2));
    assert_eq!(
        remote_pids(&marker).len(),
        1,
        "the pane was reclaimed before its timeout had run"
    );

    wait_within(
        "the abandoned pane to be reclaimed",
        Duration::from_secs(configured * 2 + 10),
        || remote_pids(&marker).is_empty(),
    );
}

/// A resume names a pane the broker is supposed to still be holding. If it is
/// not holding it, saying no is the only safe answer: starting a fresh process
/// under a pane the server believes it already has would be worse.
#[test]
#[ignore]
fn a_resume_for_an_unknown_token_is_refused() {
    let env = test_env();
    proxy_set_enabled(true);

    // A forward of our own, so this speaks to the broker directly rather than
    // through a `RemoteConn` that would never send this message.
    let endpoint = ssh::bootstrap(HOST).expect("failed to bootstrap a broker");
    let local = env.sockets.join("direct-resume.sock");
    let _ = std::fs::remove_file(&local);
    let mut forward = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-N", "-T", "-L"])
        .arg(format!("{}:{}", local.display(), endpoint.socket))
        .arg(HOST)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start an ssh forward");
    let _kill = KillOnDrop(&mut forward);

    let deadline = Instant::now() + SETTLE;
    let socket = loop {
        if let Ok(socket) = UnixStream::connect(&local) {
            break socket;
        }
        assert!(
            Instant::now() < deadline,
            "the ssh forward to {HOST} never came up"
        );
        std::thread::sleep(Duration::from_millis(100));
    };

    let mut write = socket.try_clone().expect("failed to clone the socket");
    let mut read = socket.try_clone().expect("failed to clone the socket");

    protocol::write_control(
        &mut write,
        &ServerToBrokerMsg::Hello {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            zellij_version: zellij_utils::consts::VERSION.to_owned(),
        },
    )
    .expect("failed to say hello");
    let ack = read_control(&mut read);
    assert!(
        matches!(ack, BrokerToServerMsg::HelloAck { .. }),
        "expected a hello ack, got {ack:?}"
    );

    protocol::write_control(
        &mut write,
        &ServerToBrokerMsg::Resume {
            pane_token: "a-pane-this-broker-never-had".to_owned(),
            received: 0,
        },
    )
    .expect("failed to send a resume");

    match read_control(&mut read) {
        BrokerToServerMsg::ResumeRefused { reason } => {
            assert!(
                reason.contains("a-pane-this-broker-never-had"),
                "the refusal should name the pane it is about: {reason}"
            );
        },
        other => panic!("expected the resume to be refused, got {other:?}"),
    }

    let _ = std::fs::remove_file(&local);
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

struct KillOnDrop<'a>(&'a mut std::process::Child);

impl Drop for KillOnDrop<'_> {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn read_control(reader: &mut impl Read) -> BrokerToServerMsg {
    let frame = protocol::read_frame(reader)
        .expect("failed to read a frame from the broker")
        .expect("the broker closed the connection instead of answering");
    frame
        .decode_control()
        .expect("the broker sent something that was not a control message")
}

/// The highest `<marker>-<n>` the pane has printed.
fn highest_counter(output: &str, marker: &str) -> u32 {
    let prefix = format!("{marker}-");
    output
        .lines()
        .filter_map(|line| line.trim().strip_prefix(&prefix)?.parse::<u32>().ok())
        .max()
        .unwrap_or(0)
}

/// How many bytes the reconnect notice admitted losing.
fn reported_dropped(output: &str) -> u64 {
    let (_, tail) = output
        .rsplit_once("reconnected; ")
        .expect("no lossy-reconnect notice in the output");
    tail.split_whitespace()
        .next()
        .expect("the notice had no byte count")
        .parse()
        .expect("the notice's byte count was not a number")
}

/// The last `name=value` in some pane output, as a whitespace-delimited field.
///
/// The *last*, because a pty echoes what was typed: `echo tty=$SSH_TTY` puts the
/// unexpanded name in the output ahead of the shell's answer to it.
fn last_field(output: &str, name: &str) -> Option<String> {
    output
        .split_whitespace()
        .filter_map(|word| word.strip_prefix(name))
        .last()
        .map(|value| value.trim().to_owned())
}

// --- the pane's environment ---
//
// A remote pane is not an ssh session: a detached broker owns its pty, and that
// broker's environment is frozen from whichever `ssh ... --pty-broker ensure`
// happened to start it. These pin that nothing is taken from there - the pane's
// terminal comes from the asking side, its identity from this machine's passwd
// entry, and its tty from its own pty.

/// The reported bug: a remote pane had no `TERM` at all, because sshd sends none
/// without a pty request and the bootstrap asks for none.
#[test]
#[ignore]
fn a_remote_pane_uses_the_terminal_type_this_side_is_using() {
    let hosts = start();
    let pane = open_default_shell_pane_with(
        &hosts,
        None,
        vec![("TERM".to_owned(), "xterm-zellij-remote-test".to_owned())],
        0,
    );
    pane.wait_for_status("a live shell", |s| s.shell_argv.is_some());

    pane.send_line("echo term=$TERM");
    pane.wait_for_output("term=xterm-zellij-remote-test");

    pane.close();
}

/// The point of carrying `TERM` rather than merely setting one: terminfo has to
/// resolve on the far machine, or every curses program in the pane is broken.
#[test]
#[ignore]
fn a_remote_pane_has_a_working_terminfo() {
    let hosts = start();
    let pane = open_default_shell_pane_with(
        &hosts,
        None,
        vec![("TERM".to_owned(), "xterm".to_owned())],
        0,
    );
    pane.wait_for_status("a live shell", |s| s.shell_argv.is_some());

    pane.send_line("tput cols >/dev/null 2>&1 && echo tput=ok || echo tput=broken");
    pane.wait_for_output("tput=ok");

    pane.close();
}

/// The locale describes the person typing, so it crosses. A `PATH` or a `HOME`
/// from the asking machine names directories that are not there, so it does not.
#[test]
#[ignore]
fn the_locale_crosses_but_the_asking_machines_filesystem_does_not() {
    let hosts = start();
    let pane = open_default_shell_pane_with(
        &hosts,
        None,
        vec![
            ("LANG".to_owned(), "en_US.UTF-8".to_owned()),
            ("LC_TIME".to_owned(), "C".to_owned()),
            // Neither of these is on the allowlist, so neither should arrive
            // even though they are handed to the pane-opening call.
            (
                "PATH".to_owned(),
                "/nonsense-from-the-asking-machine".to_owned(),
            ),
            ("HOME".to_owned(), "/nonsense-home".to_owned()),
        ],
        0,
    );
    pane.wait_for_status("a live shell", |s| s.shell_argv.is_some());

    pane.send_line("echo lang=$LANG lc_time=$LC_TIME");
    pane.wait_for_output("lang=en_US.UTF-8 lc_time=C");

    pane.send_line("case \"$PATH:$HOME\" in *nonsense*) echo leak=yes;; *) echo leak=no;; esac");
    pane.wait_for_output("leak=no");

    pane.close();
}

/// `SSH_TTY` is the one `SSH_*` a pane can answer honestly, and the supervisor is
/// the only thing that can: the path exists only after its `openpty`.
#[test]
#[ignore]
fn a_remote_pane_knows_its_own_tty() {
    let hosts = start();
    let pane = open_default_shell_pane(&hosts, None);
    pane.wait_for_status("a live shell", |s| s.shell_argv.is_some());

    pane.send_line("echo ssh_tty=$SSH_TTY tty=$(tty)");
    // The pty echoes the command back, so the literal `$SSH_TTY` appears in the
    // output before its expansion does. Waiting on the expansion is what
    // distinguishes them - and `/dev/` cannot appear in the echo.
    let output = pane.wait_for_output("ssh_tty=/dev/");

    let ssh_tty = last_field(&output, "ssh_tty=").expect("an SSH_TTY");
    assert!(
        ssh_tty.starts_with("/dev/"),
        "SSH_TTY should name the pane's own pty, got {ssh_tty:?}"
    );
    assert_eq!(
        last_field(&output, "tty="),
        Some(ssh_tty),
        "SSH_TTY must be the pty the pane is actually on: {output:?}"
    );

    pane.close();
}

/// The frozen-broker regression, stated at the pane level: two panes on one
/// machine share a broker, so anything inherited from it would be identical.
#[test]
#[ignore]
fn two_panes_on_one_host_get_two_different_ttys() {
    let hosts = start();
    let read_tty = |pane: &Pane| {
        pane.wait_for_status("a live shell", |s| s.shell_argv.is_some());
        pane.send_line("echo ssh_tty=$SSH_TTY");
        let output = pane.wait_for_output("ssh_tty=/dev/");
        last_field(&output, "ssh_tty=").expect("an SSH_TTY")
    };

    let first = open_default_shell_pane(&hosts, None);
    let second = open_default_shell_pane(&hosts, None);
    let first_tty = read_tty(&first);
    let second_tty = read_tty(&second);

    assert_ne!(
        first_tty, second_tty,
        "each pane must be told its own pty, not one inherited from the broker"
    );

    first.close();
    second.close();
}

/// Two servers reach the *same* broker on this host, and a broker that handed out
/// its own environment would give both panes whichever `TERM` arrived first.
#[test]
#[ignore]
fn each_pane_gets_its_own_terminal_type_from_the_same_broker() {
    let hosts = start();
    let first = open_default_shell_pane_with(
        &hosts,
        None,
        vec![("TERM".to_owned(), "xterm-first".to_owned())],
        0,
    );
    let second = open_default_shell_pane_with(
        &hosts,
        None,
        vec![("TERM".to_owned(), "xterm-second".to_owned())],
        0,
    );

    first.wait_for_status("a live shell", |s| s.shell_argv.is_some());
    second.wait_for_status("a live shell", |s| s.shell_argv.is_some());
    first.send_line("echo term=$TERM");
    first.wait_for_output("term=xterm-first");
    second.send_line("echo term=$TERM");
    second.wait_for_output("term=xterm-second");

    first.close();
    second.close();
}

/// Who the pane is, straight from this machine's passwd entry. Asserted on an
/// explicit-command pane so no profile can have set any of it.
#[test]
#[ignore]
fn a_pane_is_told_who_it_is_on_the_far_machine() {
    let hosts = start();
    let pane = open_pane(
        &hosts,
        &format!(
            "echo who=$USER/$LOGNAME/$HOME/$SHELL mail=$MAIL; {}",
            idle_marked("who")
        ),
    );

    pane.wait_for_output("who=test/test//home/test//bin/bash");
    pane.wait_for_output("mail=/var/mail/test");

    pane.close();
}

/// An explicit-command pane gets no login shell and so no profile, which is
/// exactly why it needs the sshd default `PATH` rather than the broker's frozen
/// one. `ssh host 'echo $PATH'` prints the same thing.
#[test]
#[ignore]
fn an_explicit_command_pane_gets_the_sshd_default_path() {
    let hosts = start();
    let pane = open_pane(&hosts, &format!("echo path=$PATH; {}", idle_marked("path")));

    pane.wait_for_output("path=/usr/local/bin:/usr/bin:/bin");

    pane.close();
}

/// The profile still owns `PATH`: this composes the *starting* environment ssh
/// would have given, and a login shell runs after it and may override anything.
/// The pair is the whole precedence rule, seen from outside.
#[test]
#[ignore]
fn the_profile_still_owns_the_path_for_a_login_shell() {
    let hosts = start();

    let login = open_default_shell_pane(&hosts, None);
    login.wait_for_status("a live shell", |s| s.shell_argv.is_some());
    login.send_line(
        "case \"$PATH\" in */opt/profile-bin*) echo profile=yes;; *) echo profile=no;; esac",
    );
    login.wait_for_output("profile=yes");
    login.close();

    let explicit = open_pane(
        &hosts,
        &format!(
            "case \"$PATH\" in */opt/profile-bin*) echo profile=yes;; *) echo profile=no;; esac; {}",
            idle_marked("path")
        ),
    );
    explicit.wait_for_output("profile=no");
    explicit.close();
}

/// Nothing about the asking machine's session, and nothing about the bootstrap's
/// connection, reaches a pane. See `remote::env::SCRUBBED_VARS` for why
/// `SSH_CLIENT`/`SSH_CONNECTION` are absent rather than approximated.
#[test]
#[ignore]
fn no_pane_inherits_a_stale_ssh_connection_or_the_asking_sessions_identity() {
    let hosts = start();
    let pane = open_pane(
        &hosts,
        &format!(
            "echo c=[$SSH_CLIENT] n=[$SSH_CONNECTION] a=[$SSH_AUTH_SOCK] z=[$ZELLIJ] s=[$ZELLIJ_SESSION_NAME]; {}",
            idle_marked("scrub")
        ),
    );

    pane.wait_for_output("c=[] n=[] a=[] z=[] s=[]");

    pane.close();
}

/// `ZELLIJ_PANE_ID` used to be `0` for every remote pane, because the broker sent
/// a placeholder id it never replaced.
#[test]
#[ignore]
fn a_remote_pane_is_told_its_real_pane_id() {
    let hosts = start();
    let pane = open_pane_with(
        &hosts,
        &format!("echo pane=$ZELLIJ_PANE_ID; {}", idle_marked("pane-id")),
        None,
        vec![],
        73,
    );

    pane.wait_for_output("pane=73");

    pane.close();
}
