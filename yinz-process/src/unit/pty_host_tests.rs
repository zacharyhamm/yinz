use super::*;
use yinz_types::internal_ids::ProcessId;
use yinz_types::internal_ids::SupervisorId;

use crate::pty_host::fdpass::recv_fd;
use nix::pty::openpty;
use nix::sys::termios;
use std::os::fd::{FromRawFd, IntoRawFd};
use std::path::PathBuf;
use std::process::Command;
use std::thread::JoinHandle;
use std::time::Instant;

const TEST_TIMEOUT: Duration = Duration::from_secs(10);

#[test]
fn zeroed_waitid_info_is_no_event() {
    let info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    assert!(matches!(decode_child_event(&info), Ok(None)));
}

#[test]
fn waitid_continued_with_zero_pid_is_an_event() {
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    info.si_signo = libc::SIGCHLD;
    info.si_code = libc::CLD_CONTINUED;
    assert_eq!(unsafe { info.si_pid() }, 0);
    assert!(matches!(
        decode_child_event(&info),
        Ok(Some(HostEvent::ChildContinued))
    ));
}

struct TestHost {
    control: UnixStream,
    handle: Option<JoinHandle<Result<()>>>,
}

impl TestHost {
    /// Boots a supervisor in a thread over a socketpair and completes the
    /// handshake, returning once `HelloAck` is in hand.
    fn start() -> (Self, SupervisorId, Option<PathBuf>) {
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
            yinz_version: VERSION.to_owned(),
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
    fn spawn(&mut self, cmd: RunCommand) -> (ProcessId, std::fs::File) {
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
    ) -> (ProcessId, std::fs::File) {
        self.spawn_with_arg0(cmd, termios, initial_size, None)
    }

    fn spawn_with_arg0(
        &mut self,
        cmd: RunCommand,
        termios: Option<WireTermios>,
        initial_size: Option<WireWinsize>,
        arg0: Option<String>,
    ) -> (ProcessId, std::fs::File) {
        self.spawn_with_env(cmd, termios, initial_size, arg0, vec![], false)
    }

    /// As `spawn_with_arg0`, but lets a test say what environment the child is
    /// given and whether it is a remote login - the two things only the remote
    /// broker sets in production.
    fn spawn_with_env(
        &mut self,
        cmd: RunCommand,
        termios: Option<WireTermios>,
        initial_size: Option<WireWinsize>,
        arg0: Option<String>,
        env: Vec<(String, String)>,
        login_session: bool,
    ) -> (ProcessId, std::fs::File) {
        self.send(&ServerToHostMsg::Spawn {
            job_control: true,
            terminal_id: 0,
            session_name: Some("pty-host-tests".to_owned().into()),
            cmd,
            termios,
            initial_size,
            arg0,
            env,
            login_session,
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
        agent_theme: None,
        command: PathBuf::from("/bin/sh"),
        args: vec!["-c".to_owned(), script.to_owned()],
        cwd: None,
        hold_on_close: false,
        hold_on_start: false,
        originating_plugin: None,
        use_terminal_title: false,
        target: Default::default(),
        remote_default_shell: false,
    }
}

fn process_alive(pid: ProcessId) -> bool {
    kill(Pid::from_raw(i32::try_from(pid).unwrap()), None).is_ok()
}

/// The argv of `pid` as the kernel currently sees it, so a test can tell a shell
/// apart from what it has `exec`ed into.
fn process_argv(pid: ProcessId) -> Option<Vec<String>> {
    let pid = sysinfo::Pid::from_u32(pid.get());
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[pid]),
        false,
        sysinfo::ProcessRefreshKind::nothing().with_cmd(sysinfo::UpdateKind::Always),
    );
    let cmd = system.process(pid)?.cmd();
    if cmd.is_empty() {
        return None;
    }
    Some(
        cmd.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect(),
    )
}

/// Wait until the pane's process really *is* `program`, not the shell that is
/// about to `exec` into it.
///
/// `echo ready` runs before that `exec`, so pty output proves the shell started
/// and nothing more. The distinction matters to anything that stops the child:
/// an `exec` discards a pending continue notification on XNU, so a stop landing
/// in the gap is a stop of a process that is about to be replaced, and the
/// resume that follows is reported to nobody.
fn wait_for_exec_into(pid: ProcessId, program: &str) {
    wait_until(
        // argv[0], not any argument: the shell's own argv is `sh -c ... exec
        // sleep 30`, which contains the program's name while still being the
        // shell.
        || process_argv(pid).is_some_and(|argv| argv[0].contains(program)),
        &format!("the pane's process to exec into {program}"),
    );
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
    let (child_pid, mut master) = host.spawn(sh("echo yinz-pty-host-marker"));
    assert!(child_pid > yinz_types::internal_ids::ProcessId::new(0));

    let output = read_until(&mut master, "yinz-pty-host-marker");
    assert!(
        output.contains("yinz-pty-host-marker"),
        "did not see the child's output: {output:?}"
    );

    match host.recv() {
        HostToServerMsg::ChildExited { status } => assert_eq!(status, Some(0)),
        other => panic!("expected ChildExited, got {other:?}"),
    }
    host.send(&ServerToHostMsg::Terminate);
    host.join();
}

/// `arg0` is the whole of what makes a remote pane's shell a *login* shell —
/// a shell decides to read its profile by looking at whether `argv[0]` begins
/// with `-`, and nothing else. The remote broker is the only caller that sets
/// it, so this pins both halves: that it reaches the exec, and that a pane
/// without one still runs under the command's own name, as local panes do.
#[test]
fn arg0_overrides_the_childs_own_name() {
    let (mut host, _host_id, _sock) = TestHost::start();
    let (_child_pid, mut master) =
        host.spawn_with_arg0(sh("echo argv0=$0"), None, None, Some("-sh".to_owned()));

    let output = read_until(&mut master, "argv0=");
    assert!(
        output.contains("argv0=-sh"),
        "the child should have been exec'd as a login shell: {output:?}"
    );

    host.send(&ServerToHostMsg::Terminate);
    host.join();
}

#[test]
fn without_arg0_the_child_runs_under_its_own_name() {
    let (mut host, _host_id, _sock) = TestHost::start();
    let (_child_pid, mut master) = host.spawn(sh("echo argv0=$0"));

    let output = read_until(&mut master, "argv0=");
    assert!(
        output.contains("argv0=/bin/sh"),
        "a local pane's exec must be unchanged: {output:?}"
    );

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

struct GroupProcess {
    pid: ProcessId,
    child: Option<Child>,
    reaper: Option<JoinHandle<std::process::ExitStatus>>,
    _stdin: std::process::ChildStdin,
    output: yinz_test_support::Watched<String>,
}

impl GroupProcess {
    fn spawn(script: &str, group: i32) -> Self {
        use std::io::BufRead;
        let mut child = Command::new("/bin/sh")
            .args(["-c", script])
            .process_group(group)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let process = Self {
            pid: ProcessId::new(child.id()),
            child: Some(child),
            reaper: None,
            _stdin: stdin,
            output: yinz_test_support::Watched::default(),
        };
        let output = process.output.clone();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                output.lock().push_str(&format!("{}\n", line.unwrap()));
            }
        });
        process
            .output
            .wait_until("group member readiness", TEST_TIMEOUT, |output| {
                output.contains("ready\n").then_some(())
            });
        process
    }

    fn reap(&mut self) {
        let mut child = self.child.take().unwrap();
        self.reaper = Some(std::thread::spawn(move || child.wait().unwrap()));
    }

    fn join(&mut self) -> std::process::ExitStatus {
        let reaper = self.reaper.as_ref().unwrap();
        wait_until(|| reaper.is_finished(), "group member reaping");
        self.reaper.take().unwrap().join().unwrap()
    }
}

