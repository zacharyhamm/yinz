//! Text selection and shell-command navigation.

use super::{calculate_row_display_height, Grid, Osc133MarkerKind, Row};
use crate::panes::terminal_character::EMPTY_TERMINAL_CHARACTER;
use crate::panes::Selection;
use std::collections::VecDeque;
use yinz_types::geometry::Direction;
use yinz_types::position::Position;

impl Grid {
    pub fn start_selection(&mut self, start: &Position) {
        let old_selection = self.selection;
        // a mouse selection takes over from a keyboard one, cursor included
        self.text_selection_active = false;
        self.click.record_click(*start);

        if self.click.is_double_click() {
            let Some((start_position, end_position)) = self.word_around_position(start) else {
                // no-op
                return;
            };
            self.selection
                .set_start_and_end_positions(start_position, end_position);
            for i in std::cmp::min(start_position.line.get(), end_position.line.get())
                ..=std::cmp::max(start_position.line.get(), end_position.line.get())
            {
                self.output_buffer.update_line(i as usize);
            }
            self.mark_for_rerender();
            return;
        } else if self.click.is_triple_click() {
            let Some((start_position, end_position)) = self
                .osc133_command_selection
                .then(|| self.osc133_command_around_position(start))
                .flatten()
                .or_else(|| self.canonical_line_around_position(start))
            else {
                // no-op
                return;
            };
            self.selection
                .set_start_and_end_positions(start_position, end_position);
            let current_selection = self.selection;
            self.update_selected_lines(&old_selection, &current_selection);
            self.mark_for_rerender();
            return;
        }

        self.selection.start(*start);
        self.update_selected_lines(&old_selection, &self.selection.clone());
        self.mark_for_rerender();
    }
    pub fn start_text_selection(&mut self, start: Position) {
        let old_selection = self.selection;
        self.copied_text_selection = None;
        self.text_selection_active = true;
        self.selection.start(start);
        self.update_selected_lines(&old_selection, &self.selection.clone());
        self.mark_for_rerender();
    }
    pub fn move_text_selection(&mut self, direction: Direction) {
        if !self.selection.is_active() || self.height == 0 || self.width == 0 {
            return;
        }
        let mut end = self.selection.end;
        match direction {
            Direction::Left => end.change_column(end.column.get().saturating_sub(1)),
            Direction::Right => end.change_column((end.column.get() + 1).min(self.width)),
            Direction::Up if end.line.get() > 0 => end.change_line(end.line.get() - 1),
            Direction::Down if end.line.get() < self.height.saturating_sub(1) as isize => {
                end.change_line(end.line.get() + 1);
            },
            Direction::Up => {
                self.scroll_up_one_line();
                return;
            },
            Direction::Down => {
                self.scroll_down_one_line();
                return;
            },
        }
        let old_selection = self.selection;
        self.selection.to(end);
        self.update_selected_lines(&old_selection, &self.selection.clone());
        self.mark_for_rerender();
    }
    pub fn swap_text_selection_ends(&mut self) {
        if !self.selection.is_active() {
            return;
        }
        self.selection.swap_ends();
        self.mark_for_rerender();
    }
    pub fn remember_copied_text_selection(&mut self) {
        let mut copied_text_selection = self.selection;
        copied_text_selection.finalize();
        self.copied_text_selection = Some(copied_text_selection);
    }
    pub fn finish_text_selection(&mut self) {
        let old_selection = self.selection;
        self.text_selection_active = false;
        if let Some(copied_text_selection) = self.copied_text_selection.take() {
            self.selection = copied_text_selection;
        } else if self.selection.is_active() {
            self.reset_selection();
            return;
        }
        self.update_selected_lines(&old_selection, &self.selection.clone());
        self.mark_for_rerender();
    }
    pub fn update_selection(&mut self, to: &Position) {
        let old_selection = self.selection;
        if &old_selection.end != to {
            if self.click.is_double_click() {
                let Some((word_start_position, word_end_position)) = self.word_around_position(to)
                else {
                    // no-op
                    return;
                };
                self.selection
                    .add_word_to_position(word_start_position, word_end_position);
                let current_selection = self.selection;
                self.update_selected_lines(&old_selection, &current_selection);
                self.mark_for_rerender();
            } else if self.click.is_triple_click() {
                let Some(last_index_in_line) = self.last_index_in_line(to) else {
                    return;
                };
                self.selection
                    .add_line_to_position(to.line.get(), last_index_in_line);
                let current_selection = self.selection;
                self.update_selected_lines(&old_selection, &current_selection);
                self.mark_for_rerender();
            } else {
                self.selection.to(*to);
                self.update_selected_lines(&old_selection, &self.selection.clone());
                self.mark_for_rerender();
            }
        }
    }

