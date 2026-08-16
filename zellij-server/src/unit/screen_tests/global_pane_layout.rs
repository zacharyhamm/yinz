use super::*;

#[test]
fn gp02_moved_floating_pane_keeps_position_across_noop_reflows_beneath_top_global_pane() {
    use zellij_utils::data::GlobalPaneEdge;
    use zellij_utils::pane_size::Viewport;

    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    gp_insert_global_pane(
        &mut screen,
        99,
        GlobalPaneEdge::Top,
        PercentOrFixed::Fixed(4),
        false,
    );
    screen.render_to_clients().expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().get_display_area_origin(),
        (0, 4)
    );

    {
        let tab = screen.get_active_tab_mut(1).unwrap();
        tab.new_floating_pane(PaneId::Terminal(2), None, None, false, true, None, None)
            .expect("TEST");
        tab.change_floating_pane_coordinates(
            &PaneId::Terminal(2),
            FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(10)),
                y: Some(PercentOrFixed::Fixed(2)),
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
    assert_eq!(before.y, 6, "moved pane should sit at tab-local y=2");

    let display_area = Viewport {
        x: 0,
        y: 4,
        cols: 121,
        rows: 16,
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

#[test]
fn gp02_moved_floating_pane_keeps_position_across_noop_reflows_with_left_and_top_global_panes() {
    use zellij_utils::data::GlobalPaneEdge;
    use zellij_utils::pane_size::Viewport;

    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    gp_insert_global_pane(
        &mut screen,
        98,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(10),
        false,
    );
    gp_insert_global_pane(
        &mut screen,
        99,
        GlobalPaneEdge::Top,
        PercentOrFixed::Fixed(4),
        false,
    );
    screen.render_to_clients().expect("TEST");
    assert_eq!(
        screen.tabs.get(&0).unwrap().get_display_area_origin(),
        (10, 4)
    );

    {
        let tab = screen.get_active_tab_mut(1).unwrap();
        tab.new_floating_pane(PaneId::Terminal(2), None, None, false, true, None, None)
            .expect("TEST");
        tab.change_floating_pane_coordinates(
            &PaneId::Terminal(2),
            FloatingPaneCoordinates {
                x: Some(PercentOrFixed::Fixed(10)),
                y: Some(PercentOrFixed::Fixed(2)),
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
    assert_eq!(
        (before.x, before.y),
        (20, 6),
        "moved pane should sit at tab-local (10, 2)"
    );

    let display_area = Viewport {
        x: 10,
        y: 4,
        cols: 111,
        rows: 16,
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

#[test]
fn gp04_placement_change_to_opposite_edge_updates_boundary_reservation_on_next_render() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Left, 8);
    screen.render_to_clients().expect("TEST");

    screen
        .set_global_pane_placement(
            PaneId::Terminal(99),
            GlobalPanePlacement {
                edge: GlobalPaneEdge::Right,
                size: PercentOrFixed::Fixed(8),
                length: None,
            },
        )
        .expect("TEST");
    screen.render_to_clients().expect("TEST");

    let pane = screen
        .tabs
        .get(&0)
        .unwrap()
        .get_pane_with_id(PaneId::Terminal(1))
        .unwrap();
    let offset = pane.get_content_offset();
    assert_eq!(
        offset.right, 1,
        "tab pane now abuts the right global pane and must reserve its last column"
    );
    assert_eq!(
        offset.left, 0,
        "tab pane no longer abuts a left global pane"
    );
}

// `borderless` on a global pane means "no outer frame", not "no tab/global separator": the tab
// grows its boundary viewport over every visible strip and draws the separator one cell inside a
// left/top global pane, so a borderless pane reserves that cell exactly like a bordered one does.
// Neither layer can then overwrite the other, whichever renders last.
#[test]
fn gp05_borderless_left_global_pane_reserves_the_tab_separator_column() {
    use zellij_utils::data::{GlobalPaneEdge, ResizeStrategy};

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size); // PaneFrameStyle::None
    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));

    new_tab(&mut screen, 1, 0);
    gp_insert_global_pane(
        &mut screen,
        99,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(8),
        true,
    );
    // 8 columns wide, of which the last is reserved for the tab separator
    gp_fill_global_rows_left(&mut screen, 10, 7);
    screen
        .tabs
        .get_mut(&0)
        .unwrap()
        .get_pane_with_id_mut(PaneId::Terminal(1))
        .unwrap()
        .handle_pty_bytes(b"TTTTTTTTTTTTTTTTTTTT".to_vec());

    screen.render_to_clients().expect("TEST");
    let mut grid = grid_for_render_snapshot(size);
    let first_snapshot = gp_drain_client_1_snapshot(&server_receiver, &mut grid);

    // split the tab so the boundary layout changes (this also clears the host display, which
    // force-renders the global panes back on top)
    screen
        .get_active_tab_mut(1)
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
            Some(1),
            None,
        )
        .expect("TEST");
    screen.render_to_clients().expect("TEST");
    let second_snapshot = gp_drain_client_1_snapshot(&server_receiver, &mut grid);

    // resize the top tab pane without clearing the host display - boundaries are re-emitted
    // while the borderless global pane has no new output, so they must not land inside its
    // content area
    screen
        .focus_pane_with_id(PaneId::Terminal(1), false, false, 1)
        .expect("TEST");
    screen
        .get_active_tab_mut(1)
        .unwrap()
        .resize(
            1,
            ResizeStrategy {
                resize: Resize::Increase,
                direction: Some(Direction::Down),
                invert_on_boundaries: false,
            },
        )
        .expect("TEST");
    screen.render_to_clients().expect("TEST");
    let third_snapshot = gp_drain_client_1_snapshot(&server_receiver, &mut grid);

    // stable across all three renders: the global pane never writes column 7, the tab always draws
    // its separator there
    for (render, snapshot) in [
        ("first", &first_snapshot),
        ("second", &second_snapshot),
        ("third", &third_snapshot),
    ] {
        for row in 0..10 {
            let content = gp_line_content(snapshot.lines().nth(row).unwrap_or(""));
            assert_eq!(
                content.chars().take(7).collect::<String>(),
                "GGGGGGG",
                "{render} render, row {row}: the borderless left global pane must keep its 7 content \
                 columns, got: {content:?}"
            );
            // a separator or one of its junctions, never pane content
            assert!(
                matches!(content.chars().nth(7), Some('\u{2502}') | Some('\u{251c}')),
                "{render} render, row {row}: column 7 is reserved for the tab separator, \
                 got: {content:?}"
            );
        }
    }
}

#[test]
fn gp05_borderless_top_global_pane_reserves_the_tab_separator_row() {
    use zellij_utils::data::{GlobalPaneEdge, ResizeStrategy};

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size); // PaneFrameStyle::None
    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));

    new_tab(&mut screen, 1, 0);
    gp_insert_global_pane(
        &mut screen,
        99,
        GlobalPaneEdge::Top,
        PercentOrFixed::Fixed(3),
        true,
    );
    // 3 rows tall, of which the last is reserved for the tab separator
    let mut global_content = Vec::new();
    for row in 0..2 {
        global_content.extend(std::iter::repeat_n(b'G', 40));
        if row + 1 < 2 {
            global_content.extend_from_slice(b"\r\n");
        }
    }
    screen
        .global_panes
        .pane_mut(PaneId::Terminal(99))
        .unwrap()
        .handle_pty_bytes(global_content);
    screen
        .tabs
        .get_mut(&0)
        .unwrap()
        .get_pane_with_id_mut(PaneId::Terminal(1))
        .unwrap()
        .handle_pty_bytes(b"TTTTTTTTTTTTTTTTTTTT".to_vec());

    screen.render_to_clients().expect("TEST");
    let mut grid = grid_for_render_snapshot(size);
    let first_snapshot = gp_drain_client_1_snapshot(&server_receiver, &mut grid);

    // split the tab so the boundary layout changes (this also clears the host display, which
    // force-renders the global panes back on top)
    screen
        .get_active_tab_mut(1)
        .unwrap()
        .new_pane(
            PaneId::Terminal(2),
            None,
            None,
            false,
            true,
            NewPanePlacement::Tiled {
                direction: Some(Direction::Right),
                borderless: None,
            },
            Some(1),
            None,
        )
        .expect("TEST");
    screen.render_to_clients().expect("TEST");
    let second_snapshot = gp_drain_client_1_snapshot(&server_receiver, &mut grid);

    // resize the tab pane without clearing the host display - boundaries are re-emitted while the
    // borderless global pane has no new output, so they must not land inside its content row
    screen
        .get_active_tab_mut(1)
        .unwrap()
        .resize(
            1,
            ResizeStrategy {
                resize: Resize::Increase,
                direction: Some(Direction::Left),
                invert_on_boundaries: false,
            },
        )
        .expect("TEST");
    screen.render_to_clients().expect("TEST");
    let third_snapshot = gp_drain_client_1_snapshot(&server_receiver, &mut grid);

    // stable across all three renders: the global pane never writes row 2, the tab always draws its
    // separator there
    for (render, snapshot) in [
        ("first", &first_snapshot),
        ("second", &second_snapshot),
        ("third", &third_snapshot),
    ] {
        for row in 0..2 {
            let content = gp_line_content(snapshot.lines().nth(row).unwrap_or(""));
            assert!(
                content.starts_with(&"G".repeat(40)),
                "{render} render: the borderless top global pane's content row {row} must not be \
                 overwritten by the tab separator, got: {content:?}"
            );
        }
        let separator_row = gp_line_content(snapshot.lines().nth(2).unwrap_or(""));
        assert!(
            !separator_row.contains('G') && separator_row.contains('\u{2500}'),
            "{render} render: row 2 is reserved for the tab separator, got: {separator_row:?}"
        );
    }
}

