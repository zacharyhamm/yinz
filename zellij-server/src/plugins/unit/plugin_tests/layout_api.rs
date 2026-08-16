use super::*;

#[test]
#[ignore]
pub fn generate_random_name_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2,
        &PermissionType::ReadApplicationState,
        cache_path,
        plugin_thread_sender,
        1
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
    let client_id = 1;
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
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Send Ctrl+Shift+A to trigger generate_random_name()
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('a'))
                .with_ctrl_modifier()
                .with_shift_modifier(),
        ),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let plugin_bytes_event = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                for plugin_render_asset in plugin_render_assets {
                    let plugin_bytes = plugin_render_asset.bytes.clone();
                    let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                    if plugin_bytes.contains("Generated name") {
                        return Some(plugin_bytes);
                    }
                }
            }
            None
        });
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[ignore]
pub fn dump_layout_success_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2,
        &PermissionType::ReadApplicationState,
        cache_path,
        plugin_thread_sender,
        1
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
    let client_id = 1;
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
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Send Ctrl+Shift+B to trigger dump_layout("default")
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('b'))
                .with_ctrl_modifier()
                .with_shift_modifier(),
        ),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let plugin_bytes_event = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                for plugin_render_asset in plugin_render_assets {
                    let plugin_bytes = plugin_render_asset.bytes.clone();
                    let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                    if plugin_bytes.contains("Layout dump") {
                        return Some(plugin_bytes);
                    }
                }
            }
            None
        });
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

/// The data store round trip across the wasm boundary: declare an entity, write to it, read it
/// back, full-text search it, count it and watch it.
///
/// The engine itself is covered by unit tests in `zellij-utils`; what this covers is the hop those
/// cannot reach — protobuf encode, the JSON-wrapped pipe, dispatch, and decode back in the guest.
#[test]
#[ignore]
pub fn data_store_plugin_commands() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2,
        &PermissionType::PluginData,
        cache_path,
        plugin_thread_sender,
        1
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
    let client_id = 1;
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
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Ctrl+Shift+D runs the whole data store round trip in the fixture plugin
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('d'))
                .with_ctrl_modifier()
                .with_shift_modifier(),
        ),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let plugin_bytes_event = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                for plugin_render_asset in plugin_render_assets {
                    let plugin_bytes = plugin_render_asset.bytes.clone();
                    let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                    if plugin_bytes.contains("Data store") {
                        return Some(plugin_bytes);
                    }
                }
            }
            None
        });
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[ignore]
pub fn dump_layout_not_found_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2,
        &PermissionType::ReadApplicationState,
        cache_path,
        plugin_thread_sender,
        1
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
    let client_id = 1;
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
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Send Ctrl+Shift+C to trigger dump_layout("nonexistent_layout_xyz")
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('c'))
                .with_ctrl_modifier()
                .with_shift_modifier(),
        ),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let plugin_bytes_event = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                for plugin_render_asset in plugin_render_assets {
                    let plugin_bytes = plugin_render_asset.bytes.clone();
                    let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                    if plugin_bytes.contains("Layout dump error") {
                        return Some(plugin_bytes);
                    }
                }
            }
            None
        });
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[ignore]
pub fn get_layout_dir_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2,
        &PermissionType::ReadApplicationState,
        cache_path,
        plugin_thread_sender,
        1
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
    let client_id = 1;
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
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Send Ctrl+Shift+D to trigger get_layout_dir()
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('d'))
                .with_ctrl_modifier()
                .with_shift_modifier(),
        ),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let plugin_bytes_event = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                for plugin_render_asset in plugin_render_assets {
                    let plugin_bytes = plugin_render_asset.bytes.clone();
                    let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                    if plugin_bytes.contains("Got layout folder") {
                        return Some(plugin_bytes);
                    }
                }
            }
            None
        });
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[ignore]
pub fn get_focused_pane_info_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let screen_thread = grant_permissions_and_log_actions_in_thread_struct_variant!(
        received_screen_instructions,
        ScreenInstruction::GetFocusedPaneInfo,
        screen_receiver,
        1,
        &PermissionType::ReadApplicationState,
        cache_path,
        plugin_thread_sender,
        1
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
    let client_id = 1;
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
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Send Ctrl+Shift+E to trigger get_focused_pane_info()
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('e'))
                .with_ctrl_modifier()
                .with_shift_modifier(),
        ),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let focused_pane_info_query = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::GetFocusedPaneInfo { .. } = i {
                return Some(i.clone());
            } else {
                None
            }
        });
    assert_snapshot!(format!("{:#?}", focused_pane_info_query));
}

#[test]
#[ignore]
pub fn dump_session_layout_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2,
        &PermissionType::ReadApplicationState,
        cache_path,
        plugin_thread_sender,
        1
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
    let client_id = 1;
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
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Send Ctrl+Shift+F to trigger dump_session_layout()
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('f'))
                .with_ctrl_modifier()
                .with_shift_modifier(),
        ),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let dump_layout_instruction = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::DumpLayoutToPlugin { .. } = i {
                return Some(i.clone());
            }
            None
        });
    assert_snapshot!(format!("{:#?}", dump_layout_instruction));
}

