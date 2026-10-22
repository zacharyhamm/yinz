use crate::stdin_ansi_parser::SyncOutput;
use crate::terminal_escapes::{write_synchronised, HostWriter, PixelSizeQueries};
use std::collections::VecDeque;
use std::io::{self, ErrorKind, Write};
use std::time::{Duration, Instant};

/// A host terminal that accepts a few bytes at a time and is full in between, as a
/// non-blocking stdout is while the host falls behind.
#[derive(Default)]
struct FullHost {
    written: Vec<u8>,
    full_next: bool,
    write_errors: VecDeque<ErrorKind>,
    flush_errors: VecDeque<ErrorKind>,
}

impl Write for FullHost {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Some(kind) = self.write_errors.pop_front() {
            return Err(kind.into());
        }
        self.full_next = !self.full_next;
        if !self.full_next {
            return Err(ErrorKind::WouldBlock.into());
        }
        let accepted = bytes.len().min(3);
        self.written.extend_from_slice(&bytes[..accepted]);
        Ok(accepted)
    }
    fn flush(&mut self) -> io::Result<()> {
        match self.flush_errors.pop_front() {
            Some(kind) => Err(kind.into()),
            None => Ok(()),
        }
    }
}

/// Retries at once: `FullHost` takes more on the very next write.
fn host_writer<W: Write>(host: W) -> HostWriter<W> {
    HostWriter::new(host, |_| Ok(()))
}

#[test]
fn a_full_host_terminal_delays_a_frame_without_losing_bytes() {
    let mut host = host_writer(FullHost::default());
    write_synchronised(&mut host, None, b"a whole frame").unwrap();
    assert_eq!(host.get_ref().written, b"a whole frame");
}

#[test]
fn a_full_host_terminal_delays_any_write_without_losing_bytes() {
    // Not only frames: nested-session frames, host queries and teardown write
    // straight to stdout, and a truncated escape sequence desynchronises the host.
    let mut host = host_writer(FullHost::default());
    host.write_all(b"\x1b_nested-session-frame\x1b\\").unwrap();
    host.flush().unwrap();
    assert_eq!(host.get_ref().written, b"\x1b_nested-session-frame\x1b\\");
}

#[test]
fn a_broken_host_terminal_is_reported_instead_of_panicking() {
    let mut host = host_writer(FullHost {
        write_errors: [ErrorKind::BrokenPipe].into(),
        ..Default::default()
    });
    let error = write_synchronised(&mut host, None, b"frame").unwrap_err();
    assert_eq!(error.kind(), ErrorKind::BrokenPipe);
}

#[test]
fn synchronized_frames_survive_partial_writes_interrupts_and_a_full_flush() {
    for sync in [SyncOutput::Csi, SyncOutput::Dcs] {
        let mut host = host_writer(FullHost {
            write_errors: [ErrorKind::Interrupted].into(),
            flush_errors: [ErrorKind::Interrupted, ErrorKind::WouldBlock].into(),
            ..Default::default()
        });
        write_synchronised(&mut host, Some(sync), b"whole-frame").unwrap();
        let expected = [sync.start_seq(), b"whole-frame", sync.end_seq()].concat();
        assert_eq!(host.get_ref().written, expected);
        assert!(host.get_ref().flush_errors.is_empty());
    }
}

#[test]
fn a_terminal_that_breaks_during_flush_reports_the_error() {
    let mut host = host_writer(FullHost {
        flush_errors: [ErrorKind::WouldBlock, ErrorKind::BrokenPipe].into(),
        ..Default::default()
    });
    let error = write_synchronised(&mut host, None, b"frame").unwrap_err();
    assert_eq!(error.kind(), ErrorKind::BrokenPipe);
    assert_eq!(host.get_ref().written, b"frame");
}

#[test]
fn readiness_errors_reach_both_write_and_flush_callers() {
    for kind in [ErrorKind::BrokenPipe, ErrorKind::PermissionDenied] {
        // Use the host's error field to inject the poll failure; write/flush still
        // return WouldBlock so each operation must consult the wait callback.
        struct UnreadyHost(ErrorKind);
        impl Write for UnreadyHost {
            fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
                Err(ErrorKind::WouldBlock.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(ErrorKind::WouldBlock.into())
            }
        }
        let mut host = HostWriter::new(UnreadyHost(kind), |host| Err(host.0.into()));
        assert_eq!(host.write(b"frame").unwrap_err().kind(), kind);
        assert_eq!(host.flush().unwrap_err().kind(), kind);
    }
}

