use super::*;
use yinz_test_support::Watched;
use yinz_types::internal_ids::TabId;

#[test]
#[serial(plugin_tests)]
pub fn start_or_reload_plugin() {
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
        ScreenInstruction::StartOrReloadPluginPane,
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
        Event::Key(KeyWithModifier::new(BareKey::Char('f')).with_ctrl_modifier()), // this triggers the enent in the fixture plugin
    )]));
    screen_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let new_tab_event = received_screen_instructions
        .lock()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::StartOrReloadPluginPane(..) = i {
                Some(i.clone())
            } else {
                None
            }
        })
        .clone();
    assert!(matches!(
        new_tab_event,
        Some(ScreenInstruction::StartOrReloadPluginPane(_, _, client, _)) if client == client_id
    ));
    assert_snapshot!(format!("{:#?}", new_tab_event));
}

#[test]
#[serial(plugin_tests)]
pub fn quit_yinz_plugin_command() {
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
    let received_server_instruction = Watched::new(vec![]);
    let server_thread = log_actions_in_thread!(
        received_server_instruction,
        ServerInstruction::ClientExit,
        server_receiver,
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
        Event::Key(KeyWithModifier::new(BareKey::Char('8'))), // this triggers the enent in the fixture plugin
    )]));
    server_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let new_tab_event = received_server_instruction
        .lock()
        .iter()
        .find_map(|i| {
            if let ServerInstruction::ClientExit(..) = i {
                Some(i.clone())
            } else {
                None
            }
        })
        .clone();
    assert_snapshot!(format!("{:#?}", new_tab_event));
}

#[test]
#[serial(plugin_tests)]
pub fn detach_plugin_command() {
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
    let received_server_instruction = Watched::new(vec![]);
    let server_thread = log_actions_in_thread!(
        received_server_instruction,
        ServerInstruction::DetachSession,
        server_receiver,
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
        Event::Key(KeyWithModifier::new(BareKey::Char('l'))), // this triggers the enent in the fixture plugin
    )]));
    server_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let new_tab_event = received_server_instruction
        .lock()
        .iter()
        .find_map(|i| {
            if let ServerInstruction::DetachSession(..) = i {
                Some(i.clone())
            } else {
                None
            }
        })
        .clone();
    assert_snapshot!(format!("{:#?}", new_tab_event));
}

#[test]
#[serial(plugin_tests)]
pub fn open_file_floating_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, pty_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_pty_receiver(Some(plugin_host_folder), None, None);
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
    let received_pty_instructions = Watched::new(vec![]);
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::SpawnTerminal,
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
        Event::Key(KeyWithModifier::new(BareKey::Char('h')).with_ctrl_modifier()), // this triggers the enent in the fixture plugin
    )]));
    pty_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let new_tab_event = received_pty_instructions
        .lock()
        .iter()
        .find_map(|i| {
            if let PtyInstruction::SpawnTerminal(..) = i {
                Some(i.clone())
            } else {
                None
            }
        })
        .clone();
    // we do the replace below to avoid the randomness of the temporary folder in the snapshot
    // while still testing it
    assert_snapshot!(
        format!("{:#?}", new_tab_event).replace(&format!("{:?}", temp_folder.path()), "\"CWD\"")
    );
}

#[test]
#[serial(plugin_tests)]
pub fn open_file_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, pty_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_pty_receiver(Some(plugin_host_folder), None, None);
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
    let received_pty_instructions = Watched::new(vec![]);
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::SpawnTerminal,
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
        Event::Key(KeyWithModifier::new(BareKey::Char('g')).with_ctrl_modifier()), // this triggers the enent in the fixture plugin
    )]));
    pty_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let new_tab_event = received_pty_instructions
        .lock()
        .iter()
        .find_map(|i| {
            if let PtyInstruction::SpawnTerminal(..) = i {
                Some(i.clone())
            } else {
                None
            }
        })
        .clone();
    // we do the replace below to avoid the randomness of the temporary folder in the snapshot
    // while still testing it
    assert_snapshot!(
        format!("{:#?}", new_tab_event).replace(&format!("{:?}", temp_folder.path()), "\"CWD\"")
    );
}