impl Drop for GroupProcess {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(reaper) = self.reaper.take() {
            if !reaper.is_finished() {
                let _ = kill(super::super::kernel_pid(self.pid), Some(Signal::SIGKILL));
            }
            let _ = reaper.join();
        }
    }
}

fn session_with_child(child: Child) -> (Session, Receiver<HostEvent>, UnixStream) {
    let child_pid = ProcessId::new(child.id());
    let pty = openpty(None, None).unwrap();
    let (event_tx, event_rx) = mpsc::channel();
    let (wake_tx, wake_rx) = UnixStream::pair().unwrap();
    wake_rx.set_nonblocking(true).unwrap();
    let (reap_child, child_reaped) = spawn_child_waiter(child, event_tx, wake_tx);
    (
        Session {
            master: pty.master,
            slave: Some(pty.slave),
            child_pid,
            reap_child: Some(reap_child),
            child_reaped,
            exit_status: None,
            drained: false,
        },
        event_rx,
        wake_rx,
    )
}

#[test]
fn exited_leader_stays_reserved_until_the_last_group_signal() {
    let mut leader = GroupProcess::spawn("trap 'exit 0' HUP; echo ready; read line", 0);
    let (mut session, events, _wake) = session_with_child(leader.child.take().unwrap());
    session.signal_child_group(Signal::SIGHUP).unwrap();
    assert!(matches!(
        events.recv_timeout(TEST_TIMEOUT).unwrap(),
        HostEvent::ChildExited(Some(0))
    ));
    assert!(matches!(
        wait_child(
            super::super::kernel_pid(leader.pid),
            libc::WEXITED | libc::WNOWAIT | libc::WNOHANG
        ),
        Ok(Some(HostEvent::ChildExited(Some(0))))
    ));
    session.signal_child_group(Signal::SIGKILL).unwrap();
    session.reap_child.take();
    assert!(session
        .child_reaped
        .recv_timeout(TEST_TIMEOUT)
        .unwrap()
        .unwrap()
        .success());
    assert!(matches!(
        wait_child(
            super::super::kernel_pid(leader.pid),
            libc::WEXITED | libc::WNOWAIT | libc::WNOHANG
        ),
        Err(Errno::ECHILD)
    ));

    // Simulate PID reuse without waiting for the kernel to recycle an ID.
    let mut unrelated = GroupProcess::spawn("echo ready; read line", 0);
    session.child_pid = unrelated.pid;
    for signal in [Signal::SIGCONT, Signal::SIGHUP, Signal::SIGKILL] {
        assert!(session.signal_child_group(signal).is_err());
    }
    assert!(unrelated
        .child
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none());
}

#[test]
fn dropping_a_session_releases_its_exited_leader() {
    let mut leader = GroupProcess::spawn("trap 'exit 0' HUP; echo ready; read line", 0);
    let (session, events, _wake) = session_with_child(leader.child.take().unwrap());
    session.signal_child_group(Signal::SIGHUP).unwrap();
    assert!(matches!(
        events.recv_timeout(TEST_TIMEOUT).unwrap(),
        HostEvent::ChildExited(Some(0))
    ));
    drop(session);
    wait_until(
        || !process_alive(leader.pid),
        "dropped session's child reaping",
    );
}

#[test]
fn group_signals_refuse_a_leader_reaped_outside_the_waiter() {
    let mut leader = GroupProcess::spawn("trap 'exit 0' HUP; echo ready; read line", 0);
    let (session, events, _wake) = session_with_child(leader.child.take().unwrap());
    session.signal_child_group(Signal::SIGHUP).unwrap();
    assert!(matches!(
        events.recv_timeout(TEST_TIMEOUT).unwrap(),
        HostEvent::ChildExited(Some(0))
    ));
    nix::sys::wait::waitpid(super::super::kernel_pid(leader.pid), None).unwrap();
    for signal in [Signal::SIGCONT, Signal::SIGHUP, Signal::SIGKILL] {
        let error = session.signal_child_group(signal).unwrap_err();
        assert_eq!(error.downcast_ref::<Errno>(), Some(&Errno::ECHILD));
    }
}

// Both members remain children of the test, so leader-exit cases never rely on init
// reaping orphan zombies. The supervisor owns the leader's real waiter.
fn host_with_process_group(member_script: &str) -> (TestHost, PathBuf, GroupProcess, GroupProcess) {
    let mut leader = GroupProcess::spawn("trap 'exit 0' HUP; echo ready; read line", 0);
    let member = GroupProcess::spawn(member_script, super::super::kernel_pid(leader.pid).as_raw());
    let host_id = protocol::new_host_id();
    let listener = bind_listener(&host_id).unwrap();
    let socket = protocol::socket_path(&host_id);
    let (session, event_rx, wake_rx) = session_with_child(leader.child.take().unwrap());
    let mut ctx = HostContext {
        host_id,
        host_pid: ProcessId::new(std::process::id()),
        label: "test/group".into(),
        terminal_id: TerminalId::new(0),
        session_name: Some("pty-host-tests".into()),
        cmd: sh(""),
        started_at_unix: UnixSeconds::new(unix_now()),
        state: State::Attached,
        ring: RingBuffer::new(),
        session: Some(session),
    };
    let (control, host_control) = UnixStream::pair().unwrap();
    let handle = std::thread::spawn(move || {
        let result = event_loop(&mut ctx, host_control, Some(listener), event_rx, wake_rx);
        cleanup_socket(&ctx.host_id);
        result
    });
    (
        TestHost {
            control,
            handle: Some(handle),
        },
        socket,
        leader,
        member,
    )
}

