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
                Some(LayoutError::InsufficientStackSpace),
                "{members} members, expanded {expanded:?}, {} rows",
                rows - 1
            );
        }
    }
}

#[test]
fn flexible_panes_enforce_both_minimum_dimensions_after_splitting() {
    for direction in [SplitDirection::Horizontal, SplitDirection::Vertical] {
        let area = |extent| {
            PaneGeom::from(&match direction {
                SplitDirection::Horizontal => Size {
                    rows: extent,
                    cols: MIN_TERMINAL_WIDTH,
                },
                SplitDirection::Vertical => Size {
                    rows: MIN_TERMINAL_HEIGHT,
                    cols: extent,
                },
            })
        };
        let minimum = match direction {
            SplitDirection::Horizontal => MIN_TERMINAL_HEIGHT,
            SplitDirection::Vertical => MIN_TERMINAL_WIDTH,
        };
        let mut layout = TiledPaneLayout {
            children_split_direction: direction,
            children: vec![TiledPaneLayout::default(); 2],
            ..Default::default()
        };
        assert!(layout
            .position_panes_in_space(&area(2 * minimum), None, false, false)
            .is_ok());
        assert_eq!(
            layout
                .position_panes_in_space(&area(2 * minimum - 1), None, false, false)
                .unwrap_err(),
            LayoutError::InsufficientSplitSpace,
        );

        // Budgeting alone cannot catch a small percentage; validate the rounded leaves too.
        layout.children[0].split_size = Some(SplitSize::Percent(10));
        assert_eq!(
            layout
                .position_panes_in_space(&area(4 * minimum), None, false, false)
                .unwrap_err(),
            LayoutError::UndersizedPane,
        );
        assert!(layout
            .position_panes_in_space(&area(4 * minimum), None, true, false)
            .is_ok());

        // An explicitly fixed bar can be smaller than the flexible minimum.
        layout.children[0].split_size = Some(SplitSize::Fixed(1));
        assert!(layout
            .position_panes_in_space(&area(minimum + 1), None, false, false)
            .is_ok());
        layout.children[0].split_size = Some(SplitSize::Fixed(0));
        assert_eq!(
            layout
                .position_panes_in_space(&area(minimum + 1), None, false, false)
                .unwrap_err(),
            LayoutError::UndersizedPane,
        );

        layout.children[0] = TiledPaneLayout {
            children_split_direction: direction,
            children: vec![TiledPaneLayout::default(); 2],
            ..Default::default()
        };
        assert_eq!(
            layout
                .position_panes_in_space(&area(3 * minimum), None, false, false)
                .unwrap_err(),
            LayoutError::InsufficientSplitSpace,
        );
        assert!(layout
            .position_panes_in_space(&area(4 * minimum), None, false, false)
            .is_ok());
    }
    for size in [Size { rows: 4, cols: 80 }, Size { rows: 24, cols: 4 }] {
        assert_eq!(
            TiledPaneLayout::default()
                .position_panes_in_space(&PaneGeom::from(&size), None, false, false)
                .unwrap_err(),
            LayoutError::UndersizedPane,
        );
    }
    assert_eq!(
        stack_of(2, None)
            .position_panes_in_space(
                &PaneGeom::from(&Size { rows: 6, cols: 4 }),
                None,
                false,
                false
            )
            .unwrap_err(),
        LayoutError::UndersizedPane,
    );
}
