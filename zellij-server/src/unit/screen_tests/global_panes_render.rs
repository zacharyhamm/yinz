use super::*;

/// A render tick can land before any client has reported a size: the screen thread is up as
/// soon as the session is built, and the debounced render job fires on its own thread while the
/// first `RecomputeTabSize` is still in flight. `surface_size` used to bottom out in
/// `Size::default()` there, and `allocate_global_panes` rejects a zero-size area even for a
/// session that declares no global panes at all - so that one tick killed the screen thread with
/// `global panes leave no usable tab area`, and every test sharing the process failed as an
/// unrelated timeout.
#[test]
fn rendering_before_any_client_size_is_known_uses_the_session_size() {
    let size = Size { cols: 80, rows: 24 };
    let mut screen = create_new_screen(size, false, false);
    // undo the helper's seeding: this is the state between the screen thread starting and the
    // first client size reaching it
    screen.client_sizes.clear();

    assert_eq!(
        screen.surface_size(),
        size,
        "the size the session was built with stands in until a client reports one"
    );
    screen
        .reflow_global_panes()
        .expect("reflow with no client size");
    screen
        .render_to_clients()
        .expect("render with no client size");
}

/// `CwdChanged` announces a move once. The manifest says where every pane is
/// every time, which is what puts right a plugin that was not listening yet.
#[test]
fn a_pane_manifest_carries_the_pane_cwd() {
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 1, 0);
    screen
        .terminal_cwds
        .insert(1, std::path::PathBuf::from("/work/hocus"));

    let manifest = screen.generate_and_report_pane_state().expect("TEST");

    let pane = manifest
        .panes
        .values()
        .flatten()
        .find(|pane| pane.id == 1 && !pane.is_plugin)
        .expect("the terminal pane is in the manifest");
    assert_eq!(pane.cwd.as_deref(), Some("/work/hocus"));
}

/// A plugin is not a process in a folder, and a plugin id shares its number
/// space with a terminal id — so an unguarded lookup would hand it one.
#[test]
fn a_plugin_pane_has_no_cwd() {
    let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 24 });
    new_tab(&mut screen, 1, 0);
    screen
        .terminal_cwds
        .insert(1, std::path::PathBuf::from("/work/hocus"));

    let manifest = screen.generate_and_report_pane_state().expect("TEST");

    assert!(
        manifest
            .panes
            .values()
            .flatten()
            .chain(manifest.global_panes.iter())
            .filter(|pane| pane.is_plugin)
            .all(|pane| pane.cwd.is_none()),
        "no plugin pane was given a folder"
    );
}

#[test]
fn global_pane_renders_once_for_clients_on_different_tabs() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    screen.add_client(2, false).expect("TEST");
    screen.switch_active_tab(0, None, true, 2).expect("TEST");

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
    screen
        .global_panes
        .focus_pane(PaneId::Terminal(99), 1)
        .unwrap();
    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));
    screen.render_to_clients().expect("TEST");
    let (render_instruction, _) = server_receiver.recv().expect("render instruction");
    let ServerInstruction::Render(Some(mut rendered_clients)) = render_instruction else {
        panic!("expected client render output")
    };
    let mut client_1_grid = grid_for_render_snapshot(size);
    let mut client_2_grid = grid_for_render_snapshot(size);
    let (client_1_cursor, client_1_snapshot) = take_snapshot_and_cursor_coordinates(
        &rendered_clients.remove(&1).expect("client 1 output"),
        &mut client_1_grid,
    );
    let (client_2_cursor, client_2_snapshot) = take_snapshot_and_cursor_coordinates(
        &rendered_clients.remove(&2).expect("client 2 output"),
        &mut client_2_grid,
    );
    assert_eq!(client_1_cursor, Some((1, 0)));
    assert_ne!(client_2_cursor, Some((1, 0)));

    let normalized_snapshot =
        format!("client 1:\n{client_1_snapshot}\nclient 2:\n{client_2_snapshot}")
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n");
    assert_snapshot!(
        normalized_snapshot,
        @r###"
    client 1:
    00 (C): GLOBAL   │
    01 (C):          │
    02 (C):          │
    03 (C):          │
    04 (C):          │
    05 (C):          │
    06 (C):          │
    07 (C):          │
    08 (C):          │
    09 (C):          │

    client 2:
    00 (C): GLOBAL   │
    01 (C):          │
    02 (C):          │
    03 (C):          │
    04 (C):          │
    05 (C):          │
    06 (C):          │
    07 (C):          │
    08 (C):          │
    09 (C):          │
    "###
    );
    for tab in screen.tabs.values() {
        assert_eq!(tab.get_display_area(), Size { cols: 30, rows: 10 });
        assert_eq!(tab.get_viewport().x, 10);
        assert!(!tab.get_all_pane_ids().contains(&PaneId::Terminal(99)));
    }
}

