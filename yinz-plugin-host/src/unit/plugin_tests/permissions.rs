use super::*;
use yinz_test_support::Watched;
use yinz_types::internal_ids::TabId;

#[test]
#[serial(plugin_tests)]
pub fn send_configuration_to_plugins() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let plugin_should_float = Some(false);
    let plugin_title = Some("test_plugin".to_owned());
    let mut configuration = BTreeMap::new();
    configuration.insert(
        "fake_config_key_1".to_owned(),
        "fake_config_value_1".to_owned(),
    );
    configuration.insert(
        "fake_config_key_2".to_owned(),
        "fake_config_value_2".to_owned(),
    );
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: PluginUserConfiguration::new(configuration.clone()),
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
        ScreenInstruction::GoToTabName,
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
        Event::Key(KeyWithModifier::new(BareKey::Char('z')).with_ctrl_modifier()), // this triggers the enent in the fixture plugin
    )]));
    screen_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let received_configuration = received_screen_instructions
        .lock()
        .iter()
        .find_map(|instruction| {
            if let ScreenInstruction::GoToTabName(name, _, _, _, _) = instruction {
                Some(serde_json::from_str::<BTreeMap<String, String>>(name).unwrap())
            } else {
                None
            }
        })
        .expect("fixture echoes its configuration through GoToTabName");
    assert_eq!(received_configuration, configuration);
}

#[test]
#[serial(plugin_tests)]
pub fn request_plugin_permissions() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
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
    let screen_thread = log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::RequestPluginPermissions,
        screen_receiver,
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
    std::thread::sleep(std::time::Duration::from_millis(500));
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('1')).with_ctrl_modifier()), // this triggers the enent in the fixture plugin
    )]));
    screen_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let new_tab_event = received_screen_instructions
        .lock()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::RequestPluginPermissions(_, plugin_permission) = i {
                Some(plugin_permission.permissions.clone())
            } else {
                None
            }
        })
        .clone();
    assert_snapshot!(format!("{:#?}", new_tab_event));
}

#[test]
#[serial(plugin_tests)]
pub fn granted_permission_request_result() {
    let temp_folder = tempdir().unwrap();
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
        cache_path.clone(),
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
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('1')).with_ctrl_modifier()), // this triggers the enent in the fixture plugin
    )]));
    // The answer can be deferred while initialization finishes; sending it is not completion.
    received_screen_instructions.wait_until(
        "permission answer applied",
        FAKE_SCREEN_RECV_TIMEOUT,
        |events| {
            events
                .iter()
                .any(|event| matches!(event, ScreenInstruction::CancelPluginPermission(_)))
                .then_some(())
        },
    );
    teardown();
    screen_thread.join().unwrap();

    let permission_cache = PermissionCache::from_path_or_default(Some(cache_path));
    let mut permissions =
        permission_cache.get_permissions(PathBuf::from(&*PLUGIN_FIXTURE).display().to_string());
    let permissions = permissions.as_mut().map(|p| {
        let mut permissions = p.clone();
        permissions.sort_unstable();
        permissions
    });

    assert_snapshot!(format!("{:#?}", permissions));
}

#[test]
#[serial(plugin_tests)]
pub fn denied_permission_request_result() {
    let temp_folder = tempdir().unwrap();
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
        cache_path.clone(),
        plugin_thread_sender.clone(),
        client_id,
        PermissionStatus::Denied,
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
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('1')).with_ctrl_modifier()), // this triggers the enent in the fixture plugin
    )]));
    // The answer can be deferred while initialization finishes; sending it is not completion.
    received_screen_instructions.wait_until(
        "permission answer applied",
        FAKE_SCREEN_RECV_TIMEOUT,
        |events| {
            events
                .iter()
                .any(|event| matches!(event, ScreenInstruction::CancelPluginPermission(_)))
                .then_some(())
        },
    );
    teardown();
    screen_thread.join().unwrap();

    let permission_cache = PermissionCache::from_path_or_default(Some(cache_path));
    let permissions =
        permission_cache.get_permissions(PathBuf::from(&*PLUGIN_FIXTURE).display().to_string());

    assert_snapshot!(format!("{:#?}", permissions));
}

/// A denied command the guest waits on an answer to must not kill the guest: the SDK reads no
/// reply and answers `None`, and the plugin goes on handling keys.
#[test]
#[serial(plugin_tests)]
pub fn open_command_pane_without_run_commands_returns_none_and_plugin_survives() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);

    let received_screen_instructions = Watched::new(vec![]);
    let client_id = ClientId::new(1);
    let screen_thread = deny_permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        client_id,
    );

    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: Default::default(),
        ..Default::default()
    });
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        Some(false),
        false,
        false, // close_replaced_pane
        Some("test_plugin".to_owned()),
        run_plugin,
        Some(TabId::new(1)),
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
    wait_for_permissions_to_be_denied(&received_screen_instructions);

    // Ctrl+Shift+L: `open_command_pane`, which needs RunCommands.
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('l'))
                .with_ctrl_modifier()
                .with_shift_modifier(),
        ),
    )]));
    wait_for_plugin_to_handle(
        &received_screen_instructions,
        yinz_types::internal_ids::PluginId::new(0),
        &["open_command_pane: None", "open_command_pane: Some"],
    );
    // A second key proves the guest is still there to handle it.
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(
            KeyWithModifier::new(BareKey::Char('a'))
                .with_ctrl_modifier()
                .with_shift_modifier(),
        ),
    )]));
    wait_for_render_containing(&received_screen_instructions, "empty name");

    teardown();
    screen_thread.join().unwrap();

    let log = received_screen_instructions.lock();
    let frames: Vec<String> = rendered_assets(&log)
        .into_iter()
        .map(|(_, frame)| frame)
        .collect();
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("open_command_pane: None")),
        "the denied open answers None, got: {frames:?}"
    );
    assert!(
        !frames
            .iter()
            .any(|frame| frame.contains("open_command_pane: Some")),
        "a denied command must not open a pane, got: {frames:?}"
    );
    assert!(
        !log.iter().any(|instruction| {
            matches!(
                instruction,
                ScreenInstruction::UpdatePluginLoadingStage(_, indication) if indication.is_error()
            )
        }),
        "the guest must not trap on a denied command"
    );
}
