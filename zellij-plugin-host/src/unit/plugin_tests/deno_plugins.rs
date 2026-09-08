//! Real JS/TS files through the plugin thread and shared host command handlers.
//! Synchronization observes guest progress, never elapsed time or render counts.

use super::*;
use std::time::Duration;
use zellij_types::internal_ids::{PluginId, TabId};

const CLIENT: ClientId = ClientId::new(1);
const PLUGIN: PluginId = PluginId::new(0);
const SIZE: Size = Size { cols: 80, rows: 10 };

#[test]
fn deno_headless_hooks_deliver_the_first_payload_after_async_permissions_and_reload() {
    use zellij_types::input::hooks::HookPoint;
    let harness = Harness::new(
        "ts",
        r#"
let ready;
zellij.on('Hook', async event => {
  await ready;
  if (event.point !== 'pane_exit' || event.payload.exit_code !== 0 || !event.payload.held) throw Error('hook wire payload');
  zellij.command('RenameTabWithId', [1, 'deno hook:' + event.payload.pane_id]);
});
export default {load() {zellij.subscribe(['Hook']); ready = zellij.waitForPermissions(['ChangeApplicationState']); return ready;}};
"#,
        PermissionStatus::Granted,
    );
    super::lua_plugins::bind_hook(
        &harness.sender,
        HookPoint::PaneExit,
        run_plugin(&harness.path),
    );
    let fire = |id| {
        harness
            .sender
            .send(PluginInstruction::RunHook {
                point: HookPoint::PaneExit,
                pane_id: Some(PaneId::Terminal(crate::TerminalId::new(id))),
                payload: super::lua_plugins::pane_exit_payload(id),
                client_id: CLIENT,
            })
            .unwrap()
    };
    let observed = |name: &str| {
        harness.log.wait_until("the headless Deno hook changed the tab", std::time::Duration::from_secs(20), |log| {
        log.iter().any(|instruction| matches!(instruction, ScreenInstruction::RenameTabWithId(_, bytes, _) if bytes == name.as_bytes())).then_some(())
    })
    };
    // A headless launch can reuse a detached UI client's ID without inheriting its cancellation.
    harness
        .sender
        .send(PluginInstruction::RemoveClient(CLIENT))
        .unwrap();
    fire(7);
    observed("deno hook:7");
    fire(8);
    observed("deno hook:8");
    harness.reload();
    fire(9);
    observed("deno hook:9");
}

#[test]
fn deno_filesystem_helpers_keep_binary_data_confinement_and_config_revision_checks() {
    let harness = Harness::new(
        "ts",
        r#"
let state = 'starting';
export default {
  async load() { try {
    const assert = (ok) => { if (!ok) throw Error('filesystem host assertion'); };
    const refused = (call) => { try { call(); return false; } catch (_) { return true; } };
    assert(refused(() => zellij.fs.read('/host/input')));
    assert(refused(() => zellij.command('CheckPlugin', {name:'check.ts',source:'export default {};',mode:'Syntax'})));
    await zellij.waitForPermissions(['ReadFiles', 'WriteFiles']);
    zellij.fs.mkdir('/host/files/nested', {all: true});
    const bytes = new Uint8Array([0, 128, 255, 10]);
    assert(zellij.fs.write('/host/files/nested/b', bytes) === undefined);
    assert([...zellij.fs.read('/host/files/nested/b')].join() === [...bytes].join());
    assert(refused(() => zellij.fs.read('/host/files/nested/b', {maxBytes: 2})));
    zellij.fs.write('/host/files/nested/a', 'hé🙂');
    zellij.fs.write('/host/files/nested/a', '!', {append: true});
    assert(zellij.decodeUtf8(zellij.fs.read('/host/files/nested/a')) === 'hé🙂!');
    assert(zellij.fs.list('/host/files/nested').map(e => e.path.split('/').at(-1)).join() === 'a,b');
    assert(zellij.fs.metadata('/host/files/missing') === null);
    assert(zellij.fs.metadata('/host/files/nested/b').len === 4);
    for (const path of ['/host/../escape', '/host/link/secret', '/host/link/new/nested']) {
      assert(refused(() => zellij.fs.read(path)));
      assert(refused(() => zellij.fs.write(path, 'bad', {createDirs: true})));
      assert(refused(() => zellij.fs.rename('/host/files/nested/b', path)));
      assert(refused(() => zellij.fs.rename(path, '/host/files/from-outside')));
    }
    zellij.fs.rename('/host/files/nested/b', '/host/files/moved');
    assert(zellij.fs.metadata('/host/files/nested/b') === null);
    assert([...zellij.fs.read('/host/files/moved')].join() === [...bytes].join());
    zellij.fs.remove('/host/files', {recursive: true});
    assert(zellij.fs.metadata('/host/files') === null);
    const previous = zellij.projectConfig.read('.');
    assert(previous.source === null);
    assert(refused(() => zellij.projectConfig.write('.', 'agent_exit {}')));
    const current = zellij.projectConfig.write('.', 'agent_exit { default_branch "main"; }', previous.source);
    assert(current.config.default_branch === 'main');
    assert(refused(() => zellij.projectConfig.write('.', 'agent_exit {}', previous.source)));
    assert(zellij.projectConfig.read('.').source === current.source);
    state = 'filesystem and project config verified';
  } catch (error) {state = 'filesystem failed: ' + error.stack;} },
  render() { return state; }
};
"#,
        PermissionStatus::Granted,
    );
    let outside = tempdir().unwrap();
    std::fs::write(outside.path().join("secret"), "unchanged").unwrap();
    std::os::unix::fs::symlink(outside.path(), harness.directory.path().join("link")).unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "--quiet"])
        .arg(harness.directory.path())
        .status()
        .unwrap()
        .success());
    harness.load();
    let frame = harness.rendered("filesystem");
    assert!(
        frame.contains("filesystem and project config verified"),
        "{frame}"
    );
    assert_eq!(
        std::fs::read_to_string(outside.path().join("secret")).unwrap(),
        "unchanged"
    );
    assert!(!outside.path().join("new").exists());
}

#[test]
fn deno_data_helpers_preserve_values_rollback_conflicts_watch_disposal_and_reload_persistence() {
    data_helpers_persistence(None);
}

#[test]
#[ignore = "requires cargo x postgres-test --build and ZELLIJ_PLUGIN_DATA_TEST_URL"]
fn deno_data_helpers_on_postgres() {
    let directory = tempdir().unwrap();
    let url = std::env::var("ZELLIJ_PLUGIN_DATA_TEST_URL").expect("Postgres test URL");
    data_helpers_persistence(Some(DataAccess::postgres(
        &url,
        "deno_sdk_tests",
        directory.path(),
    )));
}

fn data_helpers_persistence(data_access: Option<DataAccess>) {
    let harness = Harness::with_services(
        "ts",
        r#"
let state = 'starting';
export default {
  async load() { try {
    await zellij.waitForPermissions(['PluginData']);
    const d = zellij.data;
    const assert = (ok) => { if (!ok) throw Error('data helper assertion'); };
    let denial = '';
    try {d.define('shared_item', {text:'Text'}, {namespace:'shared:deno_sdk'});} catch (error) {denial = String(error);}
    if (!denial.includes('SharedPluginData') && !denial.includes('shared_plugin_data')) throw Error('shared permission denial: ' + denial);
    await zellij.waitForPermissions(['SharedPluginData']);
    d.define('shared_item', {text:'Text'}, {namespace:'shared:deno_sdk'});
    d.apply([d.put('shared_item', {text:'shared from Deno'}, {uid:'shared'})], {namespace:'shared:deno_sdk'});
    d.define('items', {n:'Int', r:'Real', text:'Text', json:'Json', blob:'Blob', flag:'Bool', nullable:'Text', host_id:'Text', zellij_session_name:'Text'});
    const values = {n:9223372036854775807n, r:d.real(1), text:'hello', json:d.json({nested:[1,true]}),
      blob:new Uint8Array([0,128,255]), flag:false, nullable:null, host_id:'author-host', zellij_session_name:'author-session'};
    d.apply([d.put('items', values, {uid:'row', expectRev:0})]);
    const query = {entity:'items', filter:d.eq('uid','row')};
    const snapshot = d.fetch(query);
    const first = snapshot.records[0], second = d.fetch(query).records[0];
    assert(first.n === values.n && first.r === 1 && first.flag === false && first.nullable === null);
    assert(first.json === '{"nested":[1,true]}' && [...first.blob].join() === '0,128,255');
    assert(first.host_id === values.host_id && first.zellij_session_name === values.zellij_session_name);
    assert(snapshot.raw.rows[0][snapshot.raw.columns.indexOf('r')].Real === 1);
    assert(d.catalog().some(entry => entry.def.name === 'items'));
    d.apply([d.update('items', d.eq('uid','row'), {text:'second author'}, {expectRev:second.rev})]);
    let conflict = false;
    try { d.apply([d.put('items', values, {uid:'rollback'}), d.update('items', d.eq('uid','row'), {text:'stale author'}, {expectRev:first.rev})]); }
    catch (error) { conflict = d.isConflict(error); }
    assert(conflict && d.count({entity:'items'}) === 1 && d.fetch(query).records[0].text === 'second author');
    assert(d.fetch({entity:'items',filter:d.and(d.gt('n',0),d.not(d.eq('flag',true)),d.contains('text','author'))}).records.length === 1);
    let changes = 0;
    const watch = d.watch(query, snapshot => { changes++; assert(snapshot.records[0].text === 'watched'); });
    assert(watch.initial.records[0].text === 'second author');
    const changed = zellij.waitForEvent('PluginDataChanged', e => e.watch_id === watch.watchId);
    d.apply([d.update('items',d.eq('uid','row'),{text:'watched'})]);
    await changed;
    assert(changes === 1);
    watch.dispose(); watch.dispose();
    d.apply([d.update('items',d.eq('uid','row'),{text:'persisted'})]);
    // A later host event provides an ordering barrier without a sleep.
    state = 'data helpers: watch disposed'; zellij.requestRender();
    await zellij.waitForEvent('CustomMessage');
    assert(changes === 1 && d.fetch(query).records[0].text === 'persisted');
    state = 'data helpers verified';
  } catch (error) {state = 'data helpers failed: ' + error.stack;} },
  render() { return state; }
};
"#,
        Some(PermissionStatus::Granted),
        None,
        data_access,
    );
    harness.load();
    let frame = harness.rendered("data helpers");
    assert!(frame.contains("watch disposed"), "{frame}");
    harness.event(release_event());
    harness.rendered("data helpers verified");
    std::fs::write(
        &harness.path,
        r#"
let state = 'reloading';
export default {async load() {
  await zellij.waitForPermissions(['PluginData']);
  const row = zellij.data.fetch({entity:'items'}).records[0];
  if (row.text !== 'persisted' || row.n !== 9223372036854775807n) throw Error('lost persistence');
  state = 'same URL persistence verified';
}, render() {return state;}};
"#,
    )
    .unwrap();
    harness.reload();
    harness.rendered("same URL persistence verified");
    let other = harness.directory.path().join("other.ts");
    std::fs::write(
        &other,
        r#"
let state='starting';
export default {async load() {
  await zellij.waitForPermissions(['PluginData']);
  zellij.data.define('items', {text:'Text'});
  if (zellij.data.count({entity:'items'}) !== 0) throw Error('namespace leaked');
  await zellij.waitForPermissions(['SharedPluginData']);
  if (zellij.data.fetch({entity:'shared_item',namespace:'shared:deno_sdk'}).records[0].text !== 'shared from Deno') throw Error('shared namespace missing');
  state='different URL namespace verified';
},render(){return state;}};
"#,
    )
    .unwrap();
    load(&harness.sender, run_plugin(&other));
    harness.rendered("different URL namespace verified");
    let lua = harness.directory.path().join("shared_reader.lua");
    std::fs::write(
        &lua,
        r#"
local z = require('zellij')
local state = 'waiting'
local M = {}
function M.load()
  z.subscribe {'permission_request_result'}
  z.request_permission {'plugin_data', 'shared_plugin_data'}
end
z.on('permission_request_result', function()
  local rows = z.data.fetch {entity='shared_item', namespace='shared:deno_sdk'}
  assert(rows[1].text == 'shared from Deno')
  state = 'Lua reads Deno shared data'
end)
function M.render() return state end
return M
"#,
    )
    .unwrap();
    load(&harness.sender, run_plugin(&lua));
    harness.rendered("Lua reads Deno shared data");
}

