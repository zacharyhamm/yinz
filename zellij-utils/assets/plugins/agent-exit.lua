-- The `agent_exit` hook: what you see when an agent pane's command exits.
--
-- This used to be painted by the terminal emulator itself - a branch in
-- `render_first_run_banner` gated on one `|| is_hocus_agent` in `TerminalPane::hold`.
-- It lives here instead so that the question, the choices, and what each choice
-- does are a script you can copy and change rather than a compiled-in constant:
--
--   hooks {
--       agent_exit "file:~/.config/zellij/hooks/my-agent-exit.lua"
--   }
--
-- The host launches this plugin *in place of* the exited pane, which is suppressed
-- behind it rather than closed - so its scrollback survives and it can be re-run.
-- Closing this pane is what brings it back, which is why both branches below end in
-- `close_self()`.
--
-- No `request_permission` here, deliberately, for the reason `hocus-ask` gives: a
-- builtin - one loaded from a `zellij:` location - is granted every gated call
-- unconditionally by `check_command_permission`, and the permission dialog is drawn
-- *in the requesting plugin's pane*, which is this one. A copy of this file loaded
-- from a `file:` location is not a builtin and does need the request, so add
--
--   zellij.request_permission { "change_application_state" }
--
-- to `load` if you fork it.
local zellij = require("zellij")

local M = {}

local QUESTION = "Agent exited. Create another agent in the same directory?"
local CONTROLS = "<ENTER> create another agent  <ESC> close pane"

--- The pane this hook stands in for. Supplied by the host in the hook payload; nil
--- means the plugin was launched by hand rather than by the hook, in which case
--- there is nothing to restart and Esc is all that can work.
local function pane_id()
  return M.pane_id
end

--- Display width of a string. `#` counts bytes, which centres a path with any
--- non-ASCII in it a few columns off.
local function width(text)
  return utf8.len(text) or #text
end

local function centered(text, cols)
  local padding = math.floor((cols - width(text)) / 2)
  if padding < 1 then
    return text
  end
  return string.rep(" ", padding) .. text
end

function M.load(config)
  -- Arrives as a string: the payload is a plugin configuration map, which is
  -- `BTreeMap<String, String>` on the host side. The `agent_` prefix is not decoration -
  -- `PluginUserConfiguration::new` drops a reserved word such as a bare `cwd`.
  M.pane_id = tonumber(config.agent_pane_id)
  M.cwd = config.agent_cwd or ""
  zellij.subscribe { "key" }
end

zellij.on("key", function(event)
  if event.key == "Enter" then
    -- The pane replays the `RunCommand` it already holds, so the hook does not have
    -- to know how the agent was started. `rerun_terminal_pane_with_id` looks in
    -- `suppressed_panes`, so this reaches the pane hiding behind us.
    if pane_id() then
      zellij.rerun_command_pane(pane_id())
    end
    zellij.close_self()
  elseif event.key == "Esc" then
    -- Order matters. Closing the suppressed pane first takes the "pane is itself
    -- suppressed, it just goes" branch in `Tab::close_pane`; after that this pane no
    -- longer stands in for anything and closes without resurrecting it.
    if pane_id() then
      zellij.close_terminal_pane(pane_id())
    end
    zellij.close_self()
  end
end)

function M.render(rows, cols)
  local middle = math.floor(rows / 2)
  local lines = {}
  for _ = 1, math.max(middle - 2, 0) do
    table.insert(lines, "")
  end
  table.insert(lines, "\27[1m" .. centered(QUESTION, cols) .. "\27[0m")
  table.insert(lines, "")
  table.insert(lines, centered("Directory: " .. M.cwd, cols))
  table.insert(lines, "")
  table.insert(lines, "")
  table.insert(lines, "\27[1m" .. centered(CONTROLS, cols) .. "\27[0m")
  return table.concat(lines, "\n")
end

return M
