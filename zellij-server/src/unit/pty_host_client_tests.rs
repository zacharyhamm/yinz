//! Tests for the server's half of the pty-host relationship.
//!
//! Almost none of this needs a supervisor process. `handshake_and_spawn` takes an
//! already-connected `UnixStream`, so a socketpair with the supervisor's replies
//! already written into it drives the whole server side deterministically, with
//! no threads and nothing to race. The handshake is a fixed script — `Hello`,
//! `HelloAck`, `Spawn`, `Spawned` + descriptor — so pre-loading the replies is
//! equivalent to a live peer, including for the `SCM_RIGHTS` handoff: the byte
//! carrying the descriptor sits in the stream immediately after the `Spawned`
//! frame, and `read_msg`'s exact-sized reads leave it there for `recv_fd`.

use super::*;

use crate::pty_host::fdpass::send_fd;
use crate::pty_host::protocol::{self, HostStatus};
use nix::pty::openpty;
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixListener;
use std::os::unix::process::ExitStatusExt;
use std::sync::Mutex;
use std::thread::JoinHandle;

const TEST_TIMEOUT: Duration = Duration::from_secs(10);

fn sh(script: &str) -> RunCommand {
    RunCommand {
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

/// A long-lived stand-in for a supervisor process, for the paths that care about
/// the pid rather than the protocol.
///
/// `sleep` directly rather than through a shell: a `sh -c` that forks instead of
/// exec-ing leaves a grandchild that a SIGKILL aimed at the pid we know about
/// does not reach, and it would outlive the test run.
fn sleeping_child() -> Child {
    Command::new("sleep")
        .arg("60")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn a stand-in supervisor")
}

/// Already dead by the time it is used, so `reap_host` finishes instantly on the
/// paths that are not about reaping.
fn finished_child() -> Child {
    Command::new("true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn a stand-in supervisor")
}

fn params<'a>(host_bin: &'a Path, cmd: &RunCommand) -> SpawnParams<'a> {
    SpawnParams {
        host_bin,
        terminal_id: 7,
        session_name: Some("client-tests".to_owned()),
        cmd: cmd.clone(),
        failover_cmd: None,
        termios: None,
        initial_size: Some(WireWinsize {
            cols: 100,
            rows: 30,
            width_in_pixels: 0,
            height_in_pixels: 0,
        }),
        arg0: None,
        env: vec![],
        job_control: true,
        login_session: false,
    }
}

/// A control channel with the supervisor's side of the conversation already in
/// it. The returned host end stays open so the client sees a live peer rather
/// than an immediate EOF.
fn preloaded(replies: &[HostToServerMsg]) -> (UnixStream, UnixStream) {
    let (server_end, mut host_end) = UnixStream::pair().expect("socketpair failed");
    for reply in replies {
        write_msg(&mut host_end, reply).expect("failed to preload a reply");
    }
    (server_end, host_end)
}

fn hello_ack() -> HostToServerMsg {
    HostToServerMsg::HelloAck {
        protocol_version: PROTOCOL_VERSION,
        zellij_version: VERSION.to_owned(),
        host_id: "abcdef012345".to_owned(),
        host_pid: 4242,
        control_socket: Some(PathBuf::from("/tmp/zellij-pty-host-tests.sock")),
    }
}

// ---- the handshake ----

/// The happy path, end to end over the control channel: the server has to come
/// away with the supervisor's identity and a descriptor that really is the pty
/// the supervisor opened.
#[test]
fn the_handshake_hands_the_server_a_working_pty_master() {
    let pty = openpty(None, &None).expect("openpty failed");
    let (server_end, host_end) =
        preloaded(&[hello_ack(), HostToServerMsg::Spawned { child_pid: 991 }]);
    send_fd(&host_end, pty.master.as_fd().as_raw_fd()).expect("send_fd failed");

    let cmd = sh("echo hi");
    let (child_pid, master, host_id, host_pid, control_socket) =
        handshake_and_spawn(&server_end, &params(Path::new("/nonexistent"), &cmd))
            .expect("the handshake should have succeeded");

    assert_eq!(child_pid, 991);
    assert_eq!(host_id, "abcdef012345");
    assert_eq!(host_pid, 4242);
    assert_eq!(
        control_socket,
        Some(PathBuf::from("/tmp/zellij-pty-host-tests.sock"))
    );

    // Writing to what we were handed must show up on the supervisor's slave,
    // which is the only way to know it is the same pty and not some other fd.
    let mut master = std::fs::File::from(master);
    master.write_all(b"handed\n").expect("write failed");
    let mut slave = std::fs::File::from(pty.slave);
    let mut buf = [0u8; 7];
    slave.read_exact(&mut buf).expect("read from slave failed");
    assert_eq!(&buf, b"handed\n");

    // And the supervisor must have been told what to spawn.
    let mut host_end = host_end;
    match read_msg::<ServerToHostMsg, _>(&mut host_end).expect("read failed") {
        Some(ServerToHostMsg::Hello {
            protocol_version, ..
        }) => assert_eq!(protocol_version, PROTOCOL_VERSION),
        other => panic!("expected Hello, got {other:?}"),
    }
    match read_msg::<ServerToHostMsg, _>(&mut host_end).expect("read failed") {
        Some(ServerToHostMsg::Spawn {
            terminal_id,
            session_name,
            cmd: sent,
            initial_size,
            ..
        }) => {
            assert_eq!(terminal_id, 7);
            assert_eq!(session_name.as_deref(), Some("client-tests"));
            assert_eq!(sent, cmd);
            assert_eq!(initial_size.map(|s| (s.cols, s.rows)), Some((100, 30)));
        },
        other => panic!("expected Spawn, got {other:?}"),
    }
}

