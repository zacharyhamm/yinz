use super::*;
use yinz_test_support::Watched;
use yinz_types::internal_ids::TabId;

#[test]
fn moving_a_plugin_in_place_replaces_and_restores_the_active_pane() {
    for (replace_floating, overlay, empty_destination) in [
        (false, None, false),
        (true, None, false),
        (false, None, true),
        (false, Some(OverlayPlacement::dismissible(None)), false),
        (true, Some(OverlayPlacement::dismissible(None)), false),
        (false, Some(OverlayPlacement::modal(None)), false),
        (true, Some(OverlayPlacement::modal(None)), false),
        (false, Some(OverlayPlacement::dismissible(None)), true),
        (false, Some(OverlayPlacement::modal(None)), true),
    ] {
        let client_id = ClientId::new(1);
        let plugin_id = PaneId::Plugin(crate::PluginId::new(7));
        let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 20 });
        let (plugin_sender, _plugin_receiver) = channels::unbounded();
        screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_sender));
        new_tab(&mut screen, 1, TabId::new(0));
        new_tab(&mut screen, 2, TabId::new(1));
        let run_plugin = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
            "file:/path/to/fake/plugin",
            &None,
            None,
            None,
        )
        .unwrap();
        screen
            .tabs
            .get_mut(&TabId::new(0))
            .unwrap()
            .new_pane(
                plugin_id,
                None,
                Some(Run::Plugin(run_plugin.clone())),
                false,
                true,
                NewPanePlacement::Tiled {
                    direction: None,
                    borderless: None,
                },
                Some(client_id),
                None,
            )
            .unwrap();
        let destination = screen.get_active_tab_mut(client_id).unwrap();
        if replace_floating {
            destination
                .new_pane(
                    PaneId::Terminal(TerminalId::new(3)),
                    None,
                    None,
                    false,
                    true,
                    NewPanePlacement::Floating(None),
                    Some(client_id),
                    None,
                )
                .unwrap();
        }
        if empty_destination {
            destination.close_pane(PaneId::Terminal(TerminalId::new(2)), false, None);
        }
        if empty_destination && overlay.is_none() {
            assert!(screen
                .focus_plugin_pane(&run_plugin, false, true, true, client_id, &mut None)
                .is_err());
            let source = screen.tabs.get_mut(&TabId::new(0)).unwrap();
            assert!(source.has_non_suppressed_pane_with_pid(&plugin_id));
            let plugin = source.extract_pane(plugin_id, true).unwrap();
            let destination = screen.get_active_tab_mut(client_id).unwrap();
            assert!(!destination.has_pane_with_pid(&plugin_id));
            destination.park_pane(plugin);
            assert!(destination
                .focus_pane_with_id(plugin_id, false, true, client_id)
                .is_err());
            assert!(destination.has_pane_with_pid(&plugin_id));
            assert!(!destination.has_non_suppressed_pane_with_pid(&plugin_id));
            destination
                .focus_pane_with_id(plugin_id, true, false, client_id)
                .unwrap();
            assert_eq!(destination.get_active_pane_id(client_id), Some(plugin_id));
            continue;
        }
        if let Some(placement) = overlay {
            destination
                .new_pane(
                    PaneId::Terminal(TerminalId::new(4)),
                    None,
                    None,
                    false,
                    true,
                    NewPanePlacement::Overlay(placement),
                    Some(client_id),
                    None,
                )
                .unwrap();
            assert_eq!(
                destination.get_active_pane_id(client_id),
                Some(PaneId::Terminal(TerminalId::new(4)))
            );
        }
        let replaced_id = destination.get_active_pane_id(client_id).unwrap();
        let replaced_geom = destination
            .get_pane_with_id(replaced_id)
            .unwrap()
            .current_geom();
        let replaced_overlay = destination.overlay_placement_for(replaced_id);
        let underlying_panes: Vec<_> = destination
            .get_all_pane_ids()
            .into_iter()
            .filter(|id| *id != replaced_id)
            .map(|id| (id, destination.get_pane_with_id(id).unwrap().current_geom()))
            .collect();

        assert!(screen
            .focus_plugin_pane(&run_plugin, false, true, true, client_id, &mut None)
            .unwrap());
        assert!(!screen.tabs[&TabId::new(0)].has_pane_with_pid(&plugin_id));
        let destination = screen.get_active_tab_mut(client_id).unwrap();
        assert_eq!(destination.get_active_pane_id(client_id), Some(plugin_id));
        assert_eq!(destination.are_floating_panes_visible(), replace_floating);
        assert_eq!(
            destination
                .get_pane_with_id(plugin_id)
                .unwrap()
                .current_geom(),
            replaced_geom
        );
        assert!(destination.has_pane_with_pid(&replaced_id));
        assert!(!destination.has_non_suppressed_pane_with_pid(&replaced_id));
        assert_eq!(
            destination.overlay_placement_for(plugin_id),
            replaced_overlay
        );
        for (id, geom) in &underlying_panes {
            assert!(destination.has_non_suppressed_pane_with_pid(id));
            assert_eq!(
                destination.get_pane_with_id(*id).unwrap().current_geom(),
                *geom
            );
        }

        destination.close_pane(plugin_id, false, None);
        assert_eq!(destination.get_active_pane_id(client_id), Some(replaced_id));
        assert!(destination.has_non_suppressed_pane_with_pid(&replaced_id));
        assert!(!destination.has_pane_with_pid(&plugin_id));
        assert_eq!(
            destination.overlay_placement_for(replaced_id),
            replaced_overlay
        );
        assert_eq!(
            destination
                .get_pane_with_id(replaced_id)
                .unwrap()
                .current_geom(),
            replaced_geom
        );
        assert_eq!(destination.are_floating_panes_visible(), replace_floating);
    }
}

