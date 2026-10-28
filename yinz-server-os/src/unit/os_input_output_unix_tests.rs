use super::*;
use nix::fcntl::{fcntl, FcntlArg, OFlag};
use nix::pty::openpty;
use nix::sys::termios;
use std::fs::File;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};

#[test]
fn failed_in_process_spawns_report_errors_and_close_the_pty() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("broken-command");
    std::fs::write(&script, b"#!/nonexistent/yinz-test-interpreter\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let non_executable = dir.path().join("non-executable");
    std::fs::write(&non_executable, b"#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&non_executable, std::fs::Permissions::from_mode(0o600)).unwrap();

    let terminal_id = TerminalId::new(1);
    for (command, expected_errno) in [
        (dir.path().join("missing-command"), None),
        // The executable passes preflight; its missing interpreter fails at exec.
        (script, Some(libc::ENOENT)),
        (non_executable, None),
    ] {
        let pty = crate::pty_host::terminal::open(None, None).unwrap();
        let mut master = File::from(pty.master.try_clone().unwrap());
        fcntl(&master, FcntlArg::F_SETFL(OFlag::O_NONBLOCK)).unwrap();
        let result = handle_openpty(
            pty,
            RunCommand {
                command: command.clone(),
                ..Default::default()
            },
            Box::new(|_, _, _| panic!("a failed spawn cannot produce a child exit")),
            None,
            terminal_id,
            &"test".into(),
        );
        let error = result.expect_err("a failed spawn must reach the caller");
        match (error.downcast_ref::<YinzError>(), expected_errno) {
            (
                Some(YinzError::CommandNotFound {
                    terminal_id: id, ..
                }),
                None,
            ) => {
                assert_eq!(*id, terminal_id);
            },
            (
                Some(YinzError::PaneStartFailed {
                    terminal_id: id,
                    message,
                }),
                Some(errno),
            ) => {
                assert_eq!(*id, terminal_id);
                assert!(message.contains(&command.display().to_string()));
                assert!(message.contains(&io::Error::from_raw_os_error(errno).to_string()));
            },
            _ => panic!("spawn error must identify the failed pane: {error:#}"),
        }
        // The slave must have been closed even when validation or exec failed.
        // Linux returns EIO and BSDs return EOF once the last slave is gone.
        let read = master.read(&mut [0u8; 1]);
        assert!(
            matches!(&read, Ok(0))
                || matches!(&read, Err(e) if e.raw_os_error() == Some(libc::EIO)),
            "a failed spawn leaked its pty slave: {read:?}"
        );
    }
}

#[test]
fn a_resize_for_a_terminal_with_no_pty_is_an_error() {
    // A pane that has no pty to receive a resize must not read as resized.
    let backend = UnixPtyBackend::with_termios(None).expect("a backend");
    backend
        .terminal_id_to_transport
        .lock()
        .unwrap()
        .insert(TerminalId::new(7), PaneTransport::Reserved);

    assert!(backend
        .set_terminal_size(TerminalId::new(7), 80, 24, None, None)
        .is_err());
    assert!(backend
        .set_terminal_size(TerminalId::new(8), 80, 24, None, None)
        .is_err());
}

#[test]
fn in_process_pane_exit_reports_status_after_reaping_the_child() {
    for (command, expected_status) in [("exit 7", Some(7)), ("kill -TERM $$", None)] {
        let (exited, exit) = std::sync::mpsc::channel();
        let terminal_id = TerminalId::new(1);
        let run_command = RunCommand {
            command: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), command.into()],
            ..Default::default()
        };
        let (master, _) = handle_terminal(
            run_command.clone(),
            None,
            Box::new(move |pane_id, status, command| {
                exited.send((pane_id, status, command)).unwrap();
            }),
            None,
            terminal_id,
            &"test".into(),
        )
        .unwrap();
        assert_ne!(
            fcntl(&master, FcntlArg::F_GETFD).unwrap() & libc::FD_CLOEXEC,
            0,
            "the pty must not leak into unrelated children"
        );
        let result = exit.recv_timeout(Duration::from_secs(10));
        drop(master);
        assert_eq!(
            result.expect("the pane exit must reach its quit callback"),
            (PaneId::Terminal(terminal_id), expected_status, run_command)
        );
    }
}

