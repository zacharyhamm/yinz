//! What the shared animation clock is asked to do, and when.
//!
//! These assert on the background jobs a `Screen` sends rather than on pixels: the clock's whole
//! job is to decide *how often* a frame is wanted and to stop asking once nothing is moving, and
//! both of those are decisions rather than drawings.

use super::*;
use crate::screen::Notification;
use crate::ui::loading_indication::DOT_PERIOD;
use crate::ui::notification::EXIT_DURATION;
use std::time::{Duration, Instant};
use yinz_types::animation::{Tween, FRAME_PERIOD};
use yinz_types::internal_ids::NotificationId;
use yinz_types::internal_ids::PluginId;
use yinz_types::internal_ids::TabId;

const TEXT: &str = "build finished";

fn screen_with_a_settled_toast(
    size: Size,
    now: Instant,
) -> (Screen, Receiver<(BackgroundJob, ErrorContext)>) {
    let (mut screen, background_jobs) = screen_with_background_jobs(size);
    new_tab(&mut screen, 1, TabId::new(0));
    let mut notification =
        Notification::new(NotificationId::new(0), TEXT.to_owned(), None, None, now);
    notification.reveal = Tween::settled(1.0);
    notification.y_offset = Tween::settled(0.0);
    screen.notifications.push(notification);
    (screen, background_jobs)
}

fn started_clock_periods(jobs: &[BackgroundJob]) -> Vec<Duration> {
    jobs.iter()
        .filter_map(|job| match job {
            BackgroundJob::StartAnimationClock(period) => Some(*period),
            _ => None,
        })
        .collect()
}

/// The regression test for a toast left painted a sliver short of gone.
///
/// Renders are debounced, so the frame a still-animating tick asks for is composited *before* the
/// deadline it was aiming at - with a 160ms `EaseIn` exit, a frame a few milliseconds early still
/// has a tenth of the toast's columns on screen. The tick that finds everything settled has to ask
/// for one more frame, or nothing ever paints the settled state and nothing ever reaps the toast.
#[test]
fn the_tick_that_finds_everything_settled_still_asks_for_a_frame() {
    let size = Size { cols: 80, rows: 20 };
    let start = Instant::now();
    let (mut screen, background_jobs) = screen_with_a_settled_toast(size, start);

    screen
        .dismiss_notification(NotificationId::new(0), start)
        .unwrap();
    assert_eq!(
        started_clock_periods(&drain_background_jobs(&background_jobs)),
        vec![FRAME_PERIOD],
        "a slide-out runs at the frame period"
    );

    let settled = start + EXIT_DURATION;
    assert!(
        !screen.animations_are_live(settled),
        "the exit is over by now - this is the tick that used to go quiet without painting"
    );
    screen.on_animation_tick(settled).unwrap();

    let jobs = drain_background_jobs(&background_jobs);
    assert!(
        jobs.contains(&BackgroundJob::RenderToClients),
        "the settling tick asks for the frame that paints the settled state: {jobs:?}"
    );
    assert!(
        jobs.contains(&BackgroundJob::StopAnimationClock),
        "and only then goes quiet: {jobs:?}"
    );

    screen.reap_settled_notifications(settled);
    assert!(
        screen.notifications.is_empty(),
        "and that frame is the one that drops the toast from the stack"
    );
}

/// The fix is "one more frame", not "a frame instead of": the ticks during the animation still ask
/// for theirs, and they do not re-ask the clock for the period it is already running at.
#[test]
fn a_tick_mid_animation_asks_for_a_frame_and_leaves_the_clock_alone() {
    let size = Size { cols: 80, rows: 20 };
    let start = Instant::now();
    let (mut screen, background_jobs) = screen_with_a_settled_toast(size, start);

    screen
        .dismiss_notification(NotificationId::new(0), start)
        .unwrap();
    let _ = drain_background_jobs(&background_jobs);

    let mid_exit = start + EXIT_DURATION / 2;
    assert!(screen.animations_are_live(mid_exit), "still sliding out");
    screen.on_animation_tick(mid_exit).unwrap();

    let jobs = drain_background_jobs(&background_jobs);
    assert!(jobs.contains(&BackgroundJob::RenderToClients));
    assert_eq!(
        jobs.iter()
            .filter(|job| matches!(
                job,
                BackgroundJob::StartAnimationClock(_) | BackgroundJob::StopAnimationClock
            ))
            .count(),
        0,
        "the clock is already doing the right thing, so nothing is said to it: {jobs:?}"
    );
}

