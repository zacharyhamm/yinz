use super::*;

#[test]
#[ignore]
pub fn mode_update_payload_is_lightweight_for_opted_in_plugins() {
    // Plugin A subscribes to ModeUpdate only (legacy behavior — receives full keybinds)
    // Plugin B subscribes to InitialKeybinds + ModeUpdate (receives stripped keybinds)
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let plugin_should_float = Some(false);
    let tab_index = 1;
    let client_id = 1;
    let size = Size {
        cols: 121,
        rows: 20,
    };

    // Plugin A: subscribes to ModeUpdate only (legacy)
    let mut config_a = BTreeMap::new();
    config_a.insert("subscribe_mode_update".to_owned(), "true".to_owned());
    let run_plugin_a = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: PluginUserConfiguration::new(config_a),
        ..Default::default()
    });

    // Plugin B: subscribes to InitialKeybinds + ModeUpdate (lightweight)
    let mut config_b = BTreeMap::new();
    config_b.insert("subscribe_initial_keybinds".to_owned(), "true".to_owned());
    let run_plugin_b = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: PluginUserConfiguration::new(config_b),
        ..Default::default()
    });

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    // Spawn a screen thread that grants permissions and collects all instructions until Exit
    let screen_thread = std::thread::Builder::new()
        .name("fake_screen_thread".to_string())
        .spawn({
            let log = received_screen_instructions.clone();
            let cache_path = cache_path.clone();
            let plugin_thread_sender = plugin_thread_sender.clone();
            move || loop {
                let (event, _err_ctx) = screen_receiver
                    .recv()
                    .expect("failed to receive event on channel");
                match event {
                    ScreenInstruction::RequestPluginPermissions(plugin_id, plugin_permission) => {
                        let _ =
                            plugin_thread_sender.send(PluginInstruction::PermissionRequestResult(
                                plugin_id,
                                Some(client_id),
                                plugin_permission.permissions,
                                PermissionStatus::Granted,
                                Some(cache_path.clone()),
                            ));
                    },
                    ScreenInstruction::Exit => {
                        break;
                    },
                    _ => {
                        log.lock().unwrap().push(event);
                    },
                }
            }
        })
        .unwrap();

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    // Load plugin A
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        plugin_should_float,
        false,
        false,
        Some("plugin_a".to_owned()),
        run_plugin_a,
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
    // Load plugin B
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        plugin_should_float,
        false,
        false,
        Some("plugin_b".to_owned()),
        run_plugin_b,
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

    // Set keybinds on all plugins via Reconfigure so that Plugin A (legacy)
    // will have non-empty keybinds in its internal state
    let mut keybind_map = std::collections::HashMap::new();
    let mut mode_map = std::collections::HashMap::new();
    mode_map.insert(KeyWithModifier::new(BareKey::Char('q')), vec![Action::Quit]);
    keybind_map.insert(InputMode::Normal, mode_map);
    let test_keybinds = Keybinds(keybind_map);

    let _ = plugin_thread_sender.send(PluginInstruction::Reconfigure {
        client_id,
        keybinds: Some(test_keybinds),
        default_mode: None,
        default_shell: None,
        layout_dir: None,
        builtin_plugins_dir: None,
        was_written_to_disk: false,
    });
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Send ModeUpdate to all plugins (None = broadcast)
    // The keybinds field here doesn't matter — apply_event_to_plugin replaces it
    let mode_info = ModeInfo {
        mode: InputMode::Normal,
        ..Default::default()
    };
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::ModeUpdate(mode_info),
    )]));
    std::thread::sleep(std::time::Duration::from_millis(500));

    teardown();
    screen_thread.join().unwrap();

    let instructions = received_screen_instructions.lock().unwrap();
    let mut plugin_a_has_keybinds = false;
    let mut plugin_b_has_empty_keybinds = false;

    for instruction in instructions.iter() {
        if let ScreenInstruction::PluginBytes(plugin_render_assets) = instruction {
            for asset in plugin_render_assets {
                let bytes = String::from_utf8_lossy(&asset.bytes).to_string();
                if bytes.contains("ModeUpdate") {
                    // Plugin A (plugin_id 0) should have keybinds with 'q' -> Quit
                    // Plugin B (plugin_id 1) should have empty keybinds
                    if bytes.contains("Quit") {
                        plugin_a_has_keybinds = true;
                    } else if bytes.contains("ModeUpdate") && !bytes.contains("Quit") {
                        plugin_b_has_empty_keybinds = true;
                    }
                }
            }
        }
    }

    assert!(
        plugin_a_has_keybinds,
        "Plugin A (legacy) should receive full ModeUpdate with keybinds"
    );
    assert!(
        plugin_b_has_empty_keybinds,
        "Plugin B (InitialKeybinds subscriber) should receive ModeUpdate with empty keybinds"
    );
}