/// `current_exe()` re-execs whatever is on disk *now*, so a package upgrade
/// mid-session lands a new server in front of an old supervisor. That must cost
/// the pane its detachability, not its existence — which is why the caller
/// branches on the error *not* being a `ZellijError`.
#[test]
fn a_host_speaking_a_different_protocol_version_is_a_fallback_not_a_pane_failure() {
    let (server_end, _host_end) = preloaded(&[HostToServerMsg::HelloAck {
        protocol_version: PROTOCOL_VERSION + 1,
        zellij_version: "99.0.0".to_owned(),
        host_id: "abcdef012345".to_owned(),
        host_pid: 4242,
        control_socket: None,
    }]);

    let cmd = sh("true");
    let err = handshake_and_spawn(&server_end, &params(Path::new("/nonexistent"), &cmd))
        .expect_err("a version mismatch should be refused");

    assert!(
        err.downcast_ref::<ZellijError>().is_none(),
        "a version mismatch must not surface as a ZellijError, or the pty thread \
         will fail the pane instead of falling back to an in-process pty: {err:#}"
    );
    let message = format!("{err:#}");
    assert!(
        message.contains(&format!("v{}", PROTOCOL_VERSION + 1))
            && message.contains(&format!("v{PROTOCOL_VERSION}")),
        "the error should name both versions: {message}"
    );
}

/// The one failure that is *not* a fallback. Eight sites in `pty.rs` downcast for
/// this exact error, and the values have to be the server's own — the supervisor
/// only reports that the command was missing, not which pane wanted it.
#[test]
fn a_host_that_cannot_find_the_command_produces_the_error_the_pty_thread_downcasts_for() {
    let (server_end, _host_end) = preloaded(&[
        hello_ack(),
        HostToServerMsg::SpawnFailed {
            kind: SpawnFailureKind::CommandNotFound,
            message: "no such file".to_owned(),
        },
    ]);

    let cmd = sh("true");
    let err = handshake_and_spawn(&server_end, &params(Path::new("/nonexistent"), &cmd))
        .expect_err("a missing command should be refused");

    match err.downcast_ref::<ZellijError>() {
        Some(ZellijError::CommandNotFound {
            terminal_id,
            command,
        }) => {
            assert_eq!(*terminal_id, 7, "the pane's own id should be reported");
            assert_eq!(command, "/bin/sh");
        },
        other => panic!("expected a ZellijError::CommandNotFound, got {other:?}: {err:#}"),
    }
}

#[test]
fn a_host_that_fails_to_spawn_for_any_other_reason_is_a_fallback() {
    let (server_end, _host_end) = preloaded(&[
        hello_ack(),
        HostToServerMsg::SpawnFailed {
            kind: SpawnFailureKind::Other,
            message: "openpty ran out of ptys".to_owned(),
        },
    ]);

    let cmd = sh("true");
    let err = handshake_and_spawn(&server_end, &params(Path::new("/nonexistent"), &cmd))
        .expect_err("a failed spawn should be refused");

    assert!(
        err.downcast_ref::<ZellijError>().is_none(),
        "a generic spawn failure must stay a fallback: {err:#}"
    );
    assert!(
        format!("{err:#}").contains("openpty ran out of ptys"),
        "the supervisor's explanation should survive: {err:#}"
    );
}

/// The handshake is positional — there are no request ids — so a message
/// arriving out of order has to be reported rather than read as the message that
/// was expected in that slot.
#[test]
fn a_host_that_does_not_follow_the_handshake_is_refused() {
    let cmd = sh("true");
    let host_bin = Path::new("/nonexistent");

    // A first message that is not a HelloAck.
    let (server_end, _host_end) = preloaded(&[HostToServerMsg::Spawned { child_pid: 1 }]);
    let err = handshake_and_spawn(&server_end, &params(host_bin, &cmd))
        .expect_err("a missing HelloAck should be refused");
    assert!(
        format!("{err:#}").contains("expected HelloAck"),
        "unexpected error: {err:#}"
    );

    // A second message that is not a spawn result.
    let (server_end, _host_end) = preloaded(&[hello_ack(), hello_ack()]);
    let err = handshake_and_spawn(&server_end, &params(host_bin, &cmd))
        .expect_err("a missing spawn result should be refused");
    assert!(
        format!("{err:#}").contains("expected Spawned"),
        "unexpected error: {err:#}"
    );
}

/// A supervisor that dies mid-handshake must be an error rather than a hang; the
/// pty thread is blocked on this call.
#[test]
fn a_host_that_dies_during_the_handshake_is_an_error_not_a_hang() {
    let cmd = sh("true");
    let host_bin = Path::new("/nonexistent");

    // Shutting down only the supervisor's write half — rather than dropping its
    // end outright — is what a supervisor that exits after reading looks like:
    // our `Hello` still lands, and the reply never comes.
    let (server_end, host_end) = preloaded(&[]);
    host_end.shutdown(Shutdown::Write).expect("shutdown failed");
    let err = handshake_and_spawn(&server_end, &params(host_bin, &cmd))
        .expect_err("a supervisor that hung up should be refused");
    assert!(
        format!("{err:#}").contains("closed the control channel"),
        "unexpected error: {err:#}"
    );

    // And one that is gone before we can even greet it.
    let (server_end, host_end) = preloaded(&[]);
    drop(host_end);
    handshake_and_spawn(&server_end, &params(host_bin, &cmd))
        .expect_err("a supervisor that is already gone should be refused");
}