#[test]
fn closing_the_last_pane_with_a_visible_global_pane_exits_the_session() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));

    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Left, 12);
    screen.render_to_clients().expect("TEST");
    while server_receiver.try_recv().is_ok() {} // drain the first frame

    // the user exits their last shell
    assert!(screen
        .tabs
        .get_mut(&0)
        .unwrap()
        .extract_pane(PaneId::Terminal(1), true)
        .is_some());
    screen.render_to_clients().expect("TEST");

    assert!(
        screen.tabs.is_empty(),
        "a visible global pane must not keep an emptied tab alive"
    );
    assert!(
        std::iter::from_fn(|| server_receiver.try_recv().ok())
            .any(|(instruction, _)| matches!(instruction, ServerInstruction::Render(None))),
        "expected ServerInstruction::Render(None) to exit the session"
    );
}

#[test]
fn global_pane_render_does_not_move_the_cursor_of_a_client_focused_on_a_tab_pane() {
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
    // client 1 is left focused on its tab pane, not on the global pane

    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));

    let next_client_1_render = || loop {
        let (instruction, _) = server_receiver.recv().expect("render instruction");
        if let ServerInstruction::Render(Some(mut rendered_clients)) = instruction {
            return rendered_clients.remove(&1).expect("client 1 output");
        }
    };

    // first frame: both the tab pane and the global pane render
    screen
        .tabs
        .get_mut(&0)
        .unwrap()
        .handle_pty_bytes(1, b"tab pane".to_vec())
        .unwrap();
    screen.render_to_clients().expect("TEST");
    let mut grid = grid_for_render_snapshot(size);
    let cursor_in_tab_pane =
        take_snapshot_and_cursor_coordinates(&next_client_1_render(), &mut grid).0;
    assert!(
        cursor_in_tab_pane.is_some(),
        "the client's cursor should be visible in its tab pane"
    );

    // second frame: only the global pane has anything new to render
    screen.global_panes.handle_pty_bytes(99, b"MORE".to_vec());
    screen.render_to_clients().expect("TEST");
    let second_frame = next_client_1_render();
    let cursor_after_global_pane_render =
        take_snapshot_and_cursor_coordinates(&second_frame, &mut grid).0;

    assert_eq!(
        cursor_after_global_pane_render, cursor_in_tab_pane,
        "a global pane rendering must not strand the cursor at the end of its own contents"
    );
    assert!(
        second_frame.contains("\u{1b}[?25l"),
        "the cursor should be hidden while the global pane's contents are drawn"
    );
}

/// The tab's ui strips (tab bar, status bar) span the whole screen and the left global pane fills
/// only the rows between them.
#[test]
fn tab_chrome_spans_the_full_width_beside_a_left_global_pane() {
    let client_id = 1;
    let global_cols = 10;
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    // a non-selectable pane on the bottom edge is chrome, same as the status-bar plugin
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
                borderless: Some(true),
            },
            Some(client_id),
            None,
        )
        .unwrap();
    screen
        .get_active_tab_mut(client_id)
        .unwrap()
        .set_pane_selectable(PaneId::Terminal(2), false);

    insert_left_global_pane(&mut screen, global_cols);
    screen
        .resize_to_screen(Size { cols: 50, rows: 12 })
        .unwrap();
    screen.render_to_clients().unwrap();

    let tab = screen.get_active_tab(client_id).unwrap();
    let (chrome_top, chrome_bottom) = tab.chrome_rows();
    assert_eq!(chrome_top, 0);
    assert!(
        chrome_bottom > 0,
        "the tab has no bottom chrome to speak of"
    );
    let widened_chrome = tab
        .get_tiled_panes()
        .find(|(id, _)| **id == PaneId::Terminal(2))
        .map(|(_, pane)| pane.position_and_size())
        .unwrap();
    assert_eq!(
        (widened_chrome.x, widened_chrome.cols.as_usize()),
        (0, screen.surface_size().cols),
        "the chrome pane does not span the full screen width"
    );

    let global_pane = screen.global_panes.pane(PaneId::Terminal(99)).unwrap();
    let global_pane_geom = global_pane.position_and_size();
    assert_eq!(
        (global_pane_geom.y, global_pane_geom.rows.as_usize()),
        (0, screen.surface_size().rows - chrome_bottom),
        "the global pane covers rows belonging to the tab's chrome"
    );
    assert_eq!(global_pane_geom.x, 0);
    assert_eq!(global_pane_geom.cols.as_usize(), global_cols);
    // the chrome is all that is below the band, and chrome draws no line above itself - so the
    // band's last row is its own, like a tab pane flush with the bottom of the viewport
    assert_eq!(
        global_pane.get_content_offset().bottom,
        0,
        "the global pane gives up a row for a border nothing draws"
    );
    assert_eq!(
        global_pane.get_content_rows(),
        global_pane_geom.rows.as_usize()
    );
}

