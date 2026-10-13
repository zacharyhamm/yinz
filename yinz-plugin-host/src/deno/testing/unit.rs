use super::*;

macro_rules! builtin_unit_suites {
    ($($test:ident: $name:literal,)*) => {
        $(#[test]
        fn $test() {
            let started = Instant::now();
            eprintln!("builtin unit start: {}", $name);
            let report = run_named(Path::new("/unused"), $name, &PluginTestOptions {
                kind: PluginTestKind::Unit,
                ..Default::default()
            }, None, &AtomicBool::new(false)).unwrap();
            eprintln!("builtin unit done: {} {:?}", $name, started.elapsed());
            assert!(report.passed(), "{}: {report:#?}", $name);
        })*

        #[test]
        fn builtin_unit_suites_cover_the_catalog() {
            let mut covered = vec![$($name),*];
            let mut catalog = yinz_runtime::consts::BUILTIN_DENO_PLUGIN_NAMES.to_vec();
            catalog.push("workflow-view");
            covered.sort_unstable();
            catalog.sort_unstable();
            assert_eq!(covered, catalog);
        }
    };
}

builtin_unit_suites! {
    builtin_unit_compact_bar: "compact-bar",
    builtin_unit_tab_bar: "tab-bar",
    builtin_unit_status_bar: "status-bar",
    builtin_unit_context_menu: "context-menu",
    builtin_unit_agent_policy: "agent-policy",
    builtin_unit_agent_opencode: "agent-opencode",
    builtin_unit_swamp_bridge: "swamp-bridge",
    builtin_unit_swamp_workflow: "swamp-workflow",
    builtin_unit_agent_exit: "agent-exit",
    builtin_unit_mother_overview: "mother-overview",
    builtin_unit_session_palette: "session-palette",
    builtin_unit_workflow_view: "workflow-view",
}

