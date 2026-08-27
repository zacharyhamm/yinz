use super::*;
use zellij_test_support::Watched;

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
        configuration: PluginUserConfiguration::new(configuration),
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
        Event::Key(KeyWithModifier::new(BareKey::Char('z')).with_ctrl_modifier()), // this triggers the enent in the fixture plugin
    )]));
    screen_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    // here we make sure we received a rename_tab event with the title being the stringified
    // (Debug) configuration we sent to the fixture plugin to make sure it got there properly

    let go_to_tab_event = received_screen_instructions
        .lock()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::GoToTabName(..) = i {
                Some(i.clone())
            } else {
                None
            }
        })
        .clone();
    assert_snapshot!(format!("{:#?}", go_to_tab_event));
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
    let client_id = 1;
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
    let client_id = 1;
    let size = Size {
        cols: 121,
        rows: 20,
    };

    // here we create a fake screen thread that will send a PermissionStatus::Granted
    // message for every permission request it gets
    let screen_thread = std::thread::Builder::new()
        .name("fake_screen_thread".to_string())
        .spawn({
            let cache_path = cache_path.clone();
            let plugin_thread_sender = plugin_thread_sender.clone();
            move || loop {
                let (event, _err_ctx) = screen_receiver
                    .recv()
                    .expect("failed to receive event on channel");
                match event {
                    ScreenInstruction::RequestPluginPermissions(_, plugin_permission) => {
                        let _ =
                            plugin_thread_sender.send(PluginInstruction::PermissionRequestResult(
                                0,
                                Some(client_id),
                                plugin_permission.permissions,
                                PermissionStatus::Granted,
                                Some(cache_path.clone()),
                            ));
                        break;
                    },
                    ScreenInstruction::Exit => {
                        break;
                    },
                    _ => {},
                }
            }
        })
        .unwrap();

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
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('1')).with_ctrl_modifier()), // this triggers the enent in the fixture plugin
    )]));
    screen_thread.join().unwrap();
    teardown();

    let permission_cache = PermissionCache::from_path_or_default(Some(cache_path));
    let mut permissions = permission_cache
        .get_permissions(PathBuf::from(&*PLUGIN_FIXTURE).display().to_string())
        .clone();
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
    let client_id = 1;
    let size = Size {
        cols: 121,
        rows: 20,
    };

    // here we create a fake screen thread that will send a PermissionStatus::Granted
    // message for every permission request it gets
    let screen_thread = std::thread::Builder::new()
        .name("fake_screen_thread".to_string())
        .spawn({
            let cache_path = cache_path.clone();
            let plugin_thread_sender = plugin_thread_sender.clone();
            move || loop {
                let (event, _err_ctx) = screen_receiver
                    .recv()
                    .expect("failed to receive event on channel");
                match event {
                    ScreenInstruction::RequestPluginPermissions(_, plugin_permission) => {
                        let _ =
                            plugin_thread_sender.send(PluginInstruction::PermissionRequestResult(
                                0,
                                Some(client_id),
                                plugin_permission.permissions,
                                PermissionStatus::Denied,
                                Some(cache_path.clone()),
                            ));
                        break;
                    },
                    ScreenInstruction::Exit => {
                        break;
                    },
                    _ => {},
                }
            }
        })
        .unwrap();

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
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('1')).with_ctrl_modifier()), // this triggers the enent in the fixture plugin
    )]));
    screen_thread.join().unwrap();
    teardown();

    let permission_cache = PermissionCache::from_path_or_default(Some(cache_path));
    let permissions =
        permission_cache.get_permissions(PathBuf::from(&*PLUGIN_FIXTURE).display().to_string());

    assert_snapshot!(format!("{:#?}", permissions));
}
