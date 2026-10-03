#[cfg(test)]
const PLUGIN_2: yinz_types::internal_ids::PluginId = yinz_types::internal_ids::PluginId::new(2);

use super::*;
use yinz_test_support::Watched;
use yinz_types::internal_ids::TabNumber;
use yinz_types::internal_ids::TabPosition;

fn assert_no_cursor_reports(instructions: impl Iterator<Item = PluginInstruction>) {
    for instruction in instructions {
        if let PluginInstruction::Update(updates) = instruction {
            for (_, _, event) in updates {
                assert!(
                    !matches!(
                        event,
                        Event::PaneUpdate(..) | Event::TabUpdate(..) | Event::SessionUpdate(..)
                    ),
                    "cursor broadcast session state: {event:?}"
                );
            }
        }
    }
}

#[test]
fn plugin_cursor_show_move_and_hide_render_without_session_reports() {
    let size = Size { cols: 80, rows: 10 };
    let mut mock = MockScreen::new(size);
    let client_id = mock.main_client_id;
    let plugin_rx = mock.plugin_receiver.take().unwrap();
    let server_rx = mock.server_receiver.take().unwrap();
    let screen_thread = mock.run(None, vec![]);
    let to_screen = mock.to_screen.clone();
    // The reply is a FIFO barrier for every preceding screen handler and its reports.
    let barrier = || {
        let (tx, rx) = crossbeam::channel::bounded(1);
        to_screen
            .send(ScreenInstruction::GetPaneInfo {
                pane_id: PaneId::Plugin(PLUGIN_2),
                response_channel: tx,
            })
            .unwrap();
        rx.recv_timeout(yinz_test_support::default_timeout())
            .unwrap()
    };
    assert!(barrier().is_none());
    plugin_rx.try_iter().for_each(drop);

    // A cursor sent during load stays buffered until the screen places the plugin.
    mock.to_screen
        .send(ScreenInstruction::ShowPluginCursor(
            PLUGIN_2,
            client_id,
            Some((2, 1)),
        ))
        .unwrap();
    assert!(barrier().is_none());
    assert_no_cursor_reports(plugin_rx.try_iter().map(|(instruction, _)| instruction));
    mock.new_tab_with_plugins(vec![PLUGIN_2]);
    mock.to_screen
        .send(ScreenInstruction::PluginBytes(vec![PluginRenderAsset {
            plugin_id: PLUGIN_2,
            client_id,
            bytes: b"workflow".to_vec(),
        }]))
        .unwrap();

    let received = Watched::new(vec![]);
    let server_thread = log_actions_in_thread!(received, ServerInstruction::KillSession, server_rx);
    wait_for_render_after(
        &received,
        size,
        0,
        "buffered plugin cursor after placement",
        |cursor, frame| cursor == Some((3, 2)) && frame.contains("workflow"),
    );

    assert!(barrier().is_some());
    plugin_rx.try_iter().for_each(drop);
    for position in [None, Some((4, 2)), Some((7, 3)), None] {
        let checkpoint = received.lock().len();
        mock.to_screen
            .send(ScreenInstruction::ShowPluginCursor(
                PLUGIN_2, client_id, position,
            ))
            .unwrap();
        wait_for_render_after(
            &received,
            size,
            checkpoint,
            "plugin cursor output",
            |cursor, frame| {
                cursor == position.map(|(x, y)| (x + 1, y + 1)) && frame.contains("workflow")
            },
        );
        barrier();
        assert_no_cursor_reports(plugin_rx.try_iter().map(|(instruction, _)| instruction));
    }
    mock.teardown(vec![screen_thread, server_thread]);
}

