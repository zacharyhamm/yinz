use super::*;

#[test]
fn borderless_floating_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(crate::TerminalId(2));
    let mut output = Output::default();

    // Create borderless floating pane
    let coordinates = FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(10)),
        y: Some(PercentOrFixed::Fixed(5)),
        width: Some(PercentOrFixed::Fixed(50)),
        height: Some(PercentOrFixed::Fixed(10)),
        pinned: None,
        borderless: Some(true),
    };

    tab.new_floating_pane(
        new_pane_id,
        None,
        None,
        false,
        true,
        Some(coordinates),
        None,
    )
    .unwrap();

    tab.handle_pty_bytes(
        crate::TerminalId(2),
        Vec::from("\n\n\n                   I am borderless".as_bytes()),
    )
    .unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot);
}

#[test]
fn borderless_pane_content_fills_edges() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let new_pane_id = PaneId::Terminal(crate::TerminalId(2));
    let mut output = Output::default();

    let coordinates = FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(10)),
        y: Some(PercentOrFixed::Fixed(5)),
        width: Some(PercentOrFixed::Fixed(30)),
        height: Some(PercentOrFixed::Fixed(5)),
        pinned: None,
        borderless: Some(true),
    };

    tab.new_floating_pane(
        new_pane_id,
        None,
        None,
        false,
        true,
        Some(coordinates),
        None,
    )
    .unwrap();

    // Fill with X's to verify content reaches edges
    tab.handle_pty_bytes(
        crate::TerminalId(2),
        Vec::from("XXXXXXXXXXXXXXXXXXXXXXXXXXXXXX\r\nXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX\r\nXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX\r\nXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX\r\nXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX".as_bytes()),
    )
    .unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot);
}

#[test]
fn borderless_pinned_floating_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let borderless_pane_id = PaneId::Terminal(crate::TerminalId(2));
    let mut output = Output::default();

    let coordinates = FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(30)),
        y: Some(PercentOrFixed::Fixed(5)),
        width: Some(PercentOrFixed::Fixed(60)),
        height: Some(PercentOrFixed::Fixed(8)),
        pinned: Some(true),
        borderless: Some(true),
    };

    tab.new_floating_pane(
        borderless_pane_id,
        None,
        None,
        false,
        true,
        Some(coordinates),
        None,
    )
    .unwrap();

    // Toggle floating panes off to test pinned behavior
    tab.toggle_floating_panes(Some(client_id), None, None)
        .unwrap();

    tab.handle_pty_bytes(
        crate::TerminalId(1),
        Vec::from("Content behind pinned pane".as_bytes()),
    )
    .unwrap();

    tab.handle_pty_bytes(
        crate::TerminalId(2),
        Vec::from("\n\nPinned borderless pane".as_bytes()),
    )
    .unwrap();

    tab.render(&mut output, None).unwrap();
    let snapshot = take_snapshot(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert_snapshot!(snapshot);
}

