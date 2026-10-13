use super::*;
use yinz_test_support::Watched;
use yinz_types::internal_ids::TabId;

#[test]
#[serial(plugin_tests)]
pub fn generate_random_name_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        crate::ClientId::new(1),
        PermissionStatus::Granted,
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
    let client_id = ClientId::new(1);
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

    wait_for_permissions(&received_screen_instructions);

    let checkpoint = received_screen_instructions.lock().len();
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

    let plugin_bytes_event = Some(received_screen_instructions.wait_until_described(
        "plugin_bytes_event",
        default_timeout(),
        |instructions| {
            instructions.iter().skip(checkpoint).find_map(|i| {
                if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                    for plugin_render_asset in plugin_render_assets {
                        let plugin_bytes = plugin_render_asset.bytes.clone();
                        let plugin_bytes =
                            String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                        if plugin_bytes.contains("Generated name") {
                            return Some(plugin_bytes);
                        }
                    }
                }
                None
            })
        },
        |instructions| describe_renders(instructions),
    ));
    teardown();
    screen_thread.join().unwrap();
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[serial(plugin_tests)]
pub fn dump_layout_success_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        crate::ClientId::new(1),
        PermissionStatus::Granted,
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
    let client_id = ClientId::new(1);
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

    wait_for_permissions(&received_screen_instructions);

    let checkpoint = received_screen_instructions.lock().len();
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

    let plugin_bytes_event = Some(received_screen_instructions.wait_until_described(
        "plugin_bytes_event",
        default_timeout(),
        |instructions| {
            instructions.iter().skip(checkpoint).find_map(|i| {
                if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                    for plugin_render_asset in plugin_render_assets {
                        let plugin_bytes = plugin_render_asset.bytes.clone();
                        let plugin_bytes =
                            String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                        if plugin_bytes.contains("Layout dump") {
                            return Some(plugin_bytes);
                        }
                    }
                }
                None
            })
        },
        |instructions| describe_renders(instructions),
    ));
    teardown();
    screen_thread.join().unwrap();
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

/// The data store round trip across the wasm boundary: declare an entity, write to it, read it
/// back, full-text search it, count it and watch it.
///
/// The engine itself is covered by unit tests in `yinz-config`; what this covers is the hop those
/// cannot reach — protobuf encode, the JSON-wrapped pipe, dispatch, and decode back in the guest.
#[test]
#[serial(plugin_tests)]
pub fn data_store_plugin_commands() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        crate::ClientId::new(1),
        PermissionStatus::Granted,
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
    let client_id = ClientId::new(1);
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

    wait_for_permissions(&received_screen_instructions);

    // Ctrl+Shift+K runs the whole data store round trip in the fixture plugin
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('k'))
                .with_ctrl_modifier()
                .with_shift_modifier(),
        ),
    )]));

    let plugin_bytes_event = Some(received_screen_instructions.wait_until_described(
        "plugin_bytes_event",
        default_timeout(),
        |instructions| {
            instructions.iter().find_map(|i| {
                if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                    for plugin_render_asset in plugin_render_assets {
                        let plugin_bytes = plugin_render_asset.bytes.clone();
                        let plugin_bytes =
                            String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                        if plugin_bytes.contains("Data store") {
                            return Some(plugin_bytes);
                        }
                    }
                }
                None
            })
        },
        |instructions| describe_renders(instructions),
    ));
    teardown();
    screen_thread.join().unwrap();
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[serial(plugin_tests)]
pub fn dump_layout_not_found_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        crate::ClientId::new(1),
        PermissionStatus::Granted,
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
    let client_id = ClientId::new(1);
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

    wait_for_permissions(&received_screen_instructions);

    let checkpoint = received_screen_instructions.lock().len();
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

    let plugin_bytes_event = Some(received_screen_instructions.wait_until_described(
        "plugin_bytes_event",
        default_timeout(),
        |instructions| {
            instructions.iter().skip(checkpoint).find_map(|i| {
                if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                    for plugin_render_asset in plugin_render_assets {
                        let plugin_bytes = plugin_render_asset.bytes.clone();
                        let plugin_bytes =
                            String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                        if plugin_bytes.contains("Layout dump error") {
                            return Some(plugin_bytes);
                        }
                    }
                }
                None
            })
        },
        |instructions| describe_renders(instructions),
    ));
    teardown();
    screen_thread.join().unwrap();
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[serial(plugin_tests)]
pub fn get_layout_dir_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        crate::ClientId::new(1),
        PermissionStatus::Granted,
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
    let client_id = ClientId::new(1);
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

    wait_for_permissions(&received_screen_instructions);

    let checkpoint = received_screen_instructions.lock().len();
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

    let plugin_bytes_event = Some(received_screen_instructions.wait_until_described(
        "plugin_bytes_event",
        default_timeout(),
        |instructions| {
            instructions.iter().skip(checkpoint).find_map(|i| {
                if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                    for plugin_render_asset in plugin_render_assets {
                        let plugin_bytes = plugin_render_asset.bytes.clone();
                        let plugin_bytes =
                            String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                        if plugin_bytes.contains("Got layout folder") {
                            return Some(plugin_bytes);
                        }
                    }
                }
                None
            })
        },
        |instructions| describe_renders(instructions),
    ));
    teardown();
    screen_thread.join().unwrap();
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[serial(plugin_tests)]
pub fn get_focused_pane_info_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        crate::ClientId::new(1),
        PermissionStatus::Granted,
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
    let client_id = ClientId::new(1);
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

    wait_for_permissions(&received_screen_instructions);

    let checkpoint = received_screen_instructions.lock().len();
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

    let focused_pane_info_query = Some(received_screen_instructions.wait_until_described(
        "focused_pane_info_query",
        default_timeout(),
        |instructions| {
            instructions.iter().skip(checkpoint).find_map(|i| {
                if let ScreenInstruction::GetFocusedPaneInfo { .. } = i {
                    Some(i.clone())
                } else {
                    None
                }
            })
        },
        |instructions| describe_renders(instructions),
    ));
    teardown();
    screen_thread.join().unwrap();
    assert_snapshot!(format!("{:#?}", focused_pane_info_query));
}