#[test]
pub fn screen_can_suppress_pane() {
    let size = Size { cols: 80, rows: 20 };
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let _ = mock_screen.to_screen.send(ScreenInstruction::SuppressPane(
        PaneId::Terminal(crate::TerminalId::new(1)),
        crate::ClientId::new(1),
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);

    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(snapshot.to_string());
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn screen_can_break_pane_to_a_new_tab() {
    let size = Size { cols: 80, rows: 20 };
    let pane_to_break_free = TiledPaneLayout {
        name: Some("pane_to_break_free".to_owned()),
        ..Default::default()
    };
    let pane_to_stay = TiledPaneLayout {
        name: Some("pane_to_stay".to_owned()),
        ..Default::default()
    };
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![pane_to_break_free, pane_to_stay],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let _ = mock_screen.to_screen.send(ScreenInstruction::BreakPane(
        Default::default(),
        crate::ClientId::new(1),
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // we send ApplyLayout, because in prod this is eventually received after the message traverses
    // through the plugin and pty threads (to open extra stuff we need in the layout, eg. the
    // default plugins)
    let _ = mock_screen.to_screen.send(ScreenInstruction::ApplyLayout(
        TiledPaneLayout::default(),
        vec![], // floating_panes_layout
        Default::default(),
        vec![], // floating panes ids
        vec![], // adopted agents
        Default::default(),
        TabId::new(1),
        true,
        crate::ClientId::new(1),
        None,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // move back to make sure the other pane is in the previous tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusLeftOrPreviousTab(
            crate::ClientId::new(1),
            None,
        ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // move forward to make sure the broken pane is in the previous tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusRightOrNextTab(
            crate::ClientId::new(1),
            None,
        ));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(snapshot.to_string());
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
fn screen_can_break_last_selectable_pane_to_a_new_tab() {
    for break_count in [1, 2] {
        let size = Size { cols: 80, rows: 20 };
        let client_id = ClientId::new(1);
        let mut mock_screen = MockScreen::new(size);
        let screen_thread = mock_screen.run(Some(TiledPaneLayout::default()), vec![]);
        let received_server_instructions = Watched::new(vec![]);
        let server_receiver = mock_screen.server_receiver.take().unwrap();
        let server_thread = log_actions_in_thread!(
            received_server_instructions,
            ServerInstruction::KillSession,
            server_receiver
        );
        let received_plugin_instructions = Watched::new(vec![]);
        let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
        let plugin_thread = log_actions_in_thread!(
            received_plugin_instructions,
            PluginInstruction::Exit,
            plugin_receiver
        );

        for _ in 0..break_count {
            mock_screen
                .to_screen
                .send(ScreenInstruction::BreakPane(None, client_id, None))
                .unwrap();
        }
        let layout = received_plugin_instructions.wait_until(
            "the broken pane's destination layout",
            yinz_test_support::default_timeout(),
            |instructions| {
                instructions
                    .iter()
                    .find_map(|instruction| match instruction {
                        PluginInstruction::NewTab(_, _, Some(layout), _, tab_id, ..)
                            if *tab_id == TabId::new(1) =>
                        {
                            Some(layout.clone())
                        },
                        _ => None,
                    })
            },
        );
        // This FIFO query must succeed after every breakout, before the layout is ready.
        let (tx, rx) = crossbeam::channel::bounded(1);
        mock_screen
            .to_screen
            .send(ScreenInstruction::GetTabInfo {
                tab_id: TabId::new(1),
                response_channel: tx,
            })
            .unwrap();
        assert!(rx
            .recv_timeout(yinz_test_support::default_timeout())
            .unwrap()
            .is_some());
        mock_screen
            .to_screen
            .send(ScreenInstruction::ApplyLayout(
                layout,
                vec![],
                vec![], // The existing pane supplies the layout's terminal.
                vec![],
                vec![],
                HashMap::new(),
                TabId::new(1),
                true,
                client_id,
                None,
                None,
            ))
            .unwrap();
        // Queries follow ApplyLayout and settlement on the same FIFO.
        let (tx, rx) = crossbeam::channel::bounded(1);
        mock_screen
            .to_screen
            .send(ScreenInstruction::GetFocusedPaneInfo {
                client_id,
                response_channel: tx,
            })
            .unwrap();
        assert_eq!(
            rx.recv_timeout(yinz_test_support::default_timeout())
                .unwrap(),
            yinz_types::data::GetFocusedPaneInfoResponse::Ok {
                tab_id: TabId::new(1),
                pane_id: PaneId::Terminal(TerminalId::new(0)),
            }
        );
        let (tx, rx) = crossbeam::channel::bounded(1);
        mock_screen
            .to_screen
            .send(ScreenInstruction::GetTabInfo {
                tab_id: TabId::new(0),
                response_channel: tx,
            })
            .unwrap();
        assert!(rx
            .recv_timeout(yinz_test_support::default_timeout())
            .unwrap()
            .is_none());

        mock_screen.teardown(vec![server_thread, plugin_thread, screen_thread]);
        assert_eq!(
            received_plugin_instructions
                .lock()
                .iter()
                .filter_map(|instruction| match instruction {
                    PluginInstruction::NewTab(_, _, _, _, tab_id, ..)
                        if *tab_id != TabId::new(0) =>
                    {
                        Some(*tab_id)
                    },
                    _ => None,
                })
                .collect::<Vec<_>>(),
            vec![TabId::new(1)],
            "only one breakout destination is requested"
        );
        assert!(!received_server_instructions
            .lock()
            .iter()
            .any(|instruction| { matches!(instruction, ServerInstruction::Render(None)) }));
        assert!(!mock_screen
            .pty_receiver
            .as_ref()
            .unwrap()
            .try_iter()
            .any(|(instruction, _)| match instruction {
                PtyInstruction::ClosePane(_, _) => true,
                PtyInstruction::CloseTab(ids) => !ids.is_empty(),
                _ => false,
            }));
    }
}

#[test]
fn breaking_last_selectable_pane_preserves_work_while_its_layout_loads() {
    for (another_tab, hidden_work) in [(false, false), (true, false), (false, true), (true, true)] {
        let client_id = ClientId::new(1);
        let source = TabId::new(1);
        let moved = PaneId::Terminal(TerminalId::new(1));
        let hidden = PaneId::Terminal(TerminalId::new(2));
        let mut screen = create_non_mirrored_screen(Size { cols: 80, rows: 20 });
        let server_receiver = screen_wired_to_server(&mut screen);
        let (pty_tx, pty_rx) = channels::unbounded();
        screen.bus.senders.to_pty = Some(SenderWithContext::new(pty_tx));
        let (screen_tx, screen_rx) = channels::unbounded();
        screen.bus.senders.to_screen = Some(SenderWithContext::new(screen_tx));
        let (plugin_tx, plugin_rx) = channels::unbounded();
        screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
        if another_tab {
            new_tab(&mut screen, 3, TabId::new(0));
        }
        new_tab(&mut screen, 1, source);
        screen.set_quit_notice(crate::PluginId::new(7), None, true);
        if hidden_work {
            add_pane_to_tab(&mut screen, source, hidden, false);
            screen
                .tabs
                .get_mut(&source)
                .unwrap()
                .suppress_pane(hidden, None);
        }
        let respawn = (RunCommand::default(), true);
        screen
            .tabs
            .get_mut(&source)
            .unwrap()
            .restore_pending_respawn(moved, respawn.clone());
        let destination = screen.get_new_tab_id();

        screen
            .break_pane(None, screen.default_layout.clone(), client_id)
            .unwrap();
        assert!(screen.tabs[&destination].is_pending());
        assert!(screen.tabs[&destination].has_pane_with_pid(&moved));
        assert!(!screen.tabs[&source].has_pane_with_pid(&moved));
        screen.settle_tabs().unwrap();
        assert!(screen.tabs[&destination].is_pending());
        if !another_tab {
            assert!(
                screen.tabs.contains_key(&source),
                "wait for a usable destination"
            );
            assert_eq!(screen.active_tab_ids[&client_id], source);
            assert_eq!(screen.active_pane_id(client_id), None);
            let tab_count = screen.tabs.len();
            for direction in [Direction::Right, Direction::Left] {
                screen.break_pane_to_new_tab(direction, client_id).unwrap();
                assert_eq!(screen.active_tab_ids[&client_id], source);
                assert_eq!(screen.active_pane_id(client_id), None);
                assert_eq!(screen.tabs.len(), tab_count);
                assert!(screen.tabs[&destination].is_pending());
                assert!(screen.tabs[&destination].has_pane_with_pid(&moved));
            }
        }
        assert!(!screen.replacement_pending);
        assert!(screen.replacement_tab.is_none());
        let requests: Vec<_> = plugin_rx
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                PluginInstruction::NewTab(_, _, Some(layout), _, tab_id, ..) => {
                    Some((tab_id, layout))
                },
                _ => None,
            })
            .collect();
        assert_eq!(
            requests.len(),
            1,
            "only the requested breakout tab is spawned"
        );
        let (tab_id, layout) = requests.into_iter().next().unwrap();
        assert_eq!(tab_id, destination);
        assert_eq!(layout.run_instructions_to_ignore.len(), 1);
        screen
            .apply_layout(
                layout,
                vec![],
                vec![],
                vec![],
                HashMap::new(),
                destination,
                true,
                client_id,
                None,
            )
            .unwrap();
        screen.settle_tabs().unwrap();

        assert_eq!(screen.active_tab_ids[&client_id], destination);
        assert_eq!(screen.active_pane_id(client_id), Some(moved));
        assert_eq!(screen.tabs.contains_key(&source), hidden_work);
        assert_eq!(screen.tabs.contains_key(&TabId::new(0)), another_tab);
        assert_eq!(
            screen.tabs.len(),
            1 + usize::from(another_tab) + usize::from(hidden_work)
        );
        assert_eq!(
            screen
                .tabs
                .get_mut(&destination)
                .unwrap()
                .take_pending_respawn(moved),
            Some(respawn)
        );
        if hidden_work {
            assert!(screen.tabs[&source].has_pane_with_pid(&hidden));
        }
        assert!(!server_receiver
            .try_iter()
            .any(|(instruction, _)| matches!(instruction, ServerInstruction::Render(None))));
        assert!(!screen_rx
            .try_iter()
            .any(|(instruction, _)| matches!(instruction, ScreenInstruction::LaunchPlugin(..))));
        for (instruction, _) in pty_rx.try_iter() {
            match instruction {
                PtyInstruction::ClosePane(id, _) => assert!(id != moved && id != hidden),
                PtyInstruction::CloseTab(ids) => {
                    assert!(!ids.contains(&moved) && !ids.contains(&hidden))
                },
                _ => {},
            }
        }
    }
}

#[test]
pub fn screen_can_break_floating_pane_to_a_new_tab() {
    let size = Size { cols: 80, rows: 20 };
    let pane_to_break_free = TiledPaneLayout {
        name: Some("tiled_pane".to_owned()),
        ..Default::default()
    };
    let floating_pane = FloatingPaneLayout {
        name: Some("floating_pane_to_eject".to_owned()),
        ..Default::default()
    };
    let mut floating_panes_layout = vec![floating_pane];
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![pane_to_break_free],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let screen_thread = mock_screen.run(Some(initial_layout), floating_panes_layout.clone());
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let _ = mock_screen.to_screen.send(ScreenInstruction::BreakPane(
        Default::default(),
        crate::ClientId::new(1),
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // we send ApplyLayout, because in prod this is eventually received after the message traverses
    // through the plugin and pty threads (to open extra stuff we need in the layout, eg. the
    // default plugins)
    floating_panes_layout.get_mut(0).unwrap().already_running = true;
    let _ = mock_screen.to_screen.send(ScreenInstruction::ApplyLayout(
        TiledPaneLayout::default(),
        floating_panes_layout,
        vec![], // tiled pane ids
        vec![], // floating pane ids
        vec![], // adopted agents
        Default::default(),
        TabId::new(1),
        true,
        crate::ClientId::new(1),
        None,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(200));
    // move back to make sure the other pane is in the previous tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusLeftOrPreviousTab(
            crate::ClientId::new(1),
            None,
        ));
    std::thread::sleep(std::time::Duration::from_millis(200));
    // move forward to make sure the broken pane is in the previous tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusRightOrNextTab(
            crate::ClientId::new(1),
            None,
        ));
    std::thread::sleep(std::time::Duration::from_millis(200));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(snapshot.to_string());
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn screen_can_break_multiple_stacked_panes_to_a_new_tab() {
    let size = Size { cols: 80, rows: 20 };
    let stacked_parent = TiledPaneLayout {
        children_are_stacked: true,
        children: vec![
            TiledPaneLayout {
                name: Some("pane_to_stay".to_owned()),
                ..Default::default()
            },
            TiledPaneLayout {
                name: Some("pane_to_break_1".to_owned()),
                ..Default::default()
            },
            TiledPaneLayout {
                name: Some("pane_to_break_2".to_owned()),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let initial_layout = TiledPaneLayout {
        children: vec![stacked_parent],
        ..Default::default()
    };

    let mut mock_screen = MockScreen::new(size);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    std::thread::sleep(std::time::Duration::from_millis(100)); // give time for the async render

    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::BreakPanesToNewTab {
            pane_ids: vec![
                PaneId::Terminal(crate::TerminalId::new(1)),
                PaneId::Terminal(crate::TerminalId::new(2)),
            ],
            default_shell: None,
            should_change_focus_to_new_tab: true,
            new_tab_name: None,
            client_id: crate::ClientId::new(1),
            completion_tx: None,
        });
    std::thread::sleep(std::time::Duration::from_millis(100));
    // we send ApplyLayout, because in prod this is eventually received after the message traverses
    // through the plugin and pty threads (to open extra stuff we need in the layout, eg. the
    // default plugins)
    let _ = mock_screen.to_screen.send(ScreenInstruction::ApplyLayout(
        TiledPaneLayout::default(),
        vec![],
        Default::default(),
        vec![],
        vec![],
        Default::default(),
        TabId::new(1),
        true,
        crate::ClientId::new(1),
        None,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // move back to make sure the other pane is in the previous tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusLeftOrPreviousTab(
            crate::ClientId::new(1),
            None,
        ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    // move forward to make sure the broken panes are in the next tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusRightOrNextTab(
            crate::ClientId::new(1),
            None,
        ));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in &snapshots {
        eprintln!("{}", snapshot);
    }
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(snapshot.to_string());
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn screen_can_break_plugin_pane_to_a_new_tab() {
    let size = Size { cols: 80, rows: 20 };
    let pane_to_break_free = TiledPaneLayout {
        name: Some("plugin_pane_to_break_free".to_owned()),
        run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
            _allow_exec_host_cmd: false,
            location: RunPluginLocation::File(PathBuf::from("/path/to/fake/plugin")),
            configuration: Default::default(),
            ..Default::default()
        }))),
        ..Default::default()
    };
    let pane_to_stay = TiledPaneLayout {
        name: Some("pane_to_stay".to_owned()),
        ..Default::default()
    };
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![pane_to_break_free, pane_to_stay],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let initial = wait_for_settled_render(
        &received_server_instructions,
        size,
        "both panes before breaking",
        |frame| frame.contains("plugin_pane_to_break_free") && frame.contains("pane_to_stay"),
    );
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::FocusPaneWithId(
            PaneId::Plugin(crate::PluginId::new(1)),
            false,
            false,
            crate::ClientId::new(1),
            None,
        ));
    let _ = mock_screen.to_screen.send(ScreenInstruction::BreakPane(
        Default::default(),
        crate::ClientId::new(1),
        None,
    ));
    // we send ApplyLayout, because in prod this is eventually received after the message traverses
    // through the plugin and pty threads (to open extra stuff we need in the layout, eg. the
    // default plugins)
    let _ = mock_screen.to_screen.send(ScreenInstruction::ApplyLayout(
        TiledPaneLayout::default(),
        vec![], // floating_panes_layout
        Default::default(),
        vec![], // floating panes ids
        vec![], // adopted agents
        Default::default(),
        TabId::new(1),
        true,
        crate::ClientId::new(1),
        None,
        None,
    ));
    let _ = mock_screen.to_screen.send(ScreenInstruction::GoToTab(
        yinz_types::internal_ids::TabNumber::new(2),
        Some(crate::ClientId::new(1)),
        None,
    ));
    let moved = wait_for_settled_render(
        &received_server_instructions,
        size,
        "plugin alone in its new tab",
        |frame| frame.contains("plugin_pane_to_break_free") && !frame.contains("pane_to_stay"),
    );
    let checkpoint = received_server_instructions.lock().len();
    // move back to make sure the other pane is in the previous tab
    let _ = mock_screen.to_screen.send(ScreenInstruction::GoToTab(
        yinz_types::internal_ids::TabNumber::new(1),
        Some(crate::ClientId::new(1)),
        None,
    ));
    let stayed = wait_for_render_after(
        &received_server_instructions,
        size,
        checkpoint,
        "original pane in its old tab",
        |_, frame| frame.contains("pane_to_stay") && !frame.contains("plugin_pane_to_break_free"),
    );
    let checkpoint = received_server_instructions.lock().len();
    // move forward to make sure the broken pane is in the previous tab
    let _ = mock_screen.to_screen.send(ScreenInstruction::GoToTab(
        yinz_types::internal_ids::TabNumber::new(2),
        Some(crate::ClientId::new(1)),
        None,
    ));

    let returned = wait_for_render_after(
        &received_server_instructions,
        size,
        checkpoint,
        "return to the plugin tab",
        |_, frame| frame.contains("plugin_pane_to_break_free") && !frame.contains("pane_to_stay"),
    );
    mock_screen.teardown(vec![server_thread, screen_thread]);

    assert_snapshot!("screen_can_break_plugin_pane_to_a_new_tab", initial);
    assert_snapshot!("screen_can_break_plugin_pane_to_a_new_tab-3", moved);
    assert_snapshot!("screen_can_break_plugin_pane_to_a_new_tab-2", stayed);
    assert_eq!(returned, moved);
}

#[test]
pub fn screen_can_break_floating_plugin_pane_to_a_new_tab() {
    let size = Size { cols: 80, rows: 20 };
    let pane_to_break_free = TiledPaneLayout {
        name: Some("tiled_pane".to_owned()),
        ..Default::default()
    };
    let floating_pane = FloatingPaneLayout {
        name: Some("floating_plugin_pane_to_eject".to_owned()),
        run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
            _allow_exec_host_cmd: false,
            location: RunPluginLocation::File(PathBuf::from("/path/to/fake/plugin")),
            configuration: Default::default(),
            ..Default::default()
        }))),
        ..Default::default()
    };
    let mut floating_panes_layout = vec![floating_pane];
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![pane_to_break_free],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let screen_thread = mock_screen.run(Some(initial_layout), floating_panes_layout.clone());
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let initial = wait_for_settled_render(
        &received_server_instructions,
        size,
        "both panes before breaking",
        |frame| frame.contains("floating_plugin_pane_to_eject") && frame.contains("tiled_pane"),
    );
    let _ = mock_screen.to_screen.send(ScreenInstruction::BreakPane(
        Default::default(),
        crate::ClientId::new(1),
        None,
    ));
    // we send ApplyLayout, because in prod this is eventually received after the message traverses
    // through the plugin and pty threads (to open extra stuff we need in the layout, eg. the
    // default plugins)
    floating_panes_layout.get_mut(0).unwrap().already_running = true;
    let _ = mock_screen.to_screen.send(ScreenInstruction::ApplyLayout(
        TiledPaneLayout::default(),
        floating_panes_layout,
        vec![], // tiled pane ids
        vec![], // floating pane ids
        vec![], // adopted agents
        Default::default(),
        TabId::new(1),
        true,
        crate::ClientId::new(1),
        None,
        None,
    ));
    let moved = wait_for_settled_render(
        &received_server_instructions,
        size,
        "plugin alone in its new tab",
        |frame| frame.contains("floating_plugin_pane_to_eject") && !frame.contains("tiled_pane"),
    );
    let checkpoint = received_server_instructions.lock().len();
    // move back to make sure the other pane is in the previous tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusLeftOrPreviousTab(
            crate::ClientId::new(1),
            None,
        ));
    let stayed = wait_for_render_after(
        &received_server_instructions,
        size,
        checkpoint,
        "original pane in its old tab",
        |_, frame| frame.contains("tiled_pane") && !frame.contains("floating_plugin_pane_to_eject"),
    );
    let checkpoint = received_server_instructions.lock().len();
    // move forward to make sure the broken pane is in the previous tab
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusRightOrNextTab(
            crate::ClientId::new(1),
            None,
        ));

    let returned = wait_for_render_after(
        &received_server_instructions,
        size,
        checkpoint,
        "return to the plugin tab",
        |_, frame| frame.contains("floating_plugin_pane_to_eject") && !frame.contains("tiled_pane"),
    );
    mock_screen.teardown(vec![server_thread, screen_thread]);

    assert_snapshot!(
        "screen_can_break_floating_plugin_pane_to_a_new_tab",
        initial
    );
    assert_snapshot!(
        "screen_can_break_floating_plugin_pane_to_a_new_tab-3",
        moved
    );
    assert_snapshot!(
        "screen_can_break_floating_plugin_pane_to_a_new_tab-5",
        stayed
    );
    assert_eq!(returned, moved);
}