#[test]
fn cursor_hidden_when_floating_pane_is_under_pinned_pane() {
    // Test that cursor is correctly hidden when focused on a floating pane that's under a pinned pane
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let mut output = Output::default();

    // Create first floating pane (will be underneath)
    let bottom_pane_coordinates = FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(10)),
        y: Some(PercentOrFixed::Fixed(5)),
        width: Some(PercentOrFixed::Fixed(40)),
        height: Some(PercentOrFixed::Fixed(10)),
        pinned: Some(false),
        borderless: Some(false),
    };

    tab.new_floating_pane(
        PaneId::Terminal(crate::TerminalId(2)),
        None,
        None,
        false,
        true,
        Some(bottom_pane_coordinates),
        None,
    )
    .unwrap();

    // Create overlapping pinned pane on top
    let top_pane_coordinates = FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(15)),
        y: Some(PercentOrFixed::Fixed(6)),
        width: Some(PercentOrFixed::Fixed(30)),
        height: Some(PercentOrFixed::Fixed(8)),
        pinned: Some(true),
        borderless: Some(false),
    };

    tab.new_floating_pane(
        PaneId::Terminal(crate::TerminalId(3)),
        None,
        None,
        false,
        true,
        Some(top_pane_coordinates),
        None,
    )
    .unwrap();

    // Add some text to both panes so we can see them
    tab.handle_pty_bytes(
        crate::TerminalId(2),
        Vec::from("Bottom floating pane".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId(3),
        Vec::from("Top pinned pane".as_bytes()),
    )
    .unwrap();

    // Explicitly focus the bottom floating pane (PaneId::Terminal(TerminalId(2)))
    // Use move_focus_left to switch from pane 3 to pane 2
    tab.move_focus_left(client_id).unwrap();

    // Render
    tab.render(&mut output, None).unwrap();
    let (_snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    // The cursor should be hidden because the focused pane (2) is under the pinned pane (3)
    // and the cursor position falls within the overlapping area
    assert_eq!(
        cursor_coordinates, None,
        "Cursor should be hidden when focused floating pane is under a pinned pane that covers it"
    );
}

#[test]
fn hidden_panes_do_not_inflate_overlay_cursor_z_index() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let mut output = Output::default();
    let hidden_pane_id = PaneId::Terminal(crate::TerminalId(2));
    let overlay_pane_id = PaneId::Terminal(crate::TerminalId(3));
    let pinned_pane_id = PaneId::Terminal(crate::TerminalId(4));
    let coordinates = FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(10)),
        y: Some(PercentOrFixed::Fixed(5)),
        width: Some(PercentOrFixed::Fixed(30)),
        height: Some(PercentOrFixed::Fixed(8)),
        ..Default::default()
    };

    tab.new_floating_pane(
        hidden_pane_id,
        None,
        None,
        false,
        true,
        Some(coordinates.clone()),
        None,
    )
    .unwrap();
    tab.new_pane(
        overlay_pane_id,
        None,
        None,
        false,
        true,
        NewPanePlacement::Overlay(OverlayPlacement::dismissible(Some(
            FloatingPaneCoordinates {
                ..coordinates.clone()
            },
        ))),
        Some(client_id),
        None,
    )
    .unwrap();
    tab.new_floating_pane(
        pinned_pane_id,
        None,
        None,
        false,
        true,
        Some(FloatingPaneCoordinates {
            pinned: Some(true),
            ..coordinates
        }),
        None,
    )
    .unwrap();
    tab.hide_floating_panes();

    // the hidden pane is in neither occlusion stack, so it cannot push anything drawn above it
    // further down the flat list `Output` clips against
    assert_eq!(
        tab.floating_panes.rendered_pane_ids(),
        vec![pinned_pane_id],
        "a hidden floating pane is not drawn; the pinned one still is"
    );
    assert_eq!(tab.floating_panes.get_pane_z_index(hidden_pane_id), None);
    assert_eq!(tab.floating_panes.get_pane_z_index(pinned_pane_id), Some(0));
    assert_eq!(tab.overlay_panes.rendered_pane_ids(), vec![overlay_pane_id]);
    assert_eq!(tab.overlay_panes.get_pane_z_index(overlay_pane_id), Some(0));

    tab.render(&mut output, None).unwrap();
    let (_, cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );
    assert!(
        cursor_coordinates.is_some(),
        "the overlay is drawn above the pinned pane that covers it, so its cursor is visible"
    );
}

#[test]
fn cursor_visible_when_pinned_pane_is_focused() {
    // Test that cursor is visible when focused on a pinned pane that's on top
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId(1);
    let mut tab = create_new_tab(size, ModeInfo::default());
    let mut output = Output::default();

    // Create first floating pane (will be underneath)
    let bottom_pane_coordinates = FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(10)),
        y: Some(PercentOrFixed::Fixed(5)),
        width: Some(PercentOrFixed::Fixed(40)),
        height: Some(PercentOrFixed::Fixed(10)),
        pinned: Some(false),
        borderless: Some(false),
    };

    tab.new_floating_pane(
        PaneId::Terminal(crate::TerminalId(2)),
        None,
        None,
        false,
        true,
        Some(bottom_pane_coordinates),
        None,
    )
    .unwrap();

    // Create overlapping pinned pane on top
    let top_pane_coordinates = FloatingPaneCoordinates {
        x: Some(PercentOrFixed::Fixed(15)),
        y: Some(PercentOrFixed::Fixed(6)),
        width: Some(PercentOrFixed::Fixed(30)),
        height: Some(PercentOrFixed::Fixed(8)),
        pinned: Some(true),
        borderless: Some(false),
    };

    tab.new_floating_pane(
        PaneId::Terminal(crate::TerminalId(3)),
        None,
        None,
        false,
        true,
        Some(top_pane_coordinates),
        None,
    )
    .unwrap();

    // Add some text to both panes
    tab.handle_pty_bytes(
        crate::TerminalId(2),
        Vec::from("Bottom floating pane".as_bytes()),
    )
    .unwrap();
    tab.handle_pty_bytes(
        crate::TerminalId(3),
        Vec::from("Top pinned pane".as_bytes()),
    )
    .unwrap();

    // The pinned pane should be focused by default (last created)
    // Render
    tab.render(&mut output, None).unwrap();
    let (_snapshot, cursor_coordinates) = take_snapshot_and_cursor_position(
        output.serialize().unwrap().get(&client_id).unwrap(),
        size.rows,
        size.cols,
        Palette::default(),
    );

    // The cursor should be visible because the focused pane is the pinned pane on top
    assert!(
        cursor_coordinates.is_some(),
        "Cursor should be visible when focused on pinned pane that's on top"
    );
}