// Panes sharing an edge form one strip: a second pane on an occupied edge splits the strip's long
// axis with the first, adopting its thickness, and closing one gives its span back.
#[test]
fn gp06_same_edge_global_panes_split_the_strip() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size); // PaneFrameStyle::None
    new_tab(&mut screen, 1, 0);
    gp_insert_global_pane(
        &mut screen,
        98,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(8),
        false,
    );
    screen.render_to_clients().expect("TEST");
    let geom_alone = screen
        .global_panes
        .pane(PaneId::Terminal(98))
        .unwrap()
        .position_and_size();

    // a second visible pane on the occupied edge splits the strip's rows with the first;
    // its different declared thickness loses to the strip's first pane
    gp_try_insert_global_pane(
        &mut screen,
        99,
        GlobalPaneEdge::Left,
        PercentOrFixed::Fixed(12),
        false,
        false,
    )
    .expect("a second pane on an occupied edge splits the strip");
    screen.render_to_clients().expect("TEST");
    let geom_first = screen
        .global_panes
        .pane(PaneId::Terminal(98))
        .unwrap()
        .position_and_size();
    let geom_second = screen
        .global_panes
        .pane(PaneId::Terminal(99))
        .unwrap()
        .position_and_size();
    assert_eq!(
        geom_first.cols.as_usize(),
        8,
        "the strip keeps the first pane's thickness"
    );
    assert_eq!(
        geom_second.cols.as_usize(),
        8,
        "the joiner adopts the strip's thickness"
    );
    assert_eq!(
        geom_first.rows.as_usize() + geom_second.rows.as_usize(),
        geom_alone.rows.as_usize(),
        "the two panes tile the strip's rows"
    );
    assert_eq!(
        geom_second.y,
        geom_first.y + geom_first.rows.as_usize(),
        "the joiner sits after the first pane in the strip"
    );

    // removing the joiner gives the strip back to the first pane
    assert!(screen.global_panes.remove(PaneId::Terminal(99)).is_some());
    screen.reflow_global_panes().expect("TEST");
    // the widening happens as part of laying the tabs out over their center
    screen.render_to_clients().expect("TEST");
    screen.render_to_clients().expect("TEST");
    assert_eq!(
        screen
            .global_panes
            .pane(PaneId::Terminal(98))
            .unwrap()
            .position_and_size(),
        geom_alone,
        "the survivor reclaims the whole strip"
    );
}

