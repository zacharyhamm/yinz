use super::*;
use zellij_test_support::Watched;

#[test]
#[serial(plugin_tests)]
pub fn generate_random_name_permission_denied() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);

    let received_screen_instructions = Watched::new(vec![]);
    let client_id = ClientId(1);

    // The permission is denied, so the fixture must never get a name to print.
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

    // The key has to reach the plugin after the denial, or the test is about something else.
    wait_for_permissions_to_be_denied(&received_screen_instructions);

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

    // Either outcome the fixture can print, or the crash: a denied command gets no reply and the
    // fixture unwraps the reply, so denial in fact traps the guest. Listing the success too keeps
    // a wrongly granted command from timing out here instead of failing the assertion below.
    wait_for_plugin_to_handle(
        &received_screen_instructions,
        0,
        &["Generated name", "empty name"],
    );

    // Everything the plugin thread will ever send has been sent once teardown has joined it, and
    // the observer returns when the last sender is gone.
    teardown();
    screen_thread.join().unwrap();

    let plugin_bytes_with_name = received_screen_instructions.lock().iter().find_map(|i| {
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
        create_plugin_thread(Some(plugin_host_folder), None);

    let received_screen_instructions = Watched::new(vec![]);
    let client_id = ClientId(1);

    // The permission is denied, so the save must fail rather than succeed.
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

    // The key has to reach the plugin after the denial, or the test is about something else.
    wait_for_permissions_to_be_denied(&received_screen_instructions);

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

    // Either outcome the fixture can print, so that a wrongly granted save fails the assertion
    // below rather than timing out here.
    wait_for_plugin_to_handle(
        &received_screen_instructions,
        0,
        &["Save layout success", "Save layout error"],
    );

    // Everything the plugin thread will ever send has been sent once teardown has joined it, and
    // the observer returns when the last sender is gone.
    teardown();
    screen_thread.join().unwrap();

    let plugin_bytes_with_save = received_screen_instructions.lock().iter().find_map(|i| {
        if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
            for plugin_render_asset in plugin_render_assets {
                let plugin_bytes = plugin_render_asset.bytes.clone();
                let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                if plugin_bytes.contains("Save layout success") {
                    return Some(plugin_bytes);
                }
            }
        }
        None
    });

    // Should be None because permission was denied
    assert_snapshot!(format!("{:#?}", plugin_bytes_with_save));
    // The denied command gets no reply, which the fixture reports as a save error: the key was
    // handled, and handled as a failure.
    assert!(
        rendered_assets(&received_screen_instructions.lock())
            .iter()
            .any(|(_, frame)| frame.contains("Save layout error")),
        "the denied save should have been reported as an error by the fixture"
    );
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
    let client_id = ClientId(1);
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
    let client_id = ClientId(1);
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
        crate::ClientId(1)
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
    let client_id = ClientId(1);
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

/// The plugin thread holds the hook bindings and the configured context menu as state captured at
/// session start, so a reload that does not hand the new ones over leaves every hook point and
/// every right-click answering out of the config the session began with.
#[test]
#[serial(plugin_tests)]
pub fn a_reload_replaces_the_hooks_and_the_configured_context_menu() {
    use zellij_utils::data::{ContextMenuContext, ContextMenuItem};
    use zellij_utils::input::context_menu::ContextMenu;
    use zellij_utils::input::hooks::Hooks;

    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let client_id = ClientId(1);

    let reloaded = ContextMenu::from_items(vec![ContextMenuItem::new(
        "Only this",
        vec![Action::CloseFocus],
    )]);
    let _ = plugin_thread_sender.send(PluginInstruction::Reconfigure {
        client_id,
        keybinds: None,
        default_mode: None,
        default_shell: None,
        layout_dir: None,
        user_plugin_dir: None,
        builtin_plugins_dir: None,
        // No hook bound, so the answer comes straight out of the configuration below. That is also
        // what proves the hooks were replaced: the default binds the shipped hook to this point.
        hooks: Hooks::from_data(Default::default()),
        context_menu: reloaded.clone(),
        was_written_to_disk: false,
    });
    let _ = plugin_thread_sender.send(PluginInstruction::RequestContextMenu {
        context: Box::new(ContextMenuContext {
            request_id: 7,
            client_id: client_id.0,
            ..Default::default()
        }),
        client_id,
    });

    let answer = screen_receiver
        .recv_timeout(zellij_test_support::default_timeout())
        .map(|(instruction, _)| instruction)
        .expect("the plugin thread answers every request");
    teardown();

    match answer {
        ScreenInstruction::SetContextMenu {
            request_id,
            items,
            plugin_id,
            ..
        } => {
            assert_eq!(request_id, 7);
            assert_eq!(plugin_id, None, "nobody answered but the configuration");
            assert_eq!(
                items.iter().map(|item| &item.label).collect::<Vec<_>>(),
                vec!["Only this"],
                "the reloaded menu, not the one the session started with"
            );
        },
        other => panic!("expected a menu, got: {other:?}"),
    }
}

/// A non-shared hook has one instance per client. Client 2 here is never registered via
/// `AddClient`, so none of `WasmBridge`'s own proactive per-client provisioning (on plugin load,
/// on a client connecting) ever runs for it - the only way it can get answered is if
/// `request_context_menu` notices it has no owned instance and loads one itself. Before the fix,
/// `plugin_ids` being non-empty (client 1's instance) was enough to skip loading anything, the
/// event landed on client 1's instance tagged with client 2's id, and
/// `message_is_directed_at_plugin` dropped it as a mismatch - so client 2's request produced no
/// second load at all.
#[test]
#[serial(plugin_tests)]
pub fn a_second_clients_context_menu_request_loads_its_own_hook_instance() {
    use std::collections::BTreeMap;
    use zellij_utils::data::ContextMenuContext;
    use zellij_utils::input::hooks::{HookPoint, Hooks};

    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);

    let hook_plugin = RunPluginOrAlias::RunPlugin(
        RunPlugin::from_url(&format!("file:{}", *PLUGIN_FIXTURE)).unwrap(),
    );
    let _ = plugin_thread_sender.send(PluginInstruction::Reconfigure {
        client_id: crate::ClientId(1),
        keybinds: None,
        default_mode: None,
        default_shell: None,
        layout_dir: None,
        user_plugin_dir: None,
        builtin_plugins_dir: None,
        hooks: Hooks::from_data(BTreeMap::from([(HookPoint::ContextMenu, hook_plugin)])),
        context_menu: Default::default(),
        was_written_to_disk: false,
    });
    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(crate::ClientId(1)));

    let _ = plugin_thread_sender.send(PluginInstruction::RequestContextMenu {
        context: Box::new(ContextMenuContext {
            request_id: 1,
            client_id: 1,
            ..Default::default()
        }),
        client_id: crate::ClientId(1),
    });

    let mut plugin_ids_loaded = std::collections::HashSet::new();
    let collect_loads_for = |plugin_ids_loaded: &mut std::collections::HashSet<u32>,
                             want: usize| {
        let deadline = std::time::Instant::now() + zellij_test_support::default_timeout();
        while plugin_ids_loaded.len() < want && std::time::Instant::now() < deadline {
            let Ok((instruction, _)) =
                screen_receiver.recv_timeout(std::time::Duration::from_millis(200))
            else {
                continue;
            };
            if let ScreenInstruction::AddPlugin(_, _, _, _, _, _, plugin_id, ..) = instruction {
                plugin_ids_loaded.insert(plugin_id.0);
            }
        }
    };
    // Wait for client 1's own load to land before client 2 asks, so client 2's request cannot be
    // mistaken for the same in-flight load - it must see a plugin already registered.
    collect_loads_for(&mut plugin_ids_loaded, 1);
    assert_eq!(
        plugin_ids_loaded.len(),
        1,
        "client 1's own first-ever request should load exactly one instance"
    );

    let _ = plugin_thread_sender.send(PluginInstruction::RequestContextMenu {
        context: Box::new(ContextMenuContext {
            request_id: 2,
            client_id: 2,
            ..Default::default()
        }),
        client_id: crate::ClientId(2),
    });
    collect_loads_for(&mut plugin_ids_loaded, 2);
    teardown();

    assert_eq!(
        plugin_ids_loaded.len(),
        2,
        "client 2's request must load its own instance rather than being silently dropped \
         against client 1's, got: {plugin_ids_loaded:?}"
    );
}

