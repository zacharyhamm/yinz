-- Everything a Lua plugin can currently do, in one pane.
--
--   zellij plugin --floating -- file:$PWD/example-plugins/capabilities.lua
--
-- Edit this file and press <TAB> on it in the plugin manager (or run
-- `zellij action start-or-reload-plugin file:$PWD/example-plugins/capabilities.lua`)
-- to see the change: the source is re-read on every load.
--
-- See docs/LUA_PLUGINS.md.
local zellij = require("zellij")

local M = {}

-- Configuration from the layout (or `zellij plugin -- ... -- key=value`) is merged over these.
M.defaults = { title = "Lua capabilities", tick = 1.0 }

function M.load(config)
  M.title = config.title
  M.tick = tonumber(config.tick) or 1.0
  M.ticks = 0
  M.timer_running = true
  M.selectable = true
  M.last_key = "-"
  M.modifiers = {}
  M.mode = "?"
  M.tabs = {}
  M.panes = 0
  M.log = {}

  -- The state events need `read_application_state`; `close_self` needs
  -- `change_application_state`; `open_terminal` needs `open_terminals_or_plugins`.
  -- The user is prompted once, and until they answer both events and commands are refused.
  zellij.request_permission {
    "read_application_state",
    "change_application_state",
    "open_terminals_or_plugins",
  }

  zellij.subscribe {
    "key",
    "timer",
    "mode_update",
    "tab_update",
    "pane_update",
    "permission_request_result",
  }

  zellij.set_timeout(M.tick)
  M.note("loaded with title=%q tick=%s", M.title, M.tick)
end

function M.note(fmt, ...)
  table.insert(M.log, 1, string.format(fmt, ...))
  M.log[6] = nil
end

-- Handlers registered with `zellij.on` run before `update`; either may ask for a re-render.
zellij.on("timer", function()
  M.ticks = M.ticks + 1
  if M.timer_running then
    zellij.set_timeout(M.tick) -- timers do not repeat
  end
  return true
end)

zellij.on("permission_request_result", function(event)
  M.note("permission result: %s", tostring(event.value))
  return true
end)

-- ...or write one `update` and match on `event.name`. Both mechanisms are live here.
function M.update(event)
  if event.name == "mode_update" then
    M.mode = tostring(event.mode) -- struct payloads are merged into the event
  elseif event.name == "tab_update" then
    M.tabs = event.value -- a list payload stays on `value`
  elseif event.name == "pane_update" then
    local count = 0
    for _, panes in pairs(event.panes or {}) do
      count = count + #panes
    end
    M.panes = count
  elseif event.name == "key" then
    M.last_key = event.key -- "t", "Esc", "F2" - not the raw enum
    M.modifiers = event.key_modifiers or {}
    M.on_key(event.key)
  else
    return false
  end
  return true
end

function M.on_key(key)
  if key == "Esc" then
    zellij.close_self()
  elseif key == "o" then
    zellij.open_terminal(".")
    M.note("opened a terminal in .")
  elseif key == "s" then
    M.selectable = not M.selectable
    zellij.set_selectable(M.selectable)
    M.note("selectable = %s", tostring(M.selectable))
  elseif key == "t" then
    M.timer_running = not M.timer_running
    if M.timer_running then
      zellij.set_timeout(M.tick)
    end
    M.note("timer %s", M.timer_running and "resumed" or "paused")
  elseif key == "u" then
    -- No named wrapper for `unsubscribe` yet, so go through the by-name escape hatch.
    zellij.command("unsubscribe", { "PaneUpdate" })
    M.note("unsubscribed from pane_update")
  end
end

-- Rendering ----------------------------------------------------------------------------------
-- `render` returns the whole frame as a string; ANSI escapes work, and `rows`/`cols` are the
-- pane's content size.

local BOLD, DIM, OFF = "\27[1m", "\27[2m", "\27[0m"
local CYAN, GREEN = "\27[36m", "\27[32m"

local function row(label, value)
  return string.format("  %s%-12s%s %s", DIM, label, OFF, value)
end

function M.render(rows, cols)
  local tab_names = {}
  for _, tab in ipairs(M.tabs) do
    table.insert(tab_names, tab.active and (GREEN .. tab.name .. OFF) or tab.name)
  end

  local mods = #M.modifiers > 0 and (" +" .. table.concat(M.modifiers, "+")) or ""

  local lines = {
    string.format("%s%s%s%s  %s%dx%d%s", BOLD, CYAN, M.title, OFF, DIM, rows, cols, OFF),
    "",
    row("uptime", string.format("%ds %s", M.ticks, M.timer_running and "" or DIM .. "(paused)" .. OFF)),
    row("mode", M.mode),
    row("last key", M.last_key .. mods),
    row("tabs", string.format("%d: %s", #M.tabs, table.concat(tab_names, ", "))),
    row("panes", tostring(M.panes)),
    row("selectable", tostring(M.selectable)),
    "",
    string.format("  %so%s open terminal  %ss%s selectable  %st%s timer  %su%s unsubscribe  %sEsc%s close",
      BOLD, OFF, BOLD, OFF, BOLD, OFF, BOLD, OFF, BOLD, OFF),
  }

  -- Only draw the log if the pane is tall enough to hold it: every render replaces the whole
  -- pane, so overflowing it just scrolls the useful part away.
  if rows > #lines + 2 then
    table.insert(lines, "")
    for _, entry in ipairs(M.log) do
      table.insert(lines, string.format("  %s%s%s", DIM, entry, OFF))
    end
  end

  return table.concat(lines, "\n")
end

return M
