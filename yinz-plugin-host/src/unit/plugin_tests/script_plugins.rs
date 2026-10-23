//! End-to-end tests for Scheme plugins: a `.scm` file on disk, through the real plugin thread, to
//! the bytes the screen would draw.
//!
//! These use the same harness as the wasm plugin tests, which is the point - a Scheme plugin is
//! loaded, subscribed, updated and rendered by exactly the same code.

#[cfg(test)]
const CLIENT_1: yinz_types::internal_ids::ClientId = yinz_types::internal_ids::ClientId::new(1);
#[cfg(test)]
const TERMINAL_7: yinz_types::internal_ids::TerminalId =
    yinz_types::internal_ids::TerminalId::new(7);

use super::*;
use crate::HookFiring;
use crate::PluginId;
use yinz_bus::pty::ClientTabIndexOrPaneId;
use yinz_test_support::Watched;
use yinz_types::data::HookPayload;
use yinz_types::input::command::TerminalAction;
use yinz_types::input::hooks::HookPoint;
use yinz_types::internal_ids::TabId;

/// Load the Scheme fixture and return everything the screen was told to draw for it.
pub(super) fn rendered_output(instructions: &[ScreenInstruction]) -> Vec<String> {
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

fn script_run_plugin(configuration: BTreeMap<String, String>) -> RunPluginOrAlias {
    script_run_plugin_at(&SCRIPT_PLUGIN_FIXTURE, configuration)
}

fn script_run_plugin_at(path: &str, configuration: BTreeMap<String, String>) -> RunPluginOrAlias {
    RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(path)),
        configuration: PluginUserConfiguration::new(configuration),
        ..Default::default()
    })
}

#[test]
#[serial(plugin_tests)]
pub fn script_plugin_loads_and_renders() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let client_id = ClientId::new(1);
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = std::thread::spawn({
        let log = received_screen_instructions.clone();
        move || {
            while let Ok((event, _)) =
                screen_receiver.recv_timeout(yinz_test_support::default_timeout())
            {
                log.lock().push(event);
                if rendered_output(&log.lock())
                    .iter()
                    .any(|frame| frame.contains("presses 1"))
                {
                    break;
                }
            }
        }
    });

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        Some(false),
        false,
        false,
        Some("script_test_plugin".to_owned()),
        script_run_plugin(BTreeMap::new()),
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
    received_screen_instructions.wait_until(
        "the plugin is ready",
        yinz_test_support::default_timeout(),
        |instructions| {
            rendered_output(instructions)
                .iter()
                .any(|frame| frame.contains("presses 0"))
                .then_some(())
        },
    );
    // The fixture subscribes to Key, so this reaches `update` and asks for a re-render.
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('a'))),
    )]));
    received_screen_instructions.wait_until(
        "the key reached update",
        yinz_test_support::default_timeout(),
        |instructions| {
            rendered_output(instructions)
                .iter()
                .any(|frame| frame.contains("presses 1"))
                .then_some(())
        },
    );
    teardown();
    screen_thread.join().unwrap();

    let rendered = rendered_output(&received_screen_instructions.lock());
    assert!(
        rendered.iter().any(|r| r.contains("hello from scheme")),
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
/// This is what makes `plugin location="file:probe.scm"` in a layout - and the same URL from a
/// plugin asking for a reload - mean the obvious thing without anyone spelling out an absolute
/// path. The resolution chain itself is unit-tested in `yinz-config`; what this covers is that
/// the folder actually reaches the loader through the whole plugin thread.
#[test]
#[serial(plugin_tests)]
pub fn a_script_plugin_is_found_in_the_user_plugin_dir_by_name() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let user_plugin_dir = tempdir().unwrap();
    std::fs::write(
        user_plugin_dir.path().join("probe.scm"),
        r#"
        (define (render rows cols) "found by name")
        "#,
    )
    .unwrap();

    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread_with_user_plugin_dir(
            Some(plugin_host_folder),
            None,
            Some(PathBuf::from(user_plugin_dir.path())),
        );
    let client_id = ClientId::new(1);
    let size = Size { cols: 80, rows: 10 };
    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = log_until_disconnect(&received_screen_instructions, screen_receiver);

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    // A bare file name, with nothing to say where it lives.
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        Some(false),
        false,
        false,
        Some("script_test_plugin".to_owned()),
        script_run_plugin_at("probe.scm", BTreeMap::new()),
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
    wait_for_render_containing(&received_screen_instructions, "found by name");
    teardown();
    screen_thread.join().unwrap();

    let rendered = rendered_output(&received_screen_instructions.lock());
    assert!(
        rendered.iter().any(|r| r.contains("found by name")),
        "expected the plugin from the user plugin dir to render, got: {:?}",
        rendered
    );
}

