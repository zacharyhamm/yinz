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
            // `Form::enter_never_submits` opens the list with it.
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
mod tests {
    use super::*;
    use hegel::generators;

    fn a_select() -> Select {
        Select::new(["default", "compact", "dev-three-pane"])
    }

    fn key(bare_key: BareKey) -> KeyWithModifier {
        KeyWithModifier::new(bare_key)
    }

    fn char_key(c: char) -> KeyWithModifier {
        KeyWithModifier::new(BareKey::Char(c))
    }

    #[test]
    fn a_new_select_has_the_first_option_and_is_closed() {
        let select = a_select();
        assert_eq!(select.selected_option(), Some("default"));
        assert!(!select.is_open());
    }

    #[test]
    fn a_select_with_no_options_has_no_value() {
        let select = Select::new(Vec::<String>::new());
        assert_eq!(select.selected_option(), None);
        assert_eq!(select.selected_index(), None);
    }

    /// Enter has to keep meaning "submit" on a closed select, or a form with one
    /// in it can only be submitted from a text field.
    #[test]
    fn enter_on_a_closed_select_is_not_consumed() {
        let mut select = a_select();
        assert_eq!(
            select.handle_key(key(BareKey::Enter)),
            SelectAction::NoAction
        );
        assert!(!select.is_open());
        assert_eq!(select.handle_key(key(BareKey::Esc)), SelectAction::NoAction);
    }

    #[test]
    fn space_and_the_arrows_open_the_list() {
        for opener in [BareKey::Char(' '), BareKey::Down, BareKey::Up] {
            let mut select = a_select();
            assert_eq!(select.handle_key(key(opener)), SelectAction::Continue);
            assert!(select.is_open(), "{:?} did not open the list", opener);
            assert_eq!(select.filter_text(), "");
        }
    }

    #[test]
    fn opening_puts_the_highlight_on_the_current_choice() {
        let mut select = a_select().with_selected(2);
        select.open();
        assert_eq!(select.highlighted_option(), Some("dev-three-pane"));
    }

    #[test]
    fn typing_opens_the_list_and_filters_it() {
        let mut select = a_select();
        select.handle_key(char_key('c'));
        assert!(select.is_open());
        assert_eq!(select.filter_text(), "c");
        assert_eq!(
            select.dropdown.matches().len(),
            1,
            "only 'compact' contains a 'c': {:?}",
            select.dropdown.matches()
        );
        assert_eq!(select.highlighted_option(), Some("compact"));
    }

    #[test]
    fn enter_commits_the_highlighted_option_and_closes() {
        let mut select = a_select();
        select.handle_key(char_key('c'));
        assert_eq!(
            select.handle_key(key(BareKey::Enter)),
            SelectAction::Committed
        );
        assert_eq!(select.selected_option(), Some("compact"));
        assert!(!select.is_open());
        assert_eq!(select.filter_text(), "", "the filter outlived the dropdown");
    }

    #[test]
    fn esc_closes_without_changing_the_value() {
        let mut select = a_select();
        select.handle_key(char_key('c'));
        assert_eq!(
            select.handle_key(key(BareKey::Esc)),
            SelectAction::Cancelled
        );
        assert!(!select.is_open());
        assert_eq!(select.selected_option(), Some("default"));
    }

    #[test]
    fn the_arrows_move_the_highlight_and_wrap() {
        let mut select = a_select();
        select.open();
        select.handle_key(key(BareKey::Down));
        assert_eq!(select.highlighted_option(), Some("compact"));
        select.handle_key(key(BareKey::Up));
        assert_eq!(select.highlighted_option(), Some("default"));
        select.handle_key(key(BareKey::Up));
        assert_eq!(
            select.highlighted_option(),
            Some("dev-three-pane"),
            "up from the first option should wrap to the last"
        );
    }

    /// The highlight indexing into the old, longer match list is how a select
    /// commits an option the user cannot see.
    #[test]
    fn narrowing_the_filter_keeps_the_highlight_on_a_visible_row() {
        let mut select = a_select();
        select.open();
        select.handle_key(key(BareKey::Down));
        select.handle_key(key(BareKey::Down));
        assert_eq!(select.highlighted_option(), Some("dev-three-pane"));
        select.handle_key(char_key('d'));
        select.handle_key(char_key('e'));
        select.handle_key(char_key('f'));
        assert_eq!(select.highlighted_option(), Some("default"));
    }

