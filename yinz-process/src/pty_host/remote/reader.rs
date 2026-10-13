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
use std::sync::{Arc, Mutex};
use yinz_types::internal_ids::TerminalId;

use tokio::io::unix::AsyncFd;
use yinz_types::input::command::RunCommand;

use super::conn::{RemoteConn, RemoteGitProbe, RemoteStatus};
use super::protocol::{self, BrokerToServerMsg, Frame};
use crate::pty_host::client::{AgentReportCb, QuitCb};
use crate::AsyncReader;

pub use async_trait::async_trait;

/// How many times to try before admitting the pane is gone.
const RECONNECT_ATTEMPTS: u32 = 8;
const RECONNECT_INITIAL_DELAY: std::time::Duration = std::time::Duration::from_millis(250);
const RECONNECT_MAX_DELAY: std::time::Duration = std::time::Duration::from_secs(10);

/// Overrides `RECONNECT_ATTEMPTS`.
///
/// A test seam. Walking the full ladder to exhaustion takes upwards of a minute,
/// nearly all of it spent asleep, which is too long to pay for a test that only
/// wants to see the giving-up.
pub const RECONNECT_ATTEMPTS_ENV: &str = "YINZ_REMOTE_RECONNECT_ATTEMPTS";

fn reconnect_attempts() -> u32 {
    match std::env::var(RECONNECT_ATTEMPTS_ENV) {
        Ok(attempts) => match attempts.trim().parse::<u32>() {
            Ok(attempts) if attempts > 0 => attempts,
            _ => {
                log::warn!(
                    "ignoring {RECONNECT_ATTEMPTS_ENV}={attempts:?}: expected a positive count"
                );
                RECONNECT_ATTEMPTS
            },
        },
        Err(_) => RECONNECT_ATTEMPTS,
    }
}

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
    /// while `decoded` is empty: the read loop hands back buffered output before touching
    /// the socket, and gap notices wait for preceding output to drain. Synthetic bytes are
    /// therefore always a contiguous prefix.
    synthetic_prefix: usize,
    /// What to tell the pty thread when the pane ends, and what it needs to
    /// hear: `HoldPane` or `ClosePane`, and `CommandPaneExited` for a plugin
    /// that is waiting on this pane.
    ///
    /// A `Mutex` because `AsyncReader` is `Sync` but a `QuitCb` is not, and
    /// because taking the callback out of the `Option` is what makes it fire
    /// exactly once.
    on_exit: Mutex<Option<QuitCb>>,
    /// Passed back to the callback, which reports it to the plugin that asked
    /// for the pane and uses it to re-run a held one.
    cmd: RunCommand,
    terminal_id: TerminalId,
    /// Where a coding agent's hook reports from this pane go. One closure for
    /// every pane, registered on the backend; `None` on a reader nobody asked.
    agent_report_cb: Option<AgentReportCb>,
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
            on_exit: Mutex::new(None),
            cmd: RunCommand::default(),
            terminal_id: TerminalId::new(0),
            agent_report_cb: None,
        })
    }

    /// Say where a coding agent's hook reports from this pane go.
    pub fn on_agent_report(mut self, cb: AgentReportCb) -> Self {
        self.agent_report_cb = Some(cb);
        self
    }

    /// Say what to do when the pane ends.
    ///
    /// Without this a remote pane never closes itself: `TerminalBytes::listen`
    /// only breaks its loop, and the instruction that actually retires a pane —
    /// `ClosePane`, or `HoldPane` — comes from nowhere else.
    pub fn on_exit(mut self, terminal_id: TerminalId, cmd: RunCommand, quit_cb: QuitCb) -> Self {
        self.on_exit = Mutex::new(Some(quit_cb));
        self.cmd = cmd;
        self.terminal_id = terminal_id;
        self
    }

    /// Tell the pty thread the pane is over. Taking the callback out is what
    /// keeps it to once.
    fn report_exit(&self, status: Option<i32>) {
        let cb = match self.on_exit.lock() {
            Ok(mut slot) => slot.take(),
            Err(_) => None,
        };
        if let Some(cb) = cb {
            cb(self.terminal_id, status, self.cmd.clone());
        }
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
                    self.conn.display_host()
                ),
            ));
        }
        // Whatever was mid-flight belongs to a socket that no longer exists; a
        // partial frame would resume mid-length and read payload as a header.
        self.inbuf.clear();
        self.async_fd = None;
        self.pending = None;
        self.announce_disconnected();

        let attempts = reconnect_attempts();
        let mut delay = RECONNECT_INITIAL_DELAY;
        for attempt in 1..=attempts {
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
                        "reconnect to {} failed (attempt {attempt}/{attempts}): {e:#}",
                        self.conn.display_host()
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
                "gave up reconnecting to {} after {attempts} attempts",
                self.conn.display_host()
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
            self.conn.display_host()
        );
        self.inject(notice.as_bytes());
    }

    fn on_reconnected(&mut self, dropped: u64) {
        if dropped == 0 {
            let notice = "\x1b[32m── reconnected ──\x1b[0m\r\n";
            self.inject(notice.as_bytes());
            // A resize during the outage never reached the broker — the send
            // failed on a broken connection and the queues were cleared behind
            // it — so the pane on the far machine is still the shape it was
            // before. Nothing else will say so: the size only goes out when it
            // changes, and from here it never changes again.
            self.conn.resend_size();
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
    ///
    /// Keeps going after the exit rather than returning on it. The broker sends
    /// the output before the exit, but the two can land in one read, and
    /// stopping at the exit would drop whatever shared the buffer with it.
    fn consume_frames(&mut self) -> io::Result<bool> {
        let mut ended = false;
        loop {
            let decoded = protocol::try_decode_frame(&self.inbuf)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{e:#}")))?;
            let (frame, consumed) = match decoded {
                Some(pair) => pair,
                None => return Ok(ended),
            };
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
                    if matches!(msg, BrokerToServerMsg::OutputGap { .. })
                        && !self.decoded.is_empty()
                    {
                        // Preserve both delivery order and the contiguous synthetic prefix.
                        // Leave this frame buffered until all preceding bytes are handed out.
                        return Ok(ended);
                    }
                    if self.handle_control(msg) {
                        ended = true;
                    }
                },
            }
            self.inbuf.drain(..consumed);
        }
    }

    /// Returns true if this message ends the pane.
    fn handle_control(&mut self, msg: BrokerToServerMsg) -> bool {
        match msg {
            BrokerToServerMsg::OutputGap { dropped } => {
                if dropped > 0 {
                    self.conn.restore_received(self.conn.received(), dropped);
                    self.on_reconnected(dropped);
                }
                false
            },
            BrokerToServerMsg::ChildExited { status } => {
                self.conn.set_child_exit(status);
                true
            },
            BrokerToServerMsg::GitProbe {
                refresh_token,
                cwd,
                exit_code,
                metadata_base64,
                config_base64,
            } => {
                use crate::pty_host::protocol::decode_buffered;
                let metadata = decode_buffered(&metadata_base64).unwrap_or_default();
                let config = decode_buffered(&config_base64).unwrap_or_default();
                self.conn.set_git_probe(RemoteGitProbe {
                    refresh_token,
                    host: self.conn.host().to_owned(),
                    cwd,
                    exit_code,
                    metadata,
                    config,
                });
                false
            },
            BrokerToServerMsg::StatusReport {
                child_group_terminated,
                cwd,
                shell_argv,
                foreground_argv,
            } => {
                self.conn.set_status(RemoteStatus {
                    child_group_terminated,
                    cwd,
                    shell_argv,
                    foreground_argv,
                });
                child_group_terminated && self.conn.child_exit().is_some()
            },
            // An event, not state: handed on at once rather than cached, or a
            // burst of them would collapse to whichever came last.
            BrokerToServerMsg::AgentReport { kind, json } => {
                match &self.agent_report_cb {
                    Some(cb) => cb(self.terminal_id, kind, json),
                    None => log::warn!(
                        "pane {} reported an agent hook event but nothing is listening",
                        self.terminal_id
                    ),
                }
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
        let result = self.read_inner(buf).await;
        // Both of `TerminalBytes::listen`'s exits, not just the clean one. A
        // reader that gave up reconnecting, or hit a protocol error, has ended
        // the pane just as surely — and leaving it unreported is what strands a
        // dead pane on screen. `child_exit` is `None` in those cases, which is
        // exactly what a supervisor dying reports on the local path.
        if matches!(result, Ok(0) | Err(_)) {
            self.report_exit(self.conn.child_exit().flatten());
        }
        result
    }
}

impl RemoteReader {
    async fn read_inner(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        loop {
            // Always hand back buffered output first, even after the child has
            // exited: its last words matter as much as any others.
            if !self.decoded.is_empty() {
                return Ok(self.drain_into(buf));
            }
            if self.consume_frames()? {
                self.finished = true;
            }
            if !self.decoded.is_empty() {
                continue;
            }
            // Keep receiving group status and accepting escalation after wrapper exit.
            if let Some(status) = self.conn.child_exit() {
                self.report_exit(status);
            }
            if self.finished {
                return Ok(0);
            }

            let conn = self.conn.clone();
            let has_pending = conn.has_pending_writes();
            let async_fd = self.get_async_fd()?;
            let mut guard = tokio::select! {
                ready = async_fd.readable() => ready?,
                ready = async_fd.writable(), if has_pending => {
                    let mut ready = ready?;
                    match conn.flush_pending() {
                        Ok(true) => ready.clear_ready(),
                        Ok(false) => {},
                        Err(error) => {
                            drop(ready);
                            log::warn!("remote pane write failed ({error:#}); trying to reconnect");
                            self.reconnect().await?;
                        },
                    }
                    continue;
                },
                _ = conn.outbound_ready.notified() => continue,
            };
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
                    _ => {
                        // The pane is still running on the other machine; only
                        // the path to it is gone. Go and get it back rather than
                        // reporting an end that has not happened.
                        self.reconnect().await?;
                        continue;
                    },
                }
            }

            self.inbuf.extend_from_slice(&chunk[..n]);
        }
    }
}

#[cfg(test)]
#[path = "../../unit/pty_host_remote_reader_tests.rs"]
mod pty_host_remote_reader_tests;
