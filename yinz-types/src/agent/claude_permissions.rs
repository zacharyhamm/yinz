//! Process-scoped presentation evidence, never a turn or task completion signal.
use super::claude_notifications::Source;
use serde::{Deserialize, Serialize};

pub const EVENT: &str = "ClaudePermissionStatus";
pub const DETAIL: &str = "approve Claude permission request";
/// Active observations renew every 500ms. Loss releases only this overlay.
pub const LEASE_MS: u64 = 3_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Active,
    Inactive,
    #[serde(other)]
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sample {
    pub source: Source,
    pub updated_ms: u64,
    /// Time the guarded reader saw this record. Heartbeats carry fresh observation,
    /// even when Claude has not rewritten an unchanged dialog.
    #[serde(default)]
    pub observed_ms: u64,
    pub status: Status,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Overlay {
    pub sample: Sample,
    pub remaining_ms: u64,
}

impl Overlay {
    /// Provider releases/cancellation win equal registry timestamps. Observation loss
    /// can recover only from a fresh read, never from a delayed heartbeat.
    pub fn accept(&mut self, sample: &Sample) {
        if sample.source != self.sample.source
            || sample.updated_ms < self.sample.updated_ms
            || (sample.updated_ms == self.sample.updated_ms
                && (sample.observed_ms < self.sample.observed_ms
                    || (self.sample.status == Status::Inactive && sample.status == Status::Active)
                    || (self.sample.status == Status::Unavailable
                        && sample.status == Status::Active
                        && sample.observed_ms == self.sample.observed_ms)))
        {
            return;
        }
        // Observation loss must not erase an authoritative release/cancellation fence.
        let released = sample.updated_ms == self.sample.updated_ms
            && self.sample.status == Status::Inactive
            && sample.status == Status::Unavailable;
        self.sample = sample.clone();
        if released {
            self.sample.status = Status::Inactive;
        }
        self.remaining_ms = LEASE_MS;
    }

    pub fn active(&self) -> bool {
        self.sample.status == Status::Active && self.remaining_ms > 0
    }

    pub fn release(&mut self) {
        self.sample.status = Status::Inactive;
        self.remaining_ms = 0;
    }

    pub fn elapse(&mut self, ms: u64) {
        if self.active() && ms >= self.remaining_ms {
            // Fence all reads from before the lost lease. A current, later guarded read
            // can recover the same dialog; it does not claim a new provider publication.
            self.sample.observed_ms = self.sample.observed_ms.saturating_add(self.remaining_ms);
            self.sample.status = Status::Unavailable;
        }
        self.remaining_ms = self.remaining_ms.saturating_sub(ms);
    }
}
