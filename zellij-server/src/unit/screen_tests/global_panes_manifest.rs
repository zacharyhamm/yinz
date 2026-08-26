use super::*;

#[test]
fn global_pane_manifest_reports_each_global_once_outside_tabs() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    add_global_terminal(&mut screen, 42);
    screen.global_panes.hide(PaneId::Terminal(42)).unwrap();

    let manifest = screen.generate_and_report_pane_state().unwrap();
    let placement = GlobalPanePlacement {
        edge: GlobalPaneEdge::Bottom,
        size: PercentOrFixed::Fixed(3),
        length: None,
    };

    assert_eq!(manifest.panes.len(), 2);
    assert!(manifest.panes.values().flatten().all(|pane| pane.id != 42));
    assert_eq!(manifest.global_panes.len(), 1);
    assert_eq!(manifest.global_panes[0].id, 42);
    assert_eq!(manifest.global_panes[0].global_placement, Some(placement));
    assert!(manifest.global_panes[0].is_hidden);

    let layout_manifest: zellij_utils::session_serialization::GlobalLayoutManifest =
        screen.get_layout_metadata(None, None).into();
    assert_eq!(layout_manifest.global_panes.len(), 1);
    assert_eq!(
        layout_manifest.global_panes[0].global_placement,
        Some(placement)
    );
    assert!(layout_manifest.global_panes[0].is_hidden);
}

