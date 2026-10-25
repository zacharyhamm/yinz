use super::*;
use crate::geometry::FloatingPaneCoordinates;
use crate::geometry::PercentOrFixed;
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

/// Screen-sized coordinates: large but small enough that a sum of two cannot
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
    let coords = FloatingPaneCoordinates {
        x: tc.draw(generators::optional(percent_or_fixeds())),
        y: tc.draw(generators::optional(percent_or_fixeds())),
        width: tc.draw(generators::optional(percent_or_fixeds())),
        height: tc.draw(generators::optional(percent_or_fixeds())),
        pinned: None,
        borderless: None,
    };
    let mut geom = PaneGeom::default();
    geom.adjust_coordinates(coords.clone(), viewport);
    assert!(
        geom.x >= viewport.x,
        "x {} left of viewport {:?} (coords {:?})",
        geom.x,
        viewport,
        coords
    );
    assert!(
        geom.y >= viewport.y,
        "y {} above viewport {:?} (coords {:?})",
        geom.y,
        viewport,
        coords
    );
    assert!(
        geom.x + geom.cols.as_usize() <= viewport.x + viewport.cols,
        "x {} + cols {} past viewport {:?} (coords {:?})",
        geom.x,
        geom.cols.as_usize(),
        viewport,
        coords
    );
    assert!(
        geom.y + geom.rows.as_usize() <= viewport.y + viewport.rows,
        "y {} + rows {} past viewport {:?} (coords {:?})",
        geom.y,
        geom.rows.as_usize(),
        viewport,
        coords
    );
}

#[test]
fn apply_floating_pane_position_places_a_pane_at_the_coordinates_it_names() {
    let mut geom = PaneGeom::default();
    geom.apply_floating_pane_position(
        Some(PercentOrFixed::Fixed(5)),
        Some(PercentOrFixed::Fixed(3)),
        Some(PercentOrFixed::Fixed(20)),
        Some(PercentOrFixed::Fixed(10)),
        80,
        24,
    );
    assert_eq!(geom.x, 5);
    assert_eq!(geom.y, 3);
    assert_eq!(geom.cols.as_usize(), 20);
    assert_eq!(geom.rows.as_usize(), 10);
}

#[test]
fn apply_floating_pane_position_centers_a_sized_pane_with_no_coordinates() {
    let mut geom = PaneGeom::default();
    geom.apply_floating_pane_position(
        None,
        None,
        Some(PercentOrFixed::Fixed(20)),
        Some(PercentOrFixed::Fixed(10)),
        80,
        24,
    );
    assert_eq!(geom.x, (80 - 20) / 2);
    assert_eq!(geom.y, (24 - 10) / 2);
}

#[test]
fn apply_floating_pane_position_reads_a_percentage_of_the_viewport() {
    let mut geom = PaneGeom::default();
    geom.apply_floating_pane_position(
        Some(PercentOrFixed::Percent(10)),
        Some(PercentOrFixed::Percent(20)),
        None,
        None,
        100,
        50,
    );
    // 10% of 100, 20% of 50
    assert_eq!(geom.x, 10);
    assert_eq!(geom.y, 10);
}