#[test]
pub fn screen_can_move_pane_to_a_new_tab_right() {
    let size = Size { cols: 80, rows: 20 };
    let pane_to_break_free = TiledPaneLayout {
        name: Some("pane_to_break_free".to_owned()),
        ..Default::default()
    };
    let pane_to_stay = TiledPaneLayout {
        name: Some("pane_to_stay".to_owned()),
        ..Default::default()
    };
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![pane_to_break_free, pane_to_stay],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let _ = mock_screen.to_screen.send(ScreenInstruction::BreakPane(
        Default::default(),
        crate::ClientId::new(1),
        None,
    ));

    std::thread::sleep(std::time::Duration::from_millis(100));
    let _ = mock_screen.to_screen.send(ScreenInstruction::ApplyLayout(
        TiledPaneLayout::default(),
        Default::default(),
        vec![], // tiled pane ids
        vec![], // floating pane ids
        vec![], // adopted agents
        Default::default(),
        TabId::new(1),
        true,
        crate::ClientId::new(1),
        None,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusLeftOrPreviousTab(
            crate::ClientId::new(1),
            None,
        ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::BreakPaneRight(
            crate::ClientId::new(1),
            None,
        ));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(snapshot.to_string());
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn screen_can_move_pane_to_a_new_tab_left() {
    let size = Size { cols: 80, rows: 20 };
    let pane_to_break_free = TiledPaneLayout {
        name: Some("pane_to_break_free".to_owned()),
        ..Default::default()
    };
    let pane_to_stay = TiledPaneLayout {
        name: Some("pane_to_stay".to_owned()),
        ..Default::default()
    };
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![pane_to_break_free, pane_to_stay],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    let _ = mock_screen.to_screen.send(ScreenInstruction::BreakPane(
        Default::default(),
        crate::ClientId::new(1),
        None,
    ));
    let _ = mock_screen.to_screen.send(ScreenInstruction::ApplyLayout(
        TiledPaneLayout::default(),
        Default::default(),
        vec![], // tiled pane ids
        vec![], // floating pane ids
        vec![], // adopted agents
        Default::default(),
        TabId::new(1),
        true,
        crate::ClientId::new(1),
        None,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::MoveFocusLeftOrPreviousTab(
            crate::ClientId::new(1),
            None,
        ));
    std::thread::sleep(std::time::Duration::from_millis(100));
    let _ = mock_screen.to_screen.send(ScreenInstruction::BreakPaneLeft(
        crate::ClientId::new(1),
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(snapshot.to_string());
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn send_cli_stack_panes_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![
            TiledPaneLayout::default(),
            TiledPaneLayout::default(),
            TiledPaneLayout::default(),
            TiledPaneLayout::default(),
            TiledPaneLayout::default(),
        ],
        ..Default::default()
    };
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let stack_panes_action = CliAction::StackPanes {
        pane_ids: vec!["1".to_owned(), "2".to_owned(), "3".to_owned()],
    };
    send_cli_action_to_server(&session_metadata, stack_panes_action, client_id);
    let snapshot = wait_for_settled_render(
        &received_server_instructions,
        size,
        "stacked panes to render",
        |frame| {
            frame
                .lines()
                .nth(1)
                .is_some_and(|line| line.contains("Pane #3"))
                && frame
                    .lines()
                    .nth(2)
                    .is_some_and(|line| line.contains("Pane #4"))
        },
    );
    mock_screen.teardown(vec![server_thread, screen_thread]);
    assert_snapshot!(snapshot.to_string());
}

#[test]
pub fn send_cli_change_floating_pane_coordinates_action() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let initial_tiled_layout = TiledPaneLayout::default();
    let initial_floating_panes = vec![FloatingPaneLayout::default()];
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_tiled_layout), initial_floating_panes);

    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );
    let change_floating_pane_coordinates_action = CliAction::ChangeFloatingPaneCoordinates {
        pane_id: "0".to_owned(),
        x: Some("0".to_owned()),
        y: Some("0".to_owned()),
        width: Some("10".to_owned()),
        height: Some("10".to_owned()),
        pinned: None,
        borderless: Some(false),
    };
    send_cli_action_to_server(
        &session_metadata,
        change_floating_pane_coordinates_action,
        client_id,
    );
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![server_thread, screen_thread]);
    let snapshots = take_snapshots_and_cursor_coordinates_from_render_events(
        received_server_instructions.lock().iter(),
        size,
    );
    let snapshot_count = snapshots.len();
    for (_cursor_coordinates, snapshot) in snapshots {
        assert_snapshot!(snapshot.to_string());
    }
    assert_snapshot!(format!("{}", snapshot_count));
}

