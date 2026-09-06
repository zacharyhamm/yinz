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
  save_event(uid, outcome, {message=event.last_message or z.null, hook=event.hook, agentSession=event.session_id or z.null})
  agent.outstanding = nil
  put(agent_uid, agent, row)
end
local function command(argv, cwd)
  local r = z.await(z.async.run_command {command=argv, cwd=cwd or '/', context={swamp_owner=encode(identity)}, max_bytes_returned=1048576})
  assert(r.exit_code == 0, 'Host command failed: ' .. tostring(r.stderr))
  return r.stdout
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
  if current.owner then assert(current.owner==encode(agent.owner),'Remote ownership changed') end
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
  local history=decode(command({'swamp','workflow','history','search','--input','launch='..work,'--json'},root))
  for _,run in ipairs(history.results or {}) do
    if run.inputs.session==identity.session then
      handle{method='workflow_attach',work=work,key='attachment',run=run.runId,workflow=run.workflowName,directory=root}
    end
  end
  return read(scope(work,'runs'))
end
handle = function(args)
  assert(type(args) == 'table', 'Expected a typed request')
  if args.method == 'identity' then return identity end
  if args.method == 'workflow_cancel' then
    local snapshot=handle{method='workflow_show',directory=args.directory,run=args.run}
    local histories={snapshot.history}
    for _,child in ipairs(snapshot.children) do histories[#histories+1]=child end
    local operations={}
    for _,history in ipairs(histories) do
      if history.status=='running' or history.status=='suspended' then
        operations[#operations+1]=handle{method='command',work=history.inputs.launch,key='cancel-'..history.id,cwd=snapshot.root,
          argv={'swamp','workflow','cancel',history.workflowName,'--run',history.id,'--json'}}
      end
    end
    return {operations=operations}
  end
  if args.method == 'workflow_show' then
    local config = z.project_config.read(assert(args.directory, 'Missing control checkout'))
    assert(not config.error, config.error)
    local attachment,work
    if args.run and args.run~=z.null then
      attachment=read(scope(args.run,'attachment'))
      if attachment then work=attachment.work
      else
        local history=decode(command({'swamp','workflow','history','get',args.run,'--json'},config.root))
        assert(history.inputs.session==identity.session,'Run belongs to a different session')
        work=history.inputs.launch
      end
    elseif args.launch and args.launch~=z.null then work=args.launch end
    local runs=work and discover(work,config.root)
    if args.run and args.run~=z.null then attachment=read(scope(args.run,'attachment'))
    elseif runs then
      for id,entry in pairs(runs.entries) do
        if entry.workflow=='@hammz/zellij-agent-exit' or not attachment then attachment=entry;attachment.run=id end
      end
    end
    assert(attachment,'Launch has no reported run yet; reconcile by launch token without repeating it')
    local run=attachment.run
    local history=decode(command({'swamp','workflow','history','get',run,'--json'},config.root))
    local runs=read(scope(attachment.work,'runs'))
    local ids={}
    for id in pairs(runs.entries) do ids[#ids+1]=id end
    local predicate='has(tags.specName) && tags.specName == "question" && attributes.run in '..encode(ids)
    local questions=decode(command({'swamp','data','query',predicate,'--select','{"model": modelName, "version": version, "attributes": attributes}','--json'},config.root))
    local outputs=decode(command({'swamp','data','query','has(tags.workflowRunId) && tags.workflowRunId in '..encode(ids),'--select','{"model":modelName,"name":name,"version":version,"attributes":attributes}','--json'},config.root))
    local children={}
    for _,id in ipairs(ids) do
      if id~=run then children[#children+1]=decode(command({'swamp','workflow','history','get',id,'--json'},config.root)) end
    end
    return {run=run,root=config.root,history=history,children=children,
      outputs=outputs,logs=command({'swamp','workflow','history','logs',run,'--tail','100','--json'},config.root),questions=questions}

  end
  key(args.work)
  if args.method == 'lookup' then return read(scope(args.work,args.key)) or z.null end
  if args.method == 'observe' then
    local uid = scope(args.work,args.key)
    local operation, row = read(uid)
    assert(operation, 'No operation with that identity')
    if operation.outcome=='pending' and operation.dispatcher~=z.instance_id then
      operation.outcome='unknown'
      put(uid,operation,row)
    end
    if operation.log and operation.pane and operation.supervisor then
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
    return {work=args.work,key=args.key,request=encode(args),incarnation=identity.incarnation,outcome='completed'}
  end
  if args.method == 'reconcile' then
    local operation_uid = scope(args.work, args.operation)
    local operation, operation_row = read(operation_uid)
    assert(operation, 'Operation is absent')
    if decode(operation.request).method=='workflow_run' and operation.checkout then
      discover(args.work,operation.checkout)
    end
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
    command({'swamp','workflow','validate',name,'--json'},config.root)
    local inputs = args.inputs or {}
    inputs.launch, inputs.session = args.work, identity.session
    inputs.binary=z.server_binary or 'zellij'
    inputs.checkout = inputs.checkout or command({'git','rev-parse','--show-toplevel'},args.directory):gsub('\n$', '')
    inputs.target = inputs.target or config.root
    inputs.gate = inputs.gate or config.config.ci or {'cargo','x','ci','check'}
    inputs.targetBranch = inputs.targetBranch or config.config.default_branch or 'main'
    value.workflow,value.checkout,value.launch=name,config.root,args.work
    put(uid,value,row)
    local launch = handle({method='command',work=args.work,key='swamp-process',cwd=config.root,
      argv={'swamp','workflow','run',name,'--no-supersede','--input',encode(inputs),'--json'}})
    value, row = read(uid)
    value.outcome, value.pane, value.supervisor = launch.outcome, launch.pane, launch.supervisor
    value.process = launch.process
    value.checkout, value.log, value.workflow, value.launch = config.root, launch.log, name, args.work
    put(uid,value,row)
  elseif args.method == 'agent_spawn' then
    local opts = assert(args.spec, 'Missing agent specification')
    assert(not opts.prompt, 'Spawn without prompt, then call agent_prompt to register observation first')
    local owner = {host_id=identity.host_id, session=identity.session, incarnation=identity.incarnation, work=args.work, agent=args.key}
    value.owner, value.outcome = owner, 'pending'
    put(uid, value, row)
    opts.context = {swamp_owner=encode(owner), swamp_operation=args.key}
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
    dispatch_prompt(agent)
  elseif args.method == 'command' then
    assert(type(args.argv) == 'table' and #args.argv > 0, 'Missing literal argv')
    local owner = {host_id=identity.host_id, session=identity.session, incarnation=identity.incarnation, work=args.work, agent=args.key}
    value.owner, value.outcome = owner, 'pending'
    put(uid, value, row)
    local argv = {}
    for i=2,#args.argv do argv[#argv+1] = args.argv[i] end
    local pane = z.open_command_pane {{path=args.argv[1], args=argv, cwd=args.cwd}, {swamp_owner=encode(owner), swamp_operation=args.key}}
    value, row = read(uid)
    value.pane = pane.Terminal or pane
    value.checkout, value.log = args.cwd, 'terminal_' .. value.pane
    put(uid, value, row)
    local host = supervisor(value.pane)
    value, row = read(uid)
    value.supervisor, value.process = host.host_id, host.child_pid
    put(uid, value, row)
  elseif args.method == 'vm_inspect' or args.method == 'vm_sync' or args.method=='vm_status' then
    local agent = owned(args)
    value.owner,value.target=agent.owner,agent.target
    local target = agent.target and agent.target.Firecracker
    if not target then
      value.outcome, value.vm, value.clean = 'completed', z.null, true
    else
      local name = assert(target.vm_name or target.vm, 'Missing VM identity')
      -- Route through the existing provider CLI. Failure leaves this claim unknown.
      local warnings, output = '', ''
      if args.method == 'vm_sync' then
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
    return handle(args)
  end)
end
function M.pipe(message)
  if message.name ~= 'swamp' or not message.payload then return end
  local pipe = type(message.source) == 'table' and message.source.Cli
  if not pipe then return end
  z.block_cli_pipe_input(pipe)
  z.task(function()
    local ok, response = pcall(function() return handle(decode(message.payload)) end)
    if not ok then
      pcall(function()
        local args=decode(message.payload)
        local uid=scope(args.work,args.key)
        local value,row=read(uid)
        if value and value.request==encode(args) and value.outcome=='pending' then
          value.outcome='unknown'; put(uid,value,row)
        end
      end)
    end
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
  if not value or encode(value.owner) ~= encode(owner) then return end
  local request = decode(value.request)
  local outcome = request.method == 'command' and (event.exit_code == 0 and 'completed' or 'failed') or 'process-exit'
  save_event(uid, outcome, {exitCode=event.exit_code or z.null})
  if request.method=='agent_spawn' then
    local current, row=read(uid)
    current.processExited=true
    put(uid,current,row)
  end
  if value.outstanding then save_event(value.outstanding, 'process-exit', {exitCode=event.exit_code or z.null}) end
end)
return M
