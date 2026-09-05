# Lua plugins

Zellij runs two kinds of plugin. The original kind is a wasm module, which is what every builtin
plugin is. The second kind is a Lua script, which exists because the wasm route asks for a Rust
crate, a `wasm32-wasip1` toolchain and a compile step before you can change what your status bar
says. A Lua plugin is a file you drop on disk.

Both kinds get the same events and the same host commands. Nothing about a plugin's capabilities
depends on which language it is written in.

## A plugin

```lua
local zellij = require("zellij")

local M = {}

M.defaults = { greeting = "hello" }

function M.load(config)
  M.greeting = config.greeting
  zellij.subscribe { "key" }
end

function M.update(event)
  if event.name == "key" then
    M.presses = (M.presses or 0) + 1
    return true          -- re-render
  end
end

function M.render(rows, cols)
  return ("%s - %d keys"):format(M.greeting, M.presses or 0)
end

return M
```

Drop it in your plugin folder - `~/.config/zellij/plugins/` - and load it by name:

```sh
zellij plugins write keys --file keys.lua   # compiles it first; --overwrite to replace
                                            # (an overwrite keeps the old one as keys.bak.lua)
zellij plugins check keys                   # the compile step on its own
zellij plugins reload keys                  # in the running session, in a pane if it is not up
zellij plugins log keys                     # what it logged
zellij plugins dir                          # where these land
```

is the same loop from a shell - the one a coding agent in a pane has - and hocus's `plugin_*`
tools (below) are it from inside the session. Either way the file goes in the folder, and

```kdl
pane {
    plugin location="file:keys.lua" {
        greeting "hi"
    }
}
```

or directly: `zellij plugin -- file:keys.lua`.

A path that is not there is looked for in that folder, so the file name on its own is enough.
Keep the `.lua`: the extension is how the runtime is chosen, so `file:keys` is read as wasm and
never gets as far as the folder. A path that *is* there always wins, so `file:./keys.lua` and
`file:/opt/plugins/keys.lua` mean exactly what they say and nothing in the folder can shadow
them. Set `user_plugin_dir` in the config to keep plugins somewhere else:

```kdl
user_plugin_dir "/path/to/my/plugins"
```

The folder is made when something first writes a plugin into it, so it is not there until you
put something in it. `zellij setup --check` prints the path it resolved either way.

A bare `keys` with no `file:` is *not* looked for there: an unprefixed name is a plugin alias,
and letting a file shadow one would make a single spelling mean two things.

Hocus writes plugins into that folder itself, and that is the whole authoring loop: `plugin_write
name=keys` compiles the complete file and puts it there - a source that does not compile is
refused with the error, before anyone is asked to approve it, and approving the write grants
whatever permissions the source asks for - then `plugin_reload name=keys` loads it and answers
with its first render or the load error, and `plugin_log name=keys` has anything it logged.
`plugin_check` is the compile step on its own, for trying a fix before writing, and both it and
the write warn about the traps a compile cannot see - a permission spelled wrong, a namespace
that is not shared, a column that cannot be declared. `plugin_docs` is this guide -
`plugin_docs query=<regex>` finds where something is spelled without fetching a section, and
`section=` with `start_line`/`line_count` reads part of one. `plugin_list` is the folder as it
stands, with each plugin's length; `plugin_outline name=keys` is one plugin's functions, handlers,
requires and permissions with their line numbers; `plugin_search query=<regex>` is a
ripgrep-shaped search over the folder or one plugin; `plugin_read name=keys start_line=N
line_count=M` reads a range.

Once a plugin works, change it with `plugin_edit name=keys old_string=... new_string=...` rather
than sending the whole file again - or several replacements at once as `edits=[{old_string,
new_string}, ...]`, applied in order and written only if every one applies. The answer names the
lines that changed, and a miss says which lines nearly matched. An exact replacement cannot drop
the parts of the file it does not mention, and a rewrite from memory routinely does. Either way
the version replaced is kept as `<name>.bak.lua` - `plugin_read name=keys.bak` - so a rewrite
that lost something is recoverable for exactly one round.

The compile is a host command any plugin can call: `zellij.check_lua_plugin { name = "keys.lua",
source = src }` answers `nil` when the source compiles and the error text otherwise. It only
compiles - a plugin that parses and then fails in `load` is a load error, seen on reload.

The runtime is chosen by the file extension, so there is no new URL scheme: a `.lua` path is a Lua
plugin and anything else is wasm.

## The contract

Your file **returns a table**. Zellij calls these fields, all optional:

| Field | Called | Notes |
|---|---|---|
| `load(config)` | once, at startup | `config` is the layout's configuration, merged over `M.defaults` if you set one |
| `update(event)` | per subscribed event | return a truthy value to request a re-render |
| `pipe(message)` | per pipe message | same |
| `render(rows, cols)` | when a re-render was requested | **return the string to display** |
| `defaults` | - | a table of default configuration values |

`render` returns its output rather than printing it. `rows` and `cols` are the pane's content
size. Every render replaces the whole pane, so return the full frame, not a delta. ANSI escapes
work.

## Events

Subscribe by name, in either spelling - `"key"` or `"Key"`:

```lua
zellij.subscribe { "key", "tab_update", "pane_update" }
```

Events arrive as a table with a snake_case `name`. `event.value` is always the whole payload,
whatever its shape, and an event whose payload is a set of named fields also exposes them
directly:

```lua
function M.update(event)
  if event.name == "key" then
    -- key events carry a readable `key`: "t", "Esc", "F2"
    if event.key == "Esc" then ... end
  elseif event.name == "tab_update" then
    for _, tab in ipairs(event.value) do ... end   -- a list
  elseif event.name == "timer" then
    local elapsed = event.value                    -- a number
  elseif event.name == "run_command_result" then
    -- positional payloads are named too, and bytes arrive as strings
    if event.exit_code == 0 then M.output = event.stdout end
  elseif event.name == "mouse" then
    -- event.kind is "left_click", "scroll_up", ...; clicks have event.line/event.col
  end
end
```

The named fields of the positional events: `run_command_result` has `exit_code`, `stdout`,
`stderr`, `context`; `web_request_result` has `status`, `headers`, `body`, `context`;
`web_request_stream_chunk` has `stream_id`, `chunk`, `done`, `error`; `command_pane_exited` and
`edit_pane_exited` have `pane_id`, `exit_code`, `context`; `command_pane_opened`,
`edit_pane_opened` and `command_pane_re_run` have `pane_id`, `context`; `session_update` has
`sessions`, `resurrectable_sessions`; `custom_message` has `message`, `payload`; the
`file_system_*` events have `files`, a list of `{ path, metadata }`; and `pane_render_report`
has `panes`, keyed `"terminal:3"` / `"plugin:3"` - `zellij.pane_key(pane_id)` spells a pane id
that way. `zellij.bytes_to_string` is the byte conversion, for anything else that arrives as a
list of numbers.

`zellij.key_name(bare_key)` is the same conversion, if you need it elsewhere.

