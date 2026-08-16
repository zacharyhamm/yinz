//! A remote pane, which outlives the connection that asked for it.
//!
//! Phase 1 tied a pane to its connection: the connection ended, the pane ended.
//! That cannot survive a network, where a connection ending is routine and says
//! nothing about the process. So the pane lives here instead, in a registry
//! keyed by a token, and connections attach to and detach from it.
//!
//! The buffering lives here rather than in the supervisor on purpose. The
//! supervisor's ring is sized for warp handoffs measured in milliseconds
//! (256 KiB), and its `take()` resets the dropped count, so a second gap hides
//! the first. An outage is measured in minutes, and whether bytes were lost is
//! precisely what the reconnect needs to know.

use std::collections::{HashMap, VecDeque};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::pty_host::client::PtyHostHandle;

/// How much output to keep for a pane whose server is not currently connected.
///
/// Generous compared to the supervisor's 256 KiB because it is covering a
/// different event: not a handoff, but an outage. A full-screen application
/// redrawing a 200x60 terminal costs on the order of 100 KiB per repaint, so
/// this holds a good many of them.
const REPLAY_CAPACITY: usize = 4 * 1024 * 1024;

/// How long a pane waits, unclaimed, before the broker gives up on it.
///
/// The whole point of the pane outliving its connection is that it can be picked
/// back up, so this is long. But it is not infinite: a server that crashed and
/// never came back would otherwise leave a process and a supervisor running
/// here forever, with nothing on this machine aware they were abandoned.
const ORPHAN_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Overrides `ORPHAN_TIMEOUT`, so a test can watch a pane be reclaimed without
/// waiting half an hour for it.
///
/// A test seam rather than configuration: it is read here, on the machine
/// running the broker, so it has to be set in that machine's environment —
/// which is exactly the awkwardness that keeps it from being a real knob.
pub const ORPHAN_TIMEOUT_ENV: &str = "ZELLIJ_REMOTE_ORPHAN_TIMEOUT_SECS";

/// The last `capacity` bytes the pane has produced, and how many it produced in
/// total.
///
/// The total is what makes an exact replay possible: a resuming server says how
/// many bytes it actually took delivery of, and the difference is either
/// something this buffer still holds or a hole that has to be admitted to.
pub struct OutputRing {
    buf: VecDeque<u8>,
    capacity: usize,
    produced: u64,
}

impl OutputRing {
    fn new(capacity: usize) -> Self {
        Self {
            buf: VecDeque::new(),
            capacity,
            produced: 0,
        }
    }

    pub fn append(&mut self, bytes: &[u8]) {
        self.produced = self.produced.saturating_add(bytes.len() as u64);
        self.buf.extend(bytes);
        while self.buf.len() > self.capacity {
            let excess = self.buf.len() - self.capacity;
            self.buf.drain(..excess);
        }
    }

    pub fn produced(&self) -> u64 {
        self.produced
    }

    /// The bytes a peer that has taken `received` still needs, and how many were
    /// lost before this buffer could hold them.
    pub fn replay_from(&self, received: u64) -> (Vec<u8>, u64) {
        let oldest = self.produced.saturating_sub(self.buf.len() as u64);
        if received < oldest {
            // The gap is longer than the buffer. Everything between what they
            // have and what we still hold is gone.
            let dropped = oldest - received;
            return (self.buf.iter().copied().collect(), dropped);
        }
        // A peer claiming more than was ever produced is confused; treat it as
        // caught up rather than panicking on the subtraction.
        let skip = (received - oldest).min(self.buf.len() as u64) as usize;
        (self.buf.iter().skip(skip).copied().collect(), 0)
    }
}

/// One pane on this machine.
pub struct PaneSession {
    pub token: String,
    handle: Mutex<PtyHostHandle>,
    /// Kept alive for as long as the pane exists. Both pumps borrow the raw
    /// descriptor; if either owned it, its exit would close a number the other
    /// might still be using — and by then it could belong to another pane.
    _master: OwnedFd,
    master_fd: RawFd,
    pub child_pid: u32,
    ring: Mutex<OutputRing>,
    /// `Some` while a server is attached.
    wire: Mutex<Option<super::broker::Wire>>,
    child_exit: Mutex<Option<Option<i32>>>,
    /// The last git probe, kept so a reconnecting server gets the pane's project
    /// straight away rather than waiting for its cwd to happen to change.
    git_probe: Mutex<Option<super::status::GitProbe>>,
    disconnected_since: Mutex<Option<Instant>>,
    terminated: AtomicBool,
}

impl PaneSession {
    pub fn new(
        token: String,
        handle: PtyHostHandle,
        master: OwnedFd,
        child_pid: u32,
        wire: super::broker::Wire,
    ) -> Self {
        let master_fd = master.as_raw_fd();
        Self {
            token,
            handle: Mutex::new(handle),
            _master: master,
            master_fd,
            child_pid,
            ring: Mutex::new(OutputRing::new(REPLAY_CAPACITY)),
            wire: Mutex::new(Some(wire)),
            child_exit: Mutex::new(None),
            git_probe: Mutex::new(None),
            disconnected_since: Mutex::new(None),
            terminated: AtomicBool::new(false),
        }
    }

