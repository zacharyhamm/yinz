-- Agent exit attaches a project-owned Swamp workflow. The held pane and its scrollback survive.
local z = require('zellij')
local M = {status='Agent exited.', busy=false}
local view=require('zellij.workflow_view')
function M.load(config)
  M.pane=tonumber(config.agent_pane_id)
  M.directory=config.agent_firecracker_worktree or config.agent_cwd
  z.subscribe {'key'}
  if config.swamp_owner or config.agent_workflow_owner then
    M.status='Managed helper exited. Its owning operation receives the exit event.'
    return
  end
  if not M.directory or M.directory=='' or config.agent_target=='ssh' then
    M.status='No local control checkout is available. Enter: restart. Esc: return to scrollback.'
    return
  end
  local ok, snapshot=pcall(z.project_config.read,M.directory)
  if not ok or snapshot.error or not snapshot.config.workflow then
    M.status=(not ok and tostring(snapshot) or snapshot.error or 'No exit workflow configured.') .. '\nUse Hocus /project-setup to configure .zellij/.swamp-workflows.kdl.'
    return
  end
  view.active=true
  view.load{on_restart=function()
    if not M.busy and not view.active and not view.submitting and #view.pending==0 then
      z.rerun_command_pane(M.pane)
      z.close_self()
    end
  end}
  view.status='Starting workflow...'
  M.workflow=true
  M.busy=true
  z.task(function()
    -- load() can run before the screen places this hook. Register observation before
    -- checking, so an already-running bridge never authenticates an unplaced pane.
    local pane=z.plugin_pane(z.get_plugin_ids().plugin_id)
    local function visible()
      local ok,info=pcall(z.get_pane_info,pane)
      return ok and info and not info.is_suppressed
    end
    local placed=z.async.event('pane_update',visible)
    if not visible() then z.await(placed) end
    -- An exit reloaded in the same server attaches through the same launch claim.
    local token=assert(config.agent_exit_token, 'Missing host exit correlation token')
    local reply=z.await(z.rpc.call {plugin_url='zellij:swamp-bridge',name='swamp',timeout=30,payload={method='workflow_run',work=token,key='launch',directory=M.directory,inputs={sourcePane=M.pane,kind=config.agent_kind or '',firecracker=config.agent_firecracker_vm or ''}}})
    if reply.error then
      view.status=reply.error
      -- The launch may have dispatched before the response failed. Keep reconciling.
    end
    view.attach{directory=M.directory,launch=token}
    M.busy=false
    z.request_render()
  end)
end
function M.update(event)
  if M.workflow then
    local active=M.busy or view.active or view.submitting
    if event.name=='key' and #view.pending==0 and event.key=='Delete' then
      if not active then z.close_terminal_pane(M.pane);z.close_self() end
      return true
    end
    return view.update(event)
  end
  if event.name~='key' then return false end
  if event.key=='Esc' then z.close_self()
  elseif event.key=='Enter' and not M.busy then
    if M.pane then z.rerun_command_pane(M.pane) end
    z.close_self()
  elseif event.key=='Delete' and not M.busy then
    if M.pane then z.close_terminal_pane(M.pane) end
    z.close_self()
  end
  return true
end
function M.render(rows,cols)
  if M.workflow then
    return view.render(math.max(0,rows-1),cols) .. z.ui.text(z.text.truncate(
      (M.busy or view.active) and 'Workflow active. Esc: scrollback (detach). Cancel run: explicit cancellation.'
      or 'Enter: activate button  Esc: scrollback  Delete: close held pane',cols)):dim():render{x=0,y=math.max(0,rows-1),width=cols,height=1}
  end
  local lines={}
  for line in M.status:gmatch('[^\n]+') do lines[#lines+1]=line end
  lines[#lines+1]='Directory: '..(M.directory or 'unknown')
  lines[#lines+1]='Enter: restart agent  Esc: scrollback  Delete: close held pane'
  local frame={}
  for i=1,math.min(rows,#lines) do
    frame[#frame+1]=z.ui.text(z.text.truncate(z.text.sanitize(lines[i]),cols)):render{x=0,y=i-1,width=cols,height=1}
  end
  return table.concat(frame)
end
return M
