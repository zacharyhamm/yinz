//! End-to-end tests for Lua plugins: a `.lua` file on disk, through the real plugin thread, to
//! the bytes the screen would draw.
//!
//! These use the same harness as the wasm plugin tests, which is the point - a Lua plugin is
//! loaded, subscribed, updated and rendered by exactly the same code.

use super::*;

/// Load the Lua fixture and return everything the screen was told to draw for it.
fn rendered_output(instructions: &[ScreenInstruction]) -> Vec<String> {
    instructions
        .iter()
        .filter_map(|instruction| match instruction {
            ScreenInstruction::PluginBytes(plugin_render_assets) => Some(plugin_render_assets),
            _ => None,
        })
        .flatten()
        .map(|asset| String::from_utf8_lossy(&asset.bytes).to_string())
        .filter(|rendered| !rendered.is_empty())
        .collect()
}

fn lua_run_plugin(configuration: BTreeMap<String, String>) -> RunPluginOrAlias {
    lua_run_plugin_at(&LUA_PLUGIN_FIXTURE, configuration)
}

fn lua_run_plugin_at(path: &str, configuration: BTreeMap<String, String>) -> RunPluginOrAlias {
    RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(path)),
        configuration: PluginUserConfiguration::new(configuration),
        ..Default::default()
    })
}

#[test]
#[serial(plugin_tests)]
pub fn lua_plugin_loads_and_renders() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let client_id = 1;
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let screen_thread = log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        Some(false),
        false,
        false,
        Some("lua_test_plugin".to_owned()),
        lua_run_plugin(BTreeMap::new()),
        Some(1),
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
    // The fixture subscribes to Key, so this reaches `update` and asks for a re-render.
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('a'))),
    )]));
    screen_thread.join().unwrap();
    teardown();

    let rendered = rendered_output(&received_screen_instructions.lock().unwrap());
    assert!(
        rendered.iter().any(|r| r.contains("hello from lua")),
        "expected the plugin's render output, got: {:?}",
        rendered
    );
    assert!(
        rendered.iter().any(|r| r.contains("presses 1")),
        "expected the key event to have reached `update`, got: {:?}",
        rendered
    );
    assert!(
        rendered.iter().any(|r| r.contains("121")),
        "expected the pane size to reach `render`, got: {:?}",
        rendered
    );
}

#[test]
#[serial(plugin_tests)]
pub fn lua_plugin_receives_its_configuration() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let client_id = 1;
    let size = Size { cols: 80, rows: 10 };
    let mut configuration = BTreeMap::new();
    // The fixture defaults this to "hello"; the prelude merges the user's value over the top.
    configuration.insert("greeting".to_owned(), "configured".to_owned());

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let screen_thread = log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        Some(false),
        false,
        false,
        Some("lua_test_plugin".to_owned()),
        lua_run_plugin(configuration),
        Some(1),
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
        Event::Key(KeyWithModifier::new(BareKey::Char('a'))),
    )]));
    screen_thread.join().unwrap();
    teardown();

    let rendered = rendered_output(&received_screen_instructions.lock().unwrap());
    assert!(
        rendered.iter().any(|r| r.contains("configured from lua")),
        "expected the merged configuration to reach `load`, got: {:?}",
        rendered
    );
}

#[test]
#[serial(plugin_tests)]
pub fn lua_plugin_is_not_sent_events_it_did_not_subscribe_to() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let client_id = 1;
    let size = Size { cols: 80, rows: 10 };
    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let screen_thread = log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        Some(false),
        false,
        false,
        Some("lua_test_plugin".to_owned()),
        lua_run_plugin(BTreeMap::new()),
        Some(1),
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
    // The fixture subscribes to Key and TabUpdate but not Timer, so this must not increment the
    // press counter or produce a render of its own.
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Timer(1.0),
    )]));
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('a'))),
    )]));
    screen_thread.join().unwrap();
    teardown();

    let rendered = rendered_output(&received_screen_instructions.lock().unwrap());
    assert!(
        rendered.iter().any(|r| r.contains("presses 1")),
        "the subscribed key event should have been delivered, got: {:?}",
        rendered
    );
    assert!(
        !rendered.iter().any(|r| r.contains("presses 2")),
        "the unsubscribed timer event should not have been delivered, got: {:?}",
        rendered
    );
}

#[test]
#[serial(plugin_tests)]
pub fn lua_plugin_host_command_reaches_the_pty_thread() {
    // The whole point of the design is that a Lua plugin's commands go through the same handlers
    // wasm plugins use, so this checks one all the way through: `zellij.open_terminal` in Lua
    // should arrive at the pty thread as a `SpawnTerminal`, permission-gated like any other.
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, pty_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_pty_receiver(Some(plugin_host_folder), None, None);
    let client_id = 1;
    let size = Size { cols: 80, rows: 10 };

    let received_pty_instructions = Arc::new(Mutex::new(vec![]));
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::SpawnTerminal,
        pty_receiver,
        1
    );
    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let _screen_thread = grant_permissions_and_log_actions_in_thread_naked_variant!(
        received_screen_instructions,
        ScreenInstruction::Exit,
        screen_receiver,
        1,
        &PermissionType::OpenTerminalsOrPlugins,
        cache_path,
        plugin_thread_sender,
        client_id
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        Some(false),
        false,
        false,
        Some("lua_test_plugin".to_owned()),
        lua_run_plugin_at(&LUA_PLUGIN_FIXTURE_WITH_PERMISSIONS, BTreeMap::new()),
        Some(1),
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
    // 't' is what the fixture maps to `zellij.open_terminal`.
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('t'))),
    )]));
    pty_thread.join().unwrap();
    teardown();

    let spawned = received_pty_instructions
        .lock()
        .unwrap()
        .iter()
        .any(|instruction| matches!(instruction, PtyInstruction::SpawnTerminal(..)));
    assert!(
        spawned,
        "expected `zellij.open_terminal` to reach the pty thread, got: {:?}",
        received_pty_instructions.lock().unwrap()
    );
}

#[test]
#[serial(plugin_tests)]
pub fn plugin_configuration_reaches_load_without_being_re_normalized() {
    // `PluginUserConfiguration::new` strips reserved keys such as `payload`, which is right when
    // parsing a layout and wrong afterwards: a configuration that already exists must be handed
    // to the plugin as-is. Rebuilding one on the way to `load` silently dropped these keys.
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let client_id = 1;
    let size = Size { cols: 80, rows: 10 };

    let mut configuration = PluginUserConfiguration::new(BTreeMap::new());
    // Inserted after construction, exactly as the reserved-word filter intends to allow.
    configuration.insert("payload", "carried");
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*LUA_PLUGIN_FIXTURE)),
        configuration,
        ..Default::default()
    });

    let received_screen_instructions = Arc::new(Mutex::new(vec![]));
    let screen_thread = log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        Some(false),
        false,
        false,
        Some("lua_test_plugin".to_owned()),
        run_plugin,
        Some(1),
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
        Event::Key(KeyWithModifier::new(BareKey::Char('a'))),
    )]));
    screen_thread.join().unwrap();
    teardown();

    let rendered = rendered_output(&received_screen_instructions.lock().unwrap());
    assert!(
        rendered.iter().any(|r| r.contains("payload carried")),
        "the reserved-word key should have survived to `load`, got: {:?}",
        rendered
    );
}
