-- A multi-step command sequence in ONE pane: `zellij.pane_workflow`.
--
-- The shape this replaces: a sync-skills-style job (list a remote folder, read a file, copy it)
-- that opened a brand-new pane per step, scattering panes and losing the thread between them.
-- Here the pane opens once, each step's command replaces the previous one when it exits, and
-- every finished step's output scrolls into the same pane's history - one transcript.
--
-- Configure a host and the steps run over ssh/scp; without one they run a local stand-in, so
-- the example works anywhere:
--
--   zellij plugin --floating -- file:$PWD/example-plugins/workflow.lua
--
-- Press `r` to run the workflow, `c` to cancel it mid-flight. The plugin's own pane shows each
-- step's exit code as it lands; the workflow's pane shows the commands themselves running.
local zellij = require("zellij")

local M = {}

M.defaults = {
  -- Set to an ssh host (e.g. "devbox") for the real thing; nil demonstrates locally.
  host = nil,
  skill = "deploy",
}

M.lines = { "r: run the workflow, c: cancel it" }
local function say(line)
  table.insert(M.lines, line)
  zellij.request_render()
end

local function steps(host, skill)
  if host then
    local remote = "~/.hocus/skills/" .. skill .. "/SKILL.md"
    return {
      { name = "list", path = "ssh", args = { host, "ls ~/.hocus/skills" }, timeout = 30 },
      { name = "read", path = "ssh", args = { host, "cat " .. remote }, timeout = 30 },
      { name = "copy", path = "scp", args = { host .. ":" .. remote, "/tmp/" .. skill .. ".SKILL.md" }, timeout = 60 },
    }
  end
  -- The same three-beat rhythm, locally: each step is a real child of the same pane.
  return {
    { name = "list", path = "sh", args = { "-c", "echo listing; ls; sleep 1" }, timeout = 30 },
    { name = "read", path = "sh", args = { "-c", "echo reading; head -3 CLAUDE.md 2>/dev/null; sleep 1" }, timeout = 30 },
    { name = "copy", path = "sh", args = { "-c", "echo copying; sleep 1; echo done" }, timeout = 30 },
  }
end

local function run_workflow()
  if M.wf and M.wf.status == "running" then
    say("already running in pane " .. M.wf.pane_id)
    return
  end
  M.wf = zellij.pane_workflow {
    name = "sync-skills",
    floating = true,
    steps = steps(M.host, M.skill),
    on_step = function(n, step, exit_code)
      say(("step %d (%s) exited %s"):format(n, step.name, tostring(exit_code)))
    end,
    on_done = function(result)
      say(("workflow %s; exit codes: %s"):format(result.status, zellij.json.encode(result.exit_codes)))
    end,
  }
  say("running in pane " .. M.wf.pane_id)
end

function M.load(config)
  M.host = config.host
  M.skill = config.skill
  zellij.subscribe { "key", "permission_request_result" }
  -- `timeout`/`cancel` escalate with signals, which is `change_application_state`.
  zellij.request_permission { "run_commands", "change_application_state" }
end

zellij.on("key", function(event)
  if event.key == "r" then
    run_workflow()
  elseif event.key == "c" and M.wf then
    M.wf:cancel()
    say("cancelling...")
  end
  return true
end)

function M.render()
  return table.concat(M.lines, "\n")
end

return M
