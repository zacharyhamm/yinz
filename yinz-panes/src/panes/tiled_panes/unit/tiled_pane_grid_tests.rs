use super::super::pane_resizer_test_mock::{dim_pct, spec, MockPane, PaneSpec};
use super::*;

fn oriented(mut pane: PaneSpec, direction: Direction) -> PaneSpec {
    if matches!(direction, Direction::Left | Direction::Right) {
        std::mem::swap(&mut pane.x, &mut pane.y);
        std::mem::swap(&mut pane.cols, &mut pane.rows);
    }
    pane
}

#[test]
fn hole_filling_characterization() {
    for direction in [
        Direction::Up,
        Direction::Down,
        Direction::Left,
        Direction::Right,
    ] {
        // Canonical coordinates describe up/down; transpose for left/right.
        let specs = [
            spec(1, 10, 0, dim_pct(20.0, 10), dim_pct(20.0, 10)),
            spec(2, 30, 0, dim_pct(20.0, 10), dim_pct(20.0, 10)),
            spec(3, 0, 0, dim_pct(20.0, 10), dim_pct(100.0, 50)),
            spec(4, 20, 0, dim_pct(20.0, 10), dim_pct(100.0, 50)),
            spec(5, 40, 0, dim_pct(20.0, 10), dim_pct(100.0, 50)),
        ];
        // Missing IDs fail during collection; fixed expansion dimensions fail before writes
        // for that hole, but preserve earlier writes. Fixed hole dimensions remain fixed.
        #[derive(Clone, Copy, Debug, PartialEq)]
        enum Case {
            InteriorAndTrailing,
            EmptyHoles,
            MissingExpansion,
            MissingHole,
            FixedExpansion,
            FixedHole,
            EmptyExpansion,
        }
        use Case::*;
        for case in [
            InteriorAndTrailing,
            EmptyHoles,
            MissingExpansion,
            MissingHole,
            FixedExpansion,
            FixedHole,
            EmptyExpansion,
        ] {
            let mut input = specs;
            if case == FixedExpansion {
                input[1].rows = Dimension::fixed(10);
            }
            if case == FixedHole {
                input[2].rows = Dimension::fixed(50);
            }
            let mut panes: HashMap<_, Box<dyn Pane>> = input
                .iter()
                .map(|s| {
                    (
                        s.id,
                        Box::new(MockPane::from_spec(oriented(*s, direction))) as Box<dyn Pane>,
                    )
                })
                .collect();
            let mut grid = TiledPaneGrid::new(
                panes.iter_mut(),
                &HashSet::new(),
                Viewport::default(),
                Viewport::default(),
            );
            let expand = if case == MissingExpansion {
                vec![PaneId::Terminal(crate::TerminalId::new(99))]
            } else if case == EmptyExpansion {
                vec![]
            } else {
                vec![
                    PaneId::Terminal(crate::TerminalId::new(2)),
                    PaneId::Terminal(crate::TerminalId::new(1)),
                ]
            };
            let holes = if case == EmptyHoles {
                vec![]
            } else if case == MissingHole {
                vec![
                    PaneId::Terminal(crate::TerminalId::new(3)),
                    PaneId::Terminal(crate::TerminalId::new(99)),
                ]
            } else {
                vec![
                    PaneId::Terminal(crate::TerminalId::new(5)),
                    PaneId::Terminal(crate::TerminalId::new(4)),
                    PaneId::Terminal(crate::TerminalId::new(3)),
                ]
            };
            assert_eq!(
                grid.fill_geom_holes(&expand, &holes, direction).is_err(),
                matches!(
                    case,
                    MissingExpansion | MissingHole | FixedExpansion | EmptyExpansion
                )
            );
            drop(grid);
            let mut expected = input;
            if matches!(case, InteriorAndTrailing | FixedExpansion | FixedHole) {
                expected[0].x = 0;
                expected[0].cols = dim_pct(20.0, 20);
                if case != FixedHole {
                    expected[2].rows = dim_pct(80.0, 40);
                }
                if matches!(direction, Direction::Down | Direction::Right) {
                    expected[2].y = 10;
                }
                if case != FixedExpansion {
                    // The trailing write uses the original pane 2 snapshot, overwriting its
                    // interior-hole x adjustment rather than accumulating expansion.
                    expected[1].cols = dim_pct(20.0, 20);
                    for i in [3, 4] {
                        expected[i].rows = dim_pct(80.0, 40);
                        if matches!(direction, Direction::Down | Direction::Right) {
                            expected[i].y = 10;
                        }
                    }
                }
            }
            for pane in expected {
                assert_eq!(
                    panes[&pane.id].position_and_size(),
                    oriented(pane, direction).geom(),
                    "{direction:?}, case {case:?}, {:?}",
                    pane.id
                );
            }
        }
    }
}

