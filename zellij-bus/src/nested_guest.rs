use crate::PaneId;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
#[cfg(test)]
use zellij_types::internal_ids::TerminalId;

pub const PING_INTERVAL_MS: u64 = 2000;
pub const PONG_TIMEOUT_MS: u64 = 5000;

pub fn ping_interval_ms() -> u64 {
    millis_from_env(
        std::env::var("ZELLIJ_NESTED_PING_INTERVAL_MS").ok(),
        PING_INTERVAL_MS,
    )
}

pub fn pong_timeout_ms() -> u64 {
    millis_from_env(
        std::env::var("ZELLIJ_NESTED_PONG_TIMEOUT_MS").ok(),
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
mod tests {
    use super::*;

    fn millis(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn announced_guest_gets_pinged_and_pong_keeps_it_alive() {
        let mut tracker = NestedGuestTracker::default();
        let pane_id = PaneId::Terminal(TerminalId::new(1));
        let start = Instant::now();
        tracker.on_announce(pane_id, start);
        // `PING_INTERVAL_MS` here is decorative: the tracker does not pace pings, the
        // background job's ticker does (`background_jobs.rs`), and `on_tick` answers `SendPing`
        // at any instant inside the pong timeout. It is used so the test reads as the cadence
        // production actually ticks at.
        assert_eq!(
            tracker.on_tick(pane_id, start + millis(PING_INTERVAL_MS)),
            GuestPingTickAction::SendPing
        );
        tracker.on_pong(pane_id, start + millis(4000));
        assert_eq!(
            tracker.on_tick(pane_id, start + millis(8000)),
            GuestPingTickAction::SendPing
        );
    }

    #[test]
    fn guest_that_never_pongs_suspends_after_timeout() {
        let mut tracker = NestedGuestTracker::default();
        let pane_id = PaneId::Terminal(TerminalId::new(1));
        let start = Instant::now();
        tracker.on_announce(pane_id, start);
        // the timeout is exclusive: a guest exactly at it is still pinged, one past it is not
        assert_eq!(
            tracker.on_tick(pane_id, start + millis(PONG_TIMEOUT_MS)),
            GuestPingTickAction::SendPing
        );
        assert_eq!(
            tracker.on_tick(pane_id, start + millis(PONG_TIMEOUT_MS + 1)),
            GuestPingTickAction::Suspended
        );
        assert!(!tracker.is_tracked(pane_id));
        assert!(tracker.is_dormant(pane_id));
        assert!(tracker.is_known(pane_id));
        assert_eq!(
            tracker.on_tick(pane_id, start + millis(PONG_TIMEOUT_MS + 2)),
            GuestPingTickAction::Unknown
        );
    }

    #[test]
    fn suspended_guest_revives_on_reannounce_however_long_later() {
        let mut tracker = NestedGuestTracker::default();
        let pane_id = PaneId::Terminal(TerminalId::new(1));
        let start = Instant::now();
        tracker.on_announce(pane_id, start);
        assert_eq!(
            tracker.on_tick(pane_id, start + millis(PONG_TIMEOUT_MS + 1)),
            GuestPingTickAction::Suspended
        );
        let much_later = start + millis(PONG_TIMEOUT_MS + 120_000);
        assert_eq!(
            tracker.on_announce(pane_id, much_later),
            AnnounceKind::Revived
        );
        assert!(tracker.is_tracked(pane_id));
        assert!(!tracker.is_dormant(pane_id));
        assert_eq!(
            tracker.on_tick(pane_id, much_later + millis(PING_INTERVAL_MS)),
            GuestPingTickAction::SendPing
        );
    }

    #[test]
    fn reannounce_of_live_guest_is_a_refresh() {
        let mut tracker = NestedGuestTracker::default();
        let pane_id = PaneId::Terminal(TerminalId::new(1));
        let start = Instant::now();
        assert_eq!(tracker.on_announce(pane_id, start), AnnounceKind::New);
        assert_eq!(
            tracker.on_announce(pane_id, start + millis(1000)),
            AnnounceKind::Refresh
        );
        assert!(tracker.is_tracked(pane_id));
    }

    #[test]
    fn pong_refreshes_the_expiry_deadline() {
        let mut tracker = NestedGuestTracker::default();
        let pane_id = PaneId::Terminal(TerminalId::new(1));
        let start = Instant::now();
        tracker.on_announce(pane_id, start);
        tracker.on_pong(pane_id, start + millis(4000));
        assert_eq!(
            tracker.on_tick(pane_id, start + millis(PONG_TIMEOUT_MS + 1000)),
            GuestPingTickAction::SendPing
        );
        assert_eq!(
            tracker.on_tick(pane_id, start + millis(4000 + PONG_TIMEOUT_MS + 1)),
            GuestPingTickAction::Suspended
        );
    }

    #[test]
    fn pong_from_unannounced_pane_is_ignored() {
        let mut tracker = NestedGuestTracker::default();
        let pane_id = PaneId::Terminal(TerminalId::new(1));
        let now = Instant::now();
        tracker.on_pong(pane_id, now);
        assert!(!tracker.is_tracked(pane_id));
        assert_eq!(tracker.on_tick(pane_id, now), GuestPingTickAction::Unknown);
    }

    #[test]
    fn removed_guest_stops_being_tracked() {
        let mut tracker = NestedGuestTracker::default();
        let pane_id = PaneId::Terminal(TerminalId::new(1));
        let now = Instant::now();
        tracker.on_announce(pane_id, now);
        tracker.remove(pane_id);
        assert_eq!(tracker.on_tick(pane_id, now), GuestPingTickAction::Unknown);
    }

    /// Removal forgets a dormant guest too, not just a live one. A dormant entry that
    /// survived removal would be a leak with a consequence: the pane id, once reused, would
    /// re-announce as `Revived` and skip the fresh-guest setup.
    #[test]
    fn removing_a_dormant_guest_forgets_it() {
        let mut tracker = NestedGuestTracker::default();
        let pane_id = PaneId::Terminal(TerminalId::new(1));
        let start = Instant::now();
        tracker.on_announce(pane_id, start);
        assert_eq!(
            tracker.on_tick(pane_id, start + millis(PONG_TIMEOUT_MS + 1)),
            GuestPingTickAction::Suspended
        );
        assert!(tracker.is_dormant(pane_id));
        tracker.remove(pane_id);
        assert!(!tracker.is_dormant(pane_id));
        assert!(!tracker.is_known(pane_id));
        assert_eq!(
            tracker.on_announce(pane_id, start + millis(PONG_TIMEOUT_MS + 2)),
            AnnounceKind::New,
            "a reused pane id is a fresh guest, not a revived one"
        );
    }

    /// Guests are tracked per pane: one going quiet says nothing about another.
    #[test]
    fn suspending_one_guest_leaves_the_other_tracked() {
        let mut tracker = NestedGuestTracker::default();
        let quiet = PaneId::Terminal(TerminalId::new(1));
        let chatty = PaneId::Terminal(TerminalId::new(2));
        let start = Instant::now();
        tracker.on_announce(quiet, start);
        tracker.on_announce(chatty, start);
        tracker.on_pong(chatty, start + millis(PONG_TIMEOUT_MS - 1000));

        let later = start + millis(PONG_TIMEOUT_MS + 1);
        assert_eq!(
            tracker.on_tick(quiet, later),
            GuestPingTickAction::Suspended
        );
        assert!(tracker.is_dormant(quiet));
        assert!(
            tracker.is_tracked(chatty),
            "the other guest was forgotten too"
        );
        assert!(!tracker.is_dormant(chatty));
        assert_eq!(
            tracker.on_tick(chatty, later),
            GuestPingTickAction::SendPing
        );
    }

    #[test]
    fn removing_one_guest_leaves_the_other_tracked() {
        let mut tracker = NestedGuestTracker::default();
        let closed = PaneId::Terminal(TerminalId::new(1));
        let open = PaneId::Terminal(TerminalId::new(2));
        let start = Instant::now();
        tracker.on_announce(closed, start);
        tracker.on_announce(open, start);

        tracker.remove(closed);
        assert!(!tracker.is_known(closed));
        assert!(tracker.is_tracked(open), "the other guest was removed too");
        assert_eq!(
            tracker.on_tick(open, start + millis(1000)),
            GuestPingTickAction::SendPing
        );
        assert_eq!(
            tracker.on_tick(closed, start + millis(1000)),
            GuestPingTickAction::Unknown
        );
    }

    /// The env override wins when it parses as milliseconds and is ignored otherwise. Tested
    /// through the pure helper so no test has to set a process-wide variable that `on_tick`
    /// reads in every other test here.
    #[test]
    fn a_millisecond_override_is_read_from_the_environment() {
        assert_eq!(
            millis_from_env(Some("750".to_owned()), PONG_TIMEOUT_MS),
            750
        );
        assert_eq!(
            millis_from_env(Some(" 12 ".to_owned()), PING_INTERVAL_MS),
            12
        );
        for junk in ["", "soon", "1.5", "-1"] {
            assert_eq!(
                millis_from_env(Some(junk.to_owned()), PONG_TIMEOUT_MS),
                PONG_TIMEOUT_MS,
                "{junk:?} should fall back to the default"
            );
        }
        assert_eq!(millis_from_env(None, PING_INTERVAL_MS), PING_INTERVAL_MS);
        // and the two readers really consult the variables they are named for
        for (var, read, default) in [
            (
                "ZELLIJ_NESTED_PING_INTERVAL_MS",
                ping_interval_ms as fn() -> u64,
                PING_INTERVAL_MS,
            ),
            (
                "ZELLIJ_NESTED_PONG_TIMEOUT_MS",
                pong_timeout_ms as fn() -> u64,
                PONG_TIMEOUT_MS,
            ),
        ] {
            assert!(
                std::env::var_os(var).is_none(),
                "{var} is set in this test's environment, so the default cannot be checked"
            );
            assert_eq!(read(), default, "{var}");
        }
    }
}
