//! Synchronized output (DECSET 2026): a frame is held back until it ends, a reset ends it, and
//! a frame that never ends lapses after `SYNCHRONIZED_OUTPUT_TIMEOUT`.
use super::*;
use crate::panes::grid::SYNCHRONIZED_OUTPUT_TIMEOUT;
use std::time::{Duration, Instant};

/// The text of a render, or `None` if the grid held it back.
fn rendered_text(grid: &mut Grid) -> Option<String> {
    let (chunks, ..) = grid.render(0, 0, &Style::default()).unwrap()?;
    Some(
        chunks
            .iter()
            .flat_map(|chunk| chunk.terminal_characters.iter().map(|c| c.character))
            .collect(),
    )
}

/// A grid that has rendered "completed", then began a frame and drew "partial" in it.
fn grid_mid_frame() -> Grid {
    let mut grid = create_grid_with_size_and_raw(4, 20, b"completed");
    assert!(rendered_text(&mut grid).unwrap().contains("completed"));
    feed_bytes(&mut grid, b"\x1b[?2026h\x1b[H\x1b[2Kpartial");
    assert!(grid.is_mid_frame());
    grid
}

#[test]
fn a_synchronized_frame_is_held_back_until_it_ends() {
    let mut grid = grid_mid_frame();
    assert_eq!(rendered_text(&mut grid), None);
    grid.render_full_viewport();
    let repaint = rendered_text(&mut grid).unwrap();
    assert!(repaint.contains("completed"), "{repaint:?}");
    assert!(!repaint.contains("partial"), "{repaint:?}");
    feed_bytes(&mut grid, b" frame\x1b[?2026l");
    assert!(!grid.is_mid_frame());
    assert_eq!(grid.sync_output_deadline(), None);
    let finished = rendered_text(&mut grid).unwrap();
    assert!(finished.contains("partial frame"), "{finished:?}");
}

#[test]
fn a_full_reset_ends_a_synchronized_frame() {
    let mut grid = grid_mid_frame();
    feed_bytes(&mut grid, b"\x1bcafter reset");
    assert!(!grid.is_mid_frame());
    assert_eq!(grid.sync_output_deadline(), None);
    let text = rendered_text(&mut grid).unwrap();
    assert!(text.contains("after reset"), "{text:?}");
    // A forced repaint shows the live screen, not the frame completed before the reset.
    grid.render_full_viewport();
    let repaint = rendered_text(&mut grid).unwrap();
    assert!(repaint.contains("after reset"), "{repaint:?}");
    assert!(!repaint.contains("completed"), "{repaint:?}");
}

#[test]
fn a_reset_that_keeps_scrollback_ends_a_synchronized_frame() {
    let mut grid = grid_mid_frame();
    grid.reset_terminal_state_preserving_scrollback();
    assert!(!grid.is_mid_frame());
    assert_eq!(grid.sync_output_deadline(), None);
    feed_bytes(&mut grid, b"respawned");
    let text = rendered_text(&mut grid).unwrap();
    assert!(text.contains("respawned"), "{text:?}");
}

#[test]
fn a_synchronized_frame_that_never_ends_lapses_after_the_timeout() {
    let before = Instant::now();
    let mut grid = grid_mid_frame();
    let deadline = grid.sync_output_deadline().unwrap();
    assert!(deadline >= before + SYNCHRONIZED_OUTPUT_TIMEOUT);
    assert!(deadline <= Instant::now() + SYNCHRONIZED_OUTPUT_TIMEOUT);
    let just_before = deadline - Duration::from_millis(1);
    assert!(!grid.expire_sync_output(just_before));
    assert_eq!(grid.sync_output_deadline(), Some(deadline));
    grid.should_render = false;
    assert!(grid.expire_sync_output(deadline));
    assert!(grid.should_render, "a lapsed lock asks for a render");
    assert!(!grid.is_mid_frame());
    assert_eq!(grid.sync_output_deadline(), None);
    assert!(!grid.expire_sync_output(deadline));
    let text = rendered_text(&mut grid).unwrap();
    assert!(text.contains("partial"), "{text:?}");
}

#[test]
fn rendering_releases_a_lapsed_lock_by_itself() {
    let mut grid = grid_mid_frame();
    grid.render_lock_deadline = Some(Instant::now());
    assert!(!grid.is_mid_frame());
    let text = rendered_text(&mut grid).unwrap();
    assert!(text.contains("partial"), "{text:?}");
    assert_eq!(grid.sync_output_deadline(), None);
}

#[test]
fn repeating_the_begin_marker_keeps_the_original_deadline() {
    let mut grid = grid_mid_frame();
    // Well inside the timeout, and earlier than any deadline a fresh lock would set.
    let pinned = Instant::now() + SYNCHRONIZED_OUTPUT_TIMEOUT / 2;
    grid.render_lock_deadline = Some(pinned);
    feed_bytes(&mut grid, b"\x1b[?2026h\x1b[?2026h");
    assert_eq!(grid.sync_output_deadline(), Some(pinned));
    // Once the lock has lapsed, the next frame gets a deadline of its own.
    let lapsed = Instant::now();
    grid.render_lock_deadline = Some(lapsed);
    feed_bytes(&mut grid, b"\x1b[?2026h");
    assert!(grid.is_mid_frame());
    assert!(grid.sync_output_deadline().unwrap() >= lapsed + SYNCHRONIZED_OUTPUT_TIMEOUT);
}