#[test]
fn global_tab_only_screen_instructions_report_exact_completion_errors() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let mut mock_screen = MockScreen::new(Size { cols: 40, rows: 10 });
    let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
    let global_id = PaneId::Terminal(42);
    let (created_tx, created_rx) = tokio::sync::oneshot::channel();
    mock_screen
        .to_screen
        .send(ScreenInstruction::NewGlobalPane {
            pane_id: global_id,
            initial_pane_title: Some("global".to_owned()),
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
            completion_tx: Some(NotificationEnd::new(created_tx)),
            set_blocking: false,
            declaration_order: None,
            split: None,
        })
        .unwrap();
    created_rx.blocking_recv().unwrap();

    assert_global_tab_command_rejected(&mock_screen, "move", |completion| {
        ScreenInstruction::MovePane(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "swap", |completion| {
        ScreenInstruction::MovePaneBackwards(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "move-down", |completion| {
        ScreenInstruction::MovePaneDown(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "move-up", |completion| {
        ScreenInstruction::MovePaneUp(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "move-right", |completion| {
        ScreenInstruction::MovePaneRight(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "move-left", |completion| {
        ScreenInstruction::MovePaneLeft(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "previous-swap-layout", |completion| {
        ScreenInstruction::PreviousSwapLayout(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "next-swap-layout", |completion| {
        ScreenInstruction::NextSwapLayout(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "break", |completion| {
        ScreenInstruction::BreakPane(None, 1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "break-right", |completion| {
        ScreenInstruction::BreakPaneRight(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "break-left", |completion| {
        ScreenInstruction::BreakPaneLeft(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "floating", |completion| {
        ScreenInstruction::TogglePaneEmbedOrFloating(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "fullscreen", |completion| {
        ScreenInstruction::ToggleActiveTerminalFullscreen(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "no-ui-fullscreen", |completion| {
        ScreenInstruction::ToggleActiveTerminalNoUiFullscreen(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "pin", |completion| {
        ScreenInstruction::TogglePanePinned(1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "stack", |completion| {
        ScreenInstruction::StackPanes(vec![global_id], 1, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "coordinates", |completion| {
        ScreenInstruction::ChangeFloatingPanesCoordinates(
            vec![(global_id, FloatingPaneCoordinates::default())],
            Some(completion),
        )
    });
    assert_global_tab_command_rejected(&mock_screen, "move-id", |completion| {
        ScreenInstruction::MovePaneWithPaneIdCli(global_id, None, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "move-backwards-id", |completion| {
        ScreenInstruction::MovePaneBackwardsWithPaneId(global_id, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "fullscreen-id", |completion| {
        ScreenInstruction::ToggleFullscreenWithPaneId(global_id, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "no-ui-fullscreen-id", |completion| {
        ScreenInstruction::ToggleNoUiFullscreenWithPaneId(global_id, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "floating-id", |completion| {
        ScreenInstruction::TogglePaneEmbedOrFloatingWithPaneId(global_id, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "pin-id", |completion| {
        ScreenInstruction::TogglePanePinnedWithPaneId(global_id, Some(completion))
    });
    assert_global_tab_command_rejected(&mock_screen, "break-to-new-tab", |completion| {
        ScreenInstruction::BreakPanesToNewTab {
            pane_ids: vec![global_id],
            default_shell: None,
            should_change_focus_to_new_tab: false,
            new_tab_name: None,
            client_id: 1,
            completion_tx: Some(completion),
        }
    });
    assert_global_tab_command_rejected(&mock_screen, "break-to-tab", |completion| {
        ScreenInstruction::BreakPanesToTabWithIndex {
            pane_ids: vec![global_id],
            tab_index: 0,
            should_change_focus_to_new_tab: false,
            client_id: 1,
            completion_tx: Some(completion),
        }
    });
    assert_global_tab_command_rejected(&mock_screen, "break-to-tab-id", |completion| {
        ScreenInstruction::BreakPanesToTabWithId {
            pane_ids: vec![global_id],
            tab_id: 0,
            should_change_focus_to_target_tab: false,
            client_id: 1,
            completion_tx: Some(completion),
        }
    });

    mock_screen.teardown(vec![screen_thread]);
}

#[test]
fn global_pane_commands_resolve_generic_ids_and_reject_tab_only_ids() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement, Resize, ResizeStrategy};

    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    add_global_terminal(&mut screen, 42);
    let global_id = PaneId::Terminal(42);

    screen
        .focus_pane_with_id(global_id, false, false, 1)
        .unwrap();
    assert_eq!(screen.active_pane_id(1), Some(global_id));

    let (pty_writer_tx, pty_writer_rx) =
        channels::unbounded::<(PtyWriteInstruction, ErrorContext)>();
    screen.bus.senders.to_pty_writer = Some(SenderWithContext::new(pty_writer_tx));
    screen
        .write_to_pane_id(
            &None,
            b"global input".to_vec(),
            false,
            global_id,
            Some(1),
            None,
        )
        .unwrap();
    assert!(matches!(
        pty_writer_rx.recv().unwrap().0,
        PtyWriteInstruction::Write(bytes, 42, None) if bytes == b"global input"
    ));

    screen
        .with_pane(global_id, |pane| {
            pane.handle_pty_bytes(b"global command output".to_vec());
            pane.rename(b"renamed global".to_vec());
        })
        .unwrap();
    assert!(screen
        .with_pane(global_id, |pane| pane.dump_screen(false, None))
        .unwrap()
        .contains("global command output"));
    assert_eq!(
        screen
            .with_pane(global_id, |pane| pane.current_title())
            .unwrap(),
        "renamed global"
    );

    screen
        .resize_pane_with_id(ResizeStrategy::new(Resize::Increase, None), global_id)
        .unwrap();
    assert_eq!(
        screen.global_pane_placement(global_id).unwrap().size,
        PercentOrFixed::Fixed(4)
    );

    screen.hide_pane(global_id).unwrap();
    assert!(screen.global_panes.is_hidden(global_id).unwrap());
    assert_eq!(screen.active_pane_id(1), Some(PaneId::Terminal(1)));
    screen.show_pane(global_id).unwrap();
    assert!(!screen.global_panes.is_hidden(global_id).unwrap());

    let placement = GlobalPanePlacement {
        edge: GlobalPaneEdge::Right,
        size: PercentOrFixed::Fixed(6),
        length: None,
    };
    screen
        .set_global_pane_placement(global_id, placement)
        .unwrap();
    assert_eq!(screen.global_pane_placement(global_id), Some(placement));

    let (pty_tx, pty_rx) = channels::unbounded::<(PtyInstruction, ErrorContext)>();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));
    let rerun = RunCommand {
        command: PathBuf::from("rerun-global"),
        ..Default::default()
    };
    screen
        .with_pane(global_id, |pane| pane.hold(Some(0), false, rerun.clone()))
        .unwrap();
    screen.rerun_command_pane_with_id(42, None);
    assert!(matches!(
        pty_rx.recv().unwrap().0,
        PtyInstruction::ReRunCommandInPane(PaneId::Terminal(42), command, None)
            if command == rerun
    ));

    let snapshot = (
        screen.global_pane_placement(global_id),
        screen.global_panes.is_hidden(global_id).ok(),
        screen
            .with_pane(global_id, |pane| pane.current_title())
            .unwrap(),
    );
    for command in [
        "move",
        "swap",
        "move-to-tab",
        "break",
        "stack",
        "floating",
        "fullscreen",
        "pin",
    ] {
        assert_eq!(
            screen
                .reject_global_for_tab_operation(global_id)
                .unwrap_err()
                .to_string(),
            "global pane has no tab",
            "{command}"
        );
        assert_eq!(
            (
                screen.global_pane_placement(global_id),
                screen.global_panes.is_hidden(global_id).ok(),
                screen
                    .with_pane(global_id, |pane| pane.current_title())
                    .unwrap(),
            ),
            snapshot,
            "{command} mutated the global pane"
        );
    }
}

#[test]
fn global_plugin_commands_use_the_global_first_resolver() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    let run_plugin =
        RunPluginOrAlias::from_url("file:/path/to/global/plugin/77", &None, None, None).unwrap();
    add_matching_global_plugin(&mut screen, 77, run_plugin, false);
    let global_id = PaneId::Plugin(77);
    while plugin_rx.try_recv().is_ok() {}

    screen
        .focus_pane_with_id(global_id, false, false, 1)
        .unwrap();
    assert_eq!(screen.active_pane_id(1), Some(global_id));
    while plugin_rx.try_recv().is_ok() {}
    screen
        .write_to_pane_id(&None, b"x".to_vec(), false, global_id, Some(1), None)
        .unwrap();
    assert!(matches!(
        plugin_rx.recv().unwrap().0,
        PluginInstruction::Update(updates)
            if updates.iter().any(|(plugin_id, client_id, event)| {
                *plugin_id == Some(77)
                    && *client_id == Some(1)
                    && matches!(event, Event::Key(_))
            })
    ));

    screen
        .with_pane(global_id, |pane| pane.rename(b"renamed plugin".to_vec()))
        .unwrap();
    assert_eq!(
        screen
            .with_pane(global_id, |pane| pane.current_title())
            .unwrap(),
        "renamed plugin"
    );
    screen.hide_pane(global_id).unwrap();
    assert_eq!(screen.active_pane_id(1), Some(PaneId::Terminal(1)));
    screen.show_pane(global_id).unwrap();

    let placement = GlobalPanePlacement {
        edge: GlobalPaneEdge::Top,
        size: PercentOrFixed::Fixed(2),
        length: None,
    };
    screen
        .set_global_pane_placement(global_id, placement)
        .unwrap();
    assert_eq!(screen.global_pane_placement(global_id), Some(placement));
    assert_eq!(
        screen
            .reject_global_for_tab_operation(global_id)
            .unwrap_err()
            .to_string(),
        "global pane has no tab"
    );

    assert!(screen.close_global_pane(global_id, None).unwrap());
    assert!(screen.global_panes.pane(global_id).is_none());
}

#[test]
fn switching_tabs_retains_global_pane_focus() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    new_tab(&mut screen, 2, 1);
    add_global_terminal(&mut screen, 42);

    screen
        .focus_pane_with_id(PaneId::Terminal(42), false, false, 1)
        .unwrap();
    assert_eq!(screen.active_pane_id(1), Some(PaneId::Terminal(42)));

    screen.switch_active_tab(0, None, true, 1).unwrap();

    assert_eq!(
        screen.global_panes.focused_pane_id(1),
        Some(PaneId::Terminal(42))
    );
    assert_eq!(screen.active_pane_id(1), Some(PaneId::Terminal(42)));
    // the destination tab still remembers its own focus for when the global pane is released
    assert_eq!(
        screen.tabs.get(&0).unwrap().get_active_pane_id(1),
        Some(PaneId::Terminal(1))
    );
}

#[test]
fn global_focus_keeps_tab_focus_remembered_but_reports_the_tab_inactive() {
    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    let (pty_tx, pty_rx) = channels::unbounded::<(PtyInstruction, ErrorContext)>();
    let (server_tx, server_rx) = channels::unbounded::<(ServerInstruction, ErrorContext)>();
    screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));
    screen.bus.senders.to_server = Some(SenderWithContext::new(server_tx));
    new_tab(&mut screen, 1, 0);
    add_global_terminal(&mut screen, 42);

    screen.render_to_clients().unwrap();
    while server_rx.try_recv().is_ok() {}
    while pty_rx.try_recv().is_ok() {}

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(8, 1)), 1);
    screen.render_to_clients().unwrap();

    let focused_render = server_rx.recv().unwrap().0;
    let focused_cursor = take_snapshots_and_cursor_coordinates_from_render_events(
        [&focused_render].into_iter(),
        size,
    )[0]
    .0;

    assert_eq!(
        screen.tabs.get(&0).unwrap().get_active_pane_id(1),
        Some(PaneId::Terminal(1))
    );
    assert!(pty_rx
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, PtyInstruction::UpdateActivePane(None, 1))));

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(1, 1)), 1);
    screen.render_to_clients().unwrap();

    let tab_focused_render = server_rx.recv().unwrap().0;
    let tab_focused_cursor = take_snapshots_and_cursor_coordinates_from_render_events(
        [&tab_focused_render].into_iter(),
        size,
    )[0]
    .0;

    assert_eq!(
        screen.tabs.get(&0).unwrap().get_active_pane_id(1),
        Some(PaneId::Terminal(1))
    );
    assert!(pty_rx.try_iter().any(|(instruction, _)| matches!(
        instruction,
        PtyInstruction::UpdateActivePane(Some(PaneId::Terminal(1)), 1)
    )));
    assert_eq!(focused_cursor, Some((0, 7)));
    assert_eq!(tab_focused_cursor, Some((0, 0)));
}

#[test]
fn mouse_click_focuses_global_pane_and_center_click_restores_tab_focus() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    add_global_terminal(&mut screen, 42);

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(8, 1)), 1);
    assert_eq!(screen.active_pane_id(1), Some(PaneId::Terminal(42)));

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(1, 1)), 1);
    assert_eq!(screen.global_panes.focused_pane_id(1), None);
    assert_eq!(screen.active_pane_id(1), Some(PaneId::Terminal(1)));
}

#[test]
fn global_terminal_mouse_uses_content_coordinates_and_ignores_frames() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    add_global_terminal(&mut screen, 42);
    screen
        .global_panes
        .set_pane_frame_style(PaneFrameStyle::Full)
        .unwrap();
    let (pty_writer_tx, pty_writer_rx) =
        channels::unbounded::<(PtyWriteInstruction, ErrorContext)>();
    screen.bus.senders.to_pty_writer = Some(SenderWithContext::new(pty_writer_tx));
    screen
        .global_panes
        .handle_pty_bytes(42, b"\x1b[?1000h\x1b[?1006h".to_vec());

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(8, 3)), 1);
    assert!(matches!(
        pty_writer_rx.recv().unwrap().0,
        PtyWriteInstruction::Write(bytes, 42, None) if bytes == b"\x1b[<0;3;1M"
    ));

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(7, 3)), 1);
    assert!(pty_writer_rx.try_recv().is_err());
}

#[test]
fn global_terminal_mouse_drag_uses_the_existing_selection_behavior() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    new_tab(&mut screen, 1, 0);
    add_global_terminal(&mut screen, 42);
    screen
        .global_panes
        .handle_pty_bytes(42, b"select me".to_vec());

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(7, 0)), 1);
    screen.handle_mouse_event(MouseEvent::new_left_motion_event(Position::new(7, 6)), 1);
    screen.handle_mouse_event(MouseEvent::new_left_release_event(Position::new(7, 6)), 1);

    assert_eq!(
        screen
            .with_pane(PaneId::Terminal(42), |pane| pane.get_selected_text(1))
            .flatten()
            .as_deref(),
        Some("select")
    );
    assert!(plugin_rx
        .try_iter()
        .any(|(instruction, _)| match instruction {
            PluginInstruction::Update(updates) => updates
                .into_iter()
                .any(|(_, _, event)| matches!(event, Event::CopyToClipboard(_))),
            _ => false,
        }));
}

#[test]
fn global_terminal_mouse_wheel_sends_cursor_keys_in_alternate_mode() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    add_global_terminal(&mut screen, 42);
    let (pty_writer_tx, pty_writer_rx) =
        channels::unbounded::<(PtyWriteInstruction, ErrorContext)>();
    screen.bus.senders.to_pty_writer = Some(SenderWithContext::new(pty_writer_tx));
    screen
        .global_panes
        .handle_pty_bytes(42, b"\x1b[?1049h".to_vec());

    screen.handle_mouse_event(MouseEvent::new_scroll_up_event(Position::new(7, 1)), 1);

    let writes: Vec<_> = pty_writer_rx
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PtyWriteInstruction::Write(bytes, 42, None) => Some(bytes),
            _ => None,
        })
        .collect();
    assert_eq!(writes, vec![b"\x1b[A".to_vec(); 3]);
}

#[test]
fn global_plugin_mouse_and_paste_use_the_existing_plugin_events() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    let run_plugin =
        RunPluginOrAlias::from_url("file:/path/to/global/plugin/77", &None, None, None).unwrap();
    add_matching_global_plugin(&mut screen, 77, run_plugin, false);
    screen
        .global_panes
        .set_pane_frame_style(PaneFrameStyle::Full)
        .unwrap();
    while plugin_rx.try_recv().is_ok() {}

    for event in [
        MouseEvent::new_left_press_event(Position::new(2, 2)),
        MouseEvent::new_left_motion_event(Position::new(3, 3)),
        MouseEvent::new_left_release_event(Position::new(3, 3)),
        MouseEvent::new_right_press_event(Position::new(2, 2)),
        MouseEvent::new_scroll_up_event(Position::new(2, 2)),
        MouseEvent::new_scroll_down_event(Position::new(2, 2)),
    ] {
        screen.handle_mouse_event(event, 1);
    }
    screen
        .paste_to_pane_id(b"global paste".to_vec(), PaneId::Plugin(77), Some(1), None)
        .unwrap();

    let events: Vec<_> = plugin_rx
        .try_iter()
        .flat_map(|(instruction, _)| match instruction {
            PluginInstruction::Update(updates) => updates
                .into_iter()
                .filter_map(|(plugin_id, client_id, event)| {
                    (plugin_id == Some(77) && client_id == Some(1)).then_some(event)
                })
                .collect::<Vec<_>>(),
            _ => vec![],
        })
        .collect();
    assert!(
        events.contains(&Event::Mouse(Mouse::LeftClick(1, 1))),
        "{events:?}"
    );
    assert!(
        events.contains(&Event::Mouse(Mouse::Hold(2, 2))),
        "{events:?}"
    );
    assert!(
        events.contains(&Event::Mouse(Mouse::Release(2, 2))),
        "{events:?}"
    );
    assert!(
        events.contains(&Event::Mouse(Mouse::RightClick(1, 1))),
        "{events:?}"
    );
    assert!(
        events.contains(&Event::Mouse(Mouse::ScrollUp(3))),
        "{events:?}"
    );
    assert!(
        events.contains(&Event::Mouse(Mouse::ScrollDown(3))),
        "{events:?}"
    );
    assert!(
        events.contains(&Event::PastedText("global paste".to_owned())),
        "{events:?}"
    );
}

/// A plugin opts into text selection with `set_self_mouse_selection_support`, which arrives as
/// `ScreenInstruction::SetMouseSelectionSupport`. That used to search only the tabs, so a global
/// pane - which belongs to no tab - never got the opt-in and its drags stayed click events.
#[test]
fn global_plugin_pane_selection_copies_once_the_plugin_opts_in() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    new_tab(&mut screen, 1, 0);
    let run_plugin =
        RunPluginOrAlias::from_url("file:/path/to/global/plugin/77", &None, None, None).unwrap();
    add_matching_global_plugin(&mut screen, 77, run_plugin, false);
    screen
        .global_panes
        .handle_plugin_bytes(77, 1, b"abc".to_vec());

    // the lookup the `SetMouseSelectionSupport` handler does - `None` here means the instruction
    // gets parked in `pending_events_waiting_for_tab` and the opt-in is lost
    assert!(
        screen
            .with_pane(PaneId::Plugin(77), |pane| pane
                .set_mouse_selection_support(true))
            .is_some(),
        "the plugin's mouse selection opt-in must reach a global pane"
    );
    while plugin_rx.try_recv().is_ok() {}

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(0, 0)), 1);
    screen.handle_mouse_event(MouseEvent::new_left_motion_event(Position::new(0, 3)), 1);
    screen.handle_mouse_event(MouseEvent::new_left_release_event(Position::new(0, 3)), 1);

    assert_eq!(
        screen
            .with_pane(PaneId::Plugin(77), |pane| pane.get_selected_text(1))
            .flatten()
            .as_deref(),
        Some("abc")
    );
    assert!(plugin_rx
        .try_iter()
        .any(|(instruction, _)| match instruction {
            PluginInstruction::Update(updates) => updates
                .into_iter()
                .any(|(_, _, event)| matches!(event, Event::CopyToClipboard(_))),
            _ => false,
        }));
}

/// A left global pane reserves its own last column for the tab separator, so a drag to end-of-line
/// crosses it. That cell is "on the frame", and bailing out there used to drop the release and
/// leave the client's mouse captured by the global pane.
#[test]
fn global_terminal_selection_survives_a_release_on_the_frame() {
    use zellij_utils::data::{GlobalPaneEdge, GlobalPanePlacement};

    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    add_global_terminal_on_edge(&mut screen, 42, GlobalPaneEdge::Left);
    screen
        .set_global_pane_placement(
            PaneId::Terminal(42),
            GlobalPanePlacement {
                edge: GlobalPaneEdge::Left,
                size: PercentOrFixed::Fixed(11),
                length: None,
            },
        )
        .unwrap();
    screen
        .global_panes
        .handle_pty_bytes(42, b"select me".to_vec());
    // 11 columns wide, the last of which is the separator the tab draws into
    assert_eq!(
        screen
            .with_pane(PaneId::Terminal(42), |pane| pane.get_content_columns())
            .unwrap(),
        10
    );

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(0, 0)), 1);
    screen.handle_mouse_event(MouseEvent::new_left_motion_event(Position::new(0, 6)), 1);
    screen.handle_mouse_event(MouseEvent::new_left_release_event(Position::new(0, 10)), 1);

    assert_eq!(
        screen
            .with_pane(PaneId::Terminal(42), |pane| pane.get_selected_text(1))
            .flatten()
            .as_deref(),
        Some("select me")
    );
    // and the client is no longer captured: a motion over the tab area is the tab's again
    assert_eq!(
        screen
            .global_panes
            .pane_id_for_mouse_event(&MouseEvent::new_left_motion_event(Position::new(5, 30)), 1),
        None
    );
}

/// A release outside the pane is clamped into the content box, the same way the tab path clamps it.
#[test]
fn global_terminal_selection_clamps_a_release_outside_the_pane() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    add_global_terminal(&mut screen, 42);
    screen
        .global_panes
        .handle_pty_bytes(42, b"line one\r\nline two".to_vec());

    // the bottom pane starts at row 7 - drag up from its second row and release above it entirely
    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(8, 4)), 1);
    screen.handle_mouse_event(MouseEvent::new_left_release_event(Position::new(2, 4)), 1);

    assert_eq!(
        screen
            .with_pane(PaneId::Terminal(42), |pane| pane.get_selected_text(1))
            .flatten()
            .as_deref(),
        Some(" one\nline")
    );
}

/// A wheel event is also a `Press`, and used to reset the drag bookkeeping mid-selection.
#[test]
fn global_terminal_wheel_does_not_cancel_a_selection_drag() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    add_global_terminal(&mut screen, 42);
    screen
        .global_panes
        .handle_pty_bytes(42, b"select me".to_vec());

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(7, 0)), 1);
    screen.handle_mouse_event(MouseEvent::new_scroll_up_event(Position::new(7, 3)), 1);
    screen.handle_mouse_event(MouseEvent::new_left_motion_event(Position::new(7, 9)), 1);
    screen.handle_mouse_event(MouseEvent::new_left_release_event(Position::new(7, 9)), 1);

    assert_eq!(
        screen
            .with_pane(PaneId::Terminal(42), |pane| pane.get_selected_text(1))
            .flatten()
            .as_deref(),
        Some("select me")
    );
}