#[test]
fn deno_integer_commands_keep_values_and_revision_guards_through_the_store() {
    let harness = Harness::new(
        "ts",
        r#"
let state = 'starting';
export default {
  async load() {
    await zellij.waitForPermissions(['PluginData']);
    const assert = (ok) => { if (!ok) throw Error('numeric host assertion'); };
    const attr = (name, kind) => ({name, kind, indexed: false, full_text: false, unique: false, required: false, default: null});
    zellij.command('DataDefineEntity', {namespace: null, allow_destructive: false,
      def: {name: 'numbers', version: 1, attrs: [attr('n', 'Int'), attr('r', 'Real')]}});
    const put = (n, r, expect_rev) => ({Put: {entity: 'numbers', uid: 'row',
      values: [['n', {Int: n}], ['r', {Real: r}]], expect_rev}});
    const apply = (...mutations) => zellij.command('DataApply', {namespace: null, mutations});
    const read = () => zellij.command('DataFetch', {request: {namespace: null, entity: 'numbers',
      filter: null, search: null, sort: [], select: [], limit: 100, offset: 0, after_id: null}});
    const field = (rows, name) => rows.rows[0][rows.columns.indexOf(name)];
    for (const n of [-9223372036854775808n, 9223372036854775807n, 9007199254740991, -9007199254740991]) {
      apply(put(n, 1.5, null));
      const rows = read();
      assert(field(rows, 'n').Int === n);
      assert(field(rows, 'r').Real === 1.5);
    }
    const before = read();
    const rev = field(before, 'rev').Int;
    for (const invalid of [NaN, Infinity, -Infinity, 0.5, -0.5, 9007199254740992, 18446744073709551616n]) {
      let refused = false;
      try { apply(put(10, 2, invalid)); } catch (_) { refused = true; }
      assert(refused && field(read(), 'rev').Int === rev);
    }
    apply(put(42, 2, rev));
    let conflict = false;
    try { apply(put(99, 3, rev)); } catch (error) { conflict = String(error).includes('conflict'); }
    assert(conflict && field(read(), 'n').Int === 42);
    state = 'integer host path verified';
  },
  render() { return state; }
};
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("integer host path verified");
}

#[test]
fn deno_exhaustion_disposes_the_guest_and_reload_recovers_in_a_live_session() {
    const CHILD: &str = "ZELLIJ_TEST_DENO_SMALL_BUDGET";
    if std::env::var_os(CHILD).is_none() {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "plugins::plugin_tests::deno_plugins::deno_exhaustion_disposes_the_guest_and_reload_recovers_in_a_live_session", "--nocapture"])
            .env(CHILD, "1").spawn().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(40);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "session quota child: {status}");
                return;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("session quota child exceeded deadline");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let harness = Harness::new(
        "ts",
        r#"
export default {
  load() { zellij.subscribe(['CustomMessage', 'BeforeClose']); },
  update(event) {
    if (event.CustomMessage) {
      try { new Uint8Array(32 * 1024 * 1024); } catch (_) {}
    }
    if (event === 'BeforeClose') throw Error('EXHAUSTED GUEST CLEANUP RAN');
  },
  render() { return 'quota guest ready'; }
};
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("quota guest ready");
    harness.event(release_event());
    harness.log.wait_until("allocation failure is reported", Duration::from_secs(10), |log| {
        log.iter().any(|instruction| matches!(instruction,
            ScreenInstruction::UpdatePluginLoadingStage(id, stage) if *id == PLUGIN && stage.is_error())).then_some(())
    });
    let survivor = harness.directory.path().join("survivor.js");
    std::fs::write(
        &survivor,
        "export default {render() {return 'other plugin survived quota';}};",
    )
    .unwrap();
    load(&harness.sender, run_plugin(&survivor));
    harness.rendered("other plugin survived quota");
    std::fs::write(
        &harness.path,
        "export default {render() {return 'quota replacement healthy';}};",
    )
    .unwrap();
    harness.reload();
    harness.rendered("quota replacement healthy");
}

// CustomMessage is ungated; InputReceived would require ReadApplicationState.
fn release_event() -> Event {
    Event::CustomMessage("acceptance-release".into(), String::new())
}

fn run_plugin(path: &std::path::Path) -> RunPluginOrAlias {
    RunPluginOrAlias::RunPlugin(RunPlugin {
        location: RunPluginLocation::File(path.to_owned()),
        configuration: PluginUserConfiguration::new(BTreeMap::from([(
            "greeting".into(),
            "configured".into(),
        )])),
        ..Default::default()
    })
}

fn load(sender: &SenderWithContext<PluginInstruction>, plugin: RunPluginOrAlias) {
    sender
        .send(PluginInstruction::Load(
            Some(false),
            false,
            false,
            Some("deno acceptance".into()),
            plugin,
            Some(TabId::new(1)),
            None,
            CLIENT,
            SIZE,
            None,
            None,
            false,
            None,
            None,
            None,
            None,
        ))
        .unwrap();
}

struct Harness {
    sender: SenderWithContext<PluginInstruction>,
    log: Watched<Vec<ScreenInstruction>>,
    directory: tempfile::TempDir,
    path: PathBuf,
    teardown: Option<Box<dyn FnOnce()>>,
    logger: Option<std::thread::JoinHandle<()>>,
}

impl Harness {
    fn new(extension: &str, source: &str, status: PermissionStatus) -> Self {
        Self::with_permission_reply(extension, source, Some(status))
    }

    fn with_permission_reply(
        extension: &str,
        source: &str,
        status: Option<PermissionStatus>,
    ) -> Self {
        Self::with_event_sender(extension, source, status, None)
    }

    fn with_event_sender(
        extension: &str,
        source: &str,
        status: Option<PermissionStatus>,
        event_sender: Option<SenderWithContext<PluginInstruction>>,
    ) -> Self {
        Self::with_services(extension, source, status, event_sender, None)
    }

    fn with_services(
        extension: &str,
        source: &str,
        status: Option<PermissionStatus>,
        event_sender: Option<SenderWithContext<PluginInstruction>>,
        data_access: Option<DataAccess>,
    ) -> Self {
        let directory = tempdir().unwrap();
        let path = directory.path().join(format!("acceptance.{extension}"));
        std::fs::write(&path, source).unwrap();
        let (sender, receiver, teardown) = create_plugin_thread_with_hooks_and_event_sender(
            Some(directory.path().to_owned()),
            None,
            None,
            Default::default(),
            event_sender.clone(),
            data_access,
        );
        let log = Watched::new(vec![]);
        let logger = match status {
            Some(status) => permissions_and_log_until_disconnect(
                &log,
                receiver,
                directory.path().join("permissions.kdl"),
                // An intercepted plugin bus must also carry permission answers:
                // otherwise they can overtake CachePluginEvents on that bus and
                // leave all subsequent events cached after the grant.
                event_sender.unwrap_or_else(|| sender.clone()),
                CLIENT,
                status,
            ),
            None => log_until_disconnect(&log, receiver),
        };
        sender.send(PluginInstruction::AddClient(CLIENT)).unwrap();
        // Callers may write imported dependencies before starting the load.
        Self {
            sender,
            log,
            directory,
            path,
            teardown: Some(teardown),
            logger: Some(logger),
        }
    }

