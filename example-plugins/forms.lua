-- Run: zellij action new-pane --plugin file:example-plugins/forms.lua
local zellij = require('zellij')
local ui = zellij.ui
local M = {}
local form = ui.form {
  {kind='text_input', id='name', label='Name'},
  {kind='select', id='layout', label='Layout', options={'default','compact','development'}},
  {kind='combo', id='branch', label='Branch', options={'main','feature/login','feature/search'}},
  {kind='checkbox', id='frames', label='Frames', value=true},
  {kind='button', id='save', label='Save'},
}
local confirm = ui.dialog {
  title='Save these settings?', message='This example only displays the values.',
  fields={{kind='button',id='yes',label='Save'}, {kind='button',id='no',label='Cancel'}},
  help='Enter: choose  Esc: cancel',
}
local markdown = ui.markdown([[# Shared widgets

Type a name, then press **Tab** to move between fields.

Open Layout with **Down**, then type to search. Branch accepts new text and suggestions.

Choose **Save** to open a confirmation dialog. No files or settings change.

## Markdown

Use **PageDown** and **PageUp** to scroll these instructions.

- [x] Unicode: 日本語
- [x] Searchable choices
- [x] Mouse press and release
- [x] Viewport scrolling

Resize the pane to wrap this text again. Parsed Markdown stays in memory.
]])
local confirming, scroll, line_count, page_rows = false, 0, 0, 0
local status = 'Ready'
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
      local values = form:values()
      status = 'Saved: ' .. values.name .. ', branch ' .. values.branch
      confirming = false
    elseif action.kind == 'cancel' or (action.kind == 'button_pressed' and action.id == 'no') then
      confirming = false
    end
  elseif action.kind == 'submit' or (action.kind == 'button_pressed' and action.id == 'save') then
    confirm:focus('yes')
    confirming = true
  end
  return action.should_render
end
function M.render(rows, cols)
  if confirming then return confirm:render{width=cols,height=rows} end
  local form_rows = math.min(rows, 5)
  local frame = form:render{width=cols,height=form_rows}
  if rows > form_rows then
    frame = frame .. ui.text(status):render{x=0,y=form_rows,width=cols,height=1}
  end
  page_rows = math.max(0, rows-form_rows-2)
  local content
  content, line_count = markdown:render{x=0,y=form_rows+2,width=cols,height=page_rows,scroll=scroll}
  return frame .. content
end
return M
