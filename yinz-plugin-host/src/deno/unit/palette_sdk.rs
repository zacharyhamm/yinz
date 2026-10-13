#[test]
fn palette_sdk_async_views_preserve_clients_and_release_callbacks() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      globalThis.requests = []; globalThis.executions = []; globalThis.resolveDiscovery = [];
      bind({load() {
        globalThis.unregisterPalette = yinz.commandPalette.registerProvider({id:'remote', title:'Remote',
          async provideActions(context) {
            requests.push(context);
            await new Promise(resolve => resolveDiscovery.push(resolve));
            check(yinz.getPluginIds().client_id === context.clientId);
            return [{id:'semantic',label:'Unrelated result',run: async context => {
              executions.push(context); await Promise.resolve();
              check(yinz.getPluginIds().client_id === context.clientId);
              return {type:'actions',title:'Nested',actions: request => [{id:'form',label:request.query || 'Form',run: () => ({
                type:'form',title:'Form', fields:[{type:'text',id:'name',label:'Name'}, {type:'choice',id:'kind',label:'Kind',options:['a','b']}],
                async onSubmit(values, context) {
                  await Promise.resolve(); check(yinz.getPluginIds().client_id === context.clientId);
                  if (!values.name) throw Error('Name required');
                  if (values.name === 'reject') return Promise.reject();
                  return {type:'text',title:'Result',text:values.name + ':' + values.kind};
                }
              })} ]};
            }}];
          }});
      }}); bridge('load',{},1);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      globalThis.registration = sent.find(([c]) => c.CommandPalette?.type === 'register')[0].CommandPalette.registration;
      globalThis.scope = {registration,opening:1,view:0,revision:1};
      globalThis.context = {clientId:2,paneId:{Terminal:7},tabId:3,currentProject:'project'};
      bridge('palette',{type:'discover',scope,context,query:'semantic query',source:null},2);
      bridge('palette',{type:'discover',scope:{...scope,opening:2},context:{...context,clientId:3},query:'other',source:null},3);
      check(requests.length === 2 && requests[0].query === 'semantic query');
      resolveDiscovery[1](); resolveDiscovery[0]();
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      globalThis.replies = sent.filter(([c]) => c.CommandPalette?.type === 'reply');
      check(replies.length === 2 && replies[0][1] === 3 && replies[1][1] === 2);
      globalThis.action = replies[1][0].CommandPalette.result.Ok.actions[0];
      bridge('palette',{type:'execute',scope:{...scope,revision:2},context,handle:action.handle,values:null},2);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      globalThis.nested = sent.at(-1)[0].CommandPalette.result.Ok.view;
      check(nested.type === 'actions' && nested.source && nested.actions.length === 0);
      bridge('palette',{type:'discover',scope:{...scope,view:1,revision:3},context,query:'dynamic',source:nested.source},2);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      globalThis.formAction = sent.at(-1)[0].CommandPalette.result.Ok.actions[0];
      check(formAction.label === 'dynamic');
      bridge('palette',{type:'execute',scope:{...scope,view:1,revision:4},context,handle:formAction.handle,values:null},2);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      globalThis.form = sent.at(-1)[0].CommandPalette.result.Ok.view;
      check(form.fields[0].initialValue === '' && form.fields[1].initialValue === 'a' && form.submitLabel === 'Submit');
      bridge('palette',{type:'execute',scope:{...scope,view:2,revision:5},context,handle:form.handle,values:{name:'',kind:'b'}},2);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      check(sent.at(-1)[0].CommandPalette.result.Err === 'Name required');
      bridge('palette',{type:'execute',scope:{...scope,view:2,revision:6},context,handle:form.handle,values:{name:'reject',kind:'b'}},2);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      check(sent.at(-1)[0].CommandPalette.result.Err === 'Palette callback failed');
      bridge('palette',{type:'execute',scope:{...scope,view:2,revision:6},context,handle:form.handle,values:{name:'value',kind:'b'}},2);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      check(sent.at(-1)[0].CommandPalette.result.Ok.view.text === 'value:b');
      bridge('palette',{type:'release',handles:[form.handle]},2);
      bridge('palette',{type:'execute',scope:{...scope,view:2,revision:7},context,handle:form.handle,values:{}},2);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, "check(sent.at(-1)[0].CommandPalette.result.Err.includes('no longer available')); unregisterPalette(); unregisterPalette();");
}

