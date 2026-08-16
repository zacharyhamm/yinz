use super::*;

#[test]
fn focusing_a_global_pane_highlights_the_border_it_shares_with_the_tab() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Left, 10);

    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));
    let next_client_1_render = || loop {
        let (instruction, _) = server_receiver.recv().expect("render instruction");
        if let ServerInstruction::Render(Some(mut rendered_clients)) = instruction {
            return rendered_clients.remove(&1).expect("client 1 output");
        }
    };

    // the shared column is the strip's last one: the strip cedes it, the tab's edge pane draws it
    let shared_column = 9;
    let tab_rows = {
        let viewport = screen.tabs.get(&0).unwrap().get_viewport();
        viewport.y..viewport.y + viewport.rows
    };

    screen
        .focus_global_pane(PaneId::Terminal(99), 1)
        .expect("TEST");
    screen.render_to_clients().expect("TEST");
    let focused = column_foregrounds(
        &next_client_1_render(),
        size,
        shared_column,
        tab_rows.clone(),
    );
    assert!(
        focused
            .iter()
            .all(|color| *color == Some(AnsiCode::ColorIndex(154))),
        "every cell of the border shared with the tab should carry the focused frame colour, got {focused:?}"
    );

    // and back: focus returns to the tab pane, which owns the colour again
    screen.defocus_global_pane(1);
    screen.render_to_clients().expect("TEST");
    let defocused = column_foregrounds(&next_client_1_render(), size, shared_column, tab_rows);
    assert!(
        defocused
            .iter()
            .all(|color| *color == Some(AnsiCode::ColorIndex(154))),
        "the tab pane is focused now, so the shared border is still highlighted, got {defocused:?}"
    );
}

#[test]
fn only_the_focused_global_panes_own_rows_are_highlighted() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 40, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    // two panes splitting one Left strip, as the hocus sidebar does
    insert_global_pane_on_edge(&mut screen, 98, GlobalPaneEdge::Left, 10);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Left, 10);

    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));

    // nobody is focused on the tab, so its pane cannot be the one colouring the column
    screen
        .focus_global_pane(PaneId::Terminal(99), 1)
        .expect("TEST");
    screen.render_to_clients().expect("TEST");
    let rendered = loop {
        let (instruction, _) = server_receiver.recv().expect("render instruction");
        if let ServerInstruction::Render(Some(mut rendered_clients)) = instruction {
            break rendered_clients.remove(&1).expect("client 1 output");
        }
    };

    let lower = screen.global_panes.pane(PaneId::Terminal(99)).unwrap();
    let lower_geom = lower.position_and_size();
    let viewport = screen.tabs.get(&0).unwrap().get_viewport();
    let shared_column = 9;
    let colors = column_foregrounds(
        &rendered,
        size,
        shared_column,
        viewport.y..viewport.y + viewport.rows,
    );

    for (offset, color) in colors.iter().enumerate() {
        let y = viewport.y + offset;
        let inside_focused_pane =
            y >= lower_geom.y && y < lower_geom.y + lower_geom.rows.as_usize();
        if inside_focused_pane {
            assert_eq!(
                *color,
                Some(AnsiCode::ColorIndex(154)),
                "row {y} abuts the focused global pane and should be highlighted"
            );
        } else {
            assert_ne!(
                *color,
                Some(AnsiCode::ColorIndex(154)),
                "row {y} abuts the unfocused global pane and should not be highlighted"
            );
        }
    }
}

#[test]
fn opposite_edge_global_panes_reserve_boundary_space_for_tab_edge_panes() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 98, GlobalPaneEdge::Left, 8);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Right, 8);

    screen.render_to_clients().expect("TEST");

    let tab = screen.tabs.get(&0).unwrap();
    let pane = tab.get_pane_with_id(PaneId::Terminal(1)).unwrap();
    // the pane abuts the right global pane, so its last column is drawn as a boundary and must be
    // reserved from its content (and pty size)
    assert_eq!(
        pane.get_content_offset().right,
        1,
        "tab pane abutting a right global pane must reserve a boundary column"
    );
    assert_eq!(pane.get_content_columns(), pane.cols() - 1);
}

