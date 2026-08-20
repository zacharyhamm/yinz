//! Boolean and mutually-exclusive choices for a one-row form field.

use super::viewport::take_columns;
use super::Rect;
use crate::prelude::*;
use crate::ui_components::{print_text_with_coordinates, Text};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Default)]
pub struct Checkbox {
    checked: bool,
}

impl Checkbox {
    pub fn new(checked: bool) -> Self {
        Self { checked }
    }

    pub fn is_checked(&self) -> bool {
        self.checked
    }

    pub fn set_checked(&mut self, checked: bool) {
        self.checked = checked;
    }

    pub fn toggle(&mut self) {
        self.checked = !self.checked;
    }

    pub(super) fn render_row(&self, prompt: &str, x: usize, y: usize, width: usize, focused: bool) {
        let prompt = take_columns(prompt, width);
        let prompt_width = prompt.width();
        let mut prompt = Text::new(prompt).color_all(2);
        if focused {
            prompt = prompt.selected();
        }
        print_text_with_coordinates(prompt, x, y, Some(width), None);

        let room = width.saturating_sub(prompt_width);
        if room > 0 {
            let marker = take_columns(if self.checked { "[x]" } else { "[ ]" }, room);
            let text = if focused {
                Text::new(marker).selected()
            } else {
                Text::new(marker)
            };
            print_text_with_coordinates(text, x + prompt_width, y, Some(room), None);
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RadioGroup {
    options: Vec<String>,
    selected: Option<usize>,
    last_option_rects: Vec<(usize, Rect)>,
}

impl RadioGroup {
    pub fn new<I, S>(options: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: ToString,
    {
        let options: Vec<String> = options
            .into_iter()
            .map(|option| option.to_string())
            .collect();
        let selected = (!options.is_empty()).then_some(0);
        Self {
            options,
            selected,
            last_option_rects: vec![],
        }
    }

    pub fn with_selected(mut self, index: usize) -> Self {
        self.select_index(index);
        self
    }

    pub fn options(&self) -> &[String] {
        &self.options
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    pub fn selected_option(&self) -> Option<&str> {
        self.selected
            .and_then(|index| self.options.get(index))
            .map(String::as_str)
    }

    pub fn select_index(&mut self, index: usize) -> bool {
        if index >= self.options.len() || self.selected == Some(index) {
            return false;
        }
        self.selected = Some(index);
        true
    }

    pub fn select_option(&mut self, option: &str) -> bool {
        self.options
            .iter()
            .position(|candidate| candidate == option)
            .map(|index| self.select_index(index))
            .unwrap_or(false)
    }

    pub(super) fn handle_key(&mut self, key: KeyWithModifier) -> bool {
        let forward = match key.bare_key {
            BareKey::Right | BareKey::Down if key.has_no_modifiers() => true,
            BareKey::Left | BareKey::Up if key.has_no_modifiers() => false,
            _ => return false,
        };
        self.advance(forward)
    }

    /// Move to the next or previous option, wrapping. Returns whether the
    /// selection changed - a group of fewer than two options has nowhere to go.
    pub(super) fn advance(&mut self, forward: bool) -> bool {
        let count = self.options.len();
        if count < 2 {
            return false;
        }
        let current = self.selected.unwrap_or(0);
        let next = if forward {
            (current + 1) % count
        } else {
            (current + count - 1) % count
        };
        self.select_index(next)
    }

    pub(super) fn render_row(
        &mut self,
        prompt: &str,
        x: usize,
        y: usize,
        width: usize,
        focused: bool,
    ) {
        self.last_option_rects.clear();
        let prompt = take_columns(prompt, width);
        let prompt_width = prompt.width();
        let mut prompt = Text::new(prompt).color_all(2);
        if focused {
            prompt = prompt.selected();
        }
        print_text_with_coordinates(prompt, x, y, Some(width), None);

        let right = x.saturating_add(width);
        let mut option_x = x.saturating_add(prompt_width);
        for (index, option) in self.options.iter().enumerate() {
            if option_x >= right {
                break;
            }
            if index > 0 {
                option_x = option_x.saturating_add(1);
                if option_x >= right {
                    break;
                }
            }
            let marker = if self.selected == Some(index) {
                "(*) "
            } else {
                "( ) "
            };
            let visible = take_columns(&format!("{}{}", marker, option), right - option_x);
            let visible_width = visible.width();
            if visible_width == 0 {
                break;
            }
            let text = if focused {
                Text::new(visible).selected()
            } else {
                Text::new(visible)
            };
            print_text_with_coordinates(text, option_x, y, Some(visible_width), None);
            self.last_option_rects
                .push((index, Rect::new(option_x, y, visible_width, 1)));
            option_x = option_x.saturating_add(visible_width);
        }
    }

    pub(super) fn clear_rendered(&mut self) {
        self.last_option_rects.clear();
    }

    pub(super) fn option_at(&self, x: usize, y: usize) -> Option<usize> {
        self.last_option_rects
            .iter()
            .find(|(_, rect)| rect.contains(x, y))
            .map(|(index, _)| *index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(bare_key: BareKey) -> KeyWithModifier {
        KeyWithModifier::new(bare_key)
    }

    #[test]
    fn checkbox_can_be_set_and_toggled() {
        let mut checkbox = Checkbox::new(false);
        checkbox.set_checked(true);
        assert!(checkbox.is_checked());
        checkbox.toggle();
        assert!(!checkbox.is_checked());
    }

    #[test]
    fn radio_group_selects_by_index_or_label() {
        let mut radio = RadioGroup::new(["one", "two"]);
        assert_eq!(radio.selected_option(), Some("one"));
        assert!(radio.select_index(1));
        assert_eq!(radio.selected_option(), Some("two"));
        assert!(radio.select_option("one"));
        assert!(!radio.select_option("missing"));
    }

    #[test]
    fn radio_arrows_wrap() {
        let mut radio = RadioGroup::new(["one", "two", "three"]);
        assert!(radio.handle_key(key(BareKey::Left)));
        assert_eq!(radio.selected_option(), Some("three"));
        assert!(radio.handle_key(key(BareKey::Right)));
        assert_eq!(radio.selected_option(), Some("one"));
    }

    #[test]
    fn empty_radio_group_has_no_selection() {
        let radio = RadioGroup::new(Vec::<String>::new());
        assert_eq!(radio.selected_index(), None);
        assert_eq!(radio.selected_option(), None);
    }

    #[test]
    fn narrow_unicode_radio_options_stay_inside_the_row() {
        let mut radio = RadioGroup::new(["日本語", "English"]);
        radio.render_row("Mode: ", 4, 2, 12, true);
        assert!(radio
            .last_option_rects
            .iter()
            .all(|(_, rect)| rect.x + rect.columns <= 16));
    }
}
