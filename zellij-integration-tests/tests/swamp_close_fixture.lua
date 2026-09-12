local z=require('zellij')
local token,source,view,worker,release
local status,delayed='running',false
local cancelled,hosts,owners={},{},{}
local real_info=z.get_pane_info
z.get_pane_info=function(pane)
  local info=real_info(pane)
  if info and info.plugin_url and info.plugin_url:match('/exit.lua$') then info.plugin_url='zellij:agent-exit' end
  return info
end
z.project_config.read=function() return {root='/control',config={workflow='fixture'}} end
z.workflow_remote_panes=function() return {} end
local handler
local real_handle=z.rpc.handle
z.rpc.handle=function(name,fn)
  handler=fn
  real_handle(name,function(args,call)
    if args.method=='workflow_run' then
      token,source,view=args.work,args.inputs.sourcePane,call.source.Plugin
      hosts[#hosts+1]={terminal_id=source,attached=true,session_name=z.server_session,host_id='source-'..source,child_pid=100+source}
    end
    return fn(args,call)
  end)
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
  local future,resolve=z.async.future()
  resolve{exit_code=0,stdout=opts.command[1]=='git' and '/control\n' or z.json.encode(hosts),stderr=''}
  return future
end
z.async.swamp_request=function(opts)
  local op,p=opts.operation,opts.payload
  local future,resolve=z.async.future()
  local result={}
  if op=='workflow.validate' then result={passed=true}
  elseif op=='workflow.run' then
    if delayed then release=resolve;return future end
    result={runId='run'}
  elseif op=='workflow.cancel' then cancelled[p.runId]=true;status='cancelled'
  elseif op=='workflow.history.search' then
    result={results=delayed and {} or {{runId='run',workflowName='fixture',inputs={session=z.server_session,launch=token}}}}
  elseif op=='workflow.history.get' then result={id='run',workflowName='fixture',status=status,jobs={},inputs={session=z.server_session,launch=token}}
  elseif op=='workflow.history.logs' then result={log={lines={'fixture workflow'}}}
  elseif op=='data.query' then
    result={results={}}
    if status=='suspended' and p.predicate:find('question') then
      result.results={{model='questions',version=1,attributes={question='exit',run='run',kind='choice',prompt='Agent exited. Choose',answers={'restart','fork','leave'},state='pending'}}}
    end
  end
  resolve(result);return future
end
local function call(args) return handler(args,{source={Plugin=view}}) end
local function closed()
  if not token then return false end
  local uid=z.json.encode{z.get_plugin_ids().host_id,z.server_session,token,'view'}
  local rows=z.data.fetch{entity='delivery',filter=z.data.eq('uid',uid),limit=1}
  return rows[1] and z.json.decode(rows[1].value).closed or false
end
-- BRIDGE is inserted here by the Rust harness.
local function plugin(bridge)
  local M={}
  function M.load()
    z.task(function()
      z.await(z.async.permissions{'read_application_state','change_application_state','run_commands','write_to_stdin','read_cli_pipes','plugin_data','read_pane_contents','message_and_launch_other_plugins'})
      bridge.load()
    end)
  end
  function M.pipe(message)
    if message.name~='fixture' then return bridge.pipe(message) end
    z.block_cli_pipe_input(message.source.Cli)
    z.task(function()
      if message.payload=='delay' then delayed=true
      elseif message.payload=='release' then delayed=false;assert(release)({runId='run'})
      elseif message.payload=='question' then
        status='suspended'
        z.message_to_plugin{destination_plugin_id=view,message_name='workflow-changed'}
      elseif message.payload=='agent' then
        worker=call{method='agent_spawn',work=token,key='worker',spec={kind='claude',cwd='/control',allow_outside_worktree=true}}.pane
        z.__update({}, {AgentEvent={pane_id=worker,hook='SessionStart',raw=z.json.encode{_zellij_workflow_owner=owners[worker]}}})
        call{method='agent_prompt',work=token,key='turn',agent='worker',prompt='work'}
      end
      z.cli_pipe_output(message.source.Cli,z.json.encode{closed=closed(),cancelled=cancelled.run or false,waiting=release~=nil})
      z.unblock_cli_pipe_input(message.source.Cli)
    end)
  end
  return M
end
