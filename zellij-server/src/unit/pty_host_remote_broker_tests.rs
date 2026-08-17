//! Protocol-level tests for the broker, driven in-process over a `socketpair`
//! the way `pty_host_tests.rs` drives `serve()`.
//!
//! Spawning a real pane needs a real zellij binary to re-exec as a supervisor,
//! so the full spawn/output/exit path belongs to the integration suite. What is
//! covered here is everything the broker decides *before* it needs one — which
//! is where the cross-machine failure modes live.

use super::*;

use std::io::Cursor;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;

use crate::pty_host::remote::session::PaneRegistry;
use zellij_utils::input::command::RunCommand;

use crate::pty_host::remote::protocol::{try_decode_frame, write_control, Frame};

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
            zellij_version: "0.0.0-test".to_owned(),
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

/// A remote box on a different zellij build is the normal case, not the
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

fn pump_on_a_pair() -> (UnixStream, std::thread::JoinHandle<Result<Outcome>>) {
    use std::os::unix::io::AsRawFd;
    let pty = nix::pty::openpty(None, None).expect("openpty failed");
    let master_fd = pty.master.as_raw_fd();
    let (ours, theirs) = UnixStream::pair().expect("socketpair failed");
    let pump = std::thread::spawn(move || {
        // The pty is kept alive by this closure for the pump's lifetime.
        let _keep = pty;
        let mut theirs = theirs;
        pump_control(&mut theirs, master_fd, std::process::id())
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
fn a_connection_that_just_ends_is_not_a_terminate() {
    let (ours, pump) = pump_on_a_pair();
    drop(ours);

    let outcome = pump.join().expect("pump panicked").expect("pump errored");
    assert!(
        matches!(outcome, Outcome::Disconnected),
        "a dropped connection must not read as a terminate, or a reconnectable pane is killed"
    );
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
        remote_host: host.map(String::from),
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
        remote_host: Some("build-box".to_owned()),
        ..Default::default()
    };
    assert_eq!(
        resolve_default_shell(&mut ordinary, &ShellSource::default()),
        None
    );
    assert_eq!(ordinary.command, PathBuf::from("/usr/bin/htop"));
    assert_eq!(ordinary.args, vec!["-d".to_owned(), "5".to_owned()]);
}
