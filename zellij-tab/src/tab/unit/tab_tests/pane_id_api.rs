use super::*;

#[test]
fn undirected_resize_reports_fixed_blockers_only_when_no_direction_succeeds() {
    for can_grow_down in [false, true] {
        let top = TiledPaneLayout {
            children_split_direction: SplitDirection::Vertical,
            children: vec![
                TiledPaneLayout {
                    split_size: Some(SplitSize::Fixed(30)),
                    ..Default::default()
                },
                TiledPaneLayout::default(),
                TiledPaneLayout {
                    split_size: Some(SplitSize::Fixed(30)),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let layout = if can_grow_down {
            TiledPaneLayout {
                children: vec![top, TiledPaneLayout::default()],
                ..Default::default()
            }
        } else {
            top
        };
        let mut tab = create_new_tab_with_layout(
            Size {
                cols: 100,
                rows: if can_grow_down { 40 } else { 20 },
            },
            layout,
        );
        let active = PaneId::Terminal(TerminalId::new(1));
        let result = tab.tiled_panes.resize_pane_with_id(
            ResizeStrategy::new(Resize::Increase, None),
            active,
            None,
        );
        if can_grow_down {
            assert!(result.unwrap());
            assert!(
                tab.get_pane_with_id(active)
                    .unwrap()
                    .current_geom()
                    .rows
                    .as_usize()
                    > 20
            );
        } else {
            let error = result.unwrap_err();
            match error.downcast_ref::<ZellijError>() {
                Some(ZellijError::CantResizeFixedPanes { pane_ids }) => assert_eq!(
                    pane_ids,
                    &vec![
                        PaneId::Terminal(TerminalId::new(0)),
                        PaneId::Terminal(TerminalId::new(2)),
                    ]
                ),
                _ => panic!("unexpected resize error: {error:?}"),
            }
        }
    }
}

#[test]
fn extracting_a_suppressed_pane_discards_its_old_layout_position() {
    let client = ClientId::new(1);
    for id in [
        PaneId::Terminal(TerminalId::new(2)),
        PaneId::Plugin(crate::PluginId::new(2)),
    ] {
        let mut tab = create_new_tab(
            Size {
                cols: 121,
                rows: 20,
            },
            false,
        );
        let (sender, _receiver) = unbounded();
        tab.senders
            .replace_to_plugin(SenderWithContext::new(sender));
        tab.new_pane(
            id,
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(client),
            None,
        )
        .unwrap();
        let pane = tab.get_pane_with_id_mut(id).unwrap();
        let mut geom = pane.position_and_size();
        geom.logical_position = Some(7);
        pane.set_geom(geom);
        tab.suppress_pane(id, Some(client));

        let pane = tab.extract_pane(id, true).unwrap();
        assert_eq!(pane.pid(), id);
        assert_eq!(pane.position_and_size().logical_position, None);
        assert!(!tab.has_pane_with_pid(&id));
        assert!(tab.extract_pane(id, true).is_none());
    }
}

#[test]
fn first_client_focus_follows_visible_pane_positions() {
    let client = ClientId::new(1);
    let first = PaneId::Terminal(TerminalId::new(1));
    let second = PaneId::Terminal(TerminalId::new(2));
    for (explicit_focus, unselectable, fullscreen, expected) in [
        (None, false, false, second),
        (Some(first), false, false, first),
        (None, true, false, first),
        (None, false, true, first),
    ] {
        let mut tab = create_new_tab(
            Size {
                cols: 121,
                rows: 20,
            },
            false,
        );
        tab.horizontal_split(second, None, client, None, None)
            .unwrap();
        for (id, y) in [(first, 10), (second, 0)] {
            let pane = tab.get_pane_with_id_mut(id).unwrap();
            let mut geom = pane.position_and_size();
            geom.y = y;
            pane.set_geom(geom);
        }
        if unselectable {
            tab.set_pane_selectable(second, false);
        }
        if fullscreen {
            tab.tiled_panes.toggle_pane_fullscreen(first);
        }
        tab.remove_client(client);
        tab.focus_pane_id = explicit_focus;
        let joining = ClientId::new(7);
        tab.add_client(joining, None).unwrap();
        assert_eq!(tab.get_active_pane_id(joining), Some(expected));
    }
}

#[test]
fn revealing_a_suppressed_pane_in_place_replaces_the_focused_overlay() {
    let client = ClientId::new(1);
    let tiled = PaneId::Terminal(TerminalId::new(1));
    let floating = PaneId::Terminal(TerminalId::new(2));
    let replacement = PaneId::Terminal(TerminalId::new(3));
    let overlay = PaneId::Terminal(TerminalId::new(4));
    for show_floating in [false, true] {
        for placement in [
            OverlayPlacement::dismissible(None),
            OverlayPlacement::modal(None),
        ] {
            let mut tab = create_new_tab(
                Size {
                    cols: 121,
                    rows: 20,
                },
                false,
            );
            tab.new_pane(
                replacement,
                None,
                None,
                false,
                true,
                NewPanePlacement::Tiled {
                    direction: None,
                    borderless: None,
                },
                Some(client),
                None,
            )
            .unwrap();
            tab.suppress_pane(replacement, Some(client));
            if show_floating {
                tab.new_floating_pane(floating, None, None, false, true, None, None)
                    .unwrap();
            }
            tab.new_pane(
                overlay,
                None,
                None,
                false,
                true,
                NewPanePlacement::Overlay(placement),
                Some(client),
                None,
            )
            .unwrap();
            let original_geom = tab.get_pane_with_id(overlay).unwrap().current_geom();
            let underlying: Vec<_> = [tiled, floating]
                .into_iter()
                .filter_map(|id| {
                    tab.get_pane_with_id(id)
                        .map(|pane| (id, pane.current_geom()))
                })
                .collect();
            assert!(tab.can_replace_active_pane(client));
            tab.focus_pane_with_id(replacement, false, true, client)
                .unwrap();
            assert_eq!(tab.get_active_pane_id(client), Some(replacement));
            assert!(tab.pane_is_overlay(replacement));
            assert_eq!(
                tab.get_pane_with_id(replacement).unwrap().current_geom(),
                original_geom
            );
            assert!(tab.has_pane_with_pid(&overlay));
            assert!(!tab.has_non_suppressed_pane_with_pid(&overlay));
            assert_eq!(tab.are_floating_panes_visible(), show_floating);
            for (id, geom) in underlying {
                assert!(tab.has_non_suppressed_pane_with_pid(&id));
                assert_eq!(tab.get_pane_with_id(id).unwrap().current_geom(), geom);
            }
            tab.close_pane(replacement, false, None);
            assert_eq!(tab.get_active_pane_id(client), Some(overlay));
            assert!(tab.pane_is_overlay(overlay));
            assert_eq!(
                tab.get_pane_with_id(overlay).unwrap().current_geom(),
                original_geom
            );
            assert!(!tab.has_pane_with_pid(&replacement));
        }
    }
}

#[test]
fn focusing_an_unselectable_pane_preserves_focus_and_layer_visibility() {
    let client = ClientId::new(1);
    let tiled = PaneId::Terminal(TerminalId::new(1));
    let floating = PaneId::Terminal(TerminalId::new(2));
    let overlay = PaneId::Terminal(TerminalId::new(3));
    for target in [tiled, floating, overlay] {
        let mut tab = create_new_tab(
            Size {
                cols: 121,
                rows: 20,
            },
            false,
        );
        tab.new_floating_pane(floating, None, None, false, true, None, None)
            .unwrap();
        tab.new_pane(
            overlay,
            None,
            None,
            false,
            true,
            NewPanePlacement::Overlay(OverlayPlacement::dismissible(None)),
            Some(client),
            None,
        )
        .unwrap();
        tab.get_pane_with_id_mut(target)
            .unwrap()
            .set_selectable(false);
        if target == overlay {
            tab.overlay_panes.defocus_pane(client);
            tab.hide_overlay_pane(overlay);
        }
        let focus_before = tab.get_active_pane_id(client);
        let overlay_before = tab.overlay_panes.rendered_pane_ids();
        assert!(tab
            .focus_pane_with_id(target, false, false, client)
            .is_err());
        assert_eq!(tab.get_active_pane_id(client), focus_before);
        assert!(tab.are_floating_panes_visible());
        assert_eq!(tab.overlay_panes.rendered_pane_ids(), overlay_before);
    }

    // Explicitly revealing a parked pane still makes it selectable.
    let mut tab = create_tab_with_two_floating_panes();
    tab.suppress_pane(floating, Some(client));
    tab.get_pane_with_id_mut(floating)
        .unwrap()
        .set_selectable(false);
    tab.focus_pane_with_id(floating, true, false, client)
        .unwrap();
    assert_eq!(tab.get_active_pane_id(client), Some(floating));
    assert!(tab.get_pane_with_id(floating).unwrap().selectable());
}

#[test]
fn floating_hit_testing_uses_topmost_matching_pane() {
    use zellij_types::position::Position;

    let mut tab = create_tab_with_two_floating_panes();
    let client = ClientId::new(1);
    let lower = PaneId::Terminal(TerminalId::new(2));
    let upper = PaneId::Terminal(TerminalId::new(3));
    let geom = floating_pane_geom(&tab, lower);
    tab.floating_panes
        .get_pane_mut(upper)
        .unwrap()
        .set_geom(geom);
    tab.floating_panes.focus_pane(upper, client);
    let point = Position::new((geom.y + 1) as i32, (geom.x + 1) as u16);
    assert_eq!(
        tab.floating_panes.get_pane_id_at(&point, true).unwrap(),
        Some(upper)
    );
    tab.floating_panes
        .get_pane_mut(upper)
        .unwrap()
        .set_selectable(false);
    assert_eq!(
        tab.floating_panes.get_pane_id_at(&point, true).unwrap(),
        Some(lower)
    );
    assert_eq!(
        tab.floating_panes.get_pane_id_at(&point, false).unwrap(),
        Some(upper)
    );

    tab.set_floating_pane_pinned(lower, true);
    assert_eq!(
        tab.floating_panes
            .get_pinned_pane_id_at(&point, false)
            .unwrap(),
        Some(lower)
    );
    tab.set_floating_pane_pinned(upper, true);
    assert_eq!(
        tab.floating_panes
            .get_pinned_pane_id_at(&point, false)
            .unwrap(),
        Some(upper)
    );
    assert_eq!(
        tab.floating_panes
            .get_pinned_pane_id_at(&point, true)
            .unwrap(),
        Some(lower)
    );
    assert!(tab.floating_panes.has_pinned_pane_at(&point));
    let outside = Position::new(20, 121);
    assert_eq!(
        tab.floating_panes.get_pane_id_at(&outside, false).unwrap(),
        None
    );
    assert_eq!(
        tab.floating_panes
            .get_pinned_pane_id_at(&outside, false)
            .unwrap(),
        None
    );
    assert!(!tab.floating_panes.has_pinned_pane_at(&outside));
}

#[test]
fn correctly_resize_frameless_panes_on_pane_close() {
    // check that https://github.com/zellij-org/zellij/issues/1773 is fixed
    let cols = 60;
    let rows = 20;
    let size = Size { cols, rows };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    tab.set_pane_frames(PaneFrameStyle::None);

    // a single frameless pane should take up all available space
    let pane = tab
        .tiled_panes
        .panes
        .get(&PaneId::Terminal(crate::TerminalId::new(1)))
        .unwrap();
    let content_size = (pane.get_content_columns(), pane.get_content_rows());
    assert_eq!(content_size, (cols, rows));

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
    tab.close_pane(PaneId::Terminal(crate::TerminalId::new(2)), true, None);

    // the size should be the same after adding and then removing a pane
    let pane = tab
        .tiled_panes
        .panes
        .get(&PaneId::Terminal(crate::TerminalId::new(1)))
        .unwrap();
    let content_size = (pane.get_content_columns(), pane.get_content_rows());
    assert_eq!(content_size, (cols, rows));
}

#[test]
fn floating_pane_z_index_is_tracked() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = false;
    let mut tab = create_new_tab(size, stacked_resize);
    let _client_id = ClientId::new(1);

    // Create first floating pane (should_float = true means it will be a floating pane)
    tab.new_floating_pane(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        None,
        false,
        true,
        None,
        None,
    )
    .unwrap();

    // Create second floating pane
    tab.new_floating_pane(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        None,
        false,
        true,
        None,
        None,
    )
    .unwrap();

    // Verify z-indices exist and are different
    let z_index_pane2 = tab
        .floating_panes
        .get_pane_z_index(PaneId::Terminal(crate::TerminalId::new(2)));
    let z_index_pane3 = tab
        .floating_panes
        .get_pane_z_index(PaneId::Terminal(crate::TerminalId::new(3)));

    assert!(
        z_index_pane2.is_some(),
        "First floating pane should have a z-index"
    );
    assert!(
        z_index_pane3.is_some(),
        "Second floating pane should have a z-index"
    );
    assert_ne!(
        z_index_pane2, z_index_pane3,
        "Different panes should have different z-indices"
    );
}

#[test]
fn pinned_floating_pane_has_higher_z_index() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = false;
    let mut tab = create_new_tab(size, stacked_resize);
    let _client_id = ClientId::new(1);

    // Create first floating pane (will be unpinned)
    tab.new_floating_pane(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        None,
        false,
        true,
        None,
        None,
    )
    .unwrap();

    // Create second floating pane and pin it
    tab.new_floating_pane(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        None,
        false,
        true,
        None,
        None,
    )
    .unwrap();
    tab.set_floating_pane_pinned(PaneId::Terminal(crate::TerminalId::new(3)), true);

    // Get z-indices
    let z_index_unpinned = tab
        .floating_panes
        .get_pane_z_index(PaneId::Terminal(crate::TerminalId::new(2)))
        .expect("Unpinned pane should have z-index");
    let z_index_pinned = tab
        .floating_panes
        .get_pane_z_index(PaneId::Terminal(crate::TerminalId::new(3)))
        .expect("Pinned pane should have z-index");

    assert!(
        z_index_pinned > z_index_unpinned,
        "Pinned pane should have higher z-index than unpinned pane (pinned: {}, unpinned: {})",
        z_index_pinned,
        z_index_unpinned
    );
}

#[test]
fn pinned_pane_z_index_higher_than_regular_floating_panes() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = false;
    let mut tab = create_new_tab(size, stacked_resize);
    let _client_id = ClientId::new(1);

    // Create first floating pane
    tab.new_floating_pane(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        None,
        false,
        true,
        None,
        None,
    )
    .unwrap();

    // Create second floating pane
    tab.new_floating_pane(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        None,
        false,
        true,
        None,
        None,
    )
    .unwrap();

    // Pin the second pane so it's on top
    tab.set_floating_pane_pinned(PaneId::Terminal(crate::TerminalId::new(3)), true);

    // Verify that get_pane_z_index returns correct values for both panes
    let z_index_bottom = tab
        .floating_panes
        .get_pane_z_index(PaneId::Terminal(crate::TerminalId::new(2)));
    let z_index_top = tab
        .floating_panes
        .get_pane_z_index(PaneId::Terminal(crate::TerminalId::new(3)));

    assert!(
        z_index_bottom.is_some(),
        "Regular floating pane should have z-index"
    );
    assert!(
        z_index_top.is_some(),
        "Pinned floating pane should have z-index"
    );
    assert!(
        z_index_top.unwrap() > z_index_bottom.unwrap(),
        "Pinned pane should have higher z-index than regular floating pane (pinned: {}, regular: {})",
        z_index_top.unwrap(),
        z_index_bottom.unwrap()
    );
}