/// Not a test: the entry point `original_termios_reaches_both_local_pane_paths` re-execs
/// this binary at, to run a real supervisor over the production fd-3 handoff. In every
/// other run - which is every run of the suite - the variable is unset and this is a
/// deliberate no-op, so it can never go red on its own.
#[test]
fn termios_supervisor_entry() {
    if std::env::var_os("YINZ_TEST_TERMIOS_SUPERVISOR").is_some() {
        if let Some(gate) = std::env::var_os("YINZ_TEST_SPAWN_GATE") {
            use std::io::Write;
            let mut gate = std::os::unix::net::UnixStream::connect(gate).unwrap();
            gate.set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            gate.write_all(b"ready").unwrap();
            gate.read_exact(&mut [0]).unwrap();
        }
        crate::pty_host::run_pty_host("termios-test/0".to_owned());
    }
    // Otherwise this is the suite proper, and the test below is what exercises this path.
}

#[test]
fn a_spawn_overlapping_rename_is_registered_before_the_new_name_is_published() {
    use std::io::Write;
    use std::os::unix::net::UnixListener;
    let dir = tempfile::tempdir().unwrap();
    let socket_root = dir.path().join("sock");
    let gate_path = dir.path().join("gate");
    let gate = UnixListener::bind(&gate_path).unwrap();
    let exe = std::env::current_exe().unwrap();
    let wrapper = dir.path().join("supervisor");
    yinz_test_support::write_warm_script(&wrapper, &format!(
            "YINZ_SOCKET_DIR='{}' YINZ_TEST_SPAWN_GATE='{}' YINZ_TEST_TERMIOS_SUPERVISOR=1 exec '{}' --exact os_input_output_unix::tests::termios_supervisor_entry --nocapture\n",
            socket_root.display(), gate_path.display(), exe.display(),
        ));
    let previous_name = yinz_runtime::envs::get_session_name().ok();
    let mut backend = UnixPtyBackend::with_termios(None).unwrap();
    backend.set_session_state(SessionState::new("spawn-old".into()));
    backend.host_bin = Some(wrapper);
    backend.pty_host_enabled.store(true, Ordering::SeqCst);
    let spawning = backend.clone();
    let spawn = thread::spawn(move || {
        spawning.spawn_terminal(
            RunCommand {
                command: PathBuf::from("/bin/sh"),
                args: vec!["-c".into(), "read line".into()],
                ..Default::default()
            },
            Box::new(|_, _, _| {}),
            TerminalId::new(0),
        )
    });
    let (mut gate, _) = gate.accept().unwrap();
    gate.set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    gate.read_exact(&mut [0; 5]).unwrap();
    // The real spawn is in flight, before the supervisor handshake and
    // registration. Rename must use this exact lock, across backend clones.
    assert!(matches!(
        backend.ownership.try_lock(),
        Err(std::sync::TryLockError::WouldBlock)
    ));
    assert!(backend.terminal_id_to_host.lock().unwrap().is_empty());
    let renaming = backend.clone();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let started = barrier.clone();
    let rename = thread::spawn(move || {
        started.wait();
        renaming.rename_pty_host_session_in(
            &socket_root.join("pty"),
            &"spawn-old".into(),
            &"spawn-new".into(),
        )
    });
    barrier.wait();
    assert_eq!(backend.session_name().as_str(), "spawn-old");
    gate.write_all(b"go").unwrap();
    let (_reader, child) = spawn.join().unwrap().unwrap();
    let result = rename.join().unwrap();
    let socket = backend
        .terminal_id_to_host
        .lock()
        .unwrap()
        .get(&TerminalId::new(0))
        .unwrap()
        .control_socket
        .clone()
        .unwrap();
    let status = pty_host_client::query(&socket).unwrap();
    let rejected = backend.rename_pty_host_session_in(
        socket.parent().unwrap(),
        &"spawn-old".into(),
        &"must-not-publish".into(),
    );
    backend.begin_confirmed_pty_host_shutdown().unwrap();
    backend.shutdown_pty_hosts();
    backend.confirm_pty_host_shutdown().unwrap();
    let published = backend.session_name();
    assert_eq!(yinz_runtime::envs::get_session_name().ok(), previous_name);
    result.unwrap();
    assert!(rejected.is_err(), "a stale old name must reject rename");
    assert_eq!(published.as_str(), "spawn-new");
    assert_eq!(status.session_name.unwrap(), published);
    assert_eq!(status.child_pid, Some(child));
    assert!(status.attached);
}

