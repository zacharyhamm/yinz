use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use strum::IntoEnumIterator;
use zellij_utils::data::{BareKey, CommandType, Event, KeyWithModifier, PermissionType};
use zellij_utils::data_access::DataAccess;
use zellij_utils::errors::prelude::*;
use zellij_utils::input::layout::RunPluginLocation;
use zellij_utils::input::plugins::PluginConfig;

use super::{check_lua_plugin, load_lua_plugin};
use crate::plugins::plugin_limits::PluginResourceLimits;
use crate::plugins::plugin_map::{PluginEnv, ResponseSink};
use crate::thread_bus::ThreadSenders;

#[test]
fn agent_exit_queries_skip_operation_logs_and_preserve_failures() {
    use mlua::LuaSerdeExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("checkout ' $;");
    std::fs::create_dir(&root).unwrap();
    assert!(std::process::Command::new("git")
        .arg("init")
        .arg(&root)
        .output()
        .unwrap()
        .status
        .success());
    let logs = dir.path().join("logs");
    let lock = root.join(".git/agent-exit.lock");
    std::fs::create_dir(&logs).unwrap();
    std::fs::create_dir(&lock).unwrap();
    let source = zellij_utils::consts::builtin_lua_plugin("agent-exit").unwrap().replace(
        "\nfunction M.load(config)",
        "M.test_queries = { state=W, query=must_git, git=git, rebase=rebase_in_progress }\nfunction M.load(config)",
    );
    let plugin = load(&source).unwrap();
    plugin
        .lua
        .globals()
        .set(
            "T",
            plugin.plugin.get::<mlua::Table>("test_queries").unwrap(),
        )
        .unwrap();
    plugin
        .lua
        .globals()
        .set("root", root.to_str().unwrap())
        .unwrap();
    plugin
        .lua
        .globals()
        .set("logs", logs.to_str().unwrap())
        .unwrap();
    plugin.lua.globals().set("__run", plugin.lua.create_function(|lua, value: mlua::Value| {
        let argv: Vec<String> = lua.from_value(value)?;
        let out = std::process::Command::new(&argv[0]).args(&argv[1..]).output().map_err(mlua::Error::external)?;
        lua.to_value(&serde_json::json!({"exit_code":out.status.code(), "stdout":String::from_utf8_lossy(&out.stdout), "stderr":String::from_utf8_lossy(&out.stderr)}))
    }).unwrap()).unwrap();
    plugin.lua.load(r#"
local z = require('zellij')
local calls = {}
z.async.run_command = function(opts)
  table.insert(calls, opts.command)
  return {done=true, value=__run(opts.command), on_settle={}}
end
z.task(function()
T.state.root, T.state.tmp = root, logs
T.state.gitdir, T.state.lock = root .. '/.git', root .. '/.git/agent-exit.lock'
assert(T.git('not-a-git-command') ~= 0)
local diagnostic, log = T.state.diagnostic, T.state.log
calls = {}
assert(T.query('status', '--porcelain=v1') == '')
assert(#calls == 1 and calls[1][1] == 'git' and calls[1][2] == '--no-optional-locks')
local ok, err = pcall(T.query, 'rev-parse', '--verify', 'refs/heads/missing')
assert(not ok and err:find('fatal:', 1, true))
assert(T.state.diagnostic == diagnostic and T.state.log == log and T.state.sequence == 1)
for _, name in ipairs({'absent', 'rebase-merge', 'rebase-apply'}) do
  if name ~= 'absent' then assert(__run({'mkdir', '--', T.state.gitdir .. '/' .. name}).exit_code == 0) end
  calls = {}
  assert(T.rebase() == (name ~= 'absent'))
  assert(#calls == 1 and calls[1][1] == 'sh')
  if name ~= 'absent' then assert(__run({'rmdir', '--', T.state.gitdir .. '/' .. name}).exit_code == 0) end
end
assert(T.state.diagnostic == diagnostic and T.state.sequence == 1)
end)
"#).exec().unwrap();
    assert!(!lock.join("operation").exists());
    assert_eq!(std::fs::read_dir(logs).unwrap().count(), 1);
}

#[test]
fn agent_exit_documented_fork_waits_for_permissions() {
    use zellij_utils::data::PermissionStatus;
    let docs = include_str!("../../../../docs/LUA_PLUGINS.md");
    let wrapper = docs
        .split("```lua\nlocal load = M.load\n")
        .nth(1)
        .unwrap()
        .split("\n```")
        .next()
        .unwrap();
    let wrapper = format!("local load = M.load\n{wrapper}");
    let builtin = zellij_utils::consts::builtin_lua_plugin("agent-exit").unwrap();
    for line in wrapper.lines() {
        assert!(
            builtin.contains(&format!("-- {line}")),
            "hook instructions differ: {line}"
        );
    }
    for (cached, answer) in [
        (false, PermissionStatus::Granted),
        (true, PermissionStatus::Granted),
        (false, PermissionStatus::Denied),
    ] {
        let mut env = test_env();
        if cached {
            env.permissions = Arc::new(Mutex::new(Some(PermissionType::iter().collect())));
        }
        let source = format!("local zellij = require('zellij')\nlocal M = {{}}\nfunction M.load(config) M.initialized = config end\n{wrapper}\nreturn M");
        let mut plugin = load_lua_plugin(&source, "fork.lua", env).unwrap();
        plugin
            .load(&BTreeMap::from([("marker".into(), "loaded".into())]))
            .unwrap();
        assert!(plugin
            .plugin
            .get::<Option<mlua::Table>>("initialized")
            .unwrap()
            .is_none());
        let granted = answer == PermissionStatus::Granted;
        plugin
            .update(&Event::PermissionRequestResult(answer))
            .unwrap();
        let initialized = plugin
            .plugin
            .get::<Option<mlua::Table>>("initialized")
            .unwrap();
        assert_eq!(initialized.is_some(), granted);
        if let Some(config) = initialized {
            assert_eq!(config.get::<String>("marker").unwrap(), "loaded");
        }
    }
}

/// Real Git operations, with pane exits delivered while helper-ID writes await their results.
#[test]
fn agent_exit_workflow_merges_and_cleans_real_worktrees() {
    use mlua::LuaSerdeExt;
    use std::process::Command;
    for (scenario, cleanup) in [
        ("manual", "k"),
        ("manual", "w"),
        ("manual", "d"),
        ("manual", "b"),
        ("generated", "k"),
        ("spawn_failed", "k"),
        ("ci_repair", "k"),
        ("ci_closed", "k"),
        ("dirty_target", "k"),
        ("target_moved", "k"),
        ("head_changed", "k"),
        ("commit_failed", "k"),
        ("missing_ci", "k"),
        ("recovery", "k"),
        ("inferred_default", "k"),
        ("missing_local_default", "k"),
        ("cancel_default", "k"),
        ("duplicate", "k"),
        ("outstanding", "k"),
        ("config_changed", "k"),
        ("conflict_repair", "k"),
        ("conflict_abort", "k"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("primary");
        let linked = dir.path().join("topic ' $; checkout");
        let git = |cwd: &std::path::Path, args: &[&str]| {
            let out = Command::new("git")
                .arg("-C")
                .arg(cwd)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap()
        };
        std::fs::create_dir(&primary).unwrap();
        git(&primary, &["init", "-b", "trunk"]);
        git(&primary, &["config", "user.name", "Workflow Test"]);
        git(
            &primary,
            &["config", "user.email", "workflow@example.invalid"],
        );
        std::fs::write(primary.join(".zellij.kdl"), "agent_exit { ci \"sh\" \"-c\" \"test ! -f fail && test -f 'new $; file'\"; default_branch \"trunk\"; }\n").unwrap();
        if scenario == "missing_ci" {
            std::fs::write(
                primary.join(".zellij.kdl"),
                "agent_exit { default_branch \"trunk\"; }\n",
            )
            .unwrap();
        }
        if matches!(
            scenario,
            "inferred_default" | "missing_local_default" | "cancel_default"
        ) {
            std::fs::write(
                primary.join(".zellij.kdl"),
                "agent_exit { ci \"sh\" \"-c\" \"test -f 'new $; file'\"; }\n",
            )
            .unwrap();
        }
        if scenario == "config_changed" {
            std::fs::write(primary.join(".gitignore"), ".zellij.kdl\n").unwrap();
        }
        std::fs::write(primary.join("shared"), "base\n").unwrap();
        git(&primary, &["add", "."]);
        git(&primary, &["commit", "-m", "initial"]);
        if matches!(
            scenario,
            "inferred_default" | "missing_local_default" | "cancel_default"
        ) {
            let remote = if scenario == "inferred_default" {
                "refs/remotes/origin/trunk"
            } else {
                "refs/remotes/origin/main"
            };
            git(&primary, &["update-ref", remote, "HEAD"]);
            git(
                &primary,
                &["symbolic-ref", "refs/remotes/origin/HEAD", remote],
            );
        }
        git(
            &primary,
            &["worktree", "add", "-b", "topic", linked.to_str().unwrap()],
        );
        if scenario == "config_changed" {
            std::fs::copy(primary.join(".zellij.kdl"), linked.join(".zellij.kdl")).unwrap();
        }
        std::fs::write(linked.join("new $; file"), "new content\n").unwrap();
        if scenario == "ci_repair" {
            std::fs::write(linked.join("fail"), "fail").unwrap();
        }
        if scenario == "commit_failed" {
            git(&primary, &["config", "user.name", ""]);
        }
        if scenario.starts_with("conflict_") {
            std::fs::write(primary.join("shared"), "target\n").unwrap();
            git(&primary, &["commit", "-am", "target moves"]);
            std::fs::write(linked.join("shared"), "topic\n").unwrap();
        }

        let source = zellij_utils::consts::builtin_lua_plugin("agent-exit")
            .unwrap()
            .replace(
                "\nfunction M.load(config)",
                "M.test_workflow = { state = W, run = workflow }\nfunction M.load(config)",
            );
        let plugin = load(&source).unwrap();
        plugin
            .lua
            .globals()
            .set(
                "T",
                plugin.plugin.get::<mlua::Table>("test_workflow").unwrap(),
            )
            .unwrap();
        plugin
            .lua
            .globals()
            .set("M", plugin.plugin.clone())
            .unwrap();
        plugin
            .lua
            .globals()
            .set("checkout", linked.to_str().unwrap())
            .unwrap();
        plugin.lua.globals().set("cleanup", cleanup).unwrap();
        plugin.lua.globals().set("scenario", scenario).unwrap();
        plugin
            .lua
            .globals()
            .set("primary", primary.to_str().unwrap())
            .unwrap();
        plugin
            .lua
            .globals()
            .set("test_pid", std::process::id())
            .unwrap();
        plugin.lua.globals().set("__run", plugin.lua.create_function(|lua, value: mlua::Value| {
            let argv: Vec<String> = lua.from_value(value)?;
            let out = Command::new(&argv[0]).args(&argv[1..])
                .env("GIT_EDITOR", "true").env("GIT_TERMINAL_PROMPT", "0").output()
                .map_err(mlua::Error::external)?;
            lua.to_value(&serde_json::json!({"exit_code":out.status.code(), "stdout":String::from_utf8_lossy(&out.stdout), "stderr":String::from_utf8_lossy(&out.stderr)}))
        }).unwrap()).unwrap();
        plugin
            .lua
            .globals()
            .set(
                "__config",
                plugin
                    .lua
                    .create_function(|lua, root: String| {
                        let snapshot =
                            zellij_utils::project_config::access(std::path::Path::new(&root), None)
                                .map_err(mlua::Error::external)?;
                        super::to_lua_value(lua, &snapshot).map_err(mlua::Error::external)
                    })
                    .unwrap(),
            )
            .unwrap();
        plugin
            .lua
            .load(
                r#"
local z = require('zellij')
local function ready(value) return { done=true, value=value, on_settle={} } end
z.subscribe = function() end
local event = z.async.event
local pending_exit, pending_write
z.async.run_command = function(opts)
  local result = __run(opts.command)
  if pending_exit and opts.command[5] == T.state.lock .. '/helper' and opts.command[6] then
    assert(result.exit_code == 0)
    pending_write = true
    return event('run_command_result')
  end
  return ready(result)
end
z.project_config.read = __config
z.get_plugin_ids = function() return {plugin_id=41, zellij_pid=test_pid} end
z.get_pane_info = function() return nil end
local keys = {'r', 'Enter', 'c', 'f', 'i', 'x', 'Enter', 'y', 'y', 'y', cleanup}
local expected_merge = true
local expected_error
if scenario == 'generated' then keys = {'g', 'Enter', 'c', 'Enter', 'y', 'y', 'y', cleanup}
elseif scenario == 'missing_local_default' then keys = {'r', 'Enter', 'c', 'f', 'Enter', 'y', 't', 'r', 'u', 'n', 'k', 'Enter', 'y', 'y', cleanup}
elseif scenario == 'cancel_default' then keys = {'r', 'Enter', 'n', 'Esc'}; expected_merge = false
elseif scenario == 'spawn_failed' then keys = {'g', 'q'}; expected_merge = false
elseif scenario == 'ci_repair' then keys = {'c', 'f', 'Enter', 'y', 'y', 's', 'c', 'r', 'Enter', 'y', 'y', 'y', cleanup}
elseif scenario == 'ci_closed' then keys = {'c', 'f', 'Enter', 'y', 'y', 'q'}; expected_merge = false
elseif scenario == 'dirty_target' or scenario == 'target_moved' or scenario == 'head_changed' or scenario == 'config_changed' then
  keys = {'c', 'f', 'Enter', 'y', 'y', 'y', 'q'}; expected_merge = false
elseif scenario == 'commit_failed' then keys = {'c', 'f', 'Enter', 'y', 'q'}; expected_merge = false
elseif scenario == 'missing_ci' then keys = {'c', 'f', 'Enter', 'y', 'q'}; expected_merge = false
elseif scenario == 'conflict_repair' then keys = {'c', 'f', 'Enter', 'y', 's', 'n', 'y', 'y', cleanup}
elseif scenario == 'conflict_abort' then keys = {'c', 'f', 'Enter', 'y', 'a', 'q'}; expected_merge = false
elseif scenario == 'recovery' or scenario == 'duplicate' or scenario == 'outstanding' then
  local out = __run({'git', '-C', checkout, 'rev-parse', '--absolute-git-dir'})
  local lock = out.stdout:gsub('\n$', '') .. '/agent-exit.lock'
  assert(__run({'mkdir', lock}).exit_code == 0)
  assert(__run({'sh', '-c', 'printf "%s\\n" "$2" > "$1/owner"', 'test', lock, test_pid .. ' 99 abandoned'}).exit_code == 0)
  if scenario == 'duplicate' then
    z.get_pane_info = function() return {exited=false} end
    keys = {}; expected_merge = false; expected_error = 'Another workflow owns'
  elseif scenario == 'outstanding' then
    assert(__run({'sh', '-c', 'printf "%s\\n" "$2" > "$1/operation"', 'test', lock, tostring(test_pid)}).exit_code == 0)
    keys = {}; expected_merge = false; expected_error = 'outstanding Git/CI operation'
  else table.insert(keys, 1, 'y') end
end
z.async.event = function(name)
  assert(name == 'key')
  local key = table.remove(keys, 1)
  assert(key, scenario .. ': unexpected prompt: ' .. tostring(T.state.title) .. '\n' .. tostring(T.state.detail))
  if scenario == 'missing_local_default' or scenario == 'cancel_default' then
    if key == 'Enter' and T.state.title:find('Review diff', 1, true) then
      assert(T.state.detail:find('Default branch unresolved', 1, true))
    elseif key == 't' or key == 'Esc' then
      assert(T.state.title == 'Local default branch (no fetch)')
    end
  end
  if scenario == 'spawn_failed' and key == 'q' then
    assert(T.state.title == 'Operation failed' and T.state.detail == 'helper spawn failed')
    assert(__run({'test', '!', '-e', T.state.lock .. '/helper'}).exit_code == 0)
  end
  return ready({key=key})
end
z.open_command_pane = function(opts)
  local argv = {opts[1].path}
  for _, arg in ipairs(opts[1].args) do table.insert(argv, arg) end
  pane_result = __run(argv)
  if scenario == 'ci_closed' then pane_result.closed = true
  elseif scenario == 'dirty_target' then assert(__run({'sh', '-c', 'touch "$1/dirty"', 'test', primary}).exit_code == 0)
  elseif scenario == 'target_moved' then assert(__run({'git', '-C', primary, 'commit', '--allow-empty', '-m', 'moved'}).exit_code == 0)
  elseif scenario == 'config_changed' then assert(__run({'sh', '-c', 'printf "\\n" >> "$1/.zellij.kdl"', 'test', checkout}).exit_code == 0)
  elseif scenario == 'head_changed' then assert(__run({'git', '-C', checkout, 'commit', '--allow-empty', '-m', 'changed'}).exit_code == 0) end
  pending_exit = pane_result.closed and {PaneClosed={Terminal=55}} or
    {CommandPaneExited={55, pane_result.exit_code, {}}}
  return {Terminal=55}
end
z.agent.spawn = function(opts)
  assert(opts.cwd == checkout and opts.source_directory == checkout)
  assert(opts.kind == 'codex')
  assert(opts.context.agent_workflow_run == T.state.run)
  if scenario == 'spawn_failed' then error('helper spawn failed', 0) end
  if opts.context.agent_workflow_role == 'message' then
    local file = assert(opts.prompt:match('this file: ([^\n]+)'))
    assert(__run({'sh', '-c', 'printf "%s" "generated message" > "$1"', 'test', file}).exit_code == 0)
  else
    assert(opts.context.agent_workflow_role == 'repair')
    if scenario == 'ci_repair' then assert(__run({'rm', '--', checkout .. '/fail'}).exit_code == 0)
    elseif scenario == 'conflict_repair' then
      assert(__run({'sh', '-c', 'printf resolved > "$1/shared"', 'test', checkout}).exit_code == 0)
      assert(__run({'git', '-C', checkout, 'add', 'shared'}).exit_code == 0)
      assert(__run({'git', '-C', checkout, 'rebase', '--continue'}).exit_code == 0)
    end
  end
  pending_exit = {CommandPaneExited={56, 0, {}}}
  return {Terminal=56}
end
M.cwd = checkout
M.kind = 'codex'
local finished = false
z.task(function()
  if expected_error then
    local ok, err = pcall(T.run)
    assert(not ok and tostring(err):find(expected_error, 1, true), tostring(err))
  else T.run() end
  finished = true
end)
while pending_write do
  local exited = pending_exit
  pending_write, pending_exit = nil, nil
  -- The exit is not replayed: it must settle a waiter before the marker write completes.
  z.__update(M, exited)
  z.__update(M, {RunCommandResult={0, {}, {}, {}}})
end
assert(finished, scenario .. ': workflow missed a pane exit: ' .. tostring(T.state.title))
assert(#keys == 0, 'workflow stopped early: ' .. tostring(T.state.detail))
assert((T.state.merged ~= nil) == expected_merge, scenario .. ': incorrect merge outcome: ' .. tostring(T.state.detail))
assert(not T.state.owns, 'ownership was retained')
if not expected_error then
  assert(__run({'test', '!', '-e', T.state.gitdir .. '/agent-exit.lock'}).exit_code == 0,
    scenario .. ': ownership lock was retained')
end
"#,
            )
            .exec()
            .unwrap();
        if ![
            "manual",
            "generated",
            "ci_repair",
            "recovery",
            "inferred_default",
            "missing_local_default",
            "conflict_repair",
        ]
        .contains(&scenario)
        {
            assert!(
                !primary.join("new $; file").exists(),
                "{scenario}: merged without a valid gate"
            );
            continue;
        }
        assert_eq!(
            std::fs::read_to_string(primary.join("new $; file")).unwrap(),
            "new content\n"
        );
        assert_eq!(linked.exists(), matches!(cleanup, "k" | "d"));
        let branches = git(&primary, &["branch", "--list", "topic"]);
        assert_eq!(!branches.trim().is_empty(), matches!(cleanup, "k" | "w"));
        if cleanup == "d" {
            assert!(!Command::new("git")
                .arg("-C")
                .arg(&linked)
                .args(["symbolic-ref", "HEAD"])
                .output()
                .unwrap()
                .status
                .success());
        }
    }
}

/// A `PluginEnv` with no wasm behind it and no channels wired up. Enough to load a Lua plugin and
/// run its entrypoints; commands that talk to other threads silently fail, which is what
/// `ThreadSenders::default()` does.
fn test_env() -> PluginEnv {
    PluginEnv {
        response_sink: ResponseSink::WasiStdin,
        plugin_id: 1,
        cancel: Default::default(),
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
        permissions_requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        senders: ThreadSenders::default().silently_fail_on_send(),
        wasi_ctx: None,
        plugin_own_data_dir: PathBuf::from("."),
        data_access: Arc::new(DataAccess::sqlite(&PathBuf::from("."))),
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
        cancel_tripped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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

/// An agent's hook event reaches a plugin already read: the state the session derived, the
/// agent's own words, and the pane as a number - flat on the event, as every struct payload is.
#[test]
fn an_agent_event_arrives_flat_and_typed() {
    use zellij_utils::agent::AgentEvent;
    let mut plugin = load(
        r#"
        local zellij = require("zellij")
        local M = {}
        function M.load() zellij.subscribe { "agent_event" } end
        function M.update(event)
          if event.name == "agent_event" then
            M.seen = table.concat({
              tostring(event.pane_id), event.kind, event.hook, tostring(event.state),
              event.last_message or "-", event.value.hook,
            }, "|")
            return true
          end
        end
        function M.render() return M.seen or "nothing" end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    let event = Event::AgentEvent(AgentEvent {
        pane_id: 7,
        kind: "claude".to_owned(),
        hook: "Stop".to_owned(),
        state: Some(zellij_utils::data::AgentState::Idle),
        last_message: Some("done".to_owned()),
        raw: "{}".to_owned(),
        ..Default::default()
    });
    assert!(plugin.update(&event).unwrap());
    assert_eq!(
        String::from_utf8(plugin.render(1, 80).unwrap()).unwrap(),
        "7|claude|Stop|Idle|done|Stop"
    );
}

/// The agent records are kept off every manifest that flows, not only those after the first
/// `info()`: a plugin that subscribed to `pane_update` itself is answered on its first ask.
/// And a pane that closed is gone from the records, not left at the state it last showed.
#[test]
fn agent_records_are_current_on_the_first_ask_and_forget_a_closed_pane() {
    use zellij_utils::data::{AgentInfo, AgentSource, AgentState, PaneInfo, PaneManifest};
    let mut plugin = load(
        r#"
        local zellij = require("zellij")
        local M = {}
        function M.load() zellij.subscribe { "pane_update", "key" } end
        function M.update(event)
          if event.name == "key" then
            local info = zellij.agent.info(3)
            local all = zellij.agent.all()
            local count = 0
            for _ in pairs(all) do count = count + 1 end
            M.seen = (info and info.state or "nil") .. "|" .. count
            return true
          end
        end
        function M.render() return M.seen or "nothing" end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    let manifest_with = |panes: Vec<PaneInfo>| {
        let mut manifest = PaneManifest::default();
        manifest.panes.insert(0, panes);
        Event::PaneUpdate(manifest)
    };
    let agent_pane = PaneInfo {
        id: 3,
        agent: Some(AgentInfo {
            kind: "claude".to_owned(),
            state: AgentState::Busy,
            source: AgentSource::Spawned,
            detail: None,
            waiting_on: None,
            wait_needs_a_person: false,
            cwd: None,
            session_id: None,
            run_id: None,
        }),
        ..Default::default()
    };
    let shell_pane = PaneInfo {
        id: 4,
        ..Default::default()
    };
    let ask = Event::Key(KeyWithModifier::new(BareKey::Char('a')));

    plugin
        .update(&manifest_with(vec![agent_pane, shell_pane.clone()]))
        .unwrap();
    plugin.update(&ask).unwrap();
    // One record: the shell pane has no agent, and a nil record is no entry.
    assert_eq!(
        String::from_utf8(plugin.render(1, 80).unwrap()).unwrap(),
        "Busy|1",
        "the manifest that already flowed answers the first ask"
    );

    plugin.update(&manifest_with(vec![shell_pane])).unwrap();
    plugin.update(&ask).unwrap();
    assert_eq!(
        String::from_utf8(plugin.render(1, 80).unwrap()).unwrap(),
        "nil|0",
        "a closed pane is gone from the records"
    );
}

/// One host timer in flight for the nearest deadline, however many sleeps are outstanding: the
/// host cannot cancel a timer, so a later, nearer deadline arms a second one and the first fires
/// into an ordinary sweep. `os.time` is the plugin's own here, so the clock is the test's.
#[test]
fn several_sleeps_arm_one_host_timer_and_wake_in_deadline_order() {
    let mut plugin = load(
        r#"
        local zellij = require("zellij")
        local M = { log = {}, now = 100 }
        os.time = function() return M.now end
        zellij.set_timeout = function(seconds) table.insert(M.log, "arm:" .. seconds) end
        local function sleeper(seconds)
          zellij.task(function()
            zellij.await(zellij.async.sleep(seconds))
            table.insert(M.log, "woke:" .. seconds)
          end)
        end
        function M.load()
          zellij.subscribe { "custom_message" }
          sleeper(5)
          sleeper(1)
          sleeper(3)
        end
        function M.update(event)
          -- The test's clock: a custom message names the time now.
          if event.name == "custom_message" then M.now = tonumber(event.message) end
        end
        function M.render() return table.concat(M.log, " ") end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    // 5 armed first, for exactly 5 seconds - no extra second; 1 is nearer and arms again; 3 is
    // behind an armed timer and does not.
    assert_eq!(plugin.render(1, 80).unwrap(), b"arm:5 arm:1");

    plugin.update(&Event::Timer(1.0)).unwrap();
    assert_eq!(
        plugin.render(1, 80).unwrap(),
        b"arm:5 arm:1",
        "nothing is due yet"
    );

    // The clock moves and a timer fires: everything due wakes, in deadline order, and the
    // next deadline is armed afresh.
    plugin
        .update(&Event::CustomMessage("103".to_owned(), String::new()))
        .unwrap();
    plugin.update(&Event::Timer(1.0)).unwrap();
    // Armed before the tasks run: the sweep settles what is due and arms the next deadline,
    // and only then are the woken tasks resumed.
    assert_eq!(
        plugin.render(1, 80).unwrap(),
        b"arm:5 arm:1 arm:2 woke:1 woke:3"
    );
    plugin
        .update(&Event::CustomMessage("106".to_owned(), String::new()))
        .unwrap();
    plugin.update(&Event::Timer(1.0)).unwrap();
    assert_eq!(
        plugin.render(1, 80).unwrap(),
        b"arm:5 arm:1 arm:2 woke:1 woke:3 woke:5"
    );
}

/// `zellij.every` re-arms from each firing and stops on `cancel`; `zellij.after` fires once.
#[test]
fn every_ticks_until_cancelled_and_after_fires_once() {
    let mut plugin = load(
        r#"
        local zellij = require("zellij")
        local M = { ticks = 0, once = 0, now = 0, armed = {} }
        os.time = function() return M.now end
        zellij.set_timeout = function(seconds) table.insert(M.armed, seconds) end
        function M.load()
          zellij.subscribe { "custom_message" }
          M.clock = zellij.every(1, function() M.ticks = M.ticks + 1 return true end)
          zellij.after(1, function() M.once = M.once + 1 end)
        end
        function M.update(event)
          if event.name == "custom_message" then M.now = tonumber(event.message) end
        end
        zellij.on("key", function() M.clock:cancel() end)
        function M.render()
          return M.ticks .. "/" .. M.once .. " armed:" .. table.concat(M.armed, ",")
        end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    assert_eq!(
        plugin.render(1, 40).unwrap(),
        b"0/0 armed:1",
        "every(1) arms one second, not two; after(1) shares the timer"
    );
    // The test's clock: one second passes, then the host's timer fires.
    let tick = |plugin: &mut super::LuaPlugin, now: u32| -> bool {
        plugin
            .update(&Event::CustomMessage(now.to_string(), String::new()))
            .unwrap();
        plugin.update(&Event::Timer(1.0)).unwrap()
    };
    assert!(tick(&mut plugin, 1), "a truthy tick asks for a render");
    tick(&mut plugin, 2);
    assert_eq!(
        plugin.render(1, 40).unwrap(),
        b"2/1 armed:1,1,1",
        "re-armed for one second from each firing"
    );
    plugin
        .update(&Event::Key(KeyWithModifier::new(BareKey::Char('c'))))
        .unwrap();
    tick(&mut plugin, 3);
    tick(&mut plugin, 4);
    assert_eq!(
        plugin.render(1, 40).unwrap(),
        b"2/1 armed:1,1,1",
        "cancelled, and after fired once"
    );
}

/// The next deadline is armed before any callback runs, so an `every` whose callback raises
/// still reports the error, still re-arms, and still ticks again - and a sleep due on the same
/// timer still wakes on time.
#[test]
fn a_raising_every_callback_re_arms_and_keeps_ticking() {
    let mut plugin = load(
        r#"
        local zellij = require("zellij")
        local M = { log = {}, now = 0 }
        os.time = function() return M.now end
        zellij.set_timeout = function(seconds) table.insert(M.log, "arm:" .. seconds) end
        function M.load()
          zellij.subscribe { "custom_message" }
          zellij.every(1, function()
            table.insert(M.log, "tick")
            if #M.log < 4 then error("tick broke") end
          end)
          zellij.task(function()
            zellij.await(zellij.async.sleep(3))
            table.insert(M.log, "woke")
          end)
        end
        function M.update(event)
          if event.name == "custom_message" then M.now = tonumber(event.message) end
        end
        function M.render() return table.concat(M.log, " ") end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    assert_eq!(
        plugin.render(1, 80).unwrap(),
        b"arm:1",
        "the sleep is behind the tick"
    );

    let tick = |plugin: &mut super::LuaPlugin, now: u32| {
        plugin
            .update(&Event::CustomMessage(now.to_string(), String::new()))
            .unwrap();
        plugin.update(&Event::Timer(1.0))
    };
    let error = tick(&mut plugin, 1).expect_err("the callback's error is the entrypoint's");
    assert!(format!("{error:#}").contains("tick broke"), "{error:#}");
    // The sleep's deadline (3, two seconds out) was armed before the callback ran, and the
    // callback's own next tick was scheduled despite the error - nearer, so armed too.
    assert_eq!(plugin.render(1, 80).unwrap(), b"arm:1 arm:2 tick arm:1");

    tick(&mut plugin, 2).unwrap();
    tick(&mut plugin, 3).unwrap();
    assert_eq!(
        plugin.render(1, 80).unwrap(),
        b"arm:1 arm:2 tick arm:1 arm:1 tick tick arm:1 woke",
        "the clock kept going and the sleeper woke on time"
    );
}

/// Two deadlines due on the same timer: the first callback raising does not stop the second
/// from running, and the error is still reported.
#[test]
fn a_raising_after_callback_still_settles_the_other_due_deadlines() {
    let mut plugin = load(
        r#"
        local zellij = require("zellij")
        local M = { log = {}, now = 0 }
        os.time = function() return M.now end
        zellij.set_timeout = function() end
        function M.load()
          zellij.subscribe { "custom_message" }
          zellij.after(1, function() error("first broke") end)
          zellij.after(1, function() table.insert(M.log, "second") end)
        end
        function M.update(event)
          if event.name == "custom_message" then M.now = tonumber(event.message) end
        end
        function M.render() return table.concat(M.log, " ") end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    plugin
        .update(&Event::CustomMessage("1".to_owned(), String::new()))
        .unwrap();
    let error = plugin.update(&Event::Timer(1.0)).expect_err("reported");
    assert!(format!("{error:#}").contains("first broke"), "{error:#}");
    assert_eq!(plugin.render(1, 80).unwrap(), b"second");
}

/// A sleeper due on the tick whose `after` callback raises is resumed on that same tick. Its
/// task is queued when the sweep settles the sleep; had the callback's error left `deliver`
/// before the drain, the task would sit queued until some later event happened to arrive -
/// and with no other deadline or subscription, none would.
#[test]
fn a_raising_after_callback_does_not_strand_a_sleeper_due_on_the_same_tick() {
    let mut plugin = load(
        r#"
        local zellij = require("zellij")
        local M = { log = {}, now = 0 }
        os.time = function() return M.now end
        zellij.set_timeout = function() end
        function M.load()
          zellij.subscribe { "custom_message" }
          zellij.after(1, function() error("broke") end)
          zellij.task(function()
            zellij.await(zellij.async.sleep(1))
            table.insert(M.log, "woke")
          end)
        end
        function M.update(event)
          if event.name == "custom_message" then M.now = tonumber(event.message) end
        end
        function M.render() return table.concat(M.log, " ") end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    plugin
        .update(&Event::CustomMessage("1".to_owned(), String::new()))
        .unwrap();
    let error = plugin.update(&Event::Timer(1.0)).expect_err("reported");
    assert!(format!("{error:#}").contains("broke"), "{error:#}");
    // No further event: the one timer tick both reported the error and woke the sleeper.
    assert_eq!(plugin.render(1, 80).unwrap(), b"woke");
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

/// `example-plugins/agent-harness.lua` is what the model reads when asked to run agents: its
/// task must start, ask for its permissions and wait for them, and the pane must render.
#[test]
fn the_agent_harness_example_loads_and_waits_for_its_permissions() {
    let source = example_plugin("agent-harness.lua");
    let mut plugin = load_lua_plugin(&source, "agent-harness.lua", test_env()).unwrap();
    plugin
        .load(&BTreeMap::from([("check".to_owned(), "true".to_owned())]))
        .unwrap();
    let rendered = component_text(&String::from_utf8(plugin.render(5, 60).unwrap()).unwrap());
    assert!(rendered.contains("starting"), "got: {rendered}");
    let subscriptions = plugin.env().subscriptions.lock().unwrap().clone();
    assert!(
        subscriptions.contains(&zellij_utils::data::EventType::PermissionRequestResult),
        "the task awaits the grant: {subscriptions:?}"
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
    use crate::background_jobs::BackgroundJob;
    use zellij_utils::channels::SenderWithContext;
    use zellij_utils::errors::ErrorContext;

    let (to_background_jobs, background_jobs) =
        zellij_utils::channels::unbounded::<(BackgroundJob, ErrorContext)>();
    let mut env = test_env();
    env.senders.to_background_jobs = Some(SenderWithContext::new(to_background_jobs));
    env.permissions = Arc::new(Mutex::new(Some(PermissionType::iter().collect())));
    let mut plugin = load_lua_plugin(
        zellij_utils::consts::builtin_lua_plugin("agent-exit").expect("the hook is a builtin"),
        "agent-exit.lua",
        env,
    )
    .unwrap();
    plugin
        .load(&BTreeMap::from([
            ("agent_pane_id".to_owned(), "7".to_owned()),
            ("agent_cwd".to_owned(), "/work/.worktrees/topic".to_owned()),
            ("agent_firecracker_vm".to_owned(), "review".to_owned()),
            (
                "agent_firecracker_worktree".to_owned(),
                "/work/.worktrees/topic".to_owned(),
            ),
            ("agent_session".to_owned(), "work".to_owned()),
            (
                "agent_zellij_executable".to_owned(),
                "/bin/zellij".to_owned(),
            ),
            (
                "agent_zellij_config_file".to_owned(),
                "/config/config.kdl".to_owned(),
            ),
            ("agent_zellij_config_dir".to_owned(), "/config".to_owned()),
        ]))
        .unwrap();
    let config_env = BTreeMap::from([
        (
            "ZELLIJ_CONFIG_FILE".to_owned(),
            "/config/config.kdl".to_owned(),
        ),
        ("ZELLIJ_CONFIG_DIR".to_owned(), "/config".to_owned()),
    ]);
    let next_command = || match background_jobs.recv().unwrap().0 {
        BackgroundJob::RunCommand(_, _, command, args, env, cwd, context, Some(8192)) => {
            assert_eq!(env, config_env);
            assert_eq!(cwd, PathBuf::from("/"));
            (command, args, context)
        },
        other => panic!("expected a Firecracker helper command, got {other:?}"),
    };
    let rendered = String::from_utf8(plugin.render(20, 100).unwrap()).unwrap();
    assert!(
        rendered.contains("Agent exited. Create another agent in the same directory?"),
        "got: {rendered}"
    );
    assert!(
        rendered.contains("Directory: /work/.worktrees/topic"),
        "got: {rendered}"
    );
    assert!(rendered.contains("Checking VM worktree"), "got: {rendered}");

    let (command, args, check_context) = next_command();
    assert_eq!(command, "/bin/zellij");
    assert_eq!(
        args,
        ["firecracker", "changes", "review", "--session", "work"]
    );
    plugin
        .update(&Event::RunCommandResult(
            Some(0),
            b"2\n".to_vec(),
            vec![],
            check_context,
        ))
        .unwrap();
    let rendered = String::from_utf8(plugin.render(20, 100).unwrap()).unwrap();
    assert!(
        rendered.contains("2 changed paths in the VM"),
        "got: {rendered}"
    );
    assert!(rendered.contains("<S> sync 2"), "got: {rendered}");

    plugin
        .update(&Event::Key(KeyWithModifier::new(BareKey::Char('s'))))
        .unwrap();
    let (command, args, sync_context) = next_command();
    assert_eq!(command, "/bin/zellij");
    assert_eq!(args, ["firecracker", "sync", "review", "--session", "work"]);
    plugin
        .update(&Event::RunCommandResult(
            Some(1),
            vec![],
            b"provider unavailable\n".to_vec(),
            sync_context,
        ))
        .unwrap();
    let rendered = String::from_utf8(plugin.render(20, 100).unwrap()).unwrap();
    assert!(
        rendered.contains("VM worktree could not be synced. See plugin log for details."),
        "got: {rendered}"
    );
    assert!(rendered.contains("<S> sync 2"), "got: {rendered}");

    plugin
        .update(&Event::Key(KeyWithModifier::new(BareKey::Char('s'))))
        .unwrap();
    let (command, args, retry_context) = next_command();
    assert_eq!(command, "/bin/zellij");
    assert_eq!(args, ["firecracker", "sync", "review", "--session", "work"]);
    plugin
        .update(&Event::RunCommandResult(
            Some(0),
            vec![],
            b"warning: kept guest copy\n".to_vec(),
            retry_context,
        ))
        .unwrap();
    let rendered = String::from_utf8(plugin.render(20, 100).unwrap()).unwrap();
    assert!(
        rendered.contains("VM worktree synced. warning: kept guest copy"),
        "got: {rendered}"
    );
    assert!(rendered.contains("<S> sync 2"), "got: {rendered}");

    plugin
        .update(&Event::Key(KeyWithModifier::new(BareKey::Char('s'))))
        .unwrap();
    let (command, args, recheck_context) = next_command();
    assert_eq!(command, "/bin/zellij");
    assert_eq!(
        args,
        ["firecracker", "changes", "review", "--session", "work"]
    );
    assert!(
        background_jobs.try_recv().is_err(),
        "sync must remain locked until the recheck finishes"
    );
    plugin
        .update(&Event::RunCommandResult(
            Some(0),
            b"1\n".to_vec(),
            vec![],
            recheck_context,
        ))
        .unwrap();
    let rendered = String::from_utf8(plugin.render(20, 100).unwrap()).unwrap();
    assert!(
        rendered.contains("1 changed path remains"),
        "got: {rendered}"
    );
    assert!(rendered.contains("<S> sync 1"), "got: {rendered}");

    plugin
        .update(&Event::Key(KeyWithModifier::new(BareKey::Char('s'))))
        .unwrap();
    let (_, _, sync_context) = next_command();
    plugin
        .update(&Event::RunCommandResult(
            Some(0),
            vec![],
            vec![],
            sync_context,
        ))
        .unwrap();
    let (_, _, recheck_context) = next_command();
    plugin
        .update(&Event::RunCommandResult(
            Some(1),
            vec![],
            b"provider unavailable\n".to_vec(),
            recheck_context,
        ))
        .unwrap();
    let rendered = String::from_utf8(plugin.render(20, 100).unwrap()).unwrap();
    assert!(
        rendered.contains("VM changes could not be checked. See plugin log for details."),
        "got: {rendered}"
    );
    assert!(!rendered.contains("<S> sync"), "got: {rendered}");
    assert!(!plugin.plugin.get::<bool>("syncing").unwrap());
    plugin
        .update(&Event::Key(KeyWithModifier::new(BareKey::Char('s'))))
        .unwrap();
    assert!(background_jobs.try_recv().is_err());
    // A fresh count permits sync again; the failed recheck must have released the lock.
    plugin
        .update(&Event::RunCommandResult(
            Some(0),
            b"1\n".to_vec(),
            vec![],
            BTreeMap::from([("step".to_owned(), "check".to_owned())]),
        ))
        .unwrap();

    plugin
        .update(&Event::Key(KeyWithModifier::new(BareKey::Char('s'))))
        .unwrap();
    let (_, _, sync_context) = next_command();
    plugin
        .update(&Event::RunCommandResult(
            Some(0),
            vec![],
            vec![],
            sync_context,
        ))
        .unwrap();
    let (_, _, recheck_context) = next_command();
    plugin
        .update(&Event::RunCommandResult(
            Some(0),
            b"0\n".to_vec(),
            vec![],
            recheck_context,
        ))
        .unwrap();
    let rendered = String::from_utf8(plugin.render(20, 100).unwrap()).unwrap();
    assert!(
        rendered.contains("Sync conflicts require resolution"),
        "got: {rendered}"
    );
    plugin
        .update(&Event::Key(KeyWithModifier::new(BareKey::Char('r'))))
        .unwrap();
    let rendered = String::from_utf8(plugin.render(20, 100).unwrap()).unwrap();
    assert!(rendered.contains("kept guest copy"), "got: {rendered}");
    plugin
        .update(&Event::Key(KeyWithModifier::new(BareKey::Char('y'))))
        .unwrap();
    let (_, _, context) = next_command();
    plugin
        .update(&Event::RunCommandResult(
            Some(0),
            b"0\n".to_vec(),
            vec![],
            context,
        ))
        .unwrap();
    let rendered = String::from_utf8(plugin.render(20, 100).unwrap()).unwrap();
    assert!(
        rendered.contains("Inspecting repository..."),
        "got: {rendered}"
    );
}

#[test]
fn agent_exit_zero_guest_changes_start_the_workflow() {
    let source = zellij_utils::consts::builtin_lua_plugin("agent-exit")
        .unwrap()
        .replace(
            "\nfunction M.load(config)",
            "M.test_state = W\nfunction M.load(config)",
        );
    let plugin = load(&source).unwrap();
    plugin
        .lua
        .globals()
        .set("M", plugin.plugin.clone())
        .unwrap();
    plugin.lua.load(r#"
local z = require('zellij')
local starts, commands = 0, {}
M.test_state.start = function() starts = starts + 1 end
z.subscribe = function() end
z.run_command = function(opts) table.insert(commands, opts) end
M.load { agent_firecracker_vm='vm', agent_firecracker_worktree='/host/topic', agent_session='work' }
assert(#commands == 1 and commands[1].context.step == 'check')
for _, step in ipairs({'check', 'recheck'}) do
  for _, case in ipairs({{0, '0\n', true}, {0, '2\n', false}, {0, 'bad', false}, {1, '0\n', false}}) do
    for _, warnings in ipairs({false, true}) do
      starts, commands = 0, {}
      M.sync_warnings = warnings and 'kept guest copy' or nil
      M.syncing = step == 'recheck'
      z.__update(M, {RunCommandResult={case[1], {case[2]:byte(1, -1)}, {}, {step=step}}})
      assert(starts == (case[3] and not warnings and 1 or 0))
      assert(not M.syncing)
      if case[3] then
        assert(not M.render(20, 120):find('<S> sync', 1, true))
        z.__update(M, {Key={bare_key='s', key_modifiers={}}})
        assert(#commands == 0)
        if warnings then assert(M.status:find('Sync conflicts require resolution', 1, true)) end
      end
    end
  end
end
"#).exec().unwrap();
}

#[test]
fn agent_exit_dialog_keys_do_not_reach_restart_or_close() {
    let source = zellij_utils::consts::builtin_lua_plugin("agent-exit")
        .unwrap()
        .replace(
            "\nfunction M.load(config)",
            "M.test_dialogs = { state = W, choose = choose, edit = edit }\nfunction M.load(config)",
        );
    let plugin = load(&source).unwrap();
    plugin
        .lua
        .globals()
        .set("M", plugin.plugin.clone())
        .unwrap();
    plugin
        .lua
        .load(
            r#"
local z = require('zellij')
local T = M.test_dialogs
local actions, commands = {}, {}
z.subscribe = function() end
z.rerun_command_pane = function(id) table.insert(actions, 'restart ' .. id) end
z.close_terminal_pane = function(id) table.insert(actions, 'close ' .. id) end
z.close_self = function() table.insert(actions, 'self') end
z.run_command = function(opts) table.insert(commands, opts) end
M.load { agent_pane_id = '7' }
local function key(k) z.__update(M, {Key={bare_key=k, key_modifiers={}}}) end
for _, dismiss in ipairs({'Enter', 'Esc', 'q'}) do
  for _, next_key in ipairs({'Enter', 'Esc'}) do
    actions, commands = {}, {}
    M.sync_warnings = 'kept guest copy'
    key('r')
    assert(T.state.active and T.state.title == 'Resolve sync conflicts before continuing')
    key(dismiss)
    assert(not T.state.active and #actions == 0 and #commands == 0)
    assert(M.sync_warnings == 'kept guest copy')
    key(next_key)
    assert(#actions == 2 and actions[2] == 'self')
    assert(actions[1] == (next_key == 'Enter' and 'restart 7' or 'close 7'))
  end
end
actions, commands = {}, {}
M.vm, M.session, M.zellij = 'review', 'work', 'zellij'
key('r'); key('y')
assert(not T.state.active and M.syncing and not M.sync_warnings and #actions == 0)
assert(#commands == 1 and commands[1].context.step == 'recheck')
M.syncing = false
-- Both dialog primitives can be the last await before a workflow returns.
for _, dialog in ipairs({'choose', 'edit'}) do
  for _, dismiss in ipairs({'Enter', 'Esc', 'q'}) do
    if dialog == 'choose' or dismiss ~= 'q' then
      T.state.active = true
      z.task(function()
        if dialog == 'choose' then T.choose('Workflow', {Enter='back'})
        else T.edit('Draft', 'message') end
        T.state.active = false
      end)
      key(dismiss)
      assert(not T.state.active and #actions == 0, dialog .. ': ' .. dismiss)
    end
  end
end
"#,
        )
        .exec()
        .unwrap();
}

#[test]
fn agent_exit_handles_missing_metadata_and_unavailable_changes() {
    let source = zellij_utils::consts::builtin_lua_plugin("agent-exit").unwrap();
    for cwd in [None, Some(""), Some("/guest")] {
        for missing in [
            None,
            Some("agent_firecracker_vm"),
            Some("agent_firecracker_worktree"),
            Some("agent_session"),
        ] {
            for empty in [false, true] {
                let mut plugin = load(source).unwrap();
                plugin
                    .lua
                    .load(
                        r#"
                    commands, logs, actions = {}, {}, {}
                    local z = require("zellij")
                    z.run_command = function(command) table.insert(commands, command) end
                    z.log = function(message) table.insert(logs, message) end
                    z.rerun_command_pane = function(id) table.insert(actions, "restart " .. id) end
                    z.close_terminal_pane = function(id) table.insert(actions, "close " .. id) end
                    z.close_self = function() table.insert(actions, "self") end
                "#,
                    )
                    .exec()
                    .unwrap();
                let mut config = BTreeMap::from([
                    ("agent_pane_id".to_owned(), "7".to_owned()),
                    ("agent_firecracker_vm".to_owned(), "review".to_owned()),
                    (
                        "agent_firecracker_worktree".to_owned(),
                        "/host/topic".to_owned(),
                    ),
                    ("agent_session".to_owned(), "work".to_owned()),
                    ("agent_zellij_executable".to_owned(), "".to_owned()),
                    ("agent_zellij_config_file".to_owned(), "".to_owned()),
                    ("agent_zellij_config_dir".to_owned(), "".to_owned()),
                ]);
                if let Some(cwd) = cwd {
                    config.insert("agent_cwd".to_owned(), cwd.to_owned());
                }
                if let Some(key) = missing {
                    if empty {
                        config.insert(key.to_owned(), "".to_owned());
                    } else {
                        config.remove(key);
                    }
                }
                plugin.load(&config).unwrap();
                let rendered = String::from_utf8(plugin.render(20, 120).unwrap()).unwrap();
                let directory = if cwd == Some("/guest") {
                    "/guest"
                } else if missing == Some("agent_firecracker_worktree") {
                    "unavailable"
                } else {
                    "/host/topic (host worktree)"
                };
                assert!(
                    rendered.contains(&format!("Directory: {directory}")),
                    "{rendered}"
                );
                if missing.is_some() {
                    assert!(
                        rendered.contains(
                            "VM changes not checked: VM or worktree details are unavailable."
                        ),
                        "{rendered}"
                    );
                    plugin.lua.load("assert(#commands == 0)").exec().unwrap();
                } else {
                    plugin.lua.load(r#"
                        assert(#commands == 1)
                        assert(commands[1].cwd == "/")
                        assert(table.concat(commands[1].command, " ") == "zellij firecracker changes review --session work")
                        assert(next(commands[1].env_variables) == nil)
                    "#).exec().unwrap();
                    for (status, stdout, stderr) in [
                        (None, "", "No such file or directory (os error 2)"),
                        (Some(1), "", "failed to count changes: provider unavailable"),
                        (Some(0), "garbage", ""),
                        (Some(0), "-1", ""),
                        (Some(0), "1.5", ""),
                        (Some(0), "1e999", ""),
                    ] {
                        plugin
                            .update(&Event::RunCommandResult(
                                status,
                                stdout.as_bytes().to_vec(),
                                stderr.as_bytes().to_vec(),
                                BTreeMap::from([("step".to_owned(), "check".to_owned())]),
                            ))
                            .unwrap();
                        let rendered = String::from_utf8(plugin.render(20, 120).unwrap()).unwrap();
                        assert!(
                            rendered.contains(
                                "VM changes could not be checked. See plugin log for details."
                            ),
                            "{rendered}"
                        );
                        assert!(
                            !rendered.contains("os error") && !rendered.contains("<S> sync"),
                            "{rendered}"
                        );
                    }
                    plugin.lua.load(r#"
                        assert(#logs == 6)
                        assert(logs[1]:find("VM check failed: vm=review session=work helper=zellij exit_status=nil", 1, true))
                        assert(logs[1]:find("os error 2", 1, true))
                        assert(logs[2]:find("provider unavailable", 1, true))
                        assert(logs[3]:find('stdout="garbage"', 1, true))
                    "#).exec().unwrap();
                }
                for key in [BareKey::Enter, BareKey::Esc] {
                    plugin
                        .update(&Event::Key(KeyWithModifier::new(key)))
                        .unwrap();
                }
                plugin
                    .lua
                    .load(r#"assert(table.concat(actions, ",") == "restart 7,self,close 7,self")"#)
                    .exec()
                    .unwrap();
            }
        }
    }
    // An ordinary exit has no VM details and does not advertise a VM failure.
    let mut plugin = load(source).unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    let rendered = String::from_utf8(plugin.render(20, 120).unwrap()).unwrap();
    assert!(rendered.contains("Directory: unavailable"));
    assert!(!rendered.contains("VM changes"));
}

/// The agent-policy hook is headless: a syntax error in it would surface the first time an agent
/// stopped for a question, which is the moment its notification is for. So it is loaded and
/// fired at here, and what is checked is its one visible act - the notification, addressed to
/// the pane that is waiting - and that the other points it may be bound to stay quiet.
#[test]
fn the_shipped_agent_policy_hook_notifies_when_an_agent_waits() {
    use crate::panes::PaneId;
    use crate::screen::ScreenInstruction;
    use zellij_utils::channels::SenderWithContext;
    use zellij_utils::errors::ErrorContext;
    use zellij_utils::input::hooks::HookPoint;

    let source =
        zellij_utils::consts::builtin_lua_plugin("agent-policy").expect("the hook is a builtin");
    let (to_screen, screen_receiver) =
        zellij_utils::channels::unbounded::<(ScreenInstruction, ErrorContext)>();
    let mut env = test_env();
    env.senders.to_screen = Some(SenderWithContext::new(to_screen));
    env.permissions = Arc::new(Mutex::new(Some(PermissionType::iter().collect())));

    let mut plugin = load_lua_plugin(source, "agent-policy.lua", env).unwrap();
    plugin.load(&BTreeMap::new()).unwrap();

    let notifications =
        |receiver: &zellij_utils::channels::Receiver<(ScreenInstruction, ErrorContext)>| {
            receiver
                .try_iter()
                .filter_map(|(instruction, _)| match instruction {
                    ScreenInstruction::Notify(text, pane_id, _) => Some((text, pane_id)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

    use zellij_utils::data::AgentState;
    let payload =
        |state: AgentState, detail: Option<&str>| zellij_utils::data::HookPayload::Agent {
            pane_id: 7,
            agent: zellij_utils::data::AgentInfo {
                kind: "claude".to_owned(),
                state,
                source: zellij_utils::data::AgentSource::Reported,
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
            Some(PaneId::Terminal(7)),
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
            Some(PaneId::Terminal(7)),
        )],
        "a wait with no detail still says who is waiting"
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

/// The denial every plugin meets first, and the one whose message used to point away from the
/// fix: during `load` nothing has answered the permissions yet - not even for a plugin whose
/// grant is already on disk - so "ask for it in `load`" is advice the author has already taken.
/// What zellij's own text components actually say in a frame.
///
/// `ui.text` is emitted as `\u{1b}Pztext;<styles>$<char codes>\u{1b}\\`, so a test that cares
/// about the words rather than the escapes has to read the codes back.
fn component_text(rendered: &str) -> String {
    let mut out = String::new();
    for part in rendered.split("Pztext;").skip(1) {
        let body = part.split('\u{1b}').next().unwrap_or("");
        let codes = body.trim_start_matches(['x', 'o', 'd', 'e', 'u', 'i', 's']);
        let codes = codes.rsplit('$').next().unwrap_or("");
        for code in codes.split(',') {
            if let Some(c) = code.trim().parse::<u32>().ok().and_then(char::from_u32) {
                out.push(c);
            }
        }
        out.push('\n');
    }
    out
}

/// The shared-data pair the guide points at for a plugin that keeps rows: an extension that owns
/// the namespace and a pane that reads it. Both are `plugin_docs` sections, so both rot the
/// moment the API moves under them.
#[test]
fn the_todo_examples_load_and_the_pane_renders_its_keys() {
    let mut env = test_env();
    env.permissions = Arc::new(Mutex::new(Some(PermissionType::iter().collect())));
    let extension = example_plugin("todo-extension.lua");
    let mut plugin = load_lua_plugin(&extension, "todo.lua", env).unwrap();
    plugin
        .load(&BTreeMap::from([
            ("hocus_extension_name".to_owned(), "todo".to_owned()),
            (
                "hocus_extension_path".to_owned(),
                "/plugins/hocus/todo.lua".to_owned(),
            ),
        ]))
        .unwrap();
    // The discover broadcast is how a hocus reload finds an extension that is already running,
    // and re-registering marshals the whole tool list.
    let discover = zellij_utils::data::PipeMessage::new(
        zellij_utils::data::PipeSource::Plugin(1),
        "hocus_extension_discover",
        &None,
        &None,
        true,
    );
    plugin.pipe(&discover).unwrap();

    // The same file reloaded by hand has none of that configuration, and the registration it
    // would send is addressed to nothing. It has to say so rather than raise or go quiet.
    let mut env = test_env();
    env.permissions = Arc::new(Mutex::new(Some(PermissionType::iter().collect())));
    let mut unlaunched = load_lua_plugin(&extension, "todo.lua", env).unwrap();
    unlaunched.load(&BTreeMap::new()).unwrap();
    unlaunched
        .update(&Event::PermissionRequestResult(
            zellij_utils::data::PermissionStatus::Granted,
        ))
        .unwrap();
    unlaunched.pipe(&discover).unwrap();

    let mut env = test_env();
    env.permissions = Arc::new(Mutex::new(Some(PermissionType::iter().collect())));
    let pane = example_plugin("todo-pane.lua");
    let mut plugin = load_lua_plugin(&pane, "todo-pane.lua", env).unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    let rendered = component_text(&String::from_utf8(plugin.render(10, 80).unwrap()).unwrap());
    assert!(rendered.contains("nothing yet"), "got: {rendered}");

    // The input mode is the part a rewrite is most likely to drop, so it is the part pressed
    // here: `a` opens it, and every key after that is text until Esc.
    // A space among them on purpose: there is no `Space` key, so a plugin that compares against
    // one has a branch that never runs. It arrives as the character it prints.
    for key in ["a", "h", "i", " ", "t"] {
        let key = BareKey::Char(key.chars().next().unwrap());
        plugin
            .update(&Event::Key(KeyWithModifier::new(key)))
            .unwrap();
    }
    let rendered = component_text(&String::from_utf8(plugin.render(10, 80).unwrap()).unwrap());
    assert!(rendered.contains("new: hi t_"), "got: {rendered}");
    plugin
        .update(&Event::Key(KeyWithModifier::new(BareKey::Esc)))
        .unwrap();
    let rendered = component_text(&String::from_utf8(plugin.render(10, 80).unwrap()).unwrap());
    assert!(rendered.contains("a add"), "got: {rendered}");
}

#[test]
fn a_denial_before_any_answer_says_to_wait_for_the_answer() {
    let source = r#"
        local zellij = require("zellij")
        local M = {}
        function M.load()
          zellij.request_permission { "plugin_data" }
          M.err = tostring(select(2, pcall(function()
            zellij.data.fetch { entity = "note" }
          end)))
        end
        function M.render() return M.err end
        return M
    "#;
    let load_with = |permissions| {
        let mut env = test_env();
        env.permissions = Arc::new(Mutex::new(permissions));
        load_lua_plugin(source, "test.lua", env).unwrap()
    };
    // `None`: the state every plugin loads in, whatever is already granted on disk.
    let mut plugin = load_with(None);
    plugin.load(&BTreeMap::new()).unwrap();
    let rendered = String::from_utf8(plugin.render(10, 200).unwrap()).unwrap();
    assert!(rendered.contains("PluginData"), "got: {rendered}");
    assert!(
        rendered.contains("permission_request_result"),
        "the denial has to name the handler the work belongs in, got: {rendered}"
    );

    // Answered, and still missing this one: the ordinary denial, which says to ask for it.
    let mut plugin = load_with(Some(HashSet::new()));
    plugin.load(&BTreeMap::new()).unwrap();
    let rendered = String::from_utf8(plugin.render(10, 200).unwrap()).unwrap();
    assert!(rendered.contains("request_permission"), "got: {rendered}");
    assert!(
        !rendered.contains("permission_request_result"),
        "an answered plugin is not waiting for anything, got: {rendered}"
    );
}

/// A plugin that never asked is in the same `permissions: None` state as one whose answer has
/// not arrived, and telling it to wait for an answer names a moment that will never come. Its
/// fix is the ordinary one: ask.
#[test]
fn a_denial_with_no_request_behind_it_says_to_ask() {
    let source = r#"
        local zellij = require("zellij")
        local M = {}
        function M.load()
          M.err = tostring(select(2, pcall(function()
            zellij.data.fetch { entity = "note" }
          end)))
        end
        function M.render() return M.err end
        return M
    "#;
    let mut plugin = load_lua_plugin(source, "test.lua", test_env()).unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    let rendered = String::from_utf8(plugin.render(10, 200).unwrap()).unwrap();
    assert!(rendered.contains("PluginData"), "got: {rendered}");
    assert!(rendered.contains("request_permission"), "got: {rendered}");
    assert!(
        !rendered.contains("permission_request_result"),
        "nothing is pending for a plugin that never asked, got: {rendered}"
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
        Ok(PluginCommand::OpenCommandPaneInGitCheckoutNearPlugin(_, source, checkout, false)) => {
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

/// `SpawnAgent` is a struct variant with a nested struct, so a table with named fields reads
/// straight through serde - and every spec field is optional, so an empty spec is a spec.
#[test]
fn spawn_agent_is_built_from_a_nested_spec_table() {
    use zellij_utils::data::{AgentSpawnSpec, PluginCommand};

    let build = |expression: &str| {
        let lua = mlua::Lua::new();
        let args: mlua::Value = lua.load(expression).eval().expect("the test's own Lua");
        super::command::plugin_command_from_lua(&lua, "SpawnAgent", "spawn_agent", args)
    };
    match build(
        r#"{ spec = { kind = "claude", prompt = "go", hooks = false, env = { A = "1" } },
             source_directory = "/host", name = "fixer", context = { step = "1" } }"#,
    ) {
        Ok(PluginCommand::SpawnAgent {
            spec,
            source_directory,
            cwd,
            name,
            context,
            automatic_firecracker_vm,
        }) => {
            assert_eq!(
                spec,
                AgentSpawnSpec {
                    kind: Some("claude".to_owned()),
                    prompt: Some("go".to_owned()),
                    hooks: Some(false),
                    env: BTreeMap::from([("A".to_owned(), "1".to_owned())]),
                    ..Default::default()
                }
            );
            assert_eq!(source_directory, Some(PathBuf::from("/host")));
            assert_eq!(cwd, None);
            assert_eq!(name, Some("fixer".to_owned()));
            assert_eq!(context.get("step").map(String::as_str), Some("1"));
            assert!(!automatic_firecracker_vm, "an omitted flag reads as false");
        },
        Ok(other) => panic!("built the wrong command: {other:?}"),
        Err(e) => panic!("{e:#}"),
    }
    match build("{ spec = {} }") {
        Ok(PluginCommand::SpawnAgent { spec, .. }) => assert_eq!(spec, AgentSpawnSpec::default()),
        other => panic!("an empty spec should be all defaults, got: {other:?}"),
    }
}

/// The prelude's half: `zellij.agent.spawn` takes one flat table and sorts the spec fields from
/// the command's own. The host then tries to open a pane, which these senders cannot do - so
/// what is checked is that serde accepted what the prelude built, not that a pane opened.
#[test]
fn zellij_agent_spawn_flattens_the_authors_table() {
    for source in [
        r#"zellij.agent.spawn { kind = "claude", prompt = "fix it", cwd = "/host" }"#,
        r#"zellij.agent.spawn { command = "sh", args = { "-c", "true" }, name = "probe" }"#,
        r#"zellij.agent.spawn { kind = "codex", branch = "topic", worktree = false, context = { a = "b" } }"#,
    ] {
        if let Some(err) = error_from_load(source) {
            assert!(
                !err.contains("could not read the arguments"),
                "the arguments were refused: {err}
source: {source}"
            );
        }
    }
    let err = error_from_load(r#"zellij.agent.spawn("claude")"#).expect("a string is refused");
    assert!(err.contains("takes a table"), "got: {err}");
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
                agent = {
                    kind = "codex",
                    state = "Waiting",
                    source = "Reported",
                    detail = "y/n",
                    wait_needs_a_person = true,
                },
            }
            zellij.set_pane_agent { pane_id = zellij.terminal_pane(3) }
            zellij.set_pane_agent_run {
                pane_id = zellij.terminal_pane(3),
                run_id = "codex:s1",
            }
            zellij.set_pane_agent_run { pane_id = zellij.terminal_pane(3) }
        end
        return M
        "#,
        "agents.lua",
        env,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();

    // Drained once: `try_iter` consumes, so the two assertions below read one snapshot of the
    // channel rather than racing each other for it.
    let instructions: Vec<_> = screen_receiver
        .try_iter()
        .map(|(instruction, _)| instruction)
        .collect();
    let records: Vec<_> = instructions
        .iter()
        .filter_map(|instruction| match instruction {
            ScreenInstruction::SetPaneAgent(pane_id, agent, _) => Some((*pane_id, agent.clone())),
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
            waiting_on: None,
            // A plugin speaking for an agent says this for itself. It defaults to `false`
            // rather than to the state, because a plugin *registering* a pane as an agent -
            // hocus's `agent_register` - is not reporting a prompt anybody is looking at.
            wait_needs_a_person: true,
            cwd: None,
            session_id: None,
            run_id: None,
        })
    );
    assert_eq!(records[1].1, None, "leaving `agent` out clears the record");

    // The additive twin: it names one field and says nothing about the rest of the record.
    let runs: Vec<_> = instructions
        .iter()
        .filter_map(|instruction| match instruction {
            ScreenInstruction::SetPaneAgentRun(pane_id, run_id, _) => {
                Some((*pane_id, run_id.clone()))
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        runs,
        vec![
            (
                crate::panes::PaneId::Terminal(3),
                Some("codex:s1".to_owned())
            ),
            (crate::panes::PaneId::Terminal(3), None),
        ],
        "leaving `run_id` out clears it"
    );
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

/// A plugin's log lines come from the server's own log, a file no root reaches, so the read is
/// a command gated on `author_plugins` - `read_files` is not enough - and it answers whether or
/// not the log is there yet: a missing log is the error, not an empty answer.
#[test]
fn reading_a_plugins_log_is_an_author_plugins_command() {
    let dirs = FsDirs::new();
    let body = r#"return zellij.read_plugin_log("keys", 5)"#;
    let refused = format!(
        "{:#}",
        render_with_dirs(body, &dirs, file_permissions()).unwrap_err()
    );
    assert!(
        refused.to_lowercase().contains("permission"),
        "got: {refused}"
    );

    let authoring = Some([PermissionType::AuthorPlugins].into_iter().collect());
    match render_with_dirs(body, &dirs, authoring) {
        Ok(lines) => assert!(
            lines.lines().all(|line| line.contains("keys")),
            "only the plugin's own lines: {lines}"
        ),
        Err(e) => {
            let e = format!("{e:#}");
            assert!(e.contains("no zellij log at"), "got: {e}");
        },
    }
    let bad_name = format!(
        "{:#}",
        render_with_dirs(
            r#"return zellij.read_plugin_log("../keys", 5)"#,
            &dirs,
            Some([PermissionType::AuthorPlugins].into_iter().collect()),
        )
        .unwrap_err()
    );
    assert!(bad_name.contains("not a plugin name"), "got: {bad_name}");
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
/// name the config spells, and `payload` the typed payload the fire site built - untagged, so
/// a hook reads `payload.pane_id` and `payload.agent.state` rather than a variant name.
#[test]
fn a_hook_event_has_its_point_and_payload() {
    use zellij_utils::data::{AgentInfo, AgentSource, AgentState, HookPayload};
    use zellij_utils::input::hooks::HookPoint;
    let event = Event::Hook {
        point: HookPoint::AgentWaiting,
        payload: HookPayload::Agent {
            pane_id: 7,
            agent: AgentInfo {
                kind: "claude".to_owned(),
                state: AgentState::Waiting,
                source: AgentSource::Reported,
                detail: Some("y/n".to_owned()),
                waiting_on: None,
                wait_needs_a_person: true,
                cwd: None,
                session_id: None,
                run_id: None,
            },
            previous_state: Some(AgentState::Busy),
            command: None,
        },
    };
    let out = describe_event(
        event,
        r#"return event.name .. "|" .. event.point .. "|" .. event.payload.pane_id .. "|"
              .. event.payload.agent.state .. "|" .. event.payload.previous_state
              .. "|" .. tostring(event.payload.command)"#,
    );
    assert_eq!(out, "hook|agent_waiting|7|Waiting|Busy|nil");
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
            payload: zellij_utils::data::HookPayload::SessionStart {
                session_name: "main".to_owned(),
                client_id: 1,
            },
        })
        .unwrap();
    plugin
        .update(&Event::Hook {
            point: HookPoint::PaneExit,
            payload: zellij_utils::data::HookPayload::PaneExit {
                pane_id: 7,
                exit_code: None,
                cwd: None,
                command: None,
                held: false,
            },
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
    CommandType::ReadPluginLog,
    CommandType::GetPluginIds,
    CommandType::GetZellijVersion,
    CommandType::GenerateRandomName,
    CommandType::GetUserPluginDir,
    CommandType::GetLocalUtcOffset,
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
    CommandType::DataCatalog,
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
    CommandType::SpawnAgent,
    CommandType::ProjectConfigRead,
    CommandType::ProjectConfigWrite,
    CommandType::OpenCommandPaneInNewTab,
    CommandType::OpenPluginPaneInNewTab,
    CommandType::OpenEditorPaneInNewTab,
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
    env.data_access = Arc::new(DataAccess::sqlite(store.path()));
    let mut plugin = load_lua_plugin(source, "test.lua", env).unwrap();
    let out = String::from_utf8(plugin.render(1, 500).unwrap()).unwrap();
    for (i, part) in out.split('|').enumerate() {
        assert!(
            !part.contains("could not be built") && !part.contains("missing field"),
            "call {i} did not marshal: {part}"
        );
    }
}

/// A Lua plugin gets the same compare-and-set the wasm side does. Both halves are load-bearing:
/// `expect_rev` has to survive the trip out (it is deserialized into `Mutation` by serde, and a
/// field the table does not name has to read as "unguarded"), and `per_mutation` has to survive
/// the trip back.
#[test]
fn a_lua_plugin_can_guard_a_write_on_a_revision() {
    let source = r#"
        local M = {}
        function M.render()
          zellij.data.define {
            name = "note", version = 1,
            attrs = { { "title", "text", required = true } },
          }
          local out = {}

          -- Claiming a uid nobody holds works; claiming it twice does not.
          zellij.data.apply(zellij.data.put("note", { title = "first" }, "n1",
                                            zellij.data.REV_ABSENT))
          local ok = pcall(function()
            zellij.data.apply(zellij.data.put("note", { title = "again" }, "n1",
                                              zellij.data.REV_ABSENT))
          end)
          out[#out + 1] = "claimed-twice:" .. tostring(ok)

          -- The revision comes back on the row, and a write naming it lands.
          local rows = zellij.data.fetch { entity = "note" }
          local rev = rows[1].rev
          out[#out + 1] = "rev:" .. tostring(rev)
          local report = zellij.data.apply(
            zellij.data.put("note", { title = "second" }, "n1", rev))
          out[#out + 1] = "per_mutation:" .. tostring(report.per_mutation[1])

          -- The same revision a second time is stale, and the row keeps what it had.
          local fresh = pcall(function()
            zellij.data.apply(zellij.data.put("note", { title = "third" }, "n1", rev))
          end)
          out[#out + 1] = "stale:" .. tostring(fresh)
          out[#out + 1] = "title:" .. zellij.data.fetch { entity = "note" }[1].title

          return table.concat(out, "|")
        end
        return M
    "#;
    let store = tempfile::tempdir().unwrap();
    let mut env = test_env();
    env.permissions = Arc::new(Mutex::new(Some(PermissionType::iter().collect())));
    env.data_access = Arc::new(DataAccess::sqlite(store.path()));
    let mut plugin = load_lua_plugin(source, "test.lua", env).unwrap();
    let out = String::from_utf8(plugin.render(1, 500).unwrap()).unwrap();
    assert_eq!(
        out.trim(),
        "claimed-twice:false|rev:1|per_mutation:1|stale:false|title:second"
    );
}

/// The catalog is what a plugin reaches for when it does not know the schema, so what matters is
/// that a declared entity comes back with its columns readable as plain Lua - and that the kind is
/// spelled the way `zellij.data.define` spells it, not the way the wire does.
#[test]
fn the_catalog_answers_with_the_entities_the_plugin_may_see() {
    let source = r#"
        local M = {}
        function M.render()
          zellij.data.define {
            name = "note", version = 2,
            attrs = { { "title", "text", required = true }, { "done", "bool", indexed = true } },
          }
          local out = {}
          for _, e in ipairs(zellij.data.catalog()) do
            local attrs = {}
            for _, a in ipairs(e.attrs) do
              attrs[#attrs + 1] = a.name .. ":" .. a.kind .. ":" .. tostring(a.indexed)
            end
            out[#out + 1] = e.entity .. "@" .. e.version .. "[" .. table.concat(attrs, ",") .. "]"
          end
          return table.concat(out, "|")
        end
        return M
    "#;
    let store = tempfile::tempdir().unwrap();
    let mut env = test_env();
    env.permissions = Arc::new(Mutex::new(Some(PermissionType::iter().collect())));
    env.data_access = Arc::new(DataAccess::sqlite(store.path()));
    let mut plugin = load_lua_plugin(source, "test.lua", env).unwrap();
    let out = String::from_utf8(plugin.render(1, 500).unwrap()).unwrap();
    assert_eq!(out.trim(), "note@2[title:text:false,done:bool:true]");
}

/// `example-plugins/hocus-todos.lua` is what "Hocus data" in the guide points at, so it should not
/// be allowed to rot - and it is the end-to-end proof that a Lua plugin reads hocus's rows with no
/// bridge and no declaration of its own: the store here is seeded the way `hocus-store` seeds it.
#[test]
fn the_hocus_todos_example_reads_a_project_list_out_of_the_shared_namespace() {
    use zellij_utils::data::{AttrDef, EntityDef, Mutation, PermissionStatus};

    let store_dir = tempfile::tempdir().unwrap();
    let data_access = Arc::new(DataAccess::sqlite(store_dir.path()));
    let store = data_access.plugin_data();
    // Exactly the definitions `hocus-proto`'s `store::shared_defs()` declares for these two, by
    // another plugin - this one declares nothing.
    store
        .define_entity(
            "shared:hocus",
            "zellij:hocus-store",
            &EntityDef::new("project", 1)
                .attr(AttrDef::text("name").required())
                .attr(AttrDef::text("root").indexed().required()),
            false,
        )
        .unwrap();
    store
        .define_entity(
            "shared:hocus",
            "zellij:hocus-store",
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
            "shared:hocus",
            &[
                Mutation::put("project")
                    .uid("work")
                    .set("name", "work")
                    .set("root", ".")
                    .build(),
                // A shallower root that also matches, to prove the longest one wins.
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
            "shared:hocus",
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
            "shared:hocus",
            &[Mutation::put("todo")
                .set("root", ".")
                .set("parent_id", parent)
                .set("text", "write the test")
                .set("done", true)
                .build()],
        )
        .unwrap();

    let source = example_plugin("hocus-todos.lua");
    let mut env = test_env();
    env.permissions = Arc::new(Mutex::new(Some(PermissionType::iter().collect())));
    env.data_access = data_access;
    let mut plugin = load_lua_plugin(&source, "hocus-todos.lua", env).unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    plugin
        .update(&Event::PermissionRequestResult(PermissionStatus::Granted))
        .unwrap();

    let rendered = String::from_utf8(plugin.render(10, 60).unwrap()).unwrap();
    // The UI components carry their text as byte codes, so the assertions have to as well.
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
    // Indented one level under its parent by the `parent_id` edge, and crossed off. The list's
    // items are `;`-separated and an indent is a leading `|`.
    let child = rendered
        .split(';')
        .find(|item| item.contains(&codes("x write the test")))
        .unwrap_or_else(|| panic!("no child item in: {rendered}"));
    assert!(child.starts_with('|'), "not indented: {child}");
}

/// `example-plugins/hocus-notes.lua` is the other plugin "Hocus data" points at. It reads three
/// entities - the notes, then their tags and links by id - so the store here is seeded with all
/// three, exactly as `hocus-proto`'s `store::shared_defs()` declares them.
#[test]
fn the_hocus_notes_example_reads_a_projects_notes_out_of_the_shared_namespace() {
    use zellij_utils::data::{AttrDef, EntityDef, Mutation, PermissionStatus};

    let store_dir = tempfile::tempdir().unwrap();
    let data_access = Arc::new(DataAccess::sqlite(store_dir.path()));
    let store = data_access.plugin_data();
    let owner = "zellij:hocus-store";
    store
        .define_entity(
            "shared:hocus",
            owner,
            &EntityDef::new("project", 1)
                .attr(AttrDef::text("name").required())
                .attr(AttrDef::text("root").indexed().required()),
            false,
        )
        .unwrap();
    store
        .define_entity(
            "shared:hocus",
            owner,
            &EntityDef::new("note", 1)
                .attr(AttrDef::text("root").indexed())
                .attr(AttrDef::text("title").full_text().required())
                .attr(AttrDef::text("body").full_text().required())
                .attr(AttrDef::text("author").required().default("user"))
                .attr(
                    AttrDef::boolean("archived")
                        .indexed()
                        .required()
                        .default(false),
                ),
            false,
        )
        .unwrap();
    store
        .define_entity(
            "shared:hocus",
            owner,
            &EntityDef::new("note_link", 1)
                .attr(AttrDef::int("note_id").indexed().required())
                .attr(AttrDef::int("todo_id").indexed().required()),
            false,
        )
        .unwrap();
    store
        .define_entity(
            "shared:hocus",
            owner,
            &EntityDef::new("note_tag", 1)
                .attr(AttrDef::int("note_id").indexed().required())
                .attr(AttrDef::text("tag").indexed().required()),
            false,
        )
        .unwrap();
    store
        .apply(
            "shared:hocus",
            &[Mutation::put("project")
                .uid("work")
                .set("name", "work")
                .set("root", ".")
                .build()],
        )
        .unwrap();
    let live = store
        .apply(
            "shared:hocus",
            &[Mutation::put("note")
                .set("root", ".")
                .set("title", "why the parser")
                .set("body", "because")
                .set("author", "hocus")
                .set("archived", false)
                .build()],
        )
        .unwrap()
        .report
        .ids[0];
    store
        .apply(
            "shared:hocus",
            &[
                // Archived, so the example must leave it out.
                Mutation::put("note")
                    .set("root", ".")
                    .set("title", "old thought")
                    .set("body", "gone")
                    .set("author", "user")
                    .set("archived", true)
                    .build(),
                // Another project's, so the root filter must leave it out.
                Mutation::put("note")
                    .set("root", "/elsewhere")
                    .set("title", "elsewhere")
                    .set("body", "b")
                    .set("author", "user")
                    .set("archived", false)
                    .build(),
                Mutation::put("note_tag")
                    .uid(format!("{live}:design"))
                    .set("note_id", live)
                    .set("tag", "design")
                    .build(),
                Mutation::put("note_link")
                    .uid(format!("{live}:7"))
                    .set("note_id", live)
                    .set("todo_id", 7)
                    .build(),
            ],
        )
        .unwrap();

    let source = example_plugin("hocus-notes.lua");
    let mut env = test_env();
    env.permissions = Arc::new(Mutex::new(Some(PermissionType::iter().collect())));
    env.data_access = data_access;
    let mut plugin = load_lua_plugin(&source, "hocus-notes.lua", env).unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    plugin
        .update(&Event::PermissionRequestResult(PermissionStatus::Granted))
        .unwrap();

    let rendered = String::from_utf8(plugin.render(10, 60).unwrap()).unwrap();
    let codes = |text: &str| {
        text.bytes()
            .map(|b| b.to_string())
            .collect::<Vec<_>>()
            .join(",")
    };
    assert!(rendered.contains(&codes("work notes")), "{rendered}");
    assert!(rendered.contains(&codes("why the parser")), "{rendered}");
    assert!(
        !rendered.contains(&codes("old thought")),
        "archived: {rendered}"
    );
    assert!(
        !rendered.contains(&codes("elsewhere")),
        "other root: {rendered}"
    );
    // The detail line under the title, from the two side tables, indented one level.
    let detail = rendered
        .split(';')
        .find(|item| item.contains(&codes("#design")))
        .unwrap_or_else(|| panic!("no detail line in: {rendered}"));
    assert!(detail.starts_with('|'), "not indented: {detail}");
    assert!(detail.contains(&codes("todos 7")), "{detail}");
    assert!(detail.contains(&codes("by hocus")), "{detail}");
}

/// What the catalog is allowed to show, which is the one new rule the command carries.
///
/// `SharedPluginData` is a claim on `shared:` and nothing else, so another plugin's *private*
/// namespace stays invisible either way - this is the only call that could otherwise enumerate
/// namespaces nobody chose to publish.
#[test]
fn the_catalog_shows_shared_namespaces_only_with_the_permission_and_never_a_private_one() {
    let source = r#"
        local M = {}
        function M.render()
          zellij.data.define { name = "mine", attrs = { { "t", "text" } } }
          local ok, shared = pcall(zellij.data.catalog, "shared:hocus")
          local seen = {}
          for _, e in ipairs(zellij.data.catalog()) do
            seen[#seen + 1] = e.namespace .. "/" .. e.entity
          end
          return table.concat(seen, ",") .. " | " .. (ok and ("shared:" .. #shared) or "refused")
        end
        return M
    "#;
    let store_dir = tempfile::tempdir().unwrap();
    let data_access = Arc::new(DataAccess::sqlite(store_dir.path()));
    let store = data_access.plugin_data();
    // Two namespaces this plugin did not declare: one published, one another plugin's own.
    for (namespace, entity) in [("shared:hocus", "todo"), ("plugin:someone_else", "secret")] {
        store
            .define_entity(
                namespace,
                "zellij:elsewhere",
                &zellij_utils::data::EntityDef::new(entity, 1)
                    .attr(zellij_utils::data::AttrDef::text("text")),
                false,
            )
            .unwrap();
    }

    let run = |permissions: Vec<PermissionType>| {
        let mut env = test_env();
        env.permissions = Arc::new(Mutex::new(Some(permissions.into_iter().collect())));
        env.data_access = data_access.clone();
        let mut plugin = load_lua_plugin(source, "test.lua", env).unwrap();
        String::from_utf8(plugin.render(1, 200).unwrap())
            .unwrap()
            .trim()
            .to_owned()
    };

    let own_only = run(vec![PermissionType::PluginData]);
    assert!(own_only.contains("/mine"), "{own_only}");
    assert!(!own_only.contains("shared:hocus/todo"), "{own_only}");
    assert!(own_only.ends_with("| refused"), "{own_only}");

    let with_shared = run(vec![
        PermissionType::PluginData,
        PermissionType::SharedPluginData,
    ]);
    assert!(with_shared.contains("shared:hocus/todo"), "{with_shared}");
    assert!(with_shared.ends_with("| shared:1"), "{with_shared}");

    // Never, on either run.
    assert!(!own_only.contains("someone_else"), "{own_only}");
    assert!(!with_shared.contains("someone_else"), "{with_shared}");
}

// Pane workflows ------------------------------------------------------------------------------
//
// `zellij.pane_workflow` is pure prelude: it sequences `open_command_pane` /
// `run_command_in_pane` calls off `command_pane_exited` events. These drive it end to end with
// the pane-facing commands stubbed out in Lua, so what it *did* is read back from a log and
// what *happened* is synthetic events - no pty, no screen.

/// A plugin running one workflow over `steps`, with every outward pane command logged instead
/// of dispatched. A `key` event cancels the workflow; `input_received` pushes a step "d".
fn workflow_plugin(options: &str) -> super::LuaPlugin {
    let source = format!(
        r#"
        local zellij = require("zellij")
        local M = {{}}
        M.log = {{}}
        local function log(entry) table.insert(M.log, entry) end
        zellij.open_command_pane = function(args)
          log("open:" .. args[1].path)
          return {{ Terminal = 7 }}
        end
        zellij.run_command_in_pane = function(args)
          local cwd = args.command.cwd and ":" .. args.command.cwd or ""
          log("run:" .. args.command.path .. ":" .. tostring(args.keep_scrollback) .. cwd)
        end
        zellij.send_sigint_to_pane_id = function() log("sigint") end
        zellij.send_sigkill_to_pane_id = function() log("sigkill") end
        zellij.close_terminal_pane = function() log("close") end
        zellij.write_chars_to_pane_id = function(chars) log("stdin:" .. chars) end
        function M.load()
          M.wf = zellij.pane_workflow {{
            {options}
            on_step = function(n, step, code) log("step" .. n .. "=" .. tostring(code)) end,
            on_done = function(r) log("done:" .. r.status) end,
          }}
        end
        function M.update(event)
          if event.name == "key" then M.wf:cancel() end
          if event.name == "input_received" then M.wf:push {{ path = "d" }} end
          return false
        end
        function M.render() return table.concat(M.log, " ") end
        return M
        "#
    );
    let mut plugin = load(&source).unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    plugin
}

fn workflow_context(step: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("__wf".to_owned(), "wf1".to_owned()),
        ("__step".to_owned(), step.to_owned()),
    ])
}

fn step_exited(step: &str, code: Option<i32>) -> Event {
    Event::CommandPaneExited(7, code, workflow_context(step))
}

fn logged(plugin: &mut super::LuaPlugin) -> String {
    String::from_utf8(plugin.render(1, 10).unwrap()).unwrap()
}

#[test]
fn a_workflow_subscribes_before_opening_its_first_pane() {
    use zellij_utils::data::EventType;

    for floating in [false, true] {
        let mut plugin = load(
            r#"
            local zellij = require("zellij")
            local M = { log = {} }
            local function log(entry) table.insert(M.log, entry) end
            zellij.write_chars_to_pane_id = function(chars) log("stdin:" .. chars) end
            zellij.set_timeout = function() log("timeout") end
            function M.load(config) M.floating = config.floating == "true" end
            function M.update(event)
              if event.name == "input_received" then
                M.wf = zellij.pane_workflow {
                  floating = M.floating,
                  steps = { { path = "a", stdin = "hello", timeout = 60 } },
                  on_done = function(r) log("done:" .. r.status) end,
                }
              end
            end
            function M.render() return table.concat(M.log, " ") end
            return M
            "#,
        )
        .unwrap();
        let subscriptions = plugin.env().subscriptions.clone();
        let open = plugin
            .lua
            .create_function(move |lua, _: mlua::Value| {
                // The host filters events while the synchronous open is still in progress.
                let subscriptions = subscriptions.lock().unwrap();
                for event in [
                    EventType::CommandPaneOpened,
                    EventType::CommandPaneReRun,
                    EventType::CommandPaneExited,
                    EventType::PaneClosed,
                ] {
                    assert!(subscriptions.contains(&event), "missing {event:?} at spawn");
                }
                let pane = lua.create_table()?;
                pane.set("Terminal", 7)?;
                Ok(pane)
            })
            .unwrap();
        let command = if floating {
            "open_command_pane_floating"
        } else {
            "open_command_pane"
        };
        plugin.zellij.set(command, open).unwrap();
        plugin
            .load(&BTreeMap::from([("floating".into(), floating.to_string())]))
            .unwrap();
        plugin.update(&Event::InputReceived).unwrap();
        assert_eq!(logged(&mut plugin), "");
        plugin
            .update(&Event::CommandPaneOpened(7, workflow_context("1")))
            .unwrap();
        assert_eq!(logged(&mut plugin), "stdin:hello timeout");
        plugin.update(&step_exited("1", Some(0))).unwrap();
        assert_eq!(logged(&mut plugin), "stdin:hello timeout done:success");
    }
}

#[test]
fn a_pane_workflow_runs_each_step_in_the_same_pane() {
    let mut plugin =
        workflow_plugin(r#"steps = { { path = "a" }, { path = "b" }, { path = "c" } },"#);
    plugin.update(&step_exited("1", Some(0))).unwrap();
    plugin.update(&step_exited("2", Some(0))).unwrap();
    plugin.update(&step_exited("3", Some(0))).unwrap();
    assert_eq!(
        logged(&mut plugin),
        "open:a step1=0 run:b:true step2=0 run:c:true step3=0 done:success",
        "one pane opened, every later step respawned into it, scrollback kept by default"
    );
}

#[test]
fn a_failing_step_ends_the_workflow_by_default() {
    let mut plugin = workflow_plugin(r#"steps = { { path = "a" }, { path = "b" } },"#);
    plugin.update(&step_exited("1", Some(2))).unwrap();
    assert_eq!(
        logged(&mut plugin),
        "open:a step1=2 done:failed",
        "step b never ran"
    );
}

#[test]
fn stop_on_failure_false_runs_on_through_a_failure() {
    let mut plugin =
        workflow_plugin(r#"steps = { { path = "a" }, { path = "b" } }, stop_on_failure = false,"#);
    plugin.update(&step_exited("1", Some(2))).unwrap();
    plugin.update(&step_exited("2", Some(0))).unwrap();
    assert_eq!(
        logged(&mut plugin),
        "open:a step1=2 run:b:true step2=0 done:success"
    );
}

#[test]
fn cancelling_interrupts_the_running_step_and_reports_cancelled() {
    let mut plugin = workflow_plugin(r#"steps = { { path = "a" }, { path = "b" } },"#);
    plugin
        .update(&Event::Key(KeyWithModifier::new(BareKey::Char('c'))))
        .unwrap();
    // The interrupted step's exit - killed, so no code - completes the cancellation.
    plugin.update(&step_exited("1", None)).unwrap();
    assert_eq!(
        logged(&mut plugin),
        "open:a sigint step1=nil done:cancelled",
        "no further step ran"
    );
}

#[test]
fn a_workflow_inherits_cwd_only_on_the_same_host() {
    use zellij_utils::data::{PaneInfo, PaneManifest};

    let mut plugin = workflow_plugin(
        r#"steps = {
          { path = "local" },
          { path = "remote-a", remote_host = "build-box", cwd = "inherit" },
          { path = "remote-b", remote_host = "build-box", cwd = "inherit" },
          { path = "other-host", remote_host = "deploy-box", cwd = "inherit" },
        },"#,
    );
    let pane_update = |cwd: &str, host: Option<&str>| {
        let mut manifest = PaneManifest::default();
        manifest.panes.insert(
            0,
            vec![PaneInfo {
                id: 7,
                cwd: Some(cwd.to_owned()),
                host: host.map(str::to_owned),
                ..Default::default()
            }],
        );
        Event::PaneUpdate(manifest)
    };

    plugin.update(&pane_update("/local/work", None)).unwrap();
    plugin.update(&step_exited("1", Some(0))).unwrap();
    plugin
        .update(&pane_update("/remote/work", Some("build-box")))
        .unwrap();
    plugin.update(&step_exited("2", Some(0))).unwrap();
    plugin.update(&step_exited("3", Some(0))).unwrap();

    assert_eq!(
        logged(&mut plugin),
        "open:local step1=0 run:remote-a:true step2=0 \
         run:remote-b:true:/remote/work step3=0 run:other-host:true",
    );
}

#[test]
fn denied_cancellation_does_not_advance_or_finish_the_workflow() {
    let mut plugin = load(
        r#"
        local zellij = require("zellij")
        local M = { done = false }
        zellij.open_command_pane = function() return { Terminal = 7 } end
        function M.load()
          M.wf = zellij.pane_workflow {
            steps = { { path = "a" } },
            on_done = function() M.done = true end,
          }
        end
        function M.update(event)
          if event.name == "key" then M.wf:cancel() end
          return false
        end
        function M.render() return M.wf.status .. ":" .. tostring(M.done) end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();

    let error = plugin
        .update(&Event::Key(KeyWithModifier::new(BareKey::Char('c'))))
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("ChangeApplicationState"),
        "got: {error:#}"
    );
    assert_eq!(plugin.render(1, 20).unwrap(), b"running:false");
}

#[test]
fn a_step_timeout_escalates_from_sigint_to_sigkill() {
    let mut plugin = workflow_plugin(
        r#"steps = { { path = "a", timeout = 0 }, { path = "b" } }, kill_grace = 0,"#,
    );
    plugin
        .update(&Event::CommandPaneOpened(7, workflow_context("1")))
        .unwrap();
    plugin.update(&Event::Timer(1.0)).unwrap();
    plugin.update(&Event::Timer(1.0)).unwrap();
    assert_eq!(logged(&mut plugin), "open:a sigint sigkill");
    // Even a child that handles the signal and exits zero still timed out.
    plugin.update(&step_exited("1", Some(0))).unwrap();
    assert_eq!(
        logged(&mut plugin),
        "open:a sigint sigkill step1=0 done:failed"
    );
}

#[test]
fn a_queued_steps_timeout_starts_only_when_that_step_does() {
    let mut plugin = workflow_plugin(
        r#"steps = { { path = "a" }, { path = "b", timeout = 0 } }, kill_grace = 0,"#,
    );
    plugin.update(&step_exited("1", Some(0))).unwrap();

    // A manual rerun of step 1 is ahead of step 2 in the pane. Neither that stale start nor a
    // timer event may interrupt it on step 2's behalf.
    plugin
        .update(&Event::CommandPaneReRun(7, workflow_context("1")))
        .unwrap();
    plugin.update(&Event::Timer(1.0)).unwrap();
    assert_eq!(logged(&mut plugin), "open:a step1=0 run:b:true");

    plugin
        .update(&Event::CommandPaneReRun(7, workflow_context("2")))
        .unwrap();
    plugin.update(&Event::Timer(1.0)).unwrap();
    assert_eq!(logged(&mut plugin), "open:a step1=0 run:b:true sigint");
}

#[test]
fn a_step_pushed_while_running_extends_the_workflow() {
    let mut plugin = workflow_plugin(r#"steps = { { path = "a" }, { path = "b" } },"#);
    plugin.update(&Event::InputReceived).unwrap();
    plugin.update(&step_exited("1", Some(0))).unwrap();
    plugin.update(&step_exited("2", Some(0))).unwrap();
    plugin.update(&step_exited("3", Some(0))).unwrap();
    assert_eq!(
        logged(&mut plugin),
        "open:a step1=0 run:b:true step2=0 run:d:true step3=0 done:success"
    );
}

#[test]
fn the_workflows_pane_closing_ends_it() {
    let mut plugin = workflow_plugin(r#"steps = { { path = "a" }, { path = "b" } },"#);
    plugin
        .update(&Event::PaneClosed(zellij_utils::data::PaneId::Terminal(7)))
        .unwrap();
    assert_eq!(logged(&mut plugin), "open:a done:pane_closed");
}

#[test]
fn a_stale_exit_from_an_earlier_step_is_ignored() {
    // The user pressed ENTER on the held pane between steps, re-running a finished step; its
    // second exit names a step the workflow has moved past and must not advance anything.
    let mut plugin =
        workflow_plugin(r#"steps = { { path = "a" }, { path = "b" }, { path = "c" } },"#);
    plugin.update(&step_exited("1", Some(0))).unwrap();
    plugin.update(&step_exited("1", Some(0))).unwrap();
    assert_eq!(
        logged(&mut plugin),
        "open:a step1=0 run:b:true",
        "the workflow is still waiting for step 2"
    );
}

#[test]
fn close_on_success_closes_the_pane_after_a_clean_run() {
    let mut plugin = workflow_plugin(r#"steps = { { path = "a" } }, close_on_success = true,"#);
    plugin.update(&step_exited("1", Some(0))).unwrap();
    assert_eq!(logged(&mut plugin), "open:a step1=0 close done:success");
}

#[test]
fn close_on_success_surfaces_a_missing_change_state_permission() {
    let mut plugin = load(
        r#"
        local zellij = require("zellij")
        local M = { done = false }
        zellij.open_command_pane = function() return { Terminal = 7 } end
        function M.load()
          M.wf = zellij.pane_workflow {
            steps = { { path = "a" } },
            close_on_success = true,
            on_done = function() M.done = true end,
          }
        end
        function M.render() return tostring(M.done) end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();

    let error = plugin.update(&step_exited("1", Some(0))).unwrap_err();
    assert!(
        format!("{error:#}").contains("ChangeApplicationState"),
        "got: {error:#}"
    );
    assert_eq!(plugin.render(1, 5).unwrap(), b"false");
}

#[test]
fn a_steps_stdin_is_written_when_the_step_starts() {
    let mut plugin =
        workflow_plugin(r#"steps = { { path = "a", stdin = "secret" }, { path = "b" } },"#);
    plugin
        .update(&Event::CommandPaneOpened(7, workflow_context("1")))
        .unwrap();
    assert_eq!(
        logged(&mut plugin),
        "open:a stdin:secret",
        "the secret goes to the child's stdin, never into argv"
    );
}

#[test]
fn run_command_in_pane_takes_a_numeric_pane_id() {
    // The named wrapper builds the `PaneId` and the struct payload; this drives it through the
    // real marshaller to the host.
    assert_marshals(
        r#"zellij.run_command_in_pane {
             pane_id = 3,
             command = { path = "sh", args = { "-c", "true" } },
             context = { step = "2" },
             keep_scrollback = true,
           }"#,
    );
}

/// `open_command_pane` reaches the pty thread as a `SpawnTerminal`; a remote
/// command's cwd is a path on that machine, so it travels as written and a
/// missing one is not filled in from the plugin's (host) cwd, while a local
/// command's `/host` still names the plugin's cwd.
#[test]
fn a_remote_command_pane_keeps_its_guest_cwd() {
    use crate::pty::PtyInstruction;
    use zellij_utils::channels::SenderWithContext;
    use zellij_utils::input::command::TerminalAction;

    let mut plugin = load_permitted(
        r#"
        local zellij = require("zellij")
        local M = {}
        function M.load()
          zellij.open_command_pane { { path = "remote", remote_host = "build-box" }, {} }
          zellij.open_command_pane {
            { path = "remote-explicit", cwd = "/srv/work", remote_host = "build-box" }, {}
          }
          zellij.open_command_pane { { path = "local", cwd = "/host/src" }, {} }
          for _, mount in ipairs({'/tmp', '/host', '/data', '/cache'}) do
            -- A dropped completion has no pane ID; the spawn instruction is what we inspect.
            pcall(zellij.agent.spawn, {
              kind = 'claude', cwd = mount .. '/checkout',
              context = {agent_workflow_role='message', agent_workflow_run='test'},
            })
            zellij.open_command_pane_near_plugin {{path='ordinary', cwd=mount .. '/checkout'}, {}}
          end
        end
        return M
        "#,
    )
    .unwrap();
    plugin.env_mut().plugin_cwd = PathBuf::from("/plugin/cwd");
    let (pty_tx, pty_rx) = zellij_utils::channels::unbounded();
    plugin.env_mut().senders.to_pty = Some(SenderWithContext::new(pty_tx));
    let spawned = Arc::new(Mutex::new(vec![]));
    let sink = spawned.clone();
    // Each spawn carries a completion the plugin call waits on; dropping the
    // instruction here is what answers it, as the pty thread would.
    let pump = std::thread::spawn(move || {
        for (instruction, _) in pty_rx.iter() {
            match instruction {
                PtyInstruction::SpawnTerminal(Some(TerminalAction::RunCommand(command)), ..) => {
                    sink.lock().unwrap().push((
                        command.command.clone(),
                        command.cwd.clone(),
                        command.target.clone(),
                    ))
                },
                PtyInstruction::Exit => break,
                _ => {},
            }
        }
    });
    plugin.load(&BTreeMap::new()).unwrap();
    plugin
        .env_mut()
        .senders
        .send_to_pty(PtyInstruction::Exit)
        .unwrap();
    pump.join().unwrap();

    let ssh = zellij_utils::data::PaneTarget::Ssh {
        host: "build-box".to_owned(),
    };
    let spawned = spawned.lock().unwrap();
    assert_eq!(
        spawned[..3],
        vec![
            (PathBuf::from("remote"), None, ssh.clone()),
            (
                PathBuf::from("remote-explicit"),
                Some(PathBuf::from("/srv/work")),
                ssh
            ),
            (
                PathBuf::from("local"),
                Some(PathBuf::from("/plugin/cwd/src")),
                zellij_utils::data::PaneTarget::Local
            ),
        ]
    );
    for (pair, mount) in spawned[3..]
        .as_chunks::<2>()
        .0
        .iter()
        .zip(["/tmp", "/host", "/data", "/cache"])
    {
        assert_eq!(pair[0].1, Some(PathBuf::from(mount).join("checkout")));
        let translated = match mount {
            "/tmp" => zellij_utils::consts::ZELLIJ_TMP_DIR.join("checkout"),
            "/host" => PathBuf::from("/plugin/cwd/checkout"),
            _ => PathBuf::from("./checkout"),
        };
        assert_eq!(pair[1].1, Some(translated));
    }
    assert_eq!(spawned.len(), 11);
}

#[test]
fn a_remote_replacement_does_not_inherit_the_plugins_cwd() {
    let mut plugin = load_permitted(
        r#"
        local zellij = require("zellij")
        local M = {}
        function M.load()
          zellij.run_command_in_pane {
            pane_id = 3,
            command = { path = "remote", remote_host = "build-box" },
          }
          zellij.run_command_in_pane {
            pane_id = 3,
            command = { path = "local" },
          }
          zellij.run_command_in_pane {
            pane_id = 3,
            command = { path = "remote-explicit", cwd = "/srv/work", remote_host = "build-box" },
          }
        end
        return M
        "#,
    )
    .unwrap();
    plugin.env_mut().plugin_cwd = PathBuf::from("/plugin/cwd");
    let (screen_tx, screen_rx) = zellij_utils::channels::unbounded();
    plugin.env_mut().senders.to_screen =
        Some(zellij_utils::channels::SenderWithContext::new(screen_tx));
    plugin.load(&BTreeMap::new()).unwrap();

    let cwds: Vec<Option<PathBuf>> = screen_rx
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            crate::screen::ScreenInstruction::RunCommandInPane(_, command, _) => Some(command.cwd),
            _ => None,
        })
        .collect();
    assert_eq!(
        cwds,
        vec![
            None,
            Some(PathBuf::from("/plugin/cwd")),
            Some(PathBuf::from("/srv/work")),
        ]
    );
}

#[test]
fn the_workflow_example_loads_and_runs_on_r() {
    // `example-plugins/workflow.lua` is what the pane-workflows documentation points at, so it
    // should not be allowed to rot. Pressing `r` starts the workflow, which opens a real
    // command pane through the host - the marshalling most likely to be got wrong.
    let source = example_plugin("workflow.lua");
    let mut env = test_env();
    env.permissions = Arc::new(Mutex::new(Some(PermissionType::iter().collect())));
    let mut plugin = load_lua_plugin(&source, "workflow.lua", env).unwrap();
    plugin.load(&BTreeMap::new()).unwrap();

    let rendered = String::from_utf8(plugin.render(12, 70).unwrap()).unwrap();
    assert!(rendered.contains("r: run the workflow"), "got: {rendered}");

    // With no pty behind the test env the pane id comes back empty, which the workflow reports
    // as an error naming itself rather than starting half-blind.
    let result = plugin.update(&Event::Key(KeyWithModifier::new(BareKey::Char('r'))));
    let error = format!("{:?}", result);
    assert!(
        error.contains("pane_workflow") || result.is_ok(),
        "pressing r must reach pane_workflow, got: {error}"
    );
}

/// Load a plugin whose `update` is `body`, set its cancel token while `update` runs, and
/// return the error `update` came back with — the test fails if it comes back at all.
fn cancel_mid_update(body: &str) -> anyhow::Error {
    let source = format!(
        r#"
        local M = {{}}
        function M.update(event)
          {body}
        end
        return M
    "#
    );
    let env = test_env();
    let cancel = env.cancel.clone();
    let tripped = env.cancel_tripped.clone();
    let mut plugin = load_lua_plugin(&source, "test.lua", env).unwrap();
    // Set from another thread while `update` is inside its loop. If the flag lands before the
    // loop starts the hook fires on the loop's first instructions instead, so the order does
    // not matter for the outcome - the delay only makes the interesting case the usual one.
    let setter = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    let error = plugin
        .update(&Event::InputReceived)
        .expect_err("the loop was stopped");
    setter.join().unwrap();
    assert!(
        tripped.load(std::sync::atomic::Ordering::Relaxed),
        "the hook records that it stopped the guest"
    );
    error
}

/// The cancel token stops a loop that never calls the host: the instruction-count hook sees the
/// flag and raises, and the entrypoint comes back as an error naming the cause.
#[test]
fn a_cancelled_plugin_stops_mid_loop() {
    let error = cancel_mid_update("while true do end");
    assert!(
        format!("{error:?}").contains(crate::plugins::plugin_map::CANCELLED_BY_HOST),
        "{error:?}"
    );
}

/// A coroutine inherits the interpreter's hook. (With a per-thread hook it would not: mlua
/// looks the callback up by thread, finds none for the coroutine, and uninstalls the hook
/// there, so a loop inside `coroutine.wrap` ran unhooked.)
#[test]
fn a_cancelled_plugin_stops_inside_a_coroutine() {
    let error = cancel_mid_update(
        r#"
        local step = coroutine.wrap(function()
          while true do end
        end)
        step()
        "#,
    );
    assert!(
        format!("{error:?}").contains(crate::plugins::plugin_map::CANCELLED_BY_HOST),
        "{error:?}"
    );
}

/// The cancel is an ordinary Lua error, so `pcall` would catch it and a loop around a `pcall`
/// would keep going — the prelude's wrappers re-raise it instead. `xpcall` and
/// `coroutine.resume`, which also report errors as values, are wrapped the same way.
#[test]
fn a_cancelled_plugin_stops_inside_a_pcall_loop() {
    for body in [
        "while true do pcall(function() while true do end end) end",
        "while true do xpcall(function() while true do end end, function(e) return e end) end",
        // A handler that replaces the message would hide the marker from a result check: the
        // prelude notes the cancel in the handler itself, before anything can rewrite it.
        "while true do xpcall(function() while true do end end, function(_) return 'failed' end) end",
        "while true do xpcall(function() while true do end end, function() end) end",
        "while true do xpcall(function() while true do end end, function() error('again') end) end",
        "while true do coroutine.resume(coroutine.create(function() while true do end end)) end",
    ] {
        let error = cancel_mid_update(body);
        assert!(
            format!("{error:?}").contains(crate::plugins::plugin_map::CANCELLED_BY_HOST),
            "{body}: {error:?}"
        );
    }
}

/// The wrappers change nothing for any other error: `pcall` and friends report it as a value,
/// as they always did.
#[test]
fn pcall_still_reports_ordinary_errors_as_values() {
    let source = r#"
        local M = {}
        function M.update(event)
          local ok, err = pcall(error, "boom")
          assert(not ok and string.find(err, "boom"), "pcall changed: " .. tostring(err))
          local ok, err = xpcall(error, function(e) return "handled " .. e end, "boom")
          assert(not ok and err == "handled boom", "xpcall changed: " .. tostring(err))
          local ok, sum = xpcall(function(a, b) return a + b end, print, 2, 3)
          assert(ok and sum == 5, "xpcall dropped its extra arguments: " .. tostring(sum))
          local ok, err = pcall(xpcall, error, "not a handler")
          assert(not ok and string.find(tostring(err), "bad argument"), "xpcall arg check changed: " .. tostring(err))
          local ok, err = coroutine.resume(coroutine.create(function() error("boom") end))
          assert(not ok and string.find(err, "boom"), "resume changed: " .. tostring(err))
          local ok, a, b = pcall(function() return 1, 2 end)
          assert(ok and a == 1 and b == 2, "pcall lost results")
          return true
        end
        return M
    "#;
    let mut plugin = load_lua_plugin(source, "test.lua", test_env()).unwrap();
    assert!(plugin.update(&Event::InputReceived).unwrap());
}

// Tasks ---------------------------------------------------------------------------------------
//
// `zellij.task` / `zellij.await` and the awaitables under `zellij.async` are pure prelude: a
// coroutine per task, resumed from `__update`/`__pipe` when the event its awaitable was
// registered for arrives. These drive that with synthetic events, the outward commands stubbed
// in Lua so what the task *did* is read back from a log.

/// A plugin whose `load` starts `body` as a task, logging to `M.log`; `zellij.run_command` is
/// stubbed to log the token it was handed, so a test can answer it.
fn task_plugin(body: &str) -> super::LuaPlugin {
    let source = format!(
        r#"
        local zellij = require("zellij")
        local M = {{ log = {{}} }}
        function log(entry) table.insert(M.log, entry) end
        zellij.run_command = function(args) log("run:" .. args.context.__async) end
        zellij.web_request = function(args) log("web:" .. args.context.__async) end
        zellij.set_timeout = function() end
        function M.load()
          M.task = zellij.task(function()
            {body}
          end)
        end
        function M.render() return table.concat(M.log, " ") end
        return M
        "#
    );
    let mut plugin = load_permitted(&source).unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    plugin
}

fn command_result(token: &str, exit_code: i32, stdout: &str) -> Event {
    Event::RunCommandResult(
        Some(exit_code),
        stdout.as_bytes().to_vec(),
        vec![],
        BTreeMap::from([("__async".to_owned(), token.to_owned())]),
    )
}

fn async_token(log: &str, index: usize) -> String {
    log.split_whitespace()
        .filter_map(|entry| {
            entry
                .strip_prefix("run:")
                .or_else(|| entry.strip_prefix("web:"))
                .or_else(|| entry.strip_prefix("token:"))
        })
        .nth(index)
        .expect("the async token was logged")
        .to_owned()
}

#[test]
fn a_task_runs_to_its_first_await_and_resumes_on_the_reply_bearing_its_token() {
    let mut plugin = task_plugin(
        r#"
        log("start")
        local result = zellij.await(zellij.async.run_command { command = { "true" } })
        log("exit:" .. tostring(result.exit_code) .. " " .. result.stdout)
        "#,
    );
    let initial = logged(&mut plugin);
    let token = async_token(&initial, 0);
    assert_eq!(
        initial,
        format!("start run:{token}"),
        "synchronous up to the await"
    );

    // Somebody else's reply is not this task's.
    plugin.update(&command_result("a9", 0, "other")).unwrap();
    assert_eq!(logged(&mut plugin), format!("start run:{token}"));

    plugin.update(&command_result(&token, 0, "out")).unwrap();
    assert_eq!(logged(&mut plugin), format!("start run:{token} exit:0 out"));
}

/// Awaitables are eager: both commands are dispatched before either is awaited, and their
/// replies are kept in whichever order they arrive.
#[test]
fn two_outstanding_commands_resolve_in_whichever_order_they_answer() {
    let mut plugin = task_plugin(
        r#"
        local first = zellij.async.run_command { command = { "a" } }
        local second = zellij.async.run_command { command = { "b" } }
        log("first:" .. zellij.await(first).stdout)
        log("second:" .. zellij.await(second).stdout)
        "#,
    );
    let initial = logged(&mut plugin);
    let first = async_token(&initial, 0);
    let second = async_token(&initial, 1);
    plugin.update(&command_result(&second, 0, "B")).unwrap();
    plugin.update(&command_result(&first, 0, "A")).unwrap();
    assert_eq!(
        logged(&mut plugin),
        format!("run:{first} run:{second} first:A second:B")
    );
}

/// Tasks are resumed before the handlers and `update`, and the event is not consumed.
#[test]
fn an_event_a_task_awaited_still_reaches_handlers_and_update() {
    let mut plugin = load_permitted(
        r#"
        local zellij = require("zellij")
        local M = { log = {} }
        local function log(entry) table.insert(M.log, entry) end
        zellij.run_command = function(args) log("run:" .. args.context.__async) end
        function M.load()
          zellij.task(function()
            zellij.await(zellij.async.run_command { command = { "x" } })
            log("task")
          end)
        end
        zellij.on("run_command_result", function() log("handler") end)
        function M.update(event)
          if event.name == "run_command_result" then log("update") end
        end
        function M.render() return table.concat(M.log, " ") end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    let token = async_token(&logged(&mut plugin), 0);
    plugin.update(&command_result(&token, 0, "")).unwrap();
    assert_eq!(
        logged(&mut plugin),
        format!("run:{token} task handler update")
    );
}

#[test]
fn a_task_that_awaits_the_same_pipe_again_waits_for_the_next_message() {
    let mut plugin = task_plugin(
        r#"
        for i = 1, 2 do
          local message = zellij.await(zellij.async.pipe("go"))
          log("got:" .. tostring(message.payload))
        end
        "#,
    );
    let go = |payload: &str| {
        zellij_utils::data::PipeMessage::new(
            zellij_utils::data::PipeSource::Keybind,
            "go",
            &Some(payload.to_owned()),
            &None,
            false,
        )
    };
    plugin.pipe(&go("one")).unwrap();
    assert_eq!(
        logged(&mut plugin),
        "got:one",
        "the second await waits for the next message"
    );
    plugin.pipe(&go("two")).unwrap();
    assert_eq!(logged(&mut plugin), "got:one got:two");
}

#[test]
fn all_resolves_when_every_child_has_and_race_with_the_first() {
    let mut plugin = task_plugin(
        r#"
        local both = zellij.await(zellij.async.all {
          a = zellij.async.run_command { command = { "a" } },
          b = zellij.async.run_command { command = { "b" } },
        })
        log("all:" .. both.a.stdout .. both.b.stdout)
        local won = zellij.await(zellij.async.race {
          slow = zellij.async.run_command { command = { "slow" } },
          fast = zellij.async.run_command { command = { "fast" } },
        })
        log("race:" .. won.key .. "=" .. won.value.stdout)
        "#,
    );
    let initial = logged(&mut plugin);
    let first = async_token(&initial, 0);
    let second = async_token(&initial, 1);
    plugin.update(&command_result(&second, 0, "B")).unwrap();
    assert_eq!(logged(&mut plugin), initial, "all waits for both");
    plugin.update(&command_result(&first, 0, "A")).unwrap();
    let after_all = logged(&mut plugin);
    let slow = async_token(&after_all, 2);
    let fast = async_token(&after_all, 3);
    assert!(after_all.ends_with(&format!("all:AB run:{slow} run:{fast}")));
    plugin.update(&command_result(&fast, 0, "F")).unwrap();
    assert_eq!(logged(&mut plugin), format!("{after_all} race:fast=F"));
}

#[test]
fn all_with_an_already_settled_child_waits_for_the_pending_ones() {
    let mut plugin = task_plugin(
        r#"
        local none = zellij.await(zellij.async.all {})
        log("empty:" .. tostring(next(none)))
        local aw, resolve = zellij.async.future()
        resolve("x")
        -- The settled child first: iterated first, it used to resolve the aggregate on its own.
        local both = zellij.await(zellij.async.all { aw, zellij.async.run_command { command = { "b" } } })
        log("all:" .. tostring(both[1]) .. "/" .. tostring(both[2] and both[2].stdout))
        "#,
    );
    let initial = logged(&mut plugin);
    let token = async_token(&initial, 0);
    assert_eq!(
        initial,
        format!("empty:nil run:{token}"),
        "an empty list resolves at once; a settled child does not stand in for a pending one"
    );
    plugin.update(&command_result(&token, 0, "B")).unwrap();
    assert_eq!(logged(&mut plugin), format!("{initial} all:x/B"));
}

#[test]
fn race_detaches_its_listeners_without_unregistering_the_losers() {
    let mut plugin = task_plugin(
        r#"
        local abort, resolve_abort = zellij.async.future()
        for i = 1, 3 do
          local step, resolve_step = zellij.async.future()
          local raced = zellij.async.race { step = step, abort = abort }
          resolve_step(i)
          local won = zellij.await(raced)
          assert(won.key == "step" and won.value == i)
          assert(#abort.on_settle == 0, "the losing listener stayed attached")
        end
        local pending = zellij.async.future()
        local raced = zellij.async.race { pending = pending, abort = abort }
        resolve_abort("stop")
        local won = zellij.await(raced)
        assert(won.key == "abort" and won.value == "stop")
        assert(#pending.on_settle == 0, "the final losing listener stayed attached")
        log("done")
        "#,
    );
    assert_eq!(logged(&mut plugin), "done");
}

fn manifest_with_agent(pane_id: u32, state: zellij_utils::data::AgentState) -> Event {
    use zellij_utils::data::{AgentInfo, AgentSource, PaneInfo, PaneManifest};
    let mut manifest = PaneManifest::default();
    manifest.panes.insert(
        0,
        vec![PaneInfo {
            id: pane_id,
            agent: Some(AgentInfo {
                kind: "claude".to_owned(),
                state,
                source: AgentSource::Reported,
                detail: None,
                waiting_on: None,
                wait_needs_a_person: false,
                cwd: None,
                session_id: Some("s1".to_owned()),
                run_id: None,
            }),
            ..Default::default()
        }],
    );
    Event::PaneUpdate(manifest)
}

#[test]
fn agent_state_resolves_on_the_state_asked_for_and_after_the_edge_when_told_to() {
    use zellij_utils::data::AgentState;
    let mut plugin = task_plugin(
        r#"
        local pane = zellij.await(zellij.async.agent_state(7, "waiting"))
        log("waiting:" .. pane.agent.kind)
        zellij.await(zellij.async.agent_state(7, "idle", { after = { "busy" } }))
        log("idle-after-busy")
        "#,
    );
    plugin
        .update(&manifest_with_agent(7, AgentState::Busy))
        .unwrap();
    plugin
        .update(&manifest_with_agent(8, AgentState::Waiting))
        .unwrap();
    assert_eq!(logged(&mut plugin), "", "another pane, and the wrong state");
    plugin
        .update(&manifest_with_agent(7, AgentState::Waiting))
        .unwrap();
    assert_eq!(logged(&mut plugin), "waiting:claude");
    // Idle now is not the edge into idle: the pane has to be seen busy first.
    plugin
        .update(&manifest_with_agent(7, AgentState::Idle))
        .unwrap();
    assert_eq!(logged(&mut plugin), "waiting:claude");
    plugin
        .update(&manifest_with_agent(7, AgentState::Busy))
        .unwrap();
    plugin
        .update(&manifest_with_agent(7, AgentState::Idle))
        .unwrap();
    assert_eq!(logged(&mut plugin), "waiting:claude idle-after-busy");
}

#[test]
fn pane_exit_resolves_on_close_as_well_as_on_exit() {
    let mut plugin = task_plugin(
        r#"
        local exit = zellij.await(zellij.async.pane_exit(7))
        log("exit:" .. tostring(exit.exit_code))
        local closed = zellij.await(zellij.async.pane_exit(7))
        log("closed:" .. tostring(closed.closed))
        "#,
    );
    plugin
        .update(&Event::CommandPaneExited(8, Some(1), BTreeMap::new()))
        .unwrap();
    assert_eq!(logged(&mut plugin), "", "another pane");
    plugin
        .update(&Event::CommandPaneExited(7, Some(3), BTreeMap::new()))
        .unwrap();
    plugin
        .update(&Event::PaneClosed(zellij_utils::data::PaneId::Terminal(7)))
        .unwrap();
    assert_eq!(logged(&mut plugin), "exit:3 closed:true");
}

#[test]
fn an_error_in_a_resumed_task_fails_the_update_that_resumed_it() {
    let mut plugin = task_plugin(
        r#"
        zellij.await(zellij.async.run_command { command = { "x" } })
        error("the task broke")
        "#,
    );
    let token = async_token(&logged(&mut plugin), 0);
    let error = plugin
        .update(&command_result(&token, 0, ""))
        .expect_err("a task error is the entrypoint's error");
    assert!(format!("{error:#}").contains("the task broke"), "{error:#}");
    // Gone: a later reply resumes nothing.
    plugin.update(&command_result("stale", 0, "")).unwrap();
}

/// Two tasks woken by one message: the first raising does not strand the second, which has
/// already been taken off the awaitable. It runs, both are finished, and the error is reported.
#[test]
fn a_raising_task_does_not_strand_the_others_in_its_batch() {
    let mut plugin = load_permitted(
        r#"
        local zellij = require("zellij")
        local M = { log = {} }
        local function log(entry) table.insert(M.log, entry) end
        function M.load()
          M.first = zellij.task(function()
            zellij.await(zellij.async.pipe("go"))
            error("first broke")
          end)
          M.second = zellij.task(function()
            zellij.await(zellij.async.pipe("go"))
            log("second ran")
          end)
        end
        function M.render()
          return table.concat(M.log, " ") .. " running:" .. tostring(M.first:is_running())
            .. "," .. tostring(M.second:is_running())
        end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    assert_eq!(logged(&mut plugin), " running:true,true");
    let go = zellij_utils::data::PipeMessage::new(
        zellij_utils::data::PipeSource::Keybind,
        "go",
        &Some("x".to_owned()),
        &None,
        false,
    );
    let error = plugin
        .pipe(&go)
        .expect_err("the first task's error is the entrypoint's");
    assert!(format!("{error:#}").contains("first broke"), "{error:#}");
    assert_eq!(logged(&mut plugin), "second ran running:false,false");
}

#[test]
fn a_raising_task_drains_newly_ready_tasks_before_reporting_the_error() {
    for entrypoint in ["pipe", "load", "update"] {
        let mut plugin = load_permitted(
            r#"
            local zellij = require("zellij")
            local M = { log = {} }
            function M.load(config)
              local start, resolve = zellij.async.future()
              M.resolve = resolve
              local next_ready, resolve_next = zellij.async.future()
              M.first = zellij.task(function()
                zellij.await(config.entrypoint == "pipe" and zellij.async.pipe("go") or start)
                resolve_next("second ran")
                error("first broke")
              end)
              M.second = zellij.task(function()
                table.insert(M.log, zellij.await(next_ready))
              end)
              if config.entrypoint == "load" then resolve() end
            end
            function M.update() M.resolve() end
            function M.render()
              return table.concat(M.log, " ") .. " running:" .. tostring(M.first:is_running())
                .. "," .. tostring(M.second:is_running())
            end
            return M
            "#,
        )
        .unwrap();
        let loaded = plugin.load(&BTreeMap::from([(
            "entrypoint".to_owned(),
            entrypoint.to_owned(),
        )]));
        let result = match entrypoint {
            "load" => loaded,
            "update" => {
                loaded.unwrap();
                plugin.update(&Event::InputReceived).map(|_| ())
            },
            "pipe" => {
                loaded.unwrap();
                plugin
                    .pipe(&zellij_utils::data::PipeMessage::new(
                        zellij_utils::data::PipeSource::Keybind,
                        "go",
                        &None,
                        &None,
                        false,
                    ))
                    .map(|_| ())
            },
            _ => unreachable!(),
        };
        let error = result.expect_err("the first task's error is the entrypoint's");
        assert!(
            format!("{error:#}").contains("first broke"),
            "{entrypoint}: {error:#}"
        );
        assert_eq!(
            logged(&mut plugin),
            "second ran running:false,false",
            "{entrypoint}"
        );
    }
}

#[test]
fn a_task_can_cancel_another_task_already_queued_in_its_batch() {
    let mut plugin = load_permitted(
        r#"
        local zellij = require("zellij")
        local M = { resumed = false }
        function M.load()
          local shared, resolve = zellij.async.future()
          M.resolve = resolve
          M.first = zellij.task(function()
            zellij.await(shared)
            M.second:cancel()
          end)
          M.second = zellij.task(function()
            zellij.await(shared)
            M.resumed = true
          end)
        end
        function M.update() M.resolve() end
        function M.render()
          return "resumed:" .. tostring(M.resumed) .. " running:" .. tostring(M.first:is_running())
            .. "," .. tostring(M.second:is_running())
        end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    plugin.update(&Event::InputReceived).unwrap();
    assert_eq!(logged(&mut plugin), "resumed:false running:false,false");
}

#[test]
fn failing_batches_keep_the_first_error_and_drain_later_tasks() {
    let mut plugin = load_permitted(
        r#"
        local zellij = require("zellij")
        local M = { log = {} }
        zellij.log = function(message) table.insert(M.log, message) end
        function M.load()
          local first, resolve_first = zellij.async.future()
          local second, resolve_second = zellij.async.future()
          local third, resolve_third = zellij.async.future()
          zellij.task(function()
            zellij.await(first)
            resolve_second()
            error("first broke", 0)
          end)
          zellij.task(function()
            zellij.await(second)
            resolve_third()
            error("second broke", 0)
          end)
          zellij.task(function()
            zellij.await(third)
            table.insert(M.log, "third ran")
          end)
          resolve_first()
        end
        function M.render() return table.concat(M.log, " | ") end
        return M
        "#,
    )
    .unwrap();
    let error = plugin.load(&BTreeMap::new()).unwrap_err();
    assert!(format!("{error:#}").contains("first broke"), "{error:#}");
    assert!(!format!("{error:#}").contains("second broke"), "{error:#}");
    assert_eq!(logged(&mut plugin), "error: second broke | third ran");
}

#[test]
fn a_cancelled_task_propagates_the_hosts_cancel() {
    let error = cancel_mid_update(
        r#"
        zellij.task(function()
          while true do end
        end)
        "#,
    );
    assert!(
        format!("{error:?}").contains(crate::plugins::plugin_map::CANCELLED_BY_HOST),
        "{error:?}"
    );
}

#[test]
fn a_task_asks_for_a_render_by_returning_truthy_or_calling_request_render() {
    let mut plugin = load_permitted(
        r#"
        local zellij = require("zellij")
        local M = {}
        zellij.run_command = function(args) table.insert(M.tokens, args.context.__async) end
        M.tokens = {}
        function M.load()
          zellij.task(function()
            zellij.await(zellij.async.run_command { command = { "x" } })
            zellij.request_render()
          end)
          zellij.task(function()
            zellij.await(zellij.async.run_command { command = { "y" } })
            return true
          end)
        end
        function M.update() return false end
        function M.render() return table.concat(M.tokens, " ") end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    let tokens = logged(&mut plugin);
    let first = tokens.split_whitespace().next().unwrap().to_owned();
    let second = tokens.split_whitespace().nth(1).unwrap().to_owned();
    assert!(plugin.update(&command_result(&first, 0, "")).unwrap());
    assert!(plugin.update(&command_result(&second, 0, "")).unwrap());
    assert!(!plugin.update(&command_result("stale", 0, "")).unwrap());
}

#[test]
fn await_outside_a_task_is_an_error() {
    let error =
        error_from_load(r#"local aw = zellij.async.future() zellij.await(aw)"#).expect("refused");
    assert!(
        error.contains("only be called from inside a zellij.task"),
        "got: {error}"
    );
    let error =
        error_from_load(r#"coroutine.wrap(function() zellij.await(zellij.async.future()) end)()"#)
            .expect("a plain coroutine is not a task");
    assert!(
        error.contains("only be called from inside a zellij.task"),
        "got: {error}"
    );
}

#[test]
fn async_constructors_subscribe_to_the_events_they_need() {
    let plugin = task_plugin(
        r#"
        zellij.async.run_command { command = { "x" } }
        zellij.async.sleep(1)
        zellij.async.agent_state(7, "idle")
        zellij.async.pane_exit(7)
        zellij.async.permissions { "run_commands" }
        "#,
    );
    let subscriptions = plugin.env().subscriptions.lock().unwrap().clone();
    for wanted in [
        zellij_utils::data::EventType::RunCommandResult,
        zellij_utils::data::EventType::Timer,
        zellij_utils::data::EventType::PaneUpdate,
        zellij_utils::data::EventType::CommandPaneExited,
        zellij_utils::data::EventType::PaneClosed,
        zellij_utils::data::EventType::PermissionRequestResult,
    ] {
        assert!(
            subscriptions.contains(&wanted),
            "{wanted:?} in {subscriptions:?}"
        );
    }
}

#[test]
fn web_request_takes_named_arguments_with_absent_stream_and_proxy() {
    // Seven tuple elements, two optional in the middle: `zellij.null` is what fills them.
    let source = r#"zellij.web_request { url = "http://x", verb = "Post", body = "hi" }"#;
    if let Some(err) = error_from_load(source) {
        assert!(
            !err.contains("could not read the arguments"),
            "the arguments were refused: {err}"
        );
    }
    // And the awaitable fills them itself.
    let mut plugin = task_plugin(
        r#"
        local reply = zellij.await(zellij.async.web_request { url = "http://x", body = "hi" })
        log("status:" .. reply.status .. " " .. reply.body)
        "#,
    );
    let token = async_token(&logged(&mut plugin), 0);
    plugin
        .update(&Event::WebRequestResult(
            200,
            BTreeMap::new(),
            b"ok".to_vec(),
            BTreeMap::from([("__async".to_owned(), token.clone())]),
        ))
        .unwrap();
    assert_eq!(logged(&mut plugin), format!("web:{token} status:200 ok"));
}

#[test]
fn permissions_awaitable_resolves_with_the_answer() {
    let mut plugin = task_plugin(
        r#"
        local answer = zellij.await(zellij.async.permissions { "run_commands" })
        log("answer:" .. answer)
        "#,
    );
    plugin
        .update(&Event::PermissionRequestResult(
            zellij_utils::data::PermissionStatus::Granted,
        ))
        .unwrap();
    assert_eq!(logged(&mut plugin), "answer:Granted");
}

#[test]
fn concurrent_permission_awaitables_are_dispatched_and_resolved_in_order() {
    for (first, second) in [
        (
            zellij_utils::data::PermissionStatus::Granted,
            zellij_utils::data::PermissionStatus::Denied,
        ),
        (
            zellij_utils::data::PermissionStatus::Denied,
            zellij_utils::data::PermissionStatus::Granted,
        ),
    ] {
        let mut plugin = task_plugin(
            r#"
            zellij.request_permission = function(permissions) log("ask:" .. permissions[1]) end
            zellij.task(function()
              local answer = zellij.await(zellij.async.permissions { "run_commands" })
              log("first:" .. answer)
            end)
            zellij.task(function()
              local answer = zellij.await(zellij.async.permissions { "read_files" })
              log("second:" .. answer)
            end)
            "#,
        );
        assert_eq!(logged(&mut plugin), "ask:run_commands");

        plugin
            .update(&Event::PermissionRequestResult(first))
            .unwrap();
        assert_eq!(
            logged(&mut plugin),
            format!("ask:run_commands ask:read_files first:{first:?}")
        );

        plugin
            .update(&Event::PermissionRequestResult(second))
            .unwrap();
        assert_eq!(
            logged(&mut plugin),
            format!("ask:run_commands ask:read_files first:{first:?} second:{second:?}")
        );
    }
}

#[test]
fn agent_send_writes_a_bracketed_paste_then_enter() {
    let mut plugin = load_permitted(
        r#"
        local zellij = require("zellij")
        local M = { log = {} }
        zellij.write_to_pane_id = function(text, pane)
          table.insert(M.log, (text:gsub("\27", "ESC")) .. "@" .. pane.Terminal)
        end
        function M.load()
          zellij.agent.send(7, "fix it")
          zellij.agent.send(zellij.terminal_pane(8), "draft", { enter = false })
        end
        function M.render() return table.concat(M.log, " ") end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    assert_eq!(
        logged(&mut plugin),
        "ESC[200~fix itESC[201~@7 \r@7 ESC[200~draftESC[201~@8"
    );
}

fn agent_event(pane_id: u32, hook: &str, state: Option<zellij_utils::data::AgentState>) -> Event {
    Event::AgentEvent(zellij_utils::agent::AgentEvent {
        pane_id,
        kind: "claude".to_owned(),
        hook: hook.to_owned(),
        state,
        last_message: Some(format!("{hook} said")),
        session_id: Some("s1".to_owned()),
        raw: "{}".to_owned(),
        ..Default::default()
    })
}

#[test]
fn a_failing_agent_listener_does_not_strand_later_listeners() {
    let mut plugin = load_permitted(
        r#"
        local zellij = require("zellij")
        local M = { log = {} }
        function M.load()
          zellij.agent.on(function() error("first listener broke") end)
          zellij.agent.on(function(event) table.insert(M.log, event.hook) end)
        end
        function M.render() return table.concat(M.log, " ") end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();

    let error = plugin
        .update(&agent_event(7, "Stop", None))
        .expect_err("the first listener's error is the entrypoint error");
    assert!(
        format!("{error:#}").contains("first listener broke"),
        "{error:#}"
    );
    assert_eq!(logged(&mut plugin), "Stop");
}

#[test]
fn a_prompt_sends_and_resolves_when_the_agents_turn_ends() {
    use zellij_utils::data::AgentState;
    let mut plugin = load_permitted(
        r#"
        local zellij = require("zellij")
        local M = { log = {} }
        zellij.write_to_pane_id = function(text) if text ~= "\r" then table.insert(M.log, "sent") end end
        function M.load()
          zellij.agent.on(function(event) table.insert(M.log, "heard:" .. event.hook) end)
          zellij.task(function()
            local reply = zellij.await(zellij.agent.prompt(7, "go"))
            table.insert(M.log, "reply:" .. reply.message .. "/" .. reply.hook)
          end)
        end
        function M.render() return table.concat(M.log, " ") end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    assert_eq!(logged(&mut plugin), "sent");
    // Working is not the turn ending; another pane's stop is not this pane's.
    plugin
        .update(&agent_event(7, "PreToolUse", Some(AgentState::Busy)))
        .unwrap();
    plugin
        .update(&agent_event(8, "Stop", Some(AgentState::Idle)))
        .unwrap();
    assert_eq!(logged(&mut plugin), "sent heard:PreToolUse heard:Stop");
    plugin
        .update(&agent_event(7, "Stop", Some(AgentState::Idle)))
        .unwrap();
    assert_eq!(
        logged(&mut plugin),
        "sent heard:PreToolUse heard:Stop reply:Stop said/Stop heard:Stop",
        "the task resumes before the listeners hear the event"
    );
}

#[test]
fn a_turn_ends_on_a_stop_and_not_on_a_session_start() {
    use zellij_utils::data::AgentState;
    let mut plugin = task_plugin(
        r#"
        for _ = 1, 2 do
          local reply = zellij.await(zellij.async.turn(7))
          log("reply:" .. tostring(reply.message) .. "/" .. reply.hook)
        end
        "#,
    );
    // `SessionStart` reads as idle on the record, but it begins a session rather than ending
    // a turn: a spawned agent fires it before it has even submitted its opening prompt.
    plugin
        .update(&agent_event(7, "SessionStart", Some(AgentState::Idle)))
        .unwrap();
    assert_eq!(
        logged(&mut plugin),
        "",
        "a session starting is not a turn ending"
    );
    plugin
        .update(&agent_event(7, "Stop", Some(AgentState::Idle)))
        .unwrap();
    assert_eq!(logged(&mut plugin), "reply:Stop said/Stop");
    plugin
        .update(&agent_event(7, "Interrupt", Some(AgentState::Idle)))
        .unwrap();
    assert_eq!(
        logged(&mut plugin),
        "reply:Stop said/Stop reply:Interrupt said/Interrupt"
    );
}

#[test]
fn a_turn_with_an_idle_fallback_resolves_on_the_records_edge_into_idle() {
    use zellij_utils::data::AgentState;
    let mut plugin = task_plugin(
        r#"
        local reply = zellij.await(zellij.async.turn(7, { fallback = "idle" }))
        log("reply:" .. tostring(reply.message) .. "/" .. reply.state)
        "#,
    );
    plugin
        .update(&manifest_with_agent(7, AgentState::Idle))
        .unwrap();
    assert_eq!(
        logged(&mut plugin),
        "",
        "idle before working is not a turn ending"
    );
    plugin
        .update(&manifest_with_agent(7, AgentState::Busy))
        .unwrap();
    plugin
        .update(&manifest_with_agent(7, AgentState::Idle))
        .unwrap();
    assert_eq!(logged(&mut plugin), "reply:nil/Idle");
}

#[test]
fn an_rpc_call_is_answered_by_the_handler_and_the_reply_resumes_the_caller() {
    let mut plugin = load_permitted(
        r#"
        local zellij = require("zellij")
        local M = { log = {} }
        zellij.message_to_plugin = function(args)
          table.insert(M.log, args.message_name .. ">" .. tostring(args.destination_plugin_id or args.plugin_url) .. ":" .. args.message_payload)
        end
        zellij.rpc.handle("double", function(payload) return payload.n * 2 end)
        zellij.rpc.handle("boom", function() error("no") end)
        function M.load()
          zellij.task(function()
            local reply = zellij.await(zellij.rpc.call { plugin_url = "file:other.lua", name = "double", payload = { n = 2 } })
            table.insert(M.log, "reply:" .. tostring(reply.result) .. "/" .. tostring(reply.error))
          end)
        end
        function M.pipe(message) table.insert(M.log, "pipe:" .. message.name) end
        function M.render() return table.concat(M.log, " | ") end
        return M
        "#,
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    let initial = logged(&mut plugin);
    let call_id = initial
        .split(r#""call_id":""#)
        .nth(1)
        .and_then(|value| value.split('"').next())
        .expect("the rpc call id was logged")
        .to_owned();
    let call = |name: &str, payload: &str| {
        zellij_utils::data::PipeMessage::new(
            zellij_utils::data::PipeSource::Plugin(3),
            name,
            &Some(payload.to_owned()),
            &None,
            false,
        )
    };
    // The callee's side: a call from plugin 3 is answered to plugin 3, and never shown to `pipe`.
    plugin
        .pipe(&call(
            "zellij_rpc_call",
            r#"{"version":1,"call_id":"c1","name":"double","payload":{"n":21}}"#,
        ))
        .unwrap();
    plugin
        .pipe(&call(
            "zellij_rpc_call",
            r#"{"version":1,"call_id":"c2","name":"boom","payload":{}}"#,
        ))
        .unwrap();
    // The caller's side: the reply to our own call resumes the task.
    plugin
        .pipe(&call(
            "zellij_rpc_reply",
            &format!(r#"{{"version":1,"call_id":"{call_id}","result":4}}"#),
        ))
        .unwrap();
    let log = logged(&mut plugin);
    let lines: Vec<&str> = log.split(" | ").collect();
    assert_eq!(
        lines[0],
        format!(
            r#"zellij_rpc_call>file:other.lua:{{"call_id":"{call_id}","name":"double","payload":{{"n":2}},"version":1}}"#
        )
    );
    assert!(
        lines[1].starts_with("zellij_rpc_reply>3:") && lines[1].contains(r#""result":42"#),
        "{}",
        lines[1]
    );
    assert!(
        lines[2].starts_with("zellij_rpc_reply>3:") && lines[2].contains("no"),
        "{}",
        lines[2]
    );
    assert_eq!(lines[3], "reply:4/nil");
    assert!(
        !log.contains("pipe:"),
        "rpc messages are the layer's own: {log}"
    );
}

#[test]
fn tracing_logs_commands_events_and_tasks() {
    let mut plugin = load_permitted(
        r#"
        local zellij = require("zellij")
        local M = { lines = {} }
        zellij.log = function(...) table.insert(M.lines, table.concat({ ... }, " ")) end
        zellij.run_command = function(args) zellij.log("token:" .. args.context.__async) end
        function M.load()
          zellij.task(function()
            zellij.await(zellij.async.run_command { command = { "x" } })
          end)
        end
        function M.render() return table.concat(M.lines, "\n") end
        return M
        "#,
    )
    .unwrap();
    plugin
        .load(&BTreeMap::from([("trace".to_owned(), "true".to_owned())]))
        .unwrap();
    let token = async_token(&logged(&mut plugin), 0);
    plugin.update(&command_result(&token, 0, "")).unwrap();
    let log = logged(&mut plugin);
    for expected in [
        "~ task#1 started",
        "-> subscribe [\"RunCommandResult\"]",
        "<- run_command_result",
        "~ task#1 finished",
    ] {
        assert!(log.contains(expected), "no {expected:?} in:\n{log}");
    }
}

#[test]
fn post_message_to_is_refused_by_name() {
    let error =
        error_from_load(r#"zellij.post_message_to { name = "w", payload = "" }"#).expect("refused");
    assert!(error.contains("not available from Lua"), "got: {error}");
}

/// `zellij.hocus.tools` is the whole of an extension's wiring: the registration hocus reads,
/// the re-registration when hocus asks who is running, and each call answered over the rpc
/// wire - with the handler's return, or its error.
#[test]
fn hocus_tools_registers_once_launched_and_answers_calls_over_rpc() {
    let source = r#"
        local zellij = require("zellij")
        local M = { log = {} }
        zellij.message_to_plugin = function(args)
          table.insert(M.log, args.message_name .. ">" .. tostring(args.destination_plugin_id) .. ":" .. args.message_payload)
        end
        function M.load()
          zellij.hocus.tools {
            forecast = {
              description = "A forecast.",
              parameters = { type = "object", properties = { city = { type = "string" } } },
              safety = "read_only",
              handler = function(args) return "clear over " .. args.city end,
            },
            boom = { description = "Raises.", handler = function() error("no forecast") end },
            shape = { description = "A table.", handler = function() return { ok = true } end },
          }
        end
        function M.pipe(message) table.insert(M.log, "pipe:" .. message.name) end
        function M.render() return table.concat(M.log, " | ") end
        return M
    "#;
    let mut plugin = load_permitted(source).unwrap();
    plugin
        .load(&BTreeMap::from([
            ("hocus_extension_name".to_owned(), "weather".to_owned()),
            (
                "hocus_extension_path".to_owned(),
                "/plugins/hocus/weather.lua".to_owned(),
            ),
        ]))
        .unwrap();
    let pipe = |name: &str, payload: &str| {
        zellij_utils::data::PipeMessage::new(
            zellij_utils::data::PipeSource::Plugin(3),
            name,
            &Some(payload.to_owned()),
            &None,
            true,
        )
    };
    plugin.pipe(&pipe("hocus_extension_discover", "")).unwrap();
    plugin
        .pipe(&pipe(
            "zellij_rpc_call",
            r#"{"version":1,"call_id":"c1","name":"hocus_tool:forecast","payload":{"city":"Oslo"}}"#,
        ))
        .unwrap();
    plugin
        .pipe(&pipe(
            "zellij_rpc_call",
            r#"{"version":1,"call_id":"c2","name":"hocus_tool:boom","payload":{}}"#,
        ))
        .unwrap();
    plugin
        .pipe(&pipe(
            "zellij_rpc_call",
            r#"{"version":1,"call_id":"c3","name":"hocus_tool:shape","payload":{}}"#,
        ))
        .unwrap();
    let log = logged(&mut plugin);
    let lines: Vec<&str> = log.split(" | ").collect();
    // Registered from `load`, and again on discover; the tools sorted by name.
    let registration = r#"hocus_extension_register>nil:{"namespace":"weather","path":"/plugins/hocus/weather.lua","tools":[{"description":"Raises.","name":"boom","parameters":{"type":"object"},"safety":"read_only"},{"description":"A forecast.","name":"forecast","parameters":{"properties":{"city":{"type":"string"}},"type":"object"},"safety":"read_only"},{"description":"A table.","name":"shape","parameters":{"type":"object"},"safety":"read_only"}],"version":1}"#;
    assert_eq!(lines[0], registration);
    assert_eq!(lines[1], registration, "discover re-registers");
    assert_eq!(
        lines[2],
        r#"zellij_rpc_reply>3:{"call_id":"c1","result":"clear over Oslo","version":1}"#
    );
    assert!(
        lines[3].starts_with(r#"zellij_rpc_reply>3:{"call_id":"c2","error":"#)
            && lines[3].contains("no forecast"),
        "{}",
        lines[3]
    );
    assert_eq!(
        lines[4],
        r#"zellij_rpc_reply>3:{"call_id":"c3","result":"{\"ok\":true}","version":1}"#
    );
    assert!(
        !log.contains("pipe:"),
        "the wiring never reaches the plugin's pipe: {log}"
    );

    // Not launched by hocus: nothing to register to, said once in the log rather than sent.
    let mut unlaunched = load_permitted(source).unwrap();
    unlaunched.load(&BTreeMap::new()).unwrap();
    assert_eq!(logged(&mut unlaunched), "");
}
