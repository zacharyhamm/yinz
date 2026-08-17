//! Unicode placeholders.
//!
//! Spec, "Unicode placeholders": instead of positioning an image itself, a client creates
//! a *virtual placement* with `U=1` and then draws the placeholder character `U+10EEEE` in the
//! cells where the image should appear. The terminal fills those cells from the image. This is
//! how a program that does not control the cursor — anything inside a multiplexer, or a TUI
//! library that only knows how to print text — displays images at all, which is exactly the case
//! zellij is in.
//!
//! The encoding of each placeholder cell:
//!
//! * the image id is carried in the cell's **foreground colour**, either as a 256-colour index
//!   (low byte only) or as a 24-bit colour (low three bytes);
//! * the **row and column** within the image come from up to two combining marks taken from
//!   kitty's `rowcolumn-diacritics.txt`;
//! * a **third** combining mark supplies the most significant byte of the image id;
//! * the **underline colour** selects among several virtual placements of the same image;
//! * cells with fewer marks inherit from the cell to their left: no marks at all means the same
//!   row and the next column, and a row-only cell means the next column of that row. Inheritance
//!   requires the same foreground and underline colour.
//!
//! A virtual placement is invisible until placeholder cells exist, and the image is fitted into
//! the `c` x `r` box preserving its aspect ratio.
//!
//! These tests assert only externally observable state — reply codes, placement counts and
//! placement geometry. kitty's assertions on renderer `src_rect` fractions map onto
//! `KittyPlacement::source_rect` in pixels: a source rect of `left=0, top=0, right=0.5,
//! bottom=0.5` on a 20x20 image is `x=0, y=0, width=10, height=10`.
//!
//! The implementation is spread over `placeholders.rs` (the diacritic table), `parser.rs` (the
//! `U` key), `grid_state.rs` (`VirtualPlacement` and the placements derived from it) and
//! `grid.rs` (`Row::placeholder_marks`, and the resolution pass that rebuilds the derived
//! placements from the viewport).

use super::*;
use crate::panes::kitty_graphics::test_support::{
    placeholder_cells, PlaceholderCell, PlaceholderColor,
};

fn rgb(width: usize, height: usize) -> Vec<u8> {
    rgb_raster(width, height)
}

/// Transmit image `id` and create a virtual placement of it `columns` x `rows` cells in size.
///
/// `q=2` because that is what the spec's recipe prescribes — "create an image as you would
/// normally with the graphics protocol with (`q=2`)" — so that the terminal's replies cannot
/// confuse the host application that is only printing text. `q=2` suppresses success *and*
/// failure responses, so the conforming outcome is silence: the assertion here is that nothing
/// came back, and whether the placement was really created is asserted by each test through what
/// the placeholder cells then produce.
fn virtual_placement(session: &mut KittySession, id: u32, columns: u32, rows: u32) {
    let replies = session.send(
        &format!(
            "a=T,f=24,s=20,v=20,i={},U=1,c={},r={},q=2",
            id, columns, rows
        ),
        &rgb(20, 20),
    );
    assert!(
        replies.is_empty(),
        "a q=2 command must answer nothing, got {:?}",
        replies
    );
}

#[test]
fn a_virtual_placement_is_accepted() {
    let mut session = KittySession::new(10, 20);
    let reply = session.reply("a=T,f=24,s=20,v=20,i=1,U=1,c=2,r=2", &rgb(20, 20));
    assert!(reply.is_ok(), "got {}:{}", reply.code, reply.message);
}

#[test]
fn a_virtual_placement_shows_nothing_until_placeholder_cells_exist() {
    // Spec: "Virtual placements are invisible and only play the role of prototypes for real
    // images"; the placeholder cells are what instantiate them.
    let mut session = KittySession::new(10, 20);
    virtual_placement(&mut session, 1, 2, 2);
    assert_eq!(session.placement_count(), 0, "nothing yet");

    // The "until" half, which is also what keeps this test honest: without it the assertion above
    // would hold just as well in a terminal that ignored `U=1` altogether.
    session.feed(
        placeholder_cells(
            PlaceholderColor::Indexed(1),
            None,
            &[PlaceholderCell::at(0, 0)],
        )
        .as_bytes(),
    );
    assert_eq!(session.placement_count(), 1, "and now there is one");
}

