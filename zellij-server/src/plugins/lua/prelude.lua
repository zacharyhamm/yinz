-- The Lua side of zellij's plugin API.
--
-- Rust exposes exactly one function to a plugin - `__dispatch(command_name, args)` - mirroring
-- the single `host_run_plugin_command` import the wasm runtime exposes. Everything a plugin
-- author actually touches is built here, in Lua, so that the ergonomics can grow without
-- widening the Rust boundary or recompiling zellij.

local zellij = {}

local interpreter_id = __zellij_interpreter_id
zellij.instance_id = interpreter_id
__zellij_interpreter_id = nil

-- The host stops a cancelled plugin by raising this error from an instruction-count hook. It is
-- an ordinary Lua error, so `pcall` would catch it and a loop around a `pcall` would run on
-- forever. These wrappers hand every other error back exactly as the real functions do, and
-- re-raise this one, so it unwinds all the way out of the entrypoint however deeply it was
-- caught. `coroutine.wrap` already propagates; only `resume` reports errors as values.
--
-- `xpcall` is the odd one out: its message handler runs *before* the error comes back, and may
-- replace it (`function(_) return "failed" end`), so the marker cannot be looked for in the
-- result. The handler is wrapped instead, and a cancel is noted there and never shown to the
-- plugin's own handler.
do
  local cancelled_by_host = __zellij_cancelled_by_host
  __zellij_cancelled_by_host = nil
  local pack, unpack = table.pack, table.unpack
  local raw_pcall, raw_xpcall, raw_resume = pcall, xpcall, coroutine.resume

  local function is_cancel(err)
    return err ~= nil and string.find(tostring(err), cancelled_by_host, 1, true) ~= nil
  end

  local function rethrowing(raw)
    return function(...)
      local results = pack(raw(...))
      if not results[1] and is_cancel(results[2]) then
        error(results[2], 0)
      end
      return unpack(results, 1, results.n)
    end
  end

  pcall = rethrowing(raw_pcall)
  coroutine.resume = rethrowing(raw_resume)

  local xpcall_rethrowing = rethrowing(raw_xpcall)
  xpcall = function(f, handler, ...)
    if type(handler) ~= "function" then
      -- Lua's own "bad argument #2" error, unchanged.
      return xpcall_rethrowing(f, handler, ...)
    end
    local cancelled = false
    local function guard(err)
      if is_cancel(err) then
        cancelled = true
        return err
      end
      return handler(err)
    end
    local results = pack(raw_xpcall(f, guard, ...))
    -- `cancelled` is the signal; the marker check covers a cancel raised where the handler did
    -- not run (Lua reports an error inside the handler itself without calling it again).
    if cancelled or (not results[1] and is_cancel(results[2])) then
      error(cancelled_by_host, 0)
    end
    return unpack(results, 1, results.n)
  end
end

--- The host installs `zellij.__dispatch` for the duration of each call into the plugin and
--- removes it afterwards, so it is looked up per call rather than captured once here.
---
--- Names are pascal-cased on the way through, so the host only ever sees a command spelled the
--- way `PluginCommand` spells it. Doing it here rather than in Rust keeps the case rule in one
--- place: the host hands the name straight to serde, which uses the variant's own spelling.
--- The third argument is the spelling the plugin used, which the host only puts in error
--- messages - so a typo is reported back as it was written rather than pascal-cased.
local tracing = false

