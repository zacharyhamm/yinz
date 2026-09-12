// The consumer harness runs real bridge code with revision-checked durable rows.
// Resolving a transport promise or a retry timer explicitly controls each race.
globalThis.done=false;
const uid=(work,key)=>JSON.stringify(['host','session',work,key]);
const read=(work,key)=>rows.has(uid(work,key))?JSON.parse(rows.get(uid(work,key)).value):undefined;
const save=(work,key,value)=>rows.set(uid(work,key),{rev:(rows.get(uid(work,key))?.rev??0)+1,value:JSON.stringify(value)});
const owner=(work,agent)=>({host_id:'host',session:'session',incarnation:'incarnation',work,agent});
let requests=[],cancelled={},timers=[],delayed,release,panemap={},acks=0;
const flush=async()=>{for(let i=0;i<50;i++)await Promise.resolve();};
const tick=async()=>{const pending=timers;timers=[];pending.forEach(resolve=>resolve());await flush();};
const close=pane=>handlers.PaneClosed(pane);
const call=(args,from=99)=>handler(args,{source:{Plugin:from}});
const rejected=async fn=>{let error;try{await fn();}catch(e){error=e;}check(error!==undefined);return String(error);};
mock.getPluginIds=()=>({host_id:'host',plugin_id:99});
mock.sleep=()=>new Promise(resolve=>timers.push(resolve));
mock.projectConfig={read:()=>({root:'/control',config:{workflow:'exit'}})};
mock.command=(name,args)=>{
  if(name==='GetPaneInfo')return panemap['Plugin' in args?'p'+args.Plugin:'t'+args.Terminal];
  if(name==='CloseTerminalPane'){delete panemap['t'+args];close({Terminal:args});return;}
  if(name==='ClosePluginPane'){delete panemap['p'+args];close({Plugin:args});return;}
  if(name==='HidePaneWithId'){const pane=mock.command('GetPaneInfo',args);if(pane)pane.is_suppressed=true;return;}
  if(name==='ReplacePaneWithExistingPane'){
    mock.command('GetPaneInfo',args[0]).is_suppressed=true;mock.command('GetPaneInfo',args[1]).is_suppressed=false;return;
  }
  if(name==='RerunCommandPane'){panemap['t'+args].exited=false;return;}
  if(name==='SetPaneSubtitle')return;
  if(name==='MessageToPlugin'){if(args.message_name==='swamp-pane-ack'){check(!panemap.p2);acks++;}return;}
  if(name==='OpenCommandPaneBackground')return {Terminal:3};
  throw Error('Unexpected command '+name);
};
mock.runCommand=async opts=>({exitCode:0,stderr:new Uint8Array(),stdout:zellij.encodeUtf8(opts.command[0]==='git'?'/control\n':JSON.stringify(
  Object.keys(panemap).filter(p=>p[0]==='t').map(p=>({terminal_id:Number(p.slice(1)),attached:true,session_name:'session',host_id:'supervisor-'+p.slice(1),child_pid:100+Number(p.slice(1))}))))});
