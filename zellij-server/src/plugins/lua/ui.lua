-- Pure shared widgets; only cursor delivery goes through the host dispatcher.
local native, zellij = ...
local ui = zellij.ui
local function typed(value, kind, name)
  assert(type(value) == kind, name .. " must be a " .. kind)
  return value
end
local function default(value, fallback)
  if value == nil then return fallback end
  return value
end
local function integer(value, name)
  assert(type(value) == "number" and math.type(value) == "integer" and value >= 0,
    name .. " must be a nonnegative integer")
  return value
end
local function strings(options)
  typed(options, "table", "options")
  for k, v in pairs(options) do
    assert(type(k) == "number" and k >= 1 and k <= #options and math.type(k) == "integer", "options must be an array")
    typed(v, "string", "option")
  end
  for i = 1, #options do typed(options[i], "string", "option") end
  return options
end
local allowed = { kind=true, id=true, label=true, value=true, options=true,
  placeholder=true, max_visible_options=true, disabled=true, style=true }
local function descriptor(d, single)
  typed(d, "table", "field descriptor")
  for k in pairs(d) do assert(allowed[k], "unknown descriptor field: " .. tostring(k)) end
  local kind = typed(d.kind, "string", "kind")
  local value = d.value
  if kind == "text_input" or kind == "combo" then value = typed(default(value, ""), "string", "value")
  elseif kind == "checkbox" then value = typed(default(value, false), "boolean", "value")
  elseif kind == "select" or kind == "radio_group" then
    if value ~= nil then integer(value, "value"); assert(value > 0, "option indices start at 1") end
  elseif kind ~= "button" then error("unknown widget kind: " .. kind) end
  return {
    kind=kind, id=typed(default(d.id, single and "value" or nil), "string", "id"),
    label=typed(default(d.label, ""), "string", "label"), value=default(value, zellij.null),
    options=strings(default(d.options, {})), placeholder=typed(default(d.placeholder, ""), "string", "placeholder"),
    max_visible_options=integer(default(d.max_visible_options, 6), "max_visible_options"),
    disabled=typed(default(d.disabled, false), "boolean", "disabled"),
    style=typed(default(d.style, "ribbon"), "string", "style"),
  }
end
local function coords(c)
  typed(c, "table", "coordinates")
  return integer(default(c.x, 0), "x"), integer(default(c.y, 0), "y"),
    integer(c.width, "width"), integer(c.height, "height")
end
local function key(event)
  if type(event) == "string" then
    return { bare_key = utf8.len(event) == 1 and { Char=event } or event, key_modifiers={} }
  end
  typed(event, "table", "key")
  return { bare_key=event.bare_key, key_modifiers=default(event.key_modifiers, {}) }
end
local function wrap(state, single_id, ids)
  local w = {}
  local function id(value) return typed(default(value, single_id), "string", "field id") end
  function w:value(field) return state:value(id(field)) end
  function w:values()
    local values = {}
    for _, field in ipairs(ids) do values[field] = state:value(field) end
    return values
  end
  function w:set_value(field, value)
    if single_id then value, field = field, single_id end
    state:set_value(id(field), value)
    return self
  end
  function w:set_options(field, options)
    if single_id then options, field = field, single_id end
    state:set_options(id(field), strings(options))
    return self
  end
  function w:focus(field) state:focus(id(field)); return self end
  function w:focused_id() return state:focused_id() end
  function w:move_focus(forward) return state:move_focus(typed(default(forward, true), "boolean", "forward")) end
  function w:handle_key(event) return state:key(key(event)) end
  function w:insert_pasted_text(text) return state:paste(typed(text, "string", "paste")) end
  function w:handle_mouse(event)
    typed(event, "table", "mouse event")
    return state:mouse(event.value or event)
  end
  function w:set_status(text)
    if text ~= nil then typed(text, "string", "status") end
    state:set_status(text); return self
  end
  function w:render(c)
    local frame, update, x, y = state:render(coords(c))
    if update then zellij.show_cursor(x and {x, y} or zellij.null) end
    return frame
  end
  return w
end
local function construct(fields, opts, single, dialog)
  typed(fields, "table", "fields")
  opts = default(opts, {})
  typed(opts, "table", "form options")
  if not dialog then
    for k in pairs(opts) do assert(k == "enter_submits", "unknown form option: " .. tostring(k)) end
  end
  local descriptors, ids = {}, {}
  for k in pairs(fields) do
    assert(type(k) == "number" and math.type(k) == "integer" and k >= 1 and k <= #fields, "fields must be an array")
  end
  for i = 1, #fields do
    local d = descriptor(fields[i], single)
    descriptors[i] = d
    if d.kind ~= "button" then ids[#ids+1] = d.id end
  end
  local state = native.form(descriptors, typed(default(opts.enter_submits, true), "boolean", "enter_submits"), dialog)
  return wrap(state, single and descriptors[1].id, ids)
end
function ui.form(fields, opts) return construct(fields, opts) end
for _, kind in ipairs({"text_input", "checkbox", "radio_group", "select", "combo", "button"}) do
  ui[kind] = function(d)
    d = default(d, {})
    typed(d, "table", "widget descriptor")
    local copy = {}
    for k,v in pairs(d) do copy[k]=v end
    copy.kind = kind
    return construct({copy}, {}, true)
  end
end
function ui.dialog(d)
  typed(d, "table", "dialog descriptor")
  for k in pairs(d) do assert(k == "title" or k == "message" or k == "help" or k == "fields" or k == "enter_submits", "unknown dialog field: " .. tostring(k)) end
  typed(d.title, "string", "title")
  if d.message ~= nil then typed(d.message, "string", "message") end
  if d.help ~= nil then typed(d.help, "string", "help") end
  return construct(default(d.fields, {}), d, false, d)
end
function ui.markdown(source, opts)
  typed(source, "string", "Markdown source")
  opts = default(opts, {})
  typed(opts, "table", "Markdown options")
  for k in pairs(opts) do assert(k == "compact", "unknown Markdown option: " .. tostring(k)) end
  local state = native.markdown(source, typed(default(opts.compact, false), "boolean", "compact"))
  return {
    render = function(_, c)
      local x,y,width,height = coords(c)
      return state:render(x,y,width,height,integer(default(c.scroll, 0), "scroll"))
    end,
  }
end

local wrap_text, sanitize = zellij.text.wrap, zellij.text.sanitize
function zellij.text.wrap(source, width)
  return wrap_text(typed(source, "string", "text"), integer(width, "width"))
end
function zellij.text.sanitize(source)
  return sanitize(typed(source, "string", "text"))
end
