use super::*;
use crate::test_env::test_env;
use std::collections::HashSet;
use std::sync::Mutex;
use strum::IntoEnumIterator;
use yinz_types::{
    data::{EventType, PermissionStatus, PermissionType, PluginTag},
    input::layout::RunPluginLocation,
    internal_ids::{ContextMenuItemId, ContextMenuRequestId, TabPosition},
    keys::{BareKey, KeyWithModifier},
};

fn load_builtin(name: &str, source: &str, mut env: PluginEnv) -> Result<DenoPlugin> {
    env.plugin_config.location = RunPluginLocation::Yinz(PluginTag::new(name));
    env.data_owner = url::Url::from(&env.plugin_config.location);
    DenoPlugin::new(
        source,
        Path::new("/nonexistent/builtin.ts"),
        env,
        Arc::default(),
    )
}

/// The context-menu hook has no `render`, so a load-and-draw check would prove nothing about it.
/// What it does instead is transform an event into an answer, and that is what is exercised here:
/// a syntax error, a bad field name or a broken filter would otherwise go unnoticed until somebody
/// right-clicked.
#[test]
fn agent_paste_helpers_require_only_write_to_stdin_permission() {
    let directory = tempfile::tempdir().unwrap();
    let mut env = test_env();
    env.permissions = Arc::new(Mutex::new(Some(HashSet::from([
        PermissionType::WriteToStdin,
    ]))));
    let mut plugin = DenoPlugin::new(
        r#"
        import z from 'yinz:plugin';
        let state = 'starting';
        export default {
            load() {
                z.agent.send(7, 'draft', {enter:false});
                z.agent.prompt(7, 'go').catch(error => { state = String(error); });
                state = 'waiting for completion';
            },
            render() { return state; }
        };
    "#,
        &directory.path().join("paste.ts"),
        env,
        Arc::default(),
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    plugin.tick().unwrap();
    assert_eq!(frame(&mut plugin, 10, 80), "waiting for completion");
}

#[test]
fn the_shipped_context_menu_hook_filters_the_items_it_is_given() {
    use yinz_bus::screen::ScreenInstruction;
    use yinz_runtime::channels::SenderWithContext;
    use yinz_types::diagnostics::ErrorContext;
    use {
        yinz_types::data::ContextMenuContext, yinz_types::data::ContextMenuItem,
        yinz_types::data::ContextMenuLink,
    };

    let source =
        yinz_deno_builtins::builtin_deno_plugin("context-menu").expect("the hook is a builtin");
    let configured = vec![
        ContextMenuItem::new("Copy", vec![]).with_id(ContextMenuItemId::new(0)),
        ContextMenuItem::separator().with_id(ContextMenuItemId::new(1)),
        ContextMenuItem::new("Close pane", vec![]).with_id(ContextMenuItemId::new(2)),
    ];

    // The answer goes to the screen thread, so the env gets a real channel rather than the
    // silently-failing default every other test here uses.
    let answer_for = |mut context: ContextMenuContext| -> Vec<ContextMenuItem> {
        context.request_id = ContextMenuRequestId::new(73);
        let expected_request = context.request_id;
        let (to_screen, screen_receiver) =
            yinz_runtime::channels::unbounded::<(ScreenInstruction, ErrorContext)>();
        let mut env = test_env();
        env.senders.to_screen = Some(SenderWithContext::new(to_screen));

        let mut plugin = load_builtin("context-menu", source, env).unwrap();
        plugin.load(&BTreeMap::new()).unwrap();
        plugin
            .update(&Event::ContextMenuRequested(Box::new(context)))
            .expect("the hook should answer");

        screen_receiver
            .try_iter()
            .find_map(|(instruction, _)| match instruction {
                ScreenInstruction::SetContextMenu {
                    items, request_id, ..
                } => {
                    assert_eq!(request_id, expected_request);
                    Some(items)
                },
                _ => None,
            })
            .expect("the hook answers with a menu")
    };

    // No selection: "Copy" would copy nothing, so it is not offered.
    let without_selection = answer_for(ContextMenuContext {
        items: configured.clone(),
        ..Default::default()
    });
    assert!(
        !without_selection.iter().any(|item| item.label == "Copy"),
        "got: {without_selection:?}"
    );
    assert!(without_selection
        .iter()
        .any(|item| item.label == "Close pane"));

    // With one, it is - and it says what it would copy.
    let with_selection = answer_for(ContextMenuContext {
        items: configured.clone(),
        selected_text: Some("hello".to_owned()),
        ..Default::default()
    });
    assert!(
        with_selection
            .iter()
            .any(|item| item.label == r#"Copy "hello""#),
        "got: {with_selection:?}"
    );
    assert!(
        with_selection
            .iter()
            .find(|item| item.label.starts_with("Copy"))
            .and_then(|item| item.id)
            == Some(ContextMenuItemId::new(0)),
        "the id has to survive the relabelling, or the actions do not come back"
    );

    let unicode = answer_for(ContextMenuContext {
        items: configured.clone(),
        selected_text: Some(format!("  {}\n", "🙂".repeat(25))),
        ..Default::default()
    });
    assert_eq!(unicode[0].label, format!("Copy \"{}…\"", "🙂".repeat(24)));
    assert!(answer_for(ContextMenuContext {
        items: vec![ContextMenuItem::separator(), ContextMenuItem::separator()],
        ..Default::default()
    })
    .is_empty());

    // Filtering "Copy" out would otherwise strand the rule that used to sit under it, leaving a
    // menu whose first row is a horizontal line.
    assert!(
        !without_selection[0].separator,
        "a menu must not lead with a separator: {without_selection:?}"
    );
    assert!(
        !without_selection.last().unwrap().separator,
        "nor trail with one: {without_selection:?}"
    );
    assert!(
        !without_selection
            .windows(2)
            .any(|pair| pair[0].separator && pair[1].separator),
        "nor have two in a row: {without_selection:?}"
    );

    // A link under the cursor leads, and is an item the plugin made up rather than a configured one.
    let with_link = answer_for(ContextMenuContext {
        items: configured.clone(),
        link: Some(ContextMenuLink {
            matched: "https://example.com".to_owned(),
            ..Default::default()
        }),
        ..Default::default()
    });
    assert_eq!(with_link[0].label, "Open https://example.com");
    assert_eq!(with_link[0].id, None);
    assert_eq!(
        with_link[0].context.get("url").map(|s| s.as_str()),
        Some("https://example.com")
    );
}

/// The shipped hook opens a link with whichever of `xdg-open`/`open` the machine actually has,
/// which it learns by probing both at load. Worth a test of its own: the probe answers
/// asynchronously, so a selection made before it lands has to wait rather than be dropped, and
/// nothing about either path is visible until somebody right-clicks a URL on a Mac.
#[test]
fn the_shipped_context_menu_hook_opens_a_link_with_the_opener_the_machine_has() {
    use yinz_bus::background_jobs::BackgroundJob;
    use yinz_runtime::channels::SenderWithContext;
    use yinz_types::data::ContextMenuContext;
    use yinz_types::diagnostics::ErrorContext;

    let source =
        yinz_deno_builtins::builtin_deno_plugin("context-menu").expect("the hook is a builtin");
    let (to_background_jobs, background_jobs) =
        yinz_runtime::channels::unbounded::<(BackgroundJob, ErrorContext)>();
    let mut env = test_env();
    env.senders.to_background_jobs = Some(SenderWithContext::new(to_background_jobs));

    let mut plugin = load_builtin("context-menu", source, env).unwrap();
    plugin.load(&BTreeMap::new()).unwrap();

    assert!(plugin.render(1, 80).unwrap().is_empty());
    let subscriptions = plugin.env().subscriptions.lock().unwrap().clone();
    assert_eq!(
        subscriptions,
        [
            EventType::ContextMenuRequested,
            EventType::ContextMenuItemSelected,
            EventType::RunCommandResult
        ]
        .into_iter()
        .collect()
    );

    let commands = |receiver: &yinz_runtime::channels::Receiver<(BackgroundJob, ErrorContext)>| {
        receiver
            .try_iter()
            .filter_map(|(job, _)| match job {
                BackgroundJob::RunCommand(_, _, command, args, env, _, _, cap, _, _) => {
                    assert!(env.is_empty());
                    assert_eq!(cap, Some(64));
                    Some((command, args))
                },
                _ => None,
            })
            .collect::<Vec<_>>()
    };

    // Both candidates are probed at load, so the answer is there before the first selection.
    let probes = commands(&background_jobs);
    assert_eq!(
        probes,
        vec![
            ("xdg-open".to_owned(), vec!["--help".to_owned()]),
            ("open".to_owned(), vec!["--help".to_owned()]),
        ],
        "both openers are probed"
    );

    let select = |plugin: &mut DenoPlugin| {
        plugin
            .update(&Event::ContextMenuItemSelected {
                label: "Open https://example.com".to_owned(),
                item_context: [
                    ("action".to_owned(), "open_link".to_owned()),
                    ("url".to_owned(), "https://example.com".to_owned()),
                ]
                .into_iter()
                .collect(),
                context: Box::new(ContextMenuContext::default()),
            })
            .expect("the hook should take the selection");
    };

    // A selection before either probe answers is held, not dropped and not guessed at.
    select(&mut plugin);
    assert!(
        commands(&background_jobs).is_empty(),
        "nothing is run until an opener is known"
    );

    // This machine turns out to be a Mac: `xdg-open` is absent, `open` is not.
    let probe_result = |executable: &str, exit_code: Option<i32>| {
        Event::RunCommandResult(
            exit_code,
            vec![],
            vec![],
            [("probe".to_owned(), executable.to_owned())]
                .into_iter()
                .collect(),
        )
    };
    plugin.update(&probe_result("xdg-open", Some(1))).unwrap();
    assert!(
        commands(&background_jobs).is_empty(),
        "a probe that failed is not an opener"
    );
    plugin.update(&probe_result("open", Some(0))).unwrap();
    assert_eq!(
        commands(&background_jobs),
        vec![("open".to_owned(), vec!["https://example.com".to_owned()])],
        "the held url is opened as soon as an opener is known"
    );

    // xdg-open wins even when open's successful result arrived first.
    plugin.update(&probe_result("xdg-open", Some(0))).unwrap();
    plugin.update(&probe_result("open", Some(0))).unwrap();
    select(&mut plugin);
    assert_eq!(
        commands(&background_jobs),
        vec![(
            "xdg-open".to_owned(),
            vec!["https://example.com".to_owned()]
        )]
    );

    // ...and from here on it goes straight out.
    select(&mut plugin);
    assert_eq!(
        commands(&background_jobs),
        vec![(
            "xdg-open".to_owned(),
            vec!["https://example.com".to_owned()]
        )]
    );
}

#[test]
fn embedded_loading_imports_diagnostics_and_compiler_recovery() {
    for import in [
        "./sibling.ts",
        "../escape.ts",
        "file:///tmp/a.ts",
        "/tmp/a.ts",
        "npm:pkg",
        "pkg",
        "https://example.com/a.ts",
    ] {
        let source = format!("import {import:?}; export default {{}};");
        let error = load_builtin("broken", &source, test_env()).err().unwrap();
        let error = format!("{error:#}");
        assert!(
            error.contains(import) && error.contains("yinz-builtin:///broken.ts"),
            "{error}"
        );
    }
    let error = load_builtin("broken", "const x: = ;", test_env())
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("broken.ts"));
    let mut healthy = load_builtin("healthy", "import z from 'yinz:plugin'; const n: number = 42; export default {render() {return String(n);}};", test_env()).unwrap();
    assert_eq!(healthy.render(1, 80).unwrap(), b"42");
    let mut throwing = load_builtin(
        "throws",
        "const n: number = 42;\nexport default {render() {throw Error('mapped');}};",
        test_env(),
    )
    .unwrap();
    let error = format!("{:#}", throwing.render(1, 80).unwrap_err());
    assert!(error.contains("throws.ts:2"), "{error}");
}

#[test]
fn deno_preserves_server_identity_but_reload_replaces_instance_identity() {
    let source = r#"
const identity = [yinz.serverBinary, yinz.serverSession, yinz.serverIncarnation, yinz.instanceId];
if (!Object.isFrozen(yinz)) throw Error('SDK exposure');
let refused = false;
try { yinz.command('GetPluginIds'); } catch (_) { refused = true; }
if (!refused) throw Error('initialization commands allowed');
try { yinz.serverIncarnation = 'forged'; } catch (_) {}
if (identity[2] !== yinz.serverIncarnation) throw Error('mutable metadata');
export default {render() {return JSON.stringify(identity);}};
"#;
    let env = || {
        let mut env = test_env();
        env.server_session = crate::ServerSession::Fixed("identity-session".into());
        env
    };
    let first = {
        let mut plugin = load_builtin("identity", source, env()).unwrap();
        serde_json::from_slice::<Value>(&plugin.render(1, 1000).unwrap()).unwrap()
    };
    let mut reloaded = load_builtin("identity", source, env()).unwrap();
    let second: Value = serde_json::from_slice(&reloaded.render(1, 1000).unwrap()).unwrap();
    assert_eq!(first[1], "identity-session");
    for i in 0..3 {
        assert_eq!(first[i], second[i]);
    }
    assert_ne!(first[3], second[3]);
    for value in [&first[2], &first[3], &second[3]] {
        uuid::Uuid::parse_str(value.as_str().unwrap()).unwrap();
    }
}

fn grant_requested_permissions(plugin: &mut DenoPlugin) {
    let workers = plugin.env().deno_workers.clone().unwrap();
    let requested = workers.permissions.lock().unwrap().requested();
    if let Some(requested) = requested {
        workers.permission_answer(plugin.env(), &requested, PermissionStatus::Granted);
        plugin.tick().unwrap();
    }
    plugin
        .update(&Event::PermissionRequestResult(PermissionStatus::Granted))
        .unwrap();
}

#[test]
fn every_embedded_typescript_builtin_loads_and_renders() {
    for name in yinz_runtime::consts::BUILTIN_DENO_PLUGIN_NAMES {
        let source = yinz_deno_builtins::builtin_deno_plugin(name).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut env = test_env();
        env.server_session = crate::ServerSession::Fixed("builtins".into());
        env.data_access = Arc::new(yinz_storage::data_access::DataAccess::sqlite(
            directory.path(),
        ));
        env.plugin_own_data_dir = directory.path().to_owned();
        env.plugin_own_cache_dir = directory.path().to_owned();
        *env.permissions.lock().unwrap() = Some(PermissionType::iter().collect());
        let mut plugin =
            load_builtin(name, source, env).unwrap_or_else(|error| panic!("{name}: {error:#}"));
        plugin.load(&BTreeMap::new()).unwrap();
        grant_requested_permissions(&mut plugin);
        plugin
            .render(24, 80)
            .unwrap_or_else(|error| panic!("{name}: {error:#}"));
    }
}

#[test]
fn embedded_builtin_loads_without_a_working_directory() {
    const CHILD: &str = "YINZ_TEST_EMBEDDED_NO_CWD";
    if std::env::var_os(CHILD).is_none() {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "deno::builtin_tests::embedded_builtin_loads_without_a_working_directory",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    // Only the isolated child changes its cwd; other tests can keep using relative paths.
    let directory = tempfile::tempdir().unwrap();
    std::env::set_current_dir(directory.path()).unwrap();
    directory.close().unwrap();
    assert!(std::env::current_dir().is_err());
    let source = yinz_deno_builtins::builtin_deno_plugin("context-menu").unwrap();
    let mut plugin = load_builtin("context-menu", source, test_env()).unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    assert!(plugin.render(1, 80).unwrap().is_empty());
}

#[test]
fn shipped_context_menu_passes_the_typescript_checker() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../yinz-deno-builtins/assets/plugins/context-menu.ts");
    check_plugin(&path).unwrap();
}

/// The agent-policy hook is headless: a syntax error in it would surface the first time an agent
/// stopped for a question, which is the moment its notification is for. So it is loaded and
/// fired at here, and what is checked is its one visible act - the notification, addressed to
/// the pane that is waiting - and that the other points it may be bound to stay quiet.
#[test]
fn the_shipped_agent_policy_hook_notifies_when_an_agent_waits() {
    use yinz_bus::screen::ScreenInstruction;
    use yinz_runtime::channels::SenderWithContext;
    use yinz_types::diagnostics::ErrorContext;
    use yinz_types::input::hooks::HookPoint;
    use yinz_types::pane_id::PaneId;

    let source =
        yinz_deno_builtins::builtin_deno_plugin("agent-policy").expect("the hook is a builtin");
    let (to_screen, screen_receiver) =
        yinz_runtime::channels::unbounded::<(ScreenInstruction, ErrorContext)>();
    let mut env = test_env();
    env.senders.to_screen = Some(SenderWithContext::new(to_screen));

    let mut plugin = load_builtin("agent-policy", source, env).unwrap();
    plugin.load(&BTreeMap::new()).unwrap();

    let notifications =
        |receiver: &yinz_runtime::channels::Receiver<(ScreenInstruction, ErrorContext)>| {
            receiver
                .try_iter()
                .filter_map(|(instruction, _)| match instruction {
                    ScreenInstruction::PluginNotify { text, pane_id, .. } => Some((text, pane_id)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

    use yinz_types::agent::AgentState;
    let payload = |state: AgentState, detail: Option<&str>| yinz_types::data::HookPayload::Agent {
        pane_id: yinz_types::internal_ids::TerminalId::new(7),
        agent: yinz_types::agent::AgentInfo {
            kind: "claude".to_owned(),
            state,
            source: yinz_types::agent::AgentSource::Reported,
            detail: detail.map(str::to_owned),
            waiting_on: None,
            wait_needs_a_person: state == AgentState::Waiting,
            cwd: Some("/work".to_owned()),
            session_id: None,
            run_id: None,
        },
        previous_state: None,
        command: None,
    };

    plugin
        .update(&Event::Hook {
            point: HookPoint::AgentStart,
            payload: payload(AgentState::Busy, None),
        })
        .unwrap();
    assert!(
        notifications(&screen_receiver).is_empty(),
        "an agent starting is logged, not announced"
    );

    plugin
        .update(&Event::Hook {
            point: HookPoint::AgentWaiting,
            payload: payload(AgentState::Waiting, Some("Allow Bash(cargo test)?")),
        })
        .unwrap();
    assert_eq!(
        notifications(&screen_receiver),
        vec![(
            "claude in pane 7: Allow Bash(cargo test)?".to_owned(),
            Some(PaneId::Terminal(crate::TerminalId::new(7))),
        )],
        "a waiting agent is announced, and the toast focuses its pane"
    );

    plugin
        .update(&Event::Hook {
            point: HookPoint::AgentWaiting,
            payload: payload(AgentState::Waiting, None),
        })
        .unwrap();
    assert_eq!(
        notifications(&screen_receiver),
        vec![(
            "claude in pane 7: is waiting for you".to_owned(),
            Some(PaneId::Terminal(crate::TerminalId::new(7))),
        )],
        "a wait with no detail still says who is waiting"
    );
}

#[test]
fn typescript_reference_examples_load_and_render_in_deno() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../example-plugins");
    let store = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("ts") {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        let mut env = test_env();
        env.data_access = Arc::new(yinz_storage::data_access::DataAccess::sqlite(store.path()));
        let mut plugin = load_builtin("example", &source, env)
            .unwrap_or_else(|error| panic!("{}: {error:#}", path.display()));
        plugin.load(&BTreeMap::new()).unwrap();
        plugin
            .render(10, 80)
            .unwrap_or_else(|error| panic!("{}: {error:#}", path.display()));
    }
}

// Press every key an example advertises, so incorrect command payloads fail here.

fn example(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../example-plugins")
            .join(name),
    )
    .expect("the example plugin should be in the repo")
}

/// An example loaded as the user plugin it is - not as a builtin, which is granted everything
/// without asking and would make every permission assertion vacuous.
fn load_example(name: &str, permissions: HashSet<PermissionType>) -> DenoPlugin {
    let mut env = test_env();
    env.permissions = Arc::new(Mutex::new(Some(permissions)));
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../example-plugins")
        .join(name);
    let mut plugin = DenoPlugin::new(&example(name), &path, env, Arc::default()).unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    plugin
}

fn press(plugin: &mut DenoPlugin, key: char) {
    plugin
        .update(&Event::Key(KeyWithModifier::new(BareKey::Char(key))))
        .unwrap_or_else(|error| panic!("pressing {key:?} should not fail the plugin: {error:#}"));
}

fn frame(plugin: &mut DenoPlugin, rows: usize, cols: usize) -> String {
    String::from_utf8(plugin.render(rows, cols).unwrap()).unwrap()
}

#[test]
fn the_typescript_playground_example_runs_every_key_it_advertises() {
    let mut plugin = load_example(
        "playground.ts",
        PermissionType::iter()
            .filter(|permission| permission != &PermissionType::WriteToClipboard)
            .collect(),
    );
    assert!(frame(&mut plugin, 20, 60).contains("yinz TypeScript playground"));
    for key in ['i', 'v', 'p', 'd', 't', 'r', 'n', 'c', 'w', '+', '-', 'x'] {
        press(&mut plugin, key);
    }
    // `x` is the last key pressed, and the one the plugin deliberately has no permission for.
    let rendered = frame(&mut plugin, 20, 70);
    assert!(
        rendered.contains("x failed") && rendered.contains("WriteToClipboard"),
        "the unpermitted key should report the permission it wants, got: {rendered}"
    );
}

#[test]
fn the_typescript_dashboard_example_renders_tab_and_pane_updates_and_takes_input() {
    use yinz_types::data::{PaneInfo, PaneManifest, TabInfo};
    let mut plugin = load_example("dashboard.ts", PermissionType::iter().collect());
    grant_requested_permissions(&mut plugin);
    plugin
        .update(&Event::TabUpdate(vec![TabInfo {
            position: TabPosition::new(0),
            name: "main".to_owned(),
            active: true,
            ..Default::default()
        }]))
        .unwrap();
    let mut manifest = PaneManifest::default();
    manifest.panes.insert(
        TabPosition::new(0),
        vec![PaneInfo {
            id: yinz_types::pane_id::PaneId::from_legacy(3, false),
            title: "日本 shell".to_owned(),
            ..Default::default()
        }],
    );
    plugin.update(&Event::PaneUpdate(manifest)).unwrap();
    plugin
        .update(&Event::RunCommandResult(
            Some(0),
            b"up 3 days\n".to_vec(),
            vec![],
            BTreeMap::new(),
        ))
        .unwrap();
    plugin
        .update(&Event::Timer(yinz_types::units::FractionalSeconds::new(
            0.0,
        )))
        .unwrap();
    for key in ['j', 'k', 'x'] {
        press(&mut plugin, key);
    }
    plugin
        .update(&Event::Mouse(yinz_types::data::Mouse::LeftClick(3, 1)))
        .unwrap();
    let rendered = frame(&mut plugin, 20, 60);
    let codes = |text: &str| {
        text.bytes()
            .map(|b| b.to_string())
            .collect::<Vec<_>>()
            .join(",")
    };
    assert!(rendered.contains(&codes("main")), "{rendered:?}");
    assert!(rendered.contains(&codes("日本 shell")), "{rendered:?}");
    assert!(rendered.contains(&codes("clicked row 1")), "{rendered:?}");
}

#[test]
fn the_typescript_forms_example_routes_keys_paste_and_mouse() {
    let mut plugin = load_example("forms.ts", PermissionType::iter().collect());
    let initial = frame(&mut plugin, 24, 60);
    assert!(!initial.is_empty());
    plugin.update(&Event::PastedText("日本語".into())).unwrap();
    assert_ne!(
        frame(&mut plugin, 24, 60),
        initial,
        "pasting into the focused input redraws"
    );
    for key in [
        BareKey::Tab,
        BareKey::Enter,
        BareKey::Down,
        BareKey::Enter,
        BareKey::Esc,
        BareKey::PageDown,
        BareKey::PageUp,
    ] {
        plugin
            .update(&Event::Key(KeyWithModifier::new(key)))
            .unwrap_or_else(|error| panic!("{key:?}: {error:#}"));
    }
    plugin
        .update(&Event::Mouse(yinz_types::data::Mouse::LeftClick(2, 4)))
        .unwrap();
    plugin
        .update(&Event::Mouse(yinz_types::data::Mouse::ScrollDown(1)))
        .unwrap();
    assert!(!frame(&mut plugin, 24, 60).is_empty());
}

/// The Agent todo example reads a project list out of the shared namespace another plugin
/// declared, on the real SQLite store. See
/// `the_todos_example_reads_a_project_list_out_of_the_shared_namespace`.
#[test]
fn the_typescript_todos_example_reads_a_project_list_out_of_the_shared_namespace() {
    use yinz_storage::data_access::DataAccess;
    use yinz_types::data::{AttrDef, EntityDef, Mutation};
    let store_dir = tempfile::tempdir().unwrap();
    let data_access = Arc::new(DataAccess::sqlite(store_dir.path()));
    let store = data_access.plugin_data();
    store
        .define_entity(
            "shared:yinz",
            "yinz:project-store",
            &EntityDef::new("project", 1)
                .attr(AttrDef::text("name").required())
                .attr(AttrDef::text("root").indexed().required()),
            false,
        )
        .unwrap();
    store
        .define_entity(
            "shared:yinz",
            "yinz:project-store",
            &EntityDef::new("todo", 1)
                .attr(AttrDef::text("root").indexed().required())
                .attr(AttrDef::int("parent_id"))
                .attr(AttrDef::text("text").full_text().required())
                .attr(AttrDef::boolean("done").indexed().required().default(false)),
            false,
        )
        .unwrap();
    store
        .apply(
            "shared:yinz",
            &[
                Mutation::put("project")
                    .uid("work")
                    .set("name", "work")
                    .set("root", ".")
                    .build(),
                Mutation::put("project")
                    .uid("everything")
                    .set("name", "everything")
                    .set("root", "")
                    .build(),
            ],
        )
        .unwrap();
    let parent = store
        .apply(
            "shared:yinz",
            &[Mutation::put("todo")
                .set("root", ".")
                .set("text", "ship it")
                .set("done", false)
                .build()],
        )
        .unwrap()
        .report
        .ids[0];
    store
        .apply(
            "shared:yinz",
            &[Mutation::put("todo")
                .set("root", ".")
                .set("parent_id", parent)
                .set("text", "write the test")
                .set("done", true)
                .build()],
        )
        .unwrap();
    let mut env = test_env();
    env.permissions = Arc::new(Mutex::new(Some(PermissionType::iter().collect())));
    env.data_access = data_access;
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../example-plugins/todos.ts");
    let mut plugin = DenoPlugin::new(&example("todos.ts"), &path, env, Arc::default()).unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    grant_requested_permissions(&mut plugin);
    let rendered = frame(&mut plugin, 10, 60);
    let codes = |text: &str| {
        text.bytes()
            .map(|b| b.to_string())
            .collect::<Vec<_>>()
            .join(",")
    };
    assert!(rendered.contains(&codes("work todos")), "{rendered}");
    assert!(
        rendered.contains(&format!(";{};", codes("- ship it"))),
        "{rendered}"
    );
    let child = rendered
        .split(';')
        .find(|item| item.contains(&codes("x write the test")))
        .unwrap_or_else(|| panic!("no child item in: {rendered}"));
    assert!(child.starts_with('|'), "not indented: {child}");
}

#[test]
fn notification_observation_requires_read_application_state_and_requests_are_addressed_consent() {
    use yinz_types::notifications::*;
    let mut env = test_env();
    let original = NotificationData {
        id: yinz_types::internal_ids::NotificationId::new(0),
        received_at_unix_ms: yinz_types::units::UnixMillis::new(1),
        source: NotificationSource::Toast,
        pane_id: None,
        plugin_id: None,
        title: String::new(),
        body: "body".into(),
        delivery: NotificationDelivery::Toast,
    };
    let received = Event::NotificationReceived(original.clone());
    let resolved = Event::NotificationResolved(NotificationResolution::from_decision(
        original.clone(),
        NotificationDecision::Pass,
        None,
    ));
    env.permissions = Arc::new(Mutex::new(None));
    for event in [&received, &resolved] {
        assert_eq!(
            crate::wasm_bridge::check_event_permission(&env, event),
            (
                PermissionStatus::Denied,
                Some(PermissionType::ReadApplicationState)
            )
        );
    }
    assert_eq!(
        crate::wasm_bridge::check_event_permission(&env, &Event::NotificationRequested(original)),
        (PermissionStatus::Granted, None)
    );
    *env.permissions.lock().unwrap() = Some(HashSet::from([PermissionType::ReadApplicationState]));
    for event in [&received, &resolved] {
        assert_eq!(
            crate::wasm_bridge::check_event_permission(&env, event),
            (PermissionStatus::Granted, None)
        );
    }
}