`agent_event` is one event per hook a coding agent fired in a pane, read once by the session:
`pane_id` (a number), `kind`, `hook` (`UserPromptSubmit`, `PreToolUse`, `Stop`, …), `state`
(`Busy`/`Waiting`/`Idle` as the event moved the pane's record, or nil), `detail`, `prompt`,
`last_message` (what the agent said, on a `Stop`), `tool_name`, `cwd`, `session_id`, and `raw`,
the agent's own JSON. An event over 64 KiB is pruned - prose to 8 K characters - and `prompt` and
`last_message` are cut the same way `raw` is. It needs `read_application_state`, and `zellij events --type agent_event`
streams the same thing to a shell. See *Agents*.

Instead of one big `if`, you can register handlers:

```lua
zellij.on("key", function(event)
  ...
  return true    -- or zellij.request_render()
end)
```

Handlers run before `update`, and either mechanism can request a render.

## Commands

Every host command a wasm plugin can call is reachable as `zellij.<command>`, in snake_case:

```lua
zellij.new_tab { name = "logs" }
zellij.open_terminal("/tmp")
zellij.focus_next_pane()
local ids = zellij.get_plugin_ids()
```

There is no list to keep up with here, and no wrapper to wait for: a command zellij has is a
command a plugin can call. `zellij.command(name, args)` does the same thing with the name as a
string, for a name computed at runtime.

Arguments follow the shape of the command, and there are four:

| The command takes | You write |
|---|---|
| nothing | `zellij.close_self()` |
| one value | `zellij.close_tab_with_index(0)` |
| several values | `zellij.rename_tab(0, "logs")` |
| named fields | `zellij.new_tab { name = "logs", cwd = "/tmp" }` |

The positional form always works. A command taking several values *also* accepts them by name
where that reads better - `zellij.rename_tab { tab_index = 0, name = "logs" }` - for the commands
the prelude names in its `tuple_fields` table, which is the ones whose arguments are hard to tell
apart by position. Any other tuple command takes the positional form only, and a table of names it
does not know is read as the wrong shape. Fields you leave out are absent, not empty: a command
with no required arguments can be called with none.

A tuple command's arguments are counted, though, so an argument that is optional in Rust is
*nullable* rather than absent: all of them have to be there.

### Default keybindings

A Lua plugin can register fallback keybindings at startup. User configuration always wins; a
default becomes active only while its mode/key is otherwise unbound. Registrations are owned by
the plugin, replaced by its next call, and removed when it unloads. If plugins collide, the lower
plugin id wins until it unloads. Permission requests are asynchronous, so registration must wait
for the grant:

```lua
function M.load()
  zellij.subscribe { "permission_request_result" }
  zellij.request_permission { "reconfigure" }
end

zellij.on("permission_request_result", function(event)
  if event.value ~= "Granted" then return false end
  zellij.register_default_keybindings {
    {
      mode = "normal",
      key = "Ctrl g",
      handler = function(message)
        M.open = not M.open
        return true -- re-render
      end,
    },
    {
      mode = "normal",
      key = "Ctrl n",
      actions = { zellij.action("NewPane") },
    },
  }
  return false
end)
```

Each entry has exactly one of `handler` or `actions`. Handlers receive the keybinding pipe message
and can call any normal plugin command. Actions are ordinary Zellij keybinding actions; positional,
property, and child arguments use the same `zellij.action` forms documented for Lua config.

`zellij.after(seconds, fn)` calls `fn` once; `zellij.every(seconds, fn)` calls it until the
handle it returns is `:cancel()`ed. A truthy return asks for a render. Any number of either may
be outstanding: the prelude keeps one host timer in flight for the nearest deadline. Seconds
are whole - the sandbox has no finer clock - so `every(0.5, …)` ticks once a second.
`zellij.async.sleep(seconds)` is the same clock inside a task (see *Tasks*).

A few things zellij spells as Rust types have a helper, so you do not have to know that:

```lua
zellij.terminal_pane(3)          -- a PaneId; zellij.plugin_pane(3) for the other kind
zellij.write("echo hi\r")        -- bytes, from a string
zellij.write_to_pane_id("echo hi\r", zellij.terminal_pane(3))
```

### Commands that answer

A command that asks zellij something returns it:

```lua
local ids = zellij.get_plugin_ids()          -- a table
local version = zellij.get_zellij_version()  -- a string
local layout = zellij.dump_layout("default") -- a string of KDL
local pane_id = zellij.open_command_pane { { path = "sh", args = { "-c", "date" } }, {} }
```

The pane-opening commands answer with the id of the pane they made, which is how one command
feeds the next. A query zellij cannot answer raises, so wrap it in `pcall` if that is a case you
want to handle rather than a bug.

Not every reply is decoded yet - see *Current limits*. And a few commands do not answer at all
but send an **event** instead: `zellij.list_clients()` returns nothing, and the answer arrives as
a `list_clients` event, so subscribe to it.

### Pane workflows

A multi-step job - list a remote folder, read a file, copy it - used to mean one pane per step.
`zellij.pane_workflow` runs the whole sequence in **one pane**: when a step's command exits, the
next replaces it in the same pane, and each finished step's output scrolls into the pane's
history so the transcript accumulates.

```lua
local wf = zellij.pane_workflow {
  name = "sync-skills",          -- pane title
  floating = true,               -- or a FloatingPaneCoordinates table; default tiled
  steps = {
    { path = "ssh", args = { "host", "ls ~/.hocus/skills" }, timeout = 30 },
    { path = "scp", args = { "host:~/.hocus/skills/x", "/tmp/x" } },
  },
  on_step = function(n, step, exit_code) end,
  on_done = function(result) end,
  -- result = { status = "success"|"failed"|"cancelled"|"pane_closed",
  --            exit_codes = { 0, false, ... }, pane_id = 7, failed_step = 2|nil }
}
wf.pane_id      -- the terminal pane id, known immediately
wf:push(step)   -- append a step while running (works from inside on_step)
wf:cancel()     -- SIGINT, then SIGKILL; on_done says "cancelled"
```

A step is `{ path, args, cwd, remote_host, remote_default_shell, timeout, stdin, name }` - the
`CommandToRun` fields plus three of the workflow's own. A step's result is its child's real exit
status; `stop_on_failure` (default true) ends the workflow on the first non-zero exit. `timeout =
seconds` interrupts a stuck step; leave it off for interactive ones - every step is a real pty,
so the user can focus the pane and type. `stdin = "..."` is written to the step's child once it
starts, which is where a secret goes: **never put a secret in `args`** - argv is visible in the
pane manifest and in serialized sessions. `cwd = "inherit"` uses the pane's last reported
working directory only when the next step runs on the same host. A step's `cwd` is read on the
machine the step runs on: locally `/host`, `/data` and `/cache` name the plugin mounts and a
missing `cwd` means the plugin's own; with `remote_host` or `firecracker_vm` set it is passed
through as written, and a missing one lands the step wherever that machine logs the user in,
never in the plugin's cwd, which is a path here. Other options:
`close_on_success`, `on_cancel = "close"`, `keep_scrollback =
false` to wipe between steps, `kill_grace` (seconds between SIGINT and SIGKILL, default 5).

It needs `run_commands` to start a step and `read_application_state` to hear that one finished:
`command_pane_exited` is gated on the latter, so a workflow without it runs its first step and
then waits forever. `change_application_state` too if any step has a `timeout`, `cancel` is used,
or `close_on_success` / `on_cancel = "close"` is set, and `write_to_stdin` for `stdin`.
Between steps the pane is briefly held, so a user pressing ENTER on it re-runs the finished
step; the workflow's next step then queues behind that re-run. The workflow is plugin state: it
does not survive a plugin reload or session resurrection.

Underneath it is one command, usable on its own: `zellij.run_command_in_pane { pane_id, command,
context, keep_scrollback }` runs a *different* command in an existing (held) command pane,
keeping the pane's id. The `context` round-trips on the resulting `command_pane_exited`, and a
replacement sent while the pane's command is still running starts when it exits (last write
wins). `example-plugins/workflow.lua` (`section=example_workflow`) is the worked example.

### Permissions

Privileged commands need permission, which the user is prompted for once:

```lua
function M.load()
  zellij.request_permission { "change_application_state", "open_terminals_or_plugins" }
end
```

A plugin written through hocus is granted whatever its source asks for here when you approve
the write - the confirmation names them - so it never prompts in its own pane.

Calling one you have not been granted is an error naming the permission you need - the same rule
wasm plugins live under, except that a wasm plugin cannot be told about it and you can:

```lua
local ok, err = pcall(function() zellij.copy_to_clipboard("hi") end)
-- err: 'copy_to_clipboard' needs the 'WriteToClipboard' permission, ...
```

**A permission is not in force while `load` runs.** Every plugin starts with its permissions
unanswered - even one granted long ago, and even one hocus pre-granted when you approved the
write - because the grant arrives as a `permission_request_result` event, and `load` runs before
any event does. So `load` is where you *ask*; the handler is where you do the work:

```lua
function M.load()
  zellij.request_permission { "plugin_data" }      -- ask here
end

zellij.on("permission_request_result", function()
  zellij.data.define { ... }                       -- act here, on every run
end)
```

The handler fires on every load, not only the first: an already-granted permission is answered
immediately rather than prompting. A gated call made from `load` is refused, and the refusal
says so.

These are the names, as a plugin spells them:

| Permission | What it covers |
|---|---|
| `read_application_state` | the session's panes, tabs and modes - most `*_update` events |
| `change_application_state` | moving, closing, renaming and focusing panes and tabs |
| `open_files` | opening a file in the user's editor |
| `run_commands` | `zellij.run_command`, commands in panes, and `zellij.agent.spawn` |
| `open_terminals_or_plugins` | new terminal panes, and starting or reloading plugins |
| `write_to_stdin` | typing into another pane |
| `web_access` | `zellij.web_request` |
| `read_cli_pipes` | pipes from the command line |
| `message_and_launch_other_plugins` | `zellij.message_to_plugin` - what a hocus extension registers over |
| `reconfigure` | changing the session's configuration and keybindings |
| `full_hd_access` | files outside `/host`, `/data`, `/cache`, `/tmp` and `/plugins` |
| `intercept_input` | taking keys and mouse before the rest of the session |
| `read_pane_contents` | another pane's viewport and selection |
| `run_actions_as_user` | `zellij.run_action`, which is anything a keybinding can do |
| `write_to_clipboard` | the system clipboard |
| `read_session_environment_variables` | the environment the session started in |
| `plugin_data` | the plugin data store, in the plugin's own namespace |
| `shared_plugin_data` | a `shared:` namespace, on top of `plugin_data` |
| `read_files` | reading in the plugin's own folders |
| `write_files` | writing in them |
| `author_plugins` | writing into the plugin folder and reloading plugins by url |

A name that is not on this list grants **nothing** and raises nothing: hocus reads the list out
of your source lexically to pre-grant it, and a name it does not recognise is simply not there,
so the plugin is denied at run time in a pane nobody is watching. `plugin_check` and `zellij
plugins check` warn about that, and about the other traps a compile cannot see.

