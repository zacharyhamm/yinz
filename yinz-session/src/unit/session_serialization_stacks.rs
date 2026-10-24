use super::*;
use yinz_types::pane_size::{Dimension, PaneGeom};

fn pane(name: &str, x: usize, y: usize, rows: usize, stack_id: usize) -> PaneLayoutManifest {
    PaneLayoutManifest {
        title: Some(name.to_owned()),
        geom: PaneGeom {
            x,
            y,
            rows: Dimension::fixed(rows),
            cols: Dimension::fixed(10),
            stacked: Some(stack_id),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn stack_serialization_is_stable_under_reordered_input() {
    let mut panes = vec![
        pane("lower-expanded", 0, 22, 5, 1),
        pane("right-collapsed", 20, 10, 1, 2),
        pane("left-expanded", 10, 11, 5, 9),
        pane("lower-collapsed", 0, 21, 1, 1),
        pane("right-expanded", 20, 11, 5, 2),
        pane("left-collapsed", 10, 10, 1, 9),
    ];
    let split_size = Some(SplitSize::Fixed(30));
    let expected = stack_layout_from_manifest(&panes, split_size).unwrap();
    assert_eq!(expected.split_size, split_size);
    assert_eq!(
        expected
            .children
            .iter()
            .map(|stack| {
                assert!(stack.children_are_stacked);
                stack
                    .children
                    .iter()
                    .map(|p| (p.name.as_deref().unwrap(), p.is_expanded_in_stack))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>(),
        vec![
            vec![("left-collapsed", false), ("left-expanded", true)],
            vec![("right-collapsed", false), ("right-expanded", true)],
            vec![("lower-collapsed", false), ("lower-expanded", true)],
        ],
    );
    let serialized = serialize_tiled_pane(&expected, false, &mut BTreeMap::new()).to_string();
    for _ in 0..panes.len() {
        panes.rotate_left(1);
        for input in [panes.clone(), panes.iter().rev().cloned().collect()] {
            let layout = stack_layout_from_manifest(&input, split_size).unwrap();
            assert_eq!(layout, expected);
            assert_eq!(
                serialize_tiled_pane(&layout, false, &mut BTreeMap::new()).to_string(),
                serialized
            );
        }
    }
}

#[test]
fn stack_ids_break_ties_at_the_same_position() {
    let panes = vec![pane("later", 10, 20, 5, 9), pane("earlier", 10, 20, 5, 2)];
    let layout = stack_layout_from_manifest(&panes, None).unwrap();
    assert_eq!(
        layout.children[0].children[0].name.as_deref(),
        Some("earlier")
    );
    assert_eq!(
        layout.children[1].children[0].name.as_deref(),
        Some("later")
    );
}

#[test]
fn nonzero_origin_stacks_split_only_at_their_boundaries() {
    let mut panes = vec![
        pane("top-collapsed", 10, 20, 1, 8),
        pane("top-expanded", 10, 21, 8, 8),
        pane("top-tail", 10, 29, 1, 8),
        pane("bottom-collapsed", 10, 30, 1, 3),
        pane("bottom-expanded", 10, 31, 8, 3),
        pane("bottom-tail", 10, 39, 1, 3),
    ];
    for _ in 0..panes.len() {
        panes.rotate_left(1);
        assert_eq!(
            get_row_splits(&panes, &(10, 20), &(20, 40)),
            vec![20, 30, 40]
        );
        let layout = get_tiled_panes_layout_from_panegeoms(&panes, None).unwrap();
        assert_eq!(layout.children.len(), 2);
        for (stack, prefix) in layout.children.iter().zip(["top", "bottom"]) {
            assert_eq!(stack.split_size, Some(SplitSize::Fixed(10)));
            assert!(stack.children_are_stacked);
            assert_eq!(stack.children.len(), 3);
            assert_eq!(
                stack.children[1].name.as_deref(),
                Some(format!("{prefix}-expanded").as_str())
            );
            assert!(stack.children[1].is_expanded_in_stack);
        }
    }
}