#[test]
fn an_undersized_attaching_client_is_rejected_without_blocking_rendering() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let mut mock_screen = MockScreen::new(Size { cols: 40, rows: 10 });
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
    mock_screen
        .to_screen
        .send(ScreenInstruction::NewGlobalPane {
            pane_id: PaneId::Terminal(99),
            initial_pane_title: None,
            hold_for_command: None,
            invoked_with: None,
            placement: GlobalPanePlacement {
                edge: GlobalPaneEdge::Left,
                size: PercentOrFixed::Fixed(5),
                length: None,
            },
            borderless: false,
            initially_hidden: false,
            should_focus: false,
            client_id: Some(1),
            completion_tx: Some(NotificationEnd::new(completion_tx)),
            set_blocking: false,
            declaration_order: None,
            split: None,
        })
        .unwrap();
    completion_rx.blocking_recv().expect("global pane created");
    server_receiver.try_iter().for_each(drop);

    mock_screen
        .to_screen
        .send(ScreenInstruction::AddClient(
            2,
            false,
            Size { cols: 1, rows: 1 },
            None,
            None,
        ))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::RenderToClients)
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    let server_instructions: Vec<_> = server_receiver
        .try_iter()
        .map(|(instruction, _)| instruction)
        .collect();
    mock_screen.teardown(vec![screen_thread]);

    assert!(server_instructions
        .iter()
        .any(|instruction| matches!(instruction, ServerInstruction::ClientExit(2, _))));
    assert!(server_instructions.iter().any(|instruction| matches!(
        instruction,
        ServerInstruction::Render(Some(rendered_clients))
            if rendered_clients.contains_key(&1) && !rendered_clients.contains_key(&2)
    )));
}

