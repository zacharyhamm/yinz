use std::collections::HashMap;
use std::sync::Arc;

use yinz_types::structured_render::{
    CursorState, FrameView, WireCell, ATTR_FAST_BLINK, ATTR_SLOW_BLINK, CURSOR_SHAPE_BEAM,
    CURSOR_SHAPE_DEFAULT, CURSOR_SHAPE_UNDERLINE, LINK_NONE,
};

use crate::emulation::damage::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSize {
    pub rows: usize,
    pub cols: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Blink {
    pub slow: bool,
    pub fast: bool,
}

impl Blink {
    pub fn is_set(&self) -> bool {
        self.slow || self.fast
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Underline,
    Beam,
    Default,
}

impl CursorShape {
    pub fn or(self, configured: Option<CursorShape>) -> CursorShape {
        match self {
            CursorShape::Default => configured.unwrap_or(CursorShape::Block),
            asked_for => asked_for,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Occupancy {
    Single,
    WideHead,
    WideTail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyError {
    ReshapeWithoutFullRepaint {
        expected: (usize, usize),
        found: (usize, usize),
    },
    DegenerateViewport {
        found: (usize, usize),
    },
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApplyError::ReshapeWithoutFullRepaint { expected, found } => write!(
                f,
                "a partial frame of {}x{} arrived for a {}x{} screen",
                found.0, found.1, expected.0, expected.1
            ),
            ApplyError::DegenerateViewport { found } => write!(
                f,
                "a frame claimed a {}x{} viewport, which has no cells",
                found.0, found.1
            ),
        }
    }
}

impl std::error::Error for ApplyError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub seq: u64,
    pub cleared: bool,
    pub reshaped: bool,
    pub written: Vec<Span>,
    pub cursor_rows: Option<(usize, usize)>,
}

pub struct ScreenBuffer {
    cols: usize,
    rows: usize,
    cells: Vec<WireCell>,
    cursor: CursorState,
    blinking: usize,
    blinking_rows: Vec<usize>,
    links: HashMap<u8, String>,
    clusters: HashMap<usize, Arc<str>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitSpan {
    pub start: usize,
    pub end: usize,
}

pub fn cell_width(cell: &WireCell) -> usize {
    (cell.width as usize).max(1)
}

impl ScreenBuffer {
    pub fn new(rows: usize, cols: usize) -> Self {
        let rows = rows.max(1);
        let cols = cols.max(1);
        ScreenBuffer {
            cols,
            rows,
            cells: vec![WireCell::BLANK; cols * rows],
            cursor: CursorState::default(),
            blinking: 0,
            blinking_rows: vec![0; rows],
            links: HashMap::new(),
            clusters: HashMap::new(),
        }
    }

    pub fn size(&self) -> TermSize {
        TermSize {
            rows: self.rows,
            cols: self.cols,
        }
    }

    pub fn cell(&self, row: usize, col: usize) -> WireCell {
        if row >= self.rows || col >= self.cols {
            return WireCell::BLANK;
        }
        self.cells[row * self.cols + col]
    }

    pub fn cursor_is_visible(&self) -> bool {
        self.cursor.visible
    }

    pub fn cursor_position(&self) -> (usize, usize) {
        (self.cursor.y as usize, self.cursor.x as usize)
    }

    pub fn cursor_shape(&self) -> CursorShape {
        match self.cursor.shape_kind() {
            CURSOR_SHAPE_UNDERLINE => CursorShape::Underline,
            CURSOR_SHAPE_BEAM => CursorShape::Beam,
            CURSOR_SHAPE_DEFAULT => CursorShape::Default,
            _ => CursorShape::Block,
        }
    }

    pub fn cursor_is_blinking(&self) -> bool {
        self.cursor.blinking()
    }

    pub fn occupancy(&self, row: usize, col: usize) -> Occupancy {
        if self.cell(row, col).is_wide_head() {
            return Occupancy::WideHead;
        }
        if col > 0 && self.cell(row, col - 1).is_wide_head() {
            return Occupancy::WideTail;
        }
        if col > 1 && self.cell(row, col - 2).width >= 3 {
            return Occupancy::WideTail;
        }
        Occupancy::Single
    }

    pub fn cluster_at(&self, row: usize, col: usize) -> Option<&Arc<str>> {
        if row >= self.rows || col >= self.cols || self.clusters.is_empty() {
            return None;
        }
        self.clusters.get(&(row * self.cols + col))
    }

    pub fn push_text_to(&self, row: usize, col: usize, out: &mut String) {
        match self.cluster_at(row, col) {
            Some(text) => out.push_str(text),
            None => out.push(self.cell(row, col).character()),
        }
    }

    #[cfg(test)]
    pub fn text_at(&self, row: usize, col: usize) -> String {
        let mut text = String::new();
        self.push_text_to(row, col, &mut text);
        text
    }

    pub fn cluster_count(&self) -> usize {
        self.clusters.len()
    }

    pub fn is_group_continuation(&self, row: usize, col: usize) -> bool {
        self.cell(row, col).is_group_continuation()
            && self.occupancy(row, col) != Occupancy::WideTail
    }

    pub fn unit_span(&self, row: usize, col: usize) -> UnitSpan {
        if row >= self.rows || col >= self.cols {
            return UnitSpan {
                start: col,
                end: col + 1,
            };
        }
        let mut start = col;
        while start > 0 {
            if self.occupancy(row, start) == Occupancy::WideTail
                || self.cell(row, start).is_group_continuation()
            {
                start -= 1;
            } else {
                break;
            }
        }
        let mut end = start + cell_width(&self.cell(row, start));
        while end < self.cols && self.is_group_continuation(row, end) {
            end += cell_width(&self.cell(row, end));
        }
        UnitSpan {
            start,
            end: end.min(self.cols).max(col + 1),
        }
    }

    pub fn unit_text(&self, row: usize, span: UnitSpan) -> String {
        let mut text = String::new();
        let mut col = span.start;
        while col < span.end && col < self.cols {
            if self.occupancy(row, col) != Occupancy::WideTail {
                self.push_text_to(row, col, &mut text);
            }
            col += 1;
        }
        text
    }

    pub fn blink_at(&self, row: usize, col: usize) -> Blink {
        let cell = self.cell(row, col);
        Blink {
            slow: cell.has(ATTR_SLOW_BLINK),
            fast: cell.has(ATTR_FAST_BLINK),
        }
    }

    pub fn has_blinking_cells(&self) -> bool {
        self.blinking > 0
    }

    pub fn blinking_rows(&self) -> impl Iterator<Item = usize> + '_ {
        self.blinking_rows
            .iter()
            .enumerate()
            .filter(|(_, count)| **count > 0)
            .map(|(row, _)| row)
    }

    pub fn link_at(&self, row: usize, col: usize) -> Option<&str> {
        let id = self.cell(row, col).link;
        if id == LINK_NONE {
            return None;
        }
        self.links.get(&id).map(String::as_str)
    }

    pub fn link_id_at(&self, row: usize, col: usize) -> u8 {
        self.cell(row, col).link
    }

    pub fn apply(&mut self, frame: &FrameView<'_>) -> Result<Applied, ApplyError> {
        let header = frame.header();
        let (cols, rows) = (header.cols as usize, header.rows as usize);
        if cols == 0 || rows == 0 {
            return Err(ApplyError::DegenerateViewport {
                found: (rows, cols),
            });
        }
        let reshaped = cols != self.cols || rows != self.rows;
        if reshaped && !header.full_repaint() {
            return Err(ApplyError::ReshapeWithoutFullRepaint {
                expected: (self.rows, self.cols),
                found: (rows, cols),
            });
        }
        if reshaped {
            self.cols = cols;
            self.rows = rows;
            self.cells = vec![WireCell::BLANK; cols * rows];
            self.blinking_rows = vec![0; rows];
            self.blinking = 0;
            self.clusters.clear();
        } else if header.full_repaint() || header.clear() {
            self.cells
                .iter_mut()
                .for_each(|cell| *cell = WireCell::BLANK);
            self.blinking_rows.iter_mut().for_each(|count| *count = 0);
            self.blinking = 0;
            self.clusters.clear();
        }
        if header.full_repaint() {
            self.links.clear();
        }
        for entry in frame.links().entries {
            self.links.insert(entry.id, entry.uri);
        }

        let mut written = Vec::with_capacity(header.row_record_count as usize);
        if !reshaped && !header.full_repaint() && !header.clear() {
            for shift in frame.shifts().entries {
                if !shift.fits(self.cols, self.rows) {
                    continue;
                }
                shift.apply(&mut self.cells, self.cols, self.rows);
                if !self.clusters.is_empty() {
                    let cols = self.cols;
                    self.clusters = std::mem::take(&mut self.clusters)
                        .into_iter()
                        .filter_map(|(at, text)| match shift.moved_position(at, cols) {
                            None => Some((at, text)),
                            Some(moved) => moved.map(|at| (at, text)),
                        })
                        .collect();
                }
                let last = shift.x as usize + shift.cols as usize;
                for line in shift.y as usize..shift.y as usize + shift.rows as usize {
                    written.push(Span::new(line, shift.x as usize, last.saturating_sub(1)));
                }
            }
        }
        for index in 0..header.row_record_count as usize {
            if let Some(record) = frame.record(index) {
                let last = record.x0 as usize + record.len as usize;
                written.push(Span::new(
                    record.y as usize,
                    record.x0 as usize,
                    last.saturating_sub(1),
                ));
            }
        }
        frame.apply(&mut self.cells, self.cols, self.rows);
        self.resolve_clusters(frame);
        let before = self.cursor;
        self.cursor = frame.cursor();
        let cursor_rows = (before != self.cursor).then(|| {
            (
                (before.y as usize).min(self.rows.saturating_sub(1)),
                (self.cursor.y as usize).min(self.rows.saturating_sub(1)),
            )
        });
        for span in &written {
            self.recount_blinking(span.line);
        }

        Ok(Applied {
            seq: header.seq,
            cleared: header.clear() || header.full_repaint(),
            reshaped,
            written,
            cursor_rows,
        })
    }

    fn resolve_clusters(&mut self, frame: &FrameView<'_>) {
        let header = frame.header();
        let record = frame.clusters();
        let mut resolved: HashMap<u16, Option<Arc<str>>> = HashMap::new();
        for index in 0..header.row_record_count as usize {
            let Some(row) = frame.record(index) else {
                continue;
            };
            let start = row.y as usize * self.cols + row.x0 as usize;
            let end = start + row.len as usize;
            if end > self.cells.len() {
                continue;
            }
            for at in start..end {
                if !self.clusters.is_empty() {
                    self.clusters.remove(&at);
                }
                let cell = &mut self.cells[at];
                if char::from_u32(cell.ch).is_some() {
                    continue;
                }
                let text = match cell.cluster_id() {
                    Some(id) => resolved
                        .entry(id)
                        .or_insert_with(|| record.get(id).map(Arc::from))
                        .clone(),
                    None => None,
                };
                match text {
                    Some(text) => {
                        let mut characters = text.chars();
                        cell.ch = characters.next().unwrap_or(' ') as u32;
                        if characters.next().is_some() {
                            self.clusters.insert(at, text);
                        }
                    },
                    None => cell.ch = ' ' as u32,
                }
            }
        }
    }

    fn recount_blinking(&mut self, row: usize) {
        if row >= self.rows {
            return;
        }
        let counted = self.cells[row * self.cols..(row + 1) * self.cols]
            .iter()
            .filter(|cell| cell.has(ATTR_SLOW_BLINK) || cell.has(ATTR_FAST_BLINK))
            .count();
        self.blinking = self.blinking + counted - self.blinking_rows[row];
        self.blinking_rows[row] = counted;
    }
}

#[cfg(test)]
#[path = "unit/screen_buffer_tests.rs"]
mod tests;
