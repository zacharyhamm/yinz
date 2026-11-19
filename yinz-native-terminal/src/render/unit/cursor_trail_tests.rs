use std::time::{Duration, Instant};

use crate::emulation::screen_buffer::{CursorShape, TermSize};
use crate::render::cursor_trail::{
    CursorSnapshot, CursorTrail, TrailMesh, HIDE_GRACE, PEAK_ALPHA, TRAIL_DURATION,
};
use crate::text::font::CellMetrics;

const GRID: TermSize = TermSize {
    rows: 40,
    cols: 120,
};
const CURSOR_COLOR: [u8; 3] = [120, 200, 255];

fn metrics() -> CellMetrics {
    CellMetrics {
        width: 10,
        height: 20,
        baseline: 16,
        underline_top: 18,
        strikeout_top: 10,
        stroke: 2,
    }
}

fn snapshot(row: usize, col: usize, shape: CursorShape) -> CursorSnapshot {
    CursorSnapshot {
        row,
        col,
        visible: true,
        shape,
        color: CURSOR_COLOR,
    }
}

fn bounds(mesh: &TrailMesh) -> (f32, f32, f32, f32) {
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for v in &mesh.vertices {
        min_x = min_x.min(v.x);
        min_y = min_y.min(v.y);
        max_x = max_x.max(v.x);
        max_y = max_y.max(v.y);
    }
    (min_x, min_y, max_x, max_y)
}

#[test]
fn initial_cursor_and_single_cell_moves_do_not_trigger_trail() {
    let start = Instant::now();
    let mut trail = CursorTrail::default();
    let mut mesh = TrailMesh::default();
    let cell = metrics();

    trail.observe(snapshot(5, 5, CursorShape::Block), GRID, cell, true, start);
    assert!(!trail.is_active());
    assert!(!trail.sample(start, &mut mesh));
    assert!(mesh.is_empty());

    // Single-cell horizontal, vertical, and 1x1 diagonal steps stay crisp without a trail.
    for (step, (row, col)) in [(5, 6), (6, 6), (7, 7), (7, 7)].into_iter().enumerate() {
        let at = start + Duration::from_millis((step as u64 + 1) * 16);
        trail.observe(snapshot(row, col, CursorShape::Block), GRID, cell, true, at);
        assert!(
            !trail.is_active(),
            "step ({row}, {col}) should not start a trail"
        );
    }
}

#[test]
fn horizontal_jump_draws_tapered_quad_and_settles_at_duration() {
    let start = Instant::now();
    let mut trail = CursorTrail::default();
    let mut mesh = TrailMesh::default();
    let cell = metrics();

    trail.observe(snapshot(2, 1, CursorShape::Block), GRID, cell, true, start);
    trail.observe(snapshot(2, 21, CursorShape::Block), GRID, cell, true, start);
    assert!(trail.is_active());

    assert!(trail.sample(start, &mut mesh));
    // Tapered horizontal comet connects the 50%-tall tail to the full-height head (6-vertex hull -> 4 triangles).
    assert_eq!(mesh.vertices.len(), 12);
    let (min_x, min_y, max_x, max_y) = bounds(&mesh);
    // Tail starts centered at col 1 (x=15) tapered to 50% width (12.5..17.5); head ends at col 21 right edge (220).
    assert!((min_x - 12.5).abs() < 1e-3, "min_x={min_x}");
    assert!((max_x - 220.0).abs() < 1e-3, "max_x={max_x}");
    assert!((min_y - 40.0).abs() < 1e-3, "min_y={min_y}");
    assert!((max_y - 60.0).abs() < 1e-3, "max_y={max_y}");
    let tail_ys: Vec<f32> = mesh
        .vertices
        .iter()
        .filter(|v| (v.x - min_x).abs() < 1e-3)
        .map(|v| v.y)
        .collect();
    let tail_min_y = tail_ys.iter().copied().fold(f32::INFINITY, f32::min);
    let tail_max_y = tail_ys.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    assert!(
        ((tail_max_y - tail_min_y) - 10.0).abs() < 1e-3,
        "tail should taper to 50% height (10px), got {tail_min_y}..{tail_max_y}"
    );
    assert!(mesh.vertices.iter().all(|v| v.color == CURSOR_COLOR));

    let tail_alpha = mesh
        .vertices
        .iter()
        .filter(|v| (v.x - min_x).abs() < 1e-3)
        .map(|v| v.alpha)
        .fold(0.0f32, f32::max);
    let head_alpha = mesh
        .vertices
        .iter()
        .filter(|v| (v.x - max_x).abs() < 1e-3)
        .map(|v| v.alpha)
        .fold(0.0f32, f32::max);
    assert!(
        tail_alpha < 0.05,
        "tail should fade near zero: {tail_alpha}"
    );
    assert!(
        (head_alpha - PEAK_ALPHA).abs() < 1e-3,
        "head should start at PEAK_ALPHA: {head_alpha}"
    );

    // Mid-flight: tail moves closer to head and peak alpha drops smoothly.
    let mid = start + TRAIL_DURATION / 2;
    assert!(trail.sample(mid, &mut mesh));
    let (mid_min_x, _, mid_max_x, _) = bounds(&mesh);
    assert!(mid_min_x > min_x + 100.0, "cubic ease-out advances fast");
    assert!((mid_max_x - max_x).abs() < 1e-3);
    let mid_head_alpha = mesh.vertices.iter().map(|v| v.alpha).fold(0.0f32, f32::max);
    assert!(mid_head_alpha < head_alpha && mid_head_alpha > 0.0);

    // Settled at exact duration: trail clears and stops requesting frames.
    assert!(!trail.sample(start + TRAIL_DURATION, &mut mesh));
    assert!(mesh.is_empty());
    assert!(!trail.is_active());
}