#[test]
fn right_only_global_pane_reserves_boundary_space_for_tab_edge_panes() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Right, 8);

    screen.render_to_clients().expect("TEST");

    let tab = screen.tabs.get(&0).unwrap();
    let pane = tab.get_pane_with_id(PaneId::Terminal(1)).unwrap();
    assert_eq!(
        pane.get_content_offset().right,
        1,
        "tab pane abutting a right global pane must reserve a boundary column"
    );
    assert_eq!(pane.get_content_columns(), pane.cols() - 1);
}

#[test]
fn opposite_vertical_edge_global_panes_reserve_boundary_space_for_tab_edge_panes() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 40, rows: 20 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 98, GlobalPaneEdge::Top, 4);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Bottom, 4);

    screen.render_to_clients().expect("TEST");

    let tab = screen.tabs.get(&0).unwrap();
    let pane = tab.get_pane_with_id(PaneId::Terminal(1)).unwrap();
    // the pane abuts the bottom global pane, so its last row is drawn as a boundary and must be
    // reserved from its content (and pty size)
    assert_eq!(
        pane.get_content_offset().bottom,
        1,
        "tab pane abutting a bottom global pane must reserve a boundary row"
    );
    assert_eq!(pane.get_content_rows(), pane.rows() - 1);
}

#[test]
fn hidden_and_reshown_global_pane_keeps_boundary_reservation() {
    use zellij_utils::data::GlobalPaneEdge;

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    insert_global_pane_on_edge(&mut screen, 99, GlobalPaneEdge::Right, 8);
    screen.render_to_clients().expect("TEST");

    screen.hide_pane(PaneId::Terminal(99)).expect("TEST");
    screen.render_to_clients().expect("TEST");
    let pane = screen
        .tabs
        .get(&0)
        .unwrap()
        .get_pane_with_id(PaneId::Terminal(1))
        .unwrap();
    assert_eq!(pane.get_content_offset().right, 0);

    screen.show_pane(PaneId::Terminal(99)).expect("TEST");
    screen.render_to_clients().expect("TEST");
    let pane = screen
        .tabs
        .get(&0)
        .unwrap()
        .get_pane_with_id(PaneId::Terminal(1))
        .unwrap();
    assert_eq!(
        pane.get_content_offset().right,
        1,
        "tab pane abutting the reshown right global pane must reserve a boundary column"
    );
}

#[test]
fn newly_added_client_sees_existing_global_panes() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);

    let mut global_pane = PaneFactory::terminal(
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
    global_pane.handle_pty_bytes(b"GLOBAL ONLY".to_vec());
    screen
        .global_panes
        .insert(
            global_pane,
            GlobalPanePlacement {
                edge: GlobalPaneEdge::Left,
                size: PercentOrFixed::Fixed(12),
                length: None,
            },
            false,
        )
        .unwrap();

    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));
    screen.render_to_clients().expect("TEST");
    let _ = server_receiver.recv().expect("initial render");

    screen.add_client(2, false).expect("TEST");
    screen.render_to_clients().expect("TEST");

    let mut client_2_output = None;
    while let Ok((instruction, _)) = server_receiver.try_recv() {
        if let ServerInstruction::Render(Some(mut rendered_clients)) = instruction {
            if let Some(output) = rendered_clients.remove(&2) {
                client_2_output = Some(output);
            }
        }
    }
    let client_2_output = client_2_output.expect("client 2 should receive a render");
    let mut grid = grid_for_render_snapshot(size);
    let snapshot = take_snapshot_and_cursor_coordinates(&client_2_output, &mut grid).1;
    assert!(
        snapshot.contains("GLOBAL ONLY"),
        "newly added client should see the global pane contents, got:\n{snapshot}"
    );
}

