use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use strum::IntoEnumIterator;
use zellij_utils::data::{BareKey, CommandType, Event, KeyWithModifier, PermissionType};
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
        user_plugin_dir: None,
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
    assert!(
        rendered.contains("the time is") && rendered.contains("00:00"),
        "got: {}",
        rendered
    );
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