The worked examples in `example-plugins/` are reference sources to read rather than a place to
keep a plugin of your own - hocus serves each as a `plugin_docs` section. `clock.lua`
(`section=example_clock`) is the small one, `capabilities.lua` (`section=example_capabilities`)
puts most of what a plugin can do in one pane, `playground.lua` (`section=example_playground`) is
a keymap where each key calls a different shape of command and shows what came back,
`dashboard.lua` (`section=example_dashboard`) is the one to read for how a plugin draws,
`agent-harness.lua` (`section=example_agent_harness`) is the one to read for running agents: it
starts one, hands it a job, runs the check and hands failures back until green,
`agent-commit.lua` (`section=example_agent_commit`) is an `agent_exit` hook, `workflow.lua`
(`section=example_workflow`) runs a multi-step command sequence in one pane, and
`todo-extension.lua` (`section=example_todo_extension`) with `todo-pane.lua`
(`section=example_todo_pane`) are the pair to read for shared data: an extension that owns a
`shared:` namespace and gives hocus tools over it, and a pane that watches the same rows.

A plugin of your own goes in the plugin folder, written there with `plugin_write`. To run one of
these from a checkout instead:

```sh
zellij plugin --floating -- file:$PWD/example-plugins/clock.lua
```

## Tasks

A plugin is driven by events, and everything the host does for it *answers* as a later event: a
command's output, a pane's exit, an agent going idle, a timer. Written as `if event.name == …`
arms across `update`, a five-step job is a state machine, and the wiring is where it goes
wrong. A **task** is that job as straight-line code:

```lua
function M.load(config)
  zellij.task(function()
    zellij.await(zellij.async.permissions { "run_commands", "read_application_state" })
    local result = zellij.await(zellij.async.run_command { command = { "cargo", "test" } })
    if result.exit_code ~= 0 then
      zellij.notify { message = "red: " .. result.stderr }
    end
    return true                          -- a truthy return asks for a render
  end)
end
```

`zellij.task(fn)` runs `fn` as a coroutine, synchronously up to its first `zellij.await`, and
resumes it when what it awaited arrives - in a later entrypoint, with every host command
available, so nothing a task does is different from what `update` could do. It returns a handle
with `:cancel()` and `:is_running()`.

An **awaitable** is a thing that will settle. Each is eager - the command is dispatched when
the awaitable is made, and its reply is kept even while the task waits on something else - and
each subscribes to the events it needs, so you do not:

| `zellij.async.…` | resolves with | needs |
|---|---|---|
| `run_command { command, env_variables, cwd, context, max_bytes_returned }` | the `run_command_result` event (`exit_code`, `stdout`, `stderr`) | `run_commands` |
| `web_request { url, verb, headers, body, context }` | the `web_request_result` event (`status`, `headers`, `body`) | `web_access` |
| `pane_exit(pane_id)` | the `command_pane_exited` event, or `{ closed = true }` - for a pane this plugin opened | `read_application_state` |
| `run_in_pane(pane_id, step)` | as `pane_exit`, for the command `step` runs in an existing pane | `run_commands`, `read_application_state` |
| `agent_state(pane_id, "idle" \| { … }, { after = { "busy" } })` | the pane's `PaneInfo` when its agent record reaches one of the states - after one of `after`, when given | `read_application_state` |
| `agent_event(pane_id, "Stop")`, `turn(pane_id)` | see *Agents* | `read_application_state` |
| `pipe(name)` | the next pipe message named `name` | - |
| `permissions { … }` | `"Granted"` or `"Denied"`; concurrent requests are dispatched one at a time, FIFO | - |
| `sleep(seconds)` | `true`, after whole `seconds` | - |
| `event(name, match)` | the next `name` event for which `match(event)` is truthy | whatever the event needs |
| `all { a = …, b = … }` | every value, in the same shape | - |
| `race { a = …, b = … }` | `{ key, value }` of the first; the others settle unheard later, so an awaitable can be raced again | - |
| `future()` | whatever `resolve(value)` is called with - the primitive: `local aw, resolve = zellij.async.future()` | - |

Three rules. **Tasks run first and consume nothing**: an event that settles an awaitable is
handed to the task before the `zellij.on` handlers and `update` see it, and they still see it.
**A task's error is the entrypoint's error**: it is reported against the plugin like any other,
never dropped - and the other tasks the same event resumed still run first. **A reload ends every task**: the plugin starts over, and a reply still in
flight arrives with a token nobody holds. `zellij.await` outside a task is an error - an
entrypoint has no coroutine to suspend - so a task is where anything that waits is written, and
`load` is the usual place to start one.

`zellij.async.permissions` is how `load` is written now: ask, await, act. Its requests are
serialized in FIFO order because permission results carry no request id. On an uncached first
load, await the answer before any gated call. A permission the user granted before - or one hocus
granted when it approved the source - takes effect synchronously when requested, so a gated call
made straight after `zellij.request_permission` in `load` works on later, cached loads; the result
event still arrives asynchronously.

## Agents

A plugin can run coding agents in panes and drive them to an outcome: spawn one, hear what it
says, hand it the next thing, and read its state off the pane. `example-plugins/agent-harness.lua`
is all of this in ninety lines.

### Spawning

```lua
local pane = zellij.agent.spawn {
  kind = "claude",                       -- or codex, cursor; `command = "sh"` for anything else
  prompt = "make the tests pass",        -- the opening prompt, last on the command line
  branch = "fix-42", base = "main",      -- a worktree of its own at .worktrees/fix-42 (default)
  permission_mode = "acceptEdits",       -- in the agent's own terms; "skip" turns prompts off
  model = "opus", resume = "last",       -- `--model`, and `--continue` / `--resume <id>`
  hooks = true,                          -- a claude reports its state through inline hooks
  cwd = "/host", name = "fixer",         -- where it runs with no branch; the pane's title
}
```

The argv, the permission-mode flags, the resume spelling and the inline hooks are decided by the
same planner hocus's `agent_spawn` and the New agent modal use, so `agent = "cursor"` runs
`cursor-agent` everywhere. `branch` cuts the branch (worktree by default, `worktree = false` for
this checkout); an existing branch or worktree is reused. `source_directory` is the repository
that happens in, as a plugin path, `/host` by default. `firecracker` names a session-scoped VM to
run in (`firecracker_image` its image); with `automatic_firecracker_vm = true` the host names the
VM after the folder the pane runs in and mirrors that folder into it instead, so agents on one
checkout share one VM. The pane is an agent pane from its first
moment: its record is on `pane_update`, its exit fires `agent_exit`. Answers with the pane id
(`{ Terminal = n }`), raises on failure. Needs `run_commands`.

The pane opens beside the plugin's own. A plugin that has no pane - one loaded headless while no
client was attached, so nothing of it is on screen yet - opens it in the tab the session is
looking at instead, rather than losing the agent. The same holds for every other `*_near_plugin`
command.

`zellij.open_command_pane_in_git_checkout_near_plugin` is the lower-level command it is built
on - a `CommandToRun` plus a `GitCheckout` (`"Current"`, `{ NewBranch = { branch, base } }`,
`{ NewWorktree = { branch, base, path } }`) - for a command that is not an agent.

### What an agent is doing

Every `PaneInfo` in a `pane_update` has an `agent` field: nil, or `{ kind, state, source, detail,
cwd, session_id, run_id }` with `state` one of `Busy`, `Waiting`, `Idle`, `Exited`. The spawn
path writes it, and so does the agent itself through its hooks (`docs/AGENT_HOOKS.md`).
`zellij.agent.info(pane_id)` is the last record seen for a pane; `zellij.agent.all()` is every
one, by pane id. Both are nil until a `pane_update` has arrived - the first call subscribes if
the plugin has not - and a pane that closed drops out of them.

The record is a few fields; the *words* are on `agent_event` - what the user asked
(`prompt`, on `UserPromptSubmit`) and what the agent said (`last_message`, on `Stop`):

```lua
zellij.agent.on(function(e)          -- every agent_event, typed
  if e.hook == "Stop" then zellij.log(e.pane_id .. " said: " .. (e.last_message or "")) end
end)
```

A plugin speaking *for* an agent writes the record: `zellij.set_pane_agent { pane_id, agent = {
kind, state, source = "Reported", detail } }`, and `zellij.set_pane_agent_run { pane_id, run_id }`
sets the one field the agent never reports. Both need `change_application_state`.

### Steering

```lua
zellij.agent.send(pane, "run the tests")             -- a bracketed paste, then Enter
local reply = zellij.await(zellij.agent.prompt(pane, "now fix what failed"))
-- reply.message is what it said when its turn ended; reply.hook is Stop, StopFailure or Interrupt
```

`zellij.async.turn(pane_id)` resolves on the pane's next `Stop`, `StopFailure` or `Interrupt`
(`SessionStart` is idle, but it is not a turn ending); `zellij.agent.prompt` is `send` then
`turn`, the turn registered first so a fast answer is not missed. For an agent with no hooks to speak through, `{ fallback = "idle" }` also resolves on
the record's edge from `busy` or `waiting` into `idle`, with `message = nil`.
`zellij.async.agent_state(pane, "waiting")` waits for a state; `zellij.async.agent_event(pane,
"PermissionRequest")` for a hook. `send` needs `write_to_stdin`; the rest need
`read_application_state`.

### Between agents