/// The reason `HANDSHAKE_TIMEOUT` exists: `spawn_terminal` runs synchronously on
/// the pty thread, which also services pane close and tab close. A supervisor
/// that accepts the connection and then says nothing must not take the session
/// with it.
#[test]
fn a_host_that_never_replies_gives_up_rather_than_wedging_the_pty_thread() {
    // Nothing preloaded, and the host end deliberately kept alive, so the read
    // blocks rather than seeing EOF.
    let (server_end, _host_end) = preloaded(&[]);

    let cmd = sh("true");
    let started = Instant::now();
    let err = handshake_and_spawn(&server_end, &params(Path::new("/nonexistent"), &cmd))
        .expect_err("a silent supervisor should time out");
    let elapsed = started.elapsed();

    assert!(
        elapsed >= HANDSHAKE_TIMEOUT,
        "gave up after {elapsed:?}, before the {HANDSHAKE_TIMEOUT:?} timeout: {err:#}"
    );
    assert!(
        elapsed < HANDSHAKE_TIMEOUT * 3,
        "took {elapsed:?} to give up on a {HANDSHAKE_TIMEOUT:?} timeout"
    );
}

// ---- reporting a pane's exit exactly once ----

type Reports = Arc<Mutex<Vec<(PaneId, Option<i32>)>>>;

fn recording_quit_cb(reports: &Reports) -> QuitCb {
    let reports = Arc::clone(reports);
    Box::new(move |pane_id, status, _cmd| {
        reports
            .lock()
            .expect("reports mutex poisoned")
            .push((pane_id, status));
    })
}

/// Builds the state `spawn` would have produced, without needing a supervisor
/// that speaks the protocol: only the control channel and the child pid matter
/// to the lifecycle paths under test.
fn pending_host(control: UnixStream, host_child: Child) -> PendingHost {
    let reader_stream = control.try_clone().expect("try_clone failed");
    let host_pid = host_child.id();
    PendingHost {
        control: Some(control),
        reader_stream: Some(reader_stream),
        host_child: Some(host_child),
        host_id: "abcdef012345".to_owned(),
        host_pid,
        control_socket: None,
        // Any descriptor will do — nothing under test reads it, it just has to be
        // something `SpawnedHost` can own.
        master: Some(OwnedFd::from(
            std::fs::File::open("/dev/null").expect("failed to open /dev/null"),
        )),
        child_pid: 991,
        terminal_id: 7,
        cmd: sh("true"),
    }
}

fn reported(reports: &Reports) -> Vec<(PaneId, Option<i32>)> {
    reports.lock().expect("reports mutex poisoned").clone()
}

/// The central invariant of the whole file: three separate paths can decide a
/// pane is finished — a `ChildExited` message, the supervisor's control channel
/// closing, and `terminate()` — and `quit_cb` must fire exactly once across all
/// of them. Firing twice closes whatever pane happens to hold that id next.
#[test]
fn a_pane_is_reported_closed_exactly_once_even_if_its_supervisor_then_dies() {
    let (server_end, mut host_end) = UnixStream::pair().expect("socketpair failed");
    let reports: Reports = Arc::default();
    let _spawned =
        pending_host(server_end, finished_child()).finish(recording_quit_cb(&reports), None);

    write_msg(
        &mut host_end,
        &HostToServerMsg::ChildExited { status: Some(5) },
    )
    .expect("failed to report the child's exit");
    wait_until(|| !reported(&reports).is_empty(), "the pane to be reported");

    // Now the supervisor goes away, which is the second path into `quit_cb`.
    drop(host_end);
    std::thread::sleep(Duration::from_millis(200));

    assert_eq!(
        reported(&reports),
        vec![(PaneId::Terminal(7), Some(5))],
        "the pane should have been reported closed exactly once, with its real status"
    );
}

/// Without this a supervisor that is `kill -9`'d leaves a pane that looks alive
/// forever: its child is unsupervised, so nobody will ever `waitpid` it and no
/// exit status is coming.
#[test]
fn a_supervisor_that_dies_without_reporting_still_closes_its_pane() {
    let (server_end, host_end) = UnixStream::pair().expect("socketpair failed");
    let reports: Reports = Arc::default();
    let _spawned =
        pending_host(server_end, finished_child()).finish(recording_quit_cb(&reports), None);

    drop(host_end);
    wait_until(
        || !reported(&reports).is_empty(),
        "the pane to be closed out",
    );
    std::thread::sleep(Duration::from_millis(200));

    assert_eq!(
        reported(&reports),
        vec![(PaneId::Terminal(7), None)],
        "a supervisor that died silently should close the pane with an unknown status"
    );
}

/// `terminate()` pre-sets the reported flag because the pane is already being
/// torn down by the caller. Without that the reader thread would synthesize a
/// second close on its way out.
#[test]
fn terminating_a_pane_does_not_also_synthesize_a_close() {
    let (server_end, host_end) = UnixStream::pair().expect("socketpair failed");
    let reports: Reports = Arc::default();
    let mut spawned =
        pending_host(server_end, finished_child()).finish(recording_quit_cb(&reports), None);

    spawned.handle.terminate();
    drop(host_end);
    std::thread::sleep(Duration::from_millis(300));

    assert!(
        reported(&reports).is_empty(),
        "terminate() should suppress the close it caused, but got {:?}",
        reported(&reports)
    );
}

/// The path taken whenever `spawn` succeeds but the caller then falls back:
/// nothing has committed to the supervisor yet, so it must not be left running.
#[test]
fn dropping_a_pending_host_without_finishing_it_kills_the_supervisor() {
    let (server_end, _host_end) = UnixStream::pair().expect("socketpair failed");
    let child = sleeping_child();
    let host_pid = child.id();

    let pending = pending_host(server_end, child);
    assert!(
        process_alive(host_pid),
        "the stand-in supervisor should be up"
    );
    drop(pending);

    assert!(
        !process_alive(host_pid),
        "dropping an unfinished PendingHost left supervisor {host_pid} running"
    );
}

