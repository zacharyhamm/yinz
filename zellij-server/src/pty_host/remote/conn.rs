//! The local end of a remote pane: everything the server does to a pty that is
//! not reading it.
//!
//! Reading lives in `reader.rs`, because it has to be async to fit
//! `AsyncReader`. Everything here is called from the pty and pty-writer threads
//! and so is deliberately synchronous and non-blocking.

use std::collections::VecDeque;
use std::io::{self, Write};
use std::os::unix::net::UnixStream;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

use zellij_utils::errors::prelude::*;

use super::protocol::{self, ChildSignal, ServerToBrokerMsg, FRAME_HEADER_LEN, MAX_STREAM_PAYLOAD};
use crate::pty_host::protocol::WireWinsize;

/// How many bytes of un-sent input we will hold before telling the caller to
/// slow down.
///
/// This is the whole backpressure mechanism, and it is deliberately small. A
/// local pane pushes back via the pty's own buffer: `try_write_to_fd` returns a
/// short count on `EAGAIN` and `pty_writer` re-queues the remainder. A socket
/// gives no such signal — the kernel accepts into the ssh channel buffer long
/// after the remote pty has stopped draining — so the queue has to supply it.
/// When this fills, `write_input` short-counts and `pty_writer` retries within
/// `PENDING_DRAIN_TIMEOUT`, exactly as it does for a busy local pane.
const OUTBOUND_CAPACITY: usize = 256 * 1024;

/// What the broker last told us about the pane, from its own machine.
///
/// Cached rather than fetched, because the alternative is a request/response
/// round trip over a link that may be dead, from `update_and_report_cwds` on the
/// pty thread. A stale-but-plausible answer is the better failure mode.
#[derive(Debug, Clone, Default)]
pub struct RemoteStatus {
    pub cwd: Option<String>,
    pub foreground_argv: Option<Vec<String>>,
}

/// Two queues rather than one, because control messages must overtake queued
/// input.
///
/// For a local pane a resize is a `TIOCSWINSZ` ioctl and a signal is `kill(2)`;
/// neither goes anywhere near the pty write queue, so neither can be delayed by
/// a pane that is slow to read. Putting control frames behind a backlog of input
/// on a single queue would quietly break that: a resize issued while a remote
/// app was refusing to read would not land until the backlog drained, which
/// could be never.
///
/// Control jumps the queue only at frame boundaries — cutting into a
/// half-flushed input frame would corrupt the stream — which `offset` and
/// `writing_input` between them enforce.
struct Outbound {
    control: VecDeque<Vec<u8>>,
    input: VecDeque<Vec<u8>>,
    /// Bytes of the current front frame already written to the socket.
    offset: usize,
    /// Whether `offset` refers to the front of `input` rather than `control`.
    writing_input: bool,
    queued_input_bytes: usize,
    socket: UnixStream,
    /// Set once the socket has failed; further writes are dropped rather than
    /// retried forever.
    broken: bool,
}

/// A live connection to one pane's broker.
pub struct RemoteConn {
    outbound: Mutex<Outbound>,
    status: Mutex<RemoteStatus>,
    /// `Some(status)` once the broker has reported the child exiting. This is
    /// the *only* thing that means the pane is over — a transport failure must
    /// never be read that way.
    child_exit: Mutex<Option<Option<i32>>>,
    transport_lost: AtomicBool,
    /// A label for diagnostics: which host this pane lives on.
    host: String,
}