/// The dots change four times a second. Holding the clock at 60fps for them means a full
/// `render_to_clients` sixty times a second for as long as any plugin is loading - forever, for a
/// plugin that never finishes.
#[test]
fn a_loading_plugin_wakes_the_clock_at_the_dot_period() {
    let size = Size { cols: 80, rows: 20 };
    let now = Instant::now();
    let (mut screen, background_jobs) = screen_with_background_jobs(size);

    screen
        .animating_plugin_loads
        .insert(crate::PluginId::new(1));
    screen.sync_animation_clock(now).unwrap();

    assert_eq!(screen.animation_clock_period(now), Some(DOT_PERIOD));
    assert_eq!(
        started_clock_periods(&drain_background_jobs(&background_jobs)),
        vec![DOT_PERIOD],
    );
    assert!(
        DOT_PERIOD > FRAME_PERIOD * 4,
        "and that is a good deal coarser than the frame period, which is the entire point"
    );
}

/// The cadence is aggregated by the screen rather than requested per surface, so a plugin load
/// starting mid-slide cannot drop the clock to the dot period underneath a moving toast.
#[test]
fn a_plugin_load_cannot_slow_the_clock_under_a_moving_toast() {
    let size = Size { cols: 80, rows: 20 };
    let start = Instant::now();
    let (mut screen, _background_jobs) = screen_with_background_jobs(size);
    new_tab(&mut screen, 1, TabId::new(0));
    screen.notifications.push(Notification::new(
        NotificationId::new(0),
        TEXT.to_owned(),
        None,
        None,
        start,
    ));
    screen
        .animating_plugin_loads
        .insert(crate::PluginId::new(1));

    assert_eq!(
        screen.animation_clock_period(start),
        Some(FRAME_PERIOD),
        "the toast is sliding in, so the coarsest cadence anything can live with is the fast one"
    );

    let toast_settled = start + Duration::from_secs(1);
    assert_eq!(
        screen.animation_clock_period(toast_settled),
        Some(DOT_PERIOD),
        "once it stops moving, only the dots are left"
    );

    screen.animating_plugin_loads.remove(&PluginId::new(1));
    assert_eq!(
        screen.animation_clock_period(toast_settled),
        None,
        "and then nothing wants a frame at all"
    );
}

/// A tick is a wakeup, not a conversation: the clock is only told anything when what it should be
/// doing has actually changed.
#[test]
fn quiet_ticks_put_nothing_but_the_frame_on_the_bus() {
    let size = Size { cols: 80, rows: 20 };
    let start = Instant::now();
    let (mut screen, background_jobs) = screen_with_background_jobs(size);
    new_tab(&mut screen, 1, TabId::new(0));

    screen
        .animating_plugin_loads
        .insert(crate::PluginId::new(1));
    screen.sync_animation_clock(start).unwrap();
    let _ = drain_background_jobs(&background_jobs);

    for tick in 1..=10 {
        screen.on_animation_tick(start + DOT_PERIOD * tick).unwrap();
    }

    let jobs = drain_background_jobs(&background_jobs);
    assert_eq!(
        jobs.iter()
            .filter(|job| **job == BackgroundJob::RenderToClients)
            .count(),
        10,
        "one frame per tick"
    );
    assert!(
        started_clock_periods(&jobs).is_empty()
            && !jobs.contains(&BackgroundJob::StopAnimationClock),
        "and not one word to the clock, which is already running at the right period: {jobs:?}"
    );
}

#[test]
fn agent_double_escape_timeout_repaints_and_stops_the_clock() {
    let start = Instant::now();
    let (mut screen, background_jobs) = screen_with_background_jobs(Size { cols: 80, rows: 20 });
    new_tab(&mut screen, 1, TabId::new(0));
    screen
        .tabs
        .values_mut()
        .next()
        .unwrap()
        .pending_agent_escapes
        .insert(
            (
                ClientId::new(1),
                PaneId::Terminal(crate::TerminalId::new(1)),
            ),
            start,
        );
    screen.sync_animation_clock(start).unwrap();
    assert_eq!(
        started_clock_periods(&drain_background_jobs(&background_jobs)),
        vec![DOT_PERIOD]
    );
    screen
        .on_animation_tick(start + Duration::from_millis(999))
        .unwrap();
    assert!(screen.animations_are_live(start));
    drain_background_jobs(&background_jobs);
    screen
        .on_animation_tick(start + Duration::from_secs(1))
        .unwrap();
    assert!(screen
        .tabs
        .values()
        .all(|tab| tab.pending_agent_escapes.is_empty()));
    assert!(!screen.animations_are_live(start + Duration::from_secs(1)));
    let jobs = drain_background_jobs(&background_jobs);
    assert!(jobs.contains(&BackgroundJob::RenderToClients));
    assert!(jobs.contains(&BackgroundJob::StopAnimationClock));
}