mock.swampRequest=async opts=>{
  requests.push(opts);
  if(delayed===opts.operation)return new Promise(resolve=>{release=resolve;});
  if(opts.operation==='workflow.cancel'){check(read('work','view').closed);cancelled[opts.payload.runId]=true;}
  if(opts.operation==='workflow.validate')return {passed:true};
  if(opts.operation==='workflow.run')return {runId:'run'};
  if(opts.operation==='workflow.history.search')return {results:[]};
  if(opts.operation==='workflow.history.get')return {id:opts.payload.workflowIdOrName,status:'running'};
  return {};
};
function reload(){timers=[];handlers={};reloadConsumer();M.load();}
function seed(){
  rows.clear();timers=[];requests=[];cancelled={};delayed=undefined;release=undefined;acks=0;
  panemap={p99:{plugin_url:'zellij:swamp-workflow'},p2:{plugin_url:'zellij:agent-exit',is_suppressed:false},
    t1:{target:'Local',exited:true,is_suppressed:true},t3:{target:'Local',is_suppressed:true},t4:{target:'Local',is_suppressed:true},t5:{target:'Local'}};
  save('work','view',{view:2,source:1,current:3,incarnation:'incarnation'});
  save('work','launch',{work:'work',key:'launch',request:JSON.stringify({method:'workflow_run'}),checkout:'/control',run:'run',outcome:'pending',incarnation:'incarnation'});
  save('work','source',{pane:1,supervisor:'supervisor-1',process:101,owner:owner('work','source')});
  for(const [work,pane] of [['work',3],['child',4],['unrelated',5]])save(work,'worker',{pane,supervisor:'supervisor-'+pane,process:100+pane,owner:owner(work,'worker'),outcome:'pending',request:JSON.stringify({method:'command'})});
  save('work','runs',{entries:{run:{run:'run',work:'work',root:'/control'},child:{run:'child',work:'work',root:'/control'}}});
  for(const run of ['run','child'])save(run,'attachment',{run,work:'work',root:'/control'});
  reload();
}
const searches=()=>requests.filter(r=>r.operation==='workflow.history.search').length;
(async()=>{
  for(const pane of [{Plugin:2},{Terminal:1},{Terminal:3}]){
    seed();close(pane);await flush();
    check(read('work','view').closed && !read('work','view').cancel_pending);
    check(!panemap.t1&&!panemap.t3&&!panemap.t4&&!panemap.p2);
    check(cancelled.run&&cancelled.child&&panemap.t5&&read('unrelated','worker').outcome==='pending');
    const count=requests.length;close(pane);await flush();check(requests.length===count);
    for(const args of [
      {method:'agent_spawn',work:'work',key:'late',spec:{}},
      {method:'agent_prompt',work:'child',key:'late',agent:'worker',prompt:'no'},
      {method:'workflow_answer',directory:'/control',model:'questions',inputs:{run:'run'}},
      {method:'swamp_read',directory:'/control',operation:'workflow.approve',payload:{runId:'child'}},
      {method:'swamp_dispatch',work:'unrelated',key:'late',directory:'/control',operation:'workflow.resume',payload:{runId:'child'}},
    ])check((await rejected(()=>call(args))).includes('closed'));
    const shown=await call({method:'workflow_show',directory:'/control',run:'run'});
    check(shown.history.status==='cancelled'&&shown.questions.results.length===0);
    await rejected(()=>call({method:'workflow_attach',work:'work',key:'attach',run:'late',workflow:'child',directory:'/late'}));
    await flush();check(cancelled.late);
  }
  // Foreground identity survives beyond volatile placements; stale events cannot revive it.
  seed();for(const key of ['view','launch','source'])rows.delete(uid('work',key));
  panemap.p99={plugin_url:'zellij:agent-exit',is_suppressed:false};
  await call({method:'workflow_run',work:'work',key:'launch',directory:'/control',inputs:{sourcePane:1}});
  await call({method:'command',work:'work',key:'foreground',argv:['helper'],cwd:'/control',foreground:true});
  check(read('work','view').source===1&&read('work','view').current===3);
  close({Terminal:3});await flush();check(read('work','view').closed&&!panemap.p99);
  handlers.PaneUpdate();handlers.CommandPaneExited([3,0,{swamp_owner:JSON.stringify(owner('work','foreground')),swamp_operation:'foreground'}]);
  handlers.CustomMessage(['swamp-response',JSON.stringify({id:read('work','launch').requestId,event:{kind:'completed',run:{status:'succeeded'}}})]);
  await tick();check(read('work','foreground').outcome==='interrupted');

  for(const operation of ['workflow.validate','workflow.run','supervisor','spawn','command']){
    seed();rows.delete(uid('work','launch'));rows.delete(uid('work','source'));
    delayed=operation;
    const originalRun=mock.runCommand,originalSpawn=mock.agent.spawnDetails,originalCommand=mock.command;
    if(operation==='supervisor')mock.runCommand=opts=>opts.command[0]==='git'?originalRun(opts):new Promise(resolve=>{release=resolve;});
    if(operation==='spawn')mock.agent.spawnDetails=()=>{close({Plugin:2});panemap.t6={target:'Local'};return {pane_id:{Terminal:6},cwd:'/control'};};
    if(operation==='command')mock.command=(name,args)=>{if(name!=='OpenCommandPaneBackground')return originalCommand(name,args);close({Plugin:2});panemap.t3={target:'Local'};return {Terminal:3};};
    const promise=rejected(()=>call(operation==='spawn'?{method:'agent_spawn',work:'work',key:'late',spec:{kind:'claude'}}:
      operation==='command'?{method:'command',work:'work',key:'late',argv:['helper']}:
      {method:'workflow_run',work:'work',key:'launch',directory:'/control',inputs:{sourcePane:1}}));
    await flush();
    if(!['spawn','command'].includes(operation)){
      check(!!release);close({Plugin:2});delayed=undefined;
      release(operation==='workflow.validate'?{passed:true}:operation==='supervisor'?{exitCode:0,stdout:zellij.encodeUtf8(JSON.stringify([{terminal_id:1,session_name:'session',attached:true,host_id:'supervisor-1',child_pid:101}])),stderr:new Uint8Array()}:{runId:'late-run'});
    }
    await promise;await flush();check(!panemap.t6&&!panemap.t3);
    if(operation==='workflow.run'){await tick();check(cancelled['late-run']);}
    mock.runCommand=originalRun;mock.agent.spawnDetails=originalSpawn;mock.command=originalCommand;
  }
  for(const action of ['restart','leave']){
    seed();await call({method:'pane_action',work:'work',key:action,action});await flush();
    check(!read('work','view').closed&&read('work','view').transition===action&&!cancelled.run);
    check(!!panemap.t1===(action==='restart'));
  }
  seed();rows.delete(uid('work','source'));await call({method:'pane_action',work:'work',key:'restart',action:'restart'});
  for(const field of ['host_id','session','incarnation']){
    seed();rows.delete(uid('work','view'));const source=read('work','source');source.owner[field]='other';save('work','source',source);
    close({Terminal:1});await flush();check(panemap.t3&&!cancelled.run);
  }
  for(const duringClose of [false,true])for(const knownRuns of [false,true]){
    seed();rows.delete(uid('work','launch'));rows.delete(uid('work','source'));if(!knownRuns)rows.delete(uid('work','runs'));
    delayed='workflow.run';const promise=rejected(()=>call({method:'workflow_run',work:'work',key:'launch',directory:'/control',inputs:{sourcePane:1}}));
    await flush();check(!!release);if(duringClose)close({Plugin:2});await flush();const before=searches();
    delayed=undefined;release({error:{code:'not_dispatched',message:'not admitted'}});await promise;
    check(read('work','launch').not_dispatched&&read('work','launch').outcome===(duringClose?'interrupted':'failed'));
    if(!duringClose){check((await call({method:'workflow_show',launch:'work',directory:'/control'})).history.status==='failed');close({Plugin:2});}
    await tick();check(!read('work','view').cancel_pending&&searches()===before);
    if(knownRuns)check(cancelled.run&&cancelled.child);
    const count=requests.length;reload();await tick();check(requests.length===count);
  }
  for(const continuation of [false,true])for(const afterReload of [false,true]){
    seed();if(!continuation){rows.delete(uid('work','launch'));rows.delete(uid('work','source'));rows.delete(uid('work','runs'));}
    delayed='workflow.run';const promise=rejected(()=>call(continuation?
      {method:'swamp_dispatch',work:'run',key:'continuation',directory:'/checkout',operation:'workflow.run',payload:{workflowIdOrName:'child-workflow'}}:
      {method:'workflow_run',work:'work',key:'launch',directory:'/control',inputs:{sourcePane:1}}));
    await flush();close({Plugin:2});await flush();delayed=undefined;release({error:{code:'transport_unknown',message:'connection lost'}});await promise;await tick();
    check(read('work','view').cancel_pending);const before=searches();
    if(afterReload){reload();await flush();}else await tick();
    check(searches()>before&&read('work','view').cancel_pending);
    const work=continuation?'run':'work',key=continuation?'continuation':'launch';
    const query=requests.filter(r=>r.operation==='workflow.history.search').at(-1);
    check(query.directory===(continuation?'/checkout':'/control')&&query.payload.filter==='has(tags.zellijOperation) && tags.zellijOperation == '+JSON.stringify(uid(work,key)));
    delayed='workflow.history.search';await tick();
    const current=read(work,key);current.message='concurrent delivery';current.outcome='failed';save(work,key,current);
    delayed='workflow.cancel';release({results:[{runId:'late-child'}]});await flush();
    check(read(work,key).run==='late-child'&&read(work,key).message==='concurrent delivery'&&read(work,key).outcome==='failed');
    check(read('work','view').cancel_pending);
    delayed=undefined;release({error:{code:'transport_unknown',message:'cancel reply lost'}});await flush();await tick();
    check(cancelled['late-child']&&!read('work','view').cancel_pending&&panemap.t5);
    check(requests.filter(r=>r.operation==='workflow.run').length===1);
    const count=requests.length;reload();await tick();check(requests.length===count);
  }
  // Definitive rejection settles a continuation even when its parent is already known.
  seed();delayed='workflow.run';
  const continuation=rejected(()=>call({method:'swamp_dispatch',work:'run',key:'continuation',directory:'/checkout',operation:'workflow.run',payload:{workflowIdOrName:'child-workflow'}}));
  await flush();close({Plugin:2});await flush();const before=searches();
  delayed=undefined;release({error:{code:'not_dispatched',message:'not admitted'}});await continuation;await tick();
  check(read('run','continuation').not_dispatched&&cancelled.run&&cancelled.child&&!read('work','view').cancel_pending&&searches()===before);
  const count=requests.length;reload();await tick();check(requests.length===count);

  // A late attachment invalidates the cancellation pass's original revision.
  seed();delayed='workflow.cancel';close({Plugin:2});await flush();check(!!release);
  await rejected(()=>call({method:'workflow_attach',work:'work',key:'attachment',run:'late',workflow:'child',directory:'/late'}));
  delayed=undefined;release({});await flush();check(read('work','view').cancel_pending);
  await tick();check(cancelled.late&&!read('work','view').cancel_pending);

  for(const action of ['duplicate','fork'])for(const cancelFirst of [true,false])for(const terminalRemoved of [false,true]){
    seed();panemap.p6={plugin_url:'zellij:hocus-new-agent'};
    check((await call({method:'pane_action',work:'work',key:action,action})).outcome==='pending');
    const terminal=action==='fork'?1:7;panemap['t'+terminal]={target:'Local',exited:false};
    if(cancelFirst)mock.command('CloseTerminalPane',1);
    if(terminalRemoved)mock.command('CloseTerminalPane',terminal);
    const interrupted=cancelFirst||(action==='fork'&&terminalRemoved);
    const reply={name:'swamp-pane-result',source:{Plugin:6},payload:JSON.stringify({work:'work',key:action,choice:'started',pane_id:{Terminal:terminal}})};
    await M.pipe(reply);const rev=rows.get(uid('work',action)).rev;await M.pipe(reply);check(rows.get(uid('work',action)).rev===rev);
    if(interrupted)check(read('work','view').closed&&read('work',action).outcome==='interrupted'&&!panemap.p6&&!panemap['t'+terminal]&&acks===0);
    else {
      check(!read('work','view').closed&&read('work','view').transition==='fork'&&acks===2&&!panemap.p2&&panemap.p6);
      check(!!panemap['t'+terminal]===!terminalRemoved);
      mock.command('ClosePluginPane',6);await M.pipe(reply);check(acks===2);
    }
    await flush();
  }
  seed();panemap.p6={plugin_url:'zellij:hocus-new-agent'};
  await call({method:'pane_action',work:'work',key:'duplicate',action:'duplicate'});
  for(const pane_id of [null,7,{Plugin:2},{Terminal:-1},{Terminal:1.5},{Terminal:4294967296},{Terminal:1,Plugin:2}]){
    await rejected(()=>M.pipe({name:'swamp-pane-result',source:{Plugin:6},payload:JSON.stringify({work:'work',key:'duplicate',choice:'started',pane_id})}));
    check(panemap.t1&&panemap.p2&&panemap.p6&&read('work','duplicate').outcome==='pending');
  }
  done=true;
})().catch(error=>{failure=error.stack;});