#[test]
fn termination_continues_and_hangs_up_a_stopped_descendant() {
    let (mut host, _socket, leader, mut member) = host_with_process_group(
        r#"
        exec 3<&0
        /bin/sh -c 'trap "echo descendant-hung-up; exit 0" HUP
                    echo descendant=$$; kill -STOP $$; read line' <&3 &
        child=$!
        trap 'wait "$child"; exit 0' HUP
        echo ready
        wait "$child"
        "#,
    );
    let descendant = member
        .output
        .wait_until("descendant readiness", TEST_TIMEOUT, |output| {
            output.lines().find_map(|line| {
                line.strip_prefix("descendant=")
                    .map(|pid| Pid::from_raw(pid.parse().unwrap()))
            })
        });
    struct ResumeOnDrop(Pid);
    impl Drop for ResumeOnDrop {
        fn drop(&mut self) {
            let _ = kill(self.0, Some(Signal::SIGCONT));
            let _ = kill(self.0, Some(Signal::SIGHUP));
        }
    }
    let _cleanup = ResumeOnDrop(descendant);
    let pid = sysinfo::Pid::from_u32(descendant.as_raw() as u32);
    let mut system = sysinfo::System::new();
    wait_until(
        || {
            system.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::Some(&[pid]),
                true,
                sysinfo::ProcessRefreshKind::nothing(),
            );
            system
                .process(pid)
                .is_some_and(|process| process.status() == sysinfo::ProcessStatus::Stop)
        },
        "descendant to stop",
    );
    member.reap();
    host.send(&ServerToHostMsg::Terminate);
    host.join();
    member
        .output
        .wait_until("descendant hangup handler", TEST_TIMEOUT, |output| {
            output.contains("descendant-hung-up\n").then_some(())
        });
    assert!(member.join().success());
    assert!(!process_alive(leader.pid));
    assert!(super::super::process_group_terminated(leader.pid));
}

#[test]
fn termination_escalates_after_the_leader_exits_and_spares_other_groups() {
    use std::os::unix::process::ExitStatusExt;
    let (mut host, socket, leader, mut member) =
        host_with_process_group("trap '' HUP; echo ready; read line");
    let mut unrelated = GroupProcess::spawn("echo ready; read line", 0);
    member.reap();
    host.send(&ServerToHostMsg::Release);
    wait_until(|| !query_status(&socket).attached, "detachment");
    let started = Instant::now();
    assert!(matches!(
        conditional_terminate(&socket, "pty-host-tests"),
        HostToServerMsg::TerminationComplete {
            terminated: true,
            error: None
        }
    ));
    assert!(started.elapsed() >= TERMINATE_GRACE, "skipped hangup grace");
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "exceeded client deadline"
    );
    assert!(!process_alive(leader.pid));
    assert_eq!(member.join().signal(), Some(libc::SIGKILL));
    assert!(super::super::process_group_terminated(leader.pid));
    assert!(unrelated
        .child
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none());
    host.join();
}

#[test]
fn termination_cleans_the_group_of_a_previously_exited_leader() {
    use std::os::unix::process::ExitStatusExt;
    let (mut host, _socket, leader, mut member) =
        host_with_process_group("trap '' HUP; echo ready; read line");
    member.reap();
    kill(super::super::kernel_pid(leader.pid), Some(Signal::SIGHUP)).unwrap();
    assert!(matches!(
        host.recv(),
        HostToServerMsg::ChildExited { status: Some(0) }
    ));
    assert!(matches!(
        wait_child(
            super::super::kernel_pid(leader.pid),
            libc::WEXITED | libc::WNOWAIT | libc::WNOHANG
        ),
        Ok(Some(HostEvent::ChildExited(Some(0))))
    ));
    assert!(process_alive(member.pid));
    host.send(&ServerToHostMsg::Terminate);
    host.join();
    assert_eq!(member.join().signal(), Some(libc::SIGKILL));
    assert!(!process_alive(leader.pid));
    assert!(super::super::process_group_terminated(leader.pid));
}

#[test]
fn detached_termination_reports_unconfirmed_group_teardown() {
    let (mut host, socket, leader, mut member) =
        host_with_process_group("trap '' HUP; echo ready; read line");
    // Keep the killed member unreaped until after the reply: ESRCH must not be assumed.
    host.send(&ServerToHostMsg::Release);
    wait_until(|| !query_status(&socket).attached, "detachment");
    match conditional_terminate(&socket, "pty-host-tests") {
        HostToServerMsg::TerminationComplete {
            terminated: true,
            error: Some(error),
        } => {
            assert!(
                error.contains("leader reaped=true, group gone=false"),
                "{error}"
            );
        },
        other => panic!("expected unconfirmed group teardown, got {other:?}"),
    }
    assert!(!process_alive(leader.pid));
    member.child.as_mut().unwrap().wait().unwrap();
    assert!(super::super::process_group_terminated(leader.pid));
    host.join();
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

    fn ignored_signals(pid: ProcessId) -> u64 {
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
        yinz_version: VERSION.to_owned(),
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
        yinz_version: VERSION.to_owned(),
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
        job_control: true,
        terminal_id: 0,
        session_name: None,
        cmd: RunCommand {
            agent_theme: None,
            command: PathBuf::from("/nonexistent/yinz-pty-host-no-such-binary"),
            args: vec![],
            cwd: None,
            hold_on_close: false,
            hold_on_start: false,
            originating_plugin: None,
            use_terminal_title: false,
            target: Default::default(),
            remote_default_shell: false,
        },
        termios: None,
        initial_size: None,
        arg0: None,
        env: vec![],
        login_session: false,
    });
    match host.recv() {
        HostToServerMsg::SpawnFailed { kind, .. } => {
            assert_eq!(kind, SpawnFailureKind::CommandNotFound)
        },
        other => panic!("expected SpawnFailed, got {other:?}"),
    }
}

#[test]
fn an_exec_failure_is_reported_instead_of_announcing_a_spawned_child() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("broken-command");
    std::fs::write(&script, b"#!/nonexistent/yinz-test-interpreter\n").unwrap();
    for (mode, expected_kind) in [
        (0o600, SpawnFailureKind::CommandNotFound),
        // Executable access passes preflight, but exec cannot find the interpreter.
        (0o700, SpawnFailureKind::Other),
    ] {
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(mode)).unwrap();
        let (mut host, _, _) = TestHost::start();
        host.send(&ServerToHostMsg::Spawn {
            job_control: true,
            terminal_id: 0,
            session_name: None,
            cmd: RunCommand {
                command: script.clone(),
                ..Default::default()
            },
            termios: None,
            initial_size: None,
            arg0: None,
            env: vec![],
            login_session: false,
        });
        let response = host.recv();
        assert!(
            matches!(&response, HostToServerMsg::SpawnFailed { kind, .. } if *kind == expected_kind),
            "{response:?}"
        );
        assert!(host.handle.take().unwrap().join().unwrap().is_err());
    }
}

