use super::*;

use crate::pty_host::fdpass::recv_fd;
use nix::sys::termios;
use std::os::fd::{FromRawFd, IntoRawFd};
use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::Instant;

const TEST_TIMEOUT: Duration = Duration::from_secs(10);

struct TestHost {
    control: UnixStream,
    handle: Option<JoinHandle<Result<()>>>,
}

impl TestHost {
    /// Boots a supervisor in a thread over a socketpair and completes the
    /// handshake, returning once `HelloAck` is in hand.
    fn start() -> (Self, String, Option<PathBuf>) {
        let (server_end, host_end) = UnixStream::pair().expect("socketpair failed");
        let handle = std::thread::spawn(move || {
            serve(
                host_end,
                ServeConfig {
                    label: "test/0".to_owned(),
                    // Never in tests: signal_hook registration is process-wide and
                    // would hijack the test harness's SIGINT.
                    handle_signals: false,
                },
            )
        });

        let mut host = TestHost {
            control: server_end,
            handle: Some(handle),
        };
        host.send(&ServerToHostMsg::Hello {
            protocol_version: PROTOCOL_VERSION,
            zellij_version: VERSION.to_owned(),
        });
        match host.recv() {
            HostToServerMsg::HelloAck {
                host_id,
                control_socket,
                ..
            } => (host, host_id, control_socket),
            other => panic!("expected HelloAck, got {other:?}"),
        }
    }

    fn send(&mut self, msg: &ServerToHostMsg) {
        write_msg(&mut self.control, msg).expect("failed to send to the host");
    }

    fn recv(&mut self) -> HostToServerMsg {
        self.control
            .set_read_timeout(Some(TEST_TIMEOUT))
            .expect("set_read_timeout failed");
        read_msg::<HostToServerMsg, _>(&mut self.control)
            .expect("failed to read from the host")
            .expect("host closed the control channel unexpectedly")
    }

    /// Sends `Spawn` and completes the `Spawned` + fd-handoff exchange.
    fn spawn(&mut self, cmd: RunCommand) -> (u32, std::fs::File) {
        self.spawn_with(
            cmd,
            None,
            Some(WireWinsize {
                cols: 80,
                rows: 24,
                width_in_pixels: 0,
                height_in_pixels: 0,
            }),
        )
    }

    /// As `spawn`, but lets a test pin down the terminal settings and the initial
    /// window size the supervisor is asked to apply.
    fn spawn_with(
        &mut self,
        cmd: RunCommand,
        termios: Option<WireTermios>,
        initial_size: Option<WireWinsize>,
    ) -> (u32, std::fs::File) {
        self.send(&ServerToHostMsg::Spawn {
            terminal_id: 0,
            session_name: Some("pty-host-tests".to_owned()),
            cmd,
            failover_cmd: None,
            termios,
            initial_size,
        });
        let child_pid = match self.recv() {
            HostToServerMsg::Spawned { child_pid } => child_pid,
            other => panic!("expected Spawned, got {other:?}"),
        };
        let master = recv_fd(&self.control).expect("fd handoff failed");
        let master = unsafe { std::fs::File::from_raw_fd(master.into_raw_fd()) };
        (child_pid, master)
    }

