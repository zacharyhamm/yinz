//! Tests for placing a `TiledPaneLayout` in a space.
use super::*;
use crate::pane_size::{Constraint, Size, MIN_TERMINAL_HEIGHT};

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

/// Leaves along `direction` with their extents, checking that they tile `parent` exactly.
fn assert_tiles(geoms: &[PaneGeom], direction: SplitDirection, start: usize, extent: usize) {
    let mut position = start;
    for geom in geoms {
        let (at, size) = match direction {
            SplitDirection::Horizontal => (geom.y, geom.rows.as_usize()),
            SplitDirection::Vertical => (geom.x, geom.cols.as_usize()),
        };
        assert_eq!(at, position, "{geoms:#?}");
        position += size;
    }
    assert_eq!(position, start + extent, "{geoms:#?}");
}

#[test]
fn fixed_height_stack_gives_its_expanded_member_the_remaining_rows() {
    for expanded in [None, Some(0)] {
        let mut stack = stack_of(3, expanded);
        stack.split_size = Some(SplitSize::Fixed(10));
        let layout = TiledPaneLayout {
            children_split_direction: SplitDirection::Horizontal,
            children: vec![stack, TiledPaneLayout::default()],
            ..Default::default()
        };
        for ignore_percent_split_sizes in [false, true] {
            let positions = layout
                .position_panes_in_space(&space(30), None, ignore_percent_split_sizes, false)
                .unwrap();
            let geoms: Vec<PaneGeom> = positions.iter().map(|(_, geom)| *geom).collect();
            assert_eq!(geoms.len(), 4, "{geoms:#?}");
            let (stacked, rest): (Vec<_>, Vec<_>) =
                geoms.iter().partition(|geom| geom.is_stacked());
            assert_tiles(&stacked, SplitDirection::Horizontal, 0, 10);
            assert_eq!(rest.len(), 1);
            assert_eq!((rest[0].y, rest[0].rows.as_usize()), (10, 20));

            let expanded_index = expanded.unwrap_or(2);
            for (index, geom) in stacked.iter().enumerate() {
                if index == expanded_index {
                    // The expanded member shows 8 rows under the whole stack's fixed height.
                    assert_eq!(geom.rows.constraint, Constraint::Fixed(10), "{geom:#?}");
                    assert_eq!(geom.rows.as_usize(), 8);
                    assert!(geom.is_expanded_in_stack(), "{geom:#?}");
                } else {
                    assert_eq!(geom.rows, Dimension::fixed(1));
                    assert!(geom.is_collapsed_in_stack(), "{geom:#?}");
                }
            }
        }
    }
}

#[test]
fn fixed_size_pane_splits_its_space_among_flexible_children() {
    for direction in [SplitDirection::Horizontal, SplitDirection::Vertical] {
        let minimum = match direction {
            SplitDirection::Horizontal => MIN_TERMINAL_HEIGHT,
            SplitDirection::Vertical => MIN_TERMINAL_WIDTH,
        };
        let fixed = 4 * minimum + 1;
        let parent = |children: Vec<TiledPaneLayout>| TiledPaneLayout {
            children_split_direction: direction,
            split_size: Some(SplitSize::Fixed(fixed)),
            children,
            ..Default::default()
        };
        let area = PaneGeom::from(&Size {
            rows: 10 * MIN_TERMINAL_HEIGHT,
            cols: 10 * MIN_TERMINAL_WIDTH,
        });
        let leaves = |children: Vec<TiledPaneLayout>, ignore_percent_split_sizes: bool| {
            let layout = TiledPaneLayout {
                children_split_direction: direction,
                children: vec![parent(children), TiledPaneLayout::default()],
                ..Default::default()
            };
            let positions = layout
                .position_panes_in_space(&area, None, ignore_percent_split_sizes, false)
                .unwrap();
            let start = |geom: &PaneGeom| match direction {
                SplitDirection::Horizontal => geom.y,
                SplitDirection::Vertical => geom.x,
            };
            let mut geoms: Vec<PaneGeom> = positions.iter().map(|(_, geom)| *geom).collect();
            geoms.sort_by_key(start);
            let (sibling, children) = geoms.split_last().unwrap();
            assert_eq!(start(sibling), fixed, "{geoms:#?}");
            assert_tiles(children, direction, 0, fixed);
            children
                .iter()
                .map(|geom| match direction {
                    SplitDirection::Horizontal => geom.rows,
                    SplitDirection::Vertical => geom.cols,
                })
                .collect::<Vec<_>>()
        };

        // Two flexible children: equal fixed shares, the remainder to the last.
        let two = vec![TiledPaneLayout::default(); 2];
        for ignore_percent_split_sizes in [false, true] {
            assert_eq!(
                leaves(two.clone(), ignore_percent_split_sizes),
                vec![
                    Dimension::fixed(2 * minimum),
                    Dimension::fixed(2 * minimum + 1)
                ],
            );
        }

        // Fixed siblings come off the top first.
        let mut with_bar = vec![TiledPaneLayout::default(); 3];
        with_bar[1].split_size = Some(SplitSize::Fixed(1));
        assert_eq!(
            leaves(with_bar, false),
            vec![
                Dimension::fixed(2 * minimum),
                Dimension::fixed(1),
                Dimension::fixed(2 * minimum)
            ],
        );

        // A percent sibling resolves as before, and the flexible child takes the rest; with
        // percentages ignored, both become flexible.
        let mut with_percent = vec![TiledPaneLayout::default(); 2];
        with_percent[0].split_size = Some(SplitSize::Percent(10));
        let sizes = leaves(with_percent.clone(), false);
        assert!(sizes[0].is_percent());
        assert_eq!(sizes[1], Dimension::fixed(fixed - sizes[0].as_usize()));
        assert_eq!(
            leaves(with_percent, true),
            vec![
                Dimension::fixed(2 * minimum),
                Dimension::fixed(2 * minimum + 1)
            ],
        );

        // Too many flexible children for the fixed space is an error, not a panic.
        let layout = TiledPaneLayout {
            children_split_direction: direction,
            children: vec![
                parent(vec![TiledPaneLayout::default(); 5]),
                TiledPaneLayout::default(),
            ],
            ..Default::default()
        };
        assert_eq!(
            layout
                .position_panes_in_space(&area, None, false, false)
                .unwrap_err(),
            LayoutError::InsufficientSplitSpace,
        );
    }
}
