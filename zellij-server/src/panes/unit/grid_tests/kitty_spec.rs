//! Whole-pane graphics conformance: what an application sees when it drives the protocol through
//! a real terminal grid.
//!
//! Derived from `graphics-protocol.rst` ("Querying support and available transmission mediums",
//! "Controlling displayed image layout", "Deleting images", "Interaction with other terminal
//! actions") and from kitty's `test_image_put`, `test_graphics_put_with_pixel_offsets`,
//! `test_gr_scroll`, `test_gr_reset` and `test_gr_delete`.
//!
//! The layers below this one are covered in `kitty_graphics/unit/`; what is specific here is the
//! interaction with the cursor, the scrollback, erase commands and the alternate screen.
//! Deviations live in the three `kitty_*_red.rs` feature files, indexed in the module doc of
//! `kitty_graphics/mod.rs`.

use super::*;

/// `width * height` RGB bytes, the payload for `f=24,s=width,v=height`.
fn rgb(width: usize, height: usize) -> Vec<u8> {
    rgb_raster(width, height)
}

// ---- support queries ----

#[test]
fn a_query_is_answered_so_a_client_can_detect_support() {
    // Spec: a client transmits a one-pixel image with `a=q` and looks for a graphics response
    // before the reply to a following primary device attributes request. Without an answer it
    // must conclude the terminal has no graphics support.
    let mut session = KittySession::new(10, 20);
    let reply = session.reply("a=q,f=24,s=1,v=1,i=31", &rgb(1, 1));
    assert!(reply.is_ok());
    assert_eq!(reply.image_id, Some(31));
}

#[test]
fn a_query_reports_a_problem_with_the_image_rather_than_staying_silent() {
    // The point of the query action is to learn *why* an image would be refused.
    let mut session = KittySession::new(10, 20);
    let reply = session.err("a=q,f=24,s=100,v=100,i=31", &rgb(1, 1));
    assert_eq!(reply.code, "ENODATA");
    assert_eq!(reply.image_id, Some(31));
}

#[test]
fn a_query_leaves_no_image_behind() {
    let mut session = KittySession::new(10, 20);
    session.ok("a=q,f=24,s=10,v=20,i=1", &rgb(10, 20));
    assert_eq!(session.image_count(), 0);
    assert_eq!(session.placement_count(), 0);
}

#[test]
fn a_query_does_not_replace_an_image_that_already_has_the_same_id() {
    // Spec, "Querying support and available transmission mediums": with `a=q` the terminal "will
    // not replace an existing image with the same id, nor will it store the image". Without that,
    // a client probing support would silently destroy an image it had already drawn.
    let mut session = KittySession::new(10, 20);
    session.ok("a=T,f=24,s=10,v=20,i=1,C=1", &rgb(10, 20));
    assert_eq!(session.placement_count(), 1);

    session.ok("a=q,f=24,s=20,v=40,i=1", &rgb(20, 40));

    assert_eq!(session.placement_count(), 1, "the placement is untouched");
    assert_eq!(session.image_count(), 1, "and no second image was stored");
    // The original raster, not the 20x40 one the query carried.
    let placement = &session.grid.kitty_placements()[0];
    assert_eq!(
        (placement.source_rect.width, placement.source_rect.height),
        (10, 20)
    );
    assert!(session.can_still_display_by_placing_it(1));
}

// ---- transmit, display, and the responses along the way ----

#[test]
fn transmitting_then_displaying_takes_two_commands_and_two_replies() {
    let mut session = KittySession::new(10, 20);
    assert!(session
        .reply("a=t,f=24,s=10,v=20,i=1", &rgb(10, 20))
        .is_ok());
    assert_eq!(session.placement_count(), 0);

    assert!(session.reply("a=p,i=1", &[]).is_ok());
    assert_eq!(session.placement_count(), 1);
}

#[test]
fn a_transmit_and_display_does_both_at_once() {
    let mut session = KittySession::new(10, 20);
    session.ok("a=T,f=24,s=10,v=20,i=1", &rgb(10, 20));
    assert_eq!(session.image_count(), 1);
    assert_eq!(session.placement_count(), 1);
}

