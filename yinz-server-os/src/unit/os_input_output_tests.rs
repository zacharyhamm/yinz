const AMPLE_TIMEOUT: Duration = Duration::from_secs(30);

use super::*;
use yinz_types::command::PaneTarget;
use yinz_types::input::command::RunCommand;
use yinz_types::internal_ids::ProcessId;

fn make_server() -> ServerOsInputOutput {
    get_server_os_input().expect("failed to create server os input")
}

fn codex_command(with_env: bool) -> RunCommand {
    let (command, _) = yinz_runtime::agent::launch::plan(&yinz_runtime::agent::launch::Spec {
        kind: Some("codex".into()),
        env: if with_env {
            BTreeMap::from([("SETTING".into(), "value".into())])
        } else {
            BTreeMap::new()
        },
        ..Default::default()
    })
    .unwrap();
    RunCommand {
        command: command.path,
        args: command.args,
        ..Default::default()
    }
}

#[test]
fn local_codex_wrappers_use_current_binary_and_keep_portable_reruns() {
    const CHILD_ENV: &str = "YINZ_TEST_CODEX_PATH";
    if env::var_os(CHILD_ENV).is_none() {
        let bin = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/usr/bin/env", bin.path().join("env")).unwrap();
        for (incompatible, replace) in [(false, false), (true, false), (true, true)] {
            if incompatible {
                yinz_test_support::write_warm_script(&bin.path().join("yinz"), "exit 42\n");
            }
            let mut binary = env::current_exe().unwrap();
            if replace {
                let copy = bin.path().join("server");
                std::fs::copy(&binary, &copy).unwrap();
                yinz_test_support::write_warm_script(
                    &copy.with_extension("new"),
                    "[ \"$*\" = 'agent codex --command codex --' ] || exit 1\nprintf '%s\\n' \"${SETTING-}\" >> \"$0.runs\"\n",
                );
                binary = copy;
            }
            let output = Command::new(binary)
                .args(["--exact", "os_input_output::os_input_output_tests::local_codex_wrappers_use_current_binary_and_keep_portable_reruns"])
                .env(CHILD_ENV, if replace { "replace" } else { "1" })
                .env("PATH", bin.path())
                .env_remove("SETTING")
                .env("YINZ_DISABLE_PTY_HOST", "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "incompatible={incompatible}, replace={replace}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        }
        return;
    }
    let drain = |mut reader: Box<dyn AsyncReader>| {
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move {
                    let mut buffer = [0u8; 4096];
                    while matches!(
                        tokio::time::timeout(AMPLE_TIMEOUT, reader.read(&mut buffer)).await,
                        Ok(Ok(count)) if count > 0
                    ) {}
                });
        })
    };
    let server = make_server();
    server.set_pty_host_enabled(false);
    let current_exe = env::current_exe().unwrap();
    let replaced = env::var(CHILD_ENV).unwrap() == "replace";
    if replaced {
        // Match installation: replace the inode after server initialization cached its path.
        std::fs::rename(current_exe.with_extension("new"), &current_exe).unwrap();
    }
    for with_env in [false, true] {
        let requested = codex_command(with_env);
        let (resolved, rerun, endpoint) = server.resolve_target(requested.clone(), None).unwrap();
        let mut expected = requested.clone();
        if with_env {
            expected.args[1] = current_exe.to_str().unwrap().into();
        } else {
            expected.command = current_exe.clone();
        }
        assert_eq!(resolved, expected);
        assert!(command_exists(&resolved));
        assert_eq!(rerun, requested);
        assert!(endpoint.is_none());
        assert_eq!(server.resolve_target(rerun, None).unwrap().0, resolved);

        // Before replacement the test executable rejects the wrapper's arguments;
        // after replacement the stub must succeed, including on a held pane's rerun.
        let (tx, rx) = std::sync::mpsc::channel();
        let (terminal_id, reader, _) = server
            .spawn_terminal(
                TerminalAction::RunCommand(requested.clone()),
                Box::new(move |_, status, command| tx.send((status, command)).unwrap()),
                None,
            )
            .unwrap();
        let draining = drain(reader);
        let (status, held) = rx.recv_timeout(AMPLE_TIMEOUT).unwrap();
        draining.join().unwrap();
        if replaced {
            assert_eq!(status, Some(0));
        }
        assert_eq!(held, requested);
        let (tx, rx) = std::sync::mpsc::channel();
        let (reader, _) = server
            .re_run_command_in_terminal(
                terminal_id,
                held,
                Box::new(move |_, status, command| tx.send((status, command)).unwrap()),
            )
            .unwrap();
        let draining = drain(reader);
        let (status, held) = rx.recv_timeout(AMPLE_TIMEOUT).unwrap();
        draining.join().unwrap();
        if replaced {
            assert_eq!(status, Some(0));
        }
        assert_eq!(held, requested);
        server.clear_terminal_id(terminal_id).unwrap();
    }
    if replaced {
        assert_eq!(
            std::fs::read_to_string(current_exe.with_extension("runs")).unwrap(),
            "\n\nvalue\nvalue\n"
        );
    }
    for (program, args) in [
        ("codex", vec![]),
        ("yinz", vec!["action", "new-pane"]),
        ("env", vec!["SETTING=value", "codex"]),
        ("env", vec!["SETTING=value", "yinz", "action", "new-pane"]),
        ("/other/yinz", vec!["agent", "codex", "--"]),
    ] {
        let requested = RunCommand {
            command: program.into(),
            args: args.into_iter().map(String::from).collect(),
            ..Default::default()
        };
        let (resolved, rerun, endpoint) = server.resolve_target(requested.clone(), None).unwrap();
        assert_eq!(resolved, requested);
        assert_eq!(rerun, requested);
        assert!(endpoint.is_none());
    }
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
    use crate::PaneId;
    use yinz_types::input::command::TerminalAction;

    let server = make_server();
    let test_message = "hello_yinz_test";
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
            format!("echo ref=[$YINZ_PANE_REF] token=[${token}] {test_message}"),
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

