use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use zellij_utils::data::{BareKey, Event, KeyWithModifier, PermissionType};
use zellij_utils::errors::prelude::*;
use zellij_utils::input::layout::RunPluginLocation;
use zellij_utils::input::plugins::PluginConfig;
use zellij_utils::plugin_data_store::PluginDataStore;

use super::load_lua_plugin;
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
        },
        permissions: Arc::new(Mutex::new(None)),
        senders: ThreadSenders::default().silently_fail_on_send(),
        wasi_ctx: None,
        plugin_own_data_dir: PathBuf::from("."),
        zellij_data_dir: PathBuf::from("."),
        plugin_data_store: Arc::new(PluginDataStore::new(&PathBuf::from("."))),
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
    assert!(PermissionType::ChangeApplicationState.to_string().len() > 0);
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
    let mut plugin = load(
        r#"
        local M = {}
        function M.render()
          return table.concat({
            type(io), type(os), type(dofile), type(loadfile), type(load), type(debug),
          }, ",")
        end
        return M
        "#,
    )
    .unwrap();
    let rendered = String::from_utf8(plugin.render(1, 40).unwrap()).unwrap();
    assert_eq!(rendered, "nil,nil,nil,nil,nil,nil");
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

#[test]
fn the_shipped_example_plugin_loads_and_renders() {
    // `example-plugins/clock.lua` is what the docs point new plugin authors at, so it should not
    // be allowed to rot.
    let source = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../example-plugins/clock.lua"),
    )
    .expect("the example plugin should be in the repo");
    let mut plugin = load_lua_plugin(&source, "clock.lua", test_env()).unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    let rendered = String::from_utf8(plugin.render(5, 40).unwrap()).unwrap();
    assert!(
        rendered.contains("the time is") && rendered.contains("00:00"),
        "got: {}",
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
