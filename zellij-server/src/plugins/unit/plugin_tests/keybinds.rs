use super::*;
use zellij_test_support::Watched;

/// Blocks until `plugin_id` has rendered at least once, which is the observable signal that the
/// plugin finished loading.
///
/// Loading a plugin compiles and instantiates a real wasm module, so how long it takes depends
/// entirely on the machine and its current load. Sleeping a fixed 500ms for it is a guess that is
/// wasteful when it is too long and flaky when it is too short.
fn wait_for_plugin_to_load(
    log: &Watched<Vec<ScreenInstruction>>,
    plugin_id: zellij_types::internal_ids::PluginId,
) {
    log.wait_until_described(
        &format!("plugin {plugin_id} to load and render"),
        zellij_test_support::default_timeout(),
        |instructions| {
            instructions
                .iter()
                .any(|instruction| match instruction {
                    ScreenInstruction::PluginBytes(assets) => {
                        assets.iter().any(|asset| asset.plugin_id == plugin_id)
                    },
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
) -> Vec<zellij_types::internal_ids::PluginId> {
    let mut ids: Vec<zellij_types::internal_ids::PluginId> = instructions
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
    wait_for_plugin_to_load(
        &received_screen_instructions,
        zellij_types::internal_ids::PluginId::new(0),
    );

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
    wait_for_plugin_to_load(
        &received_screen_instructions,
        zellij_types::internal_ids::PluginId::new(1),
    );

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
        user_plugin_dir: None,
        builtin_plugins_dir: None,
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
            zellij_test_support::default_timeout(),
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

/// `(plugin A received keybinds, plugin B received a ModeUpdate with them stripped)`.
///
/// Plugin A subscribes to `ModeUpdate` alone and so should still be sent the full keybinds, which
/// show up as `Quit` under the `ModeUpdate`'s `keybinds:` field. Plugin B also subscribes to
/// `InitialKeybinds` and so should get a `ModeUpdate` with the keybinds stripped out.
///
/// The check looks at the `keybinds:` field of the rendered `ModeUpdate` itself rather than for
/// `Quit` anywhere in the render: plugin B's render also lists the `InitialKeybinds` event it was
/// sent on reconfiguration, which carries the same `Quit` binding.
fn mode_update_payloads(instructions: &[ScreenInstruction]) -> (bool, bool) {
    let mut plugin_a_has_keybinds = false;
    let mut plugin_b_has_empty_keybinds = false;
    for instruction in instructions {
        if let ScreenInstruction::PluginBytes(plugin_render_assets) = instruction {
            for asset in plugin_render_assets {
                let bytes = String::from_utf8_lossy(&asset.bytes).to_string();
                let Some(mode_update_start) = bytes.find("ModeUpdate(") else {
                    continue;
                };
                let mode_update = &bytes[mode_update_start..];
                let Some(keybinds_start) = mode_update.find("keybinds: ") else {
                    continue;
                };
                let keybinds = &mode_update[keybinds_start..];
                if keybinds.starts_with("keybinds: [],") {
                    plugin_b_has_empty_keybinds = true;
                } else if keybinds.starts_with("keybinds: [(Normal, ") && keybinds.contains("Quit")
                {
                    plugin_a_has_keybinds = true;
                }
            }
        }
    }
    (plugin_a_has_keybinds, plugin_b_has_empty_keybinds)
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
    wait_for_plugin_to_load(
        &received_screen_instructions,
        zellij_types::internal_ids::PluginId::new(0),
    );

    // Build new keybinds for reconfiguration
    let mut keybind_map = std::collections::HashMap::new();
    let mut mode_map = std::collections::HashMap::new();
    mode_map.insert(KeyWithModifier::new(BareKey::Char('x')), vec![Action::Quit]);
    keybind_map.insert(InputMode::Normal, mode_map);
    let new_keybinds = Keybinds(keybind_map);
    // The fixture renders every event it received with `{:?}`, so the InitialKeybinds it is sent
    // on reconfiguration shows up in its output verbatim.
    let expected_event = format!(
        "{:?}",
        Event::InitialKeybinds(new_keybinds.to_keybinds_vec())
    );

    // Send Reconfigure
    let _ = plugin_thread_sender.send(PluginInstruction::Reconfigure {
        client_id,
        keybinds: Some(new_keybinds),
        default_mode: None,
        default_shell: None,
        layout_dir: None,
        user_plugin_dir: None,
        builtin_plugins_dir: None,
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
        zellij_test_support::default_timeout(),
        |instructions| {
            renders_containing(instructions, "InitialKeybinds")
                .iter()
                .any(|render| render.contains(&expected_event))
                .then_some(())
        },
        |instructions| {
            format!(
                "expected to find: {}\nin one of the renders containing InitialKeybinds: {:#?}",
                expected_event,
                renders_containing(instructions, "InitialKeybinds"),
            )
        },
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