#[test]
#[serial(plugin_tests)]
pub fn dump_session_layout_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        crate::ClientId::new(1),
        PermissionStatus::Granted,
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
    let client_id = ClientId::new(1);
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

    wait_for_permissions(&received_screen_instructions);

    let checkpoint = received_screen_instructions.lock().len();
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

    let dump_layout_instruction = Some(received_screen_instructions.wait_until_described(
        "dump_layout_instruction",
        default_timeout(),
        |instructions| {
            instructions.iter().skip(checkpoint).find_map(|i| {
                if let ScreenInstruction::DumpLayoutToPlugin {
                    plugin_id,
                    tab_index,
                    ..
                } = i
                {
                    // The trace records variable timing; snapshot the requested plugin and tab.
                    return Some((*plugin_id, *tab_index));
                }
                None
            })
        },
        |instructions| describe_renders(instructions),
    ));
    teardown();
    screen_thread.join().unwrap();
    assert_snapshot!(format!("{:#?}", dump_layout_instruction));
}

#[test]
#[serial(plugin_tests)]
pub fn dump_session_layout_for_tab_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        crate::ClientId::new(1),
        PermissionStatus::Granted,
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
    let client_id = ClientId::new(1);
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

    wait_for_permissions(&received_screen_instructions);

    let checkpoint = received_screen_instructions.lock().len();
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

    let dump_layout_instruction = Some(received_screen_instructions.wait_until_described(
        "dump_layout_instruction",
        default_timeout(),
        |instructions| {
            instructions.iter().skip(checkpoint).find_map(|i| {
                if let ScreenInstruction::DumpLayoutToPlugin {
                    plugin_id,
                    tab_index,
                    ..
                } = i
                {
                    // The trace records variable timing; snapshot the requested plugin and tab.
                    return Some((*plugin_id, *tab_index));
                }
                None
            })
        },
        |instructions| describe_renders(instructions),
    ));
    teardown();
    screen_thread.join().unwrap();
    assert_snapshot!(format!("{:#?}", dump_layout_instruction));
}