    fn load(&self) {
        load(&self.sender, run_plugin(&self.path));
    }

    fn load_shared(&self) {
        load(
            &self.sender,
            RunPluginOrAlias::RunPlugin(RunPlugin {
                location: RunPluginLocation::File(self.path.clone()),
                configuration: PluginUserConfiguration::new(BTreeMap::from([(
                    "shared".into(),
                    "true".into(),
                )])),
                ..Default::default()
            }),
        );
    }

    fn event(&self, event: Event) {
        self.sender
            .send(PluginInstruction::Update(vec![(
                Some(PLUGIN),
                Some(CLIENT),
                event,
            )]))
            .unwrap();
    }

    fn reload(&self) {
        self.sender
            .send(PluginInstruction::Reload(
                Some(false),
                Some("deno acceptance".into()),
                run_plugin(&self.path),
                TabId::new(1),
                SIZE,
                None,
            ))
            .unwrap();
    }

    fn rendered(&self, text: &str) -> String {
        wait_for_render_containing(&self.log, text)
    }

    fn finish(mut self) -> Vec<ScreenInstruction> {
        self.stop();
        let log = self.log.lock().clone();
        log
    }

    fn stop(&mut self) {
        if let Some(teardown) = self.teardown.take() {
            teardown();
        }
        if let Some(logger) = self.logger.take() {
            logger.join().unwrap();
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if std::thread::panicking() {
            // Preserve Watched's timeout and frame diagnostics if the runtime is stuck.
            // nextest isolates each test in a process; request shutdown without joining
            // a worker that may be the reason the wait failed. Successful tests still join.
            let _ = self.sender.send(PluginInstruction::Exit);
        } else {
            self.stop();
        }
    }
}

const PERMISSIONS: &str = r#"
let state = "starting";
export default {
  async load(config) {
    zellij.subscribe(["PermissionRequestResult"]);
    // Try the protected operation both before and after the user's answer.
    try { zellij.command("RenameTabWithId", [1, "before-grant"]); } catch (_) {}
    // The helper and an independent observer must share the same host event.
    const answer = zellij.waitForEvent("PermissionRequestResult");
    let helper = "resolved";
    try { await zellij.waitForPermissions(["ChangeApplicationState"]); }
    catch (_) { helper = "rejected"; }
    const status = await answer;
    const ids = zellij.command("GetPluginIds");
    const objectIds = zellij.command({GetPluginIds: null});
    if (ids.plugin_id !== objectIds.plugin_id || ids.client_id !== objectIds.client_id) throw new Error("command forms disagree");
    let outcome = "returned";
    try { zellij.command("RenameTabWithId", [1, "after-grant"]); }
    catch (_) { outcome = "rejected"; }
    state = `${config.greeting} ${status} plugin=${ids.plugin_id} client=${ids.client_id} ${outcome} helper=${helper}`;
  },
  render() { return state; }
};
"#;

fn permission_round_trip(status: PermissionStatus, expected: &str) -> Vec<String> {
    let harness = Harness::new("js", PERMISSIONS, status);
    harness.load();
    let outcome = if status == PermissionStatus::Granted {
        "returned helper=resolved"
    } else {
        "rejected helper=rejected"
    };
    harness.rendered(&format!(
        "configured {expected} plugin=0 client=1 {outcome}"
    ));
    harness
        .finish()
        .iter()
        .filter_map(|instruction| match instruction {
            ScreenInstruction::RenameTabWithId(_, name, _) => {
                Some(String::from_utf8(name.clone()).unwrap())
            },
            _ => None,
        })
        .collect()
}

#[test]
#[serial(plugin_tests)]
fn deno_permission_grant_resumes_async_load_and_decodes_command_replies() {
    assert_eq!(
        permission_round_trip(PermissionStatus::Granted, "Granted"),
        ["after-grant"]
    );
}

#[test]
#[serial(plugin_tests)]
fn deno_permission_denial_never_routes_the_protected_command() {
    assert!(permission_round_trip(PermissionStatus::Denied, "Denied").is_empty());
}

#[test]
#[serial(plugin_tests)]
fn deno_async_commands_reject_without_permission() {
    let harness = Harness::new(
        "ts",
        r#"
let state = 'starting';
export default {
  async load() {
    const denied: string[] = [];
    for (const [name, promise] of [
      ['run', zellij.runCommand({command: ['/usr/bin/true']})],
      ['web', zellij.webRequest({url: 'https://example.com'})],
    ] as const) {
      try { await promise; } catch (error) {
        if (!(error instanceof Error) || !error.message.includes('permission')) throw error;
        denied.push(`${name} denied`);
      }
    }
    zellij.command('Unsubscribe', ['RunCommandResult', 'WebRequestResult']);
    state = denied.join(',');
  },
  render() { return state; }
};
"#,
        PermissionStatus::Denied,
    );
    harness.load();
    harness.rendered("run denied,web denied");
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn deno_typescript_awaits_an_event_and_timer_then_renders_asynchronously_after_resize() {
    let harness = Harness::new(
        "ts",
        r#"
import { zellij, definePlugin } from "zellij:plugin";
let state: string = "starting";
export default definePlugin({
  async load(config) {
    zellij.subscribe(["Key"]);
    const key = zellij.waitForEvent("Key");
    state = `${config.greeting} awaiting key`;
    zellij.requestRender();
    await key;
    await zellij.sleep(0);
    state = "key and timer completed";
  },
  async render(rows: number, columns: number) {
    await zellij.sleep(0);
    return `${state} ${rows}x${columns}`;
  }
});
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("configured awaiting key 10x80");
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
    harness.rendered("key and timer completed 10x80");
    harness
        .sender
        .send(PluginInstruction::Resize(PLUGIN, 113, 27))
        .unwrap();
    harness.rendered("key and timer completed 27x113");
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn deno_resize_discards_an_in_flight_async_render_with_the_old_dimensions() {
    let harness = Harness::new(
        "ts",
        r#"
let release: (() => void) | undefined;
export default {
  load() { zellij.subscribe(["Key"]); },
  async update() {
    await zellij.sleep(0);
    release?.();
    return true;
  },
  async render(rows: number, columns: number) {
    if (columns === 80) {
      const ready = new Promise<void>(resolve => { release = resolve; });
      // This instruction marks the point the old render is suspended.
      zellij.command("SetSelectable", false);
      await ready;
    }
    return `finished async render ${rows}x${columns}`;
  }
};
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.log.wait_until(
        "the original render is awaiting a key",
        default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false)))
                .then_some(())
        },
    );
    harness
        .sender
        .send(PluginInstruction::Resize(PLUGIN, 113, 27))
        .unwrap();
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
    harness.rendered("finished async render 27x113");
    let log = harness.finish();
    assert!(
        !rendered_assets(&log)
            .iter()
            .any(|(_, frame)| frame.contains("finished async render 10x80")),
        "the suspended render must not publish its obsolete dimensions"
    );
}

#[test]
#[serial(plugin_tests)]
fn deno_shared_promises_keep_their_origin_after_another_clients_event_and_timer() {
    let harness = Harness::new(
        "ts",
        r#"
import type { Event } from "zellij:plugin";
type Reply = { client_id: number };
const origin = () => (zellij.command("GetPluginIds") as Reply).client_id;
const waiting: number[] = [];
const completed: string[] = [];
let loads = 0;
export default {
  load() { loads++; zellij.subscribe(["Key"]); },
  async update(event: Event) {
    if (typeof event !== "object" || !("Key" in event)) return false;
    const started = origin();
    const release = zellij.waitForEvent("CustomMessage", ([name]) => name === "acceptance-release");
    waiting.push(started);
    zellij.requestRender();
    await release;
    const afterEvent = origin();
    await zellij.sleep(0);
    completed.push(`${started}->${afterEvent}->${origin()}`);
    return true;
  },
  async render() {
    const started = origin();
    await zellij.sleep(0);
    return `loads=${loads} waiting=${waiting.join(",")} done=${[...completed].sort().join(",")} render=${started}->${origin()}`;
  }
};
"#,
        PermissionStatus::Granted,
    );
    harness.load_shared();
    harness.rendered("loads=1 waiting= done= render=1->1");
    let other = ClientId::new(2);
    harness
        .sender
        .send(PluginInstruction::AddClient(other))
        .unwrap();
    for client in [CLIENT, other] {
        harness
            .sender
            .send(PluginInstruction::Update(vec![(
                Some(PLUGIN),
                Some(client),
                Event::Key(KeyWithModifier::new(BareKey::Char('a'))),
            )]))
            .unwrap();
    }
    harness.rendered("loads=1 waiting=1,2");
    // Both tasks resume during client 2's event; each must retain its own origin.
    harness
        .sender
        .send(PluginInstruction::Update(vec![(
            Some(PLUGIN),
            Some(other),
            release_event(),
        )]))
        .unwrap();
    harness.log.wait_until_described(
        "both clients' async callbacks and frames retain their origin",
        default_timeout(),
        |log| {
            [CLIENT, other]
                .into_iter()
                .all(|client| {
                    rendered_assets(log).iter().any(|(rendered_client, frame)| {
                        *rendered_client == client
                            && frame.contains("done=1->1->1,2->2->2")
                            && frame.contains(&format!("render={client}->{client}"))
                    })
                })
                .then_some(())
        },
        |log| describe_renders(log),
    );
    let log = harness.finish();
    assert!(
        !rendered_assets(&log)
            .iter()
            .any(|(_, frame)| frame.contains("loads=2")),
        "a shared plugin must keep one instance"
    );
}

#[test]
#[serial(plugin_tests)]
fn deno_shared_permission_wait_survives_an_attach_and_replays_the_queued_event() {
    let harness = Harness::with_permission_reply(
        "ts",
        r#"
import type { Event } from "zellij:plugin";
let loads = 0;
let keys = 0;
let state = "starting";
export default {
  async load() {
    loads++;
    zellij.subscribe(["Key"]);
    await zellij.waitForPermissions(["ChangeApplicationState"]);
    state = "granted";
  },
  update(event: Event) {
    if (typeof event === "object" && "Key" in event) { keys++; return true; }
    return false;
  },
  render() { return `loads=${loads} ${state} keys=${keys}`; }
};
"#,
        None,
    );
    harness.load_shared();
    harness.log.wait_until(
        "the shared plugin is waiting on the permission prompt",
        default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::RequestPluginPermissions(..)))
                .then_some(())
        },
    );
    let other = ClientId::new(2);
    harness
        .sender
        .send(PluginInstruction::AddClient(other))
        .unwrap();
    harness
        .sender
        .send(PluginInstruction::Update(vec![(
            Some(PLUGIN),
            Some(other),
            Event::Key(KeyWithModifier::new(BareKey::Char('a'))),
        )]))
        .unwrap();
    harness
        .sender
        .send(PluginInstruction::PermissionRequestResult(
            PLUGIN,
            Some(CLIENT),
            vec![PermissionType::ChangeApplicationState],
            PermissionStatus::Granted,
            Some(harness.directory.path().join("permissions.kdl")),
        ))
        .unwrap();
    harness.log.wait_until_described(
        "the attached clients see the single granted instance's queued key",
        default_timeout(),
        |log| {
            [CLIENT, other]
                .into_iter()
                .all(|client| {
                    rendered_assets(log).iter().any(|(rendered_client, frame)| {
                        *rendered_client == client && frame.contains("loads=1 granted keys=1")
                    })
                })
                .then_some(())
        },
        |log| describe_renders(log),
    );
    let log = harness.finish();
    assert_eq!(
        log.iter()
            .filter(|i| matches!(i, ScreenInstruction::RequestPluginPermissions(..)))
            .count(),
        1,
        "attaching must reuse the shared instance's permission wait"
    );
}

