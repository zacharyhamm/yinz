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
  local argv=opts.command
  if argv[1]=='bash' then
    assert(argv[6]=='workflow-history')
    argv={table.unpack(argv,7)}
  end
  local stdout=''
  if argv[1]=='git' then stdout='/host\n'
  elseif argv[2]=='pty-host' then stdout=z.json.encode(hosts)
  elseif argv[2]=='workflow' and argv[3]=='answer' then status='completed'
  elseif argv[2]=='workflow' and argv[3]=='history' then
    if argv[4]=='search' then
      stdout=z.json.encode{results={{runId='run',workflowName='fixture',inputs={session=z.server_session,launch=token}}}}
    elseif argv[4]=='get' then
      stdout=z.json.encode{id='run',workflowName='fixture',status=status,jobs={},inputs={session=z.server_session,launch=token}}
    end
  elseif argv[2]=='data' then
    local results={}
    if argv[4]:find('question') and status=='suspended' then
      results={{model='questions',version=1,attributes={question='q',run='run',kind='choice',prompt='Continue work?',answers={'yes','no'},state='pending'}}}
    end
    stdout=z.json.encode{results=results}
  end
  local future,resolve=z.async.future()
  resolve{exit_code=0,stdout=stdout,stderr=''}
  return future
end
local function emit(hook,prompt)
  z.__update({}, {AgentEvent={pane_id=worker,hook=hook,prompt=prompt,session_id='agent-session',raw=z.json.encode{_zellij_workflow_owner=owners[worker],turn_id='turn'}}})
end