#[test]
fn global_pane_fake_cursor_does_not_move_other_clients_cursor() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    screen.add_client(2, false).expect("TEST");

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
                edge: GlobalPaneEdge::Bottom,
                size: PercentOrFixed::Fixed(3),
                length: None,
            },
            false,
        )
        .unwrap();
    screen
        .focus_pane_with_id(PaneId::Terminal(99), false, false, 1)
        .expect("TEST");

    let (server_sender, server_receiver) = channels::unbounded();
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_sender));
    screen.render_to_clients().expect("TEST");

    let (render_instruction, _) = server_receiver.recv().expect("render instruction");
    let ServerInstruction::Render(Some(rendered_clients)) = render_instruction else {
        panic!("expected client render output")
    };

    // client 1 is focused on the global pane: their cursor must be inside it
    let mut client_1_grid = grid_for_render_snapshot(size);
    let (client_1_cursor, _) = take_snapshot_and_cursor_coordinates(
        rendered_clients.get(&1).expect("client 1 output"),
        &mut client_1_grid,
    );
    assert!(
        client_1_cursor.is_some_and(|(_x, y)| y >= 7),
        "client 1 cursor should be inside the bottom global pane, got: {client_1_cursor:?}"
    );

    // client 2 is focused on the tab pane: the fake cursor drawn for client 1 inside the global
    // pane must not have moved client 2's host cursor out of the tab area
    let mut client_2_grid = grid_for_render_snapshot(size);
    let (client_2_cursor, _) = take_snapshot_and_cursor_coordinates(
        rendered_clients.get(&2).expect("client 2 output"),
        &mut client_2_grid,
    );
    assert!(
        client_2_cursor.is_some_and(|(_x, y)| y < 7),
        "client 2 cursor should stay inside the tab area, got: {client_2_cursor:?}"
    );
}

#[test]
fn global_terminal_creation_preserves_placement_from_pty_to_screen() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let placement = GlobalPanePlacement {
        edge: GlobalPaneEdge::Bottom,
        size: PercentOrFixed::Fixed(3),
        length: None,
    };
    let mut mock_screen = MockScreen::new(Size { cols: 40, rows: 10 });
    let pty_bus = Bus::new(
        vec![mock_screen.pty_receiver.take().unwrap()],
        Some(&mock_screen.to_screen),
        Some(&mock_screen.to_pty),
        Some(&mock_screen.to_plugin),
        Some(&mock_screen.to_server),
        Some(&mock_screen.to_pty_writer),
        Some(&mock_screen.to_background_jobs),
        Some(Box::new(mock_screen.os_input.clone())),
    );
    let pty = Pty::new(pty_bus, false, None, None);
    let pty_thread =
        std::thread::spawn(move || pty_thread_main(pty, Box::default()).expect("pty thread"));

    route_arbitrary_action_to_server(
        &mock_screen.clone_session_metadata(),
        Action::NewPane {
            direction: None,
            pane_name: Some("global terminal".to_owned()),
            start_suppressed: false,
            global_placement: Some(placement),
        },
        1,
    );

    let screen_receiver = mock_screen.screen_receiver.take().unwrap();
    let instruction = loop {
        let (instruction, _) = screen_receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("global pane screen instruction");
        if matches!(instruction, ScreenInstruction::NewGlobalPane { .. }) {
            break instruction;
        }
    };
    match instruction {
        ScreenInstruction::NewGlobalPane {
            pane_id: PaneId::Terminal(42),
            placement: actual,
            ..
        } => assert_eq!(actual, placement),
        other => panic!("expected global terminal creation, got {other:?}"),
    }

    let _ = mock_screen.to_pty.send(PtyInstruction::Exit);
    let _ = mock_screen.to_background_jobs.send(BackgroundJob::Exit);
    pty_thread.join().unwrap();
}