#[test]
fn both_local_pane_paths_use_current_session_without_mutating_parent_environment() {
    let inherited_name = std::env::var_os("YINZ_SESSION_NAME");
    let inherited_yinz = std::env::var_os("YINZ");
    let dir = tempfile::tempdir().unwrap();
    let supervisor = dir.path().join("supervisor");
    let exe = std::env::current_exe().unwrap();
    yinz_test_support::write_warm_script(&supervisor, &format!(
        "YINZ_SOCKET_DIR='{}' YINZ_TEST_TERMIOS_SUPERVISOR=1 exec '{}' --exact os_input_output_unix::tests::termios_supervisor_entry --nocapture\n",
        dir.path().join("sock").display(),
        exe.display().to_string().replace('\'', "'\"'\"'")
    ));
    for detachable in [false, true] {
        let mut backend = UnixPtyBackend::with_termios(None).unwrap();
        let state = SessionState::new("before-rename".into());
        backend.set_session_state(state.clone());
        backend.host_bin = Some(supervisor.clone());
        backend.pty_host_enabled.store(detachable, Ordering::SeqCst);
        state.set("after-rename".into());
        for launch_override in [false, true] {
            let expected = if launch_override {
                "launch-override"
            } else {
                "after-rename"
            };
            let mut cmd = RunCommand {
                command: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    "[ \"$YINZ\" = 0 ] && [ \"$YINZ_SESSION_NAME\" = \"$1\" ]".into(),
                    "check-session".into(),
                    expected.into(),
                ],
                ..Default::default()
            };
            if launch_override {
                cmd.command = "/usr/bin/env".into();
                cmd.args.splice(
                    0..0,
                    ["YINZ_SESSION_NAME=launch-override".into(), "/bin/sh".into()],
                );
            }
            let (exited, exit) = std::sync::mpsc::channel();
            let id = TerminalId::new(0);
            let (_reader, _) = backend
                .spawn_terminal(
                    cmd,
                    Box::new(move |_, status, _| exited.send(status).unwrap()),
                    id,
                )
                .unwrap();
            assert_eq!(
                backend
                    .terminal_id_to_host
                    .lock()
                    .unwrap()
                    .contains_key(&id),
                detachable
            );
            let status = exit.recv_timeout(Duration::from_secs(30));
            backend.clear_terminal_id(id).unwrap();
            assert_eq!(
                status.unwrap(),
                Some(0),
                "detachable={detachable}, launch_override={launch_override}"
            );
        }
    }
    assert_eq!(std::env::var_os("YINZ_SESSION_NAME"), inherited_name);
    assert_eq!(std::env::var_os("YINZ"), inherited_yinz);
}

