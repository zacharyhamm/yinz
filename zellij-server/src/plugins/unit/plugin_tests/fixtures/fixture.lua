-- Fixture for the Lua plugin tests. Unlike the wasm fixture, this needs no build step: the tests
-- point zellij at this file directly.
local zellij = require("zellij")

local M = {}

M.defaults = { greeting = "hello" }

M.presses = 0

function M.load(config)
  M.greeting = config.greeting
  zellij.subscribe { "key", "tab_update" }
  zellij.set_selectable(true)
end

function M.update(event)
  if event.name == "key" then
    M.presses = M.presses + 1
    return true
  end
  if event.name == "tab_update" then
    M.tabs = #(event.value or {})
    return true
  end
  return false
end

function M.render(rows, cols)
  return string.format(
    "%s from lua | %dx%d | presses %d | tabs %s",
    M.greeting,
    rows,
    cols,
    M.presses,
    tostring(M.tabs)
  )
end

return M