#[test]
#[serial(plugin_tests)]
fn deno_reload_rejects_an_old_host_timer_completion_but_accepts_the_current_one() {
    let source = r#"
import type { Event } from "zellij:plugin";
const label = "original";
let state = "pending";
let timers = 0;
let fences = 0;
export default {
  async load() {
    zellij.subscribe(["CustomMessage"]);
    await zellij.sleep(0);
    state = "completed";
  },
  update(event: Event) {
    if (typeof event !== "object") return false;
    if ("Timer" in event) { timers++; return true; }
    if ("CustomMessage" in event && event.CustomMessage[0] === "acceptance-release") {
      fences++;
      return true;
    }
    return false;
  },
  render() { return `${label} ${state} timers=${timers} fences=${fences}`; }
};
"#;
    let (to_proxy, outgoing): ChannelWithContext<PluginInstruction> = channels::unbounded();
    let to_proxy = SenderWithContext::new(to_proxy);
    let harness = Harness::with_event_sender("ts", source, None, Some(to_proxy.clone()));
    let completions = Watched::new(Vec::<PluginInstruction>::new());
    let proxy = std::thread::spawn({
        let completions = completions.clone();
        let sender = harness.sender.clone();
        move || loop {
            let (instruction, _) = outgoing
                .recv_timeout(FAKE_SCREEN_RECV_TIMEOUT)
                .expect("the timer proxy must be stopped after teardown");
            match instruction {
                PluginInstruction::Exit => break,
                timer @ PluginInstruction::DenoEvent(_, _, _, Event::Timer(_)) => {
                    completions.lock().push(timer);
                },
                // Capture the legacy envelope too, so losing the generation at the
                // SetTimeout producer fails explicitly instead of accidentally delivering it.
                PluginInstruction::Update(ref events)
                    if events
                        .iter()
                        .any(|(_, _, event)| matches!(event, Event::Timer(_))) =>
                {
                    completions.lock().push(instruction);
                },
                other => {
                    let _ = sender.send(other);
                },
            }
        }
    });
    harness.load();
    let old = completions.wait_until(
        "the old SDK sleep produced a real host completion",
        default_timeout(),
        |events| events.first().cloned(),
    );
    harness.rendered("original pending timers=0 fences=0");
    std::fs::write(
        &harness.path,
        source.replace("\"original\"", "\"replacement\""),
    )
    .unwrap();
    harness.reload();
    let current = completions.wait_until(
        "the replacement SDK sleep produced its own host completion",
        default_timeout(),
        |events| events.get(1).cloned(),
    );
    harness.rendered("replacement pending timers=0 fences=0");
    match (&old, &current) {
        (
            PluginInstruction::DenoEvent(old_id, old_client, old_generation, Event::Timer(_)),
            PluginInstruction::DenoEvent(new_id, new_client, new_generation, Event::Timer(_)),
        ) => {
            assert_eq!((*old_id, *old_client), (PLUGIN, CLIENT));
            assert_eq!((*new_id, *new_client), (PLUGIN, CLIENT));
            assert_ne!(
                old_generation, new_generation,
                "reload must change the timer's generation"
            );
        },
        _ => panic!("host SetTimeout must tag its completions: old={old:?}, current={current:?}"),
    }
    // Both timers have already fired, but neither completion has been delivered.
    // Deliver the old one followed by a FIFO event the new lifecycle acknowledges.
    harness.sender.send(old).unwrap();
    harness.event(release_event());
    let frame = harness.rendered("fences=1");
    assert!(frame.contains("replacement pending timers=0 fences=1"),
        "an old host timer must neither reach update nor resolve the replacement's SDK sleep: {frame}");
    harness.sender.send(current).unwrap();
    harness.rendered("replacement completed timers=1 fences=1");
    harness.finish();
    // Stop explicitly: event-loop wakers may retain the plugin-only proxy sender
    // after isolate disposal, so channel disconnect is not the proxy's lifetime.
    to_proxy.send(PluginInstruction::Exit).unwrap();
    proxy.join().unwrap();
}

#[test]
#[serial(plugin_tests)]
fn deno_reload_rereads_imports_and_cancels_the_old_event_await() {
    let harness = Harness::new(
        "ts",
        r#"
import { label } from "./dependency.ts";
let state: string = "starting";
export default {
  async load() {
    const event = zellij.waitForEvent("CustomMessage", ([name]) => name === "acceptance-release");
    zellij.command("Unsubscribe", ["CustomMessage"]);
    state = `${label} awaiting input`;
    zellij.requestRender();
    await event;
    // An observable host effect catches an old continuation even if its render is discarded.
    zellij.command("SetSelectable", label === "second");
    state = `${label} completed`;
  },
  render() { return state; }
};
"#,
        PermissionStatus::Granted,
    );
    let dependency = harness.directory.path().join("dependency.ts");
    std::fs::write(&dependency, "export const label: string = 'first';").unwrap();
    harness.load();
    harness.rendered("first awaiting input");
    std::fs::write(&dependency, "export const label: string = 'second';").unwrap();
    harness.reload();
    harness.rendered("second awaiting input");
    harness.event(release_event());
    harness.rendered("second completed");
    let log = harness.finish();
    assert!(!rendered_assets(&log)
        .iter()
        .any(|(_, frame)| frame.contains("first completed")));
    assert!(!log
        .iter()
        .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false))));
    assert!(log
        .iter()
        .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, true))));
    assert_eq!(
        log.iter()
            .filter(|i| matches!(i, ScreenInstruction::AddPlugin(..)))
            .count(),
        1,
        "reload must retain the existing pane"
    );
}

#[test]
#[serial(plugin_tests)]
fn deno_command_helper_completes_and_reload_rejects_the_old_host_reply() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("command.ts");
    let source = r#"
const label = 'original';
let state = 'pending';
let replies = 0;
let fences = 0;
export default {
  async load() {
    zellij.subscribe(['CustomMessage']);
    await zellij.waitForPermissions(['RunCommands']);
    const command = zellij.runCommand({command: ['/usr/bin/true'], context: {label}});
    zellij.command({Unsubscribe: ['RunCommandResult']});
    const result = await command;
    if (result.exitCode !== 7 || zellij.decodeUtf8(result.stdout) !== 'hé🙂'
        || result.stderr[0] !== 255 || result.context.label !== label) throw Error('incorrect result');
    state = 'completed';
  },
  update(event) {
    if (typeof event !== 'object') return false;
    if ('RunCommandResult' in event) replies++;
    if ('CustomMessage' in event) fences++;
    return true;
  },
  render() { return `${label} ${state} replies=${replies} fences=${fences}`; }
};
"#;
    std::fs::write(&path, source).unwrap();
    let (sender, jobs_receiver, receiver, teardown) =
        create_plugin_thread_with_background_jobs_receiver(Some(directory.path().to_owned()), None);
    let log = Watched::new(vec![]);
    let logger = permissions_and_log_until_disconnect(
        &log,
        receiver,
        directory.path().join("permissions.kdl"),
        sender.clone(),
        CLIENT,
        PermissionStatus::Granted,
    );
    let jobs = Watched::new(vec![]);
    let job_logger = log_until_disconnect(&jobs, jobs_receiver);
    sender.send(PluginInstruction::AddClient(CLIENT)).unwrap();
    let harness = Harness {
        sender,
        log,
        directory,
        path,
        teardown: Some(teardown),
        logger: Some(logger),
    };
    let completion = |index| {
        jobs.wait_until(
            "the helper dispatched its command",
            default_timeout(),
            |jobs| {
                jobs.iter()
                    .filter_map(|job| match job {
                        BackgroundJob::RunCommand(
                            plugin,
                            client,
                            _,
                            _,
                            _,
                            _,
                            context,
                            _,
                            generation,
                        ) => Some(PluginInstruction::for_plugin_event(
                            *plugin,
                            *client,
                            *generation,
                            Event::RunCommandResult(
                                Some(7),
                                "hé🙂".as_bytes().to_vec(),
                                vec![255],
                                context.clone(),
                            ),
                        )),
                        _ => None,
                    })
                    .nth(index)
            },
        )
    };
    harness.load();
    let old = completion(0);
    harness.rendered("original pending replies=0 fences=0");
    std::fs::write(&harness.path, source.replace("'original'", "'replacement'")).unwrap();
    harness.reload();
    let current = completion(1);
    harness.rendered("replacement pending replies=0 fences=0");
    match (&old, &current) {
        (
            PluginInstruction::DenoEvent(_, _, old_generation, _),
            PluginInstruction::DenoEvent(_, _, current_generation, _),
        ) => assert_ne!(old_generation, current_generation),
        _ => panic!("commands must carry their originating generation"),
    }
    harness.sender.send(old).unwrap();
    harness.event(release_event());
    harness.rendered("replacement pending replies=0 fences=1");
    harness.sender.send(current).unwrap();
    harness.rendered("replacement completed replies=1 fences=1");
    harness.finish();
    job_logger.join().unwrap();
}