/// The converse, and the reason `finish` takes `self` by value: once the caller
/// has committed, the supervisor belongs to the reader thread and dropping the
/// husk must not touch it.
#[test]
fn finishing_a_pending_host_leaves_its_supervisor_alone() {
    let (server_end, _host_end) = UnixStream::pair().expect("socketpair failed");
    let child = sleeping_child();
    let host_pid = child.id();

    let reports: Reports = Arc::default();
    let mut spawned = pending_host(server_end, child).finish(recording_quit_cb(&reports), None);
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        process_alive(host_pid),
        "finishing a PendingHost killed supervisor {host_pid}"
    );

    // The stand-in does not speak the protocol, so `Terminate` lands in a socket
    // nobody reads and it would sit here for the full `HOST_EXIT_GRACE` before
    // the reader thread killed it. Kill it outright; the `shutdown` inside
    // `terminate` is what wakes the reader thread up to reap it.
    spawned.handle.terminate();
    let _ = kill(Pid::from_raw(host_pid as i32), Some(Signal::SIGKILL));
    wait_until(
        || !process_alive(host_pid),
        "the stand-in supervisor to be reaped",
    );
}

/// `terminate()` is a blocking write precisely so that "the server asked me to
/// stop" is distinguishable from "the server crashed". When even that cannot be
/// delivered there is nothing left to be polite with.
#[test]
fn a_supervisor_that_will_not_take_a_message_is_killed() {
    let (server_end, _host_end) = UnixStream::pair().expect("socketpair failed");
    let mut child = sleeping_child();
    let host_pid = child.id();

    // Shutting down our own write half makes the write fail without depending on
    // the peer's timing.
    server_end
        .shutdown(Shutdown::Write)
        .expect("shutdown failed");

    let mut handle = PtyHostHandle {
        host_id: "abcdef012345".to_owned(),
        host_pid,
        control_socket: None,
        control: server_end,
        reported: Arc::new(AtomicBool::new(false)),
        released: Arc::new(AtomicBool::new(false)),
    };
    handle.terminate();

    let status = child.wait().expect("wait failed");
    assert_eq!(
        status.signal(),
        Some(Signal::SIGKILL as i32),
        "a supervisor that could not be told to stop should have been killed, got {status:?}"
    );
}

/// A supervisor that will not wind itself down still has to be reaped, or the
/// server accumulates zombies for the lifetime of the session.
#[test]
fn a_supervisor_that_will_not_exit_is_killed_and_reaped() {
    let child = sleeping_child();
    let host_pid = child.id();

    let started = Instant::now();
    reap_host(HostProcess::Spawned(child), host_pid, "abcdef012345");
    let elapsed = started.elapsed();

    assert!(
        elapsed >= HOST_EXIT_GRACE,
        "reaped after {elapsed:?}, before the {HOST_EXIT_GRACE:?} grace period expired"
    );
    assert!(
        !process_alive(host_pid),
        "supervisor {host_pid} survived reaping"
    );
}

#[test]
fn a_supervisor_that_has_already_exited_is_reaped_immediately() {
    let child = finished_child();
    let host_pid = child.id();

    let started = Instant::now();
    reap_host(HostProcess::Spawned(child), host_pid, "abcdef012345");
    assert!(
        started.elapsed() < HOST_EXIT_GRACE,
        "reaping an already-dead supervisor should not wait out the grace period"
    );
}

// ---- the named socket ----

/// Accepts one connection, reads one request and replies with whatever the test
/// scripted, then hands the request back so the test can assert on it.
fn fake_supervisor_socket(
    path: &Path,
    reply: Option<HostToServerMsg>,
) -> JoinHandle<Option<ServerToHostMsg>> {
    let listener = UnixListener::bind(path).expect("failed to bind the fake supervisor socket");
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept failed");
        let request = read_msg::<ServerToHostMsg, _>(&mut stream).expect("read failed");
        if let Some(reply) = reply {
            write_msg(&mut stream, &reply).expect("failed to reply");
        }
        request
    })
}

fn a_status() -> HostStatus {
    HostStatus {
        host_id: "abcdef012345".to_owned(),
        session_name: Some("client-tests".to_owned()),
        terminal_id: 7,
        command: "/bin/sh -c 'sleep 60'".to_owned(),
        cwd: Some(PathBuf::from("/tmp")),
        child_pid: Some(991),
        host_pid: 4242,
        started_at_unix: 1_700_000_000,
        attached: false,
        child_exit_status: None,
        buffered_bytes: 4096,
    }
}

/// The classification that decides whether `pty-host list` unlinks a socket. A
/// stale file left by a `SIGKILL`'d supervisor has to be collectable, and a live
/// one must never be — unlinking it would orphan a pane beyond reach of `kill`.
#[test]
fn a_socket_whose_listener_is_gone_is_dead_and_a_live_one_is_not() {
    let dir = tempfile::tempdir().expect("failed to create a temp dir");
    let path = dir.path().join("h.sock");
    protocol::check_socket_path_length(&path, 0)
        .expect("the temp dir is too deep for a unix socket path");

    assert!(
        socket_is_dead(&path),
        "a path that was never bound should be dead"
    );

    let listener = UnixListener::bind(&path).expect("bind failed");
    assert!(
        !socket_is_dead(&path),
        "a socket with a live listener must never be collected"
    );

    // Exactly the residue a SIGKILL'd supervisor leaves: the file is still there
    // but nothing is listening.
    drop(listener);
    assert!(
        path.exists(),
        "dropping a listener should leave the socket file behind"
    );
    // Retried rather than asserted outright: other tests in this binary fork, and
    // a child that has forked but not yet exec'd holds a dup of every open
    // descriptor, this listener included. That keeps the socket connectable for
    // the microseconds until the exec, which has nothing to do with the rule
    // under test.
    wait_until(
        || socket_is_dead(&path),
        "the abandoned socket to be reported dead",
    );
}

/// A listener that is bound but never accepts still owns a live pane. Only a
/// refused or missing connection counts as dead — anything else and a busy
/// supervisor would have its socket unlinked out from under it.
#[test]
fn a_listener_that_never_accepts_is_not_treated_as_dead() {
    let dir = tempfile::tempdir().expect("failed to create a temp dir");
    let path = dir.path().join("h.sock");
    let _listener = UnixListener::bind(&path).expect("bind failed");

    assert!(
        !socket_is_dead(&path),
        "a supervisor that is merely busy must not be collected"
    );
}

