-- An `agent_exit` hook that offers to commit what the agent left behind.
--
--   hooks {
--       agent_exit "file:$PWD/example-plugins/agent-commit.lua"
--   }
--
-- The shipped hook offers two answers - restart, or close. This one adds a third, which is the
-- point of hooks existing: the choices at agent exit are a script, not a branch in the terminal
-- emulator.
--
-- Read `zellij-utils/assets/plugins/agent-exit.lua` first; this is that file with one more key.
--
-- Lua has no `io` or `os`, so git is run through the host with `zellij.run_command`, which
-- answers with a `run_command_result` event. The `context` you pass is handed back on that
-- event, which is how you tell which of several commands finished.
local zellij = require("zellij")

local M = {}

-- Enough for the output shown on failure; the whole point of the cap is that a runaway command
-- cannot fill memory.
local MAX_OUTPUT = 8192

local function run(step, argv)
  zellij.run_command {
    command = argv,
    env_variables = {},
    cwd = M.cwd,
    context = { step = step },
    max_bytes_returned = MAX_OUTPUT,
  }
end

--- `Vec<u8>` reaches Lua as a list of byte values, not as a string.
local function bytes_to_string(bytes)
  if type(bytes) ~= "table" then
    return ""
  end
  local out = {}
  for index, byte in ipairs(bytes) do
    out[index] = string.char(byte)
  end
  return table.concat(out)
end

function M.load(config)
  M.pane_id = tonumber(config.agent_pane_id)
  M.cwd = config.agent_cwd or ""
  M.status = nil
  -- A file: plugin is not a builtin, so unlike the shipped hook it does have to ask. Without
  -- `run_commands` the git calls below are refused; without `change_application_state` the pane
  -- cannot be restarted or closed.
  zellij.request_permission { "run_commands", "change_application_state" }
  zellij.subscribe { "key", "run_command_result" }
end

zellij.on("key", function(event)
  if event.key == "Enter" then
    if M.pane_id then
      zellij.rerun_command_pane(M.pane_id)
    end
    zellij.close_self()
  elseif event.key == "Esc" then
    if M.pane_id then
      zellij.close_terminal_pane(M.pane_id)
    end
    zellij.close_self()
  elseif event.key == "c" and not M.status then
    M.status = "staging..."
    run("stage", { "git", "add", "-A" })
    return true
  end
end)

zellij.on("run_command_result", function(event)
  -- A tuple variant, so the payload is positional: exit code, stdout, stderr, and the context
  -- the command was started with.
  local exit_code, _stdout, stderr, context = table.unpack(event.value)
  local step = context and context.step
  if exit_code ~= 0 then
    M.status = ("%s failed: %s"):format(step or "git", bytes_to_string(stderr))
    return true
  end
  if step == "stage" then
    M.status = "committing..."
    run("commit", { "git", "commit", "-m", "work from an agent session" })
  elseif step == "commit" then
    M.status = "committed."
  end
  return true
end)

function M.render(rows, cols)
  local function centered(text)
    local padding = math.floor((cols - (utf8.len(text) or #text)) / 2)
    return padding > 0 and string.rep(" ", padding) .. text or text
  end
  local lines = {}
  for _ = 1, math.max(math.floor(rows / 2) - 2, 0) do
    table.insert(lines, "")
  end
  table.insert(lines, "\27[1m" .. centered("Agent exited. What next?") .. "\27[0m")
  table.insert(lines, "")
  table.insert(lines, centered("Directory: " .. M.cwd))
  table.insert(lines, "")
  table.insert(lines, centered(M.status or ""))
  table.insert(lines, "")
  table.insert(
    lines,
    "\27[1m" .. centered("<ENTER> restart  <C> stage & commit  <ESC> close pane") .. "\27[0m"
  )
  return table.concat(lines, "\n")
end

return M