/// A tab opened *after* a global pane is already docked is built over the full surface and then
/// narrowed to the center between the global panes, so its chrome has to be re-widened for it as
/// well - and has to stay that way when the tabs are switched between.
#[test]
fn tab_chrome_spans_the_full_width_in_every_tab() {
    let client_id = 1;
    let global_cols = 10;
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    add_chrome_pane(&mut screen, 0, 2, client_id);

    insert_left_global_pane(&mut screen, global_cols);
    screen
        .resize_to_screen(Size { cols: 50, rows: 12 })
        .unwrap();
    screen.render_to_clients().unwrap();

    new_tab(&mut screen, 3, 1);
    add_chrome_pane(&mut screen, 1, 4, client_id);
    screen.render_to_clients().unwrap();

    let surface_cols = screen.surface_size().cols;
    let chrome_geom = |screen: &Screen, tab_id: usize, pane_id: u32| {
        screen
            .tabs
            .get(&tab_id)
            .unwrap()
            .get_tiled_panes()
            .find(|(id, _)| **id == PaneId::Terminal(pane_id))
            .map(|(_, pane)| pane.position_and_size())
            .unwrap()
    };
    for (tab_id, pane_id) in [(0, 2), (1, 4)] {
        let geom = chrome_geom(&screen, tab_id, pane_id);
        assert_eq!(
            (geom.x, geom.cols.as_usize()),
            (0, surface_cols),
            "the chrome of tab {} does not span the full screen width",
            tab_id
        );
    }

    // and switching back to the first tab does not strand either of them
    screen.go_to_tab(1, client_id).unwrap();
    screen.render_to_clients().unwrap();
    for (tab_id, pane_id) in [(0, 2), (1, 4)] {
        let geom = chrome_geom(&screen, tab_id, pane_id);
        assert_eq!(
            (geom.x, geom.cols.as_usize()),
            (0, surface_cols),
            "the chrome of tab {} does not span the full screen width after switching tabs",
            tab_id
        );
    }
}

#[test]
fn resizing_a_focused_global_pane_only_moves_the_border_it_has() {
    use zellij_utils::data::{Direction, Resize, ResizeStrategy};

    let client_id = 1;
    let mut screen = screen_with_two_tiled_panes_and_a_global_pane();
    let global_pane_id = PaneId::Terminal(99);
    screen.focus_global_pane(global_pane_id, client_id).unwrap();
    assert_eq!(global_pane_cols(&screen), 10);

    // a left global pane has no horizontal border to move up or down
    screen
        .resize_pane_with_id(
            ResizeStrategy::new(Resize::Increase, Some(Direction::Up)),
            global_pane_id,
        )
        .unwrap();
    assert_eq!(global_pane_cols(&screen), 10);

    screen
        .resize_pane_with_id(
            ResizeStrategy::new(Resize::Increase, Some(Direction::Right)),
            global_pane_id,
        )
        .unwrap();
    assert_eq!(global_pane_cols(&screen), 11);

    screen
        .resize_pane_with_id(ResizeStrategy::new(Resize::Decrease, None), global_pane_id)
        .unwrap();
    assert_eq!(global_pane_cols(&screen), 10);
}

