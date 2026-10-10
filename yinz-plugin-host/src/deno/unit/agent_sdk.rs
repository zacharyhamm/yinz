#[test]
fn agent_cache_states_turns_and_cancellation() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      globalThis.results = [];
      globalThis.pane = (id, state) => ({id,is_plugin:false,agent:{kind:'codex',state,session_id:'session'}});
      globalThis.manifest = (...panes) => bridge('update', {PaneUpdate:{panes:{0:panes}}}, 9);
      globalThis.hook = (id, hook) => bridge('update', {AgentEvent:{pane_id:id,hook,state:'Idle',last_message:'done',session_id:'session'}}, 9);
      globalThis.waits = [];
      bind({load() {
        const a = yinz.agent;
        check(a.info(7) === null);
        let count = 0;
        const off = a.on(() => count++); off(); off();
        a.on(() => check(count === 0));
        waits = [a.waitForState(7,'idle',{after:['busy','Waiting']}), a.waitForEvent({Terminal:7},['PreToolUse']), a.waitForTurn(7), a.waitForTurn(8,{fallback:'idle'}), a.waitForTurn(9,{fallback:'idle'}), a.waitForTurn(10,{fallback:'idle'})];
        waits.forEach((p,i) => p.then(v => results.push([i,v,yinz.getPluginIds().client_id]), e => results.push([i,e.name])));
        waits[4].cancel();
      }, update(e) {if (e.PaneUpdate) check(yinz.agent.info(7)?.state === e.PaneUpdate.panes[0].find(p => p.id === 7)?.agent?.state);}});
      bridge('load',{},2);
      manifest(pane(7,'Idle'),pane(8,'Idle'));
      globalThis.saved = null;
      bind({load() {saved=yinz.agent.all(); saved[7].state='Exited'; check(yinz.agent.info(7).state === 'Idle');}}); bridge('load',{},2);
      hook(7,'SessionStart'); hook(8,'Stop'); hook(99,'Stop');
      manifest(pane(7,'Busy'),pane(8,'Waiting'),pane(10,'Waiting'));
      manifest(pane(7,'Idle'),pane(8,'Idle'),pane(10,'Idle'));
      hook(7,'PreToolUse'); hook(7,'StopFailure'); hook(7,'Interrupt');
      manifest();
      bind({load() {check(yinz.agent.info(7) === null && Object.keys(yinz.agent.all()).length === 0); check(saved[8].state === 'Idle');}}); bridge('load',{},2);
    "#);
    run(&mut runtime, r#"
      check(results.length === 6);
      const get = i => results.find(r => r[0] === i);
      check(get(0)[1].agent.state === 'Idle' && get(1)[1].hook === 'PreToolUse');
      check(get(2)[1].hook === 'StopFailure' && get(2)[1].message === 'done' && get(2)[1].sessionId === 'session');
      check(get(5)[1].state === 'Idle' && get(5)[1].hook === null);
      check(get(3)[1].hook === 'Stop' && get(4)[1] === 'AbortError');
      check(results.filter(r => r[0] !== 4).every(r => r[2] === 2));
    "#);
}

