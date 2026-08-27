//! The animation properties toasts are supposed to have, asserted on a real `Screen`.
//!
//! These drive time by hand rather than by sleeping: `Screen::pinned_now` fixes what the frame
//! samples at, so a whole slide-out can be stepped frame by frame and the result is the same on a
//! loaded machine as on an idle one.

use super::*;
use crate::screen::Notification;
use crate::ui::damage::CellRect;
use crate::ui::notification::{notification_height, notification_lines, notification_width};
use std::time::{Duration, Instant};
use zellij_utils::animation::Tween;
use zellij_utils::pane_size::Viewport;

const TEXT: &str = "build finished";

fn screen_with_a_settled_toast(size: Size, now: Instant) -> Screen {
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    let mut notification = Notification::new(0, TEXT.to_owned(), None, now);
    notification.reveal = Tween::settled(1.0);
    notification.y_offset = Tween::settled(0.0);
    screen.notifications.push(notification);
    screen
}

/// Every column the toasts cover, across all the damage rects, on the row they share.
fn damaged_columns(damage: &[(Option<usize>, Vec<CellRect>)]) -> Vec<usize> {
    let mut columns: Vec<usize> = damage
        .iter()
        .flat_map(|(_, rects)| rects)
        .flat_map(|rect| rect.x..rect.x + rect.cols)
        .collect();
    columns.sort_unstable();
    columns.dedup();
    columns
}

/// The regression test for the slide-out smear: on every frame a toast gives up columns, the cells
/// it gave up must be reported as damage *that frame*, not once at the end when it is finally
/// removed. Before this, a dismissing toast left a trail of its own border behind it that only
/// cleared in one flash when the toast disappeared.
#[test]
fn every_frame_of_a_slide_out_reports_the_columns_it_just_uncovered() {
    let size = Size { cols: 80, rows: 20 };
    let start = Instant::now();
    let mut screen = screen_with_a_settled_toast(size, start);
    let surface = screen.surface_size();
    let width = notification_width(TEXT, size.cols).expect("the toast fits");

    // settle the cache on the fully-revealed toast
    screen.pinned_now = Some(start);
    let damage = screen.prepare_notification_damage(start, surface);
    assert!(
        damaged_columns(&damage).is_empty(),
        "a toast that has not moved damages nothing"
    );

    screen.dismiss_notification(0, start).unwrap();

    let mut uncovered_so_far = 0;
    let mut frames_with_damage = 0;
    for frame in 1..=20 {
        let now = start + Duration::from_millis(frame * 16);
        screen.pinned_now = Some(now);
        let damage = screen.prepare_notification_damage(now, surface);
        let columns = damaged_columns(&damage);
        if columns.is_empty() {
            continue;
        }
        frames_with_damage += 1;
        uncovered_so_far += columns.len();
        assert!(
            uncovered_so_far <= width,
            "a toast cannot uncover more columns than it ever covered"
        );
    }

    assert!(
        frames_with_damage > 3,
        "a slide-out should uncover its columns over many frames, not in one jump - got {frames_with_damage}"
    );
    assert_eq!(
        uncovered_so_far, width,
        "by the end, every column the toast covered has been handed back for repainting"
    );
}

/// The other half of the same guarantee: arriving is free. A toast that is only ever growing never
/// uncovers anything, so no pane under it is asked to repaint.
#[test]
fn a_toast_sliding_in_never_asks_for_a_repaint() {
    let size = Size { cols: 80, rows: 20 };
    let start = Instant::now();
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    let surface = screen.surface_size();
    screen
        .notifications
        .push(Notification::new(0, TEXT.to_owned(), None, start));

    for frame in 0..=20 {
        let now = start + Duration::from_millis(frame * 16);
        screen.pinned_now = Some(now);
        let damage = screen.prepare_notification_damage(now, surface);
        assert!(
            damaged_columns(&damage).is_empty(),
            "frame {frame} of an entrance should cost no repaints"
        );
    }
}

