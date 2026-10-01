use crate::terminal_escapes::write_synchronised;
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

#[test]
fn a_full_host_terminal_delays_a_frame_without_losing_bytes() {
    let mut host = FullHost {
        written: vec![],
        full_next: false,
        error: None,
    };
    write_synchronised(&mut host, None, b"a whole frame").unwrap();
    assert_eq!(host.written, b"a whole frame");
}

#[test]
fn a_broken_host_terminal_is_reported_instead_of_panicking() {
    let mut host = FullHost {
        written: vec![],
        full_next: false,
        error: Some(ErrorKind::BrokenPipe),
    };
    let error = write_synchronised(&mut host, None, b"frame").unwrap_err();
    assert_eq!(error.kind(), ErrorKind::BrokenPipe);
}