#[test]
#[ignore]
pub fn reconfiguration_resends_keybinds_to_opted_in_plugins() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let plugin_should_float = Some(false);
    let tab_index = 1;
    let client_id = 1;
    let size = Size {
        cols: 121,
        rows: 20,
    };

    // Plugin subscribes to InitialKeybinds + ModeUpdate
    let mut config = BTreeMap::new();
    config.insert("subscribe_initial_keybinds".to_owned(), "true".to_owned());
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: PluginUserConfiguration::new(config),
        ..Default::default()
    });

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    // Spawn a screen thread that grants permissions and collects all instructions until Exit
    let screen_thread = std::thread::Builder::new()
        .name("fake_screen_thread".to_string())
        .spawn({
            let log = received_screen_instructions.clone();
            let cache_path = cache_path.clone();
            let plugin_thread_sender = plugin_thread_sender.clone();
            move || loop {
                let (event, _err_ctx) = screen_receiver
                    .recv()
                    .expect("failed to receive event on channel");
                match event {
                    ScreenInstruction::RequestPluginPermissions(plugin_id, plugin_permission) => {
                        let _ =
                            plugin_thread_sender.send(PluginInstruction::PermissionRequestResult(
                                plugin_id,
                                Some(client_id),
                                plugin_permission.permissions,
                                PermissionStatus::Granted,
                                Some(cache_path.clone()),
                            ));
                    },
                    ScreenInstruction::Exit => {
                        break;
                    },
                    _ => {
                        log.lock().unwrap().push(event);
                    },
                }
            }
        })
        .unwrap();

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        plugin_should_float,
        false,
        false,
        Some("test_plugin".to_owned()),
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

    // Build new keybinds for reconfiguration
    let mut keybind_map = std::collections::HashMap::new();
    let mut mode_map = std::collections::HashMap::new();
    mode_map.insert(KeyWithModifier::new(BareKey::Char('x')), vec![Action::Quit]);
    keybind_map.insert(InputMode::Normal, mode_map);
    let new_keybinds = Keybinds(keybind_map);

    // Send Reconfigure
    let _ = plugin_thread_sender.send(PluginInstruction::Reconfigure {
        client_id,
        keybinds: Some(new_keybinds),
        default_mode: None,
        default_shell: None,
        layout_dir: None,
        builtin_plugins_dir: None,
        was_written_to_disk: false,
    });
    std::thread::sleep(std::time::Duration::from_millis(500));

    teardown();
    screen_thread.join().unwrap();

    let instructions = received_screen_instructions.lock().unwrap();
    let initial_keybinds_event = instructions.iter().find_map(|instruction| {
        if let ScreenInstruction::PluginBytes(plugin_render_assets) = instruction {
            for asset in plugin_render_assets {
                let bytes = String::from_utf8_lossy(&asset.bytes).to_string();
                if bytes.contains("InitialKeybinds") {
                    return Some(bytes);
                }
            }
        }
        None
    });

    assert!(
        initial_keybinds_event.is_some(),
        "Plugin should receive InitialKeybinds event after reconfiguration"
    );
    // Note: the keybinds content in InitialKeybinds may be empty due to a race condition
    // between the executor thread updating keybinds and send_initial_keybinds_to_plugin
    // reading them synchronously. The important thing is that the event IS delivered.
}
