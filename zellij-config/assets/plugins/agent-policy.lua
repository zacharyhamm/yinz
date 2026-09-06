-- `agent-policy`: a headless hook that watches the agents in a session.
--
-- The smallest useful hook of the headless shape, shipped so that there is one to read, bind
-- and copy. It does two things:
--
--   * logs every firing of every point it is bound to, one line per firing with the whole
--     payload, so `zellij plugins log agent-policy` (or hocus's `plugin_log`) shows what the
--     session's agents have been doing;
--   * on `agent_waiting` - an agent stopped for a permission prompt or a question - raises a
--     notification naming the pane, which focuses that pane when clicked.
--
-- Bind it to the points you want to hear about. Nothing binds it by default, because
-- hocus-agentd already notifies on a waiting agent and two toasts for one prompt is one too many:
--
--   hooks {
--       agent_waiting "zellij:agent-policy"
--       agent_start   "zellij:agent-policy"
--       pane_exit     "zellij:agent-policy"
--   }
--
-- or, from a shell, `zellij action set-hook agent_waiting zellij:agent-policy --write`.
--
-- A headless hook is loaded the first time one of its points fires and stays loaded; each later
-- firing is a `hook` event with `point` (the name the config spells) and `payload`, typed per
-- point: an agent point carries `pane_id`, `agent` (the record: `kind`, `state`, `detail`, ...)
-- and `previous_state`; `pane_exit` carries `pane_id`, `exit_code`, `cwd`, `command`, `held`;
-- `session_start` carries `session_name` and `client_id` - see docs/LUA_PLUGINS.md under
-- "Headless hooks". It has no pane, so it has no `render` and never draws.
--
-- No `request_permission` here, for the reason `agent-exit.lua` gives: a builtin - one loaded
-- from a `zellij:` location - is granted gated calls unconditionally. A copy of this file loaded
-- from a `file:` location is not a builtin, so add
--
--   zellij.request_permission { "change_application_state" }
--
-- to `load` if you fork it - `notify` is gated on it.
local zellij = require("zellij")

local M = {}

--- The payload as one line of JSON, so two firings with the same payload log the same line and
--- a reader (or a test) can grep for a field.
local function describe(payload)
  return zellij.json.encode(payload)
end

--- What to say about an agent that stopped: the kind and pane it is, and its own account of
--- why, when it gave one.
function M.waiting_message(payload)
  local agent = payload.agent or {}
  local who = (agent.kind or "agent") .. " in pane " .. tostring(payload.pane_id or "?")
  local why = agent.detail
  if why == nil or why == "" then
    why = "is waiting for you"
  end
  return who .. ": " .. why
end

function M.load(config)
  zellij.subscribe { "hook" }
end

zellij.on("hook", function(event)
  local payload = event.payload or {}
  zellij.log("hook " .. event.point .. " " .. describe(payload))
  if event.point == "agent_waiting" then
    local pane = payload.pane_id
    zellij.notify {
      message = M.waiting_message(payload),
      pane_id = pane and zellij.terminal_pane(pane) or nil,
    }
  end
end)

return M
