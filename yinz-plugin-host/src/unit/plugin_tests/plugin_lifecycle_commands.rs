use super::*;
use yinz_test_support::Watched;
use yinz_types::internal_ids::TabId;

#[test]
#[serial(plugin_tests)]
pub fn reload_plugin_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let plugin_should_float = Some(false);
    let plugin_title = Some("test_plugin".to_owned());
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: Default::default(),
        ..Default::default()
    });
    let tab_index = 1;
    let client_id = ClientId::new(1);
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = grant_permissions_and_log_actions_in_thread_naked_variant!(
        received_screen_instructions,
        ScreenInstruction::RequestStateUpdateForPlugins, // happens on successful plugin (re)load
        screen_receiver,
        2,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        client_id
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        plugin_should_float,
        false,
        false, // close_replaced_pane
        plugin_title,
        run_plugin,
        Some(TabId::new(tab_index)),
        None,
        client_id,
        size,
        None,
        None,
        false,
        None,
        None,
        None,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(500));
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('w')).with_alt_modifier()), // this triggers the enent in the fixture plugin
    )]));
    screen_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let request_state_update_requests = received_screen_instructions
        .lock()
        .iter()
        .filter(|i| matches!(i, ScreenInstruction::RequestStateUpdateForPlugins))
        .count();
    assert_eq!(request_state_update_requests, 2);
}

#[test]
#[serial(plugin_tests)]
pub fn load_new_plugin_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let plugin_should_float = Some(false);
    let plugin_title = Some("test_plugin".to_owned());
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: Default::default(),
        ..Default::default()
    });
    let tab_index = 1;
    let client_id = ClientId::new(1);
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        client_id,
        PermissionStatus::Granted,
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        plugin_should_float,
        false,
        false, // close_replaced_pane
        plugin_title,
        run_plugin,
        Some(TabId::new(tab_index)),
        None,
        client_id,
        size,
        None,
        None,
        false,
        None,
        None,
        None,
        None,
    ));
    wait_for_permissions(&received_screen_instructions);
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('x')).with_alt_modifier()), // this triggers the enent in the fixture plugin
    )]));
    wait_for_render_containing(&received_screen_instructions, "Key");
    teardown();
    screen_thread.join().unwrap();
    let request_state_update_requests = received_screen_instructions
        .lock()
        .iter()
        .filter(|i| matches!(i, ScreenInstruction::RequestStateUpdateForPlugins))
        .count();
    assert_eq!(
        request_state_update_requests, 1,
        "a background start reuses the loaded identity"
    );
}

#[test]
#[serial(plugin_tests)]
pub fn rebind_keys_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, server_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_server_receiver(Some(plugin_host_folder), None);
    let plugin_should_float = Some(false);
    let plugin_title = Some("test_plugin".to_owned());
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: Default::default(),
        ..Default::default()
    });
    let tab_index = 1;
    let client_id = ClientId::new(1);
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        client_id,
        PermissionStatus::Granted,
    );
    let received_server_instruction = Watched::new(vec![]);
    let server_thread = log_actions_in_thread_struct!(
        received_server_instruction,
        ServerInstruction::RebindKeys,
        server_receiver,
        1
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        plugin_should_float,
        false,
        false, // close_replaced_pane
        plugin_title,
        run_plugin,
        Some(TabId::new(tab_index)),
        None,
        client_id,
        size,
        None,
        None,
        false,
        None,
        None,
        None,
        None,
    ));
    wait_for_render_containing(&received_screen_instructions, "Rows: 20, Cols: 121");

    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('y')).with_alt_modifier()), // this triggers the enent in the fixture plugin
    )]));
    let rebind_event = received_server_instruction.wait_until(
        "the fixture's key rebinding reaches the server",
        default_timeout(),
        |instructions| {
            instructions
                .iter()
                .find_map(|instruction| match instruction {
                    ServerInstruction::RebindKeys { client_id: id, .. } if *id == client_id => {
                        Some(Some(instruction.clone()))
                    },
                    _ => None,
                })
        },
    );
    teardown();
    screen_thread.join().unwrap();
    server_thread.join().unwrap();
    assert_snapshot!(format!("{:#?}", rebind_event));
}

