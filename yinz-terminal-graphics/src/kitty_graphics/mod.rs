//! The kitty graphics protocol: parsing commands, storing images, placing them on the grid.
//!
//! # The conformance suite
//!
//! The suite is written against `graphics-protocol.rst`, not against this code, so a failing test
//! means the protocol work is not done rather than that the test is wrong. For a long time part of
//! it was failing on purpose, in modules ending `_red`; **that set is now empty** and the whole
//! suite is green:
//!
//! ```sh
//! cargo x test              # the gate, nothing skipped
//! cargo x integration-test  # ditto, whole-app
//! ```
//!
//! The suite spans three crates. The protocol level is this crate's `kitty_graphics/unit/`. The
//! grid and host-emission levels are `yinz-terminal`'s `panes/unit/grid_tests/kitty_*.rs` and
//! `output/unit/output_kitty_spec_tests.rs`; the `grid_tests::`, `kitty_spec::` and
//! `kitty_relative_placement::` paths below are there, as is `Grid`. The whole-app level is
//! `yinz-integration-tests/tests/kitty_graphics*.rs`.
//!
//! All nine deviations have been closed. The last two were relative placements
//! (`grid_tests::kitty_relative_placement`) and animation (`grid_tests::kitty_animation`); before
//! them, the `t=t` deletion policy, the effect of `ESC[3J` on graphics, the derived axis when
//! scaling by `c` or `r` alone, `t=s` shared memory transfers, `ENOSPC` for an image larger than
//! the whole store, `EBADPNG` for malformed PNG data, and — the one that mattered most for yinz
//! specifically — unicode placeholders, now `grid_tests::kitty_unicode_placeholders`. Those are how
//! a program displays an image without controlling the cursor, which is the position every program
//! inside a multiplexer is in.
//!
//! # Judgement calls, deliberately left green
//!
//! Twelve places where the specification does not say enough to call the current behaviour wrong,
//! or where two of its rules pull in opposite directions. They are pinned as-is so a future
//! change to any of them is a visible decision:
//!
//! * **Animation has no playback clock.** Frames are real rasters — transmitted, composed and
//!   selected for real, so `a=a,c=<n>` changes what is on screen and a client driving its own
//!   animation works. But `a=a,s=2`/`s=3` and the `z` gaps are recorded and never act: nothing
//!   advances a frame on its own. A clock would need a wakeup source per animating image and a
//!   retransmission to the host on every tick, and is deliberately left for later. A client that
//!   asks the terminal to play an animation is told OK and sees a still image, which is the one
//!   place in this module where a success reply overstates what happened. hocus's empty-chat wand
//!   is the in-tree example of the other half of that bargain: a client that drives its own frames
//!   animates fine.
//! * **A full store never evicts displayed images.** The specification says images without
//!   placements are deleted preferentially, but a store shared by many panes cannot remove the
//!   corresponding placements itself. Once only placed images remain, a transmission that needs
//!   more room receives `ENOSPC`; retained base64 caches are discarded before that decision.
//! * An absent `Q` on a relative placement selects **any** placement of the parent image rather
//!   than placement 0. The spec gives `Q` a default of 0, but a virtual placement created without
//!   a `p` key also has placement id 0, and reading `Q` as "unspecified" is what lets a client
//!   name such a parent at all. See
//!   `kitty_relative_placement::relative_a_child_of_a_virtual_parent_sits_at_the_placeholder_minimum`.
//! * A cascade that deletes a relative placement frees its image only when the delete that started
//!   it was an uppercase one. The spec says an image whose last placement goes is deleted, but says
//!   it in the context of a delete that frees data; matching the case of the originating specifier
//!   keeps the lowercase/uppercase distinction meaning the same thing everywhere.
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
//!   keeps its history buffer across a reset and so keeps those images; yinz's behaviour is
//!   self-consistent, and kitty's assertion is deliberately not ported.
//! * `X`/`Y` offsets at or beyond the cell size are clamped. The spec says they "must be smaller
//!   than the size of the cell" but gives the terminal no way to say otherwise.
//! * `N=1`, the transient-storage hint, is accepted and ignored. The spec allows either.
//! * An image whose width fills the pane wraps the cursor to the start of the next line. The spec
//!   says that when the advance would take the cursor off the screen "the exact positioning of
//!   the cursor is undefined, and up to implementations". See
//!   `kitty_spec::an_image_reaching_the_right_edge_wraps_the_cursor_to_the_next_line`.
//! * The placements a unicode placeholder run stands for are resolved from the **viewport** only,
//!   so a run that has scrolled into the scrollback stops being a placement until the user scrolls
//!   it back into view. An explicit placement, by contrast, stays in the list wherever it is. The
//!   spec says nothing either way, and this falls out of deriving the placements from the cells
//!   rather than tracking them: the cells are the placement, and only the ones on screen are ever
//!   drawn. See `Grid::kitty_refresh_placeholder_placements`.
//! * Control keys this terminal does not implement are ignored rather than rejected. The spec is
//!   silent and kitty is stricter (EINVAL on an unknown key character); a multiplexer sits
//!   between clients and a possibly-newer host terminal, so lenience is the safer default. See
//!   `parser_spec_tests::keys_this_terminal_does_not_implement_are_ignored_rather_than_failing`.

pub mod grid_state;
pub mod interceptor;
pub mod parser;
pub mod placeholders;
pub mod replies;
pub mod store;
pub use grid_state::*;
pub use interceptor::*;
pub use parser::*;
pub use placeholders::*;
pub use replies::*;
pub use store::*;

// Shared fixtures for the conformance suite. Deliberately outside the glob re-exports above so
// its `KittyReply` cannot collide with the protocol types. Compiled in every build: the whole-app
// suite needs it outside `cfg(test)`, and a feature only the suite asked for built this crate and
// every crate above it a second time in each `x integration-test`. A plain comment rather than
// `///`: rustdoc resolves an outer doc's links from this module, and would then fail on the links
// in `test_support`'s own `//!` doc.
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
