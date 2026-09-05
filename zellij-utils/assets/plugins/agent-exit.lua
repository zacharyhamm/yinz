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

local function display(text)
  text = text:gsub("[%z\1-\8\11-\31\127]", "?")
  if not utf8.len(text) then
    text = text:gsub("[\128-\255]", function(byte) return ("\\x%02x"):format(byte:byte()) end)
  end
  return text
end

local function centered(text, cols)
  text = display(text):gsub("\n", "?")
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

local function nonempty(value)
  return value ~= "" and value or nil
end

local function log_failure(step, exit_code, stdout, stderr)
  zellij.log(("VM %s failed: vm=%s session=%s helper=%s exit_status=%s stdout=%q stderr=%q"):format(
    step, tostring(M.vm), tostring(M.session), M.zellij, tostring(exit_code),
    output(stdout), output(stderr)
  ))
end

-- One interactive task owns one checkout. Git remains the source of truth after every helper.
local W = { active = false, sequence = 0 }
local function raw(argv)
  local result = zellij.await(zellij.async.run_command {
    command = argv, cwd = "/", max_bytes_returned = 65536,
    env_variables = { GIT_TERMINAL_PROMPT = "0", GIT_EDITOR = "true", GIT_PAGER = "cat" },
  })
  return result.exit_code, zellij.bytes_to_string(result.stdout), zellij.bytes_to_string(result.stderr)
end
local function shell(script, ...)
  return raw { "sh", "-c", script, "agent-exit", ... }
end
local function command(argv)
  W.sequence = W.sequence + 1
  local log = W.tmp .. "/" .. W.sequence .. ".log"
  W.log = log
  local args = { "sh", "-c", [[
log=$1; operation=$2; shift 2
if [ -f "$operation" ]; then
  previous=$(cat -- "$operation")
  if kill -0 "$previous" 2>/dev/null; then printf 'Outstanding operation %s\n' "$previous"; exit 1; fi
fi
printf '%s\n' "$$" > "$operation" || exit 1
"$@" > "$log" 2>&1
status=$?
rm -- "$operation"
cat -- "$log"
exit "$status"
]], "agent-exit", log, W.lock .. "/operation" }
  for _, arg in ipairs(argv) do table.insert(args, arg) end
  W.busy = true
  local code, stdout, stderr = raw(args)
  W.busy = false
  W.diagnostic = stdout .. stderr .. "\nFull output: " .. log
  if #stdout >= 65536 then W.diagnostic = W.diagnostic .. "\n[display truncated at 64 KiB]" end
  return code, stdout
end
local function git(...)
  return command { "git", "-C", W.root, ... }
end
local function must_git(...)
  local code, text = git(...)
  if code ~= 0 then error(W.diagnostic, 0) end
  return text:gsub("\n$", "")
end
local function choose(title, options, detail)
  W.title, W.options, W.detail, W.offset = title, options, detail or "", 0
  while true do
    local event = zellij.await(zellij.async.event("key"))
    local key = event.key
    if key == "Esc" or key == "q" then return nil end
    if key == "Down" then W.offset = W.offset + 1
    elseif key == "Up" then W.offset = math.max(0, W.offset - 1)
    elseif key == "PageDown" then W.offset = W.offset + 15
    elseif key == "PageUp" then W.offset = math.max(0, W.offset - 15)
    elseif options[key] then return key end
  end
end
local function edit(title, initial)
  local text = initial or ""
  W.title, W.options, W.offset = title, { Enter = "accept draft", Esc = "cancel" }, 0
  while true do
    W.detail = text .. "\n\nType a message. Enter accepts this draft; committing needs separate approval."
    local event = zellij.await(zellij.async.event("key"))
    if event.key == "Esc" then return nil
    elseif event.key == "Enter" then if text:match("%S") then return text end
    elseif event.key == "Backspace" then
      local offset = utf8.offset(text, -1)
      if offset then text = text:sub(1, offset - 1) end
    elseif #event.key == 1 or utf8.len(event.key) == 1 then text = text .. event.key end
  end
