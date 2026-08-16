use super::*;

#[test]
fn focus_cycle_walks_through_global_panes_and_back_into_the_tab() {
    let client_id = 1;
    let mut screen = screen_with_two_tiled_panes_and_a_global_pane();
    let global_pane_id = PaneId::Terminal(99);
    assert_eq!(
        screen.get_active_pane_id(&client_id),
        Some(PaneId::Terminal(2))
    );

    // the global panes form a segment appended to the end of the tab's cycle
    screen.cycle_focus(client_id, true);
    assert_eq!(screen.get_active_pane_id(&client_id), Some(global_pane_id));
    screen.cycle_focus(client_id, true);
    assert_eq!(
        screen.get_active_pane_id(&client_id),
        Some(PaneId::Terminal(1)),
        "cycling past the last global pane should wrap back into the tab"
    );
    screen.cycle_focus(client_id, true);
    assert_eq!(
        screen.get_active_pane_id(&client_id),
        Some(PaneId::Terminal(2))
    );

    // and backwards is the exact mirror
    screen.cycle_focus(client_id, false);
    assert_eq!(
        screen.get_active_pane_id(&client_id),
        Some(PaneId::Terminal(1))
    );
    screen.cycle_focus(client_id, false);
    assert_eq!(screen.get_active_pane_id(&client_id), Some(global_pane_id));
    screen.cycle_focus(client_id, false);
    assert_eq!(
        screen.get_active_pane_id(&client_id),
        Some(PaneId::Terminal(2))
    );
}

#[test]
fn move_focus_crosses_into_and_out_of_a_left_global_pane() {
    let client_id = 1;
    let mut screen = screen_with_two_tiled_panes_and_a_global_pane();
    let global_pane_id = PaneId::Terminal(99);

    // travelling right lands on the leftmost pane, so it has nowhere left to go inside the tab
    screen
        .get_active_tab_mut(client_id)
        .unwrap()
        .focus_pane_on_edge(Direction::Right, client_id);
    let leftmost_pane_id = screen.get_active_pane_id(&client_id).unwrap();
    let moved = screen
        .get_active_tab_mut(client_id)
        .unwrap()
        .move_focus_left(client_id)
        .unwrap();
    assert!(!moved, "expected to be at the tab's left edge");

    assert!(screen.enter_global_pane_in_direction(Direction::Left, client_id));
    assert_eq!(screen.get_active_pane_id(&client_id), Some(global_pane_id));

    // moving further left is the session boundary - focus stays put
    assert!(screen.global_pane_handles_direction(Direction::Left, client_id));
    assert_eq!(screen.get_active_pane_id(&client_id), Some(global_pane_id));

    // moving right returns to the tab pane nearest the global pane
    assert!(screen.global_pane_handles_direction(Direction::Right, client_id));
    assert_eq!(
        screen.get_active_pane_id(&client_id),
        Some(leftmost_pane_id)
    );
}

#[test]
fn move_focus_walks_within_a_strip_then_hops_to_another_edge() {
    use zellij_utils::data::GlobalPaneEdge;

    let client_id = 1;
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 12 });
    new_tab(&mut screen, 1, 0);
    gp_insert_global_pane(
        &mut screen,
        98,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(8),
        false,
    );
    gp_insert_global_pane(
        &mut screen,
        99,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(8),
        false,
    );
    gp_insert_global_pane(
        &mut screen,
        97,
        GlobalPaneEdge::Top,
        PercentOrFixed::Fixed(2),
        false,
    );
    screen.render_to_clients().expect("TEST");

    screen
        .focus_global_pane(PaneId::Terminal(98), client_id)
        .unwrap();

    // down walks the strip to the same-edge neighbor
    assert!(screen.global_pane_handles_direction(Direction::Down, client_id));
    assert_eq!(
        screen.get_active_pane_id(&client_id),
        Some(PaneId::Terminal(99))
    );

    // down again is the strip's end and no bottom pane exists - focus stays
    assert!(screen.global_pane_handles_direction(Direction::Down, client_id));
    assert_eq!(
        screen.get_active_pane_id(&client_id),
        Some(PaneId::Terminal(99))
    );

    // up walks back
    assert!(screen.global_pane_handles_direction(Direction::Up, client_id));
    assert_eq!(
        screen.get_active_pane_id(&client_id),
        Some(PaneId::Terminal(98))
    );

    // up off the strip's end hops to the pane on the top edge, as it did
    // before splits existed
    assert!(screen.global_pane_handles_direction(Direction::Up, client_id));
    assert_eq!(
        screen.get_active_pane_id(&client_id),
        Some(PaneId::Terminal(97))
    );
}

