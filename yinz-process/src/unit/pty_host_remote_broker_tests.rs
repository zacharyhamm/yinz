//! Protocol-level tests for the broker, driven in-process over a `socketpair`
//! the way `pty_host_tests.rs` drives `serve()`.
//!
//! Spawning a real pane needs a real yinz binary to re-exec as a supervisor,
//! so the full spawn/output/exit path belongs to the integration suite. What is
//! covered here is everything the broker decides *before* it needs one — which
//! is where the cross-machine failure modes live.

use super::*;
use yinz_types::internal_ids::ProcessId;

use std::io::Cursor;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;

use crate::pty_host::remote::session::PaneRegistry;
use yinz_types::command::PaneTarget;
use yinz_types::input::command::RunCommand;

use crate::pty_host::remote::protocol::{try_decode_frame, write_control, Frame};

#[test]
fn startup_reuses_a_healthy_listener_without_replacing_its_socket() {
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("broker");
    // Older brokers have no ownership claim, but must still be preserved.
    let _listener = UnixListener::bind(&socket).unwrap();
    let inode = std::fs::metadata(&socket).unwrap().ino();
    ensure_socket(&socket, |_| panic!("the broker is already listening")).unwrap();
    assert!(bind_broker(&socket).is_err());
    assert_eq!(std::fs::metadata(&socket).unwrap().ino(), inode);
    await_socket(&socket).unwrap();
}

#[test]
fn startup_binds_a_missing_socket_and_releases_ownership_on_shutdown() {
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("private/broker");
    let mut broker = None;
    ensure_socket(&socket, |path| {
        broker = bind_broker(path)?;
        Ok(())
    })
    .unwrap();
    let (owner, listener) = broker.unwrap();
    assert_eq!(std::fs::metadata(&socket).unwrap().mode() & 0o777, 0o600);
    assert_eq!(
        std::fs::metadata(socket.parent().unwrap()).unwrap().mode() & 0o777,
        0o700
    );
    assert!(bind_broker(&socket).unwrap().is_none());
    drop(listener);
    drop(owner);
    assert!(!socket.exists());
    assert!(bind_broker(&socket).unwrap().is_some());
}

#[test]
fn startup_reclaims_a_crashed_owners_socket() {
    use std::io::BufRead;
    const CHILD_PATH: &str = "YINZ_TEST_BROKER_OWNER";
    if let Some(path) = std::env::var_os(CHILD_PATH) {
        let _broker = bind_broker(std::path::Path::new(&path)).unwrap().unwrap();
        println!("LISTENING");
        std::io::stdout().flush().unwrap();
        std::io::stdin().read_exact(&mut [0]).unwrap();
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("broker");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            concat!(module_path!(), "::startup_reclaims_a_crashed_owners_socket")
                .strip_prefix("yinz_process::")
                .unwrap(),
            "--nocapture",
        ])
        .env(CHILD_PATH, &socket)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (ready, received) = std::sync::mpsc::channel();
    let reader = thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            if line.unwrap() == "LISTENING" {
                ready.send(()).unwrap();
                break;
            }
        }
    });
    let result = received.recv_timeout(std::time::Duration::from_secs(10));
    let contender = bind_broker(&socket);
    child.kill().unwrap();
    child.wait().unwrap();
    reader.join().unwrap();
    result.unwrap();
    assert!(contender.unwrap().is_none());
    assert!(socket.exists());
    let mut replacement = None;
    ensure_socket(&socket, |path| {
        replacement = bind_broker(path)?;
        Ok(())
    })
    .unwrap();
    assert!(replacement.is_some());
}

#[test]
fn concurrent_startups_leave_one_listener_and_one_owner() {
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("broker");
    let contenders = std::sync::Barrier::new(8);
    let brokers = Mutex::new(Vec::new());
    thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                ensure_socket(&socket, |path| {
                    // Every probe must see the missing socket before any daemon can bind.
                    contenders.wait();
                    if let Some(broker) = bind_broker(path)? {
                        brokers.lock().unwrap().push(broker);
                    }
                    Ok(())
                })
                .unwrap();
            });
        }
    });
    assert_eq!(brokers.lock().unwrap().len(), 1);
    let inode = std::fs::metadata(&socket).unwrap().ino();
    assert!(bind_broker(&socket).unwrap().is_none());
    await_socket(&socket).unwrap();
    assert_eq!(std::fs::metadata(&socket).unwrap().ino(), inode);
}

#[test]
fn startup_and_wait_are_bounded_when_an_owned_backlog_is_full() {
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};
    use yinz_ipc::consts::{ipc_connect, IPC_CONNECT_TIMEOUT};

    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("broker");
    let (_owner, listener) = bind_broker(&socket).unwrap().unwrap();
    let inode = std::fs::metadata(&socket).unwrap().ino();
    // Safety: the listener owns this live socket; only its backlog limit changes.
    assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 1) }, 0);
    let mut queued = Vec::new();
    let mut refused = false;
    for _ in 0..8 {
        match ipc_connect(&socket) {
            Ok(stream) => queued.push(stream),
            Err(error) => {
                refused = error.kind() == std::io::ErrorKind::ConnectionRefused;
                assert!(matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::ConnectionRefused
                ));
                break;
            },
        }
    }
    assert!(queued.len() < 8, "failed to fill the listen backlog");
    let started = Instant::now();
    assert!(ensure_socket(&socket, |path| {
        // BSD may still refuse after retries, but the owner prevents replacement.
        assert!(refused, "a timeout must not spawn a broker");
        assert!(bind_broker(path)?.is_none());
        Ok(())
    })
    .is_err());
    assert!(await_socket(&socket).is_err());
    assert!(started.elapsed() >= IPC_CONNECT_TIMEOUT);
    assert!(started.elapsed() < Duration::from_secs(30));
    assert_eq!(std::fs::metadata(&socket).unwrap().ino(), inode);
    assert!(bind_broker(&socket).unwrap().is_none());

    listener.set_nonblocking(true).unwrap();
    for _ in &queued {
        listener.accept().unwrap();
    }
    ensure_socket(&socket, |_| panic!("draining restored the broker")).unwrap();
    listener.accept().unwrap();
    await_socket(&socket).unwrap();
    assert_eq!(std::fs::metadata(&socket).unwrap().ino(), inode);
}

#[test]
fn startup_preserves_non_socket_paths_and_releases_a_failed_claim() {
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("broker");
    std::fs::write(&socket, "keep me").unwrap();
    let inode = std::fs::metadata(&socket).unwrap().ino();
    assert!(ensure_socket(&socket, |path| bind_broker(path).map(|_| ())).is_err());
    let error = bind_broker(&socket).err().unwrap();
    assert!(format!("{error:#}").contains("not a socket"));
    assert_eq!(std::fs::read_to_string(&socket).unwrap(), "keep me");
    assert_eq!(std::fs::metadata(&socket).unwrap().ino(), inode);
    assert!(SessionOwnership::acquire(&socket).unwrap().is_some());
}

