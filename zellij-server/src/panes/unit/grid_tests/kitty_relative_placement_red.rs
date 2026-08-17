//! Relative placements. **These tests fail on purpose.** See the master index in
//! `kitty_graphics/unit/parser_spec_red.rs`.
//!
//! RED-4. Spec, "Relative placements": a placement can be positioned relative to another one
//! rather than to the cursor, with
//!
//! * `P` — the parent image id, `Q` — the parent placement id,
//! * `H`, `V` — the offset in cells from the parent's top-left corner, right and down positive,
//! * `ENOPARENT` when the parent image or placement does not exist,
//! * `EINVAL` when a placement is made its own parent,
//! * `ECYCLE` when the parent chain loops,
//! * `ETOODEEP` when the chain is longer than the terminal supports (at least 8 must work),
//! * and the rule that a relative placement **never moves the cursor**, whatever `C` says.
//!
//! Deleting a parent deletes its descendants, and an image left with no placements is deleted.
//!
//! Today `parser.rs` matches `P`, `Q`, `H` and `V` in the same arm as the keys it deliberately
//! ignores, so `a=p,P=<anything>` is accepted as an ordinary placement: it is positioned at the
//! cursor, it moves the cursor, and a missing parent is never reported. That is worse than not
//! implementing the feature — a client is told its relative placement worked and gets an image
//! in the wrong place.
//!
//! Fix sites: `parser.rs` (parse `P`/`Q`/`H`/`V` into the command), `grid_state.rs::place`
//! (resolve the parent, position from it, refuse cycles and over-deep chains, cascade deletes),
//! `parser.rs::KittyErrorCode` (add `Enoparent`, `Ecycle`, `Etoodeep`).

use super::*;
use crate::panes::kitty_graphics::test_support::{
    placeholder_cells, PlaceholderCell, PlaceholderColor,
};

fn rgb(width: usize, height: usize) -> Vec<u8> {
    rgb_raster(width, height)
}

/// A session with one image transmitted and placed at the origin as placement 1, ready to be a
/// parent. Cells are 10x20 pixels.
fn session_with_a_parent() -> KittySession {
    let mut session = KittySession::new(10, 20);
    session.ok("a=T,f=24,s=10,v=20,i=1,p=1,C=1", &rgb(10, 20));
    session
}

#[test]
fn red_relative_placement_is_positioned_from_its_parent() {
    // Spec: the child's top-left corner is the parent's, offset by H columns and V rows.
    let mut session = session_with_a_parent();
    session.ok("a=t,f=24,s=10,v=20,i=2", &rgb(10, 20));

    // Move the cursor somewhere unrelated: a relative placement must ignore it.
    session.feed(b"\x1b[5;8H");
    session.ok("a=p,i=2,p=1,P=1,Q=1,H=2,V=3", &[]);

    let child = session
        .grid
        .kitty_placements()
        .iter()
        .find(|placement| placement.image_id == 2)
        .expect("the child placement exists");
    assert_eq!(
        child.display_rect.x,
        2 * 10,
        "two columns right of the parent"
    );
    assert_eq!(child.display_rect.y, 3 * 20, "three rows below the parent");
}

#[test]
fn red_relative_placement_does_not_move_the_cursor() {
    // Spec: "Since a relative placement gets its position specified based on another placement,
    // instead of the cursor, the cursor must not move after a relative position, regardless of the
    // value of the `C` key to control cursor movement."
    let mut session = session_with_a_parent();
    session.ok("a=t,f=24,s=10,v=20,i=2", &rgb(10, 20));

    session.feed(b"\x1b[5;8H");
    let before = session.cursor();
    session.ok("a=p,i=2,p=1,P=1,Q=1,H=1,V=1", &[]);

    assert_eq!(session.cursor(), before);
}

#[test]
fn red_relative_placement_with_a_missing_parent_image_is_refused() {
    let mut session = session_with_a_parent();
    session.ok("a=t,f=24,s=10,v=20,i=2", &rgb(10, 20));

    let reply = session.err("a=p,i=2,p=1,P=404,Q=1", &[]);
    assert_eq!(reply.code, "ENOPARENT");
}

#[test]
fn red_relative_placement_with_a_missing_parent_placement_is_refused() {
    let mut session = session_with_a_parent();
    session.ok("a=t,f=24,s=10,v=20,i=2", &rgb(10, 20));

    let reply = session.err("a=p,i=2,p=1,P=1,Q=99", &[]);
    assert_eq!(reply.code, "ENOPARENT");
}

#[test]
fn red_a_placement_cannot_be_its_own_parent() {
    let mut session = session_with_a_parent();
    let reply = session.err("a=p,i=1,p=1,P=1,Q=1", &[]);
    assert_eq!(reply.code, "EINVAL");
}