#[test]
fn entering_a_split_strip_lands_on_the_overlapping_pane() {
    use zellij_utils::data::GlobalPaneEdge;

    let client_id = 1;
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 12 });
    new_tab(&mut screen, 1, 0);
    gp_insert_global_pane(
        &mut screen,
        98,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(8),
        false,
    );
    gp_insert_global_pane(
        &mut screen,
        99,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(8),
        false,
    );
    // two tab panes stacked on top of each other
    screen
        .get_active_tab_mut(client_id)
        .unwrap()
        .new_pane(
            PaneId::Terminal(2),
            None,
            None,
            false,
            true,
            NewPanePlacement::Tiled {
                direction: Some(Direction::Down),
                borderless: None,
            },
            Some(client_id),
            None,
        )
        .unwrap();
    screen.render_to_clients().expect("TEST");

    // whichever tab pane overlaps a strip pane's span is the one a left move
    // from it should land on
    let strip_pane_containing = |screen: &Screen, row: usize| {
        [98u32, 99u32]
            .iter()
            .copied()
            .find(|id| {
                let geom = screen
                    .global_panes
                    .pane(PaneId::Terminal(*id))
                    .unwrap()
                    .position_and_size();
                row >= geom.y && row < geom.y + geom.rows.as_usize()
            })
            .expect("a strip pane spans this row")
    };
    for tab_pane in [1u32, 2u32] {
        screen
            .get_active_tab_mut(client_id)
            .unwrap()
            .focus_pane_with_id(PaneId::Terminal(tab_pane), false, false, client_id)
            .unwrap();
        let geom = screen
            .get_active_tab(client_id)
            .unwrap()
            .get_active_pane(client_id)
            .unwrap()
            .position_and_size();
        let expected = strip_pane_containing(&screen, geom.y + geom.rows.as_usize() / 2);
        assert!(screen.enter_global_pane_in_direction(Direction::Left, client_id));
        assert_eq!(
            screen.get_active_pane_id(&client_id),
            Some(PaneId::Terminal(expected)),
            "the move lands on the strip pane beside tab pane {tab_pane}"
        );
        screen.defocus_global_pane(client_id);
    }
}

#[test]
fn hiding_a_strip_pane_hides_the_whole_edge_and_showing_restores_it() {
    use zellij_utils::data::GlobalPaneEdge;

    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 12 });
    new_tab(&mut screen, 1, 0);
    gp_insert_global_pane(
        &mut screen,
        98,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(8),
        false,
    );
    gp_insert_global_pane(
        &mut screen,
        99,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(8),
        false,
    );
    screen.render_to_clients().expect("TEST");

    // hiding either pane hides the strip - the sidebar and its agent list go
    // together
    screen.hide_pane(PaneId::Terminal(98)).expect("TEST");
    assert!(screen.global_panes.visible_pane_ids().is_empty());

    // showing either restores both, split geometry included
    screen.show_pane(PaneId::Terminal(99)).expect("TEST");
    assert_eq!(screen.global_panes.visible_pane_ids().len(), 2);
    let first = screen
        .global_panes
        .pane(PaneId::Terminal(98))
        .unwrap()
        .position_and_size();
    let second = screen
        .global_panes
        .pane(PaneId::Terminal(99))
        .unwrap()
        .position_and_size();
    assert_eq!(second.y, first.y + first.rows.as_usize());
}

#[test]
fn restoring_a_strip_leaves_independently_hidden_panes_hidden() {
    use zellij_utils::data::GlobalPaneEdge;

    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 12 });
    new_tab(&mut screen, 1, 0);
    gp_insert_global_pane(
        &mut screen,
        98,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(8),
        false,
    );
    gp_try_insert_global_pane(
        &mut screen,
        99,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(8),
        false,
        true,
    )
    .expect("TEST");
    screen.render_to_clients().expect("TEST");

    screen.hide_pane(PaneId::Terminal(98)).expect("TEST");
    screen.show_pane(PaneId::Terminal(98)).expect("TEST");

    assert_eq!(
        screen.global_panes.visible_pane_ids(),
        HashSet::from([PaneId::Terminal(98)])
    );
    assert!(screen.global_panes.is_hidden(PaneId::Terminal(99)).unwrap());

    // Asking for the independently hidden pane reveals it too, even though
    // restoring its strip also reveals a sibling.
    screen.hide_pane(PaneId::Terminal(98)).expect("TEST");
    screen.show_pane(PaneId::Terminal(99)).expect("TEST");
    assert_eq!(
        screen.global_panes.visible_pane_ids(),
        HashSet::from([PaneId::Terminal(98), PaneId::Terminal(99)])
    );
}

