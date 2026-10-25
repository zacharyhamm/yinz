use super::resized_viewport_axis;

// the rows of a 20-row screen with a one-row tab bar on top and a two-row status bar below
const CHROME: (usize, usize, usize) = (1, 17, 20);

#[test]
fn a_screen_that_fits_the_insets_keeps_them() {
    let (start, extent, total) = CHROME;
    assert_eq!(resized_viewport_axis(start, extent, total, 10), (1, 7));
    assert_eq!(resized_viewport_axis(start, extent, total, 40), (1, 37));
    // no chrome at all: the viewport is the whole screen, before and after
    assert_eq!(resized_viewport_axis(0, 20, 20, 2), (0, 2));
}

#[test]
fn a_resize_to_the_size_the_screen_already_is_is_the_identity() {
    // `Tab::relayout_tiled_panes` and `LayoutApplier::adjust_viewport` both resize to the
    // current display area, expecting only a pass through the constraint system
    let (start, extent, total) = CHROME;
    assert_eq!(resized_viewport_axis(start, extent, total, total), (1, 17));
    assert_eq!(resized_viewport_axis(0, 80, 80, 80), (0, 80));
}

#[test]
fn a_screen_smaller_than_the_insets_empties_the_viewport() {
    let (start, extent, total) = CHROME;
    // this used to be `17 + (2 - 20)`, which wrapped around `usize`
    assert_eq!(resized_viewport_axis(start, extent, total, 2), (1, 0));
    // and here the screen is smaller than the leading inset on its own
    assert_eq!(resized_viewport_axis(start, extent, total, 0), (0, 0));
    for new_total in 0..25 {
        let (new_start, new_extent) = resized_viewport_axis(start, extent, total, new_total);
        assert!(
            new_start + new_extent <= new_total,
            "{new_start} + {new_extent} is outside a screen of {new_total}"
        );
    }
}