#[test]
fn early_plugin_output_survives_layout_and_replayed_cursor_renders_without_session_reports() {
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(1);
    let mut screen = create_new_screen(size, true, true);
    let (plugin_tx, plugin_rx) = channels::unbounded();
    screen.bus.senders.to_plugin = Some(SenderWithContext::new(plugin_tx));
    let run = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "file:/path/to/fake/plugin",
        &None,
        None,
        None,
    )
    .unwrap();
    screen
        .new_tab(TabId::new(0), (vec![], vec![]), None, Some(client_id))
        .unwrap();
    // Startup sizes the client before loading plugins, but attaches it only after layout.
    assert!(screen.client_sizes.contains_key(&client_id));
    assert!(!screen.connected_clients.borrow().contains(&client_id));
    let pending_frames = screen
        .handle_plugin_render_assets(vec![PluginRenderAsset::new(
            PLUGIN_2,
            client_id,
            b"workflow".to_vec(),
        )])
        .unwrap();
    assert_eq!(pending_frames.len(), 1);
    assert!(!screen.show_plugin_cursor(PLUGIN_2, client_id, Some((1, 1))));
    screen
        .apply_layout(
            TiledPaneLayout {
                children: vec![TiledPaneLayout {
                    run: Some(Run::Plugin(run.clone())),
                    ..Default::default()
                }],
                ..Default::default()
            },
            vec![],
            vec![],
            vec![],
            HashMap::from([(run, vec![PLUGIN_2])]),
            TabId::new(0),
            true,
            client_id,
            None,
        )
        .unwrap();
    super::super::replay_pending_plugin_events(
        &mut screen,
        vec![
            ScreenInstruction::PluginBytes(pending_frames),
            ScreenInstruction::ShowPluginCursor(PLUGIN_2, client_id, Some((1, 1))),
        ],
    )
    .unwrap();
    let server_rx = screen_wired_to_server(&mut screen);
    let mut grid = grid_for_render_snapshot(size);
    // Settle the initial frame and its derived tab title before measuring cursor-only work.
    let (cursor, frame) =
        render_frame_and_cursor_into(&mut screen, &server_rx, client_id, &mut grid);
    assert!(frame.contains("workflow"), "{frame}");
    assert_eq!(cursor, Some((1, 1)));
    plugin_rx.try_iter().for_each(drop);
    let (jobs_tx, jobs_rx) = channels::unbounded();
    screen.bus.senders.to_background_jobs = Some(SenderWithContext::new(jobs_tx));
    for position in [Some((2, 1)), Some((5, 3)), None] {
        super::super::replay_pending_plugin_events(
            &mut screen,
            vec![ScreenInstruction::ShowPluginCursor(
                PLUGIN_2, client_id, position,
            )],
        )
        .unwrap();
        assert!(jobs_rx
            .try_iter()
            .any(|(job, _)| job == BackgroundJob::RenderToClients));
        let (cursor, frame) =
            render_frame_and_cursor_into(&mut screen, &server_rx, client_id, &mut grid);
        assert_no_cursor_reports(plugin_rx.try_iter().map(|(instruction, _)| instruction));
        assert!(frame.contains("workflow"), "{frame}");
        assert_eq!(cursor, position);
    }
}

#[test]
fn a_background_plugin_subscribing_after_layout_receives_current_panes_and_tabs() {
    let mut mock = MockScreen::new(Size { cols: 80, rows: 10 });
    let plugin = crate::PluginId::new(99);
    let client = mock.main_client_id;
    let receiver = mock.plugin_receiver.take().unwrap();
    let screen_thread = mock.run(None, vec![]);
    // The reply follows all layout reports, so the subscription arrives in a quiet session.
    let (tx, rx) = crossbeam::channel::bounded(1);
    mock.to_screen
        .send(ScreenInstruction::GetPaneInfo {
            pane_id: PaneId::Terminal(TerminalId::new(0)),
            response_channel: tx,
        })
        .unwrap();
    assert!(rx
        .recv_timeout(yinz_test_support::default_timeout())
        .unwrap()
        .is_some());
    receiver.try_iter().for_each(drop);
    let received = Watched::new(vec![]);
    let plugin_thread = log_actions_in_thread!(received, PluginInstruction::Exit, receiver);
    mock.to_screen
        .send(ScreenInstruction::UpdateBackgroundPluginSubscriptions(
            plugin,
            client,
            HashSet::from([EventType::PaneUpdate, EventType::TabUpdate]),
        ))
        .unwrap();
    received.wait_until(
        "late background subscriber receives current panes and tabs",
        yinz_test_support::default_timeout(),
        |instructions| {
            let mut panes = false;
            let mut tabs = false;
            for instruction in instructions {
                if let PluginInstruction::Update(updates) = instruction {
                    for (target, owner, event) in updates {
                        if *target != Some(plugin) || *owner != Some(client) {
                            continue;
                        }
                        match event {
                            Event::PaneUpdate(manifest) => {
                                panes |=
                                    manifest.panes.values().flatten().any(|pane| {
                                        pane.id == PaneId::Terminal(TerminalId::new(0))
                                    });
                            },
                            Event::TabUpdate(current) => tabs |= !current.is_empty(),
                            _ => {},
                        }
                    }
                }
            }
            (panes && tabs).then_some(())
        },
    );
    mock.teardown(vec![screen_thread, plugin_thread]);
}