#[tokio::test]
async fn original_termios_reaches_both_local_pane_paths() {
    use termios::{InputFlags, LocalFlags, OutputFlags, SetArg, SpecialCharacterIndices};
    let reference = openpty(None, None).unwrap();
    let mut cooked = termios::tcgetattr(&reference.slave).unwrap();
    cooked.input_flags.insert(InputFlags::ICRNL);
    cooked.local_flags.insert(LocalFlags::ICANON);
    cooked.local_flags.remove(LocalFlags::ECHO);
    cooked.output_flags.remove(OutputFlags::OPOST);
    cooked.control_chars[SpecialCharacterIndices::VERASE as usize] = 8;
    termios::tcsetattr(&reference.slave, SetArg::TCSANOW, &cooked).unwrap();
    let snapshot = WireTermios::from_termios(&termios::tcgetattr(&reference.slave).unwrap());
    let mut raw = cooked.clone();
    termios::cfmakeraw(&mut raw);
    raw.local_flags.insert(LocalFlags::ISIG);
    termios::tcsetattr(&reference.slave, SetArg::TCSANOW, &raw).unwrap();
    assert!(!termios::tcgetattr(&reference.slave)
        .unwrap()
        .input_flags
        .contains(InputFlags::ICRNL));
    let json = serde_json::to_string(&Some(snapshot.clone())).unwrap();
    let decoded: Option<WireTermios> = serde_json::from_str(&json).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let supervisor = dir.path().join("supervisor");
    let exe = std::env::current_exe().unwrap();
    // Re-exec the test binary as a real supervisor, with the production fd-3 handoff.
    yinz_test_support::write_warm_script(&supervisor, &format!(
            "YINZ_TEST_TERMIOS_SUPERVISOR=1 exec '{}' --exact os_input_output_unix::tests::termios_supervisor_entry --nocapture\n",
            exe.display().to_string().replace('\'', "'\"'\"'")
        ));
    for detachable in [false, true] {
        let mut backend =
            UnixPtyBackend::with_termios(decoded.as_ref().map(WireTermios::to_termios)).unwrap();
        backend.host_bin = Some(supervisor.clone());
        backend.pty_host_enabled.store(detachable, Ordering::SeqCst);
        let id = TerminalId::new(0);
        let (mut reader, pid) = backend
            .spawn_terminal(
                RunCommand {
                    command: PathBuf::from("/bin/sh"),
                    args: vec![
                        "-c".to_owned(),
                        "printf 'ready\\n'; IFS= read -r answer; printf '%s\\n' \"$answer\""
                            .to_owned(),
                    ],
                    ..Default::default()
                },
                Box::new(|_, _, _| {}),
                id,
            )
            .unwrap();
        assert_eq!(
            backend
                .terminal_id_to_host
                .lock()
                .unwrap()
                .contains_key(&id),
            detachable,
            "the detachable path must not silently fall back"
        );
        let master = match backend
            .terminal_id_to_transport
            .lock()
            .unwrap()
            .get(&id)
            .unwrap()
        {
            PaneTransport::Local(fd) => fd.clone(),
            _ => panic!("expected local PTY"),
        };
        let applied = termios::tcgetattr(master).unwrap();
        assert_eq!(WireTermios::from_termios(&applied), snapshot);
        let outcome = tokio::time::timeout(Duration::from_secs(10), async {
            let mut output = Vec::new();
            let mut buf = [0; 128];
            while !output.ends_with(b"ready\n") {
                let n = reader.read(&mut buf).await.unwrap();
                assert_ne!(n, 0, "prompt exited before readiness");
                output.extend_from_slice(&buf[..n]);
            }
            backend.write_to_tty_stdin(id, b"y\r").unwrap();
            output.clear();
            while !output.ends_with(b"y\n") {
                let n = reader.read(&mut buf).await.unwrap();
                assert_ne!(n, 0, "prompt exited without reading the answer");
                output.extend_from_slice(&buf[..n]);
            }
        })
        .await;
        backend.kill(pid).unwrap();
        backend.clear_terminal_id(id).unwrap();
        assert!(
            outcome.is_ok(),
            "Enter did not complete the line read (detachable={detachable})"
        );
    }
}

