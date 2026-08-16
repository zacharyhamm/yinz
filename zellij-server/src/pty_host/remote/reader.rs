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

use super::conn::{RemoteConn, RemoteGitProbe, RemoteStatus};
use super::protocol::{self, BrokerToServerMsg, Frame};
use crate::os_input_output::AsyncReader;

pub use async_trait::async_trait;

/// How many times to try before admitting the pane is gone.
const RECONNECT_ATTEMPTS: u32 = 8;
const RECONNECT_INITIAL_DELAY: std::time::Duration = std::time::Duration::from_millis(250);
const RECONNECT_MAX_DELAY: std::time::Duration = std::time::Duration::from_secs(10);

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
    /// How many bytes at the front of `decoded` this side made up rather than
    /// received.
    ///
    /// They must not count towards `received`, or the broker would replay from
    /// the wrong place and silently skip real output. Injection only happens
    /// while `decoded` is empty — the read loop hands back buffered output
    /// before it ever touches the socket — so the synthetic bytes are always a
    /// contiguous prefix.
    synthetic_prefix: usize,
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
            synthetic_prefix: 0,
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
    ///
    /// This is also where delivery is counted. It has to be here rather than at
    /// the socket: the broker replays from what we *took*, and anything sitting
    /// in `decoded` or `inbuf` when the transport dies was never handed to the
    /// grid, so it still has to come again.
    fn drain_into(&mut self, buf: &mut [u8]) -> usize {
        let n = self.decoded.len().min(buf.len());
        for (slot, byte) in buf.iter_mut().zip(self.decoded.drain(..n)) {
            *slot = byte;
        }
        let synthetic = self.synthetic_prefix.min(n);
        self.synthetic_prefix -= synthetic;
        self.conn.note_received(n - synthetic);
        n
    }

    /// Put bytes of our own into the stream, excluded from delivery accounting.
    fn inject(&mut self, bytes: &[u8]) {
        self.decoded.extend(bytes);
        self.synthetic_prefix += bytes.len();
    }

    /// Get the pane's connection back, blocking this reader until it is up.
    ///
    /// Parking here rather than returning an error is the whole mechanism:
    /// `TerminalBytes::listen` has exactly two exits, `Ok(0)` and `Err`, and
    /// both tear the pane down and kill the child. For a local pty that is
    /// right — the master closing *is* the child going away. Over a network it
    /// is not, and a pane whose process is running perfectly well on another
    /// machine must not be closed because a packet went missing.
    async fn reconnect(&mut self) -> Result<(), io::Error> {
        self.conn.mark_transport_lost();
        if !self.conn.can_reconnect() {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                format!(
                    "lost the connection to {} while its pane was still running, and this pane \
                     has no way to reconnect",
                    self.conn.host()
                ),
            ));
        }
        // Whatever was mid-flight belongs to a socket that no longer exists; a
        // partial frame would resume mid-length and read payload as a header.
        self.inbuf.clear();
        self.async_fd = None;
        self.pending = None;
        self.announce_disconnected();

        let mut delay = RECONNECT_INITIAL_DELAY;
        for attempt in 1..=RECONNECT_ATTEMPTS {
            tokio::time::sleep(delay).await;
            // `reconnect` runs ssh, so it must not run on the async runtime.
            let conn = self.conn.clone();
            let result = tokio::task::spawn_blocking(move || conn.reconnect())
                .await
                .map_err(|e| io::Error::other(format!("reconnect task failed: {e}")))?;

            match result {
                Ok((socket, dropped)) => {
                    socket.set_nonblocking(true)?;
                    self.pending = Some(socket);
                    self.on_reconnected(dropped);
                    return Ok(());
                },
                Err(e) => {
                    log::warn!(
                        "reconnect to {} failed (attempt {attempt}/{RECONNECT_ATTEMPTS}): {e:#}",
                        self.conn.host()
                    );
                    delay = (delay * 2).min(RECONNECT_MAX_DELAY);
                },
            }
        }

        // Out of attempts. Now — and only now — is this pane genuinely over, and
        // the error is the honest report of why.
        Err(io::Error::new(
            io::ErrorKind::ConnectionReset,
            format!(
                "gave up reconnecting to {} after {RECONNECT_ATTEMPTS} attempts",
                self.conn.host()
            ),
        ))
    }

    /// Tell the user, in the pane itself.
    ///
    /// There is no "pane disconnected" render path in this codebase, and adding
    /// one is a Screen/Tab/Pane change out of proportion to the need. Synthetic
    /// bytes go through the ordinary output path, so nothing else has to know.
    fn announce_disconnected(&mut self) {
        let notice = format!(
            "\r\n\x1b[33m── disconnected from {} — reconnecting ──\x1b[0m\r\n",
            self.conn.host()
        );
        self.inject(notice.as_bytes());
    }

    fn on_reconnected(&mut self, dropped: u64) {
        if dropped == 0 {
            let notice = "\x1b[32m── reconnected ──\x1b[0m\r\n";
            self.inject(notice.as_bytes());
            return;
        }
        // A replay with a hole cannot simply be continued: splicing a torn
        // middle into the grid produces a corrupt screen that looks like real
        // output. Clear it, say so, and make the application repaint.
        let notice = format!(
            "\x1b[2J\x1b[H\x1b[33m── reconnected; {dropped} bytes of output were lost ──\x1b[0m\r\n"
        );
        self.inject(notice.as_bytes());
        self.conn.nudge_for_redraw();
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
            BrokerToServerMsg::GitProbe {
                cwd,
                exit_code,
                metadata_base64,
                config_base64,
            } => {
                use crate::pty_host::protocol::decode_buffered;
                let metadata = decode_buffered(&metadata_base64).unwrap_or_default();
                let config = decode_buffered(&config_base64).unwrap_or_default();
                self.conn.set_git_probe(RemoteGitProbe {
                    host: self.conn.host().to_owned(),
                    cwd,
                    exit_code,
                    metadata,
                    config,
                });
                false
            },
            BrokerToServerMsg::StatusReport {
                cwd,
                shell_argv,
                foreground_argv,
            } => {
                self.conn.set_status(RemoteStatus {
                    cwd,
                    shell_argv,
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
                    log::warn!("remote pane read failed ({e}); trying to reconnect");
                    self.reconnect().await?;
                    continue;
                },
                Err(_would_block) => continue,
            };

            if n == 0 {
                // The socket hit EOF. What that means depends entirely on
                // whether the broker told us why.
                match self.conn.child_exit() {
                    Some(_) => {
                        self.finished = true;
                        return Ok(0);
                    },
                    None => {
                        // The pane is still running on the other machine; only
                        // the path to it is gone. Go and get it back rather than
                        // reporting an end that has not happened.
                        self.reconnect().await?;
                        continue;
                    },
                }
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
