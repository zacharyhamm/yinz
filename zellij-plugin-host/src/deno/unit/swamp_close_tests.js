// The consumer harness runs real bridge code with revision-checked durable rows.
// Resolving a transport promise or a retry timer explicitly controls each race.
globalThis.done=false;
const uid=(work,key)=>JSON.stringify(['host','session',work,key]);
const read=(work,key)=>rows.has(uid(work,key))?JSON.parse(rows.get(uid(work,key)).value):undefined;
const save=(work,key,value)=>rows.set(uid(work,key),{rev:(rows.get(uid(work,key))?.rev??0)+1,value:JSON.stringify(value)});
const owner=(work,agent)=>({host_id:'host',session:'session',incarnation:'incarnation',work,agent});
let requests=[],cancelled={},timers=[],delayed,release,panemap={},acks=0,transientRestore=false,missingPane;
const flush=async()=>{for(let i=0;i<50;i++)await Promise.resolve();};
const tick=async()=>{const pending=timers;timers=[];pending.forEach(resolve=>resolve());await flush();};
const close=pane=>handlers.PaneClosed(pane);
const call=(args,from=99)=>handler(args,{source:{Plugin:from}});
const rejected=async fn=>{let error;try{await fn();}catch(e){error=e;}check(error!==undefined);return String(error);};
mock.getPluginIds=()=>({host_id:'host',plugin_id:99});
mock.sleep=()=>new Promise(resolve=>timers.push(resolve));
mock.projectConfig={read:()=>({root:'/control',config:{workflow:'exit'}})};
mock.command=(name,args)=>{
  if(name==='GetPaneInfo'&&args.Terminal===missingPane&&missingPane!==undefined){missingPane=undefined;return;}
  if(name==='GetPaneInfo')return panemap['Plugin' in args?'p'+args.Plugin:'t'+args.Terminal];
  if(name==='CloseTerminalPane'){delete panemap['t'+args];close({Terminal:args});return;}
  if(name==='ClosePluginPane'){delete panemap['p'+args];close({Plugin:args});return;}
  if(name==='HidePaneWithId'){const pane=mock.command('GetPaneInfo',args);if(pane)pane.is_suppressed=true;return;}
  if(name==='ReplacePaneWithExistingPane'){
    mock.command('GetPaneInfo',args[0]).is_suppressed=true;mock.command('GetPaneInfo',args[1]).is_suppressed=false;
    if(transientRestore&&'Terminal' in args[0]&&'Plugin' in args[1])missingPane=args[0].Terminal;
    return;
  }
  if(name==='RerunCommandPane'){panemap['t'+args].exited=false;return;}
  if(name==='SetPaneSubtitle')return;
  if(name==='MessageToPlugin'){if(args.message_name==='swamp-pane-ack'){check(!panemap.p2);acks++;}return;}
  if(name==='OpenCommandPaneBackground')return {Terminal:3};
  throw Error('Unexpected command '+name);
};
mock.runCommand=async opts=>({exitCode:0,stderr:new Uint8Array(),stdout:zellij.encodeUtf8(opts.command[0]==='git'?'/control\n':JSON.stringify(
  Object.keys(panemap).filter(p=>p[0]==='t').map(p=>({terminal_id:Number(p.slice(1)),attached:true,session_name:'session',host_id:'supervisor-'+p.slice(1),child_pid:panemap[p].process??100+Number(p.slice(1)),child_exit_status:panemap[p].exited?(panemap[p].exit_status===undefined?0:panemap[p].exit_status):null}))))});
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
  rows.clear();timers=[];requests=[];cancelled={};delayed=undefined;release=undefined;acks=0;transientRestore=false;missingPane=undefined;
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
  for(const state of ['completed','failed','signal','running','replaced','wrong-owner','missing','detached']) {
    seed();
    const operation=read('work','worker');
    operation.outcome=state==='failed'?'failed':'completed';operation.exitCode=state==='failed'?7:0;
    if(state==='signal'){operation.outcome='failed';operation.exitCode=null;panemap.t3.exit_status=null;}
    if(state==='wrong-owner')operation.owner.agent='someone-else';
    save('work','worker',operation);
    panemap.t3.exited=state!=='running';
    if(state==='replaced')panemap.t3.process=999;
    if(state==='missing')delete panemap.t3;
    if(state==='detached')delete panemap.p2;
    const args={method:'command_close',work:'work',key:'worker'};
    if(['running','replaced','wrong-owner'].includes(state)) {
      await rejected(()=>call(args));check(panemap.t3&&!read('work','worker').closed);
    } else {
      await call(args);await call(args);await flush();
      check(!panemap.t3&&read('work','worker').closed&&!read('work','view').closed);
      check(!Object.keys(cancelled).length&&read('work','view').current===undefined);
    }
    handlers.CommandPaneExited([3,99,{swamp_owner:JSON.stringify(operation.owner),swamp_operation:'worker'}]);
    check(read('work','worker').exitCode===operation.exitCode);
  }
  for(const detached of [false,true]) for(const exitCode of [0,null]) {
    seed();for(const key of ['view','launch','source'])rows.delete(uid('work',key));
    panemap.p99={plugin_url:'zellij:agent-exit',is_suppressed:false};
    await call({method:'workflow_run',work:'work',key:'launch',directory:'/control',inputs:{sourcePane:1}});
    await call({method:'command',work:'work',key:'tests',argv:['helper'],foreground:true,capture:{directory:'/capture'}});
    check(panemap.p99.is_suppressed&&!panemap.t3.is_suppressed);
    panemap.t3.exited=true;
    panemap.t3.exit_status=exitCode;
    handlers.CommandPaneExited([3,exitCode,{swamp_owner:JSON.stringify(owner('work','tests')),swamp_operation:'tests'}]);
    check(read('work','tests').outcome===(exitCode===0?'completed':'failed'));
    check(panemap.p99.is_suppressed&&!panemap.t3.is_suppressed);
    if(detached) { panemap.t3.is_suppressed=true;handlers.PaneUpdate(); }
    transientRestore=!detached;
    await call({method:'command_close',work:'work',key:'tests'});await call({method:'command_close',work:'work',key:'tests'});await flush();
    check(!panemap.t3&&panemap.p99.is_suppressed===detached&&!read('work','view').closed);
    check(!Object.keys(cancelled).length);
  }
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
  // Refusal preserves active work and the view before or after source registration.
  for(const fallback of [false,true]){
    seed();if(fallback)rows.delete(uid('work','source'));
    panemap.t1.restart_error='Cannot restart agent: unsupported print option';
    const view=rows.get(uid('work','view')).value,command=mock.command;
    mock.command=(name,args)=>{if(name==='RerunCommandPane')throw Error('refused restart must not rerun');return command(name,args);};
    const error=await rejected(()=>call({method:'pane_action',work:'work',key:'restart',action:'restart',cancel_active:true}));
    mock.command=command;await flush();
    check(error.includes('unsupported print option'));
    check(rows.get(uid('work','view')).value===view);
    check(!cancelled.run&&!cancelled.child&&panemap.t3&&panemap.t4);
    check(panemap.t1.exited&&panemap.t1.is_suppressed&&panemap.p2);
    check(read('work','restart').outcome!=='completed');
  }
  seed();
  await call({method:'pane_action',work:'work',key:'restart',action:'restart',cancel_active:true});await flush();
  check(read('work','view').closed&&read('work','view').transition==='restart'&&cancelled.run&&cancelled.child);
  check(panemap.t1&&!panemap.t1.exited&&!read('work','view').cancel_pending);
  for(const args of [
    {method:'agent_spawn',work:'work',key:'late',spec:{}},
    {method:'swamp_dispatch',work:'child',key:'late',directory:'/control',operation:'workflow.resume',payload:{runId:'child'}},
    {method:'workflow_attach',work:'work',key:'attachment',run:'late-attach',workflow:'child',directory:'/control'},
  ])check((await rejected(()=>call(args))).includes('closed'));
  await tick();check(cancelled['late-attach']);
  seed();delayed='workflow.run';
  const inFlight=rejected(()=>call({method:'swamp_dispatch',work:'run',key:'continuation',directory:'/control',operation:'workflow.run',payload:{workflowIdOrName:'child'}}));
  await flush();
  await call({method:'pane_action',work:'work',key:'restart',action:'restart',cancel_active:true});await flush();
  check(cancelled.run&&cancelled.child&&!cancelled['late-run']);
  delayed=undefined;release({runId:'late-run'});await inFlight;await tick();check(cancelled['late-run']);

  const originalCommand=mock.command;
  let pickers=[];
  mock.command=(name,args)=>{
    if(name==='MessageToPlugin'&&args.message_name==='swamp-pane-action')pickers.push(args.plugin_config);
    return originalCommand(name,args);
  };
  const picker=(key,choice='started',pane=1)=>M.pipe({name:'swamp-pane-result',source:{Plugin:6},payload:JSON.stringify({work:'work',key,choice,pane_id:choice==='started'?{Terminal:pane}:null})});
  seed();panemap.p6={plugin_url:'zellij:hocus-new-agent'};
  await call({method:'pane_action',work:'work',key:'fork-1',action:'fork',cancel_active:true});
  const again=await call({method:'pane_action',work:'work',key:'fork-2',action:'fork',cancel_active:true});
  check(again.key==='fork-1'&&!read('work','fork-2')&&pickers.length===2&&pickers.every(p=>p.swamp_key==='fork-1'));
  // A cancellation is acknowledged too, while leaving the active workflow alone.
  mock.command=(name,args)=>{
    if(name==='MessageToPlugin'&&args.message_name==='swamp-pane-ack'){acks++;return;}
    if(name==='MessageToPlugin'&&args.message_name==='swamp-pane-action')pickers.push(args.plugin_config);
    return originalCommand(name,args);
  };
  await picker('fork-1','cancelled');await picker('fork-1','cancelled');
  check(acks===2&&!cancelled.run&&!read('work','view').transition);
  await call({method:'pane_action',work:'work',key:'fork-3',action:'fork',cancel_active:true});
  check(pickers.at(-1).swamp_key==='fork-3');
  await call({method:'pane_action',work:'work',key:'fork-4',action:'fork'});
  panemap.t1.exited=false;await picker('fork-3');await flush();
  check(read('work','view').closed&&read('work','view').transition==='fork'&&cancelled.run&&cancelled.child);
  await picker('fork-3');check(panemap.t1&&panemap.p6);
  panemap.t8={target:'Local'};await picker('fork-4','started',8);check(!panemap.t8&&!panemap.p6&&panemap.t1);
  for(const action of ['duplicate','fork']){
    seed();pickers=[];panemap.p6={plugin_url:'zellij:hocus-new-agent'};
    await call({method:'pane_action',work:'work',key:'menu',action});
    const press=await call({method:'pane_action',work:'work',key:'button',action:'fork',cancel_active:true});
    check(press.key==='button'&&pickers[1].fork_focused==='true'&&!pickers[1].duplicate_focused);
    panemap.t1.exited=false;await picker('button');await flush();check(cancelled.run&&cancelled.child);
  }
  seed();panemap.p6={plugin_url:'zellij:hocus-new-agent'};
  await call({method:'pane_action',work:'work',key:'fork-c',action:'fork',cancel_active:true});
  await call({method:'pane_action',work:'work',key:'restart',action:'restart',cancel_active:true});
  check(read('work','fork-c').outcome==='interrupted');
  await picker('fork-c','cancelled');check(!panemap.p6&&acks===0);
  mock.command=originalCommand;

  let launches=0,sends=0;
  mock.agent.send=()=>sends++;
  for(const prompt of ['', '  ', 'bad\0', 'x'.repeat(262145), false, 7, '😀'.repeat(65537)]){
    seed();mock.agent.spawnDetails=()=>{launches++;throw Error('must not spawn');};
    await rejected(()=>call({method:'agent_spawn',work:'work',key:'startup',spec:{prompt}}));
    check(!read('work','startup'));
  }
  check(launches===0);
  for(const immediate of [false,true]){
    seed();const prompt='Resolve "quoted" conflicts.\nStage, then exit.';
    mock.agent.spawnDetails=opts=>{
      launches++;check(read('work','startup').owner.agent==='startup'&&read('work','startup').outcome==='pending'&&opts.prompt===prompt);
      panemap.t6={target:'Local'};
      if(immediate)handlers.CommandPaneExited([6,17,opts.context]);
      return {pane_id:{Terminal:6},cwd:'/control'};
    };
    const args={method:'agent_spawn',work:'work',key:'startup',waitForExit:true,spec:{prompt},prompt_sha256:'a'.repeat(64)};
    await call(args);check(read('work','startup').outcome===(immediate?'process-exit':'pending'));
    check((await call(args)).pane===6&&launches===(immediate?2:1));
    if(!immediate){
      const event=(hook,hash,turn)=>handlers.AgentEvent({pane_id:6,hook,session_id:'agent-session',prompt:prompt.slice(0,10),last_message:'Successful Stop message',
        raw:JSON.stringify({_zellij_workflow_owner:JSON.stringify(owner('work','startup')),prompt_sha256:hash,turn_id:turn})});
      for(const hash of [undefined,'b'.repeat(64)]){
        event('UserPromptSubmit',hash,'rejected');event('Stop',hash,'rejected');
        check(!read('work','startup').accepted&&!read('work','startup').message);
      }
      event('UserPromptSubmit',args.prompt_sha256,'accepted');
      check(read('work','startup').accepted&&read('work','startup').turn==='accepted');
      event('Stop',args.prompt_sha256,'accepted');
      check(read('work','startup').outcome==='pending'&&read('work','startup').message==='Successful Stop message');
      handlers.CommandPaneExited([6,0,{swamp_owner:JSON.stringify(owner('work','startup')),swamp_operation:'startup'}]);
      check(read('work','startup').message==='Successful Stop message');
    }
    const value=read('work','startup');check(value.outcome==='process-exit'&&value.processExited&&value.exitCode===(immediate?17:0));
  }
  check(launches===2&&sends===0);
  seed();mock.agent.spawnDetails=opts=>{check(opts.prompt===undefined);panemap.t6={target:'Local'};return {pane_id:{Terminal:6},cwd:'/control'};};
  check((await call({method:'agent_spawn',work:'work',key:'plain',spec:{prompt:null}})).outcome==='completed');
  for(const [configured,explicit,expected] of [['Project prompt',undefined,'Project prompt'],['Project prompt','Input prompt','Input prompt'],[]]){
    seed();rows.delete(uid('work','launch'));
    mock.projectConfig.read=()=>({root:'/control',config:{workflow:'exit',rebase_prompt:configured}});
    await call({method:'workflow_run',work:'work',key:'launch',directory:'/control',inputs:{rebasePrompt:explicit}});
    check(requests.find(r=>r.operation==='workflow.run').payload.inputs.rebasePrompt===expected);
  }
  for(const [role,setting] of [["commitAgent","commit_agent"],["reviewAgent","review_agent"],["reviewFixAgent","review_fix_agent"],["rebaseAgent","rebase_agent"],["testFixAgent","test_fix_agent"]])
  for(const {config,explicit,inputs,expected} of [
    {config:{[setting]:{kind:'codex',model:'commit-model',effort:'medium'},agent:{kind:'claude',model:'review-model'}},expected:{kind:'codex',model:'commit-model',effort:'medium'}},
    {config:{[setting]:{kind:'codex'},agent:{kind:'claude',model:'review-model',effort:'high'}},expected:{kind:'codex'}},
    {config:{agent:{kind:'claude',model:'shared-model',effort:'high'}},expected:{kind:'claude',model:'shared-model',effort:'high'}},
    {config:{[setting]:{kind:'claude'}},explicit:{kind:'codex',model:'input-model'},expected:{kind:'codex',model:'input-model'}},
    {config:{},expected:{kind:'claude',command:'/stub',args:['--exit']}},
    {config:{},inputs:{kind:'claude'},expected:{kind:'claude',command:''}},
    {config:{[setting]:{kind:'codex',model:null,effort:null}},expected:{kind:'codex'}},
    {config:{[setting]:{kind:null,model:'commit-model',effort:null}},expected:{kind:'claude',model:'commit-model'}},
  ]){
    seed();rows.delete(uid('work','launch'));
    mock.projectConfig.read=()=>({root:'/control',config:{...config,workflow:'exit'}});
    await call({method:'workflow_run',work:'work',key:'launch',directory:'/control',inputs:inputs??{kind:'claude',agentCommand:'/stub',agentArgs:['--exit'],[role]:explicit}});
    check(JSON.stringify(requests.find(r=>r.operation==='workflow.run').payload.inputs[role])===JSON.stringify(expected));
  }
  seed();for(const key of ['view','launch','source'])rows.delete(uid('work',key));
  panemap.p99={plugin_url:'zellij:agent-exit',is_suppressed:false};
  await call({method:'workflow_run',work:'work',key:'launch',directory:'/control',inputs:{sourcePane:1}});
  mock.agent.spawnDetails=opts=>{check(opts.startSuppressed);panemap.t6={target:'Local',is_suppressed:true};return {pane_id:{Terminal:6},cwd:'/control'};};
  await call({method:'agent_spawn',work:'work',key:'placed',waitForExit:true,spec:{prompt:'Resolve and exit'}});
  check(read('work','view').current===6&&!panemap.t6.is_suppressed);
  check((await call({method:'workflow_show',work:'work',launch:'work',directory:'/control'})).active);
  handlers.CommandPaneExited([6,0,{swamp_owner:JSON.stringify(owner('work','placed')),swamp_operation:'placed'}]);
  check(!read('work','view').current&&!panemap.p99.is_suppressed);

  // Cancellation is idempotent across dispatch, completion, and reload. It never
  // permits new execution while ownership or termination remains uncertain.
  for (const race of ['dispatch', 'replacement', 'completion', 'reload', 'adopted', 'changed-row']) {
    seed();let signals=[];mock.workflowSignal=request=>signals.push(request);
    const work='cancel-test', key='worker';
    const value={work,key,request:JSON.stringify({method:'command',work,key}),owner:owner(work,key),
      outcome:'pending',pane:3,supervisor:'supervisor-3',process:103,incarnation:'incarnation'};
    if(race==='dispatch')delete value.supervisor;
    if(race==='replacement')value.process=999;
    if(race==='adopted')value.owner.incarnation='previous-server';
    save(work,key,value);
    const runCommand=mock.runCommand;
    if(race==='changed-row')mock.runCommand=async opts=>{
      const response=await runCommand(opts);
      save(work,key,{...read(work,key),process:104});
      return response;
    };
    await call({method:'cancel_step',work,operation:key});await flush();
    if(race==='dispatch'){check(signals.length===0);save(work,key,{...read(work,key),supervisor:'supervisor-3'});await tick();}
    mock.runCommand=runCommand;
    if(race==='replacement'||race==='changed-row'){
      check(signals.length===0&&read(work,key).outcome==='unknown');
      check((await rejected(()=>call({method:'command',work,key:'retry',argv:['no']}))).includes('Cancellation'));
      check((await rejected(()=>call({method:'step_ready',work,key:'ready'}))).includes('uncertain'));
    } else {
      check(signals.length===1&&!signals[0].kill&&signals[0].process===103);
      await call({method:'cancel_step',work,operation:key});await flush();check(signals.length===1);
      if(race==='reload'){reload();await flush();check(signals.length===1);}
      if(race==='completion')await call({method:'step_finished',work,operation:key,process:103});
      const originalNow=Date.now;Date.now=()=>originalNow()+6000;await tick();Date.now=originalNow;
      check(signals.length===(race==='completion'?1:2));
      if(signals.length===2)check(signals[1].kill);
      handlers.CommandPaneExited([3,130,{swamp_owner:JSON.stringify(value.owner),swamp_operation:key}]);
      await tick();check(read(work,key).outcome==='cancelled'&&read(work,key).terminationVerified);
      check((await call({method:'step_ready',work,key:'ready'})).outcome==='completed');
    }
  }
  done=true;
})().catch(error=>{failure=error.stack;});
