fn completion_sdk() -> JsRuntime {
    let runtime = sdk();
    runtime.op_state().borrow_mut().borrow_mut::<TestHost>().session_list = json!({"live_sessions":[
        {"is_current_session":false,"plugins":{"99":{"location":"yinz:hocus-agentd"}}},
        {"is_current_session":true,"plugins":{"7":{"location":"yinz:hocus-agentd"},"8":{"location":"file:/hocus-agentd.wasm"}}}
    ]});
    runtime
}

#[test]
fn completion_concurrency_provenance_round_trips_deadlines_and_cleanup() {
    let mut runtime = completion_sdk();
    run(&mut runtime, r#"
      globalThis.now=1000; Date.now=()=>now;
      globalThis.results=[]; globalThis.requests=[]; globalThis.pipes=0;
      globalThis.reply=(id,body={},source={Plugin:7}) => bridge('pipe',{name:'hocus_complete_result',source,payload:JSON.stringify({request_id:id,content:'hello',tool_calls:[],finish_reason:'stop',usage:null,model:'local',error:'',...body})},9);
      bind({load() {
        for (let i=0;i<5;i++) {
          const p=yinz.hocus.complete({messages:[{role:'user',content:'why?'}],timeoutMs:100}); requests.push(p);
          p.then(v=>results.push([i,v,yinz.getPluginIds().client_id]),e=>results.push([i,e.name,e.message]));
        }
        yinz.waitForPipe('ordinary').then(()=>pipes++);
      },pipe(){pipes++;}}); bridge('load',{},2);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      globalThis.calls=sent.filter(([c])=>c.MessageToPlugin).map(([c])=>c.MessageToPlugin);
      globalThis.ids=calls.map(c=>JSON.parse(c.message_payload).request_id);
      check(new Set(ids).size===5 && calls.every(c=>c.destination_plugin_id===7 && c.plugin_url===null));
      reply(ids[0],{}, {Plugin:8}); reply(ids[0],{},'Keybind'); reply('old:hc1');
      for (const body of [{content:null},{tool_calls:{}},{tool_calls:[{id:'x',name:'n',arguments:2}]},{error:2},{usage:{}},{usage:{prompt_tokens:-1,completion_tokens:0,total_tokens:0}},{model:7}]) reply(ids[0],body);
      bridge('pipe',{name:'hocus_complete_result',source:{Plugin:7},payload:'{'},9);
      bridge('pipe',{name:'hocus_complete',source:{Plugin:7},payload:'{}'},9);
      bridge('pipe',{name:'ordinary',source:'Keybind',payload:null},9);
    "#);
    run(&mut runtime, r#"
      check(results.length===0 && pipes===2);
      reply(ids[1],{tool_calls:[{id:'call1',name:'lookup',arguments:'{invalid json'}],finish_reason:'tool_calls',usage:{prompt_tokens:1,completion_tokens:2,total_tokens:3}});
      reply(ids[1],{content:'duplicate'});
      reply(ids[0],{error:'daemon refused'});
      requests[2].cancel(); requests[2].cancel(); reply(ids[2]);
      reply(ids[3],{content:'é'.repeat(64001)});
      now=1100; bridge('update',{Timer:0},9);
    "#);
    run(&mut runtime, r#"
      check(results.length===5);
      const get=i=>results.find(r=>r[0]===i);
      check(get(0)[2]==='daemon refused' && get(2)[1]==='AbortError' && get(4)[1]==='TimeoutError');
      check(get(1)[1].toolCalls[0].arguments==='{invalid json' && get(1)[1].usage.total_tokens===3 && get(1)[2]===2);
      check(get(3)[1].content.length===64001);
      bind({load() {
        const p=yinz.hocus.complete({messages:[{role:'assistant',tool_calls:get(1)[1].toolCalls},{role:'tool',tool_call_id:'call1',content:'answer'}],system:'help',tools:[{type:'function'}],toolChoice:'auto',temperature:0,maxTokens:12});
        p.catch(()=>{});p.cancel(); yinz.unsubscribe(['Timer']);
      }});bridge('load',{},2);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      check(sent.at(-1)[0].Unsubscribe.includes('Timer'));
      const body=JSON.parse(sent.filter(([c])=>c.MessageToPlugin).at(-1)[0].MessageToPlugin.message_payload);
      check(body.messages[0].tool_calls[0].arguments==='{invalid json' && body.tool_choice==='auto' && body.max_tokens===12);
    "#);
}

#[test]
fn completion_validates_before_dispatch_and_handles_discovery_and_dispatch_errors() {
    let mut runtime = completion_sdk();
    run(&mut runtime, r#"
      globalThis.errors=[];globalThis.touched=false;
      bind({load() {
        const invalid=[{get messages(){touched=true;return [];}},new Proxy({}, {ownKeys(){touched=true;return [];}}),{}, {messages:[]}, {messages:[{role:'invalid'}]}, {messages:[{role:'tool'}]}, {messages:[{role:'user',content:3}]}, {messages:[{role:'user',tool_calls:[{id:3,name:'bad'}]}]}, {messages:[],system:7}, {messages:[],system:'ok',tools:Array(65).fill({})}, {messages:Array(201).fill({role:'user'})}, {messages:[],system:'x'.repeat(64000)}, {messages:[],system:'é'.repeat(32000)}, {messages:[],system:'ok',maxTokens:-1}, {messages:[],system:'ok',temperature:Infinity}, {messages:[],system:'ok',tools:[1n]}, {messages:[],system:'ok',unknown:1}];
        for(const opts of invalid)yinz.hocus.complete(opts).catch(e=>errors.push(e.message));
        const invalidTimeouts=[-1,2147483648,NaN,Infinity,-Infinity];
        for(const timeoutMs of invalidTimeouts)yinz.hocus.complete({messages:[],system:'ok',timeoutMs}).catch(e=>errors.push(e.message));
        globalThis.expected=invalid.length+invalidTimeouts.length;
      }});bridge('load',{},2);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime,"check(!touched && errors.length===expected && sent.length===0)");
    for snapshot in [json!({"live_sessions":[]}), json!({"live_sessions":[{"is_current_session":true,"plugins":{"7":{"location":"yinz:hocus-agentd"},"8":{"location":"yinz:hocus-agentd"}}}]})] {
        runtime.op_state().borrow_mut().borrow_mut::<TestHost>().session_list=snapshot;
        run(&mut runtime,"bind({load(){yinz.hocus.complete({messages:[],system:'ok'}).catch(e=>errors.push(e.message));}});bridge('load',{},2)");
    }
    run(&mut runtime,"check(errors.at(-2).includes('no hocus-agentd') && errors.at(-1).includes('multiple hocus-agentd'))");
    let mut runtime=completion_sdk();
    run(&mut runtime,"globalThis.errors=[]");
    for name in ["GetSessionList","Subscribe","SetTimeout","MessageToPlugin"] {
        runtime.op_state().borrow_mut().borrow_mut::<TestHost>().fail_command=Some(name);
        run(&mut runtime,"bind({load(){yinz.hocus.complete({messages:[{role:'user'}]}).catch(e=>errors.push(e.message));}});bridge('load',{},2)");
        run(&mut runtime,&format!("check(errors.at(-1)==='{name} dispatch failed')"));
    }
    runtime.op_state().borrow_mut().borrow_mut::<TestHost>().fail_command=None;
    run(&mut runtime,"bind({load(){yinz.unsubscribe(['Timer']);}});bridge('load',{},2)");
    capture_commands(&mut runtime);
    run(&mut runtime,"check(sent.at(-1)[0].Unsubscribe.includes('Timer'))");
}

#[test]
fn completion_request_limit_counts_the_encoded_envelope_and_isolate_ids_change() {
    let seed = |runtime: &mut JsRuntime| {
        run(runtime, r#"
          bind({load(){const p=yinz.hocus.complete({messages:[],system:''});p.catch(()=>{});p.cancel();}});bridge('load',{},2);
        "#);
        capture_commands(runtime);
        run(runtime, r#"
          globalThis.seed=JSON.parse(sent.find(([c])=>c.MessageToPlugin)[0].MessageToPlugin.message_payload);
        "#);
    };
    let id = || {
        let mut runtime = completion_sdk();
        seed(&mut runtime);
        let encoded = runtime.execute_script("id.js", "seed.request_id").unwrap();
        deno_core::scope!(scope, &mut runtime);
        v8::Local::new(scope, encoded).to_rust_string_lossy(scope)
    };
    assert_ne!(id(), id());
    let mut first = completion_sdk();
    seed(&mut first);
    run(&mut first, r#"
      globalThis.errors=[];
      bind({load(){
        const remaining=64000-yinz.encodeUtf8(JSON.stringify(seed)).length;
        const p=yinz.hocus.complete({messages:[],system:'x'.repeat(remaining)});p.catch(()=>{});p.cancel();
        yinz.hocus.complete({messages:[],system:'x'.repeat(remaining-1)+'é'}).catch(e=>errors.push(e.name));
        for(const timeoutMs of [-1,2147483648,NaN])yinz.hocus.complete({messages:[],system:'ok',timeoutMs}).catch(e=>errors.push(e.name));
        yinz.unsubscribe(['Timer']);
      }});bridge('load',{},2);
    "#);
    capture_commands(&mut first);
    run(&mut first, r#"
      const sentRequests=sent.filter(([c])=>c.MessageToPlugin);
      check(sentRequests.length===2 && yinz.encodeUtf8(sentRequests[1][0].MessageToPlugin.message_payload).length===64000);
      check(errors.length===4 && errors[0]==='RangeError' && sent.at(-1)[0].Unsubscribe.includes('Timer'));
    "#);
}