#[test]
fn split_resizing_uses_non_selectable_physical_neighbors() {
    use zellij_utils::data::{GlobalPaneEdge, Resize, ResizeStrategy};

    let client_id: ClientId = 1;
    let mut screen = create_non_mirrored_screen(Size {
        cols: 40,
        rows: 100,
    });
    new_tab(&mut screen, 1, 0);
    for id in [98, 99, 100] {
        gp_insert_global_pane(
            &mut screen,
            id,
            GlobalPaneEdge::Left,
            PercentOrFixed::Fixed(8),
            false,
        );
    }
    screen
        .global_panes
        .pane_mut(PaneId::Terminal(99))
        .unwrap()
        .set_selectable(false);
    screen.render_to_clients().expect("TEST");

    let rows = |screen: &Screen, id| {
        screen
            .global_panes
            .pane(PaneId::Terminal(id))
            .unwrap()
            .position_and_size()
            .rows
            .as_usize()
    };
    let before = [rows(&screen, 98), rows(&screen, 99), rows(&screen, 100)];
    screen
        .resize_pane_with_id(
            ResizeStrategy::new(Resize::Increase, Some(Direction::Down)),
            PaneId::Terminal(98),
        )
        .unwrap();
    assert_eq!(rows(&screen, 98), before[0] + 5);
    assert_eq!(rows(&screen, 99), before[1] - 5);
    assert_eq!(rows(&screen, 100), before[2]);

    let middle = screen
        .global_panes
        .pane(PaneId::Terminal(99))
        .unwrap()
        .position_and_size();
    let split_line = (middle.y + middle.rows.as_usize() - 1) as i32;
    let split_column = (middle.x + 1) as u16;
    let middle_before_drag = rows(&screen, 99);
    let last_before_drag = rows(&screen, 100);
    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(split_line, split_column)),
        client_id,
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_motion_event(Position::new(split_line - 1, split_column)),
        client_id,
    );
    assert_eq!(rows(&screen, 99), middle_before_drag - 1);
    assert_eq!(rows(&screen, 100), last_before_drag + 1);
}

#[test]
fn closing_a_strip_pane_refocuses_its_neighbor() {
    use zellij_utils::data::GlobalPaneEdge;

    let client_id = 1;
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 12 });
    new_tab(&mut screen, 1, 0);
    gp_insert_global_pane(
        &mut screen,
        98,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(8),
        false,
    );
    gp_insert_global_pane(
        &mut screen,
        99,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(8),
        false,
    );
    screen.render_to_clients().expect("TEST");
    screen
        .focus_global_pane(PaneId::Terminal(99), client_id)
        .unwrap();

    screen
        .close_global_pane(PaneId::Terminal(99), None)
        .expect("TEST");
    assert_eq!(
        screen.get_active_pane_id(&client_id),
        Some(PaneId::Terminal(98)),
        "the client lands on the pane that inherited the closed pane's cells"
    );
}

#[test]
fn a_new_pane_on_a_focused_global_pane_splits_it() {
    use zellij_utils::data::GlobalPaneEdge;

    let client_id = 1;
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 12 });
    new_tab(&mut screen, 1, 0);
    gp_insert_global_pane(
        &mut screen,
        98,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(8),
        false,
    );
    screen.render_to_clients().expect("TEST");
    screen
        .focus_global_pane(PaneId::Terminal(98), client_id)
        .unwrap();

    let handled = screen
        .split_focused_global_pane(
            PaneId::Terminal(98),
            PaneId::Terminal(50),
            None,
            None,
            None,
            &NewPanePlacement::default(),
            true,
            client_id,
            &mut None,
        )
        .expect("TEST");
    assert!(handled);
    let anchor = screen
        .global_panes
        .pane(PaneId::Terminal(98))
        .unwrap()
        .position_and_size();
    let split = screen
        .global_panes
        .pane(PaneId::Terminal(50))
        .unwrap()
        .position_and_size();
    assert_eq!(split.cols.as_usize(), anchor.cols.as_usize());
    assert_eq!(
        split.y,
        anchor.y + anchor.rows.as_usize(),
        "NoPreference splits after the anchor"
    );
    assert_eq!(
        screen.get_active_pane_id(&client_id),
        Some(PaneId::Terminal(50))
    );

    // a direction across the strip is rejected: the pane never lands anywhere
    let handled = screen
        .split_focused_global_pane(
            PaneId::Terminal(50),
            PaneId::Terminal(51),
            None,
            None,
            None,
            &NewPanePlacement::Tiled {
                direction: Some(Direction::Right),
                borderless: None,
            },
            true,
            client_id,
            &mut None,
        )
        .expect("TEST");
    assert!(handled, "consumed, not passed through to the tab");
    assert!(screen.global_panes.pane(PaneId::Terminal(51)).is_none());

    // an up-split lands before the anchor
    let handled = screen
        .split_focused_global_pane(
            PaneId::Terminal(50),
            PaneId::Terminal(52),
            None,
            None,
            None,
            &NewPanePlacement::Tiled {
                direction: Some(Direction::Up),
                borderless: None,
            },
            true,
            client_id,
            &mut None,
        )
        .expect("TEST");
    assert!(handled);
    let above = screen
        .global_panes
        .pane(PaneId::Terminal(52))
        .unwrap()
        .position_and_size();
    let below = screen
        .global_panes
        .pane(PaneId::Terminal(50))
        .unwrap()
        .position_and_size();
    assert!(above.y < below.y, "an up-split sits before its anchor");
}