#[test]
fn active_pane_z_index_retrieved_for_cursor_visibility() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = false;
    let mut tab = create_new_tab(size, stacked_resize);
    let client_id = ClientId::new(1);

    // Start with tiled panes - active pane should not have z-index
    let active_pane_id_tiled = tab.get_active_pane_id(client_id).unwrap();
    let z_index_tiled = tab.floating_panes.get_pane_z_index(active_pane_id_tiled);
    assert!(
        z_index_tiled.is_none(),
        "Tiled pane should not have z-index in floating panes"
    );

    // Create a floating pane
    tab.new_floating_pane(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        None,
        false,
        true,
        None,
        None,
    )
    .unwrap();

    // Active pane should now have a z-index
    let active_pane_id_floating = tab.get_active_pane_id(client_id).unwrap();
    let z_index_floating = tab.floating_panes.get_pane_z_index(active_pane_id_floating);
    assert!(
        z_index_floating.is_some(),
        "Active floating pane should have a z-index"
    );
}

#[test]
fn get_pane_z_index_returns_none_for_nonexistent_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = false;
    let mut tab = create_new_tab(size, stacked_resize);
    let _client_id = ClientId::new(1);

    // Create two floating panes
    tab.new_floating_pane(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        None,
        false,
        true,
        None,
        None,
    )
    .unwrap();
    tab.new_floating_pane(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        None,
        false,
        true,
        None,
        None,
    )
    .unwrap();

    // Query for a pane that doesn't exist
    let z_index_nonexistent = tab
        .floating_panes
        .get_pane_z_index(PaneId::Terminal(crate::TerminalId::new(999)));

    assert!(
        z_index_nonexistent.is_none(),
        "Non-existent pane should return None for z-index"
    );

    // Query for existing panes
    let z_index_2 = tab
        .floating_panes
        .get_pane_z_index(PaneId::Terminal(crate::TerminalId::new(2)));
    let z_index_3 = tab
        .floating_panes
        .get_pane_z_index(PaneId::Terminal(crate::TerminalId::new(3)));

    assert!(
        z_index_2.is_some(),
        "Existing floating pane 2 should return Some for z-index"
    );
    assert!(
        z_index_3.is_some(),
        "Existing floating pane 3 should return Some for z-index"
    );
    assert_ne!(
        z_index_2, z_index_3,
        "Different floating panes should have different z-indices"
    );
}