#[test]
fn rejected_directional_stacking_does_not_change_any_geometry() {
    for direction in [
        Direction::Up,
        Direction::Down,
        Direction::Left,
        Direction::Right,
    ] {
        for nonselectable in [2, 3] {
            // Unequal neighbors require hole filling, which must not run before validation.
            let mut specs = [
                spec(1, 0, 0, dim_pct(100.0, 100), dim_pct(20.0, 20)),
                spec(2, 0, 20, dim_pct(50.0, 50), dim_pct(30.0, 30)),
                spec(3, 50, 20, dim_pct(50.0, 50), dim_pct(50.0, 50)),
            ];
            if matches!(direction, Direction::Up | Direction::Left) {
                for pane in &mut specs {
                    pane.y = 70 - pane.y - pane.rows.as_usize();
                }
            }
            let mut panes: HashMap<_, Box<dyn Pane>> = specs
                .iter()
                .map(|s| {
                    let mut pane = MockPane::from_spec(oriented(*s, direction));
                    pane.set_selectable(
                        s.id != PaneId::Terminal(crate::TerminalId::new(nonselectable)),
                    );
                    (s.id, Box::new(pane) as Box<dyn Pane>)
                })
                .collect();
            let mut grid = TiledPaneGrid::new(
                &mut panes,
                &HashSet::new(),
                Viewport::default(),
                Viewport::default(),
            );
            let root = specs[0].id;
            let result = match direction {
                Direction::Up => grid.stack_pane_up(&root),
                Direction::Down => grid.stack_pane_down(&root),
                Direction::Left => grid.stack_pane_left(&root),
                Direction::Right => grid.stack_pane_right(&root),
            };
            assert!(result.is_none());
            drop(grid);
            for pane in specs {
                assert_eq!(
                    panes[&pane.id].position_and_size(),
                    oriented(pane, direction).geom(),
                    "{direction:?}, nonselectable {nonselectable}"
                );
            }
        }
    }
}

#[test]
fn stack_without_flexible_member_uses_complete_height_for_borders() {
    let mut first = spec(1, 0, 3, dim_pct(100.0, 80), Dimension::fixed(1));
    first.stacked = Some(9);
    let mut last = spec(2, 0, 4, dim_pct(100.0, 80), Dimension::fixed(2));
    last.stacked = Some(9);
    let mut panes: HashMap<_, Box<dyn Pane>> = [first, last]
        .into_iter()
        .map(|s| (s.id, Box::new(MockPane::from_spec(s)) as Box<dyn Pane>))
        .collect();
    let grid = TiledPaneGrid::new(
        &mut panes,
        &HashSet::new(),
        Viewport::default(),
        Viewport::default(),
    );
    let geom = grid.get_pane_geom(&last.id).unwrap();
    assert_eq!(geom.y, 3);
    assert!(geom.rows.is_fixed());
    assert_eq!(geom.rows.as_usize(), 3);
    assert_eq!(
        grid.horizontal_borders(&[first.id, last.id]),
        HashSet::from([3, 6])
    );
    assert!(grid.pane_is_between_horizontal_borders(&last.id, 3, 6));
    assert!(!grid.pane_is_between_horizontal_borders(&last.id, 4, 6));
}

#[test]
fn missing_geometry_is_ignored_by_resize_and_border_checks() {
    let mut panes = HashMap::new();
    let mut grid = TiledPaneGrid::new(
        &mut panes,
        &HashSet::new(),
        Viewport::default(),
        Viewport::default(),
    );
    let missing = PaneId::Terminal(crate::TerminalId::new(99));
    grid.reduce_pane_width(&missing, 5.0);
    grid.increase_pane_width(&missing, 5.0);
    grid.reduce_pane_height(&missing, 5.0);
    grid.increase_pane_height(&missing, 5.0);
    assert!(!grid.is_inside_viewport(&missing));
    assert!(grid.vertical_borders(&[missing]).is_empty());
    assert!(!grid.pane_is_between_horizontal_borders(&missing, 0, 20));
    assert!(!grid.pane_is_between_vertical_borders(&missing, 0, 80));
    assert!(grid.horizontal_borders(&[missing]).is_empty());
    assert!(grid.find_panes_to_grow(missing).unwrap().is_none());
    assert!(!grid.fill_space_over_pane(missing));
}

#[test]
fn closing_with_unavailable_neighbor_geometry_removes_only_the_grid_reference() {
    let closing = spec(1, 0, 0, dim_pct(50.0, 40), dim_pct(100.0, 20));
    let mut neighbor = spec(2, 40, 0, dim_pct(50.0, 40), dim_pct(100.0, 20));
    // The override refers to a stack that has no members in the base layout.
    neighbor.geom_override = Some(PaneGeom {
        stacked: Some(9),
        ..neighbor.geom()
    });
    let mut panes: HashMap<_, Box<dyn Pane>> = [closing, neighbor]
        .into_iter()
        .map(|s| (s.id, Box::new(MockPane::from_spec(s)) as Box<dyn Pane>))
        .collect();
    let viewport = Viewport {
        x: 0,
        y: 0,
        cols: 80,
        rows: 20,
    };
    let mut grid = TiledPaneGrid::new(&mut panes, &HashSet::new(), viewport, viewport);
    assert!(grid.find_panes_to_grow(closing.id).is_err());
    assert!(grid.fill_space_over_pane(closing.id));
    assert!(!grid.panes.borrow().contains_key(&closing.id));
    assert!(grid.panes.borrow().contains_key(&neighbor.id));
    drop(grid);
    assert_eq!(panes.len(), 2, "the caller still owns both panes");
    assert_eq!(panes[&neighbor.id].position_and_size(), neighbor.geom());

    panes.remove(&neighbor.id);
    let mut grid = TiledPaneGrid::new(&mut panes, &HashSet::new(), viewport, viewport);
    assert!(grid.find_panes_to_grow(closing.id).unwrap().is_none());
    assert!(!grid.fill_space_over_pane(closing.id));
    assert!(grid.panes.borrow().contains_key(&closing.id));
}