#[test]
fn equivalent_split_plugin_arrivals_keep_the_first_global_pane() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let mut mock_screen = MockScreen::new(Size { cols: 40, rows: 10 });
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    let run_plugin =
        RunPluginOrAlias::from_url("file:/path/to/agents.wasm", &None, None, None).unwrap();
    let placement = GlobalPanePlacement {
        edge: GlobalPaneEdge::Left,
        size: PercentOrFixed::Fixed(5),
        length: None,
    };
    let (first_tx, first_rx) = tokio::sync::oneshot::channel();
    mock_screen
        .to_screen
        .send(ScreenInstruction::NewGlobalPane {
            pane_id: PaneId::Plugin(77),
            initial_pane_title: None,
            hold_for_command: None,
            invoked_with: Some(Run::Plugin(run_plugin.clone())),
            placement,
            borderless: false,
            initially_hidden: false,
            should_focus: false,
            client_id: Some(1),
            completion_tx: Some(NotificationEnd::new(first_tx)),
            set_blocking: false,
            declaration_order: None,
            split: None,
        })
        .unwrap();
    first_rx.blocking_recv().expect("first plugin created");

    let (duplicate_tx, duplicate_rx) = tokio::sync::oneshot::channel();
    mock_screen
        .to_screen
        .send(ScreenInstruction::NewGlobalPane {
            pane_id: PaneId::Plugin(78),
            initial_pane_title: None,
            hold_for_command: None,
            invoked_with: Some(Run::Plugin(run_plugin)),
            placement,
            borderless: false,
            initially_hidden: false,
            should_focus: true,
            client_id: Some(1),
            completion_tx: Some(NotificationEnd::new(duplicate_tx)),
            set_blocking: false,
            declaration_order: None,
            split: Some(crate::pty::GlobalSplitAnchor {
                anchor: PaneId::Plugin(77),
                before: true,
            }),
        })
        .unwrap();
    let completion = duplicate_rx.blocking_recv().expect("duplicate adopted");
    std::thread::sleep(std::time::Duration::from_millis(100));
    let pty_instructions: Vec<_> = pty_receiver
        .try_iter()
        .map(|(instruction, _)| instruction)
        .collect();
    mock_screen.teardown(vec![screen_thread]);

    assert_eq!(completion.affected_pane_id, Some(PaneId::Plugin(77)));
    assert!(pty_instructions.iter().any(|instruction| matches!(
        instruction,
        PtyInstruction::ClosePane(PaneId::Plugin(78), None)
    )));
}

