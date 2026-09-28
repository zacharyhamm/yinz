//! Tests for placing a `TiledPaneLayout` in a space.
use super::*;
use crate::pane_size::{Size, MIN_TERMINAL_HEIGHT};

fn stack_of(members: usize, expanded: Option<usize>) -> TiledPaneLayout {
    let mut children = vec![TiledPaneLayout::default(); members];
    if let Some(expanded) = expanded {
        children[expanded].is_expanded_in_stack = true;
    }
    TiledPaneLayout {
        children_split_direction: SplitDirection::Horizontal,
        children_are_stacked: true,
        children,
        ..Default::default()
    }
}

fn space(rows: usize) -> PaneGeom {
    PaneGeom::from(&Size { rows, cols: 80 })
}

#[test]
fn stack_fits_exactly_when_its_expanded_pane_gets_the_minimum_height() {
    for members in 1..=6 {
        for expanded in [None, Some(0), Some(members / 2)] {
            let layout = stack_of(members, expanded);
            // One title row for each collapsed member, and the minimum for the expanded one.
            let rows = members - 1 + MIN_TERMINAL_HEIGHT;
            let positions = layout
                .position_panes_in_space(&space(rows), None, false, false)
                .unwrap_or_else(|error| {
                    panic!("{members} members, expanded {expanded:?}, {rows} rows: {error}")
                });

            assert_eq!(positions.len(), members);
            let expanded_index = expanded.unwrap_or(members - 1);
            let mut next_row = 0;
            for (index, (_, geom)) in positions.iter().enumerate() {
                let expected_rows = if index == expanded_index {
                    MIN_TERMINAL_HEIGHT
                } else {
                    1
                };
                assert!(geom.is_stacked(), "member {index} of {members}");
                assert_eq!(geom.y, next_row, "member {index} of {members}");
                assert_eq!(
                    geom.rows.as_usize(),
                    expected_rows,
                    "member {index} of {members}, expanded {expanded:?}"
                );
                next_row += geom.rows.as_usize();
            }
            assert_eq!(next_row, rows);

            assert_eq!(
                layout
                    .position_panes_in_space(&space(rows - 1), None, false, false)
                    .err(),
                Some("Not enough room for stacked panes in this layout"),
                "{members} members, expanded {expanded:?}, {} rows",
                rows - 1
            );
        }
    }
}
