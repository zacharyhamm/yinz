use serde::{Deserialize, Serialize};
use std::{
    fmt::Display,
    hash::{Hash, Hasher},
};

use crate::geometry::FloatingPaneCoordinates;
use crate::geometry::{PercentOrFixed, SplitDirection, SplitSize};
use crate::position::Position;

/// The fewest rows a flexible pane may have, frame included, when a pane is split or resized, a
/// project layout is fitted or a floating or overlay pane is clamped. A stacked layout is placed
/// only where its expanded pane gets at least this many rows
/// (`TiledPaneLayout::position_panes_in_space`). Fixed-size panes such as bars and collapsed stack
/// members can be smaller, and resizing a stack only keeps one row per member
/// (`StackedPanes::min_stack_height`). This is a cell count on purpose, not a share of the screen: resizing a
/// percent-sized tiled pane in yinz-panes also keeps it at or above `RESIZE_PERCENT`
/// (`TiledPaneGrid::can_reduce_pane_width`), and both limits apply.
pub const MIN_TERMINAL_HEIGHT: usize = 5;
/// The fewest columns a flexible pane may have, frame included. See [`MIN_TERMINAL_HEIGHT`].
pub const MIN_TERMINAL_WIDTH: usize = 5;

/// A percentage-only reduction was requested for a fixed dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("Cannot reduce_by fixed dimensions")]
pub struct FixedDimension;

/// Contains the position and size of a [`Pane`], or more generally of any terminal, measured
/// in character rows and columns.
#[derive(Clone, Copy, Default, Debug, Serialize, Deserialize)]
pub struct PaneGeom {
    pub x: usize,
    pub y: usize,
    pub rows: Dimension,
    pub cols: Dimension,
    pub stacked: Option<usize>,          // usize - stack id
    pub is_pinned: bool,                 // only relevant to floating panes
    pub logical_position: Option<usize>, // relevant when placing this pane in a layout
}

impl PartialEq for PaneGeom {
    fn eq(&self, other: &Self) -> bool {
        // compare all except is_pinned
        // NOTE: Keep this in sync with what the `Hash` trait impl does.
        self.x == other.x
            && self.y == other.y
            && self.rows == other.rows
            && self.cols == other.cols
            && self.stacked == other.stacked
    }
}

impl std::hash::Hash for PaneGeom {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // NOTE: Keep this in sync with what the `PartiqlEq` trait impl does.
        self.x.hash(state);
        self.y.hash(state);
        self.rows.hash(state);
        self.cols.hash(state);
        self.stacked.hash(state);
    }
}

impl Eq for PaneGeom {}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Viewport {
    pub x: usize,
    pub y: usize,
    pub rows: usize,
    pub cols: usize,
}

