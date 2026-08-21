-- A minimal Lua plugin: a clock that ticks once a second.
--
--   zellij plugin --floating -- file:$PWD/example-plugins/clock.lua
--
-- See docs/LUA_PLUGINS.md.
local zellij = require("zellij")

local M = {}

M.defaults = { label = "the time is" }

function M.load(config)
  M.label = config.label
  M.ticks = 0
  zellij.subscribe { "timer", "key" }
  -- `close_self` is privileged; without this, Esc would be silently refused.
  zellij.request_permission { "change_application_state" }
  zellij.set_timeout(1.0)
end

zellij.on("timer", function()
  M.ticks = M.ticks + 1
  zellij.set_timeout(1.0)
  return true
end)

zellij.on("key", function(event)
  if event.key == "Esc" then
    zellij.close_self()
  end
end)

function M.render(rows, cols)
  local seconds = M.ticks % 60
  local minutes = math.floor(M.ticks / 60)
  return string.format(
    "\27[1m%s\27[0m %02d:%02d\n\n[Esc] close   (%dx%d)",
    M.label,
    minutes,
    seconds,
    rows,
    cols
  )
end

return M
