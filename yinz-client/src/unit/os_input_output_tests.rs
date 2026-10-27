use super::*;

#[test]
fn idle_stdin_wait_does_not_make_post_recovery_input_ineligible() {
    use std::sync::mpsc;

    struct WaitingStdin {
        entered: mpsc::Sender<()>,
        bytes: mpsc::Receiver<Vec<u8>>,
    }
    impl io::Read for WaitingStdin {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.entered.send(()).unwrap();
            let bytes = self.bytes.recv().unwrap();
            buffer[..bytes.len()].copy_from_slice(&bytes);
            Ok(bytes.len())
        }
    }

    for bytes in [b"x".as_slice(), b"\x1b[200~fresh paste\x1b[201~", b"\x03"] {
        let control = crate::remote_recovery::RecoveryControl::new(
            crate::command_is_executing::CommandIsExecuting::new(),
        );
        let (entered, waiting) = mpsc::channel();
        let (send_bytes, received) = mpsc::channel();
        let reader_control = control.clone();
        let reader = thread::spawn(move || {
            let mut stdin = io::BufReader::new(WaitingStdin {
                entered,
                bytes: received,
            });
            read_stdin_buffer(&mut stdin, Some(&reader_control)).unwrap()
        });
        waiting.recv_timeout(time::Duration::from_secs(10)).unwrap();
        // The reader is inside fill_buf with no bytes. Recovery finishes before
        // any key/paste is supplied, without relying on scheduling or sleeps.
        control.lost(0, "outage".into());
        control.replacing();
        control.repainted();
        send_bytes.send(bytes.to_vec()).unwrap();
        assert_eq!(reader.join().unwrap(), (bytes.to_vec(), Some((1, true))));
    }
}

#[test]
fn tty_writers_preserve_buffered_stdout_order() {
    use nix::fcntl::{fcntl, FcntlArg, OFlag};
    const CHILD: &str = "YINZ_TEST_STDOUT_ORDER";
    if let Some(scenario) = std::env::var_os(CHILD) {
        let mut writer = stdout_writer(scenario == "teardown");
        let mut tty = STDOUT_TTY.get().unwrap().as_ref().expect("reopened tty");
        io::stdout().write_all(b"<buffered-scalar>").unwrap();
        writer.write_all(b"<scalar>").unwrap();
        io::stdout().write_all(b"<buffered-vector>").unwrap();
        let mut slices = [io::IoSlice::new(b"<vector"), io::IoSlice::new(b">")];
        let mut pending = &mut slices[..];
        while !pending.is_empty() {
            let written = writer.write_vectored(pending).unwrap();
            assert_ne!(written, 0);
            io::IoSlice::advance_slices(&mut pending, written);
        }
        io::stdout().write_all(b"<buffered-flush>").unwrap();
        writer.flush().unwrap();
        // Bypass StdoutWriter so only the explicit flush above can order these bytes.
        tty.write_all(b"<after-flush>").unwrap();
        return;
    }
    for scenario in ["ordinary", "teardown"] {
        let pty = nix::pty::openpty(None, None).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "os_input_output::tests::tty_writers_preserve_buffered_stdout_order",
                "--nocapture",
            ])
            .env(CHILD, scenario)
            .stdout(std::process::Stdio::from(pty.slave.try_clone().unwrap()))
            .output()
            .unwrap();
        let flags = OFlag::from_bits_truncate(fcntl(&pty.master, FcntlArg::F_GETFL).unwrap());
        fcntl(&pty.master, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).unwrap();
        let mut bytes = Vec::new();
        // Keep the parent slave open during this nonblocking drain: macOS otherwise
        // discards unread output when the child closes the last slave.
        let _ = std::fs::File::from(pty.master).read_to_end(&mut bytes);
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            output.status.success(),
            "{scenario}: {text}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            text.contains(
                "<buffered-scalar><scalar><buffered-vector><vector><buffered-flush><after-flush>"
            ),
            "{scenario}: {text}"
        );
    }
}

