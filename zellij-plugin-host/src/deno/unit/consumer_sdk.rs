// Consumer behavior uses the real Deno SDK widgets with deterministic service replies.
fn consumer(runtime: &mut JsRuntime, name: &str) {
    let builtin = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../zellij-config/assets/plugins/{name}.ts"));
    let path = if builtin.exists() { builtin } else {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../example-plugins/{name}.ts"))
    };
    let source = std::fs::read_to_string(&path).unwrap();
    let emitted = loader::transpile(&path, &source, false).unwrap().text;
    let source = emitted.lines().filter(|line| !line.starts_with("import "))
        .collect::<Vec<_>>().join("\n")
        .replace("export default definePlugin", "globalThis.M = definePlugin")
        .replace("export const view", "const view");
    run(runtime, r#"globalThis.plain = text => text.replace(/\x1bPztext;([^\x1b]*)\x1b\\/g, (_match, payload) => {
      const body=payload.split(';').at(-1).split('$').at(-1).replace(/^[a-z]+/, '');
      return body ? zellij.decodeUtf8(new Uint8Array(body.split(',').map(Number))) : '';
    });"#);
    let tail = if name == "workflow-view" { "globalThis.M = view;" } else { "" };
    run(runtime, &format!("(function(z, definePlugin) {{ {source}\n{tail}\n}})(mock, p => p);"));
}

