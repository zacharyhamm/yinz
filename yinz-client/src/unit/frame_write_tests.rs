use crate::terminal_escapes::{write_synchronised, HostWriter};
use std::io::{self, ErrorKind, Write};

/// A host terminal that accepts a few bytes at a time and is full in between, as a
/// non-blocking stdout is while the host falls behind.
struct FullHost {
    written: Vec<u8>,
    full_next: bool,
    error: Option<ErrorKind>,
}

impl Write for FullHost {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Some(kind) = self.error {
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
        Ok(())
    }
}

/// Retries at once: `FullHost` takes more on the very next write.
fn host_writer(host: FullHost) -> HostWriter<FullHost> {
    HostWriter::new(host, |_| {})
}

#[test]
fn a_full_host_terminal_delays_a_frame_without_losing_bytes() {
    let mut host = host_writer(FullHost {
        written: vec![],
        full_next: false,
        error: None,
    });
    write_synchronised(&mut host, None, b"a whole frame").unwrap();
    assert_eq!(host.get_ref().written, b"a whole frame");
}

#[test]
fn a_full_host_terminal_delays_any_write_without_losing_bytes() {
    // Not only frames: nested-session frames, host queries and teardown write
    // straight to stdout, and a truncated escape sequence desynchronises the host.
    let mut host = host_writer(FullHost {
        written: vec![],
        full_next: false,
        error: None,
    });
    host.write_all(b"\x1b_nested-session-frame\x1b\\").unwrap();
    host.flush().unwrap();
    assert_eq!(host.get_ref().written, b"\x1b_nested-session-frame\x1b\\");
}

#[test]
fn a_broken_host_terminal_is_reported_instead_of_panicking() {
    let mut host = host_writer(FullHost {
        written: vec![],
        full_next: false,
        error: Some(ErrorKind::BrokenPipe),
    });
    let error = write_synchronised(&mut host, None, b"frame").unwrap_err();
    assert_eq!(error.kind(), ErrorKind::BrokenPipe);
}
