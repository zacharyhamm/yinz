globalThis.rpcHandlers={};globalThis.requests=[];globalThis.done=false;
mock.rpc.handle=(name,callback)=>{rpcHandlers[name]=callback;};
mock.projectConfig={read:directory=>{check(directory==='/project'||directory==='/control');return {root:'/control',config:{}};}};
const runId='11111111-1111-4111-8111-111111111111';
const workflowId='22222222-2222-4222-8222-222222222222';
let resolveError=false;
let rpcSequence=0, passed=true, denied=false, status='failed', submitted=0, renamed=false;
const oldCommand=mock.command;
mock.command=(name,args)=>name==='GetPaneInfo'&&args.Plugin===2?{plugin_url:'zellij:hocus'}:oldCommand(name,args);
mock.swampRequest=async opts=>{
  check(opts.directory==='/control');requests.push(opts);
  opts.onProgress?.({status:'ready'});
  if(denied)return {error:{code:'transport_unknown',message:'Lost response'}};
  switch(opts.operation) {
    case 'workflow.schema': return {workflow:{type:'object'}};
    case 'workflow.search': return {query:opts.payload.query,source:'server',results:[{id:workflowId,name:'Pipeline',description:'Release checks'},{id:'other',name:'unrelated'}]};
    case 'model.search': return {query:opts.payload.query,source:'server',results:[{id:workflowId,name:'Worker',type:'@test/Runner'},{id:'other',name:'unrelated',type:'other'}]};
    case 'workflow.get': if(resolveError)return {error:{code:'not_found',message:'Missing workflow'}};return {id:workflowId,name:'test',version:1,jobs:[],source:{editable:true}};
    case 'workflow.create': return {id:workflowId};
    case 'workflow.validate':
      if(renamed)check(opts.payload.workflowIdOrName===workflowId);
      return {passed,checks:['schema']};
    case 'workflow.history.get': return {id:runId,workflowId,workflowName:'test',status,inputs:{launch:'application value with spaces',session:'session'},jobs:[]};
    case 'workflow.history.search':
      if(!opts.payload.query)return {results:[]};
      return {query:opts.payload.query,source:'server',results:[{runId,workflowName:'test',status:'failed',inputs:{branch:'Release',enabled:false}},{runId:'other',workflowName:'test',status:'failed',inputs:{branch:'main'}}]};
    case 'report.search': case 'report.get': return {workflow:opts.payload.workflow};
    case 'workflow.history.logs': return {log:{lines:['other','main/step failed','error detail']}};
    case 'workflow.resume': return {runId};
    case 'workflow.run': return {runId};
    case 'workflow.cancel': return {cancelled:runId};
    case 'model.get': return {type:'@hammz/question'};
    case 'data.query': if(opts.payload.limit===1)return {limited:true,projected:{rows:[{version:1}]}};return opts.payload.predicate.includes('question')?{results:[{attributes:{run:runId,question:'q'}}]}:
      {results:opts.payload.predicate.includes('version >= 0')&&opts.payload.predicate.includes('tags.workflowRunId == '+JSON.stringify(runId))?[{version:1,attributes:{output:'retained earlier run'}}]:[]};
    case 'model.method.run': submitted++;return {run:{status:'succeeded'}};
    default:return {};
  }
};
const flush=async()=>{for(let i=0;i<100;i++)await Promise.resolve();};
const call=(tool,args={},from=2)=>rpcHandlers.hocus_workflow({tool,args,directory:'/project'}, {source:{Plugin:from},callId:'request-'+(++rpcSequence),progress(){}});
(async()=>{
  M.load();
  for(const from of [1,99]){let error;try{await call('workflow_active',{},from);}catch(e){error=e;}check(error&&requests.length===0);}
  check((await call('command',{argv:['sh']})).error);
  check((await call('workflow_cancel',{run:'latest'})).error);
  check(!requests.length);
  const created=await call('workflow_create',{name:'test'});
  check(created.definition.id===workflowId&&requests[0].operation==='workflow.schema');
  requests=[];
  check((await call('workflow_edit',{workflow:'test',expected:created.definition,definition:{id:'different'}})).error);
  check(!requests.some(r=>r.operation==='workflow.edit'));
  // Search filtering uses the CLI's substring fields and keeps server metadata.
  for(const [tool,queries] of [['workflow_search',['pIpElInE','rElEaSe',workflowId.toUpperCase()]],['workflow_models',['wOrKeR','@TEST/runner',workflowId.toUpperCase()]],['workflow_history',['tEsT','fAiLeD',runId.toUpperCase(),'bRaNcH=rElEaSe','ENABLED=false']]]) {
    for(const query of queries) {
      const result=await call(tool,{query});
      check(result.source==='server'&&result.query===query&&result.results[0]);
      check(result.results[0].id===workflowId||result.results[0].runId===runId);
    }
    check((await call(tool,{query:'no-such-match'})).results.length===0);
  }
  for(const workflow of [workflowId,'TeSt'])for(const tool of ['workflow_history','workflow_reports','workflow_report']) {
    requests=[];
    const args={workflow,query:'branch=RELEASE',filter:'status == "failed"',name:'@swamp/workflow-summary'};
    const result=await call(tool,args);
    check(!result.error);
    const operation=tool==='workflow_history'?'workflow.history.search':tool==='workflow_reports'?'report.search':'report.get';
    const sent=requests.find(r=>r.operation===operation);
    check(sent.payload.workflow===(workflow===workflowId?'test':'TeSt'));
    check(requests.filter(r=>r.operation==='workflow.get').length===(workflow===workflowId?1:0));
    if(tool==='workflow_history')check(sent.payload.filter===args.filter&&sent.payload.query===args.query&&result.results.length===1);
  }
  resolveError=true;
  for(const tool of ['workflow_history','workflow_reports','workflow_report']) {
    requests=[];
    check((await call(tool,{workflow:workflowId,name:'@swamp/workflow-summary'})).error.code==='not_found');
    check(requests.length===1&&requests[0].operation==='workflow.get');
  }
  resolveError=false;
  const earlier=await call('workflow_run_get',{run:runId});
  check(earlier.outputs.results[0].version===1&&earlier.outputs.results[0].attributes.output==='retained earlier run');
  passed=false;requests=[];
  check((await call('workflow_launch',{workflow:'test'})).error);
  check(!requests.some(r=>r.operation==='workflow.run'));
  check((await call('workflow_resume',{run:runId,inputs:{answer:'fixed'},from:'step'})).error.code==='validation_failed');
  check(!requests.some(r=>r.operation==='workflow.resume'));
  check((await call('workflow_answer',{run:runId,model:'question-model',question:'q',answer:'yes'})).error.code==='validation_failed'&&submitted===0);
  passed=true;requests=[];
  // History retains the original name after the definition has been renamed.
  renamed=true;
  const resumed=await call('workflow_resume',{run:runId,inputs:{answer:'fixed'},from:'step'});
  check(resumed.run===runId);
  const dispatch=requests.find(r=>r.operation==='workflow.resume');
  check(typeof dispatch.onProgress==='function');
  check(dispatch.payload.workflowIdOrName===workflowId&&dispatch.payload.runId===runId&&dispatch.payload.from==='step'&&dispatch.payload.inputs.answer==='fixed');
  check(requests.findIndex(r=>r.operation==='workflow.validate')<requests.indexOf(dispatch));
  const data=await call('workflow_data',{predicate:'version >= 0',limit:1,select:'version'});
  check(data.limited&&data.results[0].version===1);
  const logs=await call('workflow_logs',{run:runId,tail:10,contains:'main/step'});
  check(logs.logs==='main/step failed');
  check((await call('workflow_cancel_step',{run:runId,work:'work',key:'worker'})).error);
  status='running';
  check((await call('workflow_cancel',{run:runId})).cancelled===runId);
  for(const inactive of ['suspended','failed','succeeded','cancelled']) {
    status=inactive;requests=[];
    check((await call('workflow_cancel',{run:runId})).error);
    check(!requests.some(r=>r.operation==='workflow.cancel'));
  }
  status='suspended';
  await call('workflow_approve',{run:runId,step:'approval'});
  await call('workflow_reject',{run:runId,step:'approval',reason:'no'});
  for(const action of ['workflow.approve','workflow.reject'])check(requests.some(r=>r.operation===action&&r.payload.runId===runId&&r.payload.workflowIdOrName===workflowId));
  await call('workflow_answer',{run:runId,model:'question-model',question:'q',answer:'yes'});
  check(submitted===1&&requests.some(r=>r.operation==='model.method.run'&&r.payload.inputs.run===runId));
  renamed=false;
  denied=true;requests=[];
  const failed=await call('workflow_validate',{workflow:'test'});
  check(failed.error.code==='transport_unknown'&&failed.reports[0].name==='@swamp/workflow-summary'&&requests.length===1);
  denied=false;
  const launched=await call('workflow_launch',{workflow:'test',inputs:{message:'hello'}});
  check(launched.run===runId&&requests.some(r=>r.operation==='workflow.run'&&r.payload.inputs.message==='hello'));
  check(JSON.stringify(requests.find(r=>r.operation==='workflow.run').payload.inputs)==='{"message":"hello"}');
  await call('workflow_run_get',{run:runId});
  check(requests.some(r=>r.operation==='workflow.history.search'&&r.payload.filter?.includes('tags.zellijOperation')));
  check(!(await call('workflow_resume',{run:runId,inputs:{session:'a workflow input'}})).error);
  // The step API reuses owned process cancellation and checks its lifecycle before dispatch.
  const save=(work,key,value)=>rows.set(JSON.stringify(['host','session',work,key]),{value:JSON.stringify(value),rev:1});
  save(runId,'attachment',{run:runId,work:'work',root:'/control',workflow:'test'});
  save('work','view',{closed:true});
  for(const tool of ['workflow_resume','workflow_approve','workflow_reject','workflow_answer','workflow_cancel','workflow_cancel_step']) {
    const r=await call(tool,{run:runId,step:'approval',work:'work',key:'worker',model:'question-model',question:'q',answer:'yes'});
    check(r.error&&r.error.message.includes('closed'));
  }
  save('work','view',{closed:false});
  save('work','worker',{work:'work',request:JSON.stringify({method:'command'}),outcome:'pending',owner:{host_id:'wrong',session:'session',work:'work'}});
  check((await call('workflow_cancel_step',{run:runId,work:'work',key:'worker'})).error);
  save('work','worker',{work:'work',request:JSON.stringify({method:'command'}),outcome:'pending',owner:{host_id:'host',session:'session',work:'work'}});
  const cancelled=await call('workflow_cancel_step',{run:runId,work:'work',key:'worker'});
  check(cancelled.cancelRequested===true);

  // Exercise real bridge attachments in both orders, with the canonical name
  // returned for a UUID launch. History discovery includes a later child.
  const childId='33333333-3333-4333-8333-333333333333';
  const grandchildId='44444444-4444-4444-8444-444444444444';
  const siblingId='66666666-6666-4666-8666-666666666666';
  const unrelatedId='55555555-5555-4555-8555-555555555555';
  const uid=(work,key)=>JSON.stringify(['host','session',work,key]);
  const read=(work,key)=>JSON.parse(rows.get(uid(work,key)).value);
  const bridge=args=>rpcHandlers.swamp(args,{source:{Plugin:1},progress(){}});
  for(const workflowFirst of [true,false]) {
    rows.clear();requests=[];
    const launch=workflowFirst?'workflow-first':'bridge-first';
    const inputs={launch,session:'session',message:'unchanged',options:{enabled:false},optional:null};
    let childVisible=false,dispatches=0,runStatus='running';
    const attach=()=>bridge({method:'workflow_attach',work:launch,key:'attachment',run:runId,workflow:'canonical',directory:'/control'});
    mock.swampRequest=async opts=>{
      requests.push(opts);
      switch(opts.operation) {
        case 'workflow.get': check(opts.payload.workflowIdOrName===workflowId);return {id:workflowId,name:'canonical'};
        case 'workflow.validate': return {passed:true};
        case 'workflow.run':
          dispatches++;
          check(opts.payload.workflowIdOrName==='canonical');
          check(JSON.stringify(opts.payload.inputs)===JSON.stringify(inputs));
          check(opts.payload.runtimeTags.zellijOperation===uid(launch,'launch'));
          if(workflowFirst)await attach();
          return {runId};
        case 'workflow.history.search':
          check(opts.payload.filter==='has(tags.zellijOperation) && tags.zellijOperation == '+JSON.stringify(uid(launch,'launch')));
          // Swamp applies this exact ownership predicate, excluding a containing tag.
          return {results:[{runId,workflowName:'canonical',tag:uid(launch,'launch')},
            ...(childVisible?[{runId:childId,workflowName:'child',tag:uid(launch,'launch')},{runId:grandchildId,workflowName:'grandchild',tag:uid(launch,'launch')},{runId:siblingId,workflowName:'sibling',tag:uid(launch,'launch')}]:[]),
            {runId:unrelatedId,workflowName:'unrelated',tag:'prefix'+uid(launch,'launch')}].filter(r=>r.tag===uid(launch,'launch')&&(!opts.payload.workflow||r.workflowName===opts.payload.workflow))};
        case 'workflow.history.get':
          return {id:opts.payload.workflowIdOrName,workflowId,workflowName:opts.payload.workflowIdOrName===runId?'canonical':'child',status:runStatus,inputs};
        case 'data.query':
          if(!opts.payload.predicate.includes('question')) {
            check(opts.payload.predicate.startsWith('version >= 0 && has(tags.workflowRunId) && tags.workflowRunId in '));
            const ids=JSON.parse(opts.payload.predicate.split(' in ')[1]);
            check(!ids.includes(unrelatedId)&&ids.includes(runId));
            return {results:[{version:1,attributes:{output:'retained earlier run'}},...(childVisible?[{version:2,attributes:{run:grandchildId,output:'grandchild-output'}}]:[])]};
          }
          return {results:childVisible&&opts.payload.predicate.includes(childId)?[{attributes:{run:childId,question:'child-question'}}]:[]};
        case 'workflow.history.logs': return {log:{lines:[]}};
        case 'workflow.cancel': return {cancelled:opts.payload.runId};
        case 'workflow.resume': return {runId};
        default: return {};
      }
    };
    const launched=await call('workflow_launch',{workflow:workflowId,inputs});
    check(!launched.error&&launched.launch===launch&&launched.workflow==='canonical');
    if(!workflowFirst)await attach();
    check(read(runId,'attachment').work===launch&&read(runId,'attachment').workflow==='canonical');
    check(read(launch,'launch').workflow==='canonical');
    const first=await call('workflow_run_get',{run:runId});
    check(!first.error&&first.children.length===0&&first.outputs.results[0].attributes.output==='retained earlier run');
    childVisible=true;
    const second=await call('workflow_run_get',{run:runId});
    check(!second.error&&second.children.length===3&&second.children.some(c=>c.id===childId)&&second.children.some(c=>c.id===grandchildId));
    check(second.outputs.results[1].attributes.output==='grandchild-output'&&second.questions.results[0].attributes.question==='child-question');
    // Both RPC routes must cancel only the supplied UUID, even when inspection
    // includes the parent, grandchild and sibling under the same launch.
    for(const direct of [false,true])for(const run of [childId,runId]) {
      requests=[];
      const cancelled=direct?await bridge({method:'workflow_cancel',directory:'/control',run}):await call('workflow_cancel',{run});
      check(!cancelled.error&&JSON.stringify(cancelled.operations)===JSON.stringify([{cancelled:run}]));
      check(JSON.stringify(requests.filter(r=>r.operation==='workflow.cancel').map(r=>r.payload.runId))===JSON.stringify([run]));
    }
    for(const inactive of ['suspended','failed','succeeded','cancelled']) {
      runStatus=inactive;requests=[];
      check((await bridge({method:'workflow_cancel',directory:'/control',run:childId})).operations.length===0);
      check(!requests.some(r=>r.operation==='workflow.cancel'));
    }
    runStatus='running';
    const attachment=read(childId,'attachment');
    save(childId,'attachment',{...attachment,root:'/other-checkout'});requests=[];
    check((await call('workflow_cancel',{run:childId})).error.message.includes('different control checkout'));
    check(!requests.some(r=>r.operation==='workflow.cancel'));
    save(childId,'attachment',attachment);
    check(!requests.some(r=>r.payload?.workflowIdOrName===unrelatedId||r.payload?.runId===unrelatedId)&&!rows.has(uid(unrelatedId,'attachment')));
    // Reconciliation identifies the parent even though all descendants share its tag.
    const unknown=read(launch,'launch');delete unknown.run;unknown.outcome='unknown';save(launch,'launch',unknown);
    const recovered=await bridge({method:'observe',work:launch,key:'launch'});
    check(recovered.run===runId&&requests.some(r=>r.operation==='workflow.history.search'&&r.payload.workflow==='canonical'));
    // The same request reuses its claim; conflicting requests never dispatch.
    check(!(await call('workflow_launch',{workflow:workflowId,inputs})).error&&dispatches===1);
    check((await call('workflow_launch',{workflow:workflowId,inputs:{...inputs,message:'different'}})).error&&dispatches===1);
    const resumeCount=()=>requests.filter(r=>r.operation==='workflow.resume').length;
    for(const field of ['launch','session'])for(const value of ['changed',null]) {
      const rejected=await call('workflow_resume',{run:runId,inputs:{[field]:value}});
      check(rejected.error&&rejected.error.message.includes('Cannot change owned '+field)&&resumeCount()===0);
    }
    check(!(await call('workflow_resume',{run:runId,inputs:{...inputs,message:'repaired'}})).error&&resumeCount()===1);
  }
  // Ordinary application inputs, including launch/session fields, remain exact.
  for(const inputs of [{launch:'application value with spaces',session:'session'}, {launch:'valid-token',session:'elsewhere'}, {launch:'valid-token'}, {}]) {
    rows.clear();requests=[];
    mock.swampRequest=async opts=>{
      requests.push(opts);
      if(opts.operation==='workflow.get')return {id:workflowId,name:'canonical'};
      if(opts.operation==='workflow.validate')return {passed:true};
      if(opts.operation==='workflow.run')return {runId};
      if(opts.operation==='workflow.history.get')return {id:runId,workflowId,workflowName:'canonical',inputs,status:'failed'};
      return {};
    };
    const launched=await call('workflow_launch',{workflow:workflowId,inputs});
    check(!launched.error&&launched.launch.startsWith('instance-hocus-'));
    check(JSON.stringify(requests.find(r=>r.operation==='workflow.run').payload.inputs)===JSON.stringify(inputs));
    check(!(await call('workflow_resume',{run:runId,inputs:{launch:null,session:'changed'}})).error);
  }
  done=true;
})().catch(e=>failure=e.stack);