#[test]
fn startup_and_wait_preserve_sockets_on_permission_errors() {
    if nix::unistd::Uid::effective().is_root() {
        return; // Root bypasses directory permissions.
    }
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("broker");
    let _listener = UnixListener::bind(&socket).unwrap();
    let inode = std::fs::metadata(&socket).unwrap().ino();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o000)).unwrap();
    let startup = ensure_socket(&socket, |_| panic!("permission denied is not absence"));
    let waiting = await_socket(&socket);
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    for result in [startup, waiting] {
        let error = result.unwrap_err();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::PermissionDenied
        );
    }
    assert_eq!(std::fs::metadata(&socket).unwrap().ino(), inode);
}

/// Run a broker against one end of a socketpair, returning the caller's end.
fn broker_on_a_pair() -> (UnixStream, std::thread::JoinHandle<Result<()>>) {
    let (ours, theirs) = UnixStream::pair().expect("socketpair failed");
    let handle = std::thread::spawn(move || {
        // No pane is ever spawned in these tests, so the path is never used.
        let registry = Arc::new(PaneRegistry::new());
        serve_connection(theirs, &PathBuf::from("/nonexistent"), &registry)
    });
    (ours, handle)
}

fn read_reply(peer: &mut UnixStream) -> Option<BrokerToServerMsg> {
    use std::io::Read;
    peer.set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .expect("timeout failed");
    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match peer.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                raw.extend_from_slice(&chunk[..n]);
                if let Ok(Some((frame, _))) = try_decode_frame(&raw) {
                    return match frame {
                        Frame::Control(_) => frame.decode_control().ok(),
                        _ => None,
                    };
                }
            },
            Err(_) => break,
        }
    }
    let _ = Cursor::new(raw);
    None
}

fn say_hello(peer: &mut UnixStream, version: u32) {
    write_control(
        peer,
        &ServerToBrokerMsg::Hello {
            protocol_version: version,
            yinz_version: "0.0.0-test".to_owned(),
        },
    )
    .expect("hello failed");
}

#[test]
fn a_matching_handshake_is_acknowledged() {
    let (mut peer, handle) = broker_on_a_pair();
    say_hello(&mut peer, REMOTE_PROTOCOL_VERSION);

    match read_reply(&mut peer) {
        Some(BrokerToServerMsg::HelloAck {
            protocol_version, ..
        }) => assert_eq!(protocol_version, REMOTE_PROTOCOL_VERSION),
        other => panic!("expected a hello ack, got {other:?}"),
    }
    drop(peer);
    let _ = handle.join();
}

/// A remote box on a different yinz build is the normal case, not the
/// exception, so the refusal has to name both versions. "Connection refused"
/// would send someone hunting the network for a version problem.
#[test]
fn a_version_mismatch_is_refused_with_both_versions_named() {
    let (mut peer, handle) = broker_on_a_pair();
    say_hello(&mut peer, REMOTE_PROTOCOL_VERSION + 41);

    let err = handle
        .join()
        .expect("broker thread panicked")
        .expect_err("a version mismatch must be refused");
    let msg = format!("{err:#}");
    assert!(
        msg.contains(&format!("v{}", REMOTE_PROTOCOL_VERSION)),
        "the refusal must name the broker's version, got: {msg}"
    );
    assert!(
        msg.contains(&format!("v{}", REMOTE_PROTOCOL_VERSION + 41)),
        "the refusal must name the server's version, got: {msg}"
    );
    drop(peer);
}

/// The refusal above is worth nothing if the far side never hears it. Closing on
/// a mismatch without acknowledging leaves the connecting server reading EOF,
/// which it can only report as "the broker closed the connection" — and the one
/// fact worth having, which version this broker speaks, is exactly what is lost.
#[test]
fn a_version_mismatch_is_acknowledged_before_it_is_refused() {
    let (mut peer, handle) = broker_on_a_pair();
    say_hello(&mut peer, REMOTE_PROTOCOL_VERSION + 41);

    match read_reply(&mut peer) {
        Some(BrokerToServerMsg::HelloAck {
            protocol_version, ..
        }) => assert_eq!(
            protocol_version, REMOTE_PROTOCOL_VERSION,
            "the ack must name the version this broker actually speaks"
        ),
        other => panic!("a mismatched hello must still be acked, got {other:?}"),
    }
    let _ = handle.join();
    drop(peer);
}

/// `ensure` cannot ask a running broker what it speaks — it prints its own
/// constant — so a daemon from a build before a version bump would be adopted by
/// a newer binary and handed a handshake it cannot answer. The version in the
/// path is what keeps the two apart.
#[test]
fn the_broker_socket_is_named_for_the_protocol_it_speaks() {
    let path = broker_socket_path();
    let name = path
        .file_name()
        .expect("no file name")
        .to_string_lossy()
        .to_string();
    assert!(
        name.contains(&format!("v{REMOTE_PROTOCOL_VERSION}")),
        "a bump must move the socket, but the name is {name}"
    );
}

#[test]
fn a_connection_that_does_not_open_with_a_hello_is_refused() {
    let (mut peer, handle) = broker_on_a_pair();
    write_control(&mut peer, &ServerToBrokerMsg::Terminate).expect("write failed");

    let err = handle
        .join()
        .expect("broker thread panicked")
        .expect_err("an unexpected opening message must be refused");
    assert!(format!("{err:#}").contains("expected a hello"));
    drop(peer);
}

#[test]
fn a_peer_that_closes_before_saying_hello_is_not_an_internal_error() {
    let (peer, handle) = broker_on_a_pair();
    drop(peer);

    let err = handle
        .join()
        .expect("broker thread panicked")
        .expect_err("a silent peer must be reported");
    assert!(format!("{err:#}").contains("closed before"));
}

/// After a good handshake the broker must be asked for a pane. Anything else is
/// a confused peer, and it should say so rather than spawning something.
#[test]
fn a_handshake_not_followed_by_a_spawn_is_refused() {
    let (mut peer, handle) = broker_on_a_pair();
    say_hello(&mut peer, REMOTE_PROTOCOL_VERSION);
    let _ = read_reply(&mut peer);
    write_control(&mut peer, &ServerToBrokerMsg::SignalChild(ChildSignal::Int))
        .expect("write failed");

    let err = handle
        .join()
        .expect("broker thread panicked")
        .expect_err("a missing spawn must be refused");
    assert!(format!("{err:#}").contains("expected a spawn"));
    drop(peer);
}

