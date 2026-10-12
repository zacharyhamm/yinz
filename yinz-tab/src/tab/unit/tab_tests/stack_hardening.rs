use super::*;

fn tab_with_small_fixed_pane() -> Tab {
    let layout = TiledPaneLayout {
        children: vec![
            TiledPaneLayout {
                split_size: Some(SplitSize::Fixed(7)),
                children_split_direction: SplitDirection::Vertical,
                children: vec![
                    TiledPaneLayout {
                        split_size: Some(SplitSize::Fixed(24)),
                        ..Default::default()
                    },
                    TiledPaneLayout::default(),
                ],
                ..Default::default()
            },
            TiledPaneLayout::default(),
        ],
        ..Default::default()
    };
    let mut tab = create_new_tab_with_layout(
        Size {
            cols: 120,
            rows: 40,
        },
        layout,
    );
    tab.focus_pane_with_id(
        PaneId::Terminal(TerminalId::new(0)),
        false,
        false,
        ClientId::new(1),
    )
    .unwrap();
    tab
}

#[test]
fn failed_stack_creation_preserves_geometry_and_pane_ownership() {
    let root = PaneId::Terminal(TerminalId::new(0));
    let new = PaneId::Terminal(TerminalId::new(3));
    for existing_stack in [false, true] {
        for placement in [
            NewPanePlacement::default(),
            NewPanePlacement::Stacked {
                pane_id_to_stack_under: None,
                borderless: None,
            },
            NewPanePlacement::Stacked {
                pane_id_to_stack_under: Some(root),
                borderless: None,
            },
        ] {
            let implicit = matches!(placement, NewPanePlacement::NoPreference { .. });
            let mut tab = tab_with_small_fixed_pane();
            if existing_stack {
                let pane = tab.tiled_panes.panes.get_mut(&root).unwrap();
                let mut geom = pane.position_and_size();
                geom.stacked = Some(999);
                pane.set_geom(geom);
            }
            let before = tiled_pane_geoms(&tab);
            tab.new_pane(
                new,
                None,
                None,
                false,
                false,
                placement,
                Some(ClientId::new(1)),
                None,
            )
            .unwrap();
            assert_eq!(
                tab.tiled_panes
                    .panes
                    .get(&root)
                    .unwrap()
                    .position_and_size(),
                before[0].1
            );
            assert!(
                tab.has_pane_with_pid(&new),
                "failed insertion must retain ownership"
            );
            if implicit {
                assert!(
                    tab.tiled_panes.panes.contains_key(&new),
                    "fallback places the pane elsewhere"
                );
            } else {
                assert_eq!(tiled_pane_geoms(&tab), before);
                assert!(
                    tab.suppressed_panes.contains_key(&new),
                    "unplaced panes are parked"
                );
            }
        }
    }
}

#[test]
fn closing_beside_a_stack_without_a_flexible_member_succeeds() {
    let mut tab = tab_with_small_fixed_pane();
    let root = PaneId::Terminal(TerminalId::new(0));
    let closing = PaneId::Terminal(TerminalId::new(3));
    tab.focus_pane_with_id(
        PaneId::Terminal(TerminalId::new(2)),
        false,
        false,
        ClientId::new(1),
    )
    .unwrap();
    tab.new_pane(
        closing,
        None,
        None,
        false,
        true,
        NewPanePlacement::default(),
        Some(ClientId::new(1)),
        None,
    )
    .unwrap();
    let pane = tab.tiled_panes.panes.get_mut(&root).unwrap();
    let mut geom = pane.position_and_size();
    assert!(geom.rows.is_fixed());
    geom.stacked = Some(999);
    pane.set_geom(geom);

    tab.close_pane(closing, false, None);

    assert!(!tab.has_pane_with_pid(&closing));
    assert_eq!(tab.tiled_panes.panes.len(), 3);
    assert!(tab.tiled_panes.panes.contains_key(&root));
}

#[test]
fn moving_a_fullscreen_hidden_pane_preserves_both_panes() {
    let mut tab = create_new_tab(
        Size {
            cols: 121,
            rows: 20,
        },
        false,
    );
    let client = ClientId::new(1);
    let hidden = PaneId::Terminal(TerminalId::new(1));
    tab.vertical_split(
        PaneId::Terminal(TerminalId::new(2)),
        None,
        None,
        client,
        None,
        None,
    )
    .unwrap();
    tab.toggle_active_pane_fullscreen(client);
    let before = tiled_pane_geoms(&tab);
    for backwards in [false, true] {
        tab.tiled_panes.move_pane(backwards, hidden);
        assert_eq!(tiled_pane_geoms(&tab), before);
    }
}

#[test]
fn fixed_height_singleton_stack_rejects_splits_without_changing_geometry() {
    let mut tab = create_new_tab_with_layout(
        Size {
            cols: 120,
            rows: 40,
        },
        TiledPaneLayout {
            children: vec![
                TiledPaneLayout {
                    split_size: Some(SplitSize::Fixed(7)),
                    ..Default::default()
                },
                TiledPaneLayout::default(),
            ],
            ..Default::default()
        },
    );
    let root = PaneId::Terminal(TerminalId::new(0));
    let new = PaneId::Terminal(TerminalId::new(2));
    let pane = tab.tiled_panes.panes.get_mut(&root).unwrap();
    let mut geom = pane.position_and_size();
    geom.stacked = Some(999);
    pane.set_geom(geom);
    let before = tiled_pane_geoms(&tab);
    let bounds = tab.tiled_panes.position_and_size_of_stack(&root).unwrap();
    assert_eq!(bounds.cols.as_usize(), 120);
    assert_eq!(bounds.rows, yinz_types::pane_size::Dimension::fixed(7));
    assert!(!tab.tiled_panes.can_split_pane_id_vertically(root));
    assert!(!tab.tiled_panes.can_split_pane_id_horizontally(root));

    tab.vertical_split_of_pane_id(new, None, None, root, None, None)
        .unwrap();
    tab.horizontal_split_of_pane_id(new, None, None, root, None, None)
        .unwrap();
    assert_eq!(tiled_pane_geoms(&tab), before);
    assert!(!tab.has_pane_with_pid(&new));
}

#[test]
fn fixed_height_stack_list_preserves_full_height_after_resize() {
    let mut tab = tab_with_small_fixed_pane();
    let root = PaneId::Terminal(TerminalId::new(0));
    let new = PaneId::Terminal(TerminalId::new(3));
    tab.new_pane(
        new,
        None,
        None,
        false,
        false,
        NewPanePlacement::Stacked {
            pane_id_to_stack_under: Some(root),
            borderless: None,
        },
        Some(ClientId::new(1)),
        None,
    )
    .unwrap();
    let pane = tab.extract_pane(new, true).unwrap();
    tab.stack_panes(root, vec![pane]);
    assert!(tab.tiled_panes.panes.contains_key(&new));
    let bounds = tab.tiled_panes.position_and_size_of_stack(&root).unwrap();
    assert_eq!(bounds.rows, yinz_types::pane_size::Dimension::fixed(7));

    *tab.stacked_pane_list.borrow_mut() = true;
    tab.sync_stacked_pane_list_mode();
    assert!(tab.pane_is_hidden_stack_list_member(&root));
    for size in [
        Size {
            cols: 120,
            rows: 40,
        },
        Size {
            cols: 130,
            rows: 50,
        },
    ] {
        tab.resize_whole_tab(size).unwrap();
        let geom = tab.get_pane_with_id(new).unwrap().position_and_size();
        assert_eq!(geom.rows, yinz_types::pane_size::Dimension::fixed(7));
    }
}
