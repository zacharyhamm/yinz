fn capture_trace(runtime: &mut JsRuntime) {
    let logs = json!(runtime.op_state().borrow().borrow::<TestHost>().logs);
    run(runtime, &format!("globalThis.logs = {logs}; globalThis.markers = logs.map(l => l.split(' client=')[1]);"));
}

#[test]
fn trace_configuration_toggles_and_reload_are_local() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      bind({load(){zellij.command('GetPluginIds');}}); bridge('load',{},1);
    "#);
    capture_trace(&mut runtime);
    run(&mut runtime, r#"
      check(logs.length===0);
      bind({defaults:{trace:'true'},load(){zellij.command('GetPluginIds');}});
      bridge('load',{trace:'false'},1);
    "#);
    capture_trace(&mut runtime);
    run(&mut runtime, r#"
      check(logs.length===0); bridge('load',{},2);
      zellij.trace(false); bridge('load',{trace:'other'},3);
      zellij.trace(); bridge('load',{trace:'other'},4);
      for(const value of [null,1,'true',{},()=>{}]) {
        let threw=false; try {zellij.trace(value);} catch(e){threw=e instanceof TypeError;} check(threw);
      }
    "#);
    capture_trace(&mut runtime);
    run(&mut runtime, r#"
      check(markers.join('|')==='2 command "GetPluginIds"|4 command "GetPluginIds"');
      check(logs.every(l=>l.startsWith('trace instance='+zellij.instanceId+' ')));
    "#);
    drop(runtime);
    let mut runtime = sdk();
    run(&mut runtime, "bind({load(){zellij.command('GetPluginIds');}}); bridge('load',{},1);");
    capture_trace(&mut runtime);
    run(&mut runtime, "check(logs.length===0);");
}

#[test]
fn trace_dispatch_async_and_consumed_inputs_preserve_behavior_and_privacy() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      globalThis.outcomes=[];
      bind({defaults:{trace:'true'},load(){
        zellij.command('GetPluginIds'); zellij.command({RenameTab:[0,'SECRET']});
        zellij.command({DataUnwatch:{watch_id:9007199254740993n}});
        zellij.command({Write:[1,2,3]});
        for(const value of [Object.defineProperty({},'SECRET',{get(){throw Error('accessed SECRET');},enumerable:true}),
          new Proxy({},{ownKeys(){throw Error('accessed SECRET');}})]) {
          try {zellij.command(value); throw Error('accepted');} catch(e){check(!String(e).includes('accessed'));}
        }
        zellij.requestPermission(['ReadFiles']); zellij.requestPermission(['RunCommands']);
        globalThis.wait=zellij.waitForPipe('done'); wait.then(()=>outcomes.push('finished'));
        const cancelled=zellij.waitForEvent('Key'); cancelled.catch(e=>outcomes.push(e.name)); cancelled.cancel();
        const rejected=zellij.waitForPipe('bad',()=>{throw Error('SECRET')}); rejected.catch(()=>outcomes.push('failed'));
        zellij.after(0,async()=>{await Promise.resolve(); zellij.command('GetPluginIds');});
        return Promise.resolve(true);
      }}); bridge('load',{},7);
      answerPermissions('Granted'); bridge('update',{PermissionRequestResult:'Granted'},9);
      bridge('pipe',{name:'done',payload:'SECRET'},9);
      bridge('pipe',{name:'bad',payload:new Uint8Array([1,2])},9);
      bridge('pipe',{name:'zellij_rpc_reply',payload:'SECRET'},9);
      bridge('pipe',{name:'hocus_complete_result',payload:'SECRET'},9);
      bridge('pipe',{name:'a\n\x1b\x85'+ 'x'.repeat(250),payload:'SECRET'},9);
      bridge('update',{Timer:0},9);
    "#);
    capture_trace(&mut runtime);
    run(&mut runtime, r#"
      check(outcomes.join('|')==='AbortError|finished|failed');
      check(!logs.join('').includes('SECRET'));
      check(markers.filter(l=>l==='7 command "GetPluginIds"').length===2);
      check(markers.filter(l=>l==='7 command "RenameTab"').length===1);
      check(markers.filter(l=>l==='7 command "RequestPluginPermissions"').length===2);
      check(markers.filter(l=>l==='7 command invalid').length===2);
      check(markers.includes('9 pipe "zellij_rpc_reply"'));
      check(markers.includes('9 pipe "hocus_complete_result"'));
      check(markers.includes('9 event "Timer"'));
      check(markers.includes('7 operation 1 finished'));
      check(markers.includes('7 operation 2 cancelled'));
      check(markers.includes('7 operation 3 failed'));
      check(markers.includes('7 task 4 finished') && markers.includes('7 task 5 finished'));
      check(logs.every(l=>!/[\n\x1b\x85]/.test(l)));
      check(markers.some(l=>l.includes('a\\n\\u001b\\u0085') && !l.includes('x'.repeat(200))));
    "#);
    runtime.op_state().borrow_mut().borrow_mut::<TestHost>().fail_command = Some("SetTimeout");
    run(&mut runtime, r#"
      bind({load(){return zellij.sleep(0);}});
      bridge('load',{trace:'true'},12);
    "#);
    capture_trace(&mut runtime);
    run(&mut runtime, r#"
      check(markers.includes('12 command "SetTimeout"'));
      check(markers.includes('12 operation 6 failed'));
      check(markers.includes('12 task 7 failed'));
      let failed=false; try{bridge('status',null,12);}catch(e){failed=String(e).includes('SetTimeout dispatch failed');} check(failed);
    "#);
}

#[test]
fn trace_watch_disposal_and_disabled_completions() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      bind({load(){
        zellij.trace();
        globalThis.watch=zellij.data.watch({entity:'items'},()=>{});
        globalThis.wait=zellij.waitForPipe('done'); wait.catch(()=>{});
      }}); bridge('load',{},5);
      watch.dispose(); watch.dispose();
      zellij.trace(false); wait.cancel();
    "#);
    capture_trace(&mut runtime);
    run(&mut runtime, r#"
      check(markers.filter(l=>l==='5 command "DataUnwatch"').length===1);
      check(markers.filter(l=>l==='5 command "DataWatch"').length===1);
      check(markers.filter(l=>l==='5 command "Subscribe"').length===1);
      check(markers.includes('5 operation 1 started'));
      check(!markers.some(l=>l.endsWith('cancelled')));
    "#);
}