// ---- what ends a supervisor, and what does not ----
//
// Only an explicit terminate ends a supervisor; dropping its handle deliberately
// does not, because a supervisor outliving its connection is what lets a pane be
// picked back up. That makes the distinction easy to get wrong in a way nothing
// notices: an earlier version of this code returned the same "done" for both, so
// every closed remote pane leaked its supervisor and its child process on the
// remote machine, forever.

/// A pump over a real pty, with a session attached to the same socketpair the
/// caller holds the other end of.
///
/// The session is real rather than faked because `Release` is answered through
/// it: the pump has to detach the pane and write the ack on the wire, and a
/// stand-in for either would be testing the stand-in.
fn pump_on_a_pair() -> (UnixStream, std::thread::JoinHandle<Result<Outcome>>) {
    let (ours, theirs) = pump_over_session(|_session, _registry| {});
    (ours, theirs)
}

/// `pump_on_a_pair`, with a look at the session and registry before the pump
/// starts — for the tests that need to say what happened to the pane afterwards.
fn pump_over_session(
    inspect: impl FnOnce(&Arc<PaneSession>, &Arc<PaneRegistry>),
) -> (UnixStream, std::thread::JoinHandle<Result<Outcome>>) {
    use crate::pty_host::client::PtyHostHandle;

    let pty = nix::pty::openpty(None, None).expect("openpty failed");
    let (control, control_peer) = UnixStream::pair().expect("socketpair failed");
    let (ours, theirs) = UnixStream::pair().expect("socketpair failed");
    let wire: Wire = Arc::new(Mutex::new(theirs.try_clone().expect("socket clone failed")));

    let session = Arc::new(PaneSession::new(
        "token".to_owned(),
        vec!["/bin/sh".to_owned()],
        PtyHostHandle::for_test(control),
        pty.master,
        ProcessId::new(std::process::id()),
    ));
    let registry = Arc::new(PaneRegistry::new());
    registry.insert(session.clone());
    let generation = session.attach(wire.clone());
    inspect(&session, &registry);

    let pump = std::thread::spawn(move || {
        // The slave and the supervisor's peer end are kept alive by this closure
        // for the pump's lifetime; dropping either would end the pty early.
        let _keep = (pty.slave, control_peer);
        let mut theirs = theirs;
        pump_control(&mut theirs, &session, generation, &wire, &registry)
    });
    (ours, pump)
}

#[test]
fn an_explicit_terminate_is_distinguished_from_a_disconnect() {
    let (mut ours, pump) = pump_on_a_pair();
    write_control(&mut ours, &ServerToBrokerMsg::Terminate).expect("write failed");

    let outcome = pump.join().expect("pump panicked").expect("pump errored");
    assert!(
        matches!(outcome, Outcome::Terminated),
        "an explicit terminate must be reported as such, or the supervisor is never reclaimed"
    );
}

#[test]
fn termination_is_acknowledged_only_after_output_is_drained() {
    use crate::pty_host::client::PtyHostHandle;
    use crate::pty_host::protocol::{read_msg, ServerToHostMsg};

    let pty = nix::pty::openpty(None, None).expect("openpty failed");
    let (control, mut control_peer) = UnixStream::pair().expect("socketpair failed");
    let (mut ours, theirs) = UnixStream::pair().expect("socketpair failed");
    let wire: Wire = Arc::new(Mutex::new(theirs));
    let session = Arc::new(PaneSession::new(
        "token".to_owned(),
        vec!["/bin/sh".to_owned()],
        PtyHostHandle::for_test(control),
        pty.master,
        ProcessId::new(std::process::id()),
    ));
    let registry = Arc::new(PaneRegistry::new());
    registry.insert(session.clone());

    let broker_session = session.clone();
    let broker_registry = registry.clone();
    let broker_wire = wire.clone();
    let broker = std::thread::spawn(move || {
        terminate_session(&broker_session, &broker_registry, &broker_wire)
    });

    assert!(matches!(
        read_msg::<ServerToHostMsg, _>(&mut control_peer).expect("termination read failed"),
        Some(ServerToHostMsg::Terminate)
    ));
    ours.set_read_timeout(Some(std::time::Duration::from_millis(50)))
        .expect("timeout failed");
    assert!(
        protocol::read_frame(&mut ours).is_err(),
        "the broker acknowledged before output was drained"
    );

    session.mark_output_drained();
    ours.set_read_timeout(Some(std::time::Duration::from_secs(1)))
        .expect("timeout failed");
    assert!(matches!(
        protocol::read_frame(&mut ours)
            .expect("ack read failed")
            .expect("broker closed before the ack")
            .decode_control::<BrokerToServerMsg>(),
        Ok(BrokerToServerMsg::ChildExited { status: None })
    ));
    broker.join().expect("broker panicked").expect("ack failed");
    drop(pty.slave);
}

/// The output pump reports an exit the moment the pty drains after the child
/// is gone; a terminate that lands after that must not report it again, or
/// the server sees two `ChildExited` frames for one pane.
#[test]
fn a_termination_after_the_pump_reported_the_exit_does_not_report_it_again() {
    use crate::pty_host::client::PtyHostHandle;

    let pty = nix::pty::openpty(None, None).expect("openpty failed");
    let (control, _control_peer) = UnixStream::pair().expect("socketpair failed");
    let (mut ours, theirs) = UnixStream::pair().expect("socketpair failed");
    let wire: Wire = Arc::new(Mutex::new(theirs));
    let session = Arc::new(PaneSession::new(
        "token".to_owned(),
        vec!["/bin/sh".to_owned()],
        PtyHostHandle::for_test(control),
        pty.master,
        ProcessId::new(std::process::id()),
    ));
    let registry = Arc::new(PaneRegistry::new());
    registry.insert(session.clone());
    session.mark_output_drained();
    assert!(
        session.claim_exit_report(),
        "the pump claims the report first"
    );

    terminate_session(&session, &registry, &wire).expect("termination failed");

    ours.set_read_timeout(Some(std::time::Duration::from_millis(100)))
        .expect("timeout failed");
    assert!(
        protocol::read_frame(&mut ours).is_err(),
        "the broker reported the exit a second time"
    );
    assert!(registry.get(&session.token).is_none());
    drop(pty.slave);
}