#[test]
fn deno_bridge_spawn_options_use_sdk_validation_and_defaults() {
    let mut runtime = sdk();
    run(&mut runtime, include_str!("bridge_fixture.js"));
    run(&mut runtime, r#"
      mock.agent=zellij.agent;
      panes[7]=true; // The test host replies to SpawnAgent with terminal 7.
    "#);
    consumer(&mut runtime, "swamp-bridge");
    run(&mut runtime, r#"
      M.load(); globalThis.done=0;
      const options=[{}, {source_directory:'/source'}, {automatic_firecracker_vm:true},
        {source_directory:'/source',automatic_firecracker_vm:false},
        {source_directory:'/source',automatic_firecracker_vm:true}];
      for(const [i,extra] of options.entries()) {
        bind({load() { (async()=>{
          const args={method:'agent_spawn',work:'options-'+i,key:'agent',spec:{kind:'codex',cwd:'/work',...extra}};
          const first=await handler(args,{source:{Plugin:1}});
          check(first.outcome==='completed' && first.pane===7 && first.supervisor==='supervisor-7');
          check((await handler(args,{source:{Plugin:1}})).pane===7);
          let rejected=false;
          try {await handler({...args,spec:{...args.spec,cwd:'/wrong'}},{source:{Plugin:1}});}
          catch(e) {check(String(e).includes('Conflicting'));rejected=true;}
          check(rejected);
          done++;
        })().catch(e=>{failure=String(e.stack)}); }});
        bridge('load',{},2);
      }
    "#);
    run(&mut runtime, "if(failure) throw Error(failure); check(done===5);");
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      const spawns=sent.map(([c])=>c.SpawnAgent).filter(Boolean);
      check(spawns.length===5); // Retries and conflicting requests never dispatch again.
      for(const [i,payload] of spawns.entries()) {
        check(JSON.stringify(payload.spec)==='{"kind":"codex"}');
        check(payload.cwd==='/work' && payload.name===null && payload.start_suppressed===false);
        check(payload.source_directory===([null,'/source',null,'/source','/source'][i]));
        check(payload.automatic_firecracker_vm===([false,false,true,false,true][i]));
        const owner=JSON.parse(payload.context.swamp_owner);
        check(owner.host_id==='host' && owner.session==='session' && owner.incarnation==='incarnation');
        check(owner.work==='options-'+i && owner.agent==='agent' && payload.context.swamp_operation==='agent');
      }
    "#);
}

#[test]
fn deno_bridge_guards_claims_callers_turns_and_reload_recovery() {
    let mut runtime = sdk();
    run(&mut runtime, include_str!("bridge_fixture.js"));
    consumer(&mut runtime, "swamp-bridge");
    run(&mut runtime, r#"
      M.load();
      globalThis.spawn = (work='one') => ({method:'agent_spawn',work,key:'agent',spec:{kind:'codex',cwd:'/work/topic'}});
      globalThis.prompt = (key,text=key) => ({method:'agent_prompt',work:'one',key,agent:'agent',prompt:text});
      globalThis.call = (args, from=1) => handler(args,{source:{Plugin:from}});
      globalThis.reject = async (promise, text) => {try {await promise;} catch(e) {if(!String(e).includes(text)) throw Error('expected '+text+', got '+e);return;} throw Error('expected rejection: '+text);};
      globalThis.results = [];
      (async () => {
        await reject(call(spawn(),99),'Only builtin');
        await reject(handler(spawn(),{source:{Cli:'forged'}}),'authenticated');
        const first=await call(spawn()); check(first.pane===7 && first.supervisor==='supervisor-7' && first.ready);
        check((await call(spawn())).pane===7 && spawned===1);
        await reject(call({...spawn(),spec:{kind:'codex',cwd:'/wrong'}}),'Conflicting');
        await call(spawn('two')); check(spawned===2);
        check((await call(prompt('fast'))).outcome==='completed' && sent===1);
        check((await call(prompt('fast'))).outcome==='completed' && sent===1);
        check((await call(prompt('slow'))).outcome==='pending');
        await reject(call(prompt('overlap')),'One outstanding');
        event(7,'Stop'); check((await call({method:'observe',work:'one',key:'slow'})).outcome==='pending');
        event(7,'UserPromptSubmit','slow',undefined,'turn','session');
        event(7,'Stop',undefined,undefined,'old','session');
        event(7,'Stop',undefined,undefined,'turn','other');
        check((await call({method:'observe',work:'one',key:'slow'})).outcome==='pending');
        event(7,'StopFailure',undefined,undefined,'turn','session');
        check((await call({method:'observe',work:'one',key:'slow'})).outcome==='failed');
        await reject(call(prompt('large','é'.repeat(131073))),'262144');
        check(await call({method:'lookup',work:'one',key:'large'})===null);
        const waiting={...prompt('exits','fast'),waitForExit:true};
        check((await call(waiting)).outcome==='pending');
        handlers.CommandPaneExited([7,0,{swamp_owner:owners[7],swamp_operation:'agent'}]);
        check((await call({method:'observe',work:'one',key:'exits'})).outcome==='process-exit');
        const uid=JSON.stringify(['host','session','one','exits']), rev=rows.get(uid).rev;
        handlers.CommandPaneExited([7,0,{swamp_owner:owners[7],swamp_operation:'agent'}]);
        check(rows.get(uid).rev===rev);
        globalThis.beforeReload=spawned;
        // Exact bytes produced by the old Lua JSON encoder, not a TS-generated fixture.
        rows.set('["host","session","legacy","agent"]',{rev:9,value:'{"incarnation":"incarnation","outcome":"unknown","request":"{\\"key\\":\\"agent\\",\\"method\\":\\"agent_spawn\\",\\"spec\\":{\\"cwd\\":\\"/work/topic\\",\\"kind\\":\\"codex\\"},\\"work\\":\\"legacy\\"}"}'});
        results.push('done');
      })().catch(e=>{globalThis.failure=String(e.stack)});
    "#);
    run(&mut runtime, "if(failure) throw Error(failure); check(results[0]==='done'); mock.instanceId='replacement';");
    consumer(&mut runtime, "swamp-bridge");
    run(&mut runtime, r#"
      M.load(); results=[];
      (async()=>{
        check((await call(spawn('legacy'))).outcome==='unknown' && spawned===beforeReload);
        check((await call(spawn())).pane===7 && spawned===beforeReload);
        conflictNext=true;
        await reject(call({method:'command',work:'conflict',key:'command',argv:['true']}),'No operation');
        check(opened===0);
        const c=await call({method:'command',work:'commands',key:'one',argv:['true'],cwd:'/work'});
        check(c.outcome==='pending' && opened===1);
        results.push('done');
      })().catch(e=>{failure=String(e.stack)});
    "#);
    run(&mut runtime, "if(failure) throw Error(failure); check(results[0]==='done'); mock.instanceId='third';");
    consumer(&mut runtime, "swamp-bridge");
    run(&mut runtime, r#"
      M.load(); results=[];
      (async()=>{
        check((await call({method:'observe',work:'commands',key:'one',includeOutput:false})).outcome==='unknown');
        check((await call({method:'command',work:'commands',key:'one',argv:['true'],cwd:'/work'})).outcome==='unknown' && opened===1);
        results.push('done');
      })().catch(e=>{failure=String(e.stack)});
    "#);
    run(&mut runtime, "if(failure) throw Error(failure); check(results[0]==='done');");
}

#[test]
fn deno_workflow_view_preserves_question_races_focus_previews_and_scrolling() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      globalThis.markdowns=[]; globalThis.polls=[]; globalThis.commands=[]; globalThis.closed=0; globalThis.renders=0;
      globalThis.mock={...zellij, ui:{...zellij.ui,markdown(source,options){markdowns.push({source,options});return zellij.ui.markdown(source,options);}}, serverBinary:'zellij', subscribe(){}, every(){}, requestRender(){renders++},
        command(name){if(name==='CloseSelf') closed++;},
        rpc:{call(){return new Promise(resolve=>polls.push(resolve));}},
        runCommand(options){return new Promise(resolve=>commands.push({options,resolve}));}};
      globalThis.q=(question,version=1,answers=['return'],state='pending',run='run')=>({model:'model',version,attributes:{run,question,kind:answers.length?'choice':'text',state,answers,prompt:'Question '+question}});
      globalThis.snapshot=questions=>({run:'run',root:"/work/it's here",history:{id:'run',workflowName:'test',status:'suspended'},questions:{results:questions},outputs:{results:[]}});
      globalThis.key=name=>M.update({Key:{bare_key:name,key_modifiers:[]}});
      globalThis.focus=id=>{for(let n=0;n<20;n++){if(M.focusedControl()===id)return;key('Tab');}throw Error('missing focus '+id+' got '+M.focusedControl());};
      globalThis.resolve=questions=>{check(polls.length===1);polls.shift()(snapshot(questions));};
      globalThis.finish=(index,exitCode=0,stderr='')=>commands[index].resolve({exitCode,stdout:new Uint8Array(),stderr:zellij.encodeUtf8(stderr)});
    "#);
    consumer(&mut runtime, "workflow-view");
    run(&mut runtime, "M.load(); M.attach({directory:'/work',run:'run'}); resolve([q('one')]);");
    run(&mut runtime, r#"
      check(M.pending.length===1 && M.focusedControl()==='choice_1');
      M.render(20,100); key('Enter'); key('Enter');
      check(commands.length===1 && commands[0].options.command[6]==='return' && M.submitting);
      finish(0);
    "#);
    run(&mut runtime, "resolve([q('one')]);");
    run(&mut runtime, "check(M.pending.length===0); M.refresh(); resolve([q('one',2,[])]);");
    run(&mut runtime, r#"
      check(M.focusedControl()==='answer'); M.update({PastedText:'kept draft'});
      key('Enter'); check(commands.length===1); // Text-input Enter does not submit.
      M.refresh(); resolve([q('one',2,[])]);
    "#);
    run(&mut runtime, r#"
      M.refresh(); focus('submit'); key('Enter');
      check(commands.length===2 && commands[1].options.command[6]==='kept draft');
      resolve([q('old')]); // A poll started before submission must not replace the question.
    "#);
    run(&mut runtime, "check(M.pending[0].attributes.question==='one'); finish(1);");
    run(&mut runtime, "resolve([q('one',2,[]),q('one',3,[],'answered'),q('one',1,['yes'],'pending','child')]);");
    run(&mut runtime, r#"
      check(M.pending.length===1 && M.pending[0].attributes.run==='child');
      for(const [rows,cols] of [[0,0],[1,1],[2,8],[8,24]]) M.render(rows,cols);
      M.render(20,100); check(!M.update({Mouse:{LeftClick:[0,0]}}));
      M.refresh(); resolve([q('one',2,['yes'],'pending','child')]);
    "#);
    run(&mut runtime, r#"
      check(M.selecting && M.dirty); check(!M.update({Mouse:{Hold:[19,0]}}));
      check(!M.update({Mouse:{Release:[19,0]}})); check(!M.selecting && commands.length===2);
      M.render(20,100); const y=M.formY;
      M.update({Mouse:{LeftClick:[y,2]}}); M.update({Mouse:{Release:[y,2]}});
      check(commands.length===3 && commands[2].options.command[6]==='yes'); finish(2,1);
    "#);
    run(&mut runtime, "resolve([q('one',2,['yes'],'pending','child')]);");
    run(&mut runtime, r#"
      check(M.notice==='Command failed'); focus('cancel_run'); key('Enter'); key('Enter');
      check(commands.length===4 && commands[3].options.command[2]==='cancel'); finish(3);
    "#);
    run(&mut runtime, "resolve([q('preview')]);");
    run(&mut runtime, r#"
      M.snapshot.outputs.results=[{model:'git-model',name:'diff-current',version:7,attributes:{output:'😀é preview',previewTruncated:true,test:{outcome:'failed',exitCode:1,log:'terminal_7',output:'nested failure',previewTruncated:true}}}];
      const rendered=plain(M.render(60,400));
      check(rendered.includes('Output preview') && rendered.includes('attributes.test.output') && rendered.includes('nested failure'));
      check(rendered.includes('version == 7') && rendered.includes('git-model'));
      check(markdowns.some(m=>m.source==='nested failure' && m.options.dim===true));
      check(markdowns.some(m=>m.source==='Question preview' && m.options.dim===false));
      // A choice question has a choice button, never a free-text Submit button.
      focus('choice_1'); key('PageDown'); key('Enter');
      check(commands.length===5 && commands[4].options.command[6]==='return');
      key('Esc'); check(closed===1);
    "#);
}

#[test]
fn deno_workflow_view_keeps_restart_explicit_and_all_details_reachable() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      globalThis.restarted=0;globalThis.closed=0;
      globalThis.mock={...zellij,subscribe(){},every(){},command(name){if(name==='CloseSelf')closed++;}};
    "#);
    consumer(&mut runtime, "workflow-view");
    run(&mut runtime, r#"
      M.active=true;M.load({onRestart(){restarted++}});
      const key=bare_key=>M.update({Key:{bare_key,key_modifiers:[]}});
      for(let i=0;i<6;i++){check(M.focusedControl()!=='restart');key('Tab');}
      while(M.focusedControl()!=='detach')key('Tab');
      M.active=false;check(M.focusedControl()==='detach');key('Enter');check(closed===1 && restarted===0);
      while(M.focusedControl()!=='restart')key('Tab');
      M.submitting=true;key('Enter');check(restarted===0);M.submitting=false;key('Enter');check(restarted===1);
      M.pending=[{model:'m',version:1,attributes:{run:'r',question:'q',kind:'text',state:'pending',answers:[],prompt:'long '.repeat(100)+'QUESTION-END'}}];
      M.snapshot={root:'/work',history:{status:'failed'},logs:'history '.repeat(100)+'HISTORY-END'};
      let rendered='';
      for(let i=0;i<100;i++){rendered+=plain(M.render(8,24));key('PageDown');}
      check(rendered.includes('QUESTION-END') && rendered.includes('HISTORY-END'));
    "#);
}

#[test]
fn typescript_pane_sequence_reuses_scrollback_and_escalates_cancellation() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      globalThis.mock=zellij;globalThis.now=100;Date.now=()=>now;
    "#);
    consumer(&mut runtime, "workflow");
    run(&mut runtime, "bind(M);bridge('load',{skill:'deploy'},2);bridge('update',{Key:{bare_key:{Char:'r'}}},2)");
    for _ in 0..3 {
        pane_context(&mut runtime);
        run(&mut runtime, "event('CommandPaneOpened');event('CommandPaneExited')");
    }
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      check(M.render().includes('sequence completed; exit codes: [0,0,0]'));
      check(sent.filter(([c])=>c.OpenCommandPaneFloating).length===1);
      check(sent.filter(([c])=>c.RunCommandInPane).every(([c])=>c.RunCommandInPane.keep_scrollback && c.RunCommandInPane.pane_id.Terminal===7));
      bridge('update',{Key:{bare_key:{Char:'r'}}},2);
      bridge('update',{Key:{bare_key:{Char:'c'}}},2);
      now+=2000;bridge('update',{Timer:0},2);
    "#);
    pane_context(&mut runtime);
    run(&mut runtime, "check(sent.some(([c])=>c.SendSigkillToPaneId));event('CommandPaneExited',1)");
    run(&mut runtime, "check(M.render().includes('sequence cancelled'));now+=4000;bridge('update',{Timer:0},2)");
}