#[test]
pub fn bell_in_unfocused_pane_sets_notification() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let client_id = ClientId::new(1);

    // Create a second pane; client is focused on pane 1 (PaneId::Terminal(TerminalId::new(1)))
    tab.horizontal_split(new_pane_id, None, client_id, None, None)
        .unwrap();
    // Move focus back to pane 1
    tab.move_focus_up(client_id).unwrap();

    // Simulate bell in pane 2 via pty bytes (\x07)
    tab.handle_pty_bytes(crate::TerminalId::new(2), vec![7u8])
        .unwrap();

    // Now call check_and_handle_bell_notifications as non-active tab
    let (new_panes, tab_newly_set) = tab.check_and_handle_bell_notifications(false);

    assert!(
        new_panes.contains(&new_pane_id),
        "Pane 2 should be in new_panes"
    );
    assert!(
        tab.panes_with_pending_bell.contains(&new_pane_id),
        "Pane 2 should be in panes_with_pending_bell"
    );
    assert!(
        tab.tab_has_pending_bell,
        "tab_has_pending_bell should be true"
    );
    assert!(tab_newly_set, "tab_bell_newly_set should be true");
    assert!(
        tab.get_pane_with_id(new_pane_id)
            .map(|p| p.get_bell_notification())
            .unwrap_or(false),
        "Pane 2 should have bell notification"
    );
}