#[test]
fn red_a_cycle_of_relative_placements_is_refused() {
    // Spec: ECYCLE. Without this check the terminal would loop forever resolving positions.
    let mut session = session_with_a_parent();
    session.ok("a=t,f=24,s=10,v=20,i=2", &rgb(10, 20));
    session.ok("a=p,i=2,p=1,P=1,Q=1,H=1", &[]);

    // Now try to make image 1's placement relative to image 2's, closing the loop.
    let reply = session.err("a=p,i=1,p=1,P=2,Q=1,H=1", &[]);
    assert_eq!(reply.code, "ECYCLE");
}

#[test]
fn red_a_chain_of_eight_relative_placements_is_supported() {
    // Spec: the terminal may cap the chain depth but "must support a depth of at least 8".
    let mut session = session_with_a_parent();
    for id in 2..=9u32 {
        session.ok(&format!("a=t,f=24,s=10,v=20,i={}", id), &rgb(10, 20));
        let reply = session.reply(&format!("a=p,i={},p=1,P={},Q=1,H=1", id, id - 1), &[]);
        assert!(
            reply.is_ok(),
            "depth {} must be supported, got {}:{}",
            id - 1,
            reply.code,
            reply.message
        );
    }
    // Each link is one column right of the last, so the ninth image is eight columns in.
    let last = session
        .grid
        .kitty_placements()
        .iter()
        .find(|placement| placement.image_id == 9)
        .expect("the deepest placement exists");
    assert_eq!(last.display_rect.x, 8 * 10);
}

// ---- where RED-4 meets RED-6: virtual placements as parents ----
//
// These two need unicode placeholder support as well, so they stay red until both land.

#[test]
fn red_relative_a_virtual_placement_cannot_also_be_relative() {
    // Spec, "Relative placements": "Virtual placements created for Unicode placeholder based
    // images cannot also be relative placements. [...] If a client attempts to make a virtual
    // placement relative the terminal must respond with the EINVAL error code."
    let mut session = session_with_a_parent();

    let reply = session.err("a=p,i=1,p=11,P=1,Q=1,U=1,c=2,r=2", &[]);
    assert_eq!(reply.code, "EINVAL");
    assert_eq!(reply.placement_id, Some(11));
}

#[test]
fn red_relative_a_child_of_a_virtual_parent_sits_at_the_placeholder_minimum() {
    // Spec: "When a virtual placement is the parent, its position is derived from all the actual
    // Unicode placeholder images that refer to it. The x position is the minimum of all the
    // placeholder x positions and the y position is the minimum of all the placeholder y
    // positions." That is what makes a one-pixel placeholder a movable anchor for a real image.
    let mut session = KittySession::new(10, 20);
    // A virtual placement of image 42, two cells wide, invisible until placeholders appear.
    session.send("a=T,f=24,s=20,v=20,i=42,U=1,c=2,r=1,q=2", &rgb(20, 20));

    // Materialise it at cells (4, 3) and (5, 3): the minimum is (4, 3).
    session.feed(b"\x1b[4;5H");
    session.feed(
        placeholder_cells(
            PlaceholderColor::Indexed(42),
            None,
            &[PlaceholderCell::at(0, 0), PlaceholderCell::at(0, 1)],
        )
        .as_bytes(),
    );

    session.ok("a=t,f=24,s=10,v=20,i=2", &rgb(10, 20));
    session.feed(b"\x1b[9;9H"); // somewhere unrelated; a relative placement ignores the cursor
    session.ok("a=p,i=2,p=1,P=42,H=1,V=2", &[]);

    let child = session
        .grid
        .kitty_placements()
        .iter()
        .find(|placement| placement.image_id == 2)
        .expect("the child placement exists");
    assert_eq!(child.display_rect.x, (4 + 1) * 10);
    assert_eq!(child.display_rect.y, ((3 + 2) * 20) as isize);
}

#[test]
fn red_deleting_a_parent_placement_deletes_its_children() {
    // Spec: "The lifetime of a relative placement is tied to the lifetime of its parent. If its
    // parent is deleted, it is deleted as well." A child has no position of its own to fall back
    // on.
    let mut session = session_with_a_parent();
    session.ok("a=t,f=24,s=10,v=20,i=2", &rgb(10, 20));
    session.ok("a=p,i=2,p=1,P=1,Q=1,H=1,V=1", &[]);
    assert_eq!(session.placement_count(), 2);

    session.ok("a=d,d=i,i=1,p=1", &[]);
    assert_eq!(session.placement_count(), 0);
}

#[test]
fn red_deleting_a_parent_image_deletes_descendants_and_orphaned_images() {
    // Spec: an image whose last placement is deleted this way is deleted too.
    let mut session = session_with_a_parent();
    session.ok("a=t,f=24,s=10,v=20,i=2", &rgb(10, 20));
    session.ok("a=p,i=2,p=1,P=1,Q=1,H=1,V=1", &[]);

    session.ok("a=d,d=I,i=1", &[]);
    assert_eq!(session.placement_count(), 0);
    assert_eq!(
        session.image_count(),
        0,
        "the child image had no other placements, so it is gone too"
    );
}