/// A supervisor may outlive the yinz that spawned it, so a later server can
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
        yinz_version: "99.0.0".to_owned(),
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
    let rc = unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCGWINSZ, &mut actual) };
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
            yinz_version: VERSION.to_owned(),
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
    let bare: anyhow::Error = YinzError::CommandNotFound {
        terminal_id: yinz_types::internal_ids::TerminalId::new(3),
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
        "context layers hid the YinzError from the classifier"
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

#[test]
fn failed_attach_preserves_owner_output_and_loss_count_for_retry() {
    let pty = openpty(None, None).unwrap();
    let (_reaped_tx, child_reaped) = mpsc::channel();
    let mut ctx = HostContext {
        host_id: protocol::new_host_id(),
        host_pid: ProcessId::new(std::process::id()),
        label: "original/7".into(),
        terminal_id: TerminalId::new(7),
        session_name: Some("original".into()),
        cmd: sh(""),
        started_at_unix: UnixSeconds::new(unix_now()),
        state: State::Detached,
        ring: RingBuffer::new(),
        session: Some(Session {
            master: pty.master,
            slave: Some(pty.slave),
            child_pid: ProcessId::new(std::process::id()),
            reap_child: None,
            child_reaped,
            exit_status: None,
            drained: false,
        }),
    };
    ctx.ring.push(&vec![b'x'; RING_CAPACITY]);
    ctx.ring.push(b"tail");
    let expected_output: Vec<_> = ctx.ring.buf.iter().copied().collect();

    let (peer, host) = UnixStream::pair().unwrap();
    peer.shutdown(std::net::Shutdown::Read).unwrap();
    assert!(matches!(
        attach(
            &mut ctx,
            host,
            PROTOCOL_VERSION,
            TerminalId::new(42),
            Some("failed-owner".into()),
        ),
        CliOutcome::Done
    ));
    assert_eq!(ctx.state, State::Detached);
    assert_eq!(ctx.terminal_id, TerminalId::new(7));
    assert_eq!(ctx.session_name, Some("original".into()));
    assert_eq!(ctx.label, "original/7");
    assert_eq!(ctx.ring.dropped, 4);
    assert!(ctx
        .ring
        .buf
        .iter()
        .copied()
        .eq(expected_output.iter().copied()));

    let (mut peer, host) = UnixStream::pair().unwrap();
    peer.set_read_timeout(Some(TEST_TIMEOUT)).unwrap();
    host.set_write_timeout(Some(TEST_TIMEOUT)).unwrap();
    let receiver = std::thread::spawn(move || {
        let ack = read_msg::<HostToServerMsg, _>(&mut peer).unwrap().unwrap();
        let master = recv_fd(&peer).unwrap();
        (ack, master)
    });
    assert!(matches!(
        attach(
            &mut ctx,
            host,
            PROTOCOL_VERSION,
            TerminalId::new(43),
            Some("retry-owner".into()),
        ),
        CliOutcome::Attached(_)
    ));
    let (ack, _master) = receiver.join().unwrap();
    let HostToServerMsg::AttachAck {
        status,
        buffered_base64,
        dropped_bytes,
        ..
    } = ack
    else {
        panic!("expected attach acknowledgement")
    };
    assert_eq!(dropped_bytes, 4);
    assert_eq!(
        protocol::decode_buffered(&buffered_base64).unwrap(),
        expected_output
    );
    assert_eq!(status.terminal_id, 43);
    assert_eq!(status.session_name, Some("retry-owner".into()));
    assert!(status.attached);
    assert_eq!(status.buffered_bytes, 0);
    assert_eq!(ctx.state, State::Attached);
    assert_eq!(ctx.terminal_id, TerminalId::new(43));
    assert_eq!(ctx.session_name, Some("retry-owner".into()));
    assert_eq!(ctx.label, "retry-owner/43");
    assert_eq!(ctx.ring.len(), 0);
    assert_eq!(ctx.ring.dropped, 0);
}

/// Connect to a supervisor's named socket and ask it to hand the pane over,
/// returning the reply and (on success) the pty master it sent with it.
fn attach_over_socket(
    socket: &std::path::Path,
    terminal_id: yinz_types::internal_ids::TerminalId,
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
            yinz_version: VERSION.to_owned(),
            terminal_id: terminal_id.get(),
            session_name: session_name.map(str::to_owned).map(Into::into),
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
    // Armed before the release, not after: macOS refuses `setsockopt` on a
    // socketpair whose peer has already closed.
    host.control
        .set_read_timeout(Some(TEST_TIMEOUT))
        .expect("set_read_timeout failed");
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
    let mut command = sh("while read line; do echo \"got:$line\"; done");
    command.originating_plugin = Some(yinz_types::data::OriginatingPlugin::new(
        yinz_types::internal_ids::PluginId::new(41),
        yinz_types::internal_ids::ClientId::new(7),
        std::collections::BTreeMap::from([("test_context".into(), "source".into())]),
    ));
    let (child_pid, mut master) = host.spawn(command);

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

    let (reply, master) = attach_over_socket(
        &control_socket,
        yinz_types::internal_ids::TerminalId::new(42),
        Some("the-other-session"),
    );
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
    assert_eq!(
        status.session_name.as_ref().map(|name| name.as_str()),
        Some("the-other-session")
    );
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

    let (first, first_master) = attach_over_socket(
        &control_socket,
        yinz_types::internal_ids::TerminalId::new(1),
        None,
    );
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
    let (second, _) = attach_over_socket(
        &control_socket,
        yinz_types::internal_ids::TerminalId::new(2),
        None,
    );
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

    let (reply, no_master) = attach_over_socket(
        &control_socket,
        yinz_types::internal_ids::TerminalId::new(42),
        None,
    );
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

#[test]
fn agent_theme_retention_tracks_launches_and_rejects_duplicate_starts_and_stale_ends() {
    use yinz_types::agent::theme::{
        AgentThemeEvent, AgentThemeLaunch, AgentThemeSnapshot, Availability, EVENT_VERSION,
        SNAPSHOT_VERSION,
    };
    let (mut host, _, socket) = TestHost::start();
    let socket = socket.unwrap();
    let snapshot = AgentThemeSnapshot {
        version: SNAPSHOT_VERSION,
        provider: "claude".into(),
        provider_version: "2.1.296".into(),
        theme: "custom:first".into(),
        provenance: vec!["fixture-v1".into()],
        availability: Availability::Available,
        definitions: vec![],
        collisions: vec![],
    };
    let mut command = sh("while read line; do echo \"$line\"; done");
    let mut previous = snapshot.clone();
    previous.theme = "previous-run".into();
    command.agent_theme = Some(AgentThemeLaunch {
        launch_id: "previous".into(),
        snapshot: previous,
    });
    let (_, master) = host.spawn(command);
    for (launch_id, theme) in [
        ("old", Some("old")),
        ("old", None),
        ("first", Some("custom:first")),
        ("first", Some("custom:later")),
        ("old", None),
    ] {
        let snapshot = theme.map(|theme| {
            let mut value = snapshot.clone();
            value.theme = theme.into();
            value
        });
        let mut report = UnixStream::connect(&socket).unwrap();
        report.set_read_timeout(Some(TEST_TIMEOUT)).unwrap();
        write_msg(
            &mut report,
            &ServerToHostMsg::AgentTheme(AgentThemeEvent {
                version: EVENT_VERSION,
                launch_id: launch_id.into(),
                snapshot,
            }),
        )
        .unwrap();
        assert!(matches!(
            read_msg::<HostToServerMsg, _>(&mut report).unwrap(),
            Some(HostToServerMsg::Status(_))
        ));
    }
    host.send(&ServerToHostMsg::Release);
    let (reply, adopted_master) = attach_over_socket(
        &socket,
        yinz_types::internal_ids::TerminalId::new(42),
        Some("new-owner"),
    );
    match reply {
        HostToServerMsg::AttachAck { cmd, .. } => assert_eq!(
            cmd.agent_theme,
            Some(AgentThemeLaunch {
                launch_id: "first".into(),
                snapshot
            })
        ),
        other => panic!("unexpected adoption response: {other:?}"),
    }
    drop(master);
    drop(adopted_master);
    crate::pty_host::client::terminate_via_socket(&socket).unwrap();
    host.join();
}

#[test]
fn learned_agent_theme_survives_server_disconnect_and_adoption() {
    use yinz_types::agent::theme::*;

    for count in [1, 2] {
        let (mut host, _, socket) = TestHost::start();
        let socket = socket.unwrap();
        let (_, master) = host.spawn(sh("while read line; do echo \"$line\"; done"));
        let initial = AgentThemeEvent {
            version: EVENT_VERSION,
            launch_id: "codex-run".into(),
            snapshot: Some(AgentThemeSnapshot {
                version: SNAPSHOT_VERSION,
                provider: "codex".into(),
                provider_version: "0.162.1".into(),
                theme: "automatic".into(),
                provenance: vec![],
                availability: Availability::Available,
                definitions: vec![],
                collisions: vec![],
            }),
        };
        let report = |event: &AgentThemeEvent| {
            let mut stream = UnixStream::connect(&socket).unwrap();
            stream.set_read_timeout(Some(TEST_TIMEOUT)).unwrap();
            write_msg(&mut stream, &ServerToHostMsg::AgentTheme(event.clone())).unwrap();
            assert!(matches!(
                read_msg::<HostToServerMsg, _>(&mut stream).unwrap(),
                Some(HostToServerMsg::Status(_))
            ));
        };
        report(&initial);
        let mut learned = initial.clone();
        for (token, channel, color, role) in [
            (
                "foreground",
                Channel::Foreground,
                SourceColor::Rgb(205, 206, 207),
                Role::Foreground,
            ),
            (
                "background",
                Channel::Background,
                SourceColor::Rgb(25, 35, 48),
                Role::Background,
            ),
        ]
        .into_iter()
        .take(count)
        {
            learned
                .snapshot
                .as_mut()
                .unwrap()
                .definitions
                .push(Definition {
                    token: format!("codex.terminal.{token}"),
                    channel,
                    color,
                    pair: None,
                    target: Some(role.into()),
                });
            host.send(&ServerToHostMsg::AgentTheme(learned.clone()));
            // Query on the same stream is a barrier for this update.
            host.send(&ServerToHostMsg::Query);
            assert!(matches!(host.recv(), HostToServerMsg::Status(_)));
        }
        // A delayed launcher report must not overwrite the enriched snapshot.
        host.send(&ServerToHostMsg::AgentTheme(learned.clone()));
        host.send(&ServerToHostMsg::Query);
        assert!(matches!(host.recv(), HostToServerMsg::Status(_)));
        report(&initial);
        let mut stale = initial.clone();
        stale.launch_id = "previous-launch".into();
        host.send(&ServerToHostMsg::AgentTheme(stale));
        let mut invalid = initial.clone();
        invalid.version += 1;
        host.send(&ServerToHostMsg::AgentTheme(invalid));

        // Simulate a crash, not an orderly Release. Updates already written to
        // this socket must be retained before the supervisor processes EOF.
        host.control.shutdown(std::net::Shutdown::Both).unwrap();
        drop(master);
        wait_until(|| !query_status(&socket).attached, "server disconnect");
        let (reply, adopted_master) = attach_over_socket(
            &socket,
            yinz_types::internal_ids::TerminalId::new(42),
            Some("recovered"),
        );
        let HostToServerMsg::AttachAck { cmd, .. } = reply else {
            panic!("expected adoption acknowledgement")
        };
        assert_eq!(
            cmd.agent_theme,
            Some(AgentThemeLaunch {
                launch_id: learned.launch_id,
                snapshot: learned.snapshot.unwrap(),
            })
        );
        drop(adopted_master);
        crate::pty_host::client::terminate_via_socket(&socket).unwrap();
        host.join();
    }
}

/// A supervisor outlives the binary that spawned it, so an upgraded yinz can
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
            yinz_version: VERSION.to_owned(),
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
            yinz_version: VERSION.to_owned(),
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
    let (reply, attached_master) = attach_over_socket(
        &control_socket,
        yinz_types::internal_ids::TerminalId::new(42),
        Some("second"),
    );
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

/// The environment composed on the far machine has to survive all the way to the
/// exec, which is the half of the remote-pane environment work that lives here.
#[test]
fn a_supervisor_applies_the_environment_it_is_given() {
    let (mut host, _host_id, _sock) = TestHost::start();
    let (_child_pid, mut master) = host.spawn_with_env(
        sh("echo term=$TERM home=$HOME"),
        None,
        None,
        None,
        vec![
            ("TERM".to_owned(), "xterm-yinz-test".to_owned()),
            ("HOME".to_owned(), "/home/far".to_owned()),
        ],
        true,
    );

    let output = read_until(&mut master, "term=");
    assert!(
        output.contains("term=xterm-yinz-test") && output.contains("home=/home/far"),
        "the child should have been given the composed environment: {output:?}"
    );

    host.send(&ServerToHostMsg::Terminate);
    host.join();
}

/// `SSH_TTY` is the one variable nothing upstream of the supervisor can answer:
/// the path only exists once this process has called `openpty`. It is also the
/// conventional "am I on a terminal over ssh" probe, which is what makes it worth
/// setting when `SSH_CONNECTION` deliberately is not.
#[test]
fn a_login_session_is_told_its_own_pty() {
    let (mut host, _host_id, _sock) = TestHost::start();
    let (_child_pid, mut master) = host.spawn_with_env(
        sh("echo ssh_tty=$SSH_TTY tty=$(tty)"),
        None,
        None,
        None,
        vec![],
        true,
    );

    let output = read_until(&mut master, "ssh_tty=");
    let field = |name: &str| {
        output
            .split_whitespace()
            .find_map(|word| word.strip_prefix(name))
            .map(|value| value.trim().to_owned())
    };
    let ssh_tty = field("ssh_tty=").expect("an SSH_TTY");
    assert!(
        ssh_tty.starts_with("/dev/"),
        "SSH_TTY should name the pane's pty, got {ssh_tty:?} in {output:?}"
    );
    assert_eq!(
        Some(ssh_tty),
        field("tty="),
        "SSH_TTY must be the pty the child is actually on: {output:?}"
    );

    host.send(&ServerToHostMsg::Terminate);
    host.join();
}

/// A local pane is not a login and must not be touched by any of this: it
/// inherits the server's environment, `SSH_*` and all, exactly as it always has.
/// The scrub is gated on `login_session` precisely so that this stays true.
#[test]
fn a_local_pane_is_spawned_exactly_as_before() {
    // This supervisor is forked from the test process, so what is set here is
    // what a local pane would inherit in production.
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("SSH_CONNECTION", "10.0.0.1 4242 10.0.0.2 22") };
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("SSH_TTY", "/dev/pts/inherited") };
    let (mut host, _host_id, _sock) = TestHost::start();
    let (_child_pid, mut master) = host.spawn(sh("echo conn=[$SSH_CONNECTION] tty=[$SSH_TTY]"));

    let output = read_until(&mut master, "conn=");
    assert!(
        output.contains("conn=[10.0.0.1 4242 10.0.0.2 22]"),
        "a local pane must keep inheriting the server's environment: {output:?}"
    );
    // Passed straight through, *not* replaced with this pane's pty: the pty is
    // named only for a remote login, and a local pane's SSH_TTY - if it has one -
    // is a true fact about the terminal yinz itself was started from.
    assert!(
        output.contains("tty=[/dev/pts/inherited]"),
        "a local pane's SSH_TTY must be inherited untouched: {output:?}"
    );

    host.send(&ServerToHostMsg::Terminate);
    host.join();
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("SSH_CONNECTION") };
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("SSH_TTY") };
}

