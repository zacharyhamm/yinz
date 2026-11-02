//! Selection of the text actually painted by any palette view.

use super::{CharacterChunk, CommandPaletteLayout, Position, Style};
use crate::output::HighlightSelection;
use crate::panes::Selection;
use crate::ui::damage::CellRect;
use yinz_types::data::HighlightLayer;

#[derive(PartialEq, Eq)]
struct Glyph {
    x: usize,
    width: usize,
    text: String,
}

#[derive(PartialEq, Eq)]
struct Snapshot {
    rect: CellRect,
    view: u64,
    rows: Vec<Vec<Glyph>>,
}

impl Snapshot {
    fn new(layout: &CommandPaletteLayout, view: u64, chunks: &[CharacterChunk]) -> Self {
        let rect = layout.rect;
        let mut rows: Vec<Vec<Glyph>> = (0..rect.rows).map(|_| vec![]).collect();
        for chunk in chunks {
            let Some(row) = chunk.y.checked_sub(rect.y).filter(|row| *row < rect.rows) else {
                continue;
            };
            let border_row = layout.border == 1 && (row == 0 || row + 1 == rect.rows);
            let mut x = chunk.x;
            for character in &chunk.terminal_characters {
                let width = character.width();
                if width == 0 {
                    if let Some(glyph) = rows[row].last_mut() {
                        glyph.text.push(character.character);
                    }
                } else if x >= rect.x + layout.border
                    && x + width <= rect.x + rect.cols.saturating_sub(layout.border)
                {
                    // Keep titles selectable, but omit the frame surrounding them.
                    let decoration =
                        border_row && matches!(character.character, '─' | '╭' | '╮' | '╰' | '╯');
                    rows[row].push(Glyph {
                        x,
                        width,
                        text: if decoration {
                            " ".into()
                        } else {
                            character.character.to_string()
                        },
                    });
                }
                x += width;
            }
        }
        Self { rect, view, rows }
    }

    fn clamp(&self, position: Position) -> Position {
        let y = position
            .line()
            .max(self.rect.y as isize)
            .min((self.rect.y + self.rect.rows.saturating_sub(1)) as isize);
        let x = position
            .column()
            .clamp(self.rect.x, self.rect.x + self.rect.cols);
        Position::new(y as i32, x as u16)
    }
}

enum Gesture {
    Pending(Position),
    Selecting,
    /// Retain capture until release after a control press or invalidation.
    Cancelled,
}

#[derive(Default)]
pub struct PaletteSelection {
    snapshot: Option<Snapshot>,
    range: Option<(Position, Position)>,
    gesture: Option<Gesture>,
}

impl PaletteSelection {
    pub fn captures_mouse(&self) -> bool {
        self.gesture.is_some()
    }

    pub fn clear(&mut self) {
        self.snapshot = None;
        self.range = None;
        if self.gesture.is_some() {
            self.gesture = Some(Gesture::Cancelled);
        }
    }

    pub fn capture_control(&mut self) {
        self.clear();
        self.gesture = Some(Gesture::Cancelled);
    }

    pub fn start(
        &mut self,
        layout: &CommandPaletteLayout,
        view: u64,
        chunks: &[CharacterChunk],
        position: Position,
    ) {
        self.snapshot = Some(Snapshot::new(layout, view, chunks));
        self.range = None;
        self.gesture = Some(Gesture::Pending(position));
    }

