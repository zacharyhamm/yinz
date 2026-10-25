use super::*;

#[test]
fn splits_with_unknown_clients_or_targets_preserve_existing_panes() {
    for direction in [SplitDirection::Horizontal, SplitDirection::Vertical] {
        let mut tab = create_new_tab(
            Size {
                cols: 121,
                rows: 20,
            },
            true,
        );
        let before = tiled_pane_geoms(&tab);
        let new_pane = PaneId::Terminal(TerminalId::new(2));
        let missing_client = ClientId::new(99);
        let missing_pane = PaneId::Terminal(TerminalId::new(99));
        match direction {
            SplitDirection::Horizontal => {
                tab.horizontal_split(new_pane, None, None, missing_client, None, None)
                    .unwrap();
                tab.horizontal_split_of_pane_id(new_pane, None, None, missing_pane, None, None)
                    .unwrap();
            },
            SplitDirection::Vertical => {
                tab.vertical_split(new_pane, None, None, missing_client, None, None)
                    .unwrap();
                tab.vertical_split_of_pane_id(new_pane, None, None, missing_pane, None, None)
                    .unwrap();
            },
        }
        assert_eq!(tiled_pane_geoms(&tab), before);
        assert_eq!(
            tab.tiled_panes.get_active_pane_id(ClientId::new(1)),
            Some(PaneId::Terminal(TerminalId::new(1)))
        );
    }
}

#[test]
fn write_to_suppressed_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();

    // Suppress pane 2 and remove it from active panes
    tab.replace_active_pane_with_editor_pane(
        PaneId::Terminal(crate::TerminalId::new(2)),
        crate::ClientId::new(1),
    )
    .unwrap();
    tab.tiled_panes
        .remove_pane(PaneId::Terminal(crate::TerminalId::new(2)));

    // Make sure it's suppressed now
    tab.suppressed_panes
        .get(&PaneId::Terminal(crate::TerminalId::new(2)))
        .unwrap();
    // Write content to it
    tab.write_to_pane_id(
        &None,
        vec![34, 127, 31, 82, 17, 182],
        false,
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        None,
    )
    .unwrap();
}

#[test]
fn split_panes_vertically() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    tab.vertical_split(new_pane_id, None, None, crate::ClientId::new(1), None, None)
        .unwrap();
    assert_eq!(tab.tiled_panes.panes.len(), 2, "The tab has two panes");
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "first pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "first pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "first pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20,
        "first pane row count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "second pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "second pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "second pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        20,
        "second pane row count"
    );
}

#[test]
fn split_panes_horizontally() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    tab.horizontal_split(new_pane_id, None, None, crate::ClientId::new(1), None, None)
        .unwrap();
    assert_eq!(tab.tiled_panes.panes.len(), 2, "The tab has two panes");

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "first pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "first pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "first pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        10,
        "first pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "second pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .y,
        10,
        "second pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        121,
        "second pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        10,
        "second pane row count"
    );
}

#[test]
fn split_largest_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = false; // note - this is not the default
    let mut tab = create_new_tab(size, stacked_resize);
    for i in 2..5 {
        let new_pane_id = PaneId::Terminal(crate::TerminalId::new(i));
        tab.new_pane(
            new_pane_id,
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(crate::ClientId::new(1)),
            None,
        )
        .unwrap();
    }
    assert_eq!(tab.tiled_panes.panes.len(), 4, "The tab has four panes");

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "first pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "first pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "first pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(1)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        10,
        "first pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "second pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .y,
        0,
        "second pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "second pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(2)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        10,
        "second pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .x,
        0,
        "third pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .y,
        10,
        "third pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        61,
        "third pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(3)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        10,
        "third pane row count"
    );

    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .x,
        61,
        "fourth pane x position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .y,
        10,
        "fourth pane y position"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .cols
            .as_usize(),
        60,
        "fourth pane column count"
    );
    assert_eq!(
        tab.tiled_panes
            .panes
            .get(&PaneId::Terminal(crate::TerminalId::new(4)))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize(),
        10,
        "fourth pane row count"
    );
}

#[test]
pub fn cannot_split_panes_vertically_when_active_pane_is_too_small() {
    let size = Size { cols: 8, rows: 20 };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        tab.tiled_panes.panes.len(),
        1,
        "Tab still has only one pane"
    );
}

#[test]
pub fn cannot_split_panes_horizontally_when_active_pane_is_too_small() {
    let size = Size { cols: 121, rows: 5 };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        tab.tiled_panes.panes.len(),
        1,
        "Tab still has only one pane"
    );
}

#[test]
pub fn cannot_split_largest_pane_when_there_is_no_room() {
    let size = Size { cols: 8, rows: 5 };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.new_pane(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(crate::ClientId::new(1)),
        None,
    )
    .unwrap();
    assert_eq!(
        tab.tiled_panes.panes.len(),
        1,
        "Tab still has only one pane"
    );
}

#[test]
pub fn cannot_split_panes_vertically_when_active_pane_has_fixed_columns() {
    let size = Size { cols: 50, rows: 20 };
    let fixed_child = TiledPaneLayout {
        split_size: Some(SplitSize::Fixed(30)),
        ..Default::default()
    };
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![fixed_child, TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut tab = create_new_tab_with_layout(size, initial_layout);
    tab.vertical_split(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    assert_eq!(tab.tiled_panes.panes.len(), 2, "Tab still has two panes");
}

#[test]
pub fn cannot_split_panes_horizontally_when_active_pane_has_fixed_rows() {
    let size = Size { cols: 50, rows: 20 };
    let fixed_child = TiledPaneLayout {
        split_size: Some(SplitSize::Fixed(12)),
        ..Default::default()
    };
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Horizontal,
        children: vec![fixed_child, TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut tab = create_new_tab_with_layout(size, initial_layout);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    assert_eq!(tab.tiled_panes.panes.len(), 2, "Tab still has two panes");
}

#[test]
pub fn split_in_direction_without_a_connected_client_still_creates_the_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.remove_client(crate::ClientId::new(1));
    tab.new_pane(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        None,
        false,
        true,
        NewPanePlacement::Tiled {
            direction: Some(Direction::Down),
            borderless: None,
        },
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        tab.tiled_panes.panes.len(),
        2,
        "the pane is created even though no client is attached"
    );
}

#[test]
pub fn split_in_direction_without_a_connected_client_splits_the_existing_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.remove_client(crate::ClientId::new(1));
    tab.new_pane(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        None,
        false,
        true,
        NewPanePlacement::Tiled {
            direction: Some(Direction::Right),
            borderless: None,
        },
        None,
        None,
    )
    .unwrap();
    let first_pane_geom = tab
        .tiled_panes
        .panes
        .get(&PaneId::Terminal(crate::TerminalId::new(1)))
        .unwrap()
        .position_and_size();
    let new_pane_geom = tab
        .tiled_panes
        .panes
        .get(&PaneId::Terminal(crate::TerminalId::new(2)))
        .unwrap()
        .position_and_size();
    assert_eq!(first_pane_geom.x, 0, "the existing pane keeps its position");
    assert!(
        new_pane_geom.x > first_pane_geom.x,
        "the new pane is placed to the right of the existing one"
    );
    assert_eq!(
        first_pane_geom.y, new_pane_geom.y,
        "both panes share the same row"
    );
}
