use super::*;
use yinz_test_support::Watched;
use yinz_types::internal_ids::PluginId;
use yinz_types::internal_ids::TabId;

#[test]
#[serial(plugin_tests)]
fn new_tab_distinguishes_default_empty_and_explicit_floating_layouts() {
    use yinz_types::input::{command::RunCommand, layout::Run};

    let layout = yinz_config::kdl::layout::from_kdl(
        &format!(
            r#"layout {{ new_tab_template {{
                pane {{ plugin location="file:{}"; }}
                floating_panes {{
                    pane command="default-floating-command"
                    pane {{ plugin location="file:{}"; }}
                }}
            }}; }}"#,
            *PLUGIN_FIXTURE, *PLUGIN_FIXTURE
        ),
        None,
        None,
        None,
    )
    .unwrap();
    let default_floating = layout.new_tab().1;
    assert_eq!(default_floating.len(), 2);
    let mut explicit_floating = default_floating.clone();
    explicit_floating[0].run = Some(Run::Command(RunCommand {
        command: PathBuf::from("explicit-floating-command"),
        ..Default::default()
    }));
    let (sender, pty_receiver, _screen_receiver, teardown) =
        create_plugin_thread_with_pty_receiver_and_layout(None, None, None, Box::new(layout));
    let client_id = ClientId::new(1);
    sender
        .send(PluginInstruction::AddClient(client_id))
        .unwrap();
    for (floating, expected) in [
        (Some(vec![]), vec![]),
        (None, default_floating),
        (Some(explicit_floating.clone()), explicit_floating),
    ] {
        sender
            .send(PluginInstruction::NewTab(
                None,
                None,
                None,
                floating,
                TabId::new(1),
                None,
                false,
                false,
                client_id,
                None,
            ))
            .unwrap();
        let (instruction, _) = pty_receiver.recv_timeout(default_timeout()).unwrap();
        let PtyInstruction::NewTab(_, _, _, resolved, _, plugin_ids, ..) = instruction else {
            panic!("expected a resolved new tab, got {instruction:?}");
        };
        // The handler requests plugins before handing commands to PTY. This also
        // proves an empty override keeps tiled chrome without starting a floating plugin.
        assert_eq!(resolved, expected);
        assert_eq!(
            plugin_ids.values().map(Vec::len).sum::<usize>(),
            if expected.is_empty() { 1 } else { 2 }
        );
    }
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
        Event::InputReceived,
    )])); // will be cached and sent to the plugin once it's loaded
    wait_for_render_containing(&received_screen_instructions, "InputReceived");
    teardown();
    screen_thread.join().unwrap();
    let plugin_bytes_event = received_screen_instructions.lock().iter().find_map(|i| {
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
    let first_client_id = ClientId::new(1);
    let second_client_id = ClientId::new(2);
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
        first_client_id,
        PermissionStatus::Granted,
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(first_client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        Some(false),
        false,
        false, // close_replaced_pane
        Some("test_plugin".to_owned()),
        run_plugin,
        Some(TabId::new(1)), // tab index
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
    ));
    wait_for_permissions(&received_screen_instructions);
    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(second_client_id));
    // directed at the second client, which has no instance of its own
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(second_client_id),
        Event::InputReceived,
    )]));
    received_screen_instructions.wait_until_described(
        "both clients to render the event",
        default_timeout(),
        |log| {
            let frames = rendered_assets(log);
            [first_client_id, second_client_id]
                .iter()
                .all(|id| {
                    frames
                        .iter()
                        .any(|(client, frame)| client == id && frame.contains("InputReceived"))
                })
                .then_some(())
        },
        |log| describe_renders(log),
    );
    teardown();
    screen_thread.join().unwrap();
    let mut clients_that_rendered_the_event = vec![];
    for screen_instruction in received_screen_instructions.lock().iter() {
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
        Event::InputReceived,
    )])); // will be cached and sent to the plugin once it's loaded
    wait_for_render_containing(&received_screen_instructions, "InputReceived");
    teardown();
    screen_thread.join().unwrap();
    let plugin_bytes_event = received_screen_instructions.lock().iter().find_map(|i| {
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
    // we send a SystemClipboardFailure to trigger the custom handler in the fixture plugin that
    // will send a message to the worker and in turn back to the plugin to be rendered, so we know
    // that this cycle is working
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::SystemClipboardFailure,
    )])); // will be cached and sent to the plugin once it's loaded
    wait_for_render_containing(&received_screen_instructions, "Payload from worker");
    teardown();
    screen_thread.join().unwrap();
    let plugin_bytes_event = received_screen_instructions.lock().iter().find_map(|i| {
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
    // we send a SystemClipboardFailure to trigger the custom handler in the fixture plugin that
    // will send a message to the worker and in turn back to the plugin to be rendered, so we know
    // that this cycle is working
    // we do this a second time so that the worker will log the first message on its own state and
    // then send us the "received 2 messages" indication we check for below, letting us know it
    // managed to persist its own state and act upon it
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::SystemClipboardFailure,
    )]));
    wait_for_render_containing(&received_screen_instructions, "Payload from worker");
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::SystemClipboardFailure,
    )]));
    wait_for_render_containing(&received_screen_instructions, "received 2 messages");
    teardown();
    screen_thread.join().unwrap();
    let plugin_bytes_event = received_screen_instructions.lock().iter().find_map(|i| {
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
    // The permission cache belongs to the harness, outside the folder being observed.
    let cache_folder = tempdir().unwrap();
    let cache_path = cache_folder.path().join("permissions_test.kdl");
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
    // The watcher starts during load, before this post-load state request.
    wait_for_permissions(&received_screen_instructions);
    received_screen_instructions.wait_until("post-load state request", default_timeout(), |log| {
        log.iter()
            .any(|i| matches!(i, ScreenInstruction::RequestStateUpdateForPlugins))
            .then_some(())
    });
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::InputReceived,
    )]));
    wait_for_render_containing(&received_screen_instructions, "InputReceived");
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(PathBuf::from(temp_folder.path()).join("test1"))
        .unwrap();
    wait_for_render_containing(
        &received_screen_instructions,
        "\"FileSystemCreate\":[[\"/host/test1\",",
    );
    teardown();
    screen_thread.join().unwrap();
    let plugin_bytes_event = received_screen_instructions.lock().iter().find_map(|i| {
        if let ScreenInstruction::PluginBytes(plugin_render_assets) = i {
            for plugin_render_asset in plugin_render_assets {
                let plugin_id = plugin_render_asset.plugin_id;
                let client_id = plugin_render_asset.client_id;
                let plugin_bytes = plugin_render_asset.bytes.clone();
                let plugin_bytes = String::from_utf8_lossy(plugin_bytes.as_slice()).to_string();
                if plugin_bytes.contains("\"FileSystemCreate\":[[\"/host/test1\",") {
                    return Some((plugin_id, client_id, plugin_bytes));
                }
            }
        }
        None
    });
    assert!(plugin_bytes_event.is_some());
}