#[test]
fn interrupted_readiness_polls_retry_without_losing_partial_output() {
    let mut host = HostWriter::new(FullHost::default(), |_| Err(ErrorKind::Interrupted.into()));
    write_synchronised(&mut host, None, b"whole-frame").unwrap();
    assert_eq!(host.get_ref().written, b"whole-frame");
}

/// Takes whatever it is given, counting the calls it took to get it.
#[derive(Default)]
struct VectoredHost {
    written: Vec<u8>,
    writes: usize,
}

impl Write for VectoredHost {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.writes += 1;
        self.written.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        self.writes += 1;
        for buf in bufs {
            self.written.extend_from_slice(buf);
        }
        Ok(bufs.iter().map(|buf| buf.len()).sum())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_host_with_room_takes_a_synchronized_frame_in_one_write() {
    use super::write_synchronised_tracking;
    use std::sync::atomic::{AtomicU8, Ordering};
    for sync in [None, Some(SyncOutput::Csi), Some(SyncOutput::Dcs)] {
        let open = AtomicU8::new(0);
        let mut host = host_writer(VectoredHost::default());
        write_synchronised_tracking(&mut host, sync, b"whole-frame", &open).unwrap();
        let expected = [
            sync.map_or(&[][..], |sync| sync.start_seq()),
            b"whole-frame",
            sync.map_or(&[][..], |sync| sync.end_seq()),
        ]
        .concat();
        assert_eq!(host.get_ref().written, expected);
        assert_eq!(host.get_ref().writes, 1);
        assert_eq!(open.load(Ordering::SeqCst), 0);
    }
}

/// Fail once after a byte prefix, so a start marker, payload or end marker can be
/// only partly written. Cleanup may fail separately with a different error.
#[derive(Default)]
struct PartialFrameHost {
    prefix: usize,
    written: Vec<u8>,
    failed: bool,
    cleanup_fails: bool,
    attempts: Vec<Vec<u8>>,
    flushed: bool,
}

impl Write for PartialFrameHost {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.attempts.push(bytes.to_vec());
        if !self.failed && self.written.len() == self.prefix {
            self.failed = true;
            return Err(ErrorKind::PermissionDenied.into());
        }
        if self.failed && self.cleanup_fails {
            return Err(ErrorKind::BrokenPipe.into());
        }
        let accepted = if self.failed {
            bytes.len()
        } else {
            bytes.len().min(self.prefix - self.written.len())
        };
        self.written.extend_from_slice(&bytes[..accepted]);
        Ok(accepted)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flushed = true;
        if self.cleanup_fails {
            Err(ErrorKind::BrokenPipe.into())
        } else {
            Ok(())
        }
    }
}

#[test]
fn partial_synchronized_frames_attempt_end_and_flush_without_hiding_the_error() {
    use super::write_synchronised_tracking;
    use std::sync::atomic::AtomicU8;
    let frame = b"frame";
    for sync in [SyncOutput::Csi, SyncOutput::Dcs] {
        for prefix in [
            1,
            sync.start_seq().len() + 1,
            sync.start_seq().len() + frame.len() + 1,
        ] {
            for cleanup_fails in [false, true] {
                let mut host = PartialFrameHost {
                    prefix,
                    cleanup_fails,
                    ..Default::default()
                };
                // Local frame state: a failed cleanup leaves it open, and the process-wide copy
                // would leak that into every later teardown test in this process.
                let open = AtomicU8::new(0);
                let error =
                    write_synchronised_tracking(&mut host, Some(sync), frame, &open).unwrap_err();
                assert_eq!(error.kind(), ErrorKind::PermissionDenied);
                assert_eq!(
                    host.attempts
                        .iter()
                        .filter(|attempt| attempt.as_slice() == sync.end_seq())
                        .count(),
                    1,
                    "HostWriter handles transient errors; a failed end marker is not retried"
                );
                assert!(host.flushed);
                if !cleanup_fails && prefix < sync.start_seq().len() + frame.len() {
                    assert!(host.written.ends_with(sync.end_seq()));
                }
            }
        }
    }
}

/// Refuses every write, however often it is told the host is ready.
#[derive(Default)]
struct RefusingHost {
    attempts: usize,
}

