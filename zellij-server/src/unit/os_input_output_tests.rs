const AMPLE_TIMEOUT: Duration = Duration::from_secs(30);

use super::*;
use zellij_types::command::PaneTarget;
use zellij_types::input::command::RunCommand;
use zellij_types::internal_ids::ProcessId;

fn make_server() -> ServerOsInputOutput {
    get_server_os_input().expect("failed to create server os input")
}

#[test]
fn firecracker_resolution_keeps_an_unresolved_command_for_reruns() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("provider");
    zellij_test_support::write_warm_script(
        &path,
        "printf '{\"version\":2,\"ssh_host\":\"build-box\",\"cwd\":\"/srv/project\"}'\n",
    );

    let server = make_server();
    *server.firecracker.write().unwrap() = Some((
        crate::firecracker::Provider::new(path, AMPLE_TIMEOUT).unwrap(),
        "work".to_owned(),
    ));
    let requested = RunCommand {
        command: PathBuf::from("claude"),
        target: PaneTarget::Firecracker {
            vm: "review".to_owned(),
            image: None,
            worktree: None,
        },
        ..Default::default()
    };

    let (resolved, rerun, endpoint) = server.resolve_target(requested.clone(), None).unwrap();
    let endpoint = endpoint.expect("a Firecracker command resolves to an endpoint");
    assert_eq!(endpoint.host, "build-box");
    assert_eq!(resolved.target, requested.target);
    assert_eq!(resolved.cwd, Some(PathBuf::from("/srv/project")));
    assert_eq!(rerun, requested);
    assert_eq!(endpoint.ssh_options, SshOptions::default());
    // The whole point of keeping the rerun unresolved: resolving it again lands on the
    // same VM, cwd and endpoint, not merely on something.
    let (again, again_rerun, again_endpoint) = server.resolve_target(rerun.clone(), None).unwrap();
    assert_eq!(again, resolved);
    assert_eq!(again_rerun, rerun);
    assert_eq!(again_endpoint, Some(endpoint));

    let explicit = RunCommand {
        command: PathBuf::from("claude"),
        cwd: Some(PathBuf::from("/elsewhere")),
        target: PaneTarget::Firecracker {
            vm: "review".to_owned(),
            image: None,
            worktree: None,
        },
        ..Default::default()
    };
    assert_eq!(
        server.resolve_target(explicit, None).unwrap().0.cwd,
        Some(PathBuf::from("/elsewhere"))
    );
}

/// A pane that names a VM and no worktree - `zellij action new-pane --firecracker review`
/// joining a VM somebody else provisioned - has no way of its own to say which checkout
/// the guest copy came from. The provider does, and the answer is adopted onto the pane so
/// everything downstream can use it: the daemon working out the pane's project, the layout
/// that serializes it, a duplicate of the pane.
#[test]
fn a_firecracker_pane_adopts_the_worktree_the_provider_names() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("provider");
    zellij_test_support::write_warm_script(
        &path,
        "printf '{\"version\":2,\"ssh_host\":\"build-box\",\"cwd\":\"/workspace\",\
         \"worktree\":\"/work/hocus\"}'\n",
    );

    let server = make_server();
    *server.firecracker.write().unwrap() = Some((
        crate::firecracker::Provider::new(path, AMPLE_TIMEOUT).unwrap(),
        "work".to_owned(),
    ));
    let joined = RunCommand {
        command: PathBuf::from("claude"),
        target: PaneTarget::Firecracker {
            vm: "review".to_owned(),
            image: None,
            worktree: None,
        },
        ..Default::default()
    };

    let (resolved, rerun, _) = server.resolve_target(joined, None).unwrap();
    assert_eq!(
        resolved.target.firecracker_worktree(),
        Some(Path::new("/work/hocus"))
    );
    // The rerun command too, so a held pane that is run again keeps the identity rather
    // than waiting for the next ensure to hand it back. Not redundant with the resolved
    // command: this is the one `HoldPane` carries, and it is the only word the layout
    // serializer and the agent restart hook get - the manifest merge patches `PaneInfo`
    // and never the pane's `invoked_with`.
    assert_eq!(
        rerun.target.firecracker_worktree(),
        Some(Path::new("/work/hocus"))
    );

    // What the caller named is what the VM was ensured with, so it is never overridden.
    let named = RunCommand {
        command: PathBuf::from("claude"),
        target: PaneTarget::Firecracker {
            vm: "review".to_owned(),
            image: None,
            worktree: Some(PathBuf::from("/work/other")),
        },
        ..Default::default()
    };
    assert_eq!(
        server
            .resolve_target(named, None)
            .unwrap()
            .0
            .target
            .firecracker_worktree(),
        Some(Path::new("/work/other"))
    );
}

#[test]
fn rename_firecracker_session_updates_the_session_key() {
    use std::fs;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("provider");
    zellij_test_support::write_warm_script(
        &path,
        "cat >> \"$(dirname \"$0\")/requests\"\nprintf '{\"version\":2}'\n",
    );
    let server = make_server();
    *server.firecracker.write().unwrap() = Some((
        crate::firecracker::Provider::new(path, AMPLE_TIMEOUT).unwrap(),
        "work".to_owned(),
    ));

    server.rename_firecracker_session_now("renamed").unwrap();

    let requests = fs::read_to_string(dir.path().join("requests")).unwrap();
    assert_eq!(
        requests,
        r#"{"action":"rename_session","version":2,"from":"work","to":"renamed"}"#
    );
    let key = server
        .firecracker
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .1
        .clone();
    assert_eq!(key, "renamed");

    // The same name again is not a request.
    server.rename_firecracker_session_now("renamed").unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("requests")).unwrap(),
        requests
    );
}