/// The `Copy` action reads the focused pane, which may be a global one.
#[test]
fn copy_action_copies_the_selection_of_a_focused_global_pane() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    new_tab(&mut screen, 1, 0);
    add_global_terminal(&mut screen, 42);
    screen.copy_options.copy_on_select = false;
    screen
        .global_panes
        .handle_pty_bytes(42, b"select me".to_vec());

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(7, 0)), 1);
    screen.handle_mouse_event(MouseEvent::new_left_release_event(Position::new(7, 6)), 1);
    while plugin_rx.try_recv().is_ok() {}

    screen.copy_selection(1).unwrap();

    assert!(plugin_rx
        .try_iter()
        .any(|(instruction, _)| match instruction {
            PluginInstruction::Update(updates) => updates
                .into_iter()
                .any(|(_, _, event)| matches!(event, Event::CopyToClipboard(_))),
            _ => false,
        }));
}

/// The status bar drops its "copied to clipboard" message on `InputReceived`, so the event that
/// caused the copy must not send one - every other one must.
#[test]
fn global_terminal_copy_leaves_the_clipboard_message_alone() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    new_tab(&mut screen, 1, 0);
    add_global_terminal(&mut screen, 42);
    screen
        .global_panes
        .handle_pty_bytes(42, b"select me".to_vec());
    // stand in for the status bar: something that wants to hear about this client's input
    screen
        .background_plugin_subscriptions
        .insert((99, 1), HashSet::from([EventType::InputReceived]));

    let input_received_since = |rx: &channels::Receiver<(PluginInstruction, ErrorContext)>| {
        rx.try_iter().any(|(instruction, _)| match instruction {
            PluginInstruction::Update(updates) => updates
                .into_iter()
                .any(|(_, _, event)| matches!(event, Event::InputReceived)),
            _ => false,
        })
    };

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(7, 0)), 1);
    while plugin_rx.try_recv().is_ok() {}
    screen.handle_mouse_event(MouseEvent::new_left_release_event(Position::new(7, 6)), 1);
    assert!(!input_received_since(&plugin_rx), "copy was announced away");

    screen.handle_mouse_event(MouseEvent::new_left_press_event(Position::new(7, 0)), 1);
    assert!(input_received_since(&plugin_rx));
}

