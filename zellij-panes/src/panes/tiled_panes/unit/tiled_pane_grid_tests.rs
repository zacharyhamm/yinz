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