/// A pane whose terminal something still holds open never drains. That is an
/// ordinary close, finished and reported, not an error on the broker.
#[test]
fn a_drain_timeout_still_finishes_and_reports_the_termination() {
    use crate::pty_host::client::PtyHostHandle;

    let pty = nix::pty::openpty(None, None).expect("openpty failed");
    let (control, _control_peer) = UnixStream::pair().expect("socketpair failed");
    let (mut ours, theirs) = UnixStream::pair().expect("socketpair failed");
    let wire: Wire = Arc::new(Mutex::new(theirs));
    let session = Arc::new(PaneSession::new(
        "token".to_owned(),
        vec!["/bin/sh".to_owned()],
        PtyHostHandle::for_test(control),
        pty.master,
        ProcessId::new(std::process::id()),
    ));
    let registry = Arc::new(PaneRegistry::new());
    registry.insert(session.clone());

    // Nothing ever marks the output drained.
    terminate_session_with_drain_timeout(
        &session,
        &registry,
        &wire,
        std::time::Duration::from_millis(50),
    )
    .expect("a drain timeout is not a failed termination");

    ours.set_read_timeout(Some(std::time::Duration::from_secs(1)))
        .expect("timeout failed");
    assert!(matches!(
        protocol::read_frame(&mut ours)
            .expect("ack read failed")
            .expect("broker closed before the ack")
            .decode_control::<BrokerToServerMsg>(),
        Ok(BrokerToServerMsg::ChildExited { status: None })
    ));
    assert!(registry.get(&session.token).is_none());
    drop(pty.slave);
}

/// A hook inside a pane reports on a connection of its own, naming the pane by
/// the token in its environment; the broker relays the event on the pane's
/// channel and tells the hook whether anyone was there.
#[test]
fn an_agent_report_is_relayed_on_the_named_panes_channel() {
    use crate::pty_host::client::PtyHostHandle;

    let pty = nix::pty::openpty(None, None).expect("openpty failed");
    let (control, _control_peer) = UnixStream::pair().expect("socketpair failed");
    let (mut server_side, theirs) = UnixStream::pair().expect("socketpair failed");
    let wire: Wire = Arc::new(Mutex::new(theirs));
    let session = Arc::new(PaneSession::new(
        "token".to_owned(),
        vec!["/bin/sh".to_owned()],
        PtyHostHandle::for_test(control),
        pty.master,
        ProcessId::new(std::process::id()),
    ));
    let registry = Arc::new(PaneRegistry::new());
    registry.insert(session.clone());
    let _generation = session.attach(wire);

    let dir = tempfile::tempdir().expect("tempdir failed");
    let socket = dir.path().join("broker.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind failed");
    let broker_registry = registry.clone();
    let broker = std::thread::spawn(move || {
        for _ in 0..10 {
            let (stream, _) = listener.accept().expect("accept failed");
            let _ = serve_connection(stream, std::path::Path::new("/bin/false"), &broker_registry);
        }
    });

    report_agent_hook_to(&socket, "token", "claude", r#"{"hook_event_name":"Stop"}"#)
        .expect("a report for an attached pane is delivered");
    server_side
        .set_read_timeout(Some(std::time::Duration::from_secs(1)))
        .expect("timeout failed");
    assert!(matches!(
        protocol::read_frame(&mut server_side)
            .expect("relay read failed")
            .expect("broker closed before the relay")
            .decode_control::<BrokerToServerMsg>(),
        Ok(BrokerToServerMsg::AgentReport { kind, json })
            if kind == "claude" && json == r#"{"hook_event_name":"Stop"}"#
    ));

    let confirmation = yinz_types::agent::HookEvent {
        hook_event_name: "Stop".into(),
        session_id: "remote-session".into(),
        prompt_id: Some("remote-prompt".into()),
        claude_turn_complete: Some(true),
        last_assistant_message: Some("final remote answer".into()),
        ..Default::default()
    };
    crate::claude_turn::send(
        &crate::claude_turn::Route::Remote {
            socket: socket.clone(),
            pane_token: "token".into(),
        },
        "claude",
        &confirmation,
    )
    .unwrap();
    let BrokerToServerMsg::AgentReport { kind, json } = protocol::read_frame(&mut server_side)
        .unwrap()
        .unwrap()
        .decode_control()
        .unwrap()
    else {
        panic!("expected confirmation");
    };
    assert_eq!(kind, "claude");
    assert_eq!(
        serde_json::from_str::<yinz_types::agent::HookEvent>(&json).unwrap(),
        confirmation
    );

    for (hook, mode, notification, state) in [
        (
            "PermissionRequest",
            "plan",
            None,
            Some(yinz_types::agent::AgentState::Busy),
        ),
        (
            "PermissionRequest",
            "default",
            None,
            Some(yinz_types::agent::AgentState::Busy),
        ),
        (
            "PermissionRequest",
            "acceptEdits",
            None,
            Some(yinz_types::agent::AgentState::Busy),
        ),
        (
            "Notification",
            "default",
            Some("permission_prompt"),
            Some(yinz_types::agent::AgentState::Waiting),
        ),
        (
            "Notification",
            "auto",
            Some("permission_prompt"),
            Some(yinz_types::agent::AgentState::Waiting),
        ),
        (
            "Notification",
            "default",
            Some("worker_permission_prompt"),
            None,
        ),
    ] {
        let payload = serde_json::json!({
            "hook_event_name": hook, "session_id": "remote-session", "permission_mode": mode,
            "notification_type": notification, "tool_name": "Bash", "tool_input": {"command": "git status"},
            "future_field": "preserved"
        }).to_string();
        let (local, local_raw) =
            yinz_runtime::agent::hooks::parse_hook_event("claude", payload.clone()).unwrap();
        report_agent_hook_to(&socket, "token", "claude", &payload).unwrap();
        let BrokerToServerMsg::AgentReport { kind, json } = protocol::read_frame(&mut server_side)
            .unwrap()
            .unwrap()
            .decode_control()
            .unwrap()
        else {
            panic!("expected agent report");
        };
        assert_eq!(kind, "claude");
        assert_eq!(json, local_raw);
        let remote = serde_json::from_str(&json).unwrap();
        let outcome = yinz_types::agent::hook_outcome(&kind, &remote);
        assert_eq!(outcome, yinz_types::agent::hook_outcome(&kind, &local));
        let Some(state) = state else {
            assert_eq!(outcome, yinz_types::agent::HookOutcome::Nothing);
            continue;
        };
        let yinz_types::agent::HookOutcome::Reported(agent) = outcome else {
            panic!("expected state");
        };
        assert_eq!(agent.state, state);
        assert_eq!(
            agent.detail.as_deref(),
            (state == yinz_types::agent::AgentState::Busy).then_some("Bash git")
        );
    }

    let transcript = dir.path().join("codex.jsonl");
    std::fs::write(&transcript, concat!(
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"remote-session\"}}\n",
        "{\"type\":\"turn_context\",\"payload\":{\"turn_id\":\"remote-turn\",\"approvals_reviewer\":\"auto_review\"}}\n",
        "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"id\":\"remote-message\",\"phase\":\"commentary\",\"content\":[{\"type\":\"output_text\",\"text\":\"remote commentary\"}]}}\n",
    )).unwrap();
    let mut hook = serde_json::json!({
        "hook_event_name": "PermissionRequest", "session_id": "remote-session",
        "turn_id": "remote-turn", "transcript_path": transcript,
        "permission_mode": "default", "future_field": "preserved"
    });
    report_agent_hook_to(&socket, "token", "codex", &hook.to_string()).unwrap();
    let report = protocol::read_frame(&mut server_side)
        .unwrap()
        .unwrap()
        .decode_control::<BrokerToServerMsg>()
        .unwrap();
    let BrokerToServerMsg::AgentReport { kind, json } = report else {
        panic!("{report:?}")
    };
    assert_eq!(kind, "codex");
    hook["approvals_reviewer"] = "auto_review".into();
    hook["assistant_messages"] = serde_json::json!([{
        "source_id":"remote-session:remote-message", "turn_id":"remote-turn",
        "text":"remote commentary", "incomplete":false
    }]);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&json).unwrap(),
        hook
    );
    let event = serde_json::from_str(&json).unwrap();
    let yinz_types::agent::HookOutcome::Reported(agent) =
        yinz_types::agent::hook_outcome(&kind, &event)
    else {
        panic!("no agent state")
    };
    assert_eq!(agent.state, yinz_types::agent::AgentState::Busy);
    assert!(agent.waiting_on.is_none());
    assert!(!agent.wait_needs_a_person);

    let refused = report_agent_hook_to(
        &socket,
        "no-such-token",
        "claude",
        r#"{"hook_event_name":"Stop"}"#,
    )
    .expect_err("a report for an unknown pane is refused");
    assert!(
        refused.to_string().contains("no session is attached"),
        "{refused}"
    );
    broker.join().expect("broker panicked");
    drop(pty.slave);
}

