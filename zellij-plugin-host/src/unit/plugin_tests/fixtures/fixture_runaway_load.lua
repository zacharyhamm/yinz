-- A plugin that never comes back from `load`: it tells the host it is there, then loops without
-- ever calling it again. Loaded by the `runaway` tests to prove an unload can still get past a
-- loader that is holding the plugin map across this call.
local zellij = require("zellij")

local M = {}

function M.load(config)
  -- Reaches the screen, so the test knows `load` is running before it sends the unload.
  zellij.set_selectable(false)
  while true do end
end

function M.update(event)
  return false
end

function M.render(rows, cols)
  return "never rendered"
end

return M