#[test]
fn diagonal_jump_produces_six_vertex_convex_hull_with_four_triangles() {
    let start = Instant::now();
    let mut trail = CursorTrail::default();
    let mut mesh = TrailMesh::default();
    let cell = metrics();

    trail.observe(snapshot(1, 2, CursorShape::Block), GRID, cell, true, start);
    trail.observe(
        snapshot(15, 42, CursorShape::Block),
        GRID,
        cell,
        true,
        start,
    );

    assert!(trail.sample(start, &mut mesh));
    // Two axis-aligned boxes offset diagonally have a 6-vertex hull -> 4 triangles (12 vertices).
    assert_eq!(mesh.vertices.len(), 12);
    let (min_x, min_y, max_x, max_y) = bounds(&mesh);
    assert!(min_x < 30.0 && min_y < 40.0);
    assert!((max_x - 430.0).abs() < 1e-3);
    assert!((max_y - 320.0).abs() < 1e-3);
}

#[test]
fn beam_and_underline_cursors_morph_cleanly_without_zero_width_collapse() {
    let start = Instant::now();
    let mut trail = CursorTrail::default();
    let mut mesh = TrailMesh::default();
    let mut thin = metrics();
    thin.stroke = 1;

    trail.observe(snapshot(0, 0, CursorShape::Beam), GRID, thin, true, start);
    trail.observe(snapshot(10, 0, CursorShape::Beam), GRID, thin, true, start);
    assert!(trail.sample(start, &mut mesh));
    let (min_x, _, max_x, _) = bounds(&mesh);
    assert!((max_x - min_x - 1.0).abs() < 1e-3, "1px beam stays >= 1px");

    // Morph from underline to block diagonally.
    trail.observe(
        snapshot(10, 0, CursorShape::Underline),
        GRID,
        thin,
        true,
        start + Duration::from_millis(200),
    );
    trail.observe(
        snapshot(20, 20, CursorShape::Block),
        GRID,
        thin,
        true,
        start + Duration::from_millis(220),
    );
    assert!(trail.sample(start + Duration::from_millis(220), &mut mesh));
    assert!(mesh.vertices.len() >= 6);
}

#[test]
fn mid_flight_redirect_and_single_cell_tracking_update_smoothly() {
    let start = Instant::now();
    let mut trail = CursorTrail::default();
    let mut mesh = TrailMesh::default();
    let cell = metrics();

    trail.observe(snapshot(0, 0, CursorShape::Block), GRID, cell, true, start);
    trail.observe(snapshot(0, 40, CursorShape::Block), GRID, cell, true, start);

    // 1-cell follow-up while active tracks the new cursor cell without resetting the animation.
    let at_40ms = start + Duration::from_millis(40);
    trail.observe(
        snapshot(0, 41, CursorShape::Block),
        GRID,
        cell,
        true,
        at_40ms,
    );
    assert!(trail.sample(at_40ms, &mut mesh));
    let (_, _, max_x, _) = bounds(&mesh);
    assert!((max_x - 420.0).abs() < 1e-3);

    // Rapid second jump redirects from current in-flight tail rather than snapping back to (0, 0).
    let at_60ms = start + Duration::from_millis(60);
    trail.observe(
        snapshot(20, 41, CursorShape::Block),
        GRID,
        cell,
        true,
        at_60ms,
    );
    assert!(trail.sample(at_60ms, &mut mesh));
    let (min_x, _, _, max_y) = bounds(&mesh);
    assert!(
        min_x > 100.0,
        "redirect should start from in-flight tail, got {min_x}"
    );
    assert!((max_y - 420.0).abs() < 1e-3);
}

