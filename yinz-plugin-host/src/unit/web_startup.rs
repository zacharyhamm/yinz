use super::*;
fn fixture() -> (Startup, Instant) {
    let mut startup = Startup::new(Default::default());
    let now = Instant::now();
    startup.begin(PluginId::new(4), 1, now);
    (startup, now)
}
#[test]
fn web_startup_retry_boundaries_exhaustion_and_terminal_failures() {
    let (mut startup, mut now) = fixture();
    for (attempt, delay) in [1, 5, 30].into_iter().enumerate() {
        startup.fail(Some(attempt as u64 + 1), None, Failure::Initialization, now);
        assert_eq!(startup.status().phase, Phase::Retrying);
        assert_eq!(
            startup.pulse(now + Duration::from_secs(delay) - Duration::from_nanos(1)),
            Action::None
        );
        now += Duration::from_secs(delay);
        assert_eq!(startup.pulse(now), Action::Start);
        startup.begin(PluginId::new(4), attempt as u64 + 2, now);
    }
    startup.fail(Some(4), None, Failure::Initialization, now);
    assert_eq!(startup.status().phase, Phase::Failed);
    assert_eq!(startup.status().attempts, 4);
    assert_eq!(startup.pulse(now + Duration::from_secs(1000)), Action::None);
    for failure in [Failure::Configuration, Failure::PermissionDenied] {
        startup.reset();
        startup.start_failed(failure, now);
        assert_eq!(startup.status().phase, Phase::Failed);
        assert_eq!(startup.status().retry_after_s, None);
    }
}
#[test]
fn web_startup_deadline_permission_pause_and_stopped_epochs() {
    let (mut startup, now) = fixture();
    startup.permission(true, now + Duration::from_secs(20));
    assert_eq!(startup.status().phase, Phase::PermissionBlocked);
    assert_eq!(startup.pulse(now + Duration::from_secs(200)), Action::None);
    startup.permission(false, now + Duration::from_secs(200));
    startup.pause(true, now + Duration::from_secs(210));
    assert_eq!(startup.status().phase, Phase::Paused);
    startup.pause(false, now + Duration::from_secs(310));
    assert_eq!(startup.pulse(now + Duration::from_secs(339)), Action::None);
    assert_eq!(
        startup.pulse(now + Duration::from_secs(340)),
        Action::Expire
    );
    startup.stop();
    startup.fail(Some(1), None, Failure::Initialization, now);
    startup.refresh(Some(99), true, now);
    assert_eq!(startup.status().phase, Phase::Stopped);
    assert_eq!(startup.pulse(now + Duration::from_secs(1000)), Action::None);
}
#[test]
fn web_startup_ready_requires_publication_and_stability_resets_budget() {
    let (mut startup, now) = fixture();
    startup.fail(Some(1), None, Failure::Initialization, now);
    startup.begin(PluginId::new(4), 2, now + Duration::from_secs(1));
    startup.refresh(Some(8), false, now + Duration::from_secs(2));
    assert_eq!(startup.status().phase, Phase::Loading);
    startup.refresh(Some(8), true, now + Duration::from_secs(3));
    assert_eq!(startup.status().phase, Phase::Ready);
    assert_eq!(startup.status().failure, None);
    startup.fail(
        Some(1),
        Some(7),
        Failure::Initialization,
        now + Duration::from_secs(4),
    );
    assert_eq!(startup.status().phase, Phase::Ready);
    startup.refresh(Some(8), true, now + Duration::from_secs(62));
    assert_eq!(startup.status().attempts, 2);
    startup.refresh(Some(8), true, now + Duration::from_secs(63));
    assert_eq!(startup.status().attempts, 1);
    startup.refresh(None, false, now + Duration::from_secs(64));
    assert_eq!(startup.status().phase, Phase::Retrying);
    assert_eq!(startup.status().retry_after_s, Some(1));
}
