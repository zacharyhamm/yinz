#[test]
fn rpc_dispatches_general_handlers_and_keeps_pipes_public() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      globalThis.message = (payload, source = {Plugin:7}, name = 'zellij_rpc_call') => ({name, source, payload:JSON.stringify(payload)});
      globalThis.finished = null;
      globalThis.pipes = 0;
      globalThis.waited = false;
      zellij.rpc.handle('echo', () => 'replaced');
      zellij.rpc.handle('echo', (payload, call) => ({payload, source:call.source, client:zellij.getPluginIds().client_id}));
      zellij.rpc.handle('async', async () => {await new Promise(r => finished = r); return zellij.getPluginIds().client_id;});
      zellij.rpc.handle('throw', () => {throw Error('thrown');});
      zellij.rpc.handle('reject', async () => {throw Error('rejected');});
      zellij.rpc.handle('nil', () => {});
      const invalid = [undefined, NaN, Infinity, 1n, new Uint8Array([1]), () => {}, Symbol(), new Date(), {x:undefined}, [undefined], Array(1), {get x() {throw Error('getter ran');}}, new Proxy({}, {}), Object.defineProperty({}, 'x', {value:1}), 'x'.repeat(1048576)];
      invalid.forEach((value, i) => zellij.rpc.handle('invalid'+i, () => ({value})));
      let reserved = false;
      try {zellij.rpc.handle('hocus_tool:x', () => null);} catch (_) {reserved = true;}
      check(reserved);
      bind({load() {zellij.waitForPipe('ordinary').then(() => waited = true);}, pipe() {pipes++;}});
      bridge('load', {}, 1);
      for (const [i, name] of ['echo','async','throw','reject','nil','unknown',...invalid.map((_,i) => 'invalid'+i)].entries())
        bridge('pipe', message({version:1, call_id:String(i), name, payload:{city:'Oslo'}, source:{Plugin:99}, reply_to:99}), 3);
      bridge('pipe', message({version:1,call_id:'forged',name:'echo',reply_to:7}, 'Keybind'), 4);
      bridge('pipe', message({version:2,call_id:'bad',name:'echo'}), 4);
      bridge('pipe', message({}, {Plugin:7}, 'zellij_rpc_reply'), 4);
      bridge('pipe', message({}, 'Keybind', 'ordinary'), 4);
      finished();
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      check(pipes === 1 && waited);
      const replies = sent.filter(([c]) => c.MessageToPlugin).map(([c,client]) => ({...JSON.parse(c.MessageToPlugin.message_payload), client, destination:c.MessageToPlugin.destination_plugin_id}));
      check(replies.length === 21 && replies.every(r => r.client === 3 && r.destination === 7));
      const get = id => replies.find(r => r.call_id === String(id));
      check(get(0).result.payload.city === 'Oslo' && get(0).result.source.Plugin === 7);
      check(get(1).result === 3 && get(2).error.includes('thrown') && get(3).error.includes('rejected'));
      check(get(4).result === null && get(5).error.includes('no rpc handler'));
      check(replies.filter(r => Number(r.call_id) >= 6).every(r => typeof r.error === 'string' && !r.error.includes('getter ran')));
    "#);
}

#[test]
fn rpc_calls_correlate_validate_cancel_timeout_and_release_timers() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      globalThis.now = 1000; Date.now = () => now;
      globalThis.results = [];
      globalThis.requests = [];
      globalThis.receive = (id, body, source = {Plugin:7}) => bridge('pipe', {name:'zellij_rpc_reply', source, payload:JSON.stringify({version:1,call_id:id,...body})}, 9);
      bind({load() {
        for (let i=0; i<5; i++) {
          const request = zellij.rpc.call({pluginId:7, name:'echo', timeoutMs:100});
          requests.push(request);
          request.then(value => results.push([i,value,zellij.getPluginIds().client_id]), error => results.push([i,error.name,zellij.getPluginIds().client_id]));
        }
        zellij.rpc.call({pluginUrl:'file:/plugin.ts',pluginConfig:{city:'Oslo'},name:'url'}).then(value => results.push(['url',value]));
      }});
      bridge('load', {}, 2);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      globalThis.calls = sent.filter(([c]) => c.MessageToPlugin).map(([c]) => c.MessageToPlugin);
      globalThis.ids = calls.map(c => JSON.parse(c.message_payload).call_id);
      check(new Set(ids).size === 6 && calls[0].destination_plugin_id === 7);
      check(JSON.parse(calls[0].message_payload).payload === null && calls[5].plugin_config.city === 'Oslo');
      receive(ids[0], {result:'forged'}, {Plugin:8});
      receive(ids[0], {result:'forged'}, 'Keybind');
      receive(ids[0], {result:'bad',error:'bad'});
      receive(ids[0], {error:7});
      receive('old:rpc1', {result:'old'});
      receive(ids[1], {result:{city:'Oslo'}});
      receive(ids[1], {result:'duplicate'});
      receive(ids[0], {error:'remote'});
      requests[2].cancel(); requests[2].cancel();
      receive(ids[2], {result:'cancelled'});
      receive(ids[3], {}); // legacy absent result
      receive(ids[5], {result:'url'}, {Plugin:123});
      now = 1100; bridge('update', {Timer:0}, 9);
    "#);
    run(&mut runtime, r#"
      check(results.length === 6);
      const get = id => results.find(r => r[0] === id);
      check(get(1)[1].city === 'Oslo' && get(0)[1] === 'Error' && get(2)[1] === 'AbortError');
      check(get(3)[1] === null && get(4)[1] === 'TimeoutError' && get('url')[1] === 'url');
      check(results.filter(r => r[0] !== 'url').every(r => r[2] === 2));
      bind({load() {zellij.unsubscribe(['Timer']);}}); bridge('load', {}, 2);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, "check(sent.some(([c]) => c.Unsubscribe?.includes('Timer')))");
    runtime.op_state().borrow_mut().borrow_mut::<TestHost>().fail_command = Some("MessageToPlugin");
    run(&mut runtime, r#"
      globalThis.errors = [];
      bind({load() {
        for (const options of [{}, {pluginId:7,pluginUrl:'x',name:'x'}, {pluginId:-1,name:'x'}, {pluginUrl:'',name:'x'}, {pluginId:7,name:''}, {pluginId:7,name:'x',payload:{x:undefined}}, {pluginId:7,name:'x',timeoutMs:-1}, {pluginId:7,name:'x',timeoutMs:5}, {pluginId:7,name:'x',typo:1}])
          zellij.rpc.call(options).catch(e => errors.push(e.message));
      }}); bridge('load', {}, 2);
    "#);
    run(&mut runtime, "check(errors.length === 9 && errors.some(e => e.includes('dispatch failed'))); bind({load() {zellij.unsubscribe(['Timer']);}}); bridge('load', {}, 2)");
    capture_commands(&mut runtime);
    run(&mut runtime, "check(sent.at(-1)[0].Unsubscribe.includes('Timer'))");
}

