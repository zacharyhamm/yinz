-- Run: zellij plugin --floating -- "file:$PWD/example-plugins/forms.lua"
local zellij = require('zellij')
local ui = zellij.ui
local M = {}
local layouts = {'default','compact','development','review','writing','monitoring','presentation','日本語'}
local modes = {'Normal','Locked','Pane'}
local defaults = {name='作業 workspace', description='**Hello** from 日本語', layout=1, branch='main', mode=1, frames=true}
local form = ui.form({
  {kind='text_input', id='name', label='Name', value=defaults.name},
  {kind='text_input', id='description', label='Markdown', value=defaults.description},
  {kind='select', id='layout', label='Layout', options=layouts, max_visible_options=4},
  {kind='combo', id='branch', label='Branch', value=defaults.branch,
    options={'main','develop','feature/login','feature/search','fix/日本語'}, max_visible_options=3},
  {kind='radio_group', id='mode', label='Mode', options=modes},
  {kind='checkbox', id='frames', label='Frames', value=true},
  {kind='button', id='preview', label='Preview'},
  {kind='button', id='reset', label='Reset'},
})
local confirm = ui.dialog {
  title='Preview these settings?', message='Confirm to render the submitted values. Nothing is saved.',
  fields={{kind='button',id='yes',label='Confirm'}, {kind='button',id='no',label='Cancel'}},
  help='Enter: choose  Esc: cancel',
}
local showcase = [[

## Markdown showcase

**Bold**, *italic*, ~~removed~~, and `inline code`.

### Checklist

- [x] Unicode: 日本語 🎉
- [ ] Try paste and cursor movement
- [ ] Search the Layout dropdown

1. Edit the form.
2. Choose Preview and Confirm.
3. Resize and scroll this panel.

> A quote wraps to fit the available terminal columns.

| Widget | Exercise |
| --- | --- |
| Select | Search and scroll |
| Combo | Suggestions or new text |
| Radio | Change mode |

```lua
local ui = require('zellij').ui
local notes = ui.markdown('# Hello')
```

---

Long paragraph: Resize the pane to check wrapping, then use PageDown and PageUp to explore the document. 日本語 uses wider terminal columns. Parsed Markdown is reused until the submitted content changes.

## End of preview

Reset restores the original form and document.
]]
local intro = '# Form playground\n\nChoose Preview to render your description and settings.'
local markdown = ui.markdown(intro .. showcase)
local confirming, scroll, line_count, page_rows = false, 0, 0, 0
local status = 'Ready — changes stay in memory'

-- Escape punctuation so settings remain literal text inside a Markdown table.
local function literal(value)
  return zellij.text.sanitize(tostring(value)):gsub('[\r\n]', ' '):gsub('(%p)', '\\%1')
end

function M.load()
  zellij.subscribe {'Key','Mouse','PastedText'}
end
function M.update(event)
  if not confirming and event.name == 'key' and (event.key == 'PageDown' or event.key == 'PageUp') then
    local delta = math.max(1, page_rows - 1) * (event.key == 'PageDown' and 1 or -1)
    scroll = math.max(0, math.min(math.max(0, line_count-page_rows), scroll+delta))
    return true
  end
  local widget = confirming and confirm or form
  local action
  if event.name == 'key' then action = widget:handle_key(event)
  elseif event.name == 'mouse' then action = widget:handle_mouse(event)
  elseif event.name == 'pasted_text' then action = widget:insert_pasted_text(event.value)
  else return false end
  if confirming then
    if action.kind == 'button_pressed' and action.id == 'yes' then
      local v = form:values()
      markdown = ui.markdown('# Submitted settings\n\n' .. v.description
        .. '\n\n| Setting | Value |\n| --- | --- |\n'
        .. '| Name | ' .. literal(v.name) .. ' |\n'
        .. '| Layout | ' .. literal(layouts[v.layout]) .. ' |\n'
        .. '| Branch | ' .. literal(v.branch) .. ' |\n'
        .. '| Mode | ' .. literal(modes[v.mode]) .. ' |\n'
        .. '| Frames | ' .. tostring(v.frames) .. ' |\n' .. showcase)
      status, scroll, confirming = 'Preview updated — nothing saved', 0, false
      return true
    elseif action.kind == 'cancel' or (action.kind == 'button_pressed' and action.id == 'no') then
      confirming = false
      return true
    end
  elseif action.kind == 'button_pressed' and action.id == 'preview' then
    for _, id in ipairs({'name', 'branch'}) do
      if not form:value(id):find('%S') then
        status = 'Required: ' .. id
        form:focus(id)
        return true
      end
    end
    confirm:focus('yes')
    confirming = true
    return true
  elseif action.kind == 'button_pressed' and action.id == 'reset' then
    for id, value in pairs(defaults) do form:set_value(id, value) end
    form:focus('name')
    markdown = ui.markdown(intro .. showcase)
    status, scroll = 'Ready — changes stay in memory', 0
    return true
  end
  return action.should_render
end
function M.render(rows, cols)
  if confirming then return confirm:render{width=cols,height=rows} end
  local body_rows = math.max(0, rows-2)
  local form_width, form_rows = cols, math.min(8, math.floor(body_rows/2))
  local x, y, width = 0, form_rows, cols
  if cols >= 100 then
    form_width, form_rows = math.floor(cols/2), body_rows
    x, y, width = form_width+1, 0, cols-form_width-1
  end
  page_rows = math.max(0, body_rows-y)
  local area = {x=x,y=y,width=width,height=page_rows,scroll=scroll}
  local content
  content, line_count = markdown:render(area)
  local clamped = math.max(0, math.min(scroll, line_count-page_rows))
  if clamped ~= scroll then
    scroll, area.scroll = clamped, clamped
    content = markdown:render(area)
  end
  local frame = content .. form:render{width=form_width,height=form_rows}
  if rows >= 2 then frame = frame .. ui.text(status):render{x=0,y=rows-2,width=cols,height=1} end
  if rows >= 1 then
    frame = frame .. ui.text('Arrows/Tab: move  Enter: activate  Esc: back  PgUp/PgDn: Markdown'):render{x=0,y=rows-1,width=cols,height=1}
  end
  return frame
end
return M
