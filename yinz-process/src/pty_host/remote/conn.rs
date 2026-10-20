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
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use yinz_runtime::errors::prelude::*;
use yinz_types::input::command::RunCommand;

use super::protocol::{
    self, BrokerToServerMsg, ChildSignal, Frame, ServerToBrokerMsg, FRAME_HEADER_LEN,
    MAX_STREAM_PAYLOAD,
};
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
    pub child_group_terminated: bool,
    pub cwd: Option<String>,
    pub shell_argv: Option<Vec<String>>,
    pub foreground_argv: Option<Vec<String>>,
}

/// What git said about a remote pane's working directory, unparsed.
///
/// Carried to the plugins that attribute panes to projects, which already know
/// how to read exactly this, by `PluginCommand::GetPaneGitProbe`. Kept raw so
/// the parsing stays in one place rather than being duplicated for remote panes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RemoteGitProbe {
    pub refresh_token: Option<String>,
    /// The machine this was run on, so two checkouts at the same path on
    /// different hosts stay distinct.
    pub host: String,
    pub cwd: String,
    pub exit_code: Option<i32>,
    pub metadata: Vec<u8>,
    pub config: Vec<u8>,
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

/// What a reconnect produced.
pub struct Reconnected {
    pub write_socket: UnixStream,
    pub read_socket: UnixStream,
    /// Output bytes lost before the broker's buffer could hold them. Non-zero
    /// means the replay has a hole in it.
    pub dropped: u64,
}

/// How to get this pane's connection back.
///
/// A trait so `conn.rs` needs to know nothing about ssh — it holds a pane's
/// state and speaks frames, and reconnecting is somebody else's transport
/// problem.
pub trait Reconnector: Send + Sync {
    fn reconnect(&self, host: &str, pane_token: &str, received: u64) -> Result<Reconnected>;
}

/// A live connection to one pane's broker.
pub struct RemoteConn {
    outbound: Mutex<Outbound>,
    pub(super) outbound_ready: tokio::sync::Notify,
    status: Mutex<RemoteStatus>,
    /// `Some(status)` once the broker has reported the child exiting. This is
    /// the *only* thing that means the pane is over — a transport failure must
    /// never be read that way.
    child_exit: Mutex<Option<Option<i32>>>,
    git_probe: Mutex<Option<RemoteGitProbe>>,
    git_probe_target: Mutex<Option<(String, Option<String>)>>,
    transport_lost: AtomicBool,
    /// A label for diagnostics, and the address to reconnect to.
    host: String,
    /// How the broker names this pane on a resume.
    pane_token: String,
    /// What the pane runs, as it was asked for.
    ///
    /// Held because nothing else on this side does. A local pane's command can
    /// always be got back from its supervisor; a broker's answer to a resume
    /// carries none, so a pane handed to another session would arrive unable to
    /// name what it runs — and so unable to title itself, be re-run on the right
    /// machine when held, or be attributed to a project.
    cmd: RunCommand,
    /// The output offset to resume from: delivered bytes plus gaps the broker
    /// reported as lost. Bytes buffered locally do not count until handed to
    /// the reader's caller, so a transport failure replays them.
    received: AtomicU64,
    /// The last size sent, kept so a reconnect can nudge the window and make a
    /// full-screen application redraw.
    last_size: Mutex<Option<WireWinsize>>,
    reconnector: Option<Arc<dyn Reconnector>>,
}