#[test]
fn brief_hide_bridges_redraw_but_long_hide_resize_metrics_and_disable_reset() {
    let start = Instant::now();
    let mut trail = CursorTrail::default();
    let cell = metrics();

    // Brief hide (< HIDE_GRACE) bridges a mid-frame redraw jump.
    trail.observe(snapshot(1, 1, CursorShape::Block), GRID, cell, true, start);
    trail.observe(
        CursorSnapshot {
            visible: false,
            ..snapshot(1, 1, CursorShape::Block)
        },
        GRID,
        cell,
        true,
        start + Duration::from_millis(20),
    );
    trail.observe(
        snapshot(1, 30, CursorShape::Block),
        GRID,
        cell,
        true,
        start + Duration::from_millis(50),
    );
    assert!(trail.is_active());

    // Long hide (>= HIDE_GRACE) clears anchor so returning later does not draw a stale trail.
    let after_settle = start + Duration::from_millis(300);
    trail.observe(
        CursorSnapshot {
            visible: false,
            ..snapshot(1, 30, CursorShape::Block)
        },
        GRID,
        cell,
        true,
        after_settle,
    );
    trail.observe(
        snapshot(20, 5, CursorShape::Block),
        GRID,
        cell,
        true,
        after_settle + HIDE_GRACE + Duration::from_millis(10),
    );
    assert!(!trail.is_active());

    // Grid resize resets without triggering a trail.
    let resized = TermSize {
        rows: 50,
        cols: 140,
    };
    trail.observe(
        snapshot(1, 1, CursorShape::Block),
        resized,
        cell,
        true,
        after_settle + Duration::from_millis(400),
    );
    assert!(!trail.is_active());

    // Metrics change (e.g. font zoom) resets without triggering a trail.
    let zoomed = CellMetrics { width: 12, ..cell };
    trail.observe(
        snapshot(30, 80, CursorShape::Block),
        resized,
        zoomed,
        true,
        after_settle + Duration::from_millis(420),
    );
    assert!(!trail.is_active());

    // Trigger a trail, then disable it -> cancels immediately.
    trail.observe(
        snapshot(2, 2, CursorShape::Block),
        resized,
        zoomed,
        true,
        after_settle + Duration::from_millis(440),
    );
    assert!(trail.is_active());
    trail.observe(
        snapshot(2, 2, CursorShape::Block),
        resized,
        zoomed,
        false,
        after_settle + Duration::from_millis(450),
    );
    assert!(!trail.is_active());
}

#[test]
fn hidden_cursor_suppresses_sampling_preserves_color_and_expires_after_grace() {
    let start = Instant::now();
    let mut trail = CursorTrail::default();
    let mut mesh = TrailMesh::default();
    let cell = metrics();

    trail.observe(snapshot(2, 1, CursorShape::Block), GRID, cell, true, start);
    trail.observe(snapshot(2, 25, CursorShape::Block), GRID, cell, true, start);
    assert!(trail.sample(start, &mut mesh));
    assert!(mesh.vertices.iter().all(|v| v.color == CURSOR_COLOR));

    // Hiding the cursor suppresses drawing and ignores any hidden-cursor color/position churn.
    let hide_at = start + Duration::from_millis(20);
    trail.observe(
        CursorSnapshot {
            row: 30,
            col: 100,
            visible: false,
            shape: CursorShape::Block,
            color: [255, 0, 0],
        },
        GRID,
        cell,
        true,
        hide_at,
    );
    assert!(!trail.sample(hide_at + Duration::from_millis(10), &mut mesh));
    assert!(mesh.is_empty());

    // Returning within HIDE_GRACE resumes smoothly with the visible cursor color.
    let show_at = hide_at + Duration::from_millis(30);
    trail.observe(
        snapshot(2, 25, CursorShape::Block),
        GRID,
        cell,
        true,
        show_at,
    );
    assert!(trail.sample(show_at, &mut mesh));
    assert!(mesh.vertices.iter().all(|v| v.color == CURSOR_COLOR));

    // If hidden again and no new frames arrive past HIDE_GRACE, sample() resets the trail.
    let hide_again = show_at + Duration::from_millis(10);
    trail.observe(
        CursorSnapshot {
            visible: false,
            ..snapshot(2, 25, CursorShape::Block)
        },
        GRID,
        cell,
        true,
        hide_again,
    );
    assert!(!trail.sample(hide_again + HIDE_GRACE, &mut mesh));
    assert!(!trail.is_active());
}

#[test]
fn out_of_bounds_cursor_is_ignored_without_overflow_or_drawing_outside_grid() {
    let start = Instant::now();
    let mut trail = CursorTrail::default();
    let mut mesh = TrailMesh::default();
    let cell = metrics();

    trail.observe(snapshot(2, 1, CursorShape::Block), GRID, cell, true, start);

    for (offset, (row, col)) in [
        (10, (2, GRID.cols)),
        (20, (GRID.rows, 5)),
        (30, (usize::MAX, usize::MAX)),
    ] {
        let at = start + Duration::from_millis(offset);
        trail.observe(snapshot(row, col, CursorShape::Block), GRID, cell, true, at);
        assert!(!trail.is_active());
        assert!(!trail.sample(at, &mut mesh));
        assert!(mesh.is_empty());
    }

    // Returning in bounds within HIDE_GRACE still bridges cleanly from the last valid anchor.
    let back = start + Duration::from_millis(40);
    trail.observe(snapshot(2, 20, CursorShape::Block), GRID, cell, true, back);
    assert!(trail.sample(back, &mut mesh));
    let (min_x, min_y, max_x, max_y) = bounds(&mesh);
    assert!(min_x >= 0.0 && min_y >= 0.0);
    assert!(max_x <= (GRID.cols as u32 * cell.width) as f32);
    assert!(max_y <= (GRID.rows as u32 * cell.height) as f32);
}
