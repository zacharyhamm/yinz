//! A select box: one row that shows the current choice, and a dropdown of the
//! options filtered by what the user types.
//!
//! The value is always one of the options — typing filters the list, it does not
//! set the value. That is the difference between this and [`TextInput`], and the
//! reason a plugin can read a select back as an index into the list it handed
//! over.
//!
//! The pieces are the ones already in this module: the filter is a `TextInput`,
//! so it gets the same editing keys as any other field; the filter row is laid
//! out by [`viewport`](super::viewport), so the cursor lands in the right column
//! next to a CJK character; the list itself is a `Dropdown`, the same one
//! [`Combo`](super::Combo) drops under its row.
//!
//! ## The dropdown overlays
//!
//! An open dropdown draws *over* whatever is below it rather than pushing it
//! down: [`Select::height`] is always 1, and a [`Form`](super::Form) holding one
//! does not change size when it opens. A form that grew and shrank as the user
//! opened a list would move the buttons out from under the pointer, and in a
//! short pane could push them off screen entirely. For the same reason a form
//! never *scrolls* to make room for a list either: the row the user just opened
//! would jump to the top of the viewport, taking everything above it off screen.
//! A list with nothing under it inside the form's own rows can still be given
//! room the caller owns —
//! [`Form::render_in_viewport_with_overlay`](super::Form::render_in_viewport_with_overlay).
//!
//! The cost is that anything the plugin prints *after* `Form::render` paints
//! over the list — see [`Form::overlay_rect`](super::Form::overlay_rect) for how
//! a screen keeps its chrome out of the way.

use super::dropdown::{Dropdown, HasDropdown};
use super::text_input::{InputAction, TextInput};
use super::viewport::{take_columns, viewport};
use super::Rect;
use super::RenderBuffer;
use crate::prelude::*;
use crate::ui_components::Text;
use unicode_width::UnicodeWidthStr;

/// Drawn at the right of a collapsed select, so a row that is a list and a row
/// that is a text field do not look alike.
const CLOSED_MARKER: &str = " ▾";

/// What handling a key did, from the containing form's point of view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectAction {
    /// The select consumed the key and something visible changed.
    Continue,
    /// An option was chosen and the dropdown closed.
    Committed,
    /// The dropdown closed without changing the value.
    Cancelled,
    /// Not consumed — the form's own bindings apply.
    NoAction,
}

#[derive(Debug, Clone, Default)]
pub struct Select {
    options: Vec<String>,
    selected: Option<usize>,
    placeholder: String,
    open: bool,
    filter: TextInput,
    /// The list under the row: what matches, and what the highlight is on.
    dropdown: Dropdown,
    /// The collapsed row, for hit-testing. `None` until it has been drawn.
    last_rendered: Option<Rect>,
}

