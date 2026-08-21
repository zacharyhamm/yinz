use super::*;

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
    let client_id = 1;
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
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
        Some(tab_index),
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
        .unwrap()
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
    let client_id = 1;
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
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
        Some(tab_index),
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
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(500));
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('x')).with_alt_modifier()), // this triggers the enent in the fixture plugin
    )]));
    screen_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let request_state_update_requests = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .filter(|i| matches!(i, ScreenInstruction::RequestStateUpdateForPlugins))
        .count();
    assert_eq!(request_state_update_requests, 2);
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
    let client_id = 1;
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
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
    let received_server_instruction = Arc::new(Mutex::new(vec![]));
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
        Some(tab_index),
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
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(500));

    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('y')).with_alt_modifier()), // this triggers the enent in the fixture plugin
    )]));
    std::thread::sleep(std::time::Duration::from_millis(500));
    teardown();
    server_thread.join().unwrap(); // this might take a while if the cache is cold
    let rebind_event = received_server_instruction
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find_map(|i| {
            if let ServerInstruction::RebindKeys { .. } = i {
                Some(i.clone())
            } else {
                None
            }
        })
        .clone();
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
    let client_id = 1;
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
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
        Some(tab_index),
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
        .unwrap()
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
    let client_id = 1;
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
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
        Some(tab_index),
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
        None,
    ));
    std::thread::sleep(std::time::Duration::from_millis(5000));
    // here we send an unload to plugin id 0 (the first plugin id, presumably this plugin)
    // so that its BeforeClose Event will be triggered and it will send a
    // HighlightAndUnhighlightPanes
    // instruction which we can assert below
    let _ = plugin_thread_sender.send(PluginInstruction::Unload(0));
    screen_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let sent_instruction = received_screen_instructions
        .lock()
        .unwrap()
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
    let client_id = 1;
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
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
        Some(tab_index),
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

    let show_cursor_instruction = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
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
    let client_id = 1;
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
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
        Some(tab_index),
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

    let hide_cursor_instruction = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
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
    let client_id = 1;
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
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
        Some(tab_index),
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

    let copy_instruction = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
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
    let client_id = 1;
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
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
        Some(tab_index),
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

    let move_focus_instruction = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
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

    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let client_id = 1;

    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::SendSigintToPaneId,
        pty_receiver,
        1
    );

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
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
        Some(tab_index),
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

    let sigint_instruction = received_pty_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
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

    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let client_id = 1;

    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::SendSigkillToPaneId,
        pty_receiver,
        1
    );

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
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
        Some(tab_index),
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

    let sigkill_instruction = received_pty_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
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

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let client_id = 1;

    // This test denies the permission and expects no CopyTextToClipboard instruction
    let screen_thread = deny_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::CopyTextToClipboard,
        screen_receiver,
        1,
        &PermissionType::WriteToClipboard,
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
        Some(tab_index),
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
        None,
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('c')).with_super_modifier()),
    )]));

    // Give it time to potentially (incorrectly) send the instruction
    std::thread::sleep(std::time::Duration::from_millis(1000));

    let _ = plugin_thread_sender.send(PluginInstruction::Exit);
    screen_thread.join().unwrap();
    teardown();

    let copy_instruction = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
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

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let client_id = 1;

    let screen_thread = deny_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::MoveFocusLeft,
        screen_receiver,
        1,
        &PermissionType::RunActionsAsUser,
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
        Some(tab_index),
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
        None,
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('d')).with_super_modifier()),
    )]));

    std::thread::sleep(std::time::Duration::from_millis(1000));

    let _ = plugin_thread_sender.send(PluginInstruction::Exit);
    screen_thread.join().unwrap();
    teardown();

    let move_focus_instruction = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::MoveFocusLeft(client_id, notification_end) = i {
                Some((*client_id, notification_end.clone()))
            } else {
                None
            }
        });

    // Should be None because permission was denied
    assert_snapshot!(format!("{:#?}", move_focus_instruction));
}