#[test]
#[serial(plugin_tests)]
pub fn a_background_plugin_loaded_before_its_client_attached_is_not_instantiated_twice() {
    // on startup, background plugins are loaded before the first client attaches, keyed under the
    // initiating client's id. when that client's `AddClient` arrives, the plugin already has an
    // instance for it: loading another would instantiate the wasm module a second time and
    // replace the first instance, dropping its state. the fixture renders every event it has
    // received, so state kept across the client's arrival is what this checks.
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) = create_plugin_thread(None, None);
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: Default::default(),
        ..Default::default()
    });
    let client_id = ClientId::new(1); // the harness's initiating client, not yet connected
    let plugin_id = PluginId::new(0); // the first plugin the thread loads
    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &received_screen_instructions,
        screen_receiver,
        cache_path,
        plugin_thread_sender.clone(),
        client_id,
        PermissionStatus::Granted,
    );
    // the index of the first render at or after `from` that contains `needle`, and the number of
    // times it contains it
    let render_containing = |log: &Vec<ScreenInstruction>, from: usize, needle: &str| {
        log.iter()
            .enumerate()
            .skip(from)
            .find_map(|(i, instruction)| {
                let ScreenInstruction::PluginBytes(render_assets) = instruction else {
                    return None;
                };
                let rendered = render_assets
                    .iter()
                    .filter(|asset| asset.plugin_id == plugin_id)
                    .map(|asset| String::from_utf8_lossy(&asset.bytes).to_string())
                    .collect::<String>();
                let occurrences = rendered.matches(needle).count();
                (occurrences > 0).then_some((i, occurrences))
            })
    };
    let describe = |log: &Vec<ScreenInstruction>| {
        log.iter()
            .map(|instruction| match instruction {
                ScreenInstruction::PluginBytes(render_assets) => render_assets
                    .iter()
                    .map(|asset| {
                        format!(
                            "PluginBytes(plugin {}, client {}: {:?})",
                            asset.plugin_id,
                            asset.client_id,
                            String::from_utf8_lossy(&asset.bytes)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
                other => {
                    let name: &'static str = other.into();
                    name.to_owned()
                },
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let _ = plugin_thread_sender.send(PluginInstruction::LoadBackgroundPlugin(
        run_plugin, client_id,
    ));
    // background plugins have no size until something gives them one, and the fixture renders
    // nothing at 0x0. the render also proves the load (permissions included) is complete: events
    // sent from here on reach the instance directly rather than being cached for it
    let _ = plugin_thread_sender.send(PluginInstruction::Resize(plugin_id, 121, 20));
    let (resized, _) = received_screen_instructions.wait_until_described(
        "the background plugin to load and render at its new size",
        yinz_test_support::default_timeout(),
        |log| render_containing(log, 0, "Rows: 20"),
        describe,
    );
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        Some(plugin_id),
        None,
        Event::InputReceived,
    )]));
    let (first_event_render, _) = received_screen_instructions.wait_until_described(
        "the background plugin to render its first event",
        yinz_test_support::default_timeout(),
        |log| render_containing(log, resized + 1, "InputReceived"),
        describe,
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        Some(plugin_id),
        None,
        Event::InputReceived,
    )]));
    let (_, events_rendered) = received_screen_instructions.wait_until_described(
        "the plugin to render an event after its client attached",
        yinz_test_support::default_timeout(),
        |log| render_containing(log, first_event_render + 1, "InputReceived"),
        describe,
    );
    teardown();
    screen_thread.join().unwrap();

    assert_eq!(
        events_rendered, 2,
        "the client's arrival must not reload a plugin it already owns: the instance that \
         received the first event should have received the second one too"
    );
}