#[test]
pub fn background_plugin_receives_broadcasts_regardless_of_active_tab() {
    // Tab 0: plugin pane 2 (from new_tab_with_plugins, queued before run)
    // Tab 1: plugin pane 3 (from new_tab_with_plugins, queued before run)
    // Tab 2: terminal panes only (from run, starts screen thread)
    // After run, client is on tab 2. Switch to tab 0 (plugin 2).
    // Background plugin 99 should also receive updates.
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10);

    let mut mock_screen = MockScreen::new(size);
    mock_screen.new_tab_with_plugins(vec![crate::PluginId::new(2)]);
    mock_screen.new_tab_with_plugins(vec![crate::PluginId::new(3)]);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);

    // Register background plugin 99 with the main_client_id (1), not the CLI client_id (10)
    let main_client_id = mock_screen.main_client_id;
    let mut bg_subs = HashSet::new();
    bg_subs.insert(EventType::TabUpdate);
    bg_subs.insert(EventType::ModeUpdate);
    let _ = mock_screen
        .to_screen
        .send(ScreenInstruction::UpdateBackgroundPluginSubscriptions(
            crate::PluginId::new(99),
            main_client_id,
            bg_subs,
        ));
    std::thread::sleep(std::time::Duration::from_millis(100));

    let received_plugin_instructions = Watched::new(vec![]);
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );

    // Drain initial setup instructions before the GoToTab action
    std::thread::sleep(std::time::Duration::from_millis(100));
    let instructions_before_switch = received_plugin_instructions.lock().len();

    // Switch to tab 0 (1-based index 1 = position 0 = tab with plugin 2)
    let goto_tab = CliAction::GoToTab {
        index: TabNumber::new(1),
    };
    send_cli_action_to_server(&session_metadata, goto_tab, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![plugin_thread, screen_thread]);

    let instructions = received_plugin_instructions.lock();
    // Only examine instructions sent after the switch
    let instructions_after_switch = &instructions[instructions_before_switch..];
    let mut plugin_ids_that_received_tab_update: Vec<u32> = vec![];
    let mut plugin_ids_that_received_mode_update: Vec<u32> = vec![];
    for instruction in instructions_after_switch.iter() {
        if let PluginInstruction::Update(updates) = instruction {
            for (pid, _cid, event) in updates {
                match event {
                    Event::TabUpdate(..) => {
                        if let Some(id) = pid {
                            plugin_ids_that_received_tab_update.push(id.get());
                        }
                    },
                    Event::ModeUpdate(..) => {
                        if let Some(id) = pid {
                            plugin_ids_that_received_mode_update.push(id.get());
                        }
                    },
                    _ => {},
                }
            }
        }
    }

    // Plugin 2 (active tab) and plugin 99 (background) should receive updates
    assert!(
        plugin_ids_that_received_tab_update.contains(&2),
        "Active tab plugin 2 should receive TabUpdate, got: {:?}",
        plugin_ids_that_received_tab_update
    );
    assert!(
        plugin_ids_that_received_tab_update.contains(&99),
        "Background plugin 99 should receive TabUpdate, got: {:?}",
        plugin_ids_that_received_tab_update
    );
    // Plugin 3 (inactive tab) should NOT receive updates
    assert!(
        !plugin_ids_that_received_tab_update.contains(&3),
        "Inactive tab plugin 3 should NOT receive TabUpdate, got: {:?}",
        plugin_ids_that_received_tab_update
    );

    // ModeUpdate is sent via update_input_modes() to tab plugins only (not background plugins).
    // Background plugins receive ModeUpdate only via explicit broadcast_mode_update calls.
    // So during tab switch, only the active tab's plugins get ModeUpdate.
    assert!(
        plugin_ids_that_received_mode_update.contains(&2),
        "Active tab plugin 2 should receive ModeUpdate, got: {:?}",
        plugin_ids_that_received_mode_update
    );
    assert!(
        !plugin_ids_that_received_mode_update.contains(&3),
        "Inactive tab plugin 3 should NOT receive ModeUpdate, got: {:?}",
        plugin_ids_that_received_mode_update
    );
}

