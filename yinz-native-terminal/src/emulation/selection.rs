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

/// Local selection ownership across asynchronous server resize repaints.
/// The wire format has no stable line identity or soft-wrap metadata. Copy keeps
/// its original bytes; highlights move only on a unique, unit-aligned match.
#[derive(Default)]
pub(crate) struct LocalSelection {
    range: Option<Selection>,
    captured: Option<CapturedSelection>,
    pending: Option<(usize, usize)>,
}

struct CapturedSelection {
    text: String,
    pattern: Vec<String>,
    before: usize,
    selected: usize,
    reverse: bool,
    panes: Vec<yinz_types::structured_render::PaneRect>,
    pane_index: Option<usize>,
    mappable: bool,
}

#[derive(Clone, Copy)]
struct Region {
    row: usize,
    col: usize,
    rows: usize,
    cols: usize,
}

impl Region {
    fn of(
        state: &crate::emulation::terminal::TerminalState,
        pane_index: Option<usize>,
    ) -> Option<Self> {
        let size = state.size();
        let region = if let Some(index) = pane_index {
            let pane = state.geometry().panes.get(index)?;
            if pane.plugin_ui() || pane.popup() || pane.floating() || pane.under_floating() {
                return None;
            }
            Self {
                row: pane.content_y() as usize,
                col: pane.content_x() as usize,
                rows: pane.content_rows() as usize,
                cols: pane.content_cols() as usize,
            }
        } else {
            if !state.geometry().panes.is_empty() {
                return None;
            }
            Self {
                row: 0,
                col: 0,
                rows: size.rows,
                cols: size.cols,
            }
        };
        if region.rows == 0
            || region.cols == 0
            || region.row + region.rows > size.rows
            || region.col + region.cols > size.cols
        {
            return None;
        }
        for row in region.row..region.row + region.rows {
            if state.screen().unit_span(row, region.col).start != region.col
                || state
                    .screen()
                    .unit_span(row, region.col + region.cols - 1)
                    .end
                    != region.col + region.cols
            {
                return None;
            }
        }
        Some(region)
    }

    fn contains(self, point: (usize, usize)) -> bool {
        (self.row..self.row + self.rows).contains(&point.0)
            && (self.col..self.col + self.cols).contains(&point.1)
    }

    fn units(self, buffer: &ScreenBuffer) -> impl Iterator<Item = (String, Selection)> + '_ {
        let (mut row, mut col) = (self.row, self.col);
        std::iter::from_fn(move || {
            if row >= self.row + self.rows {
                return None;
            }
            let span = buffer.unit_span(row, col);
            // Units cut by a pane boundary cannot prove a mapping.
            if span.start < self.col || span.end > self.col + self.cols {
                return None;
            }
            let unit = (
                buffer.unit_text(row, span),
                Selection::at(row, span.start).extended_to(row, span.end - 1),
            );
            col = span.end;
            if col == self.col + self.cols {
                row += 1;
                col = self.col;
            }
            Some(unit)
        })
    }
}

