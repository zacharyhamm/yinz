//! Form widgets: labels, text inputs, buttons, and the focus container that
//! ties them together.
//!
//! The rest of `ui_components` is stateless renderers — [`Text`](super::Text)
//! and friends take a string, emit an escape sequence, and return nothing. That
//! is the right shape for a status bar and the wrong shape for anything a user
//! types into, so every plugin that needed a form grew its own: two drifted
//! copies of a text editor, three incompatible ways of drawing a cursor, and a
//! different meaning for `Tab` in each one. This module is the shared version.
//!
//! Nothing here needs server support. Widgets render by composing the existing
//! components, so a plugin using them emits the same `text` and `ribbon`
//! sequences it always did.
//!
//! ```no_run
//! use zellij_tile::prelude::*;
//! use zellij_tile::ui_components::forms::*;
//!
//! let mut form = Form::new()
//!     .field("name", "Name", TextInput::empty())
//!     .button("save", Button::new("Save"));
//!
//! # let key = KeyWithModifier::new(BareKey::Enter);
//! match form.handle_key(key) {
//!     FormEvent::Submit => { /* read form.text("name") */ },
//!     FormEvent::Ignored => { /* the plugin's own bindings apply */ },
//!     _ => {},
//! }
//!
//! form.render(0, 0, 40); // places the cursor too
//! ```
//!
//! The three pieces worth knowing before using this:
//!
//! - [`Form::render`] calls `show_cursor` itself, before printing. That
//!   ordering is required by the host and is easy to get wrong by hand.
//! - [`FormEvent::Ignored`] means the form did not consume the key, so a plugin
//!   keeps its own bindings.
//! - Traversal keys are configurable ([`Form::with_traversal_keys`]) because
//!   `Tab` is already spoken for in most plugins.

mod button;
mod form;
mod label;
mod text_input;
mod viewport;

pub use button::{Button, ButtonStyle, Rect};
pub use form::{Field, FieldId, Form, FormEvent};
pub use label::{Label, DEFAULT_LABEL_LEVEL};
pub use text_input::{InputAction, TextInput};
pub use viewport::{column_to_cursor, take_columns, viewport, viewport_window};