#[test]
pub fn tab_switch_only_updates_active_tab_plugins() {
    // Tab 0: plugin pane 2 (from new_tab_with_plugins)
    // Tab 1: plugin pane 3 (from new_tab_with_plugins)
    // Tab 2: terminal panes only (from run)
    // After run, client is on tab 2. Switch to tab 0 (plugin 2).
    // Only plugin 2 should receive updates; plugin 3 should not.
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10);

    let mut mock_screen = MockScreen::new(size);
    mock_screen.new_tab_with_plugins(vec![crate::PluginId::new(2)]);
    mock_screen.new_tab_with_plugins(vec![crate::PluginId::new(3)]);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);

    let received_plugin_instructions = Watched::new(vec![]);
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );

    // Drain initial setup instructions before the GoToTab action
    std::thread::sleep(std::time::Duration::from_millis(100));
    let instructions_before_switch = received_plugin_instructions.lock().len();

    // Switch to tab 0 (1-based index 1 = position 0 = tab with plugin 2)
    let goto_tab = CliAction::GoToTab {
        index: TabNumber::new(1),
    };
    send_cli_action_to_server(&session_metadata, goto_tab, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![plugin_thread, screen_thread]);

    let instructions = received_plugin_instructions.lock();
    let instructions_after_switch = &instructions[instructions_before_switch..];
    let mut plugin_ids_that_received_updates: Vec<u32> = vec![];
    for instruction in instructions_after_switch.iter() {
        if let PluginInstruction::Update(updates) = instruction {
            for (pid, _cid, event) in updates {
                match event {
                    Event::TabUpdate(..) | Event::ModeUpdate(..) => {
                        if let Some(id) = pid {
                            plugin_ids_that_received_updates.push(id.get());
                        }
                    },
                    _ => {},
                }
            }
        }
    }

    // Only plugin 2 (active tab after switch) should receive TabUpdate/ModeUpdate
    assert!(
        plugin_ids_that_received_updates.contains(&2),
        "Active tab plugin 2 should receive updates, got: {:?}",
        plugin_ids_that_received_updates
    );
    assert!(
        !plugin_ids_that_received_updates.contains(&3),
        "Inactive tab plugin 3 should NOT receive updates, got: {:?}",
        plugin_ids_that_received_updates
    );
}

#[test]
pub fn inactive_tab_plugins_get_fresh_state_on_activation() {
    // Tab 0: plugin pane 2 (from new_tab_with_plugins)
    // Tab 1: terminal panes only (from run, client starts here)
    // Switch to tab 0 → plugin 2 becomes active and receives TabUpdate with both tabs.
    let size = Size { cols: 80, rows: 10 };
    let client_id = ClientId::new(10);

    let mut mock_screen = MockScreen::new(size);
    mock_screen.new_tab_with_plugins(vec![crate::PluginId::new(2)]);
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(None, vec![]);

    let received_plugin_instructions = Watched::new(vec![]);
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );

    // Drain initial setup instructions before the GoToTab action
    std::thread::sleep(std::time::Duration::from_millis(100));
    let instructions_before_switch = received_plugin_instructions.lock().len();

    // Switch to tab 0 (1-based index 1 = position 0 = tab with plugin 2)
    let goto_tab = CliAction::GoToTab {
        index: TabNumber::new(1),
    };
    send_cli_action_to_server(&session_metadata, goto_tab, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![plugin_thread, screen_thread]);

    let instructions = received_plugin_instructions.lock();
    let instructions_after_switch = &instructions[instructions_before_switch..];
    let tab_update_for_plugin_2 = instructions_after_switch.iter().find_map(|instruction| {
        if let PluginInstruction::Update(updates) = instruction {
            for (pid, _cid, event) in updates {
                if let (Some(PLUGIN_2), Event::TabUpdate(tab_infos)) = (pid, event) {
                    return Some(tab_infos.clone());
                }
            }
        }
        None
    });

    assert!(
        tab_update_for_plugin_2.is_some(),
        "Plugin 2 should receive a TabUpdate after becoming active"
    );
    let tab_infos = tab_update_for_plugin_2.unwrap();
    assert!(
        tab_infos.len() >= 2,
        "TabUpdate should contain info for both tabs, got {} tabs",
        tab_infos.len()
    );
    let active_tab = tab_infos.iter().find(|t| t.active);
    assert!(
        active_tab.is_some(),
        "TabUpdate should have an active tab marked"
    );
    // Tab at position 0 should be active after switching to GoToTab index 1 (1-based)
    let active_tab = active_tab.unwrap();
    assert_eq!(
        active_tab.position,
        TabPosition::new(0),
        "The first tab (position 0) should be active, got position {}",
        active_tab.position
    );
}