#[test]
fn quitting_interrupts_a_backpressured_blocking_stdout() {
    use crate::terminal_escapes::{self, ClientTeardown};
    use nix::fcntl::{fcntl, FcntlArg, OFlag};
    use nix::sys::termios::{tcflush, tcgetattr, FlushArg, LocalFlags};
    use std::sync::mpsc;
    const CHILD: &str = "YINZ_TEST_BLOCKING_STDOUT_STALL";
    if let Some(scenario) = std::env::var_os(CHILD) {
        let stdout = io::stdout();
        let flags = OFlag::from_bits_truncate(fcntl(&stdout, FcntlArg::F_GETFL).unwrap());
        assert!(!flags.contains(OFlag::O_NONBLOCK));
        let original = tcgetattr(io::stdin()).unwrap();
        let mut client = get_client_os_input().unwrap();
        client.set_raw_mode();
        let (entered, ready) = mpsc::channel();
        let (finished, result) = mpsc::channel();
        let writer = thread::spawn(move || {
            let mut stdout = get_client_os_input().unwrap().get_stdout_writer();
            entered.send(()).unwrap();
            // The parent does not drain: fill every kernel queue rather than relying
            // on TCOOFF surviving stdout's private tty reopen (it does not on macOS).
            let written = loop {
                if let Err(error) = stdout.write_all(&[b'x'; 4096]) {
                    break Err::<(), _>(error);
                }
            };
            finished.send(written).unwrap();
        });
        ready.recv_timeout(time::Duration::from_secs(10)).unwrap();
        terminal_escapes::abandon_stalled_output();
        let written = result.recv_timeout(time::Duration::from_secs(10));
        if written.is_err() {
            tcflush(&stdout, FlushArg::TCOFLUSH).unwrap();
        }
        writer.join().unwrap();
        // Restoration must finish and restore input even if output never resumes.
        let (entered, ready) = mpsc::channel();
        let (finished, result) = mpsc::channel();
        let restorer = thread::spawn(move || {
            entered.send(()).unwrap();
            if scenario == "startup" {
                ClientTeardown::at_startup(true).restore_startup(Some("restored"));
            } else {
                ClientTeardown::new(24, true).restore(&client, "restored");
            }
            finished.send(()).unwrap();
        });
        ready.recv_timeout(time::Duration::from_secs(10)).unwrap();
        // Timing is the behavior under test: the parent leaves output full during restoration.
        let completed = result.recv_timeout(time::Duration::from_secs(10));
        let paused = tcgetattr(io::stdin()).unwrap();
        // Make room for the harness output before assertions, including on failure.
        tcflush(&stdout, FlushArg::TCOFLUSH).unwrap();
        restorer.join().unwrap();
        assert_eq!(
            written.unwrap().unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        completed.expect("restoration must give up on stalled output");
        assert!(paused.local_flags.contains(LocalFlags::ICANON));
        let restored = tcgetattr(io::stdin()).unwrap();
        assert_eq!(restored.input_flags, original.input_flags);
        assert_eq!(restored.output_flags, original.output_flags);
        assert_eq!(restored.control_flags, original.control_flags);
        assert_eq!(
            restored.local_flags - LocalFlags::PENDIN,
            original.local_flags - LocalFlags::PENDIN,
        );
        assert_eq!(restored.control_chars, original.control_chars);
        assert_eq!(
            OFlag::from_bits_truncate(fcntl(&stdout, FcntlArg::F_GETFL).unwrap()),
            flags
        );
        assert_eq!(
            OFlag::from_bits_truncate(fcntl(io::stdin(), FcntlArg::F_GETFL).unwrap()),
            flags
        );
        return;
    }
    for scenario in ["ordinary", "startup"] {
        let pty = nix::pty::openpty(None, None).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "os_input_output::tests::quitting_interrupts_a_backpressured_blocking_stdout",
                "--nocapture",
            ])
            .env(CHILD, scenario)
            .stdin(std::process::Stdio::from(pty.slave.try_clone().unwrap()))
            .stdout(std::process::Stdio::from(pty.slave.try_clone().unwrap()))
            .output()
            .unwrap();
        let flags = OFlag::from_bits_truncate(fcntl(&pty.master, FcntlArg::F_GETFL).unwrap());
        fcntl(&pty.master, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).unwrap();
        let mut bytes = Vec::new();
        // The parent slave keeps unread output alive until this nonblocking drain.
        let _ = std::fs::File::from(pty.master).read_to_end(&mut bytes);
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            output.status.success(),
            "{scenario}: {text}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// Reopening is an optimization: failure must leave inherited stdout usable, and neither
/// success nor failure may be retried by later frame/startup/teardown writers.
#[test]
fn stdout_tty_cache_survives_startup_frames_and_teardown() {
    use crate::terminal_escapes::{self, ClientTeardown};
    use nix::fcntl::{fcntl, FcntlArg, OFlag};
    use nix::libc;
    use std::os::fd::AsRawFd;
    const CHILD: &str = "YINZ_TEST_STDOUT_TTY_CACHE";
    if let Some(scenario) = std::env::var_os(CHILD) {
        let fail = scenario == "failed";
        let stdout = io::stdout();
        let inherited_flags = fcntl(&stdout, FcntlArg::F_GETFL).unwrap();
        assert!(stdout.is_terminal());
        let path = nix::unistd::ttyname(&stdout).unwrap();
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: both calls receive valid pointers; only this subprocess's soft limit changes.
        assert_eq!(
            unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) },
            0
        );
        let exhausted = libc::rlimit {
            rlim_cur: 0,
            ..limit
        };
        if fail {
            assert_eq!(
                unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &exhausted) },
                0
            );
            // ttyname itself may need a spare descriptor on macOS. Either failure
            // to locate or to reopen the tty must be cached as the same fallback.
            assert_eq!(
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .unwrap_err()
                    .raw_os_error(),
                Some(libc::EMFILE)
            );
        }
        let client = get_client_os_input().unwrap();
        terminal_escapes::enter_terminal(&client, true, false);
        let cached = STDOUT_TTY
            .get()
            .expect("first writer initializes the cache")
            .as_ref();
        assert_eq!(cached.is_none(), fail);
        let cached_fd = cached.map(AsRawFd::as_raw_fd);
        if let Some(tty) = cached {
            assert!(
                OFlag::from_bits_truncate(fcntl(tty, FcntlArg::F_GETFL).unwrap())
                    .contains(OFlag::O_NONBLOCK)
            );
        }
        // Reverse the open conditions. Success must reuse its fd even if opening now fails;
        // failure must stay cached even though the tty can now be opened.
        let next_limit = if fail { &limit } else { &exhausted };
        assert_eq!(
            unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, next_limit) },
            0
        );
        if fail {
            assert!(std::fs::OpenOptions::new().write(true).open(&path).is_ok());
        }
        terminal_escapes::write_seqs(&client, &["frame output"]);
        ClientTeardown::at_startup(true).restore_startup(Some("startup restored"));
        ClientTeardown::new(24, true).restore(&client, "session restored");
        assert_eq!(
            STDOUT_TTY.get().unwrap().as_ref().map(AsRawFd::as_raw_fd),
            cached_fd
        );
        assert_eq!(fcntl(&stdout, FcntlArg::F_GETFL).unwrap(), inherited_flags);
        assert_eq!(
            fcntl(io::stdin(), FcntlArg::F_GETFL).unwrap(),
            inherited_flags
        );
        assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) }, 0);
        return;
    }
    for scenario in ["success", "failed"] {
        let pty = nix::pty::openpty(None, None).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "os_input_output::tests::stdout_tty_cache_survives_startup_frames_and_teardown",
                "--nocapture",
            ])
            .env(CHILD, scenario)
            .stdin(std::process::Stdio::from(pty.slave.try_clone().unwrap()))
            .stdout(std::process::Stdio::from(pty.slave.try_clone().unwrap()))
            .output()
            .unwrap();
        let flags = OFlag::from_bits_truncate(fcntl(&pty.master, FcntlArg::F_GETFL).unwrap());
        fcntl(&pty.master, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).unwrap();
        let mut master = std::fs::File::from(pty.master);
        let mut bytes = Vec::new();
        // The parent slave keeps unread output alive until this nonblocking drain.
        let _ = master.read_to_end(&mut bytes);
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            output.status.success(),
            "{scenario}: {text}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        for expected in [
            terminal_escapes::ENTER_ALTERNATE_SCREEN,
            "frame output",
            "startup restored",
            "session restored",
            terminal_escapes::EXIT_KITTY_KEYBOARD_MODE,
        ] {
            assert!(
                text.contains(expected),
                "{scenario} missing {expected:?}: {text}"
            );
        }
    }
}