#[test]
pub fn clearing_last_pane_bell_clears_tab_bell() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let client_id = ClientId::new(1);

    tab.horizontal_split(new_pane_id, None, client_id, None, None)
        .unwrap();
    tab.move_focus_up(client_id).unwrap();

    // Set bell on unfocused pane 2 via check_and_handle_bell_notifications
    tab.handle_pty_bytes(crate::TerminalId::new(2), vec![7u8])
        .unwrap();
    tab.check_and_handle_bell_notifications(false);

    assert!(
        tab.tab_has_pending_bell,
        "tab_has_pending_bell should be set before clearing"
    );

    // Clear bell for pane 2
    tab.clear_bell_notification_for_pane(new_pane_id);

    assert!(
        tab.panes_with_pending_bell.is_empty(),
        "panes_with_pending_bell should be empty after clearing"
    );
    assert!(
        !tab.tab_has_pending_bell,
        "tab_has_pending_bell should be false after last pane bell cleared"
    );
}

// Category 5: pane-id-based operations

#[test]
pub fn scroll_up_by_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    assert!(tab.has_pane_with_pid(&pane_id));
    tab.scroll_up_by_pane_id(pane_id);
}

#[test]
pub fn scroll_down_by_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    assert!(tab.has_pane_with_pid(&pane_id));
    let _ = tab.scroll_down_by_pane_id(pane_id);
}

