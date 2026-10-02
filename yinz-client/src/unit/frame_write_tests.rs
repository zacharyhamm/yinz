use crate::stdin_ansi_parser::SyncOutput;
use crate::terminal_escapes::{write_synchronised, HostWriter};
use std::collections::VecDeque;
use std::io::{self, ErrorKind, Write};

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
fn host_writer(host: FullHost) -> HostWriter<FullHost> {
    HostWriter::new(host, |_| {})
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