#[test]
pub fn go_to_tab_by_id_verifies_screen_state() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(1);
    let mut screen = create_new_screen(size, true, true);

    // Create multiple tabs with known IDs
    new_tab(&mut screen, 1, TabId::new(0)); // ID 0
    new_tab(&mut screen, 2, TabId::new(1)); // ID 1
    new_tab(&mut screen, 3, TabId::new(2)); // ID 2

    // Active tab should be the last one created (ID 2)
    assert_eq!(screen.get_active_tab(client_id).unwrap().id, TabId::new(2));

    // Switch to tab with ID 0
    if let Some(tab_position) = screen.get_tab_position_by_id(TabId::new(0)) {
        screen
            .switch_active_tab(tab_position, None, true, client_id)
            .expect("TEST");
    }

    // Verify active tab is now ID 0
    assert_eq!(
        screen.get_active_tab(client_id).unwrap().id,
        TabId::new(0),
        "Active tab should be tab with ID 0"
    );

    // Switch to tab with ID 1
    if let Some(tab_position) = screen.get_tab_position_by_id(TabId::new(1)) {
        screen
            .switch_active_tab(tab_position, None, true, client_id)
            .expect("TEST");
    }

    // Verify active tab is now ID 1
    assert_eq!(
        screen.get_active_tab(client_id).unwrap().id,
        TabId::new(1),
        "Active tab should be tab with ID 1"
    );
}