#[test]
#[serial(plugin_tests)]
fn deno_removed_client_reattaches_with_fresh_state_and_without_its_old_awaits() {
    client_reattaches(false);
}

#[test]
#[serial(plugin_tests)]
fn deno_reused_client_id_reconnects_without_waiting_for_disposal() {
    client_reattaches(true);
}

fn client_reattaches(immediate: bool) {
    let harness = Harness::new(
        "ts",
        r#"
import type { Event } from "zellij:plugin";
let keys = 0;
let completed = 0;
export default {
  load() { zellij.subscribe(["Key"]); },
  async update(event: Event) {
    if (typeof event !== "object" || !("Key" in event)) return false;
    const ordinal = ++keys;
    const ready = zellij.waitForEvent("CustomMessage", ([name]) => name === "acceptance-release");
    zellij.requestRender();
    await ready;
    // Only the discarded instance reaches ordinal 2.
    zellij.command("SetSelectable", ordinal !== 2);
    completed++;
    return true;
  },
  render() { return `client=${zellij.getPluginIds().client_id} keys=${keys} completed=${completed}`; }
};
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("client=1 keys=0 completed=0");
    let other = ClientId::new(2);
    harness
        .sender
        .send(PluginInstruction::AddClient(other))
        .unwrap();
    harness.rendered("client=2 keys=0 completed=0");
    for _ in 0..2 {
        harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
    }
    harness.rendered("client=1 keys=2 completed=0");
    let before_reattach = harness.log.lock().len();
    harness
        .sender
        .send(PluginInstruction::RemoveClient(CLIENT))
        .unwrap();
    if !immediate {
        // The original case observes disposal before reattaching. The immediate
        // case deliberately sends RemoveClient/AddClient back to back.
        harness
            .sender
            .send(PluginInstruction::Update(vec![(
                Some(PLUGIN),
                Some(other),
                Event::Key(KeyWithModifier::new(BareKey::Char('b'))),
            )]))
            .unwrap();
        harness.rendered("client=2 keys=1 completed=0");
    }
    harness
        .sender
        .send(PluginInstruction::AddClient(CLIENT))
        .unwrap();
    let replacement_rendered = |expected: &str| {
        harness.log.wait_until_described(
            &format!("reattached client 1 renders {expected}"),
            default_timeout(),
            |log| {
                rendered_assets(&log[before_reattach..])
                    .iter()
                    .any(|(client, frame)| *client == CLIENT && frame.contains(expected))
                    .then_some(())
            },
            |log| describe_renders(log),
        );
    };
    replacement_rendered("client=1 keys=0 completed=0");
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('c'))));
    // The removed instance also rendered keys=1. Only the replacement's frame
    // proves its event waiter exists before broadcasting the release below.
    replacement_rendered("client=1 keys=1 completed=0");
    if immediate {
        harness
            .sender
            .send(PluginInstruction::Update(vec![(
                Some(PLUGIN),
                Some(other),
                Event::Key(KeyWithModifier::new(BareKey::Char('b'))),
            )]))
            .unwrap();
        harness.rendered("client=2 keys=1 completed=0");
    }
    harness
        .sender
        .send(PluginInstruction::Update(vec![(
            Some(PLUGIN),
            None,
            release_event(),
        )]))
        .unwrap();
    for client in [CLIENT, other] {
        harness.rendered(&format!("client={client} keys=1 completed=1"));
    }
    let log = harness.finish();
    assert!(
        !log.iter()
            .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false))),
        "the removed instance's pending callbacks must never run"
    );
    assert!(!rendered_assets(&log)
        .iter()
        .any(|(_, frame)| frame.contains("completed=2")));
}

#[test]
#[serial(plugin_tests)]
fn deno_disconnect_interrupts_attached_clients_infinite_load() {
    disconnect_attached_runaway(true);
}

#[test]
#[serial(plugin_tests)]
fn deno_disconnect_interrupts_attached_clients_infinite_callback() {
    disconnect_attached_runaway(false);
}

fn disconnect_attached_runaway(during_load: bool) {
    let source = r#"
let keys = 0;
function runaway() {
  if (zellij.getPluginIds().client_id !== 2) return;
  zellij.command('SetSelectable', false);
  for (;;) {}
}
export default {
  load() { zellij.subscribe(['Key']); LOAD },
  update(event) {
    if (!event.Key) return false;
    runaway();
    keys++;
    return true;
  },
  render() { return `client=${zellij.getPluginIds().client_id} keys=${keys}`; }
};
"#;
    let harness = Harness::new(
        "js",
        &source.replace("LOAD", if during_load { "runaway();" } else { "" }),
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("client=1 keys=0");
    let other = ClientId::new(2);
    harness
        .sender
        .send(PluginInstruction::AddClient(other))
        .unwrap();
    if !during_load {
        harness.rendered("client=2 keys=0");
        harness
            .sender
            .send(PluginInstruction::Update(vec![(
                Some(PLUGIN),
                Some(other),
                Event::Key(KeyWithModifier::new(BareKey::Char('a'))),
            )]))
            .unwrap();
    }
    harness.log.wait_until(
        "the attaching client's instance entered its loop",
        default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false)))
                .then_some(())
        },
    );
    harness
        .sender
        .send(PluginInstruction::RemoveClient(other))
        .unwrap();
    // This instance uses the same executor: a new frame proves cancellation freed it
    // and did not cancel the original client's token.
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('b'))));
    harness.rendered("client=1 keys=1");
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn deno_shared_instance_survives_its_owners_disconnect() {
    let harness = Harness::new(
        "js",
        r#"
let keys = 0;
export default {
  load() { zellij.subscribe(['Key']); },
  update(event) { if (event.Key) { keys++; return true; } },
  render() { return `shared keys=${keys}`; }
};
"#,
        PermissionStatus::Granted,
    );
    harness.load_shared();
    harness.rendered("shared keys=0");
    let other = ClientId::new(2);
    harness
        .sender
        .send(PluginInstruction::AddClient(other))
        .unwrap();
    harness
        .sender
        .send(PluginInstruction::RemoveClient(CLIENT))
        .unwrap();
    harness
        .sender
        .send(PluginInstruction::Update(vec![(
            Some(PLUGIN),
            Some(other),
            Event::Key(KeyWithModifier::new(BareKey::Char('a'))),
        )]))
        .unwrap();
    harness.rendered("shared keys=1");
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn deno_disconnect_during_pending_async_load_discards_its_continuation() {
    let source = r#"
const label = "original";
let state = `${label} load pending`;
export default {
  async load() {
    await zellij.waitForEvent("CustomMessage", ([name]) => name === "acceptance-release");
    zellij.command("SetSelectable", label === "replacement");
    state = `${label} load completed`;
  },
  render() { return state; }
};
"#;
    let harness = Harness::new("ts", source, PermissionStatus::Granted);
    harness.load();
    harness.rendered("original load pending");
    std::fs::write(
        &harness.path,
        source.replace("\"original\"", "\"replacement\""),
    )
    .unwrap();
    harness
        .sender
        .send(PluginInstruction::RemoveClient(CLIENT))
        .unwrap();
    harness
        .sender
        .send(PluginInstruction::AddClient(CLIENT))
        .unwrap();
    harness.rendered("replacement load pending");
    harness.event(release_event());
    harness.rendered("replacement load completed");
    let log = harness.finish();
    assert!(
        !log.iter()
            .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false))),
        "disconnect must revoke the suspended load's host access"
    );
    assert!(log
        .iter()
        .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, true))));
    assert!(!rendered_assets(&log)
        .iter()
        .any(|(_, frame)| frame.contains("original load completed")));
}

#[test]
#[serial(plugin_tests)]
fn deno_disconnect_interrupts_initial_load_before_the_instance_is_published() {
    let harness = Harness::new(
        "js",
        r#"
export default {
  load() {
    zellij.command("SetSelectable", false);
    for (;;) {}
  },
  render() { return "disconnected initialization was published"; }
};
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.log.wait_until(
        "the loader entered guest code before publishing the instance",
        default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false)))
                .then_some(())
        },
    );
    // Reattaching may provision this plugin for the new client. Its next load must
    // be safe, while the original isolate remains inside the already-running loop.
    std::fs::write(
        &harness.path,
        "export default { render() { return 'replacement after interrupted initialization'; } };",
    )
    .unwrap();
    harness
        .sender
        .send(PluginInstruction::RemoveClient(CLIENT))
        .unwrap();
    // A new client and plugin can progress only once the old initialization released
    // the plugin map. No release event or second host call can free that infinite loop.
    harness
        .sender
        .send(PluginInstruction::AddClient(ClientId::new(2)))
        .unwrap();
    let survivor = harness.directory.path().join("survivor.js");
    std::fs::write(
        &survivor,
        "export default { render() { return 'client survived pending initialization'; } };",
    )
    .unwrap();
    load(&harness.sender, run_plugin(&survivor));
    harness.rendered("client survived pending initialization");
    let log = harness.finish();
    assert!(!rendered_assets(&log)
        .iter()
        .any(|(_, frame)| frame.contains("disconnected initialization was published")));
}

