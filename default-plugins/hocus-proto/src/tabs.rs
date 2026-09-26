use unicode_width::UnicodeWidthStr;
use zellij_tile::prelude::Text;

/// What the panel's footer says while the panel's own tab bar has the keys, on every tab.
///
/// The bar is the only keyboard route between Agents, Todos, VMs and Swamp, and the keys it
/// answers mean something else there than they do on the tab below - `Enter`, `↓`, `Tab` and
/// `Esc` step down into the list rather than acting on it - so the footer says how to use the
/// bar rather than advertising what those keys do in the list. Any other key is handed on to
/// the list and does what it always does there. Keys first, like every hint line.
pub const BAR_HINT: &str = "←/→ switch · Enter/↓ list · Tab next · Esc list";

/// One tab cell, including the geometry used to draw and click it.
#[derive(Clone, Debug, PartialEq)]
pub struct TabCell<T> {
    pub x: usize,
    pub width: usize,
    pub text: String,
    pub active: bool,
    pub focused: bool,
    pub target: T,
}

impl<T> TabCell<T> {
    /// The cell as it is painted.
    ///
    /// The active tab stays bold and the others are not, so which tab is showing does not rest
    /// on colour alone: an unfocused bar used to tell it apart only by colour 2 against dim. An
    /// error marker - the `!` [`cells`] puts at the end of a name - is painted in the error
    /// colour, since on a dimmed tab it is otherwise the easiest character on the row to miss.
    pub fn styled(&self) -> Text {
        let text = Text::new(&self.text);
        let text = match self.active {
            true => text,
            false => text.unbold_range(..),
        };
        let text = if self.focused {
            text.selected()
        } else if self.active {
            text.color_range(2, ..)
        } else {
            text.dim_range(..)
        };
        match self.marker() {
            Some(at) => text.error_color_range(at..=at),
            None => text,
        }
    }

    /// Where the error marker is, by character: a `!` ending the name, just before the
    /// separator or at the very end of the cell.
    ///
    /// Read from the text rather than carried beside it, and safe for the conversation's chat
    /// tabs, which share this type: [`label`] always leaves a space between a title and its
    /// separator, so a title that ends in `!` is never mistaken for a marker.
    fn marker(&self) -> Option<usize> {
        let name = self.text.strip_suffix('│').unwrap_or(&self.text);
        name.ends_with('!').then(|| name.chars().count() - 1)
    }
}

/// The framing shared by the hocus tab bars. [`cells`] frames a marked name the same way, with
/// its `!` where the closing space would be.
pub fn label(text: &str) -> String {
    format!(" {text} │")
}

/// Lay out a small, fixed tab bar, as legibly as `width` allows.
///
/// A name ending in `!` is an error marker (`"VMs!"`). Three tiers are tried in turn:
///
/// 1. Padded, `" Agents │ Todos │ VMs!│ Swamp │"`. The marker takes the place of the space
///    before its separator, so it costs no width: an error can never be what pushes the bar
///    into a tighter tier and so cuts away the very marker that caused it.
/// 2. Tight, `"Agents│Todos│VMs!│Swamp"`: whole names, separated but not padded, with no
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
                ("VMs!".to_owned(), false, true, 1),
            ],
            width,
        )
    }

    /// The panel's own four tabs, with a marker on whichever of VMs and Swamp is failing.
    fn panel(vms: bool, swamp: bool, width: usize) -> Vec<String> {
        let mark = |name: &str, marked: bool| match marked {
            true => format!("{name}!"),
            false => name.to_owned(),
        };
        cells(
            [
                ("Agents".to_owned(), true, false, 0),
                ("Todos".to_owned(), false, false, 1),
                (mark("VMs", vms), false, false, 2),
                (mark("Swamp", swamp), false, false, 3),
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
                text: " VMs!│".to_owned(),
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
            [(0, 1, "A", 0), (1, 1, "V", 1)]
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
        assert_eq!(plain, [" Agents │", " Todos │", " VMs │", " Swamp │"]);
        assert_eq!(
            panel(true, true, 31),
            [" Agents │", " Todos │", " VMs!│", " Swamp!│"]
        );
    }

    /// Between padded and initials, whole names: the narrowest default list half still reads.
    #[test]
    fn a_tight_tier_keeps_whole_names() {
        assert_eq!(
            panel(false, false, 30),
            ["Agents│", "Todos│", "VMs│", "Swamp"]
        );
        assert_eq!(
            panel(false, false, 22),
            ["Agents│", "Todos│", "VMs│", "Swamp"]
        );
        assert_eq!(
            panel(true, true, 24),
            ["Agents│", "Todos│", "VMs!│", "Swamp!"]
        );
        assert_eq!(
            panel(false, true, 23),
            ["Agents│", "Todos│", "VMs│", "Swamp!"]
        );
        // Below the names, initials - and the markers too while there is room for them.
        assert_eq!(panel(false, false, 21), ["A", "T", "V", "S"]);
        assert_eq!(panel(true, true, 23), ["A", "T", "V!", "S!"]);
        assert_eq!(panel(true, false, 5), ["A", "T", "V!", "S"]);
        assert_eq!(panel(true, false, 4), ["A", "T", "V", "S"]);
    }

    /// The active tab keeps its bold, and only a marker is painted in the error colour - never
    /// a `!` a chat title happens to end with.
    #[test]
    fn styling_marks_the_active_tab_and_the_error() {
        let styled = |text: &str, active: bool| {
            format!(
                "{:?}",
                TabCell {
                    x: 0,
                    width: text.width(),
                    text: text.to_owned(),
                    active,
                    focused: false,
                    target: (),
                }
                .styled()
            )
        };
        assert_eq!(
            styled(" VMs!│", true),
            format!(
                "{:?}",
                Text::new(" VMs!│")
                    .color_range(2, ..)
                    .error_color_range(4..=4)
            )
        );
        assert_eq!(
            styled("Swamp!", false),
            format!(
                "{:?}",
                Text::new("Swamp!")
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
