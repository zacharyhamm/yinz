-- A plugin that runs its own model turn, with its own tool, and loops on the results.
--
--   zellij action new-pane --plugin file:example-plugins/hocus-complete.lua
--
-- `zellij.hocus.complete` is the mirror image of `zellij.hocus.tools`: an extension gives the
-- model in hocus's conversation a tool to call, while this asks the model a question of its
-- own. It is served by hocus-agentd, so no hocus pane need be open, and it is stateless — the
-- request carries everything the model sees, which is exactly what makes the loop below the
-- caller's to drive rather than hocus's.
--
-- See "Hocus completions" in docs/LUA_PLUGINS.md.
local zellij = require("zellij")

local M = { lines = {}, asking = false }

-- What the model may call. The same OpenAI shape hocus sends for its own tools; the daemon
-- passes these through untouched, and never runs one — that is this plugin's job.
local TOOLS = {
  {
    type = "function",
    ["function"] = {
      name = "count_panes",
      description = "How many panes are open in this session, and what is running in them.",
      parameters = { type = "object", additionalProperties = false },
    },
  },
}

-- The manifest, kept level by the `pane_update` event rather than asked for on demand: this
-- is what the tool answers out of.
local function run_tool(name)
  if name ~= "count_panes" then
    return "no such tool: " .. tostring(name)
  end
  local running = {}
  for _, panes in pairs(M.panes or {}) do
    for _, pane in ipairs(panes) do
      table.insert(running, pane.title or "?")
    end
  end
  return #running .. " panes: " .. table.concat(running, ", ")
end

local function say(line)
  table.insert(M.lines, line)
  zellij.__should_render = true
end

-- One turn, then as many more as the model's tool calls ask for. The transcript is this
-- plugin's own: each round appends the assistant's calls and this plugin's results and sends
-- the whole thing again, because the daemon kept none of it.
local function ask(question)
  if M.asking then
    return
  end
  M.asking = true
  zellij.task(function()
    local messages = { { role = "user", content = question } }
    -- A bound rather than `while true`: a model that keeps calling tools would otherwise
    -- spend the user's key until the session ends.
    for _ = 1, 5 do
      local reply = zellij.await(zellij.hocus.complete {
        system = "You answer about this terminal session. Be brief.",
        messages = messages,
        tools = TOOLS,
        timeout = 60,
      })
      if reply.error then
        say("error: " .. reply.error)
        break
      end
      if #(reply.tool_calls or {}) == 0 then
        say(reply.content)
        break
      end
      -- `content` travels back beside the calls even when empty: providers require it.
      table.insert(messages, { role = "assistant", content = reply.content, tool_calls = reply.tool_calls })
      for _, call in ipairs(reply.tool_calls) do
        say("· " .. call.name .. " " .. call.arguments)
        table.insert(messages, {
          role = "tool",
          tool_call_id = call.id,
          content = run_tool(call.name),
        })
      end
    end
    M.asking = false
  end)
end

function M.load()
  -- Completion requests and replies need messaging permission. Application state lets the
  -- prelude discover the daemon, and lets this plugin read the manifest and close itself.
  zellij.request_permission { "message_and_launch_other_plugins", "read_application_state" }
  zellij.subscribe { "pane_update", "key" }
  say("press <Enter> to ask, or q to quit")
end

zellij.on("pane_update", function(event)
  M.panes = event.panes
end)

zellij.on("key", function(event)
  if event.key == "Enter" then
    say("asking...")
    ask("What am I running in this session right now?")
    return true
  elseif event.key == "q" or event.key == "Esc" then
    zellij.close_self()
  end
  return false
end)

function M.render()
  return table.concat(M.lines, "\n")
end

return M