#[test]
fn fallback_panes_clear_inherited_inline_hooks_but_keep_launch_assignments() {
    let marker = "YINZ_CLAUDE_INLINE_HOOKS";
    std::env::set_var(marker, "1");
    let server = make_server();
    for inline in [false, true] {
        let expected = if inline { "1" } else { "unset" };
        let mut cmd = RunCommand {
            command: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                format!("[ \"${{{marker}-unset}}\" = {expected} ]"),
            ],
            ..Default::default()
        };
        if inline {
            cmd.args
                .splice(0..0, [format!("{marker}=1"), "/bin/sh".into()]);
            cmd.command = "/usr/bin/env".into();
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let (terminal_id, _reader, _) = server
            .spawn_terminal(
                TerminalAction::RunCommand(cmd),
                Box::new(move |_, status, _| tx.send(status).unwrap()),
                None,
            )
            .unwrap();
        assert_eq!(rx.recv_timeout(AMPLE_TIMEOUT).unwrap(), Some(0));
        server.clear_terminal_id(terminal_id).unwrap();
    }
    std::env::remove_var(marker);
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

#[cfg(unix)]
#[test]
fn tcgetpgrp_returns_foreground_group() {
    use crate::PaneId;
    use yinz_types::input::command::TerminalAction;

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
    use yinz_types::data::PaneId as DataPaneId;

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

    fn overflow(queue: &CoalescingSendQueue, msg: ServerToClientMsg) -> QueueOverflow {
        match queue.push(msg) {
            SendQueuePush::ControlQueueFull(details) => details,
            _ => panic!("expected queue overflow"),
        }
    }

    #[test]
    fn lifecycle_records_use_bounded_fifo_without_coalescing() {
        let queue = CoalescingSendQueue::new();
        let event = |sequence| ServerToClientMsg::EventUpdate {
            event_type: "agent_lifecycle".into(),
            event_json: format!("{{\"sequence\":{sequence}}}"),
        };
        for sequence in 0..SEND_QUEUE_CONTROL_CAPACITY {
            queue.push(event(sequence));
        }
        assert_eq!(
            overflow(&queue, event(SEND_QUEUE_CONTROL_CAPACITY)).reason,
            "message_count"
        );
        queue.close();
        for sequence in 0..SEND_QUEUE_CONTROL_CAPACITY {
            assert_eq!(queue.pop(), Some(event(sequence)));
        }
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn simultaneous_registration_and_drain_leave_no_unnotified_backlog() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Barrier,
        };
        for _ in 0..32 {
            let queue = CoalescingSendQueue::new();
            queue.push(render("frame"));
            let ready = Arc::new(AtomicBool::new(false));
            let barrier = Barrier::new(2);
            let backlog = std::thread::scope(|scope| {
                scope.spawn(|| {
                    barrier.wait();
                    queue.pop();
                });
                barrier.wait();
                let ready = ready.clone();
                let notify: RenderReady = Arc::new(move || {
                    ready.store(true, Ordering::SeqCst);
                });
                queue.render_backlog_and_notify(1, Some(&notify))
            });
            assert!(backlog.queued_renders == 0 || ready.load(Ordering::SeqCst));
        }
    }

    #[test]
    fn render_readiness_registration_cannot_lose_a_drain_or_close() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        for close in [false, true] {
            for register_first in [false, true] {
                let queue = Arc::new(CoalescingSendQueue::new());
                queue.push(render("frame"));
                let calls = Arc::new(AtomicUsize::new(0));
                let callback: RenderReady = {
                    let queue = queue.clone();
                    let calls = calls.clone();
                    Arc::new(move || {
                        // Reentering proves the callback does not own the queue lock.
                        assert_eq!(queue.render_backlog().queued_renders, 0);
                        calls.fetch_add(1, Ordering::SeqCst);
                    })
                };
                let drain = || {
                    if close {
                        queue.close();
                    } else {
                        queue.pop();
                    }
                };
                if !register_first {
                    drain();
                }
                let backlog = queue.render_backlog_and_notify(1, Some(&callback));
                assert_eq!(backlog.queued_renders, usize::from(register_first));
                if register_first {
                    drain();
                }
                queue.close();
                queue.finish_send();
                assert_eq!(calls.load(Ordering::SeqCst), usize::from(register_first));
            }
        }
    }

    #[test]
    fn enqueue_holds_the_registry_until_the_message_is_stored() {
        use std::os::unix::net::UnixStream;
        use std::sync::TryLockError;

        let server = make_server();
        let client = ClientId::new(1);
        let queue = Arc::new(CoalescingSendQueue::new());
        let (socket, _peer) = UnixStream::pair().unwrap();
        server.client_senders.lock().unwrap().insert(
            client,
            ClientSender {
                queue: queue.clone(),
                stream_fd: socket.into(),
            },
        );
        // Stop the send inside enqueue. Replacement must not acquire the registry
        // until this queue lock is released and the message has been stored.
        let state = queue.state.lock().unwrap();
        let sending = {
            let server = server.clone();
            std::thread::spawn(move || server.send_to_client(client, control("before replacement")))
        };
        let started = Instant::now();
        loop {
            match server.client_senders.try_lock() {
                Err(TryLockError::WouldBlock) => break,
                Err(TryLockError::Poisoned(_)) => panic!("registry poisoned"),
                Ok(_) => {},
            }
            assert!(
                started.elapsed() < AMPLE_TIMEOUT,
                "enqueue released the registry before storing the message"
            );
            std::thread::yield_now();
        }
        let replacement = Arc::new(CoalescingSendQueue::new());
        let (checked, was_checked) = std::sync::mpsc::channel();
        let replacing = {
            let server = server.clone();
            let replacement = replacement.clone();
            let (socket, _peer) = UnixStream::pair().unwrap();
            std::thread::spawn(move || {
                checked
                    .send(matches!(
                        server.client_senders.try_lock(),
                        Err(TryLockError::WouldBlock)
                    ))
                    .unwrap();
                let old = server.lock_client_senders().insert(
                    client,
                    ClientSender {
                        queue: replacement,
                        stream_fd: socket.into(),
                    },
                );
                drop(old);
            })
        };
        assert!(
            was_checked.recv_timeout(AMPLE_TIMEOUT).unwrap(),
            "replacement acquired the registry before enqueue completed"
        );
        drop(state);
        sending.join().unwrap().unwrap();
        replacing.join().unwrap();
        assert_eq!(queue.pop(), Some(control("before replacement")));
        assert_eq!(queue.pop(), None);
        assert_eq!(replacement.len(), 0);
        server
            .send_to_client(client, control("after replacement"))
            .unwrap();
        assert_eq!(replacement.pop(), Some(control("after replacement")));
    }

    #[test]
    fn enqueue_runs_poison_recovery_callback_outside_the_registry() {
        use std::os::unix::net::UnixStream;
        use std::sync::mpsc;

        let server = make_server();
        let client = ClientId::new(1);
        let queue = Arc::new(CoalescingSendQueue::new());
        let (socket, _peer) = UnixStream::pair().unwrap();
        server.client_senders.lock().unwrap().insert(
            client,
            ClientSender {
                queue: queue.clone(),
                stream_fd: socket.into(),
            },
        );
        queue.push(render("frame"));
        let (called, was_called) = mpsc::channel();
        let ready: RenderReady = {
            let server = server.clone();
            Arc::new(move || {
                let _ = server.client_queue(client);
                called.send(()).unwrap();
            })
        };
        server.client_render_backlog_and_notify(client, 1, &ready);
        let poisoned = queue.clone();
        assert!(std::thread::spawn(move || {
            let _state = poisoned.state.lock().unwrap();
            panic!("poison writer queue with an armed callback");
        })
        .join()
        .is_err());
        let sending = std::thread::spawn(move || server.send_to_client(client, render("late")));
        was_called
            .recv_timeout(AMPLE_TIMEOUT)
            .expect("enqueue callback deadlocked on the registry");
        sending.join().unwrap().unwrap();
        assert!(queue.state.lock().unwrap().closed);
        assert_eq!(queue.len(), 0);
    }

    /// A render-ready callback consults the sender registry again (the screen's next pass
    /// does), so the paths that close a queue must not run it while holding the registry.
    #[test]
    fn render_ready_callbacks_run_outside_the_sender_registry() {
        use std::os::unix::net::UnixStream;
        use std::sync::mpsc;

        for flush in [false, true] {
            let mut server = make_server();
            let client = ClientId::new(1);
            let queue = Arc::new(CoalescingSendQueue::new());
            queue.push(render("frame"));
            let (socket, _peer) = UnixStream::pair().unwrap();
            server.client_senders.lock().unwrap().insert(
                client,
                ClientSender {
                    queue: queue.clone(),
                    stream_fd: socket.into(),
                },
            );
            let (called, was_called) = mpsc::channel();
            let ready: RenderReady = {
                let server = server.clone();
                let called = std::sync::Mutex::new(called);
                Arc::new(move || {
                    let _ = server.client_queue(client);
                    let _ = called.lock().unwrap().send(());
                })
            };
            assert_eq!(
                server
                    .client_render_backlog_and_notify(client, 1, &ready)
                    .map(|backlog| backlog.queued_renders),
                Some(1)
            );
            // Run where a deadlock cannot hang the test: the callback reports from that thread.
            std::thread::spawn(move || {
                if flush {
                    server.flush_client_writers(Duration::ZERO);
                } else {
                    server.remove_client(client).unwrap();
                }
            });
            was_called
                .recv_timeout(Duration::from_secs(10))
                .unwrap_or_else(|_| panic!("callback deadlocked on the registry, flush={flush}"));
        }
    }

    #[test]
    fn poisoned_sender_locks_recover_once_and_allow_teardown() {
        use std::os::unix::net::UnixStream;

        let server = make_server();
        let client = ClientId::new(1);
        let queue = Arc::new(CoalescingSendQueue::new());
        let (socket, _peer) = UnixStream::pair().unwrap();
        server.client_senders.lock().unwrap().insert(
            client,
            ClientSender {
                queue: queue.clone(),
                stream_fd: socket.into(),
            },
        );
        let poisoned_queue = queue.clone();
        assert!(std::thread::spawn(move || {
            let _guard = poisoned_queue.state.lock().unwrap();
            panic!("poison writer queue");
        })
        .join()
        .is_err());

        assert_eq!(
            server
                .client_queue(client)
                .map(|queue| queue.render_backlog()),
            Some(RenderBacklog::default())
        );
        assert!(!queue.state.is_poisoned());
        assert!(queue.state.lock().unwrap().closed);

        let senders = server.client_senders.clone();
        assert!(std::thread::spawn(move || {
            let _guard = senders.lock().unwrap();
            panic!("poison client sender registry");
        })
        .join()
        .is_err());
        assert_eq!(
            server
                .client_queue(client)
                .map(|queue| queue.render_backlog()),
            Some(RenderBacklog::default())
        );
        assert!(!server.client_senders.is_poisoned());
        server.send_to_client(client, render("late")).unwrap();
        server.flush_client_writers(Duration::ZERO);
        let mut server = server;
        server.remove_client(client).unwrap();
        assert!(server.client_queue(client).is_none());
    }

    #[test]
    fn every_queue_operation_closes_poisoned_state() {
        let operations: [fn(&CoalescingSendQueue); 5] = [
            |queue| {
                queue.push(render("late"));
            },
            |queue| {
                assert!(queue.pop().is_none());
            },
            CoalescingSendQueue::finish_send,
            CoalescingSendQueue::close,
            |queue| {
                assert_eq!(queue.render_backlog(), RenderBacklog::default());
            },
        ];
        for operation in operations {
            let queue = CoalescingSendQueue::new();
            queue.push(render("queued"));
            assert!(std::panic::catch_unwind(|| {
                let mut state = queue.state.lock().unwrap();
                state.render_bytes = usize::MAX;
                panic!("interrupt queue accounting");
            })
            .is_err());

            operation(&queue);

            assert!(!queue.state.is_poisoned());
            assert!(queue.state.lock().unwrap().closed);
            assert_eq!(queue.len(), 0);
            assert!(matches!(queue.push(render("later")), SendQueuePush::Queued));
            assert!(queue.pop().is_none());
            queue.finish_send();
            assert_eq!(queue.render_backlog(), RenderBacklog::default());
            queue.close();
        }
    }

    #[test]
    fn incremental_renders_survive_backlog_and_precede_exit() {
        let queue = CoalescingSendQueue::new();
        // Independent cell changes and an OSC side effect all have to arrive.
        let messages = [
            render("\x1b[1;1Hfirst"),
            render("\x1b[2;1Hsecond"),
            control("notice"),
            render("\x1b]0;title\x07"),
            ServerToClientMsg::Exit {
                exit_reason: ExitReason::Normal,
            },
        ];
        for message in &messages {
            assert!(matches!(queue.push(message.clone()), SendQueuePush::Queued));
        }
        queue.close();
        for message in messages {
            assert_eq!(queue.pop(), Some(message));
        }
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn a_full_ordered_queue_refuses_render_deltas_without_losing_accepted_frames() {
        let queue = CoalescingSendQueue::new();
        for i in 0..SEND_QUEUE_CONTROL_CAPACITY {
            assert!(matches!(
                queue.push(render(&i.to_string())),
                SendQueuePush::Queued
            ));
        }
        let details = overflow(&queue, render("overflow"));
        assert_eq!(details.reason, "message_count");
        assert_eq!(details.ordered, SEND_QUEUE_CONTROL_CAPACITY);
        assert_eq!(details.render_bytes, SEND_QUEUE_CONTROL_CAPACITY);
        assert_eq!(details.rejected_render_len, 8);
        assert_eq!(details.rejected_render_capacity, 8);
        assert_eq!(
            overflow(&queue, control("overflow")).reason,
            "message_count"
        );
        queue.close();
        for i in 0..SEND_QUEUE_CONTROL_CAPACITY {
            assert_eq!(queue.pop(), Some(render(&i.to_string())));
        }
        assert_eq!(queue.pop(), None);
        assert_eq!(queue.state.lock().unwrap().render_bytes, 0);
    }

    #[test]
    fn render_byte_budget_preserves_fifo_and_recovers_after_pop() {
        let queue = CoalescingSendQueue::new();
        let first = "a".repeat(SEND_QUEUE_RENDER_BYTE_CAPACITY / 2);
        let second = "b".repeat(SEND_QUEUE_RENDER_BYTE_CAPACITY / 2);
        let third = "c".repeat(SEND_QUEUE_RENDER_BYTE_CAPACITY / 2);
        for message in [render(&first), control("notice"), render(&second)] {
            assert!(matches!(queue.push(message), SendQueuePush::Queued));
        }
        assert!(queue.len() < SEND_QUEUE_CONTROL_CAPACITY);
        let details = overflow(&queue, render("overflow"));
        assert_eq!(details.reason, "render_allocation_bytes");
        assert_eq!(details.ordered, 3);
        assert_eq!(details.render_bytes, SEND_QUEUE_RENDER_BYTE_CAPACITY);
        assert_eq!(queue.pop(), Some(render(&first)));
        assert_eq!(
            queue.state.lock().unwrap().render_bytes,
            SEND_QUEUE_RENDER_BYTE_CAPACITY / 2
        );
        queue.finish_send();
        assert!(matches!(queue.push(render(&third)), SendQueuePush::Queued));
        assert_eq!(queue.pop(), Some(control("notice")));
        assert!(matches!(
            queue.push(render("still full")),
            SendQueuePush::ControlQueueFull(_)
        ));
        let exit = ServerToClientMsg::Exit {
            exit_reason: ExitReason::Normal,
        };
        assert!(matches!(queue.push(exit.clone()), SendQueuePush::Queued));
        queue.close();
        assert_eq!(queue.pop(), Some(render(&second)));
        assert_eq!(queue.pop(), Some(render(&third)));
        assert_eq!(queue.pop(), Some(exit));
        assert_eq!(queue.pop(), None);
        assert_eq!(queue.state.lock().unwrap().render_bytes, 0);
    }

    #[test]
    fn render_backlog_counts_frames_and_coalesced_pane_snapshots() {
        let queue = CoalescingSendQueue::new();
        assert_eq!(queue.render_backlog(), RenderBacklog::default());
        for message in [
            render("first"),
            control("notice"),
            pane_render(DataPaneId::Terminal(TerminalId::new(1)), "pane"),
            render("second"),
        ] {
            assert!(matches!(queue.push(message), SendQueuePush::Queued));
        }
        assert_eq!(queue.render_backlog().queued_renders, 3);
        assert!(matches!(
            queue.push(pane_render(
                DataPaneId::Terminal(TerminalId::new(1)),
                "newer pane"
            )),
            SendQueuePush::CoalescedRender
        ));
        assert_eq!(queue.render_backlog().queued_renders, 3);
        assert_eq!(queue.pop(), Some(render("first")));
        assert_eq!(queue.render_backlog().queued_renders, 2);
        queue.finish_send();
        assert_eq!(queue.pop(), Some(control("notice")));
        assert_eq!(queue.render_backlog().queued_renders, 2);
        assert_eq!(queue.pop(), Some(render("second")));
        assert_eq!(queue.render_backlog().queued_renders, 1);
        assert_eq!(
            queue.pop(),
            Some(pane_render(
                DataPaneId::Terminal(TerminalId::new(1)),
                "newer pane"
            ))
        );
        queue.finish_send();
        assert_eq!(queue.render_backlog(), RenderBacklog::default());
    }

    #[test]
    fn overflow_snapshot_reflects_a_drained_queue() {
        let queue = CoalescingSendQueue::new();
        assert!(matches!(queue.push(render("x")), SendQueuePush::Queued));
        assert_eq!(queue.pop(), Some(render("x")));
        queue.finish_send();
        let details = overflow(
            &queue,
            render(&"z".repeat(SEND_QUEUE_RENDER_BYTE_CAPACITY + 1)),
        );
        assert_eq!(details.ordered, 0);
        assert_eq!(details.render_bytes, 0);
        assert!(details.in_flight.is_none());
    }

    #[test]
    fn an_oversized_render_leaves_the_byte_budget_untouched() {
        let queue = CoalescingSendQueue::new();
        let details = overflow(
            &queue,
            render(&"x".repeat(SEND_QUEUE_RENDER_BYTE_CAPACITY + 1)),
        );
        assert_eq!(details.reason, "single_oversized_render");
        assert_eq!(
            details.rejected_render_len,
            SEND_QUEUE_RENDER_BYTE_CAPACITY + 1
        );
        assert_eq!(
            details.rejected_render_capacity,
            SEND_QUEUE_RENDER_BYTE_CAPACITY + 1
        );
        assert_eq!(details.render_bytes, 0);
        assert_eq!(queue.len(), 0);
        let content = "y".repeat(SEND_QUEUE_RENDER_BYTE_CAPACITY);
        assert!(matches!(
            queue.push(render(&content)),
            SendQueuePush::Queued
        ));
        queue.close();
        assert!(matches!(queue.push(render("late")), SendQueuePush::Queued));
        assert_eq!(queue.pop(), Some(render(&content)));
        assert_eq!(queue.pop(), None);
        assert_eq!(queue.state.lock().unwrap().render_bytes, 0);
    }

    #[test]
    fn render_byte_budget_counts_and_releases_spare_string_capacity() {
        let queue = CoalescingSendQueue::new();
        let mut content = String::with_capacity(SEND_QUEUE_RENDER_BYTE_CAPACITY);
        content.push('x');
        assert!(matches!(
            queue.push(ServerToClientMsg::Render { content }),
            SendQueuePush::Queued
        ));
        let details = overflow(&queue, render("overflow"));
        assert_eq!(details.reason, "render_allocation_bytes");
        assert_eq!(details.ordered, 1);
        assert_eq!(details.render_bytes, SEND_QUEUE_RENDER_BYTE_CAPACITY);
        assert_eq!(details.rejected_render_len, 8);
        assert_eq!(details.rejected_render_capacity, 8);
        assert_eq!(queue.pop(), Some(render("x")));
        assert_eq!(queue.state.lock().unwrap().render_bytes, 0);
        queue.finish_send();
        let content = "y".repeat(SEND_QUEUE_RENDER_BYTE_CAPACITY);
        assert!(matches!(
            queue.push(render(&content)),
            SendQueuePush::Queued
        ));
        queue.close();
        assert_eq!(queue.pop(), Some(render(&content)));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn overflow_reports_an_active_send_and_clears_it_on_finish() {
        let queue = CoalescingSendQueue::new();
        assert!(matches!(
            queue.push(render("sending")),
            SendQueuePush::Queued
        ));
        assert_eq!(queue.pop(), Some(render("sending")));
        for _ in 0..SEND_QUEUE_CONTROL_CAPACITY {
            assert!(matches!(queue.push(render("x")), SendQueuePush::Queued));
        }
        let details = overflow(&queue, render("rejected"));
        let active = details.in_flight.unwrap();
        assert_eq!(active.kind, "Render");
        assert_eq!(active.render_bytes, 7);
        assert!(active.started.elapsed() <= Duration::from_secs(1));
        queue.finish_send();
        assert!(overflow(&queue, render("rejected")).in_flight.is_none());
    }

    #[test]
    fn slow_ipc_warnings_keep_suppressed_count_and_maximum() {
        let start = Instant::now();
        let mut warnings = SlowWarnings::default();
        assert_eq!(warnings.record(start, Duration::from_millis(99)), None);
        assert_eq!(
            warnings.record(start, Duration::from_millis(100)),
            Some((0, Duration::ZERO))
        );
        assert_eq!(
            warnings.record(
                start + Duration::from_millis(50),
                Duration::from_millis(250)
            ),
            None
        );
        assert_eq!(warnings.finish(), Some((1, Duration::from_millis(250))));
        assert_eq!(
            warnings.record(start + Duration::from_secs(1), Duration::from_millis(120)),
            Some((1, Duration::from_millis(250)))
        );
        assert_eq!(warnings.finish(), None);
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
    fn initial_and_scrollback_pane_renders_replace_older_pending_viewports() {
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
        assert_eq!(queue.len(), 1);
        assert!(matches!(
            queue.push(pane_render(pane, "before scrollback")),
            SendQueuePush::Queued
        ));
        assert!(matches!(
            queue.push(with_scrollback.clone()),
            SendQueuePush::Queued
        ));
        assert_eq!(queue.len(), 2);
        assert!(matches!(
            queue.push(pane_render(pane, "latest")),
            SendQueuePush::Queued
        ));
        queue.close();
        assert_eq!(queue.pop(), Some(initial));
        assert_eq!(queue.pop(), Some(with_scrollback));
        assert_eq!(queue.pop(), Some(pane_render(pane, "latest")));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn ordered_messages_precede_replaceable_pane_snapshots() {
        let queue = CoalescingSendQueue::new();
        let pane = DataPaneId::Plugin(yinz_types::internal_ids::PluginId::new(7));
        assert!(matches!(queue.push(render("full")), SendQueuePush::Queued));
        assert!(matches!(
            queue.push(pane_render(pane, "pane")),
            SendQueuePush::Queued
        ));
        assert!(matches!(queue.push(control("ctl")), SendQueuePush::Queued));
        queue.close();
        assert_eq!(queue.pop(), Some(render("full")));
        assert_eq!(queue.pop(), Some(control("ctl")));
        assert_eq!(queue.pop(), Some(pane_render(pane, "pane")));
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
            DataPaneId::Terminal(yinz_types::internal_ids::TerminalId::new(n))
        } else {
            DataPaneId::Plugin(yinz_types::internal_ids::PluginId::new(n))
        }
    }

    struct QueueModel {
        real: CoalescingSendQueue,
        control: VecDeque<ServerToClientMsg>,
        pane_renders: BTreeMap<DataPaneId, ServerToClientMsg>,
        closed: bool,
        seq: u32,
    }

    impl QueueModel {
        fn new() -> Self {
            QueueModel {
                real: CoalescingSendQueue::new(),
                control: VecDeque::new(),
                pane_renders: BTreeMap::new(),
                closed: false,
                seq: 0,
            }
        }
        fn next_seq(&mut self) -> String {
            self.seq += 1;
            self.seq.to_string()
        }
        fn render_bytes(&self) -> usize {
            self.control
                .iter()
                .map(|msg| match msg {
                    ServerToClientMsg::Render { content } => content.capacity(),
                    _ => 0,
                })
                .sum()
        }
    }

    #[hegel::state_machine]
    impl QueueModel {
        /// What the model says the control FIFO does with `msg`, and the check against the
        /// real queue. Shared by every message that takes the FIFO path.
        fn push_fifo(&mut self, msg: ServerToClientMsg) {
            let render_bytes = match &msg {
                ServerToClientMsg::Render { content } => content.capacity(),
                _ => 0,
            };
            let outcome = self.real.push(msg.clone());
            if self.closed {
                assert!(matches!(outcome, SendQueuePush::Queued));
            } else if self.control.len() >= SEND_QUEUE_CONTROL_CAPACITY
                || self.render_bytes() + render_bytes > SEND_QUEUE_RENDER_BYTE_CAPACITY
            {
                assert!(matches!(outcome, SendQueuePush::ControlQueueFull(_)));
            } else {
                assert!(matches!(outcome, SendQueuePush::Queued));
                if let ServerToClientMsg::PaneRenderUpdate { pane_id, .. } = &msg {
                    self.pane_renders.remove(pane_id);
                }
                self.control.push_back(msg);
            }
        }

        #[rule]
        fn push_control(&mut self, _tc: hegel::TestCase) {
            let msg = control(&self.next_seq());
            self.push_fifo(msg);
        }

        #[rule]
        fn push_full_render(&mut self, tc: hegel::TestCase) {
            let repeats = tc.draw(
                generators::integers::<usize>().max_value(SEND_QUEUE_RENDER_BYTE_CAPACITY + 1),
            );
            let msg = render(&self.next_seq().repeat(repeats));
            self.push_fifo(msg);
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
            let model_has_message = !self.control.is_empty() || !self.pane_renders.is_empty();
            // pop blocks on an open empty queue; only pop when it can't block
            tc.assume(model_has_message || self.closed);
            let expected = if let Some(msg) = self.control.pop_front() {
                Some(msg)
            } else if let Some((_, msg)) = self.pane_renders.pop_first() {
                Some(msg)
            } else {
                None
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
                self.real.render_backlog().queued_renders,
                if self.closed {
                    0
                } else {
                    self.control
                        .iter()
                        .filter(|message| matches!(message, ServerToClientMsg::Render { .. }))
                        .count()
                        + self.pane_renders.len()
                },
            );
            assert_eq!(
                self.real.len(),
                self.control.len() + self.pane_renders.len(),
            );
            assert_eq!(
                self.real.state.lock().unwrap().render_bytes,
                self.render_bytes(),
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
        yinz_ipc::ipc::ClientToServerMsg, yinz_ipc::ipc::ExitReason,
        yinz_ipc::ipc::IpcReceiverWithContext, yinz_ipc::ipc::IpcRecv,
        yinz_ipc::ipc::IpcSenderWithContext, yinz_ipc::ipc::ServerToClientMsg,
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
        )
        .unwrap();

        // What every break in the server main loop does on its way out.
        crate::os_input_output::finish_enqueue(
            crate::ClientId::new(1),
            sender.queue.push_deferred(ServerToClientMsg::Exit {
                exit_reason: ExitReason::Normal,
            }),
        )
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

    #[test]
    fn failed_send_clears_in_flight_state() {
        use std::os::fd::AsFd;
        let (_guard, server_end, client_end) = socket_pair();
        let server_fd = match &server_end {
            interprocess::local_socket::Stream::UdSocket(uds) => {
                uds.as_fd().try_clone_to_owned().expect("dup failed")
            },
        };
        let live_writers = Arc::new(LiveWriters::default());
        let sender = ClientSender::new(
            crate::ClientId::new(1),
            IpcSenderWithContext::<ServerToClientMsg>::new(server_end),
            live_writers.clone(),
            server_fd,
        )
        .unwrap();
        let queue = sender.queue.clone();
        drop(client_end);
        crate::os_input_output::finish_enqueue(
            crate::ClientId::new(1),
            sender.queue.push_deferred(ServerToClientMsg::Render {
                content: "x".to_owned(),
            }),
        )
        .unwrap();
        sender.close();
        assert!(live_writers.wait_until_drained(Duration::from_secs(5)));
        assert!(queue.state.lock().unwrap().in_flight.is_none());
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
        )
        .unwrap();
        crate::os_input_output::finish_enqueue(
            crate::ClientId::new(1),
            sender.queue.push_deferred(ServerToClientMsg::Exit {
                exit_reason: ExitReason::Normal,
            }),
        )
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
    /// `connect` to it never returned, hanging every yinz command on the
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
            receiver.get_sender().unwrap(),
            live_writers.clone(),
            server_fd,
        )
        .unwrap();

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
    use yinz_runtime::termios::WireTermios;

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

#[test]
fn note_file_editor_precedence_and_arguments_use_executable_stubs() {
    const CHILD: &str = "YINZ_TEST_NOTE_EDITOR_PRECEDENCE";
    const NAME: &str = "os_input_output::os_input_output_tests::note_file_editor_precedence_and_arguments_use_executable_stubs";
    if env::var_os(CHILD).is_none() {
        let directory = tempfile::tempdir().unwrap();
        for name in ["configured", "editor", "visual", "vi"] {
            yinz_test_support::write_warm_script(
                &directory.path().join(name),
                "printf '%s\\n' \"$0\" \"$@\"\n",
            );
        }
        for level in ["configured", "editor", "visual", "vi"] {
            let mut command = Command::new(env::current_exe().unwrap());
            command
                .args(["--exact", NAME])
                .env(CHILD, level)
                .env("PATH", directory.path())
                .env("YINZ_DISABLE_PTY_HOST", "1")
                .env_remove("EDITOR")
                .env_remove("VISUAL");
            if level != "vi" {
                command.env("VISUAL", "visual --visual-arg");
            }
            if matches!(level, "configured" | "editor") {
                command.env("EDITOR", "editor --editor-arg");
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{level}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        }
        return;
    }
    let level = env::var(CHILD).unwrap();
    let payload =
        yinz_types::input::command::OpenFilePayload::new("/tmp/note.md".into(), None, None);
    let configured = (level == "configured").then(|| PathBuf::from("configured --configured-arg"));
    let (command, fallback) = build_command(TerminalAction::OpenFile(payload), configured);
    assert_eq!(command.command, PathBuf::from(&level));
    assert_eq!(command.args.last().unwrap(), "/tmp/note.md");
    assert_eq!(command.args.len(), if level == "vi" { 1 } else { 2 });
    if level != "vi" {
        assert_eq!(command.args[0], format!("--{level}-arg"));
    }
    assert!(fallback.is_none());
    let output = Command::new(&command.command)
        .args(&command.args)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("/tmp/note.md"));
}

/// A Bubblewrap launch that cannot be checked is refused, never run on the host, and the
/// refusal names the pane it belongs in.
#[test]
fn a_bubblewrap_launch_that_cannot_resolve_is_a_pane_start_failure() {
    let target = PaneTarget::Bubblewrap {
        root: "/repo".into(),
    };
    let no_directory = RunCommand {
        command: "claude".into(),
        target: target.clone(),
        ..Default::default()
    };
    let refused = make_server()
        .resolve_target(no_directory, None)
        .unwrap_err();
    let failed = resolve_failed(TerminalId::new(7), &target, refused);
    match failed.downcast_ref::<YinzError>() {
        Some(YinzError::PaneStartFailed {
            terminal_id,
            message,
        }) => {
            assert_eq!(*terminal_id, TerminalId::new(7));
            assert!(message.contains("Bubblewrap environment"), "{message}");
            assert!(message.contains("working directory"), "{message}");
        },
        other => panic!("expected a pane start failure, got {other:?}"),
    }

    // A program missing from the sandbox's PATH reads as any missing command does.
    let missing = resolve_failed(
        TerminalId::new(7),
        &target,
        YinzError::CommandNotFound {
            terminal_id: TerminalId::new(0),
            command: "claude".into(),
        }
        .into(),
    );
    assert!(matches!(
        missing.downcast_ref::<YinzError>(),
        Some(YinzError::CommandNotFound { terminal_id, command })
            if *terminal_id == TerminalId::new(7) && command == "claude"
    ));
}

#[test]
fn remote_codex_wrappers_keep_the_remote_executable() {
    let server = make_server();
    for with_env in [false, true] {
        let requested = RunCommand {
            target: PaneTarget::ssh("build-box").unwrap(),
            ..codex_command(with_env)
        };
        let (resolved, rerun, endpoint) = server.resolve_target(requested.clone(), None).unwrap();
        assert_eq!(resolved, requested);
        assert_eq!(rerun, requested);
        assert_eq!(endpoint.unwrap().host, "build-box");
    }
}