#[test]
fn placeholder_cells_materialise_the_image() {
    let mut session = KittySession::new(10, 20);
    virtual_placement(&mut session, 1, 2, 2);

    session.feed(
        placeholder_cells(
            PlaceholderColor::Indexed(1),
            None,
            &[PlaceholderCell::at(0, 0), PlaceholderCell::at(0, 1)],
        )
        .as_bytes(),
    );

    assert_eq!(session.placement_count(), 1);
}

#[test]
fn contiguous_cells_in_a_row_become_one_placement() {
    // Spec: the terminal is free to coalesce, and a run of adjacent cells of the same image is
    // one rectangle. Two cells of a 2x2 image cover its left and right halves.
    let mut session = KittySession::new(10, 20);
    virtual_placement(&mut session, 1, 2, 2);

    session.feed(
        placeholder_cells(
            PlaceholderColor::Indexed(1),
            None,
            &[PlaceholderCell::at(0, 0), PlaceholderCell::at(0, 1)],
        )
        .as_bytes(),
    );

    assert_eq!(session.placement_count(), 1);
    let placement = &session.grid.kitty_placements()[0];
    assert_eq!(placement.dest_cells, (2, 1));
    assert_eq!(
        placement.source_rect.width, 20,
        "the full width of the image"
    );
    assert_eq!(placement.source_rect.height, 10, "the top half of it");
}

#[test]
fn cells_that_are_not_adjacent_stay_separate() {
    let mut session = KittySession::new(10, 20);
    virtual_placement(&mut session, 1, 2, 2);

    let mut text = placeholder_cells(
        PlaceholderColor::Indexed(1),
        None,
        &[PlaceholderCell::at(0, 0)],
    );
    text.push_str("  ");
    text.push_str(&placeholder_cells(
        PlaceholderColor::Indexed(1),
        None,
        &[PlaceholderCell::at(0, 1)],
    ));
    session.feed(text.as_bytes());

    assert_eq!(session.placement_count(), 2);
}

#[test]
fn each_cell_shows_its_own_part_of_the_image() {
    // The second row of a 2x2 placement is the bottom half of the image.
    let mut session = KittySession::new(10, 20);
    virtual_placement(&mut session, 1, 2, 2);

    session.feed(
        placeholder_cells(
            PlaceholderColor::Indexed(1),
            None,
            &[PlaceholderCell::at(1, 0), PlaceholderCell::at(1, 1)],
        )
        .as_bytes(),
    );

    let placement = &session.grid.kitty_placements()[0];
    assert_eq!(
        placement.source_rect.y, 10,
        "the bottom half starts halfway down"
    );
    assert_eq!(placement.source_rect.height, 10);
}

#[test]
fn the_image_id_can_be_carried_in_a_24_bit_foreground_colour() {
    // Spec: with a 24-bit colour the low three bytes of the id are the red, green and blue
    // components. Here: id 0x010203.
    let mut session = KittySession::new(10, 20);
    virtual_placement(&mut session, 0x010203, 1, 1);

    session.feed(
        placeholder_cells(
            PlaceholderColor::Rgb(1, 2, 3),
            None,
            &[PlaceholderCell::at(0, 0)],
        )
        .as_bytes(),
    );

    assert_eq!(session.placement_ids(), vec![(0x010203, 0)]);
}

#[test]
fn the_image_id_can_be_carried_in_a_256_colour_index() {
    let mut session = KittySession::new(10, 20);
    virtual_placement(&mut session, 42, 1, 1);

    session.feed(
        placeholder_cells(
            PlaceholderColor::Indexed(42),
            None,
            &[PlaceholderCell::at(0, 0)],
        )
        .as_bytes(),
    );

    assert_eq!(session.placement_ids(), vec![(42, 0)]);
}

