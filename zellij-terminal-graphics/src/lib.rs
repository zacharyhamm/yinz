//! The terminal graphics protocols: kitty's graphics protocol (command parsing, the image store,
//! placements, unicode placeholders and replies) and Sixel (decoding, the image store and cell
//! placement).
//!
//! `zellij-terminal`'s `Grid` owns a [`kitty_graphics::KittyGrid`] and a [`sixel::SixelGrid`] and
//! re-exports both modules as `zellij_terminal::panes::{kitty_graphics, sixel}`, which is the path
//! the rest of the workspace uses.
pub mod kitty_graphics;
pub mod sixel;
