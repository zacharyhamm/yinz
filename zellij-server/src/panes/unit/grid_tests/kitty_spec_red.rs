//! Grid-level deviations from the kitty graphics specification. **These tests fail on purpose.**
//! See the master index in `kitty_graphics/unit/parser_spec_red.rs`.
//!
//! Two entries live here: RED-2, the effect of `ESC[3J` on graphics, which is a genuine deviation;
//! and RED-8, the error code a storage-quota refusal carries, which is a chosen improvement rather
//! than something the specification requires.
//!
//! RED-2. Spec, "Interaction with other terminal actions":
//!
//! > The other commands to erase text must have no effect on graphics. The dedicated delete
//! > graphics commands must be used for those.
//!
//! Only `ESC[2J` and a terminal reset are given permission to remove images. `ESC[3J` erases the
//! *scrollback* — it does not touch the screen at all — so it must leave the images on screen
//! alone. Today the `clear_type == 3` branch calls `KittyGrid::clear_all_placements`, which frees
//! every placement in the pane, so a program that trims its scrollback loses the image it is
//! currently displaying.
//!
//! Fix site: `grid.rs`, the ED branch for `clear_type == 3`. `clear_lines_above()` already drops
//! the scrolled-off rows; what the kitty grid needs is the matching sweep of placements that sit
//! *above* the viewport, not all of them.
//!
//! RED-8 is **not** a spec deviation — it is a deliberate improvement, kept in the red set because
//! it is work that has not been done rather than a decision that has been made.
//!
//! Read the specification carefully and it requires nothing here. "Image persistence and storage
//! quotas" says only that a terminal "should have a maximum storage quota" and "should delete
//! older images to make space for the new one"; it names no error code. `ENOSPC` appears once in
//! the whole document, in the "Composing animation frames" note, and it is about frame storage:
//! "if this happens and there is not enough storage space, kitty will respond with ENOSPC". kitty's
//! own `test_graphics_quota_enforcement` matches that reading — its `ENOSPC` assertion is on the
//! *animation frame* quota, and a plain image that overflows the quota is answered `OK` after
//! eviction.
//!
//! What is left is a usability argument, and it is a good one: an image larger than the entire
//! quota cannot be made to fit by evicting anything, so the client is being told about storage
//! pressure. `EINVAL` cannot be told apart from "your command was malformed". One is worth
//! retrying with a smaller image; the other is not. That the oversized image is *refused*, and
//! that refusing it does not cost the client the images it had already sent, is already asserted
//! in the green gate by
//! `kitty_spec::an_image_larger_than_the_whole_store_is_refused_without_emptying_it` — only the
//! code is outstanding.
//!
//! Fix sites: `store.rs::store_image`, the `size > self.quota_bytes` branch; a new
//! `KittyErrorCode::Enospc` and its arm in `parser.rs::KittyErrorCode::as_str`; the enumeration in
//! `replies_tests::every_error_code_renders_as_its_errno_name`. Note that
//! `store_tests::single_image_exceeding_quota_is_refused_with_einval_and_store_unchanged` pins
//! today's code and must be renamed along with the fix.

use super::*;

fn rgb(width: usize, height: usize) -> Vec<u8> {
    rgb_raster(width, height)
}

#[test]
fn red_ed3_leaves_the_images_on_screen_alone() {
    let mut session = KittySession::new(10, 20);
    session.ok("a=T,f=24,s=30,v=60,i=1,C=1", &rgb(30, 60));
    assert_eq!(session.visible_placement_count(), 1);

    session.feed(b"\x1b[3J");

    assert_eq!(
        session.visible_placement_count(),
        1,
        "erasing the scrollback must not remove an image from the screen"
    );
}

#[test]
fn red_ed3_leaves_the_image_data_alone() {
    // The stronger form: not only is the placement still on screen, the image is still there to
    // be displayed again.
    let mut session = KittySession::new(10, 20);
    session.ok("a=T,f=24,s=30,v=60,i=1,C=1", &rgb(30, 60));

    session.feed(b"\x1b[3J");

    assert_eq!(session.image_count(), 1);
    assert!(session.can_still_display_by_placing_it(1));
}

#[test]
fn red_an_image_larger_than_the_whole_store_reports_enospc() {
    // RED-8. The image is larger than the entire quota, so no amount of eviction makes room for
    // it: this is storage pressure, not a malformed command.
    let mut session = KittySession::with_quota(10, 20, 1024);

    let reply = session.err("a=T,f=32,s=100,v=100,i=1", &rgba_raster(100, 100));
    assert_eq!(reply.code, "ENOSPC");
    assert_eq!(reply.image_id, Some(1));
    assert_eq!(session.image_count(), 0);
}

// The other half of the RED-2 rule — ED-3 *does* discard images that live only in the
// scrollback — is behaviour the implementation already gets right, so it is a green test:
// `kitty_spec::erasing_the_scrollback_removes_the_images_that_lived_only_there`. It is there so
// that fixing the above does not overshoot into never deleting anything.
