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
-- No `request_permission` here, deliberately: a
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
local MAX_OUTPUT = 8192

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

local function run(step, args)
  zellij.run_command {
    command = args,
    env_variables = M.env_variables,
    cwd = "/",
    context = { step = step },
    max_bytes_returned = MAX_OUTPUT,
  }
end

local function output(bytes)
  local text = zellij.bytes_to_string(bytes):gsub("%s+$", "")
  return text
end

function M.load(config)
  -- Arrives as a string: the payload is a plugin configuration map, which is
  -- `BTreeMap<String, String>` on the host side. The `agent_` prefix is not decoration -
  -- `PluginUserConfiguration::new` drops a reserved word such as a bare `cwd`.
  M.pane_id = tonumber(config.agent_pane_id)
  M.cwd = config.agent_cwd or ""
  M.vm = config.agent_firecracker_vm
  M.worktree = config.agent_firecracker_worktree
  M.session = config.agent_session
  M.zellij = config.agent_zellij_executable or "zellij"
  M.env_variables = {}
  if config.agent_zellij_config_file then
    M.env_variables.ZELLIJ_CONFIG_FILE = config.agent_zellij_config_file
  end
  if config.agent_zellij_config_dir then
    M.env_variables.ZELLIJ_CONFIG_DIR = config.agent_zellij_config_dir
  end
  zellij.subscribe { "key", "run_command_result" }
  if M.vm and M.worktree and M.session then
    M.status = "Checking VM worktree for changes..."
    run("check", { M.zellij, "firecracker", "changes", M.vm, "--session", M.session })
  end
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
  elseif event.key == "s" and M.changes and M.changes > 0 and not M.syncing then
    M.syncing = true
    M.status = "Syncing VM worktree..."
    run("sync", { M.zellij, "firecracker", "sync", M.vm, "--session", M.session })
    return true
  end
end)

zellij.on("run_command_result", function(event)
  local exit_code, stdout, stderr, context = table.unpack(event.value)
  local step = context and context.step
  if step == "check" or step == "recheck" then
    local changes = exit_code == 0 and tonumber(output(stdout)) or nil
    if changes then
      M.changes = changes
      if step == "check" then
        M.status = changes > 0 and ("%d changed %s in the VM."):format(
          changes,
          changes == 1 and "path" or "paths"
        ) or nil
      elseif changes > 0 then
        M.status = ("VM worktree synced, but %d changed %s."):format(
          changes,
          changes == 1 and "path remains" or "paths remain"
        )
      end
    else
      M.status = (step == "recheck" and "Could not recheck VM changes: " or
        "Could not check VM changes: ") .. output(stderr)
    end
    if step == "recheck" then
      M.syncing = false
    end
    return true
  elseif step == "sync" then
    if exit_code == 0 then
      local warnings = output(stderr):gsub("%s*\n%s*", " ")
      M.status = warnings == "" and "VM worktree synced." or "VM worktree synced. " .. warnings
      run("recheck", { M.zellij, "firecracker", "changes", M.vm, "--session", M.session })
    else
      M.syncing = false
      M.status = "VM worktree sync failed: " .. output(stderr)
    end
    return true
  end
end)

function M.render(rows, cols)
  local middle = math.floor(rows / 2)
  local lines = {}
  for _ = 1, math.max(middle - 2, 0) do
    table.insert(lines, "")
  end
  local question = M.changes and M.changes > 0 and "Agent exited. What next?" or QUESTION
  table.insert(lines, "\27[1m" .. centered(question, cols) .. "\27[0m")
  table.insert(lines, "")
  table.insert(lines, centered("Directory: " .. M.cwd, cols))
  table.insert(lines, centered(zellij.text.truncate(M.status or "", cols), cols))
  table.insert(lines, "")
  local controls = CONTROLS
  if M.changes and M.changes > 0 then
    controls = ("<ENTER> restart  <S> sync %d  <ESC> close pane"):format(M.changes)
  end
  table.insert(lines, "\27[1m" .. centered(controls, cols) .. "\27[0m")
  return table.concat(lines, "\n")
end

return M
