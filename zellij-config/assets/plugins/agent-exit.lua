-- Agent exit attaches a project-owned Swamp workflow. The source is retained until Leave.
local z = require('zellij')
local M = {status='Agent exited.', busy=false}
local view=require('zellij.workflow_view')
local launch_rpc,launch_ready
function M.load(config)
  M.pane=tonumber(config.agent_pane_id)
  M.launch=config.agent_exit_token
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
    M.status=(not ok and tostring(snapshot) or snapshot.error or 'No exit workflow configured.') .. '\nUse Hocus /project-setup to configure .zellij/swamp-workflows.kdl.'
    return
  end
  view.active=true
  -- Offered at every step, the launch included; the bridge cancels any run still going, and
  -- one still dispatching, once the choice lands.
  local function pane_action(action,key)
    if M.acting or view.submitting then return end
    M.acting=true
    z.task(function()
      local reply=z.await(z.rpc.call{plugin_url='zellij:swamp-bridge',name='swamp',payload={
        method='pane_action',work=M.launch,key=key,action=action,cancel_active=true}})
      M.acting=false
      if reply.error then
        view.action,view.notice,view.notice_error=action,reply.error,true
        z.request_render()
      end
    end)
  end
  view.load{
    on_restart=function() pane_action('restart','restart') end,
    -- A cancelled picker must allow another fork, so each press has its own key. The bridge
    -- folds a press into a picker still open for this pane rather than opening a second one.
    on_fork=function() pane_action('fork','fork-'..z.monotonic_ms()) end,
  }
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
    view.attach{directory=M.directory,launch=token}
    launch_ready=false
    launch_rpc=z.rpc.call {timeout=90,plugin_url='zellij:swamp-bridge',name='swamp',payload={method='workflow_run',work=token,key='launch',directory=M.directory,inputs={sourcePane=M.pane,kind=config.agent_kind or '',firecracker=config.agent_firecracker_vm or ''}}}
    local reply=z.await(launch_rpc)
    launch_rpc=nil
    if reply.error then
      view.status=reply.error
      -- The launch may have dispatched before the response failed. Keep reconciling.
    end
    view.attach{directory=M.directory,launch=token}
    M.busy=false
    z.request_render()
  end)
end
function M.pipe(message)
  if not M.workflow then return end
  if message.name=='workflow-progress' and message.payload and launch_rpc and not launch_ready then
    local progress=z.json.decode(message.payload)
    if not progress.submission then
      launch_ready=progress.status=='ready' and progress.operation=='workflow.run'
      launch_rpc:reset_timeout(90)
    end
  end
  view.pipe(message)
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
      (M.busy or view.active or view.submitting) and 'Enter: activate button. Restart/Fork: cancel run and continue. Cancel run: explicit cancellation.'
      or 'Enter: activate button  Delete: leave and close held pane',cols)):dim():render{x=0,y=math.max(0,rows-1),width=cols,height=1}
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
