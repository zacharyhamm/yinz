//! The kitty graphics protocol: parsing commands, storing images, placing them on the grid.
//!
//! # Master index of the red set
//!
//! The conformance suite is written against `graphics-protocol.rst`, not against this code. Where
//! the two disagree the test stays failing on purpose, and the red set **is** the to-do list for
//! the protocol work. Every expected-red test lives in a module ending `_red` and is named
//! `red_*` — filter on the **module path**, not the name prefix, because `red_` also appears
//! inside unrelated names such as `test_has_rendered_assets`.
//!
//! ```sh
//! cargo x test -- --skip _red::                          # the green gate
//! cargo test -p zellij-server --all-features -- _red::   # exactly the red set
//! ```
//!
//! | id | tests | spec | today | fix site |
//! |----|-------|------|-------|----------|
//! | RED-4 | `red_relative_*` (grid) | `P`/`Q` position a placement relative to a parent, with `ENOPARENT`, `EINVAL`, `ECYCLE` and `ETOODEEP` responses | `P`/`Q`/`H`/`V` fall into the ignore arm, so the placement is accepted and positioned absolutely | `parser.rs` key match, `grid_state.rs::place` |
//! | RED-5 | `red_anim_*` (grid) | animation: `a=f` frame transmission, `a=a` control, `a=c` compose, `d=f`/`d=F` frame deletion | `a=f/a/c` answer ENOTSUPPORTED; `d=f` is not even a recognised delete specifier | `parser.rs`, `store.rs`, `grid_state.rs` |
//! | RED-6 | `red_uph_*` (grid) | unicode placeholders: `U=1` virtual placements materialised by `U+10EEEE` cells, ids from the foreground colour, rows/columns from combining marks | `U!=0` answers ENOTSUPPORTED; the grid has no placeholder handling | `parser.rs`, `grid.rs::print`, `grid_state.rs` |
//!
//! 46 red tests, all in `zellij-server`; the whole-app suite is green. RED-6 matters most for
//! zellij specifically: unicode placeholders are how a program displays an image without
//! controlling the cursor, which is the position every program inside a multiplexer is in. Two of
//! the RED-4 tests — `red_relative_a_virtual_placement_cannot_also_be_relative` and
//! `red_relative_a_child_of_a_virtual_parent_sits_at_the_placeholder_minimum` — need RED-6 as
//! well, and cannot go green before it does.
//!
//! Six further entries have been closed and their tests moved into the sibling `*_spec_tests.rs`
//! files: the `t=t` deletion policy, the effect of `ESC[3J` on graphics, the derived axis when
//! scaling by `c` or `r` alone, `t=s` shared memory transfers, `ENOSPC` for an image larger than
//! the whole store, and `EBADPNG` for malformed PNG data.
//!
//! # Judgement calls, deliberately left green
//!
//! Eight places where the specification does not say enough to call the current behaviour wrong,
//! or where two of its rules pull in opposite directions. They are pinned as-is so a future
//! change to any of them is a visible decision:
//!
//! * `ESC[2J` scrolls the screen into the scrollback rather than discarding it, so a short image
//!   at the top follows its text out of the viewport instead of being freed, while one too tall to
//!   fit in the scrolled-away line is cut by the viewport top and freed outright. Nothing is left
//!   on screen either way, which is all the spec asks of `[2J`. See the three tests under
//!   "interaction with erase commands" in `kitty_spec`, in particular
//!   `erasing_the_whole_screen_lets_a_short_image_follow_its_line_into_the_scrollback`.
//! * `i=0,I=<n>` is accepted, although "Specifying both `i` and `I` keys in any command is an
//!   error". `i=0` is the protocol's own way of spelling "no image id" — it is the default, and
//!   `p` is documented as ignored for an image with `id=0` — so reading it as absent rather than
//!   as a specified id is the more useful of the two readings. See
//!   `parser_spec_tests::an_explicit_zero_image_id_does_not_count_as_specifying_one`.
//! * `ESC[3J` *does* free images that live only in the scrollback, even though the same sentence
//!   can be read as "erase commands must not touch graphics at all". An image anchored to a line
//!   that no longer exists has nowhere to live.
//!   `kitty_spec::erasing_the_scrollback_removes_the_images_that_lived_only_there` is the guard
//!   against its complement,
//!   `kitty_spec::erasing_the_scrollback_leaves_the_images_on_screen_alone`, overshooting.
//! * A terminal reset drops the scrollback, and with it images that had scrolled into it. kitty
//!   keeps its history buffer across a reset and so keeps those images; zellij's behaviour is
//!   self-consistent, and kitty's assertion is deliberately not ported.
//! * `X`/`Y` offsets at or beyond the cell size are clamped. The spec says they "must be smaller
//!   than the size of the cell" but gives the terminal no way to say otherwise.
//! * `N=1`, the transient-storage hint, is accepted and ignored. The spec allows either.
//! * An image whose width fills the pane wraps the cursor to the start of the next line. The spec
//!   says that when the advance would take the cursor off the screen "the exact positioning of
//!   the cursor is undefined, and up to implementations". See
//!   `kitty_spec::an_image_reaching_the_right_edge_wraps_the_cursor_to_the_next_line`.
//! * Control keys this terminal does not implement are ignored rather than rejected. The spec is
//!   silent and kitty is stricter (EINVAL on an unknown key character); a multiplexer sits
//!   between clients and a possibly-newer host terminal, so lenience is the safer default. See
//!   `parser_spec_tests::keys_this_terminal_does_not_implement_are_ignored_rather_than_failing`.

pub mod grid_state;
pub mod interceptor;
pub mod parser;
pub mod replies;
pub mod store;
pub use grid_state::*;
pub use interceptor::*;
pub use parser::*;
pub use replies::*;
pub use store::*;

/// Shared fixtures for the conformance suite. Deliberately outside the glob re-exports above so
/// its `KittyReply` cannot collide with the protocol types.
#[cfg(test)]
pub mod test_support;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KittyHostSupport {
    Supported,
    Unsupported,
    ProtocolDisabled,
}

impl KittyHostSupport {
    pub fn from_host_capability(supported: bool) -> Self {
        if supported {
            KittyHostSupport::Supported
        } else {
            KittyHostSupport::Unsupported
        }
    }
    pub fn protocol_is_enabled(&self) -> bool {
        *self != KittyHostSupport::ProtocolDisabled
    }
    pub fn host_supports_graphics(&self) -> bool {
        *self == KittyHostSupport::Supported
    }
}
