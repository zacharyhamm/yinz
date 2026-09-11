# Lua configuration

Zellij accepts `config.lua` as a peer of `config.kdl`. When both files are in the configuration
directory, `config.lua` wins. An explicit `--config` path always wins and may name either format.

Configuration-as-code is Lua only, by decision rather than omission: the configuration is read
before any server exists, by the client and by `zellij setup`, and the Deno runtime is a
per-plugin isolate that lives in the server. There is no `config.ts`; a TypeScript file is a
plugin (see `docs/DENO_PLUGINS.md`), and a `hooks { }` or `plugins { }` block naming one is the
way TypeScript reaches the configuration.

Lua configuration is trusted user code. It has the normal Lua standard libraries, environment
access through `os.getenv`, and `require` searches beside the configuration file before the normal
Lua search path. Zellij evaluates the file from scratch at startup and whenever the file changes.
If a reload fails, the running session keeps its last valid configuration and logs the error.

## A configuration

The file imports and returns Zellij's configuration builder:

```lua
local config = require("zellij.config")

config.options {
  theme = "dracula",
  default_mode = "Normal",
  mouse_mode = true,
}

config.ui {
  pane_frames = {
    rounded_corners = true,
    hide_session_name = false,
  },
}

config.env { EDITOR = "nvim" }
config.plugin("my-tools", "file:tools.lua", { greeting = "hello" })
config.background_plugin("my-tools")
config.hook("agent_exit", "file:agent-exit.lua")

config.bind("normal", "Ctrl g", {
  config.action("NewPane", "Right"),
  config.action("SwitchToMode", "Normal"),
})

return config
```

The builder covers the same sections as KDL:

- `options`, `ui`, and `env` take tables.
- `theme(name, palette)` adds or replaces a theme.
- `plugin(alias, location, settings)`, `background_plugin(location, settings)`, and
  `hook(name, location, settings)` configure plugins. A hook `name` is any point in
  `docs/LUA_PLUGINS.md`'s table: `agent_exit`, `agent_start`, `agent_waiting`, `pane_exit`,
  `session_start`, `context_menu`.
- `bind(mode, key, actions)` and `unbind(mode, key)` change keybindings.
- `clear_options`, `clear_keybinds`, `clear_themes`, `clear_plugins`, `clear_hooks`, `clear_ui`,
  `clear_env`, and `clear_background_plugins` discard inherited values.

An action with positional KDL arguments uses `config.action("SwitchToMode", "Pane")`. A table is
read as KDL properties. For an action needing all forms, use
`config.action(name, { args = {...}, props = {...}, children = {...} })`. `config.raw(kdl)` is the
escape hatch for newly added KDL syntax that predates a documentation update.

Zellij never rewrites executable Lua. What it is asked to persist - `zellij action reconfigure
--write`, `zellij action set-hook --write`, a plugin's `reconfigure(kdl, true)` - goes into
`config.d/zellij-managed.kdl` beside the config, whatever the config is written in; every `*.kdl`
in `config.d/` is layered over the config in file-name order, so the managed file wins for the
keys it sets and leaves the rest of `config.lua` alone. Plugin keybinding changes with
`write_config_to_disk` are the one thing still written as a whole config file, and those are
refused for Lua: they apply to the live session and report that writing `config.lua` failed.

Only the selected top-level file is watched. A required module is re-evaluated the next time
`config.lua` itself is saved.