#[test]
fn query_returns_the_status_the_supervisor_reports() {
    let dir = tempfile::tempdir().expect("failed to create a temp dir");
    let path = dir.path().join("h.sock");
    let host = fake_supervisor_socket(&path, Some(HostToServerMsg::Status(a_status())));

    let status = query(&path).expect("query should have succeeded");
    assert_eq!(status.host_id, "abcdef012345");
    assert_eq!(status.terminal_id, 7);
    assert_eq!(status.child_pid, Some(991));
    assert_eq!(status.buffered_bytes, 4096);
    assert!(!status.attached);

    assert!(
        matches!(
            host.join().expect("host thread panicked"),
            Some(ServerToHostMsg::Query)
        ),
        "query should have sent a Query"
    );
}

#[test]
fn query_rejects_a_reply_that_is_not_a_status() {
    let dir = tempfile::tempdir().expect("failed to create a temp dir");

    let wrong = dir.path().join("a.sock");
    let host = fake_supervisor_socket(&wrong, Some(HostToServerMsg::ChildExited { status: None }));
    let err = query(&wrong).expect_err("a non-Status reply should be refused");
    assert!(
        format!("{err:#}").contains("Status"),
        "unexpected error: {err:#}"
    );
    let _ = host.join();

    let silent = dir.path().join("b.sock");
    let host = fake_supervisor_socket(&silent, None);
    query(&silent).expect_err("a supervisor that says nothing should be refused");
    let _ = host.join();
}

/// The only behavioural difference from `query` is which message goes out, so
/// that is what this asserts — a copy-paste of the wrong constant would be
/// invisible from the return value alone.
#[test]
fn terminate_via_socket_asks_the_supervisor_to_stop_and_returns_its_final_status() {
    let dir = tempfile::tempdir().expect("failed to create a temp dir");
    let path = dir.path().join("h.sock");
    let host = fake_supervisor_socket(&path, Some(HostToServerMsg::Status(a_status())));

    let status = terminate_via_socket(&path).expect("terminate should have succeeded");
    assert_eq!(status.host_id, "abcdef012345");

    assert!(
        matches!(
            host.join().expect("host thread panicked"),
            Some(ServerToHostMsg::Terminate)
        ),
        "terminate_via_socket should have sent a Terminate, not a Query"
    );
}

// ---- putting the control socket on fd 3, across a real exec ----

/// Runs `place_control_fd` in the real place it runs — between fork and exec —
/// and lets the exec'd program prove the descriptor arrived by writing to it.
fn exec_with_control_fd(
    script: &str,
    place: impl Fn(RawFd) -> std::io::Result<()> + Send + Sync + 'static,
) -> String {
    let (server_end, host_end) = UnixStream::pair().expect("socketpair failed");
    let host_raw = host_end.as_raw_fd();

    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(move || place(host_raw));
    }
    let mut child = command.spawn().expect("spawn failed");
    // The parent must not hold the supervisor's end open or the read below would
    // never see EOF.
    drop(host_end);
    child.wait().expect("wait failed");

    let mut got = String::new();
    (&server_end)
        .read_to_string(&mut got)
        .expect("read from the control channel failed");
    got
}

#[test]
fn placing_the_control_fd_moves_it_to_fd_three() {
    let got = exec_with_control_fd("printf moved >&3", place_control_fd);
    assert_eq!(
        got, "moved",
        "the exec'd process could not write to fd 3, so the control socket never arrived"
    );
}

/// `dup2(fd, fd)` is a no-op that does **not** clear `FD_CLOEXEC`, so a control
/// socket that already happens to sit on fd 3 would be closed by the exec and the
/// supervisor would block forever on a read. Reachable no other way: it needs the
/// descriptor to be exactly 3 at the moment `place_control_fd` runs.
#[test]
fn a_control_fd_that_is_already_fd_three_still_has_close_on_exec_cleared() {
    let got = exec_with_control_fd("printf stayed >&3", |raw| {
        // Put the socket on fd 3 ourselves, close-on-exec and all, so that
        // `place_control_fd` meets the no-op case.
        if unsafe { libc::dup2(raw, CONTROL_FD) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let flags = unsafe { libc::fcntl(CONTROL_FD, libc::F_GETFD) };
        if unsafe { libc::fcntl(CONTROL_FD, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        place_control_fd(CONTROL_FD)
    });
    assert_eq!(
        got, "stayed",
        "fd 3 was closed by the exec, so the no-op dup2 left FD_CLOEXEC set"
    );
}

// ---- a real supervisor process ----

/// `spawn` execs `<host_bin> --pty-host <label>`, and libtest would reject
/// `--pty-host` as an unknown flag. A two-line wrapper re-execs this same test
/// binary with a filter that selects the helper below instead.
///
/// `exec` rather than a plain call is load-bearing: it keeps the pid, and both
/// `reap_host`'s `try_wait` and `terminate()`'s SIGKILL fallback act on the pid
/// `Command::spawn` returned. `sh` does not close fd 3 across the exec, so the
/// control socket `place_control_fd` put there survives into the test binary.
fn write_host_wrapper(dir: &Path) -> PathBuf {
    let test_bin = std::env::current_exe().expect("failed to find the test binary");
    let path = dir.join("host.sh");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\n{REEXEC_ENV}=1 exec '{}' '{}' --exact\n",
            test_bin.display(),
            REEXEC_HELPER,
        ),
    )
    .expect("failed to write the wrapper");
    let mut perms = std::fs::metadata(&path)
        .expect("failed to stat the wrapper")
        .permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&path, perms).expect("failed to make the wrapper executable");
    path
}

const REEXEC_HELPER: &str =
    "pty_host::client::pty_host_client_tests::re_executed_as_a_supervisor_this_process_serves_one_pane";