/// A pane keeps this reference in its process environment while its per-session
/// name and terminal id change. The named supervisor socket resolves it to the
/// pane's current owner after a warp.
#[test]
fn a_local_pane_inherits_its_stable_supervisor_reference() {
    let (mut host, host_id, control_socket) = TestHost::start();
    assert!(
        control_socket.is_some(),
        "the supervisor needs a named socket"
    );
    let (_child_pid, mut master) = host.spawn(sh("echo pane_ref=[$YINZ_PANE_REF]"));

    let output = read_until(&mut master, "pane_ref=");
    assert!(
        output.contains(&format!("pane_ref=[{host_id}]")),
        "the pane should inherit its supervisor's stable id: {output:?}"
    );

    host.send(&ServerToHostMsg::Terminate);
    host.join();
}

/// A yinz session started *inside* a remote pane spawns panes of its own, and they
/// inherit that pane's broker token from its shell. Unlike a supervisor reference the
/// token sits beside a `YINZ_PANE_ID` that really is this machine's, so nothing else
/// would catch it: a coding agent's hook reads the token first - it has to, because in a
/// remote pane the pane id is the *asking* session's - and would report the outer pane.
#[test]
fn a_pane_does_not_inherit_an_outer_panes_broker_token() {
    let token = crate::pty_host::remote::broker::PANE_TOKEN_ENV;
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var(token, "an-outer-panes-token") };
    let (mut host, _host_id, _sock) = TestHost::start();
    let (_child_pid, mut master) = host.spawn(sh(&format!("echo token=[${token}]")));

    let output = read_until(&mut master, "token=");
    assert!(
        output.contains("token=[]"),
        "a pane reused an outer pane's broker token: {output:?}"
    );

    host.send(&ServerToHostMsg::Terminate);
    host.join();
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var(token) };
}