    /// Ignore style-only changes, including focus and hook flashes.
    pub fn reconcile(
        &mut self,
        layout: &CommandPaletteLayout,
        view: u64,
        chunks: &[CharacterChunk],
    ) {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| *snapshot != Snapshot::new(layout, view, chunks))
        {
            self.clear();
        }
    }

    pub fn extend(&mut self, position: Position) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let end = snapshot.clamp(position);
        match self.gesture {
            Some(Gesture::Pending(start)) if start != end => {
                self.range = Some((start, end));
                self.gesture = Some(Gesture::Selecting);
            },
            Some(Gesture::Selecting) => {
                if let Some((_, previous_end)) = &mut self.range {
                    *previous_end = end;
                }
            },
            _ => {},
        }
    }

    /// A pending gesture is an ordinary click; a completed drag keeps its selection.
    pub fn release(&mut self, position: Position) -> Option<Position> {
        self.extend(position);
        match self.gesture.take() {
            Some(Gesture::Pending(position)) => Some(position),
            _ => None,
        }
    }

    fn row_ranges(&self) -> Vec<(usize, usize, usize)> {
        let (Some(snapshot), Some((start, end))) = (&self.snapshot, self.range) else {
            return vec![];
        };
        if start == end {
            return vec![];
        }
        let key = |position: Position| (position.line(), position.column());
        let (start, end) = if key(start) <= key(end) {
            (start, end)
        } else {
            (end, start)
        };
        snapshot
            .rows
            .iter()
            .enumerate()
            .filter_map(|(row, glyphs)| {
                let y = snapshot.rect.y + row;
                if (y as isize) < start.line() || (y as isize) > end.line() {
                    return None;
                }
                let left = if y as isize == start.line() {
                    start.column()
                } else {
                    snapshot.rect.x
                };
                let right = if y as isize == end.line() {
                    end.column()
                } else {
                    snapshot.rect.x + snapshot.rect.cols
                };
                // Expand partial wide glyphs to whole glyphs for both copying and painting.
                let mut included = glyphs
                    .iter()
                    .filter(|glyph| glyph.x < right && glyph.x + glyph.width > left);
                let first = included.next()?;
                let last = included.next_back().unwrap_or(first);
                Some((y, first.x, last.x + last.width))
            })
            .collect()
    }

    pub fn selected_text(&self) -> Option<String> {
        let snapshot = self.snapshot.as_ref()?;
        let lines: Vec<_> = self
            .row_ranges()
            .into_iter()
            .map(|(y, left, right)| {
                snapshot.rows[y - snapshot.rect.y]
                    .iter()
                    .filter(|glyph| glyph.x >= left && glyph.x < right)
                    .map(|glyph| glyph.text.as_str())
                    .collect::<String>()
                    .trim_end_matches(' ')
                    .to_owned()
            })
            .collect();
        let text = lines.join("\n");
        (!text.trim().is_empty()).then_some(text)
    }

    pub fn highlight(&self, chunks: &mut [CharacterChunk], style: &Style) {
        for (y, left, right) in self.row_ranges() {
            let mut selection = Selection::default();
            selection.start(Position::new(y as i32, left as u16));
            selection.end(Position::new(y as i32, right as u16));
            for chunk in chunks.iter_mut().filter(|chunk| chunk.y == y) {
                let mut bg = style.colors.text_selected.background.into();
                let mut fg = style.colors.text_selected.base.into();
                let mut x = chunk.x;
                let already_selected = chunk.terminal_characters.iter().any(|character| {
                    let overlaps = x < right && x + character.width() > left;
                    x += character.width();
                    let background = character
                        .styles
                        .background
                        .filter(|color| *color != super::AnsiCode::Reset)
                        .unwrap_or(style.colors.text_unselected.background.into());
                    overlaps && background == bg
                });
                // Focused message/list rows already use the selection palette. Reverse
                // it there so a text selection remains distinguishable from row focus.
                if already_selected {
                    std::mem::swap(&mut bg, &mut fg);
                }
                chunk.add_selection_and_colors(
                    HighlightSelection {
                        selection,
                        bg: Some(bg),
                        fg: Some(fg),
                        bold: false,
                        italic: false,
                        underline: false,
                        layer: HighlightLayer::ActionFeedback,
                    },
                    0,
                    0,
                );
            }
        }
    }
}

#[cfg(test)]
#[path = "unit/command_palette_selection_tests.rs"]
mod tests;
