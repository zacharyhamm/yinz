use super::*;
use yinz_test_support::Watched;
use yinz_types::internal_ids::ContextMenuRequestId;
use yinz_types::internal_ids::TabId;

#[test]
#[serial(plugin_tests)]
pub fn generate_random_name_permission_denied() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);

    let received_screen_instructions = Watched::new(vec![]);
    let client_id = ClientId::new(1);

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

    // Either outcome the fixture can print. Listing the success too keeps a wrongly granted
    // command from timing out here instead of failing the assertion below.
    wait_for_plugin_to_handle(
        &received_screen_instructions,
        yinz_types::internal_ids::PluginId::new(0),
        &["Generated name", "empty name"],
    );

    // Everything the plugin thread will ever send has been sent once teardown has joined it, and
    // the observer returns when the last sender is gone.
    teardown();
    screen_thread.join().unwrap();

    // The denied command answers with an empty name; the guest does not trap on it.
    let frames: Vec<String> = rendered_assets(&received_screen_instructions.lock())
        .into_iter()
        .map(|(_, frame)| frame)
        .collect();
    assert!(
        frames.iter().any(|frame| frame.contains("empty name")),
        "the fixture reports the empty name, got: {frames:?}"
    );
    assert!(
        !received_screen_instructions.lock().iter().any(|instruction| {
            matches!(
                instruction,
                ScreenInstruction::UpdatePluginLoadingStage(_, indication) if indication.is_error()
            )
        }),
        "a denied command must not trap the guest"
    );

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
    let client_id = ClientId::new(1);

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
        yinz_types::internal_ids::PluginId::new(0),
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
        run_plugin.clone(),
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

    let frame =
        wait_for_render_containing(&received_screen_instructions, "PluginConfigurationChanged");
    teardown();
    screen_thread.join().unwrap();
    assert!(
        fixture_events(frame.as_bytes()).contains(&Event::PluginConfigurationChanged(new_config))
    );
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
    let client_id = ClientId::new(1);
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

    let (plugin_thread_sender, screen_receiver, teardown) = create_plugin_thread(
        Some(plugin_host_folder.clone()),
        Some(test_env_vars.clone()),
    );

    let client_id = ClientId::new(1);
    // Logs until disconnect rather than for a fixed number of renders: the fixture watches its
    // host folder, where the permission cache is written, so filesystem-event renders race with
    // the key's and the frame this test is after need not be among the first few.
    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        client_id,
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

    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('j'))
                .with_ctrl_modifier()
                .with_shift_modifier(),
        ),
    )]));

    wait_for_render_containing(&received_screen_instructions, "Got environment variables");
    teardown();
    screen_thread.join().unwrap();

    let frame = rendered_assets(&received_screen_instructions.lock())
        .into_iter()
        .map(|(_, frame)| frame)
        .find(|frame| frame.starts_with("Got environment variables: "))
        .unwrap();
    let variables: BTreeMap<String, String> = serde_json::from_str(
        frame
            .trim()
            .strip_prefix("Got environment variables: ")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(variables, test_env_vars);
}