#[test]
fn panes_clear_inherited_inline_hooks_but_keep_launch_assignments() {
    let marker = "YINZ_CLAUDE_INLINE_HOOKS";
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var(marker, "1") };
    for login_session in [false, true] {
        for inline in [false, true] {
            let (mut host, _host_id, _sock) = TestHost::start();
            let mut cmd = sh(&format!("echo inline=[${{{marker}-unset}}] done"));
            if inline {
                cmd.args
                    .splice(0..0, [format!("{marker}=1"), "/bin/sh".into()]);
                cmd.command = "/usr/bin/env".into();
            }
            let (_, mut master) = host.spawn_with_env(cmd, None, None, None, vec![], login_session);
            let output = read_until(&mut master, "done");
            let expected = if inline {
                "inline=[1]"
            } else {
                "inline=[unset]"
            };
            assert!(output.contains(expected), "{output:?}");
            host.send(&ServerToHostMsg::Terminate);
            host.join();
        }
    }
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var(marker) };
}

/// The other half of that: a real remote pane is *given* its token by the broker, which
/// mints one per pane and names it in the spawn. The scrub above runs first for exactly
/// this reason - it must not be able to take back what the spawn asked for.
#[test]
fn a_spawn_that_names_a_broker_token_keeps_it() {
    let token = crate::pty_host::remote::broker::PANE_TOKEN_ENV;
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var(token, "an-outer-panes-token") };
    let (mut host, _host_id, _sock) = TestHost::start();
    let (_child_pid, mut master) = host.spawn_with_env(
        sh(&format!("echo token=[${token}]")),
        None,
        None,
        None,
        vec![(token.to_owned(), "this-panes-token".to_owned())],
        true,
    );

    let output = read_until(&mut master, "token=");
    assert!(
        output.contains("token=[this-panes-token]"),
        "a remote pane lost the token its broker minted for it: {output:?}"
    );

    host.send(&ServerToHostMsg::Terminate);
    host.join();
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var(token) };
}

/// The frozen `SSH_*` of whichever ssh session started the broker are inherited
/// by every pane on that machine. They describe a connection this pane has no
/// part in, so a remote login drops them rather than passing them on.
#[test]
fn a_login_session_scrubs_the_brokers_frozen_ssh_variables() {
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("SSH_CLIENT", "10.0.0.1 4242 22") };
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("SSH_AUTH_SOCK", "/tmp/agent-from-the-bootstrap") };
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("YINZ_SESSION_NAME", "a-session-on-the-asking-machine") };
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe {
        std::env::set_var(
            crate::pty_host::PANE_REF_ENV_KEY,
            "an-outer-panes-reference",
        )
    };
    let (mut host, _host_id, _sock) = TestHost::start();
    let (_child_pid, mut master) = host.spawn_with_env(
        sh("echo client=[$SSH_CLIENT] agent=[$SSH_AUTH_SOCK] \
             sess=[$YINZ_SESSION_NAME] ref=[$YINZ_PANE_REF]"),
        None,
        None,
        None,
        vec![],
        true,
    );

    let output = read_until(&mut master, "client=");
    for absent in ["client=[]", "agent=[]", "sess=[]", "ref=[]"] {
        assert!(
            output.contains(absent),
            "expected {absent} in a remote login's environment: {output:?}"
        );
    }

    host.send(&ServerToHostMsg::Terminate);
    host.join();
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("SSH_CLIENT") };
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("SSH_AUTH_SOCK") };
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("YINZ_SESSION_NAME") };
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var(crate::pty_host::PANE_REF_ENV_KEY) };
}

