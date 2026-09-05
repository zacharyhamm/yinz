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
                        // Enter back is what lets the form submit from it.
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
mod tests {
    use super::*;
    use hegel::generators;

    const SUGGESTIONS: [&str; 3] = ["main", "feature/login", "feature/logout"];

    fn a_combo() -> Combo {
        Combo::new(SUGGESTIONS)
    }

    fn key(bare_key: BareKey) -> KeyWithModifier {
        KeyWithModifier::new(bare_key)
    }

    fn char_key(c: char) -> KeyWithModifier {
        KeyWithModifier::new(BareKey::Char(c))
    }

    fn type_text(combo: &mut Combo, text: &str) {
        for c in text.chars() {
            combo.handle_key(char_key(c));
        }
    }

    #[test]
    fn a_new_combo_is_empty_and_closed() {
        let combo = a_combo();
        assert_eq!(combo.text(), "");
        assert!(!combo.is_open());
        assert_eq!(combo.highlighted_suggestion(), None);
    }

    #[test]
    fn typing_goes_into_the_value_and_opens_the_list() {
        let mut combo = a_combo();
        type_text(&mut combo, "fea");
        assert_eq!(combo.text(), "fea");
        assert!(combo.is_open());
        assert_eq!(combo.dropdown.matches().len(), 2);
        assert_eq!(
            combo.highlighted_suggestion(),
            None,
            "typing must not put the highlight on a row"
        );
    }

    #[test]
    fn typing_something_nothing_matches_leaves_no_list_in_the_way() {
        let mut combo = a_combo();
        type_text(&mut combo, "zzz");
        assert_eq!(combo.text(), "zzz");
        assert!(!combo.is_open());
    }

    #[test]
    fn a_list_with_nothing_in_it_comes_back_when_the_text_matches_again() {
        let mut combo = a_combo();
        type_text(&mut combo, "mz");
        assert!(!combo.is_open());
        combo.handle_key(key(BareKey::Backspace));
        assert!(combo.is_open());
        assert_eq!(combo.text(), "m");
    }

    /// The whole point of the widget: a value the user typed is the value, and
    /// Enter is what commits it.
    #[test]
    fn enter_with_nothing_highlighted_keeps_what_was_typed() {
        let mut combo = a_combo();
        type_text(&mut combo, "fea");
        assert_eq!(
            combo.handle_key(key(BareKey::Enter)),
            ComboAction::Input(InputAction::Submit),
            "the Enter belongs to the form when no suggestion is highlighted"
        );
        assert_eq!(combo.text(), "fea");
        assert!(!combo.is_open());
    }

    #[test]
    fn down_then_enter_takes_the_suggestion() {
        let mut combo = a_combo();
        type_text(&mut combo, "fea");
        assert_eq!(combo.handle_key(key(BareKey::Down)), ComboAction::Continue);
        assert_eq!(combo.highlighted_suggestion(), Some("feature/login"));
        assert_eq!(
            combo.handle_key(key(BareKey::Enter)),
            ComboAction::Committed
        );
        assert_eq!(combo.text(), "feature/login");
        assert!(!combo.is_open());
        assert_eq!(
            combo.input.get_cursor_position(),
            "feature/login".chars().count(),
            "the caret should be at the end of the suggestion, ready to be edited"
        );
    }

    #[test]
    fn down_on_a_closed_combo_opens_the_list_at_the_first_row() {
        let mut combo = a_combo();
        combo.handle_key(key(BareKey::Down));
        assert!(combo.is_open());
        assert_eq!(combo.highlighted_suggestion(), Some("main"));
    }

    #[test]
    fn up_reaches_the_last_row_first() {
        let mut combo = a_combo();
        combo.handle_key(key(BareKey::Up));
        assert!(combo.is_open());
        assert_eq!(combo.highlighted_suggestion(), Some("feature/logout"));
    }

    #[test]
    fn typing_after_arrowing_drops_the_highlight_again() {
        let mut combo = a_combo();
        type_text(&mut combo, "fea");
        combo.handle_key(key(BareKey::Down));
        assert!(combo.highlighted_suggestion().is_some());
        combo.handle_key(char_key('t'));
        assert_eq!(
            combo.highlighted_suggestion(),
            None,
            "a highlight chosen against text that no longer exists would undo the keystroke"
        );
        assert_eq!(combo.text(), "feat");
    }

    #[test]
    fn esc_closes_the_list_and_keeps_the_text() {
        let mut combo = a_combo();
        type_text(&mut combo, "fea");
        assert_eq!(combo.handle_key(key(BareKey::Esc)), ComboAction::Cancelled);
        assert!(!combo.is_open());
        assert_eq!(combo.text(), "fea");
        assert_eq!(
            combo.handle_key(key(BareKey::Esc)),
            ComboAction::Input(InputAction::Cancel),
            "with the list gone, Esc is the form's again"
        );
    }

    #[test]
    fn editing_keys_reach_the_input() {
        let mut combo = a_combo();
        type_text(&mut combo, "main");
        combo.handle_key(KeyWithModifier::new(BareKey::Char('u')).with_ctrl_modifier());
        assert_eq!(combo.text(), "");
    }

    #[test]
    fn a_paste_lands_in_the_value_and_refilters() {
        let mut combo = a_combo();
        combo.insert_pasted_text("feature/log");
        assert_eq!(combo.text(), "feature/log");
        assert!(combo.is_open());
        assert_eq!(combo.dropdown.matches().len(), 2);
    }

