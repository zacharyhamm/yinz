fn pane_context(runtime: &mut JsRuntime) {
    capture_commands(runtime);
    run(runtime, r#"
      globalThis.ctx = sent.map(([c]) => c.RunCommandInPane?.context ?? c.OpenCommandPane?.[1] ?? c.OpenCommandPaneFloating?.[2]).filter(Boolean).at(-1);
      globalThis.event = (type, code=0, context=ctx, id=7) => bridge('update', {[type]: type==='PaneClosed'?{Terminal:id}:type==='CommandPaneExited'?[id,code,context]:[id,context]}, 2);
    "#);
}

#[test]
fn pane_sequence_reuses_pane_matches_fast_and_stale_events_and_appends() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      globalThis.result=null;globalThis.calls=[];
      bind({load(){globalThis.seq=zellij.paneSequence({steps:[{path:'true'},{path:'echo',args:['two'],stdin:'input',timeoutMs:10}],
        onStep(n,s,code){calls.push([n,s.path,code]);if(n===2)seq.push({path:'true'});throw Error('callback');},
        onDone(){throw Error('done callback');}});seq.done.then(r=>result=r);}});bridge('load',{},2);
      check(seq.paneId.Terminal===7 && seq.status==='running');
    "#);
    pane_context(&mut runtime);
    run(&mut runtime, "globalThis.old=ctx; event('CommandPaneExited',0,ctx,8);event('CommandPaneOpened');event('CommandPaneExited');");
    pane_context(&mut runtime);
    run(&mut runtime, r#"
      check(sent.filter(([c])=>c.OpenCommandPane).length===1);
      check(sent.at(-1)[0].RunCommandInPane.keep_scrollback);
      check(!sent.some(([c])=>c.WriteCharsToPaneId));
      event('CommandPaneReRun',0,old); event('CommandPaneExited',7,old);
      check(calls.length===1);event('CommandPaneReRun');event('CommandPaneReRun');event('CommandPaneExited');
    "#);
    pane_context(&mut runtime);
    run(&mut runtime, "check(sent.filter(([c])=>c.WriteCharsToPaneId).length===1);event('CommandPaneExited');");
    run(&mut runtime, r#"
      check(result.status==='success' && result.exitCodes.join()==='0,0,0' && calls.length===3);
      seq.push(null);seq.cancel();event('CommandPaneExited');check(calls.length===3);
      bind({load(){zellij.unsubscribe(['CommandPaneOpened','CommandPaneReRun','CommandPaneExited','PaneClosed','Timer']);}});bridge('load',{},2);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, "check(sent.at(-1)[0].Unsubscribe.length===5)");
}

#[test]
fn pane_sequence_validation_failure_policies_and_observation_cancellation() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      globalThis.errors=[];globalThis.results=[];
      bind({load(){
        for(const options of [{steps:[]},{steps:[, {path:'true'}]},{steps:[{path:'x',args:[,]}]},{steps:[{path:'true'},{path:42}]},{steps:[{path:'x',args:[1]}]},{steps:[{path:'x',stdin:42}]},{steps:[{path:'x',timeoutMs:-1}]},{steps:[{path:'x'}],killGraceMs:Infinity},{steps:[{path:'x'}],floating:1},{steps:[{path:'x'}],onDone:1}]) {
          try{zellij.paneSequence(options);}catch(e){errors.push(e);}
        }
      }});bridge('load',{},2);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, "check(errors.length===10 && sent.length===0)");
    for stop in [true, false] {
        run(&mut runtime, &format!(r#"
          bind({{load(){{globalThis.seq=zellij.paneSequence({{steps:[{{path:'false'}},{{path:'true'}}],stopOnFailure:{stop}}});seq.done.then(r=>results.push(r));
          try{{seq.push({{path:'true',timeoutMs:'bad'}});}}catch(e){{errors.push(e);}}}}}});bridge('load',{{}},2);
        "#));
        pane_context(&mut runtime);
        run(&mut runtime, "event('CommandPaneExited',3)");
        if !stop {
            pane_context(&mut runtime);
            run(&mut runtime, "event('CommandPaneExited',0)");
        }
        run(&mut runtime, "check(results.at(-1).status==='failed' && results.at(-1).failedStep===1)");
    }
    runtime.op_state().borrow_mut().borrow_mut::<TestHost>().pane_info = json!({"id":7,"is_plugin":false,"exited":false});
    run(&mut runtime, r#"
      bind({load(){globalThis.wait=zellij.waitForPaneExit(7);wait.catch(e=>errors.push(e.name));wait.cancel();
        globalThis.primitive=zellij.runInPane(7,{path:'cat',stdin:'hello'});primitive.catch(e=>errors.push(e.name));primitive.cancel();
        globalThis.closed=zellij.waitForPaneExit(7);closed.then(r=>results.push(r));}});bridge('load',{},2);
    "#);
    pane_context(&mut runtime);
    run(&mut runtime, "event('CommandPaneReRun');event('PaneClosed')");
    capture_commands(&mut runtime);
    run(&mut runtime, "check(results.at(-1).closed && errors.slice(-2).every(e=>e==='AbortError'));check(!sent.some(([c])=>c.SendSigintToPaneId || c.WriteCharsToPaneId))");
}

#[test]
fn pane_sequence_cwd_preserves_targets_and_resolves_only_at_dispatch() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      bind({load(){globalThis.seq=zellij.paneSequence({steps:[{path:'true'},
        {path:'pwd',cwd:'inherit',target:{Ssh:{host:'host'}},remote_host:'legacy',remote_default_shell:false},
        {path:'pwd',cwd:'inherit',target:{Firecracker:{vm:'vm',image:'image',worktree:'/work'}},firecracker_vm:'vm',firecracker_image:'image',firecracker_worktree:'/work'}]});seq.done.catch(()=>{});}});bridge('load',{},2);
    "#);
    runtime.op_state().borrow_mut().borrow_mut::<TestHost>().pane_info = json!({"cwd":"/latest","target":{"Ssh":{"host":"host"}}});
    pane_context(&mut runtime);
    run(&mut runtime, "event('CommandPaneExited')");
    pane_context(&mut runtime);
    run(&mut runtime, "check(sent.at(-1)[0].RunCommandInPane.command.cwd==='/latest');check(sent.at(-1)[0].RunCommandInPane.command.remote_host==='legacy');event('CommandPaneExited')");
    pane_context(&mut runtime);
    run(&mut runtime, r#"
      const cmd=sent.at(-1)[0].RunCommandInPane.command;
      check(cmd.cwd===null && cmd.target.Firecracker.vm==='vm' && cmd.firecracker_image==='image' && cmd.firecracker_worktree==='/work');
      event('PaneClosed');check(seq.status==='pane_closed');
    "#);
}

#[test]
fn pane_sequence_timeout_cancel_grace_refusal_and_cleanup() {
    let mut runtime = sdk();
    run(&mut runtime, "globalThis.now=100;Date.now=()=>now;globalThis.results=[];globalThis.errors=[];");
    for timeout in [false, true] {
        run(&mut runtime, &format!(r#"
          bind({{load(){{globalThis.seq=zellij.paneSequence({{steps:[{{path:'cat',timeoutMs:10}}],killGraceMs:20}});seq.done.then(r=>results.push(r),e=>errors.push(e.message));}}}});bridge('load',{{}},2);
        "#));
        pane_context(&mut runtime);
        run(&mut runtime, "now+=100;bridge('update',{Timer:0},2);check(seq.status==='running');event('CommandPaneOpened');");
        if timeout {
            run(&mut runtime, "now+=10;bridge('update',{Timer:0},2);");
        } else {
            runtime.op_state().borrow_mut().borrow_mut::<TestHost>().fail_command = Some("SendSigintToPaneId");
            run(&mut runtime, "bind({load(){try{seq.cancel();}catch(e){check(seq.status==='running');}}});bridge('load',{},2)");
            runtime.op_state().borrow_mut().borrow_mut::<TestHost>().fail_command = None;
            run(&mut runtime, "bind({load(){seq.cancel();seq.cancel();seq.push(null);}});bridge('load',{},2);check(seq.status==='cancelling')");
        }
        run(&mut runtime, "now+=20;bridge('update',{Timer:0},2)");
        capture_commands(&mut runtime);
        run(&mut runtime, "check(sent.some(([c])=>c.SendSigkillToPaneId?.Terminal===7));now+=20;bridge('update',{Timer:0},2)");
        run(&mut runtime, &format!("check(results.at(-1).status==='{}' && results.at(-1).exitCodes[0]===null)",if timeout {"failed"} else {"cancelled"}));
        run(&mut runtime, "event('CommandPaneExited');now+=100;bridge('update',{Timer:0},2)");
    }
    for command in ["Subscribe", "OpenCommandPane", "RunCommandInPane", "WriteCharsToPaneId", "SetTimeout"] {
        runtime.op_state().borrow_mut().borrow_mut::<TestHost>().fail_command = if command == "RunCommandInPane" || command == "WriteCharsToPaneId" || command == "SetTimeout" {None} else {Some(command)};
        run(&mut runtime, r#"
          bind({load(){globalThis.seq=zellij.paneSequence({steps:[{path:'cat',stdin:'hello',timeoutMs:10},{path:'true'}]});seq.done.catch(e=>errors.push(e.message));}});bridge('load',{},2);
        "#);
        runtime.op_state().borrow_mut().borrow_mut::<TestHost>().fail_command = Some(command);
        pane_context(&mut runtime);
        run(&mut runtime, if command == "RunCommandInPane" {"event('CommandPaneExited')"} else {"event('CommandPaneOpened')"});
        run(&mut runtime, &format!("check(errors.at(-1)==='{command} dispatch failed' && seq.status==='failed')"));
        runtime.op_state().borrow_mut().borrow_mut::<TestHost>().fail_command = None;
        run(&mut runtime, "bind({load(){zellij.unsubscribe(['CommandPaneOpened','CommandPaneReRun','CommandPaneExited','PaneClosed','Timer']);}});bridge('load',{},2)");
        capture_commands(&mut runtime);
        run(&mut runtime, "check(sent.at(-1)[0].Unsubscribe.length===5)");
    }
}

#[test]
fn pane_sequence_close_policies_and_exits_cancel_remaining_signals() {
    let mut runtime = sdk();
    run(&mut runtime, "globalThis.now=0;Date.now=()=>now;globalThis.results=[];");
    for policy in ["success", "cancelled", "pane_closed"] {
        run(&mut runtime, r#"
          bind({load(){globalThis.seq=zellij.paneSequence({name:'named',floating:{width:{Percent:50n}},closeOnSuccess:true,onCancel:'close',
            steps:[{path:'cat',stdin:'ready',timeoutMs:50}],killGraceMs:10});seq.done.then(r=>results.push(r));}});bridge('load',{},2);
        "#);
        pane_context(&mut runtime);
        run(&mut runtime, "event('CommandPaneOpened')");
        if policy != "success" {
            run(&mut runtime, "bind({load(){seq.cancel();}});bridge('load',{},2)");
        }
        run(&mut runtime, if policy == "pane_closed" {"event('PaneClosed')"} else {"event('CommandPaneExited')"});
        run(&mut runtime, &format!("check(results.at(-1).status==='{policy}');now+=100;bridge('update',{{Timer:0}},2)"));
    }
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      check(!sent.some(([c])=>c.SendSigkillToPaneId));
      check(sent.filter(([c])=>c.CloseTerminalPane===7).length===2);
      check(sent.filter(([c])=>c.RenameTerminalPane?.[1]==='named').length===3);
      bind({load(){globalThis.wait=zellij.runInPane(7,{path:'cat',stdin:'primitive'});wait.then(r=>results.push(r));}});bridge('load',{},2);
    "#);
    pane_context(&mut runtime);
    run(&mut runtime, "event('CommandPaneReRun');event('CommandPaneExited',4)");
    capture_commands(&mut runtime);
    run(&mut runtime, "check(results.at(-1).exitCode===4);check(sent.some(([c])=>c.WriteCharsToPaneId?.[0]==='primitive'))");
}

#[test]
fn pane_exit_observes_broadcast_metadata_and_cleans_up_once() {
    let mut runtime = sdk();
    runtime.op_state().borrow_mut().borrow_mut::<TestHost>().pane_info = json!({"id":7,"is_plugin":false,"exited":false});
    run(&mut runtime, r#"
      globalThis.results=[];
      bind({load(){zellij.waitForPaneExit({Terminal:7}).then(r=>{results.push(r);zellij.command('GetPluginIds');});}});
      bridge('load',{},2);
      globalThis.snapshot = panes => bridge('update',{PaneUpdate:{panes:{0:panes}}},9);
      snapshot([{id:8,is_plugin:false,exited:true,exit_status:1},{id:7,is_plugin:true,exited:true,exit_status:2}]);
      snapshot([]);
      snapshot([{id:7,is_plugin:false,exited:false,exit_status:null}]);
    "#);
    run(&mut runtime, "check(results.length===0);snapshot([{id:7,is_plugin:false,exited:true,exit_status:null}]);");
    run(&mut runtime, r#"
      check(results.length===1 && results[0].paneId.Terminal===7 && results[0].exitCode===null && !results[0].closed);
      snapshot([{id:7,is_plugin:false,exited:true,exit_status:4}]);
      bridge('update',{CommandPaneExited:[7,5,{}]},9);
      bridge('update',{PaneClosed:{Terminal:7}},9);
      check(results.length===1);
      bind({load(){zellij.unsubscribe(['CommandPaneOpened','CommandPaneReRun','CommandPaneExited','PaneClosed','PaneUpdate']);}});bridge('load',{},2);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      check(sent.at(-1)[0].Unsubscribe.length===5);
      check(sent.every(([,client])=>client===2));
      const query=sent.findIndex(([c])=>c.GetPaneInfo);
      check(sent.slice(0,query).some(([c])=>c.Subscribe?.includes('PaneUpdate')));
      check(sent[query][0].GetPaneInfo.Terminal===7);
    "#);
}

#[test]
fn pane_exit_initial_query_handles_recorded_exits_missing_and_other_panes() {
    for (info, expected) in [
        (json!({"id":7,"is_plugin":false,"exited":true,"exit_status":23}), "results.length===1 && results[0].exitCode===23 && !results[0].closed"),
        (json!({"id":7,"is_plugin":false,"exited":true,"exit_status":null}), "results.length===1 && results[0].exitCode===null && !results[0].closed"),
        (Value::Null, "results.length===1 && results[0].exitCode===null && results[0].closed"),
        (json!({"id":8,"is_plugin":false,"exited":true,"exit_status":23}), "results.length===0"),
        (json!({"id":7,"is_plugin":true,"exited":true,"exit_status":23}), "results.length===0"),
    ] {
        let mut runtime = sdk();
        runtime.op_state().borrow_mut().borrow_mut::<TestHost>().pane_info = info;
        run(&mut runtime, r#"
          globalThis.results=[];
          bind({load(){globalThis.wait=zellij.waitForPaneExit(7);wait.then(r=>results.push(r),()=>{});}});bridge('load',{},2);
        "#);
        run(&mut runtime, &format!("check({expected});wait.cancel();"));
        run(&mut runtime, r#"
          bind({load(){zellij.unsubscribe(['CommandPaneOpened','CommandPaneReRun','CommandPaneExited','PaneClosed','PaneUpdate']);}});bridge('load',{},2);
        "#);
        capture_commands(&mut runtime);
        run(&mut runtime, "check(sent.at(-1)[0].Unsubscribe.length===5)");
    }
}

#[test]
fn pane_exit_cancellation_query_failure_and_event_exit_release_observers() {
    for outcome in ["cancel", "GetPaneInfo", "event"] {
        let mut runtime = sdk();
        {
            let state = runtime.op_state();
            let mut state = state.borrow_mut();
            let host = state.borrow_mut::<TestHost>();
            host.pane_info = json!({"id":7,"is_plugin":false,"exited":false});
            if outcome == "GetPaneInfo" { host.fail_command = Some("GetPaneInfo"); }
        }
        run(&mut runtime, r#"
          globalThis.results=[];globalThis.errors=[];
          bind({load(){globalThis.wait=zellij.waitForPaneExit(7);wait.then(r=>results.push(r),e=>errors.push(e));}});bridge('load',{},2);
        "#);
        if outcome == "cancel" { run(&mut runtime, "wait.cancel();wait.cancel();"); }
        run(&mut runtime, r#"
          bridge('update',{CommandPaneExited:[7,31,{}]},9);
          bridge('update',{PaneUpdate:{panes:{0:[{id:7,is_plugin:false,exited:true,exit_status:32}]}}},9);
          bridge('update',{PaneClosed:{Terminal:7}},9);
          bind({load(){zellij.unsubscribe(['CommandPaneOpened','CommandPaneReRun','CommandPaneExited','PaneClosed','PaneUpdate']);}});bridge('load',{},2);
        "#);
        capture_commands(&mut runtime);
        run(&mut runtime, "check(sent.at(-1)[0].Unsubscribe.length===5)");
        run(&mut runtime, match outcome {
            "cancel" => "check(results.length===0 && errors.length===1 && errors[0].name==='AbortError')",
            "GetPaneInfo" => "check(results.length===0 && errors.length===1 && errors[0].message==='GetPaneInfo dispatch failed')",
            _ => "check(errors.length===0 && results.length===1 && results[0].exitCode===31 && !results[0].closed)",
        });
    }
}

#[test]
fn pane_commands_ignore_metadata_from_previous_exits() {
    for sequence in [false, true] {
        let mut runtime = sdk();
        runtime.op_state().borrow_mut().borrow_mut::<TestHost>().pane_info = json!({"id":7,"is_plugin":false,"exited":true,"exit_status":12});
        let start = if sequence { "zellij.paneSequence({steps:[{path:'true'}]}).done" } else { "zellij.runInPane(7,{path:'true'})" };
        run(&mut runtime, &format!("globalThis.results=[];bind({{load(){{({start}).then(r=>results.push(r));}}}});bridge('load',{{}},2);"));
        pane_context(&mut runtime);
        run(&mut runtime, r#"
          check(!sent.some(([c])=>c.GetPaneInfo || c.Subscribe?.includes('PaneUpdate')));
          bridge('update',{PaneUpdate:{panes:{0:[{id:7,is_plugin:false,exited:true,exit_status:12}]}}},2);
          event('CommandPaneExited',12,{__zellij_pane_step:'old'});
        "#);
        run(&mut runtime, "check(results.length===0);event('CommandPaneExited',0);");
        run(&mut runtime, "check(results.length===1)");
    }
}