#[test]
fn global_new_tiled_pane_route_preserves_command_shell_and_spawn_context() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let mut mock_screen = MockScreen::new(Size { cols: 40, rows: 10 });
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let placement = GlobalPanePlacement {
        edge: GlobalPaneEdge::Right,
        size: PercentOrFixed::Fixed(8),
        length: None,
    };
    let default_shell = TerminalAction::RunCommand(
        RunCommandAction {
            command: PathBuf::from("default-shell"),
            ..Default::default()
        }
        .into(),
    );
    let actions = [
        Action::NewTiledPane {
            direction: Some(Direction::Right),
            command: Some(RunCommandAction {
                command: PathBuf::from("global-command"),
                ..Default::default()
            }),
            pane_name: Some("command global".to_owned()),
            near_current_pane: false,
            no_focus: false,
            borderless: Some(true),
            tab_id: None,
            global_placement: Some(placement),
        },
        Action::NewTiledPane {
            direction: None,
            command: None,
            pane_name: Some("shell global".to_owned()),
            near_current_pane: false,
            no_focus: true,
            borderless: Some(false),
            tab_id: None,
            global_placement: Some(placement),
        },
    ];
    let invalid_actions = [
        Action::NewTiledPane {
            direction: None,
            command: None,
            pane_name: None,
            near_current_pane: true,
            no_focus: false,
            borderless: None,
            tab_id: None,
            global_placement: Some(placement),
        },
        Action::NewTiledPane {
            direction: None,
            command: None,
            pane_name: None,
            near_current_pane: false,
            no_focus: false,
            borderless: None,
            tab_id: Some(2),
            global_placement: Some(placement),
        },
    ];
    let route_results: Vec<_> = actions
        .into_iter()
        .map(|action| {
            route_action(
                action,
                1,
                None,
                None,
                mock_screen.session_metadata.senders.clone(),
                Some(default_shell.clone()),
                None,
                InputMode::Normal,
                None,
            )
        })
        .collect();
    let invalid_route_results: Vec<_> = invalid_actions
        .into_iter()
        .map(|action| {
            route_action(
                action,
                1,
                None,
                None,
                mock_screen.session_metadata.senders.clone(),
                Some(default_shell.clone()),
                None,
                InputMode::Normal,
                None,
            )
        })
        .collect();
    let spawned: Vec<_> = pty_receiver
        .try_iter()
        .map(|(instruction, _)| instruction)
        .collect();
    mock_screen.teardown(vec![]);

    assert!(route_results.iter().all(Result::is_ok));
    assert!(invalid_route_results[0]
        .as_ref()
        .unwrap_err()
        .to_string()
        .contains("near-current-pane"));
    assert!(invalid_route_results[1]
        .as_ref()
        .unwrap_err()
        .to_string()
        .contains("cannot target a tab"));
    assert_eq!(spawned.len(), 2);
    assert!(matches!(
        &spawned[0],
        PtyInstruction::SpawnTerminal(
            Some(TerminalAction::RunCommand(command)),
            Some(name),
            _,
            _,
            ClientTabIndexOrPaneId::ClientId(1),
            _,
            _,
            Some(context),
        ) if command.command == *"global-command"
            && name == "command global"
            && context.placement == placement
            && context.borderless
            && context.should_focus
    ));
    assert!(matches!(
        &spawned[1],
        PtyInstruction::SpawnTerminal(
            Some(TerminalAction::RunCommand(command)),
            Some(name),
            _,
            _,
            ClientTabIndexOrPaneId::ClientIdNoFocus(1),
            _,
            _,
            Some(context),
        ) if command.command == *"default-shell"
            && name == "shell global"
            && context.placement == placement
            && !context.borderless
            && !context.should_focus
    ));
}