    pub fn end_selection(&mut self, end: &Position) {
        self.text_selection_active = false;
        if !self.click.is_double_click() && !self.click.is_triple_click() {
            let old_selection = self.selection;
            self.selection.end(*end);
            self.update_selected_lines(&old_selection, &self.selection.clone());
        } else {
            // we do this rather than using .end() so that the selection will be marked as inactive
            // (so we won't keep changing its start/end points as we scroll) but so we won't update
            // its end position to the above "end" position (which is incorrect behavior for
            // double/triple click - it will mean we won't mark until the end of the word/line)
            self.selection.finalize();
        }
        self.mark_for_rerender();
    }

    pub fn reset_selection(&mut self) {
        let old_selection = self.selection;
        self.text_selection_active = false;
        self.selection.reset();
        self.update_selected_lines(&old_selection, &self.selection.clone());
        self.mark_for_rerender();
    }
    pub fn get_selected_text(&self) -> Option<String> {
        if self.selection.is_empty() {
            return None;
        }
        self.text_for_selection(&self.selection)
    }
    /// The text between two positions, resolved the way a selection is (scrollback included).
    pub fn text_in_range(&self, start: Position, end: Position) -> Option<String> {
        let mut range_selection = Selection::default();
        range_selection.set_start_and_end_positions(start, end);
        if range_selection.is_empty() {
            return None;
        }
        self.text_for_selection(&range_selection)
    }
    /// The text a `Selection` covers, whether or not it is the selection the user is making.
    ///
    /// Split out of `get_selected_text` so that anything with a pair of positions - the word under
    /// the cursor, the logical line under it - can be turned into text without going through the
    /// grid's own selection state. Two things it carries that a naive row-slice would not: the
    /// lookup spans `lines_above` and `lines_below`, so a selection reaching into scrollback
    /// resolves; and a non-canonical row is joined onto the previous one, so a wrapped logical
    /// line comes back as the single line it is on screen.
    pub fn text_for_selection(&self, selection: &Selection) -> Option<String> {
        // Track the last explicit character's byte offset in each logical line.
        let mut lines: Vec<(String, usize)> = vec![];

        let sorted_selection = selection.sorted();
        let (start, end) = (sorted_selection.start, sorted_selection.end);

        for l in sorted_selection.line_indices() {
            // on the first line of the selection, use the selection start column
            // otherwise, start at the beginning of the line
            let start_column = if l == start.line.get() {
                start.column.get()
            } else {
                0
            };

            // same thing on the last line, but with the selection end column
            let end_column = if l == end.line.get() {
                end.column.get()
            } else {
                self.width
            };

            if start_column == end_column {
                continue;
            }

            let empty_row =
                Row::from_columns(VecDeque::from(vec![EMPTY_TERMINAL_CHARACTER; self.width]));

            // get the row from lines_above, viewport, or lines below depending on index
            let row = if l < 0 && self.lines_above.len() >= l.unsigned_abs() {
                let offset_from_end = l.abs();
                &self.lines_above[self
                    .lines_above
                    .len()
                    .saturating_sub(offset_from_end as usize)]
            } else if l >= 0 && (l as usize) < self.viewport.len() {
                &self.viewport[l as usize]
            } else if (l as usize) < self.height {
                // index is in viewport but there is no line
                &empty_row
            } else if self.lines_below.len() > (l as usize).saturating_sub(self.viewport.len()) {
                &self.lines_below[(l as usize) - self.viewport.len()]
            } else {
                // can't find the line, this probably it's on the pane border
                // is on the pane border
                continue;
            };

            if row.is_canonical || lines.is_empty() {
                lines.push((String::new(), 0));
            }
            let (line, explicit_end) = lines.last_mut()?;
            let mut terminal_col = 0;
            for terminal_character in &row.columns {
                if (start_column..end_column).contains(&terminal_col) {
                    line.push(terminal_character.character);
                    if !terminal_character.is_implicit_blank() {
                        *explicit_end = line.len();
                    }
                }

                terminal_col += terminal_character.width();
            }
        }

        // Trim padding only after joining wrapped rows, preserving interior gaps.
        let lines: Vec<_> = lines
            .into_iter()
            .map(|(mut line, end)| {
                line.truncate(end);
                line
            })
            .collect();

        if lines.is_empty() {
            None
        } else {
            Some(lines.join("\n"))
        }
    }
    pub fn absolute_position_in_scrollback(&self) -> usize {
        self.lines_above.len() + self.cursor.y
    }
    pub fn last_index_in_line(&self, position: &Position) -> Option<usize> {
        let position_row = self.viewport.get(position.line.get() as usize)?;
        Some(position_row.last_index_in_line())
    }
    pub fn word_around_position(&self, position: &Position) -> Option<(Position, Position)> {
        let position_row = self.viewport.get(position.line.get() as usize)?;
        let (index_start, index_end) = position_row
            .word_indices_around_character_index(position.column.get(), &self.word_separators)?;

        let mut position_start = Position::new(position.line.get() as i32, index_start as u16);
        let mut position_end = Position::new(position.line.get() as i32, index_end as u16);
        let mut position_row_is_canonical = position_row.is_canonical;

        while !position_row_is_canonical && position_start.column.get() == 0 {
            if let Some(position_row_above) = self
                .viewport
                .get(position_start.line.get().saturating_sub(1) as usize)
            {
                let new_start_index =
                    position_row_above.word_start_index_of_last_character(&self.word_separators);
                position_start = Position::new(
                    position_start.line.get().saturating_sub(1) as i32,
                    new_start_index as u16,
                );
                position_row_is_canonical = position_row_above.is_canonical;
            } else {
                break;
            }
        }

        let mut column_count_in_row = position_row.width();
        while position_end.column.get() == column_count_in_row {
            if let Some(position_row_below) =
                self.viewport.get(position_end.line.get() as usize + 1)
            {
                if position_row_below.is_canonical {
                    break;
                }
                let new_end_index =
                    position_row_below.word_end_index_of_first_character(&self.word_separators);
                position_end =
                    Position::new(position_end.line.get() as i32 + 1, new_end_index as u16);
                column_count_in_row = position_row_below.width();
            } else {
                break;
            }
        }

        Some((position_start, position_end))
    }
    pub fn canonical_line_around_position(
        &self,
        position: &Position,
    ) -> Option<(Position, Position)> {
        let position_row = self.viewport.get(position.line.get() as usize)?;

        let mut position_start = Position::new(position.line.get() as i32, 0);
        let mut position_end = Position::new(
            position.line.get() as i32,
            (position_row.columns.len() + position_row.excess_width()) as u16,
        );

        let mut found_canonical_row_start = position_row.is_canonical;
        while !found_canonical_row_start {
            if let Some(row_above) = self
                .viewport
                .get(position_start.line.get().saturating_sub(1) as usize)
            {
                position_start.change_line(position_start.line.get().saturating_sub(1));
                found_canonical_row_start = row_above.is_canonical;
            } else {
                break;
            }
        }

        let mut found_canonical_row_end = false;
        while !found_canonical_row_end {
            if let Some(row_below) = self.viewport.get(position_end.line.get() as usize + 1) {
                if row_below.is_canonical {
                    found_canonical_row_end = true;
                } else {
                    position_end = Position::new(
                        position_end.line.get() as i32 + 1,
                        row_below.columns.len() as u16,
                    );
                }
            } else {
                break;
            }
        }
        Some((position_start, position_end))
    }

