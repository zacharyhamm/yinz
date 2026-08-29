//! End-to-end tests for Lua plugins: a `.lua` file on disk, through the real plugin thread, to
//! the bytes the screen would draw.
//!
//! These use the same harness as the wasm plugin tests, which is the point - a Lua plugin is
//! loaded, subscribed, updated and rendered by exactly the same code.

use super::*;
use crate::plugins::PluginId;
use zellij_test_support::Watched;
use zellij_utils::input::command::TerminalAction;
use zellij_utils::input::hooks::HookPoint;

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

/// A plugin named rather than pathed is found in the user's plugin folder.
///
/// This is what makes `plugin location="file:probe.lua"` in a layout - and the same URL from a
/// plugin asking for a reload - mean the obvious thing without anyone spelling out an absolute
/// path. The resolution chain itself is unit-tested in `zellij-utils`; what this covers is that
/// the folder actually reaches the loader through the whole plugin thread.
#[test]
#[serial(plugin_tests)]
pub fn a_lua_plugin_is_found_in_the_user_plugin_dir_by_name() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let user_plugin_dir = tempdir().unwrap();
    std::fs::write(
        user_plugin_dir.path().join("probe.lua"),
        r#"
        local M = {}
        function M.render() return "found by name" end
        return M
        "#,
    )
    .unwrap();

    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread_with_user_plugin_dir(
            Some(plugin_host_folder),
            None,
            Some(PathBuf::from(user_plugin_dir.path())),
        );
    let client_id = 1;
    let size = Size { cols: 80, rows: 10 };
    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        1
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    // A bare file name, with nothing to say where it lives.
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        Some(false),
        false,
        false,
        Some("lua_test_plugin".to_owned()),
        lua_run_plugin_at("probe.lua", BTreeMap::new()),
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
    ));
    screen_thread.join().unwrap();
    teardown();

    let rendered = rendered_output(&received_screen_instructions.lock());
    assert!(
        rendered.iter().any(|r| r.contains("found by name")),
        "expected the plugin from the user plugin dir to render, got: {:?}",
        rendered
    );
}

