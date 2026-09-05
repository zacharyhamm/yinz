-- An agent harness: start a coding agent, hand it a job, run the check when it says it is done,
-- hand the failure back until the check is green - and record how it went.
--
--   plugin_write name=harness, then plugin_reload name=harness headless=true    (from hocus)
--   zellij plugins write harness < example-plugins/agent-harness.lua              (from a shell)
--   zellij plugin --floating -- file:harness.lua -- prompt="fix the flaky test" check="cargo test"
--
-- Configuration, from the layout or `-- key=value`, merged over `M.defaults`:
--   prompt    what to ask the agent for
--   check     the shell command whose exit status says whether it did it
--   agent     claude, codex or cursor
--   branch    a branch to work on, in a worktree of its own (absent: the checkout as it is)
--   retries   how many times a failure is handed back before giving up
--   trace     "true" to log every command dispatched and every event received
--
-- The whole harness is one task: straight-line code that awaits the agent's turn, the check's
-- exit, and the agent's next turn, in order. See "Tasks" and "Agents" in docs/LUA_PLUGINS.md.
local zellij = require("zellij")

local M = {}

M.defaults = {
  prompt = "make the tests pass",
  check = "cargo test",
  agent = "claude",
  retries = "3",
}

M.lines = {}

local function say(line)
  table.insert(M.lines, line)
  zellij.log(line)
  zellij.request_render()
end

--- The last `n` lines of a command's output: what the agent is handed back on a red check.
local function tail(text, n)
  local lines = {}
  for line in (text or ""):gmatch("[^\n]+") do
    lines[#lines + 1] = line
  end
  local from = math.max(#lines - n + 1, 1)
  return table.concat(lines, "\n", from)
end

--- One row per run, in the plugin's own data namespace, so `zellij.data.fetch { entity = "run" }`
--- from another plugin (or a later version of this one) can read what happened.
local function record(config, status, attempts)
  pcall(function()
    zellij.data.define {
      name = "run",
      version = 1,
      attrs = {
        { "prompt", "text", required = true },
        { "check", "text", required = true },
        { "status", "text", required = true, indexed = true },
        { "attempts", "int", required = true },
      },
    }
    zellij.data.apply(zellij.data.put("run", {
      prompt = config.prompt,
      check = config.check,
      status = status,
      attempts = attempts,
    }))
  end)
end

function M.load(config)
  M.config = config
  M.status = "starting"
  zellij.task(function()
    zellij.await(zellij.async.permissions {
      "run_commands", "read_application_state", "write_to_stdin", "change_application_state",
      "plugin_data",
    })

    -- Start the agent: the argv, the permission mode, the branch and the inline hooks are the
    -- planner's job, and the pane it opens is an agent pane from the first moment.
    local pane = zellij.agent.spawn {
      kind = config.agent,
      prompt = config.prompt,
      branch = config.branch,
      name = "harness: " .. config.agent,
    }
    M.pane = pane
    say(("spawned %s in pane %d: %s"):format(config.agent, pane.Terminal, config.prompt))

    -- Its first turn: the answer to the opening prompt. The turn ends on a `Stop`, not on the
    -- `SessionStart` that comes first, and `fallback = "idle"` covers an agent whose hooks are
    -- not wired, off the pane's record.
    local turn = zellij.await(zellij.async.turn(pane, { fallback = "idle" }))
    say("agent: " .. tail(turn.message or "(no message)", 3))

    -- Where the agent works: what it said, or the worktree the branch was cut in.
    local info = zellij.agent.info(pane)
    local cwd = (info and info.cwd)
      or (config.branch and (".worktrees/" .. config.branch))
      or "/host"

    local retries = tonumber(config.retries) or 3
    for attempt = 1, retries + 1 do
      M.status = "checking"
      say(("check %d: %s"):format(attempt, config.check))
      local result = zellij.await(zellij.async.run_command {
        command = { "sh", "-c", config.check },
        cwd = cwd,
        max_bytes_returned = 64 * 1024,
      })
      if result.exit_code == 0 then
        M.status = "green"
        say("green")
        record(config, "green", attempt)
        zellij.notify { message = ("harness: %s is green after %d attempt(s)"):format(config.check, attempt) }
        return true
      end
      if attempt > retries then
        break
      end
      M.status = "red"
      say(("red (exit %s); handing it back, %d of %d"):format(tostring(result.exit_code), attempt, retries))
      local reply = zellij.await(zellij.agent.prompt(pane, table.concat({
        "The check `" .. config.check .. "` failed with exit " .. tostring(result.exit_code) .. ":",
        tail(result.stderr .. "\n" .. result.stdout, 40),
        "Fix it, then say what you changed.",
      }, "\n"), { fallback = "idle" }))
      say("agent: " .. tail(reply.message or "(no message)", 3))
    end
    M.status = "gave up"
    say(("still red after %d retries"):format(retries))
    record(config, "red", retries)
    zellij.notify { message = ("harness: %s is still red after %d retries"):format(config.check, retries) }
    return true
  end)
end

-- Headless, this is never asked for; in a pane it shows the story so far.
function M.render(rows, cols)
  local ui = zellij.ui
  local out = { ui.text("harness: " .. (M.status or "?")):selected():render() }
  local from = math.max(#M.lines - rows + 2, 1)
  for i = from, #M.lines do
    out[#out + 1] = zellij.text.truncate(M.lines[i], cols, "…")
  end
  return table.concat(out, "\n")
end

return M