#[test]
#[serial(plugin_tests)]
fn pane_tabs_lazy_children_subscribe_as_foreground_plugins() {
    use yinz_types::pane_tabs::{PaneTabDescriptor, PaneTabsId};

    let temp_folder = tempdir().unwrap();
    let (sender, screen_receiver, teardown) = create_plugin_thread(None, None);
    let client_id = ClientId::new(1);
    let log = Watched::new(vec![]);
    let screen_thread = permissions_and_log_until_disconnect(
        &log,
        screen_receiver,
        temp_folder.path().join("permissions.kdl"),
        sender.clone(),
        client_id,
        PermissionStatus::Granted,
    );
    sender
        .send(PluginInstruction::AddClient(client_id))
        .unwrap();
    sender
        .send(PluginInstruction::LoadPaneTab {
            id: PaneTabsId(7),
            index: 1,
            generation: 1,
            descriptor: PaneTabDescriptor {
                label: "Child".into(),
                location: format!("file:{}", *PLUGIN_FIXTURE),
                configuration: BTreeMap::from([("subscribe_mode_update".into(), "true".into())]),
                initial_cwd: Some(temp_folder.path().to_path_buf()),
            },
            client_id,
            tab_id: TabId::new(42),
            size: Size {
                cols: 121,
                rows: 20,
            },
        })
        .unwrap();
    wait_for_permissions(&log);
    sender
        .send(PluginInstruction::Update(vec![(
            None,
            Some(client_id),
            Event::InputReceived,
        )]))
        .unwrap();
    wait_for_render_containing(&log, "InputReceived");
    teardown();
    screen_thread.join().unwrap();
    let log = log.lock();
    assert!(log.iter().any(|instruction| matches!(
        instruction,
        ScreenInstruction::PaneTabLoaded {
            id: PaneTabsId(7),
            result: Ok(_),
            ..
        }
    )));
    // Background registration would make this child receive unrelated-tab input and a
    // second ModeUpdate in addition to the one sent by its owning tab.
    assert!(!log.iter().any(|instruction| matches!(
        instruction,
        ScreenInstruction::UpdateBackgroundPluginSubscriptions(..)
    )));
}