    fn row_at(&self, line: isize) -> Option<&Row> {
        if line < 0 {
            let offset_from_end = line.unsigned_abs();
            if self.lines_above.len() >= offset_from_end {
                self.lines_above
                    .get(self.lines_above.len() - offset_from_end)
            } else {
                None
            }
        } else if (line as usize) < self.viewport.len() {
            self.viewport.get(line as usize)
        } else {
            self.lines_below.get((line as usize) - self.viewport.len())
        }
    }

    /// The command around `position`, from its `Input` (or `Output`) marker to the next marker.
    ///
    /// Not gated on `osc133_command_selection`: that option only opts out of triple-click grabbing
    /// a command, and the explicit prompt/command keybindings stay usable.
    fn osc133_command_around_position(&self, position: &Position) -> Option<(Position, Position)> {
        if !self.osc133_markers_seen {
            return None;
        }
        let first_line = -(self.lines_above.len() as isize);
        let last_line = (self.viewport.len() + self.lines_below.len()) as isize - 1;
        let clicked_line = position.line.get();
        let clicked_column = position.column.get();
        let marker_position = Self::osc133_marker_position;

        let mut latest_output = None;
        let mut selection_start = None;
        'backward: for line in (first_line..=clicked_line.min(last_line)).rev() {
            let Some(row) = self.row_at(line) else {
                continue;
            };
            for marker in row.osc133_markers.iter().rev() {
                if line == clicked_line && marker.column > clicked_column {
                    continue;
                }
                match marker.kind {
                    Osc133MarkerKind::Output => {
                        if latest_output.is_none() {
                            latest_output = Some(marker_position(line, marker.column));
                        }
                    },
                    Osc133MarkerKind::Input => {
                        latest_output?;
                        selection_start = Some(marker_position(line, marker.column));
                        break 'backward;
                    },
                    Osc133MarkerKind::Prompt | Osc133MarkerKind::End => {
                        latest_output?;
                        break 'backward;
                    },
                }
            }
        }
        let selection_start = selection_start.or(latest_output)?;