#[test]
pub fn scroll_to_top_by_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    assert!(tab.has_pane_with_pid(&pane_id));
    let _ = tab.scroll_to_top_by_pane_id(pane_id);
}

#[test]
pub fn scroll_to_bottom_by_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    assert!(tab.has_pane_with_pid(&pane_id));
    let _ = tab.scroll_to_bottom_by_pane_id(pane_id);
}

#[test]
pub fn page_scroll_up_by_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    assert!(tab.has_pane_with_pid(&pane_id));
    tab.page_scroll_up_by_pane_id(pane_id);
}

#[test]
pub fn page_scroll_down_by_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    assert!(tab.has_pane_with_pid(&pane_id));
    let _ = tab.page_scroll_down_by_pane_id(pane_id);
}

#[test]
pub fn half_page_scroll_up_by_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    assert!(tab.has_pane_with_pid(&pane_id));
    tab.half_page_scroll_up_by_pane_id(pane_id);
}

#[test]
pub fn half_page_scroll_down_by_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    assert!(tab.has_pane_with_pid(&pane_id));
    let _ = tab.half_page_scroll_down_by_pane_id(pane_id);
}

#[test]
pub fn rename_pane_by_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    assert!(tab.has_pane_with_pid(&pane_id));
    let _ = tab.rename_pane_by_pane_id(pane_id, "new-name".as_bytes().to_vec());
}