#[test]
fn global_insertion_failure_reports_error_cleans_up_once_and_keeps_screen_alive() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let mut mock_screen = MockScreen::new(Size { cols: 40, rows: 10 });
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    let (failed_tx, failed_rx) = tokio::sync::oneshot::channel();
    mock_screen
        .to_screen
        .send(ScreenInstruction::NewGlobalPane {
            pane_id: PaneId::Terminal(41),
            initial_pane_title: Some("invalid global".to_owned()),
            hold_for_command: None,
            invoked_with: None,
            placement: GlobalPanePlacement {
                edge: GlobalPaneEdge::Bottom,
                size: PercentOrFixed::Fixed(10),
                length: None,
            },
            borderless: false,
            initially_hidden: false,
            should_focus: true,
            client_id: Some(1),
            completion_tx: Some(NotificationEnd::new(failed_tx)),
            set_blocking: false,
            declaration_order: None,
            split: None,
        })
        .unwrap();
    let failed = failed_rx.blocking_recv().unwrap();

    let (succeeded_tx, succeeded_rx) = tokio::sync::oneshot::channel();
    mock_screen
        .to_screen
        .send(ScreenInstruction::NewGlobalPane {
            pane_id: PaneId::Terminal(42),
            initial_pane_title: Some("valid global".to_owned()),
            hold_for_command: None,
            invoked_with: None,
            placement: GlobalPanePlacement {
                edge: GlobalPaneEdge::Bottom,
                size: PercentOrFixed::Fixed(3),
                length: None,
            },
            borderless: false,
            initially_hidden: false,
            should_focus: true,
            client_id: Some(1),
            completion_tx: Some(NotificationEnd::new(succeeded_tx)),
            set_blocking: false,
            declaration_order: None,
            split: None,
        })
        .unwrap();
    let succeeded = succeeded_rx.blocking_recv().unwrap();

    let (duplicate_tx, duplicate_rx) = tokio::sync::oneshot::channel();
    mock_screen
        .to_screen
        .send(ScreenInstruction::NewGlobalPane {
            pane_id: PaneId::Terminal(42),
            initial_pane_title: Some("duplicate global".to_owned()),
            hold_for_command: None,
            invoked_with: None,
            placement: GlobalPanePlacement {
                edge: GlobalPaneEdge::Bottom,
                size: PercentOrFixed::Fixed(3),
                length: None,
            },
            borderless: false,
            initially_hidden: false,
            should_focus: true,
            client_id: Some(1),
            completion_tx: Some(NotificationEnd::new(duplicate_tx)),
            set_blocking: false,
            declaration_order: None,
            split: None,
        })
        .unwrap();
    let duplicate = duplicate_rx.blocking_recv().unwrap();
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
    mock_screen
        .to_screen
        .send(ScreenInstruction::ClosePane(
            PaneId::Terminal(42),
            None,
            Some(NotificationEnd::new(closed_tx)),
            None,
        ))
        .unwrap();
    closed_rx.blocking_recv().unwrap();
    let pty_instructions: Vec<_> = pty_receiver
        .try_iter()
        .map(|(instruction, _)| instruction)
        .collect();
    let invalid_cleanup_count = pty_instructions
        .iter()
        .filter(|instruction| {
            matches!(
                instruction,
                PtyInstruction::ClosePane(PaneId::Terminal(41), None)
            )
        })
        .count();
    let existing_backend_cleanup_count = pty_instructions
        .iter()
        .filter(|instruction| {
            matches!(
                instruction,
                PtyInstruction::ClosePane(PaneId::Terminal(42), None)
            )
        })
        .count();
    mock_screen.teardown(vec![screen_thread]);

    assert_eq!(failed.exit_status, Some(1));
    assert_eq!(failed.affected_pane_id, None);
    assert!(failed.error_message.is_some());
    assert_eq!(invalid_cleanup_count, 1);
    assert_eq!(existing_backend_cleanup_count, 1);
    assert_eq!(succeeded.affected_pane_id, Some(PaneId::Terminal(42)));
    assert_eq!(duplicate.exit_status, Some(1));
    assert_eq!(duplicate.affected_pane_id, None);
    assert!(duplicate.error_message.is_some());
}

