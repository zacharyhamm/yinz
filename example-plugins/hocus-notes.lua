-- The hocus notes of whatever project this pane is in, live.
--
--   zellij plugin --floating -- file:$PWD/example-plugins/hocus-notes.lua
--
-- or, authored through hocus: plugin_write name=hocus-notes, then plugin_reload name=hocus-notes.
--
-- The sibling of hocus-todos.lua, over the other thing hocus keeps per project. A note is three
-- entities rather than one: the `note` row carries the title and body, and its tags and the todo
-- items it references are rows of their own (`note_tag`, `note_link`) keyed by the note's id.
-- So a listing is one watch plus two side fetches, which is the shape this example exists to
-- show. See "Hocus data" in docs/LUA_PLUGINS.md.
--
-- Read-only on purpose, and it never reads a body: a listing does not show one, and bodies are
-- the one column with a real size. `note_get` in hocus is how a body is read.
local zellij = require("zellij")
local data = zellij.data
local ui = zellij.ui

local HOCUS = "shared:hocus"

local M = {}

M.status = "asking for permission"
M.notes = {}
M.tags = {}
M.links = {}
M.root = nil

function M.load()
  M.cwd = zellij.get_plugin_ids().initial_cwd or ""
  zellij.subscribe { "plugin_data_changed", "permission_request_result" }
  zellij.request_permission { "plugin_data", "shared_plugin_data" }
end

-- The project this pane is in: the longest `root` that prefixes our cwd, which is the rule hocus
-- itself resolves a folder by.
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

-- The tags and links of the notes on screen, gathered by note id. Two fetches, each naming the
-- ids it is about, rather than a fetch of every tag in the store; and none at all when there are
-- no notes, because an empty `is_in` is refused.
local function side_tables(notes)
  local ids = {}
  for _, note in ipairs(notes) do
    ids[#ids + 1] = note.id
  end
  M.tags, M.links = {}, {}
  if #ids == 0 then
    return
  end
  for _, row in ipairs(data.fetch {
    namespace = HOCUS, entity = "note_tag",
    filter = data.is_in("note_id", ids), limit = 1000,
  }) do
    M.tags[row.note_id] = M.tags[row.note_id] or {}
    table.insert(M.tags[row.note_id], row.tag)
  end
  for _, row in ipairs(data.fetch {
    namespace = HOCUS, entity = "note_link",
    filter = data.is_in("note_id", ids), limit = 1000,
  }) do
    M.links[row.note_id] = M.links[row.note_id] or {}
    table.insert(M.links[row.note_id], tostring(row.todo_id))
  end
end

zellij.on("permission_request_result", function()
  local project = project_for(M.cwd)
  if not project then
    M.status = "no hocus project covers " .. M.cwd
    return true
  end
  M.root, M.name = project.root, project.name
  -- Watch rather than fetch, as hocus-todos.lua does. Live notes of this project only, newest
  -- change first, and never the body.
  local _, records = data.watch {
    namespace = HOCUS,
    entity = "note",
    filter = data.all(data.eq("root", M.root), data.eq("archived", false)),
    sort = { "updated_at desc" },
    select = { "id", "title", "author", "updated_at" },
    limit = 500,
  }
  M.notes = records
  side_tables(M.notes)
  M.status = nil
  return true
end)

zellij.on("plugin_data_changed", function(event)
  M.notes = event.records
  side_tables(M.notes)
  return true
end)

-- Rendering -----------------------------------------------------------------------------------

function M.render(rows, cols)
  if M.status then
    return ui.text(M.status):dim():render({ x = 0, y = 0, width = cols, height = 1 })
  end
  local list = ui.nested_list {}
  for _, note in ipairs(M.notes) do
    list:item(ui.text(note.title or ""), 0)
    local detail = {}
    for _, tag in ipairs(M.tags[note.id] or {}) do
      detail[#detail + 1] = "#" .. tag
    end
    if M.links[note.id] then
      detail[#detail + 1] = "todos " .. table.concat(M.links[note.id], ", ")
    end
    if note.author and note.author ~= "user" then
      detail[#detail + 1] = "by " .. note.author
    end
    if #detail > 0 then
      list:item(ui.text(table.concat(detail, " · ")):dim(), 1)
    end
  end
  if #M.notes == 0 then
    list:item(ui.text("no notes"):dim(), 0)
  end
  return ui.text(M.name .. " notes"):emphasis(0):render()
    .. "\n"
    .. list:render({ x = 0, y = 1, width = cols, height = rows - 1 })
end

return M
