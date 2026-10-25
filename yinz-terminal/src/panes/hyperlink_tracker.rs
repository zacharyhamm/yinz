use crate::panes::grid::Row;
use crate::panes::link_handler::LinkHandler;
use crate::panes::terminal_character::{Cursor, LinkAnchor};
use std::collections::VecDeque;
use std::ops::RangeInclusive;

// Bound automatic detection work and storage; explicit OSC 8 links bypass this heuristic.
const MAX_DETECTED_URL_BYTES: usize = 8 * 1024;
const URL_SCHEMES: [&str; 4] = ["http://", "https://", "ftp://", "mailto:"];

#[derive(Debug, Clone)]
struct DetectedLink {
    url: String,
    start_position: HyperlinkPosition,
    end_position: HyperlinkPosition,
}

#[derive(Debug, Clone, Copy)]
struct HyperlinkPosition {
    x: isize,
    y: isize,
}

impl HyperlinkPosition {
    fn from_cursor(cursor: &Cursor) -> Self {
        Self {
            x: cursor.x as isize,
            y: cursor.y as isize,
        }
    }

    fn row_and_character_index(
        &self,
        width: usize,
        viewport: &VecDeque<Row>,
        lines_above: &VecDeque<Row>,
    ) -> Option<(usize, usize)> {
        let x = self.x.max(0) as usize;
        if self.y >= 0 {
            let y = self.y as usize;
            let row = viewport.get(y)?;
            return Some((lines_above.len() + y, row.absolute_character_index(x)));
        }

        let mut rows_above = self.y.unsigned_abs();
        for (index, row) in lines_above.iter().enumerate().rev() {
            // Match Row::split_to_rows_of_length: a wide glyph can leave a short wrap.
            let mut wrap_starts = vec![0];
            let mut column = 0;
            let mut wrap_width = 0;
            for character in &row.columns {
                if wrap_width + character.width() > width {
                    wrap_starts.push(column);
                    wrap_width = 0;
                }
                column += character.width();
                wrap_width += character.width();
            }
            if rows_above <= wrap_starts.len() {
                let column = wrap_starts[wrap_starts.len() - rows_above] + x;
                return Some((index, row.absolute_character_index(column)));
            }
            rows_above -= wrap_starts.len();
        }
        None
    }
}

#[derive(Clone)]
pub struct HyperlinkTracker {
    buffer: String,
    cursor_positions: Vec<HyperlinkPosition>,
    start_position: Option<HyperlinkPosition>,
    last_cursor: Option<HyperlinkPosition>,
}