#[test]
fn failed_global_insertion_discards_pending_plugin_events_before_id_reuse() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let size = Size { cols: 40, rows: 10 };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.drop_all_pty_messages();
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    mock_screen
        .to_screen
        .send(ScreenInstruction::PluginBytes(vec![
            PluginRenderAsset::new(88, 1, b"STALE_BYTES".to_vec()),
        ]))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::ShowPluginCursor(88, 1, Some((2, 0))))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::StartPluginLoadingIndication(
            88,
            LoadingIndication::new("STALE_START".to_owned()),
        ))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::ProgressPluginLoadingOffset(88))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::UpdatePluginLoadingStage(
            88,
            LoadingIndication::new("STALE_STAGE".to_owned()),
        ))
        .unwrap();

    let run_plugin =
        RunPluginOrAlias::from_url("file:/path/to/global/plugin/88", &None, None, None).unwrap();
    let (failed_tx, failed_rx) = tokio::sync::oneshot::channel();
    mock_screen
        .to_screen
        .send(ScreenInstruction::NewGlobalPane {
            pane_id: PaneId::Plugin(88),
            initial_pane_title: Some("invalid global plugin".to_owned()),
            hold_for_command: None,
            invoked_with: Some(Run::Plugin(run_plugin.clone())),
            placement: GlobalPanePlacement {
                edge: GlobalPaneEdge::Bottom,
                size: PercentOrFixed::Fixed(10),
                length: None,
            },
            borderless: true,
            initially_hidden: false,
            should_focus: true,
            client_id: Some(1),
            completion_tx: Some(NotificationEnd::new(failed_tx)),
            set_blocking: false,
            declaration_order: None,
            split: None,
        })
        .unwrap();
    assert_eq!(failed_rx.blocking_recv().unwrap().exit_status, Some(1));

    let (succeeded_tx, succeeded_rx) = tokio::sync::oneshot::channel();
    mock_screen
        .to_screen
        .send(ScreenInstruction::NewGlobalPane {
            pane_id: PaneId::Plugin(88),
            initial_pane_title: Some("reused global plugin".to_owned()),
            hold_for_command: None,
            invoked_with: Some(Run::Plugin(run_plugin)),
            placement: GlobalPanePlacement {
                edge: GlobalPaneEdge::Bottom,
                size: PercentOrFixed::Fixed(3),
                length: None,
            },
            borderless: true,
            initially_hidden: false,
            should_focus: true,
            client_id: Some(1),
            completion_tx: Some(NotificationEnd::new(succeeded_tx)),
            set_blocking: false,
            declaration_order: None,
            split: None,
        })
        .unwrap();
    assert_eq!(
        succeeded_rx.blocking_recv().unwrap().affected_pane_id,
        Some(PaneId::Plugin(88))
    );
    mock_screen
        .to_screen
        .send(ScreenInstruction::RenderToClients)
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);

    let received_server_instructions = received_server_instructions.lock().unwrap();
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.iter(),
        size,
    );
    let (cursor, snapshot) = snapshots.last().expect("final render");
    assert!(!snapshot.contains("STALE_BYTES"), "{snapshot}");
    assert!(!snapshot.contains("STALE_STAGE"), "{snapshot}");
    assert_ne!(*cursor, Some((2, 0)));
}

#[test]
fn early_global_plugin_bytes_replay_before_already_queued_later_bytes() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let size = Size { cols: 40, rows: 10 };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.drop_all_pty_messages();
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    mock_screen
        .to_screen
        .send(ScreenInstruction::PluginBytes(vec![
            PluginRenderAsset::new(90, 1, b"EARLY".to_vec()),
        ]))
        .unwrap();
    let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
    mock_screen
        .to_screen
        .send(ScreenInstruction::NewGlobalPane {
            pane_id: PaneId::Plugin(90),
            initial_pane_title: Some("ordered global plugin".to_owned()),
            hold_for_command: None,
            invoked_with: Some(Run::Plugin(
                RunPluginOrAlias::from_url("file:/path/to/global/plugin/90", &None, None, None)
                    .unwrap(),
            )),
            placement: GlobalPanePlacement {
                edge: GlobalPaneEdge::Bottom,
                size: PercentOrFixed::Fixed(3),
                length: None,
            },
            borderless: true,
            initially_hidden: false,
            should_focus: true,
            client_id: Some(1),
            completion_tx: Some(NotificationEnd::new(completion_tx)),
            set_blocking: false,
            declaration_order: None,
            split: None,
        })
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::PluginBytes(vec![
            PluginRenderAsset::new(90, 1, b"LATER".to_vec()),
        ]))
        .unwrap();

    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    assert_eq!(
        completion_rx.blocking_recv().unwrap().affected_pane_id,
        Some(PaneId::Plugin(90))
    );
    mock_screen
        .to_screen
        .send(ScreenInstruction::RenderToClients)
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);

    let received_server_instructions = received_server_instructions.lock().unwrap();
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.iter(),
        size,
    );
    let (_, snapshot) = snapshots.last().expect("final render");
    assert!(snapshot.contains("07 (C): LATER"), "{snapshot}");
    assert!(!snapshot.contains("EARLY"), "{snapshot}");
}

