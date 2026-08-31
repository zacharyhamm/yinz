-- The other half of `todo-extension.lua`: a pane over the same shared rows.
--
--   plugin_write name=todo, then plugin_reload name=todo
--
-- It watches `shared:todo` - the namespace the extension owns - so a todo added by a tool call
-- appears here without this plugin polling for it, and one completed here is what the next
-- `todo_list` returns. See "Data" in docs/LUA_PLUGINS.md.
--
-- What it deliberately does not do is define the schema. The extension owns that; a second
-- define is a migration racing another migration, and the two spellings only have to differ
-- once. A reader fetches, watches and writes rows - nothing else.
local zellij = require("zellij")
local ui = zellij.ui

local M = {}

local NAMESPACE = "shared:todo"

M.todos = {}
M.selected = 1
M.status = "loading"
-- `nil` unless the pane is taking text: the string typed so far.
M.input = nil
-- The id being renamed while `input` is open, or `nil` when the input adds a new todo.
M.editing = nil

function M.load(config)
  zellij.subscribe { "key", "plugin_data_changed", "permission_request_result" }
  -- A shared namespace needs both, in the reader as much as in the owner.
  zellij.request_permission { "plugin_data", "shared_plugin_data" }
end

-- Data --------------------------------------------------------------------------------------

local function selected_todo()
  return M.todos[M.selected]
end

-- Every write goes through here, so a refusal lands on the status line instead of taking the
-- plugin down. `apply` takes the namespace as its second argument.
local function apply(mutation)
  local ok, err = pcall(zellij.data.apply, mutation, NAMESPACE)
  if not ok then
    M.status = tostring(err)
  end
  return ok
end

-- The permissions are answered a moment after `load`, and this handler runs on every load -
-- an already-granted permission is answered at once rather than prompting. Nothing that needs
-- a permission may happen before it.
zellij.on("permission_request_result", function()
  local ok, err = pcall(function()
    -- A watch is a standing fetch: `plugin_data_changed` arrives whenever its rows change,
    -- whoever changed them.
    M.watch, M.todos = zellij.data.watch {
      entity = "todo",
      namespace = NAMESPACE,
      sort = { "done", "id" },
      limit = 200,
    }
  end)
  -- The commonest error here is "no such entity": the extension has not run yet, so nothing has
  -- defined the schema. Saying so beats an empty list that looks like an empty todo list.
  M.status = ok and (#M.todos .. " todos") or ("cannot read the todo list: " .. tostring(err))
  return true
end)

zellij.on("plugin_data_changed", function(event)
  if M.watch and event.watch_id ~= M.watch then
    return
  end
  M.todos = event.records or {}
  M.selected = math.max(1, math.min(M.selected, #M.todos))
  M.status = #M.todos .. " todos"
  return true
end)

-- Keys --------------------------------------------------------------------------------------

local function commit()
  local title = (M.input or ""):gsub("^%s+", ""):gsub("%s+$", "")
  if title ~= "" then
    if M.editing then
      apply(zellij.data.update("todo", zellij.data.eq("id", M.editing), { title = title }))
    else
      apply(zellij.data.put("todo", { title = title, done = false }))
    end
  end
  M.input, M.editing = nil, nil
end

-- While the input is open every key belongs to it, which is what makes this a mode rather than
-- a set of shortcuts. A single-character key name is the character itself.
local function typing(key)
  if key == "Esc" then
    M.input, M.editing = nil, nil
  elseif key == "Enter" then
    commit()
  elseif key == "Backspace" then
    M.input = M.input:sub(1, -2)
  elseif #key == 1 then
    M.input = M.input .. key
  end
  return true
end

zellij.on("key", function(event)
  local key = event.key
  if M.input then
    return typing(key)
  end
  if key == "q" or key == "Esc" then
    zellij.close_self()
  elseif key == "j" or key == "Down" then
    M.selected = math.min(M.selected + 1, math.max(#M.todos, 1))
  elseif key == "k" or key == "Up" then
    M.selected = math.max(M.selected - 1, 1)
  elseif key == "a" then
    M.input, M.editing = "", nil
  elseif key == "e" then
    local todo = selected_todo()
    if todo then
      M.input, M.editing = todo.title, todo.id
    end
  -- There is no `Space` key: a space arrives as `Char(" ")`, and `zellij.key_name` hands it
  -- over as the character itself. Everything a keyboard prints is one character wide here.
  elseif key == "Enter" or key == " " then
    local todo = selected_todo()
    if todo then
      apply(zellij.data.update("todo", zellij.data.eq("id", todo.id), { done = not todo.done }))
    end
  elseif key == "d" then
    local todo = selected_todo()
    if todo then
      apply(zellij.data.delete("todo", zellij.data.eq("id", todo.id)))
    end
  end
  return true
end)

-- Rendering ---------------------------------------------------------------------------------

function M.render(rows, cols)
  local lines = { ui.text("todo"):selected():render() }
  if #M.todos == 0 then
    lines[#lines + 1] = ui.text("(nothing yet - press a to add one)"):dim():render()
  end
  for i, todo in ipairs(M.todos) do
    local mark = todo.done and "[x] " or "[ ] "
    local text = ui.text(zellij.text.truncate(mark .. todo.title, math.max(10, cols - 2), "..."))
    if todo.done then
      text = text:dim()
    end
    if i == M.selected and not M.input then
      text = text:selected()
    end
    lines[#lines + 1] = text:render()
  end
  lines[#lines + 1] = ""
  if M.input then
    local what = M.editing and "rename" or "new"
    lines[#lines + 1] = ui.text(("%s: %s_"):format(what, M.input)):emphasis(0):render()
    lines[#lines + 1] = ui.text("<Enter> save  <Esc> cancel"):dim():render()
  else
    lines[#lines + 1] = ui.text(M.status):dim():render()
    lines[#lines + 1] = ui.text("j/k move  <Enter> toggle  a add  e rename  d delete  q quit")
      :dim()
      :render()
  end
  return table.concat(lines, "\n")
end

return M