end
local function config()
  local ok, snapshot = pcall(zellij.project_config.read, W.root)
  W.config_error = not ok and tostring(snapshot) or nil
  W.config = ok and snapshot.config or {}
  W.config_source = ok and snapshot.source or nil
  return ok
end
local function rebase_in_progress()
  for _, name in ipairs { "rebase-merge", "rebase-apply" } do
    local path = must_git("rev-parse", "--path-format=absolute", "--git-path", name)
    local code = raw { "test", "-d", path }
    if code == 0 then return true end
  end
  return false
end
local function clean()
  return must_git("status", "--porcelain=v1", "--untracked-files=all") == ""
end
local function default_branch()
  if not config() then error(W.config_error, 0) end
  local branch = W.config.default_branch or W.manual_default
  if not branch then
    local refs = must_git("for-each-ref", "--format=%(symref)", "refs/remotes")
    local names = {}
    for ref in refs:gmatch("[^\n]+") do
      local name = ref:match("^refs/remotes/[^/]+/(.+)$")
      if name then names[name] = true end
    end
    for name in pairs(names) do
      if branch then branch = nil; break end
      branch = name
    end
  end
  if not branch then branch = edit("Local default branch (no fetch)", ""); W.manual_default = branch end
  if not branch then return nil end
  local code = git("check-ref-format", "--branch", branch)
  if code ~= 0 or branch:sub(1, 1) == "-" then error("Invalid default branch: " .. branch, 0) end
  must_git("rev-parse", "--verify", "refs/heads/" .. branch)
  W.default = branch
  return branch
end
local function fingerprint()
  -- Include untracked bytes without staging the user's index. The temporary index is external.
  W.sequence = W.sequence + 1
  local index = W.tmp .. "/index-" .. W.sequence
  local code, tree = command { "sh", "-c", [[
root=$1; export GIT_INDEX_FILE=$2
trap 'rm -f -- "$GIT_INDEX_FILE"' EXIT
cd -- "$root" || exit 1
git read-tree HEAD 2>/dev/null || git read-tree --empty || exit 1
git add -A -- . && git write-tree
]], "agent-exit", W.root, index }
  if code ~= 0 then error(W.diagnostic, 0) end
  local _, head = git("rev-parse", "--verify", "HEAD")
  local branch = must_git("symbolic-ref", "--short", "HEAD")
  return tree .. head .. branch
end
local function inspect()
  W.tested = nil
  if rebase_in_progress() then return "conflict" end
  W.branch = must_git("symbolic-ref", "--short", "HEAD")
  return "review"
end
local function helper(role, prompt)
  if not config() then error(W.config_error, 0) end
  local spec = W.config.agent or {}
  local kind = spec.kind or M.kind
  if kind ~= "codex" and kind ~= "claude" and kind ~= "cursor" then
    error("Configure agent kind through Hocus /project-setup before spawning a helper.", 0)
  end
  if shell('printf "%s\\n" launching > "$1"', W.lock .. "/helper") ~= 0 then error("Cannot record helper launch", 0) end
  local ok, pane = pcall(zellij.agent.spawn, {
    kind = kind, model = spec.model, effort = spec.effort, prompt = prompt,
    cwd = W.root, source_directory = W.root, name = "Agent exit: " .. role,
    context = { agent_workflow_run = W.run, agent_workflow_role = role },
  })
  if not ok then
    raw { "rm", "-f", "--", W.lock .. "/helper" }
    error(pane, 0)
  end
  local exited = zellij.async.pane_exit(pane)
  local id = type(pane) == "table" and pane.Terminal or pane
  shell('printf "%s\\n" "$2" > "$1"', W.lock .. "/helper", tostring(id))
  W.title, W.detail, W.options = "Waiting for " .. role .. " agent", "Helper pane: " .. tostring(id), {}
  W.busy = true
  local result = zellij.await(exited)
  W.busy = false
  raw { "rm", "-f", "--", W.lock .. "/helper" }
  return not result.closed, result.exit_code
