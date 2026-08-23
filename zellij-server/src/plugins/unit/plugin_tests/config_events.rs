use super::*;
use zellij_test_support::Watched;

#[test]
#[serial(plugin_tests)]
pub fn generate_random_name_permission_denied() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Watched::new(vec![]);
    let client_id = 1;

    let screen_thread = deny_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2,
        &PermissionType::ReadApplicationState,
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

    // Give it time to potentially (incorrectly) send the output
    std::thread::sleep(std::time::Duration::from_millis(1000));

    let _ = plugin_thread_sender.send(PluginInstruction::Exit);
    screen_thread.join().unwrap();
    teardown();

    let plugin_bytes_with_name = received_screen_instructions.lock().iter().find_map(|i| {
        if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
            for plugin_render_asset in plugin_render_assets {
                let plugin_bytes = plugin_render_asset.bytes.clone();
                let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                if plugin_bytes.contains("Generated name:") {
                    return Some(plugin_bytes);
                }
            }
        }
        None
    });

    // Should be None because permission was denied
    assert_snapshot!(format!("{:#?}", plugin_bytes_with_name));
}

#[test]
#[serial(plugin_tests)]
pub fn save_layout_permission_denied() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Watched::new(vec![]);
    let client_id = 1;

    let screen_thread = deny_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2,
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

    // Give it time to potentially (incorrectly) send the output
    std::thread::sleep(std::time::Duration::from_millis(1000));

    let _ = plugin_thread_sender.send(PluginInstruction::Exit);
    screen_thread.join().unwrap();
    teardown();

    let plugin_bytes_with_save = received_screen_instructions.lock().iter().find_map(|i| {
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

    // Should be None because permission was denied
    assert_snapshot!(format!("{:#?}", plugin_bytes_with_save));
}

#[test]
#[serial(plugin_tests)]
pub fn plugin_receives_config_change_event() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) = create_plugin_thread(None, None);

    let mut initial_config = BTreeMap::new();
    initial_config.insert("theme".to_owned(), "dark".to_owned());
    initial_config.insert("size".to_owned(), "large".to_owned());

    let plugin_should_float = Some(false);
    let plugin_title = Some("test_plugin".to_owned());
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: PluginUserConfiguration::new(initial_config.clone()),
        ..Default::default()
    });

    let tab_index = 1;
    let client_id = 1;
    let size = Size {
        cols: 121,
        rows: 20,
    };

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2,
        &PermissionType::ReadApplicationState,
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
        run_plugin.clone(),
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

    let mut new_config = BTreeMap::new();
    new_config.insert("theme".to_owned(), "light".to_owned()); // Changed
    new_config.insert("size".to_owned(), "small".to_owned()); // Changed

    let mut plugin_aliases = PluginAliases::default();
    plugin_aliases.aliases.insert(
        "test_plugin".to_owned(),
        RunPlugin {
            _allow_exec_host_cmd: false,
            location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
            configuration: PluginUserConfiguration::new(new_config.clone()),
            ..Default::default()
        },
    );

    let _ = plugin_thread_sender.send(PluginInstruction::DetectPluginConfigChanges(plugin_aliases));

    screen_thread.join().unwrap();
    teardown();

    let plugin_renders = received_screen_instructions
        .lock()
        .iter()
        .filter_map(|i| {
            if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                plugin_render_assets.first().map(|plugin_render_asset| {
                    let plugin_bytes = plugin_render_asset.bytes.clone();
                    String::from_utf8_lossy(plugin_bytes.as_slice()).to_string()
                })
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    assert!(
        plugin_renders
            .iter()
            .any(|r| r.contains("PluginConfigurationChanged")),
        "Plugin should receive PluginConfigurationChanged event"
    );

    let last_render = plugin_renders
        .last()
        .expect("Should have at least one render");
    assert!(
        last_render.contains("theme") && last_render.contains("light"),
        "Plugin should render with new theme=light config"
    );
    assert!(
        last_render.contains("size") && last_render.contains("small"),
        "Plugin should render with new size=small config"
    );

    assert_snapshot!(format!("{:#?}", last_render));
}

#[test]
#[serial(plugin_tests)]
pub fn plugin_does_not_receive_event_when_config_unchanged() {
    // Test that plugin does NOT receive event when config hasn't changed
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) = create_plugin_thread(None, None);

    // Initial plugin configuration
    let mut initial_config = BTreeMap::new();
    initial_config.insert("theme".to_owned(), "dark".to_owned());

    let plugin_should_float = Some(false);
    let plugin_title = Some("test_plugin".to_owned());
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: PluginUserConfiguration::new(initial_config.clone()),
        ..Default::default()
    });

    let tab_index = 1;
    let client_id = 1;
    let size = Size {
        cols: 121,
        rows: 20,
    };

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2, // Only expect initial load and one update, no config change event
        &PermissionType::ReadApplicationState,
        cache_path,
        plugin_thread_sender,
        client_id
    );

    // Load plugin
    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        plugin_should_float,
        false,
        false, // close_replaced_pane
        plugin_title,
        run_plugin.clone(),
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

    // Send SAME configuration (unchanged)
    let mut plugin_aliases = PluginAliases::default();
    plugin_aliases.aliases.insert(
        "test_plugin".to_owned(),
        RunPlugin {
            _allow_exec_host_cmd: false,
            location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
            configuration: PluginUserConfiguration::new(initial_config.clone()), // SAME config
            ..Default::default()
        },
    );

    let _ = plugin_thread_sender.send(PluginInstruction::DetectPluginConfigChanges(plugin_aliases));

    // Send an unrelated event to trigger a render
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::InputReceived,
    )]));

    screen_thread.join().unwrap();
    teardown();

    // Verify plugin did NOT receive PluginConfigurationChanged event
    let plugin_renders = received_screen_instructions
        .lock()
        .iter()
        .filter_map(|i| {
            if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                plugin_render_assets.first().map(|plugin_render_asset| {
                    let plugin_bytes = plugin_render_asset.bytes.clone();
                    String::from_utf8_lossy(plugin_bytes.as_slice()).to_string()
                })
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    assert!(
        !plugin_renders
            .iter()
            .any(|r| r.contains("PluginConfigurationChanged")),
        "Plugin should NOT receive PluginConfigurationChanged when config unchanged"
    );
}

#[test]
#[serial(plugin_tests)]
pub fn get_session_environment_variables_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");

    let mut test_env_vars = std::collections::BTreeMap::new();
    test_env_vars.insert("TEST_ENV_VAR_1".to_string(), "test_value_123".to_string());
    test_env_vars.insert(
        "TEST_ENV_VAR_2".to_string(),
        "another_test_value".to_string(),
    );
    test_env_vars.insert("PATH".to_string(), "/usr/bin:/bin".to_string());

    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), Some(test_env_vars));

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2,
        &PermissionType::ReadSessionEnvironmentVariables,
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
        None,
    ));

    std::thread::sleep(std::time::Duration::from_millis(500));

    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('j'))
                .with_ctrl_modifier()
                .with_shift_modifier(),
        ),
    )]));

    screen_thread.join().unwrap();
    teardown();

    let plugin_bytes_event = received_screen_instructions.lock().iter().find_map(|i| {
        if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
            for plugin_render_asset in plugin_render_assets {
                let plugin_bytes = plugin_render_asset.bytes.clone();
                let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                if plugin_bytes.contains("Got") && plugin_bytes.contains("environment variables") {
                    return Some(plugin_bytes);
                }
            }
        }
        None
    });
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}
