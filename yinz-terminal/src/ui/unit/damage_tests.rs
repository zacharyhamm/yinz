use super::*;
use yinz_types::pane_size::Dimension;

fn rect(x: usize, y: usize, cols: usize, rows: usize) -> CellRect {
    CellRect { x, y, cols, rows }
}

fn geom(x: usize, y: usize, cols: usize, rows: usize) -> PaneGeom {
    PaneGeom {
        x,
        y,
        cols: Dimension::fixed(cols),
        rows: Dimension::fixed(rows),
        ..Default::default()
    }
}

#[test]
fn nothing_moved_is_nothing_to_repaint() {
    let painted = [rect(10, 2, 20, 4)];
    assert!(overlay_damage(&painted, &painted).is_empty());
}

#[test]
fn an_overlay_that_only_grew_damages_nothing() {
    // the whole entry animation, which is why arriving costs no repaints
    let before = [rect(10, 2, 8, 4)];
    let after = [rect(10, 2, 20, 4)];
    assert!(overlay_damage(&before, &after).is_empty());
}

#[test]
fn a_right_anchored_slide_out_damages_the_columns_it_left_behind() {
    // the box's right edge is pinned at 30; as it shrinks, `x` marches rightwards
    let before = [rect(10, 2, 20, 3)];
    let after = [rect(22, 2, 8, 3)];
    assert_eq!(overlay_damage(&before, &after), vec![rect(10, 2, 12, 3)]);
}

#[test]
fn a_left_anchored_slide_out_damages_the_columns_it_left_behind() {
    // the same bug at the other anchor: `x` is pinned and the far edge retreats
    let before = [rect(10, 2, 20, 3)];
    let after = [rect(10, 2, 8, 3)];
    assert_eq!(overlay_damage(&before, &after), vec![rect(18, 2, 12, 3)]);
}

#[test]
fn a_one_row_glide_damages_only_the_row_it_vacated() {
    let before = [rect(10, 2, 20, 3)];
    let after = [rect(10, 3, 20, 3)];
    assert_eq!(overlay_damage(&before, &after), vec![rect(10, 2, 20, 1)]);
}

#[test]
fn a_vacated_block_comes_back_as_one_rect_rather_than_one_per_row() {
    let before = [rect(10, 2, 20, 4)];
    let after: [CellRect; 0] = [];
    assert_eq!(overlay_damage(&before, &after), vec![rect(10, 2, 20, 4)]);
}

#[test]
fn a_cell_one_overlay_vacated_but_another_still_covers_is_not_damaged() {
    // two toasts transiently overlapping while one glides past the other
    let before = [rect(10, 2, 20, 2), rect(10, 3, 20, 2)];
    let after = [rect(10, 3, 20, 2)];
    assert_eq!(
        overlay_damage(&before, &after),
        vec![rect(10, 2, 20, 1)],
        "row 2 is uncovered; rows 3 and 4 are still covered by the survivor"
    );
}

#[test]
fn overlapping_coverage_never_produces_a_negative_or_doubled_span() {
    let before = [rect(10, 2, 20, 1), rect(15, 2, 20, 1)];
    let after = [rect(12, 2, 4, 1)];
    assert_eq!(
        overlay_damage(&before, &after),
        vec![rect(10, 2, 2, 1), rect(16, 2, 19, 1)],
        "the union 10..35 minus 12..16"
    );
}

#[test]
fn an_empty_rect_paints_nothing_and_damages_nothing() {
    // progress 0.0 - the toast exists but has no visible columns yet
    let before = [rect(10, 2, 0, 3)];
    let after = [rect(10, 2, 4, 3)];
    assert!(overlay_damage(&before, &after).is_empty());
    assert_eq!(
        overlay_damage(&after, &before),
        vec![rect(10, 2, 4, 3)],
        "shrinking back to nothing damages what was showing"
    );
}

#[test]
fn intersects_geom_excludes_panes_that_only_touch() {
    let toast = rect(10, 2, 20, 4);
    assert!(
        toast.intersects_geom(&geom(0, 0, 40, 10)),
        "fully overlapping"
    );
    assert!(
        toast.intersects_geom(&geom(29, 5, 10, 10)),
        "one shared cell"
    );
    assert!(
        !toast.intersects_geom(&geom(30, 2, 10, 4)),
        "starts exactly where the toast ends"
    );
    assert!(
        !toast.intersects_geom(&geom(0, 6, 40, 4)),
        "starts exactly on the row below the toast"
    );
    assert!(!toast.intersects_geom(&geom(0, 0, 10, 2)), "above and left");
    assert!(!rect(10, 2, 0, 4).intersects_geom(&geom(0, 0, 40, 10)));
}