/// Verify that `try_write_to_fd` writes as many bytes as the kernel will
/// accept in one pass and returns a partial count (not an error) when the
/// PTY buffer fills up.
///
/// A concurrent reader drains the slave side so some bytes are accepted.
/// The key assertion: the function returns Ok(n) where n <= buf.len(),
/// and the caller (PtyWriter) is responsible for re-queuing the rest.
#[test]
fn try_write_to_fd_returns_partial_on_full_buffer() {
    let pty = openpty(None, &None).expect("openpty failed");
    let master_fd = pty.master.into_raw_fd();
    let slave_fd = pty.slave.into_raw_fd();
    let borrowed_master = unsafe { BorrowedFd::borrow_raw(master_fd) };
    let borrowed_slave = unsafe { BorrowedFd::borrow_raw(slave_fd) };

    let mut attrs = termios::tcgetattr(borrowed_slave).expect("tcgetattr failed");
    termios::cfmakeraw(&mut attrs);
    termios::tcsetattr(borrowed_slave, termios::SetArg::TCSANOW, &attrs).expect("tcsetattr failed");

    // O_NONBLOCK so write() returns EAGAIN instead of blocking
    let flags = fcntl(borrowed_master, FcntlArg::F_GETFL).expect("F_GETFL");
    let mut oflags = OFlag::from_bits_truncate(flags);
    oflags.insert(OFlag::O_NONBLOCK);
    fcntl(borrowed_master, FcntlArg::F_SETFL(oflags)).expect("F_SETFL");

    // Fill most of the buffer, leaving some space
    let chunk = vec![0x42u8; 1024];
    let mut total_filled = 0;
    loop {
        match super::try_write_to_fd(master_fd, &chunk) {
            Ok(0) => break,
            Ok(n) => total_filled += n,
            Err(e) => panic!("unexpected error filling buffer: {e}"),
        }
    }
    assert!(
        total_filled > 0,
        "should have written some bytes to fill buffer"
    );

    // Read a small amount from the slave to free partial space
    let mut drain = vec![0u8; 512];
    let slave_file = unsafe { std::fs::File::from_raw_fd(slave_fd) };
    let mut slave_reader = std::io::BufReader::new(&slave_file);
    let drained = slave_reader.read(&mut drain).expect("slave read failed");
    assert!(drained > 0, "should have drained some bytes");
    // Prevent File from closing the slave fd — we close it manually below
    std::mem::forget(slave_file);

    // Now write more than the freed space — should get a partial write
    let size = 128 * 1024;
    let data: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();
    let written = super::try_write_to_fd(master_fd, &data)
        .expect("try_write_to_fd should not error on EAGAIN");

    assert!(
        written > 0 && written < size,
        "expected partial write, got {written}/{size}",
    );

    // Which bytes were accepted matters as much as how many: the caller re-queues
    // `&data[written..]`, so the kernel must hold exactly the prefix. Drain the slave
    // and check what arrived after the leftover fill is that prefix.
    let slave_flags = fcntl(borrowed_slave, FcntlArg::F_GETFL).expect("F_GETFL");
    let mut slave_oflags = OFlag::from_bits_truncate(slave_flags);
    slave_oflags.insert(OFlag::O_NONBLOCK);
    fcntl(borrowed_slave, FcntlArg::F_SETFL(slave_oflags)).expect("F_SETFL");
    let mut read_back = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match nix::unistd::read(borrowed_slave, &mut buf) {
            Ok(0) => break,
            Ok(n) => read_back.extend_from_slice(&buf[..n]),
            Err(nix::errno::Errno::EAGAIN) => break,
            Err(nix::errno::Errno::EINTR) => continue,
            Err(e) => panic!("draining the slave failed: {e}"),
        }
    }
    assert!(
        read_back.ends_with(&data[..written]),
        "the {written} accepted bytes are not the prefix of the buffer"
    );
    assert!(
        read_back.len() >= written,
        "read back {} bytes, fewer than the {written} reported written",
        read_back.len()
    );

    unsafe {
        libc::close(master_fd);
        libc::close(slave_fd);
    }
}

