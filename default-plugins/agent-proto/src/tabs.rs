use unicode_width::UnicodeWidthStr;
use yinz_tile::prelude::Text;

pub const STOP_TABS: &str = "management_tabs";

/// Navigation while the native panel tab bar has the keyboard.
pub const BAR_HINT: &str = "←/→ choose · Enter open · Tab/Esc list";

pub const BAR_KEYS: &[(&str, &str)] = &[
    ("←/→", "highlight a tab"),
    ("Enter", "activate the highlighted tab"),
    ("Tab/Esc", "return to the active tab's list"),
    ("Shift+Tab", "to the active view's last region"),
    ("other keys/paste", "ignored while the bar has focus"),
    ("click a tab/arrow", "activate a tab/browse hidden tabs"),
];

pub use yinz_types::tab_bar::TabCell;

/// The cell as it is painted.
///
/// The active tab stays bold and the others are not, so which tab is showing does not rest
/// on colour alone: an unfocused bar used to tell it apart only by colour 2 against dim. An
/// error marker - the `!` [`cells`] puts at the end of a name - is painted in the error
/// colour, since on a dimmed tab it is otherwise the easiest character on the row to miss.
pub fn styled<T>(cell: &TabCell<T>) -> Text {
    let text = Text::new(&cell.text);
    let text = match cell.active {
        true => text,
        false => text.unbold_range(..),
    };
    let text = if cell.focused {
        text.selected()
    } else if cell.active {
        text.color_range(2, ..)
    } else {
        text.dim_range(..)
    };
    let name = cell.text.strip_suffix('│').unwrap_or(&cell.text);
    if name.ends_with('!') {
        let at = name.chars().count() - 1;
        text.error_color_range(at..=at)
    } else {
        text
    }
}

/// The framing shared by the agent tab bars. [`cells`] frames a marked name the same way, with
/// its `!` where the closing space would be.
pub fn label(text: &str) -> String {
    format!(" {text} │")
}

/// Lay out a small, fixed tab bar, as legibly as `width` allows.
///
/// A name ending in `!` is an error marker (`"Log!"`). Three tiers are tried in turn:
///
/// 1. Padded, `" Agents │ Files │ Log!│ Shell │"`. The marker takes the place of the space
///    before its separator, so it costs no width: an error can never be what pushes the bar
///    into a tighter tier and so cuts away the very marker that caused it.
/// 2. Tight, `"Agents│Files│Log!│Shell"`: whole names, separated but not padded, with no
///    separator after the last. 22 columns without markers, so every name survives in the
///    narrowest half the default panel geometry gives the list.
/// 3. Initials, one column each so very narrow panes keep as many targets as possible, and
///    `"V!"` for a marked name while the whole row still fits. Where it does not, the markers
///    go before any target does.
///
/// A cell that does not fit even as an initial is dropped.
pub fn cells<T>(
    tabs: impl IntoIterator<Item = (String, bool, bool, T)>,
    width: usize,
) -> Vec<TabCell<T>> {
    let tabs: Vec<_> = tabs.into_iter().collect();
    let names: Vec<(&str, bool)> = tabs
        .iter()
        .map(|tab| match tab.0.strip_suffix('!') {
            Some(base) => (base, true),
            None => (tab.0.as_str(), false),
        })
        .collect();
    let last = names.len().saturating_sub(1);
    let padded: Vec<String> = names
        .iter()
        .map(|(base, marked)| format!(" {base}{}│", if *marked { "!" } else { " " }))
        .collect();
    let tight: Vec<String> = names
        .iter()
        .enumerate()
        .map(|(index, (base, marked))| {
            format!(
                "{base}{}{}",
                if *marked { "!" } else { "" },
                if index < last { "│" } else { "" }
            )
        })
        .collect();
    let initial = |marked: bool| {
        names
            .iter()
            .map(|(base, is_marked)| {
                let mut text: String = base.chars().take(1).collect();
                if marked && *is_marked && !text.is_empty() {
                    text.push('!');
                }
                text
            })
            .collect::<Vec<_>>()
    };
    let fits = |texts: &[String]| texts.iter().map(|text| text.width()).sum::<usize>() <= width;
    let texts = [padded, tight, initial(true)]
        .into_iter()
        .find(|texts| fits(texts))
        .unwrap_or_else(|| initial(false));
    let mut x = 0;
    tabs.into_iter()
        .zip(texts)
        .filter_map(|((_, active, focused, target), text)| {
            let cell_width = text.width();
            if text.is_empty() || x + cell_width > width {
                return None;
            }
            let cell = TabCell {
                x,
                width: cell_width,
                text,
                active,
                focused,
                target,
            };
            x += cell_width;
            Some(cell)
        })
        .collect()
}

#[cfg(test)]
#[path = "unit/tabs.rs"]
mod tests;