const REEXEC_ENV: &str = "ZELLIJ_TEST_PTY_HOST_CHILD";

/// Not a test. This is where the wrapper script's `exec` lands, and it turns this
/// process into a real supervisor for one pane. Gated on an environment variable
/// the wrapper sets, so an ordinary run of the suite passes straight through
/// instead of blocking forever on fd 3.
#[test]
fn re_executed_as_a_supervisor_this_process_serves_one_pane() {
    if std::env::var_os(REEXEC_ENV).is_none() {
        return;
    }
    crate::pty_host::run_pty_host("test/reexec".to_owned());
}

/// Kills a supervisor if the test panics. Supervisors are designed to outlive
/// their parent, so without this a failing test would leak a real process onto
/// the machine permanently.
struct KillOnDrop(u32);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = kill(Pid::from_raw(self.0 as i32), Some(Signal::SIGKILL));
    }
}

/// The wrapper sets the marker itself rather than the test process exporting it:
/// `std::env::set_var` is not safe to call while other test threads are running,
/// and these tests are not serialized.
fn spawn_real_host(dir: &Path, cmd: RunCommand) -> Result<PendingHost> {
    let wrapper = write_host_wrapper(dir);
    let params = |cmd: RunCommand| SpawnParams {
        job_control: true,
        host_bin: &wrapper,
        terminal_id: 7,
        session_name: Some("client-tests".to_owned()),
        cmd,
        failover_cmd: None,
        termios: None,
        initial_size: Some(WireWinsize {
            cols: 80,
            rows: 24,
            width_in_pixels: 0,
            height_in_pixels: 0,
        }),
        arg0: None,
        env: vec![],
        login_session: false,
    };

    // `exec` can fail with `ETXTBSY` here for reasons that have nothing to do
    // with what is being tested: these tests run in parallel threads of one
    // process, and a `fork` in this thread duplicates every fd open in the
    // others — including a sibling's still-open handle on its own wrapper
    // script. The kernel then refuses to exec a file some process holds open for
    // writing. It clears as soon as the sibling's write completes, so retry.
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        match spawn(params(cmd.clone())) {
            Err(e) if Instant::now() < deadline && format!("{e:#}").contains("Text file busy") => {
                std::thread::sleep(Duration::from_millis(20));
            },
            result => return result,
        }
    }
}

/// The whole path across a real process boundary: fork, exec, `place_control_fd`,
/// the supervisor's own `control_stream_from_fd` validation, a cross-process
/// `SCM_RIGHTS` handoff, the reader thread, and finally `terminate()` and
/// `reap_host`. Everything else in this file stops at the socketpair.
#[test]
fn spawning_a_real_supervisor_yields_a_pty_the_server_can_read() {
    let dir = tempfile::tempdir().expect("failed to create a temp dir");
    let pending = spawn_real_host(dir.path(), sh("echo zellij-real-host-marker; exit 3"))
        .expect("spawning a real supervisor failed");
    let guard = KillOnDrop(pending.host_pid);

    let reports: Reports = Arc::default();
    let mut spawned = pending.finish(recording_quit_cb(&reports), None);
    assert!(spawned.child_pid > 0, "the pane's child should have a pid");

    let mut master = std::fs::File::from(
        spawned
            .master
            .try_clone()
            .expect("failed to clone the master"),
    );
    let mut seen = String::new();
    let start = Instant::now();
    let mut buf = [0u8; 1024];
    while !seen.contains("zellij-real-host-marker") {
        assert!(
            start.elapsed() < TEST_TIMEOUT,
            "never saw the marker in the pty output; saw {seen:?}"
        );
        match master.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => seen.push_str(&String::from_utf8_lossy(&buf[..n])),
            // The pty reports the slave closing as EIO rather than EOF on Linux.
            Err(e) if e.raw_os_error() == Some(libc::EIO) => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => panic!("read from the pty master failed: {e}"),
        }
    }

    wait_until(
        || !reported(&reports).is_empty(),
        "the pane's exit to be reported",
    );
    assert_eq!(
        reported(&reports),
        vec![(PaneId::Terminal(7), Some(3))],
        "the real supervisor should have reported the child's exit status"
    );

    spawned.handle.terminate();
    wait_until(
        || !process_alive(guard.0),
        "the real supervisor to shut down",
    );
}

/// The headline claim of the feature, asserted at the server's own API rather
/// than by driving `serve()` in-process: letting go of everything the server held
/// must leave the pane running. Contrast `PendingHost`, where a drop *does* kill
/// the supervisor — the difference is exactly what `finish` commits to.
///
/// This stops short of asserting the supervisor has noticed it is detached. The
/// reader thread holds a `try_clone` of the same control socket, so in-process it
/// stays open after the handle goes; only the server *process* exiting closes it.
/// The detach transition itself is covered against `serve()` by
/// `losing_the_server_detaches_rather_than_killing_the_child`.
#[test]
fn a_pane_survives_the_server_that_started_it() {
    let dir = tempfile::tempdir().expect("failed to create a temp dir");
    let pending =
        spawn_real_host(dir.path(), sh("sleep 60")).expect("spawning a real supervisor failed");
    let guard = KillOnDrop(pending.host_pid);
    let control_socket = pending
        .control_socket
        .clone()
        .expect("the supervisor should have bound a socket");

    let reports: Reports = Arc::default();
    let spawned = pending.finish(recording_quit_cb(&reports), None);
    let child_pid = spawned.child_pid;

    // The server goes away without terminating anything — a crash, or a `kill -9`.
    drop(spawned);
    std::thread::sleep(Duration::from_millis(300));

    assert!(
        process_alive(guard.0),
        "the supervisor died with the server that started it"
    );
    assert!(
        process_alive(child_pid),
        "the pane's child died with the server that started it"
    );

    let status = query(&control_socket).expect("the orphaned supervisor should still answer");
    assert_eq!(
        status.child_pid,
        Some(child_pid),
        "the surviving supervisor should still be reporting the pane it owns"
    );
    assert!(
        status.child_exit_status.is_none(),
        "the pane's child should still be running"
    );

    terminate_via_socket(&control_socket).expect("failed to terminate the orphaned supervisor");
    wait_until(
        || !process_alive(child_pid),
        "the orphaned pane to be killed",
    );
}