        let mut selection_end = None;
        'forward: for line in clicked_line.max(first_line)..=last_line {
            let Some(row) = self.row_at(line) else {
                continue;
            };
            for marker in row.osc133_markers.iter() {
                if line == clicked_line && marker.column <= clicked_column {
                    continue;
                }
                selection_end = Some(marker_position(line, marker.column));
                break 'forward;
            }
        }
        let selection_end = selection_end?;

        Some((selection_start, selection_end))
    }

    fn osc133_marker_position(line: isize, column: usize) -> Position {
        Position::new(line as i32, column.min(u16::MAX as usize) as u16)
    }

    /// How many display rows up the previous prompt is, or `None` when there is none above.
    ///
    /// Walks the scrollback upwards. A `Prompt` marker wins outright; an `Input` marker is
    /// remembered as a fallback for shells that only mark the command, and is used once the
    /// walk passes the `Output`/`End` of the command before it.
    fn previous_prompt_line_delta(&self) -> Option<usize> {
        if !self.osc133_markers_seen {
            return None;
        }
        let first_line = -(self.lines_above.len() as isize);
        let mut input_candidate: Option<usize> = None;
        let mut display_delta = 0;
        for line in (first_line..0).rev() {
            let Some(row) = self.row_at(line) else {
                continue;
            };
            display_delta += calculate_row_display_height(row.width(), self.width);
            for marker in row.osc133_markers.iter().rev() {
                match marker.kind {
                    Osc133MarkerKind::Prompt => return Some(display_delta),
                    Osc133MarkerKind::Input => {
                        if input_candidate.is_none() {
                            input_candidate = Some(display_delta);
                        }
                    },
                    Osc133MarkerKind::Output | Osc133MarkerKind::End => {
                        if let Some(candidate) = input_candidate {
                            return Some(candidate);
                        }
                    },
                }
            }
        }
        input_candidate
    }

    /// How many rows down the next prompt is, capped at what `lines_below` can still scroll.
    fn next_prompt_line_delta(&self) -> Option<usize> {
        if !self.osc133_markers_seen {
            return None;
        }
        let last_line = (self.viewport.len() + self.lines_below.len()) as isize - 1;
        for line in 1..=last_line {
            let Some(row) = self.row_at(line) else {
                continue;
            };
            for marker in row.osc133_markers.iter() {
                match marker.kind {
                    Osc133MarkerKind::Prompt | Osc133MarkerKind::Input => {
                        let reachable_delta = (line as usize).min(self.lines_below.len());
                        return (reachable_delta > 0).then_some(reachable_delta);
                    },
                    Osc133MarkerKind::Output | Osc133MarkerKind::End => {},
                }
            }
        }
        None
    }

    pub fn scroll_to_previous_prompt(&mut self) -> bool {
        match self.previous_prompt_line_delta() {
            Some(delta) => {
                self.move_viewport_up(delta);
                true
            },
            None => false,
        }
    }

    pub fn scroll_to_next_prompt(&mut self) -> bool {
        match self.next_prompt_line_delta() {
            Some(delta) => {
                self.move_viewport_down(delta);
                true
            },
            None => false,
        }
    }

    /// The command at the top of the viewport, or failing that the first command whose output
    /// starts somewhere in the viewport.
    fn osc133_command_at_scroll_position(&self) -> Option<(Position, Position)> {
        if let Some(command) = self.osc133_command_around_position(&Position::new(0, 0)) {
            return Some(command);
        }
        for line in 0..self.viewport.len() as isize {
            let Some(row) = self.row_at(line) else {
                continue;
            };
            for marker in row.osc133_markers.iter() {
                if marker.kind != Osc133MarkerKind::Output {
                    continue;
                }
                let anchor = Self::osc133_marker_position(line, marker.column);
                if let Some(command) = self.osc133_command_around_position(&anchor) {
                    return Some(command);
                }
            }
        }
        None
    }

    pub fn select_command_at_scroll_position(&mut self) -> bool {
        let Some((start_position, end_position)) = self.osc133_command_at_scroll_position() else {
            return false;
        };
        let old_selection = self.selection;
        self.selection
            .set_start_and_end_positions(start_position, end_position);
        self.selection.finalize();
        let current_selection = self.selection;
        self.update_selected_lines(&old_selection, &current_selection);
        self.mark_for_rerender();
        true
    }

    fn command_output_text(&self, start: Position, end: Position) -> Option<String> {
        let output = self.text_in_range(start, end)?;
        let output = output
            .strip_prefix('\n')
            .map(String::from)
            .unwrap_or(output);
        if output.trim().is_empty() {
            return None;
        }
        Some(output)
    }

    /// The output of the most recent command that has both an `Output` marker and a marker
    /// after it (its `End`, or the next prompt), with the positions the text spans.
    pub fn last_completed_command_output(&self) -> Option<(String, Position, Position)> {
        if !self.osc133_markers_seen {
            return None;
        }
        let first_line = -(self.lines_above.len() as isize);
        let last_line = (self.viewport.len() + self.lines_below.len()) as isize - 1;
        let mut command_boundary: Option<Position> = None;
        for line in (first_line..=last_line).rev() {
            let Some(row) = self.row_at(line) else {
                continue;
            };
            for marker in row.osc133_markers.iter().rev() {
                let marker_position = Self::osc133_marker_position(line, marker.column);
                if marker.kind == Osc133MarkerKind::Output
                    && let Some(output_end) = command_boundary
                    && let Some(output) = self.command_output_text(marker_position, output_end)
                {
                    return Some((output, marker_position, output_end));
                }
                command_boundary = Some(marker_position);
            }
        }
        None
    }

    pub fn set_command_output_flash(&mut self, start: Position, end: Position) {
        let mut flash = Selection::default();
        flash.set_start_and_end_positions(start, end);
        flash.finalize();
        self.command_output_flash = Some(flash);
        self.output_buffer.update_all_lines();
        self.mark_for_rerender();
    }

    pub fn clear_command_output_flash(&mut self) -> bool {
        if self.command_output_flash.take().is_none() {
            return false;
        }
        self.output_buffer.update_all_lines();
        self.mark_for_rerender();
        true
    }

    #[cfg(test)]
    pub fn command_output_flash_is_shown(&self) -> bool {
        self.command_output_flash.is_some()
    }

    pub(super) fn update_selected_lines(
        &mut self,
        old_selection: &Selection,
        new_selection: &Selection,
    ) {
        for l in old_selection.diff(new_selection, self.height) {
            self.output_buffer.update_line(l as usize);
        }
    }
}
