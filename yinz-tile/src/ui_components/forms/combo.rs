//! A combo field: one row you type into, with a dropdown of the existing values
//! that match what you have typed.
//!
//! The value is the text, always. A suggestion is a shortcut for typing one of
//! the values that already exist — a branch name, a model, a project — and never
//! a constraint on what the field can hold. That is the difference between this
//! and [`Select`](super::Select), where the value can only ever be one of the
//! options, and the reason a plugin reads a combo back as a string rather than
//! as an index.
//!
//! ## Nothing is highlighted until you ask for it
//!
//! Typing opens the list, but leaves the highlight off every row, so Enter
//! commits exactly the text that is in the field. `Down` (or `Up`) moves into
//! the list; only then does Enter replace the text with a suggestion. A combo
//! that highlighted its best match as you typed would silently swap a genuinely
//! new value for an old one on the Enter that was meant to accept it, which
//! defeats the point of the widget.
//!
//! Because `Up` and `Down` belong to the list, a combo does not get
//! [`TextInput`]'s shell-style history on those keys. Everything else about
//! editing — word deletes, kill-line, undo, paste — is the same `TextInput` any
//! other field has.
//!
//! ## The dropdown overlays
//!
//! Exactly as [`Select`](super::Select)'s does: [`Combo::height`] is always 1,
//! an open list draws *over* the rows below it, and a form neither grows nor
//! scrolls to make room for one. See the module note on
//! [`Form::overlay_rect`](super::Form::overlay_rect) for how a screen keeps its
//! own chrome off an open list.

use super::dropdown::{Dropdown, HasDropdown};
use super::text_input::{InputAction, TextInput};
use super::viewport::{take_columns, viewport};
use super::Rect;
use super::RenderBuffer;
use crate::prelude::*;
use crate::ui_components::Text;
use unicode_width::UnicodeWidthStr;

/// Drawn at the right of a combo that has suggestions to offer, so a row with a
/// list behind it does not look like a plain text field.
const CLOSED_MARKER: &str = " ▾";

/// What handling a key did, from the containing form's point of view.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(Eq))]
pub enum ComboAction {
    /// The combo consumed the key and something visible changed.
    Continue,
    /// A suggestion was accepted into the text, and the list closed.
    Committed,
    /// The list closed. The text is unchanged.
    Cancelled,
    /// The list did not want the key: this is what the inner [`TextInput`] made
    /// of it, so a form treats a combo exactly as it treats a text field.
    Input(InputAction),
}

#[derive(Debug, Clone, Default)]
pub struct Combo {
    /// The value. Whatever is in here is what the field reads back as.
    input: TextInput,
    suggestions: Vec<String>,
    open: bool,
    dropdown: Dropdown,
    placeholder: String,
    /// The row, for hit-testing. `None` until it has been drawn.
    last_rendered: Option<Rect>,
}