impl Viewport {
    pub fn has_positive_size(&self) -> bool {
        self.rows > 0 && self.cols > 0
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Offset {
    pub top: usize,
    pub bottom: usize,
    pub right: usize,
    pub left: usize,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Size {
    pub rows: usize,
    pub cols: usize,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SizeInPixels {
    pub height: usize,
    pub width: usize,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Insets {
    pub top: usize,
    pub bottom: usize,
    pub left: usize,
    pub right: usize,
}

#[derive(Eq, Clone, Copy, PartialEq, Debug, Serialize, Deserialize, Hash)]
pub struct Dimension {
    pub constraint: Constraint,
    pub(crate) inner: usize,
}

impl Default for Dimension {
    fn default() -> Self {
        Self::percent(100.0)
    }
}

impl Dimension {
    pub fn fixed(size: usize) -> Dimension {
        Self {
            constraint: Constraint::Fixed(size),
            inner: size,
        }
    }

    pub fn percent(percent: f64) -> Dimension {
        Self {
            constraint: Constraint::Percent(percent),
            inner: 1,
        }
    }

    pub fn as_usize(&self) -> usize {
        self.inner
    }

    pub fn as_percent(&self) -> Option<f64> {
        if let Constraint::Percent(p) = self.constraint {
            Some(p)
        } else {
            None
        }
    }

    pub fn set_percent(&mut self, percent: f64) {
        self.constraint = Constraint::Percent(percent);
    }

    pub fn set_inner(&mut self, inner: usize) {
        self.inner = inner;
    }

    /// Resolve the constraint against `full_size`, rounding a percentage down.
    ///
    /// The rounding remainder is not returned: the one caller (`split_space`) corrects
    /// rounding across all panes afterwards with `adjust_geoms_for_rounding_errors`.
    pub fn adjust_inner(&mut self, full_size: usize) {
        match self.constraint {
            Constraint::Percent(percent) => {
                let new_inner = (percent / 100.0) * full_size as f64;
                self.set_inner(new_inner.floor() as usize);
            },
            Constraint::Fixed(fixed_size) => {
                self.set_inner(fixed_size);
            },
        }
    }
    pub fn increase_inner(&mut self, by: usize) {
        self.inner += by;
    }
    pub fn decrease_inner(&mut self, by: usize) {
        self.inner = self.inner.saturating_sub(by);
    }

    pub fn is_fixed(&self) -> bool {
        matches!(self.constraint, Constraint::Fixed(_))
    }
    pub fn is_percent(&self) -> bool {
        matches!(self.constraint, Constraint::Percent(_))
    }
    pub fn from_split_size(split_size: SplitSize, full_size: usize) -> Self {
        match split_size {
            SplitSize::Fixed(fixed) => Dimension {
                constraint: Constraint::Fixed(fixed),
                inner: fixed,
            },
            SplitSize::Percent(percent) => Dimension {
                constraint: Constraint::Percent(percent as f64),
                inner: ((percent as f64 / 100.0) * full_size as f64).floor() as usize,
            },
        }
    }
    pub fn from_percent_or_fixed(size: PercentOrFixed, full_size: usize) -> Self {
        match size {
            PercentOrFixed::Fixed(fixed) => Dimension {
                constraint: Constraint::Fixed(fixed),
                inner: fixed,
            },
            PercentOrFixed::Percent(percent) => Dimension {
                constraint: Constraint::Percent(percent as f64),
                inner: ((percent as f64 / 100.0) * full_size as f64).floor() as usize,
            },
        }
    }
    pub fn split_out(&mut self, by: f64) -> Self {
        // splitting into fewer than one part is meaningless and `by as usize` would be 0;
        // NaN and negatives land here too
        let by = if by.is_finite() && by >= 1.0 { by } else { 1.0 };
        match self.constraint {
            Constraint::Percent(percent) => {
                let split_out_value = percent / by;
                let split_out_inner_value = self.inner / by as usize;
                self.constraint = Constraint::Percent(percent - split_out_value);
                self.inner = self.inner.saturating_sub(split_out_inner_value);
                let mut split_out_dimension = Self::percent(split_out_value);
                split_out_dimension.inner = split_out_inner_value;
                split_out_dimension
            },
            Constraint::Fixed(fixed) => {
                let split_out_value = fixed / by as usize;
                self.constraint = Constraint::Fixed(fixed - split_out_value);
                Self::fixed(split_out_value)
            },
        }
    }
    pub fn reduce_by(&mut self, by: f64, by_inner: usize) -> Result<(), FixedDimension> {
        match self.constraint {
            Constraint::Percent(percent) => {
                self.constraint = Constraint::Percent(percent - by);
                self.inner = self.inner.saturating_sub(by_inner);
                Ok(())
            },
            Constraint::Fixed(_) => Err(FixedDimension),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub enum Constraint {
    /// Constrains the dimension to a fixed, integer number of rows / columns
    Fixed(usize),
    /// Constrains the dimension to a flexible percent size of the total screen
    Percent(f64),
}

impl Display for Constraint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let actual = match self {
            Constraint::Fixed(v) => *v as f64,
            Constraint::Percent(v) => *v,
        };
        write!(f, "{}", actual)?;
        Ok(())
    }
}

#[allow(clippy::derived_hash_with_manual_eq)]
impl Hash for Constraint {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Constraint::Fixed(size) => size.hash(state),
            Constraint::Percent(size) => (*size as usize).hash(state),
        }
    }
}

impl Eq for Constraint {}

impl PaneGeom {
    pub fn contains(&self, point: &Position) -> bool {
        let col = point.column.get();
        let row = point.line.get() as usize;
        self.x <= col
            && col < self.x + self.cols.as_usize()
            && self.y <= row
            && row < self.y + self.rows.as_usize()
    }
    pub fn is_at_least_minimum_size(&self) -> bool {
        self.rows.as_usize() > 0 && self.cols.as_usize() > 0
    }
    pub fn is_flexible_in_direction(&self, split_direction: SplitDirection) -> bool {
        match split_direction {
            SplitDirection::Vertical => self.cols.is_percent(),
            SplitDirection::Horizontal => self.rows.is_percent(),
        }
    }
    /// Coordinates are resolved against the screen, which a tab's display area always starts at.
    #[allow(clippy::too_many_arguments)]
    pub fn apply_floating_pane_position(
        &mut self,
        x: Option<PercentOrFixed>,
        y: Option<PercentOrFixed>,
        width: Option<PercentOrFixed>,
        height: Option<PercentOrFixed>,
        viewport_cols: usize,
        viewport_rows: usize,
    ) {
        if let Some(width) = &width {
            self.cols = Dimension::fixed(width.to_fixed(viewport_cols));
        }
        if let Some(height) = &height {
            self.rows = Dimension::fixed(height.to_fixed(viewport_rows));
        }
        // saturating: `PercentOrFixed` comes from layout config, so a typo'd `Fixed` can be
        // arbitrarily large
        self.x = match &x {
            Some(x) => x.to_fixed(viewport_cols),
            None if width.is_some() => viewport_cols.saturating_sub(self.cols.as_usize()) / 2,
            None => self.x,
        };
        self.y = match &y {
            Some(y) => y.to_fixed(viewport_rows),
            None if height.is_some() => viewport_rows.saturating_sub(self.rows.as_usize()) / 2,
            None => self.y,
        };
    }
    pub fn adjust_coordinates(
        &mut self,
        floating_pane_coordinates: FloatingPaneCoordinates,
        viewport: Viewport,
    ) {
        self.apply_floating_pane_position(
            floating_pane_coordinates.x,
            floating_pane_coordinates.y,
            floating_pane_coordinates.width,
            floating_pane_coordinates.height,
            viewport.cols,
            viewport.rows,
        );
        // saturating throughout: the coordinates come from layout config and can be arbitrarily
        // large
        let max_x = viewport.x.saturating_add(viewport.cols);
        let max_y = viewport.y.saturating_add(viewport.rows);
        // clamp high first, then low: the high clamp subtracts the pane's own size and can
        // itself land left of / above the viewport
        if self.x > max_x {
            self.x = max_x.saturating_sub(self.cols.as_usize());
        }
        if self.x < viewport.x {
            self.x = viewport.x;
        }
        if self.y > max_y {
            self.y = max_y.saturating_sub(self.rows.as_usize());
        }
        if self.y < viewport.y {
            self.y = viewport.y;
        }
        if self.x.saturating_add(self.cols.as_usize()) > max_x {
            self.cols.set_inner(max_x.saturating_sub(self.x));
        }
        if self.y.saturating_add(self.rows.as_usize()) > max_y {
            self.rows.set_inner(max_y.saturating_sub(self.y));
        }
    }
    pub fn combine_vertically_with(&self, geom_below: &PaneGeom) -> Option<Self> {
        match (self.rows.constraint, geom_below.rows.constraint) {
            (Constraint::Percent(self_percent), Constraint::Percent(geom_below_percent)) => {
                let mut combined = *self;
                combined.rows = Dimension::percent(self_percent + geom_below_percent);
                combined.rows.inner = self.rows.inner + geom_below.rows.inner;
                Some(combined)
            },
            _ => None,
        }
    }
    pub fn combine_horizontally_with(&self, geom_to_the_right: &PaneGeom) -> Option<Self> {
        match (self.cols.constraint, geom_to_the_right.cols.constraint) {
            (Constraint::Percent(self_percent), Constraint::Percent(geom_to_the_right_percent)) => {
                let mut combined = *self;
                combined.cols = Dimension::percent(self_percent + geom_to_the_right_percent);
                combined.cols.inner = self.cols.inner + geom_to_the_right.cols.inner;
                Some(combined)
            },
            _ => None,
        }
    }
    pub fn combine_vertically_with_many(&self, geoms_below: &Vec<PaneGeom>) -> Option<Self> {
        // here we expect the geoms to be sorted by their y and be contiguous (i.e. same x and
        // width, no overlaps) and be below self
        let mut combined = *self;
        for geom_below in geoms_below {
            match (combined.rows.constraint, geom_below.rows.constraint) {
                (
                    Constraint::Percent(combined_percent),
                    Constraint::Percent(geom_below_percent),
                ) => {
                    let new_rows_inner = combined.rows.inner + geom_below.rows.inner;
                    combined.rows = Dimension::percent(combined_percent + geom_below_percent);
                    combined.rows.inner = new_rows_inner;
                },
                _ => {
                    return None;
                },
            }
        }
        Some(combined)
    }
    pub fn combine_horizontally_with_many(
        &self,
        geoms_to_the_right: &Vec<PaneGeom>,
    ) -> Option<Self> {
        // here we expect the geoms to be sorted by their x and be contiguous (i.e. same x and
        // width, no overlaps) and be right of self
        let mut combined = *self;
        for geom_to_the_right in geoms_to_the_right {
            match (combined.cols.constraint, geom_to_the_right.cols.constraint) {
                (
                    Constraint::Percent(combined_percent),
                    Constraint::Percent(geom_to_the_right_percent),
                ) => {
                    let new_cols = combined.cols.inner + geom_to_the_right.cols.inner;
                    combined.cols =
                        Dimension::percent(combined_percent + geom_to_the_right_percent);
                    combined.cols.inner = new_cols;
                },
                _ => {
                    return None;
                },
            }
        }
        Some(combined)
    }
    pub fn is_stacked(&self) -> bool {
        self.stacked.is_some()
    }
}

impl Display for PaneGeom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{{ ")?;
        write!(f, r#""x": {},"#, self.x)?;
        write!(f, r#""y": {},"#, self.y)?;
        write!(f, r#""cols": {},"#, self.cols.constraint)?;
        write!(f, r#""rows": {},"#, self.rows.constraint)?;
        write!(f, r#""stacked": {:?}"#, self.stacked)?;
        write!(f, r#""logical_position": {:?}"#, self.logical_position)?;
        write!(f, " }}")?;

        Ok(())
    }
}

impl Offset {
    pub fn frame(size: usize) -> Self {
        Self {
            top: size,
            bottom: size,
            right: size,
            left: size,
        }
    }

    pub fn shift_right_top_and_bottom(right: usize, top: usize, bottom: usize) -> Self {
        Self {
            right,
            top,
            bottom,
            ..Default::default()
        }
    }

    /// Reserves `bottom` rows and `right` columns for the separators a frameless tiled pane shares
    /// with the panes below and to its right: the pane above or to the left owns each separator.
    /// `pane_content_offset` in yinz-panes decides when to reserve them, `Boundaries` draws a
    /// neighbour's top and left edges at `y - 1` and `x - 1`, inside this space, and
    /// `Pane::position_is_on_frame` gives clicks there to this pane. A stack member gets its
    /// stack's offsets, so for a collapsed member `bottom` places its title row that many rows
    /// above its `y` instead. `shift_right_top_and_bottom` follows the same convention.
    pub fn shift(bottom: usize, right: usize) -> Self {
        Self {
            bottom,
            right,
            ..Default::default()
        }
    }
}

impl From<PaneGeom> for Viewport {
    fn from(pane: PaneGeom) -> Self {
        Self {
            x: pane.x,
            y: pane.y,
            rows: pane.rows.as_usize(),
            cols: pane.cols.as_usize(),
        }
    }
}

impl From<Size> for Viewport {
    fn from(size: Size) -> Self {
        Self {
            rows: size.rows,
            cols: size.cols,
            ..Default::default()
        }
    }
}

impl From<&PaneGeom> for Size {
    fn from(pane_geom: &PaneGeom) -> Self {
        Self {
            rows: pane_geom.rows.as_usize(),
            cols: pane_geom.cols.as_usize(),
        }
    }
}

impl From<&Size> for PaneGeom {
    fn from(size: &Size) -> Self {
        let mut rows = Dimension::percent(100.0);
        let mut cols = Dimension::percent(100.0);
        rows.set_inner(size.rows);
        cols.set_inner(size.cols);
        Self {
            rows,
            cols,
            ..Default::default()
        }
    }
}

#[cfg(test)]
#[path = "unit/pane_size_tests.rs"]
mod tests;