#[test]
#[serial(plugin_tests)]
pub fn list_clients_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let plugin_should_float = Some(false);
    let plugin_title = Some("test_plugin".to_owned());
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: Default::default(),
        ..Default::default()
    });
    let tab_index = 1;
    let client_id = ClientId::new(1);
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::ListClientsToPlugin,
        screen_receiver,
        1,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        client_id
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        plugin_should_float,
        false,
        false, // close_replaced_pane
        plugin_title,
        run_plugin,
        Some(TabId::new(tab_index)),
        None,
        client_id,
        size,
        None,
        None,
        false,
        None,
        None,
        None,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(500));
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('z')).with_alt_modifier()), // this triggers the enent in the fixture plugin
    )]));
    screen_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let list_clients_instruction = received_screen_instructions
        .lock()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::ListClientsToPlugin(..) = i {
                Some(i.clone())
            } else {
                None
            }
        })
        .unwrap();
    assert_snapshot!(format!("{:#?}", list_clients_instruction));
}

#[test]
#[serial(plugin_tests)]
pub fn before_close_plugin_event() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let plugin_should_float = Some(false);
    let plugin_title = Some("test_plugin".to_owned());
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: Default::default(),
        ..Default::default()
    });
    let tab_index = 1;
    let client_id = ClientId::new(1);
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::HighlightAndUnhighlightPanes,
        screen_receiver,
        1,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        client_id
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        plugin_should_float,
        false,
        false, // close_replaced_pane
        plugin_title,
        run_plugin,
        Some(TabId::new(tab_index)),
        None,
        client_id,
        size,
        None,
        None,
        false,
        None,
        None,
        None,
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(5000));
    // here we send an unload to plugin id 0 (the first plugin id, presumably this plugin)
    // so that its BeforeClose Event will be triggered and it will send a
    // HighlightAndUnhighlightPanes
    // instruction which we can assert below
    let _ = plugin_thread_sender.send(PluginInstruction::Unload(crate::PluginId::new(0)));
    screen_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let sent_instruction = received_screen_instructions
        .lock()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::HighlightAndUnhighlightPanes(..) = i {
                Some(i.clone())
            } else {
                None
            }
        })
        .unwrap();
    assert_snapshot!(format!("{:#?}", sent_instruction));
}

#[test]
#[serial(plugin_tests)]
pub fn show_cursor_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let plugin_should_float = Some(false);
    let plugin_title = Some("test_plugin".to_owned());
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: Default::default(),
        ..Default::default()
    });
    let tab_index = 1;
    let client_id = ClientId::new(1);
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::ShowPluginCursor,
        screen_receiver,
        1,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        client_id
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        plugin_should_float,
        false,
        false, // close_replaced_pane
        plugin_title,
        run_plugin,
        Some(TabId::new(tab_index)),
        None,
        client_id,
        size,
        None,
        None,
        false,
        None,
        None,
        None,
        None,
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('a')).with_super_modifier()),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let show_cursor_instruction = received_screen_instructions.lock().iter().find_map(|i| {
        if let ScreenInstruction::ShowPluginCursor(plugin_id, client_id, cursor_position) = i {
            Some((*plugin_id, *client_id, *cursor_position))
        } else {
            None
        }
    });

    assert_snapshot!(format!("{:#?}", show_cursor_instruction));
}