impl Write for RefusingHost {
    fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
        self.attempts += 1;
        Err(ErrorKind::WouldBlock.into())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_host_that_refuses_ready_output_is_retried_with_backoff_not_a_spin() {
    let limit = Duration::from_millis(200);
    let mut host =
        HostWriter::with_stall_policy(RefusingHost::default(), |_| Ok(()), || true, limit);
    let started = Instant::now();
    let error = host.write_all(b"frame").unwrap_err();
    assert_eq!(error.kind(), ErrorKind::TimedOut);
    assert!(started.elapsed() >= limit);
    // Backoff of 1, 2, 4, 8, 16 then 25 ms makes about a dozen attempts in 200 ms;
    // a spin would make millions.
    let attempts = host.get_ref().attempts;
    assert!(attempts < 50, "{attempts} attempts in {limit:?}");
}

#[test]
fn stalls_have_no_deadline_until_output_is_abandoned() {
    // A zero limit, but nothing has abandoned output: every stall is waited out.
    let mut host =
        HostWriter::with_stall_policy(FullHost::default(), |_| Ok(()), || false, Duration::ZERO);
    write_synchronised(&mut host, Some(SyncOutput::Csi), b"whole-frame").unwrap();
    let expected = [
        SyncOutput::Csi.start_seq(),
        b"whole-frame",
        SyncOutput::Csi.end_seq(),
    ]
    .concat();
    assert_eq!(host.get_ref().written, expected);
}

/// Takes one byte at a time, and only every `interval`.
struct SlowHost {
    interval: Duration,
    next: Instant,
    written: Vec<u8>,
}

impl Write for SlowHost {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if Instant::now() < self.next {
            return Err(ErrorKind::WouldBlock.into());
        }
        self.next = Instant::now() + self.interval;
        self.written.push(bytes[0]);
        Ok(1)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn an_abandoned_host_that_is_still_draining_gets_the_whole_frame() {
    // Each stall is shorter than the limit, though the frame as a whole takes longer.
    let limit = Duration::from_millis(100);
    let interval = Duration::from_millis(30);
    let host = SlowHost {
        interval,
        next: Instant::now(),
        written: Vec::new(),
    };
    let mut host = HostWriter::with_stall_policy(host, |_| Ok(()), || true, limit);
    let started = Instant::now();
    host.write_all(b"slow frame").unwrap();
    assert!(started.elapsed() > limit);
    assert_eq!(host.get_ref().written, b"slow frame");
}

#[test]
fn pixel_size_queries_never_wait_for_the_terminal_and_coalesce() {
    use std::sync::{mpsc, Arc, Mutex};
    use yinz_test_support::{default_timeout, Watched};

    /// Blocks its first write until the test opens the gate.
    struct GatedStdout {
        gate: Arc<Mutex<mpsc::Receiver<()>>>,
        entered: mpsc::Sender<()>,
        out: Watched<Vec<u8>>,
    }
    impl Write for GatedStdout {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let _ = self.entered.send(());
            self.gate.lock().unwrap().recv().unwrap();
            self.out.lock().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let (open_gate, gate) = mpsc::channel();
    let gate = Arc::new(Mutex::new(gate));
    let (entered, writing) = mpsc::channel();
    let out = Watched::<Vec<u8>>::default();
    let (queries, worker) = PixelSizeQueries::spawn({
        let out = out.clone();
        move || -> Box<dyn Write> {
            Box::new(GatedStdout {
                gate: gate.clone(),
                entered: entered.clone(),
                out: out.clone(),
            })
        }
    })
    .unwrap();

    queries.request();
    writing.recv_timeout(Duration::from_secs(10)).unwrap();
    // The terminal is stuck on the first query; more resizes must not wait for it.
    let started = Instant::now();
    for _ in 0..10 {
        queries.request();
    }
    assert!(started.elapsed() < Duration::from_secs(1));

    for _ in 0..2 {
        open_gate.send(()).unwrap();
    }
    let query = crate::stdin_handler::PIXEL_SIZE_QUERY.as_bytes();
    out.wait_until("the queued query to be written", default_timeout(), |out| {
        (out.len() >= 2 * query.len()).then_some(())
    });
    // The ten requests made while the first was stuck came to one more query.
    assert_eq!(out.get(), [query, query].concat());
    assert!(writing.recv_timeout(Duration::from_millis(1)).is_ok());
    drop(queries);
    worker.join().unwrap().unwrap();
    assert!(writing.try_recv().is_err());
}

#[test]
fn dcs_cleanup_is_only_retained_after_an_unclosed_dcs_start() {
    use super::{write_synchronised_tracking, ClientTeardown};
    use std::sync::atomic::{AtomicU8, Ordering};
    for sync in [SyncOutput::Dcs, SyncOutput::Csi] {
        for prefix in [0, 1, sync.start_seq().len() + 1] {
            for cleanup_fails in [false, true] {
                let open = AtomicU8::new(0);
                let mut host = PartialFrameHost {
                    prefix,
                    cleanup_fails,
                    ..Default::default()
                };
                let _ = write_synchronised_tracking(&mut host, Some(sync), b"frame", &open);
                let ribbon = ClientTeardown::at_startup(false)
                    .restore_sequences_with_sync(open.load(Ordering::SeqCst));
                assert_eq!(
                    ribbon.contains("\x1bP"),
                    matches!(sync, SyncOutput::Dcs) && prefix > 0 && cleanup_fails
                );
                if prefix == 0 {
                    assert!(!host.written.windows(2).any(|w| w == b"\x1bP"));
                }
            }
        }
    }
}

#[test]
fn an_unclosed_dcs_survives_later_frames_until_its_own_end_is_flushed() {
    use super::{write_synchronised_tracking, ClientTeardown};
    use std::sync::atomic::{AtomicU8, Ordering};

    for next_sync in [None, Some(SyncOutput::Csi), Some(SyncOutput::Dcs)] {
        for prefix in [0, 1, usize::MAX] {
            let open = AtomicU8::new(0);
            let mut broken = PartialFrameHost {
                prefix: 1,
                cleanup_fails: true,
                ..Default::default()
            };
            assert!(write_synchronised_tracking(
                &mut broken,
                Some(SyncOutput::Dcs),
                b"frame",
                &open
            )
            .is_err());
            assert_eq!(open.load(Ordering::SeqCst), 2);

            let mut next = PartialFrameHost {
                prefix,
                ..Default::default()
            };
            let _ = write_synchronised_tracking(&mut next, next_sync, b"next", &open);
            let ribbon = ClientTeardown::at_startup(false)
                .restore_sequences_with_sync(open.load(Ordering::SeqCst));
            assert_eq!(
                ribbon.contains("\x1bP=2s"),
                !matches!(next_sync, Some(SyncOutput::Dcs))
            );
        }
    }
}

#[test]
fn startup_and_panic_restoration_restore_input_between_flushed_output_phases() {
    use super::{ClientTeardown, END_SYNCHRONIZED_OUTPUT};
    use crate::os_input_output::DISABLE_MOUSE_SUPPORT;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::atomic::AtomicU8;
    struct RecordingHost {
        host: FullHost,
        events: Rc<RefCell<Vec<String>>>,
        broken: bool,
    }
    impl Write for RecordingHost {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.broken {
                return Err(ErrorKind::BrokenPipe.into());
            }
            let n = self.host.write(bytes)?;
            self.events
                .borrow_mut()
                .push(String::from_utf8_lossy(&bytes[..n]).into_owned());
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            if self.broken {
                return Err(ErrorKind::BrokenPipe.into());
            }
            self.host.flush()?;
            self.events.borrow_mut().push("<flush>".into());
            Ok(())
        }
    }
    for message in [None, Some("startup failed")] {
        for broken in [false, true] {
            let events = Rc::new(RefCell::new(Vec::new()));
            let mut host = HostWriter::new(
                RecordingHost {
                    host: FullHost {
                        write_errors: [ErrorKind::Interrupted].into(),
                        flush_errors: [ErrorKind::Interrupted, ErrorKind::WouldBlock].into(),
                        ..Default::default()
                    },
                    events: events.clone(),
                    broken,
                },
                |_| Ok(()),
            );
            ClientTeardown::at_startup(true).restore_with_sync(
                &mut host,
                || {
                    events.borrow_mut().push("<input>".into());
                },
                message,
                &AtomicU8::new(0),
            );
            let events = events.borrow().concat();
            if broken {
                assert_eq!(events, "<input>");
            } else {
                // The frame closes before mouse reports stop, and both land before raw mode is off.
                assert!(
                    events.starts_with(&format!(
                        "{END_SYNCHRONIZED_OUTPUT}{DISABLE_MOUSE_SUPPORT}<flush><input>"
                    )),
                    "{events:?}"
                );
                assert_eq!(
                    events.matches(DISABLE_MOUSE_SUPPORT).count(),
                    1,
                    "{events:?}"
                );
                assert!(events.ends_with("<flush>"));
                assert!(!events.contains("\x1bP"));
                if let Some(message) = message {
                    assert!(events.contains(message));
                }
            }
        }
    }
}