#[test]
fn resizing_the_tab_pane_beside_a_global_pane_moves_their_shared_border() {
    use zellij_utils::data::{Direction, Resize, ResizeStrategy};

    let client_id = 1;
    let mut screen = screen_with_two_tiled_panes_and_a_global_pane();
    screen
        .get_active_tab_mut(client_id)
        .unwrap()
        .focus_pane_on_edge(Direction::Left, client_id);

    // growing the tab pane towards the global pane shrinks it
    assert!(screen
        .resize_global_pane_from_adjacent_tab_pane(
            client_id,
            ResizeStrategy::new(Resize::Increase, Some(Direction::Left)),
        )
        .unwrap());
    assert_eq!(global_pane_cols(&screen), 9);

    assert!(screen
        .resize_global_pane_from_adjacent_tab_pane(
            client_id,
            ResizeStrategy::new(Resize::Decrease, Some(Direction::Left)),
        )
        .unwrap());
    assert_eq!(global_pane_cols(&screen), 10);

    // there is no global pane on the right, and a directionless resize stays tab-local
    assert!(!screen
        .resize_global_pane_from_adjacent_tab_pane(
            client_id,
            ResizeStrategy::new(Resize::Increase, Some(Direction::Right)),
        )
        .unwrap());
    assert!(!screen
        .resize_global_pane_from_adjacent_tab_pane(
            client_id,
            ResizeStrategy::new(Resize::Increase, None),
        )
        .unwrap());
    assert_eq!(global_pane_cols(&screen), 10);
}

#[test]
fn dragging_the_border_of_a_global_pane_resizes_it() {
    let client_id = 1;
    let mut screen = screen_with_two_tiled_panes_and_a_global_pane();
    // the separator cell of a left global pane is its own last column
    let border_column = 9;

    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(5, border_column)),
        client_id,
    );
    // the press starts a drag rather than focusing the pane
    assert_eq!(screen.global_panes.focused_pane_id(client_id), None);

    screen.handle_mouse_event(
        MouseEvent::new_left_motion_event(Position::new(5, 14)),
        client_id,
    );
    assert_eq!(global_pane_cols(&screen), 15);
    assert_tiled_panes_are_bounded_by_left_global_pane(&screen, 15);

    screen.handle_mouse_event(
        MouseEvent::new_left_release_event(Position::new(5, 14)),
        client_id,
    );
    // once released, moving over the tab area no longer resizes anything
    screen.handle_mouse_event(
        MouseEvent::new_left_motion_event(Position::new(5, 30)),
        client_id,
    );
    assert_eq!(global_pane_cols(&screen), 15);
}

#[test]
fn a_click_on_tab_chrome_beside_a_global_pane_is_not_a_border_drag() {
    let client_id = 1;
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    // a non-selectable pane on the bottom edge is chrome, same as the status-bar plugin. It draws
    // no boundary, so the row it shares with the global pane below it is a click, not a border
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
                borderless: Some(true),
            },
            Some(client_id),
            None,
        )
        .unwrap();
    screen
        .get_active_tab_mut(client_id)
        .unwrap()
        .set_pane_selectable(PaneId::Terminal(2), false);
    add_global_terminal(&mut screen, 42);
    screen
        .resize_to_screen(Size { cols: 40, rows: 12 })
        .unwrap();
    screen.render_to_clients().unwrap();

    let global_pane_geom = |screen: &Screen| {
        screen
            .global_panes
            .pane(PaneId::Terminal(42))
            .unwrap()
            .position_and_size()
    };
    let border_line = global_pane_geom(&screen).y as i32 - 1;
    let rows_before = global_pane_geom(&screen).rows.as_usize();
    // the row is the global pane's border, it is only the chrome on top of it that saves the click
    assert!(screen
        .global_panes
        .border_hit_at(&Position::new(border_line, 5))
        .is_some());
    assert!(screen
        .get_active_tab(client_id)
        .unwrap()
        .position_is_on_ui_pane(&Position::new(border_line, 5)));

    screen.handle_mouse_event(
        MouseEvent::new_left_press_event(Position::new(border_line, 5)),
        client_id,
    );
    screen.handle_mouse_event(
        MouseEvent::new_left_motion_event(Position::new(border_line - 3, 5)),
        client_id,
    );
    assert_eq!(global_pane_geom(&screen).rows.as_usize(), rows_before);
}

#[test]
fn adding_a_tiled_pane_stays_beside_a_left_global_pane() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
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
