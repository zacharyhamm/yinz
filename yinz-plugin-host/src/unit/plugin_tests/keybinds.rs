use super::*;
use yinz_test_support::Watched;
use yinz_types::internal_ids::TabId;

/// Wait for the fixture to finish loading and report its event history.
fn wait_for_plugin_to_load(
    log: &Watched<Vec<ScreenInstruction>>,
    plugin_id: yinz_types::internal_ids::PluginId,
) {
    log.wait_until_described(
        &format!("plugin {plugin_id} to load and render"),
        yinz_test_support::default_timeout(),
        |instructions| {
            instructions
                .iter()
                .any(|instruction| match instruction {
                    ScreenInstruction::PluginBytes(assets) => assets.iter().any(|asset| {
                        asset.plugin_id == plugin_id
                            && frame_of(&asset.bytes).contains("Received events:")
                    }),
                    _ => false,
                })
                .then_some(())
        },
        |instructions| {
            format!(
                "{} instruction(s) so far, from plugin id(s) {:?}. The fixture in use is {}",
                instructions.len(),
                plugin_render_ids(instructions),
                *PLUGIN_FIXTURE
            )
        },
    )
}

fn plugin_render_ids(
    instructions: &[ScreenInstruction],
) -> Vec<yinz_types::internal_ids::PluginId> {
    let mut ids: Vec<yinz_types::internal_ids::PluginId> = instructions
        .iter()
        .filter_map(|instruction| match instruction {
            ScreenInstruction::PluginBytes(assets) => Some(assets.iter().map(|a| a.plugin_id)),
            _ => None,
        })
        .flatten()
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

#[test]
#[serial(plugin_tests)]
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
    let client_id = ClientId::new(1);
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

    let received_screen_instructions = Watched::new(vec![]);
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
                    ScreenInstruction::RequestPluginPermissions(_plugin_id, plugin_permission) => {
                        let _ = plugin_thread_sender.send(PluginInstruction::PermissionAnswer {
                            request_id: plugin_permission.request_id,
                            status: PermissionStatus::Granted,
                            cache_path: Some(cache_path.clone()),
                        });
                    },
                    ScreenInstruction::Exit => {
                        break;
                    },
                    _ => {
                        log.lock().push(event);
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
    wait_for_plugin_to_load(
        &received_screen_instructions,
        yinz_types::internal_ids::PluginId::new(0),
    );

    // Load plugin B
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        plugin_should_float,
        false,
        false,
        Some("plugin_b".to_owned()),
        run_plugin_b,
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
    wait_for_plugin_to_load(
        &received_screen_instructions,
        yinz_types::internal_ids::PluginId::new(1),
    );

    // Set keybinds on all plugins via Reconfigure so that Plugin A (legacy)
    // will have non-empty keybinds in its internal state
    let mut keybind_map = std::collections::HashMap::new();
    let mut mode_map = std::collections::HashMap::new();
    mode_map.insert(KeyWithModifier::new(BareKey::Char('q')), vec![Action::Quit]);
    keybind_map.insert(InputMode::Normal, mode_map);
    let test_keybinds = Keybinds(keybind_map);

    let _ = plugin_thread_sender.send(PluginInstruction::Reconfigure {
        plugin_aliases: Default::default(),
        client_id,
        keybinds: Some(test_keybinds),
        default_mode: None,
        default_shell: None,
        layout_dir: None,
        user_plugin_dir: None,
        hooks: Default::default(),
        context_menu: Default::default(),
        was_written_to_disk: false,
    });
    // No wait here: both plugins are loaded and the plugin thread drains its channel in order, so
    // the Reconfigure below is applied before the ModeUpdate that follows it.

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

    // Wait for both ModeUpdate renders rather than sleeping and then hoping both arrived. The
    // assertions below are what the wait is for, so a timeout here reports the same failure the
    // asserts would - just with the evidence attached.
    let (plugin_a_has_keybinds, plugin_b_has_empty_keybinds) = received_screen_instructions
        .wait_until_described(
            "both plugins to receive their ModeUpdate",
            yinz_test_support::default_timeout(),
            |instructions| {
                let (a, b) = mode_update_payloads(instructions);
                (a && b).then_some((a, b))
            },
            |instructions| {
                let (a, b) = mode_update_payloads(instructions);
                format!(
                    "plugin A got keybinds: {a}, plugin B got a stripped ModeUpdate: {b} \
                     (after {} instruction(s) from plugin id(s) {:?})",
                    instructions.len(),
                    plugin_render_ids(instructions),
                )
            },
        );

    teardown();
    screen_thread.join().unwrap();

    assert!(
        plugin_a_has_keybinds,
        "Plugin A (legacy) should receive full ModeUpdate with keybinds"
    );
    assert!(
        plugin_b_has_empty_keybinds,
        "Plugin B (InitialKeybinds subscriber) should receive ModeUpdate with empty keybinds"
    );
}

/// Check each plugin's ModeUpdate independently of InitialKeybinds in the same frame.
fn mode_update_payloads(instructions: &[ScreenInstruction]) -> (bool, bool) {
    let mut full = false;
    let mut stripped = false;
    for instruction in instructions {
        if let ScreenInstruction::PluginBytes(assets) = instruction {
            for asset in assets {
                for event in fixture_events(&asset.bytes) {
                    if let Event::ModeUpdate(mode) = event {
                        if asset.plugin_id == yinz_types::internal_ids::PluginId::new(0) {
                            full |= mode.keybinds.iter().any(|(_, bindings)| {
                                bindings
                                    .iter()
                                    .any(|(_, actions)| actions.contains(&Action::Quit))
                            });
                        } else if asset.plugin_id == yinz_types::internal_ids::PluginId::new(1) {
                            stripped |= mode.keybinds.is_empty();
                        }
                    }
                }
            }
        }
    }
    (full, stripped)
}

#[test]
#[serial(plugin_tests)]
pub fn reconfiguration_resends_keybinds_to_opted_in_plugins() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let plugin_should_float = Some(false);
    let tab_index = 1;
    let client_id = ClientId::new(1);
    let size = Size {
        cols: 121,
        rows: 20,
    };

    // Plugin subscribes to InitialKeybinds + ModeUpdate
    let mut config = BTreeMap::new();
    config.insert("subscribe_initial_keybinds".to_owned(), "true".to_owned());
    config.insert("subscribe_agent_event".to_owned(), "true".to_owned());
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*PLUGIN_FIXTURE)),
        configuration: PluginUserConfiguration::new(config),
        ..Default::default()
    });

    let received_screen_instructions = Watched::new(vec![]);
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
                    ScreenInstruction::RequestPluginPermissions(_plugin_id, plugin_permission) => {
                        let _ = plugin_thread_sender.send(PluginInstruction::PermissionAnswer {
                            request_id: plugin_permission.request_id,
                            status: PermissionStatus::Granted,
                            cache_path: Some(cache_path.clone()),
                        });
                    },
                    ScreenInstruction::Exit => {
                        break;
                    },
                    _ => {
                        log.lock().push(event);
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
    wait_for_plugin_to_load(
        &received_screen_instructions,
        yinz_types::internal_ids::PluginId::new(0),
    );

    // Build new keybinds for reconfiguration
    let mut keybind_map = std::collections::HashMap::new();
    let mut mode_map = std::collections::HashMap::new();
    mode_map.insert(KeyWithModifier::new(BareKey::Char('x')), vec![Action::Quit]);
    keybind_map.insert(InputMode::Normal, mode_map);
    let new_keybinds = Keybinds(keybind_map);
    let expected_event = Event::InitialKeybinds(new_keybinds.to_keybinds_vec());

    // Send Reconfigure
    let _ = plugin_thread_sender.send(PluginInstruction::Reconfigure {
        plugin_aliases: Default::default(),
        client_id,
        keybinds: Some(new_keybinds),
        default_mode: None,
        default_shell: None,
        layout_dir: None,
        user_plugin_dir: None,
        hooks: Default::default(),
        context_menu: Default::default(),
        was_written_to_disk: false,
    });

    // The InitialKeybinds sent on reconfiguration must carry the *new* keybinds. Reconfigure
    // updates each plugin's env asynchronously on the executor, so a payload read back from the
    // env could still be the old (empty) one; the payload is therefore built from the reconfigure
    // request itself. Wait for a render that shows the new binding rather than for any
    // InitialKeybinds at all - the plugin was also sent one when it loaded.
    received_screen_instructions.wait_until_described(
        "the plugin to render the InitialKeybinds sent on reconfiguration",
        yinz_test_support::default_timeout(),
        |instructions| {
            renders_containing(instructions, "InitialKeybinds")
                .iter()
                .any(|render| fixture_events(render.as_bytes()).contains(&expected_event))
                .then_some(())
        },
        |instructions| {
            format!(
                "expected to find: {:?}\nin one of the renders containing InitialKeybinds: {:#?}",
                expected_event,
                renders_containing(instructions, "InitialKeybinds"),
            )
        },
    );

    // Exercise the real plugin subscription path with a full-size raw hook.
    let mut raw = serde_json::json!({"hook_event_name":"UserPromptSubmit", "prompt":""});
    let overhead = raw.to_string().len();
    raw["prompt"] = serde_json::Value::String(
        "x".repeat(yinz_types::agent::HOOK_BROADCAST_MAX_BYTES - overhead),
    );
    let raw = raw.to_string();
    let hook = serde_json::from_str(&raw).unwrap();
    let event = Event::AgentEvent(yinz_types::agent::AgentEvent::from_hook(
        "codex",
        yinz_types::internal_ids::TerminalId::new(7),
        &hook,
        raw,
    ));
    let expected = event.clone();
    plugin_thread_sender
        .send(PluginInstruction::Update(vec![(
            None,
            Some(client_id),
            event,
        )]))
        .unwrap();
    received_screen_instructions.wait_until_described(
        "a subscribed Scheme plugin to receive the complete 256 KiB hook",
        yinz_test_support::default_timeout(),
        |instructions| {
            renders_containing(instructions, "AgentEvent")
                .iter()
                .any(|render| fixture_events(render.as_bytes()).contains(&expected))
                .then_some(())
        },
        |instructions| format!("{} screen instructions", instructions.len()),
    );

    teardown();
    screen_thread.join().unwrap();
}

/// The rendered output of every `PluginBytes` asset in `instructions` that contains `needle`.
fn renders_containing(instructions: &[ScreenInstruction], needle: &str) -> Vec<String> {
    instructions
        .iter()
        .filter_map(|instruction| match instruction {
            ScreenInstruction::PluginBytes(assets) => Some(assets.iter()),
            _ => None,
        })
        .flatten()
        .map(|asset| String::from_utf8_lossy(&asset.bytes).to_string())
        .filter(|bytes| bytes.contains(needle))
        .collect()
}