#[test]
#[serial(plugin_tests)]
fn deno_shutdown_delivers_before_close_to_an_idle_plugin_once() {
    let harness = Harness::new(
        "ts",
        r#"
import type { Event } from "zellij:plugin";
export default {
  load() { zellij.subscribe(["BeforeClose"]); },
  update(event: Event) {
    if (event === "BeforeClose") {
      // Shutdown must still permit host calls from an idle plugin's close callback.
      zellij.command("SetSelectable", false);
    }
    return false;
  },
  render() { return "idle plugin ready for shutdown"; }
};
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("idle plugin ready for shutdown");
    // Exercise session shutdown, not an explicit Unload that happens to close a plugin.
    harness.sender.send(PluginInstruction::Exit).unwrap();
    harness.log.wait_until_described(
        "the idle plugin receives BeforeClose during shutdown",
        default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false)))
                .then_some(())
        },
        |log| describe_renders(log),
    );
    let log = harness.finish();
    assert_eq!(
        log.iter()
            .filter(|i| matches!(i, ScreenInstruction::SetSelectable(_, false)))
            .count(),
        1,
        "BeforeClose must be delivered exactly once"
    );
    assert!(
        !log.iter().any(
            |i| matches!(i, ScreenInstruction::UpdatePluginLoadingStage(id, stage)
        if *id == PLUGIN && stage.is_error())
        ),
        "normal shutdown must not report a guest crash"
    );
}

#[test]
#[serial(plugin_tests)]
fn deno_unload_disposes_pending_load_and_render_awaits() {
    let harness = Harness::new(
        "ts",
        r#"
export default {
  async load() {
    await zellij.waitForEvent("CustomMessage", ([name]) => name === "acceptance-release");
    zellij.command("SetSelectable", false);
  },
  async render() {
    const ready = zellij.waitForEvent("CustomMessage", ([name]) => name === "acceptance-release");
    // Load has already suspended; now render has registered its own await too.
    zellij.command("SetSelectable", true);
    await ready;
    zellij.command("SetSelectable", false);
    return "disposed render resumed";
  }
};
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.log.wait_until(
        "both guest callbacks have registered pending awaits",
        default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, true)))
                .then_some(())
        },
    );
    harness
        .sender
        .send(PluginInstruction::Unload(PLUGIN))
        .unwrap();
    let survivor = harness.directory.path().join("survivor.ts");
    std::fs::write(
        &survivor,
        r#"
let state = "survivor awaiting input";
export default {
  async load() {
    await zellij.waitForEvent("CustomMessage", ([name]) => name === "acceptance-release");
    state = "survivor received input";
  },
  render() { return state; }
};
"#,
    )
    .unwrap();
    load(&harness.sender, run_plugin(&survivor));
    harness.rendered("survivor awaiting input");
    // Broadcast the very event both discarded callbacks awaited, then observe its delivery.
    harness
        .sender
        .send(PluginInstruction::Update(vec![(
            None,
            Some(CLIENT),
            release_event(),
        )]))
        .unwrap();
    harness.rendered("survivor received input");
    let log = harness.finish();
    assert!(
        !log.iter()
            .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false))),
        "unloaded callbacks must not issue host commands"
    );
    assert!(!rendered_assets(&log)
        .iter()
        .any(|(_, frame)| frame.contains("disposed render resumed")));
}

#[test]
#[serial(plugin_tests)]
fn deno_rejected_promise_reports_the_original_typescript_dependency_line() {
    let harness = Harness::new(
        "ts",
        r#"
import { rejectAfterInput } from "./failure.ts";
import type { Event } from "zellij:plugin";
let state = "ready to reject";
export default {
  load() { zellij.subscribe(["Key"]); },
  update(event: Event) {
    if (typeof event !== "object" || !("Key" in event)) return false;
    state = "awaiting rejection";
    zellij.requestRender();
    return rejectAfterInput();
  },
  render() { return state; }
};
"#,
        PermissionStatus::Granted,
    );
    // These erased lines make a generated-JS line number observably wrong.
    let dependency = r#"interface ErasedBeforeFunction {
  first: string;
  second: number;
  third: boolean;
}
export async function rejectAfterInput(): Promise<never> {
  await zellij.waitForEvent("CustomMessage", ([name]) => name === "acceptance-release");
  throw new Error("acceptance rejected promise");
}
"#;
    let expected_line = dependency
        .lines()
        .position(|line| line.contains("throw new Error"))
        .unwrap()
        + 1;
    std::fs::write(harness.directory.path().join("failure.ts"), dependency).unwrap();
    harness.load();
    harness.rendered("ready to reject");
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
    harness.rendered("awaiting rejection");
    harness.event(release_event());
    let error = harness.log.wait_until_described(
        "the asynchronous rejection reaches the plugin's error UI",
        default_timeout(),
        |log| {
            log.iter().find_map(|i| match i {
                ScreenInstruction::UpdatePluginLoadingStage(id, stage)
                    if *id == PLUGIN && stage.is_error() =>
                {
                    Some(format!("{stage:?}"))
                },
                _ => None,
            })
        },
        |log| describe_renders(log),
    );
    assert!(error.contains("acceptance rejected promise"), "{error}");
    assert!(
        error.contains(&format!("failure.ts:{expected_line}:")),
        "the error must name the original TS dependency and source line: {error}"
    );
    harness.finish();
}

fn unload_runaway(source: &str, start_with_key: bool) {
    let harness = Harness::new("js", source, PermissionStatus::Granted);
    harness.load();
    if start_with_key {
        harness.rendered("ready to loop");
        harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
    }
    harness.log.wait_until(
        "the guest entered the callback that loops",
        default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false)))
                .then_some(())
        },
    );
    harness
        .sender
        .send(PluginInstruction::Unload(PLUGIN))
        .unwrap();
    // An unloaded generation no longer owns a pane to paint an error into. Observe
    // continued plugin-thread progress instead of waiting for its suppressed crash UI.
    let survivor = harness.directory.path().join("survivor.js");
    std::fs::write(
        &survivor,
        "export default { render() { return 'survivor is alive'; } };",
    )
    .unwrap();
    load(&harness.sender, run_plugin(&survivor));
    harness.rendered("survivor is alive");
    // The survivor may get a different executor. finish also joins the screen logger,
    // which cannot disconnect until the old executor releases its screen senders.
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn deno_reload_by_url_interrupts_an_infinite_update_and_runs_the_replacement() {
    let harness = Harness::new(
        "js",
        r#"
export default {
  load() { zellij.subscribe(["Key"]); },
  update() {
    zellij.command("SetSelectable", false);
    for (;;) {}
  },
  render() { return "original ready to loop"; }
};
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("original ready to loop");
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
    harness.log.wait_until(
        "the old instance entered its infinite update",
        default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false)))
                .then_some(())
        },
    );
    std::fs::write(
        &harness.path,
        r#"
let keys = 0;
export default {
  load() {
    zellij.subscribe(["Key"]);
    zellij.command("SetSelectable", true);
  },
  update() { keys++; return true; },
  render() { return `replacement alive keys=${keys}`; }
};
"#,
    )
    .unwrap();
    // Exercise location matching while the guest owns its instance lock. An ID-only
    // reload would bypass the lookup that formerly deadlocked before cancellation.
    harness
        .sender
        .send(PluginInstruction::ReloadPluginByUrl(run_plugin(
            &harness.path,
        )))
        .unwrap();
    harness.rendered("replacement alive keys=0");
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('b'))));
    harness.rendered("replacement alive keys=1");
    let log = harness.finish();
    assert!(
        log.iter()
            .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, true))),
        "the replacement must have a fresh cancellation token and working host commands"
    );
    assert_eq!(
        log.iter()
            .filter(|i| matches!(i, ScreenInstruction::AddPlugin(..)))
            .count(),
        1,
        "reload by URL must replace the running instance in its existing pane"
    );
    assert!(log.iter().any(|i| matches!(i, ScreenInstruction::PluginBytes(assets)
        if assets.iter().any(|asset| asset.plugin_id == PLUGIN && frame_of(&asset.bytes).contains("replacement alive keys=1")))),
        "the original plugin ID must identify the responsive replacement");
}

