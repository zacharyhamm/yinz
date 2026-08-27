-- The Lua side of zellij's plugin API.
--
-- Rust exposes exactly one function to a plugin - `__dispatch(command_name, args)` - mirroring
-- the single `host_run_plugin_command` import the wasm runtime exposes. Everything a plugin
-- author actually touches is built here, in Lua, so that the ergonomics can grow without
-- widening the Rust boundary or recompiling zellij.

local zellij = {}

--- The host installs `zellij.__dispatch` for the duration of each call into the plugin and
--- removes it afterwards, so it is looked up per call rather than captured once here.
---
--- Names are pascal-cased on the way through, so the host only ever sees a command spelled the
--- way `PluginCommand` spells it. Doing it here rather than in Rust keeps the case rule in one
--- place: the host hands the name straight to serde, which uses the variant's own spelling.
--- The third argument is the spelling the plugin used, which the host only puts in error
--- messages - so a typo is reported back as it was written rather than pascal-cased.
local function dispatch(name, args)
  if not zellij.__dispatch then
    error("zellij commands can only be called from inside a plugin entrypoint", 2)
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

--- Ask for a timer, counting it as outstanding.
---
--- The host spawns an independent timer per `set_timeout` and has no way to cancel one, and a
--- `timer` event says nothing about which of them fired. Counting is what lets the interval below
--- re-arm exactly once however many timers are in the air: see `__update`.
local function arm(seconds)
  zellij.__timers = (zellij.__timers or 0) + 1
  return dispatch("set_timeout", seconds)
end

--- Ask to be sent a `timer` event in `seconds`. Timers do not repeat; set another one from the
--- handler if you want a ticking clock, or use `set_interval`.
function zellij.set_timeout(seconds)
  return arm(seconds)
end

--- A `timer` event every `seconds`, until `clear_interval`. One interval per plugin: the interval
--- is this one timer being re-armed from the `timer` event, so calling it again changes the
--- period rather than starting a second clock — and a `set_timeout` outstanding alongside it does
--- not fork the chain either, because only the timer that drains the count re-arms.
function zellij.set_interval(seconds)
  zellij.__interval = seconds
  return arm(seconds)
end

function zellij.clear_interval()
  zellij.__interval = nil
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
  notify = { "message", "pane_id" },
  cli_pipe_output = { "pipe_name", "output" },
  reconfigure = { "configuration", "write_to_disk" },
  -- Five arguments of five different meanings, three of them tables: positional is unreadable
  -- here. All five must be given - serde counts a tuple variant's elements, and
  -- `max_bytes_returned` being an `Option` makes it nullable, not absent.
  run_command = { "command", "env_variables", "cwd", "context", "max_bytes_returned" },
}

--- Turn `{ tab_index = 0, name = "logs" }` into `{ 0, "logs" }` for the commands above. A table
--- that already looks positional is left alone, so both spellings work.
local function to_positional(name, args)
  local fields = tuple_fields[zellij.snake_case(name)]
  if not fields or type(args) ~= "table" or args[1] ~= nil then
    return args
  end
  local positional = {}
  local last = 0
  for index, field in ipairs(fields) do
    positional[index] = args[field]
    if args[field] ~= nil then
      last = index
    end
  end
  -- A trailing `nil` would shorten the list serde sees, so only trailing *absent* arguments are
  -- dropped: everything up to the last one the caller named is kept.
  for index = last + 1, #fields do
    positional[index] = nil
  end
  return positional
end

-- Data ---------------------------------------------------------------------------------------
---
--- The plugin data store: a SQLite-backed store zellij keeps for plugins, with entities you
--- declare, rows you write in transactions, fetches with filters and full-text search, and
--- watches that send a `plugin_data_changed` event whenever a fetch's results change. It is
--- what outlives a reload and a session; `zellij.fs` into `/cache` is the other option, for a
--- plugin that just wants a file.
---
---     zellij.request_permission { "plugin_data" }
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
function zellij.data.put(entity, values, uid)
  return { Put = { entity = entity, uid = uid, values = value_pairs(values) } }
end

--- Set `values` on every row `filter` matches.
function zellij.data.update(entity, filter, values)
  return { Update = { entity = entity, filter = filter, values = value_pairs(values) } }
end

--- Remove every row `filter` matches.
function zellij.data.delete(entity, filter)
  return { Delete = { entity = entity, filter = filter } }
end

--- Apply one mutation or a list of them in one transaction, all or nothing. Answers with
--- `rows_written`, `rows_deleted` and the `ids` of the rows a `put` made.
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
--- namespace = }`.
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
  -- Re-armed before the handlers run, so a handler that raises does not stop the clock. Only the
  -- timer that leaves none outstanding re-arms: an overlapping `set_timeout`, or a second
  -- `set_interval`, would otherwise leave one permanent chain per call, each firing the
  -- callbacks again.
  if event.name == "timer" then
    zellij.__timers = math.max((zellij.__timers or 0) - 1, 0)
    if zellij.__interval and zellij.__timers == 0 then
      arm(zellij.__interval)
    end
  end
  for _, handler in ipairs(handlers[event.name] or {}) do
    if handler(event) then
      should_render = true
    end
  end
  if plugin.update and plugin.update(event) then
    should_render = true
  end
  return should_render or zellij.__should_render
end

--- Called by the host. Same shape as `__update`, for pipe messages.
function zellij.__pipe(plugin, message)
  zellij.__should_render = false
  local should_render = false
  if plugin.pipe and plugin.pipe(message) then
    should_render = true
  end
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
  if plugin.load then
    plugin.load(config)
  end
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
setmetatable(zellij, {
  __index = function(_, name)
    if type(name) ~= "string" or name:sub(1, 1) == "_" then
      return nil
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
