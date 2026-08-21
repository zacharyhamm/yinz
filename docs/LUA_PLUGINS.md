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
    -- named fields are readable directly, and as event.value.bare_key
    if event.bare_key == "Esc" then ... end
  elseif event.name == "tab_update" then
    for _, tab in ipairs(event.value) do ... end   -- a list
  elseif event.name == "timer" then
    local elapsed = event.value                    -- a number
  end
end
```

Instead of one big `if`, you can register handlers:

```lua
zellij.on("key", function(event)
  ...
  return true    -- or zellij.request_render()
end)
```

Handlers run before `update`, and either mechanism can request a render.

## Commands

```lua
zellij.subscribe(events)          zellij.set_selectable(bool)
zellij.request_permission(perms)  zellij.open_terminal(cwd)
zellij.close_self()               zellij.request_render()
zellij.set_timeout(seconds)
```

Anything not yet wrapped by name is reachable with `zellij.command(name, args)`.

Privileged commands need permission, which the user is prompted for once:

```lua
function M.load()
  zellij.request_permission { "change_application_state", "open_terminals_or_plugins" }
end
```

Until permission is granted the commands it covers are refused - the same rule wasm plugins live
under.

There is a worked example in `example-plugins/clock.lua`:

```sh
zellij plugin --floating -- file:$PWD/example-plugins/clock.lua
```

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

- **Commands** - the named wrappers above, plus `zellij.command` for the rest. The full set of
  ~200 commands is not yet marshalled.
- **Return values** - commands that reply (`get_plugin_ids` and friends) hand back raw protobuf
  bytes rather than a Lua table.
- **Workers** - wasm plugins can spawn background workers; Lua plugins cannot.
- **Reload on change** - editing a plugin needs an explicit reload.