Anything a plugin can do is a step in the loop: `zellij.async.run_command` for the check,
`zellij.async.run_in_pane` for a step the user should see, `zellij.data` for the record of what
happened, `zellij.notify` for the person. hocus's own run history (`agent_run`,
`agent_run_entry` in `shared:hocus`, see *Hocus data*) is readable too, for what an agent
concluded in a session that has since closed.

## Plugin-to-plugin calls

`zellij.message_to_plugin` is fire and forget. `zellij.rpc` is a call with an answer:

```lua
-- in the callee
zellij.rpc.handle("forecast", function(payload) return "clear over " .. payload.city end)

-- in the caller, inside a task
local reply = zellij.await(zellij.rpc.call {
  plugin_url = "file:weather.lua",          -- or plugin_id = n; launched if it is not running
  name = "forecast", payload = { city = "Oslo" }, timeout = 10,
})
-- reply.result, or reply.error (a handler that raised, no handler, or the timeout)
```

The wire is two plugin messages, `zellij_rpc_call` and `zellij_rpc_reply`, correlated by a
call id and answered to the plugin id the call came from; the prelude handles both, and neither
reaches a plugin's own `pipe`. Both sides need `message_and_launch_other_plugins`. It is what
hocus calls an extension's tools over (see *Hocus extensions*).

## Rendering

`render` returns the frame as a string, and ANSI escapes work in it - but a plugin that wants to
look like part of zellij uses the same components the builtin plugins do, drawn by zellij in the
user's theme:

```lua
local ui = zellij.ui

function M.render(rows, cols)
  local table = ui.table { { "pane", "command" } }        -- the first row is the title row
  for _, pane in ipairs(M.panes) do
    table:row(pane.id, ui.text(pane.command):dim())
  end
  return ui.text("panes"):selected():render()
    .. "\n" .. table:render()
    .. "\n" .. ui.nested_list { "one", { "two", indent = 1 } }:render()
end
```

The four components are `ui.text`, `ui.ribbon` (a tab-shaped label), `ui.table` (rows of cells,
each a string or a text) and `ui.nested_list` (items, each optionally `{ text, indent = n }`).
A text is styled by chaining: `:selected()`, `:opaque()`, `:disabled()`, `:emphasis(level, from,
to)` for the theme's four emphasis colours (0-3, positions counted in characters and 0-based),
and `:dim()`, `:unbold()`, `:error()`, `:success()`, each taking the same optional range.

`render(coords)` gives the string to put in the frame. Without an argument the component is
drawn where the cursor is, so components join with `"\n"` like any other lines; with
`{ x = 0, y = 2, width = 40, height = 5 }` it is drawn there, and clipped to the width and
height if they are given.

Text is measured in columns, which is neither bytes nor code points: `zellij.text.width(s)` is
the answer, and `zellij.text.pad(s, width)`, `pad_left`, `center` and `truncate(s, width,
ellipsis)` are built on it, so a box a plugin draws itself lines up whatever is in it.

## Modules

A plugin can be more than one file. `require("name")` loads `name.lua` from the plugin's own
folder, or `name/init.lua`, and `require("app.ui")` is `app/ui.lua`:

```lua
-- ~/.config/zellij/plugins/app.lua
local ui = require("app.ui")          -- ~/.config/zellij/plugins/app/ui.lua
```

A module is loaded once and cached, as Lua's `require` does, and a module that returns nothing is
remembered as `true`. The lookup is confined to the plugin's folder after symlinks are followed,
so a link out of it is refused rather than followed, and a name that climbs (`..`) or starts at
the root never reaches the disk. That is the whole of why this exists instead of `package`:
the module system is host-mediated, so it can be confined the way `zellij.fs` is.

## Data

Lua state is gone on reload, and `zellij.fs` into `/cache` is a file. For anything with rows in
it there is the plugin data store: a persistent store zellij keeps for plugins, with entities
you declare, writes in transactions, fetches with filters and full-text search, and watches
that send an event whenever a fetch's results change. SQLite is the default backend; the
`plugin_data_url` option points the store at a Postgres database instead, and a plugin cannot
tell the difference. It needs the `plugin_data` permission.

```lua
local data = zellij.data

function M.load()
  zellij.request_permission { "plugin_data" }
  zellij.subscribe { "plugin_data_changed", "permission_request_result" }
end

zellij.on("permission_request_result", function()
  data.define {
    name = "note", version = 1,
    attrs = {
      { "title", "text", required = true },
      { "body", "text", full_text = true },
      { "done", "bool", indexed = true, default = false },
    },
  }
  data.apply(data.put("note", { title = "first", body = "hello" }))
  M.watch = data.watch { entity = "note", filter = data.eq("done", false), sort = { "id desc" } }
end)

zellij.on("plugin_data_changed", function(event)
  M.notes = event.records         -- { { id = 1, uid = "...", title = "first", ... }, ... }
  return true
end)
```

`fetch` answers with records - one table per row, keyed by column, plain values - and, as a
second value, the raw rows with `truncated` and `next_cursor` for paging. `count` answers a
number. Mutations are `put(entity, values, uid)`, `update(entity, filter, values)` and
`delete(entity, filter)`, applied one at a time or as a list in one transaction. Predicates are
`eq`, `ne`, `gt`, `gte`, `lt`, `lte`, `is_in`, `is_null`, `not_null`, `prefix`, `contains`, and
`all(...)`, `any(...)`, `negate(p)`. Attribute kinds are `text`, `int`, `real`, `bool`, `blob`
and `json`; a value written as `{ json = table }` is stored as JSON and `{ blob = "bytes" }` as
a blob. To clear an attribute, write `zellij.data.null` - a `nil` has no key for a mutation to
read. `id`, `uid`, `created_at` and `updated_at` are on every row without being declared.
Read them, sort by them, and filter on them, but do not put them in `attrs`.
`rowid`, `oid`, `xmin`, `xmax`, `cmin`, `cmax`, `ctid` and `tableoid` are also reserved attribute
names on both backends. A `define` that declares any reserved attribute fails without creating
an entity or changing existing schema and data.

### Sharing a namespace

Everything in a plugin's own namespace is private to it, and a plugin cannot name another
plugin's. Rows two plugins share live in a **shared namespace**, which is a namespace whose name
begins with `shared:`:

```lua
function M.load()
  -- Both permissions, in every plugin that touches the namespace - the reader too.
  zellij.request_permission { "plugin_data", "shared_plugin_data" }
end

zellij.on("permission_request_result", function()
  M.watch = zellij.data.watch { entity = "todo", namespace = "shared:todo", sort = { "id desc" } }
end)
```

Three rules, and every one of them is a load error or a denial rather than something that
degrades quietly:

- **The prefix is not optional.** `namespace = "todo"` is refused; `"shared:todo"` is the same
  store. The segment after the colon is lower-case letters, digits, `-` and `_`.
- **`shared_plugin_data` as well as `plugin_data`**, in every plugin that reads or writes it.
- **One plugin defines; the rest only use.** `define` is a migration: two plugins racing to
  define the same entity is a bug waiting for the day their two spellings differ. `require` is
  rooted at the plugin file's own folder, so a `hocus/` extension and a top-level pane genuinely
  cannot share a schema module - which makes "the owner defines, the reader fetches" the pattern
  rather than a preference. If a reader must be defensive, define exactly what the owner defines
  and let the error out; never swallow it into an empty list.

`namespace` is an argument to `define`, `fetch`, `count` and `watch`, and the second argument to
`apply`.

### Try it on twenty lines first

The namespace, the permissions and the column names are all settled by one reload of a plugin
that does nothing else. Write this before the real thing, not after:

```lua
local zellij = require("zellij")
local M = {}

function M.load()
  zellij.request_permission { "plugin_data", "shared_plugin_data" }
end

zellij.on("permission_request_result", function()
  zellij.log(zellij.json.encode(zellij.data.define {
    name = "todo", version = 1, namespace = "shared:todo",
    attrs = { { "title", "text", required = true }, { "done", "bool", default = false } },
  }))
  zellij.data.apply(zellij.data.put("todo", { title = "probe" }), "shared:todo")
  zellij.log("rows: " .. #zellij.data.fetch { entity = "todo", namespace = "shared:todo" })
end)

return M
```

`plugin_reload` it, `plugin_log` it. A cycle spent here is a cycle not spent rewriting a finished
plugin around a constraint it was built against.

### Finding what is already there

**You do not declare an entity you only read** - `define` is for entities you own, and a fetch
works against anything already in the store. `data.catalog()` says what that is:

```lua
for _, e in ipairs(data.catalog()) do          -- every namespace this plugin may see
  zellij.log(e.namespace .. " " .. e.entity .. " v" .. e.version)
  for _, a in ipairs(e.attrs) do
    zellij.log("  " .. a.name .. " " .. a.kind .. (a.indexed and " indexed" or ""))
  end
end
data.catalog("shared:hocus")                   -- or just one namespace
```

With no argument it lists the plugin's own namespace plus every `shared:` one, once
`shared_plugin_data` is granted. Another plugin's private namespace is never listed. See
"Hocus data" below for the one every session already has.

## Hocus data

