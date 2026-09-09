-- The hocus todo list of whatever project this pane is in, live.
--
--   zellij plugin --floating -- file:$PWD/example-plugins/hocus-todos.lua
--
-- or, authored through hocus: plugin_write name=hocus-todos, then plugin_reload name=hocus-todos.
--
-- The point of this example is that there is no bridge and no message passing. Hocus keeps its
-- projects and todos as rows in the shared plugin data store, and a plugin that asks for
-- `shared_plugin_data` reads them directly. Note especially that nothing here calls
-- `zellij.data.define`: you declare entities you *own*, and a fetch works against anything
-- already in the store. See "Hocus data" in docs/LUA_PLUGINS.md.
--
-- Read-only on purpose. Hocus maintains invariants these rows do not carry, so adding a todo
-- means asking hocus, not writing the table.
local zellij = require("zellij")
local data = zellij.data
local ui = zellij.ui

local HOCUS = "shared:hocus"

local M = {}

M.status = "asking for permission"
M.items = {}
M.root = nil

function M.load()
  M.cwd = zellij.get_plugin_ids().initial_cwd or ""
  zellij.subscribe { "plugin_data_changed", "permission_request_result" }
  zellij.request_permission { "plugin_data", "shared_plugin_data" }
end

-- Discovery -----------------------------------------------------------------------------------

-- What is actually in `shared:hocus`, logged once. A plugin that knows the schema does not need
-- this, but it is how you find out - and it is the difference between an empty list because the
-- project has no todos and an empty list because hocus has never run here.
local function log_catalog()
  local ok, entities = pcall(data.catalog, HOCUS)
  if not ok or #entities == 0 then
    return false
  end
  for _, entity in ipairs(entities) do
    local names = {}
    for _, attr in ipairs(entity.attrs) do
      names[#names + 1] = attr.name .. " " .. attr.kind
    end
    zellij.log(HOCUS .. "." .. entity.entity .. ": " .. table.concat(names, ", "))
  end
  return true
end

-- The project this pane is in: the longest `root` that prefixes our cwd, which is the rule hocus
-- itself resolves a folder by. Roots are not unique and a worktree sits inside its checkout, so
-- "the first one that matches" is the wrong answer.
local function project_for(cwd)
  local best
  for _, project in ipairs(data.fetch { namespace = HOCUS, entity = "project" }) do
    local root = project.root or ""
    if root ~= "" and (cwd == root or cwd:sub(1, #root + 1) == root .. "/") then
      if not best or #root > #best.root then
        best = project
      end
    end
  end
  return best
end

zellij.on("permission_request_result", function()
  if not log_catalog() then
    M.status = "no hocus data in this session"
    return true
  end
  local project = project_for(M.cwd)
  if not project then
    M.status = "no hocus project covers " .. M.cwd
    return true
  end
  M.root, M.name = project.root, project.name
  -- Watch rather than fetch: `plugin_data_changed` arrives whenever the answer changes, so a
  -- todo added or crossed off in a hocus pane shows up here without polling. The rows as they
  -- are now come back with the handle, so nothing is missed between fetching and watching.
  local _, records = data.watch {
    namespace = HOCUS,
    entity = "todo",
    filter = data.eq("root", M.root),
    -- Siblings are only ever appended, so `id` ascending is insertion order. There is no
    -- `position` column to sort on.
    sort = { "id" },
    limit = 500,
  }
  M.items = records
  M.status = nil
  return true
end)

zellij.on("plugin_data_changed", function(event)
  M.items = event.records
  return true
end)

-- Rendering -----------------------------------------------------------------------------------

-- The tree is edges, not nesting: `parent_id` holds the store `id` of the item above, null at the
-- top. Depth follows from the chain rather than from a column.
local function children_by_parent()
  local by_parent = {}
  for _, item in ipairs(M.items) do
    local key = item.parent_id or 0
    by_parent[key] = by_parent[key] or {}
    table.insert(by_parent[key], item)
  end
  return by_parent
end

local function walk(list, by_parent, parent, depth)
  for _, item in ipairs(by_parent[parent] or {}) do
    local text = ui.text((item.done and "x " or "- ") .. (item.text or ""))
    list:item(item.done and text:dim() or text, depth)
    walk(list, by_parent, item.id, depth + 1)
  end
end

function M.render(rows, cols)
  if M.status then
    return ui.text(M.status):dim():render({ x = 0, y = 0, width = cols, height = 1 })
  end
  local list = ui.nested_list {}
  walk(list, children_by_parent(), 0, 0)
  if #M.items == 0 then
    list:item(ui.text("nothing on the list"):dim(), 0)
  end
  return ui.text(M.name .. " todos"):emphasis(0):render()
    .. "\n"
    .. list:render({ x = 0, y = 1, width = cols, height = rows - 1 })
end

return M
