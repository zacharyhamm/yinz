use unicode_width::UnicodeWidthStr;
use zellij_tile::prelude::Text;

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
    pub fn styled(&self) -> Text {
        let text = Text::new(&self.text).unbold_range(..);
        if self.focused {
            text.selected()
        } else if self.active {
            text.color_range(2, ..)
        } else {
            text.dim_range(..)
        }
    }
}

/// The framing shared by the hocus tab bars.
pub fn label(text: &str) -> String {
    format!(" {text} │")
}

/// Lay out a small, fixed tab bar. If padded labels do not fit, each visible
/// tab uses one column so narrow panes retain as many targets as possible.
pub fn cells<T>(
    tabs: impl IntoIterator<Item = (String, bool, bool, T)>,
    width: usize,
) -> Vec<TabCell<T>> {
    let tabs: Vec<_> = tabs.into_iter().collect();
    let labels: Vec<_> = tabs.iter().map(|tab| label(&tab.0)).collect();
    let compact = labels.iter().map(|label| label.width()).sum::<usize>() > width;
    let mut x = 0;
    tabs.into_iter()
        .zip(labels)
        .filter_map(|((name, active, focused, target), label)| {
            let text = if compact {
                name.chars().next()?.to_string()
            } else {
                label
            };
            let cell_width = text.width();
            if x + cell_width > width {
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
                width: 7,
                text: " VMs! │".to_owned(),
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
}