#[test]
#[serial(plugin_tests)]
pub fn hide_cursor_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let plugin_should_float = Some(false);
    let plugin_title = Some("test_plugin".to_owned());
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: Default::default(),
        ..Default::default()
    });
    let tab_index = 1;
    let client_id = ClientId::new(1);
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::ShowPluginCursor,
        screen_receiver,
        1,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        client_id
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));

    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        plugin_should_float,
        false,
        false, // close_replaced_pane
        plugin_title,
        run_plugin,
        Some(TabId::new(tab_index)),
        None,
        client_id,
        size,
        None,
        None,
        false,
        None,
        None,
        None,
        None,
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('b')).with_super_modifier()),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let hide_cursor_instruction = received_screen_instructions.lock().iter().find_map(|i| {
        if let ScreenInstruction::ShowPluginCursor(plugin_id, client_id, cursor_position) = i {
            Some((*plugin_id, *client_id, *cursor_position))
        } else {
            None
        }
    });

    assert_snapshot!(format!("{:#?}", hide_cursor_instruction));
}

#[test]
#[serial(plugin_tests)]
pub fn copy_to_clipboard_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let plugin_should_float = Some(false);
    let plugin_title = Some("test_plugin".to_owned());
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: Default::default(),
        ..Default::default()
    });
    let tab_index = 1;
    let client_id = ClientId::new(1);
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::CopyTextToClipboard,
        screen_receiver,
        1,
        &PermissionType::WriteToClipboard,
        cache_path,
        plugin_thread_sender,
        client_id
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        plugin_should_float,
        false,
        false, // close_replaced_pane
        plugin_title,
        run_plugin,
        Some(TabId::new(tab_index)),
        None,
        client_id,
        size,
        None,
        None,
        false,
        None,
        None,
        None,
        None,
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('c')).with_super_modifier()),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let copy_instruction = received_screen_instructions.lock().iter().find_map(|i| {
        if let ScreenInstruction::CopyTextToClipboard(text, plugin_id) = i {
            Some((text.clone(), *plugin_id))
        } else {
            None
        }
    });

    assert_snapshot!(format!("{:#?}", copy_instruction));
}

#[test]
#[serial(plugin_tests)]
pub fn run_action_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let plugin_should_float = Some(false);
    let plugin_title = Some("test_plugin".to_owned());
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: Default::default(),
        ..Default::default()
    });
    let tab_index = 1;
    let client_id = ClientId::new(1);
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::MoveFocusLeft,
        screen_receiver,
        1,
        &PermissionType::RunActionsAsUser,
        cache_path,
        plugin_thread_sender,
        client_id
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        plugin_should_float,
        false,
        false, // close_replaced_pane
        plugin_title,
        run_plugin,
        Some(TabId::new(tab_index)),
        None,
        client_id,
        size,
        None,
        None,
        false,
        None,
        None,
        None,
        None,
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('d')).with_super_modifier()),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let move_focus_instruction = received_screen_instructions.lock().iter().find_map(|i| {
        if let ScreenInstruction::MoveFocusLeft(client_id, notification_end) = i {
            Some((*client_id, notification_end.clone()))
        } else {
            None
        }
    });

    assert_snapshot!(format!("{:#?}", move_focus_instruction));
}

#[test]
#[serial(plugin_tests)]
pub fn send_sigint_to_pane_id_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, pty_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_pty_receiver(Some(plugin_host_folder), None, None);

    let received_pty_instructions = Watched::new(vec![]);
    let client_id = ClientId::new(1);

    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::SendSigintToPaneId,
        pty_receiver,
        1
    );

    let received_screen_instructions = Watched::new(vec![]);
    let _screen_thread = grant_permissions_and_log_actions_in_thread_naked_variant!(
        received_screen_instructions,
        ScreenInstruction::Exit,
        screen_receiver,
        1,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        client_id
    );

    let plugin_should_float = Some(false);
    let plugin_title = Some("test_plugin".to_owned());
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: Default::default(),
        ..Default::default()
    });
    let tab_index = 1;
    let size = Size {
        cols: 121,
        rows: 20,
    };

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        plugin_should_float,
        false,
        false, // close_replaced_pane
        plugin_title,
        run_plugin,
        Some(TabId::new(tab_index)),
        None,
        client_id,
        size,
        None,
        None,
        false,
        None,
        None,
        None,
        None,
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('e')).with_super_modifier()),
    )]));

    pty_thread.join().unwrap();
    teardown();

    let sigint_instruction = received_pty_instructions.lock().iter().find_map(|i| {
        if let PtyInstruction::SendSigintToPaneId(pane_id) = i {
            Some(*pane_id)
        } else {
            None
        }
    });

    assert_snapshot!(format!("{:#?}", sigint_instruction));
}