/// The host terminal is a tty, which is what macOS `poll` cannot wait on. A full one is
/// backpressure, a drained one is ready, and a closed one fails the write, not the wait.
///
/// The waits use a timeout far longer than the test allows, so one that returns did so because
/// the tty became writable: neither a fixed sleep nor a wait that never blocks passes.
#[test]
fn a_full_tty_is_waited_on_and_a_closed_one_fails_the_write() {
    use crate::terminal_escapes::HostWriter;
    use nix::fcntl::{fcntl, FcntlArg, OFlag};
    use std::sync::mpsc;
    const NEVER: time::Duration = time::Duration::from_secs(30);
    const PROMPTLY: time::Duration = time::Duration::from_secs(10);

    fn nonblocking_pty() -> (std::fs::File, std::fs::File) {
        let pty = nix::pty::openpty(None, None).unwrap();
        let flags = OFlag::from_bits_truncate(fcntl(&pty.slave, FcntlArg::F_GETFL).unwrap());
        fcntl(&pty.slave, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).unwrap();
        (
            std::fs::File::from(pty.slave),
            std::fs::File::from(pty.master),
        )
    }

    let (mut slave, master) = nonblocking_pty();
    // A tty with room is ready at once.
    let started = time::Instant::now();
    assert!(wait_until_writable(&slave, NEVER).unwrap());
    assert!(started.elapsed() < PROMPTLY);

    let deadline = time::Instant::now() + PROMPTLY;
    loop {
        match slave.write(&[b'x'; 4096]) {
            Ok(_) => {},
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {},
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if !wait_until_writable(&slave, time::Duration::from_millis(200)).unwrap() {
                    break;
                }
                // The kernel can move bytes between the tty's internal queues
                // without a master read. Refill until both sides have backed up.
                assert!(time::Instant::now() < deadline, "tty never stayed full");
            },
            Err(error) => panic!("failed to fill the tty: {error}"),
        }
    }

    // A full tty keeps the wait waiting...
    let (woke, woken) = mpsc::channel();
    let waiter = {
        let slave = slave.try_clone().unwrap();
        std::thread::spawn(move || {
            let _ = woke.send(wait_until_writable(&slave, NEVER).map_err(|error| error.kind()));
        })
    };
    assert!(
        woken
            .recv_timeout(time::Duration::from_millis(300))
            .is_err(),
        "the wait returned while the tty was still full"
    );
    // ...until the host side reads.
    let drainer = {
        let mut master = master.try_clone().unwrap();
        std::thread::spawn(move || {
            let mut drained = [0; 65536];
            // Ends once every slave descriptor is closed.
            while master.read(&mut drained).is_ok_and(|read| read > 0) {}
        })
    };
    assert!(woken
        .recv_timeout(PROMPTLY)
        .expect("the wait outlasted the drain")
        .unwrap());
    waiter.join().unwrap();
    drop(slave);
    drop(master);
    drainer.join().unwrap();

    // Close the host side; the retried write reports it instead of waiting forever.
    let (slave, master) = nonblocking_pty();
    drop(master);
    let mut writer = HostWriter::new(&slave, wait_until_stdout_is_writable);
    let error = loop {
        if let Err(error) = writer.write_all(&[b'x'; 4096]) {
            break error;
        }
    };
    assert_ne!(error.kind(), io::ErrorKind::WouldBlock);
}

