use crate::emulation::screen_buffer::ScreenBuffer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: (usize, usize),
    pub head: (usize, usize),
}

impl Selection {
    pub fn at(row: usize, col: usize) -> Self {
        Self {
            anchor: (row, col),
            head: (row, col),
        }
    }

    pub fn extended_to(self, row: usize, col: usize) -> Self {
        Self {
            head: (row, col),
            ..self
        }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    fn ordered(&self) -> ((usize, usize), (usize, usize)) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    pub fn rows(&self) -> std::ops::RangeInclusive<usize> {
        let (start, end) = self.ordered();
        start.0..=end.0
    }

    pub fn covers(&self, row: usize, col: usize) -> bool {
        if self.is_empty() {
            return false;
        }
        let (start, end) = self.ordered();
        (start..=end).contains(&(row, col))
    }

    pub fn text(&self, buffer: &ScreenBuffer) -> String {
        if self.is_empty() {
            return String::new();
        }
        let size = buffer.size();
        let (start, end) = self.ordered();
        let last_row = end.0.min(size.rows.saturating_sub(1));
        let mut lines = Vec::new();
        for row in start.0..=last_row {
            let first = if row == start.0 { start.1 } else { 0 };
            let last = if row == end.0 {
                end.1.min(size.cols - 1)
            } else {
                size.cols - 1
            };
            let mut line = String::new();
            let mut col = first;
            if col < size.cols {
                col = buffer.unit_span(row, col).start;
            }
            while col <= last && col < size.cols {
                let span = buffer.unit_span(row, col);
                line.push_str(&buffer.unit_text(row, span));
                col = span.end.max(col + 1);
            }
            lines.push(line.trim_end().to_owned());
        }
        lines.join("\n")
    }
}

#[cfg(test)]
#[path = "unit/selection_tests.rs"]
mod tests;
