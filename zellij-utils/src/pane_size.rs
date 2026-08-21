use serde::{Deserialize, Serialize};
use std::{
    fmt::Display,
    hash::{Hash, Hasher},
};

use crate::data::FloatingPaneCoordinates;
use crate::input::layout::{PercentOrFixed, SplitDirection, SplitSize};
use crate::position::Position;

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

    pub fn adjust_inner(&mut self, full_size: usize) -> f64 {
        // returns the leftover from
        // rounding if any
        // TODO: elsewhere?
        match self.constraint {
            Constraint::Percent(percent) => {
                let new_inner = (percent / 100.0) * full_size as f64;
                let rounded = new_inner.floor();
                let leftover = rounded - new_inner;
                self.set_inner(rounded as usize);
                leftover
            },
            Constraint::Fixed(fixed_size) => {
                self.set_inner(fixed_size);
                0.0
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
    pub fn reduce_by(&mut self, by: f64, by_inner: usize) {
        match self.constraint {
            Constraint::Percent(percent) => {
                self.constraint = Constraint::Percent(percent - by);
                self.inner = self.inner.saturating_sub(by_inner);
            },
            Constraint::Fixed(_fixed) => {
                log::error!("Cannot reduce_by fixed dimensions");
            },
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
        let col = point.column.0;
        let row = point.line.0 as usize;
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
    /// `origin` is where the tab's display area starts on screen - non-zero when global panes are
    /// docked to the left or top - so that coordinates are relative to the tab rather than to the
    /// screen.
    /// NOTE: Every floating-pane creation path (`tab/mod.rs`, `layout_applier.rs`,
    /// `floating_panes/mod.rs`, `tiled_pane_grid.rs` and any future one) must pass
    /// the tab's `display_area_origin` here; otherwise panes are mispositioned
    /// when left/top global strips exist (see `pane_size` tests with non-zero origin).
    #[allow(clippy::too_many_arguments)]
    pub fn apply_floating_pane_position(
        &mut self,
        x: Option<PercentOrFixed>,
        y: Option<PercentOrFixed>,
        width: Option<PercentOrFixed>,
        height: Option<PercentOrFixed>,
        viewport_cols: usize,
        viewport_rows: usize,
        origin: (usize, usize),
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
            Some(x) => origin.0.saturating_add(x.to_fixed(viewport_cols)),
            None if width.is_some() => origin
                .0
                .saturating_add(viewport_cols.saturating_sub(self.cols.as_usize()) / 2),
            None => self.x,
        };
        self.y = match &y {
            Some(y) => origin.1.saturating_add(y.to_fixed(viewport_rows)),
            None if height.is_some() => origin
                .1
                .saturating_add(viewport_rows.saturating_sub(self.rows.as_usize()) / 2),
            None => self.y,
        };
    }
    pub fn adjust_coordinates(
        &mut self,
        floating_pane_coordinates: FloatingPaneCoordinates,
        viewport: Viewport,
        origin: (usize, usize),
    ) {
        self.apply_floating_pane_position(
            floating_pane_coordinates.x,
            floating_pane_coordinates.y,
            floating_pane_coordinates.width,
            floating_pane_coordinates.height,
            viewport.cols,
            viewport.rows,
            origin,
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
            _ => {
                log::error!("Can't combine fixed panes");
                None
            },
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
            _ => {
                log::error!("Can't combine fixed panes");
                None
            },
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
                    log::error!("Can't combine fixed panes");
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
                    log::error!("Can't combine fixed panes");
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

    pub fn shift_right_and_top(right: usize, top: usize) -> Self {
        Self {
            right,
            top,
            ..Default::default()
        }
    }

    pub fn shift_right(right: usize) -> Self {
        Self {
            right,
            ..Default::default()
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

    // FIXME: This should be top and left, not bottom and right, but `boundaries.rs` would need
    // some changing
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
mod tests {
    use super::*;
    use crate::data::FloatingPaneCoordinates;
    use crate::input::layout::PercentOrFixed;
    use hegel::generators::{self, Generator};

    /// Any `Dimension`, either constraint, unbounded `inner`.
    #[hegel::composite]
    fn dimensions(tc: &hegel::TestCase) -> Dimension {
        let constraint = tc.draw(hegel::one_of!(
            generators::floats::<f64>().map(Constraint::Percent),
            generators::integers::<usize>().map(Constraint::Fixed),
        ));
        let inner = tc.draw(generators::integers::<usize>());
        Dimension { constraint, inner }
    }

    #[hegel::composite]
    fn percent_or_fixeds(tc: &hegel::TestCase) -> PercentOrFixed {
        tc.draw(hegel::one_of!(
            generators::integers::<usize>().map(PercentOrFixed::Percent),
            generators::integers::<usize>().map(PercentOrFixed::Fixed),
        ))
    }

    /// Screen-sized coordinates: large but small enough that `origin + value` cannot
    /// overflow `usize` (usize overflow is a separate concern from containment).
    #[hegel::composite]
    fn coordinates(tc: &hegel::TestCase) -> usize {
        tc.draw(generators::integers::<usize>().max_value(u32::MAX as usize))
    }

    #[hegel::test]
    fn split_out_never_panics(tc: hegel::TestCase) {
        let mut dimension = tc.draw(dimensions());
        let by = tc.draw(generators::floats::<f64>());
        let before = dimension;
        let split_out = dimension.split_out(by);
        // just touch the results so nothing is optimized away
        assert!(
            split_out.as_usize() == split_out.as_usize(),
            "split_out({}) on {:?} produced {:?} / {:?}",
            by,
            before,
            dimension,
            split_out
        );
    }

    #[hegel::test]
    fn adjust_coordinates_keeps_the_geom_inside_the_viewport(tc: hegel::TestCase) {
        let viewport = Viewport {
            x: tc.draw(coordinates()),
            y: tc.draw(coordinates()),
            rows: tc.draw(coordinates()),
            cols: tc.draw(coordinates()),
        };
        let origin = (tc.draw(coordinates()), tc.draw(coordinates()));
        let coords = FloatingPaneCoordinates {
            x: tc.draw(generators::optional(percent_or_fixeds())),
            y: tc.draw(generators::optional(percent_or_fixeds())),
            width: tc.draw(generators::optional(percent_or_fixeds())),
            height: tc.draw(generators::optional(percent_or_fixeds())),
            pinned: None,
            borderless: None,
        };
        let mut geom = PaneGeom::default();
        geom.adjust_coordinates(coords.clone(), viewport, origin);
        assert!(
            geom.x >= viewport.x,
            "x {} left of viewport {:?} (coords {:?}, origin {:?})",
            geom.x,
            viewport,
            coords,
            origin
        );
        assert!(
            geom.y >= viewport.y,
            "y {} above viewport {:?} (coords {:?}, origin {:?})",
            geom.y,
            viewport,
            coords,
            origin
        );
        assert!(
            geom.x + geom.cols.as_usize() <= viewport.x + viewport.cols,
            "x {} + cols {} past viewport {:?} (coords {:?}, origin {:?})",
            geom.x,
            geom.cols.as_usize(),
            viewport,
            coords,
            origin
        );
        assert!(
            geom.y + geom.rows.as_usize() <= viewport.y + viewport.rows,
            "y {} + rows {} past viewport {:?} (coords {:?}, origin {:?})",
            geom.y,
            geom.rows.as_usize(),
            viewport,
            coords,
            origin
        );
    }

    #[test]
    fn apply_floating_pane_position_with_non_zero_origin() {
        let mut geom = PaneGeom::default();
        // origin (10,5) simulates left/top global strips
        let origin = (10, 5);
        geom.apply_floating_pane_position(
            Some(PercentOrFixed::Fixed(5)),
            Some(PercentOrFixed::Fixed(3)),
            Some(PercentOrFixed::Fixed(20)),
            Some(PercentOrFixed::Fixed(10)),
            80,
            24,
            origin,
        );
        // x = origin.x + x, y = origin.y + y
        assert_eq!(geom.x, 15, "x should include origin.x");
        assert_eq!(geom.y, 8, "y should include origin.y");
        assert_eq!(geom.cols.as_usize(), 20);
        assert_eq!(geom.rows.as_usize(), 10);
    }

    #[test]
    fn apply_floating_pane_position_centered_with_origin() {
        let mut geom = PaneGeom::default();
        let origin = (10, 5);
        // No x/y, width/height set — should center within viewport plus origin
        geom.apply_floating_pane_position(
            None,
            None,
            Some(PercentOrFixed::Fixed(20)),
            Some(PercentOrFixed::Fixed(10)),
            80,
            24,
            origin,
        );
        // centered: origin + (viewport - size)/2
        assert_eq!(geom.x, 10 + (80 - 20) / 2);
        assert_eq!(geom.y, 5 + (24 - 10) / 2);
    }

    #[test]
    fn apply_floating_pane_position_percent_with_origin() {
        let mut geom = PaneGeom::default();
        let origin = (10, 5);
        geom.apply_floating_pane_position(
            Some(PercentOrFixed::Percent(10)),
            Some(PercentOrFixed::Percent(20)),
            None,
            None,
            100,
            50,
            origin,
        );
        // 10% of 100 =10, 20% of 50=10
        assert_eq!(geom.x, 10 + 10);
        assert_eq!(geom.y, 5 + 10);
    }
}