/// The plugin thread holds the hook bindings and the configured context menu as state captured at
/// session start, so a reload that does not hand the new ones over leaves every hook point and
/// every right-click answering out of the config the session began with.
#[test]
#[serial(plugin_tests)]
pub fn a_reload_replaces_the_hooks_and_the_configured_context_menu() {
    use yinz_types::input::context_menu::ContextMenu;
    use yinz_types::input::hooks::Hooks;
    use {yinz_types::data::ContextMenuContext, yinz_types::data::ContextMenuItem};

    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let client_id = ClientId::new(1);

    let reloaded = ContextMenu::from_items(vec![ContextMenuItem::new(
        "Only this",
        vec![Action::CloseFocus],
    )]);
    let _ = plugin_thread_sender.send(PluginInstruction::Reconfigure {
        plugin_aliases: Default::default(),
        client_id,
        keybinds: None,
        default_mode: None,
        default_shell: None,
        layout_dir: None,
        user_plugin_dir: None,
        // No hook bound, so the answer comes straight out of the configuration below. That is also
        // what proves the hooks were replaced: the default binds the shipped hook to this point.
        hooks: Hooks::from_data(Default::default()),
        context_menu: reloaded.clone(),
        was_written_to_disk: false,
    });
    let _ = plugin_thread_sender.send(PluginInstruction::RequestContextMenu {
        context: Box::new(ContextMenuContext {
            request_id: ContextMenuRequestId::new(7),
            client_id,
            ..Default::default()
        }),
        client_id,
    });

    let answer = screen_receiver
        .recv_timeout(yinz_test_support::default_timeout())
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
            assert_eq!(request_id, ContextMenuRequestId::new(7));
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
/// `AddClient`, so none of `PluginBridge`'s own proactive per-client provisioning (on plugin load,
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
    use yinz_types::data::ContextMenuContext;
    use {yinz_types::input::hooks::HookPoint, yinz_types::input::hooks::Hooks};

    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);

    let hook_plugin = RunPluginOrAlias::RunPlugin(
        yinz_runtime::plugin_location::run_plugin_from_url(&format!("file:{}", *PLUGIN_FIXTURE))
            .unwrap(),
    );
    let _ = plugin_thread_sender.send(PluginInstruction::Reconfigure {
        plugin_aliases: Default::default(),
        client_id: crate::ClientId::new(1),
        keybinds: None,
        default_mode: None,
        default_shell: None,
        layout_dir: None,
        user_plugin_dir: None,
        hooks: Hooks::from_data(BTreeMap::from([(HookPoint::ContextMenu, hook_plugin)])),
        context_menu: Default::default(),
        was_written_to_disk: false,
    });
    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(crate::ClientId::new(1)));

    let _ = plugin_thread_sender.send(PluginInstruction::RequestContextMenu {
        context: Box::new(ContextMenuContext {
            request_id: ContextMenuRequestId::new(1),
            client_id: ClientId::new(1),
            ..Default::default()
        }),
        client_id: crate::ClientId::new(1),
    });

    let log = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &log,
        screen_receiver,
        temp_folder.path().join("permissions.kdl"),
        plugin_thread_sender.clone(),
        ClientId::new(1),
        PermissionStatus::Granted,
    );
    let query = || {
        let (reply, receive) = std::sync::mpsc::channel();
        plugin_thread_sender
            .send(PluginInstruction::PluginRequest {
                request: yinz_types::cli::PluginRequest {
                    operation: yinz_types::cli::PluginOperation::List,
                    target: None,
                    configuration: None,
                    fallback: None,
                    candidates: vec![],
                    caller: None,
                },
                client_id: ClientId::new(1),
                reply,
            })
            .unwrap();
        receive.recv_timeout(default_timeout()).unwrap().unwrap()
    };
    let initial = query();
    assert_eq!(initial.plugins.len(), 1);
    let id = initial.plugins[0].id;
    plugin_thread_sender
        .send(PluginInstruction::AddClient(ClientId::new(2)))
        .unwrap();
    plugin_thread_sender
        .send(PluginInstruction::RequestContextMenu {
            context: Box::new(ContextMenuContext {
                request_id: ContextMenuRequestId::new(2),
                client_id: ClientId::new(2),
                ..Default::default()
            }),
            client_id: ClientId::new(2),
        })
        .unwrap();
    let deadline = std::time::Instant::now() + default_timeout();
    loop {
        let status = query();
        assert_eq!(status.plugins.len(), 1, "hooks share one session identity");
        assert_eq!(status.plugins[0].id, id);
        if status.plugins[0].clients == [ClientId::new(1), ClientId::new(2)] {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{status:?}");
        std::thread::yield_now();
    }
    teardown();
    screen_thread.join().unwrap();
    assert_eq!(
        log.lock()
            .iter()
            .filter(|event| matches!(event, ScreenInstruction::AddPlugin(..)))
            .count(),
        1
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
    use yinz_types::data::ContextMenuContext;
    use {yinz_types::input::hooks::HookPoint, yinz_types::input::hooks::Hooks};

    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);

    let hook_plugin = RunPluginOrAlias::RunPlugin(
        yinz_runtime::plugin_location::run_plugin_from_url(&format!("file:{}", *PLUGIN_FIXTURE))
            .unwrap()
            .with_configuration(BTreeMap::from([("shared".to_owned(), "true".to_owned())])),
    );
    let _ = plugin_thread_sender.send(PluginInstruction::Reconfigure {
        plugin_aliases: Default::default(),
        client_id: crate::ClientId::new(1),
        keybinds: None,
        default_mode: None,
        default_shell: None,
        layout_dir: None,
        user_plugin_dir: None,
        hooks: Hooks::from_data(BTreeMap::from([(HookPoint::ContextMenu, hook_plugin)])),
        context_menu: Default::default(),
        was_written_to_disk: false,
    });
    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(crate::ClientId::new(1)));

    let _ = plugin_thread_sender.send(PluginInstruction::RequestContextMenu {
        context: Box::new(ContextMenuContext {
            request_id: ContextMenuRequestId::new(1),
            client_id: ClientId::new(1),
            ..Default::default()
        }),
        client_id: crate::ClientId::new(1),
    });

    let mut plugin_ids_loaded = std::collections::HashSet::new();
    let deadline = std::time::Instant::now() + yinz_test_support::default_timeout();
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