// The integration-visible shape of the hocus sidebar: a global plugin pane with a second
// plugin pane split off above it. Both halves of the strip must reach the client's screen.
#[test]
fn gp08_both_halves_of_a_split_plugin_strip_render() {
    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));
    new_tab(&mut screen, 1, 0);
    let run_plugin =
        RunPluginOrAlias::from_url("file:/path/to/hocus.wasm", &None, None, None).unwrap();
    add_matching_global_plugin(&mut screen, 77, run_plugin, false);
    screen.render_to_clients().expect("TEST");

    // the split the chat plugin makes for its agent list, above itself
    let plugin_sender = screen.bus.senders.to_plugin.clone().unwrap_or_else(|| {
        let (plugin_tx, _plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
        SenderWithContext::new(plugin_tx)
    });
    let agents_pane = PaneFactory::plugin(
        78,
        Default::default(),
        plugin_sender,
        "agents".to_owned(),
        "agents".to_owned(),
        screen.sixel_image_store.clone(),
        screen.kitty_image_store.clone(),
        screen.terminal_emulator_colors.clone(),
        screen.terminal_emulator_color_codes.clone(),
        Rc::new(RefCell::new(LinkHandler::new())),
        screen.character_cell_size.clone(),
        screen.connected_clients.borrow().keys().copied().collect(),
        screen.style,
        None,
        screen.debug,
        screen.arrow_fonts,
        screen.styled_underlines,
    );
    screen
        .global_panes
        .insert_split(agents_pane, PaneId::Plugin(77), true, None)
        .expect("TEST");
    screen.reflow_global_panes().expect("TEST");
    // the widening happens as part of laying the tabs out over their center
    screen.render_to_clients().expect("TEST");

    screen
        .global_panes
        .handle_plugin_bytes(78, 1, b"AGENTS".to_vec());
    screen
        .global_panes
        .handle_plugin_bytes(77, 1, b"HOCUS".to_vec());
    screen.render_to_clients().expect("TEST");
    let mut grid = grid_for_render_snapshot(size);
    let snapshot = gp_drain_client_1_snapshot(&server_receiver, &mut grid);

    let row_of = |needle: &str| {
        snapshot
            .lines()
            .position(|line| gp_line_content(line).contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} rendered:\n{snapshot}"))
    };
    // the strip is 5 columns of which the last is reserved, so the content wraps at 4
    assert!(
        row_of("AGEN") < row_of("HOCU"),
        "the split pane sits above its anchor:\n{snapshot}"
    );
    let separator_row = snapshot
        .lines()
        .position(|line| gp_line_content(line).contains("────"))
        .unwrap_or_else(|| panic!("the split separator rendered:\n{snapshot}"));
    assert!(
        (row_of("AGEN")..row_of("HOCU")).contains(&separator_row),
        "the separator sits between the two panes:\n{snapshot}"
    );
}

