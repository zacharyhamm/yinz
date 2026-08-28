-- A Lua plugin that exercises the host command surface, one key at a time.
--
--   plugin_write name=playground, then plugin_reload name=playground
--
-- or, from a checkout, directly:
--
--   zellij plugin --floating -- file:$PWD/example-plugins/playground.lua
--
-- `clock.lua` is the small first example; this is the one to read when you want to know what a
-- Lua plugin can actually reach. Every key below calls a different shape of host command, and the
-- pane shows what came back. See docs/LUA_PLUGINS.md.
local zellij = require("zellij")

local M = {}

M.defaults = { command = "date" }

-- What the last key did, shown in the middle of the pane.
M.last = "press a key"
M.tabs = {}
M.panes = 0
M.opened_pane = nil

local function say(format, ...)
  M.last = string.format(format, ...)
end

--- Run `action`, reporting either what it returned or why it was refused.
---
--- Host commands raise Lua errors - a missing permission, a bad argument, a reply zellij could not
--- produce - so a plugin that wants to carry on regardless wraps them like this.
local function try(what, action)
  local ok, described, value = pcall(action)
  if not ok then
    say("%s failed: %s", what, tostring(described))
    return nil
  end
  say("%s -> %s", what, described == nil and "ok" or tostring(described))
  -- An action returns what to say, and optionally the value worth keeping.
  if value ~= nil then
    return value
  end
  return described
end

--- A pane id as it arrives from the host: `{ Terminal = 3 }`, since `PaneId` is a Rust enum.
--- `zellij.terminal_pane(3)` builds the same thing in the other direction.
local function describe_pane(pane_id)
  if type(pane_id) ~= "table" then
    return "no pane id came back"
  end
  local kind, id = next(pane_id)
  return string.format("%s pane %s", kind, tostring(id))
end

function M.load(config)
  M.command = config.command
  zellij.subscribe { "key", "tab_update", "pane_update", "permission_request_result" }
  -- Everything below the queries needs one of these. Asking for them all up front means one
  -- prompt rather than one per key.
  zellij.request_permission {
    "read_application_state",
    "change_application_state",
    "open_terminals_or_plugins",
    "run_commands",
    "write_to_stdin",
  }
end

-- Events -------------------------------------------------------------------------------------

zellij.on("tab_update", function(event)
  M.tabs = event.value or {}
  return true
end)

zellij.on("pane_update", function(event)
  local count = 0
  for _, panes in pairs((event.value or {}).panes or {}) do
    count = count + #panes
  end
  M.panes = count
  return true
end)

zellij.on("permission_request_result", function(event)
  say("permissions: %s", tostring(event.value))
  return true
end)

-- Queries: commands that answer -----------------------------------------------------------------

local function focused_tab()
  for _, tab in ipairs(M.tabs) do
    if tab.active then
      return tab
    end
  end
  return nil
end

local actions = {}

--- `GetPluginIds` answers with a struct, which arrives as a table.
actions.i = function()
  try("get_plugin_ids", function()
    local ids = zellij.get_plugin_ids()
    return string.format("pane %d, client %d, cwd %s", ids.plugin_id, ids.client_id, ids.initial_cwd)
  end)
end

--- `GetZellijVersion` answers with one string, and arrives as one.
actions.v = function()
  try("get_zellij_version", zellij.get_zellij_version)
end

--- `GetFocusedPaneInfo` is a reply zellij can refuse - it raises rather than returning nil.
actions.p = function()
  try("get_focused_pane_info", function()
    local focused = zellij.get_focused_pane_info()
    return string.format("tab %d, %s", focused.tab_index, describe_pane(focused.pane_id))
  end)
end

--- `DumpLayout` answers with a whole KDL document; report its size rather than printing it.
actions.d = function()
  try("dump_layout", function()
    local layout = zellij.dump_layout("default")
    return string.format("%d bytes of kdl", #layout)
  end)
end

-- Commands that change the session --------------------------------------------------------------

--- A struct variant: named fields, and every one of them optional.
actions.t = function()
  try("new_tab", function()
    return zellij.new_tab { name = "playground" }
  end)
end

--- A tuple variant, written with names. `zellij.rename_tab(index, name)` is the same call.
actions.r = function()
  local tab = focused_tab()
  if not tab then
    say("rename_tab: no tab is focused yet")
    return
  end
  try("rename_tab", function()
    return zellij.rename_tab { tab_index = tab.position, name = "renamed-by-lua" }
  end)
end

--- The prelude smooths this one: `OpenTerminal` reads its cwd out of a `FileToOpen`'s `path`.
actions.n = function()
  local pane_id = try("open_terminal", function()
    local opened = zellij.open_terminal("/tmp")
    return describe_pane(opened), opened
  end)
  M.opened_pane = type(pane_id) == "table" and pane_id or M.opened_pane
end

--- A nested struct - `CommandToRun` - built from a plain table. The reply is the id of the pane
--- it just made, which is what lets the next command act on it.
actions.c = function()
  local pane_id = try("open_command_pane", function()
    local opened = zellij.open_command_pane {
      { path = "sh", args = { "-c", M.command .. "; sleep 30" } },
      {},
    }
    return describe_pane(opened), opened
  end)
  M.opened_pane = type(pane_id) == "table" and pane_id or M.opened_pane
end

--- Bytes, from a string, to a specific pane. Run `c` first so there is somewhere to write.
actions.w = function()
  if not M.opened_pane then
    say("write_to_pane_id: press c first, to have a pane to write to")
    return
  end
  try("write_to_pane_id", function()
    return zellij.write_to_pane_id("echo written by lua\r", M.opened_pane)
  end)
end

--- A newtype variant wrapping an enum: `Resize(Resize)`.
actions["+"] = function()
  try("resize", function()
    return zellij.resize("Increase")
  end)
end

actions["-"] = function()
  try("resize", function()
    return zellij.resize("Decrease")
  end)
end

--- A command this plugin deliberately never asked permission for. Before zellij told Lua plugins
--- about refusals, this did nothing at all and said nothing about it.
actions.x = function()
  try("copy_to_clipboard (unpermitted, on purpose)", function()
    return zellij.copy_to_clipboard("this will not be copied")
  end)
end

actions.q = function()
  zellij.close_self()
end

zellij.on("key", function(event)
  local action = actions[event.key]
  if event.key == "Esc" then
    action = actions.q
  end
  if not action then
    return false
  end
  action()
  return true
end)

-- Rendering ------------------------------------------------------------------------------------

local BOLD, DIM, RESET = "\27[1m", "\27[2m", "\27[0m"

function M.render(rows, cols)
  local tab = focused_tab()
  local lines = {
    BOLD .. "zellij lua playground" .. RESET,
    string.format(
      "%s%d tabs, %d panes, focused: %s%s",
      DIM,
      #M.tabs,
      M.panes,
      tab and tab.name or "-",
      RESET
    ),
    "",
    BOLD .. M.last .. RESET,
    "",
    DIM .. "queries" .. RESET,
    "  i  plugin ids        v  zellij version",
    "  p  focused pane      d  dump layout",
    DIM .. "commands" .. RESET,
    "  t  new tab           r  rename tab",
    "  n  terminal in /tmp  c  run `" .. tostring(M.command) .. "` in a new pane",
    "  w  write to that pane          +/-  resize",
    DIM .. "and" .. RESET,
    "  x  a command this plugin may not run",
    "  q  close        (" .. rows .. "x" .. cols .. ")",
  }
  return table.concat(lines, "\n")
end

return M
