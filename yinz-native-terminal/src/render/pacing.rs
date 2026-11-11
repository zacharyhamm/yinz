use std::time::{Duration, Instant};

const FALLBACK_REFRESH_MILLIHERTZ: u32 = 60_000;
const SLOWEST_REFRESH_MILLIHERTZ: u32 = 20_000;
const FASTEST_REFRESH_MILLIHERTZ: u32 = 500_000;

pub fn interval_of(millihertz: Option<u32>) -> Duration {
    let millihertz = match millihertz {
        Some(rate) if (SLOWEST_REFRESH_MILLIHERTZ..=FASTEST_REFRESH_MILLIHERTZ).contains(&rate) => {
            rate
        },
        _ => FALLBACK_REFRESH_MILLIHERTZ,
    };
    Duration::from_nanos(1_000_000_000_000 / millihertz as u64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Idle,
    Now,
    At(Instant),
}

pub struct Pacer {
    interval: Duration,
    last_draw: Option<Instant>,
    pending: bool,
}

impl Pacer {
    pub fn new(millihertz: Option<u32>) -> Self {
        Self {
            interval: interval_of(millihertz),
            last_draw: None,
            pending: false,
        }
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    pub fn follow_refresh(&mut self, millihertz: Option<u32>) {
        self.interval = interval_of(millihertz);
    }

    pub fn schedule(&mut self) {
        self.pending = true;
    }

    pub fn drawn(&mut self, at: Instant) {
        self.pending = false;
        self.last_draw = Some(at);
    }

    pub fn decide(&self, now: Instant) -> Decision {
        if !self.pending {
            return Decision::Idle;
        }
        let Some(last) = self.last_draw else {
            return Decision::Now;
        };
        let due = last + self.interval;
        if now >= due {
            Decision::Now
        } else {
            Decision::At(due)
        }
    }
}

#[cfg(test)]
#[path = "unit/pacing_tests.rs"]
mod tests;