#[test]
fn early_global_plugin_events_are_replayed_after_insertion() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let size = Size { cols: 60, rows: 10 };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.drop_all_pty_messages();
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    let loading_start = LoadingIndication::new("EARLY_START".to_owned());
    let loading_stage = LoadingIndication::new("EARLY_STAGE".to_owned());
    std::thread::sleep(std::time::Duration::from_millis(450));

    mock_screen
        .to_screen
        .send(ScreenInstruction::PluginBytes(vec![
            PluginRenderAsset::new(77, 1, b"EARLY_BYTES".to_vec()),
        ]))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::ShowPluginCursor(77, 1, Some((2, 0))))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::PluginBytes(vec![
            PluginRenderAsset::new(78, 1, b"loading grid".to_vec()),
        ]))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::StartPluginLoadingIndication(
            78,
            loading_start,
        ))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::ProgressPluginLoadingOffset(78))
        .unwrap();
    mock_screen
        .to_screen
        .send(ScreenInstruction::UpdatePluginLoadingStage(
            78,
            loading_stage,
        ))
        .unwrap();

    for (plugin_id, edge, size, should_focus) in [
        (77, GlobalPaneEdge::Left, 18, true),
        (78, GlobalPaneEdge::Bottom, 3, false),
    ] {
        let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
        mock_screen
            .to_screen
            .send(ScreenInstruction::NewGlobalPane {
                pane_id: PaneId::Plugin(plugin_id),
                initial_pane_title: Some(format!("global plugin {plugin_id}")),
                hold_for_command: None,
                invoked_with: Some(Run::Plugin(
                    RunPluginOrAlias::from_url(
                        &format!("file:/path/to/global/plugin/{plugin_id}"),
                        &None,
                        None,
                        None,
                    )
                    .unwrap(),
                )),
                placement: GlobalPanePlacement {
                    edge,
                    size: PercentOrFixed::Fixed(size),
                    length: None,
                },
                borderless: true,
                initially_hidden: false,
                should_focus,
                client_id: Some(1),
                completion_tx: Some(NotificationEnd::new(completion_tx)),
                set_blocking: false,
                declaration_order: None,
                split: None,
            })
            .unwrap();
        assert_eq!(
            completion_rx.blocking_recv().unwrap().affected_pane_id,
            Some(PaneId::Plugin(plugin_id))
        );
        if plugin_id == 77 {
            mock_screen
                .to_screen
                .send(ScreenInstruction::RenderToClients)
                .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    mock_screen
        .to_screen
        .send(ScreenInstruction::RenderToClients)
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    let unblocked_plugin_ids: Vec<_> = plugin_receiver
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::UnblockCliPipes(assets) => Some(
                assets
                    .into_iter()
                    .map(|asset| asset.plugin_id)
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .flatten()
        .collect();
    mock_screen.teardown(vec![server_thread, screen_thread]);

    let received_server_instructions = received_server_instructions.lock().unwrap();
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.iter(),
        size,
    );
    let (_, snapshot) = snapshots.last().expect("final render");
    assert_eq!(unblocked_plugin_ids, vec![77, 78]);
    assert!(
        snapshots
            .iter()
            .any(|(_, snapshot)| snapshot.contains("EARLY_BYTES")),
        "{snapshots:?}"
    );
    assert!(snapshot.contains("EARLY_STAGE."), "{snapshot}");
    assert!(
        snapshots.iter().any(|(cursor, _)| *cursor == Some((2, 0))),
        "{snapshots:?}"
    );
}

#[test]
fn plugin_bytes_with_no_pane_still_unblock_cli_pipes() {
    // regression: unblocking used to be gated on finding a pane to draw the bytes into, so a
    // plugin that rendered without one (a background plugin, or one whose pane was closed
    // mid-render) never unblocked and anything piping to it hung forever
    let size = Size { cols: 60, rows: 10 };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.drop_all_pty_messages();
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let received_server_instructions = Arc::new(Mutex::new(vec![]));
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    std::thread::sleep(std::time::Duration::from_millis(450));

    // no pane exists for plugin 99, and none is ever created for it
    mock_screen
        .to_screen
        .send(ScreenInstruction::PluginBytes(vec![
            PluginRenderAsset::new(99, 1, b"ORPHAN_BYTES".to_vec()),
        ]))
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));

    let unblocked_plugin_ids: Vec<_> = plugin_receiver
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::UnblockCliPipes(assets) => Some(
                assets
                    .into_iter()
                    .map(|asset| asset.plugin_id)
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .flatten()
        .collect();
    mock_screen.teardown(vec![server_thread, screen_thread]);

    assert!(
        unblocked_plugin_ids.contains(&99),
        "a plugin that rendered without a pane must still unblock, got {unblocked_plugin_ids:?}"
    );
}