Hocus keeps its projects, todos, notes, scheduled prompts and coding-agent records in the shared
namespace `shared:hocus`, and `hocus-store` declares every entity at session start - so the
tables are there whether or not a hocus pane is open. Any plugin holding `plugin_data` and
`shared_plugin_data` can read them:

```lua
local data = zellij.data
zellij.request_permission { "plugin_data", "shared_plugin_data" }

local todos = data.fetch {
  namespace = "shared:hocus", entity = "todo",
  filter = data.all(data.eq("root", project_root), data.eq("done", false)),
  sort = { "id" },
}
```

`example-plugins/hocus-todos.lua` is a working read-only plugin over exactly this, and
`example-plugins/hocus-notes.lua` is the same over the notes and their side tables.

**Read these; do not write them.** Nothing blocks a write, but hocus maintains invariants the
rows do not carry - list caps, tree shape, project discovery, a note's tags and links - so a
plugin that adds a row by hand can produce a list hocus will not accept. Mutate through hocus's
tools (`/todo_add`, `/todo_done`, `/note_add`) or `zellij pipe` to `hocus-store` instead.

`id`, `uid`, `created_at` and `updated_at` are on every row without being declared, here as
anywhere else.

**`project`** - the project registry. `uid` is the name.

| Attribute | Kind | |
|---|---|---|
| `name` | text | required; also the uid |
| `root` | text | required, indexed; the folder. Not unique - resolve a cwd by *longest matching root* |
| `firecracker_image` | text | the default Firecracker image for agents in this project |
| `remote` | text | |
| `worktree_status` | text | snake case, as serde spells it |
| `common_dir` | text | the main checkout, for a worktree |
| `discovered` | bool | required; true for one found by a running agent rather than named by hand |

**`todo`** - one item. The tree is edges, not nesting.

| Attribute | Kind | |
|---|---|---|
| `root` | text | required, indexed; the project's `root`, which is how a list is scoped |
| `parent_id` | int | the store `id` of the item above; null at top level |
| `text` | text | required, full-text searchable; Markdown |
| `done` | bool | required, indexed |

There is no `position` and no `depth`: siblings are only ever appended, so `id` ascending *is*
sibling order, and depth follows from the parent chain. One project's whole list fits in a single
fetch.

**`note`** - one note: a title and a Markdown body, kept with a project the way a todo list is,
or with none.

| Attribute | Kind | |
|---|---|---|
| `root` | text | indexed; the project's `root`, or **null for a global note** - filter with `is_null` |
| `title` | text | required, full-text searchable |
| `body` | text | required, full-text searchable; Markdown, up to 16 KB |
| `author` | text | required; `user`, `hocus` for one the model wrote, or an agent kind |
| `archived` | bool | required, indexed; hidden from hocus's listings until un-archived |

The note's timestamps are the store's own `created_at` and `updated_at`, in milliseconds. Both
text columns are in one full-text index, so `search = "word"` finds it in either and answers one
row per note. A listing should `select` everything *but* `body` - a thousand notes at the cap
would exceed one fetch's byte limit, and a listing does not show one anyway.

**`note_link`** - a note referencing one todo item, many to many. `uid` is `<note id>:<todo id>`.
**`note_tag`** - one tag on one note. `uid` is `<note id>:<tag>`.

| Attribute | Kind | |
|---|---|---|
| `note_id` | int | required, indexed; both entities |
| `todo_id` | int | required, indexed; `note_link` only - the store `id` of a `todo` row |
| `tag` | text | required, indexed; `note_tag` only - lowercase, `[a-z0-9_-]` |

A note's tags and links are the rows naming its id; a todo item's notes are the `note_link` rows
naming its `todo_id`; and a list's notes are the notes with its `root` plus the ones linked to
any item in it. Fetch a side table with `is_in("note_id", ids)` for the notes on screen rather
than the whole table - and not at all when there are none, since an empty `is_in` is refused.

**`job`** - a scheduled prompt.

| Attribute | Kind | |
|---|---|---|
| `job_id` | int | required, unique |
| `chat_id` | int | the chat that armed it; null for one never saved |
| `every` | real | seconds |
| `next_due_at` | int | wall-clock seconds; written when armed and when it fires, never per tick |
| `prompt` | text | required |
| `show` | bool | required |
| `auto_confirm` | bool | required |

**`agent_latch`** - a kind set on a pane by hand with `agent_register`. **`agent_summary`** - what
the daemon's model last said about an idle agent's pane. Both are keyed `<session>:<pane id>`:

| Attribute | Kind | |
|---|---|---|
| `pane_id` | int | required, indexed |
| `zellij_session_name` | text | required, indexed |
| `kind` | text | `agent_latch` only - `claude`, `codex`, ... |
| `text`, `title` | text | `agent_summary` only |
| `awaiting` | bool | `agent_summary` only |
| `created_at_ms` | int | `agent_summary` only - epoch milliseconds it was made; absent on a summary stored before this attribute existed |

The session is half the key because the store is not per-session: one file serves every zellij
server on the machine, so a bare pane id would make two sessions' pane 5 into one row. Filter by
`zellij_session_name` unless you mean every session.

**`agent_run`** - one hook-backed agent session, live or finished. `uid` is `<kind>:<session id>`.

| Attribute | Kind | |
|---|---|---|
| `pane_id` | int | required, indexed |
| `session_id` | text | required, indexed; the agent's own |
| `zellij_session_name` | text | indexed |
| `kind` | text | required |
| `active` | bool | required, indexed; false once the run ended |
| `started_at_ms` | int | required |
| `header` | json | the cheap summary an archive listing reads |
| `payload` | json | required; the run's metadata |

`payload` is the larger of the two columns, so `select = { "header" }` a listing and fetch
`payload` only for the row you are showing. Decode either with `zellij.json.decode`.

**`agent_run_entry`** - one retained event on a run's timeline: a prompt, a plan, a question, a
result, or a summary somebody asked for. `uid` is `<run uid>#<revision>`.

| Attribute | Kind | |
|---|---|---|
| `run_id` | text | required, indexed; the `agent_run` uid this belongs to |
| `revision` | int | required; the run's own counter, and what orders the timeline |
| `event_id` | text | the provider's id for the event behind it, where it has one |
| `kind` | text | required; `prompt`, `plan`, `question`, `step`, `result`, `summary` |
| `text` | text | required; the whole of what the agent said, uncut |
| `created_at_ms` | int | epoch milliseconds this entry was recorded; absent on an entry recorded before this attribute existed |

A row per event rather than a list inside `payload`, so a long-running agent costs one small
insert per event instead of a rewrite of its whole history - and so nothing has to be cut down
to keep that rewrite cheap. Filter by `run_id` and sort by `revision`; a run recorded by an
older hocus has no rows here and keeps its timeline inside `payload` instead.

Pane-level agent state - busy, waiting, idle - does not need this table at all: it is on
`PaneInfo.agent` in the pane manifest, and the `agent_start` / `agent_waiting` hook points below
fire on the transitions.

## Work queues

A plugin can put *other plugin code* on a named **work queue**, to run later as its own headless
plugin instance when the queue has a free slot. Each queue runs at most `concurrency` jobs at once
(one, until someone says otherwise), so a queue is how you build a merge queue, a deploy pipeline,
or anything else that must not run everything at the same time.

The enqueuer:

```lua
zellij.subscribe { "queue_job_started", "queue_job_finished", "queue_changed" }
zellij.configure_queue { queue = "merge", concurrency = 2 }

local job_id = zellij.enqueue_queue_job {
  queue = "merge",
  plugin_url = "file:merge_one.lua",
  config = { branch = "feature/x" },   -- the job's configuration
  context = { pr = "42" },             -- yours, echoed back; the job never sees it
}

zellij.on("queue_job_started", function(e)
  -- e.queue, e.job_id, e.plugin_id, e.context.pr
end)
zellij.on("queue_job_finished", function(e)
  -- e.outcome is "Succeeded", "Failed", "Cancelled" or "Aborted"; e.payload is what the job
  -- reported (or the error, for a job that failed to load or crashed)
end)

local queues = zellij.queue_status { queue = "merge" }   -- {} for every queue
zellij.cancel_queue_job { job_id = job_id }
```

The job is an ordinary plugin with no `render`. It is loaded `shared`, so it is one instance for
the whole session however many clients are attached (its configuration carries `shared = "true"`).
It learns what it is from its configuration and ends itself by reporting:

```lua
local zellij = require("zellij")
local M = {}

function M.load(config)
  -- config.zellij_queue, config.zellij_queue_job_id, config.zellij_queue_enqueuer_plugin_id,
  -- plus whatever the enqueuer put in `config` (here, config.branch) and `caller_cwd`
  zellij.run_command { { "git", "merge", config.branch }, { branch = config.branch } }
end

zellij.on("run_command_result", function(e)
  local exit_code = e.value[1]
  zellij.finish_queue_job { success = exit_code == 0, payload = "merged " .. e.value[4].branch }
end)

return M
```

`finish_queue_job` records the outcome and unloads the job; the enqueuer's slot frees and the next
pending job starts. A job that closes itself, or whose pane is closed, ends as `Aborted`; one that
fails to load or raises ends as `Failed`, with the error as its payload. `queue_changed` goes to
every subscriber whenever a queue's contents or limit change, and carries the same `pending` and
`running` lists `queue_status` does.