#[test]
fn oversized_native_hooks_are_read_for_pruning_not_rejected_at_intake() {
    use yinz_types::agent::{HOOK_BROADCAST_MAX_BYTES, NATIVE_HOOK_MAX_BYTES};
    // A prompt `agent prompt` accepts arrives whole inside `UserPromptSubmit`; the
    // intake reads it so the report's own parse and prune can cut it to broadcast
    // size while preserving the fingerprint the acknowledgement is matched by.
    let prompt = "review\n".repeat(100 * 1024);
    assert!(prompt.len() > HOOK_BROADCAST_MAX_BYTES);
    assert!(prompt.len() < yinz_types::agent::control::MAX_PROMPT_BYTES);
    let submit = serde_json::json!({
        "hook_event_name": "UserPromptSubmit",
        "session_id": "remote-session",
        "cwd": "/work",
        "prompt": prompt,
        "turn_id": "remote-turn",
        "permission_mode": "default",
    })
    .to_string();
    let bytes = read_native_hook(&mut Cursor::new(submit.as_bytes())).unwrap();
    assert_eq!(bytes, submit.as_bytes());
    let (event, json) = yinz_runtime::agent::hooks::parse_hook_event("muse", submit).unwrap();
    let pruned = event.broadcast_json(json);
    assert!(pruned.len() <= HOOK_BROADCAST_MAX_BYTES);
    let event: yinz_types::agent::HookEvent = serde_json::from_str(&pruned).unwrap();
    assert_eq!(event.turn_id.as_deref(), Some("remote-turn"));
    assert_eq!(
        event.prompt_sha256.as_deref(),
        Some(yinz_types::agent::control::prompt_sha256(&prompt)).as_deref()
    );

    // A large final message is likewise read, keeping the completion's turn.
    let stop = serde_json::json!({
        "hook_event_name": "Stop",
        "session_id": "remote-session",
        "cwd": "/work",
        "turn_id": "remote-turn",
        "last_assistant_message": "done\n".repeat(100 * 1024),
    })
    .to_string();
    let bytes = read_native_hook(&mut Cursor::new(stop.as_bytes())).unwrap();
    let (event, json) = yinz_runtime::agent::hooks::parse_hook_event("muse", stop).unwrap();
    assert_eq!(bytes.len(), json.len());
    let pruned = event.broadcast_json(json);
    assert!(pruned.len() <= HOOK_BROADCAST_MAX_BYTES);
    let event: yinz_types::agent::HookEvent = serde_json::from_str(&pruned).unwrap();
    assert_eq!(
        yinz_types::agent::hook_state(&event),
        Some(yinz_types::agent::AgentState::Idle)
    );

    // Past the native ceiling it is not a hook event.
    let too_big = vec![b'x'; NATIVE_HOOK_MAX_BYTES + 1];
    let error = read_native_hook(&mut Cursor::new(too_big)).unwrap_err();
    assert_eq!(
        error.to_string(),
        "agent hook event exceeds the native payload limit"
    );
}

#[test]
fn a_connection_that_just_ends_is_not_a_terminate() {
    let (ours, pump) = pump_on_a_pair();
    drop(ours);

    let outcome = pump.join().expect("pump panicked").expect("pump errored");
    assert!(
        matches!(outcome, Outcome::Disconnected),
        "a dropped connection must not read as a terminate, or a reconnectable pane is killed"
    );
}

#[test]
fn an_overrun_during_catch_up_reports_the_gap_before_the_surviving_output() {
    use crate::pty_host::client::PtyHostHandle;
    use nix::sys::socket::{setsockopt, sockopt::SndBuf};

    let pty = nix::pty::openpty(None, None).unwrap();
    let (control, _control_peer) = UnixStream::pair().unwrap();
    let (mut ours, theirs) = UnixStream::pair().unwrap();
    setsockopt(&theirs, SndBuf, &4096).unwrap();
    ours.set_read_timeout(Some(yinz_test_support::default_timeout()))
        .unwrap();
    theirs
        .set_write_timeout(Some(yinz_test_support::default_timeout()))
        .unwrap();
    let wire: Wire = Arc::new(Mutex::new(theirs));
    let session = Arc::new(PaneSession::new(
        "token".to_owned(),
        vec!["/bin/sh".to_owned()],
        PtyHostHandle::for_test(control),
        pty.master,
        ProcessId::new(std::process::id()),
    ));
    let initial = vec![b'x'; MAX_STREAM_PAYLOAD];
    assert!(session.append_output(&initial).is_none());
    let generation = session.try_begin_attach().unwrap();
    let broker_session = session.clone();
    let broker = std::thread::spawn(move || {
        catch_up_and_attach(&broker_session, &wire, "token", 0, generation)
    });

    let resumed = protocol::read_frame(&mut ours).unwrap().unwrap();
    assert_eq!(
        resumed.decode_control::<BrokerToServerMsg>().unwrap(),
        BrokerToServerMsg::Resumed { dropped: 0 }
    );
    // The initial replay cannot fit in the socket. Before draining it, overrun the ring so
    // the next catch-up round must report a loss that was absent from the Resumed message.
    let replay_capacity = 4 * 1024 * 1024;
    assert!(session
        .append_output(&vec![b'y'; replay_capacity + 17])
        .is_none());
    assert_eq!(
        protocol::read_frame(&mut ours).unwrap().unwrap(),
        Frame::Output(initial)
    );
    assert_eq!(
        protocol::read_frame(&mut ours)
            .unwrap()
            .unwrap()
            .decode_control::<BrokerToServerMsg>()
            .unwrap(),
        BrokerToServerMsg::OutputGap { dropped: 17 }
    );
    let mut received = 0;
    while received < replay_capacity {
        let Frame::Output(bytes) = protocol::read_frame(&mut ours).unwrap().unwrap() else {
            panic!("expected the surviving replay after the gap");
        };
        assert!(bytes.iter().all(|byte| *byte == b'y'));
        received += bytes.len();
    }
    assert_eq!(received, replay_capacity);
    broker.join().unwrap().unwrap();
}

