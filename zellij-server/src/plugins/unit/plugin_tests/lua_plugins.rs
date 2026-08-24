//! End-to-end tests for Lua plugins: a `.lua` file on disk, through the real plugin thread, to
//! the bytes the screen would draw.
//!
//! These use the same harness as the wasm plugin tests, which is the point - a Lua plugin is
//! loaded, subscribed, updated and rendered by exactly the same code.

use super::*;
use zellij_test_support::Watched;
use zellij_utils::input::command::TerminalAction;

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
    let received_screen_instructions = Watched::new(vec![]);
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

    let rendered = rendered_output(&received_screen_instructions.lock());
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

    let received_screen_instructions = Watched::new(vec![]);
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

    let rendered = rendered_output(&received_screen_instructions.lock());
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
    let received_screen_instructions = Watched::new(vec![]);
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

    let rendered = rendered_output(&received_screen_instructions.lock());
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

    let received_pty_instructions = Watched::new(vec![]);
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::SpawnTerminal,
        pty_receiver,
        1
    );
    let received_screen_instructions = Watched::new(vec![]);
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
        .iter()
        .any(|instruction| matches!(instruction, PtyInstruction::SpawnTerminal(..)));
    assert!(
        spawned,
        "expected `zellij.open_terminal` to reach the pty thread, got: {:?}",
        received_pty_instructions.lock()
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

    let received_screen_instructions = Watched::new(vec![]);
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

    let rendered = rendered_output(&received_screen_instructions.lock());
    assert!(
        rendered.iter().any(|r| r.contains("payload carried")),
        "the reserved-word key should have survived to `load`, got: {:?}",
        rendered
    );
}

#[test]
#[serial(plugin_tests)]
pub fn a_newly_reachable_command_works_end_to_end() {
    // `open_command_pane` was one of the ~220 commands a Lua plugin could not call: its payload is
    // a `CommandToRun`, a struct nested inside a tuple variant, and nothing hand-marshalled it.
    // This runs the shipped playground plugin through the real plugin thread and presses the key
    // that calls it, so the whole path is covered - the metatable that manufactures the wrapper,
    // serde building the nested payload, the permission check, and the handler itself.
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_playground.kdl");
    let (plugin_thread_sender, pty_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_pty_receiver(Some(plugin_host_folder), None, None);
    let client_id = 1;
    let size = Size { cols: 80, rows: 20 };

    let received_pty_instructions = Watched::new(vec![]);
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::SpawnTerminal,
        pty_receiver,
        1
    );
    let received_screen_instructions = Watched::new(vec![]);
    let _screen_thread = grant_permissions_and_log_actions_in_thread_naked_variant!(
        received_screen_instructions,
        ScreenInstruction::Exit,
        screen_receiver,
        1,
        &PermissionType::RunCommands,
        cache_path,
        plugin_thread_sender,
        client_id
    );

    let playground = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../example-plugins/playground.lua")
        .display()
        .to_string();

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        Some(false),
        false,
        false,
        Some("lua_playground".to_owned()),
        lua_run_plugin_at(&playground, BTreeMap::new()),
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
    // 'c' is what the playground maps to `zellij.open_command_pane`.
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('c'))),
    )]));
    pty_thread.join().unwrap();
    teardown();

    let ran_the_command = received_pty_instructions.lock().iter().any(|instruction| {
        matches!(
            instruction,
            PtyInstruction::SpawnTerminal(Some(TerminalAction::RunCommand(command)), ..)
                if command.command.ends_with("sh")
        )
    });
    assert!(
        ran_the_command,
        "expected `zellij.open_command_pane` to reach the pty thread with its command, got: {:?}",
        received_pty_instructions.lock()
    );
}

/// The shipped `agent_exit` hook, loaded the way the hook dispatcher loads it: by builtin tag,
/// with the hook payload as its configuration.
fn agent_exit_hook(payload: BTreeMap<String, String>) -> RunPluginOrAlias {
    RunPluginOrAlias::from_url("zellij:agent-exit", &Some(payload), None, None)
        .expect("the builtin agent-exit hook resolves")
}