#[test]
pub fn send_cli_go_to_tab_by_id_action() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10);
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();

    // Create tabs
    mock_screen.new_tab(TiledPaneLayout::default());
    mock_screen.new_tab(TiledPaneLayout::default());

    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Send CLI action
    let cli_action = CliAction::GoToTabById { id: 1 };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);

    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    // Verify that CLI action caused screen updates (Render instructions sent)
    let render_count = received_server_instructions
        .lock()
        .iter()
        .filter(|instr| matches!(instr, ServerInstruction::Render(_)))
        .count();

    assert!(
        render_count > 0,
        "GoToTabById CLI action should trigger screen renders"
    );
}

#[test]
pub fn rename_tab_by_id_verifies_screen_state() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    // Create tabs with known IDs
    new_tab(&mut screen, 1, TabId::new(0)); // ID 0
    new_tab(&mut screen, 2, TabId::new(1)); // ID 1

    // Verify initial tab names
    assert_eq!(screen.get_tab_by_id(TabId::new(0)).unwrap().name, "Tab #1");
    assert_eq!(screen.get_tab_by_id(TabId::new(1)).unwrap().name, "Tab #2");

    // Rename tab with ID 1
    if let Some(tab) = screen.get_tab_by_id_mut(TabId::new(1)) {
        tab.name = "CustomTabName".to_string();
    }

    // Verify the tab name changed
    assert_eq!(
        screen.get_tab_by_id(TabId::new(1)).unwrap().name,
        "CustomTabName",
        "Tab with ID 1 should be renamed"
    );

    // Verify other tab name unchanged
    assert_eq!(
        screen.get_tab_by_id(TabId::new(0)).unwrap().name,
        "Tab #1",
        "Tab with ID 0 should keep original name"
    );
}