/// `^Z` in a command pane stops the pane's root process, and nothing inside the
/// pane can undo that: the process is stopped, and a command pane has no shell
/// above it to run `fg`. The supervisor is the only thing in a position to
/// notice, which it can only do because it waits with `WUNTRACED`.
#[test]
fn a_stopped_child_is_reported_and_so_is_its_continue() {
    let (mut host, _host_id, _sock) = TestHost::start();
    // `exec` so the pid the supervisor watches is `sleep` itself rather than a
    // shell that may or may not have exec'd it away.
    let (child_pid, mut master) = host.spawn(sh("echo ready; exec sleep 30"));
    read_until(&mut master, "ready");
    wait_for_exec_into(child_pid, "sleep");

    kill(
        Pid::from_raw(i32::try_from(child_pid).unwrap()),
        Some(Signal::SIGSTOP),
    )
    .expect("could not stop the child");
    match host.recv() {
        HostToServerMsg::ChildStopped { signal } => {
            assert_eq!(
                signal,
                Signal::SIGSTOP as i32,
                "the stop should name the signal that caused it"
            );
        },
        other => panic!("expected ChildStopped, got {other:?}"),
    }

    kill(
        Pid::from_raw(i32::try_from(child_pid).unwrap()),
        Some(Signal::SIGCONT),
    )
    .expect("could not continue the child");
    match host.recv() {
        HostToServerMsg::ChildContinued => {},
        other => panic!("expected ChildContinued, got {other:?}"),
    }

    // The waiter has to survive both transitions: a stop that swallowed the
    // exit would leave the pane open forever.
    kill(
        Pid::from_raw(i32::try_from(child_pid).unwrap()),
        Some(Signal::SIGKILL),
    )
    .expect("could not kill the child");
    match host.recv() {
        HostToServerMsg::ChildExited { status } => assert_eq!(status, None),
        other => panic!("expected ChildExited, got {other:?}"),
    }

    host.send(&ServerToHostMsg::Terminate);
    host.join();
}

#[test]
fn a_child_with_a_sigcont_handler_resumes_input_after_continuation() {
    let (mut host, _host_id, _sock) = TestHost::start();
    let (child_pid, mut master) = host.spawn(sh(
        "trap 'echo continued' CONT; echo ready; while :; do if read line; then echo processed:$line; fi; done",
    ));
    read_until(&mut master, "ready");
    let pid = super::super::kernel_pid(child_pid);
    kill(pid, Some(Signal::SIGSTOP)).unwrap();
    assert!(
        matches!(host.recv(), HostToServerMsg::ChildStopped { signal } if signal == libc::SIGSTOP)
    );
    kill(pid, Some(Signal::SIGCONT)).unwrap();
    assert!(matches!(host.recv(), HostToServerMsg::ChildContinued));
    // A shell can defer its CONT trap until the blocking read completes. Send input before
    // waiting for the trap's output so the test does not keep that read blocked itself.
    master.write_all(b"after-resume\n").unwrap();
    let output = read_until(&mut master, "processed:after-resume");
    assert!(
        output.contains("continued"),
        "CONT trap did not run: {output:?}"
    );
    assert!(
        output.contains("processed:after-resume"),
        "resumed child did not process input: {output:?}"
    );
    host.send(&ServerToHostMsg::Terminate);
    host.join();
}

/// The waiter polls while the child is stopped and waits blocking while it runs,
/// so a second suspend has to find its way back onto the blocking wait. If the
/// switch were one-way the pane would report its first `^Z` and no other.
#[test]
fn a_child_that_stops_again_after_continuing_is_reported_again() {
    let (mut host, _host_id, _sock) = TestHost::start();
    let (child_pid, mut master) = host.spawn(sh("echo ready; exec sleep 30"));
    read_until(&mut master, "ready");
    wait_for_exec_into(child_pid, "sleep");
    let pid = Pid::from_raw(i32::try_from(child_pid).unwrap());

    for round in 0..2 {
        kill(pid, Some(Signal::SIGSTOP)).expect("could not stop the child");
        match host.recv() {
            HostToServerMsg::ChildStopped { .. } => {},
            other => panic!("round {round}: expected ChildStopped, got {other:?}"),
        }
        kill(pid, Some(Signal::SIGCONT)).expect("could not continue the child");
        match host.recv() {
            HostToServerMsg::ChildContinued => {},
            other => panic!("round {round}: expected ChildContinued, got {other:?}"),
        }
    }

    host.send(&ServerToHostMsg::Terminate);
    host.join();
}

/// The exit has to come back from the polled branch too. A waiter that only
/// noticed exits while blocking would sit on a killed-while-suspended child
/// forever, and the pane would never close.
#[test]
fn a_child_killed_while_stopped_still_reports_its_exit() {
    let (mut host, _host_id, _sock) = TestHost::start();
    let (child_pid, mut master) = host.spawn(sh("echo ready; exec sleep 30"));
    read_until(&mut master, "ready");
    wait_for_exec_into(child_pid, "sleep");
    let pid = Pid::from_raw(i32::try_from(child_pid).unwrap());

    kill(pid, Some(Signal::SIGSTOP)).expect("could not stop the child");
    match host.recv() {
        HostToServerMsg::ChildStopped { .. } => {},
        other => panic!("expected ChildStopped, got {other:?}"),
    }

    // Straight to SIGKILL with no SIGCONT in between, so the waiter is still in
    // its polling branch when the child dies.
    kill(pid, Some(Signal::SIGKILL)).expect("could not kill the child");
    match host.recv() {
        HostToServerMsg::ChildExited { status } => assert_eq!(status, None),
        other => panic!("expected ChildExited, got {other:?}"),
    }

    host.send(&ServerToHostMsg::Terminate);
    host.join();
}