impl RemoteConn {
    pub fn new(
        socket: UnixStream,
        host: String,
        pane_token: String,
        cmd: RunCommand,
        reconnector: Option<Arc<dyn Reconnector>>,
    ) -> Result<Self> {
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
            outbound_ready: tokio::sync::Notify::new(),
            status: Mutex::new(RemoteStatus::default()),
            child_exit: Mutex::new(None),
            git_probe: Mutex::new(None),
            git_probe_target: Mutex::new(None),
            transport_lost: AtomicBool::new(false),
            host,
            pane_token,
            cmd,
            received: AtomicU64::new(0),
            last_size: Mutex::new(None),
            reconnector,
        })
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn display_host(&self) -> String {
        self.cmd.display_host().unwrap_or_else(|| self.host.clone())
    }

    /// What the pane runs. See the field.
    pub fn cmd(&self) -> &RunCommand {
        &self.cmd
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
                self.display_host()
            ));
        }
        // Drain first: a queue that fully flushed last tick should not report
        // back-pressure this tick just because nothing has emptied it since.
        Self::flush_locked(&mut out)?;

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
        Self::flush_locked(&mut out)?;
        self.outbound_ready.notify_one();
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
            return Err(anyhow!(
                "the connection to {} was lost",
                self.display_host()
            ));
        }
        let mut framed: Vec<u8> = Vec::new();
        protocol::write_control(&mut framed, msg)
            .context("failed to frame a control message for a remote pane")?;
        out.control.push_back(framed);
        Self::flush_locked(&mut out)?;
        self.outbound_ready.notify_one();
        Ok(())
    }

    pub(super) fn has_pending_writes(&self) -> bool {
        self.outbound
            .lock()
            .is_ok_and(|out| !out.broken && (!out.control.is_empty() || !out.input.is_empty()))
    }

    /// The reader drives socket readiness after the writer has accepted the last
    /// input chunk. No later keystroke is needed to flush the queued tail.
    pub(super) fn flush_pending(&self) -> Result<bool> {
        let mut out = self
            .outbound
            .lock()
            .to_anyhow()
            .context("failed to lock a remote pane's outbound queue")?;
        Self::flush_locked(&mut out)?;
        Ok(!out.control.is_empty() || !out.input.is_empty())
    }

    pub fn resize(&self, size: WireWinsize) -> Result<()> {
        if let Ok(mut last) = self.last_size.lock() {
            *last = Some(size);
        }
        self.send_control(&ServerToBrokerMsg::Resize(size))
    }

    /// Tell the broker the size again, whatever it currently believes.
    ///
    /// A resize made while the transport was down updated `last_size` and then
    /// failed to send — `send_control` refuses a broken connection, and the
    /// queues are cleared on the way through `mark_transport_lost` and
    /// `reconnect` besides. So a reconnect has to say it again, or the remote
    /// application keeps drawing at the size it had before the outage inside a
    /// grid that has since changed shape.
    ///
    /// Unlike `nudge_for_redraw` this is a plain resize, and deliberately: if
    /// the size did not in fact change, the broker's `TIOCSWINSZ` is a no-op, no
    /// `SIGWINCH` is raised, and a reconnect that changed nothing costs the user
    /// no repaint.
    pub fn resend_size(&self) {
        // No `cols > 1` guard here — that belongs to the nudge's wiggle. Any
        // size is worth resending; only never having had one is a no-op.
        let Some(size) = self.last_size.lock().ok().and_then(|s| *s) else {
            return;
        };
        let _ = self.send_control(&ServerToBrokerMsg::Resize(size));
    }

    /// Make a full-screen application repaint.
    ///
    /// After a lossy replay the local grid has been reset, but the remote
    /// application does not know that and will not redraw until something makes
    /// it. A window-size change raises `SIGWINCH`, which is the only lever
    /// available from this side — so send a size that differs, then the real one
    /// back. Both are ordinary resizes, so nothing else has to understand this.
    pub fn nudge_for_redraw(&self) {
        let size = match self.last_size.lock().ok().and_then(|s| *s) {
            Some(size) if size.cols > 1 => size,
            // Nothing has sized this pane yet, so there is nothing to nudge and
            // nothing on screen to repair.
            _ => return,
        };
        let smaller = WireWinsize {
            cols: size.cols - 1,
            ..size
        };
        let _ = self.send_control(&ServerToBrokerMsg::Resize(smaller));
        let _ = self.send_control(&ServerToBrokerMsg::Resize(size));
    }

    pub fn pane_token(&self) -> &str {
        &self.pane_token
    }

    /// Whether this pane has any way to get its connection back.
    ///
    /// A pane built without a reconnector — a test double, or a transport that
    /// cannot be re-established — should fail fast rather than sit through a
    /// backoff schedule that cannot succeed.
    pub fn can_reconnect(&self) -> bool {
        self.reconnector.is_some()
    }

    /// Record output handed to the reader's caller. See `received`.
    pub fn note_received(&self, bytes: usize) {
        self.received.fetch_add(bytes as u64, Ordering::SeqCst);
    }

    /// Start this connection's byte count where another session's left off.
    ///
    /// For a pane adopted from a warp. The `RemoteConn` is new, so its counter
    /// starts at zero, but the pane it speaks for is not — the broker's stream
    /// is already thousands of bytes in. Left at zero, the *next* reconnect
    /// would ask to resume from the start and be replayed the whole ring on top
    /// of a grid that already shows it. Include any gap reported by the broker:
    /// lost bytes still occupy positions in that stream.
    pub fn restore_received(&self, received: u64, dropped: u64) {
        self.received
            .store(received.saturating_add(dropped), Ordering::SeqCst);
    }

    pub fn received(&self) -> u64 {
        self.received.load(Ordering::SeqCst)
    }

    /// Re-establish this pane's connection, replacing the outbound socket.
    ///
    /// Blocking: it runs ssh. Callers on an async task must push it to a
    /// blocking pool rather than stalling the runtime.
    pub fn reconnect(&self) -> Result<(UnixStream, u64)> {
        let reconnector = self
            .reconnector
            .as_ref()
            .ok_or_else(|| anyhow!("this pane has no way to reconnect"))?;
        let received = self.received();
        let resumed = reconnector.reconnect(&self.host, &self.pane_token, received)?;
        let mut out = self
            .outbound
            .lock()
            .to_anyhow()
            .context("failed to lock a remote pane's outbound queue")?;
        resumed
            .write_socket
            .set_nonblocking(true)
            .context("failed to make a reconnected socket non-blocking")?;
        // Anything queued belonged to a socket that no longer exists. Input the
        // user typed into a disconnected pane was never accepted by the far
        // side, and re-sending a half-written frame would corrupt the stream.
        out.control.clear();
        out.input.clear();
        out.queued_input_bytes = 0;
        out.offset = 0;
        out.writing_input = false;
        out.socket = resumed.write_socket;
        out.broken = false;
        drop(out);
        self.restore_received(received, resumed.dropped);
        self.transport_lost.store(false, Ordering::SeqCst);
        if let Ok(target) = self.git_probe_target.lock()
            && let Some((cwd, refresh_token)) = target.as_ref()
        {
            self.send_control(&ServerToBrokerMsg::GetGitProbe {
                cwd: cwd.clone(),
                refresh_token: refresh_token.clone(),
            })?;
        }
        Ok((resumed.read_socket, resumed.dropped))
    }

    /// Signal the pane's child.
    ///
    /// Symbolic, never a pid: the broker resolves this against the child it
    /// owns. A remote pid used locally would not merely fail, it would name some
    /// unrelated local process.
    pub fn signal_child(&self, signal: ChildSignal) -> Result<()> {
        self.send_control(&ServerToBrokerMsg::SignalChild(signal))
    }

    /// Ask the broker to end the pane. It owns termination and pty draining.
    pub fn terminate(&self) -> Result<()> {
        self.send_control(&ServerToBrokerMsg::Terminate)
    }

    /// Hand this pane to another session: tell the broker to let it go, and wait
    /// until it says it has.
    ///
    /// The counterpart of `PtyHostHandle::release()`, and synchronous for the
    /// same reason. Merely dropping the connection *would* detach the pane —
    /// the broker notices when its read returns — but not at any moment this
    /// side can name, and the adopting server's `Resume` is refused outright
    /// while the pane still looks attached. Waiting for `Released` is what turns
    /// "it will be free soon" into "it is free now".
    ///
    /// Called only after the reader task has been aborted, which is what makes
    /// reading the ack here safe: the read half was a `try_clone` of this same
    /// socket, so with the reader gone this is the only thing left looking at
    /// the connection.
    ///
    /// Output frames arriving before the ack are discarded, and nothing is lost
    /// by that. The broker replays from the byte count *this side reports*, not
    /// from what it believes it has sent, so anything dropped here comes again
    /// on the resume — which is precisely why `received` is sampled before this
    /// is called and not after.
    pub fn release_for_warp(&self, timeout: Duration) -> Result<()> {
        let mut out = self
            .outbound
            .lock()
            .to_anyhow()
            .context("failed to lock a remote pane's outbound queue")?;
        if out.broken {
            return Err(anyhow!(
                "the connection to {} was lost, so this pane cannot be handed over",
                self.display_host()
            ));
        }
        // Refused up front rather than discovered a line later: std will not
        // bound a socket by a zero duration, and the error it gives —
        // "cannot set a 0 duration timeout" — describes the socket rather than
        // the situation, which is a caller that has already given up.
        if timeout.is_zero() {
            return Err(anyhow!(
                "no time was left to ask {} to release this pane",
                self.display_host()
            ));
        }
        // Blocking for the rest of this call. The queues are drained by whoever
        // writes next, and after a release nobody does — a half-written frame
        // left behind would be a truncated message on a connection the next
        // owner is about to resume behind.
        out.socket
            .set_nonblocking(false)
            .context("failed to make a remote pane's socket blocking to release it")?;
        out.socket
            .set_write_timeout(Some(timeout))
            .context("failed to bound a remote pane's release write")?;
        out.socket
            .set_read_timeout(Some(timeout))
            .context("failed to bound a remote pane's release reply")?;
        Self::flush_locked(&mut out)?;
        if !out.control.is_empty() || !out.input.is_empty() {
            return Err(anyhow!(
                "timed out draining queued frames before releasing the remote pane"
            ));
        }

        let mut framed: Vec<u8> = Vec::new();
        protocol::write_control(&mut framed, &ServerToBrokerMsg::Release)
            .context("failed to frame a release for a remote pane")?;
        out.socket.write_all(&framed).with_context(|| {
            format!("failed to ask {} to release this pane", self.display_host())
        })?;

        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() >= deadline {
                return Err(anyhow!(
                    "{} did not confirm the pane was released within {timeout:?}",
                    self.display_host()
                ));
            }
            let frame = protocol::read_frame(&mut out.socket)
                .with_context(|| {
                    format!(
                        "failed to read {}'s reply to a release",
                        self.display_host()
                    )
                })?
                .ok_or_else(|| {
                    anyhow!(
                        "{} closed the connection without confirming the release",
                        self.display_host()
                    )
                })?;
            let Frame::Control(_) = frame else {
                // Output the pane produced on its way out, or input echoed back
                // by a confused peer. Neither answers the question.
                continue;
            };
            match frame.decode_control::<BrokerToServerMsg>() {
                Ok(BrokerToServerMsg::Released) => return Ok(()),
                // The child ending is not a refusal, but it does mean there is
                // nothing left to hand over. Reported as an error so the warp
                // rolls back rather than moving a dead pane.
                Ok(BrokerToServerMsg::ChildExited { status }) => {
                    return Err(anyhow!(
                        "the pane's process on {} exited ({status:?}) as it was being handed over",
                        self.display_host()
                    ));
                },
                // Status and git reports keep arriving until the broker reads
                // the release; they say nothing about it.
                Ok(_) => continue,
                Err(e) => return Err(e).context("could not read the reply to a release"),
            }
        }
    }

    /// Push whatever the socket will take right now. Partial writes and `EAGAIN`
    /// are the expected case, not an error — the remainder stays queued.
    ///
    /// At each frame boundary control is preferred over input, so a resize or a
    /// signal issued behind a backlog still goes out next rather than last.
    fn flush_locked(out: &mut Outbound) -> Result<()> {
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

            let written = out.socket.write(&frame[out.offset..]).and_then(|n| {
                if n == 0 {
                    Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "remote pane socket accepted no bytes",
                    ))
                } else {
                    Ok(n)
                }
            });
            match written {
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
                    out.broken = true;
                    out.control.clear();
                    out.input.clear();
                    out.queued_input_bytes = 0;
                    out.offset = 0;
                    return Err(e).context("failed to write a remote pane frame");
                },
            }
        }
        Ok(())
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

    pub fn set_git_probe(&self, probe: RemoteGitProbe) {
        if let Ok(target) = self.git_probe_target.lock()
            && let Some((cwd, Some(token))) = target.as_ref()
            && (probe.cwd != *cwd || probe.refresh_token.as_ref() != Some(token))
        {
            return;
        }
        if let Ok(mut slot) = self.git_probe.lock() {
            *slot = Some(probe);
        }
    }

    pub fn git_probe(&self, cwd: &str) -> Result<Option<RemoteGitProbe>> {
        self.git_probe_with_refresh(cwd, None)
    }

    pub fn git_probe_with_refresh(
        &self,
        cwd: &str,
        refresh_token: Option<&str>,
    ) -> Result<Option<RemoteGitProbe>> {
        if cwd.is_empty() {
            return Err(anyhow!("git probe directory is empty"));
        }
        let mut target = self
            .git_probe_target
            .lock()
            .to_anyhow()
            .context("failed to lock the remote git probe target")?;
        if target.as_ref().map(|(path, _)| path.as_str()) != Some(cwd)
            || refresh_token.is_some_and(|token| {
                target.as_ref().and_then(|(_, token)| token.as_deref()) != Some(token)
            })
        {
            *target = Some((cwd.to_owned(), refresh_token.map(str::to_owned)));
            self.send_control(&ServerToBrokerMsg::GetGitProbe {
                cwd: cwd.to_owned(),
                refresh_token: refresh_token.map(str::to_owned),
            })?;
        }
        Ok(self
            .git_probe
            .lock()
            .to_anyhow()
            .context("failed to lock the remote git probe result")?
            .as_ref()
            .filter(|probe| {
                probe.cwd == cwd
                    && refresh_token
                        .is_none_or(|token| probe.refresh_token.as_deref() == Some(token))
            })
            .cloned())
    }

    pub fn set_child_exit(&self, status: Option<i32>) {
        if let Ok(mut slot) = self.child_exit.lock()
            && slot.is_none()
        {
            *slot = Some(status);
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