#[test]
pub fn send_cli_rename_tab_by_id_action() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10);
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();

    mock_screen.new_tab(TiledPaneLayout::default());

    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Send CLI action
    let cli_action = CliAction::RenameTabById {
        id: 1,
        name: "TestName".to_string(),
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);

    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    // Verify that CLI action was processed (no panics means routing worked)
    // The action should complete successfully
    // no panic above means: RenameTabById CLI action completed without errors
}

#[test]
pub fn close_tab_by_id_verifies_screen_state() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let mut screen = create_new_screen(size, true, true);

    // Create multiple tabs with known IDs
    new_tab(&mut screen, 1, TabId::new(0)); // ID 0
    new_tab(&mut screen, 2, TabId::new(1)); // ID 1
    new_tab(&mut screen, 3, TabId::new(2)); // ID 2

    assert_eq!(screen.tabs.len(), 3, "Should have 3 tabs initially");

    // Verify all tabs exist
    assert!(
        screen.get_tab_by_id(TabId::new(0)).is_some(),
        "Tab with ID 0 should exist"
    );
    assert!(
        screen.get_tab_by_id(TabId::new(1)).is_some(),
        "Tab with ID 1 should exist"
    );
    assert!(
        screen.get_tab_by_id(TabId::new(2)).is_some(),
        "Tab with ID 2 should exist"
    );

    // Close tab with ID 1
    screen.close_tab_by_id(TabId::new(1)).expect("TEST");

    assert_eq!(screen.tabs.len(), 2, "Should have 2 tabs after closing one");

    // Verify tab with ID 1 no longer exists
    assert!(
        screen.get_tab_by_id(TabId::new(1)).is_none(),
        "Tab with ID 1 should not exist"
    );

    // Verify other tabs still exist
    assert!(
        screen.get_tab_by_id(TabId::new(0)).is_some(),
        "Tab with ID 0 should still exist"
    );
    assert!(
        screen.get_tab_by_id(TabId::new(2)).is_some(),
        "Tab with ID 2 should still exist"
    );
}