/// A reload naming the file in full finds the plugin that was started by bare name.
///
/// The two spellings do not look alike to the plugin map: `file:probe.scm` keeps the cwd-joined
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
    let plugin_file = user_plugin_dir.path().join("probe.scm");
    let source = |version: &str| {
        format!(
            r#"
            (define (render rows cols) "{version}")
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
    let client_id = ClientId::new(1);
    let size = Size { cols: 80, rows: 10 };
    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = log_until_disconnect(&received_screen_instructions, screen_receiver);

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    // A bare file name, exactly as a layout or a keybinding would say it.
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        Some(false),
        false,
        false,
        Some("script_test_plugin".to_owned()),
        script_run_plugin_at("probe.scm", BTreeMap::new()),
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
        Some("script_test_plugin".to_owned()),
        // The absolute path, which is all a plugin that just wrote the file can name.
        script_run_plugin_at(&plugin_file.display().to_string(), BTreeMap::new()),
        TabId::new(2), // A different client's destination must not move the existing instance.
        size,
        None,
    ));

    wait_for_render_containing(&received_screen_instructions, "second version");
    teardown();
    screen_thread.join().unwrap();

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
fn a_missing_plugin_starts_in_the_requested_reload_tab() {
    let temp_folder = tempdir().unwrap();
    let (sender, screen_receiver, teardown) =
        create_plugin_thread(Some(temp_folder.path().to_owned()), None);
    let received = Watched::new(vec![]);
    let screen_thread = log_until_disconnect(&received, screen_receiver);
    sender.send(PluginInstruction::AddClient(CLIENT_1)).unwrap();
    sender
        .send(PluginInstruction::AddClient(ClientId::new(2)))
        .unwrap();
    let destination = TabId::new(7);
    sender
        .send(PluginInstruction::Reload(
            Some(false),
            Some("new from reload".into()),
            script_run_plugin(BTreeMap::new()),
            destination,
            Size { cols: 80, rows: 24 },
            None,
        ))
        .unwrap();
    let tab = received.wait_until(
        "missing plugin's destination",
        yinz_test_support::default_timeout(),
        |instructions| {
            instructions
                .iter()
                .find_map(|instruction| match instruction {
                    ScreenInstruction::AddPlugin(_, _, _, _, _, tab, ..) => *tab,
                    _ => None,
                })
        },
    );
    teardown();
    screen_thread.join().unwrap();
    assert_eq!(tab, destination);
}

#[test]
#[serial(plugin_tests)]
pub fn script_plugin_receives_its_configuration() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let client_id = ClientId::new(1);
    let size = Size { cols: 80, rows: 10 };
    let mut configuration = BTreeMap::new();
    // The fixture defaults this to "hello"; the prelude merges the user's value over the top.
    configuration.insert("greeting".to_owned(), "configured".to_owned());

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = log_until_disconnect(&received_screen_instructions, screen_receiver);

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        Some(false),
        false,
        false,
        Some("script_test_plugin".to_owned()),
        script_run_plugin(configuration),
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
    wait_for_render_containing(&received_screen_instructions, "presses 0");
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('a'))),
    )]));
    wait_for_render_containing(&received_screen_instructions, "presses 1");
    teardown();
    screen_thread.join().unwrap();

    let rendered = rendered_output(&received_screen_instructions.lock());
    assert!(
        rendered
            .iter()
            .any(|r| r.contains("configured from scheme")),
        "expected the merged configuration to reach `load`, got: {:?}",
        rendered
    );
}

#[test]
#[serial(plugin_tests)]
pub fn script_plugin_is_not_sent_events_it_did_not_subscribe_to() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let client_id = ClientId::new(1);
    let size = Size { cols: 80, rows: 10 };
    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = std::thread::spawn({
        let log = received_screen_instructions.clone();
        move || {
            while let Ok((event, _)) =
                screen_receiver.recv_timeout(yinz_test_support::default_timeout())
            {
                log.lock().push(event);
                if rendered_output(&log.lock())
                    .iter()
                    .any(|frame| frame.contains("presses 1"))
                {
                    break;
                }
            }
        }
    });

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        Some(false),
        false,
        false,
        Some("script_test_plugin".to_owned()),
        script_run_plugin(BTreeMap::new()),
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
    received_screen_instructions.wait_until(
        "the plugin is ready",
        yinz_test_support::default_timeout(),
        |instructions| {
            rendered_output(instructions)
                .iter()
                .any(|frame| frame.contains("presses 0"))
                .then_some(())
        },
    );
    // The fixture subscribes to Key and TabUpdate but not Timer, so this must not increment the
    // press counter or produce a render of its own.
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Timer(yinz_types::units::FractionalSeconds::new(1.0)),
    )]));
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('a'))),
    )]));
    received_screen_instructions.wait_until(
        "the key reached update",
        yinz_test_support::default_timeout(),
        |instructions| {
            rendered_output(instructions)
                .iter()
                .any(|frame| frame.contains("presses 1"))
                .then_some(())
        },
    );
    teardown();
    screen_thread.join().unwrap();

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

