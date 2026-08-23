use super::*;
use zellij_test_support::Watched;

#[test]
pub fn background_plugin_receives_broadcasts_regardless_of_active_tab() {
    // Tab 0: plugin pane 2 (from new_tab_with_plugins, queued before run)
    // Tab 1: plugin pane 3 (from new_tab_with_plugins, queued before run)
    // Tab 2: terminal panes only (from run, starts screen thread)
    // After run, client is on tab 2. Switch to tab 0 (plugin 2).
    // Background plugin 99 should also receive updates.
    let size = Size { cols: 80, rows: 10 };
    let client_id = 10;

    let mut mock_screen = MockScreen::new(size);
    mock_screen.new_tab_with_plugins(vec![2]);
    mock_screen.new_tab_with_plugins(vec![3]);
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
            99,
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
    let goto_tab = CliAction::GoToTab { index: 1 };
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
                            plugin_ids_that_received_tab_update.push(*id);
                        }
                    },
                    Event::ModeUpdate(..) => {
                        if let Some(id) = pid {
                            plugin_ids_that_received_mode_update.push(*id);
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
    let client_id = 10;

    let mut mock_screen = MockScreen::new(size);
    mock_screen.new_tab_with_plugins(vec![2]);
    mock_screen.new_tab_with_plugins(vec![3]);
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
    let goto_tab = CliAction::GoToTab { index: 1 };
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
                            plugin_ids_that_received_updates.push(*id);
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
    let client_id = 10;

    let mut mock_screen = MockScreen::new(size);
    mock_screen.new_tab_with_plugins(vec![2]);
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
    let goto_tab = CliAction::GoToTab { index: 1 };
    send_cli_action_to_server(&session_metadata, goto_tab, client_id);
    std::thread::sleep(std::time::Duration::from_millis(100));

    mock_screen.teardown(vec![plugin_thread, screen_thread]);

    let instructions = received_plugin_instructions.lock();
    let instructions_after_switch = &instructions[instructions_before_switch..];
    let tab_update_for_plugin_2 = instructions_after_switch.iter().find_map(|instruction| {
        if let PluginInstruction::Update(updates) = instruction {
            for (pid, _cid, event) in updates {
                if let (Some(2), Event::TabUpdate(tab_infos)) = (pid, event) {
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
        active_tab.position, 0,
        "The first tab (position 0) should be active, got position {}",
        active_tab.position
    );
}
