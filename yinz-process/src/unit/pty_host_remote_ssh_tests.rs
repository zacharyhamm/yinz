//! Tests for the parts of the ssh path that do not need an ssh.
//!
//! Actually reaching another machine belongs to a manual or integration check;
//! what is pinned here is the endpoint contract and the handshake, both of which
//! are easy to break silently and expensive to debug over a network.

use super::*;
use std::process::Stdio;

use std::os::unix::net::UnixStream;

#[test]
fn bootstrap_captures_output_larger_than_both_pipe_buffers() {
    let mut command = Command::new("/bin/sh");
    command.args([
        "-c",
        "head -c 1048576 /dev/zero; printf '\nendpoint\n'; head -c 1048576 /dev/zero >&2; printf 'diagnostic\n' >&2; exit 7",
    ]);
    let output =
        bootstrap_output(&mut command, yinz_test_support::default_timeout(), None).unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(output.stdout.len(), SSH_OUTPUT_CAPACITY);
    assert!(output.stdout.ends_with(b"\nendpoint\n"));
    assert_eq!(output.stderr.len(), SSH_OUTPUT_CAPACITY);
    assert!(output.stderr.ends_with(b"diagnostic\n"));
}

#[test]
fn forward_stderr_drains_a_live_writer_and_keeps_only_its_tail() {
    use std::io::Write;
    use yinz_test_support::Watched;
    let (reader, mut writer) = UnixStream::pair().unwrap();
    let mut capture = SshOutputCapture::new(reader).unwrap();
    let written: Watched<Option<UnixStream>> = Watched::default();
    let completed = written.clone();
    let sending = std::thread::spawn(move || {
        writer.write_all(&vec![b'x'; 2 * 1024 * 1024]).unwrap();
        writer.write_all(b"final diagnostic").unwrap();
        *completed.lock() = Some(writer);
    });
    written.wait_until(
        "SSH stderr to drain while its writer is still running",
        yinz_test_support::default_timeout(),
        |writer| writer.is_some().then_some(()),
    );
    sending.join().unwrap();
    let _still_open = written.lock().take().unwrap();
    let output = capture.finish().unwrap();
    assert_eq!(output.len(), SSH_OUTPUT_CAPACITY);
    assert!(output.ends_with(b"final diagnostic"));
}

#[test]
fn a_broker_endpoint_parses_from_the_line_ensure_prints() {
    let line = serde_json::json!({
        "socket": "/run/user/1000/yinz/cv1/pty-broker-v2.sock",
        "protocol_version": REMOTE_PROTOCOL_VERSION,
        "yinz_version": "0.45.0",
    })
    .to_string();
    let endpoint: BrokerEndpoint = serde_json::from_str(&line).expect("parse failed");
    assert_eq!(
        endpoint.socket,
        "/run/user/1000/yinz/cv1/pty-broker-v2.sock"
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
        let path = local_socket_path("server", host, &SshOptions::default());
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
        local_socket_path("server", "build-box", &SshOptions::default()),
        local_socket_path("server", "build-box", &SshOptions::default())
    );
}

#[test]
fn hosts_that_collided_in_the_old_32_bit_key_have_distinct_control_sockets() {
    // Both host + default-options strings hashed to fa970eb9 under the old key.
    let options = SshOptions::default();
    assert_ne!(
        control_path("host-9374", &options),
        control_path("host-98072", &options)
    );
    // Keep room for OpenSSH's temporary suffix in short Unix socket paths.
    assert_eq!(socket_key("host-9374").len(), 16);
}

#[test]
fn home_fallback_control_path_fits_macos_with_ssh_rename_margin() {
    const PROBE: &str = "YINZ_SSH_HOME_FALLBACK_PROBE";
    if std::env::var_os(PROBE).is_none() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "pty_host::remote::ssh::pty_host_remote_ssh_tests::home_fallback_control_path_fits_macos_with_ssh_rename_margin",
            ])
            .env(PROBE, "1")
            .env("HOME", "/Users/zacharyhamm")
            .env("TMPDIR", "/var/folders/zz/zyxvpxvq6csfxvn_n0000000000000/T")
            .env_remove("YINZ_SOCKET_DIR")
            .env_remove("XDG_RUNTIME_DIR")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let args = ssh_common_args("build-box", &SshOptions::default());
    let path = args
        .iter()
        .find_map(|arg| arg.strip_prefix("ControlPath=\""))
        .and_then(|path| path.strip_suffix('"'))
        .map(Path::new)
        .expect("no quoted ControlPath option");
    assert_eq!(
        path.parent().unwrap(),
        Path::new("/Users/zacharyhamm/.yinz/run")
            .join(format!(
                "cv{}",
                yinz_runtime::consts::CLIENT_SERVER_CONTRACT_VERSION
            ))
            .join("r")
    );
    assert_eq!(path.file_stem().unwrap().len(), 16);
    assert_eq!(path.extension().unwrap(), "ctl");
    // Pin macOS's strict limit even when this test runs on Linux.
    assert!(path.as_os_str().len() + SSH_CONTROL_PATH_RENAME_MARGIN < 104);
    check_socket_path_length(path, SSH_CONTROL_PATH_RENAME_MARGIN).unwrap();
}

