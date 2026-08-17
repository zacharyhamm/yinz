//! Properties that must hold for *any* sequence of graphics commands, rather than for the
//! particular sequences the example tests spell out.
//!
//! The example-based tests in `kitty_spec.rs` check that the documented cases behave; these check
//! that nothing a hostile or buggy client can send breaks the invariants the rest of the terminal
//! depends on — that replies are always parseable, that placements always refer to images that
//! exist, and that no input makes the pane panic.

use super::*;
use crate::panes::kitty_graphics::test_support::{apc_b64, apc_raw, parse_replies};
use hegel::generators;

/// A control string built from real protocol keys and a mix of sensible and hostile values, so
/// the generated commands reach the interesting code paths instead of bouncing off the grammar.
#[hegel::composite]
fn any_control(tc: &hegel::TestCase) -> String {
    let keys = vec![
        "a", "f", "t", "s", "v", "o", "m", "S", "O", "i", "I", "p", "x", "y", "w", "h", "c", "r",
        "X", "Y", "z", "q", "C", "d", "U", "N", "P", "Q", "H", "V",
    ];
    let values = vec![
        "0",
        "1",
        "2",
        "3",
        "t",
        "T",
        "p",
        "d",
        "q",
        "f",
        "a",
        "c",
        "24",
        "32",
        "100",
        "z",
        "i",
        "I",
        "n",
        "N",
        "A",
        "R",
        "-1",
        "-2147483648",
        "2147483647",
        "4294967295",
        "4294967296",
        "10000",
        "",
        "x",
    ];
    let pair_count = tc.draw(generators::integers::<usize>().max_value(10));
    let mut control = String::new();
    for index in 0..pair_count {
        if index > 0 {
            control.push(',');
        }
        control.push_str(tc.draw(generators::sampled_from(keys.clone())));
        control.push('=');
        control.push_str(tc.draw(generators::sampled_from(values.clone())));
    }
    control
}

/// A command a pane might receive: a graphics APC, or a run of ordinary terminal input to
/// interleave with them.
#[hegel::composite]
fn any_pane_input(tc: &hegel::TestCase) -> Vec<u8> {
    if tc.draw(generators::booleans()) {
        let control = tc.draw(any_control());
        let payload = tc.draw(generators::binary().max_size(64));
        if tc.draw(generators::booleans()) {
            apc_b64(&control, &payload)
        } else {
            apc_raw(&control, &payload)
        }
    } else {
        let text = vec![
            &b"hello"[..],
            &b"\r\n"[..],
            &b"\x1b[2J"[..],
            &b"\x1b[3J"[..],
            &b"\x1b[1K"[..],
            &b"\x1b[5;5H"[..],
            &b"\x1b[?1049h"[..],
            &b"\x1b[?1049l"[..],
            &b"\x1b[1;5r"[..],
            &b"\x1bc"[..],
        ];
        tc.draw(generators::sampled_from(text)).to_vec()
    }
}

#[hegel::test]
fn every_reply_a_pane_sends_is_a_well_formed_response(tc: hegel::TestCase) {
    // An unparseable reply would be delivered to the application as if the user had typed it.
    // `parse_replies` panics on anything that is not a valid response, including one carrying a
    // key the protocol does not define for responses.
    let mut session = KittySession::new(10, 20);
    let input_count = tc.draw(generators::integers::<usize>().max_value(12));

    for _ in 0..input_count {
        let input = tc.draw(any_pane_input());
        session.feed(&input);
        let queued: Vec<u8> = session
            .grid
            .pending_messages_to_pty
            .drain(..)
            .flatten()
            .collect();
        parse_replies(&queued);
    }
}

#[hegel::test]
fn a_pane_never_panics_on_any_graphics_input(tc: hegel::TestCase) {
    let mut session = KittySession::new(10, 20);
    let input_count = tc.draw(generators::integers::<usize>().max_value(12));
    for _ in 0..input_count {
        let input = tc.draw(any_pane_input());
        session.feed(&input);
    }
}

#[hegel::test]
fn every_placement_refers_to_an_image_that_still_exists(tc: hegel::TestCase) {
    // A placement outliving its image would be a dangling reference at render time: the
    // serializer looks the image up by internal id when it builds a frame.
    let mut session = KittySession::new(10, 20);
    let input_count = tc.draw(generators::integers::<usize>().max_value(12));

    for _ in 0..input_count {
        let input = tc.draw(any_pane_input());
        session.feed(&input);

        let store = session.store.borrow();
        for placement in session.grid.kitty_placements() {
            assert!(
                store.get(placement.internal_id).is_some(),
                "placement {:?} outlived its image",
                (placement.image_id, placement.placement_id)
            );
        }
    }
}

#[hegel::test]
fn deleting_everything_always_empties_the_screen(tc: hegel::TestCase) {
    // Whatever a client has managed to put on screen, `a=d,d=A` is the reset that gets rid of
    // it. Without that guarantee there is no way to recover a pane short of restarting it.
    let mut session = KittySession::new(10, 20);
    let input_count = tc.draw(generators::integers::<usize>().max_value(12));
    for _ in 0..input_count {
        let input = tc.draw(any_pane_input());
        session.feed(&input);
    }

    session.feed(&apc_b64("a=d,d=A,q=2", &[]));
    assert_eq!(
        session.visible_placement_count(),
        0,
        "a delete-all left {} placements on screen",
        session.visible_placement_count()
    );
}