#[test]
fn palette_sdk_cancellation_close_validation_and_unregister() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      globalThis.cancelled = 0; globalThis.contexts = []; globalThis.resolvers = [];
      bind({load() {
        globalThis.removeProvider = yinz.commandPalette.registerProvider({id:'test',title:'Test',provideActions: async context => {
          contexts.push(context);
          const remove = context.onCancel(() => cancelled += 100); remove();
          context.onCancel(() => cancelled++);
          return await new Promise(resolve => resolvers.push(resolve));
        }});
        let duplicate = false;
        try { yinz.commandPalette.registerProvider({id:'test',title:'Test',provideActions:()=>[]}); } catch (_) { duplicate = true; }
        check(duplicate);
      }}); bridge('load',{},1);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      globalThis.registration = sent.find(([c]) => c.CommandPalette?.type === 'register')[0].CommandPalette.registration;
      globalThis.scope = {registration,opening:1,view:0,revision:1};
      globalThis.context = {clientId:1,paneId:{Terminal:1},tabId:1,currentProject:null};
      globalThis.discover = revision => bridge('palette',{type:'discover',scope:{...scope,revision},context,query:'',source:null},1);
      discover(1); bridge('palette',{type:'cancel',scope},1); bridge('palette',{type:'cancel',scope},1);
      check(contexts[0].cancelled && cancelled === 1);
      contexts[0].onCancel(() => cancelled++); check(cancelled === 2);
      resolvers[0]([{id:'late',label:'Late',run(){}}]);
      discover(2); resolvers[1]([{id:'dup',label:'A',run(){}},{id:'dup',label:'B',run(){}}]);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      const replies = sent.filter(([c])=>c.CommandPalette?.type === 'reply');
      check(replies.length === 1 && replies[0][0].CommandPalette.result.Err.includes('unique'));
      discover(3); resolvers[2]([{id:'close',label:'Close',run:async context=> {
        globalThis.executionContext = context;
        context.close(); await new Promise(resolve => globalThis.continueExecution = resolve);
        check(!context.cancelled); throw Error('after close');
      }}]);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      bridge('palette',{type:'execute',scope:{...scope,revision:4},context,handle:sent.at(-1)[0].CommandPalette.result.Ok.actions[0].handle,values:null},1);
      bridge('palette',{type:'dismiss',opening:1},1);
      continueExecution();
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      check(sent.at(-2)[0].CommandPalette.type === 'close');
      check(sent.at(-1)[0].CommandPalette.result.Err === 'after close');
      discover(5); removeProvider(); check(contexts.at(-1).cancelled);
      resolvers.at(-1)([]);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, "check(sent.at(-1)[0].CommandPalette.type === 'unregister');");
}

#[test]
fn palette_commands_require_deno_parent_and_application_state_permission() {
    use crate::yinz_exports::{check_command_permission, run_plugin_command};
    use yinz_types::data::{PermissionStatus, PermissionType, PluginCommand};
    let command = PluginCommand::CommandPalette(yinz_types::command_palette::Command::Register {
        static_actions: false,
        id: "id".into(), title: "Title".into(), registration: "1".into(),
    });
    let mut env = crate::test_env::test_env();
    assert_eq!(check_command_permission(&env, &command), (PermissionStatus::Denied, Some(PermissionType::ReadApplicationState)));
    *env.permissions.lock().unwrap() = Some(std::collections::HashSet::from([PermissionType::ReadApplicationState]));
    assert!(run_plugin_command(&mut env, command.clone()).unwrap_err().to_string().contains("Deno"));
    assert!(yinz_plugin_api::plugin_command::ProtobufPluginCommand::try_from(command.clone()).is_err());
    env.deno_generation = Some(2);
    env.deno_workers = Some(Arc::new(background_worker::Workers::new(1)));
    assert!(run_plugin_command(&mut env, command).unwrap_err().to_string().contains("Background workers"));
}

