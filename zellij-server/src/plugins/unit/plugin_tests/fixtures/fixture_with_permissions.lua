-- Fixture for the Lua plugin tests that exercise host commands needing permission.
--
-- Kept separate from `fixture.lua` because a plugin that requests permissions waits for the
-- user's answer before it does anything, which would make the tests that just check loading and
-- rendering hang on a prompt nobody is there to grant.
local zellij = require("zellij")

local M = {}

function M.load()
  zellij.subscribe { "key" }
  zellij.request_permission { "change_application_state", "open_terminals_or_plugins" }
end

function M.update(event)
  if event.name == "key" then
    if event.key == "t" then
      zellij.open_terminal("/tmp/lua-fixture-cwd")
    elseif event.key == "Esc" then
      zellij.close_self()
    end
    return true
  end
  return false
end

function M.render()
  return "lua fixture with permissions"
end

return M
