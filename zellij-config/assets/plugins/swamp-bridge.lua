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
local function close_view(work)
  local placement=read(scope(work,'view'))
  if not placement or placement.incarnation~=identity.incarnation then return end
  local ok,pane=pcall(z.get_pane_info,z.plugin_pane(placement.view))
  if ok and pane and pane.plugin_url=='zellij:agent-exit' then z.close_plugin_pane(placement.view) end
end
local function active(work)
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
          if method=='command' or method=='agent_prompt' then return true end
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
  local placement=placements[work]
  if not placement or placement.detached then return end
  local view=z.plugin_pane(placement.view)
  local ok,info=pcall(z.get_pane_info,view)
  if not ok or not info or (info.plugin_url~='zellij:agent-exit' and info.plugin_url~='zellij:swamp-workflow') then
    placement.detached=true
    return
  end
  local current=placement.current and z.terminal_pane(placement.current) or view
  local ok,current_info=pcall(z.get_pane_info,current)
  if not ok or not current_info or current_info.is_suppressed then
    placement.detached=true
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
  if next_pane==placement.current then return end
  local target=next_pane and z.terminal_pane(next_pane) or view
  local ok,target_info=pcall(z.get_pane_info,target)
  if not ok or not target_info or not target_info.is_suppressed then
    placement.detached=true
    return
  end
  z.replace_pane_with_existing_pane{current,target,true}
  placement.current=next_pane
end
local function attach_manual(work, view)
  if read(scope(work,'view')) then return end
  local existing=placements[work]
  if existing then
    local ok,pane=pcall(z.get_pane_info,z.plugin_pane(existing.view))
    if ok and pane then return end
  end
  local placement={view=view,queue={},manual=true}
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
  if notifications[work] then return end
  notifications[work]=true
  z.after(0.1,function()
    notifications[work]=nil
    local view=read(scope(work,'view'))
    if view and view.incarnation==identity.incarnation then
      z.message_to_plugin{destination_plugin_id=view.view,message_name='workflow-changed'}
    end
    local placement=placements[work]
    if placement and placement.manual and not placement.detached then
      z.message_to_plugin{destination_plugin_id=placement.view,message_name='workflow-changed'}
    end
  end)
end
local function swamp(request,root)
  local reply=z.await(z.async.swamp_request{directory=root,operation=request.operation,payload=request.payload,
    id=request.id,started=request.started})
  if reply.error then error(encode(reply.error)) end
  if request.operation=='data.query' and reply.limited then error('Swamp data query reached its result limit; narrow the saved-data query') end
  if request.operation=='data.query' and reply.projected then
    reply.results=reply.projected.rows or reply.projected.values or {}
    reply.projected=nil
  elseif request.operation=='workflow.history.logs' and reply.log then
    return table.concat(reply.log.lines or {},'\n')
  end
  return encode(reply)
end
local function watch_run(run,work,root)
  if watching[run] then return end
  local id=z.instance_id..'-watch-'..run
  watching[run]=true;watch_ids[id]={run=run,work=work}
  z.task(function()
    pcall(swamp,{operation='run.attach',payload={runId=run,afterSeq=0},id=id},root)
    watching[run]=nil;watch_ids[id]=nil
  end)
