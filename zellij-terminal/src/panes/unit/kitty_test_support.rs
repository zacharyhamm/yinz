//! The kitty conformance fixtures for this crate's grid-level and output-level tests.
//!
//! Everything in `zellij_terminal_graphics::kitty_graphics::test_support` (reached here as
//! `crate::panes::kitty_graphics::test_support` and enabled by the `test-support` dev-dependency
//! feature), plus the helpers that drive a [`Grid`], which that crate cannot name: they feed
//! bytes through the APC interceptor the way `TerminalPane::handle_pty_bytes` does and read the
//! replies back off the grid's outbound queue.

pub use crate::panes::kitty_graphics::test_support::*;

use crate::panes::grid::Grid;
use crate::panes::kitty_graphics::interceptor::{InterceptorResult, KittyApcInterceptor};

/// Drive bytes through the interceptor exactly as `TerminalPane::handle_pty_bytes` does: kitty
/// APCs are captured and handed to the grid, everything else reaches vte.
pub fn feed_bytes(
    grid: &mut Grid,
    vte_parser: &mut vte::Parser,
    interceptor: &mut KittyApcInterceptor,
    bytes: &[u8],
) {
    for byte in bytes {
        match interceptor.advance(*byte) {
            InterceptorResult::Forward(forwarded) => {
                vte_parser.advance(grid, forwarded.as_slice());
            },
            InterceptorResult::Swallow => {},
            InterceptorResult::Captured(command) => {
                grid.handle_kitty_apc(&command);
            },
        }
    }
}

/// Send one graphics command and return the replies it produced, draining the grid's outbound
/// queue first so only this command's replies are returned. kitty's `send_command`.
pub fn send_kitty(
    grid: &mut Grid,
    vte_parser: &mut vte::Parser,
    interceptor: &mut KittyApcInterceptor,
    control: &str,
    payload: &[u8],
) -> Vec<KittyReply> {
    grid.pending_messages_to_pty.clear();
    feed_bytes(grid, vte_parser, interceptor, &apc_b64(control, payload));
    drain_replies(grid)
}

/// [`send_kitty`] with the payload already base64-encoded. Chunked uploads have to split the
/// *encoded* stream on a four-character boundary; encoding each raw slice separately only happens
/// to reassemble when every slice but the last is a multiple of three bytes.
pub fn send_kitty_raw(
    grid: &mut Grid,
    vte_parser: &mut vte::Parser,
    interceptor: &mut KittyApcInterceptor,
    control: &str,
    payload_b64: &str,
) -> Vec<KittyReply> {
    grid.pending_messages_to_pty.clear();
    feed_bytes(
        grid,
        vte_parser,
        interceptor,
        &apc_raw(control, payload_b64.as_bytes()),
    );
    drain_replies(grid)
}

/// [`send_kitty`], asserting the command produced at most one reply.
pub fn send_kitty_one(
    grid: &mut Grid,
    vte_parser: &mut vte::Parser,
    interceptor: &mut KittyApcInterceptor,
    control: &str,
    payload: &[u8],
) -> Option<KittyReply> {
    let replies = send_kitty(grid, vte_parser, interceptor, control, payload);
    assert!(
        replies.len() <= 1,
        "expected at most one reply to {:?}, got {:?}",
        control,
        replies
    );
    replies.into_iter().next()
}

/// Take everything the grid has queued for the application and parse it as replies.
pub fn drain_replies(grid: &mut Grid) -> Vec<KittyReply> {
    let queued: Vec<u8> = grid
        .pending_messages_to_pty
        .drain(..)
        .flatten()
        .collect::<Vec<u8>>();
    parse_replies(&queued)
}