#[test]
fn agent_validation_prompt_ordering_and_dispatch_cleanup() {
    let mut runtime = sdk();
    runtime.op_state().borrow_mut().borrow_mut::<TestHost>().pane_info = json!({"agent":{"kind":"codex"}});
    run(&mut runtime, r#"
      globalThis.errors=[];
      bind({load() {
        for (const pane of [-1, 1.5, NaN, Infinity, 4294967296, {Plugin:7}, {Terminal:7,Plugin:8}, '7', null])
          yinz.agent.waitForTurn(pane).catch(e => errors.push(e.name));
        yinz.agent.waitForState(7,'broken').catch(e => errors.push(e.name));
        yinz.agent.waitForEvent(7,[1]).catch(e => errors.push(e.name));
        yinz.agent.prompt(7,42,{fallback:'idle'}).catch(e => errors.push(e.name));
        yinz.agent.prompt(7,'go',{enter:'yes',fallback:'idle'}).catch(e => errors.push(e.name));
      }}); bridge('load',{},2);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, "check(errors.length === 13 && errors.every(e => e === 'TypeError') && sent.length === 0)");
    run(&mut runtime, r#"
      bind({load() {
        yinz.agent.prompt(7,'go').then(v => check(v.hook === 'Interrupt'));
        yinz.agent.send({Terminal:8},'draft',{enter:false});
      }}); bridge('load',{},2);
      bridge('update',{AgentEvent:{pane_id:7,hook:'Interrupt',last_message:null,state:'Idle',session_id:null}},9);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      check(sent.filter(([c]) => c.Subscribe).length === 1);
      const firstWrite = sent.findIndex(([c]) => c.WriteToPaneId);
      check(sent.slice(0,firstWrite).some(([c]) => c.Subscribe?.includes('AgentEvent')));
      const writes = sent.filter(([c]) => c.WriteToPaneId).map(([c]) => c.WriteToPaneId);
      check(writes.length === 3 && yinz.decodeUtf8(new Uint8Array(writes[0][0])) === '\x1b[200~go\x1b[201~');
      check(writes[1][0][0] === 13 && writes[2][1].Terminal === 8);
    "#);
    for command in ["WriteToPaneId", "Subscribe"] {
        runtime.op_state().borrow_mut().borrow_mut::<TestHost>().fail_command = Some(command);
        run(&mut runtime, r#"
          bind({load() {yinz.agent.prompt(7,'fail',{fallback:'idle'}).catch(e => errors.push(e.message));}}); bridge('load',{},2);
        "#);
        run(&mut runtime, &format!("check(errors.at(-1) === '{command} dispatch failed')"));
    }
    runtime.op_state().borrow_mut().borrow_mut::<TestHost>().fail_command = None;
    run(&mut runtime, "bind({load() {yinz.unsubscribe(['AgentEvent','PaneUpdate']);}}); bridge('load',{},2)");
    capture_commands(&mut runtime);
    run(&mut runtime, "check(sent.at(-1)[0].Unsubscribe.length === 2)");
}

#[test]
fn all_agent_identities_use_paste_and_unbounded_event_waits() {
    for kind in [None, Some(""), Some("opencode"), Some("fixture-agent"), Some("claude"), Some("codex"), Some("cursor-agent"), Some("claude-code"), Some("codex-cli"), Some("muse-bin-1.4.2"), Some("antigravity"), Some("/opt/bin/Gemini.js")] {
        let mut runtime = sdk();
        runtime.op_state().borrow_mut().borrow_mut::<TestHost>().pane_info = kind.map_or(json!({}), |kind| json!({"agent":{"kind":kind}}));
        run(&mut runtime, r#"
          globalThis.completed = false;
          globalThis.cancelled = false;
          globalThis.now = 0;
          Date.now = () => now;
          bind({load() {
            const pane = {Terminal:7};
            check(yinz.agent.send(pane,'draft',{enter:false}) === undefined);
            yinz.agent.send(pane,'submit');
            yinz.agent.prompt(pane,'prompt').then(result => {completed = result.message === 'done';});
            const pending = yinz.agent.prompt(8,'cancel',{enter:false});
            pending.catch(error => {cancelled = error.name === 'AbortError';});
            pending.cancel();
          }}); bridge('load',{},2);
          now = 60000;
          bridge('update',{Timer:60},9);
          check(!completed);
          bridge('update',{AgentEvent:{pane_id:8,hook:'Stop',last_message:'late',state:'Idle',session_id:null}},9);
          bridge('update',{AgentEvent:{pane_id:7,hook:'Stop',last_message:'done',state:'Idle',session_id:null}},9);
        "#);
        capture_commands(&mut runtime);
        run(&mut runtime, r#"
          check(completed && cancelled);
          // No read or RunCommands authority: only subscription and pane writes.
          check(sent.every(([c]) => c.Subscribe || c.Unsubscribe || c.WriteToPaneId));
          const writes = sent.filter(([c]) => c.WriteToPaneId).map(([c]) => c.WriteToPaneId);
          check(writes.slice(0,5).every(([,pane]) => pane.Terminal === 7) && writes[5][1].Terminal === 8);
          check(JSON.stringify(writes.map(([bytes]) => yinz.decodeUtf8(new Uint8Array(bytes)))) ===
            JSON.stringify(['\x1b[200~draft\x1b[201~','\x1b[200~submit\x1b[201~','\r','\x1b[200~prompt\x1b[201~','\r','\x1b[200~cancel\x1b[201~']));
        "#);
    }
}

#[test]
fn provider_registration_and_explicit_control_do_not_write_raw_input() {
    let mut runtime = sdk();
    runtime.op_state().borrow_mut().borrow_mut::<TestHost>().pane_info = json!({"agent":{"kind":"fixture-agent"}});
    runtime.op_state().borrow_mut().borrow_mut::<TestHost>().replies.insert("GetAgentProviders".into(), json!([{ "kind":"fixture-agent", "executable":"fixture-agent" }]));
    run(&mut runtime, r#"
      globalThis.answers = [];
      bind({load() {
        yinz.agent.registerProvider({kind:'fixture-agent', executable:'fixture-agent', input:{submit_key:'Ctrl j'}});
        check(yinz.agent.providers()[0].kind === 'fixture-agent');
        yinz.agent.control({Prompt:{pane_id:{Terminal:7},prompt:'work',wait:false,timeout_ms:30000}}).then(value => answers.push(value.outcome));
        yinz.agent.control({Prompt:{pane_id:{Terminal:7},prompt:'next',wait:true,timeout_ms:30000}}).then(value => answers.push(value.message));
      }}); bridge('load',{},2);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      check(!sent.some(([c]) => c.WriteToPaneId));
      const requests = sent.filter(([c]) => c.AgentControl).map(([c]) => c.AgentControl);
      check(requests.length === 2);
      check(JSON.parse(requests[0].request).Prompt.wait === false);
      check(JSON.parse(requests[1].request).Prompt.wait === true);
      for (const [i,request] of requests.entries()) bridge('update', {CustomMessage:['agent-response',JSON.stringify({id:request.id,response:{outcome:i === 0 ? 'submitted':'completed',message:'done',agents:[]}})]},2);
    "#);
    run(&mut runtime, "check(JSON.stringify(answers) === '[\"submitted\",\"done\"]')");
    for (command, _) in &runtime.op_state().borrow().borrow::<TestHost>().commands {
        serde_json::from_value::<yinz_types::data::PluginCommand>(command.clone()).unwrap();
    }
}

#[test]
fn agent_spawn_preserves_spec_and_host_defaults() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      bind({load() {
        check(yinz.agent.spawn({kind:'codex'}).Terminal === 7);
        check(yinz.agent.spawnDetails({kind:'claude',permission_mode:'plan',worktree_dir:'.trees',sourceDirectory:'/host/project',startSuppressed:true,context:{key:'value'}}).pane_id.Terminal === 7);
      }});bridge('load',{},2);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      const commands = sent.map(([c])=>c.SpawnAgent);
      check(commands[0].spec.kind === 'codex' && Object.keys(commands[0].spec).length === 1 && commands[0].source_directory === null && commands[0].start_suppressed === false);
      check(commands[1].spec.permission_mode === 'plan' && commands[1].source_directory === '/host/project' && commands[1].start_suppressed && commands[1].context.key === 'value');
    "#);
    for (command, _) in &runtime.op_state().borrow().borrow::<TestHost>().commands {
        serde_json::from_value::<yinz_types::data::PluginCommand>(command.clone()).unwrap();
    }
}