#[test]
#[serial(plugin_tests)]
fn deno_reload_by_url_delivers_idle_before_close_before_loading_the_replacement() {
    let harness = Harness::new(
        "js",
        r#"
export default {
  load() { zellij.subscribe(["BeforeClose"]); },
  update(event) {
    if (event === "BeforeClose") zellij.command("SetSelectable", false);
    return false;
  },
  render() { return "idle original ready for reload"; }
};
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("idle original ready for reload");
    std::fs::write(
        &harness.path,
        r#"
export default {
  load() { zellij.command("SetSelectable", true); },
  render() { return "idle replacement loaded"; }
};
"#,
    )
    .unwrap();
    harness
        .sender
        .send(PluginInstruction::ReloadPluginByUrl(run_plugin(
            &harness.path,
        )))
        .unwrap();
    harness.rendered("idle replacement loaded");
    let log = harness.finish();
    let selectable: Vec<bool> = log
        .iter()
        .filter_map(|i| match i {
            ScreenInstruction::SetSelectable(_, value) => Some(*value),
            _ => None,
        })
        .collect();
    assert_eq!(selectable, [false, true],
        "the idle original's BeforeClose must retain host access and run exactly once, before replacement load");
    assert!(
        !log.iter().any(
            |i| matches!(i, ScreenInstruction::UpdatePluginLoadingStage(id, stage)
        if *id == PLUGIN && stage.is_error())
        ),
        "reloading an idle plugin must not crash it"
    );
    assert_eq!(
        log.iter()
            .filter(|i| matches!(i, ScreenInstruction::AddPlugin(..)))
            .count(),
        1
    );
}

#[test]
#[serial(plugin_tests)]
fn deno_unload_interrupts_an_infinite_update_without_host_calls() {
    unload_runaway(
        r#"
export default {
  load() { zellij.subscribe(["Key"]); },
  update() { zellij.command("SetSelectable", false); for (;;) {} },
  render() { return "ready to loop"; }
};
"#,
        true,
    );
}

#[test]
#[serial(plugin_tests)]
fn deno_unload_interrupts_an_infinite_load_without_host_calls() {
    unload_runaway(
        r#"
export default {
  load() { zellij.command("SetSelectable", false); for (;;) {} }
};
"#,
        false,
    );
}

#[test]
fn deno_reload_disposes_host_watches_and_rejects_their_queued_deliveries() {
    let source = r#"
const label = 'old';
let changes = 0, fences = 0;
export default {
  async load() {
    // The proxy must forward the fence subscription before capturing DataChanged.
    zellij.subscribe(['CustomMessage']);
    await zellij.waitForPermissions(['PluginData']);
    const d = zellij.data;
    d.define('watched', {value:'Text'});
    d.watch({entity:'watched'}, () => {changes++; zellij.requestRender();});
    d.apply([d.put('watched', {value:label}, {uid:'one'})]);
  },
  update(event) {if (event.CustomMessage) {fences++; return true;}},
  render() {return `${label} changes=${changes} fences=${fences}`;}
};
"#;
    let (to_proxy, outgoing): ChannelWithContext<PluginInstruction> = channels::unbounded();
    let to_proxy = SenderWithContext::new(to_proxy);
    let harness = Harness::with_event_sender(
        "ts",
        source,
        Some(PermissionStatus::Granted),
        Some(to_proxy.clone()),
    );
    let replies = Watched::new(Vec::<PluginInstruction>::new());
    let proxy = std::thread::spawn({
        let replies = replies.clone();
        let sender = harness.sender.clone();
        move || loop {
            let (instruction, _) = outgoing.recv_timeout(FAKE_SCREEN_RECV_TIMEOUT).unwrap();
            match instruction {
                PluginInstruction::Exit => break,
                changed
                @ PluginInstruction::DenoEvent(_, _, _, Event::PluginDataChanged { .. }) => {
                    replies.lock().push(changed)
                },
                PluginInstruction::Update(ref events)
                    if events
                        .iter()
                        .any(|(_, _, event)| matches!(event, Event::PluginDataChanged { .. })) =>
                {
                    replies.lock().push(instruction)
                },
                other => {
                    let _ = sender.send(other);
                },
            }
        }
    });
    harness.load();
    let old = replies.wait_until("old host watch reply", Duration::from_secs(20), |events| {
        events.first().cloned()
    });
    harness.rendered("old changes=0 fences=0");
    std::fs::write(
        &harness.path,
        source.replace("label = 'old'", "label = 'new'"),
    )
    .unwrap();
    harness.reload();
    let current = replies.wait_until("new host watch reply", Duration::from_secs(20), |events| {
        events.get(1).cloned()
    });
    harness.rendered("new changes=0 fences=0");
    match (&old, &current) {
        (
            PluginInstruction::DenoEvent(_, _, old, _),
            PluginInstruction::DenoEvent(_, _, new, _),
        ) => assert_ne!(old, new),
        _ => panic!("data replies require generations: {old:?}, {current:?}"),
    }
    // Each synchronous apply emits one change before its following render tick.
    // A surviving old watch would have emitted a third change from the second apply.
    assert_eq!(replies.lock().len(), 2);
    harness.sender.send(old).unwrap();
    harness.event(release_event());
    harness.rendered("new changes=0 fences=1");
    harness.sender.send(current).unwrap();
    harness.rendered("new changes=1 fences=1");
    harness.finish();
    to_proxy.send(PluginInstruction::Exit).unwrap();
    proxy.join().unwrap();
}

#[test]
fn deno_hocus_registration_requires_the_real_message_grant() {
    let harness = Harness::new(
        "ts",
        r#"
let state = 'starting';
export default {
  async load() {
    let refusals = 0;
    const attempt = () => {
      try {zellij.hocus.tools({forecast:{handler:()=> 'unexpected'}});}
      catch (error) {if (String(error).includes('MessageAndLaunchOtherPlugins')) refusals++; else throw error;}
    };
    attempt();
    try {await zellij.waitForPermissions(['MessageAndLaunchOtherPlugins']);} catch (_) {}
    attempt();
    state = refusals === 2 ? 'registration denied before and after refusal' : 'unexpected registration';
  },
  render() {return state;}
};
"#,
        PermissionStatus::Denied,
    );
    load(
        &harness.sender,
        RunPluginOrAlias::RunPlugin(RunPlugin {
            location: RunPluginLocation::File(harness.path.clone()),
            configuration: PluginUserConfiguration::new(BTreeMap::from([
                ("hocus_extension_name".into(), "weather".into()),
                (
                    "hocus_extension_path".into(),
                    harness.path.display().to_string(),
                ),
            ])),
            ..Default::default()
        }),
    );
    harness.rendered("registration denied before and after refusal");
    harness.finish();
}

#[test]
fn deno_rpc_calls_deno_and_lua_peers_by_url_and_id() {
    for extension in ["ts", "lua"] {
        let harness = Harness::new(
            "ts",
            r#"
let state = 'starting';
zellij.rpc.handle('back', async (payload, call) => ({value:payload, from:call.source.Plugin}));
export default {
  async load(config) {try {
    await zellij.waitForPermissions(['MessageAndLaunchOtherPlugins']);
    const first = await zellij.rpc.call({pluginUrl:config.peer,pluginConfig:{greeting:'Oslo'},name:'echo',payload:'hello',timeoutMs:10000});
    if (first.greeting !== 'Oslo' || first.back.value !== 'hello' || first.back.from !== first.id) throw Error('URL/config/reverse RPC mismatch');
    const second = await zellij.rpc.call({pluginId:first.id,name:'echo',payload:'again',timeoutMs:10000});
    if (second.back.value !== 'again') throw Error('ID RPC mismatch');
    for (const name of ['missing', 'failure']) {
      try {await zellij.rpc.call({pluginId:first.id,name,timeoutMs:10000}); throw Error('unexpected success');}
      catch (error) {if (!String(error).includes(name === 'missing' ? 'no rpc handler' : 'peer failed')) throw error;}
    }
    state = 'RPC peer verified';
  } catch(error) {state = 'RPC failed: ' + error.stack;}},
  render() {return state;}
};
"#,
            PermissionStatus::Granted,
        );
        let peer = harness.directory.path().join(format!("peer.{extension}"));
        std::fs::write(&peer, if extension == "ts" { r#"
let ready, greeting;
zellij.rpc.handle('echo', async (payload, call) => {
  await ready;
  const back = await zellij.rpc.call({pluginId:call.source.Plugin,name:'back',payload,timeoutMs:10000});
  return {id:zellij.getPluginIds().plugin_id,greeting,back};
});
zellij.rpc.handle('failure', () => {throw Error('peer failed');});
export default {load(config) {greeting=config.greeting; ready=zellij.waitForPermissions(['MessageAndLaunchOtherPlugins']); return ready;}};
"# } else { r#"
local zellij = require('zellij')
local ready, greeting
local M = {}
function M.load(config)
  greeting = config.greeting
  ready = zellij.async.permissions({'message_and_launch_other_plugins'})
end
zellij.rpc.handle('echo', function(payload, call)
  zellij.await(ready)
  local back = zellij.await(zellij.rpc.call {plugin_id=call.source.Plugin,name='back',payload=payload,timeout=10})
  if back.error then error(back.error) end
  return {id=zellij.get_plugin_ids().plugin_id,greeting=greeting,back=back.result}
end)
zellij.rpc.handle('failure', function() error('peer failed') end)
return M
"# }).unwrap();
        load(
            &harness.sender,
            RunPluginOrAlias::RunPlugin(RunPlugin {
                location: RunPluginLocation::File(harness.path.clone()),
                configuration: PluginUserConfiguration::new(BTreeMap::from([(
                    "peer".into(),
                    format!("file:{}", peer.display()),
                )])),
                ..Default::default()
            }),
        );
        let frame = harness.rendered("RPC ");
        assert!(frame.contains("RPC peer verified"), "{extension}: {frame}");
        harness.finish();
    }
}

#[test]
fn deno_rpc_rejects_calls_without_message_permission() {
    let harness = Harness::new(
        "ts",
        r#"
let state = 'starting';
export default {async load() {
  let denied = 0;
  const attempt = async () => {
    try {await zellij.rpc.call({pluginId:7,name:'echo',timeoutMs:100});}
    catch (error) {if (String(error).includes('MessageAndLaunchOtherPlugins')) denied++; else throw error;}
  };
  await attempt();
  try {await zellij.waitForPermissions(['MessageAndLaunchOtherPlugins']);} catch (_) {}
  await attempt();
  state = denied === 2 ? 'RPC permission denied twice' : 'unexpected RPC permission';
},render() {return state;}};
"#,
        PermissionStatus::Denied,
    );
    harness.load();
    harness.rendered("RPC permission denied twice");
    harness.finish();
}

#[test]
fn deno_rpc_reload_ignores_old_replies_before_settling_the_new_call() {
    let source = r#"
const label = 'old';
let result = 'waiting', fences = 0;
zellij.rpc.handle('echo', () => label);
export default {
  async load() {
    zellij.subscribe(['CustomMessage']);
    await zellij.waitForPermissions(['MessageAndLaunchOtherPlugins']);
    result = await zellij.rpc.call({pluginId:zellij.getPluginIds().plugin_id,name:'echo'});
  },
  update(event) {if (event.CustomMessage) {fences++; return true;}},
  render() {return `${label} result=${result} fences=${fences}`;}
};
"#;
    let (to_proxy, outgoing): ChannelWithContext<PluginInstruction> = channels::unbounded();
    let to_proxy = SenderWithContext::new(to_proxy);
    let harness = Harness::with_event_sender(
        "ts",
        source,
        Some(PermissionStatus::Granted),
        Some(to_proxy.clone()),
    );
    let replies = Watched::new(Vec::<PluginInstruction>::new());
    let proxy = std::thread::spawn({
        let replies = replies.clone();
        let sender = harness.sender.clone();
        move || loop {
            let (instruction, _) = outgoing.recv_timeout(FAKE_SCREEN_RECV_TIMEOUT).unwrap();
            match instruction {
                PluginInstruction::Exit => break,
                PluginInstruction::MessageFromPlugin { ref message, .. }
                    if message.message_name == "zellij_rpc_reply" =>
                {
                    replies.lock().push(instruction)
                },
                other => {
                    let _ = sender.send(other);
                },
            }
        }
    });
    harness.load();
    let old = replies.wait_until("old RPC reply", Duration::from_secs(20), |events| {
        events.first().cloned()
    });
    harness.rendered("old result=waiting fences=0");
    std::fs::write(
        &harness.path,
        source.replace("const label = 'old'", "const label = 'new'"),
    )
    .unwrap();
    harness.reload();
    let current = replies.wait_until("replacement RPC reply", Duration::from_secs(20), |events| {
        events.get(1).cloned()
    });
    harness.rendered("new result=waiting fences=0");
    harness.sender.send(old).unwrap();
    harness.event(release_event());
    harness.rendered("new result=waiting fences=1");
    harness.sender.send(current).unwrap();
    harness.rendered("new result=new fences=1");
    harness.finish();
    to_proxy.send(PluginInstruction::Exit).unwrap();
    proxy.join().unwrap();
}

#[test]
#[serial(plugin_tests)]
fn deno_completion_real_dispatch_permissions_and_reload_isolation() {
    // Initialize the real discovery snapshot without a background scanner racing this fixture.
    let (tx, rx): ChannelWithContext<BackgroundJob> = channels::unbounded();
    SenderWithContext::new(tx)
        .send(BackgroundJob::Exit)
        .unwrap();
    zellij_bus::background_jobs::background_jobs_main(
        Bus::new(vec![rx], None, None, None, None, None, None, None).should_silently_fail(),
        None,
        true,
    )
    .unwrap();
    let scan = zellij_bus::background_jobs::session_scan_state().unwrap();
    *scan.current_session_name.lock().unwrap() = "deno-completion-test".into();
    scan.current_session_info.lock().unwrap().name = "deno-completion-test".into();
    scan.current_session_plugin_list.lock().unwrap().insert(
        PluginId::new(7),
        RunPlugin {
            location: RunPluginLocation::Zellij(zellij_types::data::PluginTag::new("hocus-agentd")),
            ..Default::default()
        },
    );
    let source = r#"
const label = 'old';
let state='starting', fences=0;
export default {
  async load() {
    zellij.subscribe(['CustomMessage']);
    try {await zellij.hocus.complete({messages:[{role:'user'}]}); throw Error('discovery was unprotected');}
    catch(e) {if (!String(e).includes('ReadApplicationState')) throw e;}
    await zellij.waitForPermissions(['ReadApplicationState']);
    try {await zellij.hocus.complete({messages:[{role:'user'}]}); throw Error('message was unprotected');}
    catch(e) {if (!String(e).includes('MessageAndLaunchOtherPlugins')) throw e;}
    await zellij.waitForPermissions(['MessageAndLaunchOtherPlugins']);
    state='waiting'; zellij.requestRender();
    const result=await zellij.hocus.complete({messages:[{role:'user',content:label}]});
    state=result.content;
  },
  update(e) {if(e.CustomMessage){fences++;return true;}},
  render(){return `${label} completion=${state} fences=${fences}`;}
};
"#;
    let (tx, outgoing): ChannelWithContext<PluginInstruction> = channels::unbounded();
    let tx = SenderWithContext::new(tx);
    let harness = Harness::with_event_sender(
        "ts",
        source,
        Some(PermissionStatus::Granted),
        Some(tx.clone()),
    );
    let replies = Watched::new(Vec::<PluginInstruction>::new());
    let proxy = std::thread::spawn({
        let replies = replies.clone();
        let sender = harness.sender.clone();
        move || loop {
            match outgoing.recv_timeout(FAKE_SCREEN_RECV_TIMEOUT).unwrap().0 {
                PluginInstruction::Exit => break,
                PluginInstruction::MessageFromPlugin {
                    source_plugin_id,
                    mut message,
                } if message.message_name == "hocus_complete" => {
                    assert_eq!(message.destination_plugin_id, Some(PluginId::new(7)));
                    let request: serde_json::Value =
                        serde_json::from_str(message.message_payload.as_ref().unwrap()).unwrap();
                    message.destination_plugin_id = Some(source_plugin_id);
                    message.message_name = "hocus_complete_result".into();
                    message.message_payload = Some(serde_json::json!({"request_id":request["request_id"],"content":request["messages"][0]["content"],"tool_calls":[],"finish_reason":"stop","model":"deterministic-peer","usage":null,"error":""}).to_string());
                    replies.lock().push(PluginInstruction::MessageFromPlugin {
                        source_plugin_id: PluginId::new(7),
                        message,
                    });
                },
                other => {
                    let _ = sender.send(other);
                },
            }
        }
    });
    harness.load();
    let old = replies.wait_until("first completion request", Duration::from_secs(20), |r| {
        r.first().cloned()
    });
    harness.rendered("old completion=waiting fences=0");
    // Grants survive reload, so the replacement proceeds directly to its request.
    let replacement = source.replace("const label = 'old'", "const label = 'new'");
    let start = replacement
        .find("    try {await zellij.hocus.complete")
        .unwrap();
    let end = replacement.find("    state='waiting'").unwrap();
    let replacement = format!("{}    await zellij.waitForPermissions(['ReadApplicationState', 'MessageAndLaunchOtherPlugins']);\n{}", &replacement[..start], &replacement[end..]);
    std::fs::write(&harness.path, replacement).unwrap();
    harness.reload();
    let current = replies.wait_until(
        "replacement completion request",
        Duration::from_secs(20),
        |r| r.get(1).cloned(),
    );
    harness.rendered("new completion=waiting fences=0");
    harness.sender.send(old).unwrap();
    harness.event(release_event());
    harness.rendered("new completion=waiting fences=1");
    harness.sender.send(current).unwrap();
    harness.rendered("new completion=new fences=1");
    harness.finish();
    tx.send(PluginInstruction::Exit).unwrap();
    proxy.join().unwrap();
}

#[test]
fn deno_agent_events_use_real_permissions_and_reloaded_waits() {
    use zellij_types::agent::{AgentEvent, AgentState};
    let source = r#"
const label='old';
let state='starting', heard=0;
export default {
  async load() {
    await zellij.waitForPermissions(['ReadApplicationState']);
    zellij.agent.on(()=>{heard++;});
    state='waiting'; zellij.requestRender();
    const turn=await zellij.agent.waitForTurn({Terminal:7});
    state=turn.hook+':'+turn.message;
  },render(){return `${label} agent=${state} heard=${heard}`;}
};
"#;
    let harness = Harness::new("ts", source, PermissionStatus::Granted);
    let event = |hook: &str| {
        Event::AgentEvent(AgentEvent {
            pane_id: crate::TerminalId::new(7),
            kind: "codex".into(),
            hook: hook.into(),
            state: Some(AgentState::Idle),
            last_message: Some("done".into()),
            session_id: Some("session".into()),
            detail: None,
            prompt: None,
            tool_name: None,
            cwd: None,
            raw: String::new(),
        })
    };
    harness.load();
    harness.rendered("old agent=waiting heard=0");
    harness.event(event("SessionStart"));
    harness.event(release_event());
    std::fs::write(&harness.path, source.replace("label='old'", "label='new'")).unwrap();
    harness.reload();
    harness.rendered("new agent=waiting heard=0");
    harness.event(event("Stop"));
    harness.rendered("new agent=Stop:done heard=1");
    let log = harness.finish();
    assert!(!rendered_assets(&log)
        .iter()
        .any(|(_, s)| s.contains("old agent=Stop")));
}

#[test]
fn deno_native_compilation_crashes_allow_healthy_entrypoints_and_imports_afterward() {
    for extension in ["js", "ts"] {
        for imported in [false, true] {
            let recursive = format!(
                "export default {}0{};",
                "[".repeat(10_000),
                "]".repeat(10_000)
            );
            let entry = format!("import './dep.{extension}'; export default {{}};");
            let harness = Harness::new(
                extension,
                if imported { &entry } else { &recursive },
                PermissionStatus::Granted,
            );
            let dep = harness.directory.path().join(format!("dep.{extension}"));
            if imported {
                std::fs::write(&dep, &recursive).unwrap();
            }
            harness.load();
            harness.log.wait_until("native compilation failure is reported", Duration::from_secs(40), |log| {
                log.iter().any(|instruction| matches!(instruction,
                    ScreenInstruction::UpdatePluginLoadingStage(id, stage) if *id == PLUGIN && stage.is_error())).then_some(())
            });
            std::fs::write(&dep, "export default {};").unwrap();
            std::fs::write(&harness.path, format!("import './dep.{extension}'; export default {{render() {{return 'healthy after native compilation failure';}}}};")).unwrap();
            harness.reload();
            harness.rendered("healthy after native compilation failure");
        }
    }
}
