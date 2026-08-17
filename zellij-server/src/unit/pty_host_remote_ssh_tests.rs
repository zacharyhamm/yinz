//! Tests for the parts of the ssh path that do not need an ssh.
//!
//! Actually reaching another machine belongs to a manual or integration check;
//! what is pinned here is the endpoint contract and the handshake, both of which
//! are easy to break silently and expensive to debug over a network.

use super::*;

use std::os::unix::net::UnixStream;

#[test]
fn a_broker_endpoint_parses_from_the_line_ensure_prints() {
    let line = serde_json::json!({
        "socket": "/run/user/1000/zellij/cv1/pty-broker.sock",
        "protocol_version": REMOTE_PROTOCOL_VERSION,
        "zellij_version": "0.45.0",
    })
    .to_string();
    let endpoint: BrokerEndpoint = serde_json::from_str(&line).expect("parse failed");
    assert_eq!(
        endpoint.socket,
        "/run/user/1000/zellij/cv1/pty-broker.sock"
    );
    assert_eq!(endpoint.protocol_version, REMOTE_PROTOCOL_VERSION);
}

/// Each host gets its own forward socket, and the name has to survive being a
/// path component — ssh destinations contain `@`, `.` and sometimes `:`, none of
/// which belong in a filename.
#[test]
fn each_host_gets_its_own_socket_path_whatever_it_is_called() {
    let awkward = [
        "build-box",
        "me@build-box.example.com",
        "user@[2001:db8::1]",
        "alias-with/slash",
    ];
    let mut seen = std::collections::HashSet::new();
    for host in awkward {
        let path = local_socket_path("server", host);
        let name = path.file_name().expect("no file name").to_string_lossy();
        assert!(
            !name.contains('/') && !name.contains('@') && !name.contains(':'),
            "socket name {name} is not a safe path component"
        );
        assert!(seen.insert(path.clone()), "two hosts shared {path:?}");
    }
    // Stable across calls, or a reconnect would bind a different socket than the
    // one the forward is on.
    assert_eq!(
        local_socket_path("server", "build-box"),
        local_socket_path("server", "build-box")
    );
}

/// `ZELLIJ_SOCK_DIR` is per user, not per session, so a host-only name puts two
/// of one person's sessions on the same socket. Then the second session's
/// forward unlinks and rebinds the first's, the first silently routes its panes
/// through the second's ssh, and whichever closes first deletes the other's
/// socket.
#[test]
fn two_servers_do_not_share_a_forward_socket_for_the_same_host() {
    assert_ne!(
        local_socket_path("server-a", "build-box"),
        local_socket_path("server-b", "build-box"),
        "two servers would fight over one forward to the same host"
    );
}

/// The regression above only holds if each server actually gets a distinct id —
/// a derived `Default` would hand every one of them the same empty string and
/// quietly restore the collision.
#[test]
fn each_remote_hosts_gets_its_own_identity() {
    let a = RemoteHosts::new();
    let b = RemoteHosts::new();
    assert_ne!(
        a.forward_socket_path("build-box"),
        b.forward_socket_path("build-box"),
        "two RemoteHosts in one process picked the same forward socket"
    );
}

/// A socket the owning ssh has gone from is rubbish, but one that is merely
/// young may belong to a server whose forward is still coming up — and
/// `start_forward` gives that up to fifteen seconds.
#[test]
fn the_sweep_spares_sockets_that_are_too_young_to_judge() {
    let dir = tempfile::tempdir().expect("tempdir failed");
    let fresh = dir.path().join("fresh.sock");
    std::fs::write(&fresh, b"").expect("write failed");
    let ours = dir.path().join("ours.sock");
    std::fs::write(&ours, b"").expect("write failed");

    sweep_stale_sockets(dir.path(), &ours);

    assert!(
        fresh.exists(),
        "a socket younger than the grace period was swept; a server still binding its \
         forward would have had it pulled out from under it"
    );
    assert!(ours.exists(), "the sweep removed our own socket");
}

/// The handshake must refuse a version gap rather than proceeding to spawn, and
/// it must name both sides — a mismatched remote build is the likeliest failure
/// here and the least guessable from a generic connection error.
#[test]
fn the_handshake_refuses_a_version_gap_and_names_both_sides() {
    let (ours, theirs) = UnixStream::pair().expect("socketpair failed");
    // Stand in for a broker that speaks a different version.
    let peer = std::thread::spawn(move || {
        let mut theirs = theirs;
        // Consume the hello, then answer with the wrong version.
        let _ = protocol::read_frame(&mut theirs);
        protocol::write_control(
            &mut theirs,
            &BrokerToServerMsg::HelloAck {
                protocol_version: REMOTE_PROTOCOL_VERSION + 7,
                zellij_version: "9.9.9".to_owned(),
            },
        )
        .expect("write failed");
        theirs
    });

    let cmd = RunCommand {
        command: std::path::PathBuf::from("/bin/sh"),
        ..Default::default()
    };
    let err = handshake_and_spawn(&ours, "build-box", &cmd, None)
        .expect_err("a version gap must be refused");
    let msg = format!("{err:#}");
    assert!(msg.contains("build-box"), "should name the host: {msg}");
    assert!(
        msg.contains(&format!("v{}", REMOTE_PROTOCOL_VERSION + 7)),
        "should name the remote's version: {msg}"
    );
    assert!(
        msg.contains(&format!("v{REMOTE_PROTOCOL_VERSION}")),
        "should name our version: {msg}"
    );
    let _ = peer.join();
}