#[test]
#[serial(plugin_tests)]
pub fn open_file_with_line_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, pty_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_pty_receiver(Some(plugin_host_folder), None, None);
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
    let received_pty_instructions = Watched::new(vec![]);
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::SpawnTerminal,
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
        Event::Key(KeyWithModifier::new(BareKey::Char('i')).with_ctrl_modifier()), // this triggers the enent in the fixture plugin
    )]));
    pty_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let new_tab_event = received_pty_instructions
        .lock()
        .iter()
        .find_map(|i| {
            if let PtyInstruction::SpawnTerminal(..) = i {
                Some(i.clone())
            } else {
                None
            }
        })
        .clone();
    // we do the replace below to avoid the randomness of the temporary folder in the snapshot
    // while still testing it
    assert_snapshot!(
        format!("{:#?}", new_tab_event).replace(&format!("{:?}", temp_folder.path()), "\"CWD\"")
    );
}

#[test]
#[serial(plugin_tests)]
pub fn open_file_with_line_floating_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, pty_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_pty_receiver(Some(plugin_host_folder), None, None);
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
    let received_pty_instructions = Watched::new(vec![]);
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::SpawnTerminal,
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
        Event::Key(KeyWithModifier::new(BareKey::Char('j')).with_ctrl_modifier()), // this triggers the enent in the fixture plugin
    )]));
    pty_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let new_tab_event = received_pty_instructions
        .lock()
        .iter()
        .find_map(|i| {
            if let PtyInstruction::SpawnTerminal(..) = i {
                Some(i.clone())
            } else {
                None
            }
        })
        .clone();
    // we do the replace below to avoid the randomness of the temporary folder in the snapshot
    // while still testing it
    assert_snapshot!(
        format!("{:#?}", new_tab_event).replace(&format!("{:?}", temp_folder.path()), "\"CWD\"")
    );
}

#[test]
#[serial(plugin_tests)]
pub fn open_terminal_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, pty_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_pty_receiver(Some(plugin_host_folder), None, None);
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
    let received_pty_instructions = Watched::new(vec![]);
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::SpawnTerminal,
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
        Event::Key(KeyWithModifier::new(BareKey::Char('k')).with_ctrl_modifier()), // this triggers the enent in the fixture plugin
    )]));
    pty_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let new_tab_event = received_pty_instructions
        .lock()
        .iter()
        .find_map(|i| {
            if let PtyInstruction::SpawnTerminal(..) = i {
                Some(i.clone())
            } else {
                None
            }
        })
        .clone();
    assert_snapshot!(format!("{:#?}", new_tab_event));
}

#[test]
#[serial(plugin_tests)]
pub fn open_terminal_floating_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, pty_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_pty_receiver(Some(plugin_host_folder), None, None);
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
    let received_pty_instructions = Watched::new(vec![]);
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::SpawnTerminal,
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
        Event::Key(KeyWithModifier::new(BareKey::Char('l')).with_ctrl_modifier()), // this triggers the enent in the fixture plugin
    )]));
    pty_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let new_tab_event = received_pty_instructions
        .lock()
        .iter()
        .find_map(|i| {
            if let PtyInstruction::SpawnTerminal(..) = i {
                Some(i.clone())
            } else {
                None
            }
        })
        .clone();
    assert_snapshot!(format!("{:#?}", new_tab_event));
}

#[test]
#[serial(plugin_tests)]
pub fn open_command_pane_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, pty_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_pty_receiver(Some(plugin_host_folder), None, None);
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
    let received_pty_instructions = Watched::new(vec![]);
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::SpawnTerminal,
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
        Event::Key(KeyWithModifier::new(BareKey::Char('m')).with_ctrl_modifier()), // this triggers the enent in the fixture plugin
    )]));
    pty_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let new_tab_event = received_pty_instructions
        .lock()
        .iter()
        .find_map(|i| {
            if let PtyInstruction::SpawnTerminal(..) = i {
                Some(i.clone())
            } else {
                None
            }
        })
        .clone();
    assert_snapshot!(format!("{:#?}", new_tab_event));
}

#[test]
#[serial(plugin_tests)]
pub fn open_command_pane_floating_plugin_command() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, pty_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_pty_receiver(Some(plugin_host_folder), None, None);
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
    let received_pty_instructions = Watched::new(vec![]);
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::SpawnTerminal,
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
        Event::Key(KeyWithModifier::new(BareKey::Char('n')).with_ctrl_modifier()), // this triggers the enent in the fixture plugin
    )]));
    pty_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let new_tab_event = received_pty_instructions
        .lock()
        .iter()
        .find_map(|i| {
            if let PtyInstruction::SpawnTerminal(..) = i {
                Some(i.clone())
            } else {
                None
            }
        })
        .clone();
    assert_snapshot!(format!("{:#?}", new_tab_event));
}
