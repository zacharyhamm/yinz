use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use strum::IntoEnumIterator;
use zellij_utils::data::{BareKey, CommandType, Event, KeyWithModifier, PermissionType};
use zellij_utils::errors::prelude::*;
use zellij_utils::input::layout::RunPluginLocation;
use zellij_utils::input::plugins::PluginConfig;
use zellij_utils::plugin_data_store::PluginDataStore;

use super::{check_lua_plugin, load_lua_plugin};
use crate::plugins::plugin_limits::PluginResourceLimits;
use crate::plugins::plugin_map::{PluginEnv, ResponseSink};
use crate::thread_bus::ThreadSenders;

/// A `PluginEnv` with no wasm behind it and no channels wired up. Enough to load a Lua plugin and
/// run its entrypoints; commands that talk to other threads silently fail, which is what
/// `ThreadSenders::default()` does.
fn test_env() -> PluginEnv {
    PluginEnv {
        response_sink: ResponseSink::WasiStdin,
        plugin_id: 1,
        client_id: 1,
        owner_client_id: 1,
        plugin: PluginConfig {
            path: PathBuf::from("test.lua"),
            _allow_exec_host_cmd: false,
            location: RunPluginLocation::File(PathBuf::from("test.lua")),
            initial_userspace_configuration: Default::default(),
            initial_cwd: None,
            relative_path: None,
        },
        permissions: Arc::new(Mutex::new(None)),
        senders: ThreadSenders::default().silently_fail_on_send(),
        wasi_ctx: None,
        plugin_own_data_dir: PathBuf::from("."),
        zellij_data_dir: PathBuf::from("."),
        plugin_data_store: Arc::new(PluginDataStore::new(&PathBuf::from("."))),
        queues: Default::default(),
        plugin_own_cache_dir: PathBuf::from("."),
        tab_index: Some(0),
        path_to_default_shell: PathBuf::from("."),
        default_shell: None,
        plugin_cwd: PathBuf::from("."),
        session_env_vars: BTreeMap::new(),
        plugin_aliases: Arc::new(Mutex::new(Default::default())),
        input_pipes_to_unblock: Arc::new(Mutex::new(HashSet::new())),
        input_pipes_to_block: Arc::new(Mutex::new(HashSet::new())),
        layout_dir: None,
        user_plugin_dir: None,
        resolved_source_path: None,
        default_mode: Default::default(),
        subscriptions: Arc::new(Mutex::new(HashSet::new())),
        keybinds: Default::default(),
        intercepting_key_presses: false,
        stdin_pipe: Arc::new(Mutex::new(VecDeque::new())),
        stdout_pipe: Arc::new(Mutex::new(VecDeque::new())),
        store_limits: PluginResourceLimits::new(1),
    }
}

fn load(source: &str) -> Result<super::LuaPlugin> {
    load_lua_plugin(source, "test.lua", test_env())
}

/// A plugin whose `/host`, `/data` and `/cache` are real, separate folders, for the filesystem
/// tests. `test_env` points all three at `.`, which would make "did this land in the right root"
/// unanswerable.
fn load_with_dirs(
    source: &str,
    dirs: &FsDirs,
    permissions: Option<HashSet<PermissionType>>,
) -> Result<super::LuaPlugin> {
    let mut env = test_env();
    env.plugin_cwd = dirs.host.path().to_path_buf();
    env.plugin_own_data_dir = dirs.data.path().to_path_buf();
    env.plugin_own_cache_dir = dirs.cache.path().to_path_buf();
    env.permissions = Arc::new(Mutex::new(permissions));
    load_lua_plugin(source, "test.lua", env)
}

/// Removes a path when the test ends, for the one file that cannot live in a tempdir.
struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The three per-plugin roots, kept alive for the length of a test.
struct FsDirs {
    host: tempfile::TempDir,
    data: tempfile::TempDir,
    cache: tempfile::TempDir,
}

impl FsDirs {
    fn new() -> Self {
        FsDirs {
            host: tempfile::tempdir().unwrap(),
            data: tempfile::tempdir().unwrap(),
            cache: tempfile::tempdir().unwrap(),
        }
    }
}

/// Run `body` as a plugin's `render` and hand back what it returned.
fn render_with_dirs(
    body: &str,
    dirs: &FsDirs,
    permissions: Option<HashSet<PermissionType>>,
) -> Result<String> {
    let source = format!(
        "local M = {{}}\nfunction M.render()\n{}\nend\nreturn M",
        body
    );
    let mut plugin = load_with_dirs(&source, dirs, permissions)?;
    Ok(String::from_utf8(plugin.render(1, 200)?).unwrap())
}

/// The permissions the filesystem commands need, and nothing else - so that a test asserting a
/// refusal is asserting about the permission and not about some unrelated grant.
fn file_permissions() -> Option<HashSet<PermissionType>> {
    Some(
        [PermissionType::ReadFiles, PermissionType::WriteFiles]
            .into_iter()
            .collect(),
    )
}

/// A plugin whose permissions are all already granted, for tests about *marshalling* a command
/// rather than about whether it is allowed. Without this, every privileged command is refused
/// before it is ever built.
fn load_permitted(source: &str) -> Result<super::LuaPlugin> {
    let mut env = test_env();
    env.permissions = Arc::new(Mutex::new(Some(PermissionType::iter().collect())));
    load_lua_plugin(source, "test.lua", env)
}

#[test]
fn renders_the_string_its_render_returns() {
    let mut plugin = load(
        r#"
        local M = {}
        function M.render(rows, cols)
          return ("size %dx%d"):format(rows, cols)
        end
        return M
        "#,
    )
    .unwrap();
    let rendered = plugin.render(3, 20).unwrap();
    assert_eq!(String::from_utf8(rendered).unwrap(), "size 3x20");
}

#[test]
fn translates_newlines_for_the_terminal() {
    // A plugin writes plain `\n`; the pane's vte parser needs `\n\r`.
    let mut plugin = load(
        r#"
        local M = {}
        function M.render() return "one\ntwo" end
        return M
        "#,
    )
    .unwrap();
    let rendered = plugin.render(2, 10).unwrap();
    assert_eq!(String::from_utf8(rendered).unwrap(), "one\n\rtwo");
}

#[test]
fn a_plugin_with_no_render_renders_nothing() {
    let mut plugin = load("return {}").unwrap();
    assert_eq!(plugin.render(1, 1).unwrap(), Vec::<u8>::new());
}

/// `print` has no terminal to reach from inside a plugin, so it is the same function as
/// `zellij.log`, and both take anything `tostring` does without raising.
#[test]
fn print_is_zellij_log_and_neither_minds_what_it_is_given() {
    let mut plugin = load(
        r#"
        local M = {}
        function M.render()
          print("a", 1, true, nil, {})
          zellij.log("b", 2.5)
          return tostring(print == zellij.log)
        end
        return M
        "#,
    )
    .unwrap();
    assert_eq!(plugin.render(1, 10).unwrap(), b"true");
}

#[test]
fn update_receives_a_normalized_event_and_reports_whether_to_render() {
    let mut plugin = load(
        r#"
        local M = {}
        M.seen = nil
        function M.update(event)
          M.seen = event.name
          return event.name == "key"
        end
        function M.render() return M.seen or "nothing" end
        return M
        "#,
    )
    .unwrap();

    let key = Event::Key(KeyWithModifier::new(BareKey::Char('a')));
    assert!(
        plugin.update(&key).unwrap(),
        "a key should request a render"
    );
    assert_eq!(
        String::from_utf8(plugin.render(1, 10).unwrap()).unwrap(),
        "key"
    );

    // A tuple variant the plugin does not care about still normalizes, and returning nothing
    // from `update` means "no need to re-render".
    assert!(!plugin.update(&Event::Timer(1.0)).unwrap());
    assert_eq!(
        String::from_utf8(plugin.render(1, 10).unwrap()).unwrap(),
        "timer"
    );
}