/// `zellij action new-tab` against a session every client has detached from: nobody contributes a
/// size, so the tab has to fall back to the same last-known surface `surface_size` does. A 0x0 tab
/// has no room for a pane and the next render tick closes it.
#[test]
fn a_tab_created_with_no_clients_attached_gets_the_last_known_session_size() {
    let size = Size {
        cols: 200,
        rows: 60,
    };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    screen.render_to_clients().expect("TEST");
    screen.remove_client(1).expect("TEST");

    new_tab(&mut screen, 2, 1);

    assert_eq!(
        screen.tabs.get(&1).map(|tab| tab.size),
        Some(size),
        "A tab created while nothing is attached takes the size the panes were last laid out over"
    );
    screen.render_to_clients().expect("TEST");
    assert!(
        screen.tabs.contains_key(&1),
        "and it survives the render tick rather than being closed for having no selectable panes"
    );
}

/// A terminal that reports its text area in pixels but not its cell size needs a client size to
/// divide it by. The reply comes off the route thread while the size comes off the
/// server-instruction thread, so it can land first - and it is re-sent only on a font size change,
/// so the size arriving has to be what derives the cell size.
#[test]
fn a_pixel_dimensions_reply_arriving_before_the_client_size_still_derives_the_cell_size() {
    let size = Size { cols: 80, rows: 24 };
    let mut screen = create_non_mirrored_screen(size);
    // the helper seeds a size for client 1; the race is about it not being there yet
    screen.client_sizes.clear();

    screen.update_pixel_dimensions(
        1,
        PixelDimensions {
            character_cell_size: None,
            text_area_size: Some(SizeInPixels {
                width: 800,
                height: 480,
            }),
        },
    );
    assert_eq!(
        *screen.character_cell_size.borrow(),
        None,
        "there is no client size to divide the text area by yet"
    );

    screen.set_client_size(1, size);

    assert_eq!(
        *screen.character_cell_size.borrow(),
        Some(SizeInPixels {
            width: 10,
            height: 20
        }),
        "the size arriving derives what the earlier reply could not"
    );

    screen.set_client_size(
        1,
        Size {
            cols: 100,
            rows: 30,
        },
    );

    assert_eq!(
        *screen.character_cell_size.borrow(),
        Some(SizeInPixels {
            width: 10,
            height: 20
        }),
        "a later resize must not divide the stale text area by the new grid"
    );
}

#[test]
fn recorder_pane_metadata() {
    use zellij_utils::data::{PaneInfo, PaneManifest, TabInfo};
    // Create one tab with one terminal pane and one plugin pane
    let mut pane_manifest = PaneManifest::default();
    let terminal_pane = PaneInfo {
        id: 5,
        is_plugin: false,
        title: "my terminal".to_string(),
        pane_content_rows: 24,
        pane_content_columns: 80,
        ..Default::default()
    };
    let plugin_pane = PaneInfo {
        id: 6,
        is_plugin: true,
        title: "plugin".to_string(),
        pane_content_rows: 24,
        pane_content_columns: 80,
        ..Default::default()
    };
    pane_manifest
        .panes
        .insert(0, vec![terminal_pane.clone(), plugin_pane]);
    let tab_info = TabInfo {
        position: 0,
        name: "my tab".to_string(),
        ..Default::default()
    };
    // Try to call conversion helper - will fail until implemented
    let metadata = crate::screen::Screen::convert_pane_metadata(
        &pane_manifest,
        std::slice::from_ref(&tab_info),
    );
    assert_eq!(metadata.len(), 1, "should filter plugin panes");
    assert_eq!(metadata[0].terminal_id, 5);
    assert_eq!(metadata[0].tab_id, 0);
    assert_eq!(metadata[0].tab_position, 0);
    assert_eq!(metadata[0].tab_name, "my tab");
    assert_eq!(metadata[0].title, "my terminal");
    assert_eq!(metadata[0].cols, 80);
    assert_eq!(metadata[0].rows, 24);
}