/// The regression test for the mid-glide snap: a toast arriving while the stack is still settling
/// used to recompute the survivors' "before" positions as *settled* ones, so they jumped from
/// wherever they were being drawn to a row they had never been at. Now each survivor's
/// displacement is re-aimed from its current value, so the painted row moves continuously.
#[test]
fn a_second_toast_arriving_mid_glide_never_jumps_the_survivors() {
    let size = Size { cols: 80, rows: 20 };
    let start = Instant::now();
    let mut screen = screen_with_a_settled_toast(size, start);
    let viewport = screen.surface_size();
    let viewport = Viewport {
        x: 0,
        y: 0,
        cols: viewport.cols,
        rows: viewport.rows,
    };

    let painted_row = |screen: &Screen, now: Instant| -> usize {
        screen
            .notification_layout(viewport, now)
            .iter()
            .find(|layout| layout.notification.id == 0)
            .expect("the first toast is still in the stack")
            .y
    };

    // a second toast arrives, pushing the first away from the corner
    let arrival = start + Duration::from_millis(16);
    screen.with_notification_stack_change(arrival, |screen| {
        screen
            .notifications
            .insert(0, Notification::new(1, "second".to_owned(), None, arrival));
    });

    // ...and a third arrives while the first is still travelling
    let mid_glide = arrival + Duration::from_millis(80);
    let before_the_third = painted_row(&screen, mid_glide);
    screen.with_notification_stack_change(mid_glide, |screen| {
        screen
            .notifications
            .insert(0, Notification::new(2, "third".to_owned(), None, mid_glide));
    });
    let after_the_third = painted_row(&screen, mid_glide);
    assert_eq!(
        before_the_third, after_the_third,
        "a toast arriving must not move anything on the frame it arrives"
    );

    // and from there the row only ever walks, one cell at a time, to where it belongs
    let mut previous = after_the_third;
    for frame in 1..=40 {
        let now = mid_glide + Duration::from_millis(frame * 16);
        let current = painted_row(&screen, now);
        assert!(
            current.abs_diff(previous) <= 1,
            "the painted row jumped from {previous} to {current} on frame {frame}"
        );
        previous = current;
    }

    let heights: Vec<usize> = screen
        .notifications
        .iter()
        .map(|notification| {
            let width = notification_width(&notification.text, size.cols).unwrap();
            notification_height(notification_lines(&notification.text, width).len())
        })
        .collect();
    assert_eq!(
        previous,
        heights[0] + heights[1],
        "and it ends up exactly where two toasts' worth of stack puts it"
    );
}

/// The clock has to stop on its own, or an idle session pays for 60 wakeups a second forever.
#[test]
fn animations_stop_being_live_once_everything_has_settled() {
    let size = Size { cols: 80, rows: 20 };
    let start = Instant::now();
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    screen
        .notifications
        .push(Notification::new(0, TEXT.to_owned(), None, start));

    assert!(screen.animations_are_live(start), "it is still sliding in");
    assert!(
        !screen.animations_are_live(start + Duration::from_secs(1)),
        "a toast sitting still wants no frames, however long it stays up"
    );

    screen
        .dismiss_notification(0, start + Duration::from_secs(1))
        .unwrap();
    assert!(screen.animations_are_live(start + Duration::from_secs(1)));

    let settled = start + Duration::from_secs(2);
    screen.reap_settled_notifications(settled);
    assert!(
        screen.notifications.is_empty(),
        "it slid out and was reaped"
    );
    assert!(!screen.animations_are_live(settled));
}

/// A toast killed before it finished arriving should leave in proportion to how much of it is
/// showing, rather than snapping to full width and sliding out from there.
#[test]
fn a_toast_dismissed_before_it_arrived_leaves_sooner_than_a_settled_one() {
    let size = Size { cols: 80, rows: 20 };
    let start = Instant::now();

    let mut early = create_non_mirrored_screen(size);
    early
        .notifications
        .push(Notification::new(0, TEXT.to_owned(), None, start));
    let killed_at = start + Duration::from_millis(32);
    early.dismiss_notification(0, killed_at).unwrap();

    let mut settled = screen_with_a_settled_toast(size, start);
    settled.dismiss_notification(0, killed_at).unwrap();

    let gone_by = |screen: &mut Screen| -> u64 {
        (1..=60)
            .find(|frame| {
                let now = killed_at + Duration::from_millis(frame * 8);
                screen.reap_settled_notifications(now);
                screen.notifications.is_empty()
            })
            .expect("it leaves within half a second either way")
    };

    let early_frames = gone_by(&mut early);
    let settled_frames = gone_by(&mut settled);
    assert!(
        early_frames < settled_frames,
        "a barely-arrived toast should leave sooner: {early_frames} vs {settled_frames} frames"
    );
}
