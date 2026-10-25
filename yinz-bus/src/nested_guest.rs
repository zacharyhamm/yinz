use crate::PaneId;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
#[cfg(test)]
use yinz_types::internal_ids::TerminalId;

pub const PING_INTERVAL_MS: u64 = 2000;
pub const PONG_TIMEOUT_MS: u64 = 5000;

pub fn ping_interval_ms() -> u64 {
    millis_from_env(
        std::env::var("YINZ_NESTED_PING_INTERVAL_MS").ok(),
        PING_INTERVAL_MS,
    )
}

pub fn pong_timeout_ms() -> u64 {
    millis_from_env(
        std::env::var("YINZ_NESTED_PONG_TIMEOUT_MS").ok(),
        PONG_TIMEOUT_MS,
    )
}

/// An override read from the environment: a value that is a whole number of milliseconds wins,
/// and anything else - unset, empty, or not a number - falls back to `default`. Split out from
/// the two readers so it can be tested without touching the process environment, which every
/// other test in this module reads through `on_tick`.
fn millis_from_env(value: Option<String>, default: u64) -> u64 {
    value
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(default)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestPingTickAction {
    SendPing,
    Suspended,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceKind {
    New,
    Revived,
    Refresh,
}

#[derive(Debug, Default)]
pub struct NestedGuestTracker {
    last_heard_from: HashMap<PaneId, Instant>,
    dormant: HashSet<PaneId>,
}

impl NestedGuestTracker {
    pub fn on_announce(&mut self, pane_id: PaneId, now: Instant) -> AnnounceKind {
        let was_tracked = self.last_heard_from.contains_key(&pane_id);
        let was_dormant = self.dormant.remove(&pane_id);
        self.last_heard_from.insert(pane_id, now);
        if was_tracked {
            AnnounceKind::Refresh
        } else if was_dormant {
            AnnounceKind::Revived
        } else {
            AnnounceKind::New
        }
    }
    pub fn on_pong(&mut self, pane_id: PaneId, now: Instant) {
        if let Some(last_heard_from) = self.last_heard_from.get_mut(&pane_id) {
            *last_heard_from = now;
        }
    }
    pub fn on_tick(&mut self, pane_id: PaneId, now: Instant) -> GuestPingTickAction {
        match self.last_heard_from.get(&pane_id) {
            Some(last_heard_from)
                if now.duration_since(*last_heard_from)
                    > Duration::from_millis(pong_timeout_ms()) =>
            {
                self.last_heard_from.remove(&pane_id);
                self.dormant.insert(pane_id);
                GuestPingTickAction::Suspended
            },
            Some(_) => GuestPingTickAction::SendPing,
            None => GuestPingTickAction::Unknown,
        }
    }
    pub fn remove(&mut self, pane_id: PaneId) {
        self.last_heard_from.remove(&pane_id);
        self.dormant.remove(&pane_id);
    }
    pub fn is_tracked(&self, pane_id: PaneId) -> bool {
        self.last_heard_from.contains_key(&pane_id)
    }
    pub fn is_dormant(&self, pane_id: PaneId) -> bool {
        self.dormant.contains(&pane_id)
    }
    pub fn is_known(&self, pane_id: PaneId) -> bool {
        self.is_tracked(pane_id) || self.is_dormant(pane_id)
    }
}

#[cfg(test)]
#[path = "unit/nested_guest.rs"]
mod tests;