#[test]
fn rpc_replacement_isolates_have_distinct_call_ids() {
    let call_id = || {
        let mut runtime = sdk();
        run(&mut runtime, "bind({load() {zellij.rpc.call({pluginId:7,name:'echo'});}}); bridge('load',{},1)");
        let state = runtime.op_state();
        let state = state.borrow();
        let command = state.borrow::<TestHost>().commands.iter().find_map(|(c,_)| c.get("MessageToPlugin")).unwrap();
        serde_json::from_str::<Value>(command["message_payload"].as_str().unwrap()).unwrap()["call_id"].clone()
    };
    assert_ne!(call_id(), call_id());
}

#[test]
fn rpc_bounds_error_replies_even_with_a_full_size_call_id() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      const call = {version:1,call_id:'',name:'x'};
      call.call_id = 'x'.repeat(1048576 - zellij.encodeUtf8(JSON.stringify(call)).length);
      globalThis.longId = call.call_id;
      bridge('pipe', {name:'zellij_rpc_call',source:{Plugin:7},payload:JSON.stringify(call)}, 1);
      bridge('pipe', {name:'zellij_rpc_call',source:{Plugin:7},payload:JSON.stringify({...call,call_id:call.call_id+'x'})}, 1);
      bridge('pipe', {name:'zellij_rpc_call',source:{Plugin:7},payload:'{'}, 1);
      bridge('pipe', {name:'zellij_rpc_call',source:{Plugin:7},payload:'{"version":1,"call_id":"bad","name":"x","payload":1e999}'}, 1);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      const replies = sent.filter(([c]) => c.MessageToPlugin);
      check(replies.length === 1);
      const encoded = replies[0][0].MessageToPlugin.message_payload;
      check(zellij.encodeUtf8(encoded).length <= 1048576);
      check(JSON.parse(encoded).call_id === longId && typeof JSON.parse(encoded).error === 'string');
    "#);
}

#[test]
fn rpc_carries_report_chunks_and_large_snapshots_without_raising_hocus_limits() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      globalThis.values=['😀'.repeat(16384), '\u0000'.repeat(32768), 'snapshot'.repeat(20000)];
      globalThis.results=[];globalThis.rejected=[];
      zellij.rpc.handle('report', payload=>payload);
      bind({load(){
        for(const payload of values) zellij.rpc.call({pluginId:7,name:'report',payload}).then(result=>results.push(result));
        zellij.rpc.call({pluginId:7,name:'hocus_tool:report',payload:values[0]}).catch(error=>rejected.push(error.message));
      }});bridge('load',{},2);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      check(rejected.length===1 && rejected[0].includes('64000'));
      const calls=sent.filter(([c])=>c.MessageToPlugin).map(([c])=>c.MessageToPlugin);
      check(calls.length===3);
      for(const call of calls){
        check(zellij.encodeUtf8(call.message_payload).length>64000);
        bridge('pipe',{name:'zellij_rpc_call',source:{Plugin:7},payload:call.message_payload},2);
      }
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      const replies=sent.filter(([c])=>c.MessageToPlugin?.message_name==='zellij_rpc_reply').map(([c])=>c.MessageToPlugin);
      check(replies.length===3);
      for(const reply of replies) bridge('pipe',{name:'zellij_rpc_reply',source:{Plugin:7},payload:reply.message_payload},2);
    "#);
    run(&mut runtime, "check(JSON.stringify(results)===JSON.stringify(values));");
}
