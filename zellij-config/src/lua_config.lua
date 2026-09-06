local config = {
  __cleared = {},
  __options = {},
  __keybinds = {},
  __themes = {},
  __plugins = {},
  __background_plugins = {},
  __hooks = {},
  __ui = {},
  __env = {},
  __raw = {},
}

local function sorted_keys(t)
  local keys = {}
  for key in pairs(t or {}) do keys[#keys + 1] = key end
  table.sort(keys, function(a, b) return tostring(a) < tostring(b) end)
  return keys
end

local function quote(value)
  if type(value) == "boolean" or type(value) == "number" then return tostring(value) end
  value = tostring(value)
  value = value:gsub("\\", "\\\\"):gsub('"', '\\"')
    :gsub("\n", "\\n"):gsub("\r", "\\r"):gsub("\t", "\\t")
  return '"' .. value .. '"'
end

local function name(value)
  value = tostring(value)
  if value:match("^[%a_][%w_%-]*$") then return value end
  return quote(value)
end

local function is_array(t)
  return type(t) == "table" and (#t > 0 or next(t) == nil)
end

local function node(node_name, value)
  if type(value) ~= "table" then return name(node_name) .. " " .. quote(value) end
  if is_array(value) then
    local values = {}
    for _, item in ipairs(value) do values[#values + 1] = quote(item) end
    return name(node_name) .. " " .. table.concat(values, " ")
  end
  local children = {}
  for _, key in ipairs(sorted_keys(value)) do children[#children + 1] = node(key, value[key]) end
  return name(node_name) .. " { " .. table.concat(children, "; ") .. "; }"
end

local function merge(into, values)
  for key, value in pairs(values or {}) do into[key] = value end
end

function config.options(values) merge(config.__options, values) end
function config.ui(values) merge(config.__ui, values) end
function config.env(values) merge(config.__env, values) end

function config.theme(theme_name, values) config.__themes[theme_name] = values end

local function plugin(location, settings)
  settings = settings or {}
  return { location = location, settings = settings }
end

function config.plugin(alias, location, settings)
  config.__plugins[alias] = plugin(location, settings)
end

function config.background_plugin(location, settings)
  config.__background_plugins[#config.__background_plugins + 1] = plugin(location, settings)
end

function config.hook(hook_name, location, settings)
  config.__hooks[hook_name] = plugin(location, settings)
end

function config.clear_hook(hook_name) config.__hooks[hook_name] = plugin("", {}) end

function config.action(action_name, ...)
  local arguments = { ... }
  if #arguments == 1 and type(arguments[1]) == "table" and not is_array(arguments[1]) then
    local descriptor = arguments[1]
    if descriptor.args or descriptor.props or descriptor.children then
      return { name = action_name, args = descriptor.args or {}, props = descriptor.props or {}, children = descriptor.children }
    end
    return { name = action_name, args = {}, props = descriptor }
  end
  return { name = action_name, args = arguments, props = {} }
end

function config.bind(mode, key, actions)
  config.__keybinds[#config.__keybinds + 1] = { mode = mode, key = key, actions = actions }
end

function config.unbind(mode, key)
  config.__keybinds[#config.__keybinds + 1] = { mode = mode, key = key, unbind = true }
end

function config.raw(kdl) config.__raw[#config.__raw + 1] = kdl end

for _, section in ipairs({ "options", "keybinds", "themes", "plugins", "hooks", "ui", "env" }) do
  config["clear_" .. section] = function() config.__cleared[section] = true end
end

function config.clear_background_plugins()
  config.__background_plugins = {}
  config.__clear_background_plugins = true
end

local function action_kdl(action)
  if type(action) == "string" then return name(action) end
  local out = { name(assert(action.name, "an action needs a name")) }
  for _, argument in ipairs(action.args or {}) do out[#out + 1] = quote(argument) end
  for _, key in ipairs(sorted_keys(action.props or {})) do
    out[#out + 1] = name(key) .. "=" .. quote(action.props[key])
  end
  if action.children then out[#out + 1] = node("", action.children):gsub('^"" ', '') end
  return table.concat(out, " ")
end

local function plugin_kdl(node_name, value, location_property)
  local out = { name(node_name) }
  if location_property then out[#out + 1] = "location=" .. quote(value.location)
  else out[#out + 1] = quote(value.location) end
  local children = {}
  for _, key in ipairs(sorted_keys(value.settings)) do children[#children + 1] = node(key, value.settings[key]) end
  if #children > 0 then out[#out + 1] = "{ " .. table.concat(children, "; ") .. "; }" end
  return table.concat(out, " ")
end

function config.__render(self)
  local out = {}
  for _, key in ipairs(sorted_keys(self.__options)) do out[#out + 1] = node(key, self.__options[key]) end

  local binds = {}
  for _, binding in ipairs(self.__keybinds) do
    local child
    if binding.unbind then child = "unbind " .. quote(binding.key)
    else
      local actions = {}
      for _, action in ipairs(binding.actions or {}) do actions[#actions + 1] = action_kdl(action) end
      child = "bind " .. quote(binding.key) .. " { " .. table.concat(actions, "; ") .. "; }"
    end
    binds[#binds + 1] = name(binding.mode) .. " { " .. child .. "; }"
  end
  if #binds > 0 or self.__cleared.keybinds then
    out[#out + 1] = "keybinds" .. (self.__cleared.keybinds and " clear-defaults=true" or "")
      .. " { " .. table.concat(binds, "; ") .. (#binds > 0 and "; " or "") .. "}"
  end

  local themes = {}
  for _, key in ipairs(sorted_keys(self.__themes)) do themes[#themes + 1] = node(key, self.__themes[key]) end
  if #themes > 0 then out[#out + 1] = "themes { " .. table.concat(themes, "; ") .. "; }" end

  local plugins = {}
  for _, key in ipairs(sorted_keys(self.__plugins)) do plugins[#plugins + 1] = plugin_kdl(key, self.__plugins[key], true) end
  if #plugins > 0 then out[#out + 1] = "plugins { " .. table.concat(plugins, "; ") .. "; }" end

  local background = {}
  for _, value in ipairs(self.__background_plugins) do background[#background + 1] = plugin_kdl(value.location, value, false) end
  if #background > 0 or self.__clear_background_plugins then
    out[#out + 1] = "load_plugins" .. (self.__clear_background_plugins and " clear-defaults=true" or "")
      .. " { " .. table.concat(background, "; ") .. (#background > 0 and "; " or "") .. "}"
  end

  local hooks = {}
  for _, key in ipairs(sorted_keys(self.__hooks)) do hooks[#hooks + 1] = plugin_kdl(key, self.__hooks[key], false) end
  if #hooks > 0 then out[#out + 1] = "hooks { " .. table.concat(hooks, "; ") .. "; }" end
  if next(self.__ui) then out[#out + 1] = node("ui", self.__ui) end
  if next(self.__env) then out[#out + 1] = node("env", self.__env) end
  for _, raw in ipairs(self.__raw) do out[#out + 1] = raw end
  return table.concat(out, "\n")
end

return config
