//! The list a [`Select`](super::Select) or a [`Combo`](super::Combo) drops
//! under its row: which options match, which one the highlight is on, and where
//! the rows landed for hit-testing.
//!
//! This started inside `select.rs` and moved out the moment a second widget
//! needed a list, the way [`fuzzy_filter`](super::fuzzy_filter),
//! [`list_window`](super::list_window) and [`viewport`](super::viewport)
//! already had. Nothing here knows what the value of the widget above it *is* —
//! a select's value is one of the options, a combo's is whatever was typed —
//! which is exactly why the two can share it.
//!
//! ## The highlight is optional
//!
//! `None` is not "the first row": it is *nothing chosen yet*, and it is what
//! keeps a combo from replacing a genuinely new value with a suggestion the
//! user never asked for. A select re-anchors it with
//! [`Dropdown::ensure_highlight`] after every refilter, because a select with no
//! highlight has nothing to commit.

use super::fuzzy::{fuzzy_filter, Match};
use super::list::list_window;
use super::viewport::take_columns;
use super::Rect;
use super::RenderBuffer;
use crate::ui_components::Text;
use unicode_width::UnicodeWidthStr;

/// Option rows a dropdown shows before it starts scrolling.
pub const DEFAULT_MAX_VISIBLE_OPTIONS: usize = 6;

/// Everything a widget with a dropdown needs from the form holding it.
///
/// `Form` reaches for this rather than for a concrete widget: "the field with an
/// open list" is a question about the list, not about what kind of field it
/// belongs to.
pub(super) trait HasDropdown {
    fn is_open(&self) -> bool;
    fn open(&mut self);
    fn close(&mut self);
    /// The widget's own row, for hit-testing. `None` until it has been drawn.
    fn row_rect(&self) -> Option<Rect>;
    fn row_contains(&self, x: usize, y: usize) -> bool;
    /// Which option a click at `(x, y)` lands on, if any are on screen.
    fn option_at(&self, x: usize, y: usize) -> Option<usize>;
    /// Move the highlight to an option by its index in the full list. Returns
    /// whether it moved, so a hover can skip a render.
    fn highlight_option(&mut self, index: usize) -> bool;
    /// Take the option a click landed on and close.
    fn commit_option(&mut self, index: usize) -> bool;
    fn overlay_rect(&self) -> Option<Rect>;
    fn render_overlay(&mut self, buffer: &mut RenderBuffer, max_rows: usize);
}

#[derive(Debug, Clone)]
pub(super) struct Dropdown {
    /// Options matching the filter, best first. Only meaningful while open.
    matches: Vec<Match>,
    /// Position *within `matches`* of the row under the highlight, or nothing
    /// highlighted at all.
    highlighted: Option<usize>,
    max_visible_options: usize,
    /// The option rows currently on screen, as `(option index, rectangle)`.
    last_option_rects: Vec<(usize, Rect)>,
}

impl Default for Dropdown {
    fn default() -> Self {
        Dropdown {
            matches: vec![],
            highlighted: None,
            max_visible_options: DEFAULT_MAX_VISIBLE_OPTIONS,
            last_option_rects: vec![],
        }
    }
}

impl Dropdown {
    pub(super) fn set_max_visible_options(&mut self, rows: usize) {
        self.max_visible_options = rows.max(1);
    }

    #[cfg(test)]
    pub(super) fn matches(&self) -> &[Match] {
        &self.matches
    }

    pub(super) fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    #[cfg(test)]
    pub(super) fn highlighted(&self) -> Option<usize> {
        self.highlighted
    }

    pub(super) fn set_highlighted(&mut self, position: Option<usize>) {
        self.highlighted = position.filter(|p| *p < self.matches.len());
    }

    /// The option under the highlight, as an index into the full option list.
    pub(super) fn highlighted_index(&self) -> Option<usize> {
        self.highlighted
            .and_then(|p| self.matches.get(p))
            .map(|m| m.index)
    }

    /// Where an option sits in the current matches, if it matched at all.
    pub(super) fn position_of(&self, index: usize) -> Option<usize> {
        self.matches.iter().position(|m| m.index == index)
    }

