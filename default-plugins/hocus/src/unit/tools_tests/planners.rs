use super::*;

#[test]
fn swap_planner_maps_each_unique_shared_edge_to_its_direction() {
    let source = positioned_pane(1, 10, 10, 10, 10);
    for (target, expected) in [
        (positioned_pane(2, 0, 10, 10, 10), PaneDirection::Left),
        (positioned_pane(2, 20, 10, 10, 10), PaneDirection::Right),
        (positioned_pane(2, 10, 0, 10, 10), PaneDirection::Up),
        (positioned_pane(2, 10, 20, 10, 10), PaneDirection::Down),
    ] {
        assert_eq!(
            plan_swap(
                &[source.clone(), target],
                source.id,
                PaneId::Terminal(yinz_tile::prelude::TerminalId::new(2))
            )
            .unwrap(),
            expected
        );
    }
}

#[test]
fn swap_planner_rejects_panes_that_cannot_be_safely_swapped() {
    let source = positioned_pane(1, 0, 0, 10, 10);
    let target = positioned_pane(2, 10, 0, 10, 10);
    assert_eq!(
        plan_swap(&[source.clone(), target.clone()], source.id, source.id).unwrap_err(),
        "swap_panes needs two different panes"
    );

    let mut other_tab = target.clone();
    other_tab.tab_position = Some(yinz_tile::prelude::TabPosition::new(1));
    assert_eq!(
        plan_swap(&[source.clone(), other_tab], source.id, target.id).unwrap_err(),
        "swap_panes needs both panes in the same tab"
    );

    let mut floating = target.clone();
    floating.floating = true;
    assert_eq!(
        plan_swap(&[source.clone(), floating], source.id, target.id).unwrap_err(),
        "swap_panes only supports visible selectable tiled panes"
    );

    let mut hidden = target.clone();
    hidden.visible = false;
    assert_eq!(
        plan_swap(&[source.clone(), hidden], source.id, target.id).unwrap_err(),
        "swap_panes only supports visible selectable tiled panes"
    );

    let mut unselectable = target.clone();
    unselectable.selectable = false;
    assert_eq!(
        plan_swap(&[source.clone(), unselectable], source.id, target.id).unwrap_err(),
        "swap_panes only supports visible selectable tiled panes"
    );

    assert_eq!(
        plan_swap(std::slice::from_ref(&source), source.id, target.id).unwrap_err(),
        "pane is not in the current session; call list_session again"
    );

    let distant = positioned_pane(2, 11, 0, 10, 10);
    assert_eq!(
        plan_swap(&[source.clone(), distant], source.id, target.id).unwrap_err(),
        "panes are not adjacent; do not retry with move_pane"
    );
}

#[test]
fn swap_planner_rejects_an_edge_with_multiple_possible_neighbors() {
    let source = positioned_pane(1, 0, 0, 10, 10);
    let upper = positioned_pane(2, 10, 0, 10, 5);
    let lower = positioned_pane(3, 10, 5, 10, 5);
    assert_eq!(
        plan_swap(&[source.clone(), upper.clone(), lower], source.id, upper.id).unwrap_err(),
        "the requested edge has multiple adjacent panes; swap is ambiguous"
    );

    let left = positioned_pane(1, 0, 0, 10, 5);
    let right = positioned_pane(2, 10, 0, 10, 10);
    let lower_left = positioned_pane(3, 0, 5, 10, 5);
    assert_eq!(
        plan_swap(
            &[left.clone(), right.clone(), lower_left],
            left.id,
            right.id
        )
        .unwrap_err(),
        "the requested edge has multiple adjacent panes; swap is ambiguous"
    );
}

#[test]
fn planners_reject_tabs_where_yinz_will_not_move_tiled_panes() {
    let source = positioned_pane(1, 0, 0, 10, 10);
    let target = positioned_pane(2, 10, 0, 10, 10);

    let mut fullscreen = source.clone();
    fullscreen.fullscreen = true;
    assert!(plan_move(
        &[fullscreen, target.clone()],
        source.id,
        PaneDirection::Right
    )
    .is_err());

    let mut floating_layer = source.clone();
    floating_layer.floating_layer_visible = true;
    assert!(plan_swap(
        &[floating_layer, target],
        source.id,
        PaneId::Terminal(yinz_tile::prelude::TerminalId::new(2))
    )
    .is_err());
}

#[test]
fn move_planner_requires_one_unique_neighbor_in_the_requested_direction() {
    let source = positioned_pane(1, 0, 0, 10, 10);
    let target = positioned_pane(2, 10, 0, 10, 10);
    assert_eq!(
        plan_move(
            &[source.clone(), target.clone()],
            source.id,
            PaneDirection::Right
        )
        .unwrap(),
        target.id
    );

    assert_eq!(
        plan_move(
            std::slice::from_ref(&source),
            source.id,
            PaneDirection::Right
        )
        .unwrap_err(),
        "there is no adjacent pane in that direction"
    );

    let lower = positioned_pane(3, 10, 5, 10, 5);
    assert_eq!(
        plan_move(
            &[source.clone(), target, lower],
            source.id,
            PaneDirection::Right
        )
        .unwrap_err(),
        "the requested direction has multiple adjacent panes; move is ambiguous"
    );
}

#[test]
fn pane_direction_rejects_unknown_names() {
    assert_eq!(
        PaneDirection::parse(Some("diagonal")).unwrap_err(),
        "direction must be left, right, up or down"
    );
}
