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
use yinz_types::animation::Tween;
use yinz_types::internal_ids::NotificationId;
use yinz_types::internal_ids::TabId;
use yinz_types::pane_size::Viewport;

const TEXT: &str = "build finished";

fn screen_with_a_settled_toast(size: Size, now: Instant) -> Screen {
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, TabId::new(0));
    let mut notification =
        Notification::new(NotificationId::new(0), TEXT.to_owned(), None, None, now);
    notification.reveal = Tween::settled(1.0);
    notification.y_offset = Tween::settled(0.0);
    screen.notifications.push(notification);
    screen
}

/// Every column the toasts cover, across all the damage rects, on the row they share.
fn damaged_columns(damage: &[(Option<TabId>, Vec<CellRect>)]) -> Vec<usize> {
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

    screen
        .dismiss_notification(NotificationId::new(0), start)
        .unwrap();

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
    new_tab(&mut screen, 1, TabId::new(0));
    let surface = screen.surface_size();
    screen.notifications.push(Notification::new(
        NotificationId::new(0),
        TEXT.to_owned(),
        None,
        None,
        start,
    ));

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
            .notification_layout(viewport, now, ClientId::new(1))
            .iter()
            .find(|layout| layout.notification.id == NotificationId::new(0))
            .expect("the first toast is still in the stack")
            .y
    };

    // a second toast arrives, pushing the first away from the corner
    let arrival = start + Duration::from_millis(16);
    screen.with_notification_stack_change(arrival, |screen| {
        screen.notifications.insert(
            0,
            Notification::new(
                NotificationId::new(1),
                "second".to_owned(),
                None,
                None,
                arrival,
            ),
        );
    });

    // ...and a third arrives while the first is still travelling
    let mid_glide = arrival + Duration::from_millis(80);
    let before_the_third = painted_row(&screen, mid_glide);
    screen.with_notification_stack_change(mid_glide, |screen| {
        screen.notifications.insert(
            0,
            Notification::new(
                NotificationId::new(2),
                "third".to_owned(),
                None,
                None,
                mid_glide,
            ),
        );
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
    new_tab(&mut screen, 1, TabId::new(0));
    screen.notifications.push(Notification::new(
        NotificationId::new(0),
        TEXT.to_owned(),
        None,
        None,
        start,
    ));

    assert!(screen.animations_are_live(start), "it is still sliding in");
    assert!(
        !screen.animations_are_live(start + Duration::from_secs(1)),
        "a toast sitting still wants no frames, however long it stays up"
    );

    screen
        .dismiss_notification(NotificationId::new(0), start + Duration::from_secs(1))
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
    early.notifications.push(Notification::new(
        NotificationId::new(0),
        TEXT.to_owned(),
        None,
        None,
        start,
    ));
    let killed_at = start + Duration::from_millis(32);
    early
        .dismiss_notification(NotificationId::new(0), killed_at)
        .unwrap();

    let mut settled = screen_with_a_settled_toast(size, start);
    settled
        .dismiss_notification(NotificationId::new(0), killed_at)
        .unwrap();

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

/// Where the active pane wants this client's cursor, in absolute session coordinates.
/// The regression test for the cursor jumping into the toast.
///
/// A toast repainting on its own leaves every pane under it clean, so the frame's only chunks are
/// the toast's - and `serialize_chunks` writes an absolute goto per chunk with nothing to put the
/// cursor back afterwards. `render_cursor` used to decide whether to emit its goto *before* the
/// toasts had contributed anything, so on exactly this frame it saw a clean output, said nothing,
/// and left the cursor parked at the end of the toast's bottom border.
#[test]
fn a_toast_only_frame_leaves_the_cursor_in_the_active_pane() {
    let size = Size { cols: 80, rows: 20 };
    let start = Instant::now();
    let mut screen = screen_with_a_settled_toast(size, start);
    screen.pinned_now = Some(start);
    let server_receiver = screen_wired_to_server(&mut screen);
    let expected = pane_cursor(&mut screen, crate::ClientId::new(1));

    let mut grid = grid_for_render_snapshot(size);
    // the opening frame paints the panes and the toast over them
    let (cursor, _) = render_frame_and_cursor_into(
        &mut screen,
        &server_receiver,
        crate::ClientId::new(1),
        &mut grid,
    );
    assert_eq!(
        cursor,
        Some(expected),
        "the opening frame places the cursor"
    );

    // ...and on the next one nothing under the toast has changed, so the toast is the only thing
    // in the frame. this is the frame that used to lose the cursor.
    let (cursor, snapshot) = render_frame_and_cursor_into(
        &mut screen,
        &server_receiver,
        crate::ClientId::new(1),
        &mut grid,
    );
    assert!(
        snapshot.contains(TEXT),
        "the toast must still be painting on its own, or this asserts nothing:\n{snapshot}"
    );
    assert_eq!(
        cursor,
        Some(expected),
        "a toast-only frame must leave the cursor where the active pane put it:\n{snapshot}"
    );
}

/// A toast is drawn above every pane, so it occludes the cursor exactly as a floating pane does.
/// Without this the cursor is put back on its own cell - correctly - and then blinks through the
/// box painted over it.
#[test]
fn a_toast_covering_the_cursor_hides_it_until_it_leaves() {
    let size = Size { cols: 80, rows: 20 };
    let start = Instant::now();
    let mut screen = screen_with_a_settled_toast(size, start);
    screen.pinned_now = Some(start);
    let server_receiver = screen_wired_to_server(&mut screen);

    // park the pane's cursor under the toast rather than moving the toast: where a toast settles
    // is the layout's business, and this test is about what happens when the two coincide
    let surface = screen.surface_size();
    let toast = *screen
        .notification_cell_rects(
            Viewport {
                x: 0,
                y: 0,
                cols: surface.cols,
                rows: surface.rows,
            },
            start,
            ClientId::new(1),
        )
        .first()
        .expect("the toast fits");
    let target = (toast.x + 1, toast.y + 1);
    screen
        .get_active_tab_mut(crate::ClientId::new(1))
        .expect("TEST")
        .handle_pty_bytes(
            crate::TerminalId::new(1),
            format!("\u{1b}[{};{}H", target.1 + 1, target.0 + 1)
                .as_bytes()
                .to_vec(),
        )
        .expect("TEST");
    let expected = pane_cursor(&mut screen, crate::ClientId::new(1));
    assert_eq!(
        expected, target,
        "the pane's cursor should now be under the toast"
    );

    let mut grid = grid_for_render_snapshot(size);
    let (cursor, snapshot) = render_frame_and_cursor_into(
        &mut screen,
        &server_receiver,
        crate::ClientId::new(1),
        &mut grid,
    );
    assert_eq!(
        cursor, None,
        "a toast painted over the cursor's cell must hide it:\n{snapshot}"
    );

    // and it comes back the moment the toast is gone
    screen
        .dismiss_notification(NotificationId::new(0), start)
        .unwrap();
    let gone_at = start + Duration::from_secs(1);
    screen.pinned_now = Some(gone_at);
    screen.reap_settled_notifications(gone_at);
    assert!(screen.notifications.is_empty(), "the toast slid out");

    let (cursor, snapshot) = render_frame_and_cursor_into(
        &mut screen,
        &server_receiver,
        crate::ClientId::new(1),
        &mut grid,
    );
    assert_eq!(
        cursor,
        Some(expected),
        "the cursor must come back once the toast uncovers it:\n{snapshot}"
    );
}

#[test]
fn notification_movement_and_expiry_repaint_an_agent_margin() {
    let size = Size {
        cols: 300,
        rows: 24,
    };
    let start = Instant::now();
    let mut screen = screen_with_a_settled_toast(size, start);
    let client = ClientId::new(1);
    let tab = screen.get_active_tab_mut(client).unwrap();
    tab.max_agent_width = 80;
    let id = tab.get_active_pane_id(client).unwrap();
    tab.get_pane_with_id_mut(id)
        .unwrap()
        .set_detected_agent(true);
    screen.pinned_now = Some(start);
    let receiver = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(size);
    let initial = render_frame_into(&mut screen, &receiver, client, &mut grid);
    assert!(initial.contains(TEXT));
    let pane = screen
        .get_active_tab(client)
        .unwrap()
        .get_pane_with_id(id)
        .unwrap()
        .render_geom();
    let viewport = Viewport {
        x: 0,
        y: 0,
        cols: size.cols,
        rows: size.rows,
    };
    let old = screen.notification_cell_rects(viewport, start, ClientId::new(1));
    assert!(old.iter().all(|rect| !rect.intersects_geom(&pane)));
    screen.notifications[0].y_offset = Tween::settled(5.0);
    let moved = render_frame_into(&mut screen, &receiver, client, &mut grid);
    assert!(moved.contains(TEXT));
    assert_eq!(moved.matches(TEXT).count(), 1, "{moved}");
    // Compare the accumulated terminal to a fresh repaint to catch border trails as well as text.
    screen
        .get_active_tab_mut(client)
        .unwrap()
        .set_should_clear_display_before_rendering();
    screen
        .get_active_tab_mut(client)
        .unwrap()
        .set_force_render();
    let fresh = render_frame_into(&mut screen, &receiver, client, &mut grid);
    assert_eq!(moved, fresh);
    screen
        .dismiss_notification(NotificationId::new(0), start)
        .unwrap();
    let gone = start + Duration::from_secs(1);
    screen.pinned_now = Some(gone);
    screen.reap_settled_notifications(gone);
    let expired = render_frame_into(&mut screen, &receiver, client, &mut grid);
    assert!(!expired.contains(TEXT));
    screen
        .get_active_tab_mut(client)
        .unwrap()
        .set_should_clear_display_before_rendering();
    screen
        .get_active_tab_mut(client)
        .unwrap()
        .set_force_render();
    let fresh = render_frame_into(&mut screen, &receiver, client, &mut grid);
    assert_eq!(expired, fresh);
}