/// A reload naming the file in full finds the plugin that was started by bare name.
///
/// The two spellings do not look alike to the plugin map: `file:probe.lua` keeps the cwd-joined
/// path as its `location` - that is its identity, and what a dumped layout has to say - while its
/// source is read from the user's plugin folder. A reload can only name the file it wrote, so
/// without `resolved_source_path` the lookup misses, the reload is answered with
/// `PluginDoesNotExist`, and the plugin is *started again* in a second pane instead. That is what
/// this covers: the same instance re-renders, and no `AddPlugin` is sent.
#[test]
#[serial(plugin_tests)]
pub fn a_reload_by_absolute_path_reloads_a_plugin_started_by_name() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let user_plugin_dir = tempdir().unwrap();
    let plugin_file = user_plugin_dir.path().join("probe.lua");
    let source = |version: &str| {
        format!(
            r#"
            local M = {{}}
            function M.render() return "{version}" end
            return M
            "#
        )
    };
    std::fs::write(&plugin_file, source("first version")).unwrap();

    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread_with_user_plugin_dir(
            Some(plugin_host_folder),
            None,
            Some(PathBuf::from(user_plugin_dir.path())),
        );
    let client_id = 1;
    let size = Size { cols: 80, rows: 10 };
    let received_screen_instructions = Watched::new(vec![]);
    // Two renders: the load, then the reload.
    let screen_thread = log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    // A bare file name, exactly as a layout or a keybinding would say it.
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        Some(false),
        false,
        false,
        Some("lua_test_plugin".to_owned()),
        lua_run_plugin_at("probe.lua", BTreeMap::new()),
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
    ));

    // The reload has to arrive after the load has finished, or `plugin_is_currently_being_loaded`
    // defers it - a different path, and not the one under test.
    received_screen_instructions.wait_until(
        "the plugin to render its first version",
        std::time::Duration::from_secs(10),
        |instructions| {
            rendered_output(instructions)
                .iter()
                .any(|rendered| rendered.contains("first version"))
                .then_some(())
        },
    );

    std::fs::write(&plugin_file, source("second version")).unwrap();
    let _ = plugin_thread_sender.send(PluginInstruction::Reload(
        Some(false),
        Some("lua_test_plugin".to_owned()),
        // The absolute path, which is all a plugin that just wrote the file can name.
        lua_run_plugin_at(&plugin_file.display().to_string(), BTreeMap::new()),
        1,
        size,
        None,
    ));

    screen_thread.join().unwrap();
    teardown();

    let logged = received_screen_instructions.lock();
    let rendered = rendered_output(&logged);
    assert!(
        rendered.iter().any(|r| r.contains("second version")),
        "the reload should have re-read the file, got: {:?}",
        rendered
    );
    // One `AddPlugin`, from the initial load that opened the pane. A second one is the failure
    // this covers: the reload was answered with `PluginDoesNotExist` and started a new instance.
    let panes_opened = logged
        .iter()
        .filter(|i| matches!(i, ScreenInstruction::AddPlugin(..)))
        .count();
    assert_eq!(
        panes_opened, 1,
        "the running plugin should have been reloaded, not started a second time in a new pane"
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

/// The `AddPlugin` a hook produces, as (plugin id, pane it replaces).
///
/// Asserted on rather than counted: a hook that opens *something* is not the contract - the
/// contract is that it opens over the pane that exited, which is what makes the terminal pane
/// go behind it instead of staying on screen with its exit code.
fn hook_panes_opened(instructions: &[ScreenInstruction]) -> Vec<(PluginId, PaneId)> {
    instructions
        .iter()
        .filter_map(|instruction| match instruction {
            ScreenInstruction::AddPlugin(
                _,
                true, // should_be_open_in_place
                _,
                _,
                _,
                _,
                plugin_id,
                Some(pane_id_to_replace),
                ..,
            ) => Some((*plugin_id, *pane_id_to_replace)),
            _ => None,
        })
        .collect()
}

#[test]
#[serial(plugin_tests)]
pub fn the_agent_exit_hook_opens_a_pane_in_place_of_the_one_that_exited() {
    // The whole of what a hook promises: the pane that exited is replaced by the plugin bound
    // to the point. Driven through `RunHook`, the way the screen fires it - the tests around
    // this one load the plugin directly, which is the half that was already covered.
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let client_id = 1;

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::AddPlugin,
        screen_receiver,
        1
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::RunHook {
        point: HookPoint::AgentExit,
        pane_id: Some(PaneId::Terminal(7)),
        payload: agent_exit_payload(),
        client_id,
    });
    screen_thread.join().unwrap();
    teardown();

    let opened = hook_panes_opened(&received_screen_instructions.lock());
    assert_eq!(
        opened.iter().map(|(_, pane)| *pane).collect::<Vec<_>>(),
        vec![PaneId::Terminal(7)],
        "expected the hook to open over pane 7, got: {:?}",
        received_screen_instructions.lock()
    );
}

#[test]
#[serial(plugin_tests)]
pub fn the_agent_exit_hook_opens_again_after_the_first_one_is_closed() {
    // The hook's own flow: it offers to restart the agent, and a restarted agent exits again -
    // with the same pane, cwd, command and exit code, so with the same payload and therefore
    // the same plugin identity as the hook that has just closed.
    //
    // Worth pinning because plugin identity is (location, configuration) and the hook's
    // configuration *is* its payload: the second firing looks identical to the first, and
    // anything that dedupes on that identity would answer it with the instance that has just
    // gone. The pipe in the middle is a plugin lookup landing between the two, which is the
    // shape hocus produces continuously - it must not change the answer.
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let client_id = 1;

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::AddPlugin,
        screen_receiver,
        2
    );

    let fire = || {
        let _ = plugin_thread_sender.send(PluginInstruction::RunHook {
            point: HookPoint::AgentExit,
            pane_id: Some(PaneId::Terminal(7)),
            payload: agent_exit_payload(),
            client_id,
        });
    };

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    fire();
    // Waited for rather than slept past: the id of the first hook pane is what has to be
    // unloaded, and it is not known until the screen has been told about it.
    let first = received_screen_instructions.wait_until(
        "the first hook pane",
        zellij_test_support::default_timeout(),
        |instructions| hook_panes_opened(instructions).first().map(|(id, _)| *id),
    );
    // A lookup of the same plugin while the first hook is still up, between the two firings.
    let _ = plugin_thread_sender.send(PluginInstruction::CliPipe {
        pipe_id: "cache-warmer".to_owned(),
        name: "ping".to_owned(),
        payload: None,
        plugin: Some("zellij:agent-exit".to_owned()),
        args: None,
        configuration: Some(agent_exit_payload()),
        floating: Some(false),
        pane_id_to_replace: None,
        pane_title: None,
        cwd: None,
        skip_cache: false,
        cli_client_id: client_id,
        source_pane_id: None,
    });
    let _ = plugin_thread_sender.send(PluginInstruction::Unload(first));
    fire();
    screen_thread.join().unwrap();
    teardown();

    let opened = hook_panes_opened(&received_screen_instructions.lock());
    assert_eq!(
        opened.iter().map(|(_, pane)| *pane).collect::<Vec<_>>(),
        vec![PaneId::Terminal(7), PaneId::Terminal(7)],
        "expected a second hook pane over pane 7, got: {:?}",
        received_screen_instructions.lock()
    );
    assert_ne!(
        opened[0].0, opened[1].0,
        "the second hook is a new instance, not the closed one named again"
    );
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