#[test]
pub fn send_cli_close_tab_by_id_action() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10);
    let mut mock_screen = MockScreen::new(size);
    let session_metadata = mock_screen.clone_session_metadata();

    mock_screen.new_tab(TiledPaneLayout::default());
    mock_screen.new_tab(TiledPaneLayout::default());

    let screen_thread = mock_screen.run(None, vec![]);
    let received_server_instructions = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread = log_actions_in_thread!(
        received_server_instructions,
        ServerInstruction::KillSession,
        server_receiver
    );

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Send CLI action
    let cli_action = CliAction::CloseTabById { id: 1 };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);

    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![server_thread, screen_thread]);

    // Verify that CLI action was processed (no panics means routing worked)
    // The action should complete successfully
    // no panic above means: CloseTabById CLI action completed without errors
}

#[test]
pub fn send_cli_new_pane_in_place_with_close_replaced_pane() {
    // Verify that `--close-replaced-pane` propagates from CLI through to the
    // PtyInstruction::SpawnInPlaceTerminal instruction as `true`.
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Watched::new(vec![]);
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_action = CliAction::NewPane {
        remote_host: None,

        direction: None,
        command: vec!["bash".into()],
        plugin: None,
        cwd: None,
        floating: false,
        in_place: true,
        close_replaced_pane: true,
        pane_id: None,
        name: None,
        close_on_exit: false,
        start_suspended: false,
        configuration: None,
        configuration_json: None,
        skip_plugin_cache: false,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        overlay: None,
        modal: None,
        stacked: false,
        blocking: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        block_until_exit: false,
        unblock_condition: None,
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);

    let spawn_in_place_instruction = received_pty_instructions
        .lock()
        .iter()
        .find(|instruction| matches!(instruction, PtyInstruction::SpawnInPlaceTerminal(..)))
        .cloned();

    assert_snapshot!(format!("{:#?}", spawn_in_place_instruction));
}

