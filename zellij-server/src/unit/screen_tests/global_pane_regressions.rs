use super::*;

#[test]
fn gp01_fixed_size_global_pane_keeps_rendering_when_terminal_shrinks() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Left, 12);
    screen.render_to_clients().expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().get_display_area_origin(),
        (12, 0)
    );

    let resize_result = screen.resize_to_screen(Size { cols: 12, rows: 10 });
    assert!(
        resize_result.is_ok(),
        "shrinking the terminal to the global pane width failed: {:?}",
        resize_result.err().map(|e| format!("{:#}", e))
    );
    let render_result = screen.render_to_clients();
    assert!(
        render_result.is_ok(),
        "rendering after the shrink failed: {:?}",
        render_result.err().map(|e| format!("{:#}", e))
    );

    screen
        .resize_to_screen(Size { cols: 40, rows: 10 })
        .expect("TEST");
    screen.render_to_clients().expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().get_display_area_origin(),
        (12, 0)
    );
    assert_eq!(
        screen.tabs.get(&0).unwrap().get_display_area(),
        Size { cols: 28, rows: 10 }
    );
}

#[test]
fn gp01_percent_global_pane_keeps_rendering_when_thickness_rounds_to_zero() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    gp_insert_global_pane(
        &mut screen,
        99,
        GlobalPaneEdge::Left,
        PercentOrFixed::Percent(20),
        false,
    );
    screen.render_to_clients().expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().get_display_area_origin(),
        (8, 0)
    );

    // 20% of 4 columns rounds down to zero, which the allocator rejects
    let resize_result = screen.resize_to_screen(Size { cols: 4, rows: 10 });
    assert!(
        resize_result.is_ok(),
        "shrinking until the percentage rounds to zero failed: {:?}",
        resize_result.err().map(|e| format!("{:#}", e))
    );
    let render_result = screen.render_to_clients();
    assert!(
        render_result.is_ok(),
        "rendering after the shrink failed: {:?}",
        render_result.err().map(|e| format!("{:#}", e))
    );
}

#[test]
fn gp02_moved_floating_pane_keeps_position_across_noop_reflows_beside_left_global_pane() {
    use zellij_utils::data::GlobalPaneEdge;
    use zellij_utils::pane_size::Viewport;

    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Left, 10);
    screen.render_to_clients().expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().get_display_area_origin(),
        (10, 0)
    );

    {
        let tab = screen.get_active_tab_mut(1).unwrap();
        tab.new_floating_pane(PaneId::Terminal(2), None, None, false, true, None, None)
            .expect("TEST");
        tab.change_floating_pane_coordinates(
            &PaneId::Terminal(2),
            FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(10)),
                y: Some(PercentOrFixed::Fixed(5)),
                ..Default::default()
            },
        )
        .expect("TEST");
    }
    let before = screen
        .tabs
        .get(&0)
        .unwrap()
        .get_pane_with_id(PaneId::Terminal(2))
        .unwrap()
        .position_and_size();
    assert_eq!(before.x, 20, "moved pane should sit at tab-local x=10");

    let display_area = Viewport {
        x: 10,
        y: 0,
        cols: 111,
        rows: 20,
    };
    for iteration in 0..3 {
        screen
            .tabs
            .get_mut(&0)
            .unwrap()
            .resize_whole_tab_to(display_area)
            .expect("TEST");
        let after = screen
            .tabs
            .get(&0)
            .unwrap()
            .get_pane_with_id(PaneId::Terminal(2))
            .unwrap()
            .position_and_size();
        assert_eq!(
            after.x,
            before.x,
            "floating pane x drifted on no-op reflow {}",
            iteration + 1
        );
        assert_eq!(
            after.y,
            before.y,
            "floating pane y drifted on no-op reflow {}",
            iteration + 1
        );
    }
}

/// The characters of row `y`, columns `x_range`, as the client's terminal holds them.
fn row_text(grid: &Grid, y: usize, x_range: std::ops::Range<usize>) -> String {
    x_range
        .map(|x| grid.viewport[y].columns[x].character)
        .collect()
}

#[test]
fn gp03_floating_pane_dragged_off_a_global_pane_leaves_no_ghost() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);

    let mut global_pane = PaneFactory::terminal(
        99,
        Default::default(),
        screen.style,
        3,
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
    global_pane.handle_pty_bytes(b"GLOBAL".to_vec());
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

    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));

    // every frame is applied to the same grid, exactly as a real client's terminal would - a cell
    // nobody repaints keeps whatever the previous frame left on it, which is the ghost
    let mut client_grid = grid_for_render_snapshot(size);
    let render_frame = |screen: &mut Screen, client_grid: &mut Grid| {
        screen.render_to_clients().expect("TEST");
        while let Ok((instruction, _)) = server_receiver.try_recv() {
            if let ServerInstruction::Render(Some(mut rendered_clients)) = instruction {
                if let Some(rendered) = rendered_clients.remove(&1) {
                    take_snapshot_and_cursor_coordinates(&rendered, client_grid);
                }
            }
        }
    };

    render_frame(&mut screen, &mut client_grid);
    assert_eq!(
        row_text(&client_grid, 0, 0..6),
        "GLOBAL",
        "the global pane should have painted before anything covered it"
    );

    // an overlay floating pane is positioned against the whole session surface, so this puts it
    // squarely over the global pane's columns
    {
        let tab = screen.get_active_tab_mut(1).unwrap();
        tab.new_floating_pane(PaneId::Terminal(2), None, None, false, true, None, None)
            .expect("TEST");
        tab.change_floating_pane_coordinates(
            &PaneId::Terminal(2),
            FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(0)),
                y: Some(PercentOrFixed::Fixed(0)),
                width: Some(PercentOrFixed::Fixed(8)),
                height: Some(PercentOrFixed::Fixed(5)),
                overlay: Some(true),
                ..Default::default()
            },
        )
        .expect("TEST");
    }
    render_frame(&mut screen, &mut client_grid);
    assert_ne!(
        row_text(&client_grid, 0, 0..6),
        "GLOBAL",
        "the floating pane should be covering the global pane"
    );

    // drag it off the global pane, back over the tab
    screen
        .get_active_tab_mut(1)
        .unwrap()
        .change_floating_pane_coordinates(
            &PaneId::Terminal(2),
            FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(20)),
                y: Some(PercentOrFixed::Fixed(0)),
                width: Some(PercentOrFixed::Fixed(8)),
                height: Some(PercentOrFixed::Fixed(5)),
                overlay: Some(true),
                ..Default::default()
            },
        )
        .expect("TEST");
    render_frame(&mut screen, &mut client_grid);
    assert_eq!(
        row_text(&client_grid, 0, 0..6),
        "GLOBAL",
        "the global pane did not repaint the cells the floating pane vacated"
    );
}