/// The host spawns a timer per `set_timeout` and cannot cancel one, so the interval re-arms
/// itself from the `timer` event. Re-arming on *every* timer forked the chain: a second
/// `set_interval`, or a `set_timeout` alongside one, left two permanent clocks calling the
/// plugin back twice as often as it asked.
#[test]
fn overlapping_timers_collapse_back_to_one_interval() {
    let mut plugin = load(
        r#"
        local zellij = require("zellij")
        local M = {}
        M.ticks = 0
        function M.load()
          zellij.set_interval(1)
          zellij.set_interval(1)
          zellij.set_timeout(1)
        end
        function M.update(event)
          if event.name == "timer" then M.ticks = M.ticks + 1 end
          return false
        end
        function M.render() return M.ticks .. "/" .. zellij.__timers end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    assert_eq!(
        String::from_utf8(plugin.render(1, 10).unwrap()).unwrap(),
        "0/3",
        "three timers were asked for"
    );

    // The first two drain the ones already in the air rather than adding to them.
    plugin.update(&Event::Timer(1.0)).unwrap();
    plugin.update(&Event::Timer(1.0)).unwrap();
    assert_eq!(
        String::from_utf8(plugin.render(1, 10).unwrap()).unwrap(),
        "2/1"
    );

    // The last one re-arms, and from here it is one clock however long it runs.
    plugin.update(&Event::Timer(1.0)).unwrap();
    plugin.update(&Event::Timer(1.0)).unwrap();
    assert_eq!(
        String::from_utf8(plugin.render(1, 10).unwrap()).unwrap(),
        "4/1",
        "one chain, not one per set_timeout"
    );
}

#[test]
fn a_list_payload_arrives_whole_rather_than_unwrapped() {
    // `TabUpdate(Vec<TabInfo>)` and a multi-field tuple variant serialize identically, so
    // `value` is always the entire payload - a one-tab update must not arrive as a bare tab.
    let mut plugin = load(
        r#"
        local M = {}
        function M.update(event) M.tabs = #event.value return false end
        function M.render() return tostring(M.tabs) end
        return M
        "#,
    )
    .unwrap();
    plugin
        .update(&Event::TabUpdate(vec![Default::default()]))
        .unwrap();
    assert_eq!(
        String::from_utf8(plugin.render(1, 10).unwrap()).unwrap(),
        "1"
    );
    plugin
        .update(&Event::TabUpdate(vec![
            Default::default(),
            Default::default(),
        ]))
        .unwrap();
    assert_eq!(
        String::from_utf8(plugin.render(1, 10).unwrap()).unwrap(),
        "2"
    );
}

#[test]
fn a_struct_payloads_fields_are_readable_directly_on_the_event() {
    let mut plugin = load(
        r#"
        local M = {}
        function M.update(event) M.key = event.bare_key return false end
        function M.render() return tostring(M.key) end
        return M
        "#,
    )
    .unwrap();
    plugin
        .update(&Event::Key(KeyWithModifier::new(BareKey::Esc)))
        .unwrap();
    assert_eq!(
        String::from_utf8(plugin.render(1, 10).unwrap()).unwrap(),
        "Esc"
    );
}

#[test]
fn a_scalar_payload_arrives_as_value() {
    let mut plugin = load(
        r#"
        local M = {}
        function M.update(event) M.at = event.value return false end
        function M.render() return tostring(M.at) end
        return M
        "#,
    )
    .unwrap();
    plugin.update(&Event::Timer(4.0)).unwrap();
    assert_eq!(
        String::from_utf8(plugin.render(1, 10).unwrap()).unwrap(),
        "4.0"
    );
}

#[test]
fn handlers_registered_with_on_run_alongside_update() {
    let mut plugin = load(
        r#"
        local zellij = require("zellij")
        local M = {}
        M.count = 0
        zellij.on("key", function(event) M.count = M.count + 1 return true end)
        function M.render() return tostring(M.count) end
        return M
        "#,
    )
    .unwrap();
    let key = Event::Key(KeyWithModifier::new(BareKey::Char('x')));
    assert!(plugin.update(&key).unwrap());
    plugin.update(&key).unwrap();
    assert_eq!(
        String::from_utf8(plugin.render(1, 10).unwrap()).unwrap(),
        "2"
    );
}

#[test]
fn a_default_keybinding_callback_is_dispatched_from_its_reserved_pipe() {
    let mut plugin = load_permitted(
        r#"
        local zellij = require("zellij")
        local M = { count = 0 }
        function M.load()
          zellij.register_default_keybindings {
            { mode = "normal", key = "Ctrl g", handler = function()
                M.count = M.count + 1
                return true
              end },
            { mode = "normal", key = "Ctrl n", actions = {
                zellij.action("NewPane")
              } },
          }
        end
        function M.render() return tostring(M.count) end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    let message = zellij_utils::data::PipeMessage::new(
        zellij_utils::data::PipeSource::Keybind,
        "__zellij_default_keybinding:1",
        &None,
        &None,
        true,
    );
    assert!(plugin.pipe(&message).unwrap());
    assert_eq!(plugin.render(1, 10).unwrap(), b"1");
}

#[test]
fn load_merges_user_configuration_over_the_plugins_defaults() {
    let mut plugin = load(
        r#"
        local M = {}
        M.defaults = { greeting = "hello", name = "world" }
        function M.load(config) M.config = config end
        function M.render() return M.config.greeting .. " " .. M.config.name end
        return M
        "#,
    )
    .unwrap();
    let mut configuration = BTreeMap::new();
    configuration.insert("name".to_owned(), "zellij".to_owned());
    plugin.load(&configuration).unwrap();
    assert_eq!(
        String::from_utf8(plugin.render(1, 20).unwrap()).unwrap(),
        "hello zellij"
    );
}

#[test]
fn subscribing_reaches_the_plugin_environment() {
    let mut plugin = load(
        r#"
        local zellij = require("zellij")
        local M = {}
        function M.load() zellij.subscribe { "key", "tab_update" } end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();

    let subscriptions = plugin.env().subscriptions.lock().unwrap().clone();
    let mut names: Vec<String> = subscriptions.iter().map(|s| format!("{:?}", s)).collect();
    names.sort();
    assert_eq!(names, vec!["Key".to_owned(), "TabUpdate".to_owned()]);
}

#[test]
fn requesting_permissions_is_recorded_against_the_plugin() {
    let mut plugin = load(
        r#"
        local zellij = require("zellij")
        local M = {}
        function M.load() zellij.request_permission { "change_application_state" } end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    // The grant itself is asynchronous (the user is prompted), so what we can check here is that
    // the command was understood rather than rejected as unknown.
    assert!(!PermissionType::ChangeApplicationState
        .to_string()
        .is_empty());
}

#[test]
fn an_unknown_command_is_an_error_naming_it() {
    let mut plugin = load(
        r#"
        local zellij = require("zellij")
        local M = {}
        function M.load() zellij.command("not_a_real_command") end
        return M
        "#,
    )
    .unwrap();
    let err = format!("{:#}", plugin.load(&BTreeMap::new()).unwrap_err());
    assert!(
        err.contains("not_a_real_command"),
        "error should name the command, got: {}",
        err
    );
}

#[test]
fn a_plugin_that_does_not_return_a_table_fails_to_load() {
    let err = match load("local M = {} -- forgot to return it") {
        Ok(_) => panic!("a plugin that returns no table should not load"),
        Err(e) => format!("{:#}", e),
    };
    assert!(
        err.contains("must return a table"),
        "error should explain the contract, got: {}",
        err
    );
}

#[test]
fn a_runtime_error_in_an_entrypoint_is_reported_not_swallowed() {
    let mut plugin = load(
        r#"
        local M = {}
        function M.render() error("something went wrong") end
        return M
        "#,
    )
    .unwrap();
    let err = format!("{:#}", plugin.render(1, 1).unwrap_err());
    assert!(
        err.contains("something went wrong"),
        "error should carry the plugin's message, got: {}",
        err
    );
}

#[test]
fn the_dangerous_stdlib_is_absent_rather_than_merely_erroring() {
    // `os` survives as a table because the clock lives there, but only the clock does - see
    // `the_os_library_keeps_the_clock_and_nothing_else`. Everything here is gone outright,
    // rather than present and raising, so that a plugin cannot probe for it either.
    let mut plugin = load(
        r#"
        local M = {}
        function M.render()
          return table.concat({
            type(io), type(dofile), type(loadfile), type(load), type(debug), type(package),
            type(os.execute), type(os.getenv), type(os.setlocale), type(os.exit),
            type(os.remove), type(os.rename), type(os.tmpname),
          }, ",")
        end
        return M
        "#,
    )
    .unwrap();
    let rendered = String::from_utf8(plugin.render(1, 80).unwrap()).unwrap();
    assert_eq!(rendered, ["nil"; 13].join(","));
}

#[test]
fn the_filesystem_is_not_reachable_from_lua() {
    // Lua plugins are not sandboxed the way wasm plugins are, so the stdlib they get must not
    // include the parts that would let them bypass the permission system entirely.
    for source in [
        r#"local M = {} function M.render() return tostring(io.open("/etc/passwd")) end return M"#,
        r#"local M = {} function M.render() return tostring(os.execute("true")) end return M"#,
        r#"local M = {} function M.render() return tostring(dofile("/etc/passwd")) end return M"#,
    ] {
        let mut plugin = load(source).unwrap();
        assert!(
            plugin.render(1, 1).is_err(),
            "expected this to be unreachable from a plugin: {}",
            source
        );
    }
}

#[test]
fn require_only_serves_the_zellij_module() {
    let mut plugin = load(
        r#"
        local M = {}
        function M.render() return tostring(require("socket")) end
        return M
        "#,
    )
    .unwrap();
    let err = format!("{:#}", plugin.render(1, 1).unwrap_err());
    assert!(err.contains("zellij"), "got: {}", err);
}

#[test]
fn commands_cannot_be_called_outside_an_entrypoint() {
    // `__dispatch` closes over a borrow that only lives for one call, so a plugin that squirrels
    // it away must not be able to use it later.
    let mut plugin = load(
        r#"
        local zellij = require("zellij")
        local M = {}
        function M.load() M.stashed = zellij.__dispatch end
        function M.render() return tostring(M.stashed("close_self")) end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    assert!(plugin.render(1, 1).is_err());
}

fn example_plugin(name: &str) -> String {
    std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../example-plugins")
            .join(name),
    )
    .expect("the example plugin should be in the repo")
}

#[test]
fn the_shipped_example_plugin_loads_and_renders() {
    // `example-plugins/clock.lua` is what the docs point new plugin authors at, so it should not
    // be allowed to rot.
    let source = example_plugin("clock.lua");
    let mut plugin = load_lua_plugin(&source, "clock.lua", test_env()).unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    let rendered = String::from_utf8(plugin.render(5, 40).unwrap()).unwrap();
    // The clock shows the real time now, so match its shape rather than a fixed reading.
    let clock = rendered
        .split_whitespace()
        .find(|word| word.matches(':').count() == 2)
        .unwrap_or_else(|| panic!("no HH:MM:SS in: {}", rendered));
    assert!(
        clock
            .split(':')
            .all(|part| part.len() == 2 && part.chars().all(|c| c.is_ascii_digit())),
        "got: {}",
        rendered
    );
    assert!(rendered.contains("the time is"), "got: {}", rendered);
}

#[test]
fn the_agent_commit_example_loads_and_stages_on_c() {
    // `example-plugins/agent-commit.lua` is what the hooks documentation points at, so it should
    // not be allowed to rot. Pressing `c` marshals a `RunCommand`, which is the shape most
    // likely to be got wrong.
    let source = example_plugin("agent-commit.lua");
    let mut env = test_env();
    env.permissions = Arc::new(Mutex::new(Some(PermissionType::iter().collect())));
    let mut plugin = load_lua_plugin(&source, "agent-commit.lua", env).unwrap();
    plugin
        .load(&BTreeMap::from([
            ("agent_pane_id".to_owned(), "7".to_owned()),
            ("agent_cwd".to_owned(), "/work".to_owned()),
        ]))
        .unwrap();

    let rendered = String::from_utf8(plugin.render(12, 70).unwrap()).unwrap();
    assert!(rendered.contains("stage & commit"), "got: {rendered}");
    assert!(rendered.contains("Directory: /work"), "got: {rendered}");

    plugin
        .update(&Event::Key(KeyWithModifier::new(BareKey::Char('c'))))
        .expect("staging should marshal");
    let rendered = String::from_utf8(plugin.render(12, 70).unwrap()).unwrap();
    assert!(rendered.contains("staging"), "got: {rendered}");
}

#[test]
fn the_shipped_agent_exit_hook_loads_and_renders() {
    // The builtin hook is compiled in rather than read from disk, so nothing else would notice
    // a syntax error in it until an agent exited.
    let mut plugin = load_lua_plugin(
        zellij_utils::consts::builtin_lua_plugin("agent-exit").expect("the hook is a builtin"),
        "agent-exit.lua",
        test_env(),
    )
    .unwrap();
    plugin
        .load(&BTreeMap::from([
            ("agent_pane_id".to_owned(), "7".to_owned()),
            ("agent_cwd".to_owned(), "/work/.worktrees/topic".to_owned()),
        ]))
        .unwrap();
    let rendered = String::from_utf8(plugin.render(20, 100).unwrap()).unwrap();
    assert!(
        rendered.contains("Agent exited. Create another agent in the same directory?"),
        "got: {rendered}"
    );
    assert!(
        rendered.contains("Directory: /work/.worktrees/topic"),
        "got: {rendered}"
    );
}

/// The context-menu hook has no `render`, so a load-and-draw check would prove nothing about it.
/// What it does instead is transform an event into an answer, and that is what is exercised here:
/// a syntax error, a bad field name or a broken filter would otherwise go unnoticed until somebody
/// right-clicked.
#[test]
fn the_shipped_context_menu_hook_filters_the_items_it_is_given() {
    use crate::screen::ScreenInstruction;
    use zellij_utils::channels::SenderWithContext;
    use zellij_utils::data::{ContextMenuContext, ContextMenuItem, ContextMenuLink};
    use zellij_utils::errors::ErrorContext;

    let source =
        zellij_utils::consts::builtin_lua_plugin("context-menu").expect("the hook is a builtin");
    let configured = vec![
        ContextMenuItem::new("Copy", vec![]).with_id(0),
        ContextMenuItem::separator().with_id(1),
        ContextMenuItem::new("Close pane", vec![]).with_id(2),
    ];

    // The answer goes to the screen thread, so the env gets a real channel rather than the
    // silently-failing default every other test here uses.
    let answer_for = |context: ContextMenuContext| -> Vec<ContextMenuItem> {
        let (to_screen, screen_receiver) =
            zellij_utils::channels::unbounded::<(ScreenInstruction, ErrorContext)>();
        let mut env = test_env();
        env.senders.to_screen = Some(SenderWithContext::new(to_screen));
        env.permissions = Arc::new(Mutex::new(Some(PermissionType::iter().collect())));

        let mut plugin = load_lua_plugin(source, "context-menu.lua", env).unwrap();
        plugin.load(&BTreeMap::new()).unwrap();
        plugin
            .update(&Event::ContextMenuRequested(Box::new(context)))
            .expect("the hook should answer");

        screen_receiver
            .try_iter()
            .find_map(|(instruction, _)| match instruction {
                ScreenInstruction::SetContextMenu { items, .. } => Some(items),
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
            == Some(0),
        "the id has to survive the relabelling, or the actions do not come back"
    );

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
    use crate::background_jobs::BackgroundJob;
    use zellij_utils::channels::SenderWithContext;
    use zellij_utils::data::ContextMenuContext;
    use zellij_utils::errors::ErrorContext;

    let source =
        zellij_utils::consts::builtin_lua_plugin("context-menu").expect("the hook is a builtin");
    let (to_background_jobs, background_jobs) =
        zellij_utils::channels::unbounded::<(BackgroundJob, ErrorContext)>();
    let mut env = test_env();
    env.senders.to_background_jobs = Some(SenderWithContext::new(to_background_jobs));
    env.permissions = Arc::new(Mutex::new(Some(PermissionType::iter().collect())));

    let mut plugin = load_lua_plugin(source, "context-menu.lua", env).unwrap();
    plugin.load(&BTreeMap::new()).unwrap();

    let commands = |receiver: &zellij_utils::channels::Receiver<(BackgroundJob, ErrorContext)>| {
        receiver
            .try_iter()
            .filter_map(|(job, _)| match job {
                BackgroundJob::RunCommand(_, _, command, args, ..) => Some((command, args)),
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

    let select = |plugin: &mut crate::plugins::lua::LuaPlugin| {
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

    // ...and from here on it goes straight out.
    select(&mut plugin);
    assert_eq!(
        commands(&background_jobs),
        vec![("open".to_owned(), vec!["https://example.com".to_owned()])]
    );
}

#[test]
fn the_playground_plugin_loads_and_runs_every_key_it_advertises() {
    // `example-plugins/playground.lua` is the worked example for the command surface, so it is
    // worth more than a load-and-render check: every key it offers is pressed here. None of them
    // can do anything in a fixture with no threads behind it, but all of them go through
    // marshalling, and a shape this file gets wrong fails here rather than in front of a user.
    let source = example_plugin("playground.lua");
    let mut env = test_env();
    env.permissions = Arc::new(Mutex::new(Some(
        PermissionType::iter()
            .filter(|permission| permission != &PermissionType::WriteToClipboard)
            .collect(),
    )));
    let mut plugin = load_lua_plugin(&source, "playground.lua", env).unwrap();
    plugin.load(&BTreeMap::new()).unwrap();

    let rendered = String::from_utf8(plugin.render(20, 60).unwrap()).unwrap();
    assert!(
        rendered.contains("zellij lua playground"),
        "got: {}",
        rendered
    );

    for key in ["i", "v", "p", "d", "t", "r", "n", "c", "w", "+", "-", "x"] {
        let key = BareKey::Char(key.chars().next().unwrap());
        plugin
            .update(&Event::Key(KeyWithModifier::new(key)))
            .unwrap_or_else(|e| panic!("pressing '{:?}' should not fail the plugin: {:#}", key, e));
    }

    // `x` is the last key pressed, and is the one the plugin deliberately has no permission for.
    let rendered = String::from_utf8(plugin.render(20, 70).unwrap()).unwrap();
    assert!(
        rendered.contains("WriteToClipboard"),
        "the unpermitted key should report the permission it wants, got: {}",
        rendered
    );
}

#[test]
fn a_command_cannot_be_called_re_entrantly() {
    // The dispatch function borrows `PluginEnv` for the duration of one call. A re-entrant call
    // must be refused rather than panicking, which would take down the whole plugin thread.
    let mut plugin = load(
        r#"
        local zellij = require("zellij")
        local M = {}
        function M.load()
          -- `set_selectable` is dispatched while this metatable hook runs, from inside another
          -- dispatch, which is exactly the re-entrant case.
          local reentrant = setmetatable({}, {
            __tostring = function() zellij.set_selectable(true) return "" end,
          })
          M.err = select(2, pcall(function() zellij.open_terminal({ cwd = reentrant }) end))
        end
        return M
        "#,
    )
    .unwrap();
    // Whatever happens, it must not panic and must not corrupt the plugin.
    let _ = plugin.load(&BTreeMap::new());
    assert!(plugin.render(1, 1).is_ok());
}

#[test]
fn key_events_carry_a_readable_key_name() {
    // `BareKey::Char('t')` serializes as `{ Char = "t" }` while `BareKey::Esc` serializes as the
    // string "Esc". Plugins should not have to know that, so key events also carry `event.key`.
    let mut plugin = load(
        r#"
        local M = {}
        function M.update(event) M.keys = (M.keys or "") .. event.key .. " " return false end
        function M.render() return M.keys or "" end
        return M
        "#,
    )
    .unwrap();
    for key in [BareKey::Char('t'), BareKey::Esc, BareKey::Enter] {
        plugin
            .update(&Event::Key(KeyWithModifier::new(key)))
            .unwrap();
    }
    assert_eq!(
        String::from_utf8(plugin.render(1, 40).unwrap()).unwrap(),
        "t Esc Enter "
    );
}

// The widened command surface ------------------------------------------------------------------

/// Run `source` inside a plugin's `load`, returning whatever error it raised.
fn error_from_load(source: &str) -> Option<String> {
    let body = format!(
        "local zellij = require(\"zellij\")\nlocal M = {{}}\nfunction M.load()\n{}\nend\nreturn M",
        source
    );
    let mut plugin = load_permitted(&body).unwrap();
    plugin
        .load(&BTreeMap::new())
        .err()
        .map(|e| format!("{:#}", e))
}

fn assert_marshals(source: &str) {
    if let Some(err) = error_from_load(source) {
        panic!("expected this to marshal, got: {}\nsource: {}", err, source);
    }
}

#[test]
fn a_unit_command_is_reachable_by_name() {
    // Nothing in the prelude wraps `new_pane`; the metatable manufactures it, and the host reads
    // the bare name as serde's unit variant.
    assert_marshals("zellij.new_pane()");
    assert_marshals("zellij.go_to_next_tab()");
}

#[test]
fn a_struct_command_takes_named_arguments() {
    assert_marshals(r#"zellij.new_tab { name = "logs", cwd = "/tmp" }"#);
    // Every field of `NewTab` is optional, so no arguments at all is a legitimate call.
    assert_marshals("zellij.new_tab()");
}

#[test]
fn a_tuple_command_takes_positional_or_named_arguments() {
    assert_marshals(r#"zellij.rename_tab { 0, "logs" }"#);
    assert_marshals(r#"zellij.rename_tab { tab_index = 0, name = "logs" }"#);
    assert_marshals(r#"zellij.rename_tab(0, "logs")"#);
}

#[test]
fn run_command_takes_named_arguments() {
    // Five arguments, three of them tables - the case named arguments exist for.
    assert_marshals(
        r#"zellij.run_command {
             command = { "git", "status" },
             env_variables = {},
             cwd = "/tmp",
             context = { step = "status" },
             max_bytes_returned = 1024,
           }"#,
    );
    assert_marshals(r#"zellij.run_command { { "git", "status" }, {}, "/tmp", {}, 1024 }"#);
}

#[test]
fn an_optional_tuple_element_still_has_to_be_given() {
    // `max_bytes_returned` is an `Option`, which makes it nullable rather than absent: serde
    // counts a tuple variant's elements, so a four-element list is the wrong shape however
    // optional the fifth is. Worth pinning - "it is optional so I can leave it out" is the
    // natural reading, and it does not hold.
    let err = error_from_load(
        r#"zellij.run_command {
             command = { "git", "status" },
             env_variables = {},
             cwd = "/tmp",
             context = {},
           }"#,
    )
    .expect("a four-element RunCommand is refused");
    assert!(err.contains("invalid length 4"), "got: {err}");
}

#[test]
fn a_nested_payload_is_built_from_a_plain_table() {
    // `CommandToRun` is a struct inside a tuple variant - the shape that used to need a
    // hand-written marshaller per command.
    assert_marshals(r#"zellij.open_command_pane { { path = "sh", args = { "-c", "true" } }, {} }"#);
}

/// Build the command a Lua expression means, without letting the host run it.
///
/// `assert_marshals` loads a plugin and lets the call through to `run_plugin_command`, which for
/// this command *does real work*: `prepare_git_checkout_in` shells out to git in the plugin's
/// cwd, and a `NewBranch` would switch the branch of whatever checkout the tests run in. So the
/// enum spellings are checked one step earlier, where the question actually is - does serde read
/// this table as the variant the guide says it does.
fn command_from_lua(expression: &str) -> Result<zellij_utils::data::PluginCommand> {
    let lua = mlua::Lua::new();
    let args: mlua::Value = lua
        .load(expression)
        .eval()
        .expect("the test's own Lua should parse");
    super::command::plugin_command_from_lua(
        &lua,
        "OpenCommandPaneInGitCheckoutNearPlugin",
        "open_command_pane_in_git_checkout_near_plugin",
        args,
    )
}

/// The shape nothing else in this file covers: an externally-tagged enum nested inside a tuple
/// variant. `GitCheckout` travels in its own PascalCase spelling - a bare string for the unit
/// variant, a single-key table for the others - because only the *command* name is pascal-cased
/// on the way through, never a payload. These are the tables `docs/LUA_PLUGINS.md` shows.
#[test]
fn a_nested_enum_is_read_as_the_variant_it_names() {
    use zellij_utils::data::{GitCheckout, PluginCommand};

    let checkout = |expression: &str| match command_from_lua(expression) {
        Ok(PluginCommand::OpenCommandPaneInGitCheckoutNearPlugin(_, source, checkout)) => {
            (source, checkout)
        },
        Ok(other) => panic!("built the wrong command: {other:?}"),
        Err(e) => panic!("{e:#}\nsource: {expression}"),
    };

    let (source, current) = checkout(r#"{ { path = "sh" }, "/host", "Current" }"#);
    assert_eq!(source, PathBuf::from("/host"));
    assert_eq!(current, GitCheckout::Current);

    let (_, branch) = checkout(
        r#"{ { path = "sh" }, "/host", { NewBranch = { branch = "fix-42", base = "main" } } }"#,
    );
    assert_eq!(
        branch,
        GitCheckout::NewBranch {
            branch: "fix-42".to_owned(),
            base: "main".to_owned(),
        }
    );

    let (_, worktree) = checkout(
        r#"{
             { path = "claude", args = { "--model", "opus" } },
             "/host",
             { NewWorktree = { branch = "fix-42", base = "main", path = ".worktrees/fix-42" } },
           }"#,
    );
    assert_eq!(
        worktree,
        GitCheckout::NewWorktree {
            branch: "fix-42".to_owned(),
            base: "main".to_owned(),
            path: PathBuf::from(".worktrees/fix-42"),
        }
    );
}

/// The trap the guide warns about: the checkout is not optional, and leaving it out is a length
/// error rather than a quiet `Current`. Same rule as `run_command`'s fifth element, above.
#[test]
fn a_git_checkout_command_needs_all_three_arguments() {
    let err = command_from_lua(r#"{ { path = "sh" }, "/host" }"#)
        .expect_err("a two-element call is refused");
    let err = format!("{err:#}");
    assert!(err.contains("invalid length 2"), "got: {err}");
}

/// The other half, which `command_from_lua` cannot see: the prelude turning the named form into
/// the positional one. Driven through a real load, so only `Current` is used - it is the one
/// checkout that asks git nothing but where the repository is.
#[test]
fn a_git_checkout_command_takes_named_arguments() {
    for source in [
        r#"zellij.open_command_pane_in_git_checkout_near_plugin({ path = "sh" }, ".", "Current")"#,
        r#"zellij.open_command_pane_in_git_checkout_near_plugin { { path = "sh" }, ".", "Current" }"#,
        r#"zellij.open_command_pane_in_git_checkout_near_plugin {
             command = { path = "sh" },
             source_directory = ".",
             checkout = "Current",
           }"#,
    ] {
        // The host gets as far as trying to open the pane, which these senders cannot do. That is
        // one step past the question being asked: anything serde refused would have said so
        // instead, naming the command.
        if let Some(err) = error_from_load(source) {
            assert!(
                !err.contains("could not read the arguments"),
                "the arguments were refused: {err}\nsource: {source}"
            );
        }
    }
}

#[test]
fn a_byte_payload_is_written_as_a_string() {
    // `Write(Vec<u8>)` wants a list of byte values; plugins have a string.
    assert_marshals(r#"zellij.write("echo hi\r")"#);
    assert_marshals(r#"zellij.write_to_pane_id("echo hi\r", zellij.terminal_pane(1))"#);
}

#[test]
fn a_pane_id_is_built_by_the_prelude() {
    assert_marshals("zellij.hide_pane_with_id(zellij.terminal_pane(3))");
    assert_marshals("zellij.hide_pane_with_id(zellij.plugin_pane(3))");
}

#[test]
fn bad_arguments_are_an_error_naming_the_command_rather_than_a_panic() {
    let err = error_from_load(r#"zellij.set_timeout("soon")"#).expect("this should not marshal");
    assert!(err.contains("set_timeout"), "got: {}", err);
}

#[test]
fn a_typo_is_reported_in_the_spelling_the_plugin_used() {
    // The prelude pascal-cases before handing the name over, so this is the one thing that has
    // to be carried separately - an error reading "unknown zellij command 'NotARealCommand'"
    // would send an author looking for something they never wrote.
    let err = error_from_load("zellij.not_a_real_command()").expect("this should not marshal");
    assert!(err.contains("not_a_real_command"), "got: {}", err);
}

#[test]
fn host_names_are_absent_from_the_manufactured_command_table() {
    // The host installs and removes `zellij.__dispatch` by name, and both it and the prelude
    // check it for nil. A metatable that manufactured a command function for every missing
    // underscore name would make that check meaningless, so those names stay absent.
    let mut plugin = load(
        r#"
        local zellij = require("zellij")
        local M = {}
        function M.render() return type(zellij.__nothing) .. "," .. type(zellij.not_a_command) end
        return M
        "#,
    )
    .unwrap();
    assert_eq!(
        String::from_utf8(plugin.render(1, 40).unwrap()).unwrap(),
        "nil,function"
    );
}

#[test]
fn every_command_zellij_knows_is_a_name_serde_accepts() {
    // `PluginCommand` now has two serializations: the hand-written protobuf one wasm plugins use,
    // and serde's, which is how Lua reaches it. `CommandType` is generated from the enum, so if
    // serde's variant names ever drift from it, this fails - and a Lua plugin would otherwise
    // just get "unknown variant" for a command zellij plainly has.
    let lua = mlua::Lua::new();
    for command in CommandType::iter() {
        let name = command.to_string();
        let error =
            match super::command::plugin_command_from_lua(&lua, &name, &name, mlua::Value::Nil) {
                Ok(_) => continue,
                Err(e) => format!("{:#}", e),
            };
        // Failing for want of arguments is expected here - `Nil` is only right for unit
        // variants. Failing because serde does not know the name is the drift this catches.
        assert!(
            !error.contains("unknown variant") && !error.contains("unknown zellij command"),
            "serde does not know the command '{}': {}",
            name,
            error
        );
    }
}

#[test]
fn a_command_that_answers_hands_back_a_table() {
    // `get_plugin_ids` is answered by the host itself rather than by another thread, so it is one
    // of the two queries a plugin with no channels behind it can actually run. Before the reply
    // was decoded this arrived as a string of protobuf bytes.
    let mut plugin = load_permitted(
        r#"
        local zellij = require("zellij")
        local M = {}
        function M.load()
          local ids = zellij.get_plugin_ids()
          M.shape = type(ids)
          M.plugin_id = ids.plugin_id
          M.cwd = ids.initial_cwd
        end
        function M.render()
          return string.format("%s %s %s", M.shape, tostring(M.plugin_id), tostring(M.cwd))
        end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    assert_eq!(
        String::from_utf8(plugin.render(1, 60).unwrap()).unwrap(),
        "table 1 ."
    );
}

/// `get_user_plugin_dir` is answered by the host itself, like `get_plugin_ids`, so it is one of
/// the few queries reachable with no channels wired up - and a Lua plugin gets it for free,
/// because `PluginCommand` derives serde and `response.rs` knows how to decode the reply.
#[test]
fn the_user_plugin_dir_reaches_lua_as_a_string() {
    let mut env = test_env();
    env.permissions = Arc::new(Mutex::new(Some(PermissionType::iter().collect())));
    env.user_plugin_dir = Some(PathBuf::from("/somewhere/plugins"));
    let mut plugin = load_lua_plugin(
        r#"
        local zellij = require("zellij")
        local M = {}
        function M.load() M.dir = zellij.get_user_plugin_dir() end
        function M.render() return type(M.dir) .. " " .. tostring(M.dir) end
        return M
        "#,
        "test.lua",
        env,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    assert_eq!(
        String::from_utf8(plugin.render(1, 60).unwrap()).unwrap(),
        "string /somewhere/plugins"
    );
}

/// A `zellij pipe` run inside a pane arrives knowing which pane, as `message.pane_id`, so no
/// integration has to hand-thread a `pane_id=` argument.
#[test]
fn a_cli_pipes_pane_reaches_lua_on_the_message() {
    use zellij_utils::data::{PaneId, PipeMessage, PipeSource};
    let mut plugin = load_permitted(
        r#"
        local zellij = require("zellij")
        local M = {}
        function M.pipe(message)
            M.seen = message.pane_id and message.pane_id.Terminal or "none"
            return true
        end
        function M.render() return tostring(M.seen) end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    let mut message = PipeMessage::new(
        PipeSource::Cli("p1".to_owned()),
        "hello",
        &None,
        &None,
        false,
    );
    message.pane_id = Some(PaneId::Terminal(7));
    assert!(plugin.pipe(&message).unwrap());
    assert_eq!(
        String::from_utf8(plugin.render(1, 10).unwrap()).unwrap(),
        "7"
    );
}

/// `/plugins` is the user's plugin folder: readable with `ReadFiles`, writable only with
/// `AuthorPlugins` on top of `WriteFiles`, and absent when the session has no such folder.
#[test]
fn the_plugin_folder_is_a_root_that_needs_author_plugins_to_write() {
    let dirs = FsDirs::new();
    let plugins = tempfile::tempdir().unwrap();
    let load = |permissions: HashSet<PermissionType>| {
        let mut env = test_env();
        env.plugin_cwd = dirs.host.path().to_path_buf();
        env.plugin_own_data_dir = dirs.data.path().to_path_buf();
        env.plugin_own_cache_dir = dirs.cache.path().to_path_buf();
        env.user_plugin_dir = Some(plugins.path().to_path_buf());
        env.permissions = Arc::new(Mutex::new(Some(permissions)));
        load_lua_plugin(
            r#"
            local zellij = require("zellij")
            local M = {}
            function M.render()
                local ok, err = pcall(function() zellij.fs.write("/plugins/keys.lua", "return {}") end)
                if not ok then return "refused: " .. tostring(err) end
                return zellij.fs.read("/plugins/keys.lua")
            end
            return M
            "#,
            "test.lua",
            env,
        )
    };
    let render = |permissions: HashSet<PermissionType>| -> String {
        let mut plugin = load(permissions).unwrap();
        String::from_utf8(plugin.render(1, 200).unwrap()).unwrap()
    };

    let without = render(file_permissions().unwrap());
    assert!(
        without.contains("AuthorPlugins"),
        "a write into the plugin folder names the permission it needs: {without}"
    );
    assert!(!plugins.path().join("keys.lua").exists());

    let mut with = file_permissions().unwrap();
    with.insert(PermissionType::AuthorPlugins);
    assert_eq!(render(with), "return {}");
    assert_eq!(
        std::fs::read_to_string(plugins.path().join("keys.lua")).unwrap(),
        "return {}",
        "the write landed in the real plugin folder"
    );
}

/// A reload by url is a plugin-thread instruction, not an action that opens a pane.
#[test]
fn reloading_a_plugin_by_url_asks_the_plugin_thread() {
    use crate::plugins::PluginInstruction;
    use zellij_utils::channels::SenderWithContext;
    use zellij_utils::errors::ErrorContext;

    let (to_plugin, plugin_receiver) =
        zellij_utils::channels::unbounded::<(PluginInstruction, ErrorContext)>();
    let mut env = test_env();
    env.senders.to_plugin = Some(SenderWithContext::new(to_plugin));
    env.permissions = Arc::new(Mutex::new(Some(PermissionType::iter().collect())));
    let mut plugin = load_lua_plugin(
        r#"
        local zellij = require("zellij")
        local M = {}
        function M.load() zellij.reload_plugin_by_url("file:/somewhere/keys.lua") end
        return M
        "#,
        "test.lua",
        env,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    let reloads: Vec<String> = plugin_receiver
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::ReloadPluginByUrl(run_plugin) => Some(run_plugin.location_string()),
            _ => None,
        })
        .collect();
    assert_eq!(reloads, vec!["file:/somewhere/keys.lua".to_owned()]);
}

/// The compile check is what a writer runs before a reload: it answers about a string, not a
/// file, and it answers `nil` for a source that compiles rather than raising for one that does
/// not — a failing check is the answer, not an error in the asker.
#[test]
fn check_lua_plugin_answers_nil_or_the_compile_error() {
    let mut plugin = load_permitted(
        r#"
        local zellij = require("zellij")
        local M = {}
        function M.load()
            M.good = zellij.check_lua_plugin { name = "keys.lua", source = "return {}" }
            M.bad = zellij.check_lua_plugin { name = "keys.lua", source = "return {" }
        end
        function M.render() return tostring(M.good) .. "|" .. tostring(M.bad) end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    let rendered = String::from_utf8(plugin.render(1, 200).unwrap()).unwrap();
    let (good, bad) = rendered.split_once('|').unwrap();
    assert_eq!(good, "nil", "a source that compiles answers nil");
    assert!(
        bad.contains("[string \"keys.lua\"]:1:"),
        "the error names the chunk and line as a load error would: {bad}"
    );
}

/// A plugin speaking for an agent - hocus's `agent_register`, a daemon relaying an old hook
/// script - sets the pane's record whole, and the screen is what carries it from there.
#[test]
fn a_plugin_can_set_a_panes_agent_record() {
    use crate::screen::ScreenInstruction;
    use zellij_utils::channels::SenderWithContext;
    use zellij_utils::data::{AgentInfo, AgentSource, AgentState};
    use zellij_utils::errors::ErrorContext;

    let (to_screen, screen_receiver) =
        zellij_utils::channels::unbounded::<(ScreenInstruction, ErrorContext)>();
    let mut env = test_env();
    env.senders.to_screen = Some(SenderWithContext::new(to_screen));
    env.permissions = Arc::new(Mutex::new(Some(PermissionType::iter().collect())));
    let mut plugin = load_lua_plugin(
        r#"
        local zellij = require("zellij")
        local M = {}
        function M.load()
            zellij.set_pane_agent {
                pane_id = zellij.terminal_pane(3),
                agent = { kind = "codex", state = "Waiting", source = "Reported", detail = "y/n" },
            }
            zellij.set_pane_agent { pane_id = zellij.terminal_pane(3) }
        end
        return M
        "#,
        "agents.lua",
        env,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();

    let records: Vec<_> = screen_receiver
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            ScreenInstruction::SetPaneAgent(pane_id, agent, _) => Some((pane_id, agent)),
            _ => None,
        })
        .collect();
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(records[0].0, crate::panes::PaneId::Terminal(3));
    assert_eq!(
        records[0].1,
        Some(AgentInfo {
            kind: "codex".to_owned(),
            state: AgentState::Waiting,
            source: AgentSource::Reported,
            detail: Some("y/n".to_owned()),
            cwd: None,
            session_id: None,
        })
    );
    assert_eq!(records[1].1, None, "leaving `agent` out clears the record");
}

#[test]
fn the_compile_check_compiles_only() {
    // A plugin that parses but fails when run, or returns no table, is the loader's to report:
    // the check must not run the chunk, or a `load` with side effects would run twice.
    assert_eq!(check_lua_plugin("error('boom')", "x.lua"), Ok(()));
    assert_eq!(check_lua_plugin("return 1", "x.lua"), Ok(()));
    assert!(check_lua_plugin("local = 1", "x.lua").is_err());
}

#[test]
fn a_scalar_answer_arrives_as_a_scalar() {
    // Not every reply is a table: `get_zellij_version` answers with one string, and wrapping it
    // in a table to be uniform would only make it worse to use.
    let mut plugin = load_permitted(
        r#"
        local zellij = require("zellij")
        local M = {}
        function M.load() M.version = zellij.get_zellij_version() end
        function M.render() return type(M.version) .. " " .. tostring(M.version) end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    let rendered = String::from_utf8(plugin.render(1, 40).unwrap()).unwrap();
    assert_eq!(
        rendered,
        format!("string {}", zellij_utils::consts::VERSION),
        "the version should arrive readable, not as protobuf bytes"
    );
}

#[test]
fn a_command_the_plugin_may_not_run_is_an_error_rather_than_silence() {
    // `run_plugin_command`'s denied branch logs and returns: a wasm guest is mid-call and cannot
    // be handed a refusal. A Lua plugin's command is an ordinary function call, so it can be told
    // no - and being told nothing at all, while the command quietly did not happen, was worse.
    let mut plugin = load(
        r#"
        local zellij = require("zellij")
        local M = {}
        function M.load() M.err = select(2, pcall(function() zellij.open_terminal("/tmp") end)) end
        function M.render() return tostring(M.err) end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    let rendered = String::from_utf8(plugin.render(1, 200).unwrap()).unwrap();
    assert!(
        rendered.contains("OpenTerminalsOrPlugins") && rendered.contains("request_permission"),
        "the error should name the permission and how to ask for it, got: {}",
        rendered
    );
}

#[test]
fn a_command_needing_no_permission_is_not_refused() {
    // The check has to be the same one `run_plugin_command` makes, or a Lua plugin would be
    // refused things a wasm plugin is allowed.
    let mut plugin = load(
        r#"
        local zellij = require("zellij")
        local M = {}
        function M.load() zellij.subscribe { "key" } end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
}

// Time, JSON and coroutines -----------------------------------------------------------------

#[test]
fn the_clock_is_readable_without_asking_permission() {
    // A wasm plugin reads the clock through WASI with no permission at all, so a Lua plugin
    // asking the user to approve a clock would be noise. Note `load`, not `load_permitted`.
    let mut plugin = load(
        r#"
        local M = {}
        function M.render()
          local t = os.time()
          local parts = os.date("*t", t)
          return string.format("%d,%d,%s", t, parts.year, os.date("!%Y", 0))
        end
        return M
        "#,
    )
    .unwrap();
    let rendered = String::from_utf8(plugin.render(1, 60).unwrap()).unwrap();
    let fields: Vec<&str> = rendered.split(',').collect();

    let now: i64 = fields[0].parse().expect("os.time returned a number");
    // Somewhere after 2020 and before 2100: enough to catch a clock that is not a clock, without
    // pinning the test to the date it was written.
    assert!(
        (1_577_836_800..4_102_444_800).contains(&now),
        "os.time should be a plausible unix timestamp, got {}",
        now
    );
    let year: i64 = fields[1].parse().expect("os.date gave a year");
    assert!((2020..2100).contains(&year), "got year {}", year);
    // The epoch in UTC is the one date every machine must agree on.
    assert_eq!(fields[2], "1970");
}

#[test]
fn the_os_library_keeps_the_clock_and_nothing_else() {
    // A whitelist, so this is really asserting that the *replacement* table was installed rather
    // than that a handful of known names were blanked.
    let mut plugin = load(
        r#"
        local M = {}
        function M.render()
          local kept = {}
          for name in pairs(os) do table.insert(kept, name) end
          table.sort(kept)
          return table.concat(kept, ",")
        end
        return M
        "#,
    )
    .unwrap();
    let rendered = String::from_utf8(plugin.render(1, 100).unwrap()).unwrap();
    assert_eq!(rendered, "clock,date,difftime,time");
}

#[test]
fn coroutines_are_available() {
    let mut plugin = load(
        r#"
        local M = {}
        function M.render()
          local co = coroutine.create(function(a) coroutine.yield(a + 1) return "done" end)
          local _, first = coroutine.resume(co, 1)
          local _, second = coroutine.resume(co)
          return string.format("%d,%s", first, second)
        end
        return M
        "#,
    )
    .unwrap();
    assert_eq!(
        String::from_utf8(plugin.render(1, 20).unwrap()).unwrap(),
        "2,done"
    );
}

#[test]
fn json_round_trips_and_decodes_null_to_nil() {
    let mut plugin = load(
        r#"
        local M = {}
        function M.render()
          local decoded = zellij.json.decode('{"a":1,"b":"two","c":null,"d":[1,2]}')
          return table.concat({
            tostring(decoded.a),
            decoded.b,
            tostring(decoded.c),
            tostring(#decoded.d),
            zellij.json.encode({ x = true }),
          }, ",")
        end
        return M
        "#,
    )
    .unwrap();
    assert_eq!(
        String::from_utf8(plugin.render(1, 100).unwrap()).unwrap(),
        // `c` is a real nil, not mlua's null userdata, so `if decoded.c then` answers the way
        // a Lua author expects.
        "1,two,nil,2,{\"x\":true}"
    );
}

#[test]
fn malformed_json_raises_rather_than_returning_nil() {
    let mut plugin = load(
        r#"
        local M = {}
        function M.render() return zellij.json.decode("{not json") end
        return M
        "#,
    )
    .unwrap();
    let err = format!("{:#}", plugin.render(1, 1).unwrap_err());
    assert!(err.contains("could not decode JSON"), "got: {}", err);
}

// Filesystem --------------------------------------------------------------------------------

#[test]
fn a_file_written_under_host_can_be_read_back() {
    let dirs = FsDirs::new();
    let rendered = render_with_dirs(
        r#"
        zellij.fs.write("/host/notes.txt", "hello")
        return zellij.fs.read("/host/notes.txt")
        "#,
        &dirs,
        file_permissions(),
    )
    .unwrap();
    assert_eq!(rendered, "hello");
    // ...and it really is on disk where the plugin's /host points, not somewhere else.
    assert_eq!(
        std::fs::read_to_string(dirs.host.path().join("notes.txt")).unwrap(),
        "hello"
    );
}

#[test]
fn each_root_is_a_different_folder() {
    let dirs = FsDirs::new();
    render_with_dirs(
        r#"
        zellij.fs.write("/host/where.txt", "host")
        zellij.fs.write("/data/where.txt", "data")
        zellij.fs.write("/cache/where.txt", "cache")
        return ""
        "#,
        &dirs,
        file_permissions(),
    )
    .unwrap();
    for (dir, expected) in [
        (dirs.host.path(), "host"),
        (dirs.data.path(), "data"),
        (dirs.cache.path(), "cache"),
    ] {
        assert_eq!(
            std::fs::read_to_string(dir.join("where.txt")).unwrap(),
            expected
        );
    }
}

#[test]
fn a_relative_path_is_read_against_host() {
    let dirs = FsDirs::new();
    std::fs::write(dirs.host.path().join("relative.txt"), "found").unwrap();
    let rendered = render_with_dirs(
        r#"return zellij.fs.read("relative.txt")"#,
        &dirs,
        file_permissions(),
    )
    .unwrap();
    assert_eq!(rendered, "found");
}

#[test]
fn listing_answers_in_the_root_the_caller_used() {
    // The point of re-spelling the paths: what comes out of `list` has to be something `read`
    // will accept, and a real host path is not.
    let dirs = FsDirs::new();
    std::fs::write(dirs.host.path().join("b.txt"), "b").unwrap();
    std::fs::create_dir(dirs.host.path().join("a_dir")).unwrap();
    let rendered = render_with_dirs(
        r#"
        local out = {}
        for _, entry in ipairs(zellij.fs.list("/host")) do
          table.insert(out, entry.path .. ":" .. tostring(entry.metadata.is_dir))
        end
        return table.concat(out, ",")
        "#,
        &dirs,
        file_permissions(),
    )
    .unwrap();
    assert_eq!(rendered, "/host/a_dir:true,/host/b.txt:false");
}

#[test]
fn metadata_is_nil_for_something_that_is_not_there() {
    let dirs = FsDirs::new();
    std::fs::write(dirs.host.path().join("real.txt"), "12345").unwrap();
    let rendered = render_with_dirs(
        r#"
        local there = zellij.fs.metadata("/host/real.txt")
        local missing = zellij.fs.metadata("/host/missing.txt")
        return string.format("%d,%s,%s", there.len, tostring(there.is_file), tostring(missing))
        "#,
        &dirs,
        file_permissions(),
    )
    .unwrap();
    assert_eq!(rendered, "5,true,nil");
}

#[test]
fn writes_append_and_create_folders_when_asked() {
    let dirs = FsDirs::new();
    let rendered = render_with_dirs(
        r#"
        zellij.fs.write("/host/deep/nested/log.txt", "one", { create_dirs = true })
        zellij.fs.write("/host/deep/nested/log.txt", "-two", { append = true })
        return zellij.fs.read("/host/deep/nested/log.txt")
        "#,
        &dirs,
        file_permissions(),
    )
    .unwrap();
    assert_eq!(rendered, "one-two");
}

#[test]
fn folders_and_files_can_be_made_renamed_and_removed() {
    let dirs = FsDirs::new();
    let rendered = render_with_dirs(
        r#"
        zellij.fs.mkdir("/host/work", true)
        zellij.fs.write("/host/work/a.txt", "x")
        zellij.fs.rename("/host/work/a.txt", "/host/work/b.txt")
        local after_rename = zellij.fs.read("/host/work/b.txt")
        zellij.fs.remove("/host/work", true)
        return after_rename .. "," .. tostring(zellij.fs.metadata("/host/work"))
        "#,
        &dirs,
        file_permissions(),
    )
    .unwrap();
    assert_eq!(rendered, "x,nil");
}

#[test]
fn a_read_of_a_missing_file_raises_rather_than_returning_nil() {
    let dirs = FsDirs::new();
    let err = format!(
        "{:#}",
        render_with_dirs(
            r#"return zellij.fs.read("/host/nope.txt")"#,
            &dirs,
            file_permissions(),
        )
        .unwrap_err()
    );
    assert!(err.contains("nope.txt"), "got: {}", err);
}

#[test]
fn a_read_larger_than_the_cap_is_refused() {
    let dirs = FsDirs::new();
    std::fs::write(dirs.host.path().join("big.txt"), vec![b'x'; 4096]).unwrap();
    let err = format!(
        "{:#}",
        render_with_dirs(
            r#"return zellij.fs.read("/host/big.txt", 16)"#,
            &dirs,
            file_permissions(),
        )
        .unwrap_err()
    );
    assert!(err.contains("limit"), "got: {}", err);
}

// Filesystem confinement --------------------------------------------------------------------

#[test]
fn a_path_cannot_climb_out_of_its_root() {
    // The load-bearing one. `translate_plugin_path`, which the pane-opening commands use, would
    // happily resolve this - which is exactly why these commands do not use it.
    let dirs = FsDirs::new();
    for source in [
        r#"return zellij.fs.read("/host/../../../../etc/passwd")"#,
        r#"return zellij.fs.read("/data/../../../../etc/passwd")"#,
        r#"return zellij.fs.write("/host/../escaped.txt", "x")"#,
    ] {
        let err = format!(
            "{:#}",
            render_with_dirs(source, &dirs, file_permissions()).unwrap_err()
        );
        assert!(
            err.contains("outside the plugin's own folders"),
            "expected {} to be refused, got: {}",
            source,
            err
        );
    }
}

#[test]
fn a_symlink_out_of_a_root_is_followed_and_then_refused() {
    // Containment has to be checked after the link is resolved, not before: the path as written
    // is entirely innocent.
    let dirs = FsDirs::new();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();
    std::os::unix::fs::symlink(outside.path(), dirs.host.path().join("escape")).unwrap();

    let err = format!(
        "{:#}",
        render_with_dirs(
            r#"return zellij.fs.read("/host/escape/secret.txt")"#,
            &dirs,
            file_permissions(),
        )
        .unwrap_err()
    );
    assert!(
        err.contains("outside the plugin's own folders"),
        "got: {}",
        err
    );
    // And it is named as a link: a plugin refused for `link.lua` should hear that it is one.
    assert!(err.contains("is a symlink"), "got: {}", err);
}

#[test]
fn an_absolute_path_outside_every_root_needs_full_hd_access() {
    let dirs = FsDirs::new();
    // Deliberately not a tempdir: `/tmp` *is* one of the four roots, so a path under it is
    // rewritten rather than escaping. This crate's own source tree is somewhere none of the
    // roots point at, which is what makes it the right target here.
    let outside = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("outside-the-roots.txt");
    std::fs::write(&outside, "elsewhere").unwrap();
    let _cleanup = RemoveOnDrop(outside.clone());
    let source = format!(r#"return zellij.fs.read("{}")"#, outside.display());

    let err = format!(
        "{:#}",
        render_with_dirs(&source, &dirs, file_permissions()).unwrap_err()
    );
    assert!(err.contains("FullHdAccess"), "got: {}", err);

    // ...and with it granted, the same read goes through. An absolute path is passed straight
    // to disk rather than rewritten, which is the other half of what makes this safe: nothing
    // silently redirects a path the plugin meant literally.
    let mut permitted = file_permissions().unwrap();
    permitted.insert(PermissionType::FullHdAccess);
    assert_eq!(
        render_with_dirs(&source, &dirs, Some(permitted)).unwrap(),
        "elsewhere"
    );
}

#[test]
fn renaming_out_of_the_plugins_folders_is_refused() {
    let dirs = FsDirs::new();
    std::fs::write(dirs.host.path().join("a.txt"), "x").unwrap();
    let source = format!(
        r#"return zellij.fs.rename("/host/a.txt", "{}")"#,
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("stolen.txt")
            .display()
    );
    let err = format!(
        "{:#}",
        render_with_dirs(&source, &dirs, file_permissions()).unwrap_err()
    );
    assert!(err.contains("FullHdAccess"), "got: {}", err);
}

// Filesystem permissions --------------------------------------------------------------------

#[test]
fn reading_and_writing_each_need_their_own_permission() {
    let dirs = FsDirs::new();
    std::fs::write(dirs.host.path().join("a.txt"), "x").unwrap();

    let read_only: Option<HashSet<PermissionType>> =
        Some([PermissionType::ReadFiles].into_iter().collect());
    let write_only: Option<HashSet<PermissionType>> =
        Some([PermissionType::WriteFiles].into_iter().collect());

    // A plugin that may read cannot write...
    let err = format!(
        "{:#}",
        render_with_dirs(
            r#"return zellij.fs.write("/host/b.txt", "x")"#,
            &dirs,
            read_only.clone(),
        )
        .unwrap_err()
    );
    assert!(err.contains("WriteFiles"), "got: {}", err);

    // ...and one that may write cannot read.
    let err = format!(
        "{:#}",
        render_with_dirs(r#"return zellij.fs.read("/host/a.txt")"#, &dirs, write_only,)
            .unwrap_err()
    );
    assert!(err.contains("ReadFiles"), "got: {}", err);

    // The reading half works on its own.
    assert_eq!(
        render_with_dirs(r#"return zellij.fs.read("/host/a.txt")"#, &dirs, read_only).unwrap(),
        "x"
    );
}

#[test]
fn a_plugin_that_asked_for_nothing_cannot_touch_the_filesystem() {
    let dirs = FsDirs::new();
    std::fs::write(dirs.host.path().join("a.txt"), "x").unwrap();
    let err = format!(
        "{:#}",
        render_with_dirs(r#"return zellij.fs.read("/host/a.txt")"#, &dirs, None).unwrap_err()
    );
    assert!(
        err.contains("ReadFiles") && err.contains("request_permission"),
        "the refusal should say what to ask for, got: {}",
        err
    );
}

#[test]
fn the_tmp_root_is_zellijs_own_temp_folder() {
    // Worth pinning because it is the one root that is *shared*: every plugin in every session
    // sees the same folder. It also means a real host path beginning `/tmp` is read as the
    // virtual root rather than taken literally - the same collision the wasm preopens have.
    let dirs = FsDirs::new();
    render_with_dirs(
        r#"return zellij.fs.write("/tmp/lua-sandbox-test.txt", "shared")"#,
        &dirs,
        file_permissions(),
    )
    .unwrap();
    let written = zellij_utils::consts::ZELLIJ_TMP_DIR.join("lua-sandbox-test.txt");
    let _cleanup = RemoveOnDrop(written.clone());
    assert_eq!(std::fs::read_to_string(&written).unwrap(), "shared");
    // and not in /host, which is where a relative path would have gone
    assert!(!dirs.host.path().join("lua-sandbox-test.txt").exists());
}

// -- Event ergonomics -------------------------------------------------------------------------

/// Run `body` as an `update` handler for `event` and hand back what it returned, as a string.
fn describe_event(event: Event, body: &str) -> String {
    let source = format!(
        "local M = {{}}\nM.out = ''\nfunction M.update(event)\n  M.out = (function()\n{}\n  end)()\n  return true\nend\nfunction M.render() return M.out end\nreturn M",
        body
    );
    let mut plugin = load(&source).unwrap();
    plugin.update(&event).unwrap();
    String::from_utf8(plugin.render(1, 200).unwrap()).unwrap()
}

/// `Event::Hook` is a struct variant, so its fields are merged into the event: `point` is the
/// name the config spells, and `payload` the map the fire site built.
#[test]
fn a_hook_event_has_its_point_and_payload() {
    use zellij_utils::input::hooks::HookPoint;
    let event = Event::Hook {
        point: HookPoint::AgentWaiting,
        payload: BTreeMap::from([("agent_pane_id".to_owned(), "7".to_owned())]),
    };
    let out = describe_event(
        event,
        r#"return event.name .. "|" .. event.point .. "|" .. event.payload.agent_pane_id"#,
    );
    assert_eq!(out, "hook|agent_waiting|7");
}

#[test]
fn a_hook_handler_registered_with_on_runs() {
    use zellij_utils::input::hooks::HookPoint;
    let mut plugin = load(
        r#"
        local zellij = require("zellij")
        local M = { seen = {} }
        zellij.on("hook", function(event)
          table.insert(M.seen, event.point .. "=" .. (event.payload.session_name or "?"))
        end)
        function M.render() return table.concat(M.seen, ",") end
        return M
        "#,
    )
    .unwrap();
    plugin
        .update(&Event::Hook {
            point: HookPoint::SessionStart,
            payload: BTreeMap::from([("session_name".to_owned(), "main".to_owned())]),
        })
        .unwrap();
    plugin
        .update(&Event::Hook {
            point: HookPoint::PaneExit,
            payload: BTreeMap::new(),
        })
        .unwrap();
    assert_eq!(
        String::from_utf8(plugin.render(1, 80).unwrap()).unwrap(),
        "session_start=main,pane_exit=?"
    );
}

/// A tuple variant arrives as a bare list, and `RunCommandResult`'s output as byte numbers.
/// Both are named and decoded, and `value` still holds the list for anyone reading it.
#[test]
fn a_command_result_has_named_fields_and_string_output() {
    let event = Event::RunCommandResult(
        Some(3),
        b"out\n".to_vec(),
        "é".as_bytes().to_vec(),
        BTreeMap::from([("k".to_owned(), "v".to_owned())]),
    );
    let out = describe_event(
        event,
        r#"return event.exit_code .. "|" .. event.stdout .. "|" .. event.stderr .. "|" .. event.context.k .. "|" .. #event.value"#,
    );
    // `render` translates the newline for the terminal, as it does for any output.
    assert_eq!(out, "3|out\n\r|é|v|4");
}

/// An absent exit code is `nil` in the middle of the list. Lua's `#` stops there, so the
/// fields are walked by position rather than read off the list's length.
#[test]
fn an_absent_exit_code_does_not_shift_the_fields_after_it() {
    let event =
        Event::CommandPaneExited(7, None, BTreeMap::from([("a".to_owned(), "b".to_owned())]));
    let out = describe_event(
        event,
        r#"return tostring(event.pane_id) .. "|" .. tostring(event.exit_code) .. "|" .. event.context.a"#,
    );
    assert_eq!(out, "7|nil|b");
}

#[test]
fn a_web_result_body_is_a_string_and_a_stream_chunk_too() {
    let event = Event::WebRequestResult(
        200,
        BTreeMap::from([("content-type".to_owned(), "text/plain".to_owned())]),
        b"body".to_vec(),
        BTreeMap::new(),
    );
    let out = describe_event(
        event,
        r#"return event.status .. "|" .. event.headers["content-type"] .. "|" .. event.body"#,
    );
    assert_eq!(out, "200|text/plain|body");

    let event = Event::WebRequestStreamChunk {
        stream_id: "s".to_owned(),
        chunk: b"chunk".to_vec(),
        done: true,
        error: None,
    };
    let out = describe_event(
        event,
        r#"return event.stream_id .. "|" .. event.chunk .. "|" .. tostring(event.done)"#,
    );
    assert_eq!(out, "s|chunk|true");
}

#[test]
fn a_mouse_event_says_what_kind_and_where() {
    let out = describe_event(
        Event::Mouse(zellij_utils::data::Mouse::LeftClick(3, 4)),
        r#"return event.kind .. "@" .. event.line .. "," .. event.col"#,
    );
    assert_eq!(out, "left_click@3,4");
    let out = describe_event(
        Event::Mouse(zellij_utils::data::Mouse::ScrollUp(2)),
        r#"return event.kind .. "x" .. event.lines"#,
    );
    assert_eq!(out, "scroll_upx2");
}

#[test]
fn file_system_events_carry_named_pairs() {
    let event = Event::FileSystemUpdate(vec![(PathBuf::from("/a/b.txt"), None)]);
    let out = describe_event(
        event,
        r#"return event.files[1].path .. "|" .. tostring(event.files[1].metadata) .. "|" .. event.value[1][1]"#,
    );
    assert_eq!(out, "/a/b.txt|nil|/a/b.txt");
}

/// A map keyed by pane id arrives keyed by *tables*, which nothing can look up. It is re-keyed
/// by the `terminal:3` spelling `zellij.pane_key` produces, so a plugin can.
#[test]
fn a_render_report_can_be_looked_up_by_pane() {
    use zellij_utils::data::{PaneContents, PaneId};
    let mut report = std::collections::HashMap::new();
    report.insert(
        PaneId::Terminal(3),
        PaneContents {
            viewport: vec!["hello".to_owned()],
            ..Default::default()
        },
    );
    let out = describe_event(
        Event::PaneRenderReport(report),
        r#"return event.panes[zellij.pane_key(zellij.terminal_pane(3))].viewport[1] .. "|" .. event.panes["terminal:3"].viewport[1]"#,
    );
    assert_eq!(out, "hello|hello");
}

#[test]
fn bytes_to_string_copes_with_more_than_a_stack_of_bytes() {
    let source = r#"
        local M = {}
        function M.render()
          local bytes = {}
          for i = 1, 10000 do bytes[i] = 65 end
          local s = zellij.bytes_to_string(bytes)
          return #s .. s:sub(1, 3)
        end
        return M
    "#;
    let mut plugin = load(source).unwrap();
    assert_eq!(plugin.render(1, 1).unwrap(), b"10000AAA");
}

// -- Text and UI components -------------------------------------------------------------------

/// Run `body` as `render` and hand back the string; for the pure helpers.
fn eval(body: &str) -> String {
    let source = format!(
        "local M = {{}}\nfunction M.render()\n{}\nend\nreturn M",
        body
    );
    let mut plugin = load(&source).unwrap();
    String::from_utf8(plugin.render(1, 200).unwrap()).unwrap()
}

/// Width is columns, which is neither bytes nor code points: an accent takes none of its own
/// and a CJK character takes two.
#[test]
fn text_width_measures_columns() {
    assert_eq!(
        eval(
            r#"return zellij.text.width("héllo") .. "," .. zellij.text.width("日本") .. "," .. #"héllo""#
        ),
        "5,4,6"
    );
}

#[test]
fn text_helpers_pad_and_cut_by_columns() {
    assert_eq!(
        eval(r#"return "[" .. zellij.text.pad("日本", 5) .. "]""#),
        "[日本 ]"
    );
    assert_eq!(
        eval(r#"return "[" .. zellij.text.pad_left("ab", 4) .. "]""#),
        "[  ab]"
    );
    assert_eq!(
        eval(r#"return "[" .. zellij.text.center("ab", 5) .. "]""#),
        "[ ab  ]"
    );
    assert_eq!(
        eval(r#"return zellij.text.truncate("hello world", 6)"#),
        "hello…"
    );
    assert_eq!(eval(r#"return zellij.text.truncate("hello", 6)"#), "hello");
    // A wide character that does not fit is left out rather than half-drawn.
    assert_eq!(
        eval(r#"return zellij.text.truncate("日本語", 4, "")"#),
        "日本"
    );
}

/// The ellipsis is a character too. Cutting it with `sub` handed the terminal the first *byte*
/// of "…", which is not a character at all.
#[test]
fn truncating_into_less_room_than_the_ellipsis_still_returns_characters() {
    // One column leaves room for the ellipsis and nothing else.
    assert_eq!(eval(r#"return zellij.text.truncate("hello", 1)"#), "…");
    // No room at all, and a custom ellipsis wider than what is left.
    assert_eq!(eval(r#"return zellij.text.truncate("hello", 0)"#), "");
    assert_eq!(eval(r#"return zellij.text.truncate("hello", -1)"#), "");
    assert_eq!(eval(r#"return zellij.text.truncate("hello", 1, "日")"#), "");
    assert_eq!(
        eval(r#"return zellij.text.truncate("hello", 2, "日")"#),
        "日"
    );
    assert_eq!(
        eval(r#"return zellij.text.truncate("hello", 1, "ab")"#),
        "a"
    );
    // Whatever comes back is well-formed and fits, which is the whole contract.
    for (source, width) in [
        (r#"zellij.text.truncate("héllo wörld", 1)"#, 1),
        (r#"zellij.text.truncate("日本語です", 3)"#, 3),
        (r#"zellij.text.truncate("hello", 2, "……")"#, 2),
    ] {
        let out = eval(&format!("return {source}"));
        assert!(
            std::str::from_utf8(out.as_bytes()).is_ok(),
            "{source} produced invalid UTF-8"
        );
        assert!(
            unicode_width::UnicodeWidthStr::width(out.as_str()) <= width,
            "{source} produced {out:?}, wider than {width}"
        );
    }
}

/// The wire form `zellij-tile`'s `Text::serialize` produces: flags, one `$`-terminated index
/// group per colour level, then the UTF-8 bytes in decimal.
#[test]
fn a_text_component_serializes_the_way_zellij_tile_does() {
    assert_eq!(
        eval(r#"return zellij.ui.text("hi"):selected():emphasis(0, 0, 1):render()"#),
        "\u{1b}Pztext;x0$104,105\u{1b}\\"
    );
    // Levels are positional: error is level 6, so six empty groups come first.
    assert_eq!(
        eval(r#"return zellij.ui.text("hi"):error(1, 2):render { x = 3, y = 1, width = 10 }"#),
        "\u{1b}Pztext;3/1/10/;$$$$$$1$104,105\u{1b}\\"
    );
    assert_eq!(
        eval(r#"return zellij.ui.text("é"):opaque():disabled():render()"#),
        "\u{1b}Pztext;zd195,169\u{1b}\\"
    );
    // Positions count characters, not bytes.
    assert_eq!(
        eval(r#"return zellij.ui.text("éa"):dim():render()"#),
        "\u{1b}Pztext;$$$$0,1$195,169,97\u{1b}\\"
    );
    assert_eq!(
        eval(r#"return zellij.ui.ribbon("tab"):selected():render()"#),
        "\u{1b}Pzribbon;x116,97,98\u{1b}\\"
    );
    assert_eq!(
        eval(r#"return tostring(zellij.ui.text("a"))"#),
        "\u{1b}Pztext;97\u{1b}\\"
    );
}

#[test]
fn table_and_list_components_serialize_the_way_zellij_tile_does() {
    assert_eq!(
        eval(
            r#"return zellij.ui.table { { "A", "B" }, { "C", zellij.ui.text("D"):selected() } }:render()"#
        ),
        "\u{1b}Pztable;2;2;65;66;67;x68\u{1b}\\"
    );
    assert_eq!(
        eval(r#"return zellij.ui.table():row("A"):row("B"):render { x = 0, y = 0 }"#),
        "\u{1b}Pztable;0/0//;1;2;65;66\u{1b}\\"
    );
    assert_eq!(
        eval(
            r#"return zellij.ui.nested_list { "hi", { "go", indent = 1 }, zellij.ui.text("x"):opaque():disabled() }:render()"#
        ),
        // Disabled is dropped on a list item: the host does not read it there.
        "\u{1b}Pznested_list;104,105;|103,111;z120\u{1b}\\"
    );
    assert_eq!(
        eval(r#"return zellij.ui.nested_list():item("a"):item("b", 2):render()"#),
        "\u{1b}Pznested_list;97;||98\u{1b}\\"
    );
}

// -- require ------------------------------------------------------------------------------------

/// A plugin split across files, loaded the way the real loader loads it: with the path it was
/// read from recorded, which is what `require` resolves against.
fn load_from_dir(dir: &std::path::Path, main: &str) -> Result<super::LuaPlugin> {
    let main_path = dir.join("main.lua");
    std::fs::write(&main_path, main).unwrap();
    let mut env = test_env();
    env.resolved_source_path = Some(main_path.clone());
    load_lua_plugin(main, &main_path.display().to_string(), env)
}

#[test]
fn a_plugin_can_require_a_file_beside_it_once() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("lib.lua"),
        "local lib = {}\nlib.loads = (lib.loads or 0) + 1\nfunction lib.greet(n) return 'hi ' .. n end\nreturn lib",
    )
    .unwrap();
    std::fs::create_dir(dir.path().join("pkg")).unwrap();
    std::fs::write(dir.path().join("pkg/init.lua"), "return { name = 'pkg' }").unwrap();
    std::fs::write(dir.path().join("pkg/sub.lua"), "return { name = 'sub' }").unwrap();
    let mut plugin = load_from_dir(
        dir.path(),
        r#"
        local zellij = require("zellij")
        local lib = require("lib")
        local again = require("lib")
        local pkg = require("pkg")
        local sub = require("pkg.sub")
        local M = {}
        function M.render()
          return lib.greet("x") .. "|" .. tostring(lib == again) .. "|" .. pkg.name .. "|" .. sub.name
        end
        return M
        "#,
    )
    .unwrap();
    assert_eq!(plugin.render(1, 100).unwrap(), b"hi x|true|pkg|sub");
}

#[test]
fn require_is_confined_to_the_plugins_folder() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.lua"), "return 'leaked'").unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.lua"),
        dir.path().join("link.lua"),
    )
    .unwrap();
    for (module, expected) in [
        ("../secret", "not a path"),
        ("/etc/passwd", "not a path"),
        ("link", "outside the plugin's folder"),
        ("missing", "no module found"),
    ] {
        let error = load_from_dir(dir.path(), &format!("require({:?})\nreturn {{}}", module))
            .err()
            .map(|e| format!("{:#}", e))
            .unwrap_or_else(|| panic!("require({module:?}) should have been refused"));
        assert!(error.contains(expected), "require({module:?}): {error}");
    }
}

#[test]
fn a_circular_require_is_an_error_rather_than_a_stack_overflow() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.lua"), "require('b')\nreturn {}").unwrap();
    std::fs::write(dir.path().join("b.lua"), "require('a')\nreturn {}").unwrap();
    let error = load_from_dir(dir.path(), "require('a')\nreturn {}")
        .err()
        .map(|e| format!("{:#}", e))
        .unwrap();
    assert!(error.contains("circular"), "{error}");
}

/// The in-flight mark must be something a module cannot return: marking with the string
/// "loading" made a module that legitimately returns it look circular forever after.
#[test]
fn a_module_may_return_the_value_the_cycle_check_marks_with() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lib.lua"), "return 'loading'").unwrap();
    let mut plugin = load_from_dir(
        dir.path(),
        r#"
        local first = require("lib")
        local again = require("lib")
        local M = {}
        function M.render() return first .. "|" .. again end
        return M
        "#,
    )
    .unwrap();
    assert_eq!(plugin.render(1, 100).unwrap(), b"loading|loading");
}

/// A plugin that was not loaded from a file - a builtin, or a test - has no folder, and says
/// so rather than reading relative to the server's cwd.
#[test]
fn require_without_a_folder_serves_only_zellij() {
    let error = load("require('lib')\nreturn {}")
        .err()
        .map(|e| format!("{:#}", e))
        .unwrap();
    assert!(error.contains("no folder"), "{error}");
    assert!(load("local z = require('zellij')\nreturn {}").is_ok());
}

// ------------------------------------------------------------------ decoded replies

/// Every command whose reply `zellij-tile/src/shim.rs` reads off its stdin pipe. A Lua plugin
/// has no shim, so each of these needs an arm in `response.rs`; one that falls through to the
/// raw bytes is a regression, and one decoded here but absent from this list is a drift in the
/// other direction. Add to this list when the shim gains a reply, and to `response.rs` with it.
const REPLYING_COMMANDS: &[CommandType] = &[
    CommandType::CheckLuaPlugin,
    CommandType::GetPluginIds,
    CommandType::GetZellijVersion,
    CommandType::GenerateRandomName,
    CommandType::GetUserPluginDir,
    CommandType::GetLayoutDir,
    CommandType::GetSessionEnvironmentVariables,
    CommandType::GetPluginAliasConfiguration,
    CommandType::GetFocusedPaneInfo,
    CommandType::GetPaneInfo,
    CommandType::GetTabInfo,
    CommandType::GetPaneCwd,
    CommandType::GetPanePid,
    CommandType::GetPaneRunningCommand,
    CommandType::GetPaneGitProbe,
    CommandType::GetPaneScrollback,
    CommandType::GetSessionList,
    CommandType::CurrentSessionLastSavedTime,
    CommandType::SaveSession,
    CommandType::DumpLayout,
    CommandType::DumpSessionLayout,
    CommandType::ParseLayout,
    CommandType::SaveLayout,
    CommandType::DeleteLayout,
    CommandType::RenameLayout,
    CommandType::EditLayout,
    CommandType::ShowFloatingPanes,
    CommandType::HideFloatingPanes,
    CommandType::DeleteDeadSessionAndReply,
    CommandType::DeleteAllDeadSessionsAndReply,
    CommandType::KillSessionsAndReply,
    CommandType::WarpPaneToSession,
    CommandType::WarpPaneFromSession,
    CommandType::WarpTabToSession,
    CommandType::WarpTabFromSession,
    CommandType::AdoptOrphanedPane,
    CommandType::ListOrphanedPanes,
    CommandType::DataDefineEntity,
    CommandType::DataApply,
    CommandType::DataFetch,
    CommandType::DataCount,
    CommandType::DataWatch,
    CommandType::DataUnwatch,
    CommandType::EnqueueQueueJob,
    CommandType::QueueStatus,
    CommandType::ListRecordings,
    CommandType::SearchRecordings,
    CommandType::ListNotifications,
    CommandType::NewTab,
    CommandType::NewTabUnfocused,
    CommandType::NewTabsWithLayout,
    CommandType::NewTabsWithLayoutInfo,
    CommandType::FocusOrCreateTab,
    CommandType::BreakPanesToNewTab,
    CommandType::BreakPanesToTabWithIndex,
    CommandType::BreakPanesToTabWithId,
    CommandType::NewTiledPaneInTab,
    CommandType::OpenFile,
    CommandType::OpenFileFloating,
    CommandType::OpenFileInPlace,
    CommandType::OpenFileNearPlugin,
    CommandType::OpenFileFloatingNearPlugin,
    CommandType::OpenFileInPlaceOfPlugin,
    CommandType::OpenTerminal,
    CommandType::OpenTerminalFloating,
    CommandType::OpenTerminalInPlace,
    CommandType::OpenTerminalNearPlugin,
    CommandType::OpenTerminalFloatingNearPlugin,
    CommandType::OpenTerminalInPlaceOfPlugin,
    CommandType::OpenTerminalPaneInPlaceOfPaneId,
    CommandType::OpenEditPaneInPlaceOfPaneId,
    CommandType::OpenCommandPane,
    CommandType::OpenCommandPaneFloating,
    CommandType::OpenCommandPaneInPlace,
    CommandType::OpenCommandPaneBackground,
    CommandType::OpenCommandPaneNearPlugin,
    CommandType::OpenCommandPaneOverlay,
    CommandType::OpenCommandPaneFloatingNearPlugin,
    CommandType::OpenCommandPaneInPlaceOfPlugin,
    CommandType::OpenCommandPaneInPlaceOfPaneId,
    CommandType::OpenPluginPaneFloating,
    CommandType::OpenCommandPaneInGitCheckoutNearPlugin,
    CommandType::OpenCommandPaneInNewTab,
    CommandType::OpenPluginPaneInNewTab,
    CommandType::OpenEditorPaneInNewTab,
    CommandType::GenerateWebLoginToken,
    CommandType::RevokeWebLoginToken,
    CommandType::RevokeAllWebLoginTokens,
    CommandType::RenameWebLoginToken,
    CommandType::ListWebLoginTokens,
    CommandType::FsRead,
    CommandType::FsList,
    CommandType::FsMetadata,
    CommandType::FsWrite,
    CommandType::FsRemove,
    CommandType::FsCreateDir,
    CommandType::FsRename,
];

#[test]
fn every_reply_the_shim_reads_is_decoded() {
    // An empty byte slice decodes to prost's default message, so `Some` versus `None` is the
    // whole question here: whether the command has an arm at all. An `Err` inside the `Some`
    // (a default `oneof` is "answered with nothing") is a decoded reply, not a missing one.
    let lua = mlua::Lua::new();
    let listed: HashSet<CommandType> = REPLYING_COMMANDS.iter().copied().collect();
    assert_eq!(
        listed.len(),
        REPLYING_COMMANDS.len(),
        "a command is listed twice"
    );
    for command in CommandType::iter() {
        let decoded = super::response::response_to_lua(&lua, command, &[]).is_some();
        if listed.contains(&command) {
            assert!(
                decoded,
                "{command} answers, but its reply falls through to raw bytes"
            );
        } else {
            assert!(
                !decoded,
                "{command} is decoded but not listed in REPLYING_COMMANDS - add it"
            );
        }
    }
}

/// Decode `bytes` as `command`'s reply and describe the result with a Lua expression over `v`.
fn decoded(command: CommandType, bytes: &[u8], describe: &str) -> String {
    let lua = mlua::Lua::new();
    let value = super::response::response_to_lua(&lua, command, bytes)
        .unwrap_or_else(|| panic!("{command} has no decoder"))
        .unwrap_or_else(|e| panic!("{command}: {e:#}"));
    lua.globals().set("v", value).unwrap();
    lua.load(describe).eval::<String>().unwrap()
}

/// Decode `bytes` as `command`'s reply and hand back the error it raised.
fn decode_error(command: CommandType, bytes: &[u8]) -> String {
    let lua = mlua::Lua::new();
    let error = super::response::response_to_lua(&lua, command, bytes)
        .unwrap_or_else(|| panic!("{command} has no decoder"))
        .err()
        .unwrap_or_else(|| panic!("{command} should have raised"));
    format!("{error:#}")
}

#[test]
fn a_pane_id_reply_is_the_same_table_the_prelude_builds() {
    use prost::Message;
    use zellij_utils::plugin_api::plugin_command::{
        ProtobufOpenCommandPaneNearPluginResponse, ProtobufPaneId,
    };
    let reply = ProtobufOpenCommandPaneNearPluginResponse {
        pane_id: Some(ProtobufPaneId {
            pane_type: 0, // terminal
            id: 7,
        }),
    }
    .encode_to_vec();
    assert_eq!(
        decoded(
            CommandType::OpenCommandPaneNearPlugin,
            &reply,
            "return type(v) .. ' ' .. tostring(v.Terminal) .. ' ' .. tostring(v.Plugin)"
        ),
        "table 7 nil"
    );
    // No pane id means the pane was not opened, and that is `nil`, not an error: the shim
    // returns `Option<PaneId>` for this family.
    let empty = ProtobufOpenCommandPaneNearPluginResponse { pane_id: None }.encode_to_vec();
    assert_eq!(
        decoded(
            CommandType::OpenCommandPaneNearPlugin,
            &empty,
            "return type(v)"
        ),
        "nil"
    );
}

#[test]
fn a_success_or_error_reply_is_true_or_raises() {
    use prost::Message;
    use zellij_utils::plugin_api::plugin_command::{
        save_layout_response, ProtobufSaveLayoutResponse,
    };
    let ok = ProtobufSaveLayoutResponse {
        result: Some(save_layout_response::Result::Success(true)),
    }
    .encode_to_vec();
    assert_eq!(
        decoded(CommandType::SaveLayout, &ok, "return tostring(v)"),
        "true"
    );
    let failed = ProtobufSaveLayoutResponse {
        result: Some(save_layout_response::Result::Error(
            "a layout by that name exists".to_owned(),
        )),
    }
    .encode_to_vec();
    assert_eq!(
        decode_error(CommandType::SaveLayout, &failed),
        "a layout by that name exists"
    );
}

#[test]
fn a_pane_in_a_new_tab_arrives_as_named_fields() {
    use prost::Message;
    use zellij_utils::plugin_api::plugin_command::{
        ProtobufOpenPaneInNewTabResponse, ProtobufPaneId,
    };
    let reply = ProtobufOpenPaneInNewTabResponse {
        tab_id: Some(2),
        pane_id: Some(ProtobufPaneId {
            pane_type: 1, // plugin
            id: 9,
        }),
    }
    .encode_to_vec();
    assert_eq!(
        decoded(
            CommandType::OpenCommandPaneInNewTab,
            &reply,
            "return v.tab_id .. ' ' .. v.pane_id.Plugin"
        ),
        "2 9"
    );
}

#[test]
fn the_session_list_arrives_as_the_snapshot_the_shim_builds() {
    use prost::Message;
    use zellij_utils::data::{GetSessionListResponse, SessionInfo, SessionListSnapshot};
    use zellij_utils::plugin_api::plugin_command::ProtobufGetSessionListResponse;
    let snapshot = SessionListSnapshot {
        live_sessions: vec![SessionInfo {
            name: "work".to_owned(),
            connected_clients: 2,
            is_current_session: true,
            ..Default::default()
        }],
        resurrectable_sessions: vec![("yesterday".to_owned(), std::time::Duration::from_secs(90))],
    };
    let reply =
        ProtobufGetSessionListResponse::from(GetSessionListResponse::Ok(snapshot)).encode_to_vec();
    assert_eq!(
        decoded(
            CommandType::GetSessionList,
            &reply,
            r#"local live = v.live_sessions[1]
               local dead = v.resurrectable_sessions[1]
               return live.name .. ' ' .. live.connected_clients .. ' ' .. tostring(live.is_current_session)
                 .. ' | ' .. dead[1] .. ' ' .. dead[2].secs"#
        ),
        "work 2 true | yesterday 90"
    );
    let reply = ProtobufGetSessionListResponse::from(GetSessionListResponse::Err(
        "not scanning".to_owned(),
    ))
    .encode_to_vec();
    assert_eq!(
        decode_error(CommandType::GetSessionList, &reply),
        "not scanning"
    );
}

#[test]
fn a_pane_scrollback_reply_is_the_pane_contents() {
    use prost::Message;
    use zellij_utils::data::{PaneContents, PaneScrollbackResponse};
    use zellij_utils::plugin_api::event::ProtobufPaneScrollbackResponse;
    let contents = PaneContents {
        viewport: vec!["$ ls".to_owned(), "notes.txt".to_owned()],
        ..Default::default()
    };
    let reply = ProtobufPaneScrollbackResponse::try_from(PaneScrollbackResponse::Ok(contents))
        .unwrap()
        .encode_to_vec();
    assert_eq!(
        decoded(
            CommandType::GetPaneScrollback,
            &reply,
            "return #v.viewport .. ' ' .. v.viewport[2]"
        ),
        "2 notes.txt"
    );
    let reply = ProtobufPaneScrollbackResponse::try_from(PaneScrollbackResponse::Err(
        "no such pane".to_owned(),
    ))
    .unwrap()
    .encode_to_vec();
    assert_eq!(
        decode_error(CommandType::GetPaneScrollback, &reply),
        "no such pane"
    );
}

#[test]
fn web_login_tokens_arrive_as_one_row_per_token() {
    use prost::Message;
    use zellij_utils::plugin_api::plugin_command::ListTokensResponse;
    let reply = ListTokensResponse {
        tokens: vec!["laptop".to_owned(), "phone".to_owned()],
        creation_times: vec!["2026-01-01".to_owned(), "2026-02-02".to_owned()],
        read_only_flags: vec![false, true],
        error: None,
    }
    .encode_to_vec();
    assert_eq!(
        decoded(
            CommandType::ListWebLoginTokens,
            &reply,
            "return #v .. ' ' .. v[2].token .. ' ' .. v[2].creation_time .. ' ' .. tostring(v[2].read_only)"
        ),
        "2 phone 2026-02-02 true"
    );
}

/// `parse_layout` is answered by the host itself, so it is the one `oneof { ok | error }` reply
/// a plugin with no channels behind it can exercise end to end - the prelude's call, the
/// dispatcher, the handler writing its reply, and `response.rs` reading it back.
#[test]
fn a_parsed_layout_reaches_lua_as_metadata_and_a_bad_one_raises() {
    let mut plugin = load_permitted(
        r#"
        local zellij = require("zellij")
        local M = {}
        function M.load()
          local metadata = zellij.parse_layout([[
            layout {
              tab name="editor" {
                pane
              }
            }
          ]])
          M.tabs = #metadata.tabs
          M.name = metadata.tabs[1].name
          local ok, err = pcall(zellij.parse_layout, "layout { this is not kdl")
          M.ok = tostring(ok)
          M.err = tostring(err)
        end
        function M.render()
          return M.tabs .. " " .. M.name .. " " .. M.ok .. " " .. M.err
        end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    let rendered = String::from_utf8(plugin.render(1, 400).unwrap()).unwrap();
    assert!(
        rendered.starts_with("1 editor false "),
        "the good layout should decode and the bad one raise, got: {rendered}"
    );
    assert!(
        rendered.contains("parse_layout_api"),
        "the raised error should name where the parser was looking, got: {rendered}"
    );
}

/// With no session scan running the host answers `GetSessionList` with its error arm, which
/// is the real dispatch path for a data-typed `oneof` reply raising into Lua.
#[test]
fn an_unavailable_session_list_raises_the_hosts_reason() {
    let mut plugin = load_permitted(
        r#"
        local zellij = require("zellij")
        local M = {}
        function M.load()
          local ok, err = pcall(zellij.get_session_list)
          M.result = tostring(ok) .. " " .. tostring(err)
        end
        function M.render() return M.result end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    let rendered = String::from_utf8(plugin.render(1, 200).unwrap()).unwrap();
    assert!(
        rendered.starts_with("false ") && rendered.contains("Session-scan state"),
        "got: {rendered}"
    );
}

/// A pane-opening command whose pty thread is not there to answer comes back with no pane id,
/// which reaches Lua as `nil` rather than as protobuf bytes.
#[test]
fn a_pane_that_could_not_be_opened_is_nil() {
    let mut plugin = load_permitted(
        r#"
        local zellij = require("zellij")
        local M = {}
        function M.load()
          M.result = type(zellij.open_terminal_near_plugin { path = "/host" })
        end
        function M.render() return M.result end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    assert_eq!(plugin.render(1, 40).unwrap(), b"nil");
}

// -- The dashboard example, the data sugar, and intervals -----------------------------------------

/// `example-plugins/dashboard.lua` is the worked example for rendering, so it is loaded, fed the
/// events it subscribes to, and pressed, the way the playground is.
#[test]
fn the_dashboard_example_loads_renders_components_and_takes_input() {
    use zellij_utils::data::{PaneInfo, PaneManifest, TabInfo};
    let source = example_plugin("dashboard.lua");
    let mut plugin = load_permitted(&source).unwrap();
    plugin.load(&BTreeMap::new()).unwrap();

    let tab = TabInfo {
        position: 0,
        name: "main".to_owned(),
        active: true,
        ..Default::default()
    };
    plugin.update(&Event::TabUpdate(vec![tab])).unwrap();
    let mut manifest = PaneManifest::default();
    manifest.panes.insert(
        0,
        vec![PaneInfo {
            id: 3,
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
    plugin.update(&Event::Timer(0.0)).unwrap();
    for key in ["j", "k", "x"] {
        plugin
            .update(&Event::Key(KeyWithModifier::new(BareKey::Char(
                key.chars().next().unwrap(),
            ))))
            .unwrap();
    }
    plugin
        .update(&Event::Mouse(zellij_utils::data::Mouse::LeftClick(3, 1)))
        .unwrap();

    let rendered = String::from_utf8(plugin.render(20, 60).unwrap()).unwrap();
    assert!(
        rendered.contains("\u{1b}Pzribbon;x"),
        "no selected ribbon: {rendered}"
    );
    assert!(
        rendered.contains("\u{1b}Pztable;3;2;"),
        "no table: {rendered}"
    );
    assert!(
        rendered.contains("\u{1b}Pznested_list;"),
        "no list: {rendered}"
    );
    assert!(
        rendered.contains("up 3 days"),
        "no command output: {rendered}"
    );
    // List items travel as their bytes in decimal, so the text is matched in that form.
    let encoded = |s: &str| {
        s.bytes()
            .map(|b| b.to_string())
            .collect::<Vec<_>>()
            .join(",")
    };
    assert!(
        rendered.contains(&encoded("clicked row 1")),
        "the click did not land: {rendered}"
    );
}

/// The data helpers produce exactly the Rust shapes the `data_*` commands deserialize:
/// externally tagged values and predicates, pairs for mutations, positional sort directions.
#[test]
fn the_data_helpers_spell_the_stores_shapes() {
    let json = |body: &str| eval(&format!("return zellij.json.encode({body})"));
    assert_eq!(json(r#"zellij.data.value(1)"#), r#"{"Int":1}"#);
    assert_eq!(json(r#"zellij.data.value(1.5)"#), r#"{"Real":1.5}"#);
    assert_eq!(json(r#"zellij.data.value("a")"#), r#"{"Text":"a"}"#);
    assert_eq!(json(r#"zellij.data.value(true)"#), r#"{"Bool":true}"#);
    assert_eq!(json(r#"zellij.data.value(nil)"#), r#""Null""#);
    assert_eq!(
        json(r#"zellij.data.value({ blob = "ab" })"#),
        r#"{"Blob":[97,98]}"#
    );
    assert_eq!(
        json(r#"zellij.data.value({ json = { a = 1 } })"#),
        r#"{"Text":"{\"a\":1}"}"#
    );
    assert_eq!(
        json(r#"zellij.data.eq("done", false)"#),
        r#"{"Eq":["done",{"Bool":false}]}"#
    );
    assert_eq!(
        json(
            r#"zellij.data.all(zellij.data.is_null("a"), zellij.data.negate(zellij.data.is_in("b", { 1, 2 })))"#
        ),
        r#"{"And":[{"IsNull":"a"},{"Not":{"In":["b",[{"Int":1},{"Int":2}]]}}]}"#
    );
    assert_eq!(
        json(r#"zellij.data.put("note", { title = "t", done = false }, "u1")"#),
        r#"{"Put":{"entity":"note","uid":"u1","values":[["done",{"Bool":false}],["title",{"Text":"t"}]]}}"#
    );
    // `nil` cannot survive `pairs`, so a mutation that clears an attribute needs the sentinel -
    // and the string the store spells `Null` with is still an ordinary `Text` a plugin may write.
    assert_eq!(json(r#"zellij.data.value(zellij.data.null)"#), r#""Null""#);
    assert_eq!(
        json(r#"zellij.data.update("note", nil, { done = zellij.data.null, title = "Null" })"#),
        r#"{"Update":{"entity":"note","values":[["done","Null"],["title",{"Text":"Null"}]]}}"#
    );
    assert_eq!(
        json(r#"zellij.data.delete("note", zellij.data.prefix("title", "x"))"#),
        r#"{"Delete":{"entity":"note","filter":{"Prefix":["title","x"]}}}"#
    );
    // Records: columns zipped with rows, values made plain, a blob a string, Null nil.
    assert_eq!(
        eval(
            r#"
            local records = zellij.data.records {
              columns = { "id", "title", "raw", "gone" },
              rows = { { { Int = 1 }, { Text = "a" }, { Blob = { 104, 105 } }, "Null" } },
            }
            return records[1].id .. records[1].title .. records[1].raw .. tostring(records[1].gone)
            "#
        ),
        "1ahinil"
    );
}

/// The `data_*` commands are reachable through the sugar with the shapes serde expects: a
/// request that gets as far as the host is one that deserialized, and the host's own refusal
/// (no store behind the test env) is what comes back - not a marshalling error.
#[test]
fn the_data_sugar_builds_requests_the_host_accepts() {
    let source = r#"
        local M = {}
        function M.render()
          local errors = {}
          for _, call in ipairs {
            function() zellij.data.define { name = "n", attrs = { { "t", "text", required = true, default = "" } } } end,
            function() zellij.data.apply(zellij.data.put("n", { t = "x" })) end,
            function() zellij.data.fetch { entity = "n", sort = { "id desc", { "t", "asc" } }, filter = zellij.data.eq("t", "x"), limit = 5 } end,
            function() zellij.data.count("n") end,
            function() zellij.data.watch("n", "ids") end,
            function() zellij.data.unwatch(3) end,
          } do
            local ok, err = pcall(call)
            errors[#errors + 1] = ok and "ok" or tostring(err)
          end
          return table.concat(errors, "|")
        end
        return M
    "#;
    // A store of its own: `test_env` points the store at `.`, which would leave a database
    // file in the crate folder.
    let store = tempfile::tempdir().unwrap();
    let mut env = test_env();
    env.permissions = Arc::new(Mutex::new(Some(PermissionType::iter().collect())));
    env.plugin_data_store = Arc::new(PluginDataStore::new(store.path()));
    let mut plugin = load_lua_plugin(source, "test.lua", env).unwrap();
    let out = String::from_utf8(plugin.render(1, 500).unwrap()).unwrap();
    for (i, part) in out.split('|').enumerate() {
        assert!(
            !part.contains("could not be built") && !part.contains("missing field"),
            "call {i} did not marshal: {part}"
        );
    }
}

#[test]
fn an_interval_re_arms_the_timer_from_the_timer_event() {
    let mut plugin = load_permitted(
        r#"
        local M = {}
        M.ticks = 0
        function M.load() zellij.set_interval(0.5) end
        zellij.on("timer", function() M.ticks = M.ticks + 1 end)
        function M.render() return tostring(M.ticks) .. "/" .. tostring(zellij.__interval) end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    plugin.update(&Event::Timer(0.5)).unwrap();
    plugin.update(&Event::Timer(0.5)).unwrap();
    assert_eq!(plugin.render(1, 20).unwrap(), b"2/0.5");
    let mut plugin = load(
        "local M = {}\nfunction M.render() zellij.clear_interval() return tostring(zellij.__interval) end\nreturn M",
    )
    .unwrap();
    assert_eq!(plugin.render(1, 20).unwrap(), b"nil");
}