    fn join(&mut self) {
        let handle = self.handle.take().expect("already joined");
        let start = Instant::now();
        while !handle.is_finished() {
            if start.elapsed() > TEST_TIMEOUT {
                panic!("supervisor did not exit within {TEST_TIMEOUT:?}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        handle.join().expect("host panicked").expect("host errored");
    }
}

fn sh(script: &str) -> RunCommand {
    RunCommand {
        command: PathBuf::from("/bin/sh"),
        args: vec!["-c".to_owned(), script.to_owned()],
        cwd: None,
        hold_on_close: false,
        hold_on_start: false,
        originating_plugin: None,
        use_terminal_title: false,
    }
}

fn process_alive(pid: u32) -> bool {
    kill(Pid::from_raw(pid as i32), None).is_ok()
}

fn wait_until(mut condition: impl FnMut() -> bool, what: &str) {
    let start = Instant::now();
    while !condition() {
        if start.elapsed() > TEST_TIMEOUT {
            panic!("timed out waiting for {what}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Read until `needle` shows up, so the test does not depend on how the pty
/// chunks its output.
fn read_until(master: &mut std::fs::File, needle: &str) -> String {
    let start = Instant::now();
    let mut seen = String::new();
    let mut buf = [0u8; 1024];
    while !seen.contains(needle) {
        if start.elapsed() > TEST_TIMEOUT {
            panic!("timed out waiting for {needle:?} in pty output; saw {seen:?}");
        }
        match master.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => seen.push_str(&String::from_utf8_lossy(&buf[..n])),
            // The pty reports the slave closing as EIO rather than EOF on Linux.
            Err(e) if e.raw_os_error() == Some(libc::EIO) => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => panic!("read from pty master failed: {e}"),
        }
    }
    seen
}

#[test]
fn the_handed_off_master_carries_the_childs_output() {
    let (mut host, _host_id, _sock) = TestHost::start();
    let (child_pid, mut master) = host.spawn(sh("echo zellij-pty-host-marker"));
    assert!(child_pid > 0);

    let output = read_until(&mut master, "zellij-pty-host-marker");
    assert!(
        output.contains("zellij-pty-host-marker"),
        "did not see the child's output: {output:?}"
    );

    match host.recv() {
        HostToServerMsg::ChildExited { status } => assert_eq!(status, Some(0)),
        other => panic!("expected ChildExited, got {other:?}"),
    }
    host.send(&ServerToHostMsg::Terminate);
    host.join();
}

#[test]
fn exit_status_is_reported() {
    let (mut host, _host_id, _sock) = TestHost::start();
    let (_pid, _master) = host.spawn(sh("exit 7"));
    match host.recv() {
        HostToServerMsg::ChildExited { status } => assert_eq!(status, Some(7)),
        other => panic!("expected ChildExited, got {other:?}"),
    }
    host.send(&ServerToHostMsg::Terminate);
    host.join();
}

/// The single most important regression to guard: a pane closes because reading
/// the master fails once the last slave is gone. If the supervisor held the slave
/// open past the child's exit, every command pane would hang open forever.
#[test]
fn the_master_stops_reading_once_the_child_is_gone() {
    let (mut host, _host_id, _sock) = TestHost::start();
    let (_pid, mut master) = host.spawn(sh("echo done"));
    let _ = read_until(&mut master, "done");
    match host.recv() {
        HostToServerMsg::ChildExited { .. } => {},
        other => panic!("expected ChildExited, got {other:?}"),
    }

    // Drain to the end; this must terminate rather than block forever.
    let start = Instant::now();
    let mut buf = [0u8; 1024];
    loop {
        if start.elapsed() > TEST_TIMEOUT {
            panic!("master never reported EOF after the child exited");
        }
        match master.read(&mut buf) {
            Ok(0) => break,
            Ok(_) => continue,
            Err(e) if e.raw_os_error() == Some(libc::EIO) => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => panic!("unexpected error reading the master: {e}"),
        }
    }

    host.send(&ServerToHostMsg::Terminate);
    host.join();
}

#[test]
fn terminate_kills_a_running_child() {
    let (mut host, _host_id, _sock) = TestHost::start();
    let (child_pid, _master) = host.spawn(sh("sleep 60"));
    assert!(process_alive(child_pid));

    host.send(&ServerToHostMsg::Terminate);
    host.join();

    wait_until(
        || !process_alive(child_pid),
        "the child to die after Terminate",
    );
}

/// `run_pty_host` ignores SIGHUP and SIGPIPE so the supervisor outlives the
/// terminal that spawned the server, and an ignored disposition survives `exec`.
/// If the child inherits it, POSIX then requires the pane's shell to keep it
/// ignored for everything it runs — and closing a pane sends SIGHUP
/// (`kill_current_child` here, `ServerOsApi::kill` on the server side), so the
/// pane would only die via the `TERMINATE_GRACE` `SIGKILL`, leaving descendants
/// orphaned rather than hung up.
///
/// The tests drive `serve()` in-process and so never run the `libc::signal`
/// calls in `run_pty_host`; reproducing them here is the only way to observe
/// what a real supervisor's child would inherit.
#[cfg(target_os = "linux")]
#[test]
fn pane_children_do_not_inherit_the_supervisors_ignored_signals() {
    /// Dispositions are process-wide, and the rest of the suite runs in parallel
    /// in this same process, so put it back even if an assertion panics. Nothing
    /// else in the suite sends or waits on SIGHUP.
    struct IgnoreHup(libc::sighandler_t);
    impl IgnoreHup {
        fn set() -> Self {
            IgnoreHup(unsafe { libc::signal(libc::SIGHUP, libc::SIG_IGN) })
        }
    }
    impl Drop for IgnoreHup {
        fn drop(&mut self) {
            unsafe { libc::signal(libc::SIGHUP, self.0) };
        }
    }

    fn ignored_signals(pid: u32) -> u64 {
        let status =
            std::fs::read_to_string(format!("/proc/{pid}/status")).expect("read child status");
        let field = status
            .lines()
            .find_map(|line| line.strip_prefix("SigIgn:"))
            .expect("no SigIgn in child status");
        u64::from_str_radix(field.trim(), 16).expect("malformed SigIgn")
    }

    let _ignored_hup = IgnoreHup::set();

    let (mut host, _host_id, _sock) = TestHost::start();
    // Two commands, so `sh` does not exec-optimize itself away and stay the pid
    // we were handed; the marker then proves it has already exec'd, which is what
    // makes reading its dispositions meaningful rather than a fork/exec race.
    let (child_pid, mut master) = host.spawn(sh("echo ready; sleep 60"));
    read_until(&mut master, "ready");

    let ignored = ignored_signals(child_pid);
    // SIGPIPE is worth asserting too: every Rust binary — this test included —
    // ignores it process-wide at startup.
    for (signal, name) in [(libc::SIGHUP, "SIGHUP"), (libc::SIGPIPE, "SIGPIPE")] {
        assert_eq!(
            ignored & (1 << (signal - 1)),
            0,
            "pane child inherited {name} as ignored (SigIgn {ignored:#x})"
        );
    }

    host.send(&ServerToHostMsg::Terminate);
    host.join();
}

/// Dropping the control channel is a *detach*, not a shutdown — this is the whole
/// point of the feature. The child must survive and keep making progress.
#[test]
fn losing_the_server_detaches_rather_than_killing_the_child() {
    let (server_end, host_end) = UnixStream::pair().expect("socketpair failed");
    let handle = std::thread::spawn(move || {
        serve(
            host_end,
            ServeConfig {
                label: "test/detach".to_owned(),
                handle_signals: false,
            },
        )
    });
    let mut host = TestHost {
        control: server_end,
        handle: Some(handle),
    };
    host.send(&ServerToHostMsg::Hello {
        protocol_version: PROTOCOL_VERSION,
        zellij_version: VERSION.to_owned(),
    });
    let (host_id, control_socket) = match host.recv() {
        HostToServerMsg::HelloAck {
            host_id,
            control_socket,
            ..
        } => (host_id, control_socket),
        other => panic!("expected HelloAck, got {other:?}"),
    };
    let control_socket = control_socket.expect("the supervisor should have bound a socket");

    // A child that keeps producing output, so the detached drain has work to do.
    let (child_pid, master) = host.spawn(sh(
        "i=0; while [ $i -lt 1000 ]; do echo detached-line-$i; i=$((i+1)); sleep 0.01; done",
    ));

    // Simulate the server dying: drop both the control channel and our dup of the
    // master, exactly as process teardown would.
    let TestHost { control, handle } = host;
    drop(control);
    drop(master);

    wait_until(
        || {
            std::os::unix::net::UnixStream::connect(&control_socket)
                .map(|mut cli| {
                    write_msg(&mut cli, &ServerToHostMsg::Query).is_ok()
                        && matches!(
                            read_msg::<HostToServerMsg, _>(&mut cli),
                            Ok(Some(HostToServerMsg::Status(status))) if !status.attached
                        )
                })
                .unwrap_or(false)
        },
        "the supervisor to report itself detached",
    );

    assert!(
        process_alive(child_pid),
        "the child must outlive the server that started it"
    );

    // ...and the supervisor must be draining, or the child would wedge on a full
    // pty buffer once it produced more than the kernel will hold.
    wait_until(
        || {
            let Ok(mut cli) = std::os::unix::net::UnixStream::connect(&control_socket) else {
                return false;
            };
            if write_msg(&mut cli, &ServerToHostMsg::Query).is_err() {
                return false;
            }
            matches!(
                read_msg::<HostToServerMsg, _>(&mut cli),
                Ok(Some(HostToServerMsg::Status(status))) if status.buffered_bytes > 0
            )
        },
        "the detached supervisor to buffer output",
    );

    // Clean up over the named socket, which is the only channel left.
    let mut cli = std::os::unix::net::UnixStream::connect(&control_socket).expect("connect failed");
    write_msg(&mut cli, &ServerToHostMsg::Terminate).expect("Terminate failed");
    let _: Option<HostToServerMsg> = read_msg(&mut cli).unwrap_or(None);

    let start = Instant::now();
    while !handle.as_ref().expect("handle").is_finished() {
        if start.elapsed() > TEST_TIMEOUT {
            panic!("supervisor did not exit after Terminate over its socket");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    handle
        .expect("handle")
        .join()
        .expect("host panicked")
        .expect("host errored");

    wait_until(
        || !process_alive(child_pid),
        "the detached child to be killed",
    );
    assert!(
        !protocol::socket_path(&host_id).exists(),
        "the supervisor should unlink its socket on the way out"
    );
}

/// A detached supervisor whose child has already exited is watching a pty that
/// is permanently hung up, so `poll` returns immediately every time. If it keeps
/// watching it, an orphaned pane burns a core for as long as it lives — and it
/// lives forever by design. A spinning loop still answers `Query`, so nothing
/// short of measuring CPU catches this.
#[cfg(target_os = "linux")]
#[test]
fn a_detached_supervisor_does_not_spin_after_its_child_exits() {
    // Per-*thread* CPU, not per-process: the rest of the suite runs in parallel
    // in this same process, so process-wide numbers would be pure noise.
    fn thread_cpu_ticks(tid: i32) -> u64 {
        let stat = std::fs::read_to_string(format!("/proc/self/task/{tid}/stat"))
            .expect("read thread stat");
        // utime/stime are the 14th and 15th fields, but the comm field can contain
        // spaces and parens, so start counting after the last ')'.
        let after_comm = stat.rsplit_once(')').expect("malformed stat").1;
        let fields: Vec<&str> = after_comm.split_whitespace().collect();
        let utime: u64 = fields[11].parse().unwrap_or(0);
        let stime: u64 = fields[12].parse().unwrap_or(0);
        utime + stime
    }

    let (server_end, host_end) = UnixStream::pair().expect("socketpair failed");
    let (tid_tx, tid_rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let _ = tid_tx.send(unsafe { libc::gettid() });
        serve(
            host_end,
            ServeConfig {
                label: "test/spin".to_owned(),
                handle_signals: false,
            },
        )
    });
    let serve_tid = tid_rx
        .recv_timeout(TEST_TIMEOUT)
        .expect("supervisor thread never started");
    let mut host = TestHost {
        control: server_end,
        handle: Some(handle),
    };
    host.send(&ServerToHostMsg::Hello {
        protocol_version: PROTOCOL_VERSION,
        zellij_version: VERSION.to_owned(),
    });
    let control_socket = match host.recv() {
        HostToServerMsg::HelloAck { control_socket, .. } => {
            control_socket.expect("no socket bound")
        },
        other => panic!("expected HelloAck, got {other:?}"),
    };

    let (_child_pid, master) = host.spawn(sh("echo done"));
    match host.recv() {
        HostToServerMsg::ChildExited { .. } => {},
        other => panic!("expected ChildExited, got {other:?}"),
    }

    // Detach with the child already dead: the master is hung up from the moment
    // the supervisor takes over reading it.
    let TestHost { control, handle } = host;
    drop(control);
    drop(master);
    std::thread::sleep(Duration::from_millis(300));

    let before = thread_cpu_ticks(serve_tid);
    std::thread::sleep(Duration::from_millis(700));
    let spent = thread_cpu_ticks(serve_tid).saturating_sub(before);
    // 700ms of spinning is ~70 ticks at the usual 100Hz; blocking in poll is ~0.
    assert!(
        spent < 20,
        "detached supervisor burned {spent} CPU ticks while idle — it is spinning on a hung-up pty"
    );

    let mut cli = std::os::unix::net::UnixStream::connect(&control_socket).expect("connect");
    write_msg(&mut cli, &ServerToHostMsg::Terminate).expect("Terminate");
    let _: Option<HostToServerMsg> = read_msg(&mut cli).unwrap_or(None);
    let start = Instant::now();
    while !handle.as_ref().expect("handle").is_finished() {
        if start.elapsed() > TEST_TIMEOUT {
            panic!("supervisor did not exit");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    handle
        .expect("handle")
        .join()
        .expect("panicked")
        .expect("errored");
}

#[test]
fn a_missing_command_is_reported_rather_than_panicking() {
    let (mut host, _host_id, _sock) = TestHost::start();
    host.send(&ServerToHostMsg::Spawn {
        terminal_id: 0,
        session_name: None,
        cmd: RunCommand {
            command: PathBuf::from("/nonexistent/zellij-pty-host-no-such-binary"),
            args: vec![],
            cwd: None,
            hold_on_close: false,
            hold_on_start: false,
            originating_plugin: None,
            use_terminal_title: false,
        },
        failover_cmd: None,
        termios: None,
        initial_size: None,
    });
    match host.recv() {
        HostToServerMsg::SpawnFailed { kind, .. } => {
            assert_eq!(kind, SpawnFailureKind::CommandNotFound)
        },
        other => panic!("expected SpawnFailed, got {other:?}"),
    }
}

/// A supervisor may outlive the zellij that spawned it, so a later server can
/// meet an older host. It must refuse cleanly, leaving the server free to fall
/// back to spawning the pty in-process.
#[test]
fn a_protocol_mismatch_is_refused_cleanly() {
    let (server_end, host_end) = UnixStream::pair().expect("socketpair failed");
    let handle = std::thread::spawn(move || {
        serve(
            host_end,
            ServeConfig {
                label: "test/skew".to_owned(),
                handle_signals: false,
            },
        )
    });
    let mut host = TestHost {
        control: server_end,
        handle: Some(handle),
    };
    host.send(&ServerToHostMsg::Hello {
        protocol_version: PROTOCOL_VERSION + 1,
        zellij_version: "99.0.0".to_owned(),
    });
    let control_socket = match host.recv() {
        HostToServerMsg::HelloAck {
            protocol_version,
            control_socket,
            ..
        } => {
            assert_eq!(protocol_version, PROTOCOL_VERSION);
            control_socket
        },
        other => panic!("expected HelloAck, got {other:?}"),
    };
    host.join();

    // The supervisor binds its named socket before it checks the version, so
    // refusing has to unlink it again — otherwise every version skew would leave
    // a socket behind that `pty-host list` then has to garbage-collect.
    if let Some(path) = control_socket {
        assert!(
            !path.exists(),
            "a refused supervisor left {} behind",
            path.display()
        );
    }
}

/// The supervisor is the only thing that sizes a pane's pty before the server's
/// first `TIOCSWINSZ`. Every test here has always passed an `initial_size` and
/// none of them has ever checked that it arrived.
#[test]
fn the_requested_window_size_is_applied_to_the_new_pty() {
    let (mut host, _host_id, _sock) = TestHost::start();
    let requested = WireWinsize {
        cols: 120,
        rows: 40,
        width_in_pixels: 960,
        height_in_pixels: 640,
    };
    let (_child_pid, master) = host.spawn_with(sh("sleep 60"), None, Some(requested));

    let mut actual: Winsize = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCGWINSZ.into(), &mut actual) };
    assert_eq!(
        rc,
        0,
        "TIOCGWINSZ on the handed-off master failed: {}",
        std::io::Error::last_os_error()
    );
    assert_eq!(
        (actual.ws_col, actual.ws_row),
        (requested.cols, requested.rows),
        "the pty was sized {}x{} instead of {}x{}",
        actual.ws_col,
        actual.ws_row,
        requested.cols,
        requested.rows
    );
    assert_eq!(
        (actual.ws_xpixel, actual.ws_ypixel),
        (requested.width_in_pixels, requested.height_in_pixels),
        "the pixel dimensions did not survive"
    );

    host.send(&ServerToHostMsg::Terminate);
    host.join();
}

/// `orig_termios` is sampled from the user's real terminal before the server
/// daemonizes and has to travel over the wire because a supervisor has no
/// controlling terminal to re-derive it from. Every other test passes
/// `termios: None`, so the applying half has never run under test.
#[test]
fn the_servers_terminal_settings_are_applied_to_the_new_pty() {
    let reference = openpty(None, &None).expect("openpty failed");
    let mut desired = termios::tcgetattr(reference.slave.as_fd()).expect("tcgetattr failed");
    termios::cfmakeraw(&mut desired);
    desired.control_chars[termios::SpecialCharacterIndices::VMIN as usize] = 5;

    let (mut host, _host_id, _sock) = TestHost::start();
    let (_child_pid, master) = host.spawn_with(
        sh("sleep 60"),
        Some(WireTermios::from_termios(&desired)),
        None,
    );

    let applied = termios::tcgetattr(master.as_fd()).expect("tcgetattr on the master failed");
    assert!(
        !applied
            .local_flags
            .contains(termios::LocalFlags::ICANON | termios::LocalFlags::ECHO),
        "the pty is still in canonical/echoing mode, so the raw termios never landed"
    );
    assert_eq!(
        applied.control_chars[termios::SpecialCharacterIndices::VMIN as usize],
        5,
        "VMIN did not survive the trip to the supervisor"
    );

    host.send(&ServerToHostMsg::Terminate);
    host.join();
}

/// `Terminate` sends SIGHUP and escalates to SIGKILL once `TERMINATE_GRACE` is
/// up. Nothing exercised the escalation, so a pane running something that
/// declines to die politely could have wedged its supervisor indefinitely.
#[test]
fn a_child_that_ignores_sighup_is_killed_after_the_grace_period() {
    let (mut host, _host_id, _sock) = TestHost::start();
    // More than one command, so `sh` cannot exec-optimise itself away and take the
    // trap with it — and `read`, a builtin, rather than `sleep`, so the shell
    // blocks without forking a grandchild that would survive the SIGKILL aimed at
    // its parent and outlive the test run.
    let (child_pid, mut master) = host.spawn(sh("trap '' HUP; echo ready; read ignored"));
    read_until(&mut master, "ready");

    let started = Instant::now();
    host.send(&ServerToHostMsg::Terminate);
    host.join();
    let elapsed = started.elapsed();

    assert!(
        !process_alive(child_pid),
        "the child survived Terminate; SIGKILL never landed"
    );
    assert!(
        elapsed >= TERMINATE_GRACE,
        "Terminate returned after {elapsed:?}, before the {TERMINATE_GRACE:?} grace period \
         could expire — the child cannot have been killed by the escalation"
    );
}

/// A wedged or hostile `pty-host list` must not be able to take a pane down with
/// it. These are the three arms of `serve_cli_connection` that the happy-path
/// query test never reaches.
#[test]
fn a_malformed_cli_request_does_not_take_the_supervisor_down() {
    let (mut host, host_id, control_socket) = TestHost::start();
    let Some(socket) = control_socket else {
        // Binding is best-effort; without it there is nothing to exercise.
        return;
    };
    let (_child_pid, _master) = host.spawn(sh("sleep 60"));

    let query_still_works = |what: &str| {
        let mut cli = UnixStream::connect(&socket).expect("failed to connect to the host socket");
        cli.set_read_timeout(Some(TEST_TIMEOUT))
            .expect("set_read_timeout failed");
        write_msg(&mut cli, &ServerToHostMsg::Query).expect("failed to send Query");
        match read_msg::<HostToServerMsg, _>(&mut cli) {
            Ok(Some(HostToServerMsg::Status(status))) => {
                assert_eq!(status.host_id, host_id);
            },
            other => panic!("the supervisor stopped answering after {what}: {other:?}"),
        }
    };

    // A message the CLI channel does not handle at all.
    let mut cli = UnixStream::connect(&socket).expect("connect failed");
    write_msg(
        &mut cli,
        &ServerToHostMsg::Hello {
            protocol_version: PROTOCOL_VERSION,
            zellij_version: VERSION.to_owned(),
        },
    )
    .expect("failed to send Hello");
    drop(cli);
    query_still_works("an unexpected message");

    // Bytes that are not a frame at all.
    let mut cli = UnixStream::connect(&socket).expect("connect failed");
    cli.write_all(&[0xff, 0xff, 0xff, 0xff])
        .expect("failed to write garbage");
    drop(cli);
    query_still_works("a garbage frame");

    // A connection that opens and closes without saying anything.
    drop(UnixStream::connect(&socket).expect("connect failed"));
    query_still_works("an empty connection");

    host.send(&ServerToHostMsg::Terminate);
    host.join();
}

/// The whole `CommandNotFound` chain hangs off this `downcast_ref`, and the error
/// it has to recognise reaches it wrapped in the `with_context` layers that
/// `start_session` adds. anyhow searches the context chain, but nothing pinned
/// that down, and eight sites in `pty.rs` downcast for the result.
#[test]
fn classify_spawn_failure_sees_through_the_context_layers() {
    let bare: anyhow::Error = ZellijError::CommandNotFound {
        terminal_id: 3,
        command: "/nonexistent/binary".to_owned(),
    }
    .into();
    assert_eq!(
        classify_spawn_failure(&bare),
        SpawnFailureKind::CommandNotFound
    );

    let wrapped = bare
        .context("failed to resolve the command")
        .context("failed to start the session");
    assert_eq!(
        classify_spawn_failure(&wrapped),
        SpawnFailureKind::CommandNotFound,
        "context layers hid the ZellijError from the classifier"
    );

    assert_eq!(
        classify_spawn_failure(&anyhow!("openpty failed")),
        SpawnFailureKind::Other
    );
}

// ---- property-based tests (hegel) ----

use hegel::generators;

/// Push sizes cluster around the capacity on purpose: `push` has a fast path for
/// a single write at or over `RING_CAPACITY` and a drain-then-extend path below
/// it, and a uniform draw over a 256 KiB range would essentially never land on
/// the seam between the two.
#[hegel::composite]
fn ring_push_sizes(tc: &hegel::TestCase) -> Vec<usize> {
    let count = tc.draw(generators::integers::<usize>().max_value(6));
    (0..count)
        .map(|_| {
            if tc.draw(generators::booleans()) {
                tc.draw(generators::sampled_from(vec![
                    0,
                    1,
                    RING_CAPACITY - 1,
                    RING_CAPACITY,
                    RING_CAPACITY + 1,
                ]))
            } else {
                tc.draw(generators::integers::<usize>().max_value(RING_CAPACITY / 3))
            }
        })
        .collect()
}

/// Contents are synthesized rather than drawn: these properties are about
/// lengths, ordering and accounting, and generating a quarter-megabyte of random
/// bytes per push would dominate the runtime without testing anything more. A
/// wrapping counter across the whole stream makes any reordering visible.
fn ring_writes(sizes: &[usize]) -> Vec<Vec<u8>> {
    let mut next: u8 = 0;
    sizes
        .iter()
        .map(|&len| {
            (0..len)
                .map(|_| {
                    let byte = next;
                    next = next.wrapping_add(1);
                    byte
                })
                .collect()
        })
        .collect()
}

#[hegel::test]
fn a_ring_buffer_never_holds_more_than_its_capacity(tc: hegel::TestCase) {
    let sizes = tc.draw(ring_push_sizes());
    let mut ring = RingBuffer::new();
    for write in ring_writes(&sizes) {
        ring.push(&write);
        assert!(
            ring.len() <= RING_CAPACITY,
            "the ring grew to {} bytes, over the {RING_CAPACITY} capacity, after pushes {:?}",
            ring.len(),
            sizes
        );
    }
}

/// The one that matters: an orphaned pane's buffer has to be the *tail* of what
/// the child wrote, in order. Compared against a plain `Vec` of everything
/// pushed — an off-by-one in either branch of `push` shows up here and nowhere
/// else.
// 30 rather than the default 100: each case copies several MiB through both the
// ring and the model.
#[hegel::test(test_cases = 30)]
fn a_ring_buffer_keeps_exactly_the_most_recent_bytes(tc: hegel::TestCase) {
    let sizes = tc.draw(ring_push_sizes());

    let mut ring = RingBuffer::new();
    let mut model: Vec<u8> = Vec::new();
    for write in ring_writes(&sizes) {
        ring.push(&write);
        model.extend_from_slice(&write);
    }

    let kept = model.len().min(RING_CAPACITY);
    let expected = &model[model.len() - kept..];
    let actual: Vec<u8> = ring.buf.iter().copied().collect();

    assert_eq!(
        actual.len(),
        expected.len(),
        "the ring kept {} bytes but should have kept {kept} of the {} pushed by {:?}",
        actual.len(),
        model.len(),
        sizes
    );
    if actual != expected {
        // Reported by index rather than by value: these buffers run to hundreds
        // of kilobytes and dumping them would bury the counterexample.
        let diverged = actual
            .iter()
            .zip(expected)
            .position(|(a, b)| a != b)
            .expect("lengths match, so a difference must have an index");
        panic!(
            "the ring's contents diverge from the model at byte {diverged} of {kept} \
             (pushes were {sizes:?})"
        );
    }
}

#[hegel::test(test_cases = 30)]
fn nothing_pushed_into_a_ring_buffer_is_unaccounted_for(tc: hegel::TestCase) {
    let sizes = tc.draw(ring_push_sizes());
    let mut ring = RingBuffer::new();
    let mut pushed: u64 = 0;

    for write in ring_writes(&sizes) {
        ring.push(&write);
        pushed += write.len() as u64;
        assert_eq!(
            ring.len() as u64 + ring.dropped,
            pushed,
            "the ring holds {} and claims to have dropped {}, which does not add up to the \
             {pushed} bytes pushed by {sizes:?}",
            ring.len(),
            ring.dropped
        );
    }
}

#[hegel::test(test_cases = 30)]
fn the_dropped_byte_count_never_decreases(tc: hegel::TestCase) {
    let sizes = tc.draw(ring_push_sizes());
    let mut ring = RingBuffer::new();
    let mut previous = 0;

    for write in ring_writes(&sizes) {
        ring.push(&write);
        assert!(
            ring.dropped >= previous,
            "the dropped count fell from {previous} to {} during pushes {sizes:?}",
            ring.dropped
        );
        previous = ring.dropped;
    }
}

// ---- attach ----

/// Connect to a supervisor's named socket and ask it to hand the pane over,
/// returning the reply and (on success) the pty master it sent with it.
fn attach_over_socket(
    socket: &std::path::Path,
    terminal_id: u32,
    session_name: Option<&str>,
) -> (HostToServerMsg, Option<std::fs::File>) {
    let mut stream = UnixStream::connect(socket).expect("failed to connect to the named socket");
    stream
        .set_read_timeout(Some(TEST_TIMEOUT))
        .expect("set_read_timeout failed");
    write_msg(
        &mut stream,
        &ServerToHostMsg::Attach {
            protocol_version: PROTOCOL_VERSION,
            zellij_version: VERSION.to_owned(),
            terminal_id,
            session_name: session_name.map(str::to_owned),
        },
    )
    .expect("failed to send Attach");
    let reply = read_msg::<HostToServerMsg, _>(&mut stream)
        .expect("failed to read the attach reply")
        .expect("the supervisor closed the connection");
    let master = match &reply {
        HostToServerMsg::AttachAck { .. } => {
            let fd = recv_fd(&stream).expect("failed to receive the pty master");
            Some(unsafe { std::fs::File::from_raw_fd(fd.into_raw_fd()) })
        },
        _ => None,
    };
    (reply, master)
}

fn query_status(socket: &std::path::Path) -> HostStatus {
    let mut stream = UnixStream::connect(socket).expect("failed to connect to the named socket");
    stream
        .set_read_timeout(Some(TEST_TIMEOUT))
        .expect("set_read_timeout failed");
    write_msg(&mut stream, &ServerToHostMsg::Query).expect("failed to send Query");
    match read_msg::<HostToServerMsg, _>(&mut stream)
        .expect("failed to read the query reply")
        .expect("the supervisor closed the connection")
    {
        HostToServerMsg::Status(status) => status,
        other => panic!("expected Status, got {other:?}"),
    }
}

/// `Release` is the deliberate half of what a crashed server does by accident.
/// It must detach without touching the child.
#[test]
fn a_release_detaches_without_killing_the_child() {
    let (mut host, _host_id, control_socket) = TestHost::start();
    let control_socket = control_socket.expect("the supervisor should have bound a socket");
    let (child_pid, master) = host.spawn(sh("sleep 60"));

    assert!(query_status(&control_socket).attached);
    host.send(&ServerToHostMsg::Release);
    wait_until(
        || !query_status(&control_socket).attached,
        "the supervisor to report itself detached",
    );
    assert!(
        process_alive(child_pid),
        "Release must not kill the pane's child"
    );
    // The supervisor dropped the control channel, so the released server has no
    // way to reach it any more — only the named socket is left. That is what
    // makes the hand-off a clean break rather than a shared pane.
    //
    // Asserted by reading rather than writing: a write to a socket whose peer has
    // just closed succeeds once before `EPIPE`, whereas the EOF is unambiguous
    // the moment the FIN lands.
    host.control
        .set_read_timeout(Some(TEST_TIMEOUT))
        .expect("set_read_timeout failed");
    assert!(
        matches!(read_msg::<HostToServerMsg, _>(&mut host.control), Ok(None)),
        "a released supervisor should have closed the old control channel"
    );

    drop(master);
    let _ = crate::pty_host::client::terminate_via_socket(&control_socket);
    host.join();
}

/// The point of the whole feature: a detached supervisor hands its pty to a new
/// server, along with everything the child wrote while nobody was listening.
#[test]
fn an_attach_hands_the_master_and_the_buffered_output_over() {
    let (mut host, host_id, control_socket) = TestHost::start();
    let control_socket = control_socket.expect("the supervisor should have bound a socket");
    // Driven by input rather than a timer: the child says nothing until asked, so
    // "output produced while detached" is a fact the test establishes rather than
    // a race it hopes to win.
    let (child_pid, mut master) = host.spawn(sh("while read line; do echo \"got:$line\"; done"));

    host.send(&ServerToHostMsg::Release);
    wait_until(
        || !query_status(&control_socket).attached,
        "the supervisor to report itself detached",
    );
    master
        .write_all(b"during-the-gap\n")
        .expect("failed to write to the pty while detached");
    drop(master);
    wait_until(
        || query_status(&control_socket).buffered_bytes >= b"got:during-the-gap".len(),
        "the detached supervisor to buffer its child's output",
    );

    let (reply, master) = attach_over_socket(&control_socket, 42, Some("the-other-session"));
    let (buffered, cmd, status) = match reply {
        HostToServerMsg::AttachAck {
            host_id: acked_id,
            child_pid: acked_pid,
            buffered_base64,
            cmd,
            status,
            ..
        } => {
            assert_eq!(acked_id, host_id);
            assert_eq!(acked_pid, child_pid, "the pane kept its process");
            (
                crate::pty_host::protocol::decode_buffered(&buffered_base64).expect("bad base64"),
                cmd,
                status,
            )
        },
        other => panic!("expected AttachAck, got {other:?}"),
    };
    assert!(
        master.is_some(),
        "the attach should have sent the master fd"
    );
    let buffered = String::from_utf8_lossy(&buffered).to_string();
    assert!(
        buffered.contains("during-the-gap"),
        "output produced while detached should arrive with the attach, got {buffered:?}"
    );
    assert!(
        cmd.originating_plugin.is_none(),
        "a plugin id from the previous session must not follow the pane"
    );

    // Re-keyed to the session that owns it now, so `pty-host list` does not lie.
    assert_eq!(status.terminal_id, 42);
    assert_eq!(status.session_name.as_deref(), Some("the-other-session"));
    assert!(status.attached);

    let _ = crate::pty_host::client::terminate_via_socket(&control_socket);
    host.join();
}

/// The ring is handed over, not copied: a second attach must not replay output
/// the first one already took.
#[test]
fn the_buffered_output_is_delivered_once_and_then_cleared() {
    let (mut host, _host_id, control_socket) = TestHost::start();
    let control_socket = control_socket.expect("the supervisor should have bound a socket");
    let (_child_pid, mut master) = host.spawn(sh("while read line; do echo \"got:$line\"; done"));

    host.send(&ServerToHostMsg::Release);
    wait_until(
        || !query_status(&control_socket).attached,
        "the supervisor to report itself detached",
    );
    master
        .write_all(b"only-once\n")
        .expect("failed to write to the pty while detached");
    drop(master);
    wait_until(
        || query_status(&control_socket).buffered_bytes >= b"got:only-once".len(),
        "the detached supervisor to buffer its child's output",
    );

    let (first, first_master) = attach_over_socket(&control_socket, 1, None);
    let first_buffered = match first {
        HostToServerMsg::AttachAck {
            buffered_base64, ..
        } => crate::pty_host::protocol::decode_buffered(&buffered_base64).expect("bad base64"),
        other => panic!("expected AttachAck, got {other:?}"),
    };
    assert!(!first_buffered.is_empty());
    assert_eq!(
        query_status(&control_socket).buffered_bytes,
        0,
        "the ring should be empty once its contents have been handed over"
    );

    // Let go again and take it a second time: nothing new was written, so there
    // should be nothing to replay.
    //
    // `attach_over_socket` drops its control stream on return, which is what
    // detaches the supervisor again — but it has to notice before a second attach
    // will be entertained, and "an attach is refused while attached" is a rule
    // this test is not the one to exercise.
    drop(first_master);
    wait_until(
        || !query_status(&control_socket).attached,
        "the supervisor to notice the first attacher went away",
    );
    let (second, _) = attach_over_socket(&control_socket, 2, None);
    match second {
        HostToServerMsg::AttachAck {
            buffered_base64, ..
        } => {
            let second_buffered =
                crate::pty_host::protocol::decode_buffered(&buffered_base64).expect("bad base64");
            assert!(
                second_buffered.is_empty(),
                "the second attach replayed output the first already took: {:?}",
                String::from_utf8_lossy(&second_buffered)
            );
        },
        other => panic!("expected AttachAck, got {other:?}"),
    }

    let _ = crate::pty_host::client::terminate_via_socket(&control_socket);
    host.join();
}

/// Two servers holding dups of the same master would each read half the pane's
/// output, so an attach has to lose to whoever is already there.
#[test]
fn an_attach_is_refused_while_a_server_is_attached() {
    let (mut host, _host_id, control_socket) = TestHost::start();
    let control_socket = control_socket.expect("the supervisor should have bound a socket");
    let (_child_pid, master) = host.spawn(sh("sleep 60"));

    let (reply, no_master) = attach_over_socket(&control_socket, 42, None);
    match reply {
        HostToServerMsg::AttachRefused { reason, .. } => {
            assert_eq!(reason, AttachRefusal::AlreadyAttached)
        },
        other => panic!("expected AttachRefused, got {other:?}"),
    }
    assert!(no_master.is_none(), "a refused attach must not send the fd");

    // The original control channel is untouched.
    assert!(query_status(&control_socket).attached);
    drop(master);
    host.send(&ServerToHostMsg::Terminate);
    host.join();
}

/// A supervisor outlives the binary that spawned it, so an upgraded zellij can
/// meet an older host. Refuse rather than misread its messages.
#[test]
fn an_attach_with_a_mismatched_protocol_version_is_refused() {
    let (mut host, _host_id, control_socket) = TestHost::start();
    let control_socket = control_socket.expect("the supervisor should have bound a socket");
    let (_child_pid, master) = host.spawn(sh("sleep 60"));
    host.send(&ServerToHostMsg::Release);
    drop(master);
    wait_until(
        || !query_status(&control_socket).attached,
        "the supervisor to report itself detached",
    );

    let mut stream =
        UnixStream::connect(&control_socket).expect("failed to connect to the named socket");
    stream
        .set_read_timeout(Some(TEST_TIMEOUT))
        .expect("set_read_timeout failed");
    write_msg(
        &mut stream,
        &ServerToHostMsg::Attach {
            protocol_version: PROTOCOL_VERSION + 1,
            zellij_version: VERSION.to_owned(),
            terminal_id: 42,
            session_name: None,
        },
    )
    .expect("failed to send Attach");
    match read_msg::<HostToServerMsg, _>(&mut stream)
        .expect("failed to read the reply")
        .expect("the supervisor closed the connection")
    {
        HostToServerMsg::AttachRefused { reason, .. } => {
            assert_eq!(reason, AttachRefusal::ProtocolMismatch)
        },
        other => panic!("expected AttachRefused, got {other:?}"),
    }

    // Refusing must not have disturbed it: still alive, still detached, still
    // available to a caller that speaks the right version.
    assert!(!query_status(&control_socket).attached);
    let _ = crate::pty_host::client::terminate_via_socket(&control_socket);
    host.join();
}

/// A child that died while nobody was attached has no one to report to, so the
/// exit has to be re-announced on attach. Without this the adopted pane sits
/// there looking alive forever.
#[test]
fn attaching_after_the_child_exited_reports_the_exit() {
    let (mut host, _host_id, control_socket) = TestHost::start();
    let control_socket = control_socket.expect("the supervisor should have bound a socket");
    let (_child_pid, master) = host.spawn(sh("echo done; exit 7"));

    host.send(&ServerToHostMsg::Release);
    drop(master);
    wait_until(
        || query_status(&control_socket).child_exit_status.is_some(),
        "the child to exit while detached",
    );

    let mut stream =
        UnixStream::connect(&control_socket).expect("failed to connect to the named socket");
    stream
        .set_read_timeout(Some(TEST_TIMEOUT))
        .expect("set_read_timeout failed");
    write_msg(
        &mut stream,
        &ServerToHostMsg::Attach {
            protocol_version: PROTOCOL_VERSION,
            zellij_version: VERSION.to_owned(),
            terminal_id: 42,
            session_name: None,
        },
    )
    .expect("failed to send Attach");
    match read_msg::<HostToServerMsg, _>(&mut stream)
        .expect("failed to read the attach reply")
        .expect("the supervisor closed the connection")
    {
        HostToServerMsg::AttachAck { .. } => {},
        other => panic!("expected AttachAck, got {other:?}"),
    }
    let _ = recv_fd(&stream).expect("failed to receive the pty master");

    match read_msg::<HostToServerMsg, _>(&mut stream)
        .expect("failed to read the follow-up")
        .expect("the supervisor closed the connection")
    {
        HostToServerMsg::ChildExited { status } => assert_eq!(status, Some(7)),
        other => panic!("expected ChildExited straight after the AttachAck, got {other:?}"),
    }

    let _ = crate::pty_host::client::terminate_via_socket(&control_socket);
    host.join();
}

/// The releasing server and the attaching one are different processes with
/// nothing ordering them. What makes the hand-off deterministic is that
/// `event_loop` polls the control channel before the listener, so a `Release`
/// already on the wire is always processed before a pending accept.
///
/// This drives that directly: both are queued before the supervisor gets a chance
/// to run, and the attach must still win.
#[test]
fn a_release_racing_an_attach_is_ordered_by_the_poll_loop() {
    let (mut host, _host_id, control_socket) = TestHost::start();
    let control_socket = control_socket.expect("the supervisor should have bound a socket");
    let (child_pid, master) = host.spawn(sh("sleep 60"));

    // Both land in kernel buffers with the supervisor still blocked in `poll`.
    host.send(&ServerToHostMsg::Release);
    let (reply, attached_master) = attach_over_socket(&control_socket, 42, Some("second"));
    match reply {
        HostToServerMsg::AttachAck { child_pid: p, .. } => assert_eq!(p, child_pid),
        other => panic!(
            "a Release written before the Attach should have been processed first, got {other:?}"
        ),
    }
    assert!(attached_master.is_some());

    drop(master);
    let _ = crate::pty_host::client::terminate_via_socket(&control_socket);
    host.join();
}