impl RemoteConn {
    pub fn new(socket: UnixStream, host: String) -> Result<Self> {
        socket
            .set_nonblocking(true)
            .context("failed to make a remote pane's socket non-blocking")?;
        Ok(Self {
            outbound: Mutex::new(Outbound {
                control: VecDeque::new(),
                input: VecDeque::new(),
                offset: 0,
                writing_input: false,
                queued_input_bytes: 0,
                socket,
                broken: false,
            }),
            status: Mutex::new(RemoteStatus::default()),
            child_exit: Mutex::new(None),
            transport_lost: AtomicBool::new(false),
            host,
        })
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    /// Queue pty input, returning how many bytes were accepted.
    ///
    /// Mirrors `try_write_to_fd`'s contract exactly, including `Ok(0)` meaning
    /// "not now, ask again" rather than "written". `pty_writer` already knows how
    /// to hold the remainder and retry, so nothing there has to change.
    pub fn write_input(&self, buf: &[u8]) -> Result<usize> {
        let mut out = self
            .outbound
            .lock()
            .to_anyhow()
            .context("failed to lock a remote pane's outbound queue")?;
        if out.broken {
            return Err(anyhow!(
                "the connection to {} was lost, so this pane cannot be written to",
                self.host
            ));
        }
        // Drain first: a queue that fully flushed last tick should not report
        // back-pressure this tick just because nothing has emptied it since.
        Self::flush_locked(&mut out);

        let free = OUTBOUND_CAPACITY.saturating_sub(out.queued_input_bytes);
        let take = buf.len().min(MAX_STREAM_PAYLOAD).min(free);
        if take == 0 {
            return Ok(0);
        }
        let mut framed: Vec<u8> = Vec::with_capacity(take + 8);
        protocol::write_input(&mut framed, &buf[..take])
            .context("failed to frame input for a remote pane")?;
        out.queued_input_bytes += take;
        out.input.push_back(framed);
        Self::flush_locked(&mut out);
        Ok(take)
    }

    /// Send a control message.
    ///
    /// Control frames are neither capped nor queued behind input, on purpose.
    /// They are small and rare, and a delayed one is not a slowdown but a
    /// lasting wrong state — a `Resize` stuck behind a backlog leaves the remote
    /// app convinced of the wrong window size until something else resizes it.
    fn send_control(&self, msg: &ServerToBrokerMsg) -> Result<()> {
        let mut out = self
            .outbound
            .lock()
            .to_anyhow()
            .context("failed to lock a remote pane's outbound queue")?;
        if out.broken {
            return Err(anyhow!("the connection to {} was lost", self.host));
        }
        let mut framed: Vec<u8> = Vec::new();
        protocol::write_control(&mut framed, msg)
            .context("failed to frame a control message for a remote pane")?;
        out.control.push_back(framed);
        Self::flush_locked(&mut out);
        Ok(())
    }

    pub fn resize(&self, size: WireWinsize) -> Result<()> {
        self.send_control(&ServerToBrokerMsg::Resize(size))
    }

    /// Signal the pane's child.
    ///
    /// Symbolic, never a pid: the broker resolves this against the child it
    /// owns. A remote pid used locally would not merely fail, it would name some
    /// unrelated local process.
    pub fn signal_child(&self, signal: ChildSignal) -> Result<()> {
        self.send_control(&ServerToBrokerMsg::SignalChild(signal))
    }

    pub fn terminate(&self) -> Result<()> {
        self.send_control(&ServerToBrokerMsg::Terminate)
    }

    /// Push whatever the socket will take right now. Partial writes and `EAGAIN`
    /// are the expected case, not an error — the remainder stays queued.
    ///
    /// At each frame boundary control is preferred over input, so a resize or a
    /// signal issued behind a backlog still goes out next rather than last.
    fn flush_locked(out: &mut Outbound) {
        loop {
            // Pick a source. Mid-frame we are committed to whichever we were
            // already writing; at a boundary, control wins.
            if out.offset == 0 {
                out.writing_input = out.control.is_empty();
            }
            let frame = match if out.writing_input {
                out.input.front()
            } else {
                out.control.front()
            } {
                Some(frame) => frame,
                None => break,
            };

            match out.socket.write(&frame[out.offset..]) {
                Ok(0) => break,
                Ok(n) => {
                    out.offset += n;
                    if out.offset >= frame.len() {
                        out.offset = 0;
                        if out.writing_input {
                            if let Some(done) = out.input.pop_front() {
                                // The cap counts payload, so discount the header.
                                let payload = done.len().saturating_sub(FRAME_HEADER_LEN);
                                out.queued_input_bytes =
                                    out.queued_input_bytes.saturating_sub(payload);
                            }
                        } else {
                            out.control.pop_front();
                        }
                    }
                },
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    log::warn!("remote pane write failed: {e}");
                    out.broken = true;
                    out.control.clear();
                    out.input.clear();
                    out.queued_input_bytes = 0;
                    out.offset = 0;
                    break;
                },
            }
        }
    }

    // --- state the reader publishes and the server consumes ---

    pub fn set_status(&self, status: RemoteStatus) {
        if let Ok(mut slot) = self.status.lock() {
            *slot = status;
        }
    }

    pub fn status(&self) -> RemoteStatus {
        self.status
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|_| RemoteStatus::default())
    }

    pub fn set_child_exit(&self, status: Option<i32>) {
        if let Ok(mut slot) = self.child_exit.lock() {
            if slot.is_none() {
                *slot = Some(status);
            }
        }
    }

    pub fn child_exit(&self) -> Option<Option<i32>> {
        self.child_exit.lock().ok().and_then(|s| *s)
    }

    pub fn mark_transport_lost(&self) {
        self.transport_lost.store(true, Ordering::SeqCst);
        if let Ok(mut out) = self.outbound.lock() {
            out.broken = true;
            out.control.clear();
            out.input.clear();
            out.queued_input_bytes = 0;
            out.offset = 0;
        }
    }

    pub fn transport_lost(&self) -> bool {
        self.transport_lost.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
#[path = "../../unit/pty_host_remote_conn_tests.rs"]
mod pty_host_remote_conn_tests;