/// Read from a pty master until `needle` shows up, or fail the test.
///
/// A master handed over by a supervisor arrives non-blocking: `enter_detached`
/// sets `O_NONBLOCK` to drain into the ring, and that flag lives on the open file
/// description, which `SCM_RIGHTS` shares rather than copies. The server proper
/// does not care — `RawFdAsyncReader` sets the flag itself and drives the fd
/// through epoll — but a bare read here has to expect `EAGAIN` rather than block.
fn read_until(master: &mut std::fs::File, needle: &str) {
    let mut seen = String::new();
    let mut buf = [0u8; 1024];
    let start = Instant::now();
    while !seen.contains(needle) {
        assert!(
            start.elapsed() < TEST_TIMEOUT,
            "never saw {needle:?} in the pty output; saw {seen:?}"
        );
        match master.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => seen.push_str(&String::from_utf8_lossy(&buf[..n])),
            // The pty reports the slave closing as EIO rather than EOF on Linux.
            Err(e) if e.raw_os_error() == Some(libc::EIO) => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            },
            Err(e) => panic!("read from the pty master failed: {e}"),
        }
    }
    assert!(
        seen.contains(needle),
        "the pty closed before {needle:?} appeared; saw {seen:?}"
    );
}

// ---- releasing a pane instead of closing it ----

/// The reader thread holds a `try_clone` of the control socket, so removing the
/// handle closes nothing: `release()` has to `shutdown()` the socket itself. Get
/// this wrong and the supervisor never leaves `Attached`, and every attempt to
/// warp the pane is refused for a pane its owner has already let go of.
#[test]
fn releasing_a_pane_detaches_its_supervisor() {
    let dir = tempfile::tempdir().expect("failed to create a temp dir");
    let pending =
        spawn_real_host(dir.path(), sh("sleep 60")).expect("spawning a real supervisor failed");
    let guard = KillOnDrop(pending.host_pid);
    let control_socket = pending
        .control_socket
        .clone()
        .expect("the supervisor should have bound a socket");

    let reports: Reports = Arc::default();
    let spawned = pending.finish(recording_quit_cb(&reports), None);
    let child_pid = spawned.child_pid;
    assert!(
        query(&control_socket)
            .expect("the supervisor should answer")
            .attached,
        "the supervisor should start out attached"
    );

    let released = spawned.handle.release();
    assert_eq!(released.host_pid, guard.0);

    wait_until(
        || {
            query(&control_socket)
                .map(|status| !status.attached)
                .unwrap_or(false)
        },
        "the released supervisor to report itself detached",
    );
    assert!(
        process_alive(child_pid),
        "releasing a pane must not kill its child"
    );

    terminate_via_socket(&control_socket).expect("failed to terminate the released supervisor");
    wait_until(
        || !process_alive(child_pid),
        "the released pane to be killed",
    );
}

/// `reap_host` waits `HOST_EXIT_GRACE` for an exit and then `SIGKILL`s. That is
/// right for a pane being closed and catastrophic for one being handed over: the
/// warp would appear to work, and four seconds later the pane would die in the
/// session that had just adopted it.
#[test]
fn a_released_supervisor_is_not_reaped_or_killed() {
    let dir = tempfile::tempdir().expect("failed to create a temp dir");
    let pending =
        spawn_real_host(dir.path(), sh("sleep 60")).expect("spawning a real supervisor failed");
    let guard = KillOnDrop(pending.host_pid);
    let control_socket = pending
        .control_socket
        .clone()
        .expect("the supervisor should have bound a socket");

    let reports: Reports = Arc::default();
    let spawned = pending.finish(recording_quit_cb(&reports), None);
    let child_pid = spawned.child_pid;
    spawned.handle.release();

    // Past the deadline the reader thread would have used, with room to spare.
    std::thread::sleep(HOST_EXIT_GRACE + Duration::from_millis(500));

    assert!(
        process_alive(guard.0),
        "a released supervisor was reaped and killed; it has to outlive the server that let it go"
    );
    assert!(
        process_alive(child_pid),
        "the released pane's child was killed"
    );
    assert!(
        reported(&reports).is_empty(),
        "releasing a pane must not report it closed, got {:?}",
        reported(&reports)
    );

    terminate_via_socket(&control_socket).expect("failed to terminate the released supervisor");
    wait_until(
        || !process_alive(child_pid),
        "the released pane to be killed",
    );
}