/// What `yinz.agent.spawn` hands the pty thread: the planned argv - `claude`, the prompt last,
/// `--settings <hooks>` first unless hooks are off - the folder the plugin named, and the agent
/// marker whose value `agent_from_run` turns into the pane's `Spawned` record.
fn spawned_agent_command(key: char) -> yinz_types::input::command::RunCommand {
    use yinz_types::input::command::RunCommand;
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, pty_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_pty_receiver(Some(plugin_host_folder.clone()), None, None);
    let client_id = ClientId::new(1);
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
        &PermissionType::RunCommands,
        cache_path,
        plugin_thread_sender,
        client_id
    );

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        Some(false),
        false,
        false,
        Some("script_spawn_plugin".to_owned()),
        script_run_plugin_at(&SCRIPT_PLUGIN_FIXTURE_SPAWN_AGENT, BTreeMap::new()),
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
    wait_for_render_containing(&received_screen_instructions, "spawn fixture ready");
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char(key))),
    )]));
    pty_thread.join().unwrap();
    teardown();

    let (run, placement): (RunCommand, ClientTabIndexOrPaneId) = received_pty_instructions
        .lock()
        .iter()
        .find_map(|instruction| match instruction {
            PtyInstruction::SpawnTerminal(
                Some(TerminalAction::RunCommand(run)),
                _,
                _,
                _,
                placement,
                ..,
            ) => Some((run.clone(), *placement)),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "expected `yinz.agent.spawn` to reach the pty thread, got: {:?}",
                received_pty_instructions.lock()
            )
        });
    // Beside the plugin, or - when it has none, having been loaded before any client attached -
    // wherever this client is looking. Anchoring to the pane alone would have the screen forget
    // the pane and close the agent the pty thread had already started.
    assert!(
        matches!(
            placement,
            ClientTabIndexOrPaneId::PaneIdOrClient(PaneId::Plugin(_), CLIENT_1)
        ),
        "the agent pane is placed near the plugin with a client fallback, got: {placement:?}"
    );
    // The folder's name rather than its path: a `tempdir()` lives under `/tmp`, which the host
    // reads as the plugin's shared `/tmp` mount and re-translates on the way to the pty.
    assert_eq!(
        run.cwd.as_deref().and_then(|cwd| cwd.file_name()),
        temp_folder.path().file_name(),
        "`cwd = \"/host\"` is the plugin's folder: {run:?}"
    );
    run
}

#[test]
#[serial(plugin_tests)]
pub fn a_script_plugin_spawns_an_agent_pane_with_the_planned_argv_and_an_agent_marker() {
    use yinz_types::data::AGENT_PANE_CONTEXT;
    let run = spawned_agent_command('s');
    assert_eq!(run.command, PathBuf::from("env"));
    assert_eq!(
        run.args,
        [
            "YINZ_CLAUDE_INLINE_HOOKS=0",
            "yinz",
            "agent",
            "claude",
            "--command",
            "claude",
            "--",
            "fix it"
        ]
    );
    let context = run
        .originating_plugin
        .as_ref()
        .map(|origin| origin.context.clone())
        .unwrap_or_default();
    assert_eq!(
        context.get(AGENT_PANE_CONTEXT).map(String::as_str),
        Some("claude"),
        "the marker carries the kind: {context:?}"
    );
}

#[test]
#[serial(plugin_tests)]
pub fn a_spawned_agent_is_handed_its_hooks_inline_unless_told_not_to() {
    let run = spawned_agent_command('h');
    assert_eq!(run.command, PathBuf::from("env"));
    assert_eq!(
        &run.args[..8],
        [
            "YINZ_CLAUDE_INLINE_HOOKS=1",
            "yinz",
            "agent",
            "claude",
            "--command",
            "claude",
            "--",
            "--settings"
        ]
    );
    assert!(
        run.args[8].contains("agent-report"),
        "the inline settings wire the hooks: {:?}",
        run.args
    );
    assert_eq!(run.args.last().map(String::as_str), Some("fix it"));
}

