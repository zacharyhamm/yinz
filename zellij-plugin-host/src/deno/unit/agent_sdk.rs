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
        const a = zellij.agent;
        check(a.info(7) === null);
        let count = 0;
        const off = a.on(() => count++); off(); off();
        a.on(() => check(count === 0));
        waits = [a.waitForState(7,'idle',{after:['busy','Waiting']}), a.waitForEvent({Terminal:7},['PreToolUse']), a.waitForTurn(7), a.waitForTurn(8,{fallback:'idle'}), a.waitForTurn(9,{fallback:'idle'}), a.waitForTurn(10,{fallback:'idle'})];
        waits.forEach((p,i) => p.then(v => results.push([i,v,zellij.getPluginIds().client_id]), e => results.push([i,e.name])));
        waits[4].cancel();
      }, update(e) {if (e.PaneUpdate) check(zellij.agent.info(7)?.state === e.PaneUpdate.panes[0].find(p => p.id === 7)?.agent?.state);}});
      bridge('load',{},2);
      manifest(pane(7,'Idle'),pane(8,'Idle'));
      globalThis.saved = null;
      bind({load() {saved=zellij.agent.all(); saved[7].state='Exited'; check(zellij.agent.info(7).state === 'Idle');}}); bridge('load',{},2);
      hook(7,'SessionStart'); hook(8,'Stop'); hook(99,'Stop');
      manifest(pane(7,'Busy'),pane(8,'Waiting'),pane(10,'Waiting'));
      manifest(pane(7,'Idle'),pane(8,'Idle'),pane(10,'Idle'));
      hook(7,'PreToolUse'); hook(7,'StopFailure'); hook(7,'Interrupt');
      manifest();
      bind({load() {check(zellij.agent.info(7) === null && Object.keys(zellij.agent.all()).length === 0); check(saved[8].state === 'Idle');}}); bridge('load',{},2);
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
    run(&mut runtime, r#"
      globalThis.errors=[];
      bind({load() {
        for (const pane of [-1, 1.5, NaN, Infinity, 4294967296, {Plugin:7}, {Terminal:7,Plugin:8}, '7', null])
          zellij.agent.waitForTurn(pane).catch(e => errors.push(e.name));
        zellij.agent.waitForState(7,'broken').catch(e => errors.push(e.name));
        zellij.agent.waitForEvent(7,[1]).catch(e => errors.push(e.name));
        zellij.agent.prompt(7,42,{fallback:'idle'}).catch(e => errors.push(e.name));
        zellij.agent.prompt(7,'go',{enter:'yes',fallback:'idle'}).catch(e => errors.push(e.name));
      }}); bridge('load',{},2);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, "check(errors.length === 13 && errors.every(e => e === 'TypeError') && sent.length === 0)");
    run(&mut runtime, r#"
      bind({load() {
        zellij.agent.prompt(7,'go').then(v => check(v.hook === 'Interrupt'));
        zellij.agent.send({Terminal:8},'draft',{enter:false});
      }}); bridge('load',{},2);
      bridge('update',{AgentEvent:{pane_id:7,hook:'Interrupt',last_message:null,state:'Idle',session_id:null}},9);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      check(sent.filter(([c]) => c.Subscribe).length === 1);
      const firstWrite = sent.findIndex(([c]) => c.WriteToPaneId);
      check(sent.slice(0,firstWrite).some(([c]) => c.Subscribe?.includes('AgentEvent')));
      const writes = sent.filter(([c]) => c.WriteToPaneId).map(([c]) => c.WriteToPaneId);
      check(writes.length === 3 && zellij.decodeUtf8(new Uint8Array(writes[0][0])) === '\x1b[200~go\x1b[201~');
      check(writes[1][0][0] === 13 && writes[2][1].Terminal === 8);
    "#);
    for command in ["WriteToPaneId", "Subscribe"] {
        runtime.op_state().borrow_mut().borrow_mut::<TestHost>().fail_command = Some(command);
        run(&mut runtime, r#"
          bind({load() {zellij.agent.prompt(7,'fail',{fallback:'idle'}).catch(e => errors.push(e.message));}}); bridge('load',{},2);
        "#);
        run(&mut runtime, &format!("check(errors.at(-1) === '{command} dispatch failed')"));
    }
    runtime.op_state().borrow_mut().borrow_mut::<TestHost>().fail_command = None;
    run(&mut runtime, "bind({load() {zellij.unsubscribe(['AgentEvent','PaneUpdate']);}}); bridge('load',{},2)");
    capture_commands(&mut runtime);
    run(&mut runtime, "check(sent.at(-1)[0].Unsubscribe.length === 2)");
}

#[test]
fn agent_spawn_preserves_spec_and_host_defaults() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      bind({load() {
        check(zellij.agent.spawn({kind:'codex'}).Terminal === 7);
        check(zellij.agent.spawnDetails({kind:'claude',permission_mode:'plan',worktree_dir:'.trees',sourceDirectory:'/host/project',startSuppressed:true,automaticFirecrackerVm:true,context:{key:'value'}}).pane_id.Terminal === 7);
      }});bridge('load',{},2);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      const commands = sent.map(([c])=>c.SpawnAgent);
      check(commands[0].spec.kind === 'codex' && Object.keys(commands[0].spec).length === 1 && commands[0].source_directory === null && commands[0].start_suppressed === false);
      check(commands[1].spec.permission_mode === 'plan' && commands[1].source_directory === '/host/project' && commands[1].automatic_firecracker_vm && commands[1].start_suppressed && commands[1].context.key === 'value');
    "#);
    for (command, _) in &runtime.op_state().borrow().borrow::<TestHost>().commands {
        serde_json::from_value::<zellij_types::data::PluginCommand>(command.clone()).unwrap();
    }
}