#[test]
fn palette_sdk_return_contract_and_private_commands() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      bind({load() {
        for (const command of ['CommandPalette', {CommandPalette:{type:'unregister',registration:'1'}}]) {
          let blocked = false;
          try { yinz.command(command); } catch (error) { blocked = error.message.includes('yinz.commandPalette'); }
          check(blocked);
        }
        globalThis.result = [];
        yinz.commandPalette.registerProvider({id:'test',title:'Test',provideActions:()=>result});
      }}); bridge('load',{},1);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      check(sent.filter(([c]) => c.CommandPalette).length === 1);
      globalThis.scope = {registration:sent.at(-1)[0].CommandPalette.registration,opening:1,view:0,revision:1};
      globalThis.context = {clientId:1,paneId:{Terminal:1},tabId:1,currentProject:null};
      globalThis.discover = () => bridge('palette',{type:'discover',scope,context,query:'',source:null},1);
      globalThis.execute = (handle, values) => bridge('palette',{type:'execute',scope,context,handle,values},1);
      result = [{id:'run',label:'Run',run:()=>result}]; discover();
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, "globalThis.runHandle = sent.at(-1)[0].CommandPalette.result.Ok.actions[0].handle; result = {type:'form',title:'Form',fields:[],onSubmit:()=>result}; execute(runHandle,null);");
    capture_commands(&mut runtime);
    run(&mut runtime, "globalThis.formHandle = sent.at(-1)[0].CommandPalette.result.Ok.view.handle;");
    for callback in ["execute(runHandle,null)", "execute(formHandle,{})"] {
        for value in ["undefined", "null", "true", "42", "'incidental'", "{}", "[]", "{type:'unknown'}"] {
            run(&mut runtime, &format!("result = {value}; {callback};"));
            capture_commands(&mut runtime);
            run(&mut runtime, "check(sent.at(-1)[0].CommandPalette.result.Ok.type === 'close');");
        }
        for kind in ["actions", "form", "text"] {
            run(&mut runtime, &format!("result = {{type:'{kind}'}}; {callback};"));
            capture_commands(&mut runtime);
            run(&mut runtime, "check(sent.at(-1)[0].CommandPalette.result.Err.includes('View title'));");
        }
        run(&mut runtime, &format!("result = {{type:'complete',notification:'Saved'}}; {callback};"));
        capture_commands(&mut runtime);
        run(&mut runtime, "check(sent.at(-1)[0].CommandPalette.result.Ok.notification === 'Saved');");
        for reason in ["undefined", "null"] {
            run(&mut runtime, &format!("result = Promise.reject({reason}); {callback};"));
            capture_commands(&mut runtime);
            run(&mut runtime, "check(sent.at(-1)[0].CommandPalette.result.Err === 'Palette callback failed');");
        }
    }
    for value in ["undefined", "null", "true", "{}", "{type:'actions',title:'View',actions:[]}"] {
        run(&mut runtime, &format!("result = {value}; discover();"));
        capture_commands(&mut runtime);
        run(&mut runtime, "check(sent.at(-1)[0].CommandPalette.result.Err.includes('100 actions'));");
    }
    run(&mut runtime, "result = []; discover();");
    capture_commands(&mut runtime);
    run(&mut runtime, "check(sent.at(-1)[0].CommandPalette.result.Ok.actions.length === 0);");
}