#[test]
fn a_third_combining_mark_supplies_the_high_byte_of_the_id() {
    // Spec: the id's most significant byte cannot fit in a colour, so it gets its own mark.
    // Mark index 42 with a foreground of 42 gives id 42 | (42 << 24).
    let image_id = 42 | (42u32 << 24);
    let mut session = KittySession::new(10, 20);
    virtual_placement(&mut session, image_id, 1, 1);

    session.feed(
        placeholder_cells(
            PlaceholderColor::Indexed(42),
            None,
            &[PlaceholderCell::at(0, 0).with_id_msb(42)],
        )
        .as_bytes(),
    );

    assert_eq!(session.placement_ids(), vec![(image_id, 0)]);
}

#[test]
fn a_placeholder_for_an_unknown_id_shows_nothing() {
    // A stale placeholder in the scrollback must not conjure up whatever image now holds that id
    // — it simply has nothing to show.
    let mut session = KittySession::new(10, 20);
    virtual_placement(&mut session, 1, 1, 1);

    session.feed(
        placeholder_cells(
            PlaceholderColor::Indexed(200),
            None,
            &[PlaceholderCell::at(0, 0)],
        )
        .as_bytes(),
    );

    assert_eq!(session.placement_count(), 0, "image 200 does not exist");

    // The positive control: a placeholder naming the id that *does* exist must show something,
    // otherwise this test would pass in a terminal with no placeholder support at all.
    session.feed(b"\r\n");
    session.feed(
        placeholder_cells(
            PlaceholderColor::Indexed(1),
            None,
            &[PlaceholderCell::at(0, 0)],
        )
        .as_bytes(),
    );
    assert_eq!(session.placement_count(), 1, "image 1 does");
}

#[test]
fn a_cell_with_no_marks_continues_the_cell_to_its_left() {
    // Spec: "If no diacritics are present, and the previous placeholder cell has the same
    // foreground and underline colors, then the row of the current cell will be the row of the
    // cell to the left, the column will be the column of the cell to the left plus one, and the
    // most significant image ID byte will be the most significant image ID byte of the cell to the
    // left." This is what lets a client emit a long run cheaply.
    let mut session = KittySession::new(10, 20);
    virtual_placement(&mut session, 1, 2, 2);

    session.feed(
        placeholder_cells(
            PlaceholderColor::Indexed(1),
            None,
            &[PlaceholderCell::at(0, 0), PlaceholderCell::inherited()],
        )
        .as_bytes(),
    );

    assert_eq!(session.placement_count(), 1);
    assert_eq!(session.grid.kitty_placements()[0].dest_cells, (2, 1));
}

#[test]
fn a_cell_with_only_a_row_mark_takes_the_next_column() {
    let mut session = KittySession::new(10, 20);
    virtual_placement(&mut session, 1, 2, 2);

    let mut cells = vec![PlaceholderCell::at(0, 0)];
    cells.push(PlaceholderCell {
        row: Some(0),
        column: None,
        id_most_significant_byte: None,
    });
    session.feed(placeholder_cells(PlaceholderColor::Indexed(1), None, &cells).as_bytes());

    assert_eq!(session.placement_count(), 1);
    assert_eq!(session.grid.kitty_placements()[0].dest_cells, (2, 1));
}

#[test]
fn the_underline_colour_chooses_between_placements_of_one_image() {
    // Spec: "You can also specify a placement ID using the underline color (if it's omitted or
    // zero, the terminal may choose any virtual placement of the given image)." One image can have
    // several virtual placements at different sizes; the cell says which one it belongs to.
    // `q=2` per the spec's recipe, so none of these three answer anything.
    let mut session = KittySession::new(10, 20);
    session.send("a=t,f=24,s=20,v=20,i=1,q=2", &rgb(20, 20));
    session.send("a=p,i=1,p=1,U=1,c=1,r=1,q=2", &[]);
    session.send("a=p,i=1,p=2,U=1,c=2,r=1,q=2", &[]);

    session.feed(
        placeholder_cells(
            PlaceholderColor::Indexed(1),
            Some(2),
            &[PlaceholderCell::at(0, 0)],
        )
        .as_bytes(),
    );

    assert_eq!(
        session.placement_ids(),
        vec![(1, 2)],
        "the underline colour selected placement 2"
    );
    assert_eq!(
        session.grid.kitty_placements()[0].source_rect.width,
        10,
        "one cell of a two-cell-wide placement is half the image"
    );
}

