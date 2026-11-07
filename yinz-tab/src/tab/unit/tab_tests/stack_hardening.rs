use super::*;

fn tab_with_small_fixed_pane() -> Tab {
    tab_with_fixed_pane(7)
}

/// A tab whose focused pane 0 is 24 columns by `rows` rows, both fixed, beside pane 2 and above
/// pane 1.
fn tab_with_fixed_pane(rows: usize) -> Tab {
    let layout = TiledPaneLayout {
        children: vec![
            TiledPaneLayout {
                split_size: Some(SplitSize::Fixed(rows)),
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
            // A stack of two needs a title row plus MIN_TERMINAL_HEIGHT rows, one more than
            // this pane has.
            let mut tab = tab_with_fixed_pane(yinz_types::pane_size::MIN_TERMINAL_HEIGHT);
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
fn stacking_a_fixed_height_pane_keeps_its_rows() {
    let root = PaneId::Terminal(TerminalId::new(0));
    let new = PaneId::Terminal(TerminalId::new(3));
    for pane_id_to_stack_under in [None, Some(root)] {
        let mut tab = tab_with_small_fixed_pane();
        let before = tiled_pane_geoms(&tab);
        tab.new_pane(
            new,
            None,
            None,
            false,
            false,
            NewPanePlacement::Stacked {
                pane_id_to_stack_under,
                borderless: None,
            },
            Some(ClientId::new(1)),
            None,
        )
        .unwrap();
        assert!(
            tab.tiled_panes.panes.contains_key(&new),
            "{pane_id_to_stack_under:?}"
        );
        let geom_of = |id| tab.get_pane_with_id(id).unwrap().position_and_size();
        // The root collapses to a title row and the new pane shows the other 6 rows, under the
        // stack's fixed height of 7.
        assert!(
            geom_of(root).is_collapsed_in_stack(),
            "{:#?}",
            geom_of(root)
        );
        let expanded = geom_of(new);
        assert!(expanded.is_expanded_in_stack(), "{expanded:#?}");
        assert_eq!((expanded.y, expanded.rows.as_usize()), (1, 6));
        let bounds = tab.tiled_panes.position_and_size_of_stack(&root).unwrap();
        assert_eq!(bounds.rows, yinz_types::pane_size::Dimension::fixed(7));
        assert_eq!((bounds.x, bounds.y), (before[0].1.x, before[0].1.y));
        // The other panes keep their places.
        let after = tiled_pane_geoms(&tab);
        for (id, geom) in &before[1..] {
            assert!(after.contains(&(*id, *geom)), "{id:?}: {after:#?}");
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

    // The root keeps the stack's fixed height in its constraint, which marks it expanded, so
    // it is the list's visible member.
    assert!(tab
        .get_pane_with_id(root)
        .unwrap()
        .position_and_size()
        .is_expanded_in_stack());
    *tab.stacked_pane_list.borrow_mut() = true;
    tab.sync_stacked_pane_list_mode();
    assert!(tab.pane_is_hidden_stack_list_member(&new));
    assert!(!tab.pane_is_hidden_stack_list_member(&root));
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
        let geom = tab.get_pane_with_id(root).unwrap().position_and_size();
        assert_eq!(geom.rows, yinz_types::pane_size::Dimension::fixed(7));
    }
}

#[test]
fn breaking_out_of_a_stack_under_a_fixed_pane_keeps_its_rows() {
    let root = PaneId::Terminal(TerminalId::new(0));
    let new = PaneId::Terminal(TerminalId::new(3));
    let client_id = ClientId::new(1);
    let mut tab = tab_with_small_fixed_pane();
    let before = tiled_pane_geoms(&tab);
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
        Some(client_id),
        None,
    )
    .unwrap();
    tab.resize(
        client_id,
        yinz_types::geometry::ResizeStrategy::new(yinz_types::geometry::Resize::Decrease, None),
    )
    .unwrap();
    let geom_of = |tab: &Tab, id| tab.get_pane_with_id(id).unwrap().position_and_size();
    let (root_geom, new_geom) = (geom_of(&tab, root), geom_of(&tab, new));
    // Both panes left the stack and split its 7 rows between them.
    assert!(!root_geom.is_stacked(), "{root_geom:#?}");
    assert!(!new_geom.is_stacked(), "{new_geom:#?}");
    let (top, bottom) = if root_geom.y < new_geom.y {
        (root_geom, new_geom)
    } else {
        (new_geom, root_geom)
    };
    assert_eq!(top.y, before[0].1.y, "{top:#?}");
    assert_eq!(bottom.y, top.y + top.rows.as_usize(), "{bottom:#?}");
    assert_eq!(top.rows.as_usize() + bottom.rows.as_usize(), 7);
    for geom in [top, bottom] {
        assert!(geom.rows.is_fixed(), "{geom:#?}");
        assert_eq!(
            geom.rows.constraint,
            yinz_types::pane_size::Constraint::Fixed(geom.rows.as_usize())
        );
    }
    // The other panes keep their places.
    let after = tiled_pane_geoms(&tab);
    for (id, geom) in &before[1..] {
        assert!(after.contains(&(*id, *geom)), "{id:?}: {after:#?}");
    }
}