    #[test]
    fn accepting_a_suggestion_is_what_a_click_on_one_does() {
        let mut combo = a_combo();
        type_text(&mut combo, "fea");
        assert!(HasDropdown::commit_option(&mut combo, 2));
        assert_eq!(combo.text(), "feature/logout");
        assert!(!combo.is_open());
        assert!(
            !HasDropdown::commit_option(&mut combo, 9),
            "there is no ninth suggestion"
        );
    }

    #[test]
    fn suggestions_that_arrive_later_filter_against_the_text_already_there() {
        let mut combo = Combo::empty();
        type_text(&mut combo, "fea");
        assert!(!combo.is_open(), "there was nothing to suggest");
        combo.open();
        combo.set_suggestions(SUGGESTIONS.iter().map(|s| s.to_string()).collect());
        assert_eq!(combo.dropdown.matches().len(), 2);
        assert_eq!(combo.text(), "fea");
    }

    #[test]
    fn suggestions_going_away_take_the_open_list_with_them() {
        let mut combo = a_combo();
        type_text(&mut combo, "fea");
        assert!(combo.is_open());
        combo.set_suggestions(vec![]);
        assert!(!combo.is_open());
        assert_eq!(combo.text(), "fea");
    }

    #[test]
    fn the_list_is_an_overlay_so_the_row_is_always_one_tall() {
        let mut combo = a_combo();
        assert_eq!(combo.height(), 1);
        combo.open();
        assert_eq!(combo.height(), 1);
    }

    #[test]
    fn a_combo_that_never_rendered_covers_nothing_and_hits_nothing() {
        let mut combo = a_combo();
        combo.open();
        assert_eq!(combo.overlay_rect(), None);
        assert!(!combo.row_contains(0, 0));
        assert_eq!(combo.option_at(0, 1), None);
        assert!(!combo.marker_contains(0));
    }

    #[test]
    fn a_row_shows_the_value_and_a_marker() {
        let mut combo = a_combo();
        type_text(&mut combo, "main");
        assert_eq!(
            combo.row_line("Base: ", 40),
            ("Base: main ▾".to_string(), false)
        );
    }

    #[test]
    fn a_combo_with_nothing_to_suggest_is_a_text_field() {
        let mut combo = Combo::empty();
        type_text(&mut combo, "main");
        assert_eq!(
            combo.row_line("Base: ", 40),
            ("Base: main".to_string(), false)
        );
    }

    #[test]
    fn an_empty_combo_shows_its_placeholder() {
        let combo = a_combo().placeholder("branch name");
        assert_eq!(
            combo.row_line("Base: ", 40),
            ("Base: branch name ▾".to_string(), true)
        );
    }

    #[test]
    fn a_row_never_exceeds_its_width() {
        let mut combo = a_combo().placeholder("some rather long placeholder");
        assert!(combo.row_line("Base: ", 14).0.as_str().width() <= 14);
        combo.set_text("日本語のブランチ名前");
        let (line, _) = combo.row_line("Base: ", 14);
        assert!(line.as_str().width() <= 14, "{:?} is wider than 14", line);
    }

    /// Unlike a collapsed select, a combo is a field you type into.
    #[test]
    fn a_closed_combo_still_places_a_cursor() {
        let mut combo = a_combo();
        assert_eq!(combo.cursor_column("Base: ", 40), 6);
        type_text(&mut combo, "main");
        assert_eq!(combo.cursor_column("Base: ", 40), 10);
    }

    /// Everything except the arrow keys leaves a combo behaving exactly like the
    /// text field it is - the value is never something the user did not type.
    #[hegel::test]
    fn without_the_arrows_a_combo_is_a_text_field(tc: hegel::TestCase) {
        let suggestions: Vec<String> = SUGGESTIONS.iter().map(|s| s.to_string()).collect();
        let mut combo = Combo::new(suggestions);
        let mut input = TextInput::empty();
        let keys = tc.draw(generators::vecs(generators::integers::<u8>()).max_size(40));

        for k in keys {
            let key = match k % 8 {
                0 => key(BareKey::Enter),
                1 => key(BareKey::Esc),
                2 => key(BareKey::Backspace),
                3 => key(BareKey::Left),
                _ => char_key(char::from(b'a' + (k % 26))),
            };
            combo.handle_key(key.clone());
            input.handle_key(key);
            assert_eq!(
                combo.text(),
                input.get_text(),
                "the list changed a value nobody asked it to change"
            );
        }
    }

    /// A highlight outside the matches is an Enter away from committing a
    /// suggestion that is not on screen.
    #[hegel::test]
    fn the_highlight_is_either_nothing_or_a_visible_row(tc: hegel::TestCase) {
        let suggestions: Vec<String> = tc.draw(
            generators::vecs(generators::text())
                .min_size(1)
                .max_size(10),
        );
        let mut combo = Combo::new(suggestions.clone());
        let keys = tc.draw(generators::vecs(generators::integers::<u8>()).max_size(40));

        for k in keys {
            let key = match k % 8 {
                0 => key(BareKey::Down),
                1 => key(BareKey::Up),
                2 => key(BareKey::Enter),
                3 => key(BareKey::Esc),
                4 => key(BareKey::Backspace),
                _ => char_key(char::from(b'a' + (k % 26))),
            };
            combo.handle_key(key);
            match combo.dropdown.highlighted() {
                Some(_) => assert!(
                    combo.highlighted_suggestion().is_some(),
                    "the highlight is outside the matches"
                ),
                None => assert_eq!(combo.highlighted_suggestion(), None),
            }
            assert!(
                combo.is_open() || combo.dropdown.highlighted().is_none(),
                "a closed list still has a highlight in it"
            );
        }
    }
}
