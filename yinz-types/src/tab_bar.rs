//! Shared terminal-cell geometry for native and plugin tab bars.
use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthChar;

/// Session-local group identity; IDs also preserve creation order across resurrection.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TabGroupInfo {
    pub id: u32,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabTarget {
    Tab(usize),
    Reveal(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TabCell<T> {
    pub x: usize,
    pub width: usize,
    pub text: String,
    pub active: bool,
    pub focused: bool,
    pub target: T,
}

// Both the native renderer and plugin Text renderer measure individual terminal characters.
fn cell_width(text: &str) -> usize {
    text.chars().map(|ch| ch.width().unwrap_or(0)).sum()
}

/// Agent's window: grow rightward then leftward around `keep`, reserving overflow arrows.
pub fn overflow(
    labels: &[String],
    width: usize,
    active: usize,
    focus: Option<usize>,
    keep: usize,
) -> Vec<TabCell<TabTarget>> {
    if labels.is_empty() || width == 0 {
        return vec![];
    }
    let keep = keep.min(labels.len() - 1);
    let widths: Vec<usize> = labels.iter().map(|label| cell_width(label)).collect();
    if widths.iter().sum::<usize>() <= width {
        return lay_out(labels, 0, labels.len(), width, active, focus);
    }

    // Grow the window outward from `keep` — rightward first, so the tabs after
    // the one you are on are what you see — until nothing more fits, leaving a
    // two columns for each arrow and divider the window will need.
    let mut start = keep;
    let mut end = keep + 1;
    let mut used = widths[keep];
    loop {
        // Counted even for an edge the next step would reach, which only ever
        // leaves a column spare.
        let arrows = usize::from(start > 0) + usize::from(end < labels.len());
        let room = width.saturating_sub(arrows * 2);
        if end < labels.len() && used + widths[end] <= room {
            used += widths[end];
            end += 1;
        } else if start > 0 && used + widths[start - 1] <= room {
            start -= 1;
            used += widths[start];
        } else {
            break;
        }
    }
    lay_out(labels, start, end, width, active, focus)
}

/// Print-ready cells for the tabs in `start..end`, with an arrow on either
/// side that has tabs hidden past it.
fn lay_out(
    labels: &[String],
    start: usize,
    end: usize,
    width: usize,
    active: usize,
    focus: Option<usize>,
) -> Vec<TabCell<TabTarget>> {
    let arrow_width = (usize::from(start > 0) + usize::from(end < labels.len())) * 2;
    // At least one column belongs to the label, even when neither arrow fits.
    let arrows = width > arrow_width;
    let mut cells = Vec::new();
    let mut x = 0;
    let mut push = |text: String, active: bool, focused: bool, target: TabTarget| {
        let width = cell_width(&text);
        cells.push(TabCell {
            x,
            width,
            text,
            active,
            focused,
            target,
        });
        x += width;
    };
    if arrows && start > 0 {
        push("←│".into(), false, false, TabTarget::Reveal(start - 1));
    }
    for (tab, label) in labels.iter().enumerate().take(end).skip(start) {
        let mut text = label.clone();
        if end == start + 1 {
            let room = width.saturating_sub(if arrows { arrow_width } else { 0 });
            let mut used = 0;
            text = label
                .chars()
                .take_while(|ch| {
                    used += ch.width().unwrap_or(0);
                    used <= room
                })
                .collect();
            if text.trim().is_empty() {
                text = "…".into();
            }
        }
        push(text, tab == active, focus == Some(tab), TabTarget::Tab(tab));
    }
    if arrows && end < labels.len() {
        push("→│".into(), false, false, TabTarget::Reveal(end));
    }
    cells
}

#[cfg(test)]
#[path = "unit/tab_bar.rs"]
mod tests;
