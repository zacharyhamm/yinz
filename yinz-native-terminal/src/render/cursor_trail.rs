//! Swept comet trail drawn behind jumping cursors in the native terminal window.

use std::time::{Duration, Instant};

use crate::emulation::color::Srgb;
use crate::emulation::screen_buffer::{CursorShape, TermSize};
use crate::text::font::CellMetrics;

/// Minimum Chebyshev cell distance required to trigger a cursor trail.
pub const MIN_JUMP_CELLS: usize = 2;

/// Duration of one cursor trail sweep from old position to new position.
pub const TRAIL_DURATION: Duration = Duration::from_millis(160);

/// Grace period for brief cursor hides (such as mid-frame TUI redraws).
pub const HIDE_GRACE: Duration = Duration::from_millis(150);

/// Peak opacity at the leading edge of a newly started trail.
pub const PEAK_ALPHA: f32 = 0.55;

/// Starting scale of the trailing end relative to the cursor size, producing a tapered comet.
const TAIL_SCALE: f32 = 0.5;

const COLLINEAR_EPSILON: f32 = 1e-4;
const DUPLICATE_EPSILON: f32 = 1e-3;
const AXIS_EPSILON_SQ: f32 = 1e-4;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrailVertex {
    pub x: f32,
    pub y: f32,
    pub color: Srgb,
    pub alpha: f32,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct TrailMesh {
    /// Triangle list vertices in grid pixel coordinates (relative to the grid origin).
    pub vertices: Vec<TrailVertex>,
}

impl TrailMesh {
    pub fn clear(&mut self) {
        self.vertices.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorSnapshot {
    pub row: usize,
    pub col: usize,
    pub visible: bool,
    pub shape: CursorShape,
    pub color: Srgb,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct PixelBox {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

impl PixelBox {
    fn from_cursor(row: usize, col: usize, shape: CursorShape, metrics: CellMetrics) -> Self {
        let x = (col as f32) * (metrics.width as f32);
        let y = (row as f32) * (metrics.height as f32);
        let width = metrics.width.max(1) as f32;
        let height = metrics.height.max(1) as f32;
        let stroke = metrics
            .stroke
            .max(1)
            .min(metrics.width.max(1))
            .min(metrics.height.max(1)) as f32;
        match shape {
            CursorShape::Block | CursorShape::Default => Self {
                left: x,
                top: y,
                right: x + width,
                bottom: y + height,
            },
            CursorShape::Beam => Self {
                left: x,
                top: y,
                right: x + stroke,
                bottom: y + height,
            },
            CursorShape::Underline => Self {
                left: x,
                top: y + (height - stroke).max(0.0),
                right: x + width,
                bottom: y + height,
            },
        }
    }

    fn centered(cx: f32, cy: f32, width: f32, height: f32) -> Self {
        let half_w = width * 0.5;
        let half_h = height * 0.5;
        Self {
            left: cx - half_w,
            top: cy - half_h,
            right: cx + half_w,
            bottom: cy + half_h,
        }
    }

    fn center(self) -> [f32; 2] {
        [
            (self.left + self.right) * 0.5,
            (self.top + self.bottom) * 0.5,
        ]
    }

    fn width(self) -> f32 {
        (self.right - self.left).max(0.0)
    }

    fn height(self) -> f32 {
        (self.bottom - self.top).max(0.0)
    }

    fn corners(self) -> [[f32; 2]; 4] {
        [
            [self.left, self.top],
            [self.right, self.top],
            [self.right, self.bottom],
            [self.left, self.bottom],
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Anchor {
    row: usize,
    col: usize,
    rect: PixelBox,
    grid: TermSize,
    metrics: CellMetrics,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ActiveTrail {
    from: PixelBox,
    to: PixelBox,
    color: Srgb,
    started: Instant,
    duration: Duration,
}

impl ActiveTrail {
    fn progress(&self, now: Instant) -> f32 {
        if self.duration.is_zero() {
            return 1.0;
        }
        (now.saturating_duration_since(self.started).as_secs_f32() / self.duration.as_secs_f32())
            .clamp(0.0, 1.0)
    }

    fn is_settled(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) >= self.duration
    }

    fn interpolated_box(&self, eased: f32, taper: bool) -> PixelBox {
        let [from_cx, from_cy] = self.from.center();
        let [to_cx, to_cy] = self.to.center();
        let cx = lerp(from_cx, to_cx, eased);
        let cy = lerp(from_cy, to_cy, eased);
        let base_w = lerp(self.from.width(), self.to.width(), eased);
        let base_h = lerp(self.from.height(), self.to.height(), eased);
        if !taper {
            return PixelBox::centered(cx, cy, base_w, base_h);
        }
        let scale = lerp(TAIL_SCALE, 1.0, eased);
        let width = (base_w * scale).max(base_w.min(1.0));
        let height = (base_h * scale).max(base_h.min(1.0));
        PixelBox::centered(cx, cy, width, height)
    }
}

#[derive(Debug, Default)]
pub struct CursorTrail {
    anchor: Option<Anchor>,
    hidden_since: Option<Instant>,
    active: Option<ActiveTrail>,
}

impl CursorTrail {
    pub fn reset(&mut self) {
        self.anchor = None;
        self.hidden_since = None;
        self.active = None;
    }

    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    pub fn observe(
        &mut self,
        snapshot: CursorSnapshot,
        grid: TermSize,
        metrics: CellMetrics,
        enabled: bool,
        now: Instant,
    ) {
        if !enabled {
            self.reset();
            return;
        }

        if let Some(prev) = self.anchor
            && (prev.grid != grid || prev.metrics != metrics)
        {
            self.reset();
        }

        if !snapshot.visible || snapshot.row >= grid.rows || snapshot.col >= grid.cols {
            let since = *self.hidden_since.get_or_insert(now);
            if now.saturating_duration_since(since) >= HIDE_GRACE {
                self.reset();
            }
            return;
        }

        if let Some(since) = self.hidden_since.take()
            && now.saturating_duration_since(since) >= HIDE_GRACE
        {
            self.anchor = None;
            self.active = None;
        }

        let rect = PixelBox::from_cursor(snapshot.row, snapshot.col, snapshot.shape, metrics);
        if let Some(prev) = self.anchor {
            let jump = prev
                .row
                .abs_diff(snapshot.row)
                .max(prev.col.abs_diff(snapshot.col));
            if jump >= MIN_JUMP_CELLS {
                let from = self
                    .active
                    .filter(|trail| !trail.is_settled(now))
                    .map(|trail| trail.interpolated_box(ease_out(trail.progress(now)), false))
                    .unwrap_or(prev.rect);
                self.active = Some(ActiveTrail {
                    from,
                    to: rect,
                    color: snapshot.color,
                    started: now,
                    duration: TRAIL_DURATION,
                });
            } else if let Some(active) = &mut self.active {
                active.to = rect;
                active.color = snapshot.color;
            }
        }

        self.anchor = Some(Anchor {
            row: snapshot.row,
            col: snapshot.col,
            rect,
            grid,
            metrics,
        });
    }

    /// Sample the trail geometry at `now` into `out`.
    /// Returns `true` while the trail is visible and animating and needs another frame.
    pub fn sample(&mut self, now: Instant, out: &mut TrailMesh) -> bool {
        out.clear();
        if let Some(since) = self.hidden_since {
            if now.saturating_duration_since(since) >= HIDE_GRACE {
                self.reset();
            }
            return false;
        }
        let Some(active) = &self.active else {
            return false;
        };
        if active.is_settled(now) {
            self.active = None;
            return false;
        }
        let color = active.color;

        let t = active.progress(now);
        let eased = ease_out(t);
        let tail = active.interpolated_box(eased, true);
        let head = active.to;

        let [tail_cx, tail_cy] = tail.center();
        let [head_cx, head_cy] = head.center();
        let axis_x = head_cx - tail_cx;
        let axis_y = head_cy - tail_cy;
        let axis_len_sq = axis_x * axis_x + axis_y * axis_y;
        if axis_len_sq <= AXIS_EPSILON_SQ {
            self.active = None;
            return false;
        }

        let mut points = Vec::with_capacity(8);
        points.extend_from_slice(&tail.corners());
        points.extend_from_slice(&head.corners());
        let hull = convex_hull(points);
        if hull.len() < 3 {
            self.active = None;
            return false;
        }

        let head_alpha = PEAK_ALPHA * (1.0 - t * t);
        let vertex = |[x, y]: [f32; 2]| {
            let projection =
                (((x - tail_cx) * axis_x + (y - tail_cy) * axis_y) / axis_len_sq).clamp(0.0, 1.0);
            TrailVertex {
                x,
                y,
                color,
                alpha: head_alpha * projection,
            }
        };

        let first = vertex(hull[0]);
        for pair in hull[1..].windows(2) {
            out.vertices.push(first);
            out.vertices.push(vertex(pair[0]));
            out.vertices.push(vertex(pair[1]));
        }

        !out.is_empty()
    }
}

fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

fn ease_out(progress: f32) -> f32 {
    let remaining = 1.0 - progress.clamp(0.0, 1.0);
    1.0 - remaining * remaining * remaining
}

fn cross(o: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// Andrew's monotone chain over up to 8 corner points, discarding collinear interior vertices.
fn convex_hull(mut points: Vec<[f32; 2]>) -> Vec<[f32; 2]> {
    points.sort_by(|a, b| a[0].total_cmp(&b[0]).then_with(|| a[1].total_cmp(&b[1])));
    let mut unique: Vec<[f32; 2]> = Vec::with_capacity(points.len());
    for point in points {
        if let Some(last) = unique.last()
            && (point[0] - last[0]).abs() <= DUPLICATE_EPSILON
            && (point[1] - last[1]).abs() <= DUPLICATE_EPSILON
        {
            continue;
        }
        unique.push(point);
    }
    if unique.len() < 3 {
        return Vec::new();
    }

    let mut lower: Vec<[f32; 2]> = Vec::with_capacity(unique.len());
    for &point in &unique {
        while lower.len() >= 2
            && cross(lower[lower.len() - 2], lower[lower.len() - 1], point) <= COLLINEAR_EPSILON
        {
            lower.pop();
        }
        lower.push(point);
    }

    let mut upper: Vec<[f32; 2]> = Vec::with_capacity(unique.len());
    for &point in unique.iter().rev() {
        while upper.len() >= 2
            && cross(upper[upper.len() - 2], upper[upper.len() - 1], point) <= COLLINEAR_EPSILON
        {
            upper.pop();
        }
        upper.push(point);
    }

    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

#[cfg(test)]
#[path = "unit/cursor_trail_tests.rs"]
mod tests;
