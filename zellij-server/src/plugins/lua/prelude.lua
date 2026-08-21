-- The Lua side of zellij's plugin API.
--
-- Rust exposes exactly one function to a plugin - `__dispatch(command_name, args)` - mirroring
-- the single `host_run_plugin_command` import the wasm runtime exposes. Everything a plugin
-- author actually touches is built here, in Lua, so that the ergonomics can grow without
-- widening the Rust boundary or recompiling zellij.

local zellij = {}

--- The host installs `zellij.__dispatch` for the duration of each call into the plugin and
--- removes it afterwards, so it is looked up per call rather than captured once here.
local function dispatch(...)
  if not zellij.__dispatch then
    error("zellij commands can only be called from inside a plugin entrypoint", 2)
  end
  return zellij.__dispatch(...)
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
  return dispatch("request_permission", converted)
end

--- Whether this plugin's pane can hold focus. Non-selectable plugins (status bars and the like)
--- are skipped when the user cycles panes.
function zellij.set_selectable(selectable)
  return dispatch("set_selectable", selectable and true or false)
end

--- Open a new terminal pane, optionally in `cwd`.
function zellij.open_terminal(cwd)
  return dispatch("open_terminal", cwd)
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

return zellij