Permissions: `enqueue_queue_job` needs `MessageAndLaunchOtherPlugins`, `configure_queue` and
`cancel_queue_job` need `ChangeApplicationState`, and `queue_status` (and the `queue_changed`
event) need `ReadApplicationState`. `finish_queue_job` needs nothing: a job runs headless, and a
permission prompt it raised would have no pane to be answered in.

Two things a queue does not do: remember jobs across sessions, and tell an enqueuer that has
since unloaded anything - its events are dropped, and the job runs to completion regardless.

## Hocus extensions

Hocus discovers direct `.lua` files in the `hocus` folder under `user_plugin_dir` and loads them
headlessly. (The same folder's `skills/` subfolder holds hocus skills — prompts typed as
`/<name>`, see `docs/SKILLS.md`. Tools, extension tools and skills share one `/` namespace, and a
name goes to whichever registered first.) A file named `hocus/weather.lua` owns the `weather_`
tool namespace. File and local tool names are lower snake case, so a local tool named `forecast`
is advertised to the model and available at the prompt as `weather_forecast` and
`/weather_forecast`.

An extension declares its tools once, with their handlers, and the prelude does the rest - the
registration, the re-registration when hocus reloads and asks who is running, and every call:

```lua
local zellij = require("zellij")
local M = {}

function M.load(config)
  zellij.request_permission { "message_and_launch_other_plugins" }
end

zellij.on("permission_request_result", function()
  zellij.hocus.tools {
    forecast = {
      description = "Return a short forecast for a city.",
      parameters = {
        type = "object",
        properties = { city = { type = "string" } },
        required = { "city" },
      },
      safety = "read_only",
      handler = function(args)
        return "Clear skies over " .. tostring(args.city)
      end,
    },
  }
end)

return M
```

A handler returns the text the model reads - or a table, which is sent as JSON - or raises, and
the error reaches the model as `failed: …`. A call is a `zellij.rpc` call named
`hocus_tool:<name>` (see *Plugin-to-plugin calls*); handlers may call `zellij.await` directly:

```lua
handler = function(args)
  local result = zellij.await(zellij.async.run_command { command = { "date" } })
  return result.stdout
end
```

Hocus gives up on a call that goes unanswered for thirty seconds, and on one whose
extension unloaded first, and tells the model so.

`hocus_extension_name` and `hocus_extension_path` are in the configuration only when Hocus was
the one that launched the file — that is how the registration says which extension it is for.
Reloaded by hand (`zellij plugins reload hocus/weather`, or from a layout) they are `nil`, and the
prelude logs that the tools were not registered rather than sending a payload nobody reads;
`zellij.hocus.launch()` says which of the two happened. Hocus's own `plugin_reload` always
launches it the way that registers.

Every tool declares one safety mode:

| Mode | Planning mode | Confirmation |
|---|---|---|
| `read_only` | available | none |
| `automatic` | withheld | none |
| `confirm` | withheld | required before every model call |

A `confirm` tool also needs a non-empty `consequence`, written as the sentence the confirmation
shows (for example, `consequence = "posts the message to the shared channel"`). Typing a slash
command is already a user decision, so manual `/weather_forecast` calls do not ask again.

These safety modes do not grant authority. Every host command the Lua code calls still goes
through Zellij's plugin permissions. When Hocus writes the source, its `plugin_write` confirmation
shows and pre-grants the permissions requested in the file. A file installed by hand gets the
normal permission prompt when Hocus first loads its suppressed pane.

The order matters, and it is the same order for every extension that keeps data:

1. `load` asks for the permissions - `message_and_launch_other_plugins` at least, plus
   `plugin_data` and `shared_plugin_data` if it stores anything.
2. The `permission_request_result` handler (or a task awaiting `zellij.async.permissions`)
   defines the schema, declares the tools, and takes its first fetch or watch.
3. Tools and UI come after that lifecycle works. `example_todo_extension` is that lifecycle with
   three tools on it, and `example_todo_pane` is a pane reading the same `shared:` namespace.

Use `plugin_write name=hocus/weather`, then `plugin_reload name=hocus/weather`. The reload runs
the extension in the background and reports the names it registered. A registration or runtime
error is in `plugin_log name=hocus/weather`. There is deliberately no file watcher: reload is the
point at which a complete edit becomes live.

**Registered is not working.** The reload's `registered: ...` line says the extension offered
those names, not that any of them ran. Call each one once - by name, as a tool, or as
`/weather_forecast` at the prompt - before saying the extension works, and say plainly which ones
you did not get to.

## Hooks

A **hook point** is a moment in the session's life that a plugin can own. Bind one in the config:

```kdl
hooks {
    agent_exit "file:my-agent-exit.lua"
}
```

A `file:` path that is not there is looked for in the plugin folder, so a bare name is a hook
written with `plugin_write name=my-agent-exit`. An absolute path works too, and means exactly
what it says.

The hook points:

| Point | When | Shape |
|---|---|---|
| `agent_exit` | an agent pane's command exited | opened **in place of** that pane |
| `agent_start` | a pane gained an agent record: spawned as an agent, or its agent reported for itself | headless, `hook` event |
| `agent_waiting` | an agent's record moved to `waiting` - a permission prompt, a question | headless, `hook` event |
| `agent_idle` | an agent's record moved to `idle` from `busy` or `waiting`: its turn ended | headless, `hook` event |
| `pane_exit` | any command pane's command exited, held or not | headless, `hook` event |
| `session_start` | the session's first client connected | headless, `hook` event |
| `context_menu` | a right-click asked for a menu | headless, `context_menu_requested` event |

A point has one of two shapes, and the shape is a fact about the point. `agent_exit` opens its
plugin **in place of** the pane, once per firing, with the payload as the plugin's configuration.
Every other point runs its plugin **headless**: one long-lived instance, loaded without a pane the
first time the point fires, receives every later firing as an event.

### The `agent_exit` hook

The exited pane is *suppressed* behind the hook, not closed: its scrollback survives, and closing
the hook pane is what brings it back. So a hook that wants to leave the pane alone just closes
itself.

The payload arrives as the plugin's configuration, so `load(config)` receives it. Every key is
prefixed, because an unprefixed `cwd` would collide with a reserved configuration key and arrive
empty:

| Key | |
|---|---|
| `agent_pane_id` | the pane the hook stands in for, as a string |
| `agent_cwd` | where the agent was working |
| `agent_exit_code` | absent if the command was killed by a signal |
| `agent_command` | the command that exited, for display |
| `agent_kind` | the last recognized agent kind |
| `agent_target` | `local`, `ssh`, or `firecracker` |
| `agent_workflow_owner`, `agent_workflow_run`, `agent_workflow_role` | helper handoff context, when present |
| `agent_firecracker_vm` | VM name, for a worktree-backed Firecracker pane only |
| `agent_firecracker_worktree` | host worktree, for a worktree-backed Firecracker pane only |
| `agent_session` | owning session, for a worktree-backed Firecracker pane only |
| `agent_zellij_executable` | exact host binary used for Firecracker management commands |
| `agent_zellij_config_file` | selected config file, forwarded as `ZELLIJ_CONFIG_FILE` |
| `agent_zellij_config_dir` | selected config directory, forwarded as `ZELLIJ_CONFIG_DIR` |

For a worktree-backed Firecracker pane, the shipped hook checks the guest against its last sync
baseline. Select `<S> sync` to sync before the host workflow starts. Changes include committed
and uncommitted file results. Sync failures remain on the hook with diagnostics. Restart is
disabled while sync runs.


### Agent exit workflow

The builtin `agent-exit` hook starts a local Git workflow after an agent exits, at any exit status.
For a Firecracker checkout, select **S** to sync first. Remaining guest changes block host work.
Sync warnings require **R** to review them and confirmation that the conflicts are resolved.
SSH checkouts retain restart and close controls.

Use **R** to review branch commits, staged changes, unstaged changes, and untracked files.
Use the arrow keys or page keys to scroll. Enter returns to the choices.
**C** edits a commit message and asks for approval before staging all non-ignored changes.
**G** asks a helper agent for a draft. A generated draft does not authorize a commit.
After a commit, the hook attempts a rebase onto the local default branch. It never fetches or pushes.
Conflicts offer a repair agent, continuation, and abort. Other failures show diagnostics.

Configure the workflow at the Git worktree root in `.zellij.kdl`:

```kdl
agent_exit {
    ci "cargo" "x" "ci" "check"
    // default_branch "main"
    // agent kind="codex" model="..." effort="medium"
}
```

CI is an argument vector. To use shell syntax, configure an explicit shell, such as
`ci "sh" "-c" "make lint && make test"`. Reading configuration does not execute it.
Without a default-branch override, the hook uses remote HEAD metadata or asks for a local branch.
Without agent settings, helpers use the exiting agent's kind and CLI defaults.
Unknown kinds require an explicit supported kind: `codex`, `claude`, or `cursor`.

