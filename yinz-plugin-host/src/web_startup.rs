//! Bounded configured-app recovery. All time inputs are monotonic and injectable in tests.
use std::time::{Duration, Instant};
use yinz_types::{
    internal_ids::PluginId,
    web::{
        ApplicationFailure as Failure, ApplicationHandle, ApplicationPhase as Phase,
        ApplicationStatus,
    },
};

const ACTIVE_DEADLINE: Duration = Duration::from_secs(60);
const BACKOFF: [u32; 3] = [1, 5, 30];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    None,
    Start,
    Expire,
}
#[derive(Debug)]
pub(super) struct Startup {
    handle: ApplicationHandle,
    status: ApplicationStatus,
    admitted: Option<Instant>,
    retry: Option<Instant>,
    stable: Option<Instant>,
    pause: Option<Instant>,
    permission: Option<Instant>,
    observed: Instant,
    active: Instant,
}
impl Startup {
    pub fn new(handle: ApplicationHandle) -> Self {
        let startup = Self {
            handle,
            status: ApplicationStatus {
                phase: Phase::Loading,
                ..Default::default()
            },
            admitted: None,
            retry: None,
            stable: None,
            pause: None,
            permission: None,
            observed: Instant::now(),
            active: Instant::now(),
        };
        startup.publish();
        startup
    }
    fn clock(&mut self, wall: Instant) -> Instant {
        if self.pause.is_none() && self.permission.is_none() {
            self.active += wall.saturating_duration_since(self.observed);
        }
        self.observed = wall;
        self.active
    }
    fn publish(&self) {
        let mut status = self.status.clone();
        if self.pause.is_some() {
            status.phase = Phase::Paused;
        } else if self.permission.is_some() {
            status.phase = Phase::PermissionBlocked;
        }
        self.handle.set(status);
    }
    pub fn status(&self) -> ApplicationStatus {
        self.handle.current()
    }
    pub fn stopped(&self) -> bool {
        self.status.phase == Phase::Stopped
    }
    pub fn reset(&mut self) {
        self.status = ApplicationStatus {
            phase: Phase::Loading,
            ..Default::default()
        };
        self.admitted = None;
        self.retry = None;
        self.stable = None;
        self.permission = None;
        self.publish();
    }
    pub fn stop(&mut self) {
        self.status.phase = Phase::Stopped;
        self.status.load = None;
        self.status.generation = None;
        self.status.retry_after_s = None;
        self.status.retry_not_before_ms = None;
        self.retry = None;
        self.admitted = None;
        self.stable = None;
        self.permission = None;
        self.publish();
    }
    pub fn reserve(&mut self, id: PluginId) {
        self.status.target = Some(id);
        self.publish();
    }
    pub fn begin(&mut self, id: PluginId, load: u64, now: Instant) {
        let now = self.clock(now);
        if self.stopped() {
            return;
        }
        self.status.target = Some(id);
        self.status.load = Some(load);
        self.status.generation = None;
        self.status.publication_rejection = None;
        self.status.attempts = self.status.attempts.saturating_add(1);
        self.status.phase = Phase::Loading;
        self.status.retry_after_s = None;
        self.status.retry_not_before_ms = None;
        self.admitted = Some(now);
        self.retry = None;
        self.stable = None;
        self.publish();
    }
    pub fn start_failed(&mut self, failure: Failure, now: Instant) {
        let now = self.clock(now);
        if self.stopped() {
            return;
        }
        // Even failure before load admission consumes an attempt.
        self.status.attempts = self.status.attempts.saturating_add(1);
        self.failure(failure, now);
    }
    pub fn fail(
        &mut self,
        load: Option<u64>,
        generation: Option<u64>,
        failure: Failure,
        now: Instant,
    ) {
        let now = self.clock(now);
        if self.stopped()
            || load.is_some_and(|l| self.status.load != Some(l))
            || generation.is_some_and(|g| self.status.generation != Some(g))
            || matches!(self.status.phase, Phase::Retrying | Phase::Failed)
        {
            return;
        }
        self.failure(failure, now);
    }
    fn failure(&mut self, failure: Failure, now: Instant) {
        self.status.failure = Some(failure);
        self.stable = None;
        self.admitted = None;
        self.permission = None;
        let delay = if matches!(failure, Failure::Configuration | Failure::PermissionDenied) {
            None
        } else {
            self.status
                .attempts
                .checked_sub(1)
                .and_then(|i| BACKOFF.get(i as usize))
                .copied()
        };
        self.status.phase = if delay.is_some() {
            Phase::Retrying
        } else {
            Phase::Failed
        };
        self.status.retry_after_s = delay;
        self.status.retry_not_before_ms =
            delay.map(|s| Self::wall_ms().saturating_add(u64::from(s) * 1000));
        self.retry = delay.map(|s| now + Duration::from_secs(u64::from(s)));
        self.publish();
    }
    pub fn refresh(&mut self, generation: Option<u64>, published: bool, now: Instant) {
        let now = self.clock(now);
        if self.stopped() || matches!(self.status.phase, Phase::Retrying | Phase::Failed) {
            return;
        }
        if let Some(generation) = generation {
            self.status.generation = Some(generation);
        }
        if self.pause.is_some() || self.permission.is_some() {
            self.publish();
            return;
        }
        if published {
            if self.status.phase != Phase::Ready {
                self.status.phase = Phase::Ready;
                self.status.failure = None;
                self.status.publication_rejection = None;
                self.stable = Some(now);
                self.admitted = None;
            }
            if self
                .stable
                .is_some_and(|since| now.saturating_duration_since(since) >= ACTIVE_DEADLINE)
            {
                self.status.attempts = 1;
            }
        } else if self.status.phase == Phase::Ready {
            self.failure(Failure::Initialization, now);
        }
        self.publish();
    }
    pub fn pulse(&mut self, now: Instant) -> Action {
        let now = self.clock(now);
        if self.pause.is_some() || self.permission.is_some() || self.stopped() {
            return Action::None;
        }
        match self.status.phase {
            Phase::Loading if self.status.attempts == 0 => Action::Start,
            Phase::Loading
                if self.admitted.is_some_and(|since| {
                    now.saturating_duration_since(since) >= ACTIVE_DEADLINE
                }) =>
            {
                self.failure(Failure::PublicationDeadline, now);
                Action::Expire
            },
            Phase::Retrying if self.retry.is_some_and(|due| now >= due) => Action::Start,
            _ => Action::None,
        }
    }
    fn wall_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }
    pub fn publication_rejected(&mut self, rejection: yinz_types::web::PublicationRejection) {
        self.status.publication_rejection = Some(rejection);
        self.publish();
    }
    pub fn pause(&mut self, paused: bool, now: Instant) {
        let active = self.clock(now);
        if paused {
            self.pause.get_or_insert(now);
        } else {
            self.pause = None;
            if let Some(due) = self.retry {
                self.status.retry_not_before_ms = Some(
                    Self::wall_ms().saturating_add(
                        due.saturating_duration_since(active)
                            .as_millis()
                            .try_into()
                            .unwrap_or(u64::MAX),
                    ),
                );
            }
        }
        self.publish();
    }
    pub fn permission(&mut self, waiting: bool, now: Instant) {
        self.clock(now);
        // Inspect the underlying phase: the public snapshot may currently be Paused.
        // A remaining prompt must not mask a terminal denial after update resume.
        if waiting
            && matches!(
                self.status.phase,
                Phase::Stopped | Phase::Failed | Phase::Retrying
            )
        {
            return;
        }
        if waiting {
            self.permission.get_or_insert(now);
        } else {
            self.permission = None;
        }
        self.publish();
    }
}
#[cfg(test)]
#[path = "unit/web_startup.rs"]
mod tests;
