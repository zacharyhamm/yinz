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
  -- A struct-like payload also gets its fields merged in, so `Key` can be read as
  -- `event.bare_key` rather than `event.value.bare_key`.
  if type(payload) == "table" and #payload == 0 then
    for k, v in pairs(payload) do
      normalized[k] = v
    end
  end
  if normalized.bare_key ~= nil then
    normalized.key = zellij.key_name(normalized.bare_key)
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

--- Ask to be sent a `timer` event in `seconds`. Timers do not repeat; set another one from the
--- handler if you want a ticking clock.
function zellij.set_timeout(seconds)
  return dispatch("set_timeout", seconds)
end

--- Close this plugin's own pane.
function zellij.close_self()
  return dispatch("close_self")
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

-- Rendering ----------------------------------------------------------------------------------

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