#[test]
fn installed_unit_assertions_imports_timers_and_unstubbed_operations() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("math.ts"),
        "export const double = (n: number) => n * 2;",
    )
    .unwrap();
    std::fs::write(root.path().join("plugin.ts"), r#"
      import z from 'yinz:plugin';
      let text = 'waiting';
      export default {
        async load(config: Record<string,string>) {
          z.after(10, () => {text = config.answer; return true;});
          await z.sleep(10); text += ' slept'; z.requestRender();
        },
        update(event: unknown) { if (event === 'InputReceived') z.command('Notify', ['unexpected',null,null]); },
        render() {return text;}
      };
    "#).unwrap();
    let path = root.path().join("plugin.test.ts");
    std::fs::write(&path, r#"
      import {test, assert, assertEquals, assertThrows, assertRejects, loadPlugin} from 'yinz:testing';
      import z from 'yinz:plugin';
      import {double} from './math.ts';
      test('async assertions and controlled SDK timers', async () => {
        assertEquals(double(3), 6);
        assertThrows(() => {throw Error('sync');}, 'sync');
        await assertRejects(async () => {throw Error('async');}, 'async');
        assertThrows(() => assertEquals(new Uint8Array([1]),new Uint8Array([2])));
        const cyclic: {self?: unknown} = {}; cyclic.self = cyclic; assertEquals(cyclic,cyclic);
        const p = await loadPlugin('./plugin.ts', {configuration:{answer:'done'}});
        assertEquals(Date.now(), 0); assertEquals(z.instanceId, 'test');
        await assertRejects(() => z.sleep(1), 'host commands are unavailable');
        assert((await p.render()).includes('waiting'));
        await p.advance(9); assert((await p.render()).includes('waiting'));
        await p.advance(1); assert((await p.render()).includes('done slept'));
        assertEquals(Date.now(), 10);
        await assertRejects(() => p.event('InputReceived'),'unstubbed host operation Notify');
      });
    "#).unwrap();
    let report = run(&path, &Default::default(), None, &AtomicBool::new(false)).unwrap();
    assert!(report.passed(), "{report:#?}");
}

#[test]
fn installed_permissions_complete_fifo_without_letting_event_observers_steal_answers() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("plugin.ts"), r#"
      import yinz from 'yinz:plugin';
      const settled: string[] = [];
      let observed = false;
      export default {
        async load() {
          const first = yinz.waitForPermissions(['ReadApplicationState']);
          const second = yinz.waitForPermissions(['ChangeApplicationState']);
          await first; settled.push('granted');
          try { await second; } catch (_) { settled.push('denied'); }
        },
        update(event: unknown) {
          if (!observed && typeof event === 'object' && event !== null && 'PermissionRequestResult' in event) {
            observed = true;
            yinz.waitForPermissions(['OpenFiles']).then(() => { settled.push('observer'); yinz.requestRender(); });
          }
        },
        render() { return settled.join(','); },
      };
    "#).unwrap();
    let path = root.path().join("permissions.test.ts");
    std::fs::write(&path, r#"
      import {test, assertEquals, loadPlugin} from 'yinz:testing';
      test('permission events settle their original FIFO requests', async () => {
        const p = await loadPlugin('./plugin.ts', {replies:{RequestPluginPermissions:null}});
        const requests = () => p.commands.filter((command: any) => 'RequestPluginPermissions' in command);
        assertEquals(requests(), [{RequestPluginPermissions:['ReadApplicationState']}]);
        const dispatched: unknown[] = [];
        p.stub('RequestPluginPermissions', (permissions: unknown) => { dispatched.push(permissions); return null; });
        assertEquals(dispatched, []);
        await p.event({PermissionRequestResult:'Granted'});
        assertEquals(await p.render(), 'granted');
        assertEquals(dispatched, [['ReadApplicationState', 'ChangeApplicationState']]);
        assertEquals(requests().length, 2);
        await p.event({PermissionRequestResult:'Denied'});
        assertEquals(await p.render(), 'granted,denied');
        assertEquals(dispatched, [
          ['ReadApplicationState', 'ChangeApplicationState'],
          ['ReadApplicationState', 'OpenFiles'],
        ]);
        assertEquals(requests().length, 3);
        await p.event({PermissionRequestResult:'Granted'});
        assertEquals(await p.render(), 'granted,denied,observer');
        assertEquals(requests().length, 3);
      });
    "#).unwrap();
    let report = run(&path, &Default::default(), None, &AtomicBool::new(false)).unwrap();
    assert!(report.passed(), "{report:#?}");
}

#[test]
fn installed_permission_dispatch_errors_advance_without_granting_or_stealing_answers() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("plugin.ts"),
        r#"
      import z from 'yinz:plugin';
      const settled: string[] = [];
      let observed = false;
      function request(name: string, permission: any) {
        z.waitForPermissions([permission]).then(
          () => { settled.push(name + ':granted'); z.requestRender(); },
          error => { settled.push(name + ':' + error.message); z.requestRender(); },
        );
      }
      export default {
        load() {
          request('immediate', 'WriteFiles');
          request('first', 'ReadApplicationState');
          request('queued', 'RunCommands');
          request('last', 'ChangeApplicationState');
        },
        update(event: any) {
          if (event.PermissionRequestResult && !observed) {
            observed = true;
            request('observer', 'OpenFiles');
          }
        },
        render() { return settled.join(','); },
      };
    "#,
    )
    .unwrap();
    let path = root.path().join("permissions.test.ts");
    std::fs::write(&path, r#"
      import {test, assertEquals, loadPlugin} from 'yinz:testing';
      test('dispatch errors reject their own request and leave the FIFO running', async () => {
        const initial: unknown[] = [];
        const p = await loadPlugin('./plugin.ts', {replies:{RequestPluginPermissions:(permissions: string[]) => {
          initial.push(permissions);
          if (permissions.includes('WriteFiles')) throw Error('immediate failure');
          return null;
        }}});
        assertEquals(initial, [['WriteFiles'], ['ReadApplicationState']]);
        assertEquals(await p.render(), 'immediate:immediate failure');
        const later: unknown[] = [];
        p.stub('RequestPluginPermissions', (permissions: string[]) => {
          later.push(permissions);
          if (permissions.includes('RunCommands')) throw Error('queued failure');
          return null;
        });
        await p.event({PermissionRequestResult:'Granted'});
        assertEquals(await p.render(), 'immediate:immediate failure,first:granted,queued:Error: queued failure');
        assertEquals(later, [
          ['ReadApplicationState', 'RunCommands'],
          ['ReadApplicationState', 'ChangeApplicationState'],
        ]);
        await p.event({PermissionRequestResult:'Granted'});
        assertEquals(later[2], ['ReadApplicationState', 'ChangeApplicationState', 'OpenFiles']);
        await p.event({PermissionRequestResult:'Granted'});
        assertEquals(await p.render(), 'immediate:immediate failure,first:granted,queued:Error: queued failure,last:granted,observer:granted');
        assertEquals(p.commands.filter((command: any) => 'RequestPluginPermissions' in command).length, 5);
      });
      test('an unsolicited answer cannot grant an observer-created request', async () => {
        const p = await loadPlugin('./plugin.ts', {replies:{RequestPluginPermissions:() => {throw Error('offline');}}});
        p.stub('RequestPluginPermissions', null);
        await p.event({PermissionRequestResult:'Granted'});
        const before = 'immediate:offline,first:offline,queued:offline,last:offline';
        assertEquals(await p.render(), before);
        assertEquals(p.commands.filter((command: any) => 'RequestPluginPermissions' in command).at(-1), {RequestPluginPermissions:['OpenFiles']});
        await p.event({PermissionRequestResult:'Granted'});
        assertEquals(await p.render(), before + ',observer:granted');
      });
    "#).unwrap();
    let report = run(&path, &Default::default(), None, &AtomicBool::new(false)).unwrap();
    assert!(report.passed(), "{report:#?}");
}