/// Two renames in quick succession reach the provider in order, and the
/// server ends up keyed on the last name: the rename worker applies what it
/// was last asked for rather than whichever thread wins a lock.
#[test]
fn two_quick_renames_leave_the_provider_keyed_on_the_last_name() {
    use std::fs;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("provider");
    zellij_test_support::write_warm_script(
        &path,
        "sleep 0.2\ncat >> \"$(dirname \"$0\")/requests\"\nprintf '{\"version\":2}'\n",
    );
    let server = make_server();
    *server.firecracker.write().unwrap() = Some((
        crate::firecracker::Provider::new(path, AMPLE_TIMEOUT).unwrap(),
        "a".to_owned(),
    ));

    server.rename_firecracker_session("b".to_owned());
    server.rename_firecracker_session("c".to_owned());
    server.firecracker_renames.close_and_join();

    let key = server
        .firecracker
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .1
        .clone();
    assert_eq!(key, "c");
    let requests = fs::read_to_string(dir.path().join("requests")).unwrap();
    assert!(
        requests.ends_with(r#""to":"c"}"#),
        "the last request must be the rename to c, got {requests}"
    );
    assert!(
        !requests.contains(r#""from":"c","to":"b""#),
        "a stale rename ran after the newer one: {requests}"
    );
}

/// A session shutting down does not wait out a VM boot: the flag set by
/// `stop_firecracker_session` cuts a running `ensure` short.
#[test]
fn stopping_the_session_cancels_an_ensure_in_flight() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("provider");
    // The marker says the ensure is genuinely in flight when the shutdown begins; without
    // it the pre-ensure check answers with the same message and the test proves nothing.
    zellij_test_support::write_warm_script(
        &path,
        "touch \"$(dirname \"$0\")/ensuring\"\nsleep 30\nprintf '{\"version\":2,\"ssh_host\":\"build-box\"}'\n",
    );
    let ensuring = dir.path().join("ensuring");
    let server = make_server();
    *server.firecracker.write().unwrap() = Some((
        crate::firecracker::Provider::new(path, Duration::from_secs(60)).unwrap(),
        "work".to_owned(),
    ));
    let requested = RunCommand {
        command: PathBuf::from("claude"),
        target: PaneTarget::Firecracker {
            vm: "review".to_owned(),
            image: None,
            worktree: None,
        },
        ..Default::default()
    };

    let (resolved_tx, resolved_rx) = std::sync::mpsc::channel();
    {
        let server = server.clone();
        std::thread::spawn(move || {
            let _ = resolved_tx.send(server.resolve_target(requested, None));
        });
    }
    let started = std::time::Instant::now();
    while !ensuring.exists() {
        assert!(
            started.elapsed() < AMPLE_TIMEOUT,
            "the provider was never asked to ensure the VM"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    server.begin_firecracker_shutdown();
    // Bounded here rather than by nextest's kill: a cancel that does not reach the
    // provider would otherwise wait out the whole `sleep 30` and read as a timeout.
    let resolved = resolved_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the ensure was waited out instead of cancelled");
    let error = resolved
        .expect_err("a cancelled ensure is an error")
        .to_string();
    assert!(error.contains("shutting down"), "{error}");
}

/// A provider that refuses the rename leaves the server on the old name too, so the two
/// still agree on which session owns the VMs.
#[test]
fn a_refused_rename_keeps_the_old_firecracker_session_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("provider");
    zellij_test_support::write_warm_script(&path, "echo 'unknown action' >&2\nexit 1\n");
    let server = make_server();
    *server.firecracker.write().unwrap() = Some((
        crate::firecracker::Provider::new(path, AMPLE_TIMEOUT).unwrap(),
        "work".to_owned(),
    ));

    let error = server
        .rename_firecracker_session_now("renamed")
        .unwrap_err()
        .to_string();
    assert!(error.contains("stay under \"work\""), "{error}");
    let key = server
        .firecracker
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .1
        .clone();
    assert_eq!(key, "work");
}

/// `stop_firecracker_session` does not depend on the pane teardown before it having
/// succeeded: a remote pane whose transport is already gone fails `clear_terminal_id`, and
/// the provider is still told to stop. That the server makes the two calls in that order is
/// the integration suite's to check (`firecracker_shutdown`), since the order lives in
/// `lib.rs` and not in either call.
#[test]
fn stop_firecracker_session_is_independent_of_a_failed_pane_teardown() {
    use crate::pty_host::remote::RemoteConn;
    use std::os::unix::net::UnixStream;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("provider");
    zellij_test_support::write_warm_script(
        &path,
        "touch \"$(dirname \"$0\")/called\"\nprintf '{\"version\":2}'\n",
    );
    let server = make_server();
    *server.firecracker.write().unwrap() = Some((
        crate::firecracker::Provider::new(path, AMPLE_TIMEOUT).unwrap(),
        "work".to_owned(),
    ));
    let (write_socket, broker_peer) = UnixStream::pair().unwrap();
    drop(broker_peer);
    let (read_socket, _reader_peer) = UnixStream::pair().unwrap();
    let command = RunCommand {
        command: PathBuf::from("remote-step"),
        target: PaneTarget::Firecracker {
            vm: "review".to_owned(),
            image: None,
            worktree: None,
        },
        ..Default::default()
    };
    let connection = Arc::new(
        RemoteConn::new(
            write_socket,
            "build-box".to_owned(),
            "token".to_owned(),
            command.clone(),
            None,
        )
        .unwrap(),
    );
    let _reader = server
        .pty_backend
        .register_remote_terminal(
            crate::TerminalId::new(7),
            connection.clone(),
            read_socket,
            Box::new(|_, _, _| {}),
            command,
        )
        .unwrap();
    connection.mark_transport_lost();

    assert!(server.clear_terminal_id(crate::TerminalId::new(7)).is_err());
    server.stop_firecracker_session().unwrap();
    assert!(dir.path().join("called").exists());
}

#[allow(dead_code)]
fn long_running_cmd() -> Command {
    let mut cmd = Command::new("sleep");
    cmd.arg("60");
    cmd
}

#[allow(dead_code)]
fn echo_cmd(msg: &str) -> Command {
    let mut cmd = Command::new("echo");
    cmd.arg(msg);
    cmd
}

/// Wait, bounded, for a signalled child to die; the signal it died of, or a panic if it
/// is still alive at the deadline (it is killed then, so the test cannot hang on it).
fn signal_that_killed(child: &mut std::process::Child, deadline: Duration) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    let started = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("try_wait failed") {
            return status.signal();
        }
        if started.elapsed() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the process survived the signal for {deadline:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn get_cwd() {
    let server = make_server();

    let pid = std::process::id();
    let expected = std::env::current_dir().unwrap().canonicalize().unwrap();
    assert_eq!(
        server
            .get_cwd(ProcessId::new(pid))
            .map(|cwd| cwd.canonicalize().unwrap()),
        Some(expected),
        "the cwd of PID {pid} is this process's cwd"
    );
    // Above any `pid_max` the kernel allows, so the process cannot exist; the fallthrough
    // is what a caller reads for a pane whose process has already gone.
    assert_eq!(server.get_cwd(ProcessId::new(i32::MAX as u32)), None);
}

// --- Signal delivery tests ---

#[test]
fn kill_sends_sighup_to_process() {
    let mut child = long_running_cmd()
        .spawn()
        .expect("failed to spawn long-running process");
    let pid = child.id();

    let server = make_server();

    // `kill` discards the errno, so its `Ok` says nothing; what the child died of does.
    server
        .kill(ProcessId::new(pid))
        .expect("kill should succeed");

    assert_eq!(
        signal_that_killed(&mut child, Duration::from_secs(5)),
        Some(libc::SIGHUP)
    );
}

#[test]
fn force_kill_sends_sigkill_to_process() {
    let mut child = long_running_cmd()
        .spawn()
        .expect("failed to spawn long-running process");
    let pid = child.id();

    let server = make_server();

    server
        .force_kill(ProcessId::new(pid))
        .expect("force_kill should succeed");

    assert_eq!(
        signal_that_killed(&mut child, Duration::from_secs(5)),
        Some(libc::SIGKILL)
    );
}

#[test]
fn send_sigint_to_process() {
    // `sleep` keeps SIGINT's default disposition, so it dies of it rather than ignoring it.
    let mut child = long_running_cmd()
        .spawn()
        .expect("failed to spawn long-running process");
    let pid = child.id();

    let server = make_server();

    server
        .send_sigint(ProcessId::new(pid))
        .expect("send_sigint should succeed");

    assert_eq!(
        signal_that_killed(&mut child, Duration::from_secs(5)),
        Some(libc::SIGINT)
    );
}

#[test]
fn spawn_and_read_output() {
    use crate::panes::PaneId;
    use zellij_types::input::command::TerminalAction;

    let server = make_server();
    let test_message = "hello_zellij_test";
    std::env::set_var(
        crate::pty_host::PANE_REF_ENV_KEY,
        "an-outer-panes-reference",
    );
    let token = crate::pty_host::remote::broker::PANE_TOKEN_ENV;
    std::env::set_var(token, "an-outer-panes-token");

    let cmd = RunCommand {
        command: PathBuf::from("sh"),
        args: vec![
            "-c".to_owned(),
            // Both before the message the read loop waits for, so seeing it is proof
            // the whole line arrived.
            format!("echo ref=[$ZELLIJ_PANE_REF] token=[${token}] {test_message}"),
        ],
        ..Default::default()
    };
    let action = TerminalAction::RunCommand(cmd);
    let quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send> =
        Box::new(|_pane_id, _exit_status, _run_command| {});

    let (_terminal_id, mut reader, _child_pid) = server
        .spawn_terminal(action, quit_cb, None)
        .expect("spawn_terminal should succeed");

    // Read output from the spawned terminal
    let mut output = Vec::new();
    let mut buf = [0u8; 4096];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async {
        loop {
            if std::time::Instant::now() > deadline {
                break;
            }
            match tokio::time::timeout(std::time::Duration::from_millis(500), reader.read(&mut buf))
                .await
            {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => {
                    output.extend_from_slice(&buf[..n]);
                    let s = String::from_utf8_lossy(&output);
                    if s.contains(test_message) {
                        break;
                    }
                },
                Ok(Err(_)) => break,
                Err(_) => {
                    // timeout — check if we already have enough
                    let s = String::from_utf8_lossy(&output);
                    if s.contains(test_message) {
                        break;
                    }
                },
            }
        }
    });

    let output_str = String::from_utf8_lossy(&output);
    assert!(
        output_str.contains(test_message),
        "expected output to contain '{}', got: '{}'",
        test_message,
        output_str
    );
    assert!(
        output_str.contains("ref=[]"),
        "an in-process pane reused an outer pane's supervisor: {output_str:?}"
    );
    // A server running inside a remote pane would otherwise hand every pane the outer
    // pane's token, which a coding agent's hook reads before the pane id.
    assert!(
        output_str.contains("token=[]"),
        "an in-process pane reused an outer pane's broker token: {output_str:?}"
    );
    std::env::remove_var(crate::pty_host::PANE_REF_ENV_KEY);
    std::env::remove_var(token);
}

#[cfg(unix)]
#[test]
fn a_local_rerun_terminates_the_previous_remote_transport() {
    use crate::pty_host::remote::{
        protocol::{read_frame, write_control, BrokerToServerMsg, ServerToBrokerMsg},
        RemoteConn,
    };
    use std::os::unix::net::UnixStream;

    let server = make_server();
    let (write_socket, mut broker_peer) = UnixStream::pair().unwrap();
    let (frame_tx, frame_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let frame = read_frame(&mut broker_peer);
        write_control(
            &mut broker_peer,
            &BrokerToServerMsg::ChildExited { status: Some(0) },
        )
        .unwrap();
        let _ = frame_tx.send(frame);
    });
    let (read_socket, _reader_peer) = UnixStream::pair().unwrap();
    let remote_command = RunCommand {
        command: PathBuf::from("remote-step"),
        target: PaneTarget::Ssh {
            host: "build-box".to_owned(),
        },
        ..Default::default()
    };
    let connection = Arc::new(
        RemoteConn::new(
            write_socket,
            "build-box".to_owned(),
            "token".to_owned(),
            remote_command.clone(),
            None,
        )
        .unwrap(),
    );
    let _remote_reader = server
        .pty_backend
        .register_remote_terminal(
            crate::TerminalId::new(7),
            connection,
            read_socket,
            Box::new(|_, _, _| {}),
            remote_command,
        )
        .unwrap();

    let (_local_reader, child_pid) = server
        .re_run_command_in_terminal(
            crate::TerminalId::new(7),
            RunCommand {
                command: PathBuf::from("sleep"),
                args: vec!["60".to_owned()],
                ..Default::default()
            },
            Box::new(|_, _, _| {}),
        )
        .unwrap();

    let frame = frame_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("the old broker did not receive a termination frame")
        .unwrap()
        .unwrap();
    assert_eq!(
        frame.decode_control::<ServerToBrokerMsg>().unwrap(),
        ServerToBrokerMsg::Terminate
    );
    let child_pid = child_pid.expect("the rerun should have started a local child");
    server.force_kill(child_pid).unwrap();
    server.clear_terminal_id(crate::TerminalId::new(7)).unwrap();
}

/// Rerunning locally tears the old remote transport down first, and that teardown can fail:
/// a broker whose connection is already known to be lost refuses the `Terminate`. The failure
/// must not abort the rerun, or the pane would be left dead for the sake of a transport that
/// was gone anyway.
#[cfg(unix)]
#[test]
fn a_dead_remote_transport_does_not_prevent_a_local_rerun() {
    use crate::pty_host::remote::RemoteConn;
    use std::os::unix::net::UnixStream;

    let server = make_server();
    let (write_socket, broker_peer) = UnixStream::pair().unwrap();
    drop(broker_peer);
    let (read_socket, _reader_peer) = UnixStream::pair().unwrap();
    let remote_command = RunCommand {
        command: PathBuf::from("remote-step"),
        target: PaneTarget::Ssh {
            host: "build-box".to_owned(),
        },
        ..Default::default()
    };
    let connection = Arc::new(
        RemoteConn::new(
            write_socket,
            "build-box".to_owned(),
            "token".to_owned(),
            remote_command.clone(),
            None,
        )
        .unwrap(),
    );
    let _remote_reader = server
        .pty_backend
        .register_remote_terminal(
            crate::TerminalId::new(7),
            connection.clone(),
            read_socket,
            Box::new(|_, _, _| {}),
            remote_command,
        )
        .unwrap();
    // A dropped peer alone is not enough: the first failed write only *marks* the connection
    // broken and reports success. Marking it lost up front is what the reader does on a
    // hangup, and it makes the teardown's `Terminate` genuinely fail.
    connection.mark_transport_lost();
    assert!(
        connection.terminate().is_err(),
        "precondition: tearing down the lost transport must fail"
    );

    let (_reader, child_pid) = server
        .re_run_command_in_terminal(
            crate::TerminalId::new(7),
            RunCommand {
                command: PathBuf::from("sleep"),
                args: vec!["60".to_owned()],
                ..Default::default()
            },
            Box::new(|_, _, _| {}),
        )
        .expect("a failed transport teardown must not abort the local rerun");

    let child_pid = child_pid.expect("the rerun should have started a local child");
    server.force_kill(child_pid).unwrap();
    server.clear_terminal_id(crate::TerminalId::new(7)).unwrap();
}

/// A Firecracker pane is refused a warp before the release is attempted — the
/// refusal is a message, where a failed release is a stranded pane — while an
/// ordinary remote pane whose transport is up is allowed through.
#[cfg(unix)]
#[test]
fn firecracker_panes_are_refused_a_warp_and_remote_panes_are_not() {
    use crate::pty_host::remote::RemoteConn;
    use std::os::unix::net::UnixStream;

    let server = make_server();
    // The peers are kept for the whole test: a dropped one reads as a lost
    // transport, which is a different refusal from the one under test.
    let mut kept_alive = Vec::new();
    let mut register = |terminal_id: zellij_types::internal_ids::TerminalId,
                        command: RunCommand| {
        let (write_socket, broker_peer) = UnixStream::pair().unwrap();
        let (read_socket, reader_peer) = UnixStream::pair().unwrap();
        kept_alive.push((broker_peer, reader_peer));
        let connection = Arc::new(
            RemoteConn::new(
                write_socket,
                "build-box".to_owned(),
                "token".to_owned(),
                command.clone(),
                None,
            )
            .unwrap(),
        );
        server
            .pty_backend
            .register_remote_terminal(
                terminal_id,
                connection,
                read_socket,
                Box::new(|_, _, _| {}),
                command,
            )
            .unwrap()
    };
    let _firecracker_reader = register(
        zellij_types::internal_ids::TerminalId::new(7),
        RunCommand {
            command: PathBuf::from("claude"),
            target: PaneTarget::Firecracker {
                vm: "review".to_owned(),
                image: None,
                worktree: None,
            },
            ..Default::default()
        },
    );
    let _remote_reader = register(
        zellij_types::internal_ids::TerminalId::new(8),
        RunCommand {
            command: PathBuf::from("remote-step"),
            target: PaneTarget::Ssh {
                host: "build-box".to_owned(),
            },
            ..Default::default()
        },
    );

    let error = server
        .check_terminal_can_be_warped(crate::TerminalId::new(7))
        .unwrap_err()
        .to_string();
    assert!(error.contains("Firecracker panes"), "{error}");
    server
        .check_terminal_can_be_warped(crate::TerminalId::new(8))
        .unwrap();

    server.clear_terminal_id(crate::TerminalId::new(7)).unwrap();
    server.clear_terminal_id(crate::TerminalId::new(8)).unwrap();
}

#[cfg(unix)]
#[test]
fn tcgetpgrp_returns_foreground_group() {
    use crate::panes::PaneId;
    use zellij_types::input::command::TerminalAction;

    let server = make_server();

    // `login_tty` in the child makes the spawned command the controlling terminal's
    // foreground process group, so tcgetpgrp(master) should return its pid
    let cmd = RunCommand {
        command: PathBuf::from("sleep"),
        args: vec!["60".to_string()],
        ..Default::default()
    };
    let quit_cb: Box<dyn Fn(PaneId, Option<i32>, RunCommand) + Send> = Box::new(|_, _, _| {});
    let (terminal_id, _reader, child_pid) = server
        .spawn_terminal(TerminalAction::RunCommand(cmd), quit_cb, None)
        .expect("spawn_terminal should succeed");

    // Without the pid the equality below has nothing to compare against, and "some positive
    // pgid" is not the assertion.
    let child_pid = child_pid.expect("spawn_terminal should report the child's pid");

    // poll (bounded to ~2s) for the child to finish setting up its controlling terminal
    let mut fpgid = None;
    for _ in 0..100 {
        fpgid = server.pty_backend.tcgetpgrp(terminal_id);
        if fpgid.and_then(|p| ProcessId::try_from(p).ok()) == Some(child_pid) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(
        fpgid.and_then(|p| ProcessId::try_from(p).ok()),
        Some(child_pid),
        "the spawned command leads its own foreground group, tcgetpgrp gave {fpgid:?}"
    );

    // An unknown terminal id has no fd and must not panic.
    assert_eq!(
        server
            .pty_backend
            .tcgetpgrp(crate::TerminalId::new(u32::MAX)),
        None
    );

    let _ = server.force_kill(child_pid);
}

mod coalescing_send_queue {
    use super::*;
    use zellij_types::data::PaneId as DataPaneId;

    fn render(content: &str) -> ServerToClientMsg {
        ServerToClientMsg::Render {
            content: content.to_owned(),
        }
    }

    fn control(line: &str) -> ServerToClientMsg {
        ServerToClientMsg::Log {
            lines: vec![line.to_owned()],
        }
    }

    fn pane_render(pane_id: DataPaneId, content: &str) -> ServerToClientMsg {
        ServerToClientMsg::PaneRenderUpdate {
            pane_id,
            viewport: vec![content.to_owned()],
            scrollback: None,
            is_initial: false,
        }
    }

    #[test]
    fn renders_coalesce_to_the_newest() {
        let queue = CoalescingSendQueue::new();
        assert!(matches!(queue.push(render("first")), SendQueuePush::Queued));
        assert!(matches!(
            queue.push(render("second")),
            SendQueuePush::CoalescedRender
        ));
        assert!(matches!(
            queue.push(render("third")),
            SendQueuePush::CoalescedRender
        ));
        assert_eq!(queue.len(), 1);
        queue.close();
        assert_eq!(queue.pop(), Some(render("third")));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn control_messages_stay_fifo_and_precede_the_render() {
        let queue = CoalescingSendQueue::new();
        assert!(matches!(queue.push(render("stale")), SendQueuePush::Queued));
        assert!(matches!(queue.push(control("a")), SendQueuePush::Queued));
        assert!(matches!(queue.push(control("b")), SendQueuePush::Queued));
        queue.close();
        assert_eq!(queue.pop(), Some(control("a")));
        assert_eq!(queue.pop(), Some(control("b")));
        assert_eq!(queue.pop(), Some(render("stale")));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn control_queue_fills_while_renders_still_coalesce() {
        let queue = CoalescingSendQueue::new();
        for i in 0..SEND_QUEUE_CONTROL_CAPACITY {
            assert!(matches!(
                queue.push(control(&i.to_string())),
                SendQueuePush::Queued
            ));
        }
        assert!(matches!(
            queue.push(control("overflow")),
            SendQueuePush::ControlQueueFull
        ));
        assert!(matches!(queue.push(render("r1")), SendQueuePush::Queued));
        assert!(matches!(
            queue.push(render("r2")),
            SendQueuePush::CoalescedRender
        ));
    }

    /// The shutdown shape: the writer is popping while the router closes the queue under
    /// it. `pop` checks for queued messages before it checks `closed`, which is the only
    /// reason a message queued before the close cannot be lost to it.
    #[test]
    fn a_close_racing_a_pop_still_drains_everything_queued() {
        const QUEUED: usize = SEND_QUEUE_CONTROL_CAPACITY;
        let queue = Arc::new(CoalescingSendQueue::new());
        for i in 0..QUEUED {
            assert!(matches!(
                queue.push(control(&i.to_string())),
                SendQueuePush::Queued
            ));
        }
        let popper = {
            let queue = queue.clone();
            std::thread::spawn(move || {
                let mut got = Vec::new();
                while let Some(msg) = queue.pop() {
                    got.push(msg);
                }
                got
            })
        };
        queue.close();
        let expected: Vec<ServerToClientMsg> =
            (0..QUEUED).map(|i| control(&i.to_string())).collect();
        assert_eq!(popper.join().unwrap(), expected);
    }

    #[test]
    fn blocking_pop_wakes_on_push() {
        let queue = Arc::new(CoalescingSendQueue::new());
        let popper = {
            let queue = queue.clone();
            std::thread::spawn(move || queue.pop())
        };
        // give the popper a moment to block
        std::thread::sleep(std::time::Duration::from_millis(50));
        queue.push(render("wake up"));
        assert_eq!(popper.join().unwrap(), Some(render("wake up")));
    }

    /// A `pop` already waiting on an empty queue is ended, with `None`, by the close. That
    /// `pop` blocks at all is `blocking_pop_wakes_on_push`'s to show; the handshake here only
    /// orders the pop before the close, without a sleep standing in for that order.
    #[test]
    fn closing_an_empty_queue_ends_a_pop_that_was_already_waiting() {
        let queue = Arc::new(CoalescingSendQueue::new());
        let (popping, is_popping) = std::sync::mpsc::channel();
        let popper = {
            let queue = queue.clone();
            std::thread::spawn(move || {
                popping.send(()).expect("nobody waiting for the popper");
                queue.pop()
            })
        };
        is_popping.recv().expect("the popper never started");
        queue.close();
        assert_eq!(popper.join().unwrap(), None);
    }

    #[test]
    fn push_after_close_is_dropped() {
        // late sends after the writer is gone pretend to succeed so senders
        // don't error, but nothing is queued
        let queue = CoalescingSendQueue::new();
        queue.close();
        assert!(matches!(queue.push(control("late")), SendQueuePush::Queued));
        assert!(matches!(queue.push(render("late")), SendQueuePush::Queued));
        assert_eq!(queue.len(), 0);
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn pane_renders_coalesce_per_pane() {
        let queue = CoalescingSendQueue::new();
        let pane_a = DataPaneId::Terminal(TerminalId::new(1));
        let pane_b = DataPaneId::Terminal(TerminalId::new(2));
        assert!(matches!(
            queue.push(pane_render(pane_a, "a1")),
            SendQueuePush::Queued
        ));
        assert!(matches!(
            queue.push(pane_render(pane_b, "b1")),
            SendQueuePush::Queued
        ));
        assert!(matches!(
            queue.push(pane_render(pane_a, "a2")),
            SendQueuePush::CoalescedRender
        ));
        assert_eq!(queue.len(), 2);
        queue.close();
        assert_eq!(queue.pop(), Some(pane_render(pane_a, "a2")));
        assert_eq!(queue.pop(), Some(pane_render(pane_b, "b1")));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn initial_and_scrollback_pane_renders_stay_fifo() {
        // initial snapshots and scrollback responses are one-offs, not a stream
        // of repaints - they take the FIFO path, so they precede coalesced
        // updates for the same pane
        let queue = CoalescingSendQueue::new();
        let pane = DataPaneId::Terminal(TerminalId::new(1));
        let initial = ServerToClientMsg::PaneRenderUpdate {
            pane_id: pane,
            viewport: vec!["initial".to_owned()],
            scrollback: None,
            is_initial: true,
        };
        let with_scrollback = ServerToClientMsg::PaneRenderUpdate {
            pane_id: pane,
            viewport: vec!["current".to_owned()],
            scrollback: Some(vec!["older".to_owned()]),
            is_initial: false,
        };
        assert!(matches!(
            queue.push(pane_render(pane, "coalesced")),
            SendQueuePush::Queued
        ));
        assert!(matches!(queue.push(initial.clone()), SendQueuePush::Queued));
        assert!(matches!(
            queue.push(with_scrollback.clone()),
            SendQueuePush::Queued
        ));
        queue.close();
        assert_eq!(queue.pop(), Some(initial));
        assert_eq!(queue.pop(), Some(with_scrollback));
        assert_eq!(queue.pop(), Some(pane_render(pane, "coalesced")));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn drain_order_is_control_then_pane_renders_then_full_render() {
        let queue = CoalescingSendQueue::new();
        let pane = DataPaneId::Plugin(zellij_types::internal_ids::PluginId::new(7));
        assert!(matches!(queue.push(render("full")), SendQueuePush::Queued));
        assert!(matches!(
            queue.push(pane_render(pane, "pane")),
            SendQueuePush::Queued
        ));
        assert!(matches!(queue.push(control("ctl")), SendQueuePush::Queued));
        queue.close();
        assert_eq!(queue.pop(), Some(control("ctl")));
        assert_eq!(queue.pop(), Some(pane_render(pane, "pane")));
        assert_eq!(queue.pop(), Some(render("full")));
        assert_eq!(queue.pop(), None);
    }

    // ---- property-based tests (hegel) ----

    use hegel::generators;

    /// Small pane pool on purpose: coalescing only shows up when the same pane
    /// keeps rendering.
    #[hegel::composite]
    fn any_data_pane_id(tc: &hegel::TestCase) -> DataPaneId {
        let n = tc.draw(generators::integers::<u32>().max_value(2));
        if tc.draw(generators::booleans()) {
            DataPaneId::Terminal(zellij_types::internal_ids::TerminalId::new(n))
        } else {
            DataPaneId::Plugin(zellij_types::internal_ids::PluginId::new(n))
        }
    }

    struct QueueModel {
        real: CoalescingSendQueue,
        control: VecDeque<ServerToClientMsg>,
        pane_renders: BTreeMap<DataPaneId, ServerToClientMsg>,
        full_render: Option<ServerToClientMsg>,
        closed: bool,
        seq: u32,
    }

    impl QueueModel {
        fn new() -> Self {
            QueueModel {
                real: CoalescingSendQueue::new(),
                control: VecDeque::new(),
                pane_renders: BTreeMap::new(),
                full_render: None,
                closed: false,
                seq: 0,
            }
        }
        fn next_seq(&mut self) -> String {
            self.seq += 1;
            self.seq.to_string()
        }
    }

    #[hegel::state_machine]
    impl QueueModel {
        /// What the model says the control FIFO does with `msg`, and the check against the
        /// real queue. Shared by every message that takes the FIFO path.
        fn push_fifo(&mut self, msg: ServerToClientMsg) {
            let outcome = self.real.push(msg.clone());
            if self.closed {
                assert!(matches!(outcome, SendQueuePush::Queued));
            } else if self.control.len() >= SEND_QUEUE_CONTROL_CAPACITY {
                assert!(matches!(outcome, SendQueuePush::ControlQueueFull));
            } else {
                assert!(matches!(outcome, SendQueuePush::Queued));
                self.control.push_back(msg);
            }
        }

        #[rule]
        fn push_control(&mut self, _tc: hegel::TestCase) {
            let msg = control(&self.next_seq());
            self.push_fifo(msg);
        }

        #[rule]
        fn push_full_render(&mut self, _tc: hegel::TestCase) {
            let msg = render(&self.next_seq());
            let outcome = self.real.push(msg.clone());
            if self.closed {
                assert!(matches!(outcome, SendQueuePush::Queued));
            } else {
                assert_eq!(
                    matches!(outcome, SendQueuePush::CoalescedRender),
                    self.full_render.is_some(),
                );
                self.full_render = Some(msg);
            }
        }

        #[rule]
        fn push_pane_render(&mut self, tc: hegel::TestCase) {
            let pane_id = tc.draw(any_data_pane_id());
            let msg = pane_render(pane_id, &self.next_seq());
            let outcome = self.real.push(msg.clone());
            if self.closed {
                assert!(matches!(outcome, SendQueuePush::Queued));
            } else {
                assert_eq!(
                    matches!(outcome, SendQueuePush::CoalescedRender),
                    self.pane_renders.contains_key(&pane_id),
                );
                self.pane_renders.insert(pane_id, msg);
            }
        }

        #[rule]
        fn push_initial_pane_render(&mut self, tc: hegel::TestCase) {
            // initial snapshots take the FIFO path like any control message
            let pane_id = tc.draw(any_data_pane_id());
            let msg = ServerToClientMsg::PaneRenderUpdate {
                pane_id,
                viewport: vec![self.next_seq()],
                scrollback: None,
                is_initial: true,
            };
            self.push_fifo(msg);
        }

        #[rule]
        fn push_scrollback_pane_render(&mut self, tc: hegel::TestCase) {
            // the other of `slot_for`'s two negative conditions: an update carrying
            // scrollback is never coalesced away, so it takes the FIFO path too
            let pane_id = tc.draw(any_data_pane_id());
            let seq = self.next_seq();
            let msg = ServerToClientMsg::PaneRenderUpdate {
                pane_id,
                viewport: vec![seq.clone()],
                scrollback: Some(vec![seq]),
                is_initial: false,
            };
            self.push_fifo(msg);
        }

        #[rule]
        fn pop(&mut self, tc: hegel::TestCase) {
            let model_has_message = !self.control.is_empty()
                || !self.pane_renders.is_empty()
                || self.full_render.is_some();
            // pop blocks on an open empty queue; only pop when it can't block
            tc.assume(model_has_message || self.closed);
            let expected = if let Some(msg) = self.control.pop_front() {
                Some(msg)
            } else if let Some((_, msg)) = self.pane_renders.pop_first() {
                Some(msg)
            } else {
                self.full_render.take()
            };
            assert_eq!(self.real.pop(), expected);
        }

        #[rule]
        fn close(&mut self, _tc: hegel::TestCase) {
            self.real.close();
            self.closed = true;
        }

        #[invariant]
        fn depth_agrees_with_the_model(&self, _: hegel::TestCase) {
            assert_eq!(
                self.real.len(),
                self.control.len()
                    + self.pane_renders.len()
                    + usize::from(self.full_render.is_some()),
            );
        }
    }

    #[hegel::test]
    fn queue_agrees_with_a_sequential_model(tc: hegel::TestCase) {
        hegel::stateful::run(QueueModel::new(), tc);
    }
}

/// The wait that keeps a queued `Exit` from dying with the process.
///
/// `send_to_client` only queues; a detached writer thread does the write. Every
/// server shutdown path sends clients an `Exit` and then returns from `main`, so
/// without a wait in between the client can see a bare hangup instead of being
/// told why the session ended — which is exactly what "Received empty unknown
/// from server" was.
mod client_writer_flush {
    use super::super::{ClientSender, LiveWriters};
    use interprocess::local_socket::{prelude::*, ListenerOptions};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use {
        zellij_ipc::ipc::ClientToServerMsg, zellij_ipc::ipc::ExitReason,
        zellij_ipc::ipc::IpcReceiverWithContext, zellij_ipc::ipc::IpcRecv,
        zellij_ipc::ipc::IpcSenderWithContext, zellij_ipc::ipc::ServerToClientMsg,
    };

    fn socket_pair() -> (
        tempfile::TempDir,
        interprocess::local_socket::Stream,
        interprocess::local_socket::Stream,
    ) {
        use interprocess::local_socket::{GenericFilePath, Stream as LocalSocketStream};
        let dir = tempfile::TempDir::new().expect("failed to create temp dir");
        let path = dir.path().join("flush-test.sock");
        let listener = ListenerOptions::new()
            .name(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
            .create_sync()
            .expect("bind failed");
        let client =
            LocalSocketStream::connect(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
                .expect("connect failed");
        let server = listener.incoming().next().unwrap().expect("accept failed");
        (dir, server, client)
    }

    #[test]
    fn a_queued_exit_reaches_the_socket_before_the_wait_returns() {
        let (_guard, server_end, client_end) = socket_pair();
        let live_writers = Arc::new(LiveWriters::default());
        let server_fd = {
            use std::os::fd::AsFd;
            match &server_end {
                interprocess::local_socket::Stream::UdSocket(uds) => {
                    uds.as_fd().try_clone_to_owned().expect("dup failed")
                },
            }
        };
        let sender = ClientSender::new(
            crate::ClientId::new(1),
            IpcSenderWithContext::<ServerToClientMsg>::new(server_end),
            live_writers.clone(),
            server_fd,
        );

        // What every break in the server main loop does on its way out.
        sender
            .send_or_buffer(ServerToClientMsg::Exit {
                exit_reason: ExitReason::Normal,
            })
            .expect("queueing the exit failed");
        sender.close();

        assert!(
            live_writers.wait_until_drained(Duration::from_secs(5)),
            "the writer should finish draining well inside the deadline"
        );

        // Only now would the process exit. The bytes must already be there: the read is
        // given almost no time, so a writer that was still flushing would show up as Idle
        // rather than being quietly waited for.
        let mut receiver: IpcReceiverWithContext<ServerToClientMsg> =
            IpcReceiverWithContext::new(client_end);
        receiver
            .set_recv_timeout(Some(Duration::from_millis(1)))
            .expect("setting a receive timeout failed");
        match receiver.recv_server_msg() {
            IpcRecv::Msg(
                ServerToClientMsg::Exit {
                    exit_reason: ExitReason::Normal,
                },
                _,
            ) => {},
            other => panic!(
                "the client should have been told why the session ended, got {:?}",
                match other {
                    IpcRecv::Msg(msg, _) => format!("{msg:?}"),
                    IpcRecv::Eof => "a hangup".to_string(),
                    IpcRecv::Malformed => "a malformed message".to_string(),
                    IpcRecv::Idle => "nothing".to_string(),
                }
            ),
        }
    }

    /// The other half of the ordering: the wait does not report drained while a writer still
    /// has bytes it could not put on the socket. The client here has stopped reading with its
    /// receive buffer full, so the writer blocks on the `Exit`; only once the client drains
    /// its side does the writer finish, and only then does the wait return.
    #[test]
    fn the_wait_does_not_return_while_a_writer_is_still_blocked_on_the_socket() {
        use std::io::{Read, Write};
        use std::os::fd::AsFd;
        use std::os::unix::net::UnixStream;

        let (_guard, server_end, client_end) = socket_pair();
        let dup = |stream: &interprocess::local_socket::Stream| match stream {
            interprocess::local_socket::Stream::UdSocket(uds) => {
                uds.as_fd().try_clone_to_owned().expect("dup failed")
            },
        };
        let server_fd = dup(&server_end);
        let client_fd = dup(&client_end);

        // Fill the socket from the server side until it will take no more. Non-blocking
        // mode is a property of the open file, so it is put back before the writer thread
        // shares it.
        let mut filler = UnixStream::from(server_fd.try_clone().expect("dup failed"));
        filler
            .set_nonblocking(true)
            .expect("set_nonblocking failed");
        let junk = [0u8; 4096];
        let mut junk_len = 0usize;
        loop {
            match filler.write(&junk) {
                Ok(n) => junk_len += n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => panic!("filling the socket failed: {e}"),
            }
        }
        filler
            .set_nonblocking(false)
            .expect("set_nonblocking failed");
        assert!(junk_len > 0, "precondition: the socket took some bytes");

        let live_writers = Arc::new(LiveWriters::default());
        let sender = ClientSender::new(
            crate::ClientId::new(1),
            IpcSenderWithContext::<ServerToClientMsg>::new(server_end),
            live_writers.clone(),
            server_fd,
        );
        sender
            .send_or_buffer(ServerToClientMsg::Exit {
                exit_reason: ExitReason::Normal,
            })
            .expect("queueing the exit failed");
        sender.close();

        assert!(
            !live_writers.wait_until_drained(Duration::from_millis(200)),
            "the wait returned while the writer was still blocked behind a full socket"
        );

        // The client catches up, which unblocks the writer.
        let mut drainer = UnixStream::from(client_fd);
        let mut sink = vec![0u8; junk_len];
        drainer
            .read_exact(&mut sink)
            .expect("draining the junk failed");
        assert!(
            live_writers.wait_until_drained(Duration::from_secs(5)),
            "the writer should finish once the client reads"
        );

        let mut receiver: IpcReceiverWithContext<ServerToClientMsg> =
            IpcReceiverWithContext::new(client_end);
        receiver
            .set_recv_timeout(Some(Duration::from_millis(1)))
            .expect("setting a receive timeout failed");
        assert!(
            matches!(
                receiver.recv_server_msg(),
                IpcRecv::Msg(
                    ServerToClientMsg::Exit {
                        exit_reason: ExitReason::Normal,
                    },
                    _,
                )
            ),
            "the exit should be on the socket once the wait returns"
        );
    }

    #[test]
    fn waiting_gives_up_rather_than_holding_the_process_open() {
        let live_writers = LiveWriters::default();
        // A writer that never finishes - a client whose socket buffer is full
        // and which has stopped reading looks exactly like this.
        live_writers.register();
        let started = std::time::Instant::now();
        assert!(
            !live_writers.wait_until_drained(Duration::from_millis(50)),
            "an undrained writer should be reported as such"
        );
        let waited = started.elapsed();
        assert!(
            waited >= Duration::from_millis(50),
            "the wait returned after {waited:?}, before its 50ms deadline"
        );
        // Generous: the assertion is "bounded at all".
        assert!(
            waited < Duration::from_secs(2),
            "the wait must be bounded, it took {waited:?}"
        );
    }

    #[test]
    fn draining_is_reported_once_every_writer_is_done() {
        let live_writers = LiveWriters::default();
        assert!(
            live_writers.wait_until_drained(Duration::from_millis(50)),
            "no writers means nothing to wait for"
        );
        live_writers.register();
        live_writers.register();
        live_writers.deregister();
        assert!(
            !live_writers.wait_until_drained(Duration::from_millis(50)),
            "one writer is still going"
        );
        live_writers.deregister();
        assert!(
            live_writers.wait_until_drained(Duration::from_millis(50)),
            "both writers are done"
        );
    }

    /// The router thread must not outlive the client the server has removed.
    ///
    /// `remove_client` drops the `ClientSender`, which retires the write half.
    /// The read half is a `dup` of the same socket held by the router thread,
    /// blocked in `recv_client_msg` with no deadline and leaving only on EOF —
    /// and nothing produced that EOF for a peer that connected and then kept
    /// its descriptor open. Each one stranded a router thread and an open
    /// socket for the life of the process; one server here reached 65,538 of
    /// them, at which point its listen backlog was permanently full and a
    /// `connect` to it never returned, hanging every zellij command on the
    /// machine.
    ///
    /// `shutdown(2)` acts on the socket rather than the descriptor, so this
    /// reaches the router's `dup` — which is the whole point, since the server
    /// has no handle on that thread to interrupt any other way.
    #[test]
    fn shutting_a_client_down_unblocks_the_router_reading_it() {
        let (_guard, server_end, client_end) = socket_pair();
        let live_writers = Arc::new(LiveWriters::default());
        let server_fd = {
            use std::os::fd::AsFd;
            match &server_end {
                interprocess::local_socket::Stream::UdSocket(uds) => {
                    uds.as_fd().try_clone_to_owned().expect("dup failed")
                },
            }
        };
        // Exactly how `new_client` wires a connection up: the receiver owns the
        // stream and the sender is a clone of it.
        let mut receiver: IpcReceiverWithContext<ClientToServerMsg> =
            IpcReceiverWithContext::new(server_end);
        let sender = ClientSender::new(
            crate::ClientId::new(1),
            receiver.get_sender(),
            live_writers.clone(),
            server_fd,
        );

        // The client is deliberately kept alive for the whole test: a peer that
        // hangs up would produce the EOF on its own, which is the case that
        // already worked.
        let (blocked, is_blocked) = std::sync::mpsc::channel();
        let (read_ended, read_outcome) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            blocked.send(()).expect("nobody waiting for the router");
            let _ = read_ended.send(receiver.recv_client_msg());
        });
        is_blocked.recv().expect("the router thread never started");

        sender.shutdown_reads();

        // Bounded here rather than by nextest's kill: a router that never woke is the
        // regression this test exists for, and it must fail by name rather than read as
        // contention.
        let outcome = read_outcome
            .recv_timeout(Duration::from_secs(5))
            .expect("the router's read never ended after the shutdown");
        let described = match outcome {
            IpcRecv::Eof => "Eof",
            IpcRecv::Idle => "Idle",
            IpcRecv::Malformed => "Malformed",
            IpcRecv::Msg(..) => "Msg",
        };
        assert_eq!(
            described, "Eof",
            "shutting the read half down should end the router's read as a hangup"
        );
        drop(client_end);
    }

    /// The other way a router can be freed of a peer that will never hang up:
    /// a read deadline, applied by the router until the first message arrives,
    /// so that a connection which never speaks does not hold its slot forever.
    #[test]
    fn a_silent_peer_reads_as_idle_once_the_deadline_passes() {
        let (_guard, server_end, client_end) = socket_pair();
        let mut receiver: IpcReceiverWithContext<ClientToServerMsg> =
            IpcReceiverWithContext::new(server_end);
        let deadline = Duration::from_millis(50);
        receiver
            .set_recv_timeout(Some(deadline))
            .expect("setting a receive timeout failed");

        // The client is kept alive and silent for the whole read: a hangup
        // would be EOF, which is the case that already worked.
        let started_at = Instant::now();
        let outcome = receiver.recv_client_msg();
        let waited = started_at.elapsed();

        assert!(
            matches!(outcome, IpcRecv::Idle),
            "a read that ran out of time should be Idle, not a hangup or a bad frame"
        );
        assert!(
            waited >= deadline,
            "the read returned after {waited:?}, before its {deadline:?} deadline"
        );
        // Generous: the assertion is "bounded at all".
        assert!(
            waited < deadline * 40,
            "the read should have given up near its {deadline:?} deadline, but took {waited:?}"
        );
        drop(client_end);
    }

    /// Lifting the deadline is what keeps it from ever cutting a real client's
    /// frame short: once it is cleared, a message sent later than the old
    /// deadline still arrives whole.
    #[test]
    fn a_lifted_deadline_no_longer_bounds_the_read() {
        let (_guard, server_end, client_end) = socket_pair();
        let mut receiver: IpcReceiverWithContext<ClientToServerMsg> =
            IpcReceiverWithContext::new(server_end);
        let deadline = Duration::from_millis(20);
        receiver
            .set_recv_timeout(Some(deadline))
            .expect("setting a receive timeout failed");
        receiver
            .set_recv_timeout(None)
            .expect("clearing the receive timeout failed");

        let writer = std::thread::spawn(move || {
            std::thread::sleep(deadline * 5);
            let mut sender: IpcSenderWithContext<ClientToServerMsg> =
                IpcSenderWithContext::new(client_end);
            sender
                .send_client_msg(ClientToServerMsg::ConnStatus)
                .expect("send failed");
            sender
        });

        let outcome = receiver.recv_client_msg();
        assert!(
            matches!(outcome, IpcRecv::Msg(ClientToServerMsg::ConnStatus, _)),
            "a message sent after the lifted deadline should still arrive"
        );
        drop(writer.join().expect("the writer thread panicked"));
    }
}

#[test]
fn server_termios_argument_distinguishes_defaults_and_invalid_json() {
    use nix::sys::termios::{self, InputFlags, LocalFlags, SetArg};
    use zellij_runtime::termios::WireTermios;

    assert!(get_server_os_input_from_termios_arg(None).is_ok());
    assert!(get_server_os_input_from_termios_arg(Some("null"))
        .unwrap()
        .pty_backend
        .configured_termios()
        .is_none());

    // A real snapshot, with flags no default pty has, and the backend must carry it: this
    // is the client's terminal settings reaching every local pane.
    let reference = nix::pty::openpty(None, None).unwrap();
    let mut distinctive = termios::tcgetattr(&reference.slave).unwrap();
    distinctive.input_flags.insert(InputFlags::ICRNL);
    distinctive.local_flags.remove(LocalFlags::ECHO);
    distinctive.local_flags.insert(LocalFlags::ICANON);
    termios::tcsetattr(&reference.slave, SetArg::TCSANOW, &distinctive).unwrap();
    let snapshot = WireTermios::from_termios(&termios::tcgetattr(&reference.slave).unwrap());
    let json = serde_json::to_string(&Some(snapshot.clone())).unwrap();
    assert_eq!(
        get_server_os_input_from_termios_arg(Some(&json))
            .unwrap()
            .pty_backend
            .configured_termios(),
        Some(snapshot)
    );

    for json in ["broken", "{}", "42"] {
        let error = get_server_os_input_from_termios_arg(Some(json))
            .err()
            .unwrap();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("invalid --server-termios"));
    }
}