    /// Recompute the matches for `query`, keeping the highlight on a row that
    /// still exists.
    ///
    /// Narrowing the filter can leave the highlight past the end of what is
    /// left; clamping is what keeps Enter from committing nothing.
    pub(super) fn refilter(&mut self, options: &[String], query: &str) {
        self.matches = fuzzy_filter(options, query);
        self.highlighted = match self.highlighted {
            _ if self.matches.is_empty() => None,
            Some(position) => Some(position.min(self.matches.len() - 1)),
            None => None,
        };
    }

    /// Put the highlight on the first match when there is none and there is
    /// something to highlight — what a select wants, and a combo does not.
    pub(super) fn ensure_highlight(&mut self) {
        if self.highlighted.is_none() && !self.matches.is_empty() {
            self.highlighted = Some(0);
        }
    }

    /// Move the highlight one row, wrapping. From nothing it lands on the first
    /// row going forward and the last going backward.
    pub(super) fn move_highlight(&mut self, forward: bool) {
        let count = self.matches.len();
        if count == 0 {
            return;
        }
        self.highlighted = Some(match self.highlighted {
            None if forward => 0,
            None => count - 1,
            Some(position) if forward => (position + 1) % count,
            Some(position) => (position + count - 1) % count,
        });
    }

    /// Forget the matches and the highlight. Called when a list closes.
    pub(super) fn clear(&mut self) {
        self.matches.clear();
        self.highlighted = None;
        self.last_option_rects.clear();
    }

    pub(super) fn clear_rendered(&mut self) {
        self.last_option_rects.clear();
    }

    /// The area an open dropdown covers, or `None` when it drew no rows.
    pub(super) fn overlay_rect(&self) -> Option<Rect> {
        let (_, first) = self.last_option_rects.first()?;
        Some(Rect::new(
            first.x,
            first.y,
            first.columns,
            self.last_option_rects.len(),
        ))
    }

    pub(super) fn option_at(&self, x: usize, y: usize) -> Option<usize> {
        self.last_option_rects
            .iter()
            .find(|(_, rect)| rect.contains(x, y))
            .map(|(index, _)| *index)
    }

    pub(super) fn highlight_option(&mut self, index: usize) -> bool {
        match self.position_of(index) {
            Some(position) if Some(position) != self.highlighted => {
                self.highlighted = Some(position);
                true
            },
            _ => false,
        }
    }

    /// Draw the list under `row`, in at most `max_rows` rows, over whatever is
    /// already there.
    pub(super) fn render(
        &mut self,
        buffer: &mut RenderBuffer,
        options: &[String],
        row: Rect,
        max_rows: usize,
    ) {
        self.last_option_rects.clear();
        if max_rows == 0 {
            return;
        }
        let visible_rows = self.max_visible_options.min(max_rows);
        let (start, _, end) = list_window(self.matches.len(), visible_rows, self.highlighted);

        for (offset, position) in (start..end).enumerate() {
            let Some(matched) = self.matches.get(position) else {
                continue;
            };
            let Some(option) = options.get(matched.index) else {
                continue;
            };
            let y = row.y + 1 + offset;
            // Padded to the full width so the row underneath is covered rather
            // than showing through to the right of a short option.
            let label = take_columns(option, row.columns);
            let padding = " ".repeat(row.columns.saturating_sub(label.width()));
            let mut text = Text::new(format!("{}{}", label, padding));
            let shown_chars = label.chars().count();
            let indices: Vec<usize> = matched
                .indices
                .iter()
                .copied()
                .filter(|i| *i < shown_chars)
                .collect();
            if !indices.is_empty() {
                text = text.color_indices(3, indices);
            }
            if Some(position) == self.highlighted {
                text = text.selected();
            }
            buffer.text(text, row.x, y, Some(row.columns), None);
            self.last_option_rects
                .push((matched.index, Rect::new(row.x, y, row.columns, 1)));
        }
    }
}

#[cfg(test)]
#[path = "unit/dropdown.rs"]
mod tests;