#[test]
fn installed_runner_reports_type_errors_empty_filters_source_lines_and_hangs() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("errors.test.ts");
    for (source, expected) in [
        ("import {test} from 'yinz:testing'; const n: number = 'bad'; test('types',()=>{});", "TS2322"),
        ("import {test} from 'yinz:testing';\ntest('mapped', () => {\n throw Error('mapped failure');\n});", "errors.test.ts:3"),
        ("import {test} from 'yinz:testing'; test('hang',()=>{console.log('before timeout');while(true){}});", "deadline"),
        ("while(true){}", "deadline"),
        ("export const noTests = true;", "no matching tests"),
    ] {
        std::fs::write(&path, source).unwrap();
        // Compiler startup under the full gate is not the behavior under test.
        // Deliberate hangs still need a bounded, shorter deadline.
        let timeout_ms = if expected == "deadline" { 10_000 } else { 30_000 };
        let report = run(&path, &PluginTestOptions {timeout_ms, ..Default::default()}, None, &AtomicBool::new(false)).unwrap();
        assert!(!report.passed(), "{report:#?}");
        if source.contains("before timeout") { assert!(report.tests[0].output.contains("before timeout"), "{report:#?}"); }
        assert!(report.tests.iter().any(|test| test.error.as_ref().is_some_and(|error| error.contains(expected))), "expected {expected}: {report:#?}");
    }
    std::fs::write(
        &path,
        "import {test} from 'yinz:testing'; test('one',()=>{});",
    )
    .unwrap();
    let report = run(
        &path,
        &PluginTestOptions {
            filter: Some("absent".into()),
            ..Default::default()
        },
        None,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(!report.passed());
    let report = run(&path, &Default::default(), None, &AtomicBool::new(true)).unwrap();
    assert!(report.tests[0]
        .error
        .as_ref()
        .unwrap()
        .contains("cancelled"));
    assert!(
        checker::check_semantic_source(&path, "import {test} from 'yinz:testing';").is_err(),
        "production checker cannot import test privileges"
    );
}

