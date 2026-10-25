use super::pane_content_offset;
use yinz_types::pane_size::{Dimension, PaneGeom, Viewport};

fn geom(x: usize, cols: usize, y: usize, rows: usize) -> PaneGeom {
    PaneGeom {
        x,
        y,
        cols: Dimension::fixed(cols),
        rows: Dimension::fixed(rows),
        ..Default::default()
    }
}

#[test]
fn offsets_are_reserved_relative_to_the_viewport_origin() {
    // a viewport that does not start at the screen origin
    let viewport = Viewport {
        x: 20,
        y: 0,
        cols: 80,
        rows: 40,
    };

    // a pane whose right edge lands mid-viewport still needs a boundary column, even though
    // its absolute right edge (85) is past `viewport.cols` (80)
    assert_eq!(pane_content_offset(&geom(20, 65, 0, 40), &viewport).0, 1);
    // a pane flush with the viewport's right edge does not
    assert_eq!(pane_content_offset(&geom(20, 80, 0, 40), &viewport).0, 0);
}

#[test]
fn offsets_are_unchanged_for_an_origin_viewport() {
    let viewport = Viewport {
        x: 0,
        y: 0,
        cols: 100,
        rows: 40,
    };
    assert_eq!(pane_content_offset(&geom(0, 100, 0, 40), &viewport), (0, 0));
    assert_eq!(pane_content_offset(&geom(0, 50, 0, 20), &viewport), (1, 1));
}