#[test]
fn palette_example_limits_remote_actions_and_registers_cancellation() {
    let mut runtime = sdk();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../example-plugins/command-palette.ts");
    let source = std::fs::read_to_string(&path).unwrap();
    let emitted = loader::transpile(&path, &source, false).unwrap().text.clone();
    let source = emitted.lines().filter(|line| !line.starts_with("import "))
        .collect::<Vec<_>>().join("\n")
        .replace("export default definePlugin", "globalThis.example = definePlugin");
    run(&mut runtime, &format!(r#"
      globalThis.cancellations = [];
      globalThis.cancelled = false;
      globalThis.records = Array.from({{length:105}}, (_,i)=>({{id:String(i),label:'Action '+i}}));
      records[100] = null;
      ((yinz, definePlugin) => {{ {source} }} )({{
        waitForPermissions: async()=>{{}},
        commandPalette: {{registerProvider(provider) {{ globalThis.provider = provider; }} }},
        webRequest() {{ return Object.assign(Promise.resolve({{status:200,body:JSON.stringify(records)}}), {{cancel() {{ cancelled = true; }} }}); }},
        decodeUtf8: value=>value,
      }}, p=>p);
      example.load({{url:'http://test'}});
    "#));
    run(&mut runtime, r#"
      provider.provideActions({query:'',onCancel(callback) { cancellations.push(callback); }}).then(actions=>{
        check(actions.length === 100 && actions.every((action,i)=>action.id === String(i)));
      });
    "#);
    run(&mut runtime, "check(cancellations.length === 1); cancellations[0](); check(cancelled);");
}

#[test]
fn palette_sdk_static_registration_discovery_aliases_and_client_execution() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      globalThis.runs = 0;
      globalThis.actions = [{id:'pdf',label:'Open PDF…',aliases:['reader','document'],
        async run(context) {
          runs++; await Promise.resolve();
          check(context.clientId === 2 && yinz.getPluginIds().client_id === 2);
          return {type:'actions',title:'Nested',actions:[{id:'child',label:'Child',aliases:['nested'],run(){}}]};
        }}, {id:'other',label:'Unrelated',run(){runs++;}}];
      bind({load() {
        for (const source of [{}, {actions:[],provideActions:()=>[]}, {actions:()=>[]},
            {actions:null}, {provideActions:[]}, {provideActions:null}, {actions:Array(101)}]) {
          let rejected = false;
          try { yinz.commandPalette.registerProvider({id:'bad',title:'Bad',...source}); }
          catch (_) { rejected = true; }
          check(rejected);
        }
        globalThis.register = () => yinz.commandPalette.registerProvider({id:'pdf',title:'PDF',actions});
        globalThis.unregister = register();
      }}); bridge('load',{},1);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      globalThis.registration = sent.at(-1)[0].CommandPalette;
      check(registration.staticActions === true);
      globalThis.scope = {registration:registration.registration,opening:1,view:0,revision:1};
      globalThis.context = {clientId:2,paneId:{Terminal:7},tabId:3,currentProject:null};
      bridge('palette',{type:'discover',scope,context,query:'no match',source:null},2);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      globalThis.reply = sent.at(-1)[0].CommandPalette.result.Ok;
      check(runs === 0 && reply.actions.length === 2);
      check(JSON.stringify(reply.actions[0].aliases) === '["reader","document"]');
      check(reply.actions[1].aliases.length === 0);
      bridge('palette',{type:'execute',scope:{...scope,revision:2},context,handle:reply.actions[0].handle,values:null},2);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      check(runs === 1);
      const view = sent.at(-1)[0].CommandPalette.result.Ok.view;
      check(view.source === null && view.actions[0].aliases[0] === 'nested');
      bridge('palette',{type:'disconnect'},2);
      bridge('palette',{type:'execute',scope:{...scope,revision:3},context,handle:reply.actions[0].handle,values:null},2);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      check(sent.at(-1)[0].CommandPalette.result.Err.includes('no longer available'));
      check(runs === 1);
      unregister(); unregister(); bridge('load',{},1);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      check(sent.filter(([c]) => c.CommandPalette?.type === 'unregister').length === 1);
      check(sent.at(-1)[0].CommandPalette.registration !== registration.registration);
      unregister();
    "#);
}

#[test]
fn palette_sdk_static_action_validation_uses_discovery_errors_and_utf8_bound() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      bind({load(){}}); bridge('load',{},1);
      globalThis.validateActions = actions => {
        bind({load() {
          globalThis.remove = yinz.commandPalette.registerProvider({id:'test',title:'Test',actions});
        }}); bridge('load',{},1);
      };
      globalThis.context = {clientId:1,paneId:{Terminal:1},tabId:1,currentProject:null};
      globalThis.action = {id:'pdf',label:'PDF',run(){}};
    "#);
    for (actions, valid) in [
        ("[]", true),
        ("Array.from({length:100},(_,i)=>({...action,id:String(i)}))", true),
        ("[action,action]", false),
        ("[{...action,run:null}]", false),
        ("[{...action,aliases:'reader'}]", false),
        ("[{...action,aliases:null}]", false),
        ("[{...action,aliases:[1]}]", false),
        ("[{...action,aliases:['']}]", false),
        ("[{...action,aliases:['   ']}]", false),
        ("[{...action,aliases:['\\n']}]", false),
        ("[{...action,aliases:['\\u001b']}]", false),
        ("[{...action,aliases:['読'.repeat(90000)]}]", false),
    ] {
        run(&mut runtime, &format!("validateActions({actions});"));
        capture_commands(&mut runtime);
        run(&mut runtime, r#"
          bridge('palette',{type:'discover',scope:{
            registration:sent.at(-1)[0].CommandPalette.registration,opening:1,view:0,revision:1
          },context,query:'',source:null},1);
        "#);
        capture_commands(&mut runtime);
        run(&mut runtime, &format!("check(Object.hasOwn(sent.at(-1)[0].CommandPalette.result, '{}')); remove();", if valid { "Ok" } else { "Err" }));
    }
    for extra_bytes in [0, 1] {
        run(&mut runtime, "globalThis.bounded = {...action,aliases:['読']}; validateActions([bounded]);");
        capture_commands(&mut runtime);
        run(&mut runtime, &format!(r#"
          {{
          const registration = sent.at(-1)[0].CommandPalette.registration;
          const handle = String(Number(registration) + 1);
          const display = {{type:'actions',actions:[{{id:'pdf',label:'PDF',aliases:['読'],description:'',handle}}]}};
          // JSON is ASCII except for one three-byte character (one UTF-16 code unit).
          bounded.aliases[0] += 'x'.repeat(256 * 1024 - JSON.stringify(display).length - 2 + {extra_bytes});
          bridge('palette',{{type:'discover',scope:{{registration,opening:1,view:0,revision:1}},
            context,query:'',source:null}},1);
          }}
        "#));
        capture_commands(&mut runtime);
        run(&mut runtime, &format!("check(Object.hasOwn(sent.at(-1)[0].CommandPalette.result, '{}')); remove();",
            if extra_bytes == 0 { "Ok" } else { "Err" }));
    }

}

#[test]
fn palette_sdk_completion_messages_validate_and_survive_early_close() {
    let mut runtime = sdk();
    run(&mut runtime, r#"
      globalThis.value = undefined;
      globalThis.closeEarly = false;
      bind({load() {
        yinz.commandPalette.registerProvider({id:'completion',title:'Completion',actions:[{
          id:'save',label:'Save report',async run(context) {
            if (closeEarly) context.close();
            await Promise.resolve();
            return value;
          }
        }]});
      }}); bridge('load',{},1);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      globalThis.scope = {registration:sent.find(([c]) => c.CommandPalette?.type === "register")[0].CommandPalette.registration,opening:1,view:0,revision:1};
      globalThis.context = {clientId:1,paneId:{Terminal:1},tabId:0,currentProject:null};
      bridge('palette',{type:'discover',scope,context,query:'',source:null},1);
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      globalThis.handle = sent.at(-1)[0].CommandPalette.result.Ok.actions[0].handle;
      globalThis.execute = () => bridge('palette',{type:'execute',scope:{...scope,revision:++scope.revision},context,handle,values:null},1);
      value = {type:'complete',notification:'Saved',view:{type:'text',title:'Report',text:'Contents'}};
      execute();
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      check(sent.at(-1)[0].CommandPalette.result.Ok.notification === 'Saved');
      check(sent.at(-1)[0].CommandPalette.result.Ok.view.text === 'Contents');
      closeEarly = true; execute();
    "#);
    capture_commands(&mut runtime);
    run(&mut runtime, r#"
      check(sent.at(-1)[0].CommandPalette.result.Ok.notification === 'Saved');
      check(sent.at(-1)[0].CommandPalette.result.Ok.view === null);
      closeEarly = false;
    "#);
    for text in [" ".to_string(), "bad\x1b".to_string(), "x".repeat(256 * 1024)] {
        run(&mut runtime, &format!("value = {{type:'complete',notification:{}}}; execute();", serde_json::to_string(&text).unwrap()));
        capture_commands(&mut runtime);
        run(&mut runtime, "check(typeof sent.at(-1)[0].CommandPalette.result.Err === 'string');");
    }
}

#[test]
fn palette_host_rejects_invalid_completion_and_releases_its_view() {
    use crate::yinz_exports::run_plugin_command;
    use yinz_bus::command_palette::{Delivery, Instruction};
    use yinz_bus::screen::ScreenInstruction;
    use yinz_runtime::channels::{unbounded, SenderWithContext};
    use yinz_types::command_palette::{Command, Message, Reply, Scope, View};
    use yinz_types::data::{PermissionType, PluginCommand};
    let mut env = crate::test_env::test_env();
    env.deno_generation = Some(2);
    *env.permissions.lock().unwrap() = Some(std::collections::HashSet::from([PermissionType::ReadApplicationState]));
    let (sender, screen) = unbounded();
    env.senders.to_screen = Some(SenderWithContext::new(sender));
    let (sender, plugin) = unbounded();
    env.senders.to_plugin = Some(SenderWithContext::new(sender));
    run_plugin_command(&mut env, PluginCommand::CommandPalette(Command::Reply {
        scope: Scope { registration: "1".into(), opening: 1, view: 0, revision: 2 },
        result: Ok(Reply::Complete { notification: "\x1b".into(), view: Some(View::Form {
            actions: vec![], confirmation: false,
            title: "Report".into(), fields: vec![], submit_label: "Save".into(), handle: "callback".into(),
        }) }),
    })).unwrap();
    assert!(matches!(plugin.try_recv().unwrap().0, PluginInstruction::DenoPalette(Delivery { message: Message::Release { handles }, .. }) if handles == ["callback"]));
    assert!(matches!(screen.try_recv().unwrap().0, ScreenInstruction::CommandPalette(Instruction::Command {
        command: Command::Reply { result: Err(_), .. }, owner, ..
    }) if owner.plugin_id == env.plugin_id && owner.generation == 2));
}