#[test]
fn installed_tests_confine_imports_and_bound_api_grants() {
    checker::check_semantic_source(Path::new("/tmp/rpc.ts"), include_str!("rpc.ts")).unwrap();
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("escape.test.ts");
    std::fs::write(
        &path,
        "import {test} from 'yinz:testing'; import '../outside.ts'; test('escape',()=>{});",
    )
    .unwrap();
    let report = run(&path, &Default::default(), None, &AtomicBool::new(false)).unwrap();
    assert!(!report.passed());
    std::fs::write(
        &path,
        "import {test} from 'yinz:testing'; test('grant', {grants:['RunCommands']},()=>{});",
    )
    .unwrap();
    let report = run(
        &path,
        &Default::default(),
        Some(&[]),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(report.tests[0].error.as_ref().unwrap().contains("exceed"));
}

#[test]
fn adjacent_tests_accept_all_deno_entrypoints_and_preserve_directory_restrictions() {
    let root = tempfile::tempdir().unwrap();
    let expected: Vec<_> = ["plugin.test.ts", "plugin.integration.test.ts"]
        .map(|name| root.path().join(name))
        .into();
    for path in &expected {
        std::fs::write(path, "").unwrap();
    }
    for extension in ["ts", "mts", "js", "mjs", "wasm", "lua", "tsx", "txt"] {
        let path = root.path().join(format!("plugin.{extension}"));
        std::fs::write(&path, "").unwrap();
        let mut found = vec![];
        discover(&path, root.path(), &mut found).unwrap();
        if matches!(extension, "ts" | "mts" | "js" | "mjs") {
            assert_eq!(found, expected, "{extension}");
        } else {
            assert!(found.is_empty(), "{extension}: {found:?}");
        }
        found.clear();
        discover(&path, root.path().parent().unwrap(), &mut found).unwrap();
        assert!(found.is_empty(), "adjacency stays in the selected root");
    }
    for path in &expected {
        let mut found = vec![];
        discover(path, root.path(), &mut found).unwrap();
        assert_eq!(found, std::slice::from_ref(path));
    }
}

#[test]
fn disposal_cancels_only_the_owning_instance_without_the_parent_token() {
    use crate::{deno::DenoPlugin, plugin_map::RunningPlugin, test_env::test_env};
    use std::sync::Arc;

    for disposal in ["wasm", "shared deno", "direct deno"] {
        let mut deno = DenoPlugin::new(
            "export default {};",
            Path::new("/tmp/disposal.ts"),
            test_env(),
            Arc::default(),
        )
        .unwrap();
        deno.env_mut().deno_generation = Some(deno.generation());
        let mut deno = Some(deno);
        let mut plugin = if disposal == "wasm" {
            let engine = wasmi::Engine::default();
            let module = wasmi::Module::new(&engine, b"\0asm\x01\0\0\0").unwrap();
            let mut store = wasmi::Store::new(&engine, test_env());
            let instance = wasmi::Linker::new(&engine)
                .instantiate_and_start(&mut store, &module)
                .unwrap();
            Some(RunningPlugin::new(store, instance, 24, 80))
        } else if disposal == "shared deno" {
            Some(RunningPlugin::new_deno(deno.take().unwrap(), 24, 80))
        } else {
            None
        };
        let env = plugin
            .as_ref()
            .map(RunningPlugin::env)
            .unwrap_or_else(|| deno.as_ref().unwrap().env());
        let parent_cancel = env.cancel.clone();
        let keys = [
            (
                env.plugin_id,
                env.client_id,
                env.deno_generation,
                "one".into(),
            ),
            (
                env.plugin_id,
                env.client_id,
                env.deno_generation,
                "two".into(),
            ),
            (
                env.plugin_id,
                crate::ClientId::new(2),
                env.deno_generation,
                "one".into(),
            ),
            (env.plugin_id, env.client_id, Some(u64::MAX), "one".into()),
            (
                crate::PluginId::new(2),
                env.client_id,
                env.deno_generation,
                "one".into(),
            ),
        ];
        let tokens = keys.each_ref().map(|_| Arc::new(AtomicBool::new(false)));
        RUNS.lock()
            .unwrap()
            .extend(keys.iter().cloned().zip(tokens.iter().cloned()));
        assert!(!parent_cancel.load(Ordering::Acquire));
        if disposal == "direct deno" {
            deno.as_mut().unwrap().dispose();
        } else {
            plugin.as_mut().unwrap().dispose();
        }
        assert!(!parent_cancel.load(Ordering::Acquire));
        for (index, token) in tokens.iter().enumerate() {
            assert_eq!(
                token.load(Ordering::Acquire),
                index < 2,
                "{disposal}: {index}"
            );
        }
        let mut runs = RUNS.lock().unwrap();
        for key in keys {
            assert!(
                runs.remove(&key).is_some(),
                "cleanup still belongs to the worker"
            );
        }
    }
}

#[test]
fn instance_cancellation_stops_a_running_worker_and_reaps_its_session() {
    use std::sync::{mpsc, Arc};

    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cancel.integration.test.ts");
    std::fs::write(&path, "import {test,session} from 'yinz:testing'; test('cancel',()=>{session.start(); console.log('worker is running'); while(true){}});").unwrap();
    let mut owned = session::OwnedSession::new().unwrap();
    let session_root = owned.root().to_owned();
    let pid: i32 = std::fs::read_to_string(session_root.join("server.pid"))
        .unwrap()
        .parse()
        .unwrap();
    let request = Request {
        path,
        root: root.path().to_owned(),
        index: Some(0),
        case: Some(Case {
            name: "cancel".into(),
            options: Default::default(),
        }),
        session: Some(session_root.clone()),
        journal: None,
    };
    let mut env = crate::test_env::test_env();
    env.plugin_id = crate::PluginId::new(3);
    let key = (
        env.plugin_id,
        env.client_id,
        env.deno_generation,
        "worker".into(),
    );
    let cancel = Arc::new(AtomicBool::new(false));
    RUNS.lock().unwrap().insert(key.clone(), cancel.clone());
    let (sender, receiver) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = invoke(&request, Duration::from_secs(60), &cancel);
        let cleanup = owned.shutdown();
        drop(owned);
        sender.send((result, cleanup)).unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    // Observe the worker's journal, while also detecting an early worker exit.
    while !std::fs::read_to_string(session_root.join("diagnostics"))
        .unwrap_or_default()
        .contains("worker is running")
    {
        assert!(
            Instant::now() < deadline,
            "worker never reached the running case"
        );
        match receiver.recv_timeout(Duration::from_millis(10)) {
            Err(mpsc::RecvTimeoutError::Timeout) => {},
            result => panic!("worker exited before cancellation: {result:?}"),
        }
    }
    assert!(!env.cancel.load(Ordering::Acquire));
    cancel_instance(&env);
    let (result, cleanup) = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
    worker.join().unwrap();
    cleanup.unwrap();
    let report: PluginTestResult = serde_json::from_value(result.unwrap()).unwrap();
    assert_eq!(report.status, "failed");
    assert!(report.error.unwrap().contains("cancelled"));
    assert!(report.output.contains("worker is running"));
    assert_eq!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH),
        "the disposable server must be reaped"
    );
    assert!(!session_root.exists());
    RUNS.lock().unwrap().remove(&key);
}