#[test]
fn the_image_is_fitted_to_the_box_preserving_its_aspect_ratio() {
    // Spec: "The image will eventually be fit to the specified rectangle, its aspect ratio
    // preserved". The box has to be a different shape from the image for that to mean anything:
    // a 4x1 cell box is 40x20 pixels, and the 20x20 image fitted into it stays 20x20 rather than
    // stretching to 40x20.
    let mut session = KittySession::new(10, 20);
    virtual_placement(&mut session, 1, 4, 1);

    session.feed(
        placeholder_cells(
            PlaceholderColor::Indexed(1),
            None,
            &[
                PlaceholderCell::at(0, 0),
                PlaceholderCell::at(0, 1),
                PlaceholderCell::at(0, 2),
                PlaceholderCell::at(0, 3),
            ],
        )
        .as_bytes(),
    );

    assert_eq!(
        session.placement_count(),
        1,
        "the placeholder cells must materialise the virtual placement first"
    );
    let placement = &session.grid.kitty_placements()[0];
    assert_eq!(
        (placement.display_rect.width, placement.display_rect.height),
        (20, 20),
        "a square image in a wide box keeps its shape instead of filling the box"
    );
}

#[test]
fn erasing_the_line_removes_the_images_it_held() {
    // The placeholder cells *are* the placement; erasing them erases it. This is the one case
    // where an erase command is supposed to affect graphics.
    let mut session = KittySession::new(10, 20);
    virtual_placement(&mut session, 1, 2, 2);
    session.feed(
        placeholder_cells(
            PlaceholderColor::Indexed(1),
            None,
            &[PlaceholderCell::at(0, 0), PlaceholderCell::at(0, 1)],
        )
        .as_bytes(),
    );
    assert_eq!(session.placement_count(), 1);

    session.feed(b"\x1b[1;1H\x1b[2K");
    assert_eq!(session.placement_count(), 0);
}

#[test]
fn geometric_deletes_do_not_touch_virtual_placements() {
    // Spec: a virtual placement can only be deleted by `d` in `i, I, n, N, r, R`. The others are
    // positional, and a virtual placement has no position.
    let mut session = KittySession::new(10, 20);
    virtual_placement(&mut session, 1, 2, 2);

    // These deletes name no image, so the protocol gives them no response to wait for.
    for specifier in ["a", "A", "c", "C", "z", "Z"] {
        session.send(&format!("a=d,d={},z=0", specifier), &[]);
    }

    // The placement is still there to be instantiated.
    session.feed(
        placeholder_cells(
            PlaceholderColor::Indexed(1),
            None,
            &[PlaceholderCell::at(0, 0)],
        )
        .as_bytes(),
    );
    assert_eq!(session.placement_count(), 1);
}

#[test]
fn deleting_by_image_id_does_remove_a_virtual_placement() {
    // Spec: "Virtual placements can be deleted by a deletion command only when the `d` key is
    // equal to `i`, `I`, `r`, `R`, `n` or `N`."
    let mut session = KittySession::new(10, 20);
    virtual_placement(&mut session, 1, 2, 2);

    // First show that the placement is there to delete: without this the assertion below would
    // hold in a terminal that never created it.
    let cells = placeholder_cells(
        PlaceholderColor::Indexed(1),
        None,
        &[PlaceholderCell::at(0, 0)],
    );
    session.feed(cells.as_bytes());
    assert_eq!(session.placement_count(), 1);
    session.feed(b"\x1b[1;1H\x1b[2K");

    session.reply("a=d,d=I,i=1", &[]);

    session.feed(cells.as_bytes());
    assert_eq!(session.placement_count(), 0);
}
