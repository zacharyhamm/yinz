use super::*;
use crate::panes::floating_panes::FloatingPanes;
use yinz_types::geometry::{PercentOrFixed, Resize};
use yinz_types::input::layout::FloatingPaneLayout;

#[test]
fn tiny_viewports_do_not_underflow_corner_positions_or_shrinking() {
    for cols in 0..=2 {
        for rows in 0..=2 {
            let viewport = Viewport {
                x: 0,
                y: 0,
                cols,
                rows,
            };
            for offset in [0, 2, 100] {
                let right = half_size_top_right_geom(&viewport, offset);
                let bottom = half_size_bottom_left_geom(&viewport, offset);
                let corner = half_size_bottom_right_geom(&viewport, offset);
                assert_eq!((right.x, bottom.y), (0, 0));
                assert_eq!((corner.x, corner.y), (0, 0));
            }
            let geom = PaneGeom {
                cols: Dimension::fixed(cols),
                rows: Dimension::fixed(rows),
                ..Default::default()
            };
            assert_eq!(
                geom_resized_within(
                    geom,
                    viewport,
                    &ResizeStrategy::new(Resize::Decrease, None),
                    (5, 5)
                ),
                geom
            );
        }
    }
}

#[test]
fn floating_layout_is_constrained_by_the_viewport_origin_and_extent() {
    let viewport = Viewport {
        x: 10,
        y: 4,
        cols: 80,
        rows: 30,
    };
    let layout = FloatingPaneLayout {
        x: Some(PercentOrFixed::Fixed(100)),
        y: Some(PercentOrFixed::Fixed(100)),
        width: Some(PercentOrFixed::Fixed(20)),
        height: Some(PercentOrFixed::Fixed(10)),
        ..Default::default()
    };
    let geom = FloatingPanes::position_layout_in_viewport(viewport, &layout, &[]).unwrap();
    assert_eq!((geom.x, geom.y), (70, 24));
    let layout = FloatingPaneLayout {
        x: Some(PercentOrFixed::Fixed(0)),
        y: Some(PercentOrFixed::Fixed(0)),
        ..layout
    };
    let geom = FloatingPanes::position_layout_in_viewport(viewport, &layout, &[]).unwrap();
    // Explicit positions can cover the UI above or to the left of the viewport.
    assert_eq!((geom.x, geom.y), (0, 0));
}
