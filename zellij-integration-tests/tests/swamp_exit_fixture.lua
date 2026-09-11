local z=require('zellij')
local handler,token,worker
local status='running'
local owners,hosts={},{}
local real_info=z.get_pane_info
z.get_pane_info=function(pane)
  local info=real_info(pane)
  -- The fixture embeds the builtin sources in one file-backed hook instance.
  if pane.Plugin==z.get_plugin_ids().plugin_id then info.plugin_url='zellij:agent-exit' end
  return info
end
z.project_config.read=function() return {root='/host',config={workflow='fixture'}} end
z.workflow_remote_panes=function() return {} end
z.rpc.handle=function(_,h) handler=h end
local function call(args)
  return handler(args,{source={Plugin=z.get_plugin_ids().plugin_id}})
end
z.rpc.call=function(request)
  local future,resolve=z.async.future()
  z.task(function()
    local ok,result=pcall(call,request.payload)
    resolve(ok and {result=result} or {error=tostring(result)})
  end)
  return future
end
local open=z.open_command_pane_background
z.open_command_pane_background=function(args)
  local pane=open(args)
  hosts[#hosts+1]={terminal_id=pane.Terminal,attached=true,session_name=z.server_session,host_id='host-'..pane.Terminal,child_pid=100+pane.Terminal}
  return pane
end
local spawn=z.agent.spawn_details
z.agent.spawn_details=function(opts)
  local agent=spawn(opts)
  local id=agent.pane_id.Terminal
  owners[id]=opts.context.swamp_owner
  hosts[#hosts+1]={terminal_id=id,attached=true,session_name=z.server_session,host_id='host-'..id,child_pid=100+id}
  return agent
end
z.async.run_command=function(opts)
  local stdout=opts.command[1]=='git' and '/host\n' or z.json.encode(hosts)
  local future,resolve=z.async.future();resolve{exit_code=0,stdout=stdout,stderr=''};return future
end
z.async.swamp_request=function(opts)
  local op,p=opts.operation,opts.payload
  local result={}
  if op=='workflow.validate' then result={passed=true}
  elseif op=='workflow.run' then result={runId='run'}
  elseif op=='model.method.run' then
    assert(p.inputs.run=='run' and p.modelIdOrName=='questions' and p.inputs.question=='exit' and p.inputs.answer=='leave')
    status='completed'
    assert(call{method='pane_action',work=token,key='leave',action='leave'}.outcome=='completed')
  elseif op=='workflow.cancel' then status='cancelled'
  elseif op=='workflow.history.search' then
    result={results={{runId='run',workflowName='fixture',inputs={session=z.server_session,launch=token}}}}
  elseif op=='workflow.history.logs' then
    local lines={}
    for i=1,100 do lines[#lines+1]=string.format('LOG %03d selectable output',i) end
    result={type='log',log={lines=lines}}
  elseif op=='workflow.history.get' then
    result={id='run',workflowName='fixture',status=status,jobs={},inputs={session=z.server_session,launch=token}}
  elseif op=='data.query' then
    local results={}
    if p.predicate:find('question') and status=='suspended' then
      assert(p.predicate=='specName == "question" && attributes.run in ["run"]',p.predicate)
      results={{model='questions',version=2,attributes={question='exit',run='run',kind='choice',prompt='Agent exited. Choose Restart, Fork, Begin review, or Leave.',answers={'restart','fork','begin-review','leave'},state='pending'}},
        {model='questions',version=1,attributes={question='exit',run='run',kind='choice',prompt='Stale question',answers={'old'},state='pending'}}}
    end
    result={results={},projected={shape='map',rows=results}}
  end
  local future,resolve=z.async.future();resolve(result);return future
end
local function emit(hook,prompt)
  z.__update({}, {AgentEvent={pane_id=worker,hook=hook,prompt=prompt,session_id='agent-session',raw=z.json.encode{_zellij_workflow_owner=owners[worker],turn_id='turn'}}})
end