/// Only EAGAIN and a zero-length write mean "stop here and re-queue the rest". Any
/// other errno is how `PtyWriter` learns a pane's fd is gone, and swallowing it would
/// leave the pane's writes silently dropped forever.
#[test]
fn try_write_to_fd_propagates_an_error_that_is_not_eagain() {
    // A descriptor open for reading only fails every write with EBADF, and stays
    // owned for the whole call - no closed-fd-number reuse to worry about.
    let read_only = std::fs::File::open("/dev/null").expect("open /dev/null");
    let error = super::try_write_to_fd(read_only.as_raw_fd(), b"gone")
        .expect_err("a write to a read-only fd must be an error, not a short write");
    assert!(error.to_string().contains("EBADF"), "{error}");
}

/// Verify that `try_write_to_fd` returns Ok(0) — not an error — when the
/// fd cannot accept the requested write.
#[test]
fn try_write_to_fd_returns_zero_on_full_pipe() {
    // A PTY's line discipline can free capacity after EAGAIN even without a slave
    // reader. An unread pipe keeps this precondition stable between the two writes.
    let (_reader, writer) = unistd::pipe().expect("pipe failed");
    let flags = fcntl(&writer, FcntlArg::F_GETFL).expect("F_GETFL");
    let mut oflags = OFlag::from_bits_truncate(flags);
    oflags.insert(OFlag::O_NONBLOCK);
    fcntl(&writer, FcntlArg::F_SETFL(oflags)).expect("F_SETFL");

    // Establish EAGAIN independently of the function under test. Use the same
    // payload below: a smaller atomic pipe write could still fit in the buffer.
    let fill = [0x42u8; 1024];
    let deadline = std::time::Instant::now() + yinz_test_support::default_timeout();
    loop {
        assert!(std::time::Instant::now() < deadline, "pipe did not fill");
        match unistd::write(&writer, &fill) {
            Ok(n) => assert!(n > 0, "pipe write made no progress"),
            Err(nix::errno::Errno::EAGAIN) => break,
            Err(nix::errno::Errno::EINTR) => continue,
            Err(e) => panic!("unexpected error filling buffer: {e}"),
        }
    }

    let written = super::try_write_to_fd(writer.as_raw_fd(), &fill)
        .expect("try_write_to_fd should not error on EAGAIN");
    assert_eq!(written, 0, "expected zero bytes written on full buffer");
}

/// The real backend, releasing a remote pane, on the thread that would
/// deadlock.
///
/// The lookup that finds the pane's connection and the release that removes
/// it take the same lock, and a guard kept alive across the two — which an
/// `if let` scrutinee did — blocked the pty thread for good. The peer here
/// hangs up at once, so a release that returns at all returns in
/// milliseconds; one that has not returned in five seconds is the deadlock.
#[test]
fn releasing_a_remote_pane_does_not_deadlock_on_the_transport_table() {
    use std::os::unix::net::UnixStream;

    let backend = UnixPtyBackend::new().expect("a backend");
    let (ours, theirs) = UnixStream::pair().expect("socketpair failed");
    drop(theirs);
    let conn = RemoteConn::new(
        ours,
        "build-box".to_owned(),
        "tok".to_owned(),
        RunCommand::default(),
        None,
    )
    .expect("a remote connection");
    backend
        .terminal_id_to_transport
        .lock()
        .unwrap()
        .insert(TerminalId::new(7), PaneTransport::Remote(Arc::new(conn)));

    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let releasing = backend.clone();
    std::thread::spawn(move || {
        let outcome =
            releasing.release_terminal_for_warp(TerminalId::new(7), Duration::from_secs(1));
        let _ = done_tx.send(outcome.map(|_| ()).map_err(|e| format!("{e:#}")));
    });

    // Whether the broker "confirmed" is the peer's business, and the peer
    // is gone; what is asserted is that the call came back at all.
    let _ = done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the release deadlocked on the transport table");
    assert!(
        !backend
            .terminal_id_to_transport
            .lock()
            .unwrap()
            .contains_key(&TerminalId::new(7)),
        "a released pane leaves the transport table whether or not the broker confirmed"
    );
}
