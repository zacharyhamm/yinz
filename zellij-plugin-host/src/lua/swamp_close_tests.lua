local z=require('zellij')
local rows,panes,cancelled,requests={},{},{},{}
local handler,delayed,resolve_delay
-- Reload the bridge module with only durable rows surviving: retire its handlers and timers.
local generation,timers=0,{}
local on=z.on
z.on=function(event,fn)
  local registered=generation
  on(event,function(...) if registered==generation then return fn(...) end end)
end
for _,name in ipairs{'after','every'} do
  local schedule=z[name]
  z[name]=function(...)
    local timer=schedule(...);timers[#timers+1]=timer
    return timer
  end
end
local function reload_bridge()
  for _,timer in ipairs(timers) do timer:cancel() end
  timers={};generation=generation+1
  M=new_bridge();M.load()
end
local now=1000
os.time=function() return now end
local function tick()
  now=now+2
  z.__update(M,{Timer=0})
end
z.server_session='session';z.server_incarnation='incarnation';z.instance_id='instance'
z.get_plugin_ids=function() return {host_id='host',plugin_id=99} end
z.subscribe=function() end
z.set_timeout=function() end
z.data.define=function() end
z.data.eq=function(_,uid) return uid end
z.data.fetch=function(q)
  if q.filter then return rows[q.filter] and {rows[q.filter]} or {},{} end
  local page={}
  for uid,row in pairs(rows) do page[#page+1]={uid=uid,value=row.value,rev=row.rev} end
  return page,{}
end
z.data.put=function(_,v,uid,rev) return {uid=uid,value=v.value,rev=rev} end
z.data.apply=function(changes)
  if changes.uid then changes={changes} end
  for _,c in ipairs(changes) do assert(c.rev==(rows[c.uid] and rows[c.uid].rev or 0),'revision conflict') end
  for _,c in ipairs(changes) do rows[c.uid]={value=c.value,rev=c.rev+1} end
end
local function uid(work,key) return z.json.encode{'host','session',work,key} end
local function save(work,key,value) rows[uid(work,key)]={rev=1,value=z.json.encode(value)} end
local function read(work,key) local row=rows[uid(work,key)];return row and z.json.decode(row.value) end
local function owner(work,key) return {host_id='host',session='session',incarnation='incarnation',work=work,agent=key} end
z.rpc.handle=function(_,fn) handler=fn end
z.get_pane_info=function(pane) return panes[pane.Plugin and 'p'..pane.Plugin or 't'..pane.Terminal] end
z.close_terminal_pane=function(id) panes['t'..id]=nil end
z.close_plugin_pane=function(id) panes['p'..id]=nil end
z.message_to_plugin=function() end
z.project_config.read=function() return {root='/control',config={workflow='exit'}} end
z.workflow_remote_panes=function() return {} end
z.async.run_command=function(opts)
  local hosts={}
  for id in pairs(panes) do
    local number=tonumber(id:match('^t(%d+)'))
    if number then hosts[#hosts+1]={terminal_id=number,attached=true,session_name='session',host_id='supervisor-'..number,child_pid=100+number} end
  end
  local future,resolve=z.async.future()
  resolve{exit_code=0,stdout=opts.command[1]=='git' and '/control\n' or z.json.encode(hosts),stderr=''}
  return future
end
z.async.swamp_request=function(opts)
  requests[#requests+1]=opts
  local future,resolve=z.async.future()
  if delayed==opts.operation then resolve_delay=resolve;return future end
  local result={}
  if opts.operation=='workflow.cancel' then
    assert(read('work','view').closed,'closure must be durable before cancellation')
    cancelled[opts.payload.runId]=true
  elseif opts.operation=='workflow.validate' then result={passed=true}
  elseif opts.operation=='workflow.run' then result={runId='run'}
  elseif opts.operation=='workflow.history.search' then result={results={}}
  elseif opts.operation=='workflow.history.get' then result={id=opts.payload.workflowIdOrName,status='running'} end
  resolve(result);return future
end
local function call(args, from)
  local result={}
  z.task(function() result.ok,result.value=pcall(handler,args,{source={Plugin=from or 99}}) end)
  return result
end
local function close(pane)
  z.__update(M,{PaneClosed=pane})
end
local function seed()
  rows,panes,cancelled,requests={},{},{},{}
  panes.p99={plugin_url='zellij:swamp-workflow'}
  panes.p2={plugin_url='zellij:agent-exit',is_suppressed=false}
  panes.t1={target='Local',exited=true,is_suppressed=true}
  panes.t3={target='Local',is_suppressed=true}
  panes.t4={target='Local',is_suppressed=true}
  save('work','view',{view=2,source=1,current=3,incarnation='incarnation'})
  save('work','launch',{work='work',key='launch',request=z.json.encode{method='workflow_run'},checkout='/control',run='run',outcome='pending',incarnation='incarnation'})
  save('work','source',{pane=1,supervisor='supervisor-1',process=101,owner=owner('work','source')})
  save('work','worker',{pane=3,supervisor='supervisor-3',process=103,owner=owner('work','worker'),outcome='pending',request=z.json.encode{method='command'}})
  save('work','runs',{entries={run={run='run',work='work',root='/control'},child={run='child',work='work',root='/control'}}})
  save('run','attachment',{run='run',work='work',root='/control'})
  save('child','attachment',{run='child',work='work',root='/control'})
  save('child','worker',{pane=4,supervisor='supervisor-4',process=104,owner=owner('child','worker'),outcome='pending',request=z.json.encode{method='command'}})
  save('unrelated','worker',{pane=5,owner=owner('unrelated','worker'),outcome='pending',request=z.json.encode{method='command'}})
  panes.t5={target='Local'}
end
reload_bridge()
for _,pane in ipairs{{Plugin=2},{Terminal=1},{Terminal=3}} do
  seed()
  close(pane)
  assert(read('work','view').closed,'closing an owned pane must end its lifecycle')
  assert(not panes.t1 and not panes.t3 and not panes.t4 and not panes.p2,'all owned panes close')
  assert(cancelled.run and cancelled.child,'running parent and child cancel')
  assert(panes.t5 and read('unrelated','worker').outcome=='pending','unrelated work survives')
  local count=#requests;close(pane);assert(#requests==count,'duplicate closure is harmless')
  for _,args in ipairs{
    {method='agent_spawn',work='work',key='late',spec={}},
    {method='agent_prompt',work='child',key='late',agent='worker',prompt='no'},
    {method='workflow_answer',directory='/control',model='questions',inputs={run='run'}},
    {method='swamp_read',directory='/control',operation='workflow.approve',payload={runId='child'}},
    {method='swamp_dispatch',work='child',key='late',directory='/control',operation='workflow.resume',payload={runId='child'}},
  } do
    local response=call(args)
    assert(response.ok==false and tostring(response.value):find('closed'),tostring(response.value))
  end
end
-- A real foreground placement carries both its subtitle and durable close identity.
seed()
rows[uid('work','view')]=nil;rows[uid('work','launch')]=nil;rows[uid('work','source')]=nil
panes.p99={plugin_url='zellij:agent-exit',is_suppressed=false}
local subtitles,subtitle_writes,replacements={},0,0
z.set_pane_subtitle=function(args)
  subtitles[z.json.encode(args.pane_id)]=args.subtitle
  subtitle_writes=subtitle_writes+1
end
z.hide_pane_with_id=function(pane) z.get_pane_info(pane).is_suppressed=true end
z.replace_pane_with_existing_pane=function(args)
  z.get_pane_info(args[1]).is_suppressed=true
  z.get_pane_info(args[2]).is_suppressed=false
  replacements=replacements+1
end
z.open_command_pane_background=function() return {Terminal=3} end
assert(call{method='workflow_run',work='work',key='launch',directory='/control',inputs={sourcePane=1}}.ok)
assert(call{method='command',work='work',key='foreground',argv={'helper'},cwd='/control',foreground=true}.ok)
assert(read('work','view').source==1 and read('work','view').current==3)
assert(subtitles[z.json.encode{Terminal=3}]=='Workflow · loading' and replacements==1)
close{Terminal=3}
assert(read('work','view').closed and not panes.p99 and not panes.t3)
assert(cancelled.run and cancelled.child)
local writes_after_close,replacements_after_close=subtitle_writes,replacements
z.__update(M,{PaneUpdate={panes={}}})
z.__update(M,{CommandPaneExited={3,0,{swamp_owner=z.json.encode(owner('work','foreground')),swamp_operation='foreground'}}})
z.__update(M,{CustomMessage={'swamp-response',z.json.encode{id=read('work','launch').requestId,event={kind='completed',run={status='succeeded'}}}}})
M.pipe{name='workflow-refresh-failed',source={Plugin=99},payload='work'}
panes.p88={plugin_url='zellij:swamp-workflow'}
local history=call({method='workflow_show',directory='/control',run='run'},88)
assert(history.ok and history.value.history.status=='cancelled')
tick()
assert(read('work','foreground').outcome=='interrupted' and read('work','view').closed)
assert(subtitle_writes==writes_after_close and replacements==replacements_after_close,'late updates cannot revive a closed placement')
-- Closure while a transport request is in flight must also destroy its late result.
for _,operation in ipairs{'workflow.validate','workflow.run','spawn'} do
  seed()
  rows[uid('work','launch')]=nil;rows[uid('work','source')]=nil
  delayed=operation
  z.agent.spawn_details=function()
    local future,resolve=z.async.future();resolve_delay=resolve
    return z.await(future)
  end
  local response=call(operation=='spawn' and {method='agent_spawn',work='work',key='late',spec={kind='claude'}}
    or {method='workflow_run',work='work',key='launch',directory='/control',inputs={sourcePane=1}})
  assert(resolve_delay and response.ok==nil)
  close{Plugin=2}
  delayed=nil
  if operation=='spawn' then panes.t6={target='Local'};resolve_delay{pane_id={Terminal=6},cwd='/control'}
  elseif operation=='workflow.validate' then resolve_delay{passed=true}
  else resolve_delay{runId='late-run'} end
  z.__update(M,{Timer=0})
  assert(response.ok==false,'late results must not complete successfully')
  assert(not panes.t6,'a late spawn is closed')
  if operation=='workflow.run' then assert(cancelled['late-run'],'late launch is cancelled') end
end

-- Intentional transitions are recorded before the close events they generate.
z.hide_pane_with_id=function() end
z.replace_pane_with_existing_pane=function() end
z.rerun_command_pane=function(id) assert(id==1);panes.t1.exited=false end
z.close_terminal_pane=function(id) panes['t'..id]=nil;close{Terminal=id} end
z.close_plugin_pane=function(id) panes['p'..id]=nil;close{Plugin=id} end
for _,action in ipairs{'restart','leave'} do
  seed()
  local response=call{method='pane_action',work='work',key=action,action=action}
  assert(response.ok,tostring(response.value))
  assert(not read('work','view').closed and read('work','view').transition==action)
  assert(not cancelled.run and not cancelled.child)
  assert((panes.t1~=nil)==(action=='restart'))
end
seed();rows[uid('work','source')]=nil
assert(call{method='pane_action',work='work',key='restart',action='restart'}.ok,'restart after validation failure')
-- Old host/session/incarnation records never claim this server's pane IDs.
for _,field in ipairs{'host_id','session','incarnation'} do
  seed()
  rows[uid('work','view')]=nil
  local source=read('work','source');source.owner[field]='other';save('work','source',source)
  close{Terminal=1}
  assert(panes.t3 and not cancelled.run)
end
-- Reopening history of a closed suspended run cannot bring its question back.
seed();close{Plugin=2}
local shown=call{method='workflow_show',directory='/control',run='run'}
assert(shown.ok and shown.value.history.status=='cancelled' and #shown.value.questions.results==0)

-- Admission failures are definitive even when closure interrupted the stored claim first.
local function pending_launch(known_runs)
  seed()
  rows[uid('work','launch')]=nil;rows[uid('work','source')]=nil
  if not known_runs then rows[uid('work','runs')]=nil end
  delayed='workflow.run';resolve_delay=nil
  local response=call{method='workflow_run',work='work',key='launch',directory='/control',inputs={sourcePane=1}}
  assert(resolve_delay and response.ok==nil)
  return response
end
local function history_searches()
  local count=0
  for _,request in ipairs(requests) do
    if request.operation=='workflow.history.search' then count=count+1 end
  end
  return count
end
for _,during_close in ipairs{false,true} do
  for _,known_runs in ipairs{false,true} do
    local response=pending_launch(known_runs)
    if during_close then close{Plugin=2} end
    local searches=history_searches()
    delayed=nil;resolve_delay{error={code='not_dispatched',message='extensions failed to load'}}
    z.__update(M,{Timer=0})
    assert(response.ok==false)
    local launch=read('work','launch')
    assert(launch.not_dispatched==true,'non-dispatch survives an interrupted outcome')
    assert(launch.outcome==(during_close and 'interrupted' or 'failed'))
    if not during_close then
      assert(call{method='workflow_show',launch='work',directory='/control'}.ok)
      close{Plugin=2}
    end
    tick()
    assert(not read('work','view').cancel_pending,'a known non-launch needs no retry')
    assert(history_searches()==searches,'confirmed non-dispatch never searches launch history')
    if known_runs then assert(cancelled.run and cancelled.child,'independently recorded runs still cancel') end
    local count=#requests
    reload_bridge();tick();tick()
    assert(#requests==count,'reload must not resume settled cancellation polling')
  end
end
-- A lost response remains ambiguous: history polling must continue, including after reload.
local ambiguous=pending_launch(false)
close{Plugin=2}
delayed=nil;resolve_delay{error={code='transport_unknown',message='connection lost'}}
z.__update(M,{Timer=0})
assert(ambiguous.ok==false and not read('work','launch').not_dispatched)
for _=1,2 do
  local searches=history_searches();tick()
  assert(history_searches()>searches and read('work','view').cancel_pending)
end
local searches=history_searches();reload_bridge()
assert(history_searches()>searches and read('work','view').cancel_pending,'reload recovers ambiguous launches')

-- A known parent cannot settle cancellation while a continuation's reply is lost.
for _,reload in ipairs{false,true} do
  seed()
  delayed='workflow.run';resolve_delay=nil
  local continuation=call{method='swamp_dispatch',work='run',key='continuation',directory='/checkout',
    operation='workflow.run',payload={workflowIdOrName='child-workflow'}}
  assert(resolve_delay and continuation.ok==nil)
  close{Plugin=2}
  delayed=nil;resolve_delay{error={code='transport_unknown',message='connection lost'}}
  z.__update(M,{Timer=0})
  assert(continuation.ok==false)
  assert(cancelled.run and cancelled.child,'known runs still cancel')
  assert(read('work','view').cancel_pending,'the unresolved continuation still needs cancellation')
  local searches=history_searches()
  if reload then reload_bridge() else tick() end
  assert(history_searches()>searches and read('work','view').cancel_pending)
  local search
  for _,request in ipairs(requests) do
    if request.operation=='workflow.history.search' then search=request end
  end
  assert(search.directory=='/checkout','search the continuation checkout')
  assert(search.payload.filter=='has(tags.zellijOperation) && tags.zellijOperation == '..z.json.encode(uid('run','continuation')))

  -- Delivery can advance while history is in flight. Keep its fields and outcome.
  delayed='workflow.history.search';tick()
  local current=read('run','continuation')
  current.message='concurrent delivery';current.outcome='failed'
  z.data.apply(z.data.put('delivery',{value=z.json.encode(current)},uid('run','continuation'),rows[uid('run','continuation')].rev))
  delayed='workflow.cancel'
  resolve_delay{results={{runId='late-child'}}}
  z.__update(M,{Timer=0})
  current=read('run','continuation')
  assert(current.run=='late-child' and current.message=='concurrent delivery' and current.outcome=='failed')
  assert(not read('late-child','attachment'),'discovery needs no attachment callback')
  assert(read('work','view').cancel_pending,'cancellation must finish before clearing the retry')
  delayed=nil;resolve_delay{error={code='transport_unknown',message='cancel reply lost'}}
  z.__update(M,{Timer=0})
  assert(read('work','view').cancel_pending,'failed cancellation stays durable')
  tick()
  assert(cancelled['late-child'] and not read('work','view').cancel_pending)
  assert(panes.t5 and read('unrelated','worker').outcome=='pending','unrelated work survives')
  local launches=0
  for _,request in ipairs(requests) do
    if request.operation=='workflow.run' then launches=launches+1 end
  end
  assert(launches==1,'cancellation never redispatches a continuation')
  local count=#requests;reload_bridge();tick();tick()
  assert(#requests==count,'resolved cancellation stops polling')
end

-- Definitive non-dispatch also settles an unresolved continuation beside known runs.
seed();delayed='workflow.run'
local continuation=call{method='swamp_dispatch',work='run',key='continuation',directory='/checkout',
  operation='workflow.run',payload={workflowIdOrName='child-workflow'}}
close{Plugin=2}
local searches=history_searches()
delayed=nil;resolve_delay{error={code='not_dispatched',message='not admitted'}}
z.__update(M,{Timer=0});tick()
assert(continuation.ok==false and read('run','continuation').not_dispatched)
assert(cancelled.run and cancelled.child and not read('work','view').cancel_pending)
assert(history_searches()==searches,'confirmed non-dispatch stops continuation searches')
local count=#requests;reload_bridge();tick();tick()
assert(#requests==count,'non-dispatched continuation stays settled after reload')

-- A picker launches before reporting: closure can win between those two actions.
for _,action in ipairs{'duplicate','fork'} do
  for _,cancel_first in ipairs{true,false} do
    for _,terminal_removed in ipairs{false,true} do
      seed()
      local picker=6
      local terminal=action=='fork' and 1 or 7
      local acks=0
      panes.p6={plugin_url='zellij:hocus-new-agent'}
      z.message_to_plugin=function(message)
        if message.message_name=='swamp-pane-ack' then
          assert(message.destination_plugin_id==picker)
          assert(read('work',action).outcome=='completed')
          assert(not panes.p2,'acceptance closes the view before acknowledgment')
          acks=acks+1
        end
      end
      local operation=call{method='pane_action',work='work',key=action,action=action}
      assert(operation.ok and operation.value.outcome=='pending')
      panes['t'..terminal]={target='Local',exited=false}
      if cancel_first then z.close_terminal_pane(1) end
      if terminal_removed then z.close_terminal_pane(terminal) end
      -- Removing a reused source is itself lifecycle cancellation.
      local interrupted=cancel_first or (action=='fork' and terminal_removed)
      assert((read('work','view').closed==true)==interrupted)
      assert(panes.p6,'picker stays alive until its result is handled')
      local reply={name='swamp-pane-result',source={Plugin=picker},
        payload=z.json.encode{work='work',key=action,choice='started',pane_id={Terminal=terminal}}}
      local close_picker=z.close_plugin_pane
      z.close_plugin_pane=function(id)
        if id==picker then assert(not panes['t'..terminal],'close the produced terminal before the picker') end
        close_picker(id)
      end
      M.pipe(reply)
      z.close_plugin_pane=close_picker
      local revision=rows[uid('work',action)].rev
      M.pipe(reply)
      assert(rows[uid('work',action)].rev==revision,'duplicate results do not rewrite the outcome')
      if interrupted then
        assert(read('work','view').closed and read('work',action).outcome=='interrupted')
        assert(not panes.p6 and not panes['t'..terminal] and acks==0)
      else
        assert(not read('work','view').closed and read('work','view').transition=='fork')
        assert(read('work',action).outcome=='completed' and acks==2)
        assert(not panes.p2 and panes.p6,'the picker closes itself after acknowledgment')
        assert((panes['t'..terminal]~=nil)==not terminal_removed,'accepted terminal survives view closure')
        z.close_plugin_pane(picker)
        M.pipe(reply)
        assert(acks==2,'a result from an already removed picker is harmless')
      end
    end
  end
end
-- Malformed started results cannot close any pane or settle an operation.
seed()
panes.p6={plugin_url='zellij:hocus-new-agent'}
z.message_to_plugin=function() end
assert(call{method='pane_action',work='work',key='duplicate',action='duplicate'}.ok)
for _,pane in ipairs{z.null,7,{Plugin=2},{Terminal=-1},{Terminal=1.5},{Terminal=4294967296},{Terminal=1,Plugin=2}} do
  local ok=pcall(M.pipe,{name='swamp-pane-result',source={Plugin=6},
    payload=z.json.encode{work='work',key='duplicate',choice='started',pane_id=pane}})
  assert(not ok,'invalid produced pane must be rejected')
  assert(panes.t1 and panes.p2 and panes.p6 and read('work','duplicate').outcome=='pending')
end
