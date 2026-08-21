use super::*;

#[test]
#[serial(plugin_tests)]
fn global_plugin_rejects_modal_configuration_from_resolved_alias() {
    let (plugin_thread_sender, screen_receiver, teardown) = create_plugin_thread(None, None);
    let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();

    plugin_thread_sender
        .send(PluginInstruction::Load(
            Some(false),
            false,
            false,
            None,
            RunPluginOrAlias::Alias(PluginAlias {
                name: "modal_fixture_plugin_for_tests".to_owned(),
                ..Default::default()
            }),
            Some(1),
            None,
            1,
            Size { cols: 80, rows: 24 },
            None,
            None,
            false,
            None,
            None,
            Some(crate::route::NotificationEnd::new(completion_tx)),
            Some(crate::pty::GlobalPaneSpawnContext::from(
                zellij_utils::data::GlobalPanePlacement::default(),
            )),
            None,
        ))
        .unwrap();

    let completion = completion_rx.blocking_recv().unwrap();
    assert_eq!(completion.exit_status, Some(1));
    assert_eq!(
        completion.error_message.as_deref(),
        Some("global plugins cannot be overlays or modal")
    );
    assert!(screen_receiver.try_recv().is_err());
    teardown();
}

#[test]
#[serial(plugin_tests)]
pub fn load_new_plugin_from_hd() {
    // here we load our fixture plugin into the plugin thread, and then send it an update message
    // expecting tha thte plugin will log the received event and render it later after the update
    // message (this is what the fixture plugin does)
    // we then listen on our mock screen receiver to make sure we got a PluginBytes instruction
    // that contains said render, and assert against it
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) = create_plugin_thread(None, None);
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
        ScreenInstruction::PluginBytes,
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
        Event::InputReceived,
    )])); // will be cached and sent to the plugin once it's loaded
    screen_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let plugin_bytes_event = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                for plugin_render_asset in plugin_render_assets {
                    let plugin_id = plugin_render_asset.plugin_id;
                    let client_id = plugin_render_asset.client_id;
                    let plugin_bytes = plugin_render_asset.bytes.clone();
                    let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                    if plugin_bytes.contains("InputReceived") {
                        return Some((plugin_id, client_id, plugin_bytes));
                    }
                }
            }
            None
        });
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[serial(plugin_tests)]
pub fn shared_plugin_has_one_instance_serving_all_clients() {
    // a plugin declared with `shared true` is loaded once for the client that loaded it, and is
    // not re-instantiated when a second client connects. an event directed at the second client
    // must reach that single instance, and its render must reach both clients.
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) = create_plugin_thread(None, None);
    let mut configuration = BTreeMap::new();
    configuration.insert("shared".to_owned(), "true".to_owned());
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: PluginUserConfiguration::new(configuration),
        ..Default::default()
    });
    let first_client_id = 1;
    let second_client_id = 2;
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        first_client_id
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(first_client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        Some(false),
        false,
        false, // close_replaced_pane
        Some("test_plugin".to_owned()),
        run_plugin,
        Some(1), // tab index
        None,
        first_client_id,
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
    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(second_client_id));
    std::thread::sleep(std::time::Duration::from_millis(500));
    // directed at the second client, which has no instance of its own
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(second_client_id),
        Event::InputReceived,
    )]));
    screen_thread.join().unwrap();
    teardown();
    let mut clients_that_rendered_the_event = vec![];
    for screen_instruction in received_screen_instructions.lock().unwrap().iter() {
        if let ScreenInstruction::PluginBytes(plugin_render_assets) = screen_instruction {
            for plugin_render_asset in plugin_render_assets {
                let plugin_bytes =
                    String::from_utf8_lossy(plugin_render_asset.bytes.as_slice()).to_string();
                if plugin_bytes.contains("InputReceived") {
                    clients_that_rendered_the_event.push(plugin_render_asset.client_id);
                }
            }
        }
    }
    clients_that_rendered_the_event.sort();
    clients_that_rendered_the_event.dedup();
    assert_eq!(
        clients_that_rendered_the_event,
        vec![first_client_id, second_client_id],
        "the single shared instance handled the second client's event and rendered for both clients"
    );
}