    #[test]
    fn a_filter_that_matches_nothing_commits_nothing() {
        let mut select = a_select();
        select.open();
        for c in "zzzz".chars() {
            select.handle_key(char_key(c));
        }
        assert_eq!(select.highlighted_option(), None);
        assert_eq!(
            select.handle_key(key(BareKey::Enter)),
            SelectAction::Continue,
            "committing an empty list should not close the dropdown"
        );
        assert!(select.is_open());
        assert_eq!(select.selected_option(), Some("default"));
    }

    #[test]
    fn backspacing_the_filter_widens_the_list_again() {
        let mut select = a_select();
        select.open();
        select.handle_key(char_key('z'));
        assert!(select.dropdown.is_empty());
        select.handle_key(key(BareKey::Backspace));
        assert_eq!(select.dropdown.matches().len(), 3);
        assert_eq!(
            select.highlighted_option(),
            Some("default"),
            "a list with rows in it always has one of them highlighted"
        );
    }

    #[test]
    fn refreshed_options_keep_the_selection_on_the_same_label() {
        let mut select = a_select().with_selected(2);
        select.set_options(
            ["new-one", "default", "dev-three-pane"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        );
        assert_eq!(select.selected_option(), Some("dev-three-pane"));
        assert_eq!(select.selected_index(), Some(2));
    }

    #[test]
    fn a_selection_whose_option_is_gone_falls_back_to_the_first() {
        let mut select = a_select().with_selected(2);
        select.set_options(vec!["only-one".to_string()]);
        assert_eq!(select.selected_option(), Some("only-one"));
        select.set_options(vec![]);
        assert_eq!(select.selected_option(), None);
    }

    #[test]
    fn a_select_that_never_rendered_has_no_overlay_and_hits_nothing() {
        let mut select = a_select();
        select.open();
        assert_eq!(select.overlay_rect(), None);
        assert!(!select.row_contains(0, 0));
        assert_eq!(select.option_at(0, 1), None);
    }

    #[test]
    fn a_closed_select_shows_its_value_and_a_marker() {
        let select = a_select();
        assert_eq!(select.closed_line("Layout: ", 40), "Layout: default ▾");
    }

    #[test]
    fn a_closed_select_with_no_value_shows_its_placeholder() {
        let select = Select::new(Vec::<String>::new()).placeholder("none available");
        assert_eq!(
            select.closed_line("Layout: ", 40),
            "Layout: none available ▾"
        );
    }

    #[test]
    fn a_collapsed_row_never_exceeds_its_width() {
        let select = a_select().with_selected(2);
        let line = select.closed_line("Layout: ", 14);
        assert!(
            line.as_str().width() <= 14,
            "{:?} is wider than 14 columns",
            line
        );
    }

    #[test]
    fn a_closed_select_places_no_cursor() {
        let mut select = a_select();
        assert_eq!(select.cursor_column("Layout: ", 40), None);
        select.open();
        assert_eq!(select.cursor_column("Layout: ", 40), Some(8));
    }

    /// Whatever the user types, the value stays one of the options — that is
    /// what makes this a select rather than a text field with suggestions.
    #[hegel::test]
    fn the_value_is_always_one_of_the_options(tc: hegel::TestCase) {
        let options: Vec<String> = tc.draw(
            generators::vecs(generators::text())
                .min_size(1)
                .max_size(10),
        );
        let mut select = Select::new(options.clone());
        let keys = tc.draw(generators::vecs(generators::integers::<u8>()).max_size(40));

        for k in keys {
            let key = match k % 6 {
                0 => key(BareKey::Enter),
                1 => key(BareKey::Esc),
                2 => key(BareKey::Down),
                3 => key(BareKey::Up),
                4 => key(BareKey::Backspace),
                _ => char_key(char::from(b'a' + (k % 26))),
            };
            select.handle_key(key);
            if let Some(value) = select.selected_option() {
                assert!(
                    options.iter().any(|o| o == value),
                    "{:?} is not one of {:?}",
                    value,
                    options
                );
            }
        }
    }

    /// An out-of-range highlight is an Enter away from committing the wrong
    /// option or nothing at all.
    #[hegel::test]
    fn the_highlight_stays_inside_the_matches(tc: hegel::TestCase) {
        let options: Vec<String> = tc.draw(generators::vecs(generators::text()).max_size(10));
        let query = tc.draw(generators::text());
        let mut select = Select::new(options);
        select.open();
        for c in query.chars() {
            select.handle_key(char_key(c));
        }
        if select.dropdown.is_empty() {
            assert_eq!(select.highlighted_option(), None);
        } else {
            assert!(
                select.highlighted_option().is_some(),
                "an open select with {} matches highlighted nothing",
                select.dropdown.matches().len()
            );
        }
    }
}