#[test]
pub fn closing_tab_updates_input_modes_of_destination_tab_plugins() {
    // Tab 0: plugin pane 2 (from new_tab_with_plugins)
    // Tab 1: two terminal panes (from run, client starts here)
    // Closing tab 1 sends the client back to tab 0, whose plugin last heard about the
    // client's mode when it left. It has to be told again on arrival.
    let size = Size { cols: 80, rows: 10 };

    let mut mock_screen = MockScreen::new(size);
    let client_id = mock_screen.main_client_id;
    mock_screen.new_tab_with_plugins(vec![crate::PluginId::new(2)]);
    let initial_layout = TiledPaneLayout {
        children_split_direction: SplitDirection::Vertical,
        children: vec![TiledPaneLayout::default(), TiledPaneLayout::default()],
        ..Default::default()
    };
    let session_metadata = mock_screen.clone_session_metadata();
    let screen_thread = mock_screen.run(Some(initial_layout), vec![]);

    let received_plugin_instructions = Watched::new(vec![]);
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let plugin_thread = log_actions_in_thread!(
        received_plugin_instructions,
        PluginInstruction::Exit,
        plugin_receiver
    );

    let close_tab = CliAction::CloseTab { tab_id: None };
    send_cli_action_to_server(&session_metadata, close_tab, client_id);

    // The screen thread drains its instructions in order and this log is fed by it alone, so
    // the close begins at the first `PaneClosed` (nothing else closes a pane here) and is
    // over once plugin 2 has been told the tab list shrank to the one surviving tab.
    let close_started_at = |instructions: &[PluginInstruction]| -> Option<usize> {
        instructions.iter().position(|instruction| {
            matches!(
                instruction,
                PluginInstruction::Update(updates)
                    if updates.iter().any(|(_, _, event)| matches!(event, Event::PaneClosed(..)))
            )
        })
    };
    let mode_updates_after = |instructions: &[PluginInstruction]| -> Vec<(yinz_types::internal_ids::PluginId, ClientId)> {
        let Some(from) = close_started_at(instructions) else {
            return vec![];
        };
        instructions[from..]
            .iter()
            .flat_map(|instruction| match instruction {
                PluginInstruction::Update(updates) => updates.as_slice(),
                _ => &[],
            })
            .filter_map(|(pid, cid, event)| match (pid, cid, event) {
                (Some(pid), Some(cid), Event::ModeUpdate(..)) => Some((*pid, *cid)),
                _ => None,
            })
            .collect()
    };
    let mode_updates_received = received_plugin_instructions.wait_until_described(
        "plugin 2 to be told only one tab is left after the close",
        yinz_test_support::default_timeout(),
        |instructions| {
            let from = close_started_at(instructions)?;
            let close_finished = instructions[from..].iter().any(|instruction| {
                matches!(
                    instruction,
                    PluginInstruction::Update(updates)
                        if updates.iter().any(|(pid, _, event)| matches!(
                            (pid, event),
                            (Some(PLUGIN_2), Event::TabUpdate(tabs)) if tabs.len() == 1
                        ))
                )
            });
            close_finished.then(|| mode_updates_after(instructions))
        },
        |instructions| {
            format!(
                "{} plugin instruction(s), close started at {:?}, mode updates since: {:?}",
                instructions.len(),
                close_started_at(instructions),
                mode_updates_after(instructions)
            )
        },
    );

    mock_screen.teardown(vec![plugin_thread, screen_thread]);

    assert!(
        mode_updates_received.contains(&(crate::PluginId::new(2), client_id)),
        "Plugin 2 in the tab focus returned to should receive a ModeUpdate for client {}, got: {:?}",
        client_id,
        mode_updates_received
    );
}
