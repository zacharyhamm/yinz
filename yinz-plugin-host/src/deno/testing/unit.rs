use super::*;

#[test]
fn installed_builtin_unit_suites() {
    for name in yinz_runtime::consts::BUILTIN_DENO_PLUGIN_NAMES
        .iter()
        .copied()
        .chain(["workflow-view"])
    {
        let report = run_named(
            Path::new("/unused"),
            name,
            &PluginTestOptions {
                kind: PluginTestKind::Unit,
                ..Default::default()
            },
            None,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(report.passed(), "{name}: {report:#?}");
    }
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
        load(config: Record<string,string>) { z.after(10, () => {text = config.answer; return true;}); },
        update(event: unknown) { if (event === 'InputReceived') z.command('Notify', ['unexpected',null,null]); },
        render() {return text;}
      };
    "#).unwrap();
    let path = root.path().join("plugin.test.ts");
    std::fs::write(&path, r#"
      import {test, assert, assertEquals, assertThrows, assertRejects, loadPlugin} from 'yinz:testing';
      import {double} from './math.ts';
      test('async assertions and controlled SDK timers', async () => {
        assertEquals(double(3), 6);
        assertThrows(() => {throw Error('sync');}, 'sync');
        await assertRejects(async () => {throw Error('async');}, 'async');
        assertThrows(() => assertEquals(new Uint8Array([1]),new Uint8Array([2])));
        const cyclic: {self?: unknown} = {}; cyclic.self = cyclic; assertEquals(cyclic,cyclic);
        const p = await loadPlugin('./plugin.ts', {configuration:{answer:'done'}});
        assert((await p.render()).includes('waiting'));
        await p.advance(9); assert((await p.render()).includes('waiting'));
        await p.advance(1); assert((await p.render()).includes('done'));
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
