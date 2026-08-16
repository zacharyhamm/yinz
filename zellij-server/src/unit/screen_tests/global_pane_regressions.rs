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
