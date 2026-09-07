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
//! Native callers use [`Form::render_buffer_in_viewport`] or [`Dialog::render_buffer`].
//! These return a [`RenderBuffer`] without printing or calling host shims.
//! The existing render methods deliver its cursor update before printing its output.
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
//!     .combo("model", "Model", Combo::new(["opus", "sonnet"]))
//!     .button("save", Button::new("Save"));
//!
//! # let key = KeyWithModifier::new(BareKey::Enter);
//! match form.handle_key(key) {
//!     FormEvent::ButtonPressed(id) if id == "save" => { /* read form.text("name") */ },
//!     FormEvent::Ignored => { /* the plugin's own bindings apply */ },
//!     _ => {},
//! }
//!
//! form.render(0, 0, 40); // places the cursor too
//!
//! let mut dialog = Dialog::new(Text::new("Rename"), form)
//!     .help(Text::new("Arrows/Tab: move, Enter: activate, Esc: back"));
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
//! - Arrows traverse inactive controls. Tab and Shift-Tab always move focus.
//!   [`Form::with_traversal_keys`] adds bindings without taking active arrows.
//! - An open [`Select`] or [`Combo`] dropdown *overlays* the rows below it, so a
//!   screen that prints its own chrome after the form should check
//!   [`Form::overlay_rect`].
//! - A [`Combo`] is the field for a value that is free text but is usually one
//!   of the values that already exist: typing filters a list of suggestions,
//!   and nothing is highlighted in it until an arrow key asks for it, so Enter
//!   commits what was typed.
//! - [`Form::handle_mouse`] wants the whole click, not just the press: widgets
//!   act on `Mouse::Release`, and only when it lands back where the
//!   `Mouse::LeftClick` did. A plugin that subscribes to `EventType::Mouse` and
//!   forwards every variant gets that for free.
//! - Enter activates a control. Typing or paste also starts text editing.
//!   Enter or Esc ends text editing and preserves the text. Only buttons submit.
//! - Enter/Space opens a select or activates a radio group. Active arrows change
//!   the dropdown highlight or radio selection. Enter commits a dropdown.
//!   Enter/Space/Esc leaves a radio group and preserves its selection.
//! - Esc closes a combo's suggestions first, then ends editing on the next press.
//!   Esc on an inactive control cancels the form. Ctrl-C always cancels.
//! - Traversal ends interaction and discards uncommitted dropdown highlights.
//!   `FocusMoved` reports interaction changes; `FieldChanged` reports value changes.

mod button;
mod choice;
mod combo;
mod dialog;
mod dropdown;
mod form;
mod fuzzy;
mod label;
mod list;
mod select;
mod text_input;
mod viewport;

pub use button::{Button, ButtonStyle, Rect};
pub use choice::{Checkbox, RadioGroup};
pub use combo::{Combo, ComboAction};
pub use dialog::Dialog;
pub use dropdown::DEFAULT_MAX_VISIBLE_OPTIONS;
pub use form::{Field, FieldId, Form, FormEvent};
pub use fuzzy::{fuzzy_filter, Match};
pub use label::{Label, DEFAULT_LABEL_LEVEL};
pub use list::list_window;
pub use select::{Select, SelectAction};
pub use text_input::{InputAction, TextInput};
pub use viewport::{column_to_cursor, take_columns, viewport, viewport_window};

mod buffer;
pub use buffer::RenderBuffer;