impl HyperlinkTracker {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            cursor_positions: Vec::new(),
            start_position: None,
            last_cursor: None,
        }
    }

    pub fn update(
        &mut self,
        ch: char,
        cursor: &Cursor,
        width: usize,
        viewport: &mut VecDeque<Row>,
        lines_above: &mut VecDeque<Row>,
        link_handler: &mut LinkHandler,
    ) -> Option<RangeInclusive<usize>> {
        let mut affected_rows = None;
        let current_pos = HyperlinkPosition::from_cursor(cursor);

        // Check if cursor moved non-contiguously
        if self.should_reset_due_to_cursor_jump(&current_pos) {
            if self.is_currently_tracking() {
                // Finalize the current URL before resetting
                affected_rows = self.finalize_and_apply(width, viewport, lines_above, link_handler);
            } else {
                self.clear();
            }
        }

        if self.is_currently_tracking() {
            if self.is_url_terminator(ch) {
                affected_rows = self.finalize_and_apply(width, viewport, lines_above, link_handler);
            } else {
                self.buffer.push(ch);
                self.cursor_positions.push(current_pos);
                if self.buffer.len() > MAX_DETECTED_URL_BYTES
                    || !URL_SCHEMES.iter().any(|scheme| {
                        scheme.starts_with(&self.buffer) || self.buffer.starts_with(scheme)
                    })
                {
                    self.clear();
                }
            }
        } else {
            if matches!(ch, 'h' | 'f' | 'm') {
                self.buffer.push(ch);
                self.cursor_positions.push(current_pos);
                self.start_position = Some(current_pos);
            }
        }

        self.last_cursor = Some(current_pos);
        affected_rows
    }

    pub fn offset_cursor_lines_in_range(&mut self, top: isize, bottom: isize, offset: isize) {
        // Offset only positions inside the given row range (a scroll region),
        // used when a scroll region that does not start at the top of the
        // screen scrolls: rows outside it do not move
        for pos in &mut self.cursor_positions {
            if pos.y >= top && pos.y <= bottom {
                pos.y -= offset;
            }
        }
        if let Some(start_pos) = &mut self.start_position
            && start_pos.y >= top
            && start_pos.y <= bottom
        {
            start_pos.y -= offset;
        }
        if let Some(last_cursor) = &mut self.last_cursor
            && last_cursor.y >= top
            && last_cursor.y <= bottom
        {
            last_cursor.y -= offset;
        }
    }

    pub fn offset_cursor_lines(&mut self, offset: isize) {
        // Offset all stored cursor positions
        for pos in &mut self.cursor_positions {
            pos.y -= offset;
        }

        if let Some(start_pos) = &mut self.start_position {
            start_pos.y -= offset;
        }

        if let Some(last_cursor) = &mut self.last_cursor {
            last_cursor.y -= offset;
        }
    }

    fn should_reset_due_to_cursor_jump(&self, current_pos: &HyperlinkPosition) -> bool {
        if let Some(last_pos) = &self.last_cursor {
            // Check if cursor moved non-contiguously
            let is_contiguous =
                // Same line, next column
                (current_pos.y == last_pos.y && current_pos.x == last_pos.x + 1) ||
                // Next line, first column (line wrap)
                (current_pos.y == last_pos.y + 1 && current_pos.x == 0) ||
                // Same position (overwrite)
                (current_pos.y == last_pos.y && current_pos.x == last_pos.x);

            !is_contiguous
        } else {
            false
        }
    }

    fn is_currently_tracking(&self) -> bool {
        self.start_position.is_some()
    }

    fn is_url_terminator(&self, ch: char) -> bool {
        matches!(
            ch,
            ' ' | '\n'
                | '\r'
                | '\t'
                | '"'
                | '\''
                | '<'
                | '>'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '⏎'
        )
    }

    fn finalize_and_apply(
        &mut self,
        width: usize,
        viewport: &mut VecDeque<Row>,
        lines_above: &mut VecDeque<Row>,
        link_handler: &mut LinkHandler,
    ) -> Option<RangeInclusive<usize>> {
        let mut affected_rows = None;
        let original_len = self.buffer.chars().count();
        let trimmed_url = self.trim_trailing_punctuation(&self.buffer);
        let trimmed_len = trimmed_url.chars().count();

        if self.is_valid_url(&trimmed_url) {
            // Calculate how many characters we trimmed
            let chars_trimmed = original_len.saturating_sub(trimmed_len);

            // Find the end position by walking back from the last position
            let end_position = if chars_trimmed > 0 && trimmed_len > 0 {
                // Use the position of the last character that's actually in the trimmed URL
                self.cursor_positions.get(trimmed_len.saturating_sub(1))
            } else {
                // No trimming occurred, use the last position
                self.cursor_positions.last()
            };
            let Some(end_position) = end_position.copied() else {
                self.clear();
                return None;
            };

            let detected_link = DetectedLink {
                url: trimmed_url.clone(),
                start_position: self.start_position.unwrap(),
                end_position,
            };

            affected_rows = self.apply_hyperlink_to_grid(
                &detected_link,
                width,
                viewport,
                lines_above,
                link_handler,
            );
        }

        self.clear();
        affected_rows
    }

    fn apply_hyperlink_to_grid(
        &self,
        link: &DetectedLink,
        width: usize,
        viewport: &mut VecDeque<Row>,
        lines_above: &mut VecDeque<Row>,
        link_handler: &mut LinkHandler,
    ) -> Option<RangeInclusive<usize>> {
        let (start_row, start_character) =
            match link
                .start_position
                .row_and_character_index(width, viewport, lines_above)
            {
                Some(position) => position,
                None if link.start_position.y < 0 => (0, 0), // The beginning was evicted.
                None => return None,
            };
        let (end_row, last_character) =
            link.end_position
                .row_and_character_index(width, viewport, lines_above)?;
        let scrollback_rows = lines_above.len();
        let link_anchor_start = link_handler.new_link_from_url(link.url.clone());

        for (y, row) in lines_above
            .iter_mut()
            .chain(viewport.iter_mut())
            .enumerate()
            .take(end_row + 1)
            .skip(start_row)
        {
            let start = if y == start_row { start_character } else { 0 };
            // Advance by one glyph, not one display column, for the exclusive endpoint.
            let end = if y == end_row {
                (last_character + 1).min(row.columns.len())
            } else {
                row.columns.len()
            };
            for character in row.columns.iter_mut().take(end).skip(start) {
                character
                    .styles
                    .update(|styles| styles.link_anchor = Some(link_anchor_start));
            }
            if y == end_row
                && let Some(character) = row.columns.get_mut(end)
                && !matches!(character.styles.link_anchor, Some(LinkAnchor::Start(_)))
            {
                character
                    .styles
                    .update(|styles| styles.link_anchor = Some(LinkAnchor::End));
            }
        }
        Some(start_row.saturating_sub(scrollback_rows)..=end_row.checked_sub(scrollback_rows)?)
    }

    fn trim_trailing_punctuation(&self, url: &str) -> String {
        let mut chars: Vec<char> = url.chars().collect();

        while let Some(&last_char) = chars.last() {
            if matches!(last_char, '.' | ',' | ';' | '!' | '?' | '\n' | '\r' | '⏎') {
                chars.pop();
            } else {
                break;
            }
        }

        chars.into_iter().collect()
    }

    fn is_valid_url(&self, url: &str) -> bool {
        if url.len() < 8 {
            return false;
        }

        if (url.starts_with("http://") || url.starts_with("https://"))
            && let Some(protocol_end) = url.find("://")
        {
            let after_protocol = &url[protocol_end.saturating_add(3)..];
            return !after_protocol.is_empty() && after_protocol.contains('.');
        }

        if url.starts_with("ftp://") {
            let after_protocol = url.get(6..).unwrap_or("");
            return !after_protocol.is_empty();
        }

        if url.starts_with("mailto:") {
            let after_colon = url.get(7..).unwrap_or("");
            return after_colon.contains('@');
        }

        false
    }

    fn clear(&mut self) {
        self.buffer.clear();
        self.cursor_positions.clear();
        self.start_position = None;
        // Don't clear last_cursor here - we need it for jump detection
    }
}

#[cfg(test)]
#[path = "unit/hyperlink_tracker_tests.rs"]
mod tests;
