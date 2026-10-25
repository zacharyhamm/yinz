//! A clickable, focusable label.
//!
//! The caller decides what pressing the button means.
//!
//! Hit-testing is a side effect of rendering: `render` records the rectangle it
//! drew into, and [`Button::contains`] tests against that. A button that has
//! never rendered contains nothing, which is what keeps a click from landing on
//! a button that is not on screen.

use super::viewport::take_columns;
use crate::ui_components::{
    serialize_ribbon_with_coordinates, serialize_text_with_coordinates, Text,
};
use unicode_width::UnicodeWidthStr;

/// The rectangle a widget last drew into, in pane-relative coordinates.
///
/// Bounds are half-open on both axes, so adjacent buttons never claim the same column.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub columns: usize,
    pub rows: usize,
}

impl Rect {
    pub fn new(x: usize, y: usize, columns: usize, rows: usize) -> Self {
        Rect {
            x,
            y,
            columns,
            rows,
        }
    }

    pub fn contains(&self, x: usize, y: usize) -> bool {
        x >= self.x && x < self.x + self.columns && y >= self.y && y < self.y + self.rows
    }
}

/// How a button draws.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ButtonStyle {
    /// A ribbon — the de-facto button look across the builtin plugins.
    #[default]
    Ribbon,
    /// Plain text, for inline links and other places a ribbon would be too loud.
    Text,
}

#[derive(Debug, Default, Clone)]
pub struct Button {
    label: String,
    /// Shown instead of `label` while the pointer is over the button.
    hover_label: Option<String>,
    style: ButtonStyle,
    disabled: bool,
    focused: bool,
    hovered: bool,
    last_rendered: Option<Rect>,
}

impl Button {
    pub fn new<S: ToString>(label: S) -> Self {
        Button {
            label: label.to_string(),
            hover_label: None,
            style: ButtonStyle::default(),
            disabled: false,
            focused: false,
            hovered: false,
            last_rendered: None,
        }
    }

    pub fn hover_label<S: ToString>(mut self, label: S) -> Self {
        self.hover_label = Some(label.to_string());
        self
    }

    pub fn style(mut self, style: ButtonStyle) -> Self {
        self.style = style;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    pub fn is_hovered(&self) -> bool {
        self.hovered
    }

    /// Returns whether this changed anything, so a caller can skip a re-render.
    pub fn set_hovered(&mut self, hovered: bool) -> bool {
        let changed = self.hovered != hovered;
        self.hovered = hovered;
        changed
    }

    /// The rectangle this button last drew into, or `None` if it never has.
    pub fn last_rendered(&self) -> Option<Rect> {
        self.last_rendered
    }

    pub(super) fn clear_rendered(&mut self) {
        self.last_rendered = None;
    }

    /// Whether a click at `(x, y)` lands on this button.
    ///
    /// False for a disabled button: a disabled control that still swallows
    /// clicks is worse than one that ignores them, because nothing on screen
    /// explains why the click did nothing.
    pub fn contains(&self, x: usize, y: usize) -> bool {
        if self.disabled {
            return false;
        }
        self.last_rendered
            .map(|rect| rect.contains(x, y))
            .unwrap_or(false)
    }

    /// The text currently on the button — the hover label while hovered.
    fn visible_label(&self) -> &str {
        match self.hover_label.as_ref() {
            Some(hover) if self.hovered && !self.disabled => hover,
            _ => &self.label,
        }
    }

    /// Columns the button occupies. A ribbon is drawn with one column of
    /// padding on each side by the server, so it is two wider than its text.
    pub fn width(&self) -> usize {
        let label = self.visible_label().width();
        match self.style {
            ButtonStyle::Ribbon => label + 2,
            ButtonStyle::Text => label,
        }
    }

    fn to_text(&self, max_width: Option<usize>) -> Text {
        let label = match max_width {
            Some(max) => {
                let room = match self.style {
                    ButtonStyle::Ribbon => max.saturating_sub(2),
                    ButtonStyle::Text => max,
                };
                take_columns(self.visible_label(), room)
            },
            None => self.visible_label().to_string(),
        };
        let mut text = Text::new(label);
        if self.focused {
            text = text.selected();
        }
        if self.disabled {
            text = text.disabled();
        }
        text
    }

    /// Draw at `(x, y)` and record the rectangle for hit-testing.
    ///
    /// Returns the width drawn, so a caller laying buttons out in a row can
    /// advance without measuring again.
    pub fn render(&mut self, x: usize, y: usize, max_width: Option<usize>) -> usize {
        let output = self.serialize(x, y, max_width);
        print!("{}", output);
        max_width
            .map(|max| self.width().min(max))
            .unwrap_or(self.width())
    }

    /// As [`Button::render`], but returning the escape sequence instead of
    /// printing it. Still records the rectangle — a serialized button is on
    /// screen just as much as a printed one.
    pub fn serialize(&mut self, x: usize, y: usize, max_width: Option<usize>) -> String {
        let text = self.to_text(max_width);
        let width = max_width
            .map(|max| self.width().min(max))
            .unwrap_or(self.width());
        let serialized = if width == 0 {
            String::new()
        } else {
            match self.style {
                // The host may add two arrows as well as its two spaces. If the
                // full ribbon cannot fit, use bounded text: ribbon chrome itself
                // cannot be clipped by the component protocol.
                ButtonStyle::Ribbon if max_width.is_none_or(|max| max >= self.width() + 2) => {
                    serialize_ribbon_with_coordinates(&text, x, y, None, None)
                },
                ButtonStyle::Ribbon => {
                    let text = text.with_content(take_columns(self.visible_label(), width));
                    serialize_text_with_coordinates(&text, x, y, Some(width), Some(1))
                },
                ButtonStyle::Text => {
                    serialize_text_with_coordinates(&text, x, y, Some(width), Some(1))
                },
            }
        };
        self.last_rendered = Some(Rect::new(x, y, width, 1));
        serialized
    }
}

#[cfg(test)]
#[path = "unit/button.rs"]
mod tests;