/// The whole point, end to end across two real server-side lifetimes: one server
/// lets a pane go, another takes it over, and the child never notices.
#[test]
fn a_released_pane_can_be_attached_by_another_server() {
    let dir = tempfile::tempdir().expect("failed to create a temp dir");
    // Writes on demand rather than continuously, so the test can tell output
    // produced *during* the hand-off from output produced before or after it.
    let pending = spawn_real_host(
        dir.path(),
        sh("while read line; do echo \"got:$line\"; done"),
    )
    .expect("spawning a real supervisor failed");
    let guard = KillOnDrop(pending.host_pid);
    let control_socket = pending
        .control_socket
        .clone()
        .expect("the supervisor should have bound a socket");

    let first_reports: Reports = Arc::default();
    let spawned = pending.finish(recording_quit_cb(&first_reports), None);
    let child_pid = spawned.child_pid;
    let mut first_master = std::fs::File::from(
        spawned
            .master
            .try_clone()
            .expect("failed to clone the master"),
    );

    // Wait for the shell to actually be reading before letting go. Without this
    // the test races its own fixture: the write below would sit in the pty buffer
    // unprocessed, and the assertion on `buffered` would be checking whether a
    // shell had got round to starting rather than whether the hand-off works.
    first_master
        .write_all(b"before-the-gap\n")
        .expect("failed to write to the pty");
    read_until(&mut first_master, "got:before-the-gap");

    // The first server lets go. Anything the child writes from here until the
    // second server attaches lands in the supervisor's ring.
    spawned.handle.release();
    wait_until(
        || {
            query(&control_socket)
                .map(|status| !status.attached)
                .unwrap_or(false)
        },
        "the released supervisor to report itself detached",
    );
    first_master
        .write_all(b"during-the-gap\n")
        .expect("failed to write to the pty while detached");
    // The pty echoes input, so a non-zero buffer proves nothing on its own —
    // wait for the ring to be at least as big as the shell's reply will be.
    let expected = b"got:during-the-gap".len();
    wait_until(
        || {
            query(&control_socket)
                .map(|status| status.buffered_bytes >= expected)
                .unwrap_or(false)
        },
        "the detached supervisor to buffer the child's output",
    );
    drop(first_master);

    // A second server takes the pane over under a new id, as another session
    // would: ids are per-server, so 7 there and 42 here are the same pane.
    let pending = attach(AttachParams {
        control_socket: &control_socket,
        terminal_id: 42,
        session_name: Some("the-other-session".to_owned()),
    })
    .expect("attaching to the released supervisor failed");
    assert_eq!(
        pending.status().child_pid,
        Some(child_pid),
        "the adopted supervisor should still own the same child"
    );
    assert_eq!(
        pending.status().terminal_id,
        42,
        "the supervisor should have re-keyed itself to the adopting session's pane"
    );
    assert_eq!(
        pending.status().session_name.as_deref(),
        Some("the-other-session"),
        "the supervisor should report the session that owns the pane now"
    );
    assert!(
        pending.cmd().originating_plugin.is_none(),
        "a plugin id from the previous session must not follow the pane"
    );

    let second_reports: Reports = Arc::default();
    let attached = pending.finish(recording_quit_cb(&second_reports), None);
    assert_eq!(attached.child_pid, child_pid);
    assert!(
        String::from_utf8_lossy(&attached.buffered).contains("got:during-the-gap"),
        "output produced during the hand-off should arrive with the attach, got {:?}",
        String::from_utf8_lossy(&attached.buffered)
    );

    // And the new master is live, not just a corpse.
    let mut second_master =
        std::fs::File::from(attached.master.try_clone().expect("failed to clone"));
    second_master
        .write_all(b"after-the-warp\n")
        .expect("failed to write to the adopted pty");
    read_until(&mut second_master, "got:after-the-warp");

    assert!(
        reported(&first_reports).is_empty(),
        "the releasing server must not have been told its pane closed"
    );

    let mut handle = attached.handle;
    handle.terminate();
    wait_until(|| !process_alive(guard.0), "the adopted supervisor to exit");
}

/// A supervisor still held by a live server must not be handed to a second one:
/// both would read the same master and each would get half the pane's output.
#[test]
fn attaching_to_a_pane_someone_else_holds_is_refused() {
    let dir = tempfile::tempdir().expect("failed to create a temp dir");
    let pending =
        spawn_real_host(dir.path(), sh("sleep 60")).expect("spawning a real supervisor failed");
    let guard = KillOnDrop(pending.host_pid);
    let control_socket = pending
        .control_socket
        .clone()
        .expect("the supervisor should have bound a socket");

    let reports: Reports = Arc::default();
    let mut spawned = pending.finish(recording_quit_cb(&reports), None);

    let err = match attach(AttachParams {
        control_socket: &control_socket,
        terminal_id: 42,
        session_name: Some("the-other-session".to_owned()),
    }) {
        Ok(_) => panic!("attaching to an attached supervisor should have been refused"),
        Err(e) => e,
    };
    assert!(
        format!("{err:#}").contains("still attached"),
        "expected a refusal naming the live server, got {err:#}"
    );
    assert!(
        query(&control_socket)
            .expect("the supervisor should still answer")
            .attached,
        "a refused attach must leave the original server attached"
    );

    spawned.handle.terminate();
    wait_until(|| !process_alive(guard.0), "the supervisor to shut down");
}

/// A `host_bin` that is not a supervisor at all — the shape a botched upgrade or
/// a shadowed `zellij` on `$PATH` would take. It must fail the spawn and leave no
/// process behind rather than hanging.
/// `PendingHost` has no `Debug`, so the failure cases unwrap by hand rather than
/// through `expect_err`.
fn expect_spawn_failure(host_bin: &str, what: &str) -> anyhow::Error {
    let cmd = sh("true");
    match spawn(params(Path::new(host_bin), &cmd)) {
        Ok(_) => panic!("{what}"),
        Err(e) => e,
    }
}

#[test]
fn a_host_binary_that_says_nothing_fails_the_spawn() {
    // `/usr/bin/true` rather than `/bin/true`, which does not exist on macOS —
    // where this test therefore stopped being about "a real binary that is not a
    // supervisor" and quietly became a duplicate of the missing-binary one below.
    let err = expect_spawn_failure(
        "/usr/bin/true",
        "a binary that is not a supervisor should fail the spawn",
    );
    assert!(
        err.downcast_ref::<ZellijError>().is_none(),
        "this has to stay a fallback, not a pane failure: {err:#}"
    );
}

#[test]
fn a_host_binary_that_does_not_exist_fails_the_spawn() {
    let err = expect_spawn_failure(
        "/nonexistent/zellij",
        "a missing host binary should fail the spawn",
    );
    assert!(
        err.downcast_ref::<ZellijError>().is_none(),
        "this has to stay a fallback, not a pane failure: {err:#}"
    );
}