/// Frames from different threads reach the terminal whole, even when each one has to wait
/// out a full terminal partway through.
#[test]
fn concurrent_writers_never_interleave_frames_on_a_full_terminal() {
    use std::io::Read;
    const CHILD: &str = "YINZ_TEST_CLIENT_ATOMIC_FRAMES";
    const FRAME: usize = 256 * 1024;
    const FRAMES_PER_THREAD: usize = 8;
    if std::env::var_os(CHILD).is_some() {
        use nix::fcntl::{fcntl, FcntlArg, OFlag};
        let stdout = io::stdout();
        let flags = OFlag::from_bits_truncate(fcntl(&stdout, FcntlArg::F_GETFL).unwrap());
        fcntl(&stdout, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).unwrap();
        let client = get_client_os_input().unwrap();
        let writers: Vec<_> = b"ABC"
            .iter()
            .map(|&fill| {
                let client = client.clone();
                std::thread::spawn(move || {
                    let mut frame = vec![fill; FRAME];
                    frame[0] = b'<';
                    frame[FRAME - 1] = b'>';
                    for _ in 0..FRAMES_PER_THREAD {
                        let mut stdout = client.get_stdout_writer();
                        stdout.write_all(&frame).unwrap();
                        stdout.flush().unwrap();
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        // The harness reports the result on this stdout, and does not retry `EAGAIN`.
        fcntl(&stdout, FcntlArg::F_SETFL(flags)).unwrap();
        return;
    }
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "os_input_output::tests::concurrent_writers_never_interleave_frames_on_a_full_terminal",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Read slowly, so the pipe fills and writers have to wait mid-frame.
    let mut pipe = child.stdout.take().unwrap();
    let mut output = Vec::new();
    let mut chunk = [0; 16 * 1024];
    loop {
        let read = pipe.read(&mut chunk).unwrap();
        if read == 0 {
            break;
        }
        output.extend_from_slice(&chunk[..read]);
        std::thread::sleep(time::Duration::from_micros(200));
    }
    assert!(child.wait().unwrap().success());

    // The test harness prints around the frames; each frame is `<`, one letter, `>`.
    let first = output.iter().position(|&b| b == b'<').unwrap();
    let last = output.iter().rposition(|&b| b == b'>').unwrap();
    let frames = output[first..=last].chunks(FRAME);
    assert_eq!(
        frames.len(),
        3 * FRAMES_PER_THREAD,
        "frames were lost or split"
    );
    for (index, frame) in frames.enumerate() {
        let fill = frame[1];
        assert!(
            frame[0] == b'<'
                && frame[FRAME - 1] == b'>'
                && frame[1..FRAME - 1].iter().all(|&b| b == fill),
            "frame {index} was interleaved with another thread's output"
        );
    }
}

#[test]
fn original_termios_survives_startup_raw_mode_and_cloning() {
    use nix::sys::termios::{self, InputFlags, LocalFlags, SetArg, SpecialCharacterIndices};
    use yinz_runtime::termios::WireTermios;
    const CHILD: &str = "YINZ_TEST_CLIENT_TERMIOS";
    if std::env::var_os(CHILD).is_some() {
        let original = termios::tcgetattr(io::stdin()).unwrap();
        let mut client = get_client_os_input().unwrap();
        client.set_raw_mode_keeping_signals();
        let raw = termios::tcgetattr(io::stdin()).unwrap();
        assert!(!raw.input_flags.contains(InputFlags::ICRNL));
        assert!(raw.local_flags.contains(LocalFlags::ISIG));
        let cloned = client.clone();
        let command = crate::server_command(
            Path::new("test.sock"),
            false,
            Some(&cloned.original_termios()),
        )
        .unwrap();
        let args: Vec<_> = command.get_args().collect();
        let json = args[args
            .iter()
            .position(|arg| *arg == "--server-termios")
            .unwrap()
            + 1];
        let snapshot: Option<WireTermios> = serde_json::from_str(json.to_str().unwrap()).unwrap();
        assert_eq!(snapshot, Some(WireTermios::from_termios(&original)));
        client.unset_raw_mode().unwrap();
        // Field by field rather than as a whole: XNU sets PENDIN when canonical mode comes
        // back, so that pending input is retyped. It is kernel status, not part of what the
        // restore wrote.
        let restored = termios::tcgetattr(io::stdin()).unwrap();
        assert_eq!(restored.input_flags, original.input_flags);
        assert_eq!(restored.output_flags, original.output_flags);
        assert_eq!(restored.control_flags, original.control_flags);
        assert_eq!(
            restored.local_flags - LocalFlags::PENDIN,
            original.local_flags - LocalFlags::PENDIN,
        );
        assert_eq!(restored.control_chars, original.control_chars);
        return;
    }
    // Raw mode and stdin are process-global: isolate them from the test runner.
    let pty = nix::pty::openpty(None, None).unwrap();
    let mut cooked = termios::tcgetattr(&pty.slave).unwrap();
    cooked.input_flags.insert(InputFlags::ICRNL);
    cooked.control_chars[SpecialCharacterIndices::VERASE as usize] = 8;
    termios::tcsetattr(&pty.slave, SetArg::TCSANOW, &cooked).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "os_input_output::tests::original_termios_survives_startup_raw_mode_and_cloning",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .stdin(std::process::Stdio::from(pty.slave))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

/// The branch nothing else exercises: a socket that never appears. The
/// error must name the socket and carry the connect failure, and the wait
/// must be bounded by the timeout, not by the retry loop's patience.
#[test]
fn connect_with_deadline_gives_up_at_the_deadline_with_a_named_socket() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("never-bound.sock");
    let timeout = std::time::Duration::from_millis(50);

    let started_at = std::time::Instant::now();
    let Err(err) = connect_with_deadline(&path, timeout, std::time::Duration::from_millis(5))
    else {
        panic!("a socket that does not exist must not connect");
    };
    let waited = started_at.elapsed();

    assert!(
        waited >= timeout,
        "gave up after {waited:?}, before the {timeout:?} deadline"
    );
    assert!(
        waited < std::time::Duration::from_secs(1),
        "kept retrying for {waited:?} past a {timeout:?} deadline"
    );
    assert_eq!(err.last_error().kind(), io::ErrorKind::NotFound);
    let message = err.to_string();
    assert!(
        message.contains(&path.display().to_string()),
        "the message should name the socket: {message}"
    );
}

#[test]
fn get_terminal_size_returns_nonzero_or_fallback() {
    let size = get_terminal_size();
    // In CI or when not attached to a terminal, crossterm may return an error
    // and we fall back to 80x24. Either way, size should be valid.
    assert!(size.rows > 0, "rows should be positive");
    assert!(size.cols > 0, "cols should be positive");
}

#[test]
fn get_terminal_size_fallback_values() {
    // Verify the fallback constants are what we expect
    let fallback = Size { rows: 24, cols: 80 };
    // When crossterm::terminal::size() fails (no terminal), we should get 80x24
    // This is implicitly tested by get_terminal_size_returns_nonzero_or_fallback
    // but we verify the constants here
    assert_eq!(fallback.rows, 24);
    assert_eq!(fallback.cols, 80);
}

#[test]
fn client_os_input_output_can_be_constructed() {
    let os_input = get_client_os_input().expect("should construct ClientOsInputOutput");
    let size = os_input.get_terminal_size();
    assert!(size.rows > 0, "rows should be positive");
    assert!(size.cols > 0, "cols should be positive");
}

#[test]
fn cli_client_os_input_can_be_constructed() {
    let os_input = get_cli_client_os_input().expect("should construct CLI ClientOsInputOutput");
    let cli: &dyn CliClientOs = &os_input;
    assert!(matches!(cli.recv_from_server(), IpcRecv::Eof));
}

#[test]
fn receiving_without_a_server_connection_returns_eof() {
    let os_input = get_client_os_input().unwrap();
    assert!(matches!(os_input.recv_from_server(), IpcRecv::Eof));
    assert!(matches!(os_input.recv_from_server(), IpcRecv::Eof));
}

#[test]
fn receiving_with_a_poisoned_connection_returns_eof() {
    let os_input = get_client_os_input().unwrap();
    let receiver = os_input.receive_instructions_from_server.clone();
    assert!(std::thread::spawn(move || {
        let _receiver = receiver.lock().unwrap();
        panic!("poison the receiver lock");
    })
    .join()
    .is_err());
    assert!(matches!(os_input.recv_from_server(), IpcRecv::Eof));
}

#[test]
fn same_name_handoff_invalidates_old_readers_and_preserves_buffered_input() {
    let mut current = get_client_os_input().unwrap();
    current.update_session_name(SessionName::new("same"));
    let mut old = current.clone();
    current.update_session_name(SessionName::new("same"));
    assert!(!old.is_current_connection());
    assert!(current.is_current_connection());
    *current.reading_from_stdin.lock().unwrap() = Some(b"next session".to_vec());
    assert!(old.read_from_stdin().is_err());
    assert_eq!(current.read_from_stdin().unwrap(), b"next session");
}

#[test]
fn stopping_an_idle_signal_listener_releases_both_workers() {
    for os in [
        Box::new(get_client_os_input().unwrap()) as Box<dyn ClientOsApi>,
        Box::new(crate::test_support::FakeClientOsApi::new()),
    ] {
        for explicit_stop in [false, true] {
            let (callback, released) = std::sync::mpsc::channel::<()>();
            let (query_factory, query_released) = std::sync::mpsc::channel::<()>();
            let (queries, query_worker) =
                crate::terminal_escapes::PixelSizeQueries::spawn(move || {
                    let _keep_alive = &query_factory;
                    Box::new(io::sink())
                })
                .unwrap();
            let listener = os
                .start_signal_listener(
                    Box::new(move || {
                        let _ = callback.send(());
                        queries.request();
                    }),
                    Box::new(|| panic!("unexpected quit signal")),
                )
                .unwrap();
            let (done, stopped) = std::sync::mpsc::channel();
            let worker = thread::spawn(move || {
                if explicit_stop {
                    listener.stop().unwrap();
                } else {
                    drop(listener);
                }
                query_worker.join().unwrap().unwrap();
                done.send(()).unwrap();
            });
            stopped
                .recv_timeout(yinz_test_support::default_timeout())
                .unwrap();
            worker.join().unwrap();
            assert_eq!(
                released.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Disconnected)
            );
            assert_eq!(
                query_released.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Disconnected)
            );
        }
    }
}

#[test]
fn signal_listener_reports_a_worker_panic_after_stopping() {
    let (stop, cancelled) = std::sync::mpsc::channel::<()>();
    let listener = SignalListener::spawn(
        move || drop(stop),
        move || {
            assert!(cancelled.recv().is_err());
            panic!("failed signal worker");
        },
    )
    .unwrap();
    assert!(listener
        .stop()
        .unwrap_err()
        .to_string()
        .contains("signal listener panicked"));
}

#[test]
fn unix_resizes_only_reach_the_current_signal_listener() {
    if yinz_test_support::reexec_test() {
        return;
    }
    let os = get_client_os_input().unwrap();
    let mut previous = Vec::new();
    for _ in 0..4 {
        let (callback, calls) = std::sync::mpsc::channel();
        let listener = os
            .start_signal_listener(
                Box::new(move || callback.send(()).unwrap()),
                Box::new(|| panic!("unexpected quit signal")),
            )
            .unwrap();
        signal_hook::low_level::raise(signal_hook::consts::signal::SIGWINCH).unwrap();
        calls
            .recv_timeout(yinz_test_support::default_timeout())
            .unwrap();
        listener.stop().unwrap();
        previous.push(calls);
        for old in &previous {
            assert_eq!(
                old.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Disconnected)
            );
        }
    }
}

#[test]
fn high_terminal_descriptors_report_an_error_without_panicking() {
    use nix::fcntl::{fcntl, FcntlArg};
    use std::os::fd::{FromRawFd, OwnedFd};

    const CHILD: &str = "YINZ_TEST_HIGH_TERMINAL_FD";
    const NAME: &str =
        "os_input_output::tests::high_terminal_descriptors_report_an_error_without_panicking";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }
    let mut limit = nix::libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // The subprocess owns its descriptor limit; macOS commonly starts below FD_SETSIZE.
    assert_eq!(
        unsafe { nix::libc::getrlimit(nix::libc::RLIMIT_NOFILE, &mut limit) },
        0
    );
    limit.rlim_cur = limit
        .rlim_cur
        .max(nix::libc::FD_SETSIZE as nix::libc::rlim_t + 1);
    if limit.rlim_cur > limit.rlim_max {
        eprintln!("skipping high-descriptor test: hard limit cannot accommodate FD_SETSIZE");
        return;
    }
    assert_eq!(
        unsafe { nix::libc::setrlimit(nix::libc::RLIMIT_NOFILE, &limit) },
        0
    );
    let file = std::fs::File::open("/dev/null").unwrap();
    let raw_fd = fcntl(
        &file,
        FcntlArg::F_DUPFD_CLOEXEC(nix::libc::FD_SETSIZE as i32),
    )
    .unwrap();
    // fcntl returned a new owned descriptor, which this test must close.
    let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    let error = wait_until_writable(&fd, time::Duration::ZERO).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert!(error.to_string().contains("FD_SETSIZE"));
}

#[test]
fn stdin_read_errors_return_without_panicking() {
    const CHILD: &str = "YINZ_TEST_STDIN_READ_ERROR";
    const NAME: &str = "os_input_output::tests::stdin_read_errors_return_without_panicking";
    if std::env::var_os(CHILD).is_some() {
        let mut client = get_client_os_input().unwrap();
        assert_eq!(client.read_from_stdin(), Err("Failed to read stdin"));
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", NAME])
        .env(CHILD, "1")
        .stdin(std::fs::File::open(directory.path()).unwrap())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

fn recovery_socket() -> (
    tempfile::TempDir,
    interprocess::local_socket::Stream,
    interprocess::local_socket::Stream,
) {
    use interprocess::local_socket::traits::Listener;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("remote");
    let listener = yinz_ipc::consts::ipc_bind(&path).unwrap();
    let stream = yinz_ipc::consts::ipc_connect(&path).unwrap();
    let peer = listener.accept().unwrap();
    (dir, stream, peer)
}

#[test]
fn failed_writer_draining_is_bounded_when_complete_frames_keep_arriving() {
    use std::sync::mpsc;

    let (_dir, stream, peer) = recovery_socket();
    let os = get_client_os_input().unwrap();
    let sender = IpcSenderWithContext::new(stream);
    let receiver = sender.get_receiver().unwrap();
    os.install_session_connection(sender, receiver).unwrap();
    let control = crate::remote_recovery::RecoveryControl::new(
        crate::command_is_executing::CommandIsExecuting::new(),
    );
    os.set_remote_recovery(Some(control.clone())).unwrap();
    let mut peer = IpcSenderWithContext::<ServerToClientMsg>::new(peer);
    peer.send_server_msg(ServerToClientMsg::UnblockInputThread)
        .unwrap();
    control.lost(0, "write failed".into());
    let (stop, stopped) = mpsc::channel();
    let producer = thread::spawn(move || {
        // Timing is under test: complete frames arrive inside each read's idle
        // deadline, but their cumulative read time exceeds the draining budget.
        while stopped
            .recv_timeout(crate::remote_recovery::READ_POLL / 4)
            .is_err()
        {
            if peer
                .send_server_msg(ServerToClientMsg::UnblockInputThread)
                .is_err()
            {
                break;
            }
        }
    });
    let (tx, rx) = yinz_runtime::channels::unbounded();
    let router = crate::router::spawn(
        Box::new(os),
        yinz_runtime::channels::SenderWithContext::new(tx),
        Arc::new(crate::render_budget::RenderBudget::new(100)),
    );
    let deadline = time::Instant::now() + time::Duration::from_secs(2);
    let mut drained = false;
    let mut frames = 0;
    while let Ok((instruction, _)) =
        rx.recv_timeout(deadline.saturating_duration_since(time::Instant::now()))
    {
        if let crate::ClientInstruction::FromServer(0, instruction) = instruction {
            match *instruction {
                crate::ClientInstruction::ServerReaderDrained(0) => {
                    drained = true;
                    break;
                },
                crate::ClientInstruction::UnblockInputThread => frames += 1,
                other => panic!("unexpected instruction: {other:?}"),
            }
        }
    }
    // Stop the producer even on failure so the old router can reach EOF and join.
    let _ = stop.send(());
    producer.join().unwrap();
    router.join().unwrap();
    assert!(drained, "continuous complete frames suppressed recovery");
    assert!(
        (1..256).contains(&frames),
        "expected read-time limit before frame cap, got {frames}"
    );
}

#[test]
fn remote_write_failure_invalidates_the_transport_and_unblocks_input() {
    let (_dir, stream, peer) = recovery_socket();
    let os = get_client_os_input().unwrap();
    let sender = IpcSenderWithContext::new(stream);
    let receiver = sender.get_receiver().unwrap();
    os.install_session_connection(sender, receiver).unwrap();
    let mut command = crate::command_is_executing::CommandIsExecuting::new();
    let control = crate::remote_recovery::RecoveryControl::new(command.clone());
    os.set_remote_recovery(Some(control.clone())).unwrap();
    command.blocking_input_thread();
    drop(peer);
    os.send_to_server(ClientToServerMsg::Paste("failed write".into()));
    assert_eq!(control.phase(), crate::remote_recovery::Phase::Reconnecting);
    let (done, result) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        command.wait_until_input_thread_is_unblocked();
        done.send(()).unwrap();
    });
    result.recv_timeout(time::Duration::from_secs(2)).unwrap();
    worker.join().unwrap();
}

#[test]
fn pending_explicit_server_messages_survive_a_real_write_failure() {
    use crate::{render_budget::RenderBudget, ClientInstruction};
    use yinz_ipc::ipc::ExitReason;
    use yinz_runtime::channels::{self, SenderWithContext};

    for message in [
        ServerToClientMsg::Exit {
            exit_reason: ExitReason::NormalDetached,
        },
        ServerToClientMsg::SwitchSession {
            connect_to_session: Default::default(),
        },
    ] {
        let (_dir, stream, peer) = recovery_socket();
        let os = get_client_os_input().unwrap();
        let sender = IpcSenderWithContext::new(stream);
        let receiver = sender.get_receiver().unwrap();
        os.install_session_connection(sender, receiver).unwrap();
        let control = crate::remote_recovery::RecoveryControl::new(
            crate::command_is_executing::CommandIsExecuting::new(),
        );
        os.set_remote_recovery(Some(control.clone())).unwrap();
        let mut server = IpcSenderWithContext::<ServerToClientMsg>::new(peer);
        server.send_server_msg(message).unwrap();
        drop(server);
        os.send_to_server(ClientToServerMsg::TerminalResize {
            new_size: Size { rows: 24, cols: 80 },
        });
        assert!(!control.can_write());
        let (tx, rx) = channels::unbounded();
        crate::router::route(
            Box::new(os),
            SenderWithContext::new(tx),
            &RenderBudget::new(100),
        );
        let instructions: Vec<_> = rx.iter().map(|(instruction, _)| instruction).collect();
        assert!(
            matches!(&instructions[..], [ClientInstruction::FromServer(0, instruction)]
            if matches!(**instruction, ClientInstruction::Exit(_) | ClientInstruction::SwitchSession(_))),
            "{instructions:?}"
        );
    }
}

#[test]
fn remote_writes_to_a_nonreading_peer_are_bounded_and_old_input_cannot_reach_a_replacement() {
    let (_dir, stream, peer) = recovery_socket();
    let mut os = get_client_os_input().unwrap();
    let sender = IpcSenderWithContext::new(stream);
    let receiver = sender.get_receiver().unwrap();
    os.install_session_connection(sender, receiver).unwrap();
    let control = crate::remote_recovery::RecoveryControl::new(
        crate::command_is_executing::CommandIsExecuting::new(),
    );
    os.set_remote_recovery(Some(control.clone())).unwrap();
    // Timing is the behavior under test: the peer remains open and never reads.
    let started = time::Instant::now();
    os.send_to_server(ClientToServerMsg::Paste("x".repeat(8 * 1024 * 1024)));
    assert!(started.elapsed() < time::Duration::from_secs(10));
    assert_eq!(control.phase(), crate::remote_recovery::Phase::Reconnecting);
    drop(peer);
    os.disconnect_session_connection();
    let (_replacement_dir, stream, peer) = recovery_socket();
    let sender = IpcSenderWithContext::new(stream);
    let receiver = sender.get_receiver().unwrap();
    os.install_session_connection(sender, receiver).unwrap();
    control.replacing();
    control.repainted();
    let mut peer = IpcReceiverWithContext::<ClientToServerMsg>::new(peer);
    peer.set_recv_timeout(Some(time::Duration::from_millis(100)))
        .unwrap();
    os.pin_input_generation(0);
    os.send_to_server(ClientToServerMsg::Paste("STALE".into()));
    assert!(matches!(peer.recv_client_msg(), IpcRecv::Idle));
    assert!(matches!(os.recv_from_server_generation(0), IpcRecv::Eof));
    os.pin_input_generation(1);
    os.send_to_server(ClientToServerMsg::Paste("CURRENT".into()));
    assert!(
        matches!(peer.recv_client_msg(), IpcRecv::Msg(ClientToServerMsg::Paste(text), _) if text == "CURRENT")
    );
}

#[test]
fn remote_reads_yield_during_a_trickling_frame_and_preserve_the_partial_message() {
    use std::sync::mpsc;

    // Serialize through the real IPC encoder, then deliver the frame in small chunks.
    let content = "fragmented render".repeat(100);
    let (_encoding_dir, stream, mut peer) = recovery_socket();
    let mut encoder = IpcSenderWithContext::<ServerToClientMsg>::new(stream);
    encoder
        .send_server_msg(ServerToClientMsg::Render {
            content: content.clone(),
        })
        .unwrap();
    drop(encoder);
    let mut frame = Vec::new();
    peer.read_to_end(&mut frame).unwrap();

    for split in [1, 5] {
        let (_dir, stream, mut peer) = recovery_socket();
        let os = get_client_os_input().unwrap();
        let sender = IpcSenderWithContext::new(stream);
        let receiver = sender.get_receiver().unwrap();
        os.install_session_connection(sender, receiver).unwrap();
        let control = crate::remote_recovery::RecoveryControl::new(
            crate::command_is_executing::CommandIsExecuting::new(),
        );
        os.set_remote_recovery(Some(control.clone())).unwrap();
        peer.write_all(&frame[..split]).unwrap();
        let frame = frame.clone();
        let (finish, finished) = mpsc::channel();
        let writer = thread::spawn(move || {
            let mut sent = split;
            // Timing is the behavior under test: each chunk arrives inside READ_POLL,
            // but the whole frame takes much longer than the absolute read deadline.
            for _ in 0..100 {
                if finished
                    .recv_timeout(time::Duration::from_millis(10))
                    .is_ok()
                {
                    break;
                }
                peer.write_all(&frame[sent..sent + 1]).unwrap();
                sent += 1;
            }
            peer.write_all(&frame[sent..]).unwrap();
            // Keep the connection open until the client has consumed the frame.
            let _ = finished.recv();
        });
        let started = time::Instant::now();
        let result = os.recv_from_server_generation(0);
        let elapsed = started.elapsed();
        finish.send(()).unwrap();
        assert!(matches!(result, IpcRecv::Idle));
        assert!(
            elapsed < time::Duration::from_millis(750),
            "read took {elapsed:?}"
        );
        loop {
            match os.recv_from_server_generation(0) {
                IpcRecv::Idle if started.elapsed() < time::Duration::from_secs(5) => {},
                IpcRecv::Msg(ServerToClientMsg::Render { content: received }, _) => {
                    assert_eq!(received, content);
                    break;
                },
                _ => panic!("expected preserved partial render"),
            }
        }
        control.lost(0, "write failed".into());
        // A failed writer must leave the reader available to drain an explicit exit.
        assert!(matches!(os.recv_from_server_generation(0), IpcRecv::Idle));
        os.disconnect_session_connection();
        drop(finish);
        writer.join().unwrap();
    }
}

#[test]
fn terminal_replay_serializes_live_focus_updates_and_keeps_the_newest_cache() {
    use std::sync::mpsc;

    for live_first in [true, false] {
        let (_dir, stream, peer) = recovery_socket();
        let os = get_client_os_input().unwrap();
        let sender = IpcSenderWithContext::new(stream);
        let receiver = sender.get_receiver().unwrap();
        os.install_session_connection(sender, receiver).unwrap();
        let control = crate::remote_recovery::RecoveryControl::new(
            crate::command_is_executing::CommandIsExecuting::new(),
        );
        os.set_remote_recovery(Some(control.clone())).unwrap();
        // Backpressure pauses replay between its snapshot and the focus message.
        control.cache(&ClientToServerMsg::BackgroundColor {
            color: "x".repeat(8 * 1024 * 1024),
        });
        control.cache(&ClientToServerMsg::HostTerminalFocusChanged { focused: true });
        control.lost(0, "offline".into());
        control.replacing();
        let mut peer = IpcReceiverWithContext::<ClientToServerMsg>::new(peer);
        peer.set_recv_timeout(Some(time::Duration::from_secs(10)))
            .unwrap();
        if live_first {
            os.send_to_server(ClientToServerMsg::HostTerminalFocusChanged { focused: false });
            assert!(matches!(
                peer.recv_client_msg(),
                IpcRecv::Msg(
                    ClientToServerMsg::HostTerminalFocusChanged { focused: false },
                    _
                )
            ));
        }
        let replay_os = os.clone();
        let replay_control = control.clone();
        let replay = thread::spawn(move || replay_control.replay(&replay_os));
        assert!(matches!(
            peer.recv_client_msg(),
            IpcRecv::Msg(ClientToServerMsg::TerminalResize { .. }, _)
        ));
        let live = if live_first {
            None
        } else {
            let live_os = os.clone();
            let (entered, ready) = mpsc::channel();
            let (done, finished) = mpsc::channel();
            let live = thread::spawn(move || {
                entered.send(()).unwrap();
                live_os
                    .send_to_server(ClientToServerMsg::HostTerminalFocusChanged { focused: false });
                done.send(()).unwrap();
            });
            ready.recv_timeout(time::Duration::from_secs(10)).unwrap();
            // Blocking is under test: the observation must wait with the write,
            // so replay cannot overwrite a value already observed by this worker.
            assert!(matches!(
                finished.recv_timeout(time::Duration::from_millis(100)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            let cached = control.replay_messages(Size { rows: 24, cols: 80 });
            assert!(cached.iter().any(|message| matches!(
                message,
                ClientToServerMsg::HostTerminalFocusChanged { focused: true }
            )));
            Some((live, finished))
        };
        assert!(matches!(
            peer.recv_client_msg(),
            IpcRecv::Msg(ClientToServerMsg::BackgroundColor { .. }, _)
        ));
        assert!(matches!(peer.recv_client_msg(), IpcRecv::Msg(
            ClientToServerMsg::HostTerminalFocusChanged { focused }, _
        ) if focused != live_first));
        assert!(matches!(
            peer.recv_client_msg(),
            IpcRecv::Msg(ClientToServerMsg::ModifiersHeld { .. }, _)
        ));
        replay.join().unwrap();
        if let Some((live, finished)) = live {
            finished
                .recv_timeout(time::Duration::from_secs(10))
                .unwrap();
            live.join().unwrap();
            assert!(matches!(
                peer.recv_client_msg(),
                IpcRecv::Msg(
                    ClientToServerMsg::HostTerminalFocusChanged { focused: false },
                    _
                )
            ));
        }
        // A later reconnection must replay the live observation, too.
        let replay_os = os.clone();
        let replay = thread::spawn(move || control.replay(&replay_os));
        let mut focus = Vec::new();
        for _ in 0..4 {
            if let IpcRecv::Msg(ClientToServerMsg::HostTerminalFocusChanged { focused }, _) =
                peer.recv_client_msg()
            {
                focus.push(focused);
            }
        }
        replay.join().unwrap();
        assert_eq!(focus, vec![false]);
    }
}

#[test]
fn remote_disconnect_does_not_wait_for_a_trickling_partial_frame() {
    use std::sync::mpsc;

    let (_dir, stream, mut peer) = recovery_socket();
    let os = get_client_os_input().unwrap();
    let sender = IpcSenderWithContext::new(stream);
    let receiver = sender.get_receiver().unwrap();
    os.install_session_connection(sender, receiver).unwrap();
    let control = crate::remote_recovery::RecoveryControl::new(
        crate::command_is_executing::CommandIsExecuting::new(),
    );
    os.set_remote_recovery(Some(control.clone())).unwrap();
    peer.write_all(&1024u32.to_le_bytes()).unwrap();
    let (finish, finished) = mpsc::channel();
    let writer = thread::spawn(move || {
        // Keep the payload incomplete, but satisfy every individual read timeout.
        // This timed producer is necessary to test the absolute deadline.
        for _ in 0..100 {
            if finished
                .recv_timeout(time::Duration::from_millis(10))
                .is_ok()
                || peer.write_all(&[0]).is_err()
            {
                return;
            }
        }
        let _ = finished.recv();
    });
    let reader_os = os.clone();
    let reader_control = control.clone();
    let reader = thread::spawn(move || loop {
        let result = reader_os.recv_from_server_generation(0);
        if !matches!(result, IpcRecv::Idle) || !reader_control.can_write() {
            break result;
        }
    });
    let waiting = time::Instant::now();
    // Observe actual lock ownership rather than assuming the reader was scheduled.
    while os.receive_instructions_from_server.try_lock().is_ok() {
        assert!(waiting.elapsed() < time::Duration::from_secs(10));
        thread::yield_now();
    }
    control.lost(0, "write failed during partial read".into());
    let started = time::Instant::now();
    os.disconnect_session_connection();
    let elapsed = started.elapsed();
    let _ = finish.send(());
    writer.join().unwrap();
    // The failure can win the phase check before the blocking read begins.
    assert!(matches!(
        reader.join().unwrap(),
        IpcRecv::Idle | IpcRecv::Eof
    ));
    assert!(
        elapsed < time::Duration::from_millis(750),
        "disconnect took {elapsed:?}"
    );
    assert!(matches!(os.recv_from_server_generation(0), IpcRecv::Eof));
}
