-- A hocus extension that owns a shared data namespace, and three tools over it.
--
--   plugin_write name=hocus/todo, then plugin_reload name=hocus/todo
--
-- Extensions live in the `hocus` folder of the plugin folder, so this file is written as
-- `hocus/todo.lua` and its tools are `todo_add`, `todo_list` and `todo_complete` - the file
-- name is the tool namespace. `todo-pane.lua` is the other half: a pane that watches the same
-- rows. See "Hocus extensions" and "Data" in docs/LUA_PLUGINS.md.
--
-- The order below is the whole point of this example. Permissions are asked for in `load` and
-- are not in force until the `permission_request_result` handler runs, so the schema, the
-- registration and the first fetch all happen there - a `define` from `load` is refused, and an
-- extension that was refused registers nothing at all.
local zellij = require("zellij")

local M = {}

-- Rows two plugins share live in a namespace whose name starts with `shared:`. Anything else is
-- refused, and reaching it needs `shared_plugin_data` as well as `plugin_data` - in the pane too.
local NAMESPACE = "shared:todo"

-- What the model is told this extension can do. `read_only` is available in planning mode and
-- never confirms; `automatic` is withheld from planning and never confirms; `confirm` asks
-- every time and must say what it is about to do in `consequence`.
local TOOLS = {
  {
    name = "add",
    description = "Add a todo to the shared list.",
    parameters = {
      type = "object",
      properties = { title = { type = "string" } },
      required = { "title" },
    },
    safety = "automatic",
  },
  {
    name = "list",
    description = "List the todos. `done` filters by state; omit it for everything.",
    parameters = {
      type = "object",
      properties = { done = { type = "boolean" } },
    },
    safety = "read_only",
  },
  {
    name = "complete",
    description = "Mark the todo with this id done.",
    parameters = {
      type = "object",
      properties = { id = { type = "integer" } },
      required = { "id" },
    },
    safety = "automatic",
  },
}

-- The schema. One plugin owns this - `require` is rooted at the plugin file's own folder, so a
-- `hocus/` extension and a top-level pane cannot share a module, which makes "the owner defines,
-- the reader only fetches" the pattern rather than a preference.
--
-- `id`, `uid`, `created_at` and `updated_at` are on every row already and may not be declared: a
-- define that names one of them fails whole.
local function define()
  zellij.data.define {
    name = "todo",
    version = 1,
    namespace = NAMESPACE,
    attrs = {
      { "title", "text", required = true, full_text = true },
      { "done", "bool", indexed = true, default = false },
    },
  }
end

-- Hocus names the extension it launched in the plugin's configuration, and the registration is
-- addressed with those two values. They are only there when hocus did the launching: reloaded
-- by hand (`zellij plugins reload hocus/todo`, or from a layout) this plugin runs perfectly
-- well and registers nothing, so say which of the two happened rather than sending a payload
-- hocus will drop.
local function register()
  local config = M.config or {}
  local namespace, path = config.hocus_extension_name, config.hocus_extension_path
  if not (namespace and path) then
    zellij.log("todo: not launched by hocus, so its tools are not registered - ask hocus to "
      .. "reload it (plugin_reload name=hocus/todo) to pick them up")
    return
  end
  zellij.message_to_plugin {
    message_name = "hocus_extension_register",
    message_payload = zellij.json.encode {
      version = 1,
      namespace = namespace,
      path = path,
      tools = TOOLS,
    },
  }
end

local function reply(call, result)
  zellij.message_to_plugin {
    destination_plugin_id = call.reply_to,
    message_name = "hocus_extension_result",
    message_payload = zellij.json.encode {
      version = 1,
      call_id = call.call_id,
      result = result,
    },
  }
end

local function describe(todo)
  return ("#%d [%s] %s"):format(todo.id, todo.done and "x" or " ", todo.title)
end

local function run(call)
  local arguments = call.arguments or {}
  if call.tool == "add" then
    local title = tostring(arguments.title or ""):gsub("^%s+", ""):gsub("%s+$", "")
    if title == "" then
      return "a todo needs a title"
    end
    zellij.data.apply(zellij.data.put("todo", { title = title, done = false }), NAMESPACE)
    return "added: " .. title
  end
  if call.tool == "list" then
    local filter = nil
    if arguments.done ~= nil then
      filter = zellij.data.eq("done", arguments.done and true or false)
    end
    local todos = zellij.data.fetch {
      entity = "todo",
      namespace = NAMESPACE,
      filter = filter,
      sort = { "done", "id" },
      limit = 100,
    }
    if #todos == 0 then
      return "no todos"
    end
    local lines = {}
    for i, todo in ipairs(todos) do
      lines[i] = describe(todo)
    end
    return table.concat(lines, "\n")
  end
  if call.tool == "complete" then
    local id = math.tointeger(arguments.id)
    if not id then
      return "complete needs the id of a todo"
    end
    local done = zellij.data.apply(
      zellij.data.update("todo", zellij.data.eq("id", id), { done = true }),
      NAMESPACE
    )
    if (done.rows_written or 0) == 0 then
      return "no todo has id " .. id
    end
    return "completed #" .. id
  end
  return "no such tool: " .. tostring(call.tool)
end

function M.load(config)
  M.config = config
  -- Registration is a plugin message; the store is the other two. All of them are asked for
  -- here and used only once the answer has arrived.
  zellij.request_permission {
    "message_and_launch_other_plugins",
    "plugin_data",
    "shared_plugin_data",
  }
end

zellij.on("permission_request_result", function()
  -- Loud rather than silent: an extension whose schema failed still registers, and its tools
  -- then fail one at a time with the reason in the log.
  local ok, err = pcall(define)
  if not ok then
    zellij.log("todo: the schema could not be defined: " .. tostring(err))
  end
  register()
end)

function M.pipe(message)
  -- Hocus broadcasts this when it reloads, so a running extension re-announces itself.
  if message.name == "hocus_extension_discover" then
    register()
    return
  end
  if message.name ~= "hocus_extension_call" then
    return
  end
  local call = zellij.json.decode(message.payload)
  if call.version ~= 1 then
    return
  end
  -- A tool that raises answers with the error rather than going quiet: the model can act on
  -- "no such entity", and cannot act on silence.
  local ok, result = pcall(run, call)
  reply(call, ok and result or ("failed: " .. tostring(result)))
end

return M
