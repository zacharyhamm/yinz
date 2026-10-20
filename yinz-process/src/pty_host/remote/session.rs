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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use yinz_types::internal_ids::ProcessId;

use crate::pty_host::client::PtyHostHandle;
use crate::pty_host::remote::protocol::{RemotePaneEntry, ResumeRefusal};

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
pub const ORPHAN_TIMEOUT_ENV: &str = "YINZ_REMOTE_ORPHAN_TIMEOUT_SECS";

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
    pub(super) fn new(capacity: usize) -> Self {
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

/// Who, if anyone, this pane is talking to.
///
/// The middle state is the point of having one. A resuming connection owns the
/// pane from the moment it is accepted, but must not be written to by the pumps
/// until the replay it is owed has reached it — otherwise new output overtakes
/// the catch-up and the server sees an `Output` frame where it demanded a
/// `Resumed`.
enum WireState {
    // The timer and connection state must change atomically across detach and resume.
    Detached {
        since: Option<Instant>,
    },
    /// Accepted, being brought up to date. `is_attached`, but not writable.
    Attaching(u64),
    Attached(super::broker::Wire, u64),
    /// Claimed by the orphan reaper, but not yet terminated.
    Terminating,
}

impl WireState {
    fn generation(&self) -> Option<u64> {
        match self {
            WireState::Detached { .. } | WireState::Terminating => None,
            WireState::Attaching(generation) | WireState::Attached(_, generation) => {
                Some(*generation)
            },
        }
    }
}

/// What a resuming connection should do next.
pub enum ResumeStep {
    /// Bytes it has still to be sent before it can be attached.
    Replay { bytes: Vec<u8>, dropped: u64 },
    /// Caught up, and now attached. The pumps carry it from here.
    Attached,
    /// A newer connection claimed the pane while this one was catching up.
    Superseded,
}

/// One pane on this machine.
pub struct PaneSession {
    pub token: String,
    /// How the pane's command reads, after the default shell has been resolved.
    ///
    /// Kept so `ListPanes` can describe a pane to someone hunting for one they
    /// lost. Recorded rather than derived on demand because the only other
    /// place it exists is the supervisor's copy, on the far side of a handle
    /// this would have to lock to ask.
    pub command: String,
    handle: Mutex<PtyHostHandle>,
    /// Kept alive for as long as the pane exists. Both pumps borrow the raw
    /// descriptor; if either owned it, its exit would close a number the other
    /// might still be using — and by then it could belong to another pane.
    _master: OwnedFd,
    master_fd: RawFd,
    pub child_pid: ProcessId,
    ring: Mutex<OutputRing>,
    /// Who the pane is talking to, and which connection that is.
    wire: Mutex<WireState>,
    /// Bumped for every connection that claims the pane, so a connection that
    /// ended can be told from the one that replaced it.
    generation: AtomicU64,
    child_exit: Mutex<Option<Option<i32>>>,
    /// Set once the pty has been read to its end, so nothing the child wrote is
    /// still on its way here.
    output_drained: AtomicBool,
    output_drained_wait: Mutex<()>,
    output_drained_cv: Condvar,
    /// Set once the exit has actually been put on a wire, so the two things that
    /// might do it — the quit callback and the output pump — cannot both.
    exit_reported: AtomicBool,
    /// The last git probe, kept so a reconnecting server gets the pane's project
    /// straight away rather than waiting for its cwd to happen to change.
    git_probe: Mutex<Option<(super::status::GitProbe, Option<String>)>>,
    git_probe_target: Mutex<Option<(String, Option<String>)>>,
    terminated: AtomicBool,
}

impl PaneSession {
    /// Born detached. The connection that asked for the pane attaches only once
    /// it has been told the pane exists, so nothing can reach the wire ahead of
    /// the `Spawned` that names it.
    ///
    /// The orphan timer starts `None` rather than being set by `detach_if()`:
    /// that is what keeps the reaper away from a pane in the moment between
    /// being built and being attached.
    pub fn new(
        token: String,
        command: String,
        handle: PtyHostHandle,
        master: OwnedFd,
        child_pid: ProcessId,
    ) -> Self {
        let master_fd = master.as_raw_fd();
        Self {
            token,
            command,
            handle: Mutex::new(handle),
            _master: master,
            master_fd,
            child_pid,
            ring: Mutex::new(OutputRing::new(REPLAY_CAPACITY)),
            wire: Mutex::new(WireState::Detached { since: None }),
            generation: AtomicU64::new(0),
            child_exit: Mutex::new(None),
            output_drained: AtomicBool::new(false),
            output_drained_wait: Mutex::new(()),
            output_drained_cv: Condvar::new(),
            exit_reported: AtomicBool::new(false),
            git_probe: Mutex::new(None),
            git_probe_target: Mutex::new(None),
            terminated: AtomicBool::new(false),
        }
    }

    pub fn master_fd(&self) -> RawFd {
        self.master_fd
    }

    /// Buffer output and choose its live recipient under the same lock used by
    /// `resume_step`. Otherwise a reconnect can replay this chunk and attach
    /// before the pump selects its wire, delivering the chunk twice.
    pub fn append_output(&self, bytes: &[u8]) -> Option<(super::broker::Wire, u64)> {
        let mut ring = self.ring.lock().ok()?;
        ring.append(bytes);
        self.wire_and_generation()
    }

    /// The wire to send on right now, if one is ready to be written to.
    ///
    /// Cloned out rather than held, so a send never keeps the slot locked while
    /// it blocks on a socket. `None` while a connection is still catching up:
    /// its replay has to land before anything newer does.
    pub fn wire(&self) -> Option<super::broker::Wire> {
        self.wire_and_generation().map(|(wire, _)| wire)
    }

    pub fn wire_and_generation(&self) -> Option<(super::broker::Wire, u64)> {
        match self.wire.lock() {
            Ok(slot) => match &*slot {
                WireState::Attached(wire, generation) => Some((wire.clone(), *generation)),
                WireState::Attaching(_) | WireState::Detached { .. } | WireState::Terminating => {
                    None
                },
            },
            Err(_) => None,
        }
    }

    /// Attach a connection that has nothing to catch up on.
    pub fn attach(&self, wire: super::broker::Wire) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        if let Ok(mut slot) = self.wire.lock() {
            *slot = WireState::Attached(wire, generation);
        }
        generation
    }

    /// Claim the pane for a connection that still owes it a replay.
    ///
    /// It counts as attached from here — a second resume must be refused — but
    /// the pumps stay quiet until `resume_step` says it is caught up.
    pub fn try_begin_attach(&self) -> Result<u64, ResumeRefusal> {
        let mut slot = self.wire.lock().map_err(|_| ResumeRefusal::Unknown)?;
        match &*slot {
            WireState::Detached { .. } => {},
            WireState::Terminating => return Err(ResumeRefusal::Unknown),
            WireState::Attaching(_) | WireState::Attached(_, _) => {
                return Err(ResumeRefusal::AlreadyAttached);
            },
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *slot = WireState::Attaching(generation);
        Ok(generation)
    }

    /// The next thing a resuming connection owes, or the news that it is done.
    ///
    /// The emptiness check and the attach happen under the ring lock together,
    /// and that is the whole point: a chunk the pump appends either lands before
    /// the check, and so is in this replay, or after the attach, and so the pump
    /// sends it. It cannot fall between the two and be sent by neither.
    ///
    /// No socket write happens under that lock. The connection this replaces may
    /// be blocked writing to a peer that has gone away without saying so, and
    /// that write must never be able to hold up the reconnect replacing it.
    pub fn resume_step(
        &self,
        sent_upto: u64,
        wire: &super::broker::Wire,
        generation: u64,
    ) -> ResumeStep {
        let Ok(ring) = self.ring.lock() else {
            return ResumeStep::Superseded;
        };
        let (bytes, dropped) = ring.replay_from(sent_upto);
        if !bytes.is_empty() {
            return ResumeStep::Replay { bytes, dropped };
        }
        match self.wire.lock() {
            Ok(mut slot) => match slot.generation() {
                Some(current) if current == generation => {
                    *slot = WireState::Attached(wire.clone(), generation);
                    ResumeStep::Attached
                },
                _ => ResumeStep::Superseded,
            },
            Err(_) => ResumeStep::Superseded,
        }
    }

    /// Detach without ending anything. The pane keeps running and keeps
    /// buffering; only the orphan timer starts.
    ///
    /// Ignored unless the caller is the connection currently holding the pane.
    /// A connection that has already been replaced must not clear its
    /// successor's wire — that would silence a healthy pane and, worse, start
    /// an orphan clock that reaps it out from under a server that is right
    /// there watching it.
    pub fn detach_if(&self, generation: u64) {
        if let Ok(mut slot) = self.wire.lock() {
            if slot.generation() != Some(generation) {
                return;
            }
            *slot = WireState::Detached {
                since: Some(Instant::now()),
            };
        }
    }

    pub fn is_attached(&self) -> bool {
        self.wire
            .lock()
            .map(|slot| slot.generation().is_some())
            .unwrap_or(false)
    }

    pub fn replay_from(&self, received: u64) -> (Vec<u8>, u64) {
        self.ring
            .lock()
            .map(|ring| ring.replay_from(received))
            .unwrap_or_else(|_| (Vec::new(), 0))
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

    /// The pty gave up its end. Nothing the child wrote is still in flight.
    pub fn mark_output_drained(&self) {
        self.output_drained.store(true, Ordering::SeqCst);
        if let Ok(_guard) = self.output_drained_wait.lock() {
            self.output_drained_cv.notify_all();
        }
    }

    pub fn wait_for_output_drained(&self, timeout: Duration) -> bool {
        if self.output_drained.load(Ordering::SeqCst) {
            return true;
        }
        let Ok(guard) = self.output_drained_wait.lock() else {
            return false;
        };
        let _ = self
            .output_drained_cv
            .wait_timeout_while(guard, timeout, |_| {
                !self.output_drained.load(Ordering::SeqCst)
            });
        self.output_drained.load(Ordering::SeqCst)
    }

    /// The exit status, but only once it is safe to send.
    ///
    /// Both facts have to be in: that the child went, and that everything it
    /// wrote has been read. Reporting on the first alone races the output pump,
    /// and the server stops reading at `ChildExited` — so a pane that printed
    /// and exited in the same breath would show nothing at all.
    pub fn exit_is_reportable(&self) -> Option<Option<i32>> {
        if !self.output_drained.load(Ordering::SeqCst) {
            return None;
        }
        self.child_exit()
    }

    /// Take the right to send the exit. True for the first caller only.
    pub fn claim_exit_report(&self) -> bool {
        !self.exit_reported.swap(true, Ordering::SeqCst)
    }

    pub fn request_git_probe(&self, cwd: String, refresh_token: Option<String>) {
        if let Ok(mut target) = self.git_probe_target.lock() {
            *target = Some((cwd, refresh_token));
        }
    }

    pub fn git_probe_target(&self) -> Option<(String, Option<String>)> {
        self.git_probe_target
            .lock()
            .ok()
            .and_then(|target| target.clone())
    }

    pub fn set_git_probe(&self, probe: super::status::GitProbe, refresh_token: Option<String>) {
        if let Ok(mut slot) = self.git_probe.lock() {
            *slot = Some((probe, refresh_token));
        }
    }

    pub fn git_probe(&self) -> Option<(super::status::GitProbe, Option<String>)> {
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
        self.wire.lock().ok().and_then(|slot| match &*slot {
            WireState::Detached { since } => since.map(|t| t.elapsed()),
            _ => None,
        })
    }

    fn try_claim_expired_orphan(&self, timeout: Duration) -> bool {
        let Ok(mut slot) = self.wire.lock() else {
            return false;
        };
        let WireState::Detached { since: Some(since) } = &*slot else {
            return false;
        };
        if since.elapsed() < timeout {
            return false;
        }
        *slot = WireState::Terminating;
        true
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

    /// Describe every pane this broker holds, for `ListPanes`.
    ///
    /// Sorted by token so two calls read the same way and a person scanning the
    /// output of one host after another is not re-reading a shuffled list.
    pub fn entries(&self) -> Vec<RemotePaneEntry> {
        let Ok(panes) = self.panes.lock() else {
            return Vec::new();
        };
        let mut entries: Vec<RemotePaneEntry> = panes
            .values()
            .map(|session| RemotePaneEntry {
                pane_token: session.token.clone(),
                command: session.command.clone(),
                attached: session.is_attached(),
                unattached_secs: session.orphaned_for().map(|elapsed| elapsed.as_secs()),
            })
            .collect();
        entries.sort_by(|a, b| a.pane_token.cmp(&b.pane_token));
        entries
    }

    /// Terminate panes nobody has come back for. Returns how many were reclaimed.
    pub fn reap_orphans(&self, timeout: Duration) -> usize {
        let expired: Vec<Arc<PaneSession>> = match self.panes.lock() {
            Ok(panes) => panes
                .values()
                .filter(|session| session.try_claim_expired_orphan(timeout))
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
