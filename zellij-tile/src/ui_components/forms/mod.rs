//! Form widgets: labels, text inputs, choices, buttons, and the focus container
//! that ties them together.
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
//!     .checkbox("frames", "Pane frames", Checkbox::new(true))
//!     .radio("mode", "Mode", RadioGroup::new(["Normal", "Locked"]))
//!     .select("layout", "Layout", Select::new(["default", "compact"]))
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
//!
//! let mut dialog = Dialog::new(Text::new("Rename"), form)
//!     .help(Text::new("<Enter> Save, <Esc> Cancel"));
//! dialog.render(Rect::new(0, 0, 40, 10));
//! ```
//!
//! The five pieces worth knowing before using this:
//!
//! - [`Form::render`] calls `show_cursor` itself, before printing. That
//!   ordering is required by the host and is easy to get wrong by hand.
//! - [`Form::render_in_viewport`] keeps the focused widget visible when a form
//!   has fewer rows than fields.
//! - [`FormEvent::Ignored`] means the form did not consume the key, so a plugin
//!   keeps its own bindings.
//! - Traversal keys are configurable ([`Form::with_traversal_keys`]) because
//!   `Tab` is already spoken for in most plugins.
//! - An open [`Select`] dropdown *overlays* the rows below it, so a screen that
//!   prints its own chrome after the form should check
//!   [`Form::overlay_rect`].

mod button;
mod choice;
mod dialog;
mod form;
mod fuzzy;
mod label;
mod list;
mod select;
mod text_input;
mod viewport;

pub use button::{Button, ButtonStyle, Rect};
pub use choice::{Checkbox, RadioGroup};
pub use dialog::Dialog;
pub use form::{Field, FieldId, Form, FormEvent};
pub use fuzzy::{fuzzy_filter, Match};
pub use label::{Label, DEFAULT_LABEL_LEVEL};
pub use list::list_window;
pub use select::{Select, SelectAction, DEFAULT_MAX_VISIBLE_OPTIONS};
pub use text_input::{InputAction, TextInput};
pub use viewport::{column_to_cursor, take_columns, viewport, viewport_window};