#[test]
fn global_terminal_and_plugin_output_is_delivered_to_dock_panes() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    let run_plugin =
        RunPluginOrAlias::from_url("file:/path/to/output/plugin", &None, None, None).unwrap();
    add_global_terminal(&mut screen, 42);
    add_matching_global_plugin(&mut screen, 77, run_plugin, false);
    screen
        .global_panes
        .resize(screen.surface_size().into())
        .unwrap();

    assert!(screen
        .global_panes
        .handle_pty_bytes(42, b"terminal dock output".to_vec()));
    assert!(screen
        .global_panes
        .handle_plugin_bytes(77, 1, b"plugin dock output".to_vec()));

    let terminal_contents = screen
        .global_panes
        .pane(PaneId::Terminal(42))
        .unwrap()
        .pane_contents(None, false, None)
        .viewport
        .join("\n");
    let plugin_contents = screen
        .global_panes
        .pane(PaneId::Plugin(77))
        .unwrap()
        .pane_contents(Some(1), false, None)
        .viewport
        .join("\n");
    assert!(terminal_contents.contains("terminal dock output"));
    assert!(plugin_contents
        .replace('\n', "")
        .contains("plugin dock output"));
}

#[test]
fn global_process_exit_removes_the_dock_and_restores_tab_geometry() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);
    add_global_terminal(&mut screen, 42);
    assert_eq!(screen.tabs.get(&0).unwrap().size.rows, 7);

    assert!(screen
        .close_global_pane(PaneId::Terminal(42), None)
        .unwrap());

    assert!(screen.global_panes.pane(PaneId::Terminal(42)).is_none());
    assert_eq!(screen.tabs.get(&0).unwrap().size.rows, 10);
}