#[test]
#[serial(plugin_tests)]
pub fn parse_layout_success_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        crate::ClientId::new(1),
        PermissionStatus::Granted,
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
    let client_id = ClientId::new(1);
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

    wait_for_permissions(&received_screen_instructions);

    let checkpoint = received_screen_instructions.lock().len();
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

    let plugin_bytes_event = Some(received_screen_instructions.wait_until_described(
        "plugin_bytes_event",
        default_timeout(),
        |instructions| {
            instructions.iter().skip(checkpoint).find_map(|i| {
                if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                    for plugin_render_asset in plugin_render_assets {
                        let plugin_bytes = plugin_render_asset.bytes.clone();
                        let plugin_bytes =
                            String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                        if plugin_bytes.contains("Parse success") {
                            return Some(plugin_bytes);
                        }
                    }
                }
                None
            })
        },
        |instructions| describe_renders(instructions),
    ));
    teardown();
    screen_thread.join().unwrap();
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[serial(plugin_tests)]
pub fn parse_layout_error_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        crate::ClientId::new(1),
        PermissionStatus::Granted,
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
    let client_id = ClientId::new(1);
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

    wait_for_permissions(&received_screen_instructions);

    let checkpoint = received_screen_instructions.lock().len();
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

    let plugin_bytes_event = Some(received_screen_instructions.wait_until_described(
        "plugin_bytes_event",
        default_timeout(),
        |instructions| {
            instructions.iter().skip(checkpoint).find_map(|i| {
                if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                    for plugin_render_asset in plugin_render_assets {
                        let plugin_bytes = plugin_render_asset.bytes.clone();
                        let plugin_bytes =
                            String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                        if plugin_bytes.contains("Parse error:") {
                            return Some(plugin_bytes);
                        }
                    }
                }
                None
            })
        },
        |instructions| describe_renders(instructions),
    ));
    teardown();
    screen_thread.join().unwrap();
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[serial(plugin_tests)]
pub fn save_layout_success_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        crate::ClientId::new(1),
        PermissionStatus::Granted,
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
    let client_id = ClientId::new(1);
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

    wait_for_permissions(&received_screen_instructions);

    let checkpoint = received_screen_instructions.lock().len();
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

    let plugin_bytes_event = Some(received_screen_instructions.wait_until_described(
        "plugin_bytes_event",
        default_timeout(),
        |instructions| {
            instructions.iter().skip(checkpoint).find_map(|i| {
                if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                    for plugin_render_asset in plugin_render_assets {
                        let plugin_bytes = plugin_render_asset.bytes.clone();
                        let plugin_bytes =
                            String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                        if plugin_bytes.contains("Save layout") {
                            return Some(plugin_bytes);
                        }
                    }
                }
                None
            })
        },
        |instructions| describe_renders(instructions),
    ));
    teardown();
    screen_thread.join().unwrap();
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[serial(plugin_tests)]
pub fn save_layout_already_exists_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        crate::ClientId::new(1),
        PermissionStatus::Granted,
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
    let client_id = ClientId::new(1);
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

    wait_for_permissions(&received_screen_instructions);

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

    wait_for_render_containing(&received_screen_instructions, "Save layout success");

    let checkpoint = received_screen_instructions.lock().len();
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

    let plugin_bytes_event = Some(received_screen_instructions.wait_until_described(
        "plugin_bytes_event",
        default_timeout(),
        |instructions| {
            instructions.iter().skip(checkpoint).rev().find_map(|i| {
                if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                    for plugin_render_asset in plugin_render_assets {
                        let plugin_bytes = plugin_render_asset.bytes.clone();
                        let plugin_bytes =
                            String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                        if plugin_bytes.contains("error") {
                            return Some(plugin_bytes);
                        }
                    }
                }
                None
            })
        },
        |instructions| describe_renders(instructions),
    ));
    teardown();
    screen_thread.join().unwrap();
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[serial(plugin_tests)]
pub fn save_layout_with_overwrite_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        crate::ClientId::new(1),
        PermissionStatus::Granted,
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
    let client_id = ClientId::new(1);
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

    wait_for_permissions(&received_screen_instructions);

    let checkpoint = received_screen_instructions.lock().len();
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

    let plugin_bytes_event = Some(received_screen_instructions.wait_until_described(
        "plugin_bytes_event",
        default_timeout(),
        |instructions| {
            instructions.iter().skip(checkpoint).find_map(|i| {
                if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                    for plugin_render_asset in plugin_render_assets {
                        let plugin_bytes = plugin_render_asset.bytes.clone();
                        let plugin_bytes =
                            String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                        if plugin_bytes.contains("Save layout") {
                            return Some(plugin_bytes);
                        }
                    }
                }
                None
            })
        },
        |instructions| describe_renders(instructions),
    ));
    teardown();
    screen_thread.join().unwrap();
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[serial(plugin_tests)]
pub fn save_layout_invalid_kdl_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        crate::ClientId::new(1),
        PermissionStatus::Granted,
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
    let client_id = ClientId::new(1);
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

    wait_for_permissions(&received_screen_instructions);

    let checkpoint = received_screen_instructions.lock().len();
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

    let plugin_bytes_event = Some(received_screen_instructions.wait_until_described(
        "plugin_bytes_event",
        default_timeout(),
        |instructions| {
            instructions.iter().skip(checkpoint).find_map(|i| {
                if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                    for plugin_render_asset in plugin_render_assets {
                        let plugin_bytes = plugin_render_asset.bytes.clone();
                        let plugin_bytes =
                            String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                        if plugin_bytes.contains("invalid layout error") {
                            return Some(plugin_bytes);
                        }
                    }
                }
                None
            })
        },
        |instructions| describe_renders(instructions),
    ));
    teardown();
    screen_thread.join().unwrap();
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[serial(plugin_tests)]
pub fn rename_layout_success_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        crate::ClientId::new(1),
        PermissionStatus::Granted,
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
    let client_id = ClientId::new(1);
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

    wait_for_permissions(&received_screen_instructions);

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

    wait_for_render_containing(&received_screen_instructions, "Save layout success");

    let checkpoint = received_screen_instructions.lock().len();
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

    let plugin_bytes_event = Some(received_screen_instructions.wait_until_described(
        "plugin_bytes_event",
        default_timeout(),
        |instructions| {
            instructions.iter().skip(checkpoint).rev().find_map(|i| {
                if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                    for plugin_render_asset in plugin_render_assets {
                        let plugin_bytes = plugin_render_asset.bytes.clone();
                        let plugin_bytes =
                            String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                        if plugin_bytes.contains("Rename layout") {
                            return Some(plugin_bytes);
                        }
                    }
                }
                None
            })
        },
        |instructions| describe_renders(instructions),
    ));
    teardown();
    screen_thread.join().unwrap();
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[serial(plugin_tests)]
pub fn rename_layout_not_found_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        crate::ClientId::new(1),
        PermissionStatus::Granted,
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
    let client_id = ClientId::new(1);
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

    wait_for_permissions(&received_screen_instructions);

    let checkpoint = received_screen_instructions.lock().len();
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

    let plugin_bytes_event = Some(received_screen_instructions.wait_until_described(
        "plugin_bytes_event",
        default_timeout(),
        |instructions| {
            instructions.iter().skip(checkpoint).find_map(|i| {
                if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                    for plugin_render_asset in plugin_render_assets {
                        let plugin_bytes = plugin_render_asset.bytes.clone();
                        let plugin_bytes =
                            String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                        if plugin_bytes.contains("Rename") {
                            return Some(plugin_bytes);
                        }
                    }
                }
                None
            })
        },
        |instructions| describe_renders(instructions),
    ));
    teardown();
    screen_thread.join().unwrap();
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[serial(plugin_tests)]
pub fn delete_layout_success_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let layout_folder = tempdir().unwrap();
    let (plugin_thread_sender, _pty_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_pty_receiver(
            Some(plugin_host_folder.clone()),
            Some(layout_folder.path().to_owned()),
            None,
        );

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        crate::ClientId::new(1),
        PermissionStatus::Granted,
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
    let client_id = ClientId::new(1);
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

    wait_for_permissions(&received_screen_instructions);

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

    wait_for_render_containing(&received_screen_instructions, "Save layout success");
    assert!(layout_folder.path().join("test_layout.kdl").is_file());

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

    wait_for_render_containing(&received_screen_instructions, "Rename layout success");
    assert!(!layout_folder.path().join("test_layout.kdl").exists());
    assert!(layout_folder.path().join("renamed_layout.kdl").is_file());

    let checkpoint = received_screen_instructions.lock().len();
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

    let plugin_bytes_event = Some(received_screen_instructions.wait_until_described(
        "plugin_bytes_event",
        default_timeout(),
        |instructions| {
            instructions.iter().skip(checkpoint).rev().find_map(|i| {
                if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                    for plugin_render_asset in plugin_render_assets {
                        let plugin_bytes = plugin_render_asset.bytes.clone();
                        let plugin_bytes =
                            String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                        if plugin_bytes.contains("Delete layout success") {
                            return Some(plugin_bytes);
                        }
                    }
                }
                None
            })
        },
        |instructions| describe_renders(instructions),
    ));
    assert!(!layout_folder.path().join("renamed_layout.kdl").exists());
    teardown();
    screen_thread.join().unwrap();
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[serial(plugin_tests)]
pub fn delete_layout_not_found_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        crate::ClientId::new(1),
        PermissionStatus::Granted,
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
    let client_id = ClientId::new(1);
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

    wait_for_permissions(&received_screen_instructions);

    let checkpoint = received_screen_instructions.lock().len();
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

    let plugin_bytes_event = Some(received_screen_instructions.wait_until_described(
        "plugin_bytes_event",
        default_timeout(),
        |instructions| {
            instructions.iter().skip(checkpoint).find_map(|i| {
                if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                    for plugin_render_asset in plugin_render_assets {
                        let plugin_bytes = plugin_render_asset.bytes.clone();
                        let plugin_bytes =
                            String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                        if plugin_bytes.contains("Delete") {
                            return Some(plugin_bytes);
                        }
                    }
                }
                None
            })
        },
        |instructions| describe_renders(instructions),
    ));
    teardown();
    screen_thread.join().unwrap();
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[serial(plugin_tests)]
pub fn save_layout_path_traversal_blocked_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        crate::ClientId::new(1),
        PermissionStatus::Granted,
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
    let client_id = ClientId::new(1);
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

    wait_for_permissions(&received_screen_instructions);

    let checkpoint = received_screen_instructions.lock().len();
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

    let plugin_bytes_event = Some(received_screen_instructions.wait_until_described(
        "plugin_bytes_event",
        default_timeout(),
        |instructions| {
            instructions.iter().skip(checkpoint).find_map(|i| {
                if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                    for plugin_render_asset in plugin_render_assets {
                        let plugin_bytes = plugin_render_asset.bytes.clone();
                        let plugin_bytes =
                            String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                        if plugin_bytes.contains("Path traversal") {
                            return Some(plugin_bytes);
                        }
                    }
                }
                None
            })
        },
        |instructions| describe_renders(instructions),
    ));
    teardown();
    screen_thread.join().unwrap();
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[serial(plugin_tests)]
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

    let received_screen_instructions = Watched::new(vec![]);
    let received_pty_instructions = Watched::new(vec![]);
    let pty_thread = log_until_disconnect(&received_pty_instructions, pty_receiver);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        crate::ClientId::new(1),
        PermissionStatus::Granted,
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
    let client_id = ClientId::new(1);
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

    wait_for_permissions(&received_screen_instructions);

    let checkpoint = received_pty_instructions.lock().len();
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

    // this is the editor pane for the layout being spawned
    let spawn_terminal_instruction = Some(received_pty_instructions.wait_until_described(
        "spawn_terminal_instruction",
        default_timeout(),
        |instructions| {
            instructions.iter().skip(checkpoint).find_map(|i| {
                if let PtyInstruction::SpawnInPlaceTerminal(..) = i {
                    Some(i.clone())
                } else {
                    None
                }
            })
        },
        |instructions| format!("{instructions:?}"),
    ));

    // The fake PTY must finish placement before teardown joins the waiting plugin.
    let mut completion = received_pty_instructions
        .lock()
        .iter_mut()
        .find_map(|instruction| match instruction {
            PtyInstruction::SpawnInPlaceTerminal(_, _, _, _, completion) => completion.take(),
            _ => None,
        })
        .unwrap();
    completion.set_affected_pane_id(PaneId::Terminal(crate::TerminalId::new(1)));
    drop(completion);

    teardown();
    screen_thread.join().unwrap();
    pty_thread.join().unwrap();
    assert_snapshot!(format!("{:#?}", spawn_terminal_instruction).replace(
        &format!("{}", temp_layout_folder.path().display()),
        "LAYOUT_FOLDER_PATH"
    ))
}

#[test]
#[serial(plugin_tests)]
pub fn override_layout_plugin_command() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder.clone()), None);

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        crate::ClientId::new(1),
        PermissionStatus::Granted,
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
    let client_id = ClientId::new(1);
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

    wait_for_permissions(&received_screen_instructions);

    let checkpoint = received_screen_instructions.lock().len();
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

    let override_layout_instruction = Some(received_screen_instructions.wait_until_described(
        "override_layout_instruction",
        default_timeout(),
        |instructions| {
            instructions.iter().skip(checkpoint).find_map(|i| {
                if let ScreenInstruction::OverrideLayout(..) = i {
                    Some(i.clone())
                } else {
                    None
                }
            })
        },
        |instructions| describe_renders(instructions),
    ));
    teardown();
    screen_thread.join().unwrap();
    assert_snapshot!(format!("{:#?}", override_layout_instruction));
}