#[test]
#[serial(plugin_tests)]
pub fn load_new_plugin_with_plugin_alias() {
    // here we load our fixture plugin into the plugin thread, and then send it an update message
    // expecting tha thte plugin will log the received event and render it later after the update
    // message (this is what the fixture plugin does)
    // we then listen on our mock screen receiver to make sure we got a PluginBytes instruction
    // that contains said render, and assert against it
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) = create_plugin_thread(None, None);
    let plugin_should_float = Some(false);
    let plugin_title = Some("test_plugin".to_owned());
    let run_plugin = RunPluginOrAlias::Alias(PluginAlias {
        name: "fixture_plugin_for_tests".to_owned(),
        configuration: Default::default(),
        run_plugin: None,
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
        ScreenInstruction::PluginBytes,
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
        Event::InputReceived,
    )])); // will be cached and sent to the plugin once it's loaded
    screen_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let plugin_bytes_event = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                for plugin_render_asset in plugin_render_assets {
                    let plugin_id = plugin_render_asset.plugin_id;
                    let client_id = plugin_render_asset.client_id;
                    let plugin_bytes = plugin_render_asset.bytes.clone();
                    let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                    if plugin_bytes.contains("InputReceived") {
                        return Some((plugin_id, client_id, plugin_bytes));
                    }
                }
            }
            None
        });
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[serial(plugin_tests)]
pub fn plugin_workers() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let (plugin_thread_sender, screen_receiver, teardown) = create_plugin_thread(None, None);
    let plugin_should_float = Some(false);
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
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
        ScreenInstruction::PluginBytes,
        screen_receiver,
        3,
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
    // we send a SystemClipboardFailure to trigger the custom handler in the fixture plugin that
    // will send a message to the worker and in turn back to the plugin to be rendered, so we know
    // that this cycle is working
    std::thread::sleep(std::time::Duration::from_millis(500));
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::SystemClipboardFailure,
    )])); // will be cached and sent to the plugin once it's loaded
    screen_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let plugin_bytes_event = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                for plugin_render_asset in plugin_render_assets {
                    let plugin_id = plugin_render_asset.plugin_id;
                    let client_id = plugin_render_asset.client_id;
                    let plugin_bytes = plugin_render_asset.bytes.clone();
                    let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                    if plugin_bytes.contains("Payload from worker") {
                        return Some((plugin_id, client_id, plugin_bytes));
                    }
                }
            }
            None
        });
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[serial(plugin_tests)]
pub fn plugin_workers_persist_state() {
    let temp_folder = tempdir().unwrap(); // placed explicitly in the test scope because its
                                          // destructor removes the directory
    let (plugin_thread_sender, screen_receiver, teardown) = create_plugin_thread(None, None);
    let plugin_should_float = Some(false);
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
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
        ScreenInstruction::PluginBytes,
        screen_receiver,
        5,
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
    // we send a SystemClipboardFailure to trigger the custom handler in the fixture plugin that
    // will send a message to the worker and in turn back to the plugin to be rendered, so we know
    // that this cycle is working
    // we do this a second time so that the worker will log the first message on its own state and
    // then send us the "received 2 messages" indication we check for below, letting us know it
    // managed to persist its own state and act upon it
    //std::thread::sleep(std::time::Duration::from_millis(500));
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::SystemClipboardFailure,
    )]));
    std::thread::sleep(std::time::Duration::from_millis(500));
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::SystemClipboardFailure,
    )]));
    screen_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let plugin_bytes_event = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                for plugin_render_asset in plugin_render_assets {
                    let plugin_bytes = plugin_render_asset.bytes.clone();
                    let plugin_id = plugin_render_asset.plugin_id;
                    let client_id = plugin_render_asset.client_id;
                    let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                    if plugin_bytes.contains("received 2 messages") {
                        return Some((plugin_id, client_id, plugin_bytes));
                    }
                }
            }
            None
        });
    assert_snapshot!(format!("{:#?}", plugin_bytes_event));
}

#[test]
#[serial(plugin_tests)]
pub fn can_subscribe_to_hd_events() {
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
        ScreenInstruction::PluginBytes,
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
    // extra long time because we only start the fs watcher on plugin load
    std::thread::sleep(std::time::Duration::from_millis(5000));
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(PathBuf::from(temp_folder.path()).join("test1"))
        .unwrap();
    screen_thread.join().unwrap(); // this might take a while if the cache is cold
    teardown();
    let plugin_bytes_event = received_screen_instructions
        .lock()
        .unwrap()
        .iter()
        .find_map(|i| {
            if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
                for plugin_render_asset in plugin_render_assets {
                    let plugin_id = plugin_render_asset.plugin_id;
                    let client_id = plugin_render_asset.client_id;
                    let plugin_bytes = plugin_render_asset.bytes.clone();
                    let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                    if plugin_bytes.contains("FileSystemCreate") {
                        return Some((plugin_id, client_id, plugin_bytes));
                    }
                }
            }
            None
        });
    assert!(plugin_bytes_event.is_some());
}
