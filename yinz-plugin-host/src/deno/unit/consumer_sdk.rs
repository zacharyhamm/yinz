fn consumer(runtime: &mut JsRuntime, name: &str) {
    use yinz_types::ui_components::UI_COMPONENT_DCS_FINAL;
    let builtin = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../yinz-deno-builtins/assets/plugins/{name}.ts"));
    let path = if builtin.exists() { builtin } else {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../example-plugins/{name}.ts"))
    };
    let source = std::fs::read_to_string(&path).unwrap();
    let emitted = loader::transpile(&path, &source, false).unwrap().text.clone();
    let source = emitted.lines().filter(|line| !line.starts_with("import "))
        .collect::<Vec<_>>().join("\n")
        .replace("export default definePlugin", "globalThis.M = definePlugin");
    run(runtime, &format!(r#"globalThis.plain = text => text.replace(/\x1bP{UI_COMPONENT_DCS_FINAL}text;([^\x1b]*)\x1b\\/g, (_match, payload) => {{
      const body=payload.split(';').at(-1).split('$').at(-1).replace(/^[a-z]+/, '');
      return body ? yinz.decodeUtf8(new Uint8Array(body.split(',').map(Number))) : '';
    }});"#));
    run(runtime, &format!("globalThis.reloadConsumer = () => {{ globalThis.handlers = {{}}; (function(z, definePlugin) {{ {source}\n}})(mock, p => p); }}; reloadConsumer();"));
}


#[test]
fn deno_agent_harness_spawns_with_defaults_and_a_configured_branch() {
    for config in [json!({}), json!({"branch":"fix-tests"})] {
        let mut runtime = sdk();
        run(&mut runtime, "globalThis.mock=yinz;");
        consumer(&mut runtime, "agent-harness");
        run(&mut runtime, &format!("bind(M); bridge('load',{config},2);"));
        run(&mut runtime, "answerPermissions('Granted'); bridge('update',{PermissionRequestResult:'Granted'},2);");
        let state = runtime.op_state();
        let state = state.borrow();
        let spawns: Vec<_> = state.borrow::<TestHost>().commands.iter()
            .filter_map(|(command, _)| command.get("SpawnAgent"))
            .collect();
        assert_eq!(spawns.len(), 1, "the actual harness must reach spawning: {config}");
        let spawn = spawns[0];
        assert_eq!(spawn["spec"]["kind"], "claude");
        assert_eq!(spawn["spec"]["prompt"], "make the tests pass");
        assert_eq!(spawn["spec"]["branch"], config["branch"]);
        assert_eq!(spawn["spec"]["allow_outside_worktree"], config.get("branch").is_none());
        serde_json::from_value::<yinz_types::data::PluginCommand>(json!({"SpawnAgent":spawn})).unwrap();
    }
}


#[test]
fn typescript_pane_sequence_reuses_scrollback_and_escalates_cancellation() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      globalThis.mock=yinz;globalThis.now=100;Date.now=()=>now;
    "#);
    run(&mut runtime, r#"
      let sequence, result;
      const start = () => {
        sequence = yinz.paneSequence({steps:[1,2,3].map(n=>({path:'echo',args:[String(n)]})),floating:true,killGraceMs:2000});
        sequence.done.then(value=>result=value);
      };
      globalThis.M = {
        load() { yinz.on('Key', key => { if(key.bare_key.Char==='r') start(); else sequence.cancel(); }); },
        render() { return `sequence ${result.status==='success'?'completed':result.status}; exit codes: ${JSON.stringify(result.exitCodes)}`; }
      };
    "#);
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