#[test]
#[serial(plugin_tests)]
pub fn send_sigkill_to_pane_id_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, pty_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_pty_receiver(Some(plugin_host_folder), None, None);

    let received_pty_instructions = Watched::new(vec![]);
    let client_id = ClientId::new(1);

    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::SendSigkillToPaneId,
        pty_receiver,
        1
    );

    let received_screen_instructions = Watched::new(vec![]);
    let _screen_thread = grant_permissions_and_log_actions_in_thread_naked_variant!(
        received_screen_instructions,
        ScreenInstruction::Exit,
        screen_receiver,
        1,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        client_id
    );

    let plugin_should_float = Some(false);
    let plugin_title = Some("test_plugin".to_owned());
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: Default::default(),
        ..Default::default()
    });
    let tab_index = 1;
    let size = Size {
        cols: 121,
        rows: 20,
    };

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        plugin_should_float,
        false,
        false, // close_replaced_pane
        plugin_title,
        run_plugin,
        Some(TabId::new(tab_index)),
        None,
        client_id,
        size,
        None,
        None,
        false,
        None,
        None,
        None,
        None,
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('f')).with_super_modifier()),
    )]));

    pty_thread.join().unwrap();
    teardown();

    let sigkill_instruction = received_pty_instructions.lock().iter().find_map(|i| {
        if let PtyInstruction::SendSigkillToPaneId(pane_id) = i {
            Some(*pane_id)
        } else {
            None
        }
    });

    assert_snapshot!(format!("{:#?}", sigkill_instruction));
}

#[test]
#[serial(plugin_tests)]
pub fn copy_to_clipboard_without_permission() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);

    let received_screen_instructions = Watched::new(vec![]);
    let client_id = ClientId::new(1);

    // The permission is denied, so no CopyTextToClipboard may reach the screen.
    let screen_thread = deny_permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        client_id,
    );

    let plugin_should_float = Some(false);
    let plugin_title = Some("test_plugin".to_owned());
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: Default::default(),
        ..Default::default()
    });
    let tab_index = 1;
    let size = Size {
        cols: 121,
        rows: 20,
    };

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        plugin_should_float,
        false,
        false, // close_replaced_pane
        plugin_title,
        run_plugin,
        Some(TabId::new(tab_index)),
        None,
        client_id,
        size,
        None,
        None,
        false,
        None,
        None,
        None,
        None,
    ));

    // The key has to reach the plugin after the denial, or the test is about something else.
    wait_for_permissions_to_be_denied(&received_screen_instructions);

    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('c')).with_super_modifier()),
    )]));

    // The key has been handled once the fixture prints it; a CopyTextToClipboard the host wrongly
    // sent while handling it went down the same channel before this frame did.
    wait_for_render_containing(&received_screen_instructions, r#""bare_key":{"Char":"c"}"#);

    // Everything the plugin thread will ever send has been sent once teardown has joined it, and
    // the observer returns when the last sender is gone.
    teardown();
    screen_thread.join().unwrap();

    let copy_instruction = received_screen_instructions.lock().iter().find_map(|i| {
        if let ScreenInstruction::CopyTextToClipboard(text, plugin_id) = i {
            Some((text.clone(), *plugin_id))
        } else {
            None
        }
    });

    // Should be None because permission was denied
    assert_snapshot!(format!("{:#?}", copy_instruction));
}