/// A shared hook runs as one instance for the whole session, which every client can reach - but
/// only once it exists. Before the fix, `shared true` alone was taken as proof of an instance:
/// the load below was skipped, the dispatch that follows it had no instance to address, and every
/// right-click in the session was left to fall back on its timeout.
#[test]
#[serial(plugin_tests)]
pub fn a_shared_context_menu_hook_is_loaded_on_the_first_request() {
    use std::collections::BTreeMap;
    use zellij_utils::data::ContextMenuContext;
    use zellij_utils::input::hooks::{HookPoint, Hooks};

    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);

    let hook_plugin = RunPluginOrAlias::RunPlugin(
        RunPlugin::from_url(&format!("file:{}", *PLUGIN_FIXTURE))
            .unwrap()
            .with_configuration(BTreeMap::from([("shared".to_owned(), "true".to_owned())])),
    );
    let _ = plugin_thread_sender.send(PluginInstruction::Reconfigure {
        client_id: crate::ClientId(1),
        keybinds: None,
        default_mode: None,
        default_shell: None,
        layout_dir: None,
        user_plugin_dir: None,
        builtin_plugins_dir: None,
        hooks: Hooks::from_data(BTreeMap::from([(HookPoint::ContextMenu, hook_plugin)])),
        context_menu: Default::default(),
        was_written_to_disk: false,
    });
    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(crate::ClientId(1)));

    let _ = plugin_thread_sender.send(PluginInstruction::RequestContextMenu {
        context: Box::new(ContextMenuContext {
            request_id: 1,
            client_id: 1,
            ..Default::default()
        }),
        client_id: crate::ClientId(1),
    });

    let mut plugin_ids_loaded = std::collections::HashSet::new();
    let deadline = std::time::Instant::now() + zellij_test_support::default_timeout();
    while plugin_ids_loaded.is_empty() && std::time::Instant::now() < deadline {
        let Ok((instruction, _)) =
            screen_receiver.recv_timeout(std::time::Duration::from_millis(200))
        else {
            continue;
        };
        if let ScreenInstruction::AddPlugin(_, _, _, _, _, _, plugin_id, ..) = instruction {
            plugin_ids_loaded.insert(plugin_id);
        }
    }
    teardown();

    assert_eq!(
        plugin_ids_loaded.len(),
        1,
        "a shared hook nobody has loaded yet has to be loaded by the request that needs it, \
         got: {plugin_ids_loaded:?}"
    );
}
