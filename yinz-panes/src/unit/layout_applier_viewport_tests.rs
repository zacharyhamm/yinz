use super::*;

#[test]
fn edge_strips_require_gapless_coverage_and_matching_thickness() {
    let viewport = Viewport {
        x: 5,
        y: 7,
        cols: 100,
        rows: 40,
    };
    for (spans, expected_y) in [
        (vec![(5, 50, 2), (55, 50, 2)], 9),
        (vec![(55, 50, 2), (5, 50, 2)], 9),
        (vec![(5, 60, 2), (55, 50, 2)], 9),
        (vec![(5, 49, 2), (55, 50, 2)], 7),
        (vec![(6, 49, 2), (55, 50, 2)], 7),
        (vec![(5, 50, 2), (55, 49, 2)], 7),
        (vec![(5, 50, 2), (55, 50, 3)], 7),
        (vec![(5, 100, 0)], 7),
        (vec![(5, 100, 40)], 7),
    ] {
        let geoms: Vec<_> = spans
            .into_iter()
            .map(|(x, cols, rows)| Viewport {
                x,
                cols,
                y: 7,
                rows,
            })
            .collect();
        let mut actual = viewport;
        offset_viewport_for_ui_panes(&mut actual, &geoms);
        assert_eq!(
            actual,
            Viewport {
                y: expected_y,
                rows: 40 - (expected_y - 7),
                ..viewport
            },
            "{geoms:?}"
        );
    }
}

#[test]
fn ui_strip_traversal_does_not_revisit_earlier_layers() {
    let mut viewport = Viewport {
        x: 0,
        y: 0,
        cols: 100,
        rows: 40,
    };
    let panes = [
        Viewport {
            y: 2,
            rows: 2,
            ..viewport
        },
        Viewport {
            rows: 2,
            ..viewport
        },
    ];
    offset_viewport_for_ui_panes(&mut viewport, &panes);
    assert_eq!(
        viewport,
        Viewport {
            x: 0,
            y: 2,
            cols: 100,
            rows: 38
        }
    );
}
