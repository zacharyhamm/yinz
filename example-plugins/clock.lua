-- A minimal Lua plugin: a clock that ticks once a second.
--
-- A plugin lives in your plugin folder. Hocus puts one there itself, and that is the loop:
--
--   plugin_write name=clock   -- the complete file
--   plugin_reload name=clock  -- its first render, or the load error
--   plugin_log name=clock     -- anything it logged
--
-- From a checkout you can also run this copy directly:
--
--   zellij plugin --floating -- file:$PWD/example-plugins/clock.lua
--
-- See docs/LUA_PLUGINS.md.
local zellij = require("zellij")

local M = {}

M.defaults = { label = "the time is", format = "%H:%M:%S" }

function M.load(config)
  M.label = config.label
  M.format = config.format
  -- `close_self` is privileged; without this, Esc would be silently refused. Reading the clock
  -- is not: `os.date` needs no permission, the same way a wasm plugin reads it through WASI.
  zellij.request_permission { "change_application_state" }
  zellij.subscribe { "timer", "key" }
  zellij.set_timeout(1.0)
end

zellij.on("timer", function()
  zellij.set_timeout(1.0)
  -- Ask for a redraw: the time changed even though no state of ours did.
  return true
end)

zellij.on("key", function(event)
  if event.key == "Esc" then
    zellij.close_self()
  end
end)

function M.render(rows, cols)
  return string.format(
    "\27[1m%s\27[0m %s\n\n[Esc] close   (%dx%d)",
    M.label,
    os.date(M.format),
    rows,
    cols
  )
end

return M