/// `YINZ_SOCK_DIR` is per user, not per session, so a host-only name puts two
/// of one person's sessions on the same socket. Then the second session's
/// forward unlinks and rebinds the first's, the first silently routes its panes
/// through the second's ssh, and whichever closes first deletes the other's
/// socket.
#[test]
fn two_servers_do_not_share_a_forward_socket_for_the_same_host() {
    assert_ne!(
        local_socket_path("server-a", "build-box", &SshOptions::default()),
        local_socket_path("server-b", "build-box", &SshOptions::default()),
        "two servers would fight over one forward to the same host"
    );
}

#[test]
fn one_endpoint_with_different_ssh_options_gets_distinct_state() {
    let first = SshTarget::new(
        "build-box",
        SshOptions {
            identity_file: Some("first-key".into()),
            known_hosts_file: Some("first-hosts".into()),
        },
    );
    let second = SshTarget::new(
        "build-box",
        SshOptions {
            identity_file: Some("second-key".into()),
            known_hosts_file: Some("second-hosts".into()),
        },
    );

    assert_ne!(first, second);
    assert_ne!(
        local_socket_path("server", &first.host, &first.options),
        local_socket_path("server", &second.host, &second.options)
    );
    let hosts = Arc::new(RemoteHosts::new());
    let reconnect = SshReconnector {
        hosts,
        target: first.clone(),
    };
    assert_eq!(reconnect.target, first);
}

