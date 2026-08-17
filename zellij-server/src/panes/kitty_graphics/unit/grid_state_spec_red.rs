//! Placement-model deviations from the kitty graphics specification. **These tests fail on
//! purpose.** See the master index in `parser_spec_red.rs`.
//!
//! RED-3. Spec, "Controlling displayed image layout":
//!
//! > You can also ask the terminal emulator to display the image in a specified rectangle (num of
//! > columns / num of lines), using the control codes `c,r`. [...] Note that if you specify a
//! > start cell offset via the `X,Y` keys, it is not added to the number of rows/columns. If only
//! > one of either `r` or `c` is specified, the other one is computed based on the source image
//! > aspect ratio, so that the image is displayed without distortion.
//!
//! `KittyGrid::place` honours that for the `(c, r)` case, but its single-axis branches add the
//! *perpendicular* cell offset into the axis they derive: the `(c, 0)` branch computes the height
//! from `dst_w + off_x` and the `(0, r)` branch computes the width from `dst_h + off_y`. Two
//! things are wrong with that — the offset should not be added at all, and the axis it is added
//! to is not the one it belongs to. The visible effect is an image that claims one more row or
//! column than it occupies, so the cursor lands in the wrong place and neighbouring text is
//! pushed away from the image.
//!
//! Fix site: `grid_state.rs::place`, the `(c, 0)` and `(0, r)` match arms — the derived axis is
//! `ceil(dst_h / cell.height)` and `ceil(dst_w / cell.width)` respectively.

use super::super::grid_state::{KittyGrid, KittyVerticalAnchor};
use super::super::parser::{KittyCommand, KittyCommandParser};
use super::super::store::KittyImageStore;
use super::super::test_support::*;
use std::cell::RefCell;
use std::rc::Rc;
use zellij_utils::pane_size::SizeInPixels;

const CELL: SizeInPixels = SizeInPixels {
    width: 10,
    height: 20,
};

/// Transmit and display one image, returning the cell rectangle the placement claims.
fn place_cells(control: &str, image_width: usize, image_height: usize) -> (u16, u16) {
    let store = Rc::new(RefCell::new(KittyImageStore::default()));
    let mut kitty = KittyGrid::new(Rc::new(RefCell::new(Some(CELL))), store);
    let mut parser = KittyCommandParser::new();

    let payload = rgb_block(image_width, image_height);
    let command: KittyCommand = parser
        .parse(unwrap_apc(&apc_b64(control, &payload)))
        .expect("a complete command")
        .expect("a valid command");
    let image = command.image.clone().expect("the command carries an image");
    let id = kitty.transmit(&command, image).expect("transmit succeeds");
    let internal = *kitty.pane_image_id_map().get(&id).expect("image is mapped");

    kitty
        .place(
            id,
            internal,
            &command,
            (0, 0),
            CELL,
            KittyVerticalAnchor {
                canonical_line: 0,
                offset_px_from_line_start: 0,
            },
        )
        .expect("place succeeds")
}

#[test]
fn red_scaling_by_columns_ignores_the_horizontal_cell_offset_when_deriving_rows() {
    // A square 40x40 image scaled to 4 columns is 40px wide, and by aspect ratio 40px tall,
    // which is exactly two 20px rows. A start offset within the first cell does not change how
    // many rows the image needs.
    assert_eq!(
        place_cells("a=T,f=24,s=40,v=40,i=1,c=4", 40, 40),
        (4, 2),
        "sanity: without an offset the derived row count is two"
    );
    assert_eq!(
        place_cells("a=T,f=24,s=40,v=40,i=1,c=4,X=5", 40, 40),
        (4, 2),
        "the X offset must not be added to the derived row count"
    );
}

#[test]
fn red_scaling_by_rows_ignores_the_vertical_cell_offset_when_deriving_columns() {
    // The mirror image: 40x40 scaled to 2 rows is 40px tall and 40px wide, four 10px columns.
    assert_eq!(
        place_cells("a=T,f=24,s=40,v=40,i=1,r=2", 40, 40),
        (4, 2),
        "sanity: without an offset the derived column count is four"
    );
    assert_eq!(
        place_cells("a=T,f=24,s=40,v=40,i=1,r=2,Y=5", 40, 40),
        (4, 2),
        "the Y offset must not be added to the derived column count"
    );
}
