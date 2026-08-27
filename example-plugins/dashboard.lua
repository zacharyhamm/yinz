-- A dashboard drawn with zellij's own UI components, so it looks like part of zellij.
--
--   zellij plugin --floating -- file:$PWD/example-plugins/dashboard.lua
--
-- It shows the session's tabs as ribbons, the focused tab's panes as a table, the output of a
-- command it runs every few seconds, and what the last key or click did. `playground.lua` is
-- the one to read for the shape of every command; this is the one to read for how a plugin
-- draws. See "Rendering" in docs/LUA_PLUGINS.md.
local zellij = require("zellij")
local ui = zellij.ui

local M = {}

M.defaults = { command = "uptime", every = 5 }

M.tabs = {}
M.panes = {}
M.output = "(waiting)"
M.last = "press a key or click a row"
M.selected = 1

function M.load(config)
  M.command = config.command
  M.every = tonumber(config.every) or 5
  zellij.subscribe { "tab_update", "pane_update", "key", "mouse", "timer", "run_command_result" }
  zellij.request_permission { "read_application_state", "run_commands" }
  zellij.set_timeout(0)
end

-- Events ------------------------------------------------------------------------------------

zellij.on("tab_update", function(event)
  M.tabs = event.value or {}
  return true
end)

zellij.on("pane_update", function(event)
  local focused
  for _, tab in ipairs(M.tabs) do
    if tab.active then
      focused = tab.position
    end
  end
  -- `panes` is keyed by tab position; the key arrives as a number.
  M.panes = focused and (event.value.panes or {})[focused] or {}
  return true
end)

zellij.on("timer", function()
  -- Commands raise when they are refused; a dashboard should keep drawing regardless.
  pcall(zellij.run_command, {
    command = { "sh", "-c", M.command },
    env_variables = {},
    cwd = ".",
    context = {},
    max_bytes_returned = 4096,
  })
  zellij.set_timeout(M.every)
end)

zellij.on("run_command_result", function(event)
  -- Named fields, and the output is already a string.
  if event.exit_code == 0 then
    M.output = event.stdout:gsub("%s+$", "")
  else
    M.output = "exit " .. tostring(event.exit_code) .. ": " .. event.stderr
  end
  return true
end)

zellij.on("key", function(event)
  if event.key == "Esc" or event.key == "q" then
    zellij.close_self()
  elseif event.key == "Down" or event.key == "j" then
    M.selected = math.min(M.selected + 1, math.max(#M.panes, 1))
  elseif event.key == "Up" or event.key == "k" then
    M.selected = math.max(M.selected - 1, 1)
  elseif event.key == "Enter" then
    local pane = M.panes[M.selected]
    if pane then
      local ok, err = pcall(function()
        if pane.is_plugin then
          zellij.focus_plugin_pane(pane.id, false, false)
        else
          zellij.focus_terminal_pane(pane.id, false, false)
        end
      end)
      M.last = ok and ("focused " .. pane.title) or tostring(err)
    end
  else
    M.last = "key: " .. event.key
    zellij.log("key", event.key) -- lands in zellij.log, tagged with this file
  end
  return true
end)

zellij.on("mouse", function(event)
  -- `event.kind` is the kind of mouse event; clicks carry `event.line` and `event.col`.
  if event.kind == "left_click" then
    -- Lines are 0-based: the ribbons are line 0, a blank line 1, the table's title row line 2,
    -- so the first pane row is line 3.
    local row = event.line - 2
    if row >= 1 and row <= #M.panes then
      M.selected = row
      M.last = ("clicked row %d"):format(row)
    end
  end
  return true
end)

-- Rendering ---------------------------------------------------------------------------------

function M.render(rows, cols)
  local lines = {}

  -- Tabs as ribbons: the active one selected.
  local ribbons = {}
  for _, tab in ipairs(M.tabs) do
    local ribbon = ui.ribbon(tab.name)
    if tab.active then
      ribbon:selected()
    end
    ribbons[#ribbons + 1] = ribbon:render()
  end
  lines[#lines + 1] = table.concat(ribbons)
  lines[#lines + 1] = ""

  -- The focused tab's panes as a table, one row selected. `pad` counts columns, so the title
  -- column lines up whatever the titles are made of.
  local panes = ui.table { { "pane", "title", "kind" } }
  for i, pane in ipairs(M.panes) do
    local title = ui.text(zellij.text.pad(pane.title, math.max(10, cols - 20)))
    if i == M.selected then
      title:selected()
    end
    local kind = pane.is_plugin and ui.text("plugin"):emphasis(2) or ui.text("terminal"):dim()
    panes:row(tostring(pane.id), title, kind)
  end
  lines[#lines + 1] = panes:render()
  lines[#lines + 1] = ""

  -- The command's output, and the last thing that happened.
  lines[#lines + 1] = ui.text(M.command):emphasis(0):render()
  for line in M.output:gmatch("[^\n]+") do
    lines[#lines + 1] = "  " .. zellij.text.truncate(line, cols - 2)
  end
  lines[#lines + 1] = ""
  lines[#lines + 1] = ui.nested_list {
    M.last,
    { "j/k or click to select, Enter to focus, q to close", indent = 1 },
  }:render()

  return table.concat(lines, "\n", 1, math.min(#lines, rows))
end

return M