#[test]
fn an_anonymous_transmission_produces_no_reply_at_all() {
    // Spec: a response is only sent for commands carrying an identifier.
    let mut session = KittySession::new(10, 20);
    let replies = session.send("a=T,f=24,s=10,v=20", &rgb(10, 20));
    assert!(replies.is_empty(), "got {:?}", replies);
    assert_eq!(session.placement_count(), 1, "but the image is still shown");
}

#[test]
fn a_transmission_by_number_reports_the_id_the_terminal_assigned() {
    // Spec: with an image number "the terminal will reply with the id of the newly created image"
    // — "the value of `i` is the id for the newly created image and the value of `I` is the same
    // as was sent in the creation command" — because the client did not choose one.
    let mut session = KittySession::new(10, 20);
    let reply = session.reply("a=T,f=24,s=10,v=20,I=13", &rgb(10, 20));
    assert!(reply.is_ok());
    assert_eq!(reply.image_number, Some(13));
    assert!(reply.image_id.is_some_and(|id| id != 0));
}

#[test]
fn quiet_level_one_hides_the_success_reply_but_not_a_failure() {
    let mut session = KittySession::new(10, 20);
    assert!(session
        .send("a=T,f=24,s=10,v=20,i=1,q=1", &rgb(10, 20))
        .is_empty());
    assert_eq!(session.placement_count(), 1);

    let reply = session.err("a=p,i=404,q=1", &[]);
    assert_eq!(reply.code, "ENOENT");
}

#[test]
fn quiet_level_two_hides_failures_as_well() {
    let mut session = KittySession::new(10, 20);
    assert!(session.send("a=p,i=404,q=2", &[]).is_empty());
}

// ---- cursor movement ----

#[test]
fn the_cursor_lands_past_the_image_it_just_drew() {
    // Spec: "After placing an image on the screen the cursor must be moved to the right by the
    // number of cols in the image placement rectangle and down by the number of rows in the image
    // placement rectangle", so text after an image continues after it.
    let mut session = KittySession::new(10, 20);
    session.ok("a=T,f=24,s=30,v=60,i=1", &rgb(30, 60));
    assert_eq!(session.cursor(), (3, 2));
}

#[test]
fn suppressing_cursor_movement_leaves_the_cursor_where_it_was() {
    // Spec, C=1: "cursor movement policy ... 1 is do not move the cursor". Clients that lay out
    // several images by hand rely on this.
    let mut session = KittySession::new(10, 20);
    session.ok("a=T,f=24,s=30,v=60,i=1,C=1", &rgb(30, 60));
    assert_eq!(session.cursor(), (0, 0));
}

#[test]
fn a_cell_offset_can_push_an_image_into_an_extra_column_and_row() {
    // A 10x20 image exactly fills one cell; starting it five pixels in makes it straddle four.
    let mut session = KittySession::new(10, 20);
    session.ok("a=T,f=24,s=10,v=20,i=1,X=5,Y=5", &rgb(10, 20));
    assert_eq!(session.grid.kitty_placements()[0].dest_cells, (2, 2));
    assert_eq!(session.cursor(), (2, 1));
}

#[test]
fn an_image_reaching_the_right_edge_wraps_the_cursor_to_the_next_line() {
    // A 20-column grid, and an image exactly 20 columns wide.
    let mut session = KittySession::new(10, 20);
    session.ok("a=T,f=24,s=200,v=20,i=1", &rgb(200, 20));
    assert_eq!(session.cursor(), (0, 1));
}

#[test]
fn displaying_an_image_a_second_time_moves_the_cursor_again() {
    let mut session = KittySession::new(10, 20);
    session.ok("a=T,f=24,s=10,v=20,i=1", &rgb(10, 20));
    assert_eq!(session.cursor(), (1, 0));
    session.ok("a=p,i=1,p=2", &[]);
    assert_eq!(session.cursor(), (2, 0));
}

// ---- interaction with erase commands ----

// `ESC[2J` is the one erase command the spec lets touch graphics: "The clear screen escape code
// (usually <ESC>[2J) should also clear all images. This is so that the clear command works."
//
// Zellij implements it by transferring the viewport into the scrollback rather than discarding it,
// the way `clear` behaves in a terminal with history. That splits the rule in two, and the three
// tests below are the three outcomes it produces. What the spec is protecting — that nothing is
// left on the screen — holds in all of them, which is why none of this is a deviation.