#[test]
pub fn send_cli_edit_in_place_with_close_replaced_pane() {
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Watched::new(vec![]);
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_action = CliAction::Edit {
        file: PathBuf::from("/some/file.txt"),
        direction: None,
        line_number: None,
        floating: false,
        in_place: true,
        close_replaced_pane: true,
        cwd: None,
        x: None,
        y: None,
        width: None,
        height: None,
        pinned: None,
        overlay: None,
        modal: None,
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);

    let spawn_in_place_instruction = received_pty_instructions
        .lock()
        .iter()
        .find(|instruction| matches!(instruction, PtyInstruction::SpawnInPlaceTerminal(..)))
        .cloned();

    assert_snapshot!(format!("{:#?}", spawn_in_place_instruction));
}

#[test]
pub fn send_cli_launch_or_focus_plugin_in_place_with_close_replaced_pane() {
    // Verify that `--close-replaced-pane` propagates from the `launch-or-focus-plugin --in-place`
    // CLI action through to the PtyInstruction::FillPluginCwd instruction as `true`.
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let client_id = ClientId::new(10); // fake client id should not appear in the screen's state
    let mut mock_screen = MockScreen::new(size);
    let pty_receiver = mock_screen.pty_receiver.take().unwrap();
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);
    let received_pty_instructions = Watched::new(vec![]);
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::Exit,
        pty_receiver
    );
    let cli_action = CliAction::LaunchOrFocusPlugin {
        floating: false,
        in_place: true,
        close_replaced_pane: true,
        move_to_focused_tab: false,
        url: "file:/path/to/fake/plugin".to_owned(),
        configuration: Default::default(),
        configuration_json: None,
        skip_plugin_cache: false,
        tab_id: None,
    };
    send_cli_action_to_server(&session_metadata, cli_action, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    mock_screen.teardown(vec![pty_thread, screen_thread]);

    let fill_plugin_cwd_instruction = received_pty_instructions
        .lock()
        .iter()
        .find(|instruction| matches!(instruction, PtyInstruction::FillPluginCwd(..)))
        .cloned();

    assert_snapshot!(format!("{:#?}", fill_plugin_cwd_instruction));
}
