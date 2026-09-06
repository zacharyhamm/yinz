-- Agent exit attaches a project-owned Swamp workflow. The held pane and its scrollback survive.
local z = require('zellij')
local M = {status='Agent exited.', busy=false}
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
  M.busy=true
  z.task(function()
    -- An exit reloaded in the same server attaches through the same launch claim.
    local token=assert(config.agent_exit_token, 'Missing host exit correlation token')
    local reply=z.await(z.rpc.call {plugin_url='zellij:swamp-bridge',name='swamp',timeout=30,payload={method='workflow_run',work=token,key='launch',directory=M.directory,inputs={firecracker=config.agent_firecracker_vm or ''}}})
    if reply.error then M.status=reply.error
    else
      M.status='Workflow launched. Its command pane runs independently of this view.'
      z.message_to_plugin {plugin_url='zellij:swamp-workflow',message_name='show',message_payload=z.json.encode({directory=M.directory,launch=token}),new_plugin_args={should_float=true,should_focus=true}}
    end
    M.busy=false
    z.request_render()
  end)
end
function M.update(event)
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