impl LocalSelection {
    pub(crate) fn range(&self) -> Option<Selection> {
        self.range
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn start(&mut self, row: usize, col: usize) {
        self.clear();
        self.range = Some(Selection::at(row, col));
    }

    pub(crate) fn extend(&mut self, row: usize, col: usize) {
        // Pointer motion after a resize must not revive an invalid old anchor.
        if self.captured.is_none() {
            self.range = self.range.map(|range| range.extended_to(row, col));
        }
    }

    pub(crate) fn text(&self, buffer: &ScreenBuffer) -> Option<String> {
        self.captured
            .as_ref()
            .map(|capture| capture.text.clone())
            .or_else(|| self.range.map(|range| range.text(buffer)))
    }

    pub(crate) fn resizing(
        &mut self,
        state: &crate::emulation::terminal::TerminalState,
        rows: usize,
        cols: usize,
    ) {
        if self.pending.is_none() && (rows, cols) == (state.size().rows, state.size().cols) {
            return;
        }
        if self.captured.is_none()
            && let Some(range) = self.range
            && !range.is_empty()
        {
            self.captured = Some(CapturedSelection::new(state, range));
        }
        if self.captured.is_some() {
            self.pending = Some((rows, cols));
        }
    }

    pub(crate) fn apply_frame(
        &mut self,
        state: &mut crate::emulation::terminal::TerminalState,
        frame: &[u8],
    ) -> Result<crate::emulation::terminal::FrameApplied, crate::emulation::terminal::FrameError>
    {
        let old_size = state.size();
        let applied = state.apply_frame(frame)?;
        // Failed frames never change selection ownership. Old-grid frames can
        // arrive before the resize repaint; keep the original capture intact.
        let size = (state.size().rows, state.size().cols);
        if self.captured.is_some()
            && (size != (old_size.rows, old_size.cols) || self.pending == Some(size))
        {
            self.range = self
                .captured
                .as_ref()
                .and_then(|capture| capture.locate(state));
            if self.pending == Some(size) {
                self.pending = None;
            }
        }
        Ok(applied)
    }
}

impl CapturedSelection {
    fn new(state: &crate::emulation::terminal::TerminalState, range: Selection) -> Self {
        const CONTEXT: usize = 8;
        let mut capture = Self {
            text: range.text(state.screen()),
            pattern: Vec::new(),
            before: 0,
            selected: 0,
            reverse: range.anchor > range.head,
            panes: state.geometry().panes.clone(),
            pane_index: state.geometry().panes.iter().position(|pane| {
                pane.content_contains(range.anchor.1 as u16, range.anchor.0 as u16)
                    && pane.content_contains(range.head.1 as u16, range.head.0 as u16)
            }),
            mappable: false,
        };
        let Some(region) = Region::of(state, capture.pane_index) else {
            return capture;
        };
        let (start, end) = range.ordered();
        if !region.contains(start)
            || !region.contains(end)
            || (start.0 != end.0 && region.cols != state.size().cols)
        {
            return capture;
        }
        let mut before = std::collections::VecDeque::new();
        let mut after = 0;
        let mut reached_end = false;
        for (text, unit) in region.units(state.screen()) {
            if unit.head < start {
                before.push_back(text);
                if before.len() > CONTEXT {
                    before.pop_front();
                }
            } else if unit.anchor <= end {
                if capture.selected == 0 {
                    capture.before = before.len();
                    capture.pattern.extend(before.drain(..));
                }
                reached_end |= unit.head >= end;
                capture.selected += 1;
                capture.pattern.push(text);
            } else {
                capture.pattern.push(text);
                after += 1;
                if after == CONTEXT {
                    break;
                }
            }
        }
        capture.mappable = capture.selected > 0 && reached_end;
        capture
    }

    fn locate(&self, state: &crate::emulation::terminal::TerminalState) -> Option<Selection> {
        if !self.mappable {
            return None;
        }
        let region = Region::of(state, self.pane_index)?;
        let panes = &state.geometry().panes;
        if self.panes.len() != panes.len() {
            return None;
        }
        let signature = |pane: &yinz_types::structured_render::PaneRect| {
            (pane.flags, pane.top, pane.bottom, pane.left, pane.right)
        };
        if let Some(index) = self.pane_index {
            // No wire pane IDs: require a unique structural role as well as
            // unchanged topology, never choose among indistinguishable panes.
            let role = signature(&self.panes[index]);
            if self
                .panes
                .iter()
                .filter(|pane| signature(pane) == role)
                .count()
                != 1
                || panes.iter().filter(|pane| signature(pane) == role).count() != 1
            {
                return None;
            }
        }
        for (old, new) in self.panes.iter().zip(panes) {
            if signature(old) != signature(new) {
                return None;
            }
            for (other_old, other_new) in self.panes.iter().zip(panes) {
                if (old.x.cmp(&other_old.x), old.y.cmp(&other_old.y))
                    != (new.x.cmp(&other_new.x), new.y.cmp(&other_new.y))
                {
                    return None;
                }
            }
        }
        // KMP makes ambiguous/repetitive viewports linear in units + pattern,
        // with only pattern-sized position history and eight context units.
        let pattern = &self.pattern;
        let mut prefix = vec![0; pattern.len()];
        for index in 1..pattern.len() {
            let mut matched = prefix[index - 1];
            while matched > 0 && pattern[index] != pattern[matched] {
                matched = prefix[matched - 1];
            }
            if pattern[index] == pattern[matched] {
                matched += 1;
            }
            prefix[index] = matched;
        }
        let mut positions = std::collections::VecDeque::new();
        let mut matched = 0;
        let mut found = None;
        for (text, position) in region.units(state.screen()) {
            positions.push_back(position);
            if positions.len() > pattern.len() {
                positions.pop_front();
            }
            while matched > 0 && text != pattern[matched] {
                matched = prefix[matched - 1];
            }
            if text == pattern[matched] {
                matched += 1;
            }
            if matched == pattern.len() {
                if found.is_some() {
                    return None;
                }
                let start = positions[self.before].anchor;
                let end = positions[self.before + self.selected - 1].head;
                // A viewport-coordinate highlight cannot represent pane-local
                // wrapping without painting intervening chrome/other panes.
                if start.0 != end.0 && region.cols != state.size().cols {
                    return None;
                }
                found = Some(if self.reverse {
                    Selection {
                        anchor: end,
                        head: start,
                    }
                } else {
                    Selection {
                        anchor: start,
                        head: end,
                    }
                });
                matched = prefix[matched - 1];
            }
        }
        found
    }
}

#[cfg(test)]
#[path = "unit/selection_tests.rs"]
mod tests;