/// A headless hook: names every hook it is handed after a tab, once it may.
///
/// The rename needs `change_application_state`, which a `file:` plugin has to ask for, and the
/// first firing can reach the plugin before the grant does - the event is held through the load
/// and delivered the moment it completes, while the grant round-trips through the screen. So the
/// plugin queues until `permission_request_result` and then speaks; the second firing, after the
/// grant, speaks at once. Either way the count is exact.
const HEADLESS_HOOK_LUA: &str = r#"
local zellij = require("zellij")
local M = { granted = false, queued = {} }
local function say(e)
  local subject = e.payload.exit_pane_id or e.payload.session_name or ""
  zellij.rename_tab_with_id { 1, e.point .. ":" .. subject }
end
function M.load(config)
  zellij.subscribe { "hook", "permission_request_result" }
  zellij.request_permission { "change_application_state" }
end
zellij.on("permission_request_result", function()
  M.granted = true
  for _, e in ipairs(M.queued) do say(e) end
  M.queued = {}
end)
zellij.on("hook", function(e)
  if M.granted then say(e) else table.insert(M.queued, e) end
end)
function M.render() return "" end
return M
"#;

/// Every tab rename the hook asked for, as the name it gave.
fn tab_names_set(instructions: &[ScreenInstruction]) -> Vec<String> {
    instructions
        .iter()
        .filter_map(|instruction| match instruction {
            ScreenInstruction::RenameTabWithId(_, name, _) => {
                Some(String::from_utf8_lossy(name).to_string())
            },
            _ => None,
        })
        .collect()
}

/// The `AddPlugin`s that opened nothing on screen, as plugin ids: a headless hook's loads.
fn suppressed_plugins_loaded(instructions: &[ScreenInstruction]) -> Vec<PluginId> {
    instructions
        .iter()
        .filter_map(|instruction| match instruction {
            ScreenInstruction::AddPlugin(_, _, _, _, _, _, plugin_id, _, _, true, ..) => {
                Some(*plugin_id)
            },
            _ => None,
        })
        .collect()
}

fn headless_hook_plugin(user_plugin_dir: &std::path::Path) -> RunPluginOrAlias {
    std::fs::write(user_plugin_dir.join("hook.lua"), HEADLESS_HOOK_LUA).unwrap();
    RunPluginOrAlias::RunPlugin(
        RunPlugin::from_url(&format!(
            "file:{}",
            user_plugin_dir.join("hook.lua").display()
        ))
        .unwrap(),
    )
}

fn bind_hook(
    plugin_thread_sender: &SenderWithContext<PluginInstruction>,
    point: HookPoint,
    plugin: RunPluginOrAlias,
) {
    let _ = plugin_thread_sender.send(PluginInstruction::Reconfigure {
        client_id: 1,
        keybinds: None,
        default_mode: None,
        default_shell: None,
        layout_dir: None,
        user_plugin_dir: None,
        builtin_plugins_dir: None,
        hooks: zellij_utils::input::hooks::Hooks::from_data(BTreeMap::from([(point, plugin)])),
        context_menu: Default::default(),
        was_written_to_disk: false,
    });
}

fn pane_exit_payload(pane_id: u32) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("exit_pane_id".to_owned(), pane_id.to_string()),
        ("exit_code".to_owned(), "0".to_owned()),
        ("exit_held".to_owned(), "true".to_owned()),
    ])
}