impl Combo {
    /// An empty combo offering `suggestions`.
    pub fn new<I, S>(suggestions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: ToString,
    {
        Combo {
            suggestions: suggestions.into_iter().map(|s| s.to_string()).collect(),
            ..Combo::default()
        }
    }

    /// A combo with no suggestions yet — one whose list arrives later from
    /// [`set_suggestions`](Self::set_suggestions).
    pub fn empty() -> Self {
        Combo::default()
    }

    /// Start with this text in the field, the caret at its end.
    pub fn with_text<S: ToString>(mut self, text: S) -> Self {
        self.set_text(text.to_string());
        self
    }

    /// Shown in place of the value while the field is empty.
    pub fn placeholder<S: ToString>(mut self, placeholder: S) -> Self {
        self.placeholder = placeholder.to_string();
        self
    }

    pub fn max_visible_options(mut self, rows: usize) -> Self {
        self.dropdown.set_max_visible_options(rows);
        self
    }

    pub fn suggestions(&self) -> &[String] {
        &self.suggestions
    }

    /// Replace the suggestions.
    ///
    /// A plain replace: unlike [`Select::set_options`](super::Select::set_options)
    /// there is no selection to keep, because the value is the text and the list
    /// only ever offered to fill it in.
    pub fn set_suggestions(&mut self, suggestions: Vec<String>) {
        self.suggestions = suggestions;
        if self.open {
            self.refilter();
            if self.dropdown.is_empty() {
                self.close();
            }
        }
    }

    /// What the field holds.
    pub fn text(&self) -> &str {
        self.input.get_text()
    }

    /// Replace the text, caret at the end.
    pub fn set_text<S: ToString>(&mut self, text: S) {
        self.input.set_text(text.to_string());
    }

    /// The editor behind the field. A form uses it for pasting and for placing
    /// the caret where a click landed.
    pub(super) fn input(&self) -> &TextInput {
        &self.input
    }

    pub(super) fn input_mut(&mut self) -> &mut TextInput {
        &mut self.input
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Open the list, filtered by the text already in the field, with nothing
    /// highlighted.
    pub fn open(&mut self) {
        self.open = true;
        self.refilter();
    }

    /// Close the list. The text is untouched.
    pub fn close(&mut self) {
        self.open = false;
        self.dropdown.clear();
    }

    /// The suggestion under the highlight, while open.
    pub fn highlighted_suggestion(&self) -> Option<&str> {
        self.dropdown
            .highlighted_index()
            .and_then(|index| self.suggestions.get(index))
            .map(|s| s.as_str())
    }

    fn refilter(&mut self) {
        self.dropdown
            .refilter(&self.suggestions, self.input.get_text());
    }

    /// Take a suggestion into the field, caret at the end, and close.
    fn accept(&mut self, index: usize) -> bool {
        let Some(suggestion) = self.suggestions.get(index).cloned() else {
            return false;
        };
        self.input.set_text(suggestion);
        self.close();
        true
    }

    /// The text changed, so what the list is offering did too.
    ///
    /// The highlight is deliberately not carried over: it points at a row that
    /// was chosen against text that no longer exists, and an Enter landing on it
    /// would undo the character just typed.
    fn text_changed(&mut self) {
        self.dropdown.set_highlighted(None);
        self.refilter();
        // A list with nothing in it is a list in the way. It comes back as soon
        // as the text matches something again.
        self.open = !self.dropdown.is_empty();
        if !self.open {
            self.dropdown.clear();
        }
    }

    pub fn handle_key(&mut self, key: KeyWithModifier) -> ComboAction {
        if key.has_no_modifiers() {
            match key.bare_key {
                // The arrows are the list's, open or closed - see the module
                // note about `TextInput`'s history.
                BareKey::Down | BareKey::Up => {
                    let forward = key.bare_key == BareKey::Down;
                    if !self.open {
                        self.open();
                    }
                    self.dropdown.move_highlight(forward);
                    return ComboAction::Continue;
                },
                BareKey::Enter if self.open => {
                    return match self.dropdown.highlighted_index() {
                        Some(index) => {
                            self.accept(index);
                            ComboAction::Committed
                        },
                        // Nothing highlighted: the list was only ever a
                        // suggestion, and what was typed stands. Handing the
                        // Enter back lets the form end editing with the typed text.
                        None => {
                            self.close();
                            ComboAction::Input(InputAction::Submit)
                        },
                    };
                },
                BareKey::Esc if self.open => {
                    self.close();
                    return ComboAction::Cancelled;
                },
                _ => {},
            }
        }
        match self.input.handle_key(key) {
            InputAction::Continue => {
                self.text_changed();
                ComboAction::Continue
            },
            other => ComboAction::Input(other),
        }
    }

    /// Text arriving from a paste, which is a text change like any other.
    pub(super) fn insert_pasted_text(&mut self, text: &str) {
        self.input.insert_text(text);
        self.text_changed();
    }

    /// Rows the combo itself occupies — always one. An open list overlays the
    /// rows below rather than taking any of its own.
    pub fn height(&self) -> usize {
        1
    }

    /// The area an open list covers, or `None` while closed.
    pub fn overlay_rect(&self) -> Option<Rect> {
        if !self.open {
            return None;
        }
        self.dropdown.overlay_rect()
    }

    /// The line the row draws, e.g. `"Model: opus ▾"`.
    ///
    /// The marker is only there when there is a list behind the field; a combo
    /// whose suggestions have not arrived yet is a text field and says so.
    fn row_line(&self, prompt: &str, width: usize) -> (String, bool) {
        let marker = if self.suggestions.is_empty() {
            ""
        } else {
            CLOSED_MARKER
        };
        let room = width.saturating_sub(marker.width());
        if self.input.get_text().is_empty() && !self.placeholder.is_empty() {
            let value_room = room.saturating_sub(prompt.width());
            return (
                format!(
                    "{}{}{}",
                    prompt,
                    take_columns(&self.placeholder, value_room),
                    marker
                ),
                true,
            );
        }
        let (line, _) = viewport(
            prompt,
            self.input.get_text(),
            self.input.get_cursor_position(),
            room,
        );
        (format!("{}{}", line, marker), false)
    }

    /// Where the cursor belongs while this combo has focus, as a column offset
    /// from the row's left edge.
    ///
    /// Always somewhere: unlike a collapsed [`Select`](super::Select), a combo is
    /// a field you type into whether its list is open or not.
    pub(super) fn cursor_column(&self, prompt: &str, width: usize) -> usize {
        let room = width.saturating_sub(if self.suggestions.is_empty() {
            0
        } else {
            CLOSED_MARKER.width()
        });
        let (_, column) = viewport(
            prompt,
            self.input.get_text(),
            self.input.get_cursor_position(),
            room,
        );
        column
    }

    /// The columns the marker occupies, as an offset from the row's left edge.
    ///
    /// A press there toggles the list rather than placing the caret — the one
    /// part of the row that is a control rather than text.
    pub(super) fn marker_contains(&self, x: usize) -> bool {
        let Some(row) = self.last_rendered else {
            return false;
        };
        if self.suggestions.is_empty() {
            return false;
        }
        let marker_width = CLOSED_MARKER.width().min(row.columns);
        x >= row.x + row.columns - marker_width && x < row.x + row.columns
    }

    /// Draw the row and record it for hit-testing.
    ///
    /// The list is *not* drawn here — see [`Combo::render_overlay`], which a form
    /// calls after everything else so it lands on top.
    pub(super) fn render_row(
        &mut self,
        buffer: &mut RenderBuffer,
        prompt: &str,
        x: usize,
        y: usize,
        width: usize,
        focused: bool,
        typing: bool,
    ) {
        let (line, is_placeholder) = self.row_line(prompt, width);
        let prompt_len = prompt.chars().count().min(line.chars().count());
        let mut text = Text::new(&line).color_range(2, 0..prompt_len);
        if is_placeholder {
            text = text.dim_range(prompt_len..);
        }
        if focused {
            // Dimming the value of a focused-but-not-editing field is what the
            // text arm of a form does too: the row is where you are, but nothing
            // you type is going into it yet.
            if !is_placeholder {
                if typing {
                    text = text.color_range(0, prompt_len..);
                } else {
                    text = text.dim_range(prompt_len..);
                }
            }
            text = text.selected();
        }
        buffer.text(text, x, y, Some(width), None);
        self.last_rendered = Some(Rect::new(x, y, width, 1));
    }

    /// Draw the list, if it is open, over whatever is already there.
    pub(super) fn render_overlay(&mut self, buffer: &mut RenderBuffer, max_rows: usize) {
        if !self.open {
            self.dropdown.clear_rendered();
            return;
        }
        let Some(row) = self.last_rendered else {
            self.dropdown.clear_rendered();
            return;
        };
        self.dropdown
            .render(buffer, &self.suggestions, row, max_rows);
    }

    pub(super) fn clear_rendered(&mut self) {
        self.last_rendered = None;
        self.dropdown.clear_rendered();
    }

    pub(super) fn row_rect(&self) -> Option<Rect> {
        self.last_rendered
    }

    pub(super) fn row_contains(&self, x: usize, y: usize) -> bool {
        self.last_rendered
            .map(|rect| rect.contains(x, y))
            .unwrap_or(false)
    }

    pub(super) fn option_at(&self, x: usize, y: usize) -> Option<usize> {
        self.dropdown.option_at(x, y)
    }

    pub(super) fn highlight_option(&mut self, index: usize) -> bool {
        self.dropdown.highlight_option(index)
    }
}

/// The form does not care which widget a list belongs to — see `HasDropdown`.
impl HasDropdown for Combo {
    fn is_open(&self) -> bool {
        Combo::is_open(self)
    }

    fn open(&mut self) {
        Combo::open(self)
    }

    fn close(&mut self) {
        Combo::close(self)
    }

    fn row_rect(&self) -> Option<Rect> {
        Combo::row_rect(self)
    }

    fn row_contains(&self, x: usize, y: usize) -> bool {
        Combo::row_contains(self, x, y)
    }

    fn option_at(&self, x: usize, y: usize) -> Option<usize> {
        Combo::option_at(self, x, y)
    }

    fn highlight_option(&mut self, index: usize) -> bool {
        Combo::highlight_option(self, index)
    }

    fn commit_option(&mut self, index: usize) -> bool {
        self.accept(index)
    }

    fn overlay_rect(&self) -> Option<Rect> {
        Combo::overlay_rect(self)
    }

    fn render_overlay(&mut self, buffer: &mut RenderBuffer, max_rows: usize) {
        Combo::render_overlay(self, buffer, max_rows)
    }
}

#[cfg(test)]
#[path = "unit/combo.rs"]
mod tests;