    pub fn master_fd(&self) -> RawFd {
        self.master_fd
    }

    pub fn append_output(&self, bytes: &[u8]) {
        if let Ok(mut ring) = self.ring.lock() {
            ring.append(bytes);
        }
    }

    /// The wire to send on right now, if a server is attached.
    ///
    /// Cloned out rather than held, so a send never keeps the slot locked while
    /// it blocks on a socket.
    pub fn wire(&self) -> Option<super::broker::Wire> {
        self.wire.lock().ok().and_then(|w| w.clone())
    }

    pub fn attach(&self, wire: super::broker::Wire) {
        if let Ok(mut slot) = self.wire.lock() {
            *slot = Some(wire);
        }
        if let Ok(mut since) = self.disconnected_since.lock() {
            *since = None;
        }
    }

    /// Detach without ending anything. The pane keeps running and keeps
    /// buffering; only the orphan timer starts.
    pub fn detach(&self) {
        if let Ok(mut slot) = self.wire.lock() {
            *slot = None;
        }
        if let Ok(mut since) = self.disconnected_since.lock() {
            if since.is_none() {
                *since = Some(Instant::now());
            }
        }
    }

    pub fn is_attached(&self) -> bool {
        self.wire.lock().map(|w| w.is_some()).unwrap_or(false)
    }

    pub fn replay_from(&self, received: u64) -> (Vec<u8>, u64) {
        self.ring
            .lock()
            .map(|ring| ring.replay_from(received))
            .unwrap_or_else(|_| (Vec::new(), 0))
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

    pub fn set_git_probe(&self, probe: super::status::GitProbe) {
        if let Ok(mut slot) = self.git_probe.lock() {
            *slot = Some(probe);
        }
    }

    pub fn git_probe(&self) -> Option<super::status::GitProbe> {
        self.git_probe.lock().ok().and_then(|p| p.clone())
    }

    /// End the pane. The only thing that does.
    pub fn terminate(&self) {
        if self.terminated.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Ok(mut handle) = self.handle.lock() {
            handle.terminate();
        }
    }

    pub fn is_terminated(&self) -> bool {
        self.terminated.load(Ordering::SeqCst)
    }

    fn orphaned_for(&self) -> Option<Duration> {
        self.disconnected_since
            .lock()
            .ok()
            .and_then(|since| since.map(|t| t.elapsed()))
    }
}

/// Every pane this broker holds.
#[derive(Default)]
pub struct PaneRegistry {
    panes: Mutex<HashMap<String, Arc<PaneSession>>>,
}

impl PaneRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, session: Arc<PaneSession>) {
        if let Ok(mut panes) = self.panes.lock() {
            panes.insert(session.token.clone(), session);
        }
    }

    pub fn get(&self, token: &str) -> Option<Arc<PaneSession>> {
        self.panes.lock().ok()?.get(token).cloned()
    }

    pub fn remove(&self, token: &str) -> Option<Arc<PaneSession>> {
        self.panes.lock().ok()?.remove(token)
    }

    pub fn len(&self) -> usize {
        self.panes.lock().map(|p| p.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Terminate panes nobody has come back for. Returns how many were reclaimed.
    pub fn reap_orphans(&self, timeout: Duration) -> usize {
        let expired: Vec<Arc<PaneSession>> = match self.panes.lock() {
            Ok(panes) => panes
                .values()
                .filter(|s| {
                    !s.is_attached() && s.orphaned_for().is_some_and(|elapsed| elapsed >= timeout)
                })
                .cloned()
                .collect(),
            Err(_) => return 0,
        };
        for session in &expired {
            log::info!(
                "reclaiming remote pane {} — nothing has picked it up in {:?}",
                session.token,
                timeout
            );
            session.terminate();
            self.remove(&session.token);
        }
        expired.len()
    }
}

pub fn orphan_timeout() -> Duration {
    match std::env::var(ORPHAN_TIMEOUT_ENV) {
        Ok(secs) => {
            match secs.trim().parse::<u64>() {
                // Zero would reclaim a pane the instant its connection dropped,
                // which is the one thing this timeout exists to prevent, so it is
                // treated as "unset" rather than obeyed.
                Ok(secs) if secs > 0 => Duration::from_secs(secs),
                _ => {
                    log::warn!("ignoring {ORPHAN_TIMEOUT_ENV}={secs:?}: expected a positive number of seconds");
                    ORPHAN_TIMEOUT
                },
            }
        },
        Err(_) => ORPHAN_TIMEOUT,
    }
}

#[cfg(test)]
#[path = "../../unit/pty_host_remote_session_tests.rs"]
mod pty_host_remote_session_tests;
