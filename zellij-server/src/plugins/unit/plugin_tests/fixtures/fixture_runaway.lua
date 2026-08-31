-- A plugin that never comes back from `update`: the first key it sees starts a loop that
-- neither returns nor calls the host. Loaded by the `runaway` tests to prove the host can still
-- unload it and shut the session down.
local zellij = require("zellij")

local M = {}

function M.load(config)
  zellij.subscribe { "key" }
end

function M.update(event)
  if event.name == "key" then
    while true do end
  end
  return false
end

function M.render(rows, cols)
  return "runaway plugin, idle"
end

return M