end
local function dispatch(uid,root,operation,payload)
  local value,row=read(uid)
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
  local reply=decode(swamp({operation=operation,payload=payload,id=value.requestId,started=true},root))
  value,row=read(uid)
  if reply.runId then value.run=reply.runId end
  if value.outcome=='unknown' and tracked[value.requestId] then value.outcome='pending' end
  put(uid,value,row)
  return value
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
  if request.operation=='workflow.resume' then operation.run=operation.run or request.payload.runId end
  if not operation.run and request.operation~='workflow.resume' then
    local history=decode(swamp({operation='workflow.history.search',payload={filter='has(tags.zellijOperation) && tags.zellijOperation == '..encode(operation.correlation)}},operation.checkout))
    assert(#(history.results or {})<=1,'Multiple runs have the same operation identity; reconcile explicitly')
    if history.results and history.results[1] then operation.run=history.results[1].runId end
  end
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
z.on('custom_message',function(event)
  if event.message~='swamp-response' then return end
  local frame=decode(event.payload)
  local watch=watch_ids[frame.id]
  if watch then changed(watch.work);return end
  local uid=tracked[frame.id]
  if not uid then return end
  local operation,row=read(uid)
  if not operation then return end
  local write=false
  local pending=operation.outcome=='pending' or operation.outcome=='unknown'
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
  local attached=read(scope(operation.work,'attachment'))
  changed(attached and attached.work or operation.work)
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
  local agent = result(scope(args.work, args.agent))
  assert(agent.owner and agent.owner.host_id == identity.host_id and agent.owner.session == identity.session, 'Wrong host or session')
  assert(agent.pane and agent.supervisor, 'Agent dispatch is unknown; reconcile it first')
  local current = supervisor(agent.pane, agent.supervisor)
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
  if not agent.ready or not agent.outstanding then return end
  local operation,row=read(agent.outstanding)
  if not operation or operation.outcome~='pending' or operation.dispatched then return end
  operation.dispatched=true
  put(agent.outstanding,operation,row) -- a crash after this point is an uncertain send
  z.agent.send(z.terminal_pane(agent.pane),decode(operation.request).prompt)
end
local function claim(args)
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
local handle
local function discover(work, root)
  local launch=result(scope(work,'launch'))
  assert(launch.checkout==root,'Launch belongs to a different control checkout')
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
    assert(args.operation=='workflow.history.get' or args.operation=='workflow.history.search' or args.operation=='data.query' or args.operation=='workflow.approve','Unsupported bridge request')
    return decode(swamp({operation=args.operation,payload=args.payload},args.directory))
  end
  if args.method=='workflow_answer' then
    local reply=decode(swamp({operation='model.method.run',payload={modelIdOrName=args.model,methodName='submit',inputs=args.inputs}},args.directory))
    if reply.run and (reply.run.status=='failed' or reply.run.status=='cancelled') then error(encode(reply.run)) end
    return reply.run or reply
  end
  if args.method == 'workflow_cancel' then
    local snapshot=handle{method='workflow_show',directory=args.directory,run=args.run}
    local histories={snapshot.history}
    for _,child in ipairs(snapshot.children) do histories[#histories+1]=child end
    local operations={}
    for _,history in ipairs(histories) do
      if history.status=='running' then
        operations[#operations+1]=decode(swamp({operation='workflow.cancel',payload={runId=history.id}},snapshot.root))
      end
    end
    return {operations=operations}
  end
  if args.method == 'workflow_show' then
    local failed=args.launch and args.launch~=z.null and read(scope(args.launch,'launch'))
    if failed and failed.outcome=='failed' and failed.error then
      return launch_failure(args.launch,failed,failed.error,args.directory)
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
    local runs=work and discover(work,config.root)
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
    local function preview(path)
      return ([["output":has(VALUE.output) ? (VALUE.output.size() > 16384 ? VALUE.output.substring(0,
        VALUE.output.substring(16383,16384).matches(r"[\uD800-\uDBFF]") ? 16383 : 16384) : VALUE.output) : "",
        "previewTruncated":has(VALUE.output) && VALUE.output.size() > 16384]]):gsub('VALUE',path)
    end
    local function test_preview(path)
      return ('{"evidence":'..path..'.evidence,"outcome":'..path..'.outcome,"exitCode":'..path..'.exitCode,'..
        '"log":'..path..'.log,"passed":'..path..'.passed,"outputRef":has('..path..'.outputRef) ? '..path..'.outputRef : null,'..preview(path)..'}')
    end
    local output_projection=[[{"model":modelName,"name":name,"version":version,"attributes":
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
    if args.includeOutput~=false and operation.log and operation.pane and operation.supervisor then
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
    local view=read(scope(args.work,'view'))
    if not existing and view and view.incarnation==identity.incarnation then z.message_to_plugin{destination_plugin_id=view.view,message_name='workflow-changed'} end
    return {work=args.work,key=args.key,request=encode(args),incarnation=identity.incarnation,outcome='completed'}
  end
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

  if args.method == 'agent_prompt' then
    assert(type(args.prompt) == 'string' and #args.prompt > 0 and #args.prompt <= 262144, 'Prompt must contain 1–262144 UTF-8 bytes (256 KiB)')
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
    local validation=swamp({operation='workflow.validate',payload={workflowIdOrName=name}},config.root)
    assert(decode(validation).passed==true,'Workflow validation failed: '..validation)
    local inputs = args.inputs or {}
    inputs.launch, inputs.session = args.work, identity.session
    inputs.binary=z.server_binary or 'zellij'
    inputs.checkout = inputs.checkout or command({'git','rev-parse','--show-toplevel'},args.directory):gsub('\n$', '')
    if inputs.sourcePane and inputs.sourcePane>=0 then
      local pane=z.get_pane_info(z.terminal_pane(inputs.sourcePane))
      assert(pane.exited,'Source pane must be exited')
      local host=supervisor(inputs.sourcePane)
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
    value.workflow,value.checkout,value.launch=name,config.root,args.work
    put(uid,value,row)
    return dispatch(uid,config.root,'workflow.run',{workflowIdOrName=name,inputs=inputs})
  elseif args.method=='swamp_dispatch' then
    return dispatch(uid,args.directory,args.operation,args.payload)
  elseif args.method == 'pane_action' then
    if args.action=='leave' then
      value.outcome='completed'
      put(uid,value,row)
      close_view(args.work)
      return value
    end
    local source=owned{work=args.work,agent='source'}
    assert(z.get_pane_info(z.terminal_pane(source.pane)).exited,'Source pane is no longer exited')
    if args.action=='restart' then
      z.rerun_command_pane(source.pane)
      value.outcome='completed'
      put(uid,value,row)
      close_view(args.work)
    else
      assert(args.action=='fork' or args.action=='duplicate','Unknown pane choice')
      value.outcome='pending'
      put(uid,value,row)
      z.message_to_plugin{plugin_url='zellij:hocus-new-agent',message_name='swamp-pane-action',
        plugin_config={caller_terminal_pane_id=tostring(source.pane),[args.action=='fork' and 'fork_focused' or 'duplicate_focused']='true',
          swamp_reply_plugin=tostring(z.get_plugin_ids().plugin_id),swamp_work=args.work,swamp_key=args.key},
        new_plugin_args={should_float=true,should_focus=true}}
    end
  elseif args.method == 'agent_spawn' then
    local opts = assert(args.spec, 'Missing agent specification')
    assert(not opts.prompt, 'Spawn without prompt, then call agent_prompt to register observation first')
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
    value.outcome = 'completed'
    put(uid, value, row)
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
  if ok then return response end
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
  z.subscribe {'agent_event', 'command_pane_exited'}
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
        put(uid,{view=from,incarnation=identity.incarnation})
        placements[args.work]={view=from,queue={}}
      end
    end
    return invoke(args, args.method=='workflow_show' and pane.plugin_url=='zellij:swamp-workflow' and from or nil)
  end)
end
function M.pipe(message)
  if message.name=='swamp-pane-result' and message.payload then
    local from=type(message.source)=='table' and message.source.Plugin
    if not from then return end
    local pane=z.get_pane_info(z.plugin_pane(from))
    assert(pane.plugin_url=='zellij:hocus-new-agent','Pane result must come from the agent picker')
    local reply=decode(message.payload)
    assert(reply.choice=='started' or reply.choice=='cancelled','Invalid picker result')
    local uid=scope(reply.work,reply.key)
    local current=read(uid)
    assert(current and decode(current.request).method=='pane_action','No picker operation')
    if current.outcome=='completed' then
      assert(current.choice==reply.choice,'Picker result already differs')
    else assert(current.outcome=='pending' or current.outcome=='unknown','Picker dispatch cannot accept a result') end
    save_event(uid,'completed',{choice=reply.choice})
    z.message_to_plugin{destination_plugin_id=from,message_name='swamp-pane-ack'}
    if reply.choice=='started' then
      close_view(reply.work)
    end
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
z.on('agent_event', function(event)
  local ok, raw = pcall(decode, event.raw or '')
  if not ok or not raw._zellij_workflow_owner then return end
  local owner = decode(raw._zellij_workflow_owner)
  if owner.host_id ~= identity.host_id or owner.session ~= identity.session then return end
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
  if owner.host_id ~= identity.host_id or owner.session ~= identity.session then return end
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