end
local function review()
  local code, text = command { "bash", "-c", [[
set -euo pipefail
cd -- "$1"
printf 'Branch: %s\n' "$(git symbolic-ref --short HEAD)"
if [ -n "$2" ]; then
  printf '\nBranch commits:\n'
  git log --oneline "refs/heads/$2..HEAD" --
  git diff --no-ext-diff --no-textconv "refs/heads/$2...HEAD" --
else
  printf '\nDefault branch unresolved; branch comparison unavailable.\n'
fi
printf '\nStaged changes:\n'
git diff --cached --no-ext-diff --no-textconv --
printf '\nUnstaged changes:\n'
git diff --no-ext-diff --no-textconv --
printf '\nUntracked files:\n'
git ls-files --others --exclude-standard -z | while IFS= read -r -d '' path; do
  status=0
  git diff --no-index --no-ext-diff --no-textconv -- /dev/null "$path" || status=$?
  [ "$status" -le 1 ] || exit "$status"
done
]], "agent-exit-review", W.root, W.default or "" }
  if code ~= 0 then error(W.diagnostic, 0) end
  return choose("Review diff — Up/Down/PgUp/PgDn scroll", { Enter = "return" }, text ..
    "\nFull diff: " .. W.log .. (#text >= 65536 and "\n[display truncated at 64 KiB]" or ""))
end
local function commit()
  local before = fingerprint()
  local draft = edit("Commit message", W.draft)
  if not draft then return "review" end
  W.draft = draft
  local answer = choose("Stage all non-ignored changes and commit?", { y = "approve commit", n = "back" }, draft)
  if answer ~= "y" then return answer == "n" and "review" or nil end
  if before ~= fingerprint() then
    W.notice = "Changes changed while editing; review them before approving a commit."
    return "review"
  end
  if rebase_in_progress() then return "conflict" end
  local code = git("add", "-A", "--", ".")
  if code == 0 then code = git("commit", "-m", draft, "--") end
  if code ~= 0 then W.notice = W.diagnostic; return "review" end
  W.draft = nil
  return "rebase"
end
local function rebase()
  local base = default_branch()
  if not base then return nil end
  if not clean() then W.notice = "Commit all changes before rebasing."; return "review" end
  if W.branch == base then W.notice = "The agent checkout is already on the default branch; use a topic branch."; return "review" end
  local code = git("rebase", "refs/heads/" .. base)
  local diagnostic = W.diagnostic
  W.notice = code ~= 0 and diagnostic or nil
  if rebase_in_progress() then return "conflict" end
  if code ~= 0 then W.notice = diagnostic; return "review" end
  return "gate"
end
local function conflict()
  local details = must_git("status", "--short") .. "\n" .. (W.notice or W.diagnostic or "")
  local key = choose("Rebase needs attention", { s = "spawn repair agent", r = "retry / continue", a = "abort rebase" }, details)
  if key == "s" then
    helper("repair", "Resolve the unfinished rebase in this checkout. Do not merge, push, or delete anything. Conflict details:\n" .. details)
    return inspect()
  elseif key == "a" then
    if git("rebase", "--abort") ~= 0 then W.notice = W.diagnostic; return "conflict" end
    return inspect()
  elseif key == "r" then
    if not rebase_in_progress() then return inspect() end
    local code = git("rebase", "--continue")
    if rebase_in_progress() then W.notice = W.diagnostic; return "conflict" end
    if code ~= 0 then W.notice = W.diagnostic; return "review" end
    return "gate"
  end
end
local function gate()
  local previous_base = W.default
  if not default_branch() then return nil end
  if W.default ~= previous_base then return "rebase" end
  if rebase_in_progress() then return "conflict" end
  if not clean() then return "review" end
  if not config() or not W.config.ci then
    W.notice = (W.config_error or "No CI command configured.") .. "\nUse Hocus /project-setup to configure .zellij.kdl."
    local key = choose("CI configuration required", { r = "read configuration again", b = "back to review" }, W.notice)
    return key == "r" and "gate" or key == "b" and "review" or nil
  end
  local argv = W.config.ci
  local tested = { head = must_git("rev-parse", "HEAD"), target = must_git("rev-parse", "refs/heads/" .. W.default),
    branch = W.branch, base = W.default, source = W.config_source }
  local answer = choose("Run full CI gate?", { y = "run gate", b = "back to review" }, zellij.json.encode(argv))
  if answer ~= "y" then return answer == "b" and "review" or nil end
  W.sequence = W.sequence + 1
  local log = W.tmp .. "/ci-" .. W.sequence .. ".log"
  local args = { "-c", [[
set -o pipefail
root=$1; log=$2; operation=$3; shift 3
cd -- "$root" || exit 1
printf '%s\n' "$$" > "$operation" || exit 1
trap 'rm -f -- "$operation"' EXIT
"$@" 2>&1 | tee -- "$log"
]], "agent-exit-ci", W.root, log, W.lock .. "/operation" }
  for _, arg in ipairs(argv) do table.insert(args, arg) end
  if shell('printf "%s\\n" launching > "$1"', W.lock .. "/helper") ~= 0 then error("Cannot record CI launch", 0) end
  local pane = zellij.open_command_pane { { path = "bash", args = args, cwd = "/" }, { agent_workflow_run = W.run } }
  if not pane then raw { "rm", "-f", "--", W.lock .. "/helper" }; error("CI command pane could not be opened", 0) end
  local exited = zellij.async.pane_exit(pane)
  local id = type(pane) == "table" and pane.Terminal or pane
  shell('printf "%s\\n" "$2" > "$1"', W.lock .. "/helper", tostring(id))
  W.title, W.detail, W.options, W.busy = "CI running", "Full log: " .. log, {}, true
  local result = zellij.await(exited)
  W.busy = false
  raw { "rm", "-f", "--", W.lock .. "/helper" }
  if not result.closed and result.exit_code == 0 then W.tested = tested; return "merge" end
  W.tested = nil
  local details = "Command: " .. zellij.json.encode(argv) .. "\nStatus: " .. tostring(result.exit_code) ..
    (result.closed and " (pane closed; no pass recorded)" or "") .. "\nLog: " .. log
  local key = choose("CI did not pass", { s = "spawn repair agent", r = "review changes", c = "retry CI" }, details)
  if key == "s" then
    helper("repair", "Repair the failing CI in this checkout. Do not merge, push, or delete anything.\n" .. details)
    return inspect()
  end
  return key == "r" and inspect() or key == "c" and "gate" or nil
end
local function worktrees()
  local list = must_git("worktree", "list", "--porcelain", "-z")
  local trees, tree = {}, nil
  for field in list:gmatch("([^\0]+)\0") do
    local path = field:match("^worktree (.*)$")
    if path then tree = { path = path }; table.insert(trees, tree)
    elseif tree then tree.branch = field:match("^branch refs/heads/(.*)$") or tree.branch end
  end
  return trees
end
local function merge()
  local answer = choose("CI passed — merge locally?", { y = "fast-forward default branch", b = "back to review" }, W.default)
  if answer ~= "y" then return answer == "b" and "review" or nil end
  local tested = W.tested
  config()
  if not tested or not clean() or rebase_in_progress() then return inspect() end
  if must_git("rev-parse", "HEAD") ~= tested.head or must_git("symbolic-ref", "--short", "HEAD") ~= tested.branch then return inspect() end
  if W.config_source ~= tested.source or W.config_error then W.tested = nil; return "gate" end
  if W.default ~= tested.base or must_git("rev-parse", "refs/heads/" .. W.default) ~= tested.target then W.tested = nil; return "rebase" end
  local target
  for _, tree in ipairs(worktrees()) do if tree.branch == W.default then target = tree.path end end
  local tested_config = W.tmp .. "/tested-config"
  if shell('printf "%s" "$2" > "$1"', tested_config, tested.source) ~= 0 then error("Cannot save tested configuration", 0) end
  local temporary = not target
  if temporary then
    target = W.tmp .. "/merge-checkout"
    if git("worktree", "add", "--", target, W.default) ~= 0 then error(W.diagnostic, 0) end
  end
  -- A single process rechecks the evidence and target checkout immediately before Git merges.
  local code = command { "sh", "-c", [[
set -eu
source=$1; target=$2; head=$3; base=$4; branch=$5; topic=$6; config=$7
cmp -s -- "$source/.zellij.kdl" "$config"
[ "$(git -C "$source" rev-parse HEAD)" = "$head" ] || { echo 'Tested HEAD changed'; exit 1; }
[ "$(git -C "$source" symbolic-ref --short HEAD)" = "$topic" ]
[ -z "$(git -C "$source" status --porcelain=v1 --untracked-files=all)" ]
[ "$(git -C "$target" symbolic-ref --short HEAD)" = "$branch" ]
[ "$(git -C "$target" rev-parse HEAD)" = "$base" ] || { echo 'Target branch moved; rebase and rerun CI'; exit 1; }
[ -z "$(git -C "$target" status --porcelain=v1 --untracked-files=all)" ] || { echo 'Target checkout is dirty; merge refused'; exit 1; }
git -C "$target" merge --ff-only -- "$head"
]], "agent-exit", W.root, target, tested.head, tested.target, tested.base, tested.branch, tested_config }
  local diagnostic = W.diagnostic
  if temporary then
    local removed = git("worktree", "remove", "--", target)
    if removed ~= 0 then diagnostic = diagnostic .. "\nTemporary checkout retained: " .. target .. "\n" .. W.diagnostic end
  end
  if code ~= 0 then W.notice = diagnostic; W.tested = nil; return "review" end
  W.merged = tested.head
  return "cleanup"
end
local function cleanup()
  local key = choose("Merged locally. Cleanup is optional.", { k = "keep both", w = "remove worktree", d = "delete branch", b = "remove both" }, W.branch .. "\n" .. W.root)
  if not key or key == "k" then return nil end
  if not clean() or must_git("rev-parse", "HEAD") ~= W.merged then error("Checkout changed after merge; cleanup refused.", 0) end
  local trees = worktrees()
  if not trees[1] or trees[1].path == W.root then error("Refusing cleanup of the primary checkout.", 0) end
  if must_git("symbolic-ref", "--short", "HEAD") ~= W.branch then error("Checkout branch changed; cleanup refused.", 0) end
  for _, tree in ipairs(trees) do
    if tree.path ~= W.root and tree.branch == W.branch then error("Branch is checked out elsewhere; cleanup refused.", 0) end
  end
  if key == "d" then
    if git("switch", "--detach", W.merged) ~= 0 then error(W.diagnostic, 0) end
  elseif key == "w" or key == "b" then
    if command { "git", "-C", trees[1].path, "worktree", "remove", "--", W.root } ~= 0 then error(W.diagnostic, 0) end
    -- Git removed this worktree's metadata, including its workflow lock.
    W.owns = false
    W.lock = W.tmp

  end
  if key == "d" or key == "b" then
    if command { "git", "-C", trees[1].path, "branch", "-d", "--", W.branch } ~= 0 then error(W.diagnostic, 0) end
  end
end
local function acquire()
  local ids = zellij.get_plugin_ids()
  W.run = W.tmp
  W.lock = W.gitdir .. "/agent-exit.lock"
  local code = raw { "mkdir", "--", W.lock }
  if code ~= 0 then
    local _, owner = raw { "cat", "--", W.lock .. "/owner" }
    local pid, plugin = owner:match("^(%d+) (%d+) ")
    if not pid then error("Workflow ownership is incomplete: " .. W.lock .. ". Inspect this lock before recovering it.", 0) end
    local live = raw { "kill", "-0", pid }
    if live == 0 and (tonumber(pid) ~= ids.zellij_pid or
        (tonumber(plugin) ~= ids.plugin_id and zellij.get_pane_info { Plugin = tonumber(plugin) })) then
      error("Another workflow owns this checkout: server " .. pid .. ", plugin " .. plugin .. ". Return to its pane.", 0)
    end
    local _, operation = raw { "cat", "--", W.lock .. "/operation" }
    local operation_pid = operation:match("^(%d+)")
    if operation_pid and raw { "kill", "-0", operation_pid } == 0 then
      error("An outstanding Git/CI operation is still running (PID " .. operation_pid .. "). Wait before recovering.", 0)
    end
    local _, helper_id = raw { "cat", "--", W.lock .. "/helper" }
    if helper_id:match("launching") then error("A helper launch was interrupted. Inspect outstanding panes before removing " .. W.lock .. "/helper", 0) end
    helper_id = tonumber(helper_id)
    if helper_id and live == 0 then
      local pane = zellij.get_pane_info { Terminal = helper_id }
      if pane and not pane.exited then error("Helper pane " .. helper_id .. " is still outstanding. Finish it before recovery.", 0) end
    end
    if choose("Recover abandoned workflow?", { y = "recover at repository inspection" }, "No remembered CI result will be reused.\n" .. W.lock) ~= "y" then return false end
    local recovered = shell([[
set -eu
lock=$1; expected=$2
mkdir -- "$lock/recovery"
trap 'rmdir -- "$lock/recovery" 2>/dev/null || true' EXIT
[ "$(cat -- "$lock/owner")" = "$expected" ]
rm -f -- "$lock/owner" "$lock/operation" "$lock/helper"
rmdir -- "$lock/recovery"
rmdir -- "$lock"
mkdir -- "$lock"
]], W.lock, owner:gsub("\n$", ""))
    if recovered ~= 0 then error("Ownership changed during recovery. Reopen the workflow.", 0) end
  end
  local owner = ids.zellij_pid .. " " .. ids.plugin_id .. " " .. W.run
  if shell('printf "%s\\n" "$2" > "$1"', W.lock .. "/owner", owner) ~= 0 then error("Cannot write workflow owner", 0) end
  W.owns = true
  return true
end
local function release()
  if W.owns then
    -- Do not release a lock while a command or helper can still modify this checkout.
    shell([[
lock=$1
[ ! -e "$lock/operation" ] && [ ! -e "$lock/helper" ] || exit 1
rm -- "$lock/owner" && rmdir -- "$lock"
]], W.lock)
    W.owns = false
  end
end
local function workflow()
  local code, root, err = raw { "git", "-C", M.worktree or M.cwd, "rev-parse", "--show-toplevel" }
  if code ~= 0 then error("No usable local repository.\n" .. err, 0) end
  W.root = root:gsub("\n$", "")
  local gitdir
  code, gitdir = raw { "git", "-C", W.root, "rev-parse", "--absolute-git-dir" }
  if code ~= 0 then error("Cannot resolve Git worktree metadata.", 0) end
  W.gitdir = gitdir:gsub("\n$", "")
  code, W.tmp = raw { "mktemp", "-d", "-t", "zellij-agent-exit.XXXXXXXX" }
  if code ~= 0 then error("Cannot create external workflow log directory.", 0) end
  W.tmp = W.tmp:gsub("\n$", "")
  if not acquire() then return end
  config()
  -- Resolve from configuration/metadata without forcing an input prompt until rebase needs it.
  W.default = W.config.default_branch
  if not W.default then
    local _, refs = git("for-each-ref", "--format=%(symref)", "refs/remotes")
    local names = {}
    for ref in refs:gmatch("[^\n]+") do local name = ref:match("^refs/remotes/[^/]+/(.+)$"); if name then names[name] = true end end
    local count = 0
    for name in pairs(names) do W.default = name; count = count + 1 end
    if count ~= 1 then W.default = nil end
  end
  local stage = inspect()
  while stage do
    W.title, W.options = "Inspecting repository...", {}
    local ok, next_stage = pcall(function()
      if stage == "review" then
        W.branch = must_git("symbolic-ref", "--short", "HEAD")
        if rebase_in_progress() then return "conflict" end
        local dirty = not clean()
        local key = choose("Agent exited. Review and integrate changes.", {
          r = "review diff", c = "commit", g = "generate commit message", n = "continue to rebase",
        }, "Branch: " .. W.branch .. "\nCheckout: " .. W.root .. "\n" ..
          (W.notice or W.config_error or (dirty and "Uncommitted changes present." or "Nothing needs committing.")))
        W.notice = nil
        if key == "r" then return review() and "review" or nil
        elseif key == "c" then if dirty then return commit() else return "rebase" end
        elseif key == "n" then return "rebase"
        elseif key == "g" then
          local before = fingerprint()
          local file = W.tmp .. "/commit-message"
          raw { "rm", "-f", "--", file }
          helper("message", "Analyze the branch commits and staged, unstaged, and untracked changes in this checkout. Write only a proposed commit message to this file: " .. file ..
            "\nDo not modify the checkout, stage, commit, rebase, merge, or push. The user will edit and approve the draft separately.")
          if rebase_in_progress() then return "conflict" end
          if before ~= fingerprint() then W.notice = "Checkout changed during message generation. Review it again."; return "review" end
          local status, draft = raw { "cat", "--", file }
          if status ~= 0 or not draft:match("%S") or #draft >= 65536 then
            W.notice = "No usable generated message. Generate again or enter a message with Commit."
          else W.draft = edit("Generated draft — edit before committing", draft) end
          return "review"
        end
      elseif stage == "rebase" then return rebase()
      elseif stage == "conflict" then return conflict()
      elseif stage == "gate" then return gate()
      elseif stage == "merge" then return merge()
      elseif stage == "cleanup" then return cleanup() end
    end)
    if ok then stage = next_stage else
      W.tested, W.notice = nil, tostring(next_stage)
      local key = choose("Operation failed", { r = "inspect repository again" }, W.notice)
      stage = key and inspect() or nil
    end
  end
  release()
end
function W.start()
  if W.active or not (M.worktree or M.cwd) then return end
  W.active = true
  W.title, W.detail, W.options, W.offset = "Inspecting repository...", "", {}, 0
  zellij.task(function()
    local ok, err = pcall(workflow)
    if not ok then
      if W.root then choose("Agent exit workflow unavailable", {}, tostring(err) .. "\nEsc returns to restart/close controls.")
      else M.status = tostring(err) end
      release()
    end
    W.active = false
  end)
end
function W.render(rows, cols)
  local lines = { W.title or "Inspecting repository...", "" }
  local content = {}
  -- Never interpret terminal control bytes from Git, filenames, or generated messages.
  local detail = display(W.detail or "")
  for line in (detail .. "\n"):gmatch("(.-)\n") do table.insert(content, line) end
  W.offset = math.min(W.offset or 0, math.max(0, #content - math.max(1, rows - 5)))
  for i = W.offset + 1, math.min(#content, W.offset + math.max(1, rows - 5)) do
    table.insert(lines, zellij.text.truncate(content[i], cols))
  end
  local controls = {}
  for key, label in pairs(not W.busy and W.options or {}) do table.insert(controls, key .. ": " .. label) end
  table.sort(controls)
  table.insert(lines, table.concat(controls, "  "))
  table.insert(lines, W.busy and "Operation running. Close this plugin to leave; ownership stays until it finishes." or "Esc/q: leave workflow   Up/Down: scroll")
  return table.concat(lines, "\n")
end

function M.load(config)
  -- Arrives as a string: the payload is a plugin configuration map, which is
  -- `BTreeMap<String, String>` on the host side. The `agent_` prefix is not decoration -
  -- `PluginUserConfiguration::new` drops a reserved word such as a bare `cwd`.
  M.kind = nonempty(config.agent_kind)
  M.target = nonempty(config.agent_target)
  M.pane_id = tonumber(config.agent_pane_id)
  M.cwd = nonempty(config.agent_cwd)
  M.vm = nonempty(config.agent_firecracker_vm)
  M.worktree = nonempty(config.agent_firecracker_worktree)
  M.session = nonempty(config.agent_session)
  M.zellij = nonempty(config.agent_zellij_executable) or "zellij"
  M.env_variables = {}
  if nonempty(config.agent_zellij_config_file) then
    M.env_variables.ZELLIJ_CONFIG_FILE = config.agent_zellij_config_file
  end
  if nonempty(config.agent_zellij_config_dir) then
    M.env_variables.ZELLIJ_CONFIG_DIR = config.agent_zellij_config_dir
  end
  zellij.subscribe { "key", "run_command_result", "command_pane_exited", "pane_closed" }
  if config.agent_workflow_owner and config.agent_workflow_run and config.agent_workflow_role then
    M.helper_owner = tonumber(config.agent_workflow_owner)
    M.status = "Helper finished. Return to workflow plugin " .. config.agent_workflow_owner .. "."
    zellij.message_to_plugin {
      destination_plugin_id = tonumber(config.agent_workflow_owner),
      message_name = "agent_exit_helper_finished", message_payload = config.agent_workflow_run,
      message_args = { role = config.agent_workflow_role, pane = tostring(M.pane_id) },
    }
    return
  end
  if M.vm and M.worktree and M.session then
    M.status = "Checking VM worktree for changes..."
    run("check", { M.zellij, "firecracker", "changes", M.vm, "--session", M.session })
  elseif config.agent_firecracker_vm ~= nil or config.agent_firecracker_worktree ~= nil then
    M.status = "VM changes not checked: VM or worktree details are unavailable."
  elseif M.target == "ssh" then
    M.status = "Remote SSH checkout is not a usable local repository."
  elseif M.cwd then
    W.start()
  else
    M.status = "No working directory was reported; repository workflow unavailable."
  end
end

zellij.on("key", function(event)
  if W.active then return true end
  if event.key == "Enter" and M.helper_owner then
    zellij.focus_plugin_pane(M.helper_owner, false, false)
    return true
  elseif event.key == "Enter" and M.syncing then
    return true
  elseif event.key == "Enter" then
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
  elseif event.key == "r" and M.sync_warnings and not M.syncing then
    W.active = true
    zellij.task(function()
      local answer = choose("Resolve sync conflicts before continuing", { y = "I resolved these conflicts", Enter = "back" }, M.sync_warnings)
      W.active = false
      if answer == "y" then
        M.sync_warnings = nil
        M.syncing = true
        run("recheck", { M.zellij, "firecracker", "changes", M.vm, "--session", M.session })
      end
    end)
    return true
  elseif event.key == "s" and M.changes and not M.syncing then
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
    local count = output(stdout)
    local changes = exit_code == 0 and count:match("^%d+$") and tonumber(count) or nil
    if changes and math.tointeger(changes) then
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
      M.changes = nil
      M.status = "VM changes could not be checked. See plugin log for details."
      log_failure(step, exit_code, stdout, stderr)
    end
    if step == "recheck" then
      M.syncing = false
      if M.changes == 0 then
        if M.sync_warnings then M.status = "Sync conflicts require resolution. Press R to review and confirm."
        else W.start() end
      end
    end
    return true
  elseif step == "sync" then
    if exit_code == 0 then
      local warnings = output(stderr):gsub("%s*\n%s*", " ")
      if warnings ~= "" then M.sync_warnings = (M.sync_warnings or "") .. "\n" .. output(stderr) end
      M.status = warnings == "" and "VM worktree synced." or "VM worktree synced. " .. warnings
      run("recheck", { M.zellij, "firecracker", "changes", M.vm, "--session", M.session })
    else
      M.syncing = false
      M.status = "VM worktree could not be synced. See plugin log for details."
      log_failure(step, exit_code, stdout, stderr)
    end
    return true
  end
end)

function M.render(rows, cols)
  if W.active then return W.render(rows, cols) end
  local middle = math.floor(rows / 2)
  local lines = {}
  for _ = 1, math.max(middle - 2, 0) do
    table.insert(lines, "")
  end
  local question = M.changes and M.changes > 0 and "Agent exited. What next?" or QUESTION
  table.insert(lines, "\27[1m" .. centered(question, cols) .. "\27[0m")
  table.insert(lines, "")
  table.insert(lines, centered("Directory: " .. (M.cwd or
    (M.worktree and M.worktree .. " (host worktree)") or "unavailable"), cols))
  table.insert(lines, centered(zellij.text.truncate(M.status or "", cols), cols))
  table.insert(lines, "")
  local controls = M.helper_owner and "<ENTER> return to workflow  <ESC> close helper" or CONTROLS
  if M.changes then
    controls = ("<ENTER> restart  <S> sync %d  <ESC> close pane"):format(M.changes)
  end
  if M.sync_warnings then controls = controls .. "  <R> resolve sync conflicts" end
  table.insert(lines, "\27[1m" .. centered(controls, cols) .. "\27[0m")
  return table.concat(lines, "\n")
end

return M
