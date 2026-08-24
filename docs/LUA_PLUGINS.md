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

Load it from a layout by path:

```kdl
pane {
    plugin location="file:~/.config/zellij/plugins/keys.lua" {
        greeting "hi"
    }
}
```

or directly: `zellij plugin -- file:$PWD/keys.lua`.

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
  end
end
```

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

## Hooks

A **hook point** is a moment in the session's life that a plugin can own. Bind one in the config:

```kdl
hooks {
    agent_exit "file:~/.config/zellij/hooks/agent-exit.lua"
}
```

There is one hook point so far:

| Point | When | The plugin is |
|---|---|---|
| `agent_exit` | an agent pane's command exited | opened **in place of** that pane |

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
the agent left behind, using `zellij.run_command` (Lua has no `io` or `os`, so git goes through
the host).

## Errors

Host commands raise Lua errors, and an error in an entrypoint is reported against your plugin
rather than taken quietly. Use `pcall` if you want to handle one yourself.

## What Lua plugins may not do

A wasm plugin is sandboxed by its runtime: it can only reach the directories zellij preopens for
it. Lua has no equivalent boundary, so zellij gives a plugin a **restricted standard library** -
`string`, `table`, `math`, `utf8` and the base library, and no `io`, `os`, `debug`, `dofile`,
`loadfile`, `load`, or `package.loadlib`.

Without that, a plugin could read and write anything the zellij process can, and asking the user
to approve `open_terminals_or_plugins` would mean nothing. `require` serves only `"zellij"`.

This means a Lua plugin cannot currently read or write files at all. That is a deliberate starting
position: file access is meant to arrive as host commands that go through the same path
translation wasm plugins get, not by opening `io`.

## Current limits

Everything below is a gap in the Lua binding, not in the plugin system:

- **Return values** - the commands whose purpose is to answer are decoded into Lua values, as are
  the pane-opening ones. A reply this does not yet know how to read comes back as the raw
  protobuf bytes it arrived as, in a string.
- **Workers** - wasm plugins can spawn background workers; Lua plugins cannot.
- **Reload on change** - editing a plugin needs an explicit reload.
- **Typos are caught when called, not when written** - `zellij.<anything>` is a function, because
  that is what makes every command reachable without a wrapper each. A name zellij does not know
  raises "unknown zellij command" at the call, naming what you wrote.