#[test]
#[ignore]
pub fn dump_session_layout_for_tab_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2,
        &PermissionType::ReadApplicationState,
        cache_path,
        plugin_thread_sender,
        1
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
    let client_id = 1;
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
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Send Ctrl+Shift+G to trigger dump_session_layout_for_tab(0)
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('g'))
                .with_ctrl_modifier()
                .with_shift_modifier(),
        ),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let dump_layout_instruction = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::DumpLayoutToPlugin { .. } = i {
                return Some(i.clone());
            }
            None
        });
    assert_snapshot!(format!("{:#?}", dump_layout_instruction));
}

#[test]
#[ignore]
pub fn parse_layout_success_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2,
        &PermissionType::ReadApplicationState,
        cache_path,
        plugin_thread_sender,
        1
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
    let client_id = 1;
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
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Send Ctrl+Shift+H to trigger parse_layout() with valid KDL
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('h'))
                .with_ctrl_modifier()
                .with_shift_modifier(),
        ),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let plugin_bytes_event = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                for plugin_render_asset in plugin_render_assets {
                    let plugin_bytes = plugin_render_asset.bytes.clone();
                    let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                    if plugin_bytes.contains("Parse success") {
                        return Some(plugin_bytes);
                    }
                }
            }
            None
        });
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[ignore]
pub fn parse_layout_error_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2,
        &PermissionType::ReadApplicationState,
        cache_path,
        plugin_thread_sender,
        1
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
    let client_id = 1;
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
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Send Ctrl+Shift+I to trigger parse_layout() with invalid KDL
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('i'))
                .with_ctrl_modifier()
                .with_shift_modifier(),
        ),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let plugin_bytes_event = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                for plugin_render_asset in plugin_render_assets {
                    let plugin_bytes = plugin_render_asset.bytes.clone();
                    let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                    if plugin_bytes.contains("Parse error:") {
                        return Some(plugin_bytes);
                    }
                }
            }
            None
        });
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[ignore]
pub fn save_layout_success_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        1
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
    let client_id = 1;
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
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Send Ctrl+Alt+A to trigger save_layout()
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('a'))
                .with_ctrl_modifier()
                .with_alt_modifier(),
        ),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let plugin_bytes_event = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                for plugin_render_asset in plugin_render_assets {
                    let plugin_bytes = plugin_render_asset.bytes.clone();
                    let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                    if plugin_bytes.contains("Save layout") {
                        return Some(plugin_bytes);
                    }
                }
            }
            None
        });
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[ignore]
pub fn save_layout_already_exists_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        3,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        1
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
    let client_id = 1;
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
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Send Ctrl+Alt+A to save the layout first time
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('a'))
                .with_ctrl_modifier()
                .with_alt_modifier(),
        ),
    )]));

    std::thread::sleep(std::time::Duration::from_millis(200));

    // Send Ctrl+Alt+B to try saving without overwrite
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('b'))
                .with_ctrl_modifier()
                .with_alt_modifier(),
        ),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let plugin_bytes_event = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find_map(|i| {
            if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                for plugin_render_asset in plugin_render_assets {
                    let plugin_bytes = plugin_render_asset.bytes.clone();
                    let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                    if plugin_bytes.contains("error") {
                        return Some(plugin_bytes);
                    }
                }
            }
            None
        });
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[ignore]
pub fn save_layout_with_overwrite_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        1
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
    let client_id = 1;
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
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Send Ctrl+Alt+C to save with overwrite=true
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('c'))
                .with_ctrl_modifier()
                .with_alt_modifier(),
        ),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let plugin_bytes_event = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                for plugin_render_asset in plugin_render_assets {
                    let plugin_bytes = plugin_render_asset.bytes.clone();
                    let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                    if plugin_bytes.contains("Save layout") {
                        return Some(plugin_bytes);
                    }
                }
            }
            None
        });
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[ignore]
pub fn save_layout_invalid_kdl_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        1
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
    let client_id = 1;
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
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Send Ctrl+Alt+D to save invalid KDL
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('d'))
                .with_ctrl_modifier()
                .with_alt_modifier(),
        ),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let plugin_bytes_event = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                for plugin_render_asset in plugin_render_assets {
                    let plugin_bytes = plugin_render_asset.bytes.clone();
                    let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                    if plugin_bytes.contains("invalid layout error") {
                        return Some(plugin_bytes);
                    }
                }
            }
            None
        });
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[ignore]
pub fn rename_layout_success_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        3,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        1
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
    let client_id = 1;
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
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Send Ctrl+Alt+A to save layout first
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('a'))
                .with_ctrl_modifier()
                .with_alt_modifier(),
        ),
    )]));

    std::thread::sleep(std::time::Duration::from_millis(200));

    // Send Ctrl+Alt+E to rename layout
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('e'))
                .with_ctrl_modifier()
                .with_alt_modifier(),
        ),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let plugin_bytes_event = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find_map(|i| {
            if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                for plugin_render_asset in plugin_render_assets {
                    let plugin_bytes = plugin_render_asset.bytes.clone();
                    let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                    if plugin_bytes.contains("Rename layout") {
                        return Some(plugin_bytes);
                    }
                }
            }
            None
        });
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[ignore]
pub fn rename_layout_not_found_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        1
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
    let client_id = 1;
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
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Send Ctrl+Alt+F to rename nonexistent layout
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('f'))
                .with_ctrl_modifier()
                .with_alt_modifier(),
        ),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let plugin_bytes_event = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                for plugin_render_asset in plugin_render_assets {
                    let plugin_bytes = plugin_render_asset.bytes.clone();
                    let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                    if plugin_bytes.contains("Rename") {
                        return Some(plugin_bytes);
                    }
                }
            }
            None
        });
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[ignore]
pub fn delete_layout_success_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        3,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        1
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
    let client_id = 1;
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
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Send Ctrl+Alt+A to save layout first
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('a'))
                .with_ctrl_modifier()
                .with_alt_modifier(),
        ),
    )]));

    std::thread::sleep(std::time::Duration::from_millis(200));

    // Send Ctrl+Alt+E to rename layout
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('e'))
                .with_ctrl_modifier()
                .with_alt_modifier(),
        ),
    )]));

    std::thread::sleep(std::time::Duration::from_millis(200));

    // Send Ctrl+Alt+G to delete renamed layout
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('g'))
                .with_ctrl_modifier()
                .with_alt_modifier(),
        ),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let plugin_bytes_event = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find_map(|i| {
            if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                for plugin_render_asset in plugin_render_assets {
                    let plugin_bytes = plugin_render_asset.bytes.clone();
                    let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                    if plugin_bytes.contains("Delete layout") {
                        return Some(plugin_bytes);
                    }
                }
            }
            None
        });
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[ignore]
pub fn delete_layout_not_found_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        1
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
    let client_id = 1;
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
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Send Ctrl+Alt+H to delete nonexistent layout
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('h'))
                .with_ctrl_modifier()
                .with_alt_modifier(),
        ),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let plugin_bytes_event = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                for plugin_render_asset in plugin_render_assets {
                    let plugin_bytes = plugin_render_asset.bytes.clone();
                    let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                    if plugin_bytes.contains("Delete") {
                        return Some(plugin_bytes);
                    }
                }
            }
            None
        });
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[ignore]
pub fn save_layout_path_traversal_blocked_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        1
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
    let client_id = 1;
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
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Send Ctrl+Alt+I to test path traversal blocking
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('i'))
                .with_ctrl_modifier()
                .with_alt_modifier(),
        ),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let plugin_bytes_event = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                for plugin_render_asset in plugin_render_assets {
                    let plugin_bytes = plugin_render_asset.bytes.clone();
                    let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                    if plugin_bytes.contains("Path traversal") {
                        return Some(plugin_bytes);
                    }
                }
            }
            None
        });
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[ignore]
pub fn edit_layout_plugin_command() {
    let temp_host_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_host_folder.path());
    let temp_layout_folder = tempdir().unwrap();
    let layout_folder_path = PathBuf::from(temp_layout_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, pty_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_pty_receiver(
            Some(plugin_host_folder),
            Some(layout_folder_path),
            None,
        );

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::SpawnInPlaceTerminal,
        pty_receiver,
        1
    );
    let _screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        3,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        1
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
    let client_id = 1;
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
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Send Ctrl+Alt+J to trigger edit_layout()
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('j'))
                .with_ctrl_modifier()
                .with_alt_modifier(),
        ),
    )]));

    pty_thread.join().unwrap();
    teardown();

    // this is the editor pane for the layout being spawned
    let spawn_terminal_instruction =
        received_pty_instructions
            .lock()
            .unwrap()
            .iter()
            .find_map(|i| {
                if let PtyInstruction::SpawnInPlaceTerminal(..) = i {
                    Some(i.clone())
                } else {
                    None
                }
            });

    assert_snapshot!(format!("{:#?}", spawn_terminal_instruction).replace(
        &format!("{}", temp_layout_folder.path().display()),
        "LAYOUT_FOLDER_PATH"
    ))
}

#[test]
#[ignore]
pub fn override_layout_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::OverrideLayout,
        screen_receiver,
        1,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        1
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
    let client_id = 1;
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
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    // Send Ctrl+Alt+K to trigger override_layout()
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('k'))
                .with_ctrl_modifier()
                .with_alt_modifier(),
        ),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let override_layout_instruction = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::OverrideLayout(..) = i {
                Some(i.clone())
            } else {
                None
            }
        });
    assert_snapshot!(format!("{:#?}", override_layout_instruction));
}