#[test]
pub fn undo_rename_pane_by_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    assert!(tab.has_pane_with_pid(&pane_id));
    let _ = tab.rename_pane_by_pane_id(pane_id, "new-name".as_bytes().to_vec());
    tab.undo_rename_pane_by_pane_id(pane_id);
}

#[test]
pub fn explicitly_close_pane_by_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let stacked_resize = true;
    let mut tab = create_new_tab(size, stacked_resize);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    tab.horizontal_split(new_pane_id, None, crate::ClientId::new(1), None, None)
        .unwrap();
    assert_eq!(tab.tiled_panes.panes.len(), 2);
    assert_eq!(
        tab.close_pane_explicitly(new_pane_id, None),
        vec![new_pane_id]
    );
    assert_eq!(tab.tiled_panes.panes.len(), 1);
    assert!(!tab.has_pane_with_pid(&new_pane_id));
}

#[test]
pub fn resize_by_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    tab.horizontal_split(new_pane_id, None, crate::ClientId::new(1), None, None)
        .unwrap();
    assert_eq!(tab.tiled_panes.panes.len(), 2);
    tab.resize_by_pane_id(
        new_pane_id,
        ResizeStrategy::new(Resize::Increase, Some(Direction::Down)),
    );
}

#[test]
pub fn toggle_fullscreen_by_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    tab.horizontal_split(new_pane_id, None, crate::ClientId::new(1), None, None)
        .unwrap();
    assert_eq!(tab.tiled_panes.panes.len(), 2);
    tab.toggle_fullscreen_by_pane_id(new_pane_id);
}

