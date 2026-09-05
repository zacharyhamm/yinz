-- Fixture for the plugin-thread test of `zellij.agent.spawn`: on `s` it spawns a claude with
-- its hooks left out, on `h` with them on. The pty thread records what it was asked to run.
local zellij = require("zellij")

local M = {}

function M.load()
  zellij.subscribe { "key" }
  zellij.request_permission { "run_commands" }
end

function M.update(event)
  if event.name == "key" then
    if event.key == "s" then
      zellij.agent.spawn { kind = "claude", prompt = "fix it", hooks = false, cwd = "/host" }
    elseif event.key == "h" then
      zellij.agent.spawn { kind = "claude", prompt = "fix it", cwd = "/host", name = "fixer" }
    end
    return true
  end
  return false
end

function M.render()
  return "spawn fixture"
end

return M