#[test]
#[serial(plugin_tests)]
pub fn script_plugin_host_command_reaches_the_pty_thread() {
    // The whole point of the design is that a Scheme plugin's commands go through the same handlers
    // wasm plugins use, so this checks one all the way through: `yinz.open_terminal` in Scheme
    // should arrive at the pty thread as a `SpawnTerminal`, permission-gated like any other.
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let (plugin_thread_sender, pty_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_pty_receiver(Some(plugin_host_folder), None, None);
    let client_id = ClientId::new(1);
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
        Some("script_test_plugin".to_owned()),
        script_run_plugin_at(&SCRIPT_PLUGIN_FIXTURE_WITH_PERMISSIONS, BTreeMap::new()),
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
    wait_for_render_containing(
        &received_screen_instructions,
        "scheme fixture with permissions",
    );
    // 't' is what the fixture maps to `yinz.open_terminal`.
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
        "expected `yinz.open_terminal` to reach the pty thread, got: {:?}",
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
    let client_id = ClientId::new(1);
    let size = Size { cols: 80, rows: 10 };

    let mut configuration = PluginUserConfiguration::new(BTreeMap::new());
    // Inserted after construction, exactly as the reserved-word filter intends to allow.
    configuration.insert("payload", "carried");
    let run_plugin = RunPluginOrAlias::RunPlugin(RunPlugin {
        _allow_exec_host_cmd: false,
        location: RunPluginLocation::File(PathBuf::from(&*SCRIPT_PLUGIN_FIXTURE)),
        configuration,
        ..Default::default()
    });

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = log_until_disconnect(&received_screen_instructions, screen_receiver);

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        Some(false),
        false,
        false,
        Some("script_test_plugin".to_owned()),
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
    wait_for_render_containing(&received_screen_instructions, "payload carried");
    let _ = plugin_thread_sender.send(PluginInstruction::Update(vec![(
        None,
        Some(client_id),
        Event::Key(KeyWithModifier::new(BareKey::Char('a'))),
    )]));
    wait_for_render_containing(&received_screen_instructions, "presses 1");
    teardown();
    screen_thread.join().unwrap();

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
    // `open_command_pane` was one of the ~220 commands a Scheme plugin could not call: its payload is
    // a `CommandToRun`, a struct nested inside a tuple variant, and nothing hand-marshalled it.
    // This runs the shipped playground plugin through the real plugin thread and presses the key
    // that calls it, so the whole path is covered - the metatable that manufactures the wrapper,
    // serde building the nested payload, the permission check, and the handler itself.
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_playground.kdl");
    let (plugin_thread_sender, pty_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_pty_receiver(Some(plugin_host_folder), None, None);
    let client_id = ClientId::new(1);
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
        .join("src/unit/plugin_tests/fixtures/fixture_command.scm")
        .display()
        .to_string();

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        Some(false),
        false,
        false,
        Some("script_playground".to_owned()),
        script_run_plugin_at(&playground, BTreeMap::new()),
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
    wait_for_render_containing(&received_screen_instructions, "command fixture ready");
    // 'c' is what the playground maps to `yinz.open_command_pane`.
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
        "expected `yinz.open_command_pane` to reach the pty thread with its command, got: {:?}",
        received_pty_instructions.lock()
    );
}

/// The shipped `agent_exit` hook, loaded the way the hook dispatcher loads it: by builtin tag,
/// with the hook payload as its configuration.
fn agent_exit_hook(payload: BTreeMap<String, String>) -> RunPluginOrAlias {
    yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        "yinz:agent-exit",
        &Some(payload),
        None,
        None,
    )
    .expect("the builtin agent-exit hook resolves")
}

fn agent_exit_payload() -> BTreeMap<String, String> {
    // Restart/close fallback; local Git transitions use a real-repository harness.
    BTreeMap::from([
        ("agent_target".to_owned(), "ssh".to_owned()),
        ("agent_pane_id".to_owned(), "7".to_owned()),
        (
            "agent_pane_title".to_owned(),
            "build-box topic: Review".to_owned(),
        ),
        ("agent_cwd".to_owned(), "/work/.worktrees/topic".to_owned()),
        ("agent_exit_code".to_owned(), "0".to_owned()),
        ("agent_command".to_owned(), "codex".to_owned()),
    ])
}

#[test]
#[serial(plugin_tests)]
pub fn the_agent_exit_hook_renders_the_screen_it_replaced() {
    // The hook now renders positioned UI components, whose text is serialized as byte codes.
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let client_id = ClientId::new(1);
    let size = Size {
        cols: 121,
        rows: 20,
    };
    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = log_until_disconnect(&received_screen_instructions, screen_receiver);

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        Some(false),
        false,
        false,
        Some("agent_exit_hook".to_owned()),
        agent_exit_hook(agent_exit_payload()),
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
    let ready_text = "Esc: close pane"
        .as_bytes()
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(",");
    wait_for_render_containing(&received_screen_instructions, &ready_text);
    teardown();
    screen_thread.join().unwrap();

    let rendered = rendered_output(&received_screen_instructions.lock()).join("\n");
    let contains = |text: &str| {
        let codes = text
            .as_bytes()
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(",");
        rendered.contains(&codes)
    };
    assert!(
        contains("No local checkout is available."),
        "got: {rendered}"
    );
    assert!(
        contains("Directory: /work/.worktrees/topic"),
        "the payload's cwd is shown, got: {rendered}"
    );
    assert!(contains("Enter: restart agent"), "got: {rendered}");
    assert!(contains("Esc: close pane"), "got: {rendered}");
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
    let client_id = ClientId::new(1);

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
        pane_id: Some(PaneId::Terminal(crate::TerminalId::new(7))),
        payload: HookFiring::ReplacePane(agent_exit_payload()),
        client_id,
    });
    screen_thread.join().unwrap();
    teardown();

    let opened = hook_panes_opened(&received_screen_instructions.lock());
    // The id in the title is the hook pane's own, not the terminal suppressed behind it, so it
    // is read off the same instruction rather than written out.
    let instructions = received_screen_instructions.lock();
    let (title, plugin_id) = instructions
        .iter()
        .find_map(|instruction| match instruction {
            ScreenInstruction::AddPlugin(_, _, _, _, Some(title), _, plugin_id, ..) => {
                Some((title, *plugin_id))
            },
            _ => None,
        })
        .expect("the hook opens a pane with a title");
    assert_eq!(
        title,
        &format!("[plugin_{plugin_id}] build-box topic: Review")
    );
    let reference = title.strip_prefix('[').unwrap().split_once(']').unwrap().0;
    let pane_id = reference.parse::<PaneId>().unwrap();
    assert_eq!(pane_id, PaneId::Plugin(plugin_id));
    assert_ne!(pane_id, plugin_id.to_string().parse::<PaneId>().unwrap());
    drop(instructions);
    assert_eq!(
        opened.iter().map(|(_, pane)| *pane).collect::<Vec<_>>(),
        vec![PaneId::Terminal(crate::TerminalId::new(7))],
        "expected the hook to open over pane 7, got: {:?}",
        received_screen_instructions.lock()
    );
}

