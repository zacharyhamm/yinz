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
| one value | `zellij.set_timeout(1.0)` |
| several values | `zellij.rename_tab(0, "logs")` |
| named fields | `zellij.new_tab { name = "logs", cwd = "/tmp" }` |

A command taking several values also accepts them by name where that reads better -
`zellij.rename_tab { tab_index = 0, name = "logs" }`. Fields you leave out are absent, not empty:
a command with no required arguments can be called with none.

`zellij.set_timeout(seconds)` sends one `timer` event; `zellij.set_interval(seconds)` keeps
sending them until `zellij.clear_interval()`. There is one interval per plugin: calling
`set_interval` again changes the period rather than starting a second clock.

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

There are two worked examples in `example-plugins/`. `clock.lua` is the small one:

```sh
zellij plugin --floating -- file:$PWD/example-plugins/clock.lua
```

and `playground.lua` is a keymap where each key calls a different shape of command and shows what
came back - the one to read when you want to see the surface in use:

```sh
zellij plugin --floating -- file:$PWD/example-plugins/playground.lua
```

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
it there is the plugin data store: a SQLite-backed store zellij keeps for plugins, with entities
you declare, writes in transactions, fetches with filters and full-text search, and watches
that send an event whenever a fetch's results change. It needs the `plugin_data` permission.

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

Everything in a plugin's own namespace is private to it. `shared_plugin_data` and a
`namespace` argument reach the namespaces other plugins publish, on the same terms a wasm
plugin does.

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

## Hooks

A **hook point** is a moment in the session's life that a plugin can own. Bind one in the config:

```kdl
hooks {
    agent_exit "file:~/.config/zellij/hooks/agent-exit.lua"
}
```

The hook points so far:

| Point | When | The plugin is |
|---|---|---|
| `agent_exit` | an agent pane's command exited | opened **in place of** that pane |
| `context_menu` | a right-click asked for a menu | run **headless** - it says what the menu contains, zellij draws it |

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

### The `context_menu` hook

The odd one out: it is given no pane at all. A right-click menu is a thing zellij draws over the
panes and hit-tests itself, so the plugin's whole job is to say what is on it. It gets no
`render`, and it stays loaded between clicks rather than being opened per click.

```kdl
context_menu_on_right_click true   // in the options; the menu is opt-in

hooks {
    context_menu "file:~/.config/zellij/hooks/menu.lua"
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
in the source tree. Copy it, point the config at your copy, and add
`zellij.request_permission { "change_application_state" }` to its `load`: the shipped one is a
builtin and so is granted privileged commands unconditionally, and yours will not be.

Two notes on writing one:

- **Restarting does not need the command.** `zellij.rerun_command_pane(pane_id)` makes the pane
  replay what it already holds, which reaches it even while it is suppressed.
- **Closing has an order.** Close the agent pane *first*, then yourself - closing yourself first
  restores it instead.

Because a hook is an ordinary plugin, anything a plugin can do is now reachable at agent exit.
`example-plugins/agent-commit.lua` is a worked example that offers to stage and commit whatever
the agent left behind, using `zellij.run_command` (a plugin can read and write files, but running git is
a command, and commands go through the host).

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

## Files

A plugin can read and write inside the four folders zellij gives it, which are the same four a
wasm plugin has preopened:

| path | what |
|---|---|
| `/host` | the plugin's own folder - normally where the session was started. A relative path is read against this one. |
| `/data` | scratch for this instance, thrown away with the session |
| `/cache` | this plugin's own folder, kept across sessions |
| `/tmp` | zellij's temp folder, **shared with every other plugin and session** |

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
your config should not be able to rewrite it. Reaching outside all four folders needs
`full_hd_access` as well, and is refused without it.

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
- **Workers** - wasm plugins can spawn background workers; Lua plugins cannot. A work queue
  (above) is the Lua answer to the same need, and `zellij.post_message_to` - which addresses a
  worker - does nothing from Lua. `zellij.post_message_to_plugin` reaches yourself as a
  `custom_message` event and works.
- **Reload on change** - editing a plugin needs an explicit reload:
  `zellij action start-or-reload-plugin file:keys.lua`, the plugin manager's reload key, or
  `plugin_reload` if you are asking hocus, which also reads the pane back and reports whether
  the plugin loaded. Nothing watches the folder. A reload starts the plugin over: its Lua state
  is gone, and `load` runs again.
- **Typos are caught when called, not when written** - `zellij.<anything>` is a function, because
  that is what makes every command reachable without a wrapper each. A name zellij does not know
  raises "unknown zellij command" at the call, naming what you wrote.