#[test]
pub fn move_pane_by_pane_id_down() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    tab.horizontal_split(new_pane_id, None, crate::ClientId::new(1), None, None)
        .unwrap();
    assert_eq!(tab.tiled_panes.panes.len(), 2);
    tab.move_pane_by_pane_id(new_pane_id, Some(Direction::Down));
}

#[test]
pub fn move_pane_backwards_by_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let new_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    tab.horizontal_split(new_pane_id, None, crate::ClientId::new(1), None, None)
        .unwrap();
    assert_eq!(tab.tiled_panes.panes.len(), 2);
    tab.move_pane_backwards_by_pane_id(new_pane_id);
}

#[test]
pub fn clear_screen_by_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    assert!(tab.has_pane_with_pid(&pane_id));
    let _ = tab.clear_screen_by_pane_id(pane_id);
}

#[test]
pub fn toggle_pane_embed_or_floating_by_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    assert!(tab.has_pane_with_pid(&pane_id));
    let _ = tab.toggle_pane_embed_or_floating_for_pane_id(pane_id, None);
}

#[test]
pub fn toggle_pane_pinned_by_pane_id() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    assert!(tab.has_pane_with_pid(&pane_id));
    tab.toggle_pane_pinned_by_pane_id(pane_id);
}

#[test]
pub fn scroll_up_nonexistent_pane_id_does_not_panic() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, true);
    let pane_id = PaneId::Terminal(crate::TerminalId::new(999));
    assert!(!tab.has_pane_with_pid(&pane_id));
    tab.scroll_up_by_pane_id(pane_id);
}

/// A pane named by id that is in neither layer - suppressed behind another pane, as `hide_self`
/// and the scrollback editor leave one - is left alone. The tiled grid used to be asked to move
/// it, and unwrapped the position it did not have among the selectable panes, taking the screen
/// thread down.
#[test]
pub fn moving_a_suppressed_pane_by_id_does_nothing() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = crate::ClientId::new(1);
    let suppressed_pane_id = PaneId::Terminal(crate::TerminalId::new(2));
    let mut tab = create_new_tab(size, false);
    tab.horizontal_split(suppressed_pane_id, None, client_id, None, None)
        .unwrap();
    tab.new_floating_pane(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        None,
        false,
        true,
        None,
        None,
    )
    .unwrap();
    assert!(tab.floating_panes.panes_are_visible());
    tab.suppress_pane(suppressed_pane_id, None);
    assert!(
        tab.has_pane_with_pid(&suppressed_pane_id),
        "the tab still holds the pane"
    );
    assert!(!tab.tiled_panes.panes_contain(&suppressed_pane_id));
    assert!(!tab.floating_panes.panes_contain(&suppressed_pane_id));
    let geoms_before = tiled_pane_geoms(&tab);

    for pane_id in [
        suppressed_pane_id,
        PaneId::Terminal(crate::TerminalId::new(999)),
    ] {
        tab.move_pane(pane_id);
        tab.move_pane_backwards_by_pane_id(pane_id);
        tab.move_pane_by_pane_id(pane_id, None);
        tab.move_pane_by_pane_id(pane_id, Some(Direction::Down));
    }

    assert_eq!(
        tiled_pane_geoms(&tab),
        geoms_before,
        "nothing in the grid moved"
    );
}

