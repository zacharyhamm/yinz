//! The read half of a remote pane, as an `AsyncReader`.
//!
//! `TerminalBytes` sees only `Box<dyn AsyncReader>`, so a remote pane needs no
//! changes anywhere downstream of here — the grid, the parser and the renderer
//! cannot tell the difference. What this type owes them is one rule:
//!
//! **`Ok(0)` means the child exited, and nothing else may produce it.**
//!
//! `TerminalBytes::listen` has exactly two exits, `Ok(0)` and `Err`, and both
//! unwind to `close_pane`, which kills the child and tears the pane down. For a
//! local pty that is right: the master closing *is* the child going away. Over a
//! network it is not — a dropped connection would close a pane whose process is
//! still running perfectly well on the other machine. So a transport failure is
//! reported distinctly and never as EOF.

use std::collections::VecDeque;
use std::io;
use std::os::unix::net::UnixStream;
use std::sync::Arc;

use tokio::io::unix::AsyncFd;

use super::conn::{RemoteConn, RemoteStatus};
use super::protocol::{self, BrokerToServerMsg, Frame};
use crate::os_input_output::AsyncReader;

pub use async_trait::async_trait;

pub struct RemoteReader {
    conn: Arc<RemoteConn>,
    /// Holds the socket before reactor registration; `None` after promotion.
    /// Deferred for the same reason `RawFdAsyncReader` defers it: this is built
    /// on the pty thread, outside the Tokio runtime.
    pending: Option<UnixStream>,
    async_fd: Option<AsyncFd<UnixStream>>,
    /// Bytes received but not yet handed to the caller, because its buffer was
    /// smaller than the frame we decoded.
    decoded: VecDeque<u8>,
    /// A partially-received frame. Frames are split across reads routinely.
    inbuf: Vec<u8>,
    /// Set once `ChildExited` has been seen and the decoded backlog is drained.
    finished: bool,
}

impl RemoteReader {
    pub fn new(conn: Arc<RemoteConn>, socket: UnixStream) -> io::Result<Self> {
        socket.set_nonblocking(true)?;
        Ok(Self {
            conn,
            pending: Some(socket),
            async_fd: None,
            decoded: VecDeque::new(),
            inbuf: Vec::new(),
            finished: false,
        })
    }

    fn get_async_fd(&mut self) -> io::Result<&mut AsyncFd<UnixStream>> {
        if self.async_fd.is_none() {
            let socket = self
                .pending
                .take()
                .expect("RemoteReader used after transport loss");
            self.async_fd = Some(AsyncFd::new(socket)?);
        }
        Ok(self.async_fd.as_mut().unwrap())
    }

    /// Move as much as will fit from the decoded backlog into `buf`.
    fn drain_into(&mut self, buf: &mut [u8]) -> usize {
        let n = self.decoded.len().min(buf.len());
        for (slot, byte) in buf.iter_mut().zip(self.decoded.drain(..n)) {
            *slot = byte;
        }
        n
    }

    /// Pull whole frames out of `inbuf`, routing each by kind. Returns whether
    /// the child's exit has now been reported.
    fn consume_frames(&mut self) -> io::Result<bool> {
        loop {
            let decoded = protocol::try_decode_frame(&self.inbuf)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{e:#}")))?;
            let (frame, consumed) = match decoded {
                Some(pair) => pair,
                None => return Ok(false),
            };
            self.inbuf.drain(..consumed);
            match frame {
                Frame::Output(bytes) => self.decoded.extend(bytes),
                // Only the broker sends on this connection, and it never sends
                // input. Treat it as a protocol violation rather than guessing.
                Frame::Input(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "a broker sent an input frame on a remote pane's connection",
                    ))
                },
                Frame::Control(_) => {
                    let msg: BrokerToServerMsg = frame.decode_control().map_err(|e| {
                        io::Error::new(io::ErrorKind::InvalidData, format!("{e:#}"))
                    })?;
                    if self.handle_control(msg) {
                        return Ok(true);
                    }
                },
            }
        }
    }

    /// Returns true if this message ends the pane.
    fn handle_control(&mut self, msg: BrokerToServerMsg) -> bool {
        match msg {
            BrokerToServerMsg::ChildExited { status } => {
                self.conn.set_child_exit(status);
                true
            },
            BrokerToServerMsg::StatusReport {
                cwd,
                foreground_argv,
            } => {
                self.conn.set_status(RemoteStatus {
                    cwd,
                    foreground_argv,
                });
                false
            },
            // Handshake and spawn results are consumed before the pane is handed
            // a reader; seeing one here means the broker is confused, but it is
            // not worth killing a working pane over.
            other => {
                log::warn!("unexpected control message on a live remote pane: {other:?}");
                false
            },
        }
    }
}

#[async_trait]
impl AsyncReader for RemoteReader {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        loop {
            // Always hand back buffered output first, even after the child has
            // exited: its last words matter as much as any others.
            if !self.decoded.is_empty() {
                return Ok(self.drain_into(buf));
            }
            if self.finished {
                return Ok(0);
            }

            let async_fd = self.get_async_fd()?;
            let mut guard = async_fd.readable().await?;
            let mut chunk = [0u8; 65536];
            let read = guard.try_io(|inner| {
                use std::io::Read;
                inner.get_ref().read(&mut chunk)
            });
            let n = match read {
                Ok(Ok(n)) => n,
                Ok(Err(e)) => {
                    self.conn.mark_transport_lost();
                    return Err(e);
                },
                Err(_would_block) => continue,
            };

            if n == 0 {
                // The socket hit EOF. What that means depends entirely on
                // whether the broker told us why.
                return match self.conn.child_exit() {
                    Some(_) => {
                        self.finished = true;
                        Ok(0)
                    },
                    None => {
                        self.conn.mark_transport_lost();
                        Err(io::Error::new(
                            io::ErrorKind::ConnectionReset,
                            format!(
                                "lost the connection to {} while its pane was still running",
                                self.conn.host()
                            ),
                        ))
                    },
                };
            }

            self.inbuf.extend_from_slice(&chunk[..n]);
            if self.consume_frames()? {
                self.finished = true;
            }
        }
    }
}

#[cfg(test)]
#[path = "../../unit/pty_host_remote_reader_tests.rs"]
mod pty_host_remote_reader_tests;
