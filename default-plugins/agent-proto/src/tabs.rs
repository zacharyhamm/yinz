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
mod tests {
    use super::*;

    fn tabs(width: usize) -> Vec<TabCell<usize>> {
        cells(
            [
                ("Agents".to_owned(), true, false, 0),
                ("Log!".to_owned(), false, true, 1),
            ],
            width,
        )
    }

    /// Four sample tabs, with a marker on whichever of Log and Shell is failing.
    fn panel(log: bool, notes: bool, width: usize) -> Vec<String> {
        let mark = |name: &str, marked: bool| match marked {
            true => format!("{name}!"),
            false => name.to_owned(),
        };
        cells(
            [
                ("Agents".to_owned(), true, false, 0),
                ("Files".to_owned(), false, false, 1),
                (mark("Log", log), false, false, 2),
                (mark("Shell", notes), false, false, 3),
            ],
            width,
        )
        .into_iter()
        .map(|cell| cell.text)
        .collect()
    }

    #[test]
    fn padded_cells_carry_geometry_state_and_targets() {
        let cells = tabs(20);
        assert_eq!(
            cells[0],
            TabCell {
                x: 0,
                width: 9,
                text: " Agents │".to_owned(),
                active: true,
                focused: false,
                target: 0
            }
        );
        assert_eq!(
            cells[1],
            TabCell {
                x: 9,
                width: 6,
                text: " Log!│".to_owned(),
                active: false,
                focused: true,
                target: 1
            }
        );
    }

    #[test]
    fn narrow_cells_stay_within_the_width() {
        assert_eq!(
            tabs(2)
                .iter()
                .map(|cell| (cell.x, cell.width, cell.text.as_str(), cell.target))
                .collect::<Vec<_>>(),
            [(0, 1, "A", 0), (1, 1, "L", 1)]
        );
        assert_eq!(
            tabs(1).iter().map(|cell| cell.target).collect::<Vec<_>>(),
            [0]
        );
        assert!(tabs(0).is_empty());
    }

    /// An error costs no width in the padded tier, so it can never be what collapses the bar.
    #[test]
    fn a_marker_never_changes_the_tier() {
        let plain = panel(false, false, 31);
        assert_eq!(plain, [" Agents │", " Files │", " Log │", " Shell │"]);
        assert_eq!(
            panel(true, true, 31),
            [" Agents │", " Files │", " Log!│", " Shell!│"]
        );
    }

    /// Between padded and initials, whole names: the narrowest default list half still reads.
    #[test]
    fn a_tight_tier_keeps_whole_names() {
        assert_eq!(
            panel(false, false, 30),
            ["Agents│", "Files│", "Log│", "Shell"]
        );
        assert_eq!(
            panel(false, false, 22),
            ["Agents│", "Files│", "Log│", "Shell"]
        );
        assert_eq!(
            panel(true, true, 24),
            ["Agents│", "Files│", "Log!│", "Shell!"]
        );
        assert_eq!(
            panel(false, true, 23),
            ["Agents│", "Files│", "Log│", "Shell!"]
        );
        // Below the names, initials - and the markers too while there is room for them.
        assert_eq!(panel(false, false, 21), ["A", "F", "L", "S"]);
        assert_eq!(panel(true, true, 23), ["A", "F", "L!", "S!"]);
        assert_eq!(panel(true, false, 5), ["A", "F", "L!", "S"]);
        assert_eq!(panel(true, false, 4), ["A", "F", "L", "S"]);
    }

    /// The active tab keeps its bold, and only a marker is painted in the error colour - never
    /// a `!` a chat title happens to end with.
    #[test]
    fn styling_marks_the_active_tab_and_the_error() {
        let styled = |text: &str, active: bool| {
            format!(
                "{:?}",
                super::styled(&TabCell {
                    x: 0,
                    width: text.width(),
                    text: text.to_owned(),
                    active,
                    focused: false,
                    target: (),
                })
            )
        };
        assert_eq!(
            styled(" Log!│", true),
            format!(
                "{:?}",
                Text::new(" Log!│")
                    .color_range(2, ..)
                    .error_color_range(4..=4)
            )
        );
        assert_eq!(
            styled("Shell!", false),
            format!(
                "{:?}",
                Text::new("Shell!")
                    .unbold_range(..)
                    .dim_range(..)
                    .error_color_range(5..=5)
            )
        );
        assert_eq!(
            styled(&label("Ship it!"), false),
            format!(
                "{:?}",
                Text::new(label("Ship it!")).unbold_range(..).dim_range(..)
            )
        );
    }
}