`zellij.project_config.read(host_directory)` returns `{ root, source, config }` and needs `ReadFiles`.
`zellij.project_config.write(host_directory, source, expected)` validates before writing and needs `WriteFiles`.
Pass the exact previous source as `expected`, or nil for an absent file. Stale writes fail.
Writes use an OS lock and a temporary file in the worktree’s Git metadata directory, so an
interrupted writer does not block later writes or leave new files to stage. Atomic replacement
requires the Git metadata and checkout to be on the same filesystem. Otherwise, the write fails
and preserves the original file. Legacy `.zellij.kdl.lock` files in the checkout are ignored
and left for manual inspection.
These directories are host paths. Access outside the plugin directory also needs `FullHdAccess`.
Use Hocus `/project-setup` to propose and confirm settings. Missing CI configuration blocks CI and merge.

`zellij.agent.spawn` accepts optional `effort`. In `.zellij.kdl`, setting `effort` requires an
explicit agent `kind`. Codex uses `model_reasoning_effort`, Claude uses
`--effort`, and Cursor uses a model bracket override. Cursor requires an explicit model without
existing bracket overrides. Unsupported effort values fail before the pane opens.
Workflow helpers carry `agent_workflow_run` and `agent_workflow_role` (`message` or `repair`) in
context. The host supplies `agent_workflow_owner`. Their absolute cwd is a host path.
Exit-hook configuration includes these fields, `agent_kind`, and `agent_target`.
The owner receives the command-pane exit event. The helper hook also sends `agent_exit_helper_finished`.

CI runs in a command pane. Closing it does not count as success. Complete command logs and generated
messages remain in an external temporary directory, whose path appears in the workflow.
A successful gate permits a local fast-forward merge only while HEAD, checkout contents, target branch,
and gate configuration still match. The target checkout must be clean.
After merge, choose to keep both, remove the linked worktree, delete the branch, or remove both.
Cleanup uses non-force Git operations and refuses the primary checkout or dirty worktrees.
A branch-only deletion first detaches the clean linked checkout.

Each canonical worktree has one owner under its Git metadata in `agent-exit.lock`.
Duplicate hooks identify the owner. Recovery checks the owner and outstanding operations before proceeding.
Recovered workflows start at repository inspection and never reuse a previous CI pass.
Escape leaves a prompt. During an operation, closing the plugin leaves ownership information for recovery.


### Headless hooks

`agent_start`, `agent_waiting`, `agent_idle`, `pane_exit` and `session_start` share one plugin
shape. The plugin is loaded the first time its point fires and stays loaded; each firing is a
`hook` event carrying the point's name and a payload typed per point. A firing that arrives while that first load is
still under way is held for the loading instance and delivered once it is up, in order - two
points firing within a few milliseconds of each other reach one plugin, not two.

**Quickstart.** A headless hook ships with zellij, `zellij:agent-policy`
(`zellij-utils/assets/plugins/agent-policy.lua` in the source tree). It logs every firing it
receives and raises a notification when an agent stops for a question. Nothing binds it by
default. To see the shape work, from a shell in the session:

```sh
zellij action set-hook agent_waiting zellij:agent-policy --write
zellij action set-hook agent_start zellij:agent-policy --write
zellij run -- claude          # or type `claude` in a pane whose hooks are installed
zellij plugins log agent-policy
```

The first `agent_start` loads the plugin and is delivered to it; each line it logs reads
`[lua:agent-policy] hook agent_start {"agent":{"kind":"claude",...},"pane_id":7,...}`, and the next
permission prompt in that pane raises a toast that focuses the pane when clicked. To make it
yours, copy the file into your plugin folder as a plugin of its own - `plugin_write name=my-policy`
through hocus, or `zellij plugins write my-policy < agent-policy.lua` - add
`zellij.request_permission { "change_application_state" }` to its `load` (the shipped copy is a
builtin and needs no grant; yours is not), and bind `file:my-policy.lua` instead. The whole plugin
is:

```lua
local zellij = require("zellij")
local M = {}

function M.load(config)
  zellij.subscribe { "hook" }
  zellij.request_permission { "change_application_state" }   -- for a file: copy
end

zellij.on("hook", function(e)
  zellij.log("hook " .. e.point .. " " .. zellij.json.encode(e.payload))
  if e.point == "agent_waiting" then
    local agent = e.payload.agent
    zellij.notify {
      message = agent.kind .. " in pane " .. e.payload.pane_id
        .. ": " .. (agent.detail or "is waiting for you"),
      pane_id = zellij.terminal_pane(e.payload.pane_id),
    }
  end
end)

return M
```

One plugin may be bound to several points; `e.point` tells them apart. The plugin has no pane,
so `render` is never asked for and a permission prompt has nowhere to appear: request what you
need in `load`, and the first grant is remembered.

The payloads, typed - a number is a number and the agent's record is the record:

| Point | Fields |
|---|---|
| `agent_start`, `agent_waiting`, `agent_idle` | `pane_id`; `agent`, the record as it now stands (`kind`, `state`, `source`, `detail`, `cwd`, `session_id`, `run_id`); `previous_state` (`Busy`/`Waiting`/`Idle`/`Exited`, nil for a record that just appeared); `command`, if the pane is a command pane |
| `pane_exit` | `pane_id`, `exit_code` (nil if killed by a signal), `cwd`, `command`, `held` (`true` if the pane holds and can be re-run, `false` if it is closing) |
| `session_start` | `session_name`, `client_id` |