/// A command that does not exist on the *remote* machine has to come back as
/// `ZellijError::CommandNotFound`, not as a generic failure: eight sites in
/// pty.rs downcast for exactly that, and a remote pane should behave like a
/// local one when the command is simply mistyped.
#[test]
fn a_missing_remote_command_arrives_as_command_not_found() {
    let (ours, theirs) = UnixStream::pair().expect("socketpair failed");
    let peer = std::thread::spawn(move || {
        let mut theirs = theirs;
        let _ = protocol::read_frame(&mut theirs);
        protocol::write_control(
            &mut theirs,
            &BrokerToServerMsg::HelloAck {
                protocol_version: REMOTE_PROTOCOL_VERSION,
                zellij_version: zellij_utils::consts::VERSION.to_owned(),
            },
        )
        .expect("write failed");
        let _ = protocol::read_frame(&mut theirs);
        protocol::write_control(
            &mut theirs,
            &BrokerToServerMsg::SpawnFailed {
                kind: SpawnFailureKind::CommandNotFound,
                message: "no such file".to_owned(),
            },
        )
        .expect("write failed");
        theirs
    });

    let cmd = RunCommand {
        command: std::path::PathBuf::from("definitely-not-a-command"),
        ..Default::default()
    };
    let err = handshake_and_spawn(&ours, "build-box", &cmd, None)
        .expect_err("a missing command must be refused");
    assert!(
        matches!(
            err.downcast_ref::<ZellijError>(),
            Some(ZellijError::CommandNotFound { .. })
        ),
        "expected CommandNotFound, got {err:#}"
    );
    let _ = peer.join();
}

/// The far side confirming the child is running is what lets a pane reach the
/// screen already alive, with no window in which it exists but is unspawned.
#[test]
fn a_confirmed_spawn_completes_the_handshake() {
    let (ours, theirs) = UnixStream::pair().expect("socketpair failed");
    let peer = std::thread::spawn(move || {
        let mut theirs = theirs;
        let _ = protocol::read_frame(&mut theirs);
        protocol::write_control(
            &mut theirs,
            &BrokerToServerMsg::HelloAck {
                protocol_version: REMOTE_PROTOCOL_VERSION,
                zellij_version: zellij_utils::consts::VERSION.to_owned(),
            },
        )
        .expect("write failed");
        let _ = protocol::read_frame(&mut theirs);
        protocol::write_control(
            &mut theirs,
            &BrokerToServerMsg::Spawned {
                pane_token: "tok".to_owned(),
            },
        )
        .expect("write failed");
        theirs
    });

    let cmd = RunCommand {
        command: std::path::PathBuf::from("/bin/sh"),
        ..Default::default()
    };
    handshake_and_spawn(&ours, "build-box", &cmd, None).expect("handshake should succeed");
    let _ = peer.join();
}

/// A pane that wants the far machine's login shell must ship *no* command.
///
/// This side's `$SHELL` is a path on this side's filesystem; sending it would
/// ask the other machine to exec something that need not exist there, and would
/// silently be the wrong shell where it does. The flag is the whole request.
#[test]
fn a_default_shell_pane_ships_no_command_of_its_own() {
    let (ours, theirs) = UnixStream::pair().expect("socketpair failed");
    let peer = std::thread::spawn(move || {
        let mut theirs = theirs;
        let _ = protocol::read_frame(&mut theirs);
        protocol::write_control(
            &mut theirs,
            &BrokerToServerMsg::HelloAck {
                protocol_version: REMOTE_PROTOCOL_VERSION,
                zellij_version: zellij_utils::consts::VERSION.to_owned(),
            },
        )
        .expect("write failed");
        let spawn = protocol::read_frame(&mut theirs)
            .expect("read failed")
            .expect("the server should have asked for a pane");
        protocol::write_control(
            &mut theirs,
            &BrokerToServerMsg::Spawned {
                pane_token: "tok".to_owned(),
            },
        )
        .expect("write failed");
        (theirs, spawn)
    });

    let cmd = RunCommand {
        command: std::path::PathBuf::new(),
        remote_default_shell: true,
        remote_host: Some("build-box".to_owned()),
        ..Default::default()
    };
    handshake_and_spawn(&ours, "build-box", &cmd, None).expect("handshake should succeed");

    let (_theirs, spawn) = peer.join().expect("peer panicked");
    let body = match spawn {
        protocol::Frame::Control(body) => body,
        other => panic!("expected a control frame, got {other:?}"),
    };
    let msg: ServerToBrokerMsg = serde_json::from_slice(&body).expect("undecodable spawn");
    match msg {
        ServerToBrokerMsg::Spawn { cmd, .. } => {
            assert!(cmd.remote_default_shell);
            assert_eq!(cmd.command, std::path::PathBuf::new());
            assert!(cmd.args.is_empty());
            assert_eq!(
                cmd.cwd, None,
                "a local path would name a directory on the wrong machine"
            );
        },
        other => panic!("expected a Spawn, got {other:?}"),
    }
}