/// The tiled grid itself refuses to move a pane it does not hold, rather than unwrapping the
/// position it would have had among the selectable panes.
#[test]
pub fn moving_an_unknown_pane_in_the_tiled_grid_does_nothing() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut tab = create_new_tab(size, false);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        crate::ClientId::new(1),
        None,
        None,
    )
    .unwrap();
    let unknown_pane_id = PaneId::Terminal(crate::TerminalId::new(999));
    let geoms_before = tiled_pane_geoms(&tab);

    tab.tiled_panes.move_pane(false, unknown_pane_id);
    tab.tiled_panes.move_pane(true, unknown_pane_id);

    assert_eq!(tiled_pane_geoms(&tab), geoms_before);
}

/// A tiled pane moved by id moves in the grid, even while floating panes are shown. The by-id
/// entry points used to choose the layer by whether floating panes were visible rather than by
/// where the pane is, so the move went to the floating grid, which did not have the pane and
/// did nothing.
#[test]
pub fn moving_a_tiled_pane_by_id_while_floating_panes_are_shown_moves_it_in_the_grid() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = crate::ClientId::new(1);
    let tiled_pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    let mut tab = create_new_tab(size, false);
    tab.horizontal_split(
        PaneId::Terminal(crate::TerminalId::new(2)),
        None,
        client_id,
        None,
        None,
    )
    .unwrap();
    tab.new_floating_pane(
        PaneId::Terminal(crate::TerminalId::new(3)),
        None,
        None,
        false,
        true,
        None,
        None,
    )
    .unwrap();
    assert!(tab.floating_panes.panes_are_visible());
    assert_eq!(
        tab.tiled_panes.get_pane(tiled_pane_id).unwrap().y(),
        0,
        "the first pane starts at the top"
    );

    tab.move_pane_down(tiled_pane_id);

    assert_eq!(
        tab.tiled_panes.get_pane(tiled_pane_id).unwrap().y(),
        10,
        "the first pane moved to the bottom"
    );
}

/// A tiled pane resized by id marks the tiled layout as changed, as a keyboard resize always
/// has. Without the mark the next swap-layout request discarded the layout the user had just
/// adjusted.
#[test]
pub fn resizing_a_tiled_pane_by_id_marks_the_tiled_layout_as_changed() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut tab = create_new_tab_with_layout(size, initial_layout);
    // the layout helper numbers its panes from zero
    let right_pane_id = PaneId::Terminal(crate::TerminalId::new(1));
    assert!(tab.tiled_panes.panes_contain(&right_pane_id));
    assert!(
        !tab.swap_layouts.is_tiled_damaged(),
        "a layout applied as given is not a change to it"
    );

    tab.resize_pane_with_id(
        ResizeStrategy::new(Resize::Increase, Some(Direction::Left)),
        right_pane_id,
    )
    .unwrap();

    assert!(tab.swap_layouts.is_tiled_damaged());
}

/// A floating pane resized by id marks only the floating layout as changed: the tiled grid was
/// not touched. It used to mark both.
#[test]
pub fn resizing_a_floating_pane_by_id_marks_only_the_floating_layout_as_changed() {
    let mut tab = create_tab_with_two_floating_panes();
    tab.swap_layouts.reset_floating_damage();
    assert!(!tab.swap_layouts.is_tiled_damaged());

    tab.resize_pane_with_id(
        ResizeStrategy::new(Resize::Increase, None),
        PaneId::Terminal(crate::TerminalId::new(3)),
    )
    .unwrap();

    assert!(tab.swap_layouts.is_floating_damaged());
    assert!(!tab.swap_layouts.is_tiled_damaged());
}