--- Log what this plugin does: every command it dispatches, every event and pipe it is handed,
--- every task it starts and finishes - one line each, through `zellij.log`, so `zellij plugins
--- log` (or hocus's `plugin_log`) reads as a transcript. Off by default; the plugin
--- configuration key `trace "true"` turns it on without a code change.
function zellij.trace(on)
  tracing = on ~= false
end

local function trace(line)
  if tracing and zellij.log then
    zellij.log(line)
  end
end

--- A value as one short line, for a trace.
local function compact(value)
  if value == nil then
    return ""
  end
  local ok, text = pcall(zellij.json.encode, value)
  if not ok then
    text = tostring(value)
  end
  if #text > 200 then
    text = text:sub(1, 197) .. "..."
  end
  return text
end

local function dispatch(name, args)
  if not zellij.__dispatch then
    error("zellij commands can only be called from inside a plugin entrypoint", 2)
  end
  if tracing then
    trace("-> " .. name .. " " .. compact(args))
  end
  return zellij.__dispatch(zellij.pascal_case(name), args, name)
end

--- Bytes as the host sends them - a list of numbers, because that is what `Vec<u8>` is to serde
--- - back into the string they were. Chunked, because `string.char(table.unpack(t))` puts every
--- byte on the Lua stack at once and a command's output can be megabytes.
function zellij.bytes_to_string(bytes)
  if type(bytes) ~= "table" then
    return bytes
  end
  local parts = {}
  local n = #bytes
  for i = 1, n, 4096 do
    parts[#parts + 1] = string.char(table.unpack(bytes, i, math.min(i + 4095, n)))
  end
  return table.concat(parts)
end

--- A pane id as the host spells it - `{ Terminal = 3 }` - as one string, `"terminal:3"`, for
--- use as a table key. Two tables are never equal in Lua, so a map the host keys by pane id
--- cannot be looked up until its keys are strings.
function zellij.pane_key(pane_id)
  if type(pane_id) ~= "table" then
    return tostring(pane_id)
  end
  local kind, id = next(pane_id)
  return string.lower(tostring(kind)) .. ":" .. tostring(id)
end

--- The names of the positional fields of the events that have them.
---
--- These are tuple variants in Rust, which serde turns into bare lists: `RunCommandResult`
--- arrives as `{ 0, {...}, {...}, {...} }` and nothing says which is the exit code. Named here
--- once, so a plugin reads `event.exit_code` rather than `event.value[1]`. `value` keeps the
--- list, so nothing that was reachable stops being so.
local event_fields = {
  run_command_result = { "exit_code", "stdout", "stderr", "context" },
  web_request_result = { "status", "headers", "body", "context" },
  session_update = { "sessions", "resurrectable_sessions" },
  custom_message = { "message", "payload" },
  command_pane_opened = { "pane_id", "context" },
  command_pane_exited = { "pane_id", "exit_code", "context" },
  command_pane_re_run = { "pane_id", "context" },
  edit_pane_opened = { "pane_id", "context" },
  edit_pane_exited = { "pane_id", "exit_code", "context" },
  user_action = { "action", "client_id", "terminal_id", "cli_client_id" },
  action_complete = { "action", "pane_id", "context" },
  cwd_changed = { "pane_id", "cwd", "focused_client_ids" },
  command_changed = { "pane_id", "command", "is_foreground", "focused_client_ids" },
  available_layout_info = { "layouts", "errors" },
}

--- The fields above that are bytes, and want to be strings.
local byte_fields = {
  run_command_result = { "stdout", "stderr" },
  run_command_output = { "data" },
  pane_output = { "data" },
  web_request_result = { "body" },
  web_request_stream_chunk = { "chunk" },
}

--- Turn an event as serde serializes it into the shape plugins are written against.
---
--- `Event` is an externally tagged Rust enum, so it arrives as `{ Key = {...} }` or, for unit
--- variants, as the bare string `"InputReceived"`. Neither is pleasant to match on, so both
--- become a table with a snake_case `name` plus the payload: `{ name = "key", ... }`.
local function normalize_event(event)
  if type(event) == "string" then
    return { name = zellij.snake_case(event) }
  end
  if type(event) ~= "table" then
    return { name = "unknown" }
  end
  local variant, payload = next(event)
  if variant == nil then
    return { name = "unknown" }
  end
  local normalized = { name = zellij.snake_case(variant) }
  -- `value` is always the whole payload, whatever its shape: a number for `Timer`, a list for
  -- `TabUpdate`, a table for `Key`. Keeping this rule unconditional matters because a variant
  -- wrapping a list and a variant with several fields serialize identically, so anything that
  -- tried to unwrap "the single element" would be guessing.
  normalized.value = payload
  local fields = event_fields[normalized.name]
  if fields and type(payload) == "table" then
    -- A positional payload with known names: name them. `nil` in the middle of a serde list
    -- (an absent exit code) shortens `#payload`, so the fields are walked, not the list.
    for index, field in ipairs(fields) do
      normalized[field] = payload[index]
    end
  elseif type(payload) == "table" and #payload == 0 then
    -- A struct-like payload gets its fields merged in, so `Key` can be read as
    -- `event.bare_key` rather than `event.value.bare_key`.
    for k, v in pairs(payload) do
      normalized[k] = v
    end
  end
  for _, field in ipairs(byte_fields[normalized.name] or {}) do
    normalized[field] = zellij.bytes_to_string(normalized[field])
  end
  if normalized.bare_key ~= nil then
    normalized.key = zellij.key_name(normalized.bare_key)
  end
  -- `Mouse` is an enum inside the event: `{ LeftClick = { 3, 4 } }` or `{ ScrollUp = 2 }`.
  -- Read as `event.kind`, `event.line`/`event.col` or `event.lines`.
  if normalized.name == "mouse" and type(payload) == "table" then
    local kind, at = next(payload)
    normalized.kind = zellij.snake_case(kind)
    if type(at) == "table" then
      normalized.line, normalized.col = at[1], at[2]
    else
      normalized.lines = at
    end
  end
  -- The file-system events carry `(path, metadata)` pairs; a pair wants names.
  if normalized.name:find("^file_system_") and type(payload) == "table" then
    local files = {}
    for i, pair in ipairs(payload) do
      files[i] = { path = pair[1], metadata = pair[2] }
    end
    normalized.files = files
  end
  -- A watch's rows, as records - the same shape `zellij.data.fetch` answers with.
  if normalized.name == "plugin_data_changed" then
    normalized.records = zellij.data.records(normalized.rows)
  end
  -- A map keyed by pane id, re-keyed by `zellij.pane_key` so it can be indexed at all.
  if normalized.name == "pane_render_report" and type(payload) == "table" then
    local panes = {}
    for pane_id, contents in pairs(payload) do
      panes[zellij.pane_key(pane_id)] = contents
    end
    normalized.panes = panes
  end
  return normalized
end

--- A readable name for a key.
---
--- `BareKey` is a Rust enum, so a plain key arrives as `{ Char = "t" }` while a named one arrives
--- as the bare string `"Esc"`. Comparing against that directly is miserable, so key events also
--- carry `event.key`, which is `"t"`, `"Esc"`, `"F2"` - what the user would call the key.
function zellij.key_name(bare_key)
  if type(bare_key) == "string" then
    return bare_key
  end
  if type(bare_key) ~= "table" then
    return tostring(bare_key)
  end
  local variant, value = next(bare_key)
  if variant == nil then
    return ""
  end
  if variant == "Char" then
    return tostring(value)
  end
  return variant .. tostring(value)
end

--- `TabUpdate` -> `tab_update`. Exposed because plugins subscribing by name need the same rule.
function zellij.snake_case(name)
  local out = name:gsub("(%l)(%u)", "%1_%2"):gsub("(%u)(%u%l)", "%1_%2")
  return out:lower()
end

--- `tab_update` / `TabUpdate` -> `TabUpdate`, the spelling the host's `EventType` uses.
function zellij.pascal_case(name)
  if not name:find("_") and name:sub(1, 1):match("%u") then
    return name
  end
  return (name:gsub("(%a)([%w]*)", function(first, rest)
    return first:upper() .. rest
  end):gsub("_", ""))
end

-- Host commands ------------------------------------------------------------------------------

--- Ask to be sent the named events. Accepts either spelling: `"key"` or `"Key"`.
function zellij.subscribe(event_types)
  local converted = {}
  for _, event_type in ipairs(event_types) do
    table.insert(converted, zellij.pascal_case(event_type))
  end
  return dispatch("subscribe", converted)
end

--- Ask the user for permission to use privileged commands. Zellij prompts on first request and
--- remembers the answer; until it is granted, the commands it covers are refused.
function zellij.request_permission(permissions)
  local converted = {}
  for _, permission in ipairs(permissions) do
    table.insert(converted, zellij.pascal_case(permission))
  end
  return dispatch("request_plugin_permissions", converted)
end

--- Whether this plugin's pane can hold focus. Non-selectable plugins (status bars and the like)
--- are skipped when the user cycles panes.
function zellij.set_selectable(selectable)
  return dispatch("set_selectable", selectable and true or false)
end

--- Open a new terminal pane, optionally in `cwd`.
---
--- `OpenTerminal` carries a `FileToOpen` but reads the cwd out of its `path`, so the
--- obvious-looking `{ cwd = ... }` would silently open the wrong directory. A bare string is
--- accepted and put where the handler looks.
function zellij.open_terminal(cwd)
  if type(cwd) == "table" then
    cwd = cwd.cwd or cwd.path
  end
  return dispatch("open_terminal", { path = cwd })
end

--- Close this plugin's own pane.
function zellij.close_self()
  return dispatch("close_self")
end

--- Files ---------------------------------------------------------------------------------------
---
--- Sugar over the `fs_*` commands, which the catch-all below already makes reachable as
--- `zellij.fs_read(...)`. The table exists so that a plugin reads `zellij.fs.read`, which is how
--- the same thing is spelled in every other language a plugin author has used.
---
--- Paths are the ones the wasm runtime preopens: `/host` is the plugin's own folder (normally
--- where the session was started), `/data` is per-session scratch, `/cache` outlives the session,
--- and `/tmp` is shared by every plugin. A relative path is read against `/host`. Reaching
--- outside all four needs the `full_hd_access` permission on top of `read_files`/`write_files`.
---
--- Every one of these raises on failure rather than returning nil, so wrap in `pcall` when a
--- missing file is an ordinary outcome rather than a bug.
zellij.fs = {}

--- Read a whole file and return it as a string. `max_bytes` asks for less than the host's own
--- ceiling; it cannot ask for more.
function zellij.fs.read(path, max_bytes)
  return dispatch("fs_read", { path = path, max_bytes = max_bytes })
end

--- The last `lines` lines of zellij's own log about the plugin `name` - what it logged with
--- `zellij.log`, and any loader error naming its file - as one string, the way
--- `zellij plugins log <name>` shows them. The log is outside every root `zellij.fs` reaches,
--- so this is a command of its own. Needs the `author_plugins` permission.
function zellij.read_plugin_log(name, lines)
  return dispatch("read_plugin_log", { name = name, lines = lines or 50 })
end

--- List a folder. Returns an array of `{ path = ..., metadata = ... }`, with each path spelled
--- in the same root that was asked for, so it can be passed straight back to `read`.
function zellij.fs.list(path)
  return dispatch("fs_list", { path = path })
end

--- Metadata for one path (`is_dir`, `is_file`, `is_symlink`, `len`), or nil if nothing is there.
--- This is the way to ask whether a file exists without a `pcall` around `read`.
function zellij.fs.metadata(path)
  return dispatch("fs_metadata", { path = path })
end

--- Write `contents`, replacing the file. `opts.append` adds to it instead, and
--- `opts.create_dirs` makes the folders above it first.
function zellij.fs.write(path, contents, opts)
  opts = opts or {}
  return dispatch("fs_write", {
    path = path,
    contents = contents,
    append = opts.append and true or false,
    create_dirs = opts.create_dirs and true or false,
  })
end

--- Remove a file, or a folder when `recursive` is true.
function zellij.fs.remove(path, recursive)
  return dispatch("fs_remove", { path = path, recursive = recursive and true or false })
end

--- Create a folder. `all` creates the folders above it too, and succeeds if it already exists.
function zellij.fs.mkdir(path, all)
  return dispatch("fs_create_dir", { path = path, all = all and true or false })
end

--- Rename a path. Both ends are confined the same way, so this cannot move a file out of the
--- plugin's folders any more than a write could put one there.
function zellij.fs.rename(from, to)
  return dispatch("fs_rename", { from = from, to = to })
end

--- Call a host command by name, for one this prelude has no named wrapper for yet. It reaches
--- only commands the host knows how to marshal, so an unrecognized name is an error rather than
--- a silent no-op.
function zellij.command(name, args)
  return dispatch(name, args)
end

local default_keybinding_handlers = {}

local function kdl_quote(value)
  if type(value) == "boolean" or type(value) == "number" then return tostring(value) end
  value = tostring(value):gsub("\\", "\\\\"):gsub('"', '\\"')
    :gsub("\n", "\\n"):gsub("\r", "\\r"):gsub("\t", "\\t")
  return '"' .. value .. '"'
end

local function kdl_name(value)
  value = tostring(value)
  if value:match("^[%a_][%w_%-]*$") then return value end
  return kdl_quote(value)
end

--- Describe a Zellij keybinding action. Positional arguments follow the name; a single table is
--- read as KDL properties. Use `{ args = {...}, props = {...}, children = {...} }` for both.
function zellij.action(name, ...)
  local args = { ... }
  if #args == 1 and type(args[1]) == "table" then
    local value = args[1]
    if value.args or value.props or value.children then
      return { name = name, args = value.args or {}, props = value.props or {}, children = value.children }
    end
    return { name = name, args = {}, props = value }
  end
  return { name = name, args = args, props = {} }
end

local function kdl_children(values)
  local keys, out = {}, {}
  for key in pairs(values or {}) do keys[#keys + 1] = key end
  table.sort(keys)
  for _, key in ipairs(keys) do out[#out + 1] = kdl_name(key) .. " " .. kdl_quote(values[key]) end
  return table.concat(out, "; ")
end

local function action_kdl(action)
  if type(action) == "string" then return kdl_name(action) end
  local out = { kdl_name(assert(action.name, "an action needs a name")) }
  for _, arg in ipairs(action.args or {}) do out[#out + 1] = kdl_quote(arg) end
  local keys = {}
  for key in pairs(action.props or {}) do keys[#keys + 1] = key end
  table.sort(keys)
  for _, key in ipairs(keys) do out[#out + 1] = kdl_name(key) .. "=" .. kdl_quote(action.props[key]) end
  if action.children then out[#out + 1] = "{ " .. kdl_children(action.children) .. "; }" end
  return table.concat(out, " ")
end

--- Register fallback bindings owned by this plugin. Each entry has `mode`, `key`, and exactly
--- one of `actions` or `handler`. Calling again replaces the plugin's previous defaults.
function zellij.register_default_keybindings(bindings)
  local modes = {}
  default_keybinding_handlers = {}
  for index, binding in ipairs(bindings or {}) do
    local has_actions, has_handler = binding.actions ~= nil, type(binding.handler) == "function"
    if has_actions == has_handler then
      error("default keybinding " .. index .. " needs exactly one of actions or handler", 2)
    end
    local action_list = {}
    if has_handler then
      local handler_name = "__zellij_default_keybinding:" .. index
      default_keybinding_handlers[handler_name] = binding.handler
      action_list[1] = "MessagePlugin { name " .. kdl_quote(handler_name) .. "; }"
    else
      for _, action in ipairs(binding.actions) do action_list[#action_list + 1] = action_kdl(action) end
    end
    modes[#modes + 1] = kdl_name(assert(binding.mode, "a default keybinding needs a mode"))
      .. " { bind " .. kdl_quote(assert(binding.key, "a default keybinding needs a key"))
      .. " { " .. table.concat(action_list, "; ") .. "; }; }"
  end
  return dispatch("register_default_keybindings", "keybinds clear-defaults=true { "
    .. table.concat(modes, "; ") .. (#modes > 0 and "; " or "") .. "}")
end

--- A pane's identity. `PaneId` is a Rust enum, so it travels as `{ Terminal = 3 }` rather than a
--- bare number, and commands that take one want it in that shape.
function zellij.terminal_pane(id)
  return { Terminal = id }
end

function zellij.plugin_pane(id)
  return { Plugin = id }
end

--- Commands taking raw bytes want a list of byte values, not a string. Lua has no distinction, so
--- the conversion is done here and the plugin gets to pass the string it already has.
local function bytes(text)
  if type(text) ~= "string" then
    return text
  end
  return { text:byte(1, -1) }
end

--- Write bytes to this plugin's focused pane, as if they had been typed.
function zellij.write(text)
  return dispatch("write", bytes(text))
end

--- Write bytes to a specific pane. `pane_id` comes from `zellij.terminal_pane`/`plugin_pane`.
function zellij.write_to_pane_id(text, pane_id)
  return dispatch("write_to_pane_id", { bytes(text), pane_id })
end

--- The argument order of the tuple-shaped commands worth naming.
---
--- Serde sees a tuple variant as a positional list, which reads badly at three arguments of
--- three different meanings. Commands listed here also accept a table of named arguments, which
--- is turned into the positional list the host expects.
local tuple_fields = {
  rename_tab = { "tab_index", "name" },
  rename_tab_with_id = { "tab_id", "name" },
  rename_terminal_pane = { "pane_id", "name" },
  rename_plugin_pane = { "pane_id", "name" },
  focus_terminal_pane = { "pane_id", "should_float_if_hidden", "should_be_in_place_if_hidden" },
  focus_plugin_pane = { "pane_id", "should_float_if_hidden", "should_be_in_place_if_hidden" },
  show_pane_with_id = { "pane_id", "should_float_if_hidden", "should_focus_pane" },
  set_floating_pane_pinned = { "pane_id", "should_be_pinned" },
  set_pane_borderless = { "pane_id", "borderless" },
  reveal_pane_with_id = { "pane_id", "label" },
  -- `click` is `{ message = ..., payload = ... }`, delivered to this plugin as a
  -- `custom_message` event when the toast is clicked. Needs `pane_id` to ride on.
  notify = { "message", "pane_id", "click" },
  cli_pipe_output = { "pipe_name", "output" },
  reconfigure = { "configuration", "write_to_disk" },
  -- Named arguments avoid confusing the three tables. The optional final slot enables chunks;
  -- to_positional also accepts the original five-element spelling.
  run_command = { "command", "env_variables", "cwd", "context", "max_bytes_returned", "stream_output" },
  -- Three arguments of three different meanings, the last of them a nested enum: which of the
  -- three a bare positional table meant is unguessable at the call site.
  open_command_pane_in_git_checkout_near_plugin = { "command", "source_directory", "checkout" },
}

--- Turn `{ tab_index = 0, name = "logs" }` into `{ 0, "logs" }` for the commands above. A table
--- that already looks positional is left alone, so both spellings work.
local function to_positional(name, args)
  if zellij.snake_case(name) == "run_command" and type(args) == "table" and #args == 5 then
    args[6] = zellij.null
  end
  local fields = tuple_fields[zellij.snake_case(name)]
  if not fields or type(args) ~= "table" or args[1] ~= nil then
    return args
  end
  local positional = {}
  for index, field in ipairs(fields) do
    -- Serde counts a tuple variant's elements, so every slot has to be filled: a `nil` would
    -- shorten the list and be refused as the wrong arity, whatever the field's type. `null` is
    -- the one value a Lua table can hold in a gap, and an `Option` field reads it as absent -
    -- which is what a caller leaving an optional argument out means.
    if args[field] == nil then
      positional[index] = zellij.null
    else
      positional[index] = args[field]
    end
  end
  return positional
end

-- Tasks ---------------------------------------------------------------------------------------
---
--- Straight-line code over an event-driven host. A *task* is a coroutine started with
--- `zellij.task(fn)`. Inside it, `zellij.await(aw)` suspends until an *awaitable* settles - a
--- command's result, a pane's exit, an agent's turn, a sleep - and resumes with its value in a
--- later entrypoint, with the host's `__dispatch` installed, so every host command works from a
--- task exactly as it does from `update`:
---
---     zellij.task(function()
---       zellij.await(zellij.async.permissions { "run_commands" })
---       local result = zellij.await(zellij.async.run_command { command = { "cargo", "test" } })
---       if result.exit_code ~= 0 then zellij.notify { message = "red" } end
---     end)
---
--- Awaitables are eager: `zellij.async.run_command` dispatches at once, and the reply is
--- captured even while the task waits on something else. Matching runs before the `zellij.on`
--- handlers and `update`, and consumes nothing - an event a task awaited still reaches both.
--- A task that raises fails the entrypoint that resumed it, as any error does; one that returns
--- a truthy value asks for a render. A reload ends every task: the replies still in flight
--- arrive with tokens nobody holds and are ignored here, though handlers still see them.

local tasks = {} -- coroutine -> task
local task_count = 0
local ready = {} -- { task, value } pairs, resumed FIFO by `drain`
local waiters = {} -- event name -> awaitables registered for it
local pipe_waiters = {} -- pipe name -> awaitables registered for it
local deadlines = {} -- awaitables with a `deadline`, ascending
local armed_until = nil -- the earliest deadline a host timer is known to be in flight for
local async_subscribed = {} -- event name -> true once this layer subscribed to it
local token_count = 0
local permission_queue = {} -- async permission requests, dispatched one at a time
local permission_active = false

local Task = {}
Task.__index = Task

zellij.async = {}

local function remove_from(list, item)
  for i = #list, 1, -1 do
    if list[i] == item then
      table.remove(list, i)
    end
  end
end

local function unregister(aw)
  for _, name in ipairs(aw.events or {}) do
    if waiters[name] then
      remove_from(waiters[name], aw)
    end
  end
  if aw.pipe and pipe_waiters[aw.pipe] then
    remove_from(pipe_waiters[aw.pipe], aw)
  end
  if aw.deadline then
    remove_from(deadlines, aw)
  end
end

--- Call `fn` on every item even when one raises: the rest still run, the first error is raised
--- again afterwards and any later one is logged. `pcall` is the rethrowing wrapper installed at
--- the top of this file, so a host cancel is not held back and still unwinds at once.
local function each_guarded(items, fn)
  local failed, first = false, nil
  for _, item in ipairs(items) do
    local ok, err = pcall(fn, item)
    if not ok then
      if failed then
        if zellij.log then
          zellij.log("error: " .. tostring(err))
        end
      else
        failed, first = true, err
      end
    end
  end
  if failed then
    error(first, 0)
  end
end

--- Settle `aw` with `value`: once, then every listener in the order it was attached. A listener
--- that raises does not stop the ones after it; its error is raised again once all have run.
local function settle(aw, value)
  if aw.done then
    return
  end
  aw.done, aw.value = true, value
  unregister(aw)
  local listeners = aw.on_settle
  aw.on_settle = {}
  each_guarded(listeners, function(listener)
    listener(value)
  end)
end

local function subscribe_once(name)
  if not async_subscribed[name] then
    async_subscribed[name] = true
    zellij.subscribe { name }
  end
end

--- A new awaitable, registered at once. `spec.events` names the events it matches and
--- `spec.match(aw, event)` says whether one settles it and with what; `spec.pipe` names a pipe
--- message instead. Neither means it is settled by hand, through `settle`.
local function awaitable(spec)
  local aw = {
    done = false,
    value = nil,
    on_settle = {},
    events = spec.events,
    pipe = spec.pipe,
    match = spec.match,
  }
  for _, name in ipairs(aw.events or {}) do
    subscribe_once(name)
    waiters[name] = waiters[name] or {}
    table.insert(waiters[name], aw)
  end
  if aw.pipe then
    pipe_waiters[aw.pipe] = pipe_waiters[aw.pipe] or {}
    table.insert(pipe_waiters[aw.pipe], aw)
  end
  return aw
end

--- The primitive: an awaitable settled by hand.
---
---     local aw, resolve = zellij.async.future()
---     wf.on_done = function(result) resolve(result) end
---     local result = zellij.await(aw)
function zellij.async.future()
  local aw = awaitable {}
  return aw, function(value)
    settle(aw, value)
  end
end

--- Resume `task` once. `coroutine.resume` is the rethrowing wrapper installed at the top of
--- this file, so a host cancel raised inside the task unwinds the entrypoint; any other error
--- is re-raised too, so a task cannot fail silently - from `drain`, once the rest of the queue
--- has run.
local function step(task, ...)
  local ok, yielded = coroutine.resume(task.co, ...)
  if not ok then
    tasks[task.co] = nil
    error(yielded, 0)
  end
  if coroutine.status(task.co) == "dead" then
    tasks[task.co] = nil
    trace("~ task#" .. task.id .. " finished")
    if yielded then
      zellij.__should_render = true
    end
    return
  end
  task.awaiting = yielded
  task.listener = function(value)
    task.awaiting, task.listener = nil, nil
    table.insert(ready, { task, value })
  end
  table.insert(yielded.on_settle, task.listener)
end

--- Resume every task whose awaitable settled, in the order they became ready, until none is.
--- A task that raises does not strand any ready task, including those queued by a failing
--- batch: drain the entire queue, then re-raise the first error and log any later ones.
local function drain()
  local failed, first = false, nil
  while #ready > 0 do
    local batch = ready
    ready = {}
    local ok, err = pcall(each_guarded, batch, function(entry)
      if tasks[entry[1].co] then
        step(entry[1], entry[2])
      end
    end)
    if not ok then
      if failed then
        if zellij.log then
          zellij.log("error: " .. tostring(err))
        end
      else
        failed, first = true, err
      end
    end
  end
  if failed then
    error(first, 0)
  end
end

--- Start a task. It runs synchronously up to its first `await`, so an error before that
--- raises from here.
function zellij.task(fn)
  if type(fn) ~= "function" then
    error("zellij.task takes a function", 2)
  end
  task_count = task_count + 1
  local task = setmetatable({ co = coroutine.create(fn), id = task_count }, Task)
  tasks[task.co] = task
  trace("~ task#" .. task.id .. " started")
  step(task)
  return task
end

--- Stop a task where it stands: it is never resumed again, and forgets what it awaited.
function Task:cancel()
  if self.awaiting and self.listener then
    remove_from(self.awaiting.on_settle, self.listener)
  end
  self.awaiting, self.listener = nil, nil
  tasks[self.co] = nil
  if coroutine.status(self.co) == "suspended" then
    coroutine.close(self.co)
  end
end

function Task:is_running()
  return tasks[self.co] ~= nil
end

--- Suspend the calling task until `aw` settles, and return its value; at once if it already
--- has. Only a task can await: an entrypoint has no coroutine to suspend.
function zellij.await(aw)
  local co, is_main = coroutine.running()
  if is_main or not tasks[co] then
    error("zellij.await can only be called from inside a zellij.task", 2)
  end
  if type(aw) ~= "table" or type(aw.on_settle) ~= "table" then
    error("zellij.await needs an awaitable from zellij.async", 2)
  end
  if aw.done then
    return aw.value
  end
  return coroutine.yield(aw)
end

-- Timers. One host timer in flight for the nearest deadline; the host cannot cancel a timer, so a
-- nearer deadline arriving mid-flight arms a second and the first fires later into an ordinary
-- sweep. `os.time` counts whole seconds - the sandbox has no finer clock - so a deadline is rounded
-- up to a whole second. No extra second is added when arming: `os.time` truncates and the host
-- sleeps at least what it was asked, so a deadline `floor(T) + n` armed for `n` seconds is due by
-- the time its timer fires. An extra second would make `every(n)` tick every `n + 1`.

local function arm_nearest()
  local head = deadlines[1]
  if not head then
    armed_until = nil
    return
  end
  if armed_until == nil or head.deadline < armed_until then
    armed_until = head.deadline
    -- Through the field rather than `dispatch`, so a test can stand in for the host.
    zellij.set_timeout(math.max(head.deadline - os.time(), 0))
  end
end

--- Settle every deadline that is due. The next one is armed *before* any listener runs, so a
--- listener that raises - an `after` or `every` callback - cannot stop the clock, and every due
--- deadline is settled before the error is raised again.
local function sweep_deadlines()
  local now = os.time()
  if armed_until and armed_until <= now then
    armed_until = nil
  end
  local due = {}
  while deadlines[1] and deadlines[1].deadline <= now do
    table.insert(due, table.remove(deadlines, 1))
  end
  arm_nearest()
  each_guarded(due, function(aw)
    settle(aw, true)
  end)
end

--- An awaitable that settles in `seconds`, whole seconds, rounded up.
local function at(seconds)
  local aw = awaitable {}
  aw.deadline = os.time() + math.ceil(math.max(tonumber(seconds) or 0, 0))
  local index = #deadlines + 1
  for i, other in ipairs(deadlines) do
    if other.deadline > aw.deadline then
      index = i
      break
    end
  end
  table.insert(deadlines, index, aw)
  subscribe_once("timer")
  arm_nearest()
  return aw
end

--- Resolve in `seconds`: `zellij.await(zellij.async.sleep(2))`.
function zellij.async.sleep(seconds)
  return at(seconds)
end

--- Call `fn` once, in `seconds`; a truthy return asks for a render. The handle's `:cancel()`
--- calls it off.
function zellij.after(seconds, fn)
  local handle = { aw = at(seconds) }
  table.insert(handle.aw.on_settle, function()
    if fn() then
      zellij.__should_render = true
    end
  end)
  function handle:cancel()
    if self.aw and not self.aw.done then
      unregister(self.aw)
      self.aw.done = true
    end
  end
  return handle
end

--- Call `fn` every `seconds` until the handle's `:cancel()`; a truthy return asks for a render.
--- Each call is scheduled after the last one ran, so a slow `fn` never stacks up - and a call
--- that raises is still followed by the next, its error raised again once that is scheduled.
function zellij.every(seconds, fn)
  local handle = { active = true }
  local function schedule()
    handle.aw = at(seconds)
    table.insert(handle.aw.on_settle, function()
      if not handle.active then
        return
      end
      local ok, wants_render = pcall(fn)
      if ok and wants_render then
        zellij.__should_render = true
      end
      if handle.active then
        schedule()
      end
      if not ok then
        error(wants_render, 0)
      end
    end)
  end
  function handle:cancel()
    self.active = false
    if self.aw and not self.aw.done then
      unregister(self.aw)
      self.aw.done = true
    end
  end
  schedule()
  return handle
end

-- The awaitables. Each is a command already dispatched, matched to its reply by a token the
-- reply's `context` carries back, or an event matched by the pane it names.

local function next_token()
  token_count = token_count + 1
  return interpreter_id .. ":a" .. token_count
end

--- `context` with a fresh token in it, and the token.
local function with_token(context)
  local copy = {}
  for key, value in pairs(context or {}) do
    copy[key] = value
  end
  local token = next_token()
  copy.__async = token
  return copy, token
end

local function matches_token(token)
  return function(_, event)
    return event.context ~= nil and event.context.__async == token, event
  end
end

--- The terminal pane id as a number, from a number or a `{ Terminal = n }` table.
local function pane_number(pane_id)
  if type(pane_id) == "table" then
    local _, id = next(pane_id)
    return id
  end
  return pane_id
end

--- Matches the exit of the command run in terminal pane `id` under `token`, or the pane
--- closing: settles with the `command_pane_exited` event, or `{ pane_id, closed = true }`.
local function exit_matcher(id, token)
  return function(_, event)
    if event.name == "command_pane_exited" then
      return event.pane_id == id
          and (token == nil or (event.context ~= nil and event.context.__async == token)),
        event
    end
    local kind, closed = next(event.value or {})
    return kind == "Terminal" and closed == id, { pane_id = id, closed = true }
  end
end

--- The next `name` event for which `match(event)` is truthy - every one, when `match` is nil.
--- Resolves with the event.
function zellij.async.event(name, match)
  return awaitable {
    events = { zellij.snake_case(name) },
    match = function(_, event)
      if match == nil or match(event) then
        return true, event
      end
      return false
    end,
  }
end

--- `zellij.run_command`, awaited: resolves with the `run_command_result` event (`exit_code`,
--- `stdout`, `stderr`, `context`). Takes the same table: `command` (the argv list),
--- `env_variables`, `cwd` (default `/host`), `context`, `max_bytes_returned`. Needs
--- `run_commands`. Optional `on_output(event)` receives correlated stdout/stderr chunks
--- before completion (`event.data`, `event.stderr`). It is registered before dispatch and
--- removed after both streams drain. Output limits only cap the completion result.
function zellij.async.run_command(args)
  local context, token = with_token(args.context)
  local aw = awaitable { events = { "run_command_result" }, match = matches_token(token) }
  if args.on_output then
    subscribe_once("run_command_output")
    local chunks = awaitable { events = { "run_command_output" }, match = function(_, event)
      if event.context and event.context.__async == token then args.on_output(event) end
      return false
    end }
    aw.on_settle[#aw.on_settle+1] = function() unregister(chunks) end
  end
  zellij.run_command {
    command = args.command,
    env_variables = args.env_variables or {},
    cwd = args.cwd or "/host",
    context = context,
    max_bytes_returned = args.max_bytes_returned or zellij.null,
    stream_output = args.on_output and true or args.stream_output or zellij.null,
  }
  return aw
end

--- Builtin-only local Swamp API. Credentials and endpoint selection remain in the host.
function zellij.async.swamp_request(args)
  local _,token=with_token({})
  local id=args.id or token
  local last
  local aw=awaitable{events={'custom_message'},match=function(_,event)
    if event.message~='swamp-response' then return false end
    local frame=zellij.json.decode(event.payload)
    if frame.id~=id then return false end
    if frame.type=='error' then return true,{error=frame.error} end
    if frame.type=='event' then
      last=frame.event
      if args.started and last.runId then return true,{runId=last.runId} end
      return false
    end
    if frame.type=='run.attached' then
      if args.started then return true,{runId=frame.payload.runId} end
      return false
    end
    if frame.type=='done' then return true,last or {} end
    if frame.type=='run.interrupted' or frame.type=='run.elsewhere' then
      return true,{error={message='Run stream is unavailable; reconcile persisted history'}}
    end
    return true,frame.payload and (frame.payload.data or frame.payload) or frame
  end}
  zellij.swamp_request{directory=args.directory,id=id,operation=args.operation,payload=zellij.json.encode(args.payload or {})}
  return aw
end

--- An HTTP request: `{ url, verb = "Get", headers, body, context, stream_id, proxy }`, the
--- reply arriving as a `web_request_result` event (or, with a `stream_id`, as
--- `web_request_stream_chunk` events). `body` is a string. `WebRequest` is a seven-element
--- tuple with the two optional ones in the middle, so this wrapper is what makes the absent
--- ones absent. Needs `web_access`.
function zellij.web_request(args)
  if type(args) ~= "table" then
    error("zellij.web_request takes a table: { url = ..., verb = \"Post\", body = ... }", 2)
  end
  if args[1] ~= nil then
    -- Positional, as the tuple is spelled: leave it to the author.
    return dispatch("web_request", args)
  end
  return dispatch("web_request", {
    args.url,
    args.verb or "Get",
    args.headers or {},
    bytes(args.body or ""),
    args.context or {},
    args.stream_id == nil and zellij.null or args.stream_id,
    args.proxy == nil and zellij.null or args.proxy,
  })
end

--- `zellij.web_request`, awaited: resolves with the `web_request_result` event (`status`,
--- `headers`, `body`, `context`). Takes `url`, `verb` (default `Get`), `headers`, `body`,
--- `context`. Needs `web_access`.
function zellij.async.web_request(args)
  local context, token = with_token(args.context)
  local aw = awaitable { events = { "web_request_result" }, match = matches_token(token) }
  zellij.web_request {
    url = args.url,
    verb = args.verb,
    headers = args.headers,
    body = args.body,
    context = context,
  }
  return aw
end

--- A pane's command exiting, or the pane closing: resolves with the `command_pane_exited` event
--- (`pane_id`, `exit_code`, `context`), or `{ pane_id, closed = true }`. Only for a pane this
--- plugin opened - that is who the host tells - and it needs `read_application_state`.
function zellij.async.pane_exit(pane_id)
  local id = pane_number(pane_id)
  return awaitable {
    events = { "command_pane_exited", "pane_closed" },
    match = exit_matcher(id, nil),
  }
end

--- Run a command in an existing command pane and await its exit, `run_command_in_pane` and
--- `pane_exit` in one: `step` is a CommandToRun (`path`, `args`, `cwd`, ...) plus
--- `keep_scrollback` and `context`. Resolves as `pane_exit` does, for this run only.
function zellij.async.run_in_pane(pane_id, step)
  local id = pane_number(pane_id)
  local context, token = with_token(step.context)
  local aw = awaitable {
    events = { "command_pane_exited", "pane_closed" },
    match = exit_matcher(id, token),
  }
  aw.token = token
  zellij.run_command_in_pane {
    pane_id = id,
    command = step,
    context = context,
    keep_scrollback = step.keep_scrollback ~= false,
  }
  return aw
end

--- A pane's agent record reaching one of `states` (`"idle"`, or a list), read off
--- `pane_update`; resolves with the `PaneInfo`. `opts.after` names the states the pane must be
--- seen in first - `{ after = { "busy", "waiting" } }` waits for the *edge* into idle rather
--- than a pane that is idle already. Needs `read_application_state`.
function zellij.async.agent_state(pane_id, states, opts)
  local id = pane_number(pane_id)
  if type(states) == "string" then
    states = { states }
  end
  local wanted, after = {}, nil
  for _, state in ipairs(states) do
    wanted[string.lower(state)] = true
  end
  if opts and opts.after then
    after = {}
    for _, state in ipairs(opts.after) do
      after[string.lower(state)] = true
    end
  end
  local seen_after = after == nil
  return awaitable {
    events = { "pane_update" },
    match = function(_, event)
      for _, panes in pairs(event.panes or {}) do
        for _, pane in ipairs(panes) do
          if not pane.is_plugin and pane.id == id and pane.agent then
            local state = string.lower(tostring(pane.agent.state))
            if seen_after and wanted[state] then
              return true, pane
            end
            if after and after[state] then
              seen_after = true
            end
          end
        end
      end
      return false
    end,
  }
end

--- The next pipe message named `name`; resolves with the message.
function zellij.async.pipe(name)
  return awaitable { pipe = name }
end

--- Ask for permissions and await the answer: resolves with `"Granted"` or `"Denied"`. The
--- way to write `load`:
---
---     function M.load()
---       zellij.task(function()
---         zellij.await(zellij.async.permissions { "run_commands" })
---         -- act
---       end)
---     end
---
--- Requests are dispatched one at a time in FIFO order because answers carry no request id. A
--- permission granted before - by the user, or by hocus when it approved the source - is in force
--- from the request on, and the answer still arrives on a later event.
local function dispatch_permission()
  if permission_active or #permission_queue == 0 then
    return
  end
  permission_active = true
  local request = table.remove(permission_queue, 1)
  local aw, permissions = request[1], request[2]
  aw.events = { "permission_request_result" }
  aw.match = function(_, event) return true, event.value end
  subscribe_once("permission_request_result")
  waiters.permission_request_result = waiters.permission_request_result or {}
  table.insert(waiters.permission_request_result, aw)
  table.insert(aw.on_settle, function()
    permission_active = false
    dispatch_permission()
  end)
  zellij.request_permission(permissions)
end

function zellij.async.permissions(permissions)
  local aw = awaitable {}
  table.insert(permission_queue, { aw, permissions })
  dispatch_permission()
  return aw
end

--- Every awaitable in `list` (a list or a map): resolves with their values in the same shape.
function zellij.async.all(list)
  local aw, resolve = zellij.async.future()
  local pending, results, waiting = 0, {}, {}
  -- Count first, listen second: a child that is already settled must not be able to resolve
  -- the aggregate before the ones after it in `pairs` order have been counted.
  for key, child in pairs(list) do
    if child.done then
      results[key] = child.value
    else
      pending = pending + 1
      waiting[key] = child
    end
  end
  for key, child in pairs(waiting) do
    table.insert(child.on_settle, function(value)
      results[key] = value
      pending = pending - 1
      if pending == 0 then
        resolve(results)
      end
    end)
  end
  if pending == 0 then
    resolve(results)
  end
  return aw
end

--- The first of `list` to settle: resolves with `{ key = k, value = v }`. The others stay
--- registered but this race stops listening to them, so an awaitable can be raced again,
--- which is how a timeout is retried.
function zellij.async.race(list)
  local aw, resolve = zellij.async.future()
  local listeners = {}
  local function won(key, value)
    if aw.done then
      return
    end
    for _, listener in ipairs(listeners) do
      remove_from(listener[1].on_settle, listener[2])
    end
    listeners = {}
    resolve { key = key, value = value }
  end
  for key, child in pairs(list) do
    if aw.done then
      break
    end
    if child.done then
      won(key, child.value)
    else
      local listener = function(value) won(key, value) end
      table.insert(listeners, { child, listener })
      table.insert(child.on_settle, listener)
    end
  end
  return aw
end

--- Readers of every event. `event_intercepts` run before the waiters, for the sections below
--- that keep a table current off the event stream; `event_followers` run after the tasks, for
--- listeners a plugin registered, which hear the event the way a `zellij.on` handler does.
local event_intercepts = {}
local event_followers = {}

--- Called from `__update` before the handlers: settle what the event settles, then resume.
---
--- Every stage runs even when an earlier one raises, and the error is raised again only after
--- the last: a callback that breaks on the tick that also wakes a sleeper has already queued
--- that sleeper in `ready`, and with no later event nothing else would ever drain it.
local function deliver(event)
  each_guarded({
    function()
      for _, intercept in ipairs(event_intercepts) do
        intercept(event)
      end
    end,
    function()
      local list = waiters[event.name]
      if list then
        each_guarded({ table.unpack(list) }, function(aw)
          if not aw.done then
            local matched, value = aw.match(aw, event)
            if matched then
              settle(aw, value)
            end
          end
        end)
      end
    end,
    function()
      if event.name == "timer" then
        sweep_deadlines()
      end
    end,
    drain,
    function()
      for _, follower in ipairs(event_followers) do
        follower(event)
      end
    end,
    drain,
  }, function(stage)
    stage()
  end)
end

--- Called from `__pipe`, the same way. Returns true when the message was one of this layer's
--- own (an rpc call or reply), which the plugin's `pipe` is then not shown.
local pipe_intercepts = {}

local function deliver_pipe(message)
  local intercepted = false
  each_guarded({
    function()
      for _, intercept in ipairs(pipe_intercepts) do
        if intercept(message) then
          intercepted = true
        end
      end
    end,
    function()
      each_guarded({ table.unpack(pipe_waiters[message.name] or {}) }, function(aw)
        settle(aw, message)
      end)
    end,
    drain,
  }, function(stage)
    stage()
  end)
  return intercepted
end

-- Workflow settings at the Git worktree root. These calls only read/write data.
zellij.project_config = {}
function zellij.project_config.read(directory)
  return dispatch("project_config_read", { directory = directory })
end
function zellij.project_config.write(directory, source, expected)
  return dispatch("project_config_write", { directory = directory, source = source, expected = expected })
end

-- Agents -------------------------------------------------------------------------------------
---
--- A real table rather than a manufactured dispatcher, so `zellij.agent.spawn` and its siblings
--- can take the flat, author-shaped table the guide shows and reshape it for the host.
zellij.agent = {}

--- The fields of `AgentSpawnSpec`, in `zellij.agent.spawn`'s flat table. Everything else the
--- author writes (`source_directory`, `cwd`, `name`, `context`) is the command's own.
local SPAWN_SPEC_FIELDS = {
  "kind", "command", "args", "model", "effort", "prompt", "branch", "base", "worktree", "worktree_dir",
  "env", "permission_mode", "resume", "hooks", "firecracker", "firecracker_image", "reuse",
  "allow_outside_worktree",
}

--- Start a coding agent in a command pane near this plugin:
---
---   local pane = zellij.agent.spawn { kind = "claude", prompt = "fix the parser", branch = "fix-42" }
---
--- The launch is planned the way hocus plans its own - per-kind flags, `permission_mode`,
--- `resume`, the inline Claude Code hooks (`hooks = false` to leave them out) - and the pane is
--- recorded as an agent pane, so its state rides on `pane_update` and its exit fires
--- `agent_exit`. `branch` (with `base`, default `main`) makes a worktree named after it in
--- `worktree_dir` (default `.worktrees/`), or the branch in place with `worktree = false`; a
--- name already taken is cut afresh as `<branch>-2` unless `reuse = true` says the existing
--- branch or worktree is the work meant. An agent that would start anywhere but a linked
--- worktree - the primary checkout, or no repository at all - is refused unless
--- `allow_outside_worktree = true`;
--- `source_directory` is the repository that happens in, as a plugin path (`/host` by default);
--- `cwd` is where the pane runs when no branch is named. `firecracker` names a session VM to
--- run in, `firecracker_image` its image; with `automatic_firecracker_vm = true` the host names
--- the VM after the folder the pane runs in and mirrors that folder into it instead, so agents
--- on one checkout share one VM. `name` titles the pane and `context` comes back on
--- `command_pane_exited`. Answers with the pane id and raises on failure. Needs
--- `run_commands`.
function zellij.agent.spawn_details(opts)
  if type(opts) ~= "table" then
    error("zellij.agent.spawn takes a table: { kind = \"claude\", prompt = ... }", 2)
  end
  local spec = {}
  for _, field in ipairs(SPAWN_SPEC_FIELDS) do
    spec[field] = opts[field]
  end
  return dispatch("spawn_agent", {
    spec = spec,
    source_directory = opts.source_directory,
    cwd = opts.cwd,
    name = opts.name,
    context = opts.context or {},
    automatic_firecracker_vm = opts.automatic_firecracker_vm or false,
    start_suppressed = opts.start_suppressed or false,
  })
end

--- Full host result: pane_id, actual branch, and actual checkout cwd.
--- Existing callers keep receiving a PaneId from spawn.
function zellij.agent.spawn(opts)
  return zellij.agent.spawn_details(opts).pane_id
end

-- The rest of `zellij.agent`: observing and steering an agent that is running in a pane.
--
-- State comes off `pane_update` (the pane's `agent` record: `kind`, `state`, `detail`, ...)
-- and words come off `agent_event` (one event per hook the agent fired, read by the session:
-- `pane_id`, `kind`, `hook`, `state`, `detail`, `prompt`, `last_message`, `tool_name`, `cwd`,
-- `session_id`). Both need `read_application_state`.

local agent_records = {} -- terminal pane id -> the pane's agent record, or nil
local agent_records_watched = false
local agent_listeners = {}

local function watch_agent_records()
  if not agent_records_watched then
    agent_records_watched = true
    subscribe_once("pane_update")
  end
end

--- Keep `agent_records` current; called from `deliver` for every `pane_update`, whether the
--- plugin subscribed to it itself or `watch_agent_records` did: a manifest that has already
--- flowed is the answer to the first `info()`, not something to wait for again. Rebuilt in
--- place from the whole manifest, so a pane that closed is gone from it - and so a table a
--- caller took from `all()` stays the one that is kept current.
local function record_agents(event)
  for id in pairs(agent_records) do
    agent_records[id] = nil
  end
  for _, panes in pairs(event.panes or {}) do
    for _, pane in ipairs(panes) do
      if not pane.is_plugin then
        agent_records[pane.id] = pane.agent
      end
    end
  end
end

--- The agent record the manifest last showed for `pane_id`, or nil: `{ kind, state, source,
--- detail, cwd, session_id, run_id }` with `state` one of `Busy`, `Waiting`, `Idle`, `Exited`.
--- Nil until a `pane_update` has arrived - the plugin's own subscription counts, and the first
--- call subscribes otherwise - and nil again once the pane has closed.
function zellij.agent.info(pane_id)
  watch_agent_records()
  return agent_records[pane_number(pane_id)]
end

--- Every agent record the manifest last showed, keyed by terminal pane id.
function zellij.agent.all()
  watch_agent_records()
  return agent_records
end

--- Hear every `agent_event`, already typed: `fn(event)` with `event.pane_id`, `event.kind`,
--- `event.hook` (`UserPromptSubmit`, `Stop`, ...), `event.state`, `event.prompt`,
--- `event.last_message`, `event.tool_name`, `event.detail`, `event.cwd`, `event.session_id`.
function zellij.agent.on(fn)
  subscribe_once("agent_event")
  table.insert(agent_listeners, fn)
end

--- Type `text` into the agent's pane as one paste - bracketed, so a multi-line prompt is one
--- prompt - followed by Enter unless `opts.enter == false`. Needs `write_to_stdin`.
function zellij.agent.send(pane_id, text, opts)
  local target = zellij.terminal_pane(pane_number(pane_id))
  zellij.write_to_pane_id("\27[200~" .. tostring(text) .. "\27[201~", target)
  if not (opts and opts.enter == false) then
    zellij.write_to_pane_id("\r", target)
  end
end

--- The next `agent_event` for `pane_id` whose `hook` is `hooks` (a name, or a list; nil for
--- any); resolves with the event.
function zellij.async.agent_event(pane_id, hooks)
  local id = pane_number(pane_id)
  local wanted = nil
  if type(hooks) == "string" then
    hooks = { hooks }
  end
  if hooks then
    wanted = {}
    for _, hook in ipairs(hooks) do
      wanted[hook] = true
    end
  end
  return awaitable {
    events = { "agent_event" },
    match = function(_, event)
      return event.pane_id == id and (wanted == nil or wanted[event.hook] == true), event
    end,
  }
end

--- The hooks that end a turn. `SessionStart` reads as idle on the record too, but it begins a
--- session rather than ending a turn: a spawned agent fires it before it has submitted its
--- opening prompt, so a turn that settled on it would answer before the agent had spoken.
local TURN_ENDING_HOOKS = { Stop = true, StopFailure = true, Interrupt = true }

--- The agent's current turn ending: resolves with `{ message, hook, state, session_id }` on the
--- next `Stop`, `StopFailure` or `Interrupt` for `pane_id`, with `message` the agent's last
--- words, when it left any. For an agent with no hooks to speak through,
--- `opts.fallback = "idle"` also resolves on the pane's record moving from `busy` or `waiting`
--- to `idle`, with `message = nil`.
function zellij.async.turn(pane_id, opts)
  local aw, resolve = zellij.async.future()
  local ended = zellij.async.agent_event(pane_id)
  ended.match = function(_, event)
    return event.pane_id == pane_number(pane_id) and TURN_ENDING_HOOKS[event.hook] == true, event
  end
  table.insert(ended.on_settle, function(event)
    resolve {
      message = event.last_message,
      hook = event.hook,
      state = event.state,
      session_id = event.session_id,
    }
  end)
  if opts and opts.fallback == "idle" then
    local idle = zellij.async.agent_state(pane_id, "idle", { after = { "busy", "waiting" } })
    table.insert(idle.on_settle, function(pane)
      resolve { message = nil, hook = nil, state = "Idle", session_id = pane.agent.session_id }
    end)
  end
  return aw
end

--- Send a prompt and await the turn it starts: `zellij.agent.send` then `zellij.async.turn`,
--- the turn registered first so a fast answer is not missed.
---
---     local reply = zellij.await(zellij.agent.prompt(pane, "run the tests and fix what fails"))
---     zellij.log(reply.message)
function zellij.agent.prompt(pane_id, text, opts)
  local turn = zellij.async.turn(pane_id, opts)
  zellij.agent.send(pane_id, text, opts)
  return turn
end

--- Called from `deliver`: the listeners `zellij.agent.on` registered.
local function deliver_agent_event(event)
  each_guarded(agent_listeners, function(listener)
    if listener(event) then
      zellij.__should_render = true
    end
  end)
end

table.insert(event_intercepts, function(event)
  if event.name == "pane_update" then
    record_agents(event)
  end
end)

table.insert(event_followers, function(event)
  if event.name == "agent_event" then
    deliver_agent_event(event)
  end
end)

-- Plugin-to-plugin calls ---------------------------------------------------------------------
---
--- A request with an answer, over `message_to_plugin`: the caller sends `zellij_rpc_call` with
--- a call id and the callee sends `zellij_rpc_reply` back to the plugin id the message came
--- from. Both sides need `message_and_launch_other_plugins`. The messages never reach either
--- plugin's own `pipe`.
---
---     zellij.rpc.handle("forecast", function(payload) return "clear over " .. payload.city end)
---
---     local reply = zellij.await(zellij.rpc.call {
---       plugin_url = "file:weather.lua", name = "forecast", payload = { city = "Oslo" },
---       timeout = 10,
---     })
---     -- reply.result, or reply.error

zellij.rpc = {}

local RPC_CALL, RPC_REPLY = "zellij_rpc_call", "zellij_rpc_reply"
local rpc_handlers = {}
local rpc_calls = {} -- call id -> resolve
local rpc_count = 0

--- Answer calls named `name`: `fn(payload, call)` returns the result, or raises - the error
--- text travels back as `reply.error`.
function zellij.rpc.handle(name, fn)
  rpc_handlers[name] = fn
end

--- Call `name` on another plugin - by `plugin_url` (launched if it is not running, with
--- `plugin_config`) or by `plugin_id` - with a JSON-encodable `payload`. Resolves with
--- `{ result = ... }` or `{ error = "..." }`; `timeout` (seconds) is the latter too.
function zellij.rpc.call(opts)
  rpc_count = rpc_count + 1
  local call_id = interpreter_id .. ":rpc" .. rpc_count
  local aw, resolve = zellij.async.future()
  rpc_calls[call_id] = resolve
  zellij.message_to_plugin {
    plugin_url = opts.plugin_url,
    destination_plugin_id = opts.plugin_id,
    plugin_config = opts.plugin_config,
    message_name = RPC_CALL,
    message_payload = zellij.json.encode {
      version = 1,
      call_id = call_id,
      name = opts.name,
      payload = opts.payload,
    },
  }
  if opts.timeout then
    table.insert(at(opts.timeout).on_settle, function()
      if rpc_calls[call_id] then
        rpc_calls[call_id] = nil
        resolve { error = "no answer to " .. tostring(opts.name) .. " in " .. opts.timeout .. "s" }
      end
    end)
  end
  return aw
end

table.insert(pipe_intercepts, function(message)
  if message.name == RPC_CALL then
    local ok, call = pcall(zellij.json.decode, message.payload or "")
    if not ok or type(call) ~= "table" or call.version ~= 1 then
      return true
    end
    call.source = message.source -- host provenance, never the sender-supplied JSON field
    local reply = { version = 1, call_id = call.call_id }
    local handler = rpc_handlers[call.name]
    if handler == nil then
      reply.error = "no rpc handler named " .. tostring(call.name)
    end
    local from = type(message.source) == "table" and message.source.Plugin or call.reply_to
    if from ~= nil then
      zellij.task(function()
        if handler ~= nil then
          local handled, result = pcall(handler, call.payload, call)
          if handled then
            reply.result = result
          else
            reply.error = tostring(result)
          end
        end
        zellij.message_to_plugin {
          destination_plugin_id = from,
          message_name = RPC_REPLY,
          message_payload = zellij.json.encode(reply),
        }
      end)
    end
    return true
  elseif message.name == RPC_REPLY then
    local ok, reply = pcall(zellij.json.decode, message.payload or "")
    if ok and type(reply) == "table" and rpc_calls[reply.call_id] then
      local resolve = rpc_calls[reply.call_id]
      rpc_calls[reply.call_id] = nil
      resolve(reply)
    end
    return true
  end
  return false
end)

-- Hocus extensions ---------------------------------------------------------------------------
---
--- A plugin in the `hocus/` folder of the plugin folder can give the in-session model tools.
--- Declare them once, with their handlers, and the prelude does the rest: the registration
--- hocus reads, the re-registration when hocus asks who is running, and each call - a
--- `zellij.rpc` call named `hocus_tool:<name>` - answered with what the handler returns (a
--- string, or anything `zellij.json` can encode) or the error it raised.
---
---     zellij.on("permission_request_result", function()
---       zellij.hocus.tools {
---         forecast = {
---           description = "A short forecast for a city.",
---           parameters = { type = "object", properties = { city = { type = "string" } },
---                          required = { "city" } },
---           safety = "read_only",           -- or "automatic", or "confirm" with a `consequence`
---           handler = function(args) return "Clear skies over " .. args.city end,
---         },
---       }
---     end)
---
--- The file name is the tool namespace: `hocus/weather.lua` makes `weather_forecast`. Needs
--- `message_and_launch_other_plugins`. Registered only when hocus launched the file - it names
--- the extension in the configuration - so a copy reloaded by hand logs that and offers nothing.

zellij.hocus = {}

local hocus_tools = nil
local hocus_launch = nil -- { namespace, path } from the configuration, when hocus launched us
local hocus_said_unlaunched = false

local function hocus_register()
  if not hocus_tools then
    return
  end
  if not hocus_launch then
    if not hocus_said_unlaunched then
      hocus_said_unlaunched = true
      zellij.log("hocus extension: not launched by hocus, so its tools are not registered - "
        .. "ask hocus to reload it (plugin_reload) to pick them up")
    end
    return
  end
  local names = {}
  for name in pairs(hocus_tools) do
    names[#names + 1] = name
  end
  table.sort(names)
  local list = {}
  for _, name in ipairs(names) do
    local tool = hocus_tools[name]
    list[#list + 1] = {
      name = name,
      description = tool.description or name,
      parameters = tool.parameters or { type = "object" },
      safety = tool.safety or "read_only",
      consequence = tool.consequence,
    }
  end
  zellij.message_to_plugin {
    message_name = "hocus_extension_register",
    message_payload = zellij.json.encode {
      version = 1,
      namespace = hocus_launch.namespace,
      path = hocus_launch.path,
      tools = list,
    },
  }
end

--- Declare this extension's tools, `{ name = { description, parameters, safety, consequence,
--- handler } }`, and register them with hocus. Calling it again replaces the set.
function zellij.hocus.tools(tools)
  if type(tools) ~= "table" then
    error("zellij.hocus.tools takes a table of tools keyed by name", 2)
  end
  for name, tool in pairs(tools) do
    if type(tool) ~= "table" or type(tool.handler) ~= "function" then
      error("hocus tool '" .. tostring(name) .. "' needs a `handler` function", 2)
    end
    zellij.rpc.handle("hocus_tool:" .. name, function(payload, call)
      local result = tool.handler(payload or {}, call)
      if type(result) ~= "string" then
        result = zellij.json.encode(result)
      end
      return result
    end)
  end
  hocus_tools = tools
  hocus_register()
end

--- Whether hocus launched this plugin as an extension: the namespace and file it registers as.
function zellij.hocus.launch()
  return hocus_launch
end

table.insert(pipe_intercepts, function(message)
  if message.name == "hocus_extension_discover" then
    -- Hocus reloaded and is asking who is running: say so again.
    hocus_register()
    return true
  end
  return false
end)

--- A one-shot model call: the plugin's own turn, not hocus's.
---
--- Served by hocus-agentd, the half of hocus that is always running, so it needs no hocus
--- pane open and does not contend with the user's conversation. Stateless: no transcript, no
--- hocus tools, no session prompt. Whatever you send is everything the model sees.
---
---     local reply = zellij.await(zellij.hocus.complete {
---       system = "You name panes. One short noun phrase, no punctuation.",
---       messages = { { role = "user", content = screen } },
---       timeout = 60,
---     })
---     if reply.error then
---       zellij.log(reply.error)
---     else
---       zellij.rename_terminal_pane { pane_id = id, name = reply.content }
---     end
---
--- Which means a tool loop is yours to drive: pass `tools`, get `reply.tool_calls` back as a
--- list of `{ id, name, arguments }` - `arguments` the raw JSON string the model wrote - run
--- them yourself, and call again with the assistant's calls and your results appended:
---
---     table.insert(messages, { role = "assistant", tool_calls = reply.tool_calls })
---     table.insert(messages, { role = "tool", tool_call_id = call.id, content = result })
---
--- Answers `{ content, tool_calls, finish_reason, usage, model }` on success and
--- `{ error = "..." }` on anything else - a session with no API key, a rate limit, a timeout.
--- Needs `message_and_launch_other_plugins` and `read_application_state`, and hocus configured; the
--- user can switch it off with `plugin_completions "false"` on hocus's config block.
local HOCUS_COMPLETE, HOCUS_COMPLETE_RESULT = "hocus_complete", "hocus_complete_result"
local hocus_completions = {} -- request id -> { resolve, plugin_id }
local hocus_complete_count = 0

local function completion_list(value)
  if type(value) == "table" and next(value) == nil then
    return zellij.json.decode("[]")
  end
  return value
end

function zellij.hocus.complete(opts)
  hocus_complete_count = hocus_complete_count + 1
  local request_id = interpreter_id .. ":hc" .. hocus_complete_count
  local aw, resolve = zellij.async.future()
  local ok, snapshot = pcall(zellij.get_session_list)
  if not ok or type(snapshot) ~= "table" then
    resolve { error = "cannot discover hocus-agentd: " .. tostring(snapshot) }
    return aw
  end
  local daemon_id
  for _, session in ipairs(snapshot.live_sessions or {}) do
    if session.is_current_session then
      for id, plugin in pairs(session.plugins or {}) do
        if plugin.location == "zellij:hocus-agentd" then
          if daemon_id ~= nil then
            resolve { error = "multiple hocus-agentd plugins in the current session" }
            return aw
          end
          daemon_id = id
        end
      end
    end
  end
  if daemon_id == nil then
    resolve { error = "no hocus-agentd in the current session" }
    return aw
  end
  local messages = completion_list(opts.messages)
  if type(messages) == "table" and next(messages) ~= nil then
    local copied = {}
    for key, message in pairs(messages) do
      copied[key] = message
      if type(message) == "table" and type(message.tool_calls) == "table"
          and next(message.tool_calls) == nil then
        local copy = {}
        for field, value in pairs(message) do copy[field] = value end
        copy.tool_calls = completion_list(message.tool_calls)
        copied[key] = copy
      end
    end
    messages = copied
  end
  local payload = zellij.json.encode {
    request_id = request_id,
    messages = messages,
    system = opts.system,
    model = opts.model,
    tools = completion_list(opts.tools),
    tool_choice = opts.tool_choice,
    temperature = opts.temperature,
    max_tokens = opts.max_tokens,
  }
  if #payload > 64000 then
    resolve { error = "a completion request is at most 64000 bytes, and this one is " .. #payload }
    return aw
  end
  hocus_completions[request_id] = { resolve = resolve, plugin_id = daemon_id }
  -- Address the existing daemon by its host-reported id, without launching or broadcasting.
  zellij.message_to_plugin {
    destination_plugin_id = daemon_id,
    message_name = HOCUS_COMPLETE,
    message_payload = payload,
  }
  -- A daemon can disappear or stop answering after discovery. Always bound the wait.
  local timeout = opts.timeout or 60
  table.insert(at(timeout).on_settle, function()
    if hocus_completions[request_id] then
      hocus_completions[request_id] = nil
      resolve { error = "no answer from hocus-agentd in " .. timeout .. "s" }
    end
  end)
  return aw
end

table.insert(pipe_intercepts, function(message)
  if message.name == HOCUS_COMPLETE then
    -- Reserve the completion wire for the daemon, rather than the plugin's own pipe handler.
    return true
  elseif message.name == HOCUS_COMPLETE_RESULT then
    local ok, reply = pcall(zellij.json.decode, message.payload or "")
    local pending = ok and type(reply) == "table" and hocus_completions[reply.request_id]
    if pending and type(message.source) == "table" and message.source.Plugin == pending.plugin_id then
      hocus_completions[reply.request_id] = nil
      if reply.error == "" then reply.error = nil end
      pending.resolve(reply)
    end
    return true
  end
  return false
end)

--- Called from `__load`: the two configuration keys hocus sets on an extension it launched.
local function note_hocus_launch(config)
  local namespace, path = config and config.hocus_extension_name, config and config.hocus_extension_path
  if namespace and path then
    hocus_launch = { namespace = namespace, path = path }
  end
end

-- Data ---------------------------------------------------------------------------------------
---
--- The plugin data store: a SQLite-backed store zellij keeps for plugins, with entities you
--- declare, rows you write in transactions, fetches with filters and full-text search, and
--- watches that send a `plugin_data_changed` event whenever a fetch's results change. It is
--- what outlives a reload and a session; `zellij.fs` into `/cache` is the other option, for a
--- plugin that just wants a file.
---
---     zellij.request_permission { "plugin_data" }            -- in `load`
---     -- ...and the rest from a `zellij.on("permission_request_result", ...)` handler, since a
---     -- permission is not in force while `load` runs:
---     zellij.data.define { name = "note", version = 1, attrs = { { "text", "text", full_text = true } } }
---     zellij.data.apply(zellij.data.put("note", { text = "hello" }))
---     local notes = zellij.data.fetch { entity = "note", sort = { "id desc" }, limit = 10 }
---     -- notes[1].text == "hello"; notes[1].id, .uid, .created_at, .updated_at come too
---
--- Sugar over the `data_*` commands, which take the Rust shapes: values are an enum
--- (`{ Text = "x" }`), predicates a tree of them, rows a list of lists. Everything here turns
--- plain Lua into those on the way out and back into plain Lua on the way in.
zellij.data = {}

--- The `Null` a mutation can actually carry: `{ done = zellij.data.null }` clears an attribute.
---
--- A sentinel rather than `nil`, because a mutation's values are read with `pairs` and a `nil`
--- has no key to read - and rather than the bare string `"Null"` the store spells it with, which
--- is also a perfectly ordinary `Text` a plugin may want to write.
zellij.data.null = setmetatable({}, { __tostring = function() return "null" end })

--- A Lua value as the store spells it. `nil` and `zellij.data.null` are `Null`; an integer is
--- `Int` and any other number `Real`; `{ blob = "bytes" }` is a `Blob` and `{ json = value }` a
--- `Text` holding JSON, for a `json` attribute. A value already in the store's shape is passed
--- through.
function zellij.data.value(v)
  -- Ahead of the `table` branch below, which would otherwise pass the sentinel through as if it
  -- were a value already in the store's shape.
  if v == nil or v == zellij.data.null then
    return "Null"
  end
  local t = type(v)
  if t == "boolean" then
    return { Bool = v }
  elseif t == "number" then
    if math.type(v) == "integer" then
      return { Int = v }
    end
    return { Real = v }
  elseif t == "string" then
    return { Text = v }
  elseif t == "table" then
    if v.blob ~= nil then
      return { Blob = bytes(v.blob) }
    elseif v.json ~= nil then
      return { Text = zellij.json.encode(v.json) }
    end
    return v
  end
  error("a " .. t .. " cannot be stored", 2)
end

--- The other way: a stored value as a plain Lua value. A blob comes back as a string.
function zellij.data.plain(v)
  if v == nil or v == "Null" then
    return nil
  end
  if type(v) ~= "table" then
    return v
  end
  local kind, inner = next(v)
  if kind == "Blob" then
    return zellij.bytes_to_string(inner)
  end
  return inner
end

--- Rows as the store returns them - `columns` and a list of lists - as a list of records keyed
--- by column name, with plain values. The list is what `fetch` returns.
function zellij.data.records(rows)
  local out = {}
  if type(rows) ~= "table" then
    return out
  end
  for i, row in ipairs(rows.rows or {}) do
    local record = {}
    for j, column in ipairs(rows.columns or {}) do
      record[column] = zellij.data.plain(row[j])
    end
    out[i] = record
  end
  return out
end

local ATTR_KINDS = { text = "Text", int = "Int", real = "Real", bool = "Bool", blob = "Blob", json = "Json" }

--- Declare an entity, creating or migrating its storage: `{ name = , version = , attrs = { ... },
--- namespace = , allow_destructive = }`. Each attribute is `{ name, kind, indexed = , full_text = ,
--- unique = , required = , default = }` - `{ "title", "text" }` at its shortest, and kinds are
--- `text`, `int`, `real`, `bool`, `blob` and `json`. Answers with what the store did.
---
--- `id`, `uid`, `created_at`, `updated_at`, `rowid` and `oid` are on every row already and may
--- not be declared: a define that names one fails whole. `namespace` is omitted for the
--- plugin's own store, or is `"shared:<name>"` for one shared with other plugins, which needs
--- `shared_plugin_data` on top of `plugin_data`; nothing else is a namespace. One plugin should
--- own the define - it is a migration - and the others only fetch, watch and apply.
function zellij.data.define(spec)
  local attrs = {}
  for i, attr in ipairs(spec.attrs or {}) do
    local kind = attr.kind or attr[2] or "text"
    attrs[i] = {
      name = attr.name or attr[1],
      kind = ATTR_KINDS[string.lower(tostring(kind))] or kind,
      indexed = attr.indexed and true or false,
      full_text = attr.full_text and true or false,
      unique = attr.unique and true or false,
      required = attr.required and true or false,
      default = attr.default ~= nil and zellij.data.value(attr.default) or nil,
    }
  end
  return dispatch("data_define_entity", {
    namespace = spec.namespace,
    def = { name = spec.name, version = spec.version or 1, attrs = attrs },
    allow_destructive = spec.allow_destructive and true or false,
  })
end

--- What entities exist and what their columns are: a list of
--- `{ namespace = , entity = , version = , owner = , attrs = { { name = , kind = , indexed = ,
--- full_text = , unique = , required = } } }`, ordered by namespace and entity.
---
--- With no argument it is everything this plugin may see - its own namespace, plus every
--- `shared:` one if it has `shared_plugin_data`. Another plugin's private namespace is never
--- listed. With a namespace it is that one alone.
---
--- This is how to find data another plugin publishes without being told the schema out of band.
--- Note that reading an entity someone else declared needs no `define` of your own: name the
--- namespace and fetch.
---
---     for _, e in ipairs(zellij.data.catalog("shared:hocus")) do
---       zellij.log(e.entity .. ": " .. #e.attrs .. " attributes")
---     end
function zellij.data.catalog(namespace)
  local out = {}
  for i, entity in ipairs(dispatch("data_catalog", { namespace = namespace }) or {}) do
    local def = entity.def or {}
    out[i] = {
      namespace = entity.namespace,
      owner = entity.owner_plugin,
      entity = def.name,
      version = def.version,
      attrs = def.attrs or {},
    }
    for _, attr in ipairs(out[i].attrs) do
      attr.kind = string.lower(tostring(attr.kind))
    end
  end
  return out
end

--- `{ title = "x", done = false }` as the list of pairs a mutation carries, in a fixed order so
--- the same write always looks the same.
local function value_pairs(values)
  local out = {}
  for name, v in pairs(values or {}) do
    out[#out + 1] = { name, zellij.data.value(v) }
  end
  table.sort(out, function(a, b)
    return a[1] < b[1]
  end)
  return out
end

--- Insert a row, or replace the one with `uid` if it exists. A mutation; hand it to `apply`.
---
--- `expect_rev` makes it a compare-and-set: the write applies only if the row is still at that
--- `rev`, and the whole `apply` fails otherwise - with an error `zellij.data.is_conflict` says
--- yes to. Needs a `uid`. `zellij.data.REV_ABSENT` (0) means the row must not exist yet, so the
--- put claims the uid or fails.
function zellij.data.put(entity, values, uid, expect_rev)
  return {
    Put = {
      entity = entity,
      uid = uid,
      values = value_pairs(values),
      expect_rev = expect_rev,
    },
  }
end

--- Set `values` on every row `filter` matches.
---
--- `expect_rev` narrows the match to rows still at that `rev` and fails the `apply` if none are,
--- which with a filter naming one row is a compare-and-set on it. That failure is a conflict,
--- which `zellij.data.is_conflict` tells from the failures that will never work.
function zellij.data.update(entity, filter, values, expect_rev)
  return {
    Update = {
      entity = entity,
      filter = filter,
      values = value_pairs(values),
      expect_rev = expect_rev,
    },
  }
end

--- The `expect_rev` meaning "this row must not exist yet". No stored row is ever at revision 0.
zellij.data.REV_ABSENT = 0

--- The front the host puts on a conflict's message (`response.rs`). Searched for rather than
--- compared against the start of the message: `pcall` hands the message back as it was raised,
--- but the same error seen after a trip through the host wears a prefix of its own.
local DATA_CONFLICT_MARKER = "data conflict: "

--- Did this failed `apply` lose a race, or is it broken? A conflict means the row moved on
--- between the read the value was composed from and the guarded write, so the answer is to read
--- it again and decide what to do with the other writer's version. Every other failure - an
--- entity that was never declared, a malformed filter, a permission that was not granted - will
--- fail the same way however often it is sent, and retrying one loops forever.
---
---     local ok, err = pcall(function()
---       zellij.data.apply(zellij.data.put("note", { title = title }, "n1", note.rev))
---     end)
---     if not ok and zellij.data.is_conflict(err) then
---       -- re-read and decide again
---     elseif not ok then
---       error(err)
---     end
function zellij.data.is_conflict(err)
  return err ~= nil and string.find(tostring(err), DATA_CONFLICT_MARKER, 1, true) ~= nil
end

--- Remove every row `filter` matches.
function zellij.data.delete(entity, filter)
  return { Delete = { entity = entity, filter = filter } }
end

--- Apply one mutation or a list of them in one transaction, all or nothing. Answers with
--- `rows_written`, `rows_deleted`, the `ids` of the rows a `put` made, and `per_mutation` -
--- rows affected by each mutation, in order, which is the only way to tell which one matched.
--- `namespace` is the plugin's own when omitted, or a `"shared:"` one - see `zellij.data.define`.
---
--- A mutation carrying `expect_rev` whose row has moved on fails the whole call, rolling the
--- rest of the batch back: re-read and decide again. That is the one failure worth retrying, so
--- ask `zellij.data.is_conflict` before you do - everything else fails again the same way.
function zellij.data.apply(mutations, namespace)
  if mutations.Put or mutations.Update or mutations.Delete then
    mutations = { mutations }
  end
  return dispatch("data_apply", { namespace = namespace, mutations = mutations })
end

--- Predicates, for `filter`: `zellij.data.eq("done", false)`, `gt`, `gte`, `lt`, `lte`, `ne`,
--- `is_in(attr, list)`, `is_null`, `not_null`, `prefix(attr, s)`, `contains(attr, s)`, and
--- `all(...)`, `any(...)`, `negate(p)` to combine them (`and`, `or` and `not` being keywords).
local function comparison(variant)
  return function(attr, v)
    return { [variant] = { attr, zellij.data.value(v) } }
  end
end

zellij.data.eq = comparison("Eq")
zellij.data.ne = comparison("Ne")
zellij.data.gt = comparison("Gt")
zellij.data.gte = comparison("Gte")
zellij.data.lt = comparison("Lt")
zellij.data.lte = comparison("Lte")

function zellij.data.is_in(attr, list)
  local values = {}
  for i, v in ipairs(list) do
    values[i] = zellij.data.value(v)
  end
  return { In = { attr, values } }
end

function zellij.data.is_null(attr)
  return { IsNull = attr }
end

function zellij.data.not_null(attr)
  return { NotNull = attr }
end

function zellij.data.prefix(attr, s)
  return { Prefix = { attr, s } }
end

function zellij.data.contains(attr, s)
  return { Contains = { attr, s } }
end

function zellij.data.all(...)
  return { And = { ... } }
end

function zellij.data.any(...)
  return { Or = { ... } }
end

function zellij.data.negate(p)
  return { Not = p }
end

--- A fetch, from a plain table (or just the entity name): `{ entity = , filter = , search = ,
--- sort = { "updated_at desc", "title" }, select = { ... }, limit = , offset = , after_id = ,
--- namespace = }`. `namespace` is the plugin's own when omitted, or a `"shared:"` one - see
--- `zellij.data.define`. `fetch`, `count` and `watch` all take one.
local function fetch_request(spec)
  if type(spec) == "string" then
    spec = { entity = spec }
  end
  local sort = {}
  for i, s in ipairs(spec.sort or {}) do
    local name, dir = s, nil
    if type(s) == "table" then
      name, dir = s[1], s[2]
    else
      name, dir = s:match("^(%S+)%s*(%S*)$")
    end
    sort[i] = { name, string.lower(tostring(dir or "")) == "desc" and "Desc" or "Asc" }
  end
  return {
    namespace = spec.namespace,
    entity = spec.entity,
    filter = spec.filter,
    search = spec.search,
    sort = sort,
    select = spec.select or {},
    limit = spec.limit or 100,
    offset = spec.offset or 0,
    after_id = spec.after_id,
  }
end

--- Rows matching a fetch, as records. The second value is the raw answer, whose `truncated`
--- and `next_cursor` say whether there is more and where to continue (`after_id`).
function zellij.data.fetch(spec)
  local rows = dispatch("data_fetch", { request = fetch_request(spec) })
  return zellij.data.records(rows), rows
end

--- How many rows a fetch would match.
function zellij.data.count(spec)
  return dispatch("data_count", { request = fetch_request(spec) })
end

--- Register a standing fetch: `plugin_data_changed` arrives whenever its results change, with
--- `event.watch_id` and `event.records`. Answers with the watch id and the rows right now;
--- `mode = "ids"` asks for only the ids on every change.
function zellij.data.watch(spec, mode)
  local handle = dispatch("data_watch", {
    request = fetch_request(spec),
    mode = string.lower(tostring(mode or "rows")) == "ids" and "Ids" or "Rows",
  })
  return handle.watch_id, zellij.data.records(handle.rows), handle.rows
end

function zellij.data.unwatch(watch_id)
  return dispatch("data_unwatch", { watch_id = watch_id })
end

-- Rendering ----------------------------------------------------------------------------------

--- Text measured the way the terminal draws it.
---
--- `#s` counts bytes and `utf8.len` counts code points; neither is columns, and a box drawn by
--- either is crooked as soon as a name has an accent in it. `zellij.text.width` is installed by
--- the host (it is the same measurement the server uses) and the rest is built on it here.
zellij.text = {}

--- The longest prefix of `s` that fits in `columns`, cut on character boundaries.
---
--- Character by character rather than by byte or by code point: `sub` would cut a multibyte
--- character in half and hand the terminal invalid UTF-8, and a code point is not a column.
local function fit(s, columns)
  local out = ""
  if columns <= 0 then
    return out
  end
  for _, code in utf8.codes(s) do
    local char = utf8.char(code)
    if zellij.text.width(out .. char) > columns then
      break
    end
    out = out .. char
  end
  return out
end

--- `s` cut to at most `width` columns, ending in `ellipsis` (default "…") if it was longer.
---
--- The ellipsis is cut the same way when it is the part that does not fit - one column with the
--- default "…" leaves no room for anything else, and a wider custom one may not fit at all.
function zellij.text.truncate(s, width, ellipsis)
  s = tostring(s)
  if zellij.text.width(s) <= width then
    return s
  end
  ellipsis = ellipsis or "…"
  local room = width - zellij.text.width(ellipsis)
  if room < 0 then
    return fit(ellipsis, width)
  end
  return fit(s, room) .. ellipsis
end

--- `s` padded with spaces on the right to exactly `width` columns, and cut if longer.
function zellij.text.pad(s, width)
  s = zellij.text.truncate(s, width, "")
  return s .. string.rep(" ", width - zellij.text.width(s))
end

--- `s` padded on the left instead.
function zellij.text.pad_left(s, width)
  s = zellij.text.truncate(s, width, "")
  return string.rep(" ", width - zellij.text.width(s)) .. s
end

--- `s` centred in `width` columns.
function zellij.text.center(s, width)
  s = zellij.text.truncate(s, width, "")
  local total = width - zellij.text.width(s)
  local left = math.floor(total / 2)
  return string.rep(" ", left) .. s .. string.rep(" ", total - left)
end

--- UI components ----------------------------------------------------------------------------
---
--- The same components a wasm plugin gets from `zellij-tile`: styled text, ribbons, tables
--- and nested lists, drawn by zellij in the user's theme, which is what makes a plugin look
--- like part of zellij rather than like a script. They travel as DCS escape strings, so a Lua
--- plugin only has to spell them; this is the spelling, mirrored from
--- `zellij-tile/src/ui_components/`.
---
---     local ui = zellij.ui
---     function M.render(rows, cols)
---       return ui.text("hocus"):selected():render()
---         .. "\n" .. ui.table { { "name", "size" }, { "a.lua", ui.text("2k"):success() } }:render()
---     end
---
--- Every component has `render(coords)`, which returns the string to put in the frame. Given
--- `{ x = , y = , width = , height = }` it is drawn there in the pane; without, it is drawn
--- where the cursor is, so components can be joined with "\n" like any other lines.
zellij.ui = {}

local Text = {}
Text.__index = Text

--- The colour levels of `Text`, as `zellij-tile` numbers them. Levels 0-3 are the theme's
--- emphasis colours; the named ones are effects and the two status colours.
local DIM_LEVEL, UNBOLD_LEVEL, ERROR_LEVEL, SUCCESS_LEVEL = 4, 5, 6, 7

--- A piece of styled text. `ui.text("hi")`, then chain: `:selected()`, `:opaque()`,
--- `:disabled()`, `:emphasis(level, from, to)`, `:dim()`, `:error()`, `:success()`.
function zellij.ui.text(s)
  if getmetatable(s) == Text then
    return s
  end
  return setmetatable({ text = tostring(s == nil and "" or s), levels = {} }, Text)
end

--- Length in characters, which is what colour positions count.
function Text:len()
  return utf8.len(self.text) or #self.text
end

function Text:selected()
  self.is_selected = true
  return self
end

function Text:opaque()
  self.is_opaque = true
  return self
end

function Text:disabled()
  self.is_disabled = true
  return self
end

--- Colour the characters `from` up to but not including `to` (0-based, like the Rust API) in
--- theme colour `level` (0-3). With no range, the whole text.
function Text:emphasis(level, from, to)
  from = from or 0
  to = to or self:len()
  local indices = self.levels[level] or {}
  for i = from, to - 1 do
    indices[#indices + 1] = i
  end
  self.levels[level] = indices
  return self
end

function Text:dim(from, to)
  return self:emphasis(DIM_LEVEL, from, to)
end

function Text:unbold(from, to)
  return self:emphasis(UNBOLD_LEVEL, from, to)
end

function Text:error(from, to)
  return self:emphasis(ERROR_LEVEL, from, to)
end

function Text:success(from, to)
  return self:emphasis(SUCCESS_LEVEL, from, to)
end

--- The wire form: flags, then one `$`-terminated group of character indices per colour level
--- (positional, so unused lower levels are empty groups), then the UTF-8 bytes as decimal.
function Text:serialize()
  local out = {}
  if self.is_selected then
    out[#out + 1] = "x"
  end
  if self.is_opaque then
    out[#out + 1] = "z"
  end
  if self.is_disabled then
    out[#out + 1] = "d"
  end
  local top = -1
  for level in pairs(self.levels) do
    if level > top then
      top = level
    end
  end
  for level = 0, top do
    out[#out + 1] = table.concat(self.levels[level] or {}, ",") .. "$"
  end
  out[#out + 1] = table.concat({ self.text:byte(1, -1) }, ",")
  return table.concat(out)
end

--- `x/y/width/height;` for a component drawn at a position, or nothing.
local function coordinates(coords)
  if not coords or coords.x == nil or coords.y == nil then
    return ""
  end
  return string.format(
    "%d/%d/%s/%s;",
    coords.x,
    coords.y,
    coords.width and tostring(coords.width) or "",
    coords.height and tostring(coords.height) or ""
  )
end

local DCS, ST = "\27Pz", "\27\\"

local function component(name, coords, payload)
  return DCS .. name .. ";" .. coordinates(coords) .. payload .. ST
end

function Text:render(coords)
  return component("text", coords, self:serialize())
end

Text.__tostring = Text.render

--- A ribbon: a tab-shaped label, `:selected()` when it is the active one. A row of them is
--- their renders concatenated.
local Ribbon = setmetatable({}, { __index = Text })
Ribbon.__index = Ribbon

function zellij.ui.ribbon(s)
  local text = zellij.ui.text(s)
  return setmetatable({ text = text.text, levels = text.levels }, Ribbon)
end

function Ribbon:render(coords)
  return component("ribbon", coords, self:serialize())
end

Ribbon.__tostring = Ribbon.render

--- A table: a list of rows, each a list of cells, each a string or a `Text`. The first row is
--- the title row. Every row must have as many cells as the first.
local Table = {}
Table.__index = Table

function zellij.ui.table(rows)
  return setmetatable({ rows = rows or {} }, Table)
end

function Table:row(...)
  self.rows[#self.rows + 1] = { ... }
  return self
end

function Table:render(coords)
  local columns = self.rows[1] and #self.rows[1] or 0
  local cells = {}
  for _, row in ipairs(self.rows) do
    for i = 1, columns do
      cells[#cells + 1] = zellij.ui.text(row[i] == nil and "" or row[i]):serialize()
    end
  end
  return component(
    "table",
    coords,
    string.format("%d;%d;%s", columns, #self.rows, table.concat(cells, ";"))
  )
end

Table.__tostring = Table.render

--- A nested list: items are strings or `Text`s, each optionally `{ text, indent = n }`.
--- (`disabled` is not honoured on list items; the host does not read it there.)
local List = {}
List.__index = List

function zellij.ui.nested_list(items)
  return setmetatable({ items = items or {} }, List)
end

function List:item(text, indent)
  self.items[#self.items + 1] = { text, indent = indent }
  return self
end

function List:render(coords)
  local lines = {}
  for _, item in ipairs(self.items) do
    local text, indent = item, 0
    if type(item) == "table" and getmetatable(item) ~= Text and getmetatable(item) ~= Ribbon then
      text, indent = item[1] or item.text, item.indent or 0
    end
    text = zellij.ui.text(text)
    text.is_disabled = nil
    lines[#lines + 1] = string.rep("|", indent) .. text:serialize()
  end
  return component("nested_list", coords, table.concat(lines, ";"))
end

List.__tostring = List.render

--- Request a re-render. Usually unnecessary - returning a truthy value from `update` or `pipe`
--- does the same - but useful from inside a handler registered with `zellij.on`.
function zellij.request_render()
  zellij.__should_render = true
end

-- Event handler sugar ------------------------------------------------------------------------

local handlers = {}

--- Register a handler for one event, as an alternative to writing a big `if` in `update`.
--- Handlers run before the plugin's own `update`. Returning a truthy value from one requests a
--- re-render, as does calling `zellij.request_render()`.
---
---     zellij.on("key", function(event) ... end)
function zellij.on(event_name, handler)
  local name = zellij.snake_case(event_name)
  handlers[name] = handlers[name] or {}
  table.insert(handlers[name], handler)
end

--- Called by the host. Normalizes the event, runs any `zellij.on` handlers, then the plugin's
--- own `update`, and reports whether anything asked for a re-render.
function zellij.__update(plugin, raw_event)
  local event = normalize_event(raw_event)
  zellij.__should_render = false
  local should_render = false
  if tracing then
    trace("<- " .. event.name)
  end
  -- Tasks first: an awaitable is settled and its task resumed before any handler sees the
  -- event, and the event is not consumed - handlers and `update` still see it.
  deliver(event)
  for _, handler in ipairs(handlers[event.name] or {}) do
    if handler(event) then
      should_render = true
    end
  end
  if plugin.update and plugin.update(event) then
    should_render = true
  end
  drain()
  return should_render or zellij.__should_render
end

--- Called by the host. Same shape as `__update`, for pipe messages.
function zellij.__pipe(plugin, message)
  zellij.__should_render = false
  local should_render = false
  if tracing then
    trace("<- pipe " .. tostring(message.name))
  end
  -- An rpc call or reply is this layer's own and is answered here, never shown to the plugin.
  if deliver_pipe(message) then
    drain()
    return zellij.__should_render
  end
  local keybinding_handler = default_keybinding_handlers[message.name]
  if keybinding_handler and keybinding_handler(message) then
    should_render = true
  end
  if plugin.pipe and plugin.pipe(message) then
    should_render = true
  end
  drain()
  return should_render or zellij.__should_render
end

--- Called by the host. `render` may return a string or print nothing at all.
function zellij.__render(plugin, rows, cols)
  if not plugin.render then
    return ""
  end
  local output = plugin.render(rows, cols)
  if output == nil then
    return ""
  end
  return tostring(output)
end

--- Called by the host. Merges the user's configuration over the plugin's defaults before
--- handing it to `load`, the `setup(opts)` convention Lua plugins are usually written against.
function zellij.__load(plugin, config)
  if plugin.defaults then
    local merged = {}
    for k, v in pairs(plugin.defaults) do
      merged[k] = v
    end
    for k, v in pairs(config or {}) do
      merged[k] = v
    end
    config = merged
  end
  if config and (config.trace == "true" or config.trace == true) then
    zellij.trace(true)
  end
  note_hocus_launch(config)
  if plugin.load then
    plugin.load(config)
  end
  drain()
end

-- Pane workflows -----------------------------------------------------------------------------
---
--- Run a sequence of commands in ONE pane: the pane starts the first step, and when a step
--- exits the next replaces it in the same pane, scrolling the finished step's output into the
--- pane's history. A step's result is its child's real exit status - no prompt detection, no
--- sentinel markers - delivered to `on_step`; `on_done` fires once with the whole story.
---
---     local wf = zellij.pane_workflow {
---       name = "sync-skills",          -- pane title
---       floating = true,               -- or a FloatingPaneCoordinates table; default tiled
---       steps = {
---         { path = "ssh", args = { "host", "ls" }, timeout = 30 },
---         { path = "scp", args = { "host:x", "/tmp/x" } },
---       },
---       on_step = function(n, step, exit_code) end,
---       on_done = function(result) end,
---       -- result = { status = "success"|"failed"|"cancelled"|"pane_closed",
---       --            exit_codes = { 0, false, ... }, pane_id = 7, failed_step = 2|nil }
---     }
---     wf.pane_id          -- the terminal pane id, known immediately
---     wf:push(step)       -- append a step while running (works from inside on_step)
---     wf:cancel()         -- SIGINT, then SIGKILL after `kill_grace`; on_done says "cancelled"
---
--- A step is `{ path, args, cwd, remote_host, remote_default_shell, timeout, stdin, name }`.
--- `cwd = "inherit"` uses the pane's last reported working directory when the next step runs on
--- the same host. `stdin` is written to the step's child once it starts - the place for a secret,
--- which must never go in `args` (argv is visible in the pane manifest and in serialized
--- sessions). `timeout = seconds` ends a stuck step with SIGINT, then SIGKILL; leave it out for
--- interactive steps.
---
--- Options: `stop_on_failure` (default true) ends the workflow on the first non-zero exit;
--- `close_on_success` (default false) closes the pane after a fully clean run; `on_cancel =
--- "close"` closes it after a cancel (default leaves it held); `keep_scrollback` (default true)
--- keeps each step's output in the pane's history; `kill_grace` (default 5) is the seconds
--- between SIGINT and SIGKILL.
---
--- Needs `run_commands` to start a step and `read_application_state` to hear that one
--- finished - `command_pane_exited` is gated on it, so without it a workflow stops after its
--- first step. `change_application_state` too if any step has a `timeout`, `cancel` is used, or
--- `close_on_success` / `on_cancel = "close"` is set, and `write_to_stdin` for `stdin`. Between
--- steps the pane is briefly held, so a user pressing ENTER on it can re-run the finished step;
--- the workflow's next step then queues behind that re-run.

--- Run a *different* command in an existing command pane, reusing its pane id - the primitive
--- `pane_workflow` sequences with. `pane_id` is the terminal pane id as a number (or a PaneId
--- table). The `context` round-trips on the resulting `command_pane_exited`. When the pane's
--- current command is still running, the replacement starts when it exits (last write wins).
function zellij.run_command_in_pane(args)
  local pane_id = args.pane_id
  if type(pane_id) == "number" then
    pane_id = { Terminal = pane_id }
  end
  return dispatch("run_command_in_pane", {
    pane_id = pane_id,
    command = args.command,
    context = args.context or {},
    keep_scrollback = args.keep_scrollback or false,
  })
end

local workflows = {} -- terminal pane id (number) -> workflow, for the cwd tracking
local workflow_count = 0
local pane_update_watched = false

local Workflow = {}
Workflow.__index = Workflow

--- The CommandToRun for a step, with `cwd = "inherit"` resolved against what the pane last
--- reported on the same host. Resolved at start time, not enqueue time: the previous step has
--- run by then.
local function step_command(wf, step)
  local cwd = step.cwd
  if cwd == "inherit" then
    cwd = wf.last_host == step.remote_host and wf.last_cwd or nil
  end
  return {
    path = step.path,
    args = step.args,
    cwd = cwd,
    remote_host = step.remote_host,
    remote_default_shell = step.remote_default_shell,
  }
end

--- `cwd = "inherit"` needs the pane manifest; watched lazily so workflows without it never
--- need the `read_application_state` permission.
local function watch_pane_cwds()
  if pane_update_watched then
    return
  end
  pane_update_watched = true
  zellij.subscribe { "pane_update" }
  zellij.on("pane_update", function(event)
    for _, panes in pairs(event.panes or {}) do
      for _, pane in ipairs(panes) do
        local wf = not pane.is_plugin and workflows[pane.id]
        if wf then
          wf.last_cwd = pane.cwd
          wf.last_host = pane.host
        end
      end
    end
  end)
end

local function finish(wf, status)
  workflows[wf.pane_id] = nil
  wf.status = status
  if
    (status == "success" and wf.opts.close_on_success)
    or (status == "cancelled" and wf.opts.on_cancel == "close")
  then
    zellij.close_terminal_pane(wf.pane_id)
  end
  if wf.opts.on_done then
    pcall(wf.opts.on_done, {
      status = status,
      exit_codes = wf.exit_codes,
      pane_id = wf.pane_id,
      failed_step = wf.failed_step,
    })
  end
end

--- The context a step's command carries, which its `command_pane_opened`/`_re_run`/`_exited`
--- events bring back: the workflow and the step, so a stale exit - the user re-running a
--- finished step from the held pane - names an earlier step and is not the workflow advancing.
local function step_context(wf, idx)
  return { __wf = wf.id, __step = tostring(idx) }
end

local function step_matcher(wf, idx, events)
  local step = tostring(idx)
  return function(_, event)
    if event.name == "pane_closed" then
      local kind, closed = next(event.value or {})
      return kind == "Terminal" and closed == wf.pane_id, { pane_id = wf.pane_id, closed = true }
    end
    return event.pane_id == wf.pane_id
        and event.context ~= nil
        and event.context.__wf == wf.id
        and event.context.__step == step,
      event
  end
end

--- The step's process starting: the pane opening for the first step, the command being
--- replaced for a later one.
local function step_started(wf, idx)
  return awaitable {
    events = { "command_pane_opened", "command_pane_re_run" },
    match = step_matcher(wf, idx),
  }
end

--- The step's command exiting, or the pane closing under it.
local function step_exit(wf, idx)
  return awaitable {
    events = { "command_pane_exited", "pane_closed" },
    match = step_matcher(wf, idx),
  }
end

--- Await `exit` within `timeout`: past it, SIGINT, then SIGKILL after `kill_grace` seconds, then
--- giving up after another. Returns the exit event (nil when nothing exited) and whether the
--- step timed out. `wf.abort` settling means a cancel ran out of patience.
local function await_exit(wf, exit, timeout)
  local contenders = { exit = exit, abort = wf.abort }
  if timeout then
    contenders.timeout = zellij.async.sleep(timeout)
  end
  local won = zellij.await(zellij.async.race(contenders))
  if won.key == "exit" then
    return won.value, false
  elseif won.key == "abort" then
    return nil, false
  end
  local target = zellij.terminal_pane(wf.pane_id)
  local grace = math.ceil(wf.opts.kill_grace or 5)
  for _, signal in ipairs { "send_sigint_to_pane_id", "send_sigkill_to_pane_id" } do
    zellij[signal](target)
    local next_won = zellij.await(zellij.async.race {
      exit = exit,
      abort = wf.abort,
      timeout = zellij.async.sleep(grace),
    })
    if next_won.key == "exit" then
      return next_won.value, true
    elseif next_won.key == "abort" then
      return nil, true
    end
  end
  return nil, true
end

--- The workflow's whole life, as one task: each step awaited in turn, the next started in the
--- same pane when one ends.
local function run_steps(wf, exit, started)
  while true do
    local idx = wf.idx
    local step = wf.steps[idx]
    if step.stdin or step.timeout then
      -- Wait for the step's process to start: its stdin goes to that process, and its timeout
      -- counts from then rather than from the moment the previous step ended.
      local first = zellij.await(zellij.async.race { started = started, exit = exit, abort = wf.abort })
      if first.key == "started" and step.stdin then
        zellij.write_chars_to_pane_id(step.stdin, zellij.terminal_pane(wf.pane_id))
      end
    end
    local event, timed_out = await_exit(wf, exit, step.timeout)
    if event and event.closed then
      -- The pane went, and the step with it: nothing exited, so there is no step to report.
      return finish(wf, "pane_closed")
    end
    wf.exit_codes[idx] = event and event.exit_code or false
    if wf.opts.on_step then
      pcall(wf.opts.on_step, idx, step, event and event.exit_code)
    end
    if wf.status == "cancelling" or event == nil and not timed_out then
      return finish(wf, "cancelled")
    elseif timed_out then
      wf.failed_step = idx
      return finish(wf, "failed")
    elseif wf.stop_on_failure and event.exit_code ~= 0 then
      wf.failed_step = idx
      return finish(wf, "failed")
    elseif idx >= #wf.steps then
      return finish(wf, "success")
    end
    wf.idx = idx + 1
    local next_step = wf.steps[wf.idx]
    exit = step_exit(wf, wf.idx)
    started = step_started(wf, wf.idx)
    zellij.run_command_in_pane {
      pane_id = wf.pane_id,
      command = step_command(wf, next_step),
      context = step_context(wf, wf.idx),
      keep_scrollback = wf.opts.keep_scrollback,
    }
  end
end

function Workflow:push(step)
  if self.status ~= "running" and self.status ~= "cancelling" then
    return
  end
  table.insert(self.steps, step)
  if step.cwd == "inherit" then
    watch_pane_cwds()
  end
end

--- SIGINT now, SIGKILL after `kill_grace`, and after another `kill_grace` the workflow ends
--- as cancelled whether or not anything exited; `on_done` says "cancelled".
function Workflow:cancel()
  if self.status ~= "running" then
    return
  end
  local target = zellij.terminal_pane(self.pane_id)
  local grace = math.ceil(self.opts.kill_grace or 5)
  -- The signal first: a refusal (no `change_application_state`) leaves the workflow running.
  zellij.send_sigint_to_pane_id(target)
  self.status = "cancelling"
  zellij.after(grace, function()
    if self.status ~= "cancelling" then
      return
    end
    zellij.send_sigkill_to_pane_id(target)
    zellij.after(grace, function()
      if self.status == "cancelling" then
        self.resolve_abort(true)
      end
    end)
  end)
end

--- Start a workflow. Returns its handle immediately; `handle.pane_id` is already the pane.
function zellij.pane_workflow(opts)
  if type(opts) ~= "table" or type(opts.steps) ~= "table" or #opts.steps == 0 then
    error("pane_workflow: needs a non-empty `steps` list")
  end
  for index, step in ipairs(opts.steps) do
    if type(step) ~= "table" or (not step.path and not step.remote_default_shell) then
      error("pane_workflow: step " .. index .. " needs a `path`")
    end
  end
  workflow_count = workflow_count + 1
  local wf = setmetatable({
    id = "wf" .. workflow_count,
    opts = opts,
    steps = opts.steps,
    stop_on_failure = opts.stop_on_failure ~= false,
    idx = 1,
    exit_codes = {},
    status = "running",
  }, Workflow)
  wf.opts.keep_scrollback = opts.keep_scrollback ~= false
  for _, step in ipairs(wf.steps) do
    if step.cwd == "inherit" then
      watch_pane_cwds()
    end
  end
  wf.abort, wf.resolve_abort = zellij.async.future()

  -- The host can filter startup/exit events while the synchronous pane open is still running.
  for _, event in ipairs { "command_pane_opened", "command_pane_re_run", "command_pane_exited", "pane_closed" } do
    subscribe_once(event)
  end

  -- The first step opens the pane, synchronously, so the pane id is known before this returns.
  local step = wf.steps[1]
  local context = step_context(wf, 1)
  local opened
  if opts.floating then
    local coords = type(opts.floating) == "table" and opts.floating or {}
    opened = zellij.open_command_pane_floating { step_command(wf, step), coords, context }
  else
    opened = zellij.open_command_pane { step_command(wf, step), context }
  end
  local _, id = next(opened or {})
  if not id then
    error("pane_workflow: zellij did not answer with a pane id")
  end
  wf.pane_id = id
  workflows[id] = wf
  if opts.name then
    pcall(zellij.rename_terminal_pane, { pane_id = id, name = opts.name })
  end
  local exit = step_exit(wf, 1)
  local started = step_started(wf, 1)
  wf.task = zellij.task(function()
    run_steps(wf, exit, started)
  end)
  return wf
end

--- Every other host command, without a hand-written wrapper each.
---
--- The wrappers above are real fields and so take precedence; anything else is manufactured on
--- lookup, which is what makes the whole `PluginCommand` surface reachable as `zellij.<command>`.
--- A command called with several arguments passes them as the positional list a tuple-shaped
--- command wants; one called with a single table passes it through.
---
--- Names beginning with an underscore are excluded: `__dispatch` and friends are the host's own,
--- and a manufactured function in their place would be indistinguishable from the real one.
--- Commands a Lua plugin has no use for, and the reason each is refused by name rather than
--- reaching the host to do nothing.
local not_from_lua = {
  post_message_to = "Lua plugins have no workers; a zellij.task or a work queue is the answer",
}

setmetatable(zellij, {
  __index = function(_, name)
    if type(name) ~= "string" or name:sub(1, 1) == "_" then
      return nil
    end
    if not_from_lua[name] then
      return function()
        error("zellij." .. name .. " is not available from Lua: " .. not_from_lua[name], 2)
      end
    end
    return function(...)
      local count = select("#", ...)
      if count == 0 then
        return dispatch(name, nil)
      elseif count == 1 then
        return dispatch(name, to_positional(name, (...)))
      else
        return dispatch(name, { ... })
      end
    end
  end,
})

return zellij