/// A host beginning with `-` would read as an ssh option, so
/// `-oProxyCommand=…` would run an arbitrary local command. The spawn sites
/// pass `--` before the host, which makes ssh refuse it too; this guard is the
/// up-front check with a readable error. Ordinary destinations must still
/// pass — including ssh_config aliases with `/` or `+` in them and `ssh://`
/// URIs, the only inline form that carries a port. This runs no ssh — it pins
/// the guard `bootstrap` calls before ever building a `Command`.
#[test]
fn validate_ssh_host_rejects_option_lookalikes_and_junk_but_allows_real_hosts() {
    let rejected = [
        "",
        "-oProxyCommand=sh -c 'id'",
        "-ltoor",
        "--",
        "host with spaces",
        "host\nlogin",
    ];
    for host in rejected {
        assert!(
            validate_ssh_host(host).is_err(),
            "expected {host:?} to be rejected as a remote host"
        );
    }

    let allowed = [
        "build-box",
        "example.com",
        "me@build-box.example.com",
        "user@[2001:db8::1]",
        "user@[fe80::1%eth0]",
        "192.168.1.10",
        "ssh://user@host:2222",
        "work/build",
        "gpu+a100",
    ];
    for host in allowed {
        assert!(
            validate_ssh_host(host).is_ok(),
            "expected {host:?} to be accepted as a remote host"
        );
    }
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

#[test]
fn the_socket_sweep_preserves_regular_files_even_when_old() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("important.sock");
    let file = std::fs::File::create(&path).unwrap();
    file.set_modified(std::time::UNIX_EPOCH).unwrap();

    sweep_stale_sockets(dir.path(), &dir.path().join("ours.sock"));

    assert!(path.exists());
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
                yinz_version: "9.9.9".to_owned(),
            },
        )
        .expect("write failed");
        theirs
    });

    let cmd = RunCommand {
        command: std::path::PathBuf::from("/bin/sh"),
        ..Default::default()
    };
    let err = handshake_and_spawn(&ours, "build-box", &cmd, None, TerminalId::new(0), &[])
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
/// `YinzError::CommandNotFound`, not as a generic failure: `pane_start_failure`
/// in pty.rs sorts the survivable failures from the fatal ones by exactly that,
/// and a remote pane should behave like a local one when the command is simply
/// mistyped.
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
                yinz_version: yinz_runtime::consts::VERSION.to_owned(),
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
    let err = handshake_and_spawn(&ours, "build-box", &cmd, None, TerminalId::new(0), &[])
        .expect_err("a missing command must be refused");
    assert!(
        matches!(
            err.downcast_ref::<YinzError>(),
            Some(YinzError::CommandNotFound { .. })
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
                yinz_version: yinz_runtime::consts::VERSION.to_owned(),
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
    handshake_and_spawn(&ours, "build-box", &cmd, None, TerminalId::new(0), &[])
        .expect("handshake should succeed");
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
                yinz_version: yinz_runtime::consts::VERSION.to_owned(),
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
        target: yinz_types::command::PaneTarget::Ssh {
            host: "build-box".to_owned(),
        },
        ..Default::default()
    };
    handshake_and_spawn(&ours, "build-box", &cmd, None, TerminalId::new(0), &[])
        .expect("handshake should succeed");

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

/// A forward whose broker died still accepts locally — the ssh holding it is
/// alive, so `ensure_forward`'s liveness probe says all is well and the
/// handshake is the first thing to notice, by reading EOF. Nothing rebuilds the
/// forward unless that failure does, and until something does, every later pane
/// on that host fails the same way for the life of the session.
#[test]
fn a_pane_whose_forward_has_gone_stale_is_retried_through_a_fresh_one() {
    // `.invalid` is reserved precisely so it never resolves: the retry's ssh
    // fails immediately instead of reaching anything.
    let host = "yinz-test-stale-forward.invalid";
    let dir = std::env::temp_dir().join(format!("yinz-stale-forward-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("could not make a test dir");
    let local_socket = dir.join("forward.sock");
    let _ = std::fs::remove_file(&local_socket);

    // Stands in for a forward that is up with nothing behind it: it accepts,
    // then closes without a word, exactly as ssh does when the socket on the
    // other machine is gone.
    let listener = std::os::unix::net::UnixListener::bind(&local_socket)
        .expect("could not bind a fake forward");
    let accepting = std::thread::spawn(move || {
        for stream in listener.incoming().take(1) {
            drop(stream);
        }
    });
    let ssh = Command::new("sleep")
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("could not stand in for the forward's ssh");

    let hosts = Arc::new(RemoteHosts::new());
    let target = SshTarget::new(host, SshOptions::default());
    hosts.forwards.lock().unwrap().insert(
        target.clone(),
        Arc::new(HostForward {
            host: host.to_owned(),
            local_socket: local_socket.clone(),
            ssh,
            stderr: None,
        }),
    );

    let cmd = RunCommand {
        command: std::path::PathBuf::from("/bin/sh"),
        target: yinz_types::command::PaneTarget::Ssh {
            host: host.to_owned(),
        },
        ..Default::default()
    };
    let msg = match hosts.open_pane(host, &cmd, None, TerminalId::new(0)) {
        Err(err) => format!("{err:#}"),
        Ok(_) => panic!("an unreachable host cannot open a pane"),
    };
    assert!(
        !msg.contains("the broker closed the connection"),
        "the error should come from the retry, not from the stale forward: {msg}"
    );
    assert!(
        !hosts.forwards.lock().unwrap().contains_key(&target),
        "the stale forward must not be left in the table for the next pane"
    );

    let _ = accepting.join();
    let _ = std::fs::remove_file(&local_socket);
}

/// A forward whose ssh is alive locally and dead on the wire accepts and then
/// says nothing at all — no EOF, no reply, ever. The handshake used to wait for
/// that reply with no bound of its own, which on the adopting side of a warp
/// was the pty thread waiting; the fallback to a fresh forward was never
/// reached. A resume through such a forward has to give up in bounded time.
#[test]
fn a_resume_through_a_forward_that_never_answers_gives_up_and_rebuilds() {
    let host = "yinz-test-silent-forward.invalid";
    let dir = std::env::temp_dir().join(format!("yinz-silent-forward-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("could not make a test dir");
    let local_socket = dir.join("forward.sock");
    let _ = std::fs::remove_file(&local_socket);

    // Accepts everything and holds every connection open without a word,
    // until told to stop.
    let listener = std::os::unix::net::UnixListener::bind(&local_socket)
        .expect("could not bind a fake forward");
    listener
        .set_nonblocking(true)
        .expect("could not make the fake forward non-blocking");
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let holding = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let mut held = Vec::new();
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => held.push(stream),
                    Err(_) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
        })
    };
    let ssh = Command::new("sleep")
        .arg("60")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("could not stand in for the forward's ssh");

    let hosts = Arc::new(RemoteHosts::new());
    let target = SshTarget::new(host, SshOptions::default());
    hosts.forwards.lock().unwrap().insert(
        target.clone(),
        Arc::new(HostForward {
            host: host.to_owned(),
            local_socket: local_socket.clone(),
            ssh,
            stderr: None,
        }),
    );

    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let resuming = hosts.clone();
    let resume_target = target.clone();
    std::thread::spawn(move || {
        let started = Instant::now();
        let outcome = resuming
            .try_resume_target(&resume_target, "tok", 0)
            .map(|_| ());
        let _ = done_tx.send((outcome, started.elapsed()));
    });
    // Generous next to the bound under test, and a *test* failure rather than
    // a hang if the bound is gone.
    let (outcome, took) = done_rx
        .recv_timeout(RESUME_REPLY_TIMEOUT * 4)
        .expect("the resume never gave up on a forward that never answered");
    stop.store(true, std::sync::atomic::Ordering::Relaxed);

    let msg = match outcome {
        Err(err) => format!("{err:#}"),
        Ok(()) => panic!("an unreachable host cannot resume a pane"),
    };
    assert!(
        took >= RESUME_REPLY_TIMEOUT,
        "the first attempt should have waited out its bound, not failed early: {took:?}"
    );
    assert!(
        msg.contains("did not work either"),
        "the retry through a fresh forward should have been reached: {msg}"
    );
    assert!(
        !hosts.forwards.lock().unwrap().contains_key(&target),
        "the silent forward must not be left in the table for the next pane"
    );

    let _ = holding.join();
    let _ = std::fs::remove_file(&local_socket);
}

/// This side's terminal and locale reach the far side on the `Spawn` itself,
/// along with the pane's real id. Nothing else does: the rest of what a pane
/// needs is recomputed over there, because the rest of it names a filesystem
/// this machine cannot see.
#[test]
fn a_pane_carries_this_sides_terminal_and_its_own_id() {
    let (ours, theirs) = UnixStream::pair().expect("socketpair failed");
    let peer = std::thread::spawn(move || {
        let mut theirs = theirs;
        let _ = protocol::read_frame(&mut theirs);
        protocol::write_control(
            &mut theirs,
            &BrokerToServerMsg::HelloAck {
                protocol_version: REMOTE_PROTOCOL_VERSION,
                yinz_version: yinz_runtime::consts::VERSION.to_owned(),
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
        target: yinz_types::command::PaneTarget::Ssh {
            host: "build-box".to_owned(),
        },
        ..Default::default()
    };
    let carried = vec![
        ("TERM".to_owned(), "xterm-256color".to_owned()),
        ("LC_TIME".to_owned(), "C".to_owned()),
    ];
    handshake_and_spawn(
        &ours,
        "build-box",
        &cmd,
        None,
        TerminalId::new(42),
        &carried,
    )
    .expect("handshake should succeed");

    let (_theirs, spawn) = peer.join().expect("peer panicked");
    let body = match spawn {
        protocol::Frame::Control(body) => body,
        other => panic!("expected a control frame, got {other:?}"),
    };
    let msg: ServerToBrokerMsg = serde_json::from_slice(&body).expect("undecodable spawn");
    match msg {
        ServerToBrokerMsg::Spawn {
            env, terminal_id, ..
        } => {
            assert_eq!(env, carried);
            assert_eq!(
                terminal_id, 42,
                "the far side needs the real id to set a truthful YINZ_PANE_ID"
            );
        },
        other => panic!("expected a Spawn, got {other:?}"),
    }
}

#[test]
fn known_hosts_and_control_paths_reach_ssh_quoted() {
    let options = SshOptions {
        identity_file: Some("/home/a user/key".into()),
        known_hosts_file: Some("/home/a user/known_hosts".into()),
    };
    let args = options.args();
    let known_hosts = args
        .iter()
        .position(|arg| arg == r#"UserKnownHostsFile="/home/a user/known_hosts""#)
        .expect("the known_hosts path was not quoted");
    assert_eq!(args[known_hosts - 1], "-o");
    // `-i` takes its whole argument, so the key path stays bare.
    let key = args.iter().position(|arg| arg == "-i").unwrap();
    assert_eq!(args[key + 1], "/home/a user/key");

    let common = ssh_common_args("build-box", &options);
    let control_path = common
        .iter()
        .find_map(|arg| arg.strip_prefix("ControlPath="))
        .expect("no ControlPath option");
    assert!(
        control_path.starts_with('"') && control_path.ends_with('"'),
        "ControlPath value is not quoted: {control_path}"
    );
}
