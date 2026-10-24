//! Terminal rows, cell metrics, and reflow-preserved annotations.

use crate::output::CharacterChunk;
use crate::panes::kitty_graphics::PlaceholderMark;
use crate::panes::terminal_character::AnsiCode;
use crate::panes::terminal_character::TerminalCharacter;
use crate::panes::terminal_character::EMPTY_TERMINAL_CHARACTER;
use std::cmp::Ordering;
use std::collections::VecDeque;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::rc::Rc;
use std::str;

#[derive(Clone)]
pub struct Row {
    pub(super) palettes: crate::panes::palette_spans::PaletteSpans,
    pub(super) write_palette: Option<Rc<crate::agent_theme::SourcePalette>>,
    pub columns: VecDeque<TerminalCharacter>,
    pub is_canonical: bool,
    pub(super) width: Option<usize>,
    /// Whether every character in the row occupies exactly one cell. When it does, a cell's
    /// column index *is* its index into `columns`, which lets the width-aware lookups skip their
    /// O(row) scans. Cached alongside `width` and invalidated with it - `None` means "unknown",
    /// never "false".
    pub(super) all_single_width: Option<bool>,
    pub bg_color: Option<AnsiCode>,
    pub(super) osc133_markers: Vec<Osc133Marker>,
    /// The row/column diacritics of the kitty unicode placeholder cells in this row, sparse and
    /// keyed by column. They cannot live on `TerminalCharacter`, which is pinned at 16 bytes, and
    /// they do not need to: a cell *is* a placeholder when its character is
    /// [`kitty_graphics::PLACEHOLDER`], and its image and placement ids come from its foreground
    /// and underline colours, which the cell already carries. Only the marks are extra, and only
    /// for the cells that spell them out - a cell with no marks inherits from the one to its left.
    ///
    /// Maintained exactly like `osc133_markers`, alongside it in every mutation below.
    pub(super) placeholder_marks: Vec<PlaceholderMark>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Osc133MarkerKind {
    Prompt,
    Input,
    Output,
    /// `OSC 133 ; D ; <exit code>` - the exit code is not tracked, nothing reads it yet.
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Osc133Marker {
    pub(super) column: usize,
    pub(super) kind: Osc133MarkerKind,
}

impl Debug for Row {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for character in &self.columns {
            write!(f, "{:?}", character)?;
        }
        Ok(())
    }
}

impl Row {
    #[expect(
        clippy::new_without_default,
        reason = "rows are built through `new` and its builders; no caller needs `Default`"
    )]
    pub fn new() -> Self {
        Row {
            palettes: Default::default(),
            write_palette: None,
            columns: VecDeque::new(),
            is_canonical: false,
            width: None,
            all_single_width: None,
            bg_color: None,
            osc133_markers: vec![],
            placeholder_marks: vec![],
        }
    }
    pub fn from_columns(columns: VecDeque<TerminalCharacter>) -> Self {
        Row {
            palettes: Default::default(),
            write_palette: None,
            columns,
            is_canonical: false,
            width: None,
            all_single_width: None,
            bg_color: None,
            osc133_markers: vec![],
            placeholder_marks: vec![],
        }
    }
    pub fn from_rows(mut rows: Vec<Row>) -> Self {
        if rows.is_empty() {
            Row::new()
        } else {
            let mut first_row = rows.remove(0);
            for row in &mut rows {
                first_row.append(row);
            }
            first_row
        }
    }
    pub fn with_character(mut self, terminal_character: TerminalCharacter) -> Self {
        self.columns.push_back(terminal_character);
        self.invalidate_width_cache();
        self
    }
    pub fn canonical(mut self) -> Self {
        self.is_canonical = true;
        self
    }
    pub(super) fn with_agent_theme(
        mut self,
        palette: Option<Rc<crate::agent_theme::SourcePalette>>,
    ) -> Self {
        self.palettes.set(0..usize::MAX, palette.as_ref());
        self.write_palette = palette;
        self
    }
    pub(super) fn writing(&mut self, palette: &Rc<crate::agent_theme::SourcePalette>) -> &mut Self {
        self.write_palette = palette.has_snapshot().then(|| palette.clone());
        self
    }
    pub(crate) fn split_palette_chunks(&self, mut chunk: CharacterChunk) -> Vec<CharacterChunk> {
        if self.palettes.is_empty() {
            return vec![chunk];
        }
        let mut chunks: Vec<CharacterChunk> = Vec::new();
        let mut x = chunk.x;
        let characters = std::mem::take(&mut chunk.terminal_characters);
        for character in characters {
            let palette = self.palettes.at(x);
            let same = chunks
                .last()
                .is_some_and(|c| match (&c.agent_theme, palette) {
                    (None, None) => true,
                    (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                    _ => false,
                });
            if !same {
                let mut next = chunk.clone();
                next.x = x;
                next.terminal_characters.clear();
                next.agent_theme = palette.cloned();
                chunks.push(next);
            }
            chunks
                .last_mut()
                .unwrap()
                .terminal_characters
                .push(character.clone());
            x += character.width();
        }
        if chunks.is_empty() {
            chunks.push(chunk);
        }
        chunks
    }
    pub fn with_bg_color(mut self, bg_color: Option<AnsiCode>) -> Self {
        self.bg_color = bg_color;
        self
    }
    /// Drop the cached row metrics. Called from every mutation that can change how wide the row
    /// is, or where a given column sits inside `columns`.
    pub(super) fn invalidate_width_cache(&mut self) {
        self.width = None;
        self.all_single_width = None;
    }
    pub fn width_cached(&mut self) -> usize {
        if let Some(width) = self.width {
            width
        } else {
            let mut width = 0;
            let mut all_single_width = true;
            for terminal_character in &self.columns {
                let character_width = terminal_character.width();
                width += character_width;
                all_single_width &= character_width == 1;
            }
            self.width = Some(width);
            self.all_single_width = Some(all_single_width);
            width
        }
    }
    /// `true` only when every character in the row is exactly one cell wide, so column index and
    /// `columns` index coincide. Computed with (and cached alongside) the row width; a `None`
    /// cache means "not known yet", so this recomputes rather than assuming the slow path.
    pub(super) fn all_single_width(&mut self) -> bool {
        if let Some(all_single_width) = self.all_single_width {
            return all_single_width;
        }
        // Computed here rather than by deferring to `width_cached`, which returns early when the
        // width alone is already cached and so would leave this `None` forever.
        let all_single_width = self
            .columns
            .iter()
            .all(|terminal_character| terminal_character.width() == 1);
        self.all_single_width = Some(all_single_width);
        all_single_width
    }
    pub fn width(&self) -> usize {
        let mut width = 0;
        for terminal_character in &self.columns {
            width += terminal_character.width();
        }
        width
    }
    pub fn excess_width(&self) -> usize {
        let mut acc = 0;
        for terminal_character in &self.columns {
            if terminal_character.width() > 1 {
                acc += terminal_character.width() - 1;
            }
        }
        acc
    }
    pub fn excess_width_until(&self, x: usize) -> usize {
        let mut acc = 0;
        for terminal_character in self.columns.iter().take(x) {
            if terminal_character.width() > 1 {
                acc += terminal_character.width() - 1;
            }
        }
        acc
    }
    pub fn absolute_character_index(&self, x: usize) -> usize {
        // return x's width aware index
        let mut absolute_index = x;
        for (i, terminal_character) in self.columns.iter().enumerate().take(x) {
            if i == absolute_index {
                break;
            }
            if terminal_character.width() > 1 {
                absolute_index = absolute_index.saturating_sub(1);
            }
        }
        absolute_index
    }
    pub fn absolute_character_index_and_position_in_char(&self, x: usize) -> (usize, usize) {
        // returns x's width aware index as well as its position inside the wide char (eg. 1 if
        // it's in the middle of a 2-char wide character)
        let mut accumulated_width = 0;
        let mut absolute_index = x;
        let mut position_inside_character = 0;
        for (i, terminal_character) in self.columns.iter().enumerate() {
            accumulated_width += terminal_character.width();
            absolute_index = i;
            if accumulated_width > x {
                let character_start_position = accumulated_width - terminal_character.width();
                position_inside_character = x - character_start_position;
                break;
            }
        }
        (absolute_index, position_inside_character)
    }
    pub fn add_character_at(&mut self, terminal_character: TerminalCharacter, x: usize) {
        let width = self.width_cached();
        let (start, end) =
            if x < width && !(terminal_character.width() == 1 && self.all_single_width()) {
                let (index, inside) = self.absolute_character_index_and_position_in_char(x);
                let start = x - inside;
                let mut end = start + terminal_character.width().max(self.columns[index].width());
                if terminal_character.width() > self.columns[index].width() {
                    end = end.max(
                        start
                            + self.columns[index].width()
                            + self.columns.get(index + 1).map_or(0, |c| c.width()),
                    );
                }
                (start, end)
            } else {
                (x, x + terminal_character.width())
            };
        self.palettes.set(start..end, self.write_palette.as_ref());
        match self.width_cached().cmp(&x) {
            Ordering::Equal => {
                // this is unwrapped because this always happens after self.width_cached()
                *self.width.as_mut().unwrap() += terminal_character.width();
                if terminal_character.width() != 1 {
                    // keep the companion cache honest: the width stays accurate above, so nothing
                    // else will invalidate it and recompute this
                    self.all_single_width = Some(false);
                }
                // adding the character at the end of the current line
                self.columns.push_back(terminal_character);
            },
            Ordering::Less => {
                // adding the character after the end of the current line
                // we pad the line up to the character and then add it
                let width_offset = self.excess_width_until(x);
                let mut gap_fill = EMPTY_TERMINAL_CHARACTER;
                if let Some(bg_color) = self.bg_color {
                    gap_fill
                        .styles
                        .update(|styles| styles.background = Some(bg_color));
                }
                self.columns
                    .resize(x.saturating_sub(width_offset), gap_fill);
                self.columns.push_back(terminal_character);
                self.invalidate_width_cache();
            },
            Ordering::Greater => {
                // adding the character in the middle of the line
                // we replace the character at its position

                // Fast path: in a row of single-width characters the column index is already the
                // index into `columns`, and swapping one single-width character for another can
                // neither shift the following characters nor change the row width. That skips the
                // O(row) index scan below *and* keeps the width cache warm, which matters because
                // a full-line redraw walks this branch once per column.
                if terminal_character.width() == 1 && self.all_single_width() {
                    if self.columns[x].character != ' ' {
                        self.osc133_markers.retain(|marker| marker.column != x);
                    }
                    self.discard_placeholder_marks_in(x..x + 1);
                    self.columns[x] = terminal_character;
                    return;
                }

                let (absolute_x_index, position_inside_character) =
                    self.absolute_character_index_and_position_in_char(x);
                let character_width = terminal_character.width();
                let overwrite_start = x.saturating_sub(position_inside_character);
                let overwrite_end =
                    overwrite_start + character_width.max(self.columns[absolute_x_index].width());
                let replacing_blank = self.columns[absolute_x_index].character == ' ';
                self.discard_placeholder_marks_in(overwrite_start..overwrite_end);
                self.osc133_markers.retain(|marker| {
                    marker.column < overwrite_start
                        || marker.column >= overwrite_end
                        || (replacing_blank && marker.column == overwrite_start)
                });
                let replaced_character =
                    std::mem::replace(&mut self.columns[absolute_x_index], terminal_character);
                match character_width.cmp(&replaced_character.width()) {
                    Ordering::Greater => {
                        // the replaced character is narrower than the current character
                        // (eg. we added a wide emoji in place of an English character)
                        // we remove the character after it to make room
                        let position_to_remove = absolute_x_index + 1;
                        if let Some(removed) = self.columns.remove(position_to_remove)
                            && removed.width() > 1
                        {
                            // the character we removed is a wide character itself, so we add
                            // padding
                            self.columns
                                .insert(position_to_remove, EMPTY_TERMINAL_CHARACTER);
                        }
                    },
                    Ordering::Less => {
                        // the replaced character is wider than the current character
                        // (eg. we added an English character in place of a wide emoji)
                        // we must make sure to add padding either before the character we added
                        // or after it, depending on our position inside said removed wide character
                        // (`unicode_width` never yields more than 2 for a single `char`)
                        if position_inside_character > 0 {
                            self.columns
                                .insert(absolute_x_index, EMPTY_TERMINAL_CHARACTER);
                        } else {
                            self.columns
                                .insert(absolute_x_index + 1, EMPTY_TERMINAL_CHARACTER);
                        }
                    },
                    _ => {},
                }
                self.invalidate_width_cache();
            },
        }
    }
    pub fn insert_character_at(&mut self, terminal_character: TerminalCharacter, x: usize) {
        let character_width = terminal_character.width();
        for marker in &mut self.osc133_markers {
            if marker.column > x {
                marker.column += character_width;
            }
        }
        for mark in &mut self.placeholder_marks {
            if mark.column >= x {
                mark.column += character_width;
            }
        }
        let insert_position = self.absolute_character_index(x);
        let column = self
            .columns
            .iter()
            .take(insert_position)
            .map(|c| c.width())
            .sum::<usize>()
            + insert_position.saturating_sub(self.columns.len());
        self.palettes
            .insert(column, character_width, self.write_palette.as_ref());
        match self.columns.len().cmp(&insert_position) {
            Ordering::Equal => self.columns.push_back(terminal_character),
            Ordering::Less => {
                self.columns
                    .resize(insert_position, EMPTY_TERMINAL_CHARACTER);
                self.columns.push_back(terminal_character);
            },
            Ordering::Greater => {
                self.columns.insert(insert_position, terminal_character);
            },
        }
        self.invalidate_width_cache();
    }
    pub fn replace_character_at(&mut self, terminal_character: TerminalCharacter, x: usize) {
        let absolute_x_index = self.absolute_character_index(x);
        let start = self
            .columns
            .iter()
            .take(absolute_x_index)
            .map(|c| c.width())
            .sum::<usize>();
        if let Some(character) = self.columns.get_mut(absolute_x_index) {
            let terminal_character_width = terminal_character.width();
            let overwrite_end = start + character.width().max(terminal_character_width);
            self.palettes
                .set(start..overwrite_end, self.write_palette.as_ref());
            // A narrower replacement leaves the wide character's trailing cells behind as
            // padding. That padding keeps the replacement's styles so an erased wide
            // character does not end up half pane-background, half terminal-default.
            let mut padding_character = EMPTY_TERMINAL_CHARACTER;
            padding_character.styles = terminal_character.styles.clone();
            let character = std::mem::replace(character, terminal_character);
            self.osc133_markers
                .retain(|marker| marker.column < x || marker.column >= overwrite_end);
            self.discard_placeholder_marks_in(x..overwrite_end);
            let excess_width = character.width().saturating_sub(terminal_character_width);
            for _ in 0..excess_width {
                self.columns
                    .insert(absolute_x_index, padding_character.clone());
            }
        }
        self.invalidate_width_cache();
    }
    pub fn replace_columns(&mut self, columns: VecDeque<TerminalCharacter>) {
        self.palettes
            .set(0..usize::MAX, self.write_palette.as_ref());
        self.columns = columns;
        self.osc133_markers.clear();
        self.placeholder_marks.clear();
        self.invalidate_width_cache();
    }
    pub fn push(&mut self, terminal_character: TerminalCharacter) {
        let start = self.width();
        self.palettes.set(
            start..start + terminal_character.width(),
            self.write_palette.as_ref(),
        );
        self.columns.push_back(terminal_character);
        self.invalidate_width_cache();
    }
    pub fn truncate(&mut self, x: usize) {
        let width_offset = self.excess_width_until(x);
        let truncate_position = x.saturating_sub(width_offset);
        if truncate_position < self.columns.len() {
            self.columns.truncate(truncate_position);
        }
        self.palettes.set(self.width()..usize::MAX, None);
        self.osc133_markers.retain(|marker| marker.column <= x);
        self.placeholder_marks.retain(|mark| mark.column < x);
        self.invalidate_width_cache();
    }
    pub fn position_accounting_for_widechars(&self, x: usize) -> usize {
        self.character_index_and_start_column(x).0
    }
    pub(super) fn character_index_and_start_column(&self, x: usize) -> (usize, usize) {
        let mut column = 0;
        for (index, terminal_character) in self.columns.iter().enumerate() {
            let character_width = terminal_character.width();
            if column + character_width > x {
                return (index, column);
            }
            column += character_width;
        }
        (self.columns.len() + (x - column), x)
    }
    pub fn replace_and_pad_end(
        &mut self,
        from: usize,
        to: usize,
        terminal_character: TerminalCharacter,
    ) {
        self.osc133_markers.retain(|marker| marker.column <= from);
        self.placeholder_marks.retain(|mark| mark.column < from);
        let (from_index, _) = self.character_index_and_start_column(from);
        self.columns.truncate(from_index);
        let retained_width = self.width();
        self.palettes
            .set(retained_width..usize::MAX, self.write_palette.as_ref());
        if retained_width < from {
            let mut gap_fill = EMPTY_TERMINAL_CHARACTER;
            if let Some(bg_color) = self.bg_color {
                gap_fill
                    .styles
                    .update(|styles| styles.background = Some(bg_color));
            }
            self.columns
                .extend(std::iter::repeat_n(gap_fill, from - retained_width));
        }
        let replacement_length = to.saturating_sub(self.width());
        self.columns
            .extend(std::iter::repeat_n(terminal_character, replacement_length));
        self.invalidate_width_cache();
    }
    pub fn append(&mut self, to_append: &mut Row) {
        let column_offset = self.width();
        self.palettes.append(&to_append.palettes, column_offset);
        self.columns.append(&mut to_append.columns);
        self.osc133_markers
            .extend(to_append.osc133_markers.drain(..).map(|mut marker| {
                marker.column += column_offset;
                marker
            }));
        self.placeholder_marks
            .extend(to_append.placeholder_marks.drain(..).map(|mut mark| {
                mark.column += column_offset;
                mark
            }));
        self.invalidate_width_cache();
    }
    pub fn drain_until(&mut self, x: usize) -> VecDeque<TerminalCharacter> {
        let mut drained_part_len = 0;
        let mut split_pos = 0;
        for next_character in self.columns.iter() {
            // drained_part_len == 0 here is so that if the grid is resized
            // to a size of 1, we won't drop wide characters
            if drained_part_len + next_character.width() <= x || drained_part_len == 0 {
                drained_part_len += next_character.width();
                split_pos += 1
            } else {
                break;
            }
        }
        // Can't use split_off because it doesn't reduce capacity, causing OOM with long lines
        let drained_part = self.columns.drain(..split_pos).collect();
        self.palettes = self.palettes.slice(drained_part_len..usize::MAX);
        self.placeholder_marks.retain_mut(|mark| {
            if mark.column < drained_part_len {
                false
            } else {
                mark.column -= drained_part_len;
                true
            }
        });
        self.invalidate_width_cache();
        drained_part
    }
    pub fn replace_and_pad_beginning(&mut self, to: usize, terminal_character: TerminalCharacter) {
        let (to_position_accounting_for_widechars, character_start_column) =
            self.character_index_and_start_column(to);
        let width_of_current_character = self
            .columns
            .get(to_position_accounting_for_widechars)
            .map(|character| character.width())
            .unwrap_or(1);
        let replaced_end = character_start_column + width_of_current_character;
        self.palettes
            .set(0..replaced_end, self.write_palette.as_ref());
        self.osc133_markers
            .retain(|marker| marker.column >= replaced_end);
        self.placeholder_marks
            .retain(|mark| mark.column >= replaced_end);
        let mut replace_with = VecDeque::from(vec![terminal_character; replaced_end]);
        if to_position_accounting_for_widechars > self.columns.len() {
            self.columns.clear();
        } else if to_position_accounting_for_widechars >= self.columns.len() {
            drop(self.columns.drain(0..to_position_accounting_for_widechars));
        } else {
            drop(self.columns.drain(0..=to_position_accounting_for_widechars));
        }
        replace_with.append(&mut self.columns);
        self.invalidate_width_cache();
        self.columns = replace_with;
    }
    pub fn len(&self) -> usize {
        self.columns.len()
    }
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }
    pub fn delete_and_return_character(&mut self, x: usize) -> Option<TerminalCharacter> {
        let erase_position = self.absolute_character_index(x);
        if erase_position < self.columns.len() {
            self.invalidate_width_cache();
            let deleted = self.columns.remove(erase_position);
            if let Some(deleted) = &deleted {
                let start = self
                    .columns
                    .iter()
                    .take(erase_position)
                    .map(|c| c.width())
                    .sum::<usize>();
                self.palettes.delete(start..start + deleted.width());
                let end = x + deleted.width();
                self.placeholder_marks.retain_mut(|mark| {
                    if (x..end).contains(&mark.column) {
                        false
                    } else {
                        if mark.column >= end {
                            mark.column -= deleted.width();
                        }
                        true
                    }
                });
                self.osc133_markers.retain_mut(|marker| {
                    if (x..end).contains(&marker.column) {
                        false
                    } else {
                        if marker.column >= end {
                            marker.column -= deleted.width();
                        }
                        true
                    }
                });
            }
            deleted
        } else {
            None
        }
    }
    pub fn split_to_rows_of_length(&mut self, max_row_length: usize) -> Vec<Row> {
        let markers = std::mem::take(&mut self.osc133_markers);
        let marks = std::mem::take(&mut self.placeholder_marks);
        let mut parts: Vec<Row> = vec![];
        let mut current_part: VecDeque<TerminalCharacter> = VecDeque::new();
        let mut current_part_len = 0;
        for character in self.columns.drain(..) {
            if current_part_len + character.width() > max_row_length {
                parts.push(Row::from_columns(current_part).with_bg_color(self.bg_color));
                current_part = VecDeque::new();
                current_part_len = 0;
            }
            current_part_len += character.width();
            current_part.push_back(character);
        }
        if !current_part.is_empty() {
            parts.push(Row::from_columns(current_part).with_bg_color(self.bg_color))
        };
        if !parts.is_empty()
            && self.is_canonical
            && let Some(part) = parts.get_mut(0)
        {
            part.is_canonical = true;
        }
        if parts.is_empty() {
            parts.push(self.clone());
        }
        let mut offset = 0;
        let last = parts.len().saturating_sub(1);
        for (index, part) in parts.iter_mut().enumerate() {
            let end = if index == last {
                usize::MAX
            } else {
                offset + part.width()
            };
            part.palettes = self.palettes.slice(offset..end);
            offset += part.width();
        }
        for mut marker in markers {
            let mut column_offset = 0;
            for part_index in 0..parts.len() {
                let part_width = parts[part_index].width();
                if marker.column <= column_offset + part_width || part_index + 1 == parts.len() {
                    marker.column = marker.column.saturating_sub(column_offset).min(part_width);
                    parts[part_index].osc133_markers.push(marker);
                    break;
                }
                column_offset += part_width;
            }
        }
        // The marks travel with the cell they annotate, so unlike an OSC-133 marker they are
        // placed by which part actually holds that column rather than clamped into the last one.
        for mut mark in marks {
            let mut column_offset = 0;
            for part in parts.iter_mut() {
                let part_width = part.width();
                if mark.column < column_offset + part_width {
                    mark.column -= column_offset;
                    part.placeholder_marks.push(mark);
                    break;
                }
                column_offset += part_width;
            }
        }
        self.invalidate_width_cache();
        parts
    }
    /// Forget the placeholder marks of every cell in `columns`, because those cells are being
    /// overwritten. Marks whose cell is gone are harmless - resolution only consults a mark when a
    /// placeholder character is actually sitting at that column - but leaving them would let a
    /// stale row/column reach a *later* placeholder printed at the same spot.
    pub(super) fn discard_placeholder_marks_in(&mut self, columns: std::ops::Range<usize>) {
        if self.placeholder_marks.is_empty() {
            return;
        }
        self.placeholder_marks
            .retain(|mark| !columns.contains(&mark.column));
    }
    /// Start a placeholder cell at `column`, discarding whatever was recorded there before. The
    /// marks themselves arrive afterwards, as zero-width characters, through
    /// [`Row::push_placeholder_mark_value`].
    pub(super) fn begin_placeholder_marks(&mut self, column: usize) {
        self.discard_placeholder_marks_in(column..column + 1);
        self.placeholder_marks.push(PlaceholderMark::new(column));
    }
    /// Record one more diacritic on the placeholder cell at `column`. Returns `false` when that
    /// cell never began one, which is how a combining mark that follows something other than a
    /// placeholder falls through to being dropped as before.
    pub(super) fn push_placeholder_mark_value(&mut self, column: usize, value: u16) -> bool {
        match self
            .placeholder_marks
            .iter_mut()
            .find(|mark| mark.column == column)
        {
            Some(mark) => {
                mark.push_value(value);
                true
            },
            None => false,
        }
    }
    /// The character occupying screen column `x`, accounting for the wide characters before it.
    pub(super) fn character_at(&self, x: usize) -> Option<&TerminalCharacter> {
        self.columns.get(self.absolute_character_index(x))
    }
    pub(super) fn placeholder_mark_at(&self, column: usize) -> Option<PlaceholderMark> {
        self.placeholder_marks
            .iter()
            .find(|mark| mark.column == column)
            .copied()
    }
    pub(super) fn add_osc133_marker(&mut self, column: usize, kind: Osc133MarkerKind) {
        self.osc133_markers.push(Osc133Marker { column, kind });
        self.osc133_markers.sort_by_key(|marker| marker.column);
    }
    pub fn last_index_in_line(&self) -> usize {
        self.columns.len()
    }
    pub fn word_indices_around_character_index(
        &self,
        index: usize,
        word_separators: &str,
    ) -> Option<(usize, usize)> {
        let absolute_character_index = self.absolute_character_index(index);
        let character_at_index = self.columns.get(absolute_character_index)?;
        if is_selection_boundary_character(character_at_index.character, word_separators) {
            return Some((index, index + 1));
        }
        let mut end_position = self
            .columns
            .iter()
            .enumerate()
            .skip(absolute_character_index)
            .find_map(|(i, t_c)| {
                if is_selection_boundary_character(t_c.character, word_separators) {
                    Some(i + self.excess_width_until(i))
                } else {
                    None
                }
            })
            .unwrap_or_else(|| self.columns.len() + self.excess_width());
        let start_position = self
            .columns
            .iter()
            .enumerate()
            .take(absolute_character_index)
            .rev()
            .find_map(|(i, t_c)| {
                if is_selection_boundary_character(t_c.character, word_separators) {
                    Some(i + 1 + self.excess_width_until(i))
                } else {
                    None
                }
            })
            .unwrap_or(0);
        if start_position == end_position {
            // so that if this is only one character, it'll still be marked
            end_position += 1;
        }
        Some((start_position, end_position))
    }
    pub fn word_start_index_of_last_character(&self, word_separators: &str) -> usize {
        let word_width: usize = self
            .columns
            .iter()
            .rev()
            .take_while(|t_c| !is_selection_boundary_character(t_c.character, word_separators))
            .map(TerminalCharacter::width)
            .sum();
        self.width() - word_width
    }
    pub fn word_end_index_of_first_character(&self, word_separators: &str) -> usize {
        self.columns
            .iter()
            .take_while(|t_c| !is_selection_boundary_character(t_c.character, word_separators))
            .map(TerminalCharacter::width)
            .sum()
    }
}

fn is_selection_boundary_character(character: char, word_separators: &str) -> bool {
    character.is_ascii_whitespace() || word_separators.contains(character)
}