impl Select {
    pub fn new<I, S>(options: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: ToString,
    {
        let options: Vec<String> = options.into_iter().map(|o| o.to_string()).collect();
        let selected = if options.is_empty() { None } else { Some(0) };
        Select {
            options,
            selected,
            placeholder: String::new(),
            open: false,
            filter: TextInput::empty(),
            dropdown: Dropdown::default(),
            last_rendered: None,
        }
    }

    /// Start with this option chosen. Out of range leaves the choice alone.
    pub fn with_selected(mut self, index: usize) -> Self {
        self.select_index(index);
        self
    }

    /// Shown in place of a value when nothing is selected.
    pub fn placeholder<S: ToString>(mut self, placeholder: S) -> Self {
        self.placeholder = placeholder.to_string();
        self
    }

    pub fn max_visible_options(mut self, rows: usize) -> Self {
        self.dropdown.set_max_visible_options(rows);
        self
    }

    pub fn options(&self) -> &[String] {
        &self.options
    }

    /// Replace the options, keeping the selection on the same *label*.
    ///
    /// Lists here arrive from the host and are refreshed whole (layouts,
    /// sessions, plugins), so an index-based selection would silently jump to a
    /// different entry every time one appeared or disappeared above it. When the
    /// selected label is gone the selection falls back to the first option,
    /// which is the only choice that is still valid.
    pub fn set_options(&mut self, options: Vec<String>) {
        let previously_selected = self.selected_option().map(|o| o.to_string());
        self.options = options;
        self.selected = match previously_selected {
            Some(label) => self.options.iter().position(|option| option == &label).or(
                if self.options.is_empty() {
                    None
                } else {
                    Some(0)
                },
            ),
            None if self.options.is_empty() => None,
            None => Some(0),
        };
        if self.open {
            self.refilter();
        }
    }

    pub fn clear_selection(&mut self) {
        self.selected = None;
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    pub fn selected_option(&self) -> Option<&str> {
        self.selected
            .and_then(|i| self.options.get(i))
            .map(|o| o.as_str())
    }

    /// Choose an option by index. Returns whether there was such an option.
    pub fn select_index(&mut self, index: usize) -> bool {
        if index >= self.options.len() {
            return false;
        }
        self.selected = Some(index);
        true
    }

    /// Choose an option by its text. Returns whether there was such an option.
    pub fn select_option(&mut self, option: &str) -> bool {
        match self.options.iter().position(|o| o == option) {
            Some(index) => self.select_index(index),
            None => false,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Open the dropdown, with every option showing and the highlight on the
    /// current choice.
    pub fn open(&mut self) {
        self.open = true;
        self.filter.clear();
        self.refilter();
        if let Some(position) = self.selected.and_then(|s| self.dropdown.position_of(s)) {
            self.dropdown.set_highlighted(Some(position));
        }
    }

    /// Close the dropdown and discard the filter. The value is unchanged.
    pub fn close(&mut self) {
        self.open = false;
        self.filter.clear();
        self.dropdown.clear();
    }

    /// The text currently in the filter. Empty while closed.
    pub fn filter_text(&self) -> &str {
        self.filter.get_text()
    }

    /// The option under the highlight, while open.
    pub fn highlighted_option(&self) -> Option<&str> {
        self.highlighted_index()
            .and_then(|index| self.options.get(index))
            .map(|o| o.as_str())
    }

    fn highlighted_index(&self) -> Option<usize> {
        self.dropdown.highlighted_index()
    }

    fn refilter(&mut self) {
        self.dropdown
            .refilter(&self.options, self.filter.get_text());
        // A select always commits *something*, so a list with rows in it always
        // has one of them highlighted - unlike a combo, where nothing
        // highlighted is what keeps typed text from being replaced.
        self.dropdown.ensure_highlight();
    }

    fn move_highlight(&mut self, forward: bool) -> SelectAction {
        self.dropdown.move_highlight(forward);
        SelectAction::Continue
    }

    fn commit(&mut self) -> SelectAction {
        match self.highlighted_index() {
            Some(index) => {
                self.selected = Some(index);
                self.close();
                SelectAction::Committed
            },
            // A filter that matches nothing has nothing to commit. Closing on
            // Enter anyway would silently keep the old value while looking like
            // it accepted what was typed.
            None => SelectAction::Continue,
        }
    }

    pub fn handle_key(&mut self, key: KeyWithModifier) -> SelectAction {
        if !self.open {
            return self.handle_key_closed(key);
        }
        match key.bare_key {
            BareKey::Enter if key.has_no_modifiers() => self.commit(),
            BareKey::Esc if key.has_no_modifiers() => {
                self.close();
                SelectAction::Cancelled
            },
            BareKey::Down if key.has_no_modifiers() => self.move_highlight(true),
            BareKey::Up if key.has_no_modifiers() => self.move_highlight(false),
            _ => {
                // Everything else edits the filter, so the filter gets the same
                // word-delete, kill-line and paste handling as any other field.
                match self.filter.handle_key(key) {
                    InputAction::Continue => {
                        self.refilter();
                        SelectAction::Continue
                    },
                    // Enter/Esc/Tab were dealt with above or by the form; the
                    // filter reporting them here would mean two owners for one
                    // key.
                    _ => SelectAction::Continue,
                }
            },
        }
    }

    fn handle_key_closed(&mut self, key: KeyWithModifier) -> SelectAction {
        match key.bare_key {
            BareKey::Down | BareKey::Up | BareKey::Char(' ') if key.has_no_modifiers() => {
                self.open();
                SelectAction::Continue
            },
            // Typing opens the list already filtered — the typeahead the widget
            // exists for. Enter is deliberately not in this list: the `Form`
            // owns what it means for a closed select, because that depends on
            // whether the form submits from Enter at all. A form built with
            // `Form` opens the list with it.
            BareKey::Char(c) if key.has_no_modifiers() => {
                self.open();
                self.filter.insert_char(c);
                self.refilter();
                SelectAction::Continue
            },
            _ => SelectAction::NoAction,
        }
    }

    /// Rows the select itself occupies — always one. An open dropdown overlays
    /// the rows below rather than taking any of its own.
    pub fn height(&self) -> usize {
        1
    }

    /// The area an open dropdown covers, or `None` while closed.
    ///
    /// A screen that draws chrome after the form can use this to leave the list
    /// alone; nothing else knows the list is there.
    pub fn overlay_rect(&self) -> Option<Rect> {
        if !self.open {
            return None;
        }
        self.dropdown.overlay_rect()
    }

    /// The line the collapsed row draws, e.g. `"Layout: default ▾"`.
    fn closed_line(&self, prompt: &str, width: usize) -> String {
        let value = self.selected_option().unwrap_or(&self.placeholder);
        let room = width
            .saturating_sub(prompt.width())
            .saturating_sub(CLOSED_MARKER.width());
        format!("{}{}{}", prompt, take_columns(value, room), CLOSED_MARKER)
    }

    /// Where the cursor belongs while this select has focus, as a column offset
    /// from the row's left edge, or `None` when it should not be shown.
    ///
    /// A collapsed select is not something you type into, and a cursor parked on
    /// it reads as focus being in a text field.
    pub(super) fn cursor_column(&self, prompt: &str, width: usize) -> Option<usize> {
        if !self.open {
            return None;
        }
        let (_, column) = viewport(
            prompt,
            self.filter.get_text(),
            self.filter.get_cursor_position(),
            width,
        );
        Some(column)
    }

    /// Draw the collapsed row and record it for hit-testing.
    ///
    /// The dropdown is *not* drawn here — see [`Select::render_overlay`], which
    /// a form calls after everything else so the list lands on top.
    pub(super) fn render_row(
        &mut self,
        buffer: &mut RenderBuffer,
        prompt: &str,
        x: usize,
        y: usize,
        width: usize,
        focused: bool,
    ) {
        let mut text = if self.open {
            let (line, _) = viewport(
                prompt,
                self.filter.get_text(),
                self.filter.get_cursor_position(),
                width,
            );
            let prompt_len = prompt.chars().count().min(line.chars().count());
            Text::new(&line).color_range(2, 0..prompt_len)
        } else {
            let line = self.closed_line(prompt, width);
            let prompt_len = prompt.chars().count().min(line.chars().count());
            let mut text = Text::new(&line).color_range(2, 0..prompt_len);
            if self.selected_option().is_none() {
                text = text.dim_range(prompt_len..);
            }
            text
        };
        if focused {
            text = text.selected();
        }
        buffer.text(text, x, y, Some(width), None);
        self.last_rendered = Some(Rect::new(x, y, width, 1));
    }

    /// Draw the dropdown, if it is open, over whatever is already there.
    pub(super) fn render_overlay(&mut self, buffer: &mut RenderBuffer, max_rows: usize) {
        if !self.open {
            self.dropdown.clear_rendered();
            return;
        }
        let Some(row) = self.last_rendered else {
            self.dropdown.clear_rendered();
            return;
        };
        self.dropdown.render(buffer, &self.options, row, max_rows);
    }

    pub(super) fn clear_rendered(&mut self) {
        self.last_rendered = None;
        self.dropdown.clear_rendered();
    }

    /// Whether a click at `(x, y)` lands on the collapsed row.
    pub(super) fn row_contains(&self, x: usize, y: usize) -> bool {
        self.last_rendered
            .map(|rect| rect.contains(x, y))
            .unwrap_or(false)
    }

    /// Which option a click at `(x, y)` lands on, if any are on screen.
    pub(super) fn option_at(&self, x: usize, y: usize) -> Option<usize> {
        self.dropdown.option_at(x, y)
    }

    /// Move the highlight to an option, by its index in the full list.
    ///
    /// Returns whether the highlight moved, so a hover can skip a render.
    pub(super) fn highlight_option(&mut self, index: usize) -> bool {
        self.dropdown.highlight_option(index)
    }

    /// Choose the option a click landed on and close.
    pub(super) fn commit_option(&mut self, index: usize) -> bool {
        if index >= self.options.len() {
            return false;
        }
        self.selected = Some(index);
        self.close();
        true
    }
}

/// The form does not care which widget a list belongs to — see `HasDropdown`.
impl HasDropdown for Select {
    fn is_open(&self) -> bool {
        Select::is_open(self)
    }

    fn open(&mut self) {
        Select::open(self)
    }

    fn close(&mut self) {
        Select::close(self)
    }

    fn row_rect(&self) -> Option<Rect> {
        self.last_rendered
    }

    fn row_contains(&self, x: usize, y: usize) -> bool {
        Select::row_contains(self, x, y)
    }

    fn option_at(&self, x: usize, y: usize) -> Option<usize> {
        Select::option_at(self, x, y)
    }

    fn highlight_option(&mut self, index: usize) -> bool {
        Select::highlight_option(self, index)
    }

    fn commit_option(&mut self, index: usize) -> bool {
        Select::commit_option(self, index)
    }

    fn overlay_rect(&self) -> Option<Rect> {
        Select::overlay_rect(self)
    }

    fn render_overlay(&mut self, buffer: &mut RenderBuffer, max_rows: usize) {
        Select::render_overlay(self, buffer, max_rows)
    }
}

#[cfg(test)]
#[path = "unit/select.rs"]
mod tests;