/// `TabInfo` reports two rectangles and a plugin has to be able to tell them apart: the display
/// area is the tab centre, what is left once the global-pane strips are carved off, and the
/// session surface is what they were carved out of. An overlay's coordinates resolve against the
/// second (`OverlayPanes::surface`), so a plugin placing one against the first - hocus's column
/// did - comes up short by exactly the strips' thickness.
#[test]
fn tab_info_reports_the_session_surface_beside_the_tabs_display_area() {
    let size = Size { cols: 40, rows: 10 };
    let mut screen = create_non_mirrored_screen(size);
    new_tab(&mut screen, 1, 0);
    add_global_terminal(&mut screen, 42);

    let tab_infos = screen.generate_and_report_tab_state().unwrap();
    let tab_info = tab_infos.first().expect("one tab");

    assert!(
        tab_info.session_surface_rows > tab_info.display_area_rows,
        "a bottom strip takes rows off the tab centre and the surface keeps them: surface {} \
         against display area {}",
        tab_info.session_surface_rows,
        tab_info.display_area_rows,
    );
    assert_eq!(
        tab_info.session_surface_columns, tab_info.display_area_columns,
        "and it takes nothing off the other axis"
    );
    assert_eq!(
        (
            tab_info.session_surface_rows,
            tab_info.session_surface_columns
        ),
        (size.rows, size.cols),
        "the surface is the whole screen - there is nothing else on it to account for"
    );
}

/// The other half of the contract: with nothing carved off, the two rectangles are the same one,
/// so a plugin reading the surface is never worse off than one reading the display area.
#[test]
fn tab_info_reports_one_rectangle_twice_when_there_are_no_global_panes() {
    let mut screen = create_non_mirrored_screen(Size { cols: 40, rows: 10 });
    new_tab(&mut screen, 1, 0);

    let tab_infos = screen.generate_and_report_tab_state().unwrap();
    let tab_info = tab_infos.first().expect("one tab");

    assert_eq!(
        (
            tab_info.session_surface_rows,
            tab_info.session_surface_columns
        ),
        (tab_info.display_area_rows, tab_info.display_area_columns),
    );
}