// --- handing a pane to another server ---
//
// A warp is a resume by a *different* server, and the broker cannot tell the two
// apart — which is the reason the feature is small. What it can tell apart is a
// connection that ended from one that said it was handing the pane over, and
// that distinction is the whole of what `Release` buys: the detach happens
// before the ack rather than whenever the old connection's blocking read
// happens to return, so the token can be given away and used immediately.

#[test]
fn a_release_detaches_the_pane_without_ending_it() {
    let holder = Arc::new(Mutex::new(None));
    let seen = holder.clone();
    let (mut ours, pump) = pump_over_session(move |session, registry| {
        *seen.lock().unwrap() = Some((session.clone(), registry.clone()));
    });
    write_control(&mut ours, &ServerToBrokerMsg::Release).expect("write failed");

    let outcome = pump.join().expect("pump panicked").expect("pump errored");
    assert!(
        matches!(outcome, Outcome::Released),
        "a release must be distinguishable from a disconnect, or the ack is never sent"
    );

    let (session, registry) = holder.lock().unwrap().clone().expect("no session recorded");
    assert!(
        !session.is_attached(),
        "the pane must be detached, or the adopting server's resume is refused as a double attach"
    );
    assert!(
        !session.is_terminated(),
        "a release must not end the pane — the running process is the entire point of a warp"
    );
    assert_eq!(
        registry.len(),
        1,
        "the pane must stay in the registry, or there is nothing left for the far session to \
         resume by token"
    );
}

/// The race this exists to close. A server that merely closed its socket would
/// leave the pane attached until the broker's blocking read returned, and a
/// resume arriving in that window is refused outright.
#[test]
fn a_pane_is_free_to_resume_the_moment_the_release_is_acknowledged() {
    let holder = Arc::new(Mutex::new(None));
    let seen = holder.clone();
    let (mut ours, pump) = pump_over_session(move |session, registry| {
        *seen.lock().unwrap() = Some((session.clone(), registry.clone()));
    });
    write_control(&mut ours, &ServerToBrokerMsg::Release).expect("write failed");

    // Read the ack rather than joining the pump: this is the ordering the
    // adopting server actually relies on. Joining would prove only that the pane
    // is free once the connection is fully torn down, which is the weaker claim
    // a plain disconnect already satisfies.
    let ack = read_reply(&mut ours).expect("the broker must acknowledge a release");
    assert_eq!(
        ack,
        BrokerToServerMsg::Released,
        "the release must be acknowledged, or the far side has nothing to wait for"
    );

    let (session, _) = holder.lock().unwrap().clone().expect("no session recorded");
    assert!(
        !session.is_attached(),
        "the pane must already be free when the ack lands, not merely on its way to being free"
    );

    drop(ours);
    let _ = pump.join();
}

