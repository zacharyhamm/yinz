//! Which cells a moving overlay stopped covering, so whatever is underneath can be repainted.
//!
//! Toasts paint over panes that have no idea they are there. A pane only emits chunks for the rows
//! its own grid thinks are dirty, so the moment a toast stops covering a cell, that cell keeps
//! whatever glyph the toast last put in it until something else happens to repaint it. Sliding a
//! toast out therefore leaves a trail of its own border behind - which is what this module exists
//! to find, one frame at a time, so the panes under the trail can be asked to repaint.
//!
//! Nothing here knows about toasts specifically: it is rectangle arithmetic over what an overlay
//! painted last frame and what it is painting now.

use yinz_types::pane_size::PaneGeom;

/// A block of cells in absolute session coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellRect {
    pub x: usize,
    pub y: usize,
    pub cols: usize,
    pub rows: usize,
}

impl CellRect {
    pub fn is_empty(&self) -> bool {
        self.cols == 0 || self.rows == 0
    }

    fn right(&self) -> usize {
        self.x + self.cols
    }

    fn bottom(&self) -> usize {
        self.y + self.rows
    }

    /// Whether this rect overlaps `geom`. Touching edges are not an overlap - a pane whose last
    /// column is the one before this rect's first shares no cell with it and has nothing to
    /// repaint.
    pub fn intersects_geom(&self, geom: &PaneGeom) -> bool {
        if self.is_empty() {
            return false;
        }
        let geom_right = geom.x + geom.cols.as_usize();
        let geom_bottom = geom.y + geom.rows.as_usize();
        self.x < geom_right
            && geom.x < self.right()
            && self.y < geom_bottom
            && geom.y < self.bottom()
    }

    fn covers_row(&self, row: usize) -> bool {
        row >= self.y && row < self.bottom()
    }

    /// Whether this rect paints the cell at `(x, y)`.
    pub fn contains(&self, x: usize, y: usize) -> bool {
        !self.is_empty() && self.covers_row(y) && x >= self.x && x < self.right()
    }
}

/// The cells `before` painted that `after` does not.
///
/// Returned per row, so a caller can hand them straight to a per-row repaint. Rows are only
/// examined where something was painted before, and a row whose coverage grew contributes nothing
/// - which is why a toast *arriving* costs no repaints at all, and only one leaving does.
///
/// Both sides may contain several overlapping rects: a toast gliding past another transiently
/// covers rows the other one also covers. Coverage is therefore accumulated as a union per row
/// before being subtracted, so a cell one toast vacated but another still covers is correctly not
/// reported as damaged.
pub fn overlay_damage(before: &[CellRect], after: &[CellRect]) -> Vec<CellRect> {
    let rows = before
        .iter()
        .filter(|rect| !rect.is_empty())
        .flat_map(|rect| rect.y..rect.bottom());
    let mut examined: Vec<usize> = rows.collect();
    examined.sort_unstable();
    examined.dedup();

    let mut damage: Vec<CellRect> = vec![];
    for row in examined {
        let was = spans_on_row(before, row);
        let is = spans_on_row(after, row);
        for (x, cols) in subtract_spans(&was, &is) {
            // merge into the previous row's damage when the span matches, so a toast that vacated
            // a whole block hands back one rect rather than one per row
            match damage.last_mut() {
                Some(previous)
                    if previous.x == x && previous.cols == cols && previous.bottom() == row =>
                {
                    previous.rows += 1;
                },
                _ => damage.push(CellRect {
                    x,
                    y: row,
                    cols,
                    rows: 1,
                }),
            }
        }
    }
    damage
}

/// The union of `rects`' coverage on `row`, as disjoint `(x, cols)` spans in ascending order.
fn spans_on_row(rects: &[CellRect], row: usize) -> Vec<(usize, usize)> {
    let mut spans: Vec<(usize, usize)> = rects
        .iter()
        .filter(|rect| !rect.is_empty() && rect.covers_row(row))
        .map(|rect| (rect.x, rect.right()))
        .collect();
    spans.sort_unstable();
    let mut merged: Vec<(usize, usize)> = vec![];
    for (start, end) in spans {
        match merged.last_mut() {
            Some((_, previous_end)) if start <= *previous_end => {
                *previous_end = (*previous_end).max(end);
            },
            _ => merged.push((start, end)),
        }
    }
    merged
        .into_iter()
        .map(|(start, end)| (start, end - start))
        .collect()
}

/// `was` minus `is`, both disjoint ascending `(x, cols)` spans.
fn subtract_spans(was: &[(usize, usize)], is: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut result = vec![];
    for &(start, cols) in was {
        let mut cursor = start;
        let end = start + cols;
        for &(covered_start, covered_cols) in is {
            let covered_end = covered_start + covered_cols;
            if covered_end <= cursor {
                continue;
            }
            if covered_start >= end {
                break;
            }
            if covered_start > cursor {
                result.push((cursor, covered_start - cursor));
            }
            cursor = cursor.max(covered_end);
            if cursor >= end {
                break;
            }
        }
        if cursor < end {
            result.push((cursor, end - cursor));
        }
    }
    result
}

#[cfg(test)]
#[path = "unit/damage_tests.rs"]
mod tests;