#[test]
#[serial(plugin_tests)]
pub fn run_action_without_permission() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);

    let received_screen_instructions = Watched::new(vec![]);
    let client_id = ClientId::new(1);

    // The permission is denied, so the action may not be routed: no MoveFocusLeft.
    let screen_thread = deny_permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        client_id,
    );

    let plugin_should_float = Some(false);
    let plugin_title = Some("test_plugin".to_owned());
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: Default::default(),
        ..Default::default()
    });
    let tab_index = 1;
    let size = Size {
        cols: 121,
        rows: 20,
    };

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        plugin_should_float,
        false,
        false, // close_replaced_pane
        plugin_title,
        run_plugin,
        Some(TabId::new(tab_index)),
        None,
        client_id,
        size,
        None,
        None,
        false,
        None,
        None,
        None,
        None,
    ));

    // The key has to reach the plugin after the denial, or the test is about something else.
    wait_for_permissions_to_be_denied(&received_screen_instructions);

    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('d')).with_super_modifier()),
    )]));

    // Proves the key was handled. `run_action` routes on a thread of its own, so a wrongly
    // permitted MoveFocusLeft is not ordered before this frame - it is caught by the observer
    // outliving that thread instead.
    wait_for_render_containing(&received_screen_instructions, r#""bare_key":{"Char":"d"}"#);

    // Everything the plugin thread will ever send has been sent once teardown has joined it, and
    // the observer returns when the last sender is gone.
    teardown();
    screen_thread.join().unwrap();

    let move_focus_instruction = received_screen_instructions.lock().iter().find_map(|i| {
        if let ScreenInstruction::MoveFocusLeft(client_id, notification_end) = i {
            Some((*client_id, notification_end.clone()))
        } else {
            None
        }
    });

    // Should be None because permission was denied
    assert_snapshot!(format!("{:#?}", move_focus_instruction));
}

#[test]
#[serial(plugin_tests)]
fn clipboard_history_instruction_persists_without_loaded_plugins() {
    use yinz_bus::plugins::ClipboardHistoryEntry;
    use yinz_storage::data_access::DataAccess;
    use yinz_types::plugin_data::{DataValue, FetchRequest};

    let dir = tempdir().unwrap();
    let data = DataAccess::sqlite(dir.path());
    let (sender, _server, _screen, teardown) = create_plugin_thread_with_services(
        None,
        None,
        None,
        Default::default(),
        None,
        Some(data.clone()),
        Some(dir.path().to_owned()),
    );
    for _ in 0..2 {
        sender
            .send(PluginInstruction::RecordClipboard(ClipboardHistoryEntry {
                content: "clipboard\0text".into(),
                copied_at_ms: 42,
                host_id: "test-host".into(),
                yinz_session_name: "copy-time-session".into(),
                destination: yinz_types::data::CopyDestination::Primary,
            }))
            .unwrap();
    }
    // Exit follows both submissions on the same channel and joins the plugin thread.
    teardown();
    let rows = data
        .plugin_data()
        .fetch(
            project_data::store::SHARED,
            &FetchRequest::entity("clipboard_history"),
        )
        .unwrap();
    assert_eq!(rows.rows.len(), 2);
    for row in &rows.rows {
        assert_eq!(
            rows.get(row, "content"),
            Some(&DataValue::Blob(b"clipboard\0text".to_vec()))
        );
        assert_eq!(
            rows.get(row, "destination"),
            Some(&DataValue::Text("primary".into()))
        );
        assert_eq!(
            rows.get(row, "yinz_session_name"),
            Some(&DataValue::Text("copy-time-session".into()))
        );
    }
}