#[test]
#[serial(plugin_tests)]
fn agent_exit_reentry_delivers_changed_and_identical_payloads_to_both_clients() {
    agent_exit_reentry(true, false, false);
}

#[test]
#[serial(plugin_tests)]
fn agent_exit_reentry_queues_firings_for_both_clients_during_loading() {
    agent_exit_reentry(false, false, false);
}

#[test]
#[serial(plugin_tests)]
fn agent_exit_reentry_reaches_remaining_client_after_origin_disconnects() {
    agent_exit_reentry(true, true, false);
}

#[test]
#[serial(plugin_tests)]
fn agent_exit_reentry_delivers_once_to_a_shared_instance() {
    agent_exit_reentry(true, false, true);
}

#[test]
#[serial(plugin_tests)]
fn agent_exit_reentry_queues_firings_once_for_a_shared_instance_during_loading() {
    agent_exit_reentry(false, false, true);
}

#[test]
#[serial(plugin_tests)]
fn agent_exit_reentry_reaches_a_shared_instance_after_origin_disconnects() {
    agent_exit_reentry(true, true, true);
}

fn agent_exit_reentry(wait_for_load: bool, disconnect_origin: bool, shared: bool) {
    let root = tempdir().unwrap();
    let source = root.path().join("reentry.scm");
    std::fs::write(
        &source,
        r#"
        (define text "loaded")
        (define count 0)
        (define (load config)
          (define payload-config (hash-remove config "shared"))
          (yinz-on-event "Hook"
            (lambda (event)
              (define payload (hash-ref event "payload"))
              (define code (hash-ref payload "agent_exit_code"))
              (set! count (+ count 1))
              (set! text (string-append
                (hash-ref event "point") ":" (number->string count) ":" code ":"
                (if (equal? payload (hash-insert payload-config "agent_exit_code" code))
                    "payload-ok" "payload-mismatch")))
              #t)))
        (define (render rows cols) text)
        "#,
    )
    .unwrap();
    let mut hook =
        yinz_runtime::plugin_location::run_plugin_from_url(&format!("file:{}", source.display()))
            .unwrap();
    if shared {
        hook = hook.with_configuration(BTreeMap::from([("shared".into(), "true".into())]));
    }
    let hook = RunPluginOrAlias::RunPlugin(hook);
    let (sender, receiver, teardown) = create_plugin_thread_with_hooks(
        Some(root.path().into()),
        None,
        None,
        yinz_types::input::hooks::Hooks::from_data(BTreeMap::from([(HookPoint::AgentExit, hook)])),
    );
    let instructions = Watched::new(vec![]);
    let logger = log_until_disconnect(&instructions, receiver);
    let client_id = ClientId::new(1);
    sender
        .send(PluginInstruction::AddClient(client_id))
        .unwrap();
    let other_client = ClientId::new(2);
    if !disconnect_origin {
        sender
            .send(PluginInstruction::AddClient(other_client))
            .unwrap();
    }
    let wait_for_clients = |clients: &[ClientId], text: &str| {
        instructions.wait_until(text, default_timeout(), |log| {
            let assets = rendered_assets(log);
            clients
                .iter()
                .all(|expected| {
                    assets
                        .iter()
                        .any(|(client, frame)| client == expected && frame.contains(text))
                })
                .then_some(())
        });
    };
    let fire = |code: &str| {
        let mut payload = agent_exit_payload();
        payload.insert("agent_exit_code".into(), code.into());
        sender
            .send(PluginInstruction::RunHook {
                point: HookPoint::AgentExit,
                pane_id: Some(PaneId::Terminal(TERMINAL_7)),
                payload: HookFiring::ReplacePane(payload),
                client_id,
            })
            .unwrap();
    };
    fire("0");
    if !wait_for_load {
        fire("0");
    }
    let plugin_id = instructions.wait_until("the hook pane", default_timeout(), |instructions| {
        hook_panes_opened(instructions).first().map(|(id, _)| *id)
    });
    // The real screen sizes the new pane; the host-only harness must do that too.
    sender
        .send(PluginInstruction::Resize(plugin_id, 80, 24))
        .unwrap();
    if wait_for_load {
        wait_for_render_containing(&instructions, "loaded");
        if disconnect_origin {
            sender
                .send(PluginInstruction::AddClient(other_client))
                .unwrap();
            if !shared {
                // Shared plugins reuse the loaded instance without rendering on attachment.
                wait_for_clients(&[other_client], "loaded");
            }
            sender
                .send(PluginInstruction::RemoveClient(client_id))
                .unwrap();
        } else {
            wait_for_clients(&[client_id, other_client], "loaded");
        }
        fire("0");
    }
    let recipients = if disconnect_origin {
        vec![other_client]
    } else {
        vec![client_id, other_client]
    };
    wait_for_clients(&recipients, "agent_exit:1:0:payload-ok");
    fire("17");
    wait_for_clients(&recipients, "agent_exit:2:17:payload-ok");
    fire("17");
    wait_for_clients(&recipients, "agent_exit:3:17:payload-ok");
    teardown();
    logger.join().unwrap();
    let instructions = instructions.lock();
    let opened = hook_panes_opened(&instructions);
    assert_eq!(opened.len(), 1, "{instructions:?}");
    assert_eq!(opened[0].1, PaneId::Terminal(TERMINAL_7));
    let rendered_ids: std::collections::HashSet<_> = instructions
        .iter()
        .filter_map(|instruction| match instruction {
            ScreenInstruction::PluginBytes(assets) => {
                Some(assets.iter().map(|asset| asset.plugin_id))
            },
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(rendered_ids, std::collections::HashSet::from([opened[0].0]));
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
    // shape agent produces continuously - it must not change the answer.
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let client_id = ClientId::new(1);

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
            pane_id: Some(PaneId::Terminal(crate::TerminalId::new(7))),
            payload: HookFiring::ReplacePane(agent_exit_payload()),
            client_id,
        });
    };

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    fire();
    // Waited for rather than slept past: the id of the first hook pane is what has to be
    // unloaded, and it is not known until the screen has been told about it.
    let first = received_screen_instructions.wait_until(
        "the first hook pane",
        yinz_test_support::default_timeout(),
        |instructions| hook_panes_opened(instructions).first().map(|(id, _)| *id),
    );
    // A lookup of the same plugin while the first hook is still up, between the two firings.
    let _ = plugin_thread_sender.send(PluginInstruction::CliPipe {
        focus_plugin: false,
        is_eof: false,
        pipe_id: "cache-warmer".to_owned(),
        name: "ping".to_owned(),
        payload: None,
        plugin: Some("yinz:agent-exit".to_owned()),
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
        vec![
            PaneId::Terminal(crate::TerminalId::new(7)),
            PaneId::Terminal(crate::TerminalId::new(7))
        ],
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
    let client_id = ClientId::new(1);
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
        .any(|instruction| {
            matches!(
                instruction,
                ScreenInstruction::RerunCommandPane(TERMINAL_7, _)
            )
        });
    assert!(
        rerun,
        "expected Enter to rerun pane 7, got: {:?}",
        received_screen_instructions.lock()
    );
}

#[test]
#[serial(plugin_tests)]
pub fn the_agent_exit_hook_closes_the_pane_on_escape() {
    // Escape closes the pane the hook stands in for, and only then itself: closing the suppressed
    // pane first is what stops it being restored when this one goes.
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let (plugin_thread_sender, pty_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_pty_receiver(Some(plugin_host_folder), None, None);
    let client_id = ClientId::new(1);
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
    received_screen_instructions.wait_until(
        "exit hook ready for Escape",
        yinz_test_support::default_timeout(),
        |instructions| (!rendered_output(instructions).is_empty()).then_some(()),
    );
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
            PtyInstruction::ClosePane(PaneId::Terminal(TERMINAL_7), _)
        )
    });
    assert!(
        closed,
        "expected Escape to close pane 7, got: {:?}",
        received_pty_instructions.lock()
    );
    let closed_in_screen = received_screen_instructions
        .lock()
        .iter()
        .any(|instruction| {
            matches!(
                instruction,
                ScreenInstruction::ClosePane(PaneId::Terminal(TERMINAL_7), ..)
            )
        });
    assert!(
        closed_in_screen,
        "expected Delete to close pane 7 in the screen too, got: {:?}",
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
const HEADLESS_HOOK_SCM: &str = r#"
(define ready (yinz-deferred))
(define (say event)
  (define point (hash-ref event "point"))
  (define payload (hash-ref event "payload"))
  (define value
    (cond ((hash-contains? payload "pane_id") (number->string (hash-ref payload "pane_id")))
          ((hash-contains? payload "session_name") (hash-ref payload "session_name"))
          (else "")))
  (define identity (if (equal? point "session_start") (string-append "@" (yinz-server-session)) ""))
  (yinz-command (hash "RenameTabWithId" (list 1 (string-append point ":" value identity)))))
(define (load config)
  (yinz-on-event "Hook" (lambda (event) (yinz-then ready (lambda (ignored) (say event)))))
  (yinz-resolve! ready (yinz-wait-for-permissions (list "ChangeApplicationState"))))
(define (render rows cols) "")
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
    std::fs::write(user_plugin_dir.join("hook.scm"), HEADLESS_HOOK_SCM).unwrap();
    RunPluginOrAlias::RunPlugin(
        yinz_runtime::plugin_location::run_plugin_from_url(&format!(
            "file:{}",
            user_plugin_dir.join("hook.scm").display()
        ))
        .unwrap(),
    )
}

pub(super) fn bind_hook(
    plugin_thread_sender: &SenderWithContext<PluginInstruction>,
    point: HookPoint,
    plugin: RunPluginOrAlias,
) {
    let _ = plugin_thread_sender.send(PluginInstruction::Reconfigure {
        plugin_aliases: Default::default(),
        client_id: crate::ClientId::new(1),
        keybinds: None,
        default_mode: None,
        default_shell: None,
        layout_dir: None,
        user_plugin_dir: None,
        hooks: yinz_types::input::hooks::Hooks::from_data(BTreeMap::from([(point, plugin)])),
        context_menu: Default::default(),
        was_written_to_disk: false,
    });
}

pub(super) fn pane_exit_payload(pane_id: u32) -> HookFiring {
    HookFiring::Headless(HookPayload::PaneExit {
        pane_id: crate::TerminalId::new(pane_id),
        exit_code: Some(0),
        cwd: None,
        command: None,
        held: true,
    })
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
    let client_id = ClientId::new(1);

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
        pane_id: Some(PaneId::Terminal(crate::TerminalId::new(7))),
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
    let client_id = ClientId::new(1);

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
        pane_id: Some(PaneId::Terminal(crate::TerminalId::new(7))),
        payload: pane_exit_payload(7),
        client_id,
    });
    // Once the first firing has been answered the plugin is up and granted; the second one
    // must reach the same instance rather than load another.
    received_screen_instructions.wait_until(
        "the first firing to be named",
        yinz_test_support::default_timeout(),
        |instructions| (!tab_names_set(instructions).is_empty()).then_some(()),
    );
    let _ = plugin_thread_sender.send(PluginInstruction::RunHook {
        point: HookPoint::PaneExit,
        pane_id: Some(PaneId::Terminal(crate::TerminalId::new(8))),
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
    let client_id = ClientId::new(1);

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::RunHook {
        point: HookPoint::AgentExit,
        pane_id: None,
        payload: HookFiring::ReplacePane(agent_exit_payload()),
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
    let hooks = yinz_types::input::hooks::Hooks::from_data(BTreeMap::from([(
        HookPoint::SessionStart,
        headless_hook_plugin(user_plugin_dir.path()),
    )]));
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread_with_hooks(Some(plugin_host_folder), None, None, hooks);
    let client_id = ClientId::new(1);
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

    // The hook reports the payload's session name, which the plugin thread's handle supplies,
    // and after the `@` the plugin's own `yinz-server-session`, which reaches the Steel runtime
    // through `PluginBridge`, `PluginLoader` and `PluginEnv`. Both must carry the harness's name.
    let instructions = received_screen_instructions.lock();
    assert_eq!(
        tab_names_set(&instructions),
        vec!["session_start:yinz-test@yinz-test".to_owned()],
        "the harness names its session yinz-test, got: {instructions:?}"
    );
}

/// A client attaching while a headless hook is still pending - here, waiting on the permission
/// prompt its `load` raised - used to lose the hook's first firing: `add_client` provisions an
/// instance for the new client and started the plugin's event cache afresh, throwing away the
/// firing that had loaded the hook. The cache is kept across that now, and the firing reaches
/// the hook once the permission is granted.
#[test]
#[serial(plugin_tests)]
pub fn a_client_attaching_while_a_hook_waits_on_its_permission_does_not_lose_the_firing() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let user_plugin_dir = tempdir().unwrap();
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);

    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = std::thread::Builder::new()
        .name("fake_screen_thread".to_string())
        .spawn({
            let log = received_screen_instructions.clone();
            let plugin_thread_sender = plugin_thread_sender.clone();
            let cache_path = cache_path.clone();
            let mut permission_requests = 0;
            move || loop {
                let (instruction, _err_ctx) =
                    recv_from_fake_screen(&screen_receiver, "the hook's rename", "1");
                match instruction {
                    ScreenInstruction::RequestPluginPermissions(_plugin_id, plugin_permission) => {
                        permission_requests += 1;
                        if permission_requests == 1 {
                            // A second client attaches while the hook is on its prompt: the
                            // hook is in the plugin map, so `add_client` provisions an
                            // instance for the newcomer - and, before the fix, reset the cache
                            // holding the firing below.
                            let _ = plugin_thread_sender
                                .send(PluginInstruction::AddClient(crate::ClientId::new(2)));
                        }
                        let _ = plugin_thread_sender.send(PluginInstruction::PermissionAnswer {
                            request_id: plugin_permission.request_id,
                            status: PermissionStatus::Granted,
                            cache_path: Some(cache_path.clone()),
                        });
                    },
                    ScreenInstruction::RenameTabWithId(..) => {
                        log.lock().push(instruction);
                        break;
                    },
                    other => log.lock().push(other),
                }
            }
        })
        .unwrap();

    bind_hook(
        &plugin_thread_sender,
        HookPoint::PaneExit,
        headless_hook_plugin(user_plugin_dir.path()),
    );
    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(crate::ClientId::new(1)));
    let _ = plugin_thread_sender.send(PluginInstruction::RunHook {
        point: HookPoint::PaneExit,
        pane_id: Some(PaneId::Terminal(crate::TerminalId::new(7))),
        payload: pane_exit_payload(7),
        client_id: crate::ClientId::new(1),
    });
    screen_thread.join().unwrap();
    teardown();

    let instructions = received_screen_instructions.lock();
    assert!(
        tab_names_set(&instructions).contains(&"pane_exit:7".to_owned()),
        "the firing that loaded the hook must survive a client attaching mid-load, got: \
         {instructions:?}"
    );
}

/// Two firings within one load's time reach one instance, in order. Before the fix the second
/// firing found no instance in the plugin map - the first was still loading - and loaded
/// another, so a hook's firings were split across two plugins and neither saw them all.
///
/// Another plugin is loaded first because the lookup that decides this reads a cached copy of
/// the plugin map, warmed on first use: with nothing loaded the copy stays empty, every lookup
/// takes the map's lock instead, and the loader - which holds that lock for the whole load -
/// makes the second firing wait until the instance is there. A session has its bar plugins,
/// so the copy is warm and stale, and the second firing sees nothing.
#[test]
#[serial(plugin_tests)]
pub fn a_firing_during_the_headless_hooks_load_reaches_the_loading_instance() {
    let temp_folder = tempdir().unwrap();
    let plugin_host_folder = PathBuf::from(temp_folder.path());
    let cache_path = plugin_host_folder.join("permissions_test.kdl");
    let user_plugin_dir = tempdir().unwrap();
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread(Some(plugin_host_folder), None);
    let client_id = ClientId::new(1);

    // Grants by the requesting plugin's id: the macro answers plugin 0, which is the other
    // plugin here.
    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = std::thread::Builder::new()
        .name("fake_screen_thread".to_string())
        .spawn({
            let log = received_screen_instructions.clone();
            let plugin_thread_sender = plugin_thread_sender.clone();
            let cache_path = cache_path.clone();
            let mut renames = 0;
            move || loop {
                let (instruction, _err_ctx) = recv_from_fake_screen(&screen_receiver, renames, 2);
                match instruction {
                    ScreenInstruction::RequestPluginPermissions(_plugin_id, plugin_permission) => {
                        let _ = plugin_thread_sender.send(PluginInstruction::PermissionAnswer {
                            request_id: plugin_permission.request_id,
                            status: PermissionStatus::Granted,
                            cache_path: Some(cache_path.clone()),
                        });
                    },
                    ScreenInstruction::RenameTabWithId(..) => {
                        renames += 1;
                        log.lock().push(instruction);
                        if renames == 2 {
                            break;
                        }
                    },
                    other => log.lock().push(other),
                }
            }
        })
        .unwrap();

    let _ = plugin_thread_sender.send(PluginInstruction::AddClient(client_id));
    let _ = plugin_thread_sender.send(PluginInstruction::Load(
        Some(false),
        false,
        false,
        Some("some_other_plugin".to_owned()),
        script_run_plugin(BTreeMap::new()),
        Some(TabId::new(1)),
        None,
        client_id,
        Size {
            cols: 121,
            rows: 20,
        },
        None,
        None,
        false,
        None,
        None,
        None,
        None,
    ));
    received_screen_instructions.wait_until(
        "the other plugin to be in the plugin map",
        yinz_test_support::default_timeout(),
        |instructions| {
            instructions
                .iter()
                .any(|instruction| matches!(instruction, ScreenInstruction::AddPlugin(..)))
                .then_some(())
        },
    );

    bind_hook(
        &plugin_thread_sender,
        HookPoint::PaneExit,
        headless_hook_plugin(user_plugin_dir.path()),
    );
    // Back to back: the second is read off the queue while the first's load is in flight.
    for pane_id in [7, 8] {
        let _ = plugin_thread_sender.send(PluginInstruction::RunHook {
            point: HookPoint::PaneExit,
            pane_id: Some(PaneId::Terminal(crate::TerminalId::new(pane_id))),
            payload: pane_exit_payload(pane_id),
            client_id,
        });
    }
    screen_thread.join().unwrap();
    teardown();

    let instructions = received_screen_instructions.lock();
    assert_eq!(
        tab_names_set(&instructions),
        vec!["pane_exit:7".to_owned(), "pane_exit:8".to_owned()],
        "both firings, in order, got: {instructions:?}"
    );
    assert_eq!(
        suppressed_plugins_loaded(&instructions).len(),
        1,
        "one instance serves both firings, got: {instructions:?}"
    );
}