#[test]
fn a_blocking_global_pane_split_reports_an_insertion_failure() {
    use zellij_utils::data::GlobalPaneEdge;

    let client_id = 1;
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 1 });
    new_tab(&mut screen, 1, 0);
    gp_insert_global_pane(
        &mut screen,
        98,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(8),
        false,
    );
    screen.render_to_clients().expect("TEST");
    let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
    let mut blocking_notification = Some(NotificationEnd::new(completion_tx));

    let handled = screen
        .split_focused_global_pane(
            PaneId::Terminal(98),
            PaneId::Terminal(50),
            None,
            None,
            None,
            &NewPanePlacement::default(),
            true,
            client_id,
            &mut blocking_notification,
        )
        .expect("TEST");
    let completion = completion_rx.blocking_recv().expect("completion");

    assert!(handled);
    assert_eq!(completion.exit_status, Some(1));
    assert_eq!(
        completion.error_message.as_deref(),
        Some("global panes do not fit their strip")
    );
    assert!(screen.global_panes.pane(PaneId::Terminal(50)).is_none());
}

#[test]
fn adding_a_tiled_pane_with_swap_layouts_stays_beside_a_left_global_pane() {
    // with swap layouts the new pane goes through the layout applier rather than the constraint
    // system, which positions panes in the tab's display area from scratch
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    use std::collections::BTreeMap;
    use zellij_utils::input::layout::LayoutConstraint;

    let swap_tiled_layouts = vec![(
        BTreeMap::from([(LayoutConstraint::NoConstraint, TiledPaneLayout::default())]),
        Some("swap".to_owned()),
    )];
    screen
        .new_tab(0, (swap_tiled_layouts, vec![]), None, Some(1))
        .unwrap();
    screen
        .apply_layout(
            TiledPaneLayout::default(),
            vec![],
            vec![(1, None)],
            vec![],
            HashMap::new(),
            0,
            true,
            (1, false),
            None,
        )
        .unwrap();
    insert_left_global_pane(&mut screen, 10);
    screen
        .resize_to_screen(Size { cols: 50, rows: 12 })
        .unwrap();

    screen
        .get_active_tab_mut(1)
        .unwrap()
        .new_pane(
            PaneId::Terminal(2),
            None,
            None,
            false,
            true,
            NewPanePlacement::default(),
            Some(1),
            None,
        )
        .unwrap();

    assert_tiled_panes_are_bounded_by_left_global_pane(&screen, 10);
}

#[test]
fn screen_resizes_tabs_directly_to_the_global_center_without_accumulating_origin() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    let global_pane = PaneFactory::terminal(
        99,
        Default::default(),
        screen.style,
        1,
        "global".to_owned(),
        Rc::new(RefCell::new(LinkHandler::new())),
        screen.character_cell_size.clone(),
        screen.sixel_image_store.clone(),
        screen.kitty_image_store.clone(),
        screen.terminal_emulator_colors.clone(),
        screen.terminal_emulator_color_codes.clone(),
        Some("global".to_owned()),
        None,
        false,
        true,
        true,
        true,
        false,
        None,
    );
    screen
        .global_panes
        .insert(
            global_pane,
            GlobalPanePlacement {
                edge: GlobalPaneEdge::Left,
                size: PercentOrFixed::Fixed(10),
                length: None,
            },
            false,
        )
        .unwrap();

    screen
        .resize_to_screen(Size { cols: 50, rows: 12 })
        .unwrap();
    let tab = screen.tabs.get(&0).unwrap();
    assert_eq!(tab.get_display_area_origin(), (10, 0));
    assert_eq!(tab.get_display_area(), Size { cols: 40, rows: 12 });
    assert_eq!(tab.get_pane_with_id(PaneId::Terminal(1)).unwrap().x(), 10);

    screen
        .resize_to_screen(Size { cols: 60, rows: 14 })
        .unwrap();
    let tab = screen.tabs.get(&0).unwrap();
    assert_eq!(tab.get_display_area_origin(), (10, 0));
    assert_eq!(tab.get_display_area(), Size { cols: 50, rows: 14 });
    assert_eq!(tab.get_pane_with_id(PaneId::Terminal(1)).unwrap().x(), 10);
}
