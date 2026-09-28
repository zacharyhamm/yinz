//! Pane tabs as the pane layer sees them: the tab bar's cells, which a plugin pane draws and
//! `Tab` hit-tests, and the opaque carrier that holds a pane's pane tabs while the pane is out of
//! a `Tab`.

use yinz_types::pane_tabs::PaneTabsInfo;

/// A pane's pane tabs and their hidden children, set on the pane when a `Tab` extracts it and
/// taken back by the `Tab` that next adds or parks it, which after a float or embed toggle is the
/// same one. `Tab` decides the contents; the pane layer only holds them, so it never names `Tab`'s
/// bookkeeping.
pub struct PaneTabsTransfer(Box<dyn std::any::Any>);

impl PaneTabsTransfer {
    pub fn new<T: 'static>(contents: T) -> Self {
        PaneTabsTransfer(Box::new(contents))
    }

    /// `None` when the transfer holds something other than a `T`.
    pub fn into_contents<T: 'static>(self) -> Option<T> {
        self.0.downcast().ok().map(|contents| *contents)
    }
}

/// Cell hit targets and rendering share one clipped window; the active label always fits first.
pub fn bar_cells(info: &PaneTabsInfo, width: usize) -> Vec<(char, usize)> {
    use unicode_width::UnicodeWidthChar;
    if width == 1 {
        return vec![('•', info.active)];
    }
    let labels: Vec<Vec<(char, usize)>> = info
        .tabs
        .iter()
        .enumerate()
        .map(|(index, slot)| {
            let label =
                yinz_types::text::clean_string_from_control_and_linebreak(&slot.descriptor.label);
            format!(
                " {}{}{} ",
                if index == info.active { "[" } else { "" },
                label,
                if index == info.active { "]" } else { "" }
            )
            .chars()
            .map(|ch| (ch, index))
            .collect()
        })
        .collect();
    let length = |label: &Vec<(char, usize)>| {
        label
            .iter()
            .map(|(ch, _)| ch.width().unwrap_or(0))
            .sum::<usize>()
    };
    let mut start = 0;
    while start < info.active
        && labels[start..=info.active]
            .iter()
            .map(length)
            .sum::<usize>()
            > width
    {
        start += 1;
    }
    let mut result = Vec::new();
    for &(ch, index) in labels[start..].iter().flatten() {
        let cells = ch.width().unwrap_or(0);
        if result.len() + cells > width {
            break;
        }
        if cells == 0 {
            continue;
        }
        result.push((ch, index));
        for _ in 1..cells {
            result.push(('\0', index));
        }
    }
    result
}