#[test]
fn installed_palette_bridge_drives_callbacks_and_lifecycle() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("palette.ts"), r#"
import z, {definePlugin} from 'yinz:plugin';
let runs = 0, unregister = () => {};
export default definePlugin({
  async load() {
    await z.waitForPermissions(['ReadApplicationState']);
    unregister = z.commandPalette.registerProvider({id:'example',title:'Example',actions:[{
      id:'open',label:'Open',description:'Description',aliases:['read'],
      async run(context) { await z.sleep(10); runs++; z.requestRender(); return {type:'text',title:'Result',text:context.clientId + ':' + runs}; },
    }]});
    z.commandPalette.registerProvider({id:'dynamic',title:'Dynamic',async provideActions(context) {
      if(context.query === 'fail') throw Error('callback failure');
      await z.sleep(10);
      return [{id:'dynamic',label:context.query + ':' + context.clientId,run(){}}];
    }});
  },
  pipe(message) {if(message.name === 'unregister') unregister();},
  render(){return String(runs);},
});
"#).unwrap();
    let path = root.path().join("palette.test.ts");
    let example = include_str!("testing.d.ts")
        .split(" * @example\n")
        .nth(1)
        .unwrap()
        .split(" */")
        .next()
        .unwrap()
        .lines()
        .map(|line| line.strip_prefix(" * ").unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&path, format!(r#"
import {{test,assert,assertEquals,assertRejects,loadPlugin}} from 'yinz:testing';
import type {{PaletteTestMessage}} from 'yinz:testing';
// @ts-expect-error lifecycle variants must not accept arbitrary fields
const malformed: PaletteTestMessage = {{type:'disconnect',scope:{{}}}};
// @ts-expect-error execute requires values and a handle
const incomplete: PaletteTestMessage = {{type:'execute',scope:{{registration:'1',opening:1,view:0,revision:1}}}};
test('documented discovery and execution', async()=>{{ {example} }});
"#) + r#"
test('palette lifecycle and async client context', async()=>{
  const p = await loadPlugin('./palette.ts',{replies:{RequestPluginPermissions:null,CommandPalette:null}});
  await p.event({PermissionRequestResult:'Granted'});
  const registrations = p.paletteCommands.filter(c=>c.type==='register');
  assertEquals(registrations.map(c=>c.staticActions),[true,false]);
  const context = {clientId:7,paneId:{Terminal:3},tabId:2,currentProject:'project'};
  let revision=0;
  const scope = (registration=registrations[0].registration) => ({registration,opening:1,view:0,revision:++revision});
  const discover = async()=>{
    await p.palette({type:'discover',scope:scope(),context,query:'open',source:null},7);
    const reply=p.paletteCommands.at(-1);
    assert(reply?.type==='reply' && 'Ok' in reply.result && reply.result.Ok.type==='actions');
    const action=reply.result.Ok.actions[0];
    assertEquals([action.id,action.label,action.description,action.aliases],['open','Open','Description',['read']]);
    return action.handle;
  };
  const execute = (handle:string)=>p.palette({type:'execute',scope:scope(),context,handle,values:null},7);
  const stale = async(handle:string)=>{
    await execute(handle);
    const reply=p.paletteCommands.at(-1);
    assert(reply?.type==='reply' && 'Err' in reply.result && reply.result.Err.includes('no longer available'));
  };
  let handle=await discover(); assertEquals(await p.render(),'0');
  let count=p.paletteCommands.length;
  await execute(handle); assertEquals(p.paletteCommands.length,count);
  await p.advance(10);
  const completed=p.paletteCommands.at(-1);
  assert(completed?.type==='reply');
  assertEquals(completed.result,{Ok:{type:'view',view:{type:'text',title:'Result',text:'7:1'}}});
  assertEquals(await p.render(),'1');
  await p.palette({type:'release',handles:[handle]},7); await stale(handle);
  for(const message of [{type:'dismiss',opening:1},{type:'disconnect'}] as const){
    handle=await discover(); await p.palette(message,7); await stale(handle);
  }
  const dynamic=registrations[1].registration;
  const pending=scope(dynamic);
  count=p.paletteCommands.length;
  await p.palette({type:'discover',scope:pending,context,query:'cancelled',source:null},7);
  await p.palette({type:'cancel',scope:pending},7);
  await p.advance(10); assertEquals(p.paletteCommands.length,count);
  await p.palette({type:'discover',scope:scope(dynamic),context,query:'query',source:null},7);
  await p.advance(10);
  const reply=p.paletteCommands.at(-1);
  assert(reply?.type==='reply' && 'Ok' in reply.result && reply.result.Ok.type==='actions');
  assertEquals(reply.result.Ok.actions[0].label,'query:7');
  await p.palette({type:'execute',scope:scope(dynamic),context,handle:reply.result.Ok.actions[0].handle,values:null},7);
  const asyncHandle=p.paletteCommands.at(-1); assert(asyncHandle?.type==='reply');
  assertEquals(asyncHandle.result,{Ok:{type:'close'}});
  await p.palette({type:'discover',scope:scope(dynamic),context,query:'fail',source:null},7);
  const failed=p.paletteCommands.at(-1); assert(failed?.type==='reply');
  assertEquals(failed.result,{Err:'callback failure'});
  for(const id of [-1,65536,1.5,NaN]) await assertRejects(()=>p.palette({type:'disconnect'},id),'valid client ID');
  await assertRejects(()=>p.palette({type:'discover',scope:scope(),context,query:'',source:null},1),'must match');
  handle=await discover(); await p.pipe({name:'unregister'}); await stale(handle);
  assert(p.paletteCommands.some(c=>c.type==='unregister'));
  const old=reply.result.Ok.actions[0].handle;
  await p.event('BeforeClose');
  await p.palette({type:'execute',scope:scope(dynamic),context,handle:old,values:null},7);
  const closed=p.paletteCommands.at(-1); assert(closed?.type==='reply' && 'Err' in closed.result);
  p.stub('CommandPalette',()=>{throw Error('bridge stub failure');});
  await assertRejects(()=>p.palette({type:'discover',scope:scope(),context,query:'',source:null},7),'bridge stub failure');
});
test('palette operations still require explicit stubs',async()=>{
  const p=await loadPlugin('./palette.ts',{replies:{RequestPluginPermissions:null}});
  await assertRejects(()=>p.event({PermissionRequestResult:'Granted'}),'unstubbed host operation CommandPalette');
});
"#).unwrap();
    let report = run(&path, &Default::default(), None, &AtomicBool::new(false)).unwrap();
    assert!(report.passed(), "{report:#?}");
}

#[test]
fn palette_test_types_accept_serialized_protocol() {
    use yinz_types::command_palette::{
        Action, Command, Context, Field, Message, Reply, Scope, View,
    };
    let scope = Scope {
        registration: "provider".into(),
        opening: 1,
        view: 2,
        revision: 3,
    };
    let context = Context {
        client_id: 7,
        pane_id: yinz_types::data::PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(8)),
        tab_id: 9,
        current_project: None,
    };
    let messages = [
        Message::Discover {
            scope: scope.clone(),
            context: context.clone(),
            query: "query".into(),
            source: None,
        },
        Message::Execute {
            scope: scope.clone(),
            context,
            handle: "handle".into(),
            values: None,
        },
        Message::Cancel {
            scope: scope.clone(),
        },
        Message::Release {
            handles: vec!["handle".into()],
        },
        Message::Dismiss { opening: 1 },
        Message::Disconnect,
    ];
    let actions = vec![Action {
        id: "open".into(),
        label: "Open".into(),
        description: String::new(),
        aliases: vec![],
        handle: "handle".into(),
    }];
    let mut commands = vec![
        Command::Register {
            id: "provider".into(),
            title: "Provider".into(),
            registration: "provider".into(),
            static_actions: true,
        },
        Command::Unregister {
            registration: "provider".into(),
        },
        Command::Suspend {
            scope: scope.clone(),
            suspended: true,
        },
        Command::Close {
            scope: scope.clone(),
        },
        Command::Reply {
            scope: scope.clone(),
            result: Err("failure".into()),
        },
    ];
    for reply in [
        Reply::Actions {
            actions: actions.clone(),
        },
        Reply::Fields {
            values: BTreeMap::from([("directory".into(), "/tmp".into())]),
        },
        Reply::Close,
        Reply::Complete {
            notification: "Saved".into(),
            view: None,
        },
        Reply::View {
            view: View::Actions {
                initial_query: String::new(),
                title: "Actions".into(),
                actions,
                source: Some("source".into()),
            },
        },
        Reply::View {
            view: View::Text {
                title: "Text".into(),
                text: "Body".into(),
            },
        },
        Reply::View {
            view: View::Form {
                actions: vec![Action {
                    id: "configure".into(),
                    label: "Configure".into(),
                    description: String::new(),
                    aliases: vec![],
                    handle: "secondary".into(),
                }],
                confirmation: false,
                title: "Form".into(),
                submit_label: "Submit".into(),
                handle: "submit".into(),
                fields: vec![
                    Field::Directory {
                        id: "directory".into(),
                        label: "Directory".into(),
                        initial_value: "/tmp".into(),
                    },
                    Field::Text {
                        id: "text".into(),
                        label: "Text".into(),
                        initial_value: String::new(),
                    },
                    Field::Choice {
                        id: "choice".into(),
                        label: "Choice".into(),
                        options: vec!["one".into()],
                        initial_value: None,
                    },
                ],
            },
        },
    ] {
        commands.push(Command::Reply {
            scope: scope.clone(),
            result: Ok(reply),
        });
    }
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("protocol.test.ts");
    std::fs::write(&path, format!(r#"
import {{test,assertEquals,type PaletteTestMessage,type PaletteTestCommand}} from 'yinz:testing';
const messages: PaletteTestMessage[] = {};
const commands: PaletteTestCommand[] = {};
test('Rust protocol matches test declarations',()=>{{assertEquals(messages.length,6);assertEquals(commands.length,12);}});
"#, serde_json::to_string(&messages).unwrap(), serde_json::to_string(&commands).unwrap())).unwrap();
    let report = run(&path, &Default::default(), None, &AtomicBool::new(false)).unwrap();
    assert!(report.passed(), "{report:#?}");
}

#[test]
fn awaited_picker_harness_uses_results_cancellation_and_deadlines() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("picker.ts"), r#"
import z from 'yinz:plugin';
let text = '', pending: ReturnType<typeof z.pick> | undefined;
export default {
  load() {},
  async pipe(message: {name: string}) {
    if (message.name === 'cancel') { pending?.cancel(); return; }
    try {
      pending = z.pick({kind:'directory', roots:['/home'], ...(message.name === 'timeout' ? {timeoutMs:10} : {})});
      const folder = await pending;
      if (folder === null) { text = 'cancelled'; return; }
      text = 'folder:' + folder;
      pending = z.pick({kind:'file', roots:[folder], extensions:['pdf']});
      text = 'file:' + await pending;
    } catch (error) { text = (error as Error).name + ':' + (error as Error).message; }
  },
  render() { return text; }
};
"#).unwrap();
    let path = root.path().join("picker.test.ts");
    std::fs::write(&path, r#"
import {test,assert,assertEquals,loadPlugin} from 'yinz:testing';
test('sequential results and stale replies', async()=>{
  const p=await loadPlugin('./picker.ts');
  await p.pipe({name:'start'});
  assertEquals(p.pickRequests.length,1);
  await p.completePick(p.pickRequests[0].request_id,'/home/Papers.pdf');
  assertEquals(p.pickRequests.length,2);
  assertEquals(p.pickRequests[1].options.roots,['/home/Papers.pdf']);
  await p.completePick(p.pickRequests[0].request_id,'/stale');
  await p.completePick(p.pickRequests[1].request_id,'/home/Papers.pdf/a.pdf');
  assert((await p.render()).includes('file:/home/Papers.pdf/a.pdf'));
});
test('escape', async()=>{
  const p=await loadPlugin('./picker.ts'); await p.pipe({name:'start'});
  await p.completePick(p.pickRequests[0].request_id,null);
  assert((await p.render()).includes('cancelled')); assertEquals(p.pickRequests.length,1);
});
test('failure', async()=>{
  const p=await loadPlugin('./picker.ts'); await p.pipe({name:'start'});
  await p.failPick(p.pickRequests[0].request_id,'fzf is not installed');
  assert((await p.render()).includes('fzf is not installed'));
});
test('client disconnect settles the pending promise', async()=>{
  const p=await loadPlugin('./picker.ts'); await p.pipe({name:'start'});
  await p.failPick(p.pickRequests[0].request_id,'picker client disconnected');
  assertEquals(await p.render(),'Error:picker client disconnected');
  await p.pipe({name:'cancel'});
  await p.completePick(p.pickRequests[0].request_id,'/stale');
  assertEquals(await p.render(),'Error:picker client disconnected');
  assertEquals(p.pickRequests.length,1);
});
test('escape at the second step', async()=>{
  const p=await loadPlugin('./picker.ts'); await p.pipe({name:'start'});
  await p.completePick(p.pickRequests[0].request_id,'/home/folder');
  await p.completePick(p.pickRequests[1].request_id,null);
  assert((await p.render()).includes('file:null')); assertEquals(p.pickRequests.length,2);
});
test('cancel', async()=>{
  const p=await loadPlugin('./picker.ts'); await p.pipe({name:'start'}); await p.pipe({name:'cancel'});
  assert((await p.render()).includes('AbortError'));
});
test('deadline', async()=>{
  const p=await loadPlugin('./picker.ts'); await p.pipe({name:'timeout'}); await p.advance(10);
  assert((await p.render()).includes('TimeoutError'));
});
"#).unwrap();
    let report = run(&path, &Default::default(), None, &AtomicBool::new(false)).unwrap();
    assert!(report.passed(), "{report:#?}");
}