#[test]
fn erasing_the_whole_screen_takes_the_images_off_it() {
    // The rule itself, in the terms the spec states it: after `[2J` there is nothing on screen.
    let mut session = KittySession::new(10, 20);
    session.ok("a=T,f=24,s=30,v=60,i=1,C=1", &rgb(30, 60));
    assert_eq!(session.visible_placement_count(), 1);

    session.feed(b"\x1b[2J");
    assert_eq!(session.visible_placement_count(), 0);
}

#[test]
fn erasing_the_whole_screen_frees_an_image_too_tall_to_leave_with_its_line() {
    // `[2J` scrolls one line away, so an image taller than that line is still cut by the top of
    // the viewport afterwards. It cannot follow its text, so it is freed outright — three rows of
    // image against the single row that scrolled.
    let mut session = KittySession::new(10, 20);
    session.ok("a=T,f=24,s=30,v=60,i=1,C=1", &rgb(30, 60));
    assert_eq!(session.grid.kitty_placements()[0].dest_cells, (3, 3));

    session.feed(b"\x1b[2J");

    assert_eq!(session.placement_count(), 0);
    assert_eq!(session.image_count(), 0);
}

#[test]
fn erasing_the_whole_screen_lets_a_short_image_follow_its_line_into_the_scrollback() {
    // The other half, and what keeps the test above from standing alone: a one-cell image is
    // entirely inside the single line `[2J` scrolls away, so it leaves the viewport with that line
    // instead of being freed. It is off the screen either way — which is all the spec asks — but
    // the user can still scroll up to it, the same way they can scroll up to its text.
    let mut session = KittySession::new(10, 20);
    session.ok("a=T,f=24,s=10,v=20,i=1,C=1", &rgb(10, 20));
    assert_eq!(session.grid.kitty_placements()[0].dest_cells, (1, 1));

    session.feed(b"\x1b[2J");

    assert_eq!(
        session.visible_placement_count(),
        0,
        "nothing is left on the screen"
    );
    assert_eq!(
        session.placement_count(),
        1,
        "but the image went to the scrollback with its line rather than being freed"
    );
    assert_eq!(session.image_count(), 1);
}

#[test]
fn a_terminal_reset_clears_the_images_and_their_data() {
    let mut session = KittySession::new(10, 20);
    session.ok("a=T,f=24,s=10,v=20,i=1", &rgb(10, 20));
    session.feed(b"\x1bc");
    assert_eq!(session.placement_count(), 0);
    assert_eq!(session.image_count(), 0);
}

#[test]
fn erasing_the_scrollback_removes_the_images_that_lived_only_there() {
    // `ESC[3J` discards the scrollback, so an image anchored to a line that exists only there
    // goes with it. It is the guard against the complementary rule below overshooting into never
    // deleting anything: an image anchored to a line that no longer exists has nowhere to live.
    let mut session = KittySession::new(3, 20);
    session.ok("a=T,f=24,s=10,v=20,i=1,C=1", &rgb(10, 20));
    session.feed(b"\x1b[3;1H\n\n\n\n");
    assert_eq!(
        session.visible_placement_count(),
        0,
        "the image has scrolled off the top"
    );

    session.feed(b"\x1b[3J");

    assert_eq!(session.placement_count(), 0);
    assert_eq!(session.image_count(), 0);
}