/// The two refusals mean opposite things to the adopting side — one is worth
/// waiting a moment for, the other never becomes true — so the broker has to say
/// which it is rather than leaving it to be read out of the prose.
#[test]
fn a_resume_says_whether_waiting_could_help() {
    let registry = Arc::new(PaneRegistry::new());
    let (ours, theirs) = UnixStream::pair().expect("socketpair failed");
    let wire: Wire = Arc::new(Mutex::new(theirs));

    let refused = resume_pane(&registry, &wire, "never-heard-of-it", 0);
    assert!(refused.is_err());
    let mut ours = ours;
    match read_reply(&mut ours).expect("no refusal was sent") {
        BrokerToServerMsg::ResumeRefused { kind, .. } => assert_eq!(
            kind,
            ResumeRefusal::Unknown,
            "a pane this broker has never held must not read as one worth retrying for"
        ),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// The other half of the discriminant, and the half it exists for.
///
/// A pane some connection still holds becomes free on its own — the holder
/// noticing its socket is dead takes a scheduling quantum — so the asking side
/// retries briefly rather than giving up. That is only safe because this
/// refusal says which case it is: retrying an `Unknown` would be waiting for
/// something that never becomes true.
#[test]
fn a_resume_for_a_pane_someone_still_holds_is_worth_waiting_for() {
    let holder = Arc::new(Mutex::new(None));
    let seen = holder.clone();
    let (ours, pump) = pump_over_session(move |session, registry| {
        *seen.lock().unwrap() = Some((session.clone(), registry.clone()));
    });
    let (session, registry) = holder.lock().unwrap().clone().expect("no session recorded");
    assert!(
        session.is_attached(),
        "the fixture's pane should still be held by the pump's connection"
    );

    // A second server asking for the pane the pump is still speaking for.
    let (mut asker, asker_peer) = UnixStream::pair().expect("socketpair failed");
    let wire: Wire = Arc::new(Mutex::new(asker_peer));
    let refused = resume_pane(&registry, &wire, "token", 0);
    assert!(refused.is_err());
    match read_reply(&mut asker).expect("no refusal was sent") {
        BrokerToServerMsg::ResumeRefused { kind, .. } => assert_eq!(
            kind,
            ResumeRefusal::AlreadyAttached,
            "a pane whose holder has not let go yet must read as worth retrying for, or a \
             warp that raced the release loses the pane outright"
        ),
        other => panic!("expected a refusal, got {other:?}"),
    }

    drop(ours);
    let _ = pump.join();
}

/// A pane nobody is speaking for is exactly what `pty-host list --host` is for
/// finding, so the listing has to distinguish it from a healthy one.
#[test]
fn listing_panes_says_which_have_no_server() {
    let holder = Arc::new(Mutex::new(None));
    let seen = holder.clone();
    let (mut ours, pump) = pump_over_session(move |session, registry| {
        *seen.lock().unwrap() = Some((session.clone(), registry.clone()));
    });

    write_control(&mut ours, &ServerToBrokerMsg::ListPanes).expect("write failed");
    let attached = match read_reply(&mut ours).expect("no answer to ListPanes") {
        BrokerToServerMsg::PaneList { entries } => entries,
        other => panic!("expected a pane list, got {other:?}"),
    };
    assert_eq!(attached.len(), 1);
    assert!(
        attached[0].attached,
        "a pane with this connection speaking for it must not read as stranded"
    );
    assert_eq!(attached[0].command, "/bin/sh");
    assert_eq!(attached[0].argv, Some(vec!["/bin/sh".to_owned()]));

    write_control(&mut ours, &ServerToBrokerMsg::Release).expect("write failed");
    let _ = read_reply(&mut ours);

    let (_, registry) = holder.lock().unwrap().clone().expect("no session recorded");
    let stranded = registry.entries();
    assert!(
        !stranded[0].attached,
        "a released pane nobody has adopted is precisely what this listing is for"
    );
    assert!(
        stranded[0].unattached_secs.is_some(),
        "an unattached pane must carry how long it has been that way — it is what the orphan \
         reaper measures, and what tells a user whether it is about to be reclaimed"
    );

    drop(ours);
    let _ = pump.join();
}

// --- resolving the default shell ---
//
// This machine is the only authority on which shell its user has and where their
// home directory is, so these pin the order it consults and the argv[0] that
// makes the result a *login* shell. `ShellSource` is a parameter precisely so
// this can be tested without arranging a second user on the test machine.

fn default_shell_command(host: Option<&str>) -> RunCommand {
    RunCommand {
        command: PathBuf::new(),
        remote_default_shell: true,
        target: host
            .map(|host| PaneTarget::Ssh {
                host: host.to_owned(),
            })
            .unwrap_or_default(),
        ..Default::default()
    }
}

#[test]
fn the_passwd_shell_beats_the_environment() {
    let mut cmd = default_shell_command(Some("build-box"));
    let arg0 = resolve_default_shell(
        &mut cmd,
        &ShellSource {
            passwd_shell: Some(PathBuf::from("/usr/local/bin/zsh")),
            passwd_home: Some(PathBuf::from("/home/zach")),
            env_shell: Some(PathBuf::from("/bin/bash")),
            env_home: Some(PathBuf::from("/root")),
        },
    );
    // `$SHELL` here is whatever the bootstrap ssh happened to freeze into this
    // broker's environment; the passwd entry is the fact about the user.
    assert_eq!(cmd.command, PathBuf::from("/usr/local/bin/zsh"));
    assert_eq!(arg0.as_deref(), Some("-zsh"));
    assert_eq!(cmd.cwd, Some(PathBuf::from("/home/zach")));
}

#[test]
fn the_environment_stands_in_for_a_passwd_entry_without_a_shell() {
    let mut cmd = default_shell_command(Some("build-box"));
    let arg0 = resolve_default_shell(
        &mut cmd,
        &ShellSource {
            passwd_shell: Some(PathBuf::new()),
            passwd_home: Some(PathBuf::from("/home/zach")),
            env_shell: Some(PathBuf::from("/bin/bash")),
            ..Default::default()
        },
    );
    assert_eq!(cmd.command, PathBuf::from("/bin/bash"));
    assert_eq!(arg0.as_deref(), Some("-bash"));
}

#[test]
fn with_nothing_to_go_on_it_falls_back_to_sh() {
    let mut cmd = default_shell_command(Some("build-box"));
    let arg0 = resolve_default_shell(&mut cmd, &ShellSource::default());
    assert_eq!(cmd.command, PathBuf::from("/bin/sh"));
    assert_eq!(arg0.as_deref(), Some("-sh"));
    assert_eq!(
        cmd.cwd, None,
        "no home to be found is not a reason to invent one"
    );
}

/// An explicit `--cwd` names a directory on this machine and outranks the home
/// directory the login would otherwise have chosen.
#[test]
fn an_explicit_cwd_survives_resolution() {
    let mut cmd = default_shell_command(Some("build-box"));
    cmd.cwd = Some(PathBuf::from("/srv/www"));
    resolve_default_shell(
        &mut cmd,
        &ShellSource {
            passwd_shell: Some(PathBuf::from("/bin/bash")),
            passwd_home: Some(PathBuf::from("/home/zach")),
            ..Default::default()
        },
    );
    assert_eq!(cmd.cwd, Some(PathBuf::from("/srv/www")));
}

/// The flag without a host is not a request this machine was ever given, and an
/// empty command is also how a mistyped one arrives — so it must stay empty and
/// fail as `CommandNotFound` rather than quietly logging someone in.
#[test]
fn anything_that_is_not_a_remote_default_shell_is_left_alone() {
    let mut hostless = default_shell_command(None);
    assert_eq!(
        resolve_default_shell(&mut hostless, &ShellSource::default()),
        None
    );
    assert_eq!(hostless.command, PathBuf::new());

    let mut ordinary = RunCommand {
        command: PathBuf::from("/usr/bin/htop"),
        args: vec!["-d".to_owned(), "5".to_owned()],
        target: PaneTarget::Ssh {
            host: "build-box".to_owned(),
        },
        ..Default::default()
    };
    assert_eq!(
        resolve_default_shell(&mut ordinary, &ShellSource::default()),
        None
    );
    assert_eq!(ordinary.command, PathBuf::from("/usr/bin/htop"));
    assert_eq!(ordinary.args, vec!["-d".to_owned(), "5".to_owned()]);
}

/// The shell and the login environment are two questions about the same passwd
/// entry, and `start_pane` asks the system once and answers both from it. Pinned
/// because the two structs have deliberately different fallback rules and it
/// would be easy to let them drift onto different lookups.
#[test]
fn one_passwd_lookup_answers_both_the_shell_and_the_login_environment() {
    use crate::pty_host::remote::env::LoginSource;

    let user = nix::unistd::User::from_uid(nix::unistd::Uid::effective())
        .ok()
        .flatten();
    let shell_source = ShellSource::from_user(&user);
    let login_source = LoginSource::from_user(&user);

    assert_eq!(
        shell_source.passwd_shell, login_source.shell,
        "both views must come from the same passwd entry"
    );
    assert_eq!(shell_source.passwd_home, login_source.home);
}

/// An explicit-command pane gets no login shell - and therefore no profile - which
/// is exactly why it needs a composed environment rather than the broker's frozen
/// one. `ssh host cmd` is handed HOME/USER/PATH too.
#[test]
fn an_explicit_command_pane_gets_an_environment_but_no_login_shell() {
    use crate::pty_host::remote::env::{compose, LoginSource};

    let mut cmd = RunCommand {
        command: PathBuf::from("/usr/bin/env"),
        target: PaneTarget::Ssh {
            host: "build-box".to_owned(),
        },
        remote_default_shell: false,
        ..Default::default()
    };
    let arg0 = resolve_default_shell(&mut cmd, &ShellSource::default());
    assert_eq!(
        arg0, None,
        "an explicit command must not be turned into a login shell"
    );

    let env = compose(
        &LoginSource {
            name: Some("test".to_owned()),
            uid: Some(1000),
            home: Some(PathBuf::from("/home/test")),
            shell: Some(PathBuf::from("/bin/bash")),
        },
        vec![("TERM".to_owned(), "xterm".to_owned())],
    );
    for expected in ["HOME", "USER", "PATH", "TERM"] {
        assert!(
            env.iter().any(|(name, _)| name == expected),
            "an explicit-command pane still needs {expected}: {env:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The Git probe pump
// ---------------------------------------------------------------------------

/// Every case below drives `GitProbePump::tick` directly rather than the thread
/// that owns it. The pump's whole job is *when* to ask Git and *whether* the
/// answer is worth sending, and inside the pump thread the refresh interval is
/// reachable only by waiting a minute for it.
mod git_probe_pump {

    use crate::pty_host::remote::broker::GitProbePump;
    use crate::pty_host::remote::status::GitProbe;
    use std::cell::Cell;
    use std::time::Duration;

    const TICK: Duration = Duration::from_secs(1);
    const INTERVAL: Duration = Duration::from_secs(60);

    fn probe_of(cwd: &str, branch: &str) -> GitProbe {
        GitProbe {
            cwd: cwd.to_owned(),
            exit_code: Some(0),
            metadata: format!("{cwd}\n{cwd}/.git\n{cwd}/.git\n{branch}\n").into_bytes(),
            config: Vec::new(),
        }
    }

    /// The first tick has nothing to compare against, so it probes and sends.
    #[test]
    fn the_first_look_at_a_folder_is_reported() {
        let mut pump = GitProbePump::new(INTERVAL);
        let ran = pump
            .tick("/work", TICK, |cwd| probe_of(cwd, "main"))
            .expect("the first tick must probe");
        assert!(ran.changed, "the first answer is news");
    }

    #[test]
    fn git_probe_refresh_runs_and_acknowledges_unchanged_results() {
        let mut pump = GitProbePump::new(INTERVAL);
        let probe = |cwd: &str| probe_of(cwd, "main");
        pump.tick("/work", TICK, probe).unwrap();
        for token in ["first", "second"] {
            let ran = pump
                .tick_with_refresh("/work", Some(token), TICK, probe)
                .unwrap();
            assert!(
                ran.changed,
                "a fresh request is acknowledged even if Git is unchanged"
            );
            assert!(pump
                .tick_with_refresh("/work", Some(token), TICK, probe)
                .is_none());
        }
        let ran = pump
            .tick_with_refresh("/work", Some("third"), TICK, |cwd| {
                let mut changed = probe_of(cwd, "topic");
                changed.config = b"remote.origin.url\nnew-url\0".to_vec();
                changed
            })
            .unwrap();
        assert_ne!(ran.probe, probe("/work"));
        assert!(ran.changed);
    }

    /// The property the pane's whole traffic model rests on: a pane sitting
    /// still in an unchanging checkout says nothing, however long it sits.
    #[test]
    fn an_unchanging_folder_is_never_reported_again() {
        let runs = Cell::new(0);
        let mut pump = GitProbePump::new(INTERVAL);
        let probe = |cwd: &str| {
            runs.set(runs.get() + 1);
            probe_of(cwd, "main")
        };

        assert!(pump.tick("/work", TICK, probe).is_some());
        for _ in 0..59 {
            assert!(
                pump.tick("/work", TICK, probe).is_none(),
                "nothing changed and the interval is not up"
            );
        }
        // The 61st tick crosses the interval, so Git is asked again...
        let ran = pump
            .tick("/work", TICK, probe)
            .expect("the refresh interval is up");
        // ...and says the same thing, which is not worth a frame.
        assert!(!ran.changed, "an unchanged answer must not be sent");
        assert_eq!(runs.get(), 2, "Git was asked once, then once more");
    }

    /// What the interval exists for: `git checkout` moves the branch without
    /// moving the folder, and no event anywhere says it happened.
    #[test]
    fn a_branch_switched_in_place_is_reported_at_the_interval() {
        let branch = Cell::new("main");
        let mut pump = GitProbePump::new(INTERVAL);
        let probe = |cwd: &str| probe_of(cwd, branch.get());

        assert!(pump.tick("/work", TICK, probe).is_some());
        branch.set("topic");
        for _ in 0..59 {
            assert!(
                pump.tick("/work", TICK, probe).is_none(),
                "the interval is not up yet, so nothing has looked",
            );
        }

        let ran = pump
            .tick("/work", TICK, probe)
            .expect("the refresh interval is up");
        assert!(ran.changed, "the branch moved, so this is news");
        assert!(String::from_utf8_lossy(&ran.probe.metadata).contains("topic"));
    }

    /// A new folder may be a different checkout, which is the one thing that can
    /// change a pane's project — so it does not wait for the interval.
    #[test]
    fn a_new_folder_is_probed_at_once() {
        let mut pump = GitProbePump::new(INTERVAL);
        assert!(pump
            .tick("/work", TICK, |cwd| probe_of(cwd, "main"))
            .is_some());

        let ran = pump
            .tick("/elsewhere", TICK, |cwd| probe_of(cwd, "main"))
            .expect("a folder change probes without waiting");
        assert!(ran.changed);
        assert_eq!(ran.probe.cwd, "/elsewhere");
    }

    /// Moving away and back is still a move: the answer for the folder in hand
    /// is what the pane's project is read from, and the pump holds exactly one.
    #[test]
    fn moving_back_to_a_folder_probes_it_again() {
        let mut pump = GitProbePump::new(INTERVAL);
        assert!(pump
            .tick("/work", TICK, |cwd| probe_of(cwd, "main"))
            .is_some());
        assert!(pump
            .tick("/elsewhere", TICK, |cwd| probe_of(cwd, "main"))
            .is_some());

        let ran = pump
            .tick("/work", TICK, |cwd| probe_of(cwd, "main"))
            .expect("back in the first folder, and it has not been looked at since");
        assert!(ran.changed, "the last answer described the other folder");
        assert_eq!(ran.probe.cwd, "/work");
    }
}
