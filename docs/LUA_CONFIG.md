# Lua configuration

Zellij accepts `config.lua` as a peer of `config.kdl`. When both files are in the configuration
directory, `config.lua` wins. An explicit `--config` path always wins and may name either format.

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

- `options`, `ui`, `web_client`, and `env` take tables.
- `theme(name, palette)` adds or replaces a theme.
- `plugin(alias, location, settings)`, `background_plugin(location, settings)`, and
  `hook(name, location, settings)` configure plugins.
- `bind(mode, key, actions)` and `unbind(mode, key)` change keybindings.
- `clear_options`, `clear_keybinds`, `clear_themes`, `clear_plugins`, `clear_hooks`, `clear_ui`,
  `clear_env`, `clear_web_client`, and `clear_background_plugins` discard inherited values.

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