#[test]
fn erasing_the_scrollback_leaves_the_images_on_screen_alone() {
    // Spec, "Interaction with other terminal actions": only `ESC[2J` and a terminal reset may
    // remove images — "the other commands to erase text must have no effect on graphics". `ESC[3J`
    // erases the scrollback and does not touch the screen at all, so a program that trims its
    // scrollback must not lose the image it is currently displaying.
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
fn erasing_the_scrollback_leaves_the_image_data_alone() {
    // The stronger form: not only is the placement still on screen, the image is still there to
    // be displayed again.
    let mut session = KittySession::new(10, 20);
    session.ok("a=T,f=24,s=30,v=60,i=1,C=1", &rgb(30, 60));

    session.feed(b"\x1b[3J");

    assert_eq!(session.image_count(), 1);
    assert!(session.can_still_display_by_placing_it(1));
}

#[test]
fn erasing_to_the_end_of_the_screen_leaves_images_alone() {
    // Spec: "The other commands to erase text must have no effect on graphics. The dedicated
    // delete graphics commands must be used for those." An image is not text, and a client that
    // clears a line must not lose the image drawn over it.
    let mut session = KittySession::new(10, 20);
    session.ok("a=T,f=24,s=10,v=20,i=1,C=1", &rgb(10, 20));
    session.feed(b"\x1b[0J");
    assert_eq!(session.placement_count(), 1);
}

#[test]
fn erasing_to_the_start_of_the_screen_leaves_images_alone() {
    let mut session = KittySession::new(10, 20);
    session.feed(b"\x1b[5;5H");
    session.ok("a=T,f=24,s=10,v=20,i=1,C=1", &rgb(10, 20));
    session.feed(b"\x1b[1J");
    assert_eq!(session.placement_count(), 1);
}

#[test]
fn erasing_within_a_line_leaves_images_alone() {
    let mut session = KittySession::new(10, 20);
    session.ok("a=T,f=24,s=10,v=20,i=1,C=1", &rgb(10, 20));
    for erase in [&b"\x1b[0K"[..], &b"\x1b[1K"[..], &b"\x1b[2K"[..]] {
        session.feed(erase);
        assert_eq!(
            session.placement_count(),
            1,
            "{:?} must not delete images",
            String::from_utf8_lossy(erase)
        );
    }
}

#[test]
fn writing_text_over_an_image_does_not_remove_it() {
    // Images and text occupy the same cells; the z-index decides which is on top.
    let mut session = KittySession::new(10, 20);
    session.ok("a=T,f=24,s=10,v=20,i=1,C=1", &rgb(10, 20));
    session.feed(b"hello");
    assert_eq!(session.placement_count(), 1);
}

// ---- the alternate screen ----

#[test]
fn images_drawn_on_the_alternate_screen_go_away_when_it_does() {
    // Spec: "When switching from the main screen to the alternate screen buffer (1049 private
    // mode) all images in the alternate screen must be cleared, just as all text is cleared."
    let mut session = KittySession::new(10, 20);
    session.feed(b"\x1b[?1049h");
    session.ok("a=T,f=24,s=10,v=20,i=1,C=1", &rgb(10, 20));
    assert_eq!(session.placement_count(), 1);

    session.feed(b"\x1b[?1049l");
    assert_eq!(session.placement_count(), 0);
}

#[test]
fn switching_to_the_alternate_screen_hides_the_main_screens_images() {
    let mut session = KittySession::new(10, 20);
    session.ok("a=T,f=24,s=10,v=20,i=1,C=1", &rgb(10, 20));
    session.feed(b"\x1b[?1049h");
    assert_eq!(session.placement_count(), 0);

    session.feed(b"\x1b[?1049l");
    assert_eq!(
        session.placement_count(),
        1,
        "and restores them on the way back"
    );
}

// ---- scrolling ----

#[test]
fn an_image_scrolls_up_with_the_text_around_it() {
    // Spec: "When scrolling the screen (such as when using index cursor movement commands, or
    // scrolling through the history buffer), images must be scrolled along with text", so an image
    // stays anchored to the line it was drawn on.
    let mut session = KittySession::new(10, 20);
    session.ok("a=T,f=24,s=10,v=20,i=1,C=1", &rgb(10, 20));
    assert_eq!(session.placement_viewport_row(0), 0);

    // Put the cursor on the last row and add a line, which scrolls the viewport up by one.
    session.feed(b"\x1b[10;1H\n");
    assert_eq!(
        session.placement_viewport_row(0),
        -1,
        "the image moved up with the line it was drawn on"
    );
}

#[test]
fn an_image_scrolled_out_of_the_scrollback_is_released() {
    // Otherwise a long-running program that draws images would grow without bound.
    let mut session = KittySession::new(20, 40);
    session.ok("a=T,f=24,s=10,v=20,i=1,C=1", &rgb(10, 20));
    assert_eq!(session.image_count(), 1);

    let scrollback = *SCROLL_BUFFER_SIZE.get().unwrap_or(&10_000);
    for _ in 0..scrollback + 40 {
        session.grid.add_canonical_line();
    }
    assert_eq!(session.placement_count(), 0);
    assert_eq!(session.image_count(), 0);
}

// ---- storage ----

#[test]
fn an_image_larger_than_the_whole_store_is_refused_without_emptying_it() {
    // Spec, "Image persistence and storage quotas": the terminal has a quota and deletes older
    // images to make room — but an image that cannot fit even in an empty store must be refused
    // outright rather than costing the client every image it had already sent.
    //
    // The code is ENOSPC rather than EINVAL, which the specification does not itself require: it
    // names no code for this case. But no amount of eviction can make room for such an image, so
    // the client is being told about storage pressure — worth retrying with a smaller image —
    // where EINVAL cannot be told apart from "your command was malformed".
    let mut session = KittySession::with_quota(10, 20, 10 * 20 * 4 + 1);
    session.ok("a=T,f=32,s=10,v=20,i=1,C=1", &rgba_raster(10, 20));

    let reply = session.err("a=T,f=32,s=100,v=100,i=2", &rgba_raster(100, 100));
    assert_eq!(reply.code, "ENOSPC");
    assert_eq!(reply.image_id, Some(2));
    assert_eq!(
        session.image_count(),
        1,
        "the image already stored survives"
    );
    assert!(session.can_still_display_by_placing_it(1));
}

#[test]
fn an_image_with_no_placements_is_evicted_before_one_being_displayed() {
    // Spec: "when the terminal is running out of quota space for new images, existing images
    // without placements will be preferentially deleted" — an image nothing is showing is the one
    // whose loss the client will notice least.
    // Room for two images of this size, so storing a third must evict one.
    let bytes_per_image = 10 * 20 * 4;
    let mut session = KittySession::with_quota(10, 20, bytes_per_image * 2 + 1);
    session.ok("a=T,f=32,s=10,v=20,i=1,C=1", &rgba_raster(10, 20));
    session.ok("a=t,f=32,s=10,v=20,i=2", &rgba_raster(10, 20));
    session.ok("a=t,f=32,s=10,v=20,i=3", &rgba_raster(10, 20));

    assert!(
        session.can_still_display_by_placing_it(1),
        "the displayed image must survive"
    );
}

// ---- refused commands degrade predictably ----
//
// A client asking for something this terminal will not do must get a well-formed refusal and be
// able to carry on. This is the *degradation* contract, not a feature contract.

#[test]
fn the_stream_recovers_after_a_refused_command() {
    // A refusal must not leave the parser or the grid in a state that swallows what follows.
    let mut session = KittySession::new(10, 20);
    // A frame for an image that was never transmitted: refused, and nothing to refuse it against.
    session.err("a=f,f=24,s=10,v=20,i=1", &rgb(10, 20));
    session.ok("a=T,f=24,s=10,v=20,i=9", &rgb(10, 20));
    assert_eq!(session.placement_count(), 1);
    session.feed(b"text after");
    assert_eq!(session.placement_count(), 1);
}

// ---- the protocol can be turned off entirely ----

#[test]
fn a_disabled_protocol_answers_nothing_and_stores_nothing() {
    // With the protocol disabled a graphics command must be swallowed whole: no reply for the
    // application to wait on, and no bytes leaking into the visible grid.
    let mut session = KittySession::new(10, 20);
    session
        .grid
        .update_kitty_host_support(KittyHostSupport::ProtocolDisabled);

    assert!(session.send("a=q,f=24,s=1,v=1,i=1", &rgb(1, 1)).is_empty());
    assert!(session
        .send("a=T,f=24,s=10,v=20,i=1", &rgb(10, 20))
        .is_empty());
    assert_eq!(session.placement_count(), 0);
    assert_eq!(session.image_count(), 0);
}

#[test]
fn a_query_is_refused_when_the_host_terminal_cannot_show_images() {
    // Zellij can only display an image if the terminal it is running inside can. Answering OK
    // would make clients draw images nobody would ever see.
    let mut session = KittySession::new(10, 20);
    session
        .grid
        .update_kitty_host_support(KittyHostSupport::Unsupported);

    let reply = session.err("a=q,f=24,s=1,v=1,i=1", &rgb(1, 1));
    assert_eq!(reply.code, "ENOTSUPPORTED");
}
