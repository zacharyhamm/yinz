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
        "socket": "/run/user/1000/zellij/contract_version_1/pty-broker.sock",
        "protocol_version": REMOTE_PROTOCOL_VERSION,
        "zellij_version": "0.45.0",
    })
    .to_string();
    let endpoint: BrokerEndpoint = serde_json::from_str(&line).expect("parse failed");
    assert_eq!(
        endpoint.socket,
        "/run/user/1000/zellij/contract_version_1/pty-broker.sock"
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
        let path = local_socket_path(host);
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
        local_socket_path("build-box"),
        local_socket_path("build-box")
    );
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
