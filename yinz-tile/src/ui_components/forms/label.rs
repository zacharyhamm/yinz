//! A non-interactive caption.
//!
//! Deliberately thin. It exists so a [`Form`](super::Form) row has a uniform
//! left-hand element and so captions truncate in *columns* like everything else
//! in this module, not so it replaces reaching for [`Text`] directly.

use super::viewport::take_columns;
use crate::ui_components::{print_text_with_coordinates, serialize_text_with_coordinates, Text};
use unicode_width::UnicodeWidthStr;

/// The accent level a label uses when none is given.
///
/// Level 2 is what the existing screens already use for titles and captions
/// (e.g. layout-manager's `render_title`), so an unstyled `Label` looks like
/// the captions already on screen.
pub const DEFAULT_LABEL_LEVEL: usize = 2;

#[derive(Debug, Default, Clone)]
pub struct Label {
    content: String,
    /// `None` renders with no emphasis at all, which is what body text wants.
    level: Option<usize>,
    dimmed: bool,
}

impl Label {
    pub fn new<S: ToString>(content: S) -> Self {
        Label {
            content: content.to_string(),
            level: Some(DEFAULT_LABEL_LEVEL),
            dimmed: false,
        }
    }

    /// Colour the whole label at `level` (see the index-level convention on
    /// [`Text::color_range`]: 0-3 emphasis, 6 error, 7 success).
    pub fn level(mut self, level: usize) -> Self {
        self.level = Some(level);
        self
    }

    /// Render with no emphasis — plain body text.
    pub fn plain(mut self) -> Self {
        self.level = None;
        self
    }

    pub fn dimmed(mut self) -> Self {
        self.dimmed = true;
        self
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    /// Columns this label occupies, truncation not applied.
    pub fn width(&self) -> usize {
        self.content.width()
    }

    /// The [`Text`] this label draws as, truncated to `max_width` columns if given.
    ///
    /// Truncation is by column rather than by char so a caption holding a CJK
    /// character cannot overflow the width it was handed.
    pub fn to_text(&self, max_width: Option<usize>) -> Text {
        let content = match max_width {
            Some(max) => take_columns(&self.content, max),
            None => self.content.clone(),
        };
        let mut text = Text::new(content);
        if let Some(level) = self.level {
            text = text.color_all(level);
        }
        if self.dimmed {
            text = text.dim_all();
        }
        text
    }

    pub fn render(&self, x: usize, y: usize, max_width: Option<usize>) {
        print_text_with_coordinates(self.to_text(max_width), x, y, max_width, None);
    }

    pub fn serialize(&self, x: usize, y: usize, max_width: Option<usize>) -> String {
        serialize_text_with_coordinates(&self.to_text(max_width), x, y, max_width, None)
    }
}

#[cfg(test)]
#[path = "unit/label.rs"]
mod tests;