#[test]
#[serial(plugin_tests)]
pub fn a_headless_hook_is_loaded_on_first_fire_and_receives_the_event() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let user_plugin_dir = tempdir().unwrap();
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let client_id = 1;

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::RenameTabWithId,
        screen_receiver,
        1,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        client_id
    );

    bind_hook(
        &plugin_thread_sender,
        HookPoint::PaneExit,
        headless_hook_plugin(user_plugin_dir.path()),
    );
    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::RunHook {
        point: HookPoint::PaneExit,
        pane_id: Some(PaneId::Terminal(7)),
        payload: pane_exit_payload(7),
        client_id,
    });
    screen_thread.join().unwrap();
    teardown();

    let instructions = received_screen_instructions.lock();
    assert_eq!(
        tab_names_set(&instructions),
        vec!["pane_exit:7".to_owned()],
        "got: {instructions:?}"
    );
    assert_eq!(
        suppressed_plugins_loaded(&instructions).len(),
        1,
        "the hook is loaded headless, once, got: {instructions:?}"
    );
    assert!(
        hook_panes_opened(&instructions).is_empty(),
        "a headless hook opens nothing in place of the pane, got: {instructions:?}"
    );
}

#[test]
#[serial(plugin_tests)]
pub fn a_second_firing_reuses_the_headless_hook_instance() {
    // The opposite of the pane-replacing shape: the instance is found by its configured
    // identity, so a different payload is the same plugin hearing again.
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let user_plugin_dir = tempdir().unwrap();
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let client_id = 1;

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::RenameTabWithId,
        screen_receiver,
        2,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        client_id
    );

    bind_hook(
        &plugin_thread_sender,
        HookPoint::PaneExit,
        headless_hook_plugin(user_plugin_dir.path()),
    );
    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::RunHook {
        point: HookPoint::PaneExit,
        pane_id: Some(PaneId::Terminal(7)),
        payload: pane_exit_payload(7),
        client_id,
    });
    // Once the first firing has been answered the plugin is up and granted; the second one
    // must reach the same instance rather than load another.
    received_screen_instructions.wait_until(
        "the first firing to be named",
        zellij_test_support::default_timeout(),
        |instructions| (!tab_names_set(instructions).is_empty()).then_some(()),
    );
    let _ = plugin_thread_sender.send(PluginInstruction::RunHook {
        point: HookPoint::PaneExit,
        pane_id: Some(PaneId::Terminal(8)),
        payload: pane_exit_payload(8),
        client_id,
    });
    screen_thread.join().unwrap();
    teardown();

    let instructions = received_screen_instructions.lock();
    assert_eq!(
        tab_names_set(&instructions),
        vec!["pane_exit:7".to_owned(), "pane_exit:8".to_owned()],
        "got: {instructions:?}"
    );
    assert_eq!(
        suppressed_plugins_loaded(&instructions).len(),
        1,
        "one instance for every firing, got: {instructions:?}"
    );
}

#[test]
#[serial(plugin_tests)]
pub fn a_replace_pane_hook_without_a_pane_opens_nothing() {
    // `agent_exit` replaces a pane; fired with none there is nothing to replace, and it must not
    // open its plugin somewhere else instead.
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let client_id = 1;

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::RunHook {
        point: HookPoint::AgentExit,
        pane_id: None,
        payload: agent_exit_payload(),
        client_id,
    });
    // The thread drains its instructions in order and `teardown` joins it, so everything it
    // was ever going to send the screen has been sent.
    teardown();

    let instructions: Vec<ScreenInstruction> = screen_receiver
        .try_iter()
        .map(|(instruction, _)| instruction)
        .collect();
    assert!(
        !instructions
            .iter()
            .any(|i| matches!(i, ScreenInstruction::AddPlugin(..))),
        "got: {instructions:?}"
    );
}

#[test]
#[serial(plugin_tests)]
pub fn session_start_fires_for_the_initiating_client() {
    // Bound from the start, as a config binding is: the point fires when the first client is
    // added, which in a real session is before any reconfigure could land.
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let user_plugin_dir = tempdir().unwrap();
    let hooks = zellij_utils::input::hooks::Hooks::from_data(BTreeMap::from([(
        HookPoint::SessionStart,
        headless_hook_plugin(user_plugin_dir.path()),
    )]));
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread_with_hooks(Some(plugin_host_folder), None, None, hooks);
    let client_id = 1;
    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = grant_permissions_and_log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::RenameTabWithId,
        screen_receiver,
        1,
        &PermissionType::ChangeApplicationState,
        cache_path,
        plugin_thread_sender,
        client_id
    );
    screen_thread.join().unwrap();
    teardown();

    let instructions = received_screen_instructions.lock();
    assert_eq!(
        tab_names_set(&instructions),
        vec!["session_start:zellij-test".to_owned()],
        "the harness names its session zellij-test, got: {instructions:?}"
    );
}
