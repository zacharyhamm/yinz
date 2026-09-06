-- The `context_menu` hook: what a right-click offers.
--
-- Unlike `agent-exit.lua`, this hook gets no pane and draws nothing. The menu is a first-class
-- thing the server paints over the panes and hit-tests itself; this plugin runs headless and only
-- decides what is on it. That is why it has no `render` and never calls `close_self`.
--
--   hooks {
--       context_menu "file:~/.config/zellij/hooks/my-menu.lua"
--   }
--
-- The event carries the click - which pane, what was selected, what word is under the cursor, what
-- the pane is running - and the items the configuration declared. Return whichever of them belong
-- here, in the order they should appear, plus any of your own.
--
-- Two rules about items, both from `data::ContextMenuItem`:
--
--   * A configured item you hand back **with its `id`** gets its actions back on arrival, so you
--     can relabel and reorder freely. Drop the `id` and you drop the behaviour with it.
--   * An item you invent has no actions. Give it a `context` and it reaches you as a
--     `context_menu_item_selected` event, from where `zellij.run_action` can do anything.
--
-- No `request_permission` here, for the reason `agent-exit.lua` gives: a builtin - one loaded from
-- a `zellij:` location - is granted gated calls unconditionally. A copy of this file loaded from a
-- `file:` location is not a builtin, so add
--
--   zellij.request_permission { "run_actions_as_user", "run_commands" }
--
-- to `load` if you fork it - `run_commands` because the "Open" item below shells out to the
-- platform's link opener.
local zellij = require("zellij")

local M = {}

--- The longest a piece of pane content is allowed to be when it is quoted into a label.
--- Past this the menu is describing the selection rather than naming the action.
local MAX_QUOTED = 24

--- `text` shortened to fit a label, with an ellipsis if anything was dropped.
--- Deliberately counts characters, not bytes: a label is measured in columns downstream.
local function shorten(text)
  local flat = text:gsub("%s+", " "):gsub("^%s+", ""):gsub("%s+$", "")
  if flat == "" then
    return nil
  end
  if (utf8.len(flat) or #flat) <= MAX_QUOTED then
    return flat
  end
  local offset = utf8.offset(flat, MAX_QUOTED + 1)
  return flat:sub(1, (offset or MAX_QUOTED + 1) - 1) .. "…"
end

--- Whether an item survives this click.
---
--- Only "Copy" is conditional in the shipped menu, because it is the only default whose meaning
--- depends on the click rather than on the pane. Everything else means the same thing wherever it
--- is pressed, so hiding it would only make the menu move around under the pointer.
local function keep(item, event)
  if item.label == "Copy" then
    return event.selected_text ~= nil
  end
  return true
end

--- The label an item is drawn with, given what was clicked.
local function relabel(item, event)
  if item.label == "Copy" and event.selected_text then
    local quoted = shorten(event.selected_text)
    if quoted then
      return ('Copy "%s"'):format(quoted)
    end
  end
  return item.label
end

--- Items this plugin adds, ahead of the configured ones.
---
--- A link under the cursor is the case worth having: the `link` plugin already recognises URLs and
--- paths and hands the match over, so the menu can offer to open the specific thing being pointed
--- at rather than something generic about the pane.
local function extra_items(event)
  local items = {}
  if event.link and event.link.matched ~= "" then
    local quoted = shorten(event.link.matched)
    table.insert(items, {
      label = ("Open %s"):format(quoted or event.link.matched),
      -- No `id` and no actions: this is ours. The context comes back to us on selection.
      context = { action = "open_link", url = event.link.matched },
    })
    table.insert(items, { separator = true })
  end
  return items
end

--- Drop separators that no longer separate anything.
---
--- Filtering items out leaves the rules that used to sit between them: a menu whose first row is a
--- horizontal line, or which has two in a row, looks broken rather than filtered. Nothing else can
--- do this - the server draws what it is given, and the filtering that stranded them happened
--- here.
---
--- Returns an empty list for a menu that is nothing but rules, which is how a plugin says "no menu
--- here"; the server closes one of those rather than drawing an empty box.
local function tidy(items)
  local out = {}
  local seen_content = false
  for _, item in ipairs(items) do
    if item.separator then
      -- Never lead with one, and never two in a row.
      if seen_content and not (out[#out] or {}).separator then
        table.insert(out, item)
      end
    else
      seen_content = true
      table.insert(out, item)
    end
  end
  -- ...and never trail with one either.
  while #out > 0 and out[#out].separator do
    table.remove(out)
  end
  return seen_content and out or {}
end

--- Probing for a link opener.
---
--- There is no one command that opens a link: Linux has `xdg-open`, macOS has `open`, and a
--- machine can have neither. So both are probed once, at load, exactly as the `about` and `share`
--- plugins do - `--help` is harmless and its exit code says whether the executable exists.
---
--- `M.opener` is nil until a probe answers. The hook is loaded on the first right-click, so the
--- only selection that can beat the probes is one made inside their own round-trip; that url waits
--- in `M.pending` and is opened as soon as an opener is known, rather than being dropped. One url
--- waits at a time, which is all a single pointer can have asked for inside that window.
---
--- Nothing here reads a command's output, so the cap is as small as it can be. All five arguments
--- have to be given whatever they are worth - see `prelude.lua`'s note on tuple variants - hence
--- the empty `cwd`, which the host resolves to the plugin's own.
local MAX_OUTPUT = 64

local function probe(executable)
  zellij.run_command {
    command = { executable, "--help" },
    env_variables = {},
    cwd = "",
    context = { probe = executable },
    max_bytes_returned = MAX_OUTPUT,
  }
end

local function open_link(url)
  if not M.opener then
    M.pending = url
    return
  end
  zellij.run_command {
    command = { M.opener, url },
    env_variables = {},
    cwd = "",
    context = {},
    max_bytes_returned = MAX_OUTPUT,
  }
end

function M.load(_config)
  M.opener = nil
  M.pending = nil
  zellij.subscribe {
    "context_menu_requested",
    "context_menu_item_selected",
    "run_command_result",
  }
  probe("xdg-open")
  probe("open")
end

zellij.on("run_command_result", function(event)
  -- A tuple variant, so the payload is positional: exit code, stdout, stderr, and the context the
  -- command was started with.
  local exit_code, _stdout, _stderr, context = table.unpack(event.value)
  local executable = context and context.probe
  if not executable or exit_code ~= 0 then
    return
  end
  -- `xdg-open` wins where both exist: on a system that has both, the link handling the user has
  -- configured is the freedesktop one.
  if executable == "xdg-open" or not M.opener then
    M.opener = executable
  end
  if M.pending then
    local url = M.pending
    M.pending = nil
    open_link(url)
  end
end)

zellij.on("context_menu_requested", function(event)
  local items = extra_items(event)
  for _, item in ipairs(event.items or {}) do
    if item.separator then
      table.insert(items, item)
    elseif keep(item, event) then
      item.label = relabel(item, event)
      table.insert(items, item)
    end
  end
  zellij.set_context_menu {
    request_id = event.request_id,
    items = tidy(items),
  }
end)

zellij.on("context_menu_item_selected", function(event)
  local item_context = event.item_context or {}
  if item_context.action == "open_link" and item_context.url then
    -- A host command rather than an action, because opening a link is whatever the user's system
    -- says it is and that is a command, not something the action vocabulary can express.
    open_link(item_context.url)
  end
end)

return M