#[test]
#[serial(plugin_tests)]
fn clipboard_history_contention_does_not_block_data_requests_or_input_barriers() {
    use yinz_bus::plugins::ClipboardHistoryEntry;
    use yinz_bus::route::NotificationEnd;
    use yinz_types::cli::DataRequest;
    use yinz_types::plugin_data::{DataValue, FetchRequest, WatchMode};

    let dir = tempdir().unwrap();
    let data = DataAccess::sqlite(dir.path());
    data.plugin_data()
        .define_entity(
            project_data::store::SHARED,
            "yinz:clipboard-history",
            &project_data::store::clipboard_history_def(),
            false,
        )
        .unwrap();
    let watch = data
        .plugin_data()
        .register_watch(
            project_data::store::SHARED,
            crate::PluginId::new(1),
            ClientId::new(1),
            FetchRequest::entity("clipboard_history"),
            WatchMode::Rows,
        )
        .unwrap();
    let (events, event_receiver) = channels::unbounded();
    let (sender, _server, _screen, teardown) = create_plugin_thread_with_services(
        None,
        None,
        None,
        Default::default(),
        Some(SenderWithContext::new(events)),
        Some(data.clone()),
        Some(dir.path().to_owned()),
    );
    let barrier = || {
        let (completion, answer) = tokio::sync::oneshot::channel();
        sender
            .send(PluginInstruction::InputBarrier(
                crate::PluginId::new(1),
                NotificationEnd::new(completion),
            ))
            .unwrap();
        answer
    };
    let runtime = yinz_bus::global_async_runtime::get_tokio_runtime();
    runtime.block_on(async {
        tokio::time::timeout(default_timeout(), barrier())
            .await
            .unwrap()
            .unwrap();
    });

    let mut connection = rusqlite::Connection::open(dir.path().join("plugin-data.db")).unwrap();
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    sender
        .send(PluginInstruction::RecordClipboard(ClipboardHistoryEntry {
            content: "contended copy".into(),
            copied_at_ms: 42,
            host_id: "host".into(),
            yinz_session_name: "session".into(),
            destination: yinz_types::data::CopyDestination::System,
        }))
        .unwrap();
    // Exercise synchronous store reads before each barrier while the external write lock is held.
    // Keep submitting for a polling interval so the clipboard worker has time to start even if
    // dispatch wins the initial race. Two seconds is below SQLite's five-second busy timeout.
    let completion = runtime.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            let until = std::time::Instant::now()
                + yinz_storage::plugin_data_store::DATA_WATCH_POLL_INTERVAL;
            loop {
                let replies: Vec<_> = [
                    DataRequest::Catalog {
                        namespace: Some(project_data::store::SHARED.into()),
                    },
                    DataRequest::Fetch(
                        FetchRequest::entity("clipboard_history")
                            .in_namespace(project_data::store::SHARED),
                    ),
                ]
                .into_iter()
                .map(|request| {
                    let (reply, answer) = std::sync::mpsc::channel();
                    sender
                        .send(PluginInstruction::DataRequest { request, reply })
                        .unwrap();
                    answer
                })
                .collect();
                barrier().await.unwrap();
                for reply in replies {
                    // The barrier follows both requests on the same dispatch queue.
                    reply.try_recv().unwrap().unwrap();
                }
                if std::time::Instant::now() >= until {
                    break;
                }
            }
        })
        .await
    });
    assert!(
        event_receiver.is_empty(),
        "clipboard persistence must remain pending until the external write lock is released"
    );
    transaction.commit().unwrap();
    completion.expect("database contention must not stall data requests or input barriers");

    // The independent writer's commit can be observed by the watch poller before the explicit
    // refresh reaches dispatch. Both routes share watch digests and may deliver the change first.
    let deadline = std::time::Instant::now() + default_timeout();
    let (mut refreshed, mut delivered) = (false, false);
    while !refreshed || !delivered {
        let (event, _) = event_receiver
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .unwrap();
        match event {
            PluginInstruction::RefreshDataWatches {
                ref namespace,
                ref entities,
            } => {
                assert_eq!(namespace, project_data::store::SHARED);
                assert!(entities.contains("clipboard_history"));
                refreshed = true;
                sender.send(event).unwrap();
            },
            PluginInstruction::Update(updates) => {
                assert!(
                    !delivered,
                    "a commit must not deliver duplicate watch changes"
                );
                assert!(
                    matches!(&updates[0].2, Event::PluginDataChanged { watch_id, rows, .. }
                    if *watch_id == watch.watch_id && rows.rows.len() == 1
                    && rows.get(&rows.rows[0], "content") == Some(&DataValue::Blob(b"contended copy".to_vec())))
                );
                delivered = true;
            },
            event => panic!("expected clipboard watch refresh or delivery: {event:?}"),
        }
    }
    teardown();
}
