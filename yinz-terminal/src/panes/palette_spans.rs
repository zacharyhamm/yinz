//! Sparse, coalesced ownership in terminal columns (not character indices).
use crate::agent_theme::SourcePalette;
use std::{ops::Range, rc::Rc};

#[derive(Clone)]
struct Span {
    range: Range<usize>,
    palette: Rc<SourcePalette>,
}

#[derive(Clone, Default)]
pub(super) struct PaletteSpans(Vec<Span>);

impl PaletteSpans {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn at(&self, column: usize) -> Option<&Rc<SourcePalette>> {
        self.0
            .iter()
            .find(|span| span.range.contains(&column))
            .map(|s| &s.palette)
    }

    pub fn set(&mut self, range: Range<usize>, palette: Option<&Rc<SourcePalette>>) {
        if range.is_empty() {
            return;
        }
        let palette = palette.filter(|p| p.has_snapshot());
        if self.0.is_empty() && palette.is_none() {
            return;
        }
        if let (Some(last), Some(palette)) = (self.0.last_mut(), palette)
            && Rc::ptr_eq(&last.palette, palette)
            && range.start >= last.range.start
            && range.start <= last.range.end
        {
            last.range.end = last.range.end.max(range.end);
            return;
        }
        let mut spans = Vec::with_capacity(self.0.len() + 2);
        for span in self.0.drain(..) {
            if span.range.end <= range.start || span.range.start >= range.end {
                spans.push(span);
            } else {
                if span.range.start < range.start {
                    spans.push(Span {
                        range: span.range.start..range.start,
                        palette: span.palette.clone(),
                    });
                }
                if span.range.end > range.end {
                    spans.push(Span {
                        range: range.end..span.range.end,
                        palette: span.palette,
                    });
                }
            }
        }
        if let Some(palette) = palette.filter(|p| p.has_snapshot()) {
            spans.push(Span {
                range,
                palette: palette.clone(),
            });
        }
        spans.sort_by_key(|s| s.range.start);
        for span in spans {
            if let Some(last) = self.0.last_mut()
                && last.range.end == span.range.start
                && Rc::ptr_eq(&last.palette, &span.palette)
            {
                last.range.end = span.range.end;
                continue;
            }
            self.0.push(span);
        }
    }

    pub fn slice(&self, range: Range<usize>) -> Self {
        Self(
            self.0
                .iter()
                .filter_map(|s| {
                    let start = s.range.start.max(range.start);
                    let end = s.range.end.min(range.end);
                    (start < end).then(|| Span {
                        range: start - range.start..end - range.start,
                        palette: s.palette.clone(),
                    })
                })
                .collect(),
        )
    }

    pub fn append(&mut self, other: &Self, offset: usize) {
        self.set(offset..usize::MAX, None);
        for span in &other.0 {
            self.set(
                span.range.start.saturating_add(offset)..span.range.end.saturating_add(offset),
                Some(&span.palette),
            );
        }
    }

    pub fn insert(&mut self, at: usize, width: usize, palette: Option<&Rc<SourcePalette>>) {
        let tail = self.slice(at..usize::MAX);
        self.append(&tail, at.saturating_add(width));
        self.set(at..at.saturating_add(width), palette);
    }

    pub fn delete(&mut self, range: Range<usize>) {
        let tail = self.slice(range.end..usize::MAX);
        self.append(&tail, range.start);
    }
}
