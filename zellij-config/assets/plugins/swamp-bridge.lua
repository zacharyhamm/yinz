-- Durable delivery adapter. Swamp owns the DAG, evidence, questions and decisions.
-- Only dispatch claims, ownership, attachment and hook/exit delivery live here.
local z = require('zellij')
local M = {}
local D = z.data
local identity
local function encode(v) return z.json.encode(v) end
local function decode(v) return z.json.decode(v) end
local function key(v)
  assert(type(v) == 'string' and #v > 0 and #v <= 200 and v:match('^[%w][%w_.:%-]*$'), 'Invalid correlation key')
  return v
end
local function fetch(uid)
  local rows = D.fetch { entity='delivery', filter=D.eq('uid', uid), limit=1 }
  return rows[1]
end
local function read(uid)
  local row = fetch(uid)
  if row then return decode(row.value), row end
end
local function put(uid, value, row)
  D.apply(D.put('delivery', {value=encode(value)}, uid, row and row.rev or D.REV_ABSENT))
end
local function scope(work, operation)
  return encode({identity.host_id, identity.session, key(work), key(operation)})
end
local function result(uid)
  local value = assert(read(uid), 'No operation with that identity')
  return value
end
local function save_event(uid, outcome, fields)
  local value, row = read(uid)
  if not value or (value.outcome ~= 'pending' and value.outcome ~= 'unknown') then return end
  value.outcome = outcome
  for k, v in pairs(fields or {}) do value[k] = v end
  put(uid, value, row)
end
-- Placement belongs to an authenticated builtin view, never to a caller-supplied pane.
-- It is intentionally not recovered across server/bridge reloads.
local placements={}
local manual_views={}
local function pane_subtitle(pane, subtitle)
  z.set_pane_subtitle{pane_id=pane,subtitle=subtitle}
end
local function detach(placement)
  if placement.detached then return end
  placement.detached=true
  pane_subtitle(z.plugin_pane(placement.view),nil)
  if placement.current then pane_subtitle(z.terminal_pane(placement.current),nil) end
  placement.subtitle=nil
  z.message_to_plugin{destination_plugin_id=placement.view,message_name='workflow-detached'}
end
-- History is the authority; delivery records only decide which pane is foregrounded.
local function workflow_subtitle(snapshot)
  local main=snapshot.history
  local function steps(histories, active)
    local entries={}
    for _,history in ipairs(histories) do
      for _,job in ipairs(history.jobs or {}) do
        for _,step in ipairs(job.steps or {}) do
          local running=(history.status=='running' or history.status=='suspended') and
            (step.status=='running' or step.status=='waiting_approval')
          if (active and running) or (not active and step.status=='failed') then
            local name=job.name..'/'..step.name
            if history~=main then name=(history.workflowName or history.id)..'/'..name end
            if not active and main.status~='failed' then name=name..' (failed)' end
            entries[#entries+1]=name..' · '..(active and step.status or main.status)
          end
        end
      end
    end
    table.sort(entries)
    return entries
  end
  local entries=steps(snapshot.children or {},true)
  if #entries==0 then entries=steps({main},true) end
  if #entries==0 then
    entries=steps(snapshot.children or {},false)
    if #entries==0 then entries=steps({main},false) end
  end
  return (main.workflowName or 'Workflow')..' · '..
    (#entries>0 and table.concat(entries,', ') or main.status)
end
local launch_waiting={}
local delivery_rows, cancel_lifecycle
local cancelling={}
local function lifecycle_work(work)
  local attachment=read(scope(work,'attachment'))
  return attachment and attachment.work or work
end
local function closed(work)
  if not work or work==z.null then return false end
  local view=read(scope(lifecycle_work(work),'view'))
  return view and view.closed
end
local function ensure_open(work)
  assert(not closed(work),'Pane workflow lifecycle is closed')
end
local function transition(work, action)
  local uid=scope(lifecycle_work(work),'view')
  local view,row=read(uid)
  if view then
    ensure_open(work)
    view.transition=action
    put(uid,view,row)
  end
end
-- A transition keeps the lifecycle out of close_lifecycle's cancellation. A restart or fork
-- chosen mid-workflow abandons it instead: the tombstone refuses later steps and cancels runs
-- that attach late, so no run keeps working beside the new agent. The source pane stays open.
local function cancel_after_transition(work)
  work=lifecycle_work(work)
  local uid=scope(work,'view')
  local view,row=read(uid)
  if not view then return end
  view.closed,view.cancel_pending=true,true
  put(uid,view,row) -- durable before yielding, so a reload resumes the cancellation
  -- A picker still open can no longer be accepted. Its late result closes it.
  for _,entry in ipairs(delivery_rows()) do
    local value=entry.value
    if (value.outcome=='pending' or value.outcome=='unknown') and value.request
      and lifecycle_work(entry.work)==work then
      local request=decode(value.request)
      if request.method=='pane_action' and (request.action=='fork' or request.action=='duplicate') then
        value.outcome,value.error='interrupted','Pane workflow lifecycle is closed'
        put(entry.uid,value,entry)
      end
    end
  end
  z.task(function() cancel_lifecycle(work) end)
end
local function close_view(work, restore_source)
  if placements[work] then detach(placements[work]) end
  local placement=read(scope(work,'view'))
  if not placement or placement.incarnation~=identity.incarnation then return end
  local ok,pane=pcall(z.get_pane_info,z.plugin_pane(placement.view))
  if ok and pane and pane.plugin_url=='zellij:agent-exit' then
    if restore_source and not pane.is_suppressed then
      z.replace_pane_with_existing_pane{z.plugin_pane(placement.view),z.terminal_pane(restore_source),true}
    end
    -- A restored source must not remain linked to the view being destroyed.
    z.hide_pane_with_id(z.plugin_pane(placement.view))
    z.close_plugin_pane(placement.view)
  end
end
local function active(work)
  if closed(work) then return false end
  local works={[work]=true}
  local runs=read(scope(work,'runs'))
  for id in pairs(runs and runs.entries or {}) do works[id]=true end
  local cursor
  repeat
    local rows,page=D.fetch{entity='delivery',after_id=cursor}
    for _,row in ipairs(rows) do
      local id=decode(row.uid)
      if id[1]==identity.host_id and id[2]==identity.session and works[id[3]] then
        local operation=decode(row.value)
        if operation.outcome=='pending' or operation.outcome=='unknown' then
          local method=decode(operation.request).method
          -- workflow_run copies the runner's outcome but does not track its completion.
          if method=='command' or method=='agent_prompt' or (method=='agent_spawn' and decode(operation.request).waitForExit) then return true end
        end
      end
    end
    cursor=page.next_cursor
  until not cursor or cursor==z.null
  return false
end
local function launch_failure(work, launch, logs, directory)
  return {launch=work,root=launch.checkout or directory,history={id=work,workflowName=launch.workflow or 'Workflow launch',status='failed'},
    children={},active=active(work),logs=logs}
end
local function present(work)
  if closed(work) then return end
  local placement=placements[work]
  if not placement or placement.detached then return end
  local view=z.plugin_pane(placement.view)
  local ok,info=pcall(z.get_pane_info,view)
  if not ok or not info or (info.plugin_url~='zellij:agent-exit' and info.plugin_url~='zellij:swamp-workflow') then
    detach(placement)
    return
  end
  local current=placement.current and z.terminal_pane(placement.current) or view
  local ok,current_info=pcall(z.get_pane_info,current)
  if not ok or not current_info or current_info.is_suppressed then
    detach(placement)
    return
  end
  local next_pane
  while #placement.queue>0 do
    local operation=read(placement.queue[1])
    if operation and operation.outcome=='pending' then
      next_pane=operation.pane
      break
    end
    table.remove(placement.queue,1)
  end
  pane_subtitle(view,placement.subtitle)
  if next_pane==placement.current then
    if next_pane then pane_subtitle(current,placement.subtitle) end
    return
  end
  local target=next_pane and z.terminal_pane(next_pane) or view
  local ok,target_info=pcall(z.get_pane_info,target)
  if not ok or not target_info or not target_info.is_suppressed then
    detach(placement)
    return
  end
  if not placement.current and next_pane then
    local source=read(scope(work,'source'))
    -- Keep the source for pane/VM actions, without restoring it when the view closes.
    if source then z.hide_pane_with_id(z.terminal_pane(source.pane)) end
  end
  if placement.current then pane_subtitle(current,nil) end
  pane_subtitle(target,placement.subtitle)
  local uid=scope(work,'view')
  local saved,row=read(uid)
  if saved then saved.current=next_pane;put(uid,saved,row) end
  z.replace_pane_with_existing_pane{current,target,true}
  placement.current=next_pane
end
local function update_subtitle(work, subtitle, workflow)
  local placement=placements[work]
  if not placement or placement.detached then return end
  placement.subtitle=subtitle
  placement.workflow=workflow or placement.workflow
  present(work)
end
local function unavailable(placement,work)
  update_subtitle(work,(placement.workflow or 'Workflow')..' · status unavailable')
end
local function attach_manual(work, view)
  if closed(work) then return end
  manual_views[work]=manual_views[work] or {}
  manual_views[work][view]=true
  if read(scope(work,'view')) then return end
  local existing=placements[work]
  if existing then
    local ok,pane=pcall(z.get_pane_info,z.plugin_pane(existing.view))
    if ok and pane then return end
    detach(existing)
  end
  local placement={view=view,queue={},manual=true,subtitle="Workflow · loading"}
  placements[work]=placement
  local cursor
  repeat
    -- Default row-id order preserves delivery order across pages.
    local rows,page=D.fetch{entity='delivery',after_id=cursor}
    for _,row in ipairs(rows) do
      local id=decode(row.uid)
      if id[1]==identity.host_id and id[2]==identity.session and id[3]==work then
        local operation=decode(row.value)
        if operation.outcome=='pending' and operation.pane then
          local request=decode(operation.request)
          if request.method=='command' and request.foreground then
            placement.queue[#placement.queue+1]=row.uid
          end
        end
      end
    end
    cursor=page.next_cursor
  until not cursor or cursor==z.null
  present(work)
end
local function finish_agent(owner, event, raw)
  local agent_uid = scope(owner.work, owner.agent)
  local agent, row = read(agent_uid)
  if not agent or encode(agent.owner) ~= encode(owner) or not agent.outstanding then return end
  local uid = agent.outstanding
  local terminal = {Stop='completed', StopFailure='failed', Interrupt='interrupted'}
  local outcome = terminal[event.hook]
  if not outcome then return end
  -- A prompt must have been accepted before its ending hook. Late previous turns cannot
  -- complete a newly claimed prompt just because the pane and agent session match.
  local operation = result(uid)
  if not operation.accepted or operation.agentSession ~= event.session_id then return end
  if operation.turn and operation.turn ~= raw.turn_id then return end
  if decode(operation.request).waitForExit then
    local current,row=read(uid)
    current.message,current.hook=event.last_message or z.null,event.hook
    put(uid,current,row)
    return
  end
  save_event(uid, outcome, {message=event.last_message or z.null, hook=event.hook, agentSession=event.session_id or z.null})
  agent.outstanding = nil
  put(agent_uid, agent, row)
  present(owner.work)
end
local function command(argv, cwd)
  local r = z.await(z.async.run_command {command=argv, cwd=cwd or '/', context={swamp_owner=encode(identity)}, max_bytes_returned=1048576})
  if r.exit_code ~= 0 then
    error('Host command failed: ' .. encode(argv) .. '\nDirectory: ' .. (cwd or '/') ..
      '\nExit status: ' .. tostring(r.exit_code) .. '\nstdout:\n' .. (r.stdout or '') .. '\nstderr:\n' .. (r.stderr or ''))
  end
  return r.stdout
end
local tracked,notifications,watching,watch_ids={},{},{},{}
local function changed(work)
  if closed(work) then return end
  if notifications[work] then return end
  notifications[work]=true
  z.after(0,function() -- Coalesce this event-loop turn; fractional timers round up to a second.
    notifications[work]=nil
    if closed(work) then return end
    local view=read(scope(work,'view'))
    if view and view.incarnation==identity.incarnation then
      z.message_to_plugin{destination_plugin_id=view.view,message_name='workflow-changed'}
    end
    for id in pairs(manual_views[work] or {}) do
      local ok,pane=pcall(z.get_pane_info,z.plugin_pane(id))
      if not ok or not pane then manual_views[work][id]=nil
      elseif not view or view.incarnation~=identity.incarnation or id~=view.view then
        z.message_to_plugin{destination_plugin_id=id,message_name='workflow-changed'}
      end
    end
  end)
end
local function swamp(request,root)
  local reply=z.await(z.async.swamp_request{directory=root,operation=request.operation,payload=request.payload,
    id=request.id,started=request.started,on_progress=request.on_progress})
  if reply.error then
    if reply.error.code=='not_dispatched' and request.on_not_dispatched then request.on_not_dispatched(reply.error) end
    error(encode(reply.error))
  end
  if request.operation=='data.query' and reply.limited then error('Swamp data query reached its result limit; narrow the saved-data query') end
  if request.operation=='data.query' and reply.projected then
    reply.results=reply.projected.rows or reply.projected.values or {}
    reply.projected=nil
  elseif request.operation=='workflow.history.logs' and reply.log then
    return table.concat(reply.log.lines or {},'\n')
  end
  return encode(reply)
end
local function cancel_run(run,root)
  local history=decode(swamp({operation='workflow.history.get',payload={workflowIdOrName=run}},root))
  if history.status=='running' then
    return decode(swamp({operation='workflow.cancel',payload={runId=run}},root))
  end
end
local function cancel_if_closed(work)
  if not closed(work) then return false end
  work=lifecycle_work(work)
  local uid=scope(work,'view')
  local view,row=read(uid)
  view.cancel_pending=true
  put(uid,view,row)
  z.task(function() cancel_lifecycle(work) end)
  return true
end
local function launch_progress(work,status,operation)
  if closed(work) or not read(scope(work,'launch')) then return end
  local waiting=launch_waiting[work]
  if waiting then waiting.timer:cancel() end
  launch_waiting[work]=nil
  local function send()
    local view=read(scope(work,'view'))
    if view and view.incarnation==identity.incarnation then
      z.message_to_plugin{destination_plugin_id=view.view,message_name='workflow-progress',message_payload=encode{status=status,operation=operation}}
    end
  end
  if status~='ready' then launch_waiting[work]={status=status,operation=operation,timer=z.every(10,send)} end
  changed(work)
  send()
end
local function watch_run(run,work,root)
  if watching[run] then return end
  -- Dispatch already streams these events. A second attachment replays them.
  for _,uid in pairs(tracked) do
    local operation=read(uid)
    if operation and operation.run==run then return end
  end
  local id=z.instance_id..'-watch-'..run
  watching[run]=true;watch_ids[id]={run=run,work=work}
  z.task(function()
    pcall(swamp,{operation='run.attach',payload={runId=run,afterSeq=0},id=id},root)
    watching[run]=nil;watch_ids[id]=nil
  end)
end
local function dispatch(uid,root,operation,payload,callback)
  payload=z.json.decode(encode(payload),true)
  local value,row=read(uid)
  ensure_open(value.work)
  value.transport,value.requestId,value.checkout='serve',z.instance_id..uid,root
  value.correlation=uid
  value.outcome='unknown'
  put(uid,value,row) -- durable identity before the socket can receive a mutation
  tracked[value.requestId]=uid
  if operation=='workflow.run' then
    payload.noSupersede=true
    payload.runtimeTags=payload.runtimeTags or {}
    payload.runtimeTags.zellijOperation=uid
  end
  -- A continuation is part of an execution already admitted by Swamp. Holding
  -- its callback would prevent its parent from finishing and the server draining.
  if callback then payload._zellij_active_callback=true end
  local reply=decode(swamp({operation=operation,payload=payload,id=value.requestId,started=true,
    on_progress=function(status) launch_progress(value.work,status,operation) end,
    on_not_dispatched=function(err)
      launch_progress(value.work,'ready',operation)
      local current,current_row=read(uid)
      current.not_dispatched=true
      if current.outcome=='pending' or current.outcome=='unknown' then
        current.outcome,current.error='failed',err.message
      end
      put(uid,current,current_row)
    end},root))
  value,row=read(uid)
  if reply.runId then value.run=reply.runId end
  if value.outcome=='unknown' and tracked[value.requestId] then value.outcome='pending' end
  put(uid,value,row)
  if closed(value.work) then
    if operation~='model.method.run' then cancel_if_closed(value.work) end
    ensure_open(value.work)
  end
  return value
end
local function workflow_run_id(operation,request)
  if operation.run then return operation.run end
  if request.operation=='workflow.resume' then return request.payload.runId end
  local history=decode(swamp({operation='workflow.history.search',payload={filter='has(tags.zellijOperation) && tags.zellijOperation == '..encode(operation.correlation)}},operation.checkout))
  assert(#(history.results or {})<=1,'Multiple runs have the same operation identity; reconcile explicitly')
  return history.results and history.results[1] and history.results[1].runId
end
local function serve_observe(uid,operation,row)
  local attachment
  for attempt=1,8 do
  if operation.outcome~='pending' and operation.outcome~='unknown' then return operation end
  local request=decode(operation.request)
  if request.operation=='model.method.run' then
    if operation.run then
      local id=z.instance_id..'-attach-'..uid
      local tracking=false
      for _,tracked_uid in pairs(tracked) do if tracked_uid==uid then tracking=true;break end end
      if not tracking and not attachment then
        tracked[id]=uid -- replay can arrive before the request returns
        attachment=table.pack(pcall(swamp,{operation='run.attach',payload={runId=operation.run,afterSeq=0},id=id,started=true},operation.checkout))
        if not attachment[1] then tracked[id]=nil end
      end
      if attachment then
        operation.outcome=attachment[1] and tracked[id] and 'pending' or 'unknown'
        if attachment[1] then operation.dispatcher=z.instance_id
        else operation.error=tostring(attachment[2]) end
      end
    else operation.outcome='unknown' end
  else
  operation.run=workflow_run_id(operation,request)
  if operation.run and request.operation~='model.method.run' then
    local history=decode(swamp({operation='workflow.history.get',payload={workflowIdOrName=operation.run}},operation.checkout))
    local matches=request.operation~='workflow.resume' or (history.inputs and history.inputs.submission==request.payload.inputs.submission)
    if matches then
      if history.status=='running' then
        if operation.outcome=='unknown' or operation.dispatcher~=z.instance_id then
          local attached=pcall(swamp,{operation='run.attach',payload={runId=operation.run,afterSeq=0},started=true},operation.checkout)
          operation.outcome=attached and 'pending' or 'unknown'
          if attached then operation.dispatcher=z.instance_id else operation.error='Execution is unavailable; reconcile persisted history before proceeding' end
        else operation.outcome='pending' end
      elseif history.status=='succeeded' or history.status=='suspended' then operation.outcome='completed';operation.exitCode=0
      elseif history.status=='failed' or history.status=='cancelled' or history.status=='interrupted' then operation.outcome='failed';operation.exitCode=1 end
    end
  end
  end
  local ok,err=pcall(put,uid,operation,row)
  if ok then return operation end
  if not D.is_conflict(err) or attempt==8 then error(err) end
  -- An event can advance the claim while history is in flight. Re-read and
  -- reconcile again; never attach a fresh revision to the stale observation.
  operation,row=read(uid)
  end
end
local refresh_events={started=true,step_completed=true,step_failed=true,suspended=true,
  completed=true,cancelled=true,error=true,report_completed=true,report_failed=true}
local function changes_snapshot(frame)
  return frame.type=='done' or frame.type=='error' or (frame.event and refresh_events[frame.event.kind])
end
z.on('custom_message',function(event)
  if event.message~='swamp-response' then return end
  local frame=decode(event.payload)
  local watch=watch_ids[frame.id]
  if watch then
    if changes_snapshot(frame) then changed(watch.work) end
    return
  end
  local uid=tracked[frame.id]
  if not uid then return end
  local operation,row=read(uid)
  if not operation then return end
  local write=false
  local pending=operation.outcome=='pending' or operation.outcome=='unknown'
  local previous_run=operation.run
  local method=decode(operation.request).operation=='model.method.run'
  if frame.event and frame.event.runId then
    operation.run=frame.event.runId
    if pending then operation.outcome='pending' end
    write=true
  end
  if pending and frame.event and (frame.event.kind=='completed' or frame.event.kind=='cancelled' or (not method and frame.event.kind=='suspended')) then
    local status=frame.event.run and frame.event.run.status
    local succeeded=frame.event.kind~='cancelled' and (status=='succeeded' or (not method and status=='suspended'))
    operation.outcome=succeeded and 'completed' or 'failed'
    operation.exitCode=succeeded and 0 or 1
    if not succeeded then operation.error=encode(frame.event) end
    write=true
  end
  if pending and frame.event and frame.event.kind=='error' then
    operation.error=encode(frame.event.error)
    operation.outcome='failed';operation.exitCode=1;write=true
  end
  if frame.type=='done' then
    if operation.outcome=='pending' or operation.outcome=='unknown' then
      operation.outcome='unknown'
    end
    tracked[frame.id]=nil;write=true
  elseif frame.type=='error' then
    if pending then
    operation.error=encode(frame.error)
    local attaching=method and frame.id==z.instance_id..'-attach-'..uid
    operation.outcome=(attaching or frame.error.code=='transport_unknown' or frame.error.code=='host_error' or frame.error.code=='not_found') and 'unknown' or 'failed'
    end
    tracked[frame.id]=nil;write=true
  end
  if write then put(uid,operation,row) end
  if closed(operation.work) then
    if not method and operation.run and operation.run~=previous_run then cancel_if_closed(operation.work) end
    return
  end
  if write or changes_snapshot(frame) then
    local attached=read(scope(operation.work,'attachment'))
    changed(attached and attached.work or operation.work)
  end
end)
local function commands(requests,root)
  local results={}
  for first=1,#requests,4 do
    local pending={}
    for i=first,math.min(first+3,#requests) do
      local future,resolve=z.async.future()
      pending[i]=future
      z.task(function() resolve(table.pack(pcall(swamp,requests[i],root))) end)
    end
    for i,result in pairs(z.await(z.async.all(pending))) do results[i]=result end
  end
  local output={}
  for i,result in ipairs(results) do
    if not result[1] then error(result[2]) end
    output[i]=result[2]
  end
  return output
end
local function supervisor(pane, saved)
  for _,row in ipairs(z.workflow_remote_panes and z.workflow_remote_panes() or {}) do
    if (saved and row.host_id==saved) or (not saved and row.terminal_id==pane) then
      row.session_name=identity.session
      row.attached=true
      local info=z.get_pane_info(z.terminal_pane(row.terminal_id))
      if info.exited then row.child_exit_status=info.exit_status or z.null end
      return row
    end
  end
  local rows = decode(command({z.server_binary or 'zellij', 'pty-host', 'list', '--json'}))
  for _, row in ipairs(rows) do
    if row.session_name == identity.session and ((saved and row.host_id == saved) or (not saved and row.terminal_id == pane)) then return row end
  end
  error('No supervisor identity for pane; reconcile before sending input')
end
local function owned(args, running)
  ensure_open(args.work)
  local agent = result(scope(args.work, args.agent))
  assert(agent.owner and agent.owner.host_id == identity.host_id and agent.owner.session == identity.session, 'Wrong host or session')
  assert(agent.owner.work == args.work and agent.owner.agent == args.agent, 'Wrong work or agent')
  assert(agent.pane and agent.supervisor, 'Agent dispatch is unknown; reconcile it first')
  if args.allow_closed and agent.owner.incarnation==identity.incarnation
    and not z.get_pane_info(z.terminal_pane(agent.pane)) then return end
  local current = supervisor(agent.pane, agent.supervisor)
  ensure_open(args.work)
  assert(current.host_id == agent.supervisor, 'Pane belongs to a different supervisor')
  assert(current.child_pid == agent.process, 'Agent process changed')
  assert(current.attached, 'Supervisor is detached; adopt and reconcile before sending input')
  if current.owner then assert(current.owner==(agent.transportOwner or encode(agent.owner)),'Remote ownership changed') end
  if current.terminal_id ~= agent.pane then
    local uid=scope(args.work,args.agent)
    local current_agent,row=read(uid)
    current_agent.pane=current.terminal_id
    put(uid,current_agent,row)
    agent=current_agent
  end
  if running then assert(not agent.processExited and not current.child_exit_status, 'Agent process exited') end
  return agent
end
local function dispatch_prompt(agent)
  if closed(agent.owner.work) then return end
  if not agent.ready or not agent.outstanding then return end
  local operation,row=read(agent.outstanding)
  if not operation or operation.outcome~='pending' or operation.dispatched then return end
  operation.dispatched=true
  put(agent.outstanding,operation,row) -- a crash after this point is an uncertain send
  z.agent.send(z.terminal_pane(agent.pane),decode(operation.request).prompt)
end
local function claim(args)
  ensure_open(args.work)
  local uid = scope(args.work, args.key)
  local previous = read(uid)
  local request = encode(args)
  if previous then
    assert(previous.request == request, 'Conflicting arguments for existing operation')
    return uid, previous
  end
  local value = {request=request, key=args.key, work=args.work, outcome='unknown', incarnation=identity.incarnation, dispatcher=z.instance_id}
  local ok, err = pcall(put, uid, value)
  if not ok then
    if not D.is_conflict(err) then error(err) end
    previous = result(uid)
    assert(previous.request == request, 'Conflicting concurrent operation')
    return uid, previous
  end
  return uid
end
local function open_picker(work, key, action, source_pane)
  z.message_to_plugin{plugin_url='zellij:hocus-new-agent',message_name='swamp-pane-action',
    plugin_config={caller_terminal_pane_id=tostring(source_pane),[action=='fork' and 'fork_focused' or 'duplicate_focused']='true',
      swamp_reply_plugin=tostring(z.get_plugin_ids().plugin_id),swamp_work=work,swamp_key=key},
    new_plugin_args={should_float=true,should_focus=true}}
end
-- A picker for this action that the view's own button opened, still waiting on its result.
-- The workflow's pickers never cancel the lifecycle, so a press never folds into one of them.
local function open_picker_operation(work, action)
  local lifecycle=lifecycle_work(work)
  for _,entry in ipairs(delivery_rows()) do
    local value=entry.value
    if (value.outcome=='pending' or value.outcome=='unknown') and value.request
      and lifecycle_work(entry.work)==lifecycle then
      local request=decode(value.request)
      if request.method=='pane_action' and request.action==action and request.cancel_active then
        return value
      end
    end
  end
end
local handle
local discovered={}
local function discover(work, root)
  local launch=result(scope(work,'launch'))
  assert(launch.checkout==root,'Launch belongs to a different control checkout')
  if launch.not_dispatched then return read(scope(work,'runs')) end
  local history=decode(swamp({operation='workflow.history.search',payload={inputs={launch=work}}},root))
  for _,run in ipairs(history.results or {}) do
    if run.inputs.session==identity.session then
      handle{method='workflow_attach',work=work,key='attachment',run=run.runId,workflow=run.workflowName,directory=root}
    end
  end
  return read(scope(work,'runs'))
end
handle = function(args, manual_view)
  assert(type(args) == 'table', 'Expected a typed request')
  if args.method == 'identity' then return identity end
  if args.method=='swamp_read' then
    if args.operation=='workflow.approve' then ensure_open(args.payload.runId) end
    assert(args.operation=='workflow.history.get' or args.operation=='workflow.history.search' or args.operation=='data.query' or args.operation=='workflow.approve','Unsupported bridge request')
    return decode(swamp({operation=args.operation,payload=args.payload},args.directory))
  end
  if args.method=='workflow_report' then
    assert(type(args.offset)=='number' and args.offset>=0 and args.offset%1==0,'Invalid report offset')
    assert(type(args.version)=='number' and args.version>0 and args.version%1==0,'Invalid report version')
    local predicate='modelName == '..encode(args.model)..' && name == '..encode(args.name)..' && version == '..args.version
    -- Stored lengths use substring's UTF-16 offsets; CEL size() counts code points.
    -- JSON numbers need int() when used as substring arguments.
    local finish=args.offset+32768
    local ending='(attributes.summaryLength <= '..finish..' ? int(attributes.summaryLength) : (attributes.summary.substring('..(finish-1)..','..finish..').matches(r"[\\uD800-\\uDBFF]") ? '..(finish-1)..' : '..finish..'))'
    local projection='{"text":attributes.summary.substring('..args.offset..','..ending..'),"next":'..ending..',"total":attributes.summaryLength}'
    local response=decode(swamp({operation='data.query',payload={predicate=predicate..' && has(attributes.summary) && has(attributes.summaryLength) && attributes.summaryLength >= '..args.offset,select=projection}},args.directory))
    assert(#response.results==1,'Saved test report unavailable at this version')
    return response.results[1]
  end
  if args.method=='workflow_answer' then
    ensure_open(args.inputs.run)
    local heartbeat
    local function progress(status)
      if heartbeat then heartbeat:cancel();heartbeat=nil end
      if not manual_view then return end
      local function send()
        z.message_to_plugin{destination_plugin_id=manual_view,message_name='workflow-progress',
          message_payload=encode{status=status,submission=args.inputs.submission}}
      end
      send()
      if status~='ready' then heartbeat=z.every(10,send) end
    end
    local ok,reply=pcall(function()
      return decode(swamp({operation='model.method.run',payload={modelIdOrName=args.model,methodName='submit',inputs=args.inputs},
        on_progress=progress},args.directory))
    end)
    if heartbeat then heartbeat:cancel() end
    if not ok then error(reply) end
    if reply.run and (reply.run.status=='failed' or reply.run.status=='cancelled') then error(encode(reply.run)) end
    return reply.run or reply
  end
  if args.method == 'workflow_cancel' then
    local snapshot=handle{method='workflow_show',directory=args.directory,run=args.run}
    local histories={snapshot.history}
    for _,child in ipairs(snapshot.children) do histories[#histories+1]=child end
    local operations={}
    for _,history in ipairs(histories) do
      local operation=cancel_run(history.id,snapshot.root)
      if operation then operations[#operations+1]=operation end
    end
    return {operations=operations}
  end
  if args.method == 'workflow_show' then
    local failed=args.launch and args.launch~=z.null and read(scope(args.launch,'launch'))
    if failed and failed.outcome=='failed' and failed.error then
      return launch_failure(args.launch,failed,failed.error,args.directory)
    end
    if failed and launch_waiting[args.launch] then
      return {launch=args.launch,root=failed.checkout or args.directory,history={id=args.launch,workflowName=failed.workflow or 'Workflow launch',status='queued'},
        children={},active=true,logs=launch_waiting[args.launch].status=='waiting' and 'Waiting for active workflows before loading updated extensions.' or 'Loading updated extensions.'}
    end
    local config = z.project_config.read(assert(args.directory, 'Missing control checkout'))
    assert(not config.error, config.error)
    local attachment,work
    if args.run and args.run~=z.null then
      attachment=read(scope(args.run,'attachment'))
      if attachment then work=attachment.work
      else
        local history=decode(swamp({operation='workflow.history.get',payload={workflowIdOrName=args.run}},config.root))
        assert(history.inputs.session==identity.session,'Run belongs to a different session')
        work=history.inputs.launch
      end
    elseif args.launch and args.launch~=z.null then work=args.launch end
    if closed(work) then
      return {run=args.run,launch=work,root=config.root,history={id=args.run or work,status='cancelled'},
        children={},active=false,questions={results={}},outputs={results={}},logs='Pane workflow lifecycle is closed.'}
    end
    local runs=work and read(scope(work,'runs'))
    if work then
      assert(result(scope(work,'launch')).checkout==config.root,'Launch belongs to a different control checkout')
      -- Reconcile retained history once after loading the bridge. Every workflow
      -- attaches new runs durably before opening a question; reread that registry
      -- and the run states on each refresh. Explicit recovery still discovers.
      if not discovered[work] or not runs then
        runs=discover(work,config.root)
        discovered[work]=true
      end
    end
    if work and manual_view then attach_manual(work,manual_view) end
    if args.run and args.run~=z.null then attachment=read(scope(args.run,'attachment'))
    elseif runs then
      for id,entry in pairs(runs.entries) do
        if entry.workflow=='@hammz/zellij-agent-exit' or not attachment then attachment=entry;attachment.run=id end
      end
    end
    if not attachment and work then
      local runner=read(scope(work,'swamp-process'))
      if runner and (runner.outcome=='completed' or runner.outcome=='failed' or runner.outcome=='process-exit') then
        runner=handle{method='observe',work=work,key='swamp-process'}
        local logs='Workflow runner exited without reporting a run. Exit status: '..tostring(runner.exitCode)
        if runner.output then logs=logs..'\nRetained command scrollback (may omit older output):\n'..runner.output end
        if runner.logError then logs=logs..'\nLog unavailable: '..runner.logError end
        return launch_failure(work,result(scope(work,'launch')),logs)
      end
    end
    assert(attachment,'Launch has no reported run yet; reconcile by launch token without repeating it')
    local run=attachment.run
    local runs=read(scope(attachment.work,'runs'))
    local ids={}
    for id in pairs(runs.entries) do ids[#ids+1]=id end
    local predicate='specName == "question" && attributes.run in '..encode(ids)
    -- Bound every saved log copy before the host serializes the response; its byte cap would cut JSON.
    -- CEL substring uses UTF-16 offsets: back off if the cut would split a surrogate pair.
    local function preview(path,field,limit)
      field=field or 'output';limit=limit or 16384
      return ([["output":has(VALUE.output) ? (VALUE.output.size() > 16384 ? VALUE.output.substring(0,
        VALUE.output.substring(16383,16384).matches(r"[\uD800-\uDBFF]") ? 16383 : 16384) : VALUE.output) : "",
        "previewTruncated":has(VALUE.output) && VALUE.output.size() > 16384]]):gsub('output',field):gsub('16383',tostring(limit-1)):gsub('16384',tostring(limit)):gsub('VALUE',path)
    end
    local function test_preview(path)
      return ('{"evidence":'..path..'.evidence,"outcome":'..path..'.outcome,"exitCode":'..path..'.exitCode,'..
        '"summaryLength":has('..path..'.summaryLength) ? '..path..'.summaryLength : 0,'..
        '"outputComplete":has('..path..'.outputComplete) && '..path..'.outputComplete,'..
        '"log":'..path..'.log,"passed":'..path..'.passed,"outputRef":has('..path..'.outputRef) ? '..path..'.outputRef : null,'..preview(path)..'}')
    end
    local output_projection=[[{"model":modelName,"name":name,"version":version,"createdAt":createdAt,"attributes":
      has(tags.type) && tags.type == "report" ? {} :
      specName == "claim" ? {"key":attributes.key,"fingerprint":attributes.fingerprint,"outcome":attributes.outcome} :
      specName == "commandOutput" ? {} :
      specName == "rebaseResult" ? {"exitCode":attributes.exitCode,"evidence":attributes.evidence,
        "outputRef":has(attributes.outputRef) ? attributes.outputRef : null,
        "operation":has(attributes.operation) ? {
          "key":attributes.operation.key,"work":attributes.operation.work,"outcome":attributes.operation.outcome,
          "exitCode":has(attributes.operation.exitCode) ? attributes.operation.exitCode : null,
          "log":has(attributes.operation.log) ? attributes.operation.log : "",
          "pane":has(attributes.operation.pane) ? attributes.operation.pane : false} : {},
        ]]..preview('attributes','stdout',2048):gsub('previewTruncated','stdoutTruncated')..','..
        preview('attributes','stderr',2048):gsub('previewTruncated','stderrTruncated')..[[} :
      specName == "diff" ? {"evidence":attributes.evidence,]]..preview('attributes')..[[} :
      specName == "test" ? ]]..test_preview('attributes')..[[ :
      specName == "operation" ? {
        "outcome":attributes.outcome,"pane":has(attributes.pane) ? attributes.pane : false,
        "log":has(attributes.log) ? attributes.log : "",
        "exitCode":has(attributes.exitCode) ? attributes.exitCode : null,
        "outputTruncated":has(attributes.outputTruncated) && attributes.outputTruncated,
        "logError":has(attributes.logError) ? attributes.logError : false,]]..preview('attributes')..[[} :
      specName in ["checks","assessment"] ? {
        "review":attributes.review,
        "test":attributes.test == null ? attributes.test : ]]..test_preview('attributes.test')..[[,
        "evidence":has(attributes.evidence) ? attributes.evidence : null,
        "tests":has(attributes.tests) ? attributes.tests : "",
        "codeReview":has(attributes.codeReview) ? attributes.codeReview : "",
        "ready":has(attributes.ready) && attributes.ready}
      : attributes}]]
    -- Share the attributes expression to stay within Swamp's 4096-character select limit.
    output_projection='[attributes].map(a,'..output_projection:gsub('attributes%.','a.')..')[0]'
    local requests={
      {operation='workflow.history.get',payload={workflowIdOrName=run}},
      {operation='data.query',payload={predicate=predicate,select='{"model": modelName, "version": version, "attributes": attributes}'}},
      {operation='data.query',payload={predicate='has(tags.workflowRunId) && tags.workflowRunId in '..encode(ids),select=output_projection}},
      {operation='workflow.history.logs',payload={runIdOrWorkflow=run,tail=100}},
    }
    for _,id in ipairs(ids) do
      if id~=run then requests[#requests+1]={operation='workflow.history.get',payload={workflowIdOrName=id}} end
    end
    local values=commands(requests,config.root)
    local main_history=decode(values[1])
    if main_history.status=='running' then watch_run(run,work,config.root) end
    local children={}
    for i=5,#values do
      local child=decode(values[i]);children[#children+1]=child
      if child.status=='running' then watch_run(child.id,work,config.root) end
    end
    return {run=run,launch=work,root=config.root,history=decode(values[1]),children=children,active=active(work),
      outputs=decode(values[3]),logs=values[4],questions=decode(values[2])}

  end
  key(args.work)
  if args.method == 'lookup' then return read(scope(args.work,args.key)) or z.null end
  if args.method == 'observe' then
    local uid = scope(args.work,args.key)
    local operation, row = read(uid)
    assert(operation, 'No operation with that identity')
    if operation.transport=='serve' then return serve_observe(uid,operation,row) end
    if operation.outcome=='pending' and operation.dispatcher~=z.instance_id then
      operation.outcome='unknown'
      put(uid,operation,row)
    end
    if args.includeOutput~=false and not operation.capture and operation.log and operation.pane and operation.supervisor then
      local ok,log=pcall(function()
        local host=supervisor(operation.pane, operation.supervisor)
        assert(host.host_id==operation.supervisor,'Log belongs to a different supervisor')
        assert(host.child_pid==operation.process,'Log process changed')
        assert(host.attached,'Log supervisor is detached')
        operation.pane,operation.log=host.terminal_id,'terminal_'..host.terminal_id
        local contents=z.get_pane_scrollback{pane_id=z.terminal_pane(host.terminal_id),get_full_scrollback=true}
        local lines={}
        for _,group in ipairs{contents.lines_above_viewport,contents.viewport,contents.lines_below_viewport} do
          for _,line in ipairs(group or {}) do lines[#lines+1]=line end
        end
        return table.concat(lines,'\n')
      end)
      if ok then operation.output=log;operation.outputTruncated=true
      else operation.logError=tostring(log) end
      -- A terminal retains a bounded history. Swamp saves this observation and its
      -- retention marker; the bridge does not keep a second log store.
    end
    return operation
  end
  if args.method == 'agent_inspect' then return owned(args) end
  if args.method=='workflow_attach' then
    local entry={run=args.run,work=args.work,workflow=args.workflow,root=args.directory}
    local uid=scope(args.run,'attachment')
    local existing,row=read(uid)
    assert(not existing or encode(existing)==encode(entry),'Run belongs to a different launch')
    if not existing then put(uid,entry,row) end
    local runs_uid=scope(args.work,'runs')
    for attempt=1,8 do
      local runs,runs_row=read(runs_uid)
      runs=runs or {entries={}}
      if runs.entries[args.run] then break end
      runs.entries[args.run]=entry
      local ok,err=pcall(put,runs_uid,runs,runs_row)
      if ok then break end
      if not D.is_conflict(err) or attempt==8 then error(err) end
    end
    if cancel_if_closed(args.work) then ensure_open(args.work) end
    if not existing then changed(args.work) end
    return {work=args.work,key=args.key,request=encode(args),incarnation=identity.incarnation,outcome='completed'}
  end
  ensure_open(args.work)
  if args.method == 'reconcile' then
    local operation_uid = scope(args.work, args.operation)
    local operation, operation_row = read(operation_uid)
    assert(operation, 'Operation is absent')
    if decode(operation.request).method=='workflow_run' and operation.checkout then
      discover(args.work,operation.checkout)
    end
    if operation.transport=='serve' then return serve_observe(operation_uid,operation,operation_row) end
    -- A saved supervisor, unlike a pane number, survives a server restart.
    if operation.pane and operation.supervisor then
      local host = supervisor(operation.pane, operation.supervisor)
      assert(host.host_id == operation.supervisor, 'Supervisor mismatch; attachment remains unknown')
      assert(host.child_pid == operation.process, 'Supervisor process changed')
      operation.pane = host.terminal_id
      if operation.log then operation.log='terminal_'..host.terminal_id end
      if host.child_exit_status then
        local method=decode(operation.request).method
        operation.outcome = method=='command' and (host.child_exit_status==0 and 'completed' or 'failed') or 'process-exit'
        operation.exitCode = host.child_exit_status
      elseif operation.outcome == 'pending' and operation.incarnation ~= identity.incarnation then
        operation.outcome = 'unknown' -- the event gap cannot establish successful completion
      end
      put(operation_uid, operation, operation_row)
    end
    return operation
  end

  local prompt = args.method=='agent_prompt' and args.prompt or
    (args.method=='agent_spawn' and args.spec and args.spec.prompt)
  if args.method=='agent_prompt' or (args.method=='agent_spawn' and args.spec and args.spec.prompt~=nil and args.spec.prompt~=z.null) then
    assert(type(prompt)=='string' and prompt:match('%S') and not prompt:find('%z') and #prompt<=262144,
      'Prompt must contain 1–262144 UTF-8 bytes (256 KiB), must not be blank or contain NUL')
  end
  if args.method=='pane_action' and args.cancel_active and (args.action=='fork' or args.action=='duplicate') then
    local open=open_picker_operation(args.work,args.action)
    if open then
      local request=decode(open.request)
      local source=owned{work=request.work,agent='source'}
      assert(z.get_pane_info(z.terminal_pane(source.pane)).exited,'Source pane is no longer exited')
      -- The same configuration reaches the picker still open, or relaunches one that closed
      -- without reporting, whose result then settles the operation it belongs to.
      open_picker(request.work,request.key,request.action,source.pane)
      return open
    end
  end
  local uid, previous, value, row
  if args.method=='vm_status' then
    value={work=args.work,key=args.key,request=encode(args),incarnation=identity.incarnation}
  else
    uid,previous=claim(args)
    if previous then return previous end
    value,row=read(uid)
  end
  if args.method == 'workflow_run' then
    local config = z.project_config.read(assert(args.directory, 'Missing project directory'))
    assert(not config.error, config.error)
    local name = args.workflow or config.config.workflow
    assert(name and name ~= '', 'No exit workflow configured. Use Hocus /project-setup.')
    local validation=swamp({operation='workflow.validate',payload={workflowIdOrName=name},
      on_progress=function(status) launch_progress(args.work,status,'workflow.validate') end},config.root)
    ensure_open(args.work)
    assert(decode(validation).passed==true,'Workflow validation failed: '..validation)
    local inputs = args.inputs or {}
    inputs.launch, inputs.session = args.work, identity.session
    inputs.binary=z.server_binary or 'zellij'
    inputs.checkout = inputs.checkout or command({'git','rev-parse','--show-toplevel'},args.directory):gsub('\n$', '')
    ensure_open(args.work)
    if inputs.sourcePane and inputs.sourcePane>=0 then
      local pane=z.get_pane_info(z.terminal_pane(inputs.sourcePane))
      assert(pane.exited,'Source pane must be exited')
      local host=supervisor(inputs.sourcePane)
      ensure_open(args.work)
      local source_uid=scope(args.work,'source')
      local original={pane=inputs.sourcePane,supervisor=host.host_id,process=host.child_pid,target=pane.target,transportOwner=host.owner,
        owner={host_id=identity.host_id,session=identity.session,incarnation=identity.incarnation,work=args.work,agent='source'}}
      put(source_uid,original)
      inputs.kind=(pane.agent and pane.agent.kind) or inputs.kind or ''
      local vm=pane.target and pane.target.Firecracker
      if vm then
        inputs.firecracker=vm.vm
        inputs.firecrackerImage=vm.image or ''
      else
        assert(not inputs.firecracker or inputs.firecracker=='','Local source cannot select a different VM')
      end
    end
    assert(not inputs.firecracker or inputs.firecracker=='' or (inputs.sourcePane and inputs.sourcePane>=0),'VM work requires its original pane identity')
    inputs.target = inputs.target or config.root
    inputs.gate = inputs.gate or config.config.ci or {'cargo','x','ci','check'}
    inputs.targetBranch = inputs.targetBranch or config.config.default_branch or 'main'
    inputs.rebasePrompt = inputs.rebasePrompt or config.config.rebase_prompt
    value,row=read(uid)
    value.workflow,value.checkout,value.launch=name,config.root,args.work
    put(uid,value,row)
    return dispatch(uid,config.root,'workflow.run',{workflowIdOrName=name,inputs=inputs})
  elseif args.method=='swamp_dispatch' then
    if args.operation=='workflow.resume' then ensure_open(args.payload.runId) end
    return dispatch(uid,args.directory,args.operation,args.payload,true)
  elseif args.method == 'pane_action' then
    local source
    local view=read(scope(args.work,'view'))
    if args.action=='restart' and not read(scope(args.work,'source')) and view
      and view.incarnation==identity.incarnation and view.source then
      -- Validation can fail before the supervisor record is saved. The authenticated
      -- exit view still owns its held source and must allow the user to restart it.
      source={pane=view.source}
    elseif args.action~='leave' or fetch(scope(args.work,'source')) then
      source=owned{work=args.work,agent='source',allow_closed=args.action=='leave'}
    end
    if source then assert(z.get_pane_info(z.terminal_pane(source.pane)).exited,'Source pane is no longer exited') end
    if args.action=='leave' then
      transition(args.work,'leave')
      if source then z.close_terminal_pane(source.pane) end
      value.outcome='completed'
      put(uid,value,row)
      close_view(args.work)
      return value
    end
    if args.action=='restart' then
      transition(args.work,'restart')
      if args.cancel_active then cancel_after_transition(args.work) end
      close_view(args.work,source.pane)
      z.rerun_command_pane(source.pane)
      value.outcome='completed'
      put(uid,value,row)
    else
      assert(args.action=='fork' or args.action=='duplicate','Unknown pane choice')
      value.outcome='pending'
      put(uid,value,row)
      open_picker(args.work,args.key,args.action,source.pane)
    end
  elseif args.method == 'agent_spawn' then
    local opts = assert(args.spec, 'Missing agent specification')
    if opts.prompt==z.null then opts.prompt=nil end
    local owner = {host_id=identity.host_id, session=identity.session, incarnation=identity.incarnation, work=args.work, agent=args.key}
    value.owner, value.outcome = owner, 'pending'
    put(uid, value, row)
    opts.context = {swamp_owner=encode(owner), swamp_operation=args.key}
    opts.start_suppressed = read(scope(args.work,'view')) ~= nil
    local spawned = z.agent.spawn_details(opts)
    value, row = read(uid)
    value.pane = spawned.pane_id.Terminal or spawned.pane_id
    value.branch, value.checkout = spawned.branch or z.null, spawned.cwd or z.null
    put(uid, value, row)
    if closed(args.work) then z.close_terminal_pane(value.pane);ensure_open(args.work) end
    local info = z.get_pane_info(z.terminal_pane(value.pane))
    value, row = read(uid)
    value.target = info.target
    put(uid, value, row) -- preserve the resource even if supervisor lookup fails
    local host
    if value.target and value.target.Firecracker then
      local deadline=os.time()+150
      repeat
        local ok,found=pcall(supervisor,value.pane)
        if ok then host=found;break end
        assert(os.time()<deadline,'VM dispatch has a pane but no verified transport; reconcile it without repeating spawn')
        z.await(z.async.sleep(0.2))
      until host
    else host=supervisor(value.pane) end
    value, row = read(uid)
    value.supervisor, value.process = host.host_id, host.child_pid
    value.remoteHost,value.remotePaneId=host.remote_host,host.pane_token
    ensure_open(args.work)
    if not value.processExited then value.outcome = args.waitForExit and 'pending' or 'completed' end
    put(uid, value, row)
    local placement=placements[args.work]
    if args.waitForExit and placement and not placement.manual and not placement.detached then
      placement.queue[#placement.queue+1]=uid
      present(args.work)
    end
  elseif args.method == 'agent_prompt' then
    local agent = owned(args, true)
    local agent_uid = scope(args.work, args.agent)
    local agent_row
    agent, agent_row = read(agent_uid)
    assert(not agent.outstanding, 'One outstanding prompt is permitted per agent')
    value.owner, value.pane, value.outcome = agent.owner, agent.pane, 'pending'
    value.supervisor,value.process=agent.supervisor,agent.process
    value.remoteHost,value.remotePaneId=agent.remoteHost,agent.remotePaneId
    value.checkout,value.branch,value.target=agent.checkout,agent.branch,agent.target
    agent.outstanding = uid
    -- One transaction publishes observation and the exclusive prompt slot before bytes.
    D.apply({D.put('delivery', {value=encode(value)}, uid, row.rev), D.put('delivery', {value=encode(agent)}, agent_uid, agent_row.rev)})
    local placement=placements[args.work]
    if placement and not placement.manual and not placement.detached then
      placement.queue[#placement.queue+1]=uid
      present(args.work)
    end
    dispatch_prompt(agent)
  elseif args.method == 'command' then
    assert(type(args.argv) == 'table' and #args.argv > 0, 'Missing literal argv')
    local owner = {host_id=identity.host_id, session=identity.session, incarnation=identity.incarnation, work=args.work, agent=args.key}
    value.owner, value.outcome = owner, 'pending'
    value.capture=args.capture
    put(uid, value, row)
    local argv = {}
    for i=2,#args.argv do argv[#argv+1] = args.argv[i] end
    local attachment=read(scope(args.work,'attachment'))
    local output_work=attachment and attachment.work or args.work
    local pane = z.open_command_pane_background {{path=args.argv[1], args=argv, cwd=args.cwd}, {swamp_owner=encode(owner), swamp_operation=args.key, swamp_work=output_work}}
    value, row = read(uid)
    assert(pane,'Command dispatch was not acknowledged; reconcile the saved operation before any further action')
    value.pane = pane.Terminal or pane
    value.checkout, value.log = args.cwd, 'terminal_' .. value.pane
    put(uid, value, row)
    if closed(args.work) then z.close_terminal_pane(value.pane);ensure_open(args.work) end
    local placement=placements[args.work]
    if args.foreground and placement and not placement.detached then
      placement.queue[#placement.queue+1]=uid
      present(args.work)
    end
    local host = supervisor(value.pane)
    value, row = read(uid)
    value.supervisor, value.process = host.host_id, host.child_pid
    put(uid, value, row)
  elseif args.method == 'source_sync' or args.method == 'vm_inspect' or args.method == 'vm_sync' or args.method=='vm_status' then
    local agent
    if args.method=='source_sync' then
      if read(scope(args.work,'source')) then agent=owned{work=args.work,agent='source'}
      else agent={target='Local'} end
    else agent=owned(args) end
    value.owner,value.target=agent.owner,agent.target
    local target = agent.target and agent.target.Firecracker
    if not target then
      value.outcome, value.vm, value.clean = 'completed', z.null, true
    else
      local name = assert(target.vm_name or target.vm, 'Missing VM identity')
      -- Route through the existing provider CLI. Failure leaves this claim unknown.
      local warnings, output = '', ''
      if args.method == 'vm_sync' or args.method=='source_sync' then
        local r=z.await(z.async.run_command {command={z.server_binary or 'zellij','firecracker','sync',name,'--session',identity.session},cwd='/',context={swamp_owner=encode(identity)},max_bytes_returned=1048576})
        assert(r.exit_code==0,'VM sync failed; inspect provider reports before retrying: '..r.stderr)
        warnings=r.stderr
        output=r.stdout
      end
      local count=command({z.server_binary or 'zellij','firecracker','changes',name,'--session',identity.session})
      assert(count:match('^%d+%s*$'),'Invalid guest change count')
      value.outcome, value.vm, value.output = 'completed', name, output .. warnings .. count
      value.clean = tonumber(count)==0 and warnings==''

    end
    if uid then put(uid, value, row) else return value end
  else error('Unknown bridge method') end
  return result(uid)
end
local timing_attempt=0
local function invoke_inner(args, manual_view)
  local new_launch=type(args)=='table' and args.method=='workflow_run' and not read(scope(args.work,args.key))
  local ok,response=pcall(handle,args,manual_view)
  if new_launch and launch_waiting[args.work] then launch_progress(args.work,'ready',launch_waiting[args.work].operation) end
  if ok then
    if args.method=='workflow_show' then
      response.subtitle=workflow_subtitle(response)
      local placement=placements[response.launch]
      if placement and placement.view==manual_view then
        update_subtitle(response.launch,response.subtitle,response.history.workflowName)
      end
    end
    return response
  end
  if args.method=='workflow_show' then
    for work,placement in pairs(placements) do
      if placement.view==manual_view then unavailable(placement,work) end
    end
  end
  if new_launch then
    -- A validation/configuration failure before a runner claim is safe to restart.
    -- Once dispatch was claimed, its outcome stays uncertain until reconciled.
    pcall(function()
      if not read(scope(args.work,'swamp-process')) and not (read(scope(args.work,args.key)) or {}).transport then
        save_event(scope(args.work,args.key),'failed',{error=tostring(response)})
      end
    end)
  end
  pcall(function()
    local value=read(scope(args.work,args.key))
    if value and value.request==encode(args) and value.outcome=='pending' then
      save_event(scope(args.work,args.key),'unknown',{error=tostring(response)})
      present(args.work)
    end
  end)
  error(response)
end
local function invoke(args, manual_view)
  if not z.swamp_timing then return invoke_inner(args,manual_view) end
  timing_attempt=timing_attempt+1
  local attempt,start=timing_attempt,z.monotonic_ms()
  local ok,response=pcall(invoke_inner,args,manual_view)
  pcall(function()
    local record={layer='lua',phase='invoke',attempt=z.instance_id..':'..attempt,
      elapsedMs=z.monotonic_ms()-start,outcome=ok and 'ok' or 'error'}
    for _,field in ipairs({'method','work','key'}) do
      if type(args)=='table' and type(args[field])=='string' then record[field]=args[field] end
    end
    if ok and type(response)=='table' then
      record.requestId,record.run=response.requestId,response.run
    end
    z.log('swamp_timing '..encode(record))
  end)
  if not ok then error(response) end
  return response
end
function M.load()
  local ids = z.get_plugin_ids()
  identity = {host_id=ids.host_id, session=z.server_session, incarnation=z.server_incarnation}
  assert(identity.session ~= '', 'Bridge requires a server session')
  D.define {name='delivery', version=1, attrs={{'value','text',required=true}}}
  z.subscribe {'agent_event', 'command_pane_exited', 'pane_update', 'pane_closed'}
  for _,entry in ipairs(delivery_rows()) do
    if entry.key=='view' and entry.value.closed and entry.value.cancel_pending then
      z.task(function() cancel_lifecycle(entry.work) end)
    end
  end
  z.rpc.handle('swamp', function(args, call)
    local from = type(call.source)=='table' and call.source.Plugin
    assert(from, 'Workflow RPC requires a host-authenticated plugin source')
    local pane = z.get_pane_info(z.plugin_pane(from))
    assert(pane.plugin_url=='zellij:agent-exit' or pane.plugin_url=='zellij:swamp-workflow', 'Only builtin workflow views may use the privileged bridge RPC')
    if args.method=='workflow_run' and pane.plugin_url=='zellij:agent-exit' then
      local uid=scope(args.work,'view')
      local existing=read(uid)
      if existing then
        assert(existing.view==from and existing.incarnation==identity.incarnation,'Launch already belongs to another hook instance')
      else
        put(uid,{view=from,source=args.inputs and args.inputs.sourcePane,incarnation=identity.incarnation})
        placements[args.work]={view=from,queue={},subtitle='Workflow · loading'}
        present(args.work)
      end
    end
    return invoke(args, (args.method=='workflow_answer' or args.method=='workflow_show') and from or nil)
  end)
end
function M.pipe(message)
  if message.name=='workflow-refresh-failed' and message.payload then
    local from=type(message.source)=='table' and message.source.Plugin
    for work,placement in pairs(placements) do
      if placement.view==from and (message.payload=='' or message.payload==work) then unavailable(placement,work) end
    end
    return
  end
  if message.name=='swamp-pane-result' and message.payload then
    local from=type(message.source)=='table' and message.source.Plugin
    if not from then return end
    local pane=z.get_pane_info(z.plugin_pane(from))
    if not pane then return end -- a repeated result after the picker was closed
    assert(pane.plugin_url=='zellij:hocus-new-agent','Pane result must come from the agent picker')
    local reply=decode(message.payload)
    assert(reply.choice=='started' or reply.choice=='cancelled','Invalid picker result')
    if reply.choice=='started' then
      local id=type(reply.pane_id)=='table' and reply.pane_id.Terminal
      assert(type(id)=='number' and id>=0 and id<=4294967295 and id%1==0
        and next(reply.pane_id)=='Terminal' and next(reply.pane_id,'Terminal')==nil,'Started picker result requires a terminal pane ID')
    end
    local uid=scope(reply.work,reply.key)
    local current=read(uid)
    assert(current and decode(current.request).method=='pane_action','No picker operation')
    if current.outcome=='completed' then
      -- A retried result for an accepted choice only needs its acknowledgment again.
      assert(current.choice==reply.choice,'Picker result already differs')
      z.message_to_plugin{destination_plugin_id=from,message_name='swamp-pane-ack'}
      return
    end
    -- One lifecycle forks at most once: a second picker's agent is closed, not accepted.
    local view=read(scope(lifecycle_work(reply.work),'view'))
    if closed(reply.work) or (reply.choice=='started' and view and view.transition) then
      if reply.choice=='started' then z.close_terminal_pane(reply.pane_id.Terminal) end
      z.close_plugin_pane(from)
      return
    end
    assert(current.outcome=='pending' or current.outcome=='unknown','Picker dispatch cannot accept a result')
    save_event(uid,'completed',{choice=reply.choice})
    if reply.choice=='started' then
      transition(reply.work,'fork')
      -- Only a started fork cancels: a cancelled picker leaves the workflow where it was.
      if decode(current.request).cancel_active then cancel_after_transition(reply.work) end
      close_view(reply.work)
    end
    z.message_to_plugin{destination_plugin_id=from,message_name='swamp-pane-ack'}
    return
  end
  if message.name ~= 'swamp' or not message.payload then return end
  local pipe = type(message.source) == 'table' and message.source.Cli
  if not pipe then return end
  z.block_cli_pipe_input(pipe)
  z.task(function()
    local ok, response = pcall(function()
      local args=decode(message.payload)
      -- Typed Swamp inputs distinguish null from an absent key. Preserve only the
      -- forwarded document; ordinary Lua control fields keep nil semantics.
      if args.method=='swamp_dispatch' then args.payload=z.json.decode(message.payload,true).payload end
      if args.method=='workflow_run' then args.inputs=z.json.decode(message.payload,true).inputs end
      return invoke(args)
    end)
    z.cli_pipe_output(pipe, encode(ok and {result=response} or {error=tostring(response)}))
    z.unblock_cli_pipe_input(pipe)
  end)
end
z.on('pane_update', function()
  for work,placement in pairs(placements) do
    if not placement.detached then
      local source=read(scope(work,'source'))
      local ok,original
      if source then ok,original=pcall(z.get_pane_info,z.terminal_pane(source.pane)) end
      if ok and original and original.exited==false then detach(placement)
      else present(work) end
    end
  end
end)
delivery_rows = function()
  local all,cursor={},nil
  repeat
    local rows,page=D.fetch{entity='delivery',after_id=cursor}
    for _,row in ipairs(rows) do
      local id=decode(row.uid)
      if id[1]==identity.host_id and id[2]==identity.session then
        all[#all+1]={uid=row.uid,rev=row.rev,work=id[3],key=id[4],value=decode(row.value)}
      end
    end
    cursor=page.next_cursor
  until not cursor or cursor==z.null
  return all
end
cancel_lifecycle = function(work)
  if cancelling[work] then return end
  cancelling[work]=true
  local uid=scope(work,'view')
  local view,row=read(uid)
  local ok,done=pcall(function()
    local runs,unresolved={},false
    for _,entry in ipairs(delivery_rows()) do
      if lifecycle_work(entry.work)==work then
        local value=entry.value
        if entry.key=='runs' then
          for id,run in pairs(value.entries) do runs[id]=run.root end
        elseif value.checkout and value.request then
          local request=decode(value.request)
          if request.method=='workflow_run' or request.operation=='workflow.run' or request.operation=='workflow.resume' then
            local run=value.run
            if not run and value.transport and not value.not_dispatched then
              run=workflow_run_id(value,request)
              if run then
                -- History yields to delivery events. Merge only the ID into the current claim.
                local current,current_row=read(entry.uid)
                current.run=current.run or run
                put(entry.uid,current,current_row)
              else unresolved=true end
            end
            if run then runs[run]=value.checkout end
          end
        end
      end
    end
    for run,root in pairs(runs) do cancel_run(run,root) end
    return not unresolved
  end)
  if ok and done then
    view.cancel_pending=false
    -- A late attachment changes this revision and requires another cancellation pass.
    ok,done=pcall(put,uid,view,row)
    if ok then done=true end
  end
  cancelling[work]=nil
  if not ok or not done then
    if not ok then z.log('Workflow cancellation pending: '..tostring(done)) end
    z.after(1,function() z.task(function() cancel_lifecycle(work) end) end)
  end
end
local function close_lifecycle(work)
  local uid=scope(work,'view')
  local view,row=read(uid)
  if not view or view.closed or view.transition or view.incarnation~=identity.incarnation then return end
  view.closed,view.cancel_pending=true,true
  put(uid,view,row) -- durable tombstone before closing resources or yielding to cancellation
  local placement=placements[work]
  if placement then placement.detached=true end
  local waiting=launch_waiting[work]
  if waiting then waiting.timer:cancel();launch_waiting[work]=nil end
  local panes={}
  if view.source then panes[view.source]=true end
  for _,entry in ipairs(delivery_rows()) do
    if lifecycle_work(entry.work)==work then
      local value=entry.value
      if value.owner and value.owner.host_id==identity.host_id and value.owner.session==identity.session
        and value.owner.incarnation==identity.incarnation and value.owner.work==entry.work and value.pane then
        panes[value.pane]=true
      end
      if value.outcome=='pending' or value.outcome=='unknown' then
        value.outcome,value.error='interrupted','Pane workflow lifecycle is closed'
        put(entry.uid,value,entry)
      end
    end
  end
  for pane in pairs(panes) do z.close_terminal_pane(pane) end
  z.close_plugin_pane(view.view)
  -- Screen settlement never waits for Swamp's cancellation or history transport.
  z.task(function() cancel_lifecycle(work) end)
end
z.on('pane_closed',function(event)
  local kind,id=next(event.value or {})
  if not id then return end
  local affected={}
  for _,entry in ipairs(delivery_rows()) do
    local value=entry.value
    if entry.key=='view' and value.incarnation==identity.incarnation then
      if (kind=='Plugin' and value.view==id) or (kind=='Terminal' and (value.source==id or value.current==id)) then
        affected[entry.work]=true
      end
    elseif kind=='Terminal' and entry.key=='source' and value.pane==id and value.owner
      and value.owner.host_id==identity.host_id and value.owner.session==identity.session
      and value.owner.incarnation==identity.incarnation and value.owner.work==entry.work then
      affected[lifecycle_work(entry.work)]=true
    end
  end
  for work in pairs(affected) do close_lifecycle(work) end
end)
z.on('agent_event', function(event)
  local ok, raw = pcall(decode, event.raw or '')
  if not ok or not raw._zellij_workflow_owner then return end
  local owner = decode(raw._zellij_workflow_owner)
  if owner.host_id ~= identity.host_id or owner.session ~= identity.session or closed(owner.work) then return end
  local agent = read(scope(owner.work, owner.agent))
  if not agent or (agent.pane and agent.pane ~= event.pane_id) or encode(agent.owner) ~= encode(owner) then return end
  if event.hook=='SessionStart' then
    local current,row=read(scope(owner.work,owner.agent))
    current.ready=true
    put(scope(owner.work,owner.agent),current,row)
    if current.outstanding and current.supervisor then
      z.task(function() dispatch_prompt(owned({work=owner.work,agent=owner.agent},true)) end)
    end
  end
  if event.hook == 'UserPromptSubmit' and agent.outstanding then
    local value, row = read(agent.outstanding)
    if value and value.outcome == 'pending' then
      local request = decode(value.request)
      local matches
      if request.prompt_sha256 and raw.prompt_sha256 then
        matches = request.prompt_sha256 == raw.prompt_sha256
      else
        matches = request.prompt == event.prompt
      end
      if matches and not value.accepted then
        value.accepted = true
        value.agentSession=event.session_id
        value.turn=raw.turn_id
        put(agent.outstanding, value, row)
      end
    end
  end
  finish_agent(owner, event, raw)
end)
z.on('command_pane_exited', function(event)
  local context = event.context or {}
  if not context.swamp_owner then return end
  local owner = decode(context.swamp_owner)
  if owner.host_id ~= identity.host_id or owner.session ~= identity.session or closed(owner.work) then return end
  local uid = scope(owner.work, context.swamp_operation or owner.agent)
  local value = read(uid)
  if not value or encode(value.owner) ~= encode(owner) or (value.pane and value.pane~=event.pane_id) then return end
  local request = decode(value.request)
  local outcome = request.method == 'command' and (event.exit_code == 0 and 'completed' or 'failed') or 'process-exit'
  save_event(uid, outcome, {exitCode=event.exit_code or z.null})
  if request.method=='agent_spawn' then
    local current, row=read(uid)
    current.processExited=true
    put(uid,current,row)
  end
  if value.outstanding then save_event(value.outstanding, 'process-exit', {exitCode=event.exit_code or z.null}) end
  present(owner.work)
  local view=read(scope(owner.work,'view'))
  if view and view.incarnation==identity.incarnation then z.message_to_plugin{destination_plugin_id=view.view,message_name='workflow-changed'} end
end)
return M
