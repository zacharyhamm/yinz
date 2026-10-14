//! Real JS/TS files through the plugin thread and shared host command handlers.
//! Synchronization observes guest progress, never elapsed time or render counts.

use super::*;
use std::time::Duration;
use yinz_types::internal_ids::{PluginId, TabId};

const CLIENT: ClientId = ClientId::new(1);
const PLUGIN: PluginId = PluginId::new(0);
const SIZE: Size = Size { cols: 80, rows: 10 };

#[test]
fn git_plugins_pin_imports_workers_and_attached_clients_until_reload() {
    use std::{
        os::unix::process::CommandExt,
        process::{Command, Stdio},
    };
    use yinz_types::input::git_plugin::GitPlugin;
    struct GitDaemon(std::process::Child);
    impl Drop for GitDaemon {
        fn drop(&mut self) {
            // Git spawns git-daemon, which can spawn connection workers.
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(self.0.id() as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
            let _ = self.0.wait();
        }
    }
    let harness = Harness::new("ts", "export default {};", PermissionStatus::Granted);
    let repository = tempdir().unwrap();
    let git = |args: &[&str]| {
        let result = Command::new("git")
            .current_dir(repository.path())
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "user.name=Plugin test",
                "-c",
                "user.email=plugin@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    git(&["init", "-b", "main"]);
    std::fs::create_dir(repository.path().join("src")).unwrap();
    std::fs::write(repository.path().join("src/main.ts"), r#"
import {value} from '../shared.ts';
let parent = 'idle', worker = 'idle', saved = 'pending';
export default {
  workers: {async probe() {const module = await import('../late.ts'); yinz.postMessageToPlugin('reply', module.value);}},
  async load() {
    await yinz.waitForPermissions(['PluginData']);
    yinz.data.define('markers', {value:'Text'});
    if (yinz.data.count({entity:'markers'}) === 0) yinz.data.apply([yinz.data.put('markers', {value}, {uid:'marker'})]);
    saved = yinz.data.fetch({entity:'markers'}).records[0].value;
    yinz.subscribe(['CustomMessage']);
  },
  async update(event) {
    if (!event.CustomMessage) return false;
    const [name, payload] = event.CustomMessage;
    if (name === 'probe') {parent = (await import('../late.ts')).value; yinz.postMessageTo('probe', 'go');}
    if (name === 'reply') worker = payload;
    return true;
  },
  render() {return `${value} ${parent} ${worker} ${saved}`;}
};
"#).unwrap();
    let write_revision = |value: &str| {
        for name in ["shared.ts", "late.ts"] {
            std::fs::write(
                repository.path().join(name),
                format!("export const value = '{value}';\n"),
            )
            .unwrap();
        }
        git(&["add", "."]);
        git(&["commit", "-m", value]);
    };
    write_revision("one");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let daemon = Command::new("git")
        .process_group(0)
        .args([
            "daemon",
            "--export-all",
            "--listen=127.0.0.1",
            &format!("--port={port}"),
            &format!("--base-path={}", repository.path().display()),
            repository.path().to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let daemon = GitDaemon(daemon);
    let ready = Watched::new(false);
    let notify = ready.clone();
    let probe = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        *notify.lock() = true;
    });
    ready.wait_until("Git daemon", default_timeout(), |ready| ready.then_some(()));
    probe.join().unwrap();
    let source = GitPlugin {
        repo: format!("git://127.0.0.1:{port}/"),
        entrypoint: "src/main.ts".into(),
        revision: None,
    };
    let run = RunPluginOrAlias::RunPlugin(RunPlugin {
        location: RunPluginLocation::Git(source.clone()),
        ..Default::default()
    });
    load(&harness.sender, run.clone());
    wait_for_plugin_to_handle(&harness.log, PLUGIN, &["one idle idle one"]);
    let loading_error = harness
        .log
        .lock()
        .iter()
        .find_map(|instruction| match instruction {
            ScreenInstruction::UpdatePluginLoadingStage(id, stage)
                if *id == PLUGIN && stage.is_error() =>
            {
                Some(format!("{stage:?}"))
            },
            _ => None,
        });
    assert!(
        loading_error.is_none(),
        "Git plugin failed to load: {loading_error:?}"
    );
    harness.rendered("one idle idle one");
    let first = crate::plugin_authoring::git::status(harness.directory.path(), &source)
        .unwrap()
        .unwrap();
    write_revision("two");
    let next = crate::plugin_authoring::git::install(
        harness.directory.path(),
        &source,
        std::slice::from_ref(&source),
        true,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    assert_ne!(first.commit, next.commit);
    // Offline delayed imports and new clients must still read the original worktree.
    drop(daemon);
    harness.event(Event::CustomMessage("probe".into(), "".into()));
    harness.rendered("one one one");
    let client = ClientId::new(2);
    harness
        .sender
        .send(PluginInstruction::AddClient(client))
        .unwrap();
    harness.log.wait_until(
        "attached client on original revision",
        default_timeout(),
        |log| {
            rendered_assets(log)
                .iter()
                .any(|(id, text)| *id == client && text.contains("one idle idle one"))
                .then_some(())
        },
    );
    harness
        .sender
        .send(PluginInstruction::ReloadPluginByUrl(run))
        .unwrap();
    // The probe targets client 1. Its async load must finish subscribing before the
    // event is sent; client 2's render or a frame with pending data cannot establish that.
    harness.log.wait_until(
        "original client ready on reloaded revision",
        default_timeout(),
        |log| {
            rendered_assets(log)
                .iter()
                .any(|(id, text)| *id == CLIENT && text.contains("two idle idle one"))
                .then_some(())
        },
    );
    harness.event(Event::CustomMessage("probe".into(), "".into()));
    harness.rendered("two two two one");
    // The source, not the selected path, remains the permission and private-data identity.
    let location = RunPluginLocation::Git(source);
    let identity = url::Url::from(&location).to_string();
    assert!(!identity.contains(first.root.to_str().unwrap()));
    let permissions = yinz_config::permission::PermissionCache::from_path_or_default(Some(
        harness.directory.path().join("permissions.kdl"),
    ));
    assert!(permissions
        .get_permissions(location.to_string())
        .unwrap()
        .contains(&PermissionType::PluginData));
    assert!(permissions
        .get_permissions(first.entrypoint.to_string_lossy().into_owned())
        .is_none());
    harness.finish();
}

#[test]
fn git_plugin_first_load_failure_recovers_by_id_with_original_configuration_and_pane() {
    use yinz_types::input::git_plugin::GitPlugin;
    let harness = Harness::new("ts", "export default {};", PermissionStatus::Granted);
    let path = harness.directory.path().join("missing.ts");
    load(
        &harness.sender,
        RunPluginOrAlias::RunPlugin(RunPlugin {
            location: RunPluginLocation::Git(GitPlugin {
                repo: harness.directory.path().to_string_lossy().into_owned(),
                entrypoint: "missing.ts".into(),
                revision: None,
            }),
            configuration: PluginUserConfiguration::new(BTreeMap::from([(
                "greeting".into(),
                "original configuration".into(),
            )])),
            initial_cwd: Some(harness.directory.path().to_owned()),
            ..Default::default()
        }),
    );
    harness.log.wait_until("repository preparation failed", default_timeout(), |log| {
        log.iter().any(|i| matches!(i, ScreenInstruction::UpdatePluginLoadingStage(id, stage) if *id == PLUGIN && stage.is_error())).then_some(())
    });
    std::fs::write(
        path,
        "let greeting; export default {load(config) {greeting = config.greeting;}, render(rows, cols) {return `${greeting} ${rows} ${cols}`;}};",
    )
    .unwrap();
    harness
        .sender
        .send(PluginInstruction::ReloadPluginWithId(PLUGIN))
        .unwrap();
    harness.rendered("original configuration 10 80");
    let log = harness.finish();
    let panes = log
        .iter()
        .filter_map(|instruction| match instruction {
            ScreenInstruction::AddPlugin(_, _, _, _, _, tab, id, ..) => Some((*id, *tab)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        panes,
        [(PLUGIN, Some(TabId::new(1)))],
        "recovery must retain the original pane"
    );
}

#[test]
fn git_plugin_aliases_refresh_on_config_reload_for_new_launches() {
    use yinz_types::input::git_plugin::GitPlugin;
    let harness = Harness::new("ts", "export default {};", PermissionStatus::Granted);
    std::fs::create_dir(harness.directory.path().join("src")).unwrap();
    std::fs::write(
        harness.directory.path().join("src/main.ts"),
        "import {value} from '../shared.ts'; export default {render() {return value;}};",
    )
    .unwrap();
    std::fs::write(
        harness.directory.path().join("shared.ts"),
        "export const value = 'reconfigured repository';",
    )
    .unwrap();
    let plugin = RunPlugin {
        location: RunPluginLocation::Git(GitPlugin {
            repo: harness.directory.path().to_string_lossy().into_owned(),
            entrypoint: "src/main.ts".into(),
            revision: None,
        }),
        ..Default::default()
    };
    harness
        .sender
        .send(PluginInstruction::Reconfigure {
            plugin_aliases: PluginAliases::from_data(BTreeMap::from([(
                "repository".into(),
                plugin,
            )])),
            client_id: CLIENT,
            keybinds: None,
            default_mode: None,
            default_shell: None,
            layout_dir: None,
            user_plugin_dir: None,
            builtin_plugins_dir: None,
            hooks: Default::default(),
            context_menu: Default::default(),
            was_written_to_disk: false,
        })
        .unwrap();
    load(
        &harness.sender,
        RunPluginOrAlias::Alias(yinz_types::input::layout::PluginAlias::new(
            "repository",
            &None,
            None,
        )),
    );
    harness.rendered("reconfigured repository");
    harness.finish();
}

#[test]
fn deno_headless_hooks_deliver_the_first_payload_after_async_permissions_and_reload() {
    use yinz_types::input::hooks::HookPoint;
    let harness = Harness::new(
        "ts",
        r#"
let ready;
yinz.on('Hook', async event => {
  await ready;
  if (event.point !== 'pane_exit' || event.payload.exit_code !== 0 || !event.payload.held) throw Error('hook wire payload');
  yinz.command('RenameTabWithId', [1, 'deno hook:' + event.payload.pane_id]);
});
export default {load() {yinz.subscribe(['Hook']); ready = yinz.waitForPermissions(['ChangeApplicationState']); return ready;}};
"#,
        PermissionStatus::Granted,
    );
    super::script_plugins::bind_hook(
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
                payload: super::script_plugins::pane_exit_payload(id),
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
    assert(refused(() => yinz.fs.read('/host/input')));
    assert(refused(() => yinz.command('CheckPlugin', {name:'check.ts',source:'export default {};',mode:'Semantic'})));
    assert(yinz.command('CheckPlugin', {name:'check.ts',source:'export default {};',mode:'Syntax'}).diagnostics.length === 0);
    assert(yinz.command('CheckPlugin', {name:'check.ts',source:'export const = ;',mode:'Syntax'}).diagnostics.length > 0);
    await yinz.waitForPermissions(['ReadFiles', 'WriteFiles']);
    yinz.fs.mkdir('/host/files/nested', {all: true});
    const bytes = new Uint8Array([0, 128, 255, 10]);
    assert(yinz.fs.write('/host/files/nested/b', bytes) === undefined);
    assert([...yinz.fs.read('/host/files/nested/b')].join() === [...bytes].join());
    assert(refused(() => yinz.fs.read('/host/files/nested/b', {maxBytes: 2})));
    yinz.fs.write('/host/files/nested/a', 'hé🙂');
    yinz.fs.write('/host/files/nested/a', '!', {append: true});
    assert(yinz.decodeUtf8(yinz.fs.read('/host/files/nested/a')) === 'hé🙂!');
    assert(yinz.fs.list('/host/files/nested').map(e => e.path.split('/').at(-1)).join() === 'a,b');
    assert(yinz.fs.metadata('/host/files/missing') === null);
    assert(yinz.fs.metadata('/host/files/nested/b').len === 4);
    for (const path of ['/host/../escape', '/host/link/secret', '/host/link/new/nested']) {
      assert(refused(() => yinz.fs.read(path)));
      assert(refused(() => yinz.fs.read(path, {offset: 0, maxBytes: 1})));
      assert(refused(() => yinz.fs.write(path, 'bad', {createDirs: true})));
      assert(refused(() => yinz.fs.rename('/host/files/nested/b', path)));
      assert(refused(() => yinz.fs.rename(path, '/host/files/from-outside')));
    }
    yinz.fs.rename('/host/files/nested/b', '/host/files/moved');
    assert(yinz.fs.metadata('/host/files/nested/b') === null);
    assert([...yinz.fs.read('/host/files/moved')].join() === [...bytes].join());
    yinz.fs.remove('/host/files', {recursive: true});
    assert(yinz.fs.metadata('/host/files') === null);
    const previous = yinz.projectConfig.read('.');
    assert(previous.source === null);
    assert(refused(() => yinz.projectConfig.write('.', 'agent_exit {}')));
    const current = yinz.projectConfig.write('.', 'agent_exit { default_branch "main"; }', previous.source);
    assert(current.config.default_branch === 'main');
    assert(refused(() => yinz.projectConfig.write('.', 'agent_exit {}', previous.source)));
    assert(yinz.projectConfig.read('.').source === current.source);
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
fn deno_file_ranges_read_large_files_without_weakening_whole_file_limits() {
    use std::io::{Seek, SeekFrom, Write};

    let harness = Harness::new(
        "ts",
        r#"
let state = 'starting';
export default {
  async load() { try {
    const assert = (ok) => { if (!ok) throw Error('file range assertion'); };
    const refused = (call) => { try { call(); return false; } catch (_) { return true; } };
    const path = '/host/large.bin', cap = 8 * 1024 * 1024;
    assert(refused(() => yinz.fs.read(path, {offset: 0, maxBytes: 5})));
    await yinz.waitForPermissions(['ReadFiles']);
    assert(refused(() => yinz.fs.read(path)));
    assert(refused(() => yinz.fs.read(path, {maxBytes: cap * 2})));
    assert(refused(() => yinz.command('FsRead', {path, max_bytes: null, offset: null})));
    assert(yinz.decodeUtf8(yinz.fs.read(path, {offset: 0, maxBytes: 5})) === 'start');
    assert([...yinz.fs.read(path, {offset: BigInt(cap - 2), maxBytes: 8n})].join() === '1,2,3,4,5,6');
    assert([...yinz.command('FsRead', {path, offset: cap, max_bytes: 3})].join() === '3,4,5');
    assert([...yinz.fs.read('/host/inside-link', {offset: cap})].join() === '3,4,5,6');
    assert(yinz.fs.read(path, {offset: cap + 4}).length === 0);
    assert(yinz.fs.read(path, {offset: cap + 100, maxBytes: 1}).length === 0);
    assert(yinz.fs.read(path, {offset: 0, maxBytes: 0}).length === 0);
    for (const offset of [-1, 1.5, 18446744073709551616n]) {
      assert(refused(() => yinz.fs.read(path, {offset})));
    }
    assert(refused(() => yinz.fs.read(path, {offset: 0, maxBytes: -1})));
    assert(refused(() => yinz.fs.read('/host', {offset: 0, maxBytes: 1})));
    assert(refused(() => yinz.fs.read('/host/fifo', {offset: 0, maxBytes: 1})));
    state = 'file ranges verified';
  } catch (error) {state = 'file ranges failed: ' + error.stack;} },
  render() { return state; }
};
"#,
        PermissionStatus::Granted,
    );
    let path = harness.directory.path().join("large.bin");
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(b"start").unwrap();
    file.seek(SeekFrom::Start(8 * 1024 * 1024 - 2)).unwrap();
    file.write_all(&[1, 2, 3, 4, 5, 6]).unwrap();
    std::os::unix::fs::symlink(&path, harness.directory.path().join("inside-link")).unwrap();
    nix::unistd::mkfifo(
        &harness.directory.path().join("fifo"),
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .unwrap();
    harness.load();
    let frame = harness.rendered("file ranges");
    assert!(frame.contains("file ranges verified"), "{frame}");
}

#[test]
fn deno_data_helpers_preserve_values_rollback_conflicts_watch_disposal_and_reload_persistence() {
    let harness = Harness::with_services(
        "ts",
        r#"
let state = 'starting';
export default {
  async load() { try {
    await yinz.waitForPermissions(['PluginData']);
    const d = yinz.data;
    const assert = (ok) => { if (!ok) throw Error('data helper assertion'); };
    let denial = '';
    try {d.define('shared_item', {text:'Text'}, {namespace:'shared:deno_sdk'});} catch (error) {denial = String(error);}
    if (!denial.includes('SharedPluginData') && !denial.includes('shared_plugin_data')) throw Error('shared permission denial: ' + denial);
    await yinz.waitForPermissions(['SharedPluginData']);
    d.define('shared_item', {text:'Text'}, {namespace:'shared:deno_sdk'});
    d.apply([d.put('shared_item', {text:'shared from Deno'}, {uid:'shared'})], {namespace:'shared:deno_sdk'});
    d.define('items', {n:'Int', r:'Real', text:'Text', json:'Json', blob:'Blob', flag:'Bool', nullable:'Text', host_id:'Text', yinz_session_name:'Text'});
    const values = {n:9223372036854775807n, r:d.real(1), text:'hello', json:d.json({nested:[1,true]}),
      blob:new Uint8Array([0,128,255]), flag:false, nullable:null, host_id:'author-host', yinz_session_name:'author-session'};
    d.apply([d.put('items', values, {uid:'row', expectRev:0})]);
    const query = {entity:'items', filter:d.eq('uid','row')};
    const snapshot = d.fetch(query);
    const first = snapshot.records[0], second = d.fetch(query).records[0];
    assert(first.n === values.n && first.r === 1 && first.flag === false && first.nullable === null);
    assert(first.json === '{"nested":[1,true]}' && [...first.blob].join() === '0,128,255');
    assert(first.host_id === values.host_id && first.yinz_session_name === values.yinz_session_name);
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
    const changed = yinz.waitForEvent('PluginDataChanged', e => e.watch_id === watch.watchId);
    d.apply([d.update('items',d.eq('uid','row'),{text:'watched'})]);
    await changed;
    assert(changes === 1);
    watch.dispose(); watch.dispose();
    d.apply([d.update('items',d.eq('uid','row'),{text:'persisted'})]);
    // A later host event provides an ordering barrier without a sleep.
    state = 'data helpers: watch disposed'; yinz.requestRender();
    await yinz.waitForEvent('CustomMessage');
    assert(changes === 1 && d.fetch(query).records[0].text === 'persisted');
    state = 'data helpers verified';
  } catch (error) {state = 'data helpers failed: ' + error.stack;} },
  render() { return state; }
};
"#,
        Some(PermissionStatus::Granted),
        None,
        None,
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
  await yinz.waitForPermissions(['PluginData']);
  const row = yinz.data.fetch({entity:'items'}).records[0];
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
  await yinz.waitForPermissions(['PluginData']);
  yinz.data.define('items', {text:'Text'});
  if (yinz.data.count({entity:'items'}) !== 0) throw Error('namespace leaked');
  await yinz.waitForPermissions(['SharedPluginData']);
  if (yinz.data.fetch({entity:'shared_item',namespace:'shared:deno_sdk'}).records[0].text !== 'shared from Deno') throw Error('shared namespace missing');
  state='different URL namespace verified';
},render(){return state;}};
"#,
    )
    .unwrap();
    load(&harness.sender, run_plugin(&other));
    harness.rendered("different URL namespace verified");
    let reader = harness.directory.path().join("shared_reader.ts");
    std::fs::write(
        &reader,
        r#"
import z from 'yinz:plugin';
let state = 'waiting';
export default {
  async load() {
    await z.waitForPermissions(['PluginData', 'SharedPluginData']);
    const rows = z.data.fetch({entity:'shared_item', namespace:'shared:deno_sdk'});
    if (rows.records[0].text !== 'shared from Deno') throw Error('shared data mismatch');
    state = 'TypeScript reads Deno shared data';
  },
  render() {return state;}
};
"#,
    )
    .unwrap();
    load(&harness.sender, run_plugin(&reader));
    harness.rendered("TypeScript reads Deno shared data");
}

#[test]
fn deno_integer_commands_keep_values_and_revision_guards_through_the_store() {
    let harness = Harness::new(
        "ts",
        r#"
let state = 'starting';
export default {
  async load() {
    await yinz.waitForPermissions(['PluginData']);
    const assert = (ok) => { if (!ok) throw Error('numeric host assertion'); };
    const attr = (name, kind) => ({name, kind, indexed: false, full_text: false, unique: false, required: false, default: null});
    yinz.command('DataDefineEntity', {namespace: null, allow_destructive: false,
      def: {name: 'numbers', version: 1, attrs: [attr('n', 'Int'), attr('r', 'Real')]}});
    const put = (n, r, expect_rev) => ({Put: {entity: 'numbers', uid: 'row',
      values: [['n', {Int: n}], ['r', {Real: r}]], expect_rev}});
    const apply = (...mutations) => yinz.command('DataApply', {namespace: null, mutations});
    const read = () => yinz.command('DataFetch', {request: {namespace: null, entity: 'numbers',
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
    const CHILD: &str = "YINZ_TEST_DENO_SMALL_BUDGET";
    if std::env::var_os(CHILD).is_none() {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "plugin_tests::deno_plugins::deno_exhaustion_disposes_the_guest_and_reload_recovers_in_a_live_session", "--nocapture"])
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
  load() { yinz.subscribe(['CustomMessage', 'BeforeClose']); },
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
            Some(directory.path().to_owned()),
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
    yinz.subscribe(["PermissionRequestResult"]);
    // Try the protected operation both before and after the user's answer.
    try { yinz.command("RenameTabWithId", [1, "before-grant"]); } catch (_) {}
    // The helper and an independent observer must share the same host event.
    const answer = yinz.waitForEvent("PermissionRequestResult");
    let helper = "resolved";
    try { await yinz.waitForPermissions(["ChangeApplicationState"]); }
    catch (_) { helper = "rejected"; }
    const status = await answer;
    const ids = yinz.command("GetPluginIds");
    const objectIds = yinz.command({GetPluginIds: null});
    if (ids.plugin_id !== objectIds.plugin_id || ids.client_id !== objectIds.client_id) throw new Error("command forms disagree");
    let outcome = "returned";
    try { yinz.command("RenameTabWithId", [1, "after-grant"]); }
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
      ['run', yinz.runCommand({command: ['/usr/bin/true'], onOutput() {throw Error('denied command emitted output');}})],
      ['web', yinz.webRequest({url: 'https://example.com'})],
    ] as const) {
      try { await promise; } catch (error) {
        if (!(error instanceof Error) || !error.message.includes('permission')) throw error;
        denied.push(`${name} denied`);
      }
    }
    yinz.command('Unsubscribe', ['RunCommandResult', 'WebRequestResult']);
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
import { yinz, definePlugin } from "yinz:plugin";
let state: string = "starting";
export default definePlugin({
  async load(config) {
    yinz.subscribe(["Key"]);
    const key = yinz.waitForEvent("Key");
    state = `${config.greeting} awaiting key`;
    yinz.requestRender();
    await key;
    await yinz.sleep(0);
    state = "key and timer completed";
  },
  async render(rows: number, columns: number) {
    await yinz.sleep(0);
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
  load() { yinz.subscribe(["Key"]); },
  async update() {
    await yinz.sleep(0);
    release?.();
    return true;
  },
  async render(rows: number, columns: number) {
    if (columns === 80) {
      const ready = new Promise<void>(resolve => { release = resolve; });
      // This instruction marks the point the old render is suspended.
      yinz.command("SetSelectable", false);
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
import type { Event } from "yinz:plugin";
type Reply = { client_id: number };
const origin = () => (yinz.command("GetPluginIds") as Reply).client_id;
const waiting: number[] = [];
const completed: string[] = [];
let loads = 0;
export default {
  load() { loads++; yinz.subscribe(["Key"]); },
  async update(event: Event) {
    if (typeof event !== "object" || !("Key" in event)) return false;
    const started = origin();
    const release = yinz.waitForEvent("CustomMessage", ([name]) => name === "acceptance-release");
    waiting.push(started);
    yinz.requestRender();
    await release;
    const afterEvent = origin();
    await yinz.sleep(0);
    completed.push(`${started}->${afterEvent}->${origin()}`);
    return true;
  },
  async render() {
    const started = origin();
    await yinz.sleep(0);
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
import type { Event } from "yinz:plugin";
let loads = 0;
let keys = 0;
let state = "starting";
export default {
  async load() {
    loads++;
    yinz.subscribe(["Key"]);
    await yinz.waitForPermissions(["ChangeApplicationState"]);
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
        .send(PluginInstruction::PermissionAnswer {
            request_id: harness
                .log
                .lock()
                .iter()
                .rev()
                .find_map(|instruction| match instruction {
                    ScreenInstruction::RequestPluginPermissions(_, request) => {
                        Some(request.request_id)
                    },
                    _ => None,
                })
                .unwrap(),
            status: PermissionStatus::Granted,
            cache_path: Some(harness.directory.path().join("permissions.kdl")),
        })
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
import type { Event } from "yinz:plugin";
const label = "original";
let state = "pending";
let timers = 0;
let fences = 0;
export default {
  async load() {
    yinz.subscribe(["CustomMessage"]);
    await yinz.sleep(0);
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
                timer @ PluginInstruction::DenoReply(_, _, _, Event::Timer(_), _) => {
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
            PluginInstruction::DenoReply(old_id, old_client, old_generation, Event::Timer(_), _),
            PluginInstruction::DenoReply(new_id, new_client, new_generation, Event::Timer(_), _),
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
#[cfg(unix)]
fn repository_file_url_reload_follows_retargeted_symlink() {
    let harness = Harness::new("ts", "export default {};", PermissionStatus::Granted);
    let first = harness.directory.path().join("v1");
    let second = harness.directory.path().join("v2");
    for (path, version) in [(&first, "one"), (&second, "two")] {
        std::fs::create_dir(path).unwrap();
        std::fs::write(
            path.join("main.ts"),
            format!("export default {{ render() {{ return 'repository {version}'; }} }};"),
        )
        .unwrap();
    }
    let current = harness.directory.path().join("current # %20?");
    std::os::unix::fs::symlink(&first, &current).unwrap();
    let url = url::Url::from_file_path(&current).unwrap();
    let config = yinz_config::config::Config::from_kdl(
        &format!("plugins {{ current repo=\"{url}\"; }}"),
        None,
    )
    .unwrap();
    let serialized = yinz_config::kdl::plugin_aliases::to_kdl(&config.plugins, false);
    let restored = yinz_config::kdl::plugin_aliases::from_kdl(&serialized).unwrap();
    let plugin = &restored.aliases["current"];
    let RunPluginLocation::Git(repository) = &plugin.location else {
        panic!("repository plugin")
    };
    assert_eq!(PathBuf::from(&repository.repo), current);
    load(&harness.sender, RunPluginOrAlias::RunPlugin(plugin.clone()));
    harness.rendered("repository one");
    std::fs::remove_file(&current).unwrap();
    std::os::unix::fs::symlink(&second, &current).unwrap();
    std::fs::remove_dir_all(first).unwrap();
    harness
        .sender
        .send(PluginInstruction::ReloadPluginWithId(PLUGIN))
        .unwrap();
    harness.rendered("repository two");
    let log = harness.finish();
    assert_eq!(
        log.iter()
            .filter(|i| matches!(i, ScreenInstruction::AddPlugin(..)))
            .count(),
        1
    );
    assert!(log.iter().any(|i| matches!(i, ScreenInstruction::PluginBytes(assets)
        if assets.iter().any(|asset| asset.plugin_id == PLUGIN && frame_of(&asset.bytes).contains("repository two")))));
}

#[test]
#[serial(plugin_tests)]
#[cfg(unix)]
fn deno_reload_follows_retargeted_symlinks_by_id_and_url() {
    for by_id in [true, false] {
        let harness = Harness::new(
            "ts",
            "export default { render() { return 'version one'; } };",
            PermissionStatus::Granted,
        );
        let first = harness.directory.path().join("v1.ts");
        std::fs::rename(&harness.path, &first).unwrap();
        let current = harness.directory.path().join("current.ts");
        std::os::unix::fs::symlink(&first, &current).unwrap();
        let url = url::Url::from_file_path(&current).unwrap().to_string();
        let parse = || {
            RunPluginOrAlias::RunPlugin(
                yinz_runtime::plugin_location::run_plugin_from_url(&url).unwrap(),
            )
        };
        load(&harness.sender, parse());
        harness.rendered("version one");

        let second = harness.directory.path().join("v2.ts");
        std::fs::write(
            &second,
            "export default { render() { return 'version two'; } };",
        )
        .unwrap();
        std::fs::remove_file(&current).unwrap();
        std::os::unix::fs::symlink(&second, &current).unwrap();
        harness
            .sender
            .send(if by_id {
                PluginInstruction::ReloadPluginWithId(PLUGIN)
            } else {
                PluginInstruction::ReloadPluginByUrl(parse())
            })
            .unwrap();
        harness.rendered("version two");
        harness.log.wait_until("replacement identity published", default_timeout(), |log| {
            log.iter().any(|i| matches!(i, ScreenInstruction::UpdatePluginLocation(id, RunPluginLocation::File(path))
                if *id == PLUGIN && path == &second.canonicalize().unwrap())).then_some(())
        });
        // Updating the canonical identity must not discard the declared symlink for later reloads.
        std::fs::write(
            &harness.path,
            "export default { render() { return 'version three'; } };",
        )
        .unwrap();
        std::fs::remove_file(&current).unwrap();
        std::os::unix::fs::symlink(&harness.path, &current).unwrap();
        harness
            .sender
            .send(PluginInstruction::ReloadPluginWithId(PLUGIN))
            .unwrap();
        harness.rendered("version three");
        // A reload naming the old canonical target must now start a separate plugin.
        harness
            .sender
            .send(PluginInstruction::Reload(
                Some(false),
                None,
                RunPluginOrAlias::RunPlugin(
                    yinz_runtime::plugin_location::run_plugin_from_url(
                        url::Url::from_file_path(&first).unwrap().as_ref(),
                    )
                    .unwrap(),
                ),
                TabId::new(1),
                SIZE,
                None,
            ))
            .unwrap();
        harness
            .log
            .wait_until("old target gets a new pane", default_timeout(), |log| {
                (log.iter()
                    .filter(|i| matches!(i, ScreenInstruction::AddPlugin(..)))
                    .count()
                    == 2)
                    .then_some(())
            });
        let log = harness.finish();
        assert_eq!(
            log.iter()
                .filter(|i| matches!(i, ScreenInstruction::AddPlugin(..)))
                .count(),
            2,
            "only the old target launch creates a second pane"
        );
        assert!(log.iter().any(|i| matches!(i, ScreenInstruction::PluginBytes(assets)
            if assets.iter().any(|asset| asset.plugin_id == PLUGIN && frame_of(&asset.bytes).contains("version two")))),
            "reload must retain the original plugin ID");
    }
}

#[test]
#[serial(plugin_tests)]
#[cfg(unix)]
fn deno_failed_symlink_reload_does_not_publish_a_replacement_identity() {
    let harness = Harness::new(
        "ts",
        "export default { render() { return 'original ready'; } };",
        PermissionStatus::Granted,
    );
    let current = harness.directory.path().join("current.ts");
    std::os::unix::fs::symlink(&harness.path, &current).unwrap();
    let url = url::Url::from_file_path(&current).unwrap().to_string();
    load(
        &harness.sender,
        RunPluginOrAlias::RunPlugin(
            yinz_runtime::plugin_location::run_plugin_from_url(&url).unwrap(),
        ),
    );
    harness.rendered("original ready");
    let replacement = harness.directory.path().join("broken.ts");
    std::fs::write(
        &replacement,
        "export default { load() { throw Error('replacement failed'); } };",
    )
    .unwrap();
    std::fs::remove_file(&current).unwrap();
    std::os::unix::fs::symlink(&replacement, &current).unwrap();
    harness
        .sender
        .send(PluginInstruction::ReloadPluginWithId(PLUGIN))
        .unwrap();
    harness.log.wait_until("replacement load failed", default_timeout(), |log| {
        log.iter().any(|i| matches!(i, ScreenInstruction::UpdatePluginLoadingStage(id, stage) if *id == PLUGIN && stage.is_error())).then_some(())
    });
    assert!(!harness
        .finish()
        .iter()
        .any(|i| matches!(i, ScreenInstruction::UpdatePluginLocation(..))));
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
    const event = yinz.waitForEvent("CustomMessage", ([name]) => name === "acceptance-release");
    yinz.command("Unsubscribe", ["CustomMessage"]);
    state = `${label} awaiting input`;
    yinz.requestRender();
    await event;
    // An observable host effect catches an old continuation even if its render is discarded.
    yinz.command("SetSelectable", label === "second");
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
fn deno_two_clients_share_one_transpile_and_a_reload_of_identical_source_spawns_nothing() {
    let harness = Harness::new(
        "ts",
        r#"
import { label } from "./dependency.ts";
export default {
  render() { return `label=${label}`; }
};
"#,
        PermissionStatus::Granted,
    );
    let dependency = harness.directory.path().join("dependency.ts");
    std::fs::write(&dependency, "export const label: string = 'first';").unwrap();
    let other = ClientId::new(2);
    harness
        .sender
        .send(PluginInstruction::AddClient(other))
        .unwrap();
    let spawns = || crate::deno::compilation_worker_spawn_count();
    let frames = |log: &[ScreenInstruction], client: ClientId, needle: &str| {
        rendered_assets(log)
            .iter()
            .filter(|(rendered_client, frame)| *rendered_client == client && frame.contains(needle))
            .count()
    };
    let wait_for_both = |needle: &str, at_least: (usize, usize)| {
        harness.log.wait_until_described(
            &format!("both clients rendered {needle:?}"),
            default_timeout(),
            |log| {
                (frames(log, CLIENT, needle) > at_least.0
                    && frames(log, other, needle) > at_least.1)
                    .then_some(())
            },
            |log| describe_renders(log),
        );
    };
    let before = spawns();
    harness.load();
    wait_for_both("label=first", (0, 0));
    assert_eq!(
        spawns() - before,
        2,
        "the entrypoint and its dependency are compiled once each, not once per client"
    );
    let rendered = {
        let log = harness.log.lock();
        (
            frames(&log, CLIENT, "label=first"),
            frames(&log, other, "label=first"),
        )
    };
    harness.reload();
    wait_for_both("label=first", rendered);
    assert_eq!(
        spawns() - before,
        2,
        "a reload re-reads both files and finds their content unchanged"
    );
    std::fs::write(&dependency, "export const label: string = 'second';").unwrap();
    harness.reload();
    wait_for_both("label=second", (0, 0));
    assert_eq!(
        spawns() - before,
        3,
        "only the edited dependency is compiled again"
    );
    let log = harness.finish();
    assert_eq!(
        log.iter()
            .filter(|i| matches!(i, ScreenInstruction::AddPlugin(..)))
            .count(),
        1
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
let replies = 0, chunks = 0;
let fences = 0;
export default {
  async load() {
    yinz.subscribe(['CustomMessage']);
    await yinz.waitForPermissions(['RunCommands']);
    const command = yinz.runCommand({command: ['/usr/bin/true'], context: {label}, onOutput(output) {
      if (output.data[0] !== 255 || output.context.label !== label) throw Error('incorrect chunk');
      chunks++; yinz.requestRender();
    }});
    yinz.command({Unsubscribe: ['RunCommandResult']});
    const result = await command;
    if (result.exitCode !== 7 || yinz.decodeUtf8(result.stdout) !== 'hé🙂'
        || result.stderr[0] !== 255 || result.context.label !== label) throw Error('incorrect result');
    state = 'completed';
  },
  update(event) {
    if (typeof event !== 'object') return false;
    if ('RunCommandResult' in event) replies++;
    if ('CustomMessage' in event) fences++;
    return true;
  },
  render() { return `${label} ${state} replies=${replies} fences=${fences} chunks=${chunks}`; }
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
    harness.rendered("original pending replies=0 fences=0 chunks=0");
    std::fs::write(&harness.path, source.replace("'original'", "'replacement'")).unwrap();
    harness.reload();
    let current = completion(1);
    harness.rendered("replacement pending replies=0 fences=0 chunks=0");
    match (&old, &current) {
        (
            PluginInstruction::DenoEvent(_, _, old_generation, _),
            PluginInstruction::DenoEvent(_, _, current_generation, _),
        ) => assert_ne!(old_generation, current_generation),
        _ => panic!("commands must carry their originating generation"),
    }
    let chunk = |instruction: &PluginInstruction| match instruction {
        PluginInstruction::DenoEvent(
            plugin,
            client,
            generation,
            Event::RunCommandResult(_, _, _, context),
        ) => PluginInstruction::for_plugin_event(
            *plugin,
            *client,
            Some(*generation),
            Event::RunCommandOutput {
                data: vec![255],
                stderr: true,
                context: context.clone(),
            },
        ),
        _ => panic!("expected generated command result"),
    };
    harness.sender.send(chunk(&old)).unwrap();
    harness.sender.send(old).unwrap();
    harness.event(release_event());
    harness.rendered("replacement pending replies=0 fences=1 chunks=0");
    harness.sender.send(chunk(&current)).unwrap();
    harness.sender.send(current).unwrap();
    harness.rendered("replacement completed replies=1 fences=1 chunks=1");
    harness.finish();
    job_logger.join().unwrap();
}

#[test]
#[serial(plugin_tests)]
fn deno_pane_output_subscriptions_reload_with_new_generations() {
    let source = r#"
const label='old'; let chunks=0, fences=0;
export default {
  async load() {
    yinz.subscribe(['CustomMessage', 'PaneOutput']);
    await yinz.waitForPermissions(['ReadPaneContents']);
    yinz.command('SubscribePaneOutput', {work:'one'});
  },
  update(event) {
    if(event.PaneOutput) {
      if(event.PaneOutput.pane_id!==7 || event.PaneOutput.data[0]!==255) throw Error('bad output');
      chunks++;
    }
    if(event.CustomMessage) fences++;
    return true;
  },
  render() {return `${label} chunks=${chunks} fences=${fences}`;}
};
"#;
    let harness = Harness::new("ts", source, PermissionStatus::Granted);
    let subscription = |index| {
        harness.log.wait_until(
            "pane output subscription",
            default_timeout(),
            |instructions| {
                instructions
                    .iter()
                    .filter_map(|instruction| match instruction {
                        ScreenInstruction::SubscribePaneOutput(
                            plugin,
                            client,
                            Some(context),
                            generation,
                        ) => {
                            assert_eq!(context.get("work").map(String::as_str), Some("one"));
                            Some((*plugin, *client, *generation))
                        },
                        _ => None,
                    })
                    .nth(index)
            },
        )
    };
    harness.load();
    let old = subscription(0);
    harness.rendered("old chunks=0 fences=0");
    std::fs::write(&harness.path, source.replace("label='old'", "label='new'")).unwrap();
    harness.reload();
    let current = subscription(1);
    assert!(old.2.is_some() && current.2.is_some());
    assert_ne!(old.2, current.2);
    assert!(harness.log.lock().iter().any(|instruction| matches!(instruction, ScreenInstruction::UnsubscribePaneOutput(plugin, generation) if *plugin==old.0 && *generation == old.2)));
    let chunk = |(plugin, client, generation)| {
        PluginInstruction::for_plugin_event(
            plugin,
            client,
            generation,
            Event::PaneOutput {
                pane_id: 7,
                data: vec![255],
                context: BTreeMap::from([("work".into(), "one".into())]),
            },
        )
    };
    harness.sender.send(chunk(old)).unwrap();
    harness.event(release_event());
    harness.rendered("new chunks=0 fences=1");
    harness.sender.send(chunk(current)).unwrap();
    harness.rendered("new chunks=1 fences=1");
    harness
        .sender
        .send(PluginInstruction::RemoveClient(CLIENT))
        .unwrap();
    harness
        .sender
        .send(PluginInstruction::AddClient(CLIENT))
        .unwrap();
    let reattached = subscription(2);
    assert_ne!(current.2, reattached.2);
    assert!(harness.log.lock().iter().any(|instruction| matches!(instruction,
        ScreenInstruction::SubscribePaneOutput(plugin, client, None, _) if *plugin == current.0 && *client == CLIENT)));
    harness.sender.send(chunk(current)).unwrap();
    harness.sender.send(chunk(reattached)).unwrap();
    harness.event(release_event());
    harness.rendered("new chunks=1 fences=1");
    harness.event(release_event());
    harness.rendered("new chunks=1 fences=2");
    harness.finish();
}

#[test]
fn deno_failed_worker_removes_only_its_pane_output_registrations() {
    let harness = Harness::new(
        "ts",
        r#"
export default {
  workers: {
    failed() {
      yinz.subscribe(['CustomMessage']);
      yinz.on('CustomMessage', () => { throw Error('expected worker runtime failure'); });
      yinz.command('SubscribePaneOutput', {work:'failed'});
    },
    survivor() { yinz.command('SubscribePaneOutput', {work:'survivor'}); },
  },
  async load() {
    await yinz.waitForPermissions(['ReadPaneContents']);
    yinz.command('SubscribePaneOutput', {work:'parent'});
    yinz.postMessageTo('failed', 'go');
    yinz.postMessageTo('survivor', 'go');
  },
};
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    let generations = harness.log.wait_until(
        "parent and worker registrations",
        default_timeout(),
        |log| {
            let generations: BTreeMap<_, _> = log
                .iter()
                .filter_map(|instruction| match instruction {
                    ScreenInstruction::SubscribePaneOutput(
                        plugin,
                        client,
                        Some(context),
                        Some(generation),
                    ) => {
                        assert_eq!((*plugin, *client), (PLUGIN, CLIENT));
                        Some((context["work"].clone(), *generation))
                    },
                    _ => None,
                })
                .collect();
            (generations.len() == 3).then_some(generations)
        },
    );
    harness
        .sender
        .send(PluginInstruction::DenoEvent(
            PLUGIN,
            CLIENT,
            generations["failed"],
            release_event(),
        ))
        .unwrap();
    harness
        .log
        .wait_until("failed worker disposal", default_timeout(), |log| {
            log.iter()
                .any(|instruction| {
                    matches!(instruction,
                        ScreenInstruction::UnsubscribePaneOutput(plugin, Some(generation))
                            if *plugin == PLUGIN && *generation == generations["failed"]
                    )
                })
                .then_some(())
        });
    assert_eq!(
        generations
            .values()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );
    for instruction in harness.log.lock().iter() {
        if let ScreenInstruction::UnsubscribePaneOutput(plugin, generation) = instruction {
            assert_eq!(
                (*plugin, *generation),
                (PLUGIN, Some(generations["failed"]))
            );
        }
    }
    let log = harness.finish();
    for generation in generations.values() {
        assert!(log.iter().any(|instruction| matches!(instruction,
            ScreenInstruction::UnsubscribePaneOutput(plugin, Some(removed))
                if *plugin == PLUGIN && removed == generation
        )));
    }
    assert!(log.iter().any(|instruction| matches!(instruction,
        ScreenInstruction::UnsubscribePaneOutput(plugin, None) if *plugin == PLUGIN
    )));
}

/// FNV-1a over `bytes`, as the fixtures below compute it in JavaScript with `Math.imul`.
fn fnv1a(bytes: &[u8]) -> u32 {
    bytes.iter().fold(2166136261u32, |hash, byte| {
        (hash ^ *byte as u32).wrapping_mul(16777619)
    })
}

const FNV_JS: &str = "const fnv = (bytes) => { let h = 2166136261; for (const b of bytes) { h ^= b; h = Math.imul(h, 16777619) >>> 0; } return h >>> 0; };";

#[test]
#[serial(plugin_tests)]
fn deno_streaming_output_arrives_as_uint8array_and_a_64_kib_chunk_round_trips_byte_exact() {
    let harness = Harness::new(
        "ts",
        &format!(
            r#"
{FNV_JS}
let state = 'waiting';
export default {{
  async load() {{
    yinz.subscribe(['PaneOutput', 'RunCommandOutput']);
    await yinz.waitForPermissions(['ReadPaneContents', 'RunCommands']);
    state = 'ready';
    yinz.requestRender();
  }},
  update(event) {{
    const payload = event.PaneOutput ?? event.RunCommandOutput;
    if (!payload) return false;
    const data = payload.data;
    const kind = event.PaneOutput ? `pane ${{payload.pane_id}}` : `command stderr=${{payload.stderr}}`;
    state = `${{kind}} ${{payload.context.work}} typed=${{data instanceof Uint8Array}} len=${{data.length}} hash=${{fnv(data)}} first=${{data[0]}} last=${{data[data.length - 1]}}`;
    return true;
  }},
  render() {{ return state; }}
}};
"#
        ),
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("ready");
    let chunk: Vec<u8> = (0..65536u32)
        .map(|i| (i.wrapping_mul(7) ^ (i >> 8)) as u8)
        .collect();
    let expected = format!(
        "typed=true len={} hash={} first={} last={}",
        chunk.len(),
        fnv1a(&chunk),
        chunk[0],
        chunk[chunk.len() - 1]
    );
    harness.event(Event::PaneOutput {
        pane_id: 7,
        data: chunk.clone(),
        context: BTreeMap::from([("work".into(), "one".into())]),
    });
    harness.rendered(&format!("pane 7 one {expected}"));
    harness.event(Event::RunCommandOutput {
        data: chunk,
        stderr: true,
        context: BTreeMap::from([("work".into(), "two".into())]),
    });
    harness.rendered(&format!("command stderr=true two {expected}"));
    harness.event(Event::PaneOutput {
        pane_id: 8,
        data: Vec::new(),
        context: BTreeMap::from([("work".into(), "empty".into())]),
    });
    harness
        .rendered("pane 8 empty typed=true len=0 hash=2166136261 first=undefined last=undefined");
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn deno_byte_replies_are_uint8array_without_copying_through_json() {
    let harness = Harness::new(
        "ts",
        &format!(
            r#"
{FNV_JS}
let state = 'waiting';
export default {{
  async load() {{
    await yinz.waitForPermissions(['ReadFiles']);
    const data = yinz.fs.read('/host/blob.bin');
    const raw = yinz.command('FsRead', {{path: '/host/blob.bin', max_bytes: null}});
    state = `typed=${{data instanceof Uint8Array}} raw=${{raw instanceof Uint8Array}} len=${{data.length}} hash=${{fnv(data)}} same=${{fnv(raw) === fnv(data)}}`;
  }},
  render() {{ return state; }}
}};
"#
        ),
        PermissionStatus::Granted,
    );
    let blob: Vec<u8> = (0..65536u32)
        .map(|i| (i.wrapping_mul(13) ^ (i >> 7)) as u8)
        .collect();
    std::fs::write(harness.directory.path().join("blob.bin"), &blob).unwrap();
    harness.load();
    let frame = harness.rendered("typed=");
    assert_eq!(
        frame.trim_end(),
        format!(
            "typed=true raw=true len={} hash={} same=true",
            blob.len(),
            fnv1a(&blob)
        )
    );
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn deno_removed_client_reattaches_with_fresh_state_and_without_its_old_awaits() {
    client_reattaches(false);
}

#[test]
#[serial(plugin_tests)]
fn wasm_detach_retains_state_and_output_subscriptions() {
    retained_client_reattaches();
}

fn retained_client_reattaches() {
    let harness = Harness::new("ts", "export default {};", PermissionStatus::Granted);
    let path = plugin_fixture_path();
    load(&harness.sender, run_plugin(&path));
    let first = Event::Key(KeyWithModifier::new(BareKey::Char('1')));
    let second = Event::Key(KeyWithModifier::new(BareKey::Char('2')));
    harness.event(first);
    harness.rendered("Char('1')");
    harness
        .sender
        .send(PluginInstruction::RemoveClient(CLIENT))
        .unwrap();
    harness
        .sender
        .send(PluginInstruction::AddClient(CLIENT))
        .unwrap();
    harness.event(second);
    let frame = harness.rendered("Char('2')");
    {
        assert!(
            frame.contains("Char('1')"),
            "the Wasm instance lost its event history: {frame}"
        );
    }
    // The new event ran after the disposal job on the same executor, so the
    // absence of subscription cleanup is now observable without a sleep.
    assert!(!harness.log.lock().iter().any(|instruction| matches!(instruction,
        ScreenInstruction::SubscribePaneOutput(plugin, client, None, _) if *plugin == PLUGIN && *client == CLIENT
    ) || matches!(instruction, ScreenInstruction::UnsubscribePaneOutput(plugin, _) if *plugin == PLUGIN)));
    harness.finish();
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
import type { Event } from "yinz:plugin";
let keys = 0;
let completed = 0;
export default {
  load() { yinz.subscribe(["Key"]); },
  async update(event: Event) {
    if (typeof event !== "object" || !("Key" in event)) return false;
    const ordinal = ++keys;
    const ready = yinz.waitForEvent("CustomMessage", ([name]) => name === "acceptance-release");
    yinz.requestRender();
    await ready;
    // Only the discarded instance reaches ordinal 2.
    yinz.command("SetSelectable", ordinal !== 2);
    completed++;
    return true;
  },
  render() { return `client=${yinz.getPluginIds().client_id} keys=${keys} completed=${completed}`; }
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
  if (yinz.getPluginIds().client_id !== 2) return;
  yinz.command('SetSelectable', false);
  for (;;) {}
}
export default {
  load() { yinz.subscribe(['Key']); LOAD },
  update(event) {
    if (!event.Key) return false;
    runaway();
    keys++;
    return true;
  },
  render() { return `client=${yinz.getPluginIds().client_id} keys=${keys}`; }
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
    // A late startup update must not block cancellation behind the loader's map lock.
    harness
        .sender
        .send(PluginInstruction::Update(vec![(
            Some(PLUGIN),
            None,
            Event::AvailableLayoutInfo(vec![], vec![]),
        )]))
        .unwrap();
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
fn deno_shared_infinite_load_can_unload_after_its_last_client_disconnects() {
    disconnect_shared_runaway(false);
}

#[test]
#[serial(plugin_tests)]
fn deno_shared_infinite_load_can_unload_after_its_owner_disconnects() {
    disconnect_shared_runaway(true);
}

fn disconnect_shared_runaway(another_client: bool) {
    let (finished, completion) = std::sync::mpsc::channel();
    let test = std::thread::spawn(move || {
        let harness = Harness::new(
            "js",
            r#"
export default {
  load() {
    yinz.command('SetSelectable', false);
    for (;;) {}
  }
};
"#,
            PermissionStatus::Granted,
        );
        if another_client {
            harness
                .sender
                .send(PluginInstruction::AddClient(ClientId::new(2)))
                .unwrap();
        }
        harness.load_shared();
        harness.log.wait_until(
            "the shared loader entered guest code while holding the plugin map",
            default_timeout(),
            |log| {
                log.iter()
                    .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, false)))
                    .then_some(())
            },
        );
        // Shared instances ignore client cancellation. Removal must return so the
        // next instruction can cancel the load and release its map lock.
        harness
            .sender
            .send(PluginInstruction::RemoveClient(CLIENT))
            .unwrap();
        harness
            .sender
            .send(PluginInstruction::Unload(PLUGIN))
            .unwrap();
        harness.log.wait_until(
            "the queued unload reached subscription cleanup",
            default_timeout(),
            |log| {
                log.iter()
                    .any(|i| matches!(i, ScreenInstruction::UnsubscribePaneOutput(id, None) if *id == PLUGIN))
                    .then_some(())
            },
        );
        harness.finish(); // Queues Exit and joins the plugin thread and its executors.
        finished.send(()).unwrap();
    });
    completion
        .recv_timeout(default_timeout() * 3)
        .expect("disconnect, queued Unload and Exit must complete during a shared infinite load");
    test.join().unwrap();
}

#[test]
#[serial(plugin_tests)]
fn deno_shared_instance_survives_its_owners_disconnect() {
    let harness = Harness::new(
        "js",
        r#"
let keys = 0, chunks = 0;
export default {
  async load() { yinz.subscribe(['Key', 'PaneOutput']); await yinz.waitForPermissions(['ReadPaneContents']); yinz.command('SubscribePaneOutput', {work:'one'}); },
  update(event) { if (event.Key) keys++; if (event.PaneOutput) chunks++; return true; },
  render() { return `shared keys=${keys} chunks=${chunks}`; }
};
"#,
        PermissionStatus::Granted,
    );
    harness.load_shared();
    harness.rendered("shared keys=0");
    let subscription =
        harness
            .log
            .wait_until("shared output subscription", default_timeout(), |log| {
                log.iter().find_map(|instruction| match instruction {
                    ScreenInstruction::SubscribePaneOutput(plugin, client, Some(_), generation) => {
                        Some((*plugin, *client, *generation))
                    },
                    _ => None,
                })
            });
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
    let output = || {
        PluginInstruction::for_plugin_event(
            subscription.0,
            subscription.1,
            subscription.2,
            Event::PaneOutput {
                pane_id: 7,
                data: vec![255],
                context: BTreeMap::new(),
            },
        )
    };
    harness.sender.send(output()).unwrap();
    harness.rendered("shared keys=1 chunks=1");
    harness
        .sender
        .send(PluginInstruction::AddClient(CLIENT))
        .unwrap();
    harness.sender.send(output()).unwrap();
    harness.rendered("shared keys=1 chunks=2");
    let log = harness.log.lock();
    assert_eq!(
        log.iter()
            .filter(|instruction| matches!(instruction, ScreenInstruction::SubscribePaneOutput(..)))
            .count(),
        1
    );
    assert!(!log
        .iter()
        .any(|instruction| matches!(instruction, ScreenInstruction::UnsubscribePaneOutput(..))));
    drop(log);
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
    await yinz.waitForEvent("CustomMessage", ([name]) => name === "acceptance-release");
    yinz.command("SetSelectable", label === "replacement");
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
fn deno_timer_before_unload_interrupts_initial_load() {
    cancel_load_after_timer(PluginInstruction::Unload(PLUGIN));
}

#[test]
#[serial(plugin_tests)]
fn deno_timer_before_disconnect_interrupts_initial_load() {
    cancel_load_after_timer(PluginInstruction::RemoveClient(CLIENT));
}

#[test]
#[serial(plugin_tests)]
fn deno_timer_before_shutdown_interrupts_initial_load() {
    cancel_load_after_timer(PluginInstruction::Exit);
}

fn cancel_load_after_timer(cancellation: PluginInstruction) {
    let (finished, completion) = std::sync::mpsc::channel();
    let test = std::thread::spawn(move || {
        let (to_proxy, outgoing): ChannelWithContext<PluginInstruction> = channels::unbounded();
        let to_proxy = SenderWithContext::new(to_proxy);
        let harness = Harness::with_event_sender(
            "js",
            r#"
export default {
  load() {
    yinz.command("SetTimeout", 0);
    for (;;) {}
  },
  render() { return "runaway load completed"; }
};
"#,
            None,
            Some(to_proxy.clone()),
        );
        let completions = Watched::new(Vec::new());
        let proxy = std::thread::spawn({
            let completions = completions.clone();
            let sender = harness.sender.clone();
            move || loop {
                let (instruction, _) = outgoing.recv_timeout(FAKE_SCREEN_RECV_TIMEOUT).unwrap();
                match instruction {
                    PluginInstruction::Exit => break,
                    timer @ PluginInstruction::DenoReply(_, _, _, Event::Timer(_), _) => {
                        completions.lock().push(timer);
                    },
                    other => {
                        let _ = sender.send(other);
                    },
                }
            }
        });
        harness.load();
        let timer = completions.wait_until(
            "the looping load produced a real host timer completion",
            default_timeout(),
            |events| events.first().cloned(),
        );
        // These share a FIFO: cancellation must be reachable after the real completion.
        harness.sender.send(timer).unwrap();
        let shutdown = matches!(cancellation, PluginInstruction::Exit);
        let disconnect = matches!(cancellation, PluginInstruction::RemoveClient(_));
        std::fs::write(&harness.path, "export default {};").unwrap();
        harness.sender.send(cancellation).unwrap();
        if !shutdown {
            if disconnect {
                harness
                    .sender
                    .send(PluginInstruction::AddClient(ClientId::new(2)))
                    .unwrap();
            }
            let survivor = harness.directory.path().join("survivor.js");
            std::fs::write(
                &survivor,
                "export default { render() { return 'survived timer cancellation'; } };",
            )
            .unwrap();
            load(&harness.sender, run_plugin(&survivor));
            harness.rendered("survived timer cancellation");
        }
        let log = harness.finish();
        assert!(!rendered_assets(&log)
            .iter()
            .any(|(_, frame)| frame.contains("runaway load completed")));
        to_proxy.send(PluginInstruction::Exit).unwrap();
        proxy.join().unwrap();
        finished.send(()).unwrap();
    });
    completion
        .recv_timeout(default_timeout() * 3)
        .expect("timer cancellation and teardown must complete");
    test.join().unwrap();
}

#[test]
#[serial(plugin_tests)]
fn deno_disconnect_interrupts_initial_load_before_the_instance_is_published() {
    let harness = Harness::new(
        "js",
        r#"
export default {
  load() {
    yinz.command("SetSelectable", false);
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
import type { Event } from "yinz:plugin";
export default {
  load() { yinz.subscribe(["BeforeClose"]); },
  update(event: Event) {
    if (event === "BeforeClose") {
      // Shutdown must still permit host calls from an idle plugin's close callback.
      yinz.command("SetSelectable", false);
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
    await yinz.waitForEvent("CustomMessage", ([name]) => name === "acceptance-release");
    yinz.command("SetSelectable", false);
  },
  async render() {
    const ready = yinz.waitForEvent("CustomMessage", ([name]) => name === "acceptance-release");
    // Load has already suspended; now render has registered its own await too.
    yinz.command("SetSelectable", true);
    await ready;
    yinz.command("SetSelectable", false);
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
    await yinz.waitForEvent("CustomMessage", ([name]) => name === "acceptance-release");
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
import type { Event } from "yinz:plugin";
let state = "ready to reject";
export default {
  load() { yinz.subscribe(["Key"]); },
  update(event: Event) {
    if (typeof event !== "object" || !("Key" in event)) return false;
    state = "awaiting rejection";
    yinz.requestRender();
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
  await yinz.waitForEvent("CustomMessage", ([name]) => name === "acceptance-release");
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
  load() { yinz.subscribe(["Key", "BeforeClose"]); },
  update(event) {
    if (event === "BeforeClose") { yinz.command("SetSelectable", false); return false; }
    yinz.command("SetSelectable", false);
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
    yinz.subscribe(["Key"]);
    yinz.command("SetSelectable", true);
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
    let selectable: Vec<_> = log
        .iter()
        .filter_map(|instruction| match instruction {
            ScreenInstruction::SetSelectable(_, value) => Some(*value),
            _ => None,
        })
        .collect();
    assert_eq!(
        selectable,
        [false, true],
        "interrupted guests skip cleanup, and the replacement retains host access"
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
fn deno_failed_reload_disposes_every_client_and_recovers_in_the_same_pane() {
    let harness = Harness::new(
        "js",
        r#"
export default {
  load() { yinz.subscribe(['BeforeClose', 'CustomMessage']); },
  update(event) {
    if (event === 'BeforeClose') yinz.command('SetSelectable', false);
    if (event.CustomMessage) yinz.command('SetSelectable', true);
    return true;
  },
  render() { return 'old guest ready'; }
};
"#,
        PermissionStatus::Granted,
    );
    let other = ClientId::new(2);
    harness
        .sender
        .send(PluginInstruction::AddClient(other))
        .unwrap();
    harness.load();
    let wait_for_both = |needle: &str| {
        harness
            .log
            .wait_until("both client instances rendered", default_timeout(), |log| {
                let assets = rendered_assets(log);
                [CLIENT, other]
                    .iter()
                    .all(|client| {
                        assets
                            .iter()
                            .any(|(id, frame)| id == client && frame.contains(needle))
                    })
                    .then_some(())
            });
    };
    wait_for_both("old guest ready");
    std::fs::write(&harness.path, "export default { syntax error").unwrap();
    harness.reload();
    harness.log.wait_until("replacement compilation failed", default_timeout(), |log| {
        log.iter().any(|i| matches!(i, ScreenInstruction::UpdatePluginLoadingStage(id, stage) if *id == PLUGIN && stage.is_error())).then_some(())
    });
    harness
        .sender
        .send(PluginInstruction::Update(vec![(
            Some(PLUGIN),
            None,
            release_event(),
        )]))
        .unwrap();
    std::fs::write(
        &harness.path,
        "export default { render() { return 'recovered guest'; } };",
    )
    .unwrap();
    harness.reload();
    wait_for_both("recovered guest");
    let log = harness.finish();
    let selectable: Vec<_> = log
        .iter()
        .filter_map(|i| match i {
            ScreenInstruction::SetSelectable(_, value) => Some(*value),
            _ => None,
        })
        .collect();
    assert_eq!(
        selectable,
        [false, false],
        "each old guest cleans up once and cannot handle input after a failed reload"
    );
    assert_eq!(
        log.iter()
            .filter(|i| matches!(i, ScreenInstruction::AddPlugin(..)))
            .count(),
        1,
        "recovery retains the pane"
    );
}

/// A reload that arrives while a healthy guest is inside a call must wait that call out: a
/// guest stopped mid-call is not given `BeforeClose`.
#[test]
#[serial(plugin_tests)]
fn deno_reload_by_url_waits_out_a_short_update_before_delivering_before_close() {
    let harness = Harness::new(
        "js",
        r#"
export default {
  load() { yinz.subscribe(["BeforeClose", "Key"]); },
  update(event) {
    if (event === "BeforeClose") {
      yinz.command("SetSelectable", false);
      return false;
    }
    yinz.command("SetSelectable", true);
    // Longer than the cancellation watcher's 10 ms poll, so a token set on arrival
    // would always catch this call; far shorter than the grace a reload gives it.
    const end = Date.now() + 100;
    while (Date.now() < end) {}
    return false;
  },
  render() { return "original ready for a short update"; }
};
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("original ready for a short update");
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
    harness.log.wait_until(
        "the old instance entered its update",
        default_timeout(),
        |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, true)))
                .then_some(())
        },
    );
    std::fs::write(
        &harness.path,
        r#"export default { render() { return "replacement after a short update"; } };"#,
    )
    .unwrap();
    harness
        .sender
        .send(PluginInstruction::ReloadPluginByUrl(run_plugin(
            &harness.path,
        )))
        .unwrap();
    harness.rendered("replacement after a short update");
    let log = harness.finish();
    let selectable: Vec<bool> = log
        .iter()
        .filter_map(|i| match i {
            ScreenInstruction::SetSelectable(_, value) => Some(*value),
            _ => None,
        })
        .collect();
    assert_eq!(
        selectable,
        [true, false],
        "the update returns on its own, then the original's BeforeClose runs once"
    );
}

#[test]
#[serial(plugin_tests)]
// Each synchronous call fits the three-second grace; together they cross the old
// call's deadline. The watched command establishes entry before requesting reload.
fn deno_reload_gives_before_close_its_own_grace_after_a_completed_call() {
    let harness = Harness::new(
        "js",
        r#"
export default {
  load() { yinz.subscribe(["BeforeClose", "Key"]); },
  update(event) {
    if (!event.Key && event !== "BeforeClose") return false;
    if (event.Key) yinz.command("SetSelectable", true);
    const end = Date.now() + 2000;
    while (Date.now() < end) {}
    if (event === "BeforeClose") yinz.command("SetSelectable", false);
    return false;
  },
  render() { return "original ready for timed cleanup"; }
};
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("original ready for timed cleanup");
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
    harness
        .log
        .wait_until("original invocation entered", default_timeout(), |log| {
            log.iter()
                .any(|i| matches!(i, ScreenInstruction::SetSelectable(_, true)))
                .then_some(())
        });
    std::fs::write(
        &harness.path,
        r#"export default {
      load() { yinz.command("SetSelectable", true); },
      render() { return "replacement after timed cleanup"; }
    };"#,
    )
    .unwrap();
    harness
        .sender
        .send(PluginInstruction::ReloadPluginByUrl(run_plugin(
            &harness.path,
        )))
        .unwrap();
    harness.rendered("replacement after timed cleanup");
    let log = harness.finish();
    let selectable: Vec<_> = log
        .iter()
        .filter_map(|i| match i {
            ScreenInstruction::SetSelectable(_, value) => Some(*value),
            _ => None,
        })
        .collect();
    assert_eq!(
        selectable,
        [true, false, true],
        "a completed invocation's watchdog interrupted eligible cleanup"
    );
}

#[test]
#[serial(plugin_tests)]
fn deno_reload_by_url_delivers_idle_before_close_before_loading_the_replacement() {
    let harness = Harness::new(
        "js",
        r#"
export default {
  load() { yinz.subscribe(["BeforeClose"]); },
  update(event) {
    if (event === "BeforeClose") yinz.command("SetSelectable", false);
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
  load() { yinz.command("SetSelectable", true); },
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
fn deno_reload_at_a_host_call_boundary_cleans_up_once_with_queued_work() {
    for queued in [false, true] {
        let harness = Harness::new(
            "js",
            r#"
let ready = false;
export default {
  async load() {
    await yinz.waitForPermissions(["ReadApplicationState"]);
    yinz.subscribe(["BeforeClose", "Key"]);
    ready = true;
  },
  update(event) {
    if (event === "BeforeClose") { yinz.command("SetSelectable", false); return false; }
    if (!event.Key) return false;
    yinz.command("DumpSessionLayout", {});
    return false;
  },
  render() { return ready ? "boundary original ready" : "waiting for permission"; }
};
"#,
            PermissionStatus::Granted,
        );
        harness.load();
        harness.rendered("boundary original ready");
        harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
        let reply =
            harness
                .log
                .wait_until("guest paused in a host call", default_timeout(), |log| {
                    log.iter().find_map(|instruction| match instruction {
                        ScreenInstruction::DumpLayoutToPlugin {
                            response_channel, ..
                        } => Some(response_channel.clone()),
                        _ => None,
                    })
                });
        std::fs::write(
            &harness.path,
            r#"export default {
          load() { yinz.command("SetSelectable", true); },
          render() { return "boundary replacement ready"; }
        };"#,
        )
        .unwrap();
        if queued {
            harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('b'))));
        }
        harness
            .sender
            .send(PluginInstruction::ReloadPluginByUrl(run_plugin(
                &harness.path,
            )))
            .unwrap();
        harness.log.wait_until("reload received while guest is in its call", default_timeout(), |log| {
            log.iter().any(|instruction| matches!(instruction,
                ScreenInstruction::NotificationPolicyUnavailable { reason, .. } if reason == "policy_reloaded"
            )).then_some(())
        });
        reply
            .send(yinz_bus::plugins::DumpSessionLayoutResponse {
                layout_result: Ok("layout {}".into()),
                metadata: None,
            })
            .unwrap();
        harness.rendered("boundary replacement ready");
        let log = harness.finish();
        assert_eq!(
            log.iter()
                .filter(|instruction| matches!(
                    instruction,
                    ScreenInstruction::DumpLayoutToPlugin { .. }
                ))
                .count(),
            1,
            "queued={queued}: callbacks retired at the reload boundary must not enter the host"
        );
        let selectable: Vec<_> = log
            .iter()
            .filter_map(|instruction| match instruction {
                ScreenInstruction::SetSelectable(_, value) => Some(*value),
                _ => None,
            })
            .collect();
        assert_eq!(
            selectable,
            [false, true],
            "queued={queued}: cleanup precedes replacement exactly once"
        );
    }
}

#[test]
#[serial(plugin_tests)]
fn deno_unload_during_reload_cleanup_does_not_load_the_replacement() {
    let harness = Harness::new(
        "js",
        r#"
let ready = false;
export default {
  async load() {
    await yinz.waitForPermissions(['ReadApplicationState']);
    yinz.subscribe(['BeforeClose']);
    ready = true;
  },
  update(event) {
    if (event !== 'BeforeClose') return false;
    yinz.command('SetSelectable', false);
    yinz.command('DumpSessionLayout', {});
    return false;
  },
  render() { return ready ? 'ready for cleanup unload' : 'starting'; }
};
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("ready for cleanup unload");
    std::fs::write(
        &harness.path,
        "export default { load() { yinz.command('SetSelectable', true); } };",
    )
    .unwrap();
    harness
        .sender
        .send(PluginInstruction::ReloadPluginByUrl(run_plugin(
            &harness.path,
        )))
        .unwrap();
    let reply = harness.log.wait_until(
        "reload cleanup is in a host call",
        default_timeout(),
        |log| {
            log.iter().find_map(|instruction| match instruction {
                ScreenInstruction::DumpLayoutToPlugin {
                    response_channel, ..
                } => Some(response_channel.clone()),
                _ => None,
            })
        },
    );
    harness
        .sender
        .send(PluginInstruction::Unload(PLUGIN))
        .unwrap();
    harness.log.wait_until(
        "explicit unload removed the old identity",
        default_timeout(),
        |log| {
            log.iter()
                .any(|instruction| {
                    matches!(instruction,
                        ScreenInstruction::ClearKeyPressesIntercepts(id, _) if *id == PLUGIN
                    )
                })
                .then_some(())
        },
    );
    reply
        .send(yinz_bus::plugins::DumpSessionLayoutResponse {
            layout_result: Ok("layout {}".into()),
            metadata: None,
        })
        .unwrap();
    let log = harness.finish();
    let selectable: Vec<_> = log
        .iter()
        .filter_map(|instruction| match instruction {
            ScreenInstruction::SetSelectable(_, value) => Some(*value),
            _ => None,
        })
        .collect();
    assert_eq!(
        selectable,
        [false],
        "cleanup runs once and explicit unload wins over replacement"
    );
}

#[test]
#[serial(plugin_tests)]
fn deno_unload_interrupts_an_infinite_update_without_host_calls() {
    unload_runaway(
        r#"
export default {
  load() { yinz.subscribe(["Key"]); },
  update() { yinz.command("SetSelectable", false); for (;;) {} },
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
  load() { yinz.command("SetSelectable", false); for (;;) {} }
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
    yinz.subscribe(['CustomMessage']);
    await yinz.waitForPermissions(['PluginData']);
    const d = yinz.data;
    d.define('watched', {value:'Text'});
    d.watch({entity:'watched'}, () => {changes++; yinz.requestRender();});
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
                changed @ PluginInstruction::DenoReply(
                    _,
                    _,
                    _,
                    Event::PluginDataChanged { .. },
                    _,
                ) => replies.lock().push(changed),
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
            PluginInstruction::DenoReply(_, _, old, _, _),
            PluginInstruction::DenoReply(_, _, new, _, _),
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
fn deno_has_stateless_llm_and_no_chat_namespace() {
    let harness = Harness::new(
        "ts",
        r#"
export default {
  render() { return typeof yinz.llm.complete === 'function' && !('hocus' in yinz) ? 'neutral completion API' : 'unexpected SDK'; }
};
"#,
        PermissionStatus::Granted,
    );
    load(
        &harness.sender,
        RunPluginOrAlias::RunPlugin(RunPlugin {
            location: RunPluginLocation::File(harness.path.clone()),
            ..Default::default()
        }),
    );
    harness.rendered("neutral completion API");
    harness.finish();
}

#[test]
fn deno_rpc_calls_peers_by_url_and_id() {
    for extension in ["ts", "js"] {
        let harness = Harness::new(
            "ts",
            r#"
let state = 'starting';
yinz.rpc.handle('back', async (payload, call) => ({value:payload, from:call.source.Plugin}));
export default {
  async load(config) {try {
    await yinz.waitForPermissions(['MessageAndLaunchOtherPlugins']);
    const first = await yinz.rpc.call({pluginUrl:config.peer,pluginConfig:{greeting:'Oslo'},name:'echo',payload:'hello',timeoutMs:10000});
    if (first.greeting !== 'Oslo' || first.back.value !== 'hello' || first.back.from !== first.id) throw Error('URL/config/reverse RPC mismatch');
    const second = await yinz.rpc.call({pluginId:first.id,name:'echo',payload:'again',timeoutMs:10000});
    if (second.back.value !== 'again') throw Error('ID RPC mismatch');
    for (const name of ['missing', 'failure']) {
      try {await yinz.rpc.call({pluginId:first.id,name,timeoutMs:10000}); throw Error('unexpected success');}
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
        std::fs::write(&peer, r#"
let ready, greeting;
yinz.rpc.handle('echo', async (payload, call) => {
  await ready;
  const back = await yinz.rpc.call({pluginId:call.source.Plugin,name:'back',payload,timeoutMs:10000});
  return {id:yinz.getPluginIds().plugin_id,greeting,back};
});
yinz.rpc.handle('failure', () => {throw Error('peer failed');});
export default {load(config) {greeting=config.greeting; ready=yinz.waitForPermissions(['MessageAndLaunchOtherPlugins']); return ready;}};
"#).unwrap();
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
    try {await yinz.rpc.call({pluginId:7,name:'echo',timeoutMs:100});}
    catch (error) {if (String(error).includes('MessageAndLaunchOtherPlugins')) denied++; else throw error;}
  };
  await attempt();
  try {await yinz.waitForPermissions(['MessageAndLaunchOtherPlugins']);} catch (_) {}
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
yinz.rpc.handle('echo', () => label);
export default {
  async load() {
    yinz.subscribe(['CustomMessage']);
    await yinz.waitForPermissions(['MessageAndLaunchOtherPlugins']);
    result = await yinz.rpc.call({pluginId:yinz.getPluginIds().plugin_id,name:'echo'});
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
                    if message.message_name == "yinz_rpc_reply" =>
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
    yinz_bus::background_jobs::background_jobs_main(
        Bus::new(vec![rx], None, None, None, None, None, None, None).should_silently_fail(),
        None,
        true,
    )
    .unwrap();
    let scan = yinz_bus::background_jobs::session_scan_state().unwrap();
    *scan.current_session_name.lock().unwrap() = "deno-completion-test".into();
    scan.current_session_info.lock().unwrap().name = "deno-completion-test".into();
    scan.current_session_plugin_list.lock().unwrap().insert(
        PluginId::new(7),
        RunPlugin {
            location: RunPluginLocation::Yinz(yinz_types::data::PluginTag::new("agent-service")),
            ..Default::default()
        },
    );
    let source = r#"
const label = 'old';
let state='starting', fences=0;
export default {
  async load() {
    yinz.subscribe(['CustomMessage']);
    try {await yinz.llm.complete({messages:[{role:'user'}]}); throw Error('discovery was unprotected');}
    catch(e) {if (!String(e).includes('ReadApplicationState')) throw e;}
    await yinz.waitForPermissions(['ReadApplicationState']);
    try {await yinz.llm.complete({messages:[{role:'user'}]}); throw Error('message was unprotected');}
    catch(e) {if (!String(e).includes('MessageAndLaunchOtherPlugins')) throw e;}
    await yinz.waitForPermissions(['MessageAndLaunchOtherPlugins']);
    state='waiting'; yinz.requestRender();
    const result=await yinz.llm.complete({messages:[{role:'user',content:label}]});
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
                } if message.message_name == "llm_complete" => {
                    assert_eq!(message.destination_plugin_id, Some(PluginId::new(7)));
                    let request: serde_json::Value =
                        serde_json::from_str(message.message_payload.as_ref().unwrap()).unwrap();
                    message.destination_plugin_id = Some(source_plugin_id);
                    message.message_name = "llm_complete_result".into();
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
        .find("    try {await yinz.llm.complete")
        .unwrap();
    let end = replacement.find("    state='waiting'").unwrap();
    let replacement = format!("{}    await yinz.waitForPermissions(['ReadApplicationState', 'MessageAndLaunchOtherPlugins']);\n{}", &replacement[..start], &replacement[end..]);
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
    use yinz_types::agent::{AgentEvent, AgentState};
    let source = r#"
const label='old';
let state='starting', heard=0;
export default {
  async load() {
    await yinz.waitForPermissions(['ReadApplicationState']);
    yinz.agent.on(()=>{heard++;});
    state='waiting'; yinz.requestRender();
    const turn=await yinz.agent.waitForTurn({Terminal:7});
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

const KEYBINDINGS_PLUGIN: &str = r#"
let count = 0, state = 'loading';
export default {
  async load() {
    try {
      await yinz.waitForPermissions(['Reconfigure']);
      yinz.registerDefaultKeybindings([
        {mode: 'normal', key: 'Ctrl g', handler(message) { if (message.source !== 'Keybind') throw Error('source'); count++; return true; }},
        {mode: 'normal', key: 'Ctrl n', actions: [yinz.action('NewPane')]},
      ]);
      state = 'registered';
    } catch (error) { state = 'refused: ' + error; }
    return true;
  },
  pipe() { state = 'leaked'; return true; },
  render() { return `${state} count=${count}`; },
};
"#;

fn keybind_pipe(name: &str) -> PluginInstruction {
    PluginInstruction::KeybindPipe {
        name: name.to_owned(),
        payload: None,
        plugin: None,
        args: None,
        configuration: None,
        floating: None,
        should_open_in_place: false,
        pane_id_to_replace: None,
        pane_title: None,
        cwd: None,
        skip_cache: false,
        cli_client_id: CLIENT,
        plugin_and_client_id: Some((PLUGIN, CLIENT)),
        notification_end: None,
    }
}

#[test]
fn deno_default_keybindings_register_after_the_reconfigure_grant_and_dispatch_their_reserved_pipe()
{
    use yinz_types::input::actions::Action;
    let directory = tempdir().unwrap();
    let path = directory.path().join("keybindings.ts");
    std::fs::write(&path, KEYBINDINGS_PLUGIN).unwrap();
    let (sender, server_receiver, screen_receiver, teardown) =
        create_plugin_thread_with_server_receiver(Some(directory.path().to_owned()), None);
    let log = Watched::new(vec![]);
    let logger = permissions_and_log_until_disconnect(
        &log,
        screen_receiver,
        directory.path().join("permissions.kdl"),
        sender.clone(),
        CLIENT,
        PermissionStatus::Granted,
    );
    let rebinds = Watched::new(vec![]);
    let server_logger =
        log_actions_in_thread_struct!(rebinds, ServerInstruction::KillSession, server_receiver, 1);
    sender.send(PluginInstruction::AddClient(CLIENT)).unwrap();
    load(&sender, run_plugin(&path));
    wait_for_render_containing(&log, "registered count=0");
    let registered = rebinds.wait_until(
        "the Deno plugin's default keybindings reached the server",
        default_timeout(),
        |instructions| {
            instructions
                .iter()
                .find_map(|instruction| match instruction {
                    ServerInstruction::RebindKeys {
                        keys_to_rebind,
                        keys_to_unbind,
                        write_config_to_disk,
                        plugin_id,
                        ..
                    } => Some((
                        keys_to_rebind.clone(),
                        keys_to_unbind.clone(),
                        *write_config_to_disk,
                        *plugin_id,
                    )),
                    _ => None,
                })
        },
    );
    let (keys_to_rebind, keys_to_unbind, write_config_to_disk, plugin_id) = registered;
    assert!(keys_to_unbind.is_empty() && !write_config_to_disk);
    assert_eq!(plugin_id, Some(PLUGIN));
    assert_eq!(keys_to_rebind.len(), 2);
    // The host rewrote the reserved MessagePlugin name to address this plugin.
    assert!(keys_to_rebind.iter().any(|(_, _, actions)| matches!(
        actions.as_slice(),
        [Action::KeybindPipe { name: Some(name), plugin_id: Some(id), .. }]
            if name == "__yinz_default_keybinding:1" && *id == PLUGIN
    )));
    assert!(keys_to_rebind
        .iter()
        .any(|(_, _, actions)| matches!(actions.as_slice(), [Action::NewPane { .. }])));
    sender
        .send(keybind_pipe("__yinz_default_keybinding:1"))
        .unwrap();
    wait_for_render_containing(&log, "registered count=1");
    sender
        .send(keybind_pipe("__yinz_default_keybinding:1"))
        .unwrap();
    wait_for_render_containing(&log, "registered count=2");
    // A stale index is consumed without reaching the plugin's own pipe entry point.
    sender
        .send(keybind_pipe("__yinz_default_keybinding:9"))
        .unwrap();
    sender
        .send(keybind_pipe("__yinz_default_keybinding:1"))
        .unwrap();
    let frame = wait_for_render_containing(&log, "count=3");
    assert!(frame.contains("registered count=3"), "{frame}");
    teardown();
    logger.join().unwrap();
    server_logger.join().unwrap();
}

#[test]
fn deno_default_keybindings_are_refused_without_the_reconfigure_grant() {
    let harness = Harness::new(
        "ts",
        r#"
let state = 'loading';
export default {
  load() {
    try { yinz.registerDefaultKeybindings([{mode: 'normal', key: 'Ctrl g', actions: ['NewPane']}]); state = 'registered'; }
    catch (error) { state = String(error); }
    return true;
  },
  render() { return state; },
};
"#,
        PermissionStatus::Denied,
    );
    harness.load();
    let frame = harness.rendered("Reconfigure");
    assert!(
        frame.contains("requires the 'Reconfigure' permission"),
        "{frame}"
    );
}

/// A plugin that asked in `load` is refused everything until the answer arrives as an event,
/// and telling it to ask names what it already did; a plugin that never asked needs to ask.
#[test]
fn deno_a_denial_before_any_answer_says_to_wait_for_the_answer() {
    let harness = Harness::with_permission_reply(
        "ts",
        r#"
let state = 'loading';
export default {
  load() {
    yinz.requestPermission(['PluginData']);
    try { yinz.data.fetch({entity: 'note'}); state = 'fetched'; } catch (error) { state = String(error); }
    return true;
  },
  render() { return state; },
};
"#,
        None,
    );
    harness.load();
    let frame = harness.rendered("PluginData");
    assert!(
        frame.contains("requires the 'PluginData' permission"),
        "{frame}"
    );
    assert!(
        frame.contains("PermissionRequestResult") && frame.contains("waitForPermissions"),
        "the denial has to name what the plugin is waiting for, got: {frame}"
    );
}

#[test]
fn deno_a_denial_with_no_request_behind_it_says_to_ask() {
    let harness = Harness::with_permission_reply(
        "ts",
        r#"
let state = 'loading';
export default {
  load() {
    try { yinz.data.fetch({entity: 'note'}); state = 'fetched'; } catch (error) { state = String(error); }
    return true;
  },
  render() { return state; },
};
"#,
        None,
    );
    harness.load();
    let frame = harness.rendered("PluginData");
    assert!(
        frame.contains("requires the 'PluginData' permission"),
        "{frame}"
    );
    assert!(
        frame.contains("requestPermission(['PluginData'])"),
        "{frame}"
    );
    assert!(
        !frame.contains("PermissionRequestResult"),
        "nothing is pending for a plugin that never asked, got: {frame}"
    );
}

#[test]
fn deno_a_denial_after_an_answer_that_left_this_permission_out_says_to_ask() {
    let harness = Harness::new(
        "ts",
        r#"
let state = 'loading';
export default {
  async load() {
    await yinz.waitForPermissions(['ReadFiles']);
    try { yinz.data.fetch({entity: 'note'}); state = 'fetched'; } catch (error) { state = String(error); }
    yinz.requestRender();
  },
  render() { return state; },
};
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    let frame = harness.rendered("PluginData");
    assert!(
        frame.contains("requires the 'PluginData' permission"),
        "{frame}"
    );
    assert!(
        frame.contains("requestPermission(['PluginData'])"),
        "{frame}"
    );
    assert!(
        !frame.contains("PermissionRequestResult"),
        "an answered plugin is not waiting for anything, got: {frame}"
    );
}

#[test]
fn deno_worker_permission_grant_retains_the_parents_run_commands_grant() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("permissions.ts");
    std::fs::write(
        &path,
        r#"
let state = 'starting';
export default {
  workers: {async change() {
    await yinz.waitForPermissions(['ChangeApplicationState']);
    yinz.command('RenameTabWithId', [1, 'worker changed tab']);
    yinz.postMessageToPlugin('changed');
  }},
  async load() {
    await yinz.waitForPermissions(['RunCommands']);
    const changed = yinz.waitForEvent('CustomMessage', ([name]) => name === 'changed');
    yinz.postMessageTo('change', 'go');
    await changed;
    yinz.command('RunCommand', [['/usr/bin/true'], {}, '.', {}, null, null]);
    state = 'parent and worker commands dispatched';
  },
  render() {return state;}
};
"#,
    )
    .unwrap();
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
    harness.load();
    harness.rendered("parent and worker commands dispatched");
    jobs.wait_until(
        "the parent retained RunCommands",
        default_timeout(),
        |jobs| {
            jobs.iter()
                .any(|job| matches!(job, BackgroundJob::RunCommand(..)))
                .then_some(())
        },
    );
    let log = harness.finish();
    assert!(log.iter().any(|instruction| matches!(instruction,
        ScreenInstruction::RenameTabWithId(_, name, _) if name == b"worker changed tab")));
    let requests: Vec<_> = log
        .iter()
        .filter_map(|instruction| match instruction {
            ScreenInstruction::RequestPluginPermissions(_, request) => {
                Some(request.permissions.clone())
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        requests,
        vec![
            vec![PermissionType::RunCommands],
            vec![
                PermissionType::RunCommands,
                PermissionType::ChangeApplicationState
            ],
        ]
    );
    job_logger.join().unwrap();
}

impl Harness {
    fn permission_requests(&self, count: usize) -> Vec<Vec<PermissionType>> {
        self.log
            .wait_until("the next permission prompt", default_timeout(), |log| {
                let requests: Vec<_> = log
                    .iter()
                    .filter_map(|instruction| match instruction {
                        ScreenInstruction::RequestPluginPermissions(_, request) => {
                            Some(request.permissions.clone())
                        },
                        _ => None,
                    })
                    .collect();
                (requests.len() >= count).then_some(requests)
            })
    }

    fn answer_permissions(&self, _permissions: Vec<PermissionType>, status: PermissionStatus) {
        self.sender
            .send(PluginInstruction::PermissionAnswer {
                request_id: self
                    .log
                    .lock()
                    .iter()
                    .rev()
                    .find_map(|instruction| match instruction {
                        ScreenInstruction::RequestPluginPermissions(_, request) => {
                            Some(request.request_id)
                        },
                        _ => None,
                    })
                    .unwrap(),
                status,
                cache_path: Some(self.directory.path().join("permissions.kdl")),
            })
            .unwrap();
    }
}

#[test]
fn deno_parent_and_two_workers_share_one_permission_fifo_and_keep_callback_clients() {
    for second_status in [PermissionStatus::Granted, PermissionStatus::Denied] {
        let (to_proxy, outgoing): ChannelWithContext<PluginInstruction> = channels::unbounded();
        let to_proxy = SenderWithContext::new(to_proxy);
        let harness = Harness::with_event_sender(
            "ts",
            r#"
let state = 'ready';
async function request(name, permission) {
  const result = yinz.waitForPermissions([permission]);
  if (name === 'parent') yinz.postMessageTo('first', 'go');
  if (name === 'first') yinz.postMessageTo('second', 'go');
  yinz.postMessageToPlugin('queued', name);
  let status = 'granted';
  try {await result;} catch (_) {status = 'denied';}
  yinz.postMessageToPlugin('settled', `${name}:${status}:${yinz.getPluginIds().client_id}`);
}
export default {
  workers: {
    first() {return request('first', 'ChangeApplicationState');},
    second() {return request('second', 'ReadFiles');},
  },
  load() {yinz.subscribe(['Key', 'CustomMessage']);},
  update(event) {if (event.Key) return request('parent', 'RunCommands');},
  render() {return state;}
};
"#,
            None,
            Some(to_proxy.clone()),
        );
        let messages = Watched::new(Vec::<(String, String, ClientId)>::new());
        let load_forwarded = Watched::new(false);
        let proxy = std::thread::spawn({
            let messages = messages.clone();
            let load_forwarded = load_forwarded.clone();
            let sender = harness.sender.clone();
            move || loop {
                let (instruction, _) = outgoing.recv_timeout(FAKE_SCREEN_RECV_TIMEOUT).unwrap();
                if matches!(instruction, PluginInstruction::Exit) {
                    break;
                }
                if let PluginInstruction::DenoReply(
                    _,
                    client,
                    _,
                    Event::CustomMessage(name, payload),
                    _,
                ) = &instruction
                {
                    messages
                        .lock()
                        .push((name.clone(), payload.clone(), *client));
                }
                let finishes_loading = matches!(
                    &instruction,
                    PluginInstruction::ApplyCachedEvents { plugin_ids, .. }
                        if plugin_ids.contains(&PLUGIN)
                );
                if sender.send(instruction).is_ok() && finishes_loading {
                    *load_forwarded.lock() = true;
                }
            }
        });
        harness.load_shared();
        harness.rendered("ready");
        // A ready render can precede startup replay; enqueue replay before client 2's input.
        load_forwarded.wait_until("startup replay forwarded", default_timeout(), |forwarded| {
            forwarded.then_some(())
        });
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
        messages.wait_until(
            "all three requests are queued",
            default_timeout(),
            |messages| {
                (messages
                    .iter()
                    .filter(|(name, _, _)| name == "queued")
                    .count()
                    == 3)
                    .then_some(())
            },
        );
        let settled = |count| {
            messages.wait_until(
                "only the answered requester completes",
                default_timeout(),
                |messages| {
                    let settled: Vec<_> = messages
                        .iter()
                        .filter(|(name, _, _)| name == "settled")
                        .map(|(_, payload, client)| (payload.clone(), *client))
                        .collect();
                    (settled.len() >= count).then_some(settled)
                },
            )
        };
        assert_eq!(settled(0), vec![]);
        assert_eq!(
            harness.permission_requests(1),
            vec![vec![PermissionType::RunCommands]]
        );
        harness.answer_permissions(vec![PermissionType::RunCommands], PermissionStatus::Granted);
        assert_eq!(settled(1), vec![("parent:granted:2".into(), other)]);
        let requests = harness.permission_requests(2);
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[1],
            vec![
                PermissionType::RunCommands,
                PermissionType::ChangeApplicationState
            ]
        );
        harness.answer_permissions(requests[1].clone(), second_status);
        let status = if second_status == PermissionStatus::Granted {
            "granted"
        } else {
            "denied"
        };
        assert_eq!(
            settled(2),
            vec![
                ("parent:granted:2".into(), other),
                (format!("first:{status}:2"), other)
            ]
        );
        let requests = harness.permission_requests(3);
        assert_eq!(requests.len(), 3);
        let mut expected = vec![PermissionType::RunCommands];
        if second_status == PermissionStatus::Granted {
            expected.push(PermissionType::ChangeApplicationState);
        }
        expected.push(PermissionType::ReadFiles);
        assert_eq!(requests[2], expected);
        harness.answer_permissions(requests[2].clone(), PermissionStatus::Granted);
        assert_eq!(
            settled(3),
            vec![
                ("parent:granted:2".into(), other),
                (format!("first:{status}:2"), other),
                ("second:granted:2".into(), other)
            ]
        );
        harness.finish();
        to_proxy.send(PluginInstruction::Exit).unwrap();
        proxy.join().unwrap();
    }
}

#[test]
fn deno_reload_drops_a_pending_workers_permission_completion() {
    let (to_proxy, outgoing): ChannelWithContext<PluginInstruction> = channels::unbounded();
    let to_proxy = SenderWithContext::new(to_proxy);
    let source = r#"
const label = 'original';
export default {
  workers: {async ask() {
    const answer = yinz.waitForPermissions(['RunCommands']);
    yinz.postMessageToPlugin('queued', label);
    try {
      await answer;
      yinz.postMessageToPlugin('settled', `${label}:granted`);
    } catch (_) {yinz.postMessageToPlugin('settled', `${label}:denied`);}
  }},
  load() {yinz.postMessageTo('ask', 'go');},
  update(event) {
    if (event.PermissionRequestResult) yinz.postMessageToPlugin('observed', label);
  },
  render() {return label;}
};
"#;
    let harness = Harness::with_event_sender("js", source, None, Some(to_proxy.clone()));
    let messages = Watched::new(vec![]);
    let proxy = std::thread::spawn({
        let messages = messages.clone();
        let sender = harness.sender.clone();
        move || loop {
            let (instruction, _) = outgoing.recv_timeout(FAKE_SCREEN_RECV_TIMEOUT).unwrap();
            if matches!(instruction, PluginInstruction::Exit) {
                break;
            }
            if matches!(
                instruction,
                PluginInstruction::DenoReply(_, _, _, Event::CustomMessage(..), _)
            ) {
                messages.lock().push(instruction.clone());
            }
            let _ = sender.send(instruction);
        }
    });
    let message = |name: &str, payload: &str| {
        messages.wait_until("the worker's message", default_timeout(), |messages| {
        messages.iter().find(|instruction| matches!(instruction,
            PluginInstruction::DenoReply(_, _, _, Event::CustomMessage(n, p), _) if n == name && p == payload)).cloned()
    })
    };
    harness.load();
    harness.permission_requests(1);
    let PluginInstruction::DenoReply(plugin, client, old_generation, _, _) =
        message("queued", "original")
    else {
        unreachable!()
    };
    std::fs::write(&harness.path, source.replace("'original'", "'replacement'")).unwrap();
    harness.reload();
    let requests = harness.permission_requests(2);
    assert_eq!(requests.len(), 2);
    let PluginInstruction::DenoReply(_, _, new_generation, _, _) = message("queued", "replacement")
    else {
        unreachable!()
    };
    assert_ne!(old_generation, new_generation);
    // A late reply addressed to the old parent and an ordinary broadcast cannot complete
    // the replacement's identically numbered request for the same permission.
    harness
        .sender
        .send(PluginInstruction::DenoEvent(
            plugin,
            client,
            old_generation,
            Event::PermissionRequestResult(PermissionStatus::Granted),
        ))
        .unwrap();
    harness.event(Event::PermissionRequestResult(PermissionStatus::Granted));
    harness.answer_permissions(requests[1].clone(), PermissionStatus::Denied);
    message("settled", "replacement:denied");
    assert_eq!(
        messages
            .lock()
            .iter()
            .filter_map(|instruction| match instruction {
                PluginInstruction::DenoReply(_, _, _, Event::CustomMessage(name, payload), _)
                    if name == "settled" =>
                    Some(payload.clone()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec!["replacement:denied"]
    );
    harness.finish();
    to_proxy.send(PluginInstruction::Exit).unwrap();
    proxy.join().unwrap();
}

#[test]
fn deno_workers_keep_isolated_state_order_async_timers_and_shared_permissions() {
    let harness = Harness::new(
        "ts",
        r#"
let calls = 0;
let state = 'starting';
async function work(name, payload) {
  if (name === 'denied') {
    try { yinz.command('RenameTabWithId', [1, 'not allowed']); }
    catch (_) { yinz.postMessageToPlugin('denied'); return; }
    throw Error('worker bypassed permissions');
  }
  if (name === 'error') throw Error('expected worker error');
  await yinz.sleep(1);
  yinz.command('RenameTabWithId', [1, 'worker granted']);
  yinz.postMessageToPlugin('done', `${name}:${++calls}:${payload}`);
}
export default {
  workers: {first:work, second:work},
  async load() {
    calls = 99;
    yinz.subscribe(['CustomMessage']);
    let refused = false;
    try {yinz.postMessageTo('missing', 'x');} catch (error) {refused = String(error).includes('not registered');}
    if (!refused) throw Error('missing worker was accepted');
    const denied = yinz.waitForEvent('CustomMessage', ([name]) => name === 'denied');
    yinz.postMessageTo('first', 'denied');
    await denied;
    await yinz.waitForPermissions(['ChangeApplicationState']);
    const results = [];
    yinz.on('CustomMessage', ([name, payload]) => {
      if (name === 'done') results.push(payload);
      if (results.length === 4) {
        const first = results.filter(s => s.startsWith('first'));
        if (first.join() !== 'first:1:hé🙂,first:2:two,first:3:three' || !results.includes('second:1:other') || calls !== 99) throw Error('worker state or order: ' + results);
        state = 'workers verified'; return true;
      }
    });
    yinz.postMessageTo('first', 'first', 'hé🙂');
    yinz.host.postMessageTo({worker_name:'second', name:'second', payload:'other'});
    yinz.command('PostMessageTo', {worker_name:'first', name:'first', payload:'two'});
    yinz.postMessageTo('first', 'error');
    yinz.command({PostMessageTo:{worker_name:'first', name:'first', payload:'three'}});
  },
  render() {return state;}
};
"#,
        PermissionStatus::Granted,
    );
    harness.load();
    harness.rendered("workers verified");
}

#[test]
fn deno_busy_workers_leave_the_ui_responsive_and_stop_with_the_owner() {
    for cleanup in ["reload", "unload", "disconnect", "shutdown"] {
        let harness = Harness::new(
            "js",
            r#"
let state = 'starting';
export default {
  workers: {spin() {yinz.postMessageToPlugin('started'); while (true) {}}},
  load() {yinz.subscribe(['CustomMessage']); yinz.postMessageTo('spin', 'go');},
  update(event) {
    if (event.CustomMessage?.[0] === 'started') state = 'worker is busy';
    if (event.CustomMessage?.[0] === 'acceptance-release') state = 'UI remains responsive';
    return true;
  },
  render() {return state;}
};
"#,
            PermissionStatus::Granted,
        );
        harness.load();
        harness.rendered("worker is busy");
        harness.event(release_event());
        harness.rendered("UI remains responsive");
        if cleanup == "reload" {
            std::fs::write(
                &harness.path,
                "export default {render() {return 'worker replacement loaded';}};",
            )
            .unwrap();
            harness.reload();
            harness.rendered("worker replacement loaded");
        } else if cleanup == "unload" {
            harness
                .sender
                .send(PluginInstruction::Unload(PLUGIN))
                .unwrap();
        } else if cleanup == "disconnect" {
            harness
                .sender
                .send(PluginInstruction::RemoveClient(CLIENT))
                .unwrap();
        }
        harness.finish();
    }
}

#[test]
fn deno_worker_list_clients_replies_are_generation_scoped_and_release_its_fifo() {
    let source = r#"
const label = 'original';
let state = 'starting', parentReplies = 0;
export default {
  workers: {async clients(name) {
    if (name === 'query') {
      const reply = yinz.waitForEvent('ListClients');
      yinz.host.listClients();
      const clients = await reply;
      if (clients.length !== 1 || clients[0].client_id !== 1 || !clients[0].is_current_client) throw Error('wrong clients reply');
      yinz.postMessageToPlugin('done');
    } else {
      yinz.postMessageToPlugin('after');
    }
  }},
  async load() {
    await yinz.waitForPermissions(['ReadApplicationState']);
    yinz.on('ListClients', () => {parentReplies++;});
    yinz.subscribe(['ListClients', 'CustomMessage']);
    yinz.on('CustomMessage', ([name]) => {state += ':' + name; return true;});
    yinz.postMessageTo('clients', 'query');
    yinz.postMessageTo('clients', 'after');
  },
  render() {return `${label} ${state} parent=${parentReplies}`;}
};
"#;
    let harness = Harness::new("ts", source, PermissionStatus::Granted);
    let request = |index| {
        harness
            .log
            .wait_until("worker ListClients request", default_timeout(), |log| {
                log.iter()
                    .filter_map(|instruction| match instruction {
                        ScreenInstruction::ListClientsToPlugin(plugin, client, generation) => {
                            Some((*plugin, *client, *generation))
                        },
                        _ => None,
                    })
                    .nth(index)
            })
    };
    harness.load();
    let old = request(0);
    std::fs::write(&harness.path, source.replace("'original'", "'replacement'")).unwrap();
    harness.reload();
    let current = request(1);
    assert!(old.2.is_some());
    assert!(current.2.is_some());
    assert_ne!(old.2, current.2);
    // An empty stale reply would fail the worker's assertion if it reached the replacement.
    for (origin, metadata) in [
        (old, Default::default()),
        (current, list_clients_metadata()),
    ] {
        harness
            .sender
            .send(PluginInstruction::ListClientsToPlugin(
                metadata, origin.0, origin.1, origin.2,
            ))
            .unwrap();
    }
    harness.rendered("replacement starting:done:after parent=0");
}

fn list_clients_metadata() -> yinz_bus::session_layout_metadata::SessionLayoutMetadata {
    use yinz_bus::session_layout_metadata::{PaneLayoutMetadata, SessionLayoutMetadata};
    let mut metadata = SessionLayoutMetadata::default();
    metadata.add_tab(
        "clients".into(),
        true,
        true,
        false,
        vec![PaneLayoutMetadata::new(
            PaneId::Terminal(crate::TerminalId::new(7)),
            Default::default(),
            false,
            None,
            None,
            true,
            None,
            vec![CLIENT],
            None,
            None,
        )],
        vec![],
    );
    metadata
}

#[test]
#[serial(plugin_tests)]
fn wasm_list_clients_reply_keeps_the_untagged_event_path() {
    let harness = Harness::new("ts", "export default {};", PermissionStatus::Granted);
    load(&harness.sender, run_plugin(&plugin_fixture_path()));
    wait_for_permissions(&harness.log);
    harness.event(Event::Key(
        KeyWithModifier::new(BareKey::Char('z')).with_alt_modifier(),
    ));
    let (plugin, client, generation) =
        harness
            .log
            .wait_until("Wasm ListClients request", default_timeout(), |log| {
                log.iter().find_map(|instruction| match instruction {
                    ScreenInstruction::ListClientsToPlugin(plugin, client, generation) => {
                        Some((*plugin, *client, *generation))
                    },
                    _ => None,
                })
            });
    assert_eq!(generation, None);
    harness
        .sender
        .send(PluginInstruction::ListClientsToPlugin(
            list_clients_metadata(),
            plugin,
            client,
            generation,
        ))
        .unwrap();
    let frame = harness.rendered("ListClients");
    assert!(frame.contains("is_current_client: true"), "{frame}");
}

#[test]
fn deno_worker_pane_replies_cannot_reach_a_reloaded_parent_or_worker() {
    let source = r#"
const label = 'original';
let state = 'starting', replies = 0;
const events = ['CommandPaneOpened', 'CommandPaneReRun', 'CommandPaneExited', 'EditPaneOpened', 'EditPaneExited'];
export default {
  workers: {async runner() {
    for (const event of events) yinz.on(event, () => { replies++; });
    yinz.subscribe(events);
    const waiting = yinz.runInPane(7, {path:'test'});
    yinz.postMessageToPlugin('pending');
    const result = await waiting;
    yinz.postMessageToPlugin('done', `${result.exitCode}:${replies}`);
  }},
  async load() {
    await yinz.waitForPermissions(['RunCommands', 'ReadApplicationState']);
    for (const event of events) yinz.on(event, () => { replies++; });
    yinz.subscribe([...events, 'CustomMessage']);
    yinz.on('CustomMessage', ([name, payload]) => {state = `${name}:${payload}`; return true;});
    yinz.postMessageTo('runner', 'go');
  },
  render() {return `${label} ${state} parent=${replies}`;}
};
"#;
    let harness = Harness::new("ts", source, PermissionStatus::Granted);
    let origin = |index| {
        harness
            .log
            .wait_until("worker dispatched runInPane", default_timeout(), |log| {
                log.iter()
                    .filter_map(|instruction| match instruction {
                        ScreenInstruction::RunCommandInPane(_, command, _) => {
                            command.originating_plugin.clone()
                        },
                        _ => None,
                    })
                    .nth(index)
            })
    };
    harness.load();
    let old = origin(0);
    harness.rendered("original pending: parent=0");
    std::fs::write(&harness.path, source.replace("'original'", "'replacement'")).unwrap();
    harness.reload();
    let current = origin(1);
    assert!(old.deno_generation.is_some());
    assert!(current.deno_generation.is_some());
    assert_ne!(old.deno_generation, current.deno_generation);
    harness.rendered("replacement pending: parent=0");
    let terminal = crate::TerminalId::new(7);
    for event in [
        Event::CommandPaneOpened(terminal, old.context.clone()),
        Event::CommandPaneReRun(terminal, old.context.clone()),
        Event::CommandPaneExited(terminal, Some(99), old.context.clone()),
        Event::EditPaneOpened(terminal, old.context.clone()),
        Event::EditPaneExited(terminal, Some(99), old.context.clone()),
    ] {
        harness
            .sender
            .send(PluginInstruction::for_plugin_event(
                old.plugin_id,
                old.client_id,
                old.deno_generation,
                event,
            ))
            .unwrap();
    }
    for event in [
        Event::CommandPaneReRun(terminal, current.context.clone()),
        Event::CommandPaneExited(terminal, Some(0), current.context.clone()),
    ] {
        harness
            .sender
            .send(PluginInstruction::for_plugin_event(
                current.plugin_id,
                current.client_id,
                current.deno_generation,
                event,
            ))
            .unwrap();
    }
    harness.rendered("replacement done:0:2 parent=0");
    harness.finish();
}

#[test]
fn deno_workers_preserve_shared_clients_and_discard_replies_after_reload() {
    let (to_proxy, outgoing): ChannelWithContext<PluginInstruction> = channels::unbounded();
    let to_proxy = SenderWithContext::new(to_proxy);
    let harness = Harness::with_event_sender(
        "ts",
        r#"
let state = 'worker ready';
export default {
  workers: {async echo() {
    const before = yinz.getPluginIds().client_id;
    await yinz.sleep(1);
    yinz.postMessageToPlugin('echo', `${before}:${yinz.getPluginIds().client_id}`);
  }},
  load() {yinz.subscribe(['Key', 'CustomMessage']);},
  update(event) {
    if (event.Key) yinz.postMessageTo('echo', 'go');
    if (event.CustomMessage) state = `worker client ${event.CustomMessage[1]}:${yinz.getPluginIds().client_id}`;
    return true;
  },
  render() {return state;}
};
"#,
        None,
        Some(to_proxy.clone()),
    );
    let captured = Watched::new(vec![]);
    let proxy = std::thread::spawn({
        let captured = captured.clone();
        let sender = harness.sender.clone();
        move || loop {
            let (instruction, _) = outgoing.recv_timeout(FAKE_SCREEN_RECV_TIMEOUT).unwrap();
            if matches!(instruction, PluginInstruction::Exit) {
                break;
            }
            if matches!(instruction, PluginInstruction::DenoReply(..)) {
                captured.lock().push(instruction.clone());
            }
            let _ = sender.send(instruction);
        }
    });
    harness.load_shared();
    harness.rendered("worker ready");
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
    harness.rendered("worker client 2:2:2");
    let stale = captured.lock().clone();
    assert!(stale.iter().any(|message| matches!(
        message,
        PluginInstruction::DenoReply(_, _, _, Event::Timer(_), _)
    )));
    assert!(stale.iter().any(|message| matches!(
        message,
        PluginInstruction::DenoReply(_, _, _, Event::CustomMessage(..), _)
    )));
    harness
        .sender
        .send(PluginInstruction::AddClient(CLIENT))
        .unwrap();
    std::fs::write(
        &harness.path,
        r#"
let replies = 0, keys = 0;
export default {
  load() {yinz.subscribe(['CustomMessage','Key']);},
  update(event) {if (event.CustomMessage) replies++; if (event.Key) keys++; return true;},
  render() {return `replacement replies=${replies} keys=${keys}`;}
};
"#,
    )
    .unwrap();
    harness
        .sender
        .send(PluginInstruction::ReloadPluginWithId(PLUGIN))
        .unwrap();
    harness.rendered("replacement replies=0 keys=0");
    for message in stale {
        harness.sender.send(message).unwrap();
    }
    harness.event(Event::Key(KeyWithModifier::new(BareKey::Char('a'))));
    harness.rendered("replacement replies=0 keys=1");
    harness.finish();
    to_proxy.send(PluginInstruction::Exit).unwrap();
    proxy.join().unwrap();
}

// Filesystem roots ---------------------------------------------------------------------------
//
// The `Fs*` handlers are shared with Wasm. These tests cover them in
// depth; what these establish is that a Deno plugin reaches the same four roots through the
// same environment, with the same refusals, and that `/host` follows `ChangeHostFolder`.

struct FsDirs {
    host: tempfile::TempDir,
    data: tempfile::TempDir,
    cache: tempfile::TempDir,
    plugins: tempfile::TempDir,
}

impl FsDirs {
    fn new() -> Self {
        FsDirs {
            host: tempfile::tempdir().unwrap(),
            data: tempfile::tempdir().unwrap(),
            cache: tempfile::tempdir().unwrap(),
            plugins: tempfile::tempdir().unwrap(),
        }
    }
}

/// Run `body` as a Deno plugin's `render`, on an environment rooted in `dirs`, and hand back
/// what it returned - or `error: <message>` when it threw. No plugin thread: `yinz.fs.*` is
/// synchronous, so `render` alone exercises the whole path.
fn deno_render_with_dirs(
    body: &str,
    dirs: &FsDirs,
    permissions: Option<std::collections::HashSet<PermissionType>>,
) -> String {
    let source = format!(
        "export default {{ render() {{ try {{ {body} }} catch (error) {{ return 'error: ' + error.message; }} }} }};"
    );
    let mut env = crate::test_env::test_env();
    env.plugin_cwd = dirs.host.path().to_path_buf();
    env.plugin_own_data_dir = dirs.data.path().to_path_buf();
    env.plugin_own_cache_dir = dirs.cache.path().to_path_buf();
    env.user_plugin_dir = Some(dirs.plugins.path().to_path_buf());
    env.permissions = Arc::new(std::sync::Mutex::new(permissions));
    let mut plugin = crate::deno::DenoPlugin::new(
        &source,
        &dirs.host.path().join("fs.ts"),
        env,
        Arc::default(),
    )
    .unwrap();
    plugin.load(&BTreeMap::new()).unwrap();
    String::from_utf8(plugin.render(1, 400).unwrap()).unwrap()
}

fn file_permissions() -> Option<std::collections::HashSet<PermissionType>> {
    Some(
        [PermissionType::ReadFiles, PermissionType::WriteFiles]
            .into_iter()
            .collect(),
    )
}

struct RemoveOnDrop(PathBuf);
impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn deno_each_root_is_a_different_folder() {
    let dirs = FsDirs::new();
    let unique = format!("deno-where-{}.txt", std::process::id());
    let rendered = deno_render_with_dirs(
        &format!(
            "yinz.fs.write('/host/where.txt', 'host'); yinz.fs.write('/data/where.txt', 'data'); \
             yinz.fs.write('/cache/where.txt', 'cache'); yinz.fs.write('/tmp/{unique}', 'tmp'); \
             return yinz.decodeUtf8(yinz.fs.read('relative.txt'));"
        ),
        &dirs,
        file_permissions(),
    );
    let _cleanup = RemoveOnDrop(yinz_runtime::consts::YINZ_TMP_DIR.join(&unique));
    for (dir, expected) in [
        (dirs.host.path(), "host"),
        (dirs.data.path(), "data"),
        (dirs.cache.path(), "cache"),
    ] {
        assert_eq!(
            std::fs::read_to_string(dir.join("where.txt")).unwrap(),
            expected
        );
    }
    assert_eq!(
        std::fs::read_to_string(yinz_runtime::consts::YINZ_TMP_DIR.join(&unique)).unwrap(),
        "tmp",
        "/tmp is yinz's own temp folder"
    );
    // A relative path is read against `/host`; a missing file is an error, not an empty answer.
    assert!(rendered.starts_with("error:"), "{rendered}");
    std::fs::write(dirs.host.path().join("relative.txt"), "found").unwrap();
    assert_eq!(
        deno_render_with_dirs(
            "return yinz.decodeUtf8(yinz.fs.read('relative.txt'));",
            &dirs,
            file_permissions()
        ),
        "found"
    );
}

#[test]
fn deno_a_path_cannot_climb_out_of_its_root() {
    let dirs = FsDirs::new();
    for body in [
        "return yinz.fs.read('/host/../../../../etc/passwd');",
        "return yinz.fs.read('/data/../../../../etc/passwd');",
        "yinz.fs.write('/host/../escaped.txt', 'x'); return 'written';",
    ] {
        let rendered = deno_render_with_dirs(body, &dirs, file_permissions());
        assert!(
            rendered.contains("outside the plugin's own folders"),
            "expected {body} to be refused, got: {rendered}"
        );
    }
    assert!(!dirs
        .host
        .path()
        .parent()
        .unwrap()
        .join("escaped.txt")
        .exists());
}

#[cfg(unix)]
#[test]
fn deno_fs_remove_symlinks_preserves_their_targets() {
    for recursive in [false, true] {
        let dirs = FsDirs::new();
        std::fs::create_dir(dirs.host.path().join("target")).unwrap();
        std::fs::write(dirs.host.path().join("target/keep"), "kept").unwrap();
        for target in ["target", "target/keep", "missing"] {
            let link = dirs.host.path().join("link");
            std::os::unix::fs::symlink(target, &link).unwrap();
            assert_eq!(
                deno_render_with_dirs(
                    &format!("yinz.fs.remove('/host/link', {{recursive: {recursive}}}); return 'removed';"),
                    &dirs,
                    file_permissions(),
                ),
                "removed"
            );
            assert!(std::fs::symlink_metadata(&link).is_err());
            assert_eq!(
                std::fs::read_to_string(dirs.host.path().join("target/keep")).unwrap(),
                "kept"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn deno_fs_rename_symlinks_preserves_their_targets() {
    let dirs = FsDirs::new();
    std::fs::create_dir(dirs.host.path().join("target")).unwrap();
    std::fs::write(dirs.host.path().join("target/keep"), "kept").unwrap();
    for target in ["target", "target/keep", "missing"] {
        let link = dirs.host.path().join("link");
        let moved = dirs.host.path().join("moved");
        std::os::unix::fs::symlink(target, &link).unwrap();
        assert_eq!(
            deno_render_with_dirs(
                "yinz.fs.rename('/host/link', '/host/moved'); return 'renamed';",
                &dirs,
                file_permissions(),
            ),
            "renamed"
        );
        assert!(std::fs::symlink_metadata(&link).is_err());
        assert_eq!(std::fs::read_link(&moved).unwrap(), PathBuf::from(target));
        assert_eq!(
            std::fs::read_to_string(dirs.host.path().join("target/keep")).unwrap(),
            "kept"
        );
        std::fs::remove_file(moved).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn deno_fs_rename_onto_a_symlink_replaces_the_link() {
    let dirs = FsDirs::new();
    std::fs::write(dirs.host.path().join("target"), "kept").unwrap();
    std::fs::write(dirs.host.path().join("source"), "replacement").unwrap();
    std::os::unix::fs::symlink("target", dirs.host.path().join("link")).unwrap();
    assert_eq!(
        deno_render_with_dirs(
            "yinz.fs.rename('/host/source', '/host/link'); return 'renamed';",
            &dirs,
            file_permissions(),
        ),
        "renamed"
    );
    assert!(!dirs.host.path().join("source").exists());
    assert!(!dirs.host.path().join("link").is_symlink());
    assert_eq!(
        std::fs::read_to_string(dirs.host.path().join("link")).unwrap(),
        "replacement"
    );
    assert_eq!(
        std::fs::read_to_string(dirs.host.path().join("target")).unwrap(),
        "kept"
    );
}

#[cfg(unix)]
#[test]
fn deno_fs_metadata_identifies_valid_and_dangling_symlinks() {
    let dirs = FsDirs::new();
    std::fs::write(dirs.host.path().join("target"), "kept").unwrap();
    for target in ["target", "missing"] {
        let link = dirs.host.path().join("link");
        std::os::unix::fs::symlink(target, &link).unwrap();
        assert_eq!(
            deno_render_with_dirs(
                "const m = yinz.fs.metadata('/host/link'); return JSON.stringify([m.is_symlink, m.is_file, m.is_dir]);",
                &dirs,
                file_permissions(),
            ),
            "[true,false,false]"
        );
        std::fs::remove_file(link).unwrap();
    }
    assert_eq!(
        deno_render_with_dirs(
            "return String(yinz.fs.metadata('/host').is_dir);",
            &dirs,
            file_permissions()
        ),
        "true"
    );
}

#[cfg(unix)]
#[test]
fn deno_a_symlink_out_of_a_root_is_followed_and_then_refused() {
    let dirs = FsDirs::new();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();
    std::os::unix::fs::symlink(outside.path(), dirs.host.path().join("escape")).unwrap();
    let rendered = deno_render_with_dirs(
        "return yinz.fs.read('/host/escape/secret.txt');",
        &dirs,
        file_permissions(),
    );
    assert!(
        rendered.contains("outside the plugin's own folders") && rendered.contains("is a symlink"),
        "{rendered}"
    );
}

#[cfg(unix)]
#[test]
fn deno_fs_write_cannot_create_through_a_dangling_symlink() {
    let dirs = FsDirs::new();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("must-not-be-created");
    let link = dirs.host.path().join("escape");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    for path in ["/host/escape", "/host/escape/", "/host/escape/child"] {
        for append in [false, true] {
            let rendered = deno_render_with_dirs(
                &format!(
                    "yinz.fs.write('{path}', 'escaped', {{append: {append}, createDirs: true}}); return 'written';"
                ),
                &dirs,
                file_permissions(),
            );
            assert!(rendered.contains("unresolved symlink"), "{rendered}");
            assert!(!target.exists());
            assert!(link.is_symlink());
        }
    }
}

#[cfg(unix)]
#[test]
fn deno_fs_write_refuses_fifos_and_keeps_regular_file_write_modes() {
    use std::os::unix::fs::OpenOptionsExt;

    let dirs = FsDirs::new();
    let fifo = dirs.host.path().join("fifo");
    nix::unistd::mkfifo(
        &fifo,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .unwrap();
    for append in [false, true] {
        // The reader makes the old blocking open complete, so the regression fails on the
        // missing type check without hanging. Then verify a FIFO without a reader too.
        let reader = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&fifo)
            .unwrap();
        let body =
            format!("yinz.fs.write('/host/fifo', 'data', {{append: {append}}}); return 'written';");
        let rendered = deno_render_with_dirs(&body, &dirs, file_permissions());
        assert!(rendered.contains("not a regular file"), "{rendered}");
        drop(reader);
        let rendered = deno_render_with_dirs(&body, &dirs, file_permissions());
        assert!(rendered.starts_with("error:"), "{rendered}");
    }
    assert_eq!(
        deno_render_with_dirs(
            "yinz.fs.write('/host/file', 'original'); \
             yinz.fs.write('/host/file', 'new'); \
             yinz.fs.write('/host/file', '+append', {append: true}); \
             return yinz.decodeUtf8(yinz.fs.read('/host/file'));",
            &dirs,
            file_permissions(),
        ),
        "new+append"
    );
}

#[test]
fn deno_the_plugin_folder_is_a_root_that_needs_author_plugins_to_write() {
    let dirs = FsDirs::new();
    let body = "try { yinz.fs.write('/plugins/keys.ts', 'export default {};'); } catch (error) { return 'refused: ' + error.message; } return yinz.decodeUtf8(yinz.fs.read('/plugins/keys.ts'));";
    let without = deno_render_with_dirs(body, &dirs, file_permissions());
    assert!(
        without.contains("AuthorPlugins"),
        "a write into the plugin folder names the permission it needs: {without}"
    );
    assert!(!dirs.plugins.path().join("keys.ts").exists());
    let mut with = file_permissions().unwrap();
    with.insert(PermissionType::AuthorPlugins);
    assert_eq!(
        deno_render_with_dirs(body, &dirs, Some(with)),
        "export default {};"
    );
    assert_eq!(
        std::fs::read_to_string(dirs.plugins.path().join("keys.ts")).unwrap(),
        "export default {};",
        "the write landed in the real plugin folder"
    );
}

#[test]
fn deno_renaming_out_of_the_plugins_folder_is_refused() {
    let dirs = FsDirs::new();
    std::fs::write(dirs.host.path().join("a.txt"), "x").unwrap();
    let stolen = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deno-stolen.txt");
    let _cleanup = RemoveOnDrop(stolen.clone());
    let rendered = deno_render_with_dirs(
        &format!(
            "yinz.fs.rename('/host/a.txt', '{}'); return 'renamed';",
            stolen.display()
        ),
        &dirs,
        file_permissions(),
    );
    assert!(rendered.contains("FullHdAccess"), "{rendered}");
    assert!(dirs.host.path().join("a.txt").exists() && !stolen.exists());
}

#[test]
fn deno_full_hd_access_reaches_absolute_paths_and_nothing_less_does() {
    let dirs = FsDirs::new();
    // Not a tempdir: `/tmp` is one of the roots. This crate's own tree is outside every root.
    let outside = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("deno-outside-the-roots.txt");
    std::fs::write(&outside, "elsewhere").unwrap();
    let _cleanup = RemoveOnDrop(outside.clone());
    let body = format!(
        "return yinz.decodeUtf8(yinz.fs.read('{}'));",
        outside.display()
    );
    let refused = deno_render_with_dirs(&body, &dirs, file_permissions());
    assert!(refused.contains("FullHdAccess"), "{refused}");
    let mut with = file_permissions().unwrap();
    with.insert(PermissionType::FullHdAccess);
    assert_eq!(deno_render_with_dirs(&body, &dirs, Some(with)), "elsewhere");
}

#[cfg(unix)]
#[test]
fn deno_full_hd_access_follows_absolute_links_for_io_and_preserves_entries() {
    let dirs = FsDirs::new();
    // /tmp is a virtual root, so use a temporary directory in the crate's real checkout.
    let outside = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
    let target = outside.path().join("target");
    let link = outside.path().join("link");
    std::fs::write(&target, "original").unwrap();
    std::os::unix::fs::symlink("target", &link).unwrap();
    let path = serde_json::to_string(&link).unwrap();
    let body = format!(
        "const path = {path}; \
         const text = yinz.decodeUtf8(yinz.fs.read(path)); \
         yinz.fs.write(path, text + '+updated'); \
         return String(yinz.fs.metadata(path).is_symlink);"
    );
    let refused = deno_render_with_dirs(&body, &dirs, file_permissions());
    assert!(refused.contains("FullHdAccess"), "{refused}");
    let mut permissions = file_permissions().unwrap();
    permissions.insert(PermissionType::FullHdAccess);
    assert_eq!(
        deno_render_with_dirs(&body, &dirs, Some(permissions.clone())),
        "true"
    );
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "original+updated"
    );
    assert!(link.is_symlink());
    assert_eq!(
        deno_render_with_dirs(
            &format!("yinz.fs.remove({path}); return 'removed';"),
            &dirs,
            Some(permissions),
        ),
        "removed"
    );
    assert!(!link.is_symlink());
    assert_eq!(std::fs::read_to_string(target).unwrap(), "original+updated");
}

#[test]
fn deno_project_configuration_requires_write_authorization() {
    let dirs = FsDirs::new();
    assert!(std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(dirs.host.path())
        .output()
        .unwrap()
        .status
        .success());
    std::fs::create_dir(dirs.host.path().join(".yinz")).unwrap();
    let path = dirs.host.path().join(".yinz/agent-exit.kdl");
    let source = "agent_exit { default_branch \"main\"; }";
    std::fs::write(&path, source).unwrap();
    let read = "return yinz.projectConfig.read('.').source;";
    let read_only: std::collections::HashSet<_> = [PermissionType::ReadFiles].into_iter().collect();
    assert_eq!(
        deno_render_with_dirs(read, &dirs, Some(read_only.clone())),
        source
    );
    let write = format!("return yinz.projectConfig.write('.', 'agent_exit {{ default_branch \"trunk\"; }}', {source:?}).source;");
    let denied = deno_render_with_dirs(&write, &dirs, Some(read_only));
    assert!(denied.contains("WriteFiles"), "{denied}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
    assert_eq!(
        deno_render_with_dirs(&write, &dirs, file_permissions()),
        "agent_exit { default_branch \"trunk\"; }"
    );
}

#[test]
#[serial(plugin_tests)]
fn deno_change_host_folder_moves_the_host_root() {
    let harness = Harness::new(
        "ts",
        r#"
let state = 'starting';
export default {
  async load() {
    yinz.subscribe(['CustomMessage', 'HostFolderChanged', 'FailedToChangeHostFolder']);
    yinz.on('HostFolderChanged', (folder) => { state = `moved to ${folder.split('/').at(-1)}: ${yinz.decodeUtf8(yinz.fs.read('/host/marker'))}`; return true; });
    yinz.on('FailedToChangeHostFolder', (error) => { state = `failed: ${error}`; return true; });
    await yinz.waitForPermissions(['ReadFiles', 'FullHdAccess']);
    state = `before: ${yinz.fs.metadata('/host/marker') === null ? 'no marker' : 'marker'}`;
    yinz.requestRender();
  },
  update(event) { if (event.CustomMessage) { yinz.host.changeHostFolder(event.CustomMessage[1]); } return false; },
  render() { return state; }
};
"#,
        PermissionStatus::Granted,
    );
    let elsewhere = tempdir().unwrap();
    std::fs::write(elsewhere.path().join("marker"), "found in the new host").unwrap();
    harness.load();
    harness.rendered("before: no marker");
    harness.event(Event::CustomMessage(
        "move".into(),
        elsewhere.path().display().to_string(),
    ));
    let frame = harness.rendered("moved to");
    assert_eq!(
        frame.trim_end(),
        format!(
            "moved to {}: found in the new host",
            elsewhere.path().file_name().unwrap().to_string_lossy()
        )
    );
    let log = harness.finish();
    assert!(!rendered_assets(&log)
        .iter()
        .any(|(_, frame)| frame.contains("failed:")));
}

#[test]
fn deno_worker_host_folder_replies_release_fifo_and_stay_local_across_reload() {
    let source = r#"
const label = 'original';
let state = '', parentReplies = 0;
const read = () => yinz.decodeUtf8(yinz.fs.read('/host/marker'));
export default {
  workers: {
    async move(name, path) {
      if (name === 'success' || name === 'failure') {
        const type = name === 'success' ? 'HostFolderChanged' : 'FailedToChangeHostFolder';
        const reply = yinz.waitForEvent(type);
        yinz.host.changeHostFolder(path);
        const result = await reply;
        if (name === 'success' ? result !== path : !result.includes('does not exist')) throw Error('wrong folder reply');
      }
      yinz.postMessageToPlugin(name, read());
    },
    sibling(name) {yinz.postMessageToPlugin(name, read());}
  },
  async load() {
    await yinz.waitForPermissions(['ReadFiles', 'FullHdAccess']);
    yinz.subscribe(['CustomMessage', 'HostFolderChanged', 'FailedToChangeHostFolder']);
    yinz.on('HostFolderChanged', () => {parentReplies++;});
    yinz.on('FailedToChangeHostFolder', () => {parentReplies++;});
    yinz.on('CustomMessage', ([name, payload]) => {
      if (name === 'start') {
        yinz.postMessageTo('move', 'success', payload);
        yinz.postMessageTo('move', 'failure', payload + '/missing');
        yinz.postMessageTo('move', 'after');
      } else if (name === 'probe') {
        yinz.postMessageTo('move', 'checked');
      } else {
        if ((name === 'sibling' ? 'parent' : 'moved') !== payload) throw Error('wrong host root: ' + name + ':' + payload);
        state += name + ':' + payload + ' ';
        if (name === 'after' || name === 'checked') yinz.postMessageTo('sibling', 'sibling');
      }
      return true;
    });
    state = 'ready ';
  },
  render() {return state ? `${label} ${state}parent=${read()} replies=${parentReplies}` : 'starting';}
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
    std::fs::write(harness.directory.path().join("marker"), "parent").unwrap();
    let moved = tempdir().unwrap();
    std::fs::write(moved.path().join("marker"), "moved").unwrap();
    let stale = tempdir().unwrap();
    std::fs::write(stale.path().join("marker"), "stale").unwrap();
    let captured = Watched::new(vec![]);
    let proxy = std::thread::spawn({
        let captured = captured.clone();
        let sender = harness.sender.clone();
        move || loop {
            let (instruction, _) = outgoing.recv_timeout(FAKE_SCREEN_RECV_TIMEOUT).unwrap();
            if matches!(instruction, PluginInstruction::Exit) {
                break;
            }
            if matches!(
                instruction,
                PluginInstruction::ChangePluginHostDir(..)
                    | PluginInstruction::DenoReply(
                        _,
                        _,
                        _,
                        Event::HostFolderChanged(_) | Event::FailedToChangeHostFolder(_),
                        _
                    )
            ) {
                captured.lock().push(instruction.clone());
            }
            let _ = sender.send(instruction);
        }
    });
    let exercise = |label| {
        harness.rendered(&format!("{label} ready parent=parent replies=0"));
        harness.event(Event::CustomMessage(
            "start".into(),
            moved.path().display().to_string(),
        ));
        harness.rendered(&format!(
            "{label} ready success:moved failure:moved after:moved sibling:parent parent=parent replies=0"
        ));
    };
    harness.load();
    exercise("original");
    let old = captured.lock().clone();
    assert_eq!(
        old.len(),
        4,
        "two requests and their scoped replies: {old:?}"
    );
    let PluginInstruction::ChangePluginHostDir(_, _, _, Some(old_generation)) = old[0] else {
        panic!("worker request must carry a generation: {old:?}");
    };
    std::fs::write(&harness.path, source.replace("'original'", "'replacement'")).unwrap();
    harness.reload();
    exercise("replacement");
    assert!(
        matches!(captured.lock()[4], PluginInstruction::ChangePluginHostDir(_, _, _, Some(generation)) if generation != old_generation)
    );
    // Replay real old requests and replies, with a root that would expose accidental delivery.
    for mut instruction in old {
        match &mut instruction {
            PluginInstruction::ChangePluginHostDir(path, _, _, _) if path == moved.path() => {
                *path = stale.path().to_path_buf();
            },
            PluginInstruction::DenoReply(_, _, _, Event::HostFolderChanged(path), _) => {
                *path = stale.path().to_path_buf();
            },
            _ => {},
        }
        harness.sender.send(instruction).unwrap();
    }
    harness.event(Event::CustomMessage("probe".into(), String::new()));
    harness.rendered("checked:moved sibling:parent parent=parent replies=0");
    harness.finish();
    to_proxy.send(PluginInstruction::Exit).unwrap();
    proxy.join().unwrap();
}

// Filesystem watching and scanning ------------------------------------------------------------

#[test]
#[serial(plugin_tests)]
fn deno_filesystem_watch_events_reach_the_plugin_and_its_reloaded_replacement() {
    let source = r#"
const label = 'first';
const seen: string[] = [];
export default {
  load() {
    yinz.subscribe(['FileSystemCreate', 'FileSystemUpdate', 'FileSystemDelete']);
    yinz.host.watchFilesystem();
  },
  update(event) {
    const type = typeof event === 'string' ? event : Object.keys(event)[0];
    if (!type.startsWith('FileSystem')) return false;
    // The watcher attaches no metadata (see `watch_filesystem.rs`); the pair shape is the point.
    for (const [path, metadata] of event[type]) {
      seen.push(`${type.slice('FileSystem'.length).toLowerCase()}:${path.split('/').at(-1)}${metadata === null ? '' : '?'}`);
    }
    return true;
  },
  render() { return `${label} ${seen.join(' ')}`; }
};
"#;
    let harness = Harness::new("ts", source, PermissionStatus::Granted);
    harness.load();
    harness.rendered("first");
    let watched = harness.directory.path().join("watched.txt");
    std::fs::write(&watched, "one").unwrap();
    harness.rendered("create:watched.txt");
    std::fs::write(&watched, "two").unwrap();
    harness.rendered("update:watched.txt");
    std::fs::remove_file(&watched).unwrap();
    harness.rendered("delete:watched.txt");
    // The replacement owns a fresh watcher; old callbacks retain the old generation and
    // cannot deliver their events to it.
    std::fs::write(&harness.path, source.replace("'first'", "'second'")).unwrap();
    harness.reload();
    harness.rendered("second ");
    std::fs::write(harness.directory.path().join("later.txt"), "three").unwrap();
    let frame = harness.rendered("create:later.txt");
    assert!(frame.starts_with("second "), "{frame}");
    assert!(
        !frame.contains("watched.txt"),
        "fresh state after reload: {frame}"
    );
    harness.finish();
}

#[test]
#[serial(plugin_tests)]
fn deno_scan_host_folder_answers_the_requesting_generation() {
    let source = r#"
const label = 'first';
let listing = 'unscanned';
export default {
  load() {
    yinz.subscribe(['FileSystemUpdate']);
    yinz.host.scanHostFolder('/host');
  },
  update(event) {
    if (typeof event !== 'object' || !event.FileSystemUpdate) return false;
    listing = event.FileSystemUpdate.map(([path]) => path).filter(p => p.endsWith('.txt')).sort().join(',');
    return true;
  },
  render() { return `${label} ${listing}`; }
};
"#;
    let harness = Harness::new("ts", source, PermissionStatus::Granted);
    std::fs::write(harness.directory.path().join("b.txt"), "").unwrap();
    std::fs::write(harness.directory.path().join("a.txt"), "").unwrap();
    harness.load();
    let frame = harness.rendered("first /host");
    assert_eq!(frame.trim_end(), "first /host/a.txt,/host/b.txt");
    std::fs::write(harness.directory.path().join("c.txt"), "").unwrap();
    std::fs::write(&harness.path, source.replace("'first'", "'second'")).unwrap();
    harness.reload();
    let frame = harness.rendered("second /host");
    assert_eq!(
        frame.trim_end(),
        "second /host/a.txt,/host/b.txt,/host/c.txt"
    );
    harness.finish();
}

// Name resolution -------------------------------------------------------------------------------

fn deno_run_plugin_at(path: &str) -> RunPluginOrAlias {
    RunPluginOrAlias::RunPlugin(RunPlugin {
        location: RunPluginLocation::File(PathBuf::from(path)),
        ..Default::default()
    })
}

/// A plugin named rather than pathed is found in the user's plugin folder - `file:probe.ts` in
/// a layout means the obvious thing. See `a_script_plugin_is_found_in_the_user_plugin_dir_by_name`.
#[test]
#[serial(plugin_tests)]
fn deno_plugin_is_found_in_the_user_plugin_dir_by_name() {
    let temp_folder = tempdir().unwrap();
    let user_plugin_dir = tempdir().unwrap();
    std::fs::write(
        user_plugin_dir.path().join("probe.ts"),
        "export default { render() { return 'found by name'; } };",
    )
    .unwrap();
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread_with_user_plugin_dir(
            Some(temp_folder.path().to_path_buf()),
            None,
            Some(user_plugin_dir.path().to_path_buf()),
        );
    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        1
    );
    plugin_thread_sender
        .send(PluginInstruction::AddClient(CLIENT))
        .unwrap();
    load(&plugin_thread_sender, deno_run_plugin_at("probe.ts"));
    screen_thread.join().unwrap();
    teardown();
    let rendered = super::script_plugins::rendered_output(&received_screen_instructions.lock());
    assert!(
        rendered.iter().any(|r| r.contains("found by name")),
        "expected the plugin from the user plugin dir to render, got: {rendered:?}"
    );
}

/// A reload naming the file in full finds the plugin that was started by bare name, through
/// `resolved_source_path`; without it the reload starts a second instance in a new pane. See
/// `a_reload_by_absolute_path_reloads_a_plugin_started_by_name`.
#[test]
#[serial(plugin_tests)]
fn deno_reload_by_absolute_path_reloads_a_plugin_started_by_name() {
    let temp_folder = tempdir().unwrap();
    let user_plugin_dir = tempdir().unwrap();
    let plugin_file = user_plugin_dir.path().join("probe.ts");
    let source =
        |version: &str| format!("export default {{ render() {{ return '{version}'; }} }};");
    std::fs::write(&plugin_file, source("first version")).unwrap();
    let (plugin_thread_sender, screen_receiver, teardown) =
        create_plugin_thread_with_user_plugin_dir(
            Some(temp_folder.path().to_path_buf()),
            None,
            Some(user_plugin_dir.path().to_path_buf()),
        );
    let received_screen_instructions = Watched::new(vec![]);
    let screen_thread = log_actions_in_thread!(
        received_screen_instructions,
        ScreenInstruction::PluginBytes,
        screen_receiver,
        2
    );
    plugin_thread_sender
        .send(PluginInstruction::AddClient(CLIENT))
        .unwrap();
    load(&plugin_thread_sender, deno_run_plugin_at("probe.ts"));
    received_screen_instructions.wait_until(
        "the plugin to render its first version",
        default_timeout(),
        |instructions| {
            super::script_plugins::rendered_output(instructions)
                .iter()
                .any(|rendered| rendered.contains("first version"))
                .then_some(())
        },
    );
    std::fs::write(&plugin_file, source("second version")).unwrap();
    plugin_thread_sender
        .send(PluginInstruction::Reload(
            Some(false),
            Some("deno acceptance".into()),
            deno_run_plugin_at(&plugin_file.display().to_string()),
            TabId::new(1),
            SIZE,
            None,
        ))
        .unwrap();
    screen_thread.join().unwrap();
    teardown();
    let logged = received_screen_instructions.lock();
    let rendered = super::script_plugins::rendered_output(&logged);
    assert!(
        rendered.iter().any(|r| r.contains("second version")),
        "the reload should have re-read the file, got: {rendered:?}"
    );
    assert_eq!(
        logged
            .iter()
            .filter(|i| matches!(i, ScreenInstruction::AddPlugin(..)))
            .count(),
        1,
        "the running plugin should have been reloaded, not started a second time in a new pane"
    );
}

// Configuration ---------------------------------------------------------------------------------

/// A configuration that already exists is handed to `load` as-is: `PluginUserConfiguration::new`
/// strips reserved keys such as `payload`, which is right for a layout and wrong afterwards. See
/// `plugin_configuration_reaches_load_without_being_re_normalized`.
#[test]
#[serial(plugin_tests)]
fn deno_plugin_receives_its_configuration_unnormalized() {
    let harness = Harness::new(
        "ts",
        r#"
let state = 'loading';
export default {
  defaults: {greeting: 'hello', name: 'world'},
  load(config) { state = Object.entries(config).map(([k, v]) => `${k}=${v}`).sort().join(' '); return true; },
  render() { return state; }
};
"#,
        PermissionStatus::Granted,
    );
    let mut configuration =
        PluginUserConfiguration::new(BTreeMap::from([("greeting".into(), "configured".into())]));
    // Inserted after construction, exactly as the reserved-word filter intends to allow.
    configuration.insert("payload", "carried");
    configuration.insert("name", "yinz");
    configuration.insert("Mixed_Case-key", "kept as written");
    load(
        &harness.sender,
        RunPluginOrAlias::RunPlugin(RunPlugin {
            location: RunPluginLocation::File(harness.path.clone()),
            configuration,
            ..Default::default()
        }),
    );
    let frame = harness.rendered("greeting=");
    assert_eq!(
        frame.trim_end(),
        "Mixed_Case-key=kept as written greeting=configured name=yinz payload=carried"
    );
    harness.finish();
}