What fires `agent_start`: a pane created with an agent record (a plugin's `agent_spawn`), an
agent record appearing on a pane that had none (`claude` typed at a shell, reporting through
its hooks), a record whose last state was `exited` coming back (`RerunCommandPane`, the shipped
`agent_exit` hook's Enter), and a new `session_id` in the same pane. `agent_waiting` fires on the
edge into `waiting` only - a second `waiting` report with a new detail is the same wait.
`agent_idle` fires on the edge into `idle` from `busy` or `waiting` - the turn ended - and not for
a record that appears idle, which is `agent_start`'s. Clearing a record and moving to `exited`
fire nothing; the exit is `agent_exit`'s and `pane_exit`'s. Nor does clearing a record stop `agent_exit`: a pane that has ever held one is
an agent pane for as long as it lives, which is what makes a clean exit — where the agent's own
`SessionEnd` clears the record just before the process goes — fire the hook at all. Only a pane
re-run with a *different* command forgets it, record and memory alike: `run_command_in_pane` over
a held agent pane makes it that command's pane, and its exit is `pane_exit`'s alone. Enter typed into a held pane is the same road as `RerunCommandPane` - the tab
asks the screen to re-run it - so an agent restarted by hand fires `agent_start` whether or not
an `agent_exit` hook is bound.

For a pane that is both, `pane_exit` fires before `agent_exit`, so an observer hears of the exit
before the pane is covered.

`session_start` fires when the first client connects. The first tab may not exist yet - a hook
that wants to open panes should subscribe to `tab_update` and act on the first one; the point is
for registering state, timers and subscriptions.

### The `context_menu` hook

The odd one out: it is given no pane at all. A right-click menu is a thing zellij draws over the
panes and hit-tests itself, so the plugin's whole job is to say what is on it. It gets no
`render`, and it stays loaded between clicks rather than being opened per click.

```kdl
context_menu_on_right_click true   // in the options; the menu is opt-in

hooks {
    context_menu "file:menu.lua"
}
```

```lua
zellij.subscribe { "context_menu_requested" }

zellij.on("context_menu_requested", function(e)
  local items = {}
  if e.link then
    table.insert(items, { label = "Open " .. e.link.matched,
                          context = { url = e.link.matched } })
  end
  for _, item in ipairs(e.items) do          -- what the config declared
    if item.label ~= "Copy" or e.selected_text then
      table.insert(items, item)
    end
  end
  zellij.set_context_menu { request_id = e.request_id, items = items }
end)
```

The event carries the click: `pane_id`, `click` and `click_in_pane`, `on_frame`, `pane_title`,
`pane_command`, `selected_text`, `word_under_cursor`, `line_under_cursor`, `link` (a `link`-plugin
regex match under the cursor), and `items` - what the `context_menu { }` config block declared, in
order.

Two things it does *not* carry, both because getting them would mean blocking the thread every
mouse movement goes through: the pane's **cwd** (ask for it with `zellij.get_pane_cwd`) and its
**scrollback** (`zellij.get_pane_scrollback`). Both are ordinary async calls; re-render when the
answer arrives.

**Items you were given have an `id` and no actions.** Hand one back with its `id` and its
configured actions come back with it - so you can relabel, reorder and drop freely. An item you
build yourself has no `id`, runs no actions, and reaches you as `context_menu_item_selected`
carrying its `context`, from where `zellij.run_action` can do anything an action could.

Answering is not urgent and cannot be too late: `request_id` says which menu you are answering, and
an answer for one that has been dismissed or replaced is discarded. If you do not answer within a
tenth of a second the menu shows the configured items on its own, so a plugin that is slow, wedged
or absent never makes right-click do nothing.

Answer with an empty list to say "no menu here".

**`context_menu_item_selected` goes to the plugin that offered the item, and to nobody else.** An
item's `context` is yours; no other plugin sees it, however it is subscribed.

The shipped hook is `zellij-utils/assets/plugins/context-menu.lua`. A copy loaded from a `file:`
location needs `zellij.request_permission { "run_actions_as_user", "run_commands" }` in its `load`
- `run_commands` because its "Open" item shells out to the platform's link opener, which it finds
by probing `xdg-open` and `open` once at load.

A hook point takes **one** plugin. Two plugins cannot both replace the pane, and a precedence
rule between them would be worse to explain than the restriction - a hook that wants to do
several things does them itself. Bind the empty string to turn a hook off:

```kdl
hooks {
    agent_exit ""
}
```

which leaves the pane simply holding, as any other command pane does.

The shipped `agent_exit` hook is itself a Lua plugin - `zellij-utils/assets/plugins/agent-exit.lua`
in the source tree. `example-plugins/agent-commit.lua` is a small commit-only example
(`plugin_docs section=example_agent_commit`). Read them; do not copy them. Your own hook is a
plugin you write: `plugin_write name=my-agent-exit`, then bind it. A file-loaded copy needs
permissions for commands, project config reads outside its directory, pane state and changes,
and messages to its workflow owner. Insert this wrapper just before `return M`:

```lua
local load = M.load
function M.load(config)
  zellij.task(function()
    local result = zellij.await(zellij.async.permissions {
      "change_application_state", "read_application_state", "run_commands",
      "read_files", "full_hd_access", "message_and_launch_other_plugins",
    })
    if result == "Granted" then load(config) end
  end)
end
```

The wrapper waits for approval before initialization and does not initialize after denial.
The shipped builtin has these permissions automatically. When hocus writes your hook, its
approved permission request is cached. The wrapper also handles that cached grant.

Zellij does not rewrite your own config file, so the `hooks { ... }` block above is yours to add -
or, from a shell, `zellij action set-hook agent_exit file:my-agent-exit.lua --write` binds the
point at once and persists the binding in `config.d/zellij-managed.kdl` beside your config, which
is loaded with it. hocus can write the plugin but not bind it.

Three notes on writing one:

- **A reload gives it no payload.** `plugin_reload` - and any other way of opening the plugin by
  hand - starts it outside the hook, so `load` gets no `agent_pane_id` at all. Write the hook so
  that it still renders, with whatever it does to the pane simply skipped: that is why the shipped
  one reads its pane id through a function that answers nil. It is also the only way to see the
  hook render without exiting an agent to trigger it.
- **Restarting does not need the command.** `zellij.rerun_command_pane(pane_id)` makes the pane
  replay what it already holds, which reaches it even while it is suppressed.
- **Closing has an order.** Close the agent pane *first*, then yourself - closing yourself first
  restores it instead.

Because a hook is an ordinary plugin, anything a plugin can do is now reachable at agent exit -
including starting the next agent. `open_command_pane_in_git_checkout_near_plugin` (see *Agents
and git checkouts*) opens one on a branch or in a worktree of its own, in a single call; the pane
it opens is itself an agent pane, so this hook runs again when *that* command exits.

`agent-commit.lua` above offers to stage and commit whatever the agent left behind, using
`zellij.run_command` (a plugin can read and write files, but running git is a command, and
commands go through the host).

## Errors

Host commands raise Lua errors, and an error in an entrypoint is reported against your plugin
rather than taken quietly: the error text replaces the pane's contents, prefixed with the file
and line it came from. Use `pcall` if you want to handle one yourself.

To see what a plugin is doing short of an error, write to zellij's log:

```lua
zellij.log("pressed", event.key)     -- print(...) goes to the same place
```

Each line lands in `$TMPDIR/zellij-<uid>/zellij-log/zellij.log` tagged `[lua:<your file>]`, so
`grep -F 'keys.lua]'` on it shows one plugin's lines. A plugin has no terminal of its own, which
is why `print` is redirected there rather than left writing to the server's stdout. Both take any
number of values and join them with tabs, as `print` does.

### Tracing

`zellij.trace(true)` - or the configuration key `trace "true"` - logs one line for every command
the plugin dispatches (`-> open_command_pane {...}`), every event and pipe it is handed (`<-
pane_update`), and every task it starts and finishes. `zellij plugins log <name>` (or hocus's
`plugin_log`) then reads as a transcript of what the plugin did, which for a plugin with no pane
is the whole of the evidence.

## Files

A plugin can read and write inside the four folders zellij gives it, which are the same four a
wasm plugin has preopened:

| path | what |
|---|---|
| `/host` | the plugin's own folder - normally where the session was started. A relative path is read against this one. |
| `/data` | scratch for this instance, thrown away with the session |
| `/cache` | this plugin's own folder, kept across sessions |
| `/tmp` | zellij's temp folder, **shared with every other plugin and session** |
| `/plugins` | the user's plugin folder - this guide's `~/.config/zellij/plugins/` - when the session has one |

Zellij's own log, where `zellij.log` writes, is outside every root; `zellij.read_plugin_log(name,
lines)` answers with a plugin's last `lines` lines of it - the same lines `zellij plugins log
<name>` prints - and needs `author_plugins` rather than `read_files`.

```lua
function M.load()
  zellij.request_permission { "read_files", "write_files" }
end

function M.render()
  local ok, config = pcall(function()
    return zellij.json.decode(zellij.fs.read("/host/.mytool.json"))
  end)
  return ok and config.greeting or "no config yet"
end
```

`zellij.fs` has `read`, `write`, `list`, `metadata`, `remove`, `mkdir` and `rename`. They raise on
failure rather than returning nil, so use `pcall` where a missing file is an ordinary outcome -
or ask `zellij.fs.metadata`, which answers nil for something that is not there. `list` spells its
answers in the root you asked about, so they can be handed straight back to `read`.

Reading needs `read_files` and writing needs `write_files`, separately: a plugin that only reads
your config should not be able to rewrite it. Writing under `/plugins` needs `author_plugins` as
well - that is the authority to put code where zellij will run it - and so does
`zellij.reload_plugin_by_url("file:keys.lua")`, which reloads every running instance of a plugin
in place without opening a pane (the old instance gets `before_close` first, for state worth
keeping in `zellij.data`). Reaching outside all five folders needs `full_hd_access`, and is
refused without it.

Two things to know. A path is confined *after* symlinks are followed, so a link inside `/host`
pointing elsewhere is refused rather than followed out. And `/tmp` is a real prefix here, so an
absolute path that happens to begin `/tmp` is read as the shared root rather than taken literally
- the same collision the wasm preopens have.

## What Lua plugins may not do

A wasm plugin is sandboxed by its runtime: it can only reach the directories zellij preopens for
it. Lua has no equivalent boundary, so zellij builds the interpreter with a **restricted standard
library** and hands back the capabilities worth having as host commands, where the permission
system can see them.

Gone, and staying gone:

- **`io` and `package`** - never opened, which is also what keeps `package.loadlib` out of reach.
  Files go through `zellij.fs`, which is confined and permissioned; `io` would be neither.
- **`load`, `dofile`, `loadfile`, `loadstring`** - running code from a string or a path is how
  everything above gets reintroduced by the back door. `require` serves `"zellij"` and files in
  the plugin's own folder (see *Modules*), and nothing else.
- **`os.execute`, `os.exit`, `os.getenv`, `os.remove`, `os.rename`, `os.tmpname`, `os.setlocale`**
  - the process, its environment, and unconfined file access. `setlocale` is in the list because
  it is process-global: a plugin would be changing how the whole application formats numbers.
  Running a command is `zellij.run_command`, and the session's environment is
  `zellij.get_session_environment_variables` - both permissioned.
- **`debug`** - mlua will not open it in a safe interpreter at all, because it can reach upvalues
  and userdata metatables. That is memory-unsafety rather than merely too much authority.

What you do get beyond the obvious: `os.time`, `os.date`, `os.difftime` and `os.clock` need no
permission, because a wasm plugin reads the clock through WASI without one either and a clock is
not authority. `coroutine` is there. So is `zellij.json.encode`/`decode`, with the one caveat that
Lua cannot tell an empty array from an empty table, so `encode {}` is `[]`.

## Current limits

Everything below is a gap in the Lua binding, not in the plugin system:

- **Return values** - every command that answers is decoded into a Lua value, and a test fails
  when a new one is not. Should one slip through, the reply comes back as the raw protobuf
  bytes it arrived as, in a string.
- **Workers** - wasm plugins can spawn background workers; Lua plugins cannot. A task (see
  *Tasks*) is the answer to waiting, and a work queue (above) to running plugin code elsewhere;
  `zellij.post_message_to`, which addresses a worker, is refused by name.
  `zellij.post_message_to_plugin` reaches yourself as a `custom_message` event and works.
- **Timers are whole seconds** - `os.time` is the only clock the sandbox has.
- **Reload on change** - editing a plugin needs an explicit reload:
  `zellij action start-or-reload-plugin file:keys.lua`, the plugin manager's reload key, or
  `plugin_reload` if you are asking hocus, which also reads the pane back and reports whether
  the plugin loaded. Nothing watches the folder. A reload starts the plugin over: its Lua state
  is gone, and `load` runs again.
- **Typos are caught when called, not when written** - `zellij.<anything>` is a function, because
  that is what makes every command reachable without a wrapper each. A name zellij does not know
  raises "unknown zellij command" at the call, naming what you wrote.