fn agent_exit_payload() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("agent_pane_id".to_owned(), "7".to_owned()),
        ("agent_cwd".to_owned(), "/work/.worktrees/topic".to_owned()),
        ("agent_exit_code".to_owned(), "0".to_owned()),
        ("agent_command".to_owned(), "codex".to_owned()),
    ])
}

#[test]
#[serial(plugin_tests)]
pub fn the_agent_exit_hook_renders_the_screen_it_replaced() {
    // The text this asserts used to be produced by `render_first_run_banner` in the terminal
    // emulator. It is the same text because the point of moving it was to make it editable, not
    // to change what an agent exiting looks like.
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let client_id = 1;
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Watched::new(vec![]);
    // One frame: the hook paints its question once and then waits for a key. Waiting for a
    // second would be waiting for something that never comes.
    let screen_thread = log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        1
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        Some(false),
        false,
        false,
        Some("agent_exit_hook".to_owned()),
        agent_exit_hook(agent_exit_payload()),
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
    screen_thread.join().unwrap();
    teardown();

    let rendered = rendered_output(&received_screen_instructions.lock()).join("\n");
    assert!(
        rendered.contains("Agent exited. Create another agent in the same directory?"),
        "got: {rendered}"
    );
    assert!(
        rendered.contains("Directory: /work/.worktrees/topic"),
        "the payload's cwd is shown, got: {rendered}"
    );
    assert!(rendered.contains("create another agent"), "got: {rendered}");
    assert!(rendered.contains("close pane"), "got: {rendered}");
}

#[test]
#[serial(plugin_tests)]
pub fn the_agent_exit_hook_restarts_the_pane_on_enter() {
    // Enter restarts the pane the hook stands in for. It reaches the pane through
    // `RerunCommandPane`, which finds it among the suppressed panes - the hook does not need to
    // know the command, because the pane replays the one it already holds.
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let client_id = 1;
    let size = Size { cols: 80, rows: 10 };

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::RerunCommandPane,
        screen_receiver,
        1
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        Some(false),
        false,
        false,
        Some("agent_exit_hook".to_owned()),
        agent_exit_hook(agent_exit_payload()),
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
        Event::Key(KeyWithModifier::new(BareKey::Enter)),
    )]));
    screen_thread.join().unwrap();
    teardown();

    let rerun = received_screen_instructions
        .lock()
        .iter()
        .any(|instruction| matches!(instruction, ScreenInstruction::RerunCommandPane(7, _)));
    assert!(
        rerun,
        "expected Enter to rerun pane 7, got: {:?}",
        received_screen_instructions.lock()
    );
}

#[test]
#[serial(plugin_tests)]
pub fn the_agent_exit_hook_closes_the_pane_on_escape() {
    // Esc closes the pane the hook stands in for, and only then itself: closing the suppressed
    // pane first is what stops it being restored when this one goes.
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let (plugin_thread_sender, pty_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_pty_receiver(Some(plugin_host_folder), None, None);
    let client_id = 1;
    let size = Size { cols: 80, rows: 10 };

    let received_pty_instructions = Watched::new(vec![]);
    let pty_thread = log_actions_in_thread!(
        received_pty_instructions,
        PtyInstruction::ClosePane,
        pty_receiver,
        1
    );
    let received_screen_instructions = Watched::new(vec![]);
    let _screen_thread = log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::ClosePane,
        screen_receiver,
        1
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        Some(false),
        false,
        false,
        Some("agent_exit_hook".to_owned()),
        agent_exit_hook(agent_exit_payload()),
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
        Event::Key(KeyWithModifier::new(BareKey::Esc)),
    )]));
    pty_thread.join().unwrap();
    teardown();

    let closed = received_pty_instructions.lock().iter().any(|instruction| {
        matches!(
            instruction,
            PtyInstruction::ClosePane(PaneId::Terminal(7), _)
        )
    });
    assert!(
        closed,
        "expected Esc to close pane 7, got: {:?}",
        received_pty_instructions.lock()
    );
    let closed_in_screen = received_screen_instructions
        .lock()
        .iter()
        .any(|instruction| {
            matches!(
                instruction,
                ScreenInstruction::ClosePane(PaneId::Terminal(7), ..)
            )
        });
    assert!(
        closed_in_screen,
        "expected Esc to close pane 7 in the screen too, got: {:?}",
        received_screen_instructions.lock()
    );
}
