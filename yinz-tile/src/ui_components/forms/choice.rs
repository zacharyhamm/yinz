//! Boolean and mutually-exclusive choices for a one-row form field.

use super::viewport::take_columns;
use super::Rect;
use super::RenderBuffer;
use crate::prelude::*;
use crate::ui_components::Text;
use unicode_width::UnicodeWidthStr;

/// The whole row is the click target - it is the label that says what the box
/// means - but only the `[x]` marker is highlighted under the pointer, because
/// that is the part a click changes. `last_marker` is where it was drawn.
#[derive(Debug, Clone, Default)]
pub struct Checkbox {
    checked: bool,
    hovered: bool,
    last_marker: Option<Rect>,
}

impl Checkbox {
    pub fn new(checked: bool) -> Self {
        Self {
            checked,
            ..Default::default()
        }
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

    /// Returns whether this changed anything, so a caller can skip a re-render.
    pub(super) fn set_hovered(&mut self, hovered: bool) -> bool {
        let changed = self.hovered != hovered;
        self.hovered = hovered;
        changed
    }

    pub(super) fn clear_rendered(&mut self) {
        self.last_marker = None;
    }

    pub(super) fn render_row(
        &mut self,
        buffer: &mut RenderBuffer,
        prompt: &str,
        x: usize,
        y: usize,
        width: usize,
        focused: bool,
    ) {
        let prompt = take_columns(prompt, width);
        let prompt_width = prompt.width();
        let mut prompt = Text::new(prompt).color_all(2);
        if focused {
            prompt = prompt.selected();
        }
        buffer.text(prompt, x, y, Some(width), None);

        let room = width.saturating_sub(prompt_width);
        if room > 0 {
            let marker = take_columns(if self.checked { "[x]" } else { "[ ]" }, room);
            let marker_width = marker.width();
            let text = if focused || self.hovered {
                Text::new(marker).selected()
            } else {
                Text::new(marker)
            };
            let marker_x = x + prompt_width;
            buffer.text(text, marker_x, y, Some(room), None);
            self.last_marker = Some(Rect::new(marker_x, y, marker_width, 1));
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RadioGroup {
    options: Vec<String>,
    selected: Option<usize>,
    hovered: Option<usize>,
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
            hovered: None,
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

    pub fn clear_selection(&mut self) {
        self.selected = None;
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
        buffer: &mut RenderBuffer,
        prompt: &str,
        x: usize,
        y: usize,
        width: usize,
        focused: bool,
        active: bool,
    ) {
        self.last_option_rects.clear();
        let prompt = take_columns(prompt, width);
        let prompt_width = prompt.width();
        let mut prompt = Text::new(prompt).color_all(2);
        if focused && (!active || prompt_width >= width) {
            prompt = prompt.selected();
        }
        buffer.text(prompt, x, y, Some(width), None);

        let right = x.saturating_add(width);
        let mut option_x = x.saturating_add(prompt_width);
        // Keep the active option visible when the full group does not fit.
        let start = self
            .selected
            .filter(|selected| {
                focused
                    && active
                    && self.options[..=*selected]
                        .iter()
                        .map(|option| option.width() + 5)
                        .sum::<usize>()
                        > width.saturating_sub(prompt_width) + 1
            })
            .unwrap_or(0);
        for (index, option) in self.options.iter().enumerate().skip(start) {
            if option_x >= right {
                break;
            }
            if index > start {
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
            let text = if (focused && active && self.selected == Some(index))
                || self.hovered == Some(index)
            {
                Text::new(visible).selected()
            } else {
                Text::new(visible)
            };
            buffer.text(text, option_x, y, Some(visible_width), None);
            self.last_option_rects
                .push((index, Rect::new(option_x, y, visible_width, 1)));
            option_x = option_x.saturating_add(visible_width);
        }
    }

    pub(super) fn clear_rendered(&mut self) {
        self.last_option_rects.clear();
    }

    /// Highlight the option under the pointer, or none. Returns whether this
    /// changed anything, so a caller can skip a re-render.
    pub(super) fn set_hovered(&mut self, hovered: Option<usize>) -> bool {
        let changed = self.hovered != hovered;
        self.hovered = hovered;
        changed
    }

    pub(super) fn option_at(&self, x: usize, y: usize) -> Option<usize> {
        self.last_option_rects
            .iter()
            .find(|(_, rect)| rect.contains(x, y))
            .map(|(index, _)| *index)
    }
}

#[cfg(test)]
#[path = "unit/choice.rs"]
mod tests;