#[test]
fn closed_or_rejected_global_plugins_unload_exactly_once_through_pty() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let mut mock_screen = MockScreen::new(Size { cols: 40, rows: 10 });
    let pty_bus = Bus::new(
        vec![mock_screen.pty_receiver.take().unwrap()],
        Some(&mock_screen.to_screen),
        Some(&mock_screen.to_pty),
        Some(&mock_screen.to_plugin),
        Some(&mock_screen.to_server),
        Some(&mock_screen.to_pty_writer),
        Some(&mock_screen.to_background_jobs),
        Some(Box::new(mock_screen.os_input.clone())),
    );
    let pty = Pty::new(pty_bus, false, None, None);
    let pty_thread =
        std::thread::spawn(move || pty_thread_main(pty, Box::default()).expect("pty thread"));
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    let run_plugin =
        RunPluginOrAlias::from_url("file:/path/to/global/plugin/77", &None, None, None).unwrap();
    let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
    mock_screen
        .to_screen
        .send(ScreenInstruction::NewGlobalPane {
            pane_id: PaneId::Plugin(77),
            initial_pane_title: Some("global plugin".to_owned()),
            hold_for_command: None,
            invoked_with: Some(Run::Plugin(run_plugin)),
            placement: GlobalPanePlacement {
                edge: GlobalPaneEdge::Bottom,
                size: PercentOrFixed::Fixed(3),
                length: None,
            },
            borderless: false,
            initially_hidden: false,
            should_focus: true,
            client_id: Some(1),
            completion_tx: Some(NotificationEnd::new(completion_tx)),
            set_blocking: false,
            declaration_order: None,
            split: None,
        })
        .unwrap();
    assert_eq!(
        completion_rx.blocking_recv().unwrap().affected_pane_id,
        Some(PaneId::Plugin(77))
    );

    mock_screen
        .to_screen
        .send(ScreenInstruction::ClosePane(
            PaneId::Plugin(77),
            None,
            None,
            None,
        ))
        .unwrap();

    let (failed_tx, failed_rx) = tokio::sync::oneshot::channel();
    mock_screen
        .to_screen
        .send(ScreenInstruction::NewGlobalPane {
            pane_id: PaneId::Plugin(78),
            initial_pane_title: Some("invalid global plugin".to_owned()),
            hold_for_command: None,
            invoked_with: Some(Run::Plugin(
                RunPluginOrAlias::from_url("file:/path/to/global/plugin/78", &None, None, None)
                    .unwrap(),
            )),
            placement: GlobalPanePlacement {
                edge: GlobalPaneEdge::Bottom,
                size: PercentOrFixed::Fixed(10),
                length: None,
            },
            borderless: false,
            initially_hidden: false,
            should_focus: true,
            client_id: Some(1),
            completion_tx: Some(NotificationEnd::new(failed_tx)),
            set_blocking: false,
            declaration_order: None,
            split: None,
        })
        .unwrap();
    let failed = failed_rx.blocking_recv().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    let unloaded_plugin_ids: Vec<_> = plugin_receiver
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::Unload(plugin_id) => Some(plugin_id),
            _ => None,
        })
        .collect();
    mock_screen.teardown(vec![screen_thread, pty_thread]);

    assert_eq!(failed.exit_status, Some(1));
    assert_eq!(failed.affected_pane_id, None);
    assert_eq!(unloaded_plugin_ids, vec![77, 78]);
}