#[test]
fn a_child_exiting_on_continue_reports_its_exit_code() {
    let (mut host, _host_id, _sock) = TestHost::start();
    let (child_pid, _master) = host.spawn(sh("kill -STOP $$; exit 7"));
    assert!(matches!(host.recv(), HostToServerMsg::ChildStopped { .. }));
    kill(super::super::kernel_pid(child_pid), Some(Signal::SIGCONT)).unwrap();
    loop {
        match host.recv() {
            HostToServerMsg::ChildContinued => {},
            HostToServerMsg::ChildExited { status: Some(7) } => break,
            other => panic!("expected continue or exit status 7, got {other:?}"),
        }
    }
    host.send(&ServerToHostMsg::Terminate);
    host.join();
}

/// A stop is not an exit. `Terminate` waits for the child to really be gone, and
/// mistaking a suspend for that would leave the pane's process running while the
/// supervisor walked away from it.
#[test]
fn terminating_a_stopped_child_still_waits_for_its_real_exit() {
    let (mut host, _host_id, _sock) = TestHost::start();
    let (child_pid, mut master) = host.spawn(sh("echo ready; exec sleep 30"));
    read_until(&mut master, "ready");

    kill(
        Pid::from_raw(i32::try_from(child_pid).unwrap()),
        Some(Signal::SIGSTOP),
    )
    .expect("could not stop the child");
    match host.recv() {
        HostToServerMsg::ChildStopped { .. } => {},
        other => panic!("expected ChildStopped, got {other:?}"),
    }

    host.send(&ServerToHostMsg::Terminate);
    host.join();
    wait_until(
        || !process_alive(child_pid),
        "the stopped child to be reaped by Terminate",
    );
}

fn conditional_terminate(socket: &std::path::Path, session: &str) -> HostToServerMsg {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream.set_read_timeout(Some(TEST_TIMEOUT)).unwrap();
    write_msg(
        &mut stream,
        &ServerToHostMsg::TerminateDetached {
            expected_session: session.to_owned().into(),
        },
    )
    .unwrap();
    read_msg(&mut stream).unwrap().unwrap()
}

#[test]
fn conditional_termination_checks_current_ownership_and_confirms_child_teardown() {
    let (mut host, _, socket) = TestHost::start();
    let socket = socket.unwrap();
    let (pid, _master) = host.spawn(sh("exec sleep 60"));
    assert!(matches!(
        conditional_terminate(&socket, "pty-host-tests"),
        HostToServerMsg::TerminationComplete {
            terminated: false,
            error: None
        }
    ));
    host.send(&ServerToHostMsg::Release);
    wait_until(|| !query_status(&socket).attached, "detachment");
    assert!(matches!(
        conditional_terminate(&socket, "unrelated"),
        HostToServerMsg::TerminationComplete {
            terminated: false,
            error: None
        }
    ));
    assert!(process_alive(pid));
    crate::pty_host::client::relabel_via_socket(
        &socket,
        &"pty-host-tests".into(),
        &"renamed".into(),
        true,
    )
    .unwrap();
    assert!(matches!(
        conditional_terminate(&socket, "pty-host-tests"),
        HostToServerMsg::TerminationComplete {
            terminated: false,
            error: None
        }
    ));
    assert!(matches!(
        conditional_terminate(&socket, "renamed"),
        HostToServerMsg::TerminationComplete {
            terminated: true,
            error: None
        }
    ));
    assert!(!process_alive(pid), "completion preceded child teardown");
    host.join();
}

#[test]
fn adoption_before_conditional_termination_keeps_the_pane_alive() {
    let (mut host, _, socket) = TestHost::start();
    let socket = socket.unwrap();
    let (pid, _master) = host.spawn(sh("exec sleep 60"));
    host.send(&ServerToHostMsg::Release);
    wait_until(|| !query_status(&socket).attached, "detachment");
    let mut adopted = UnixStream::connect(&socket).unwrap();
    write_msg(
        &mut adopted,
        &ServerToHostMsg::Attach {
            protocol_version: PROTOCOL_VERSION,
            yinz_version: VERSION.to_owned(),
            terminal_id: 3,
            session_name: Some("pty-host-tests".into()),
        },
    )
    .unwrap();
    assert!(matches!(
        read_msg::<HostToServerMsg, _>(&mut adopted).unwrap(),
        Some(HostToServerMsg::AttachAck { .. })
    ));
    let _adopted_master = recv_fd(&adopted).unwrap();
    assert!(matches!(
        conditional_terminate(&socket, "pty-host-tests"),
        HostToServerMsg::TerminationComplete {
            terminated: false,
            error: None
        }
    ));
    assert!(process_alive(pid));
    write_msg(&mut adopted, &ServerToHostMsg::Terminate).unwrap();
    host.join();
}

#[test]
fn agent_theme_retention_clears_on_end_and_dedicated_child_exit() {
    use yinz_types::agent::theme::*;
    for teardown in [false, true] {
        let (mut host, _, socket) = TestHost::start();
        let socket = socket.unwrap();
        let (_, mut master) = host.spawn(sh("read -r finish; printf done"));
        let snapshot = AgentThemeSnapshot {
            version: SNAPSHOT_VERSION,
            provider: "fixture".into(),
            provider_version: "1".into(),
            theme: "test".into(),
            provenance: vec![],
            availability: Availability::Available,
            definitions: vec![],
            collisions: vec![],
        };
        let report = |snapshot| {
            let mut stream = UnixStream::connect(&socket).unwrap();
            stream.set_read_timeout(Some(TEST_TIMEOUT)).unwrap();
            write_msg(
                &mut stream,
                &ServerToHostMsg::AgentTheme(AgentThemeEvent {
                    version: EVENT_VERSION,
                    launch_id: "run".into(),
                    snapshot,
                }),
            )
            .unwrap();
            assert!(matches!(
                read_msg::<HostToServerMsg, _>(&mut stream).unwrap(),
                Some(HostToServerMsg::Status(_))
            ));
        };
        report(Some(snapshot.clone()));
        if teardown {
            master.write_all(b"finish\n").unwrap();
            // macOS waits for pending terminal output to drain before closing the slave.
            read_until(&mut master, "done");
            assert!(matches!(host.recv(), HostToServerMsg::ChildExited { .. }));
        } else {
            report(None);
        }
        // Late observations cannot resurrect an ended or exited launch.
        host.send(&ServerToHostMsg::AgentTheme(AgentThemeEvent {
            version: EVENT_VERSION,
            launch_id: "run".into(),
            snapshot: Some(snapshot),
        }));
        host.send(&ServerToHostMsg::Release);
        wait_until(
            || !query_status(&socket).attached,
            "detachment after late palette update",
        );
        let (reply, adopted) =
            attach_over_socket(&socket, yinz_types::internal_ids::TerminalId::new(42), None);
        match reply {
            HostToServerMsg::AttachAck { cmd, .. } => assert!(cmd.agent_theme.is_none()),
            other => panic!("unexpected adoption: {other:?}"),
        }
        drop(adopted);
        drop(master);
        crate::pty_host::client::terminate_via_socket(&socket).unwrap();
        host.join();
    }
}
