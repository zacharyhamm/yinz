//! Bookkeeping around plugins that are still *loading* - in `loading_plugins`, not yet in the
//! `PluginMap`. These are the states no end-to-end test can observe: whether a cancellation mark
//! was left for a loader that will run, or for one that already gave up.

use super::*;

use std::collections::BTreeMap;
use tempfile::tempdir;
use zellij_runtime::channels::{self, SenderWithContext};

/// Senders nothing listens to. The receivers are returned so the channels stay connected for as
/// long as the bridge does.
struct TestBridge {
    bridge: WasmBridge,
    _receivers: Vec<Box<dyn std::any::Any>>,
    _dirs: Vec<tempfile::TempDir>,
}

impl TestBridge {
    fn new() -> Self {
        let (to_pty, receive_pty) = channels::unbounded();
        let (to_screen, receive_screen) = channels::unbounded();
        let (to_plugin, receive_plugin) = channels::unbounded();
        let (to_server, receive_server) = channels::unbounded();
        let (to_pty_writer, receive_pty_writer) = channels::unbounded();
        let (to_background_jobs, receive_background_jobs) = channels::unbounded();
        let senders = ThreadSenders {
            to_pty: Some(SenderWithContext::new(to_pty)),
            to_screen: Some(SenderWithContext::new(to_screen)),
            to_plugin: Some(SenderWithContext::new(to_plugin)),
            to_server: Some(SenderWithContext::new(to_server)),
            to_pty_writer: Some(SenderWithContext::new(to_pty_writer)),
            to_background_jobs: Some(SenderWithContext::new(to_background_jobs)),
            session_recorder: None,
            should_silently_fail: true,
        };
        let data_dir = tempdir().unwrap();
        let data_access = Arc::new(DataAccess::sqlite(data_dir.path()));
        let bridge = WasmBridge::new(
            senders,
            Engine::default(),
            data_dir.path().to_path_buf(),
            data_access,
            PathBuf::from("."),
            PathBuf::from("."),
            BTreeMap::new(),
            None,
            None,
            None,
            None,
            vec![],
            vec![],
            InputMode::Normal,
            Keybinds::default(),
            PluginAliases::default(),
        )
        .unwrap();
        TestBridge {
            bridge,
            _receivers: vec![
                Box::new(receive_pty),
                Box::new(receive_screen),
                Box::new(receive_plugin),
                Box::new(receive_server),
                Box::new(receive_pty_writer),
                Box::new(receive_background_jobs),
            ],
            _dirs: vec![data_dir],
        }
    }
}

fn run_plugin() -> RunPlugin {
    zellij_runtime::plugin_location::run_plugin_from_url("file:/plugins/job.lua")
        .expect("a valid plugin url")
}

/// Put `plugin_id` on `queue` as a running job, the way `pump_queue` would have.
fn launch_job(bridge: &mut WasmBridge, queue: &str, plugin_id: PluginId) {
    let mut queues = bridge.queues.lock().unwrap();
    queues.enqueue(
        queue,
        (crate::PluginId::new(1), crate::ClientId::new(1)),
        zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
            "file:/plugins/job.lua",
            &None,
            None,
            None,
        )
        .unwrap(),
        BTreeMap::new(),
        None,
    );
    let job = queues.pop_launchable(queue).expect("the queue has a slot");
    queues.on_job_launched(job, plugin_id);
}

fn cancellation_marks(bridge: &WasmBridge) -> Vec<PluginId> {
    let mut marks: Vec<PluginId> = bridge
        .unloaded_while_loading
        .lock()
        .unwrap()
        .iter()
        .copied()
        .collect();
    marks.sort();
    marks
}

#[test]
fn shutting_down_cancels_a_plugin_that_is_still_loading() {
    let mut test = TestBridge::new();
    test.bridge
        .loading_plugins
        .insert((crate::PluginId::new(7), run_plugin()));

    test.bridge
        .cleanup(std::time::Instant::now() + std::time::Duration::from_secs(1));

    assert_eq!(
        cancellation_marks(&test.bridge),
        vec![PluginId::new(7)],
        "the loader must find the plugin cancelled rather than instantiate it after shutdown"
    );
    assert!(test.bridge.loading_plugins.is_empty());
}

#[test]
fn shutting_down_after_screen_exit_cancels_a_plugin_that_is_still_loading() {
    let mut test = TestBridge::new();
    drop(test._receivers.remove(1)); // Disconnect the screen channel.
    test.bridge.senders.should_silently_fail = false;
    test.bridge
        .loading_plugins
        .insert((crate::PluginId::new(7), run_plugin()));

    test.bridge
        .cleanup(std::time::Instant::now() + std::time::Duration::from_secs(1));

    assert_eq!(cancellation_marks(&test.bridge), vec![PluginId::new(7)]);
    assert!(test.bridge.loading_plugins.is_empty());
}

#[test]
fn a_job_whose_load_failed_leaves_no_cancellation_mark() {
    let mut test = TestBridge::new();
    launch_job(&mut test.bridge, "merge", crate::PluginId::new(7));
    test.bridge
        .loading_plugins
        .insert((crate::PluginId::new(7), run_plugin()));

    test.bridge
        .queue_plugin_failed(crate::PluginId::new(7), "no such file".to_owned(), true)
        .unwrap();

    assert!(
        cancellation_marks(&test.bridge).is_empty(),
        "the loader already ran and gave up: a mark here would never be consumed"
    );
    assert!(test.bridge.loading_plugins.is_empty());
    let snapshot = test
        .bridge
        .queues
        .lock()
        .unwrap()
        .queue_snapshot("merge")
        .unwrap();
    assert!(
        snapshot.running.is_empty() && snapshot.pending.is_empty(),
        "the failed job freed its slot: {snapshot:?}"
    );
}

#[test]
fn a_job_that_trapped_while_still_loading_is_cancelled() {
    // the other side of the coin: a plugin that failed *after* it loaded has a loader still
    // pending only if it never got there, so this path must keep marking it.
    let mut test = TestBridge::new();
    launch_job(&mut test.bridge, "merge", crate::PluginId::new(7));
    test.bridge
        .loading_plugins
        .insert((crate::PluginId::new(7), run_plugin()));

    test.bridge
        .queue_plugin_failed(crate::PluginId::new(7), "boom".to_owned(), false)
        .unwrap();

    assert_eq!(cancellation_marks(&test.bridge), vec![PluginId::new(7)]);
    assert!(test.bridge.loading_plugins.is_empty());
}

/// Observe filesystem state with bounded waits, including cleanup on executor/worker threads.
struct FileEvents {
    _watcher: RecommendedWatcher,
    changes: zellij_test_support::Watched<usize>,
}

impl FileEvents {
    fn new(path: &std::path::Path) -> Self {
        use notify_debouncer_full::notify::{self, Watcher};
        let changes = zellij_test_support::Watched::new(0);
        let observed = changes.clone();
        let mut watcher = notify::recommended_watcher(move |_| *observed.lock() += 1).unwrap();
        watcher
            .watch(path, notify::RecursiveMode::Recursive)
            .unwrap();
        Self {
            _watcher: watcher,
            changes,
        }
    }

    fn wait(&self, what: &str, predicate: impl Fn() -> bool) {
        self.changes
            .wait_until(what, Duration::from_secs(30), |_| predicate().then_some(()));
    }
}

const SCRATCH_PLUGIN: &str = r#"
return {
  load = function() zellij.subscribe { 'before_close' } end,
  update = function(event)
    if event.name == 'before_close' then
      zellij.fs.write('/data/closed', zellij.fs.read('/data/value'))
      zellij.fs.write('/cache/saved', zellij.fs.read('/data/closed'))
    end
    return false
  end,
}
"#;

impl TestBridge {
    fn context(&mut self, id: u32, client: u16, source: &str) -> LoadingContext {
        let plugin_id = PluginId::new(id);
        let client_id = ClientId::new(client);
        self.bridge.cancel_token(plugin_id);
        self.bridge.plugin_executor.register_plugin(plugin_id);
        let path = self._dirs[0].path().join(format!("{id}.lua"));
        std::fs::write(&path, source).unwrap();
        let config = PluginConfig::from_run_plugin(&RunPlugin {
            location: RunPluginLocation::File(path),
            ..Default::default()
        })
        .unwrap();
        let mut context = LoadingContext::new(
            &self.bridge,
            None,
            config,
            plugin_id,
            client_id,
            None,
            Size { rows: 5, cols: 20 },
        )
        .unwrap();
        context.plugin_own_cache_dir = self._dirs[0].path().join(format!("cache-{id}"));
        context
    }

    fn load(&self, context: LoadingContext) -> Result<()> {
        let mut map = self.bridge.plugin_map.lock().unwrap();
        PluginLoader::new(
            false,
            context,
            self.bridge.senders.clone(),
            Engine::default(),
            Default::default(),
            &mut map,
            self.bridge.connected_clients.clone(),
        )
        .without_connected_clients()
        .start_plugin()
    }

    fn grant_files(&self, id: u32, client: u16) {
        let map = self.bridge.plugin_map.lock().unwrap();
        let (plugin, _) = map
            .get_running_plugin_and_subscriptions(PluginId::new(id), ClientId::new(client))
            .unwrap();
        *plugin.lock().unwrap().env().permissions.lock().unwrap() = Some(HashSet::from([
            PermissionType::ReadFiles,
            PermissionType::WriteFiles,
        ]));
    }
}

#[test]
fn temporary_data_lasts_through_before_close_and_cache_survives_shutdown() {
    let mut test = TestBridge::new();
    let root = test.bridge.temporary_root.path().to_path_buf();
    let events = FileEvents::new(root.parent().unwrap());
    let one = test.context(1, 1, SCRATCH_PLUGIN);
    let one_path = one.plugin_own_data_dir.clone();
    let cache = one.plugin_own_cache_dir.clone();
    std::fs::write(one_path.join("value"), "saved by BeforeClose").unwrap();
    test.load(one).unwrap();
    test.grant_files(1, 1);
    let two = test.context(2, 1, "return {}");
    let two_path = two.plugin_own_data_dir.clone();
    test.load(two).unwrap();
    test.bridge.unload_plugin(PluginId::new(1)).unwrap();
    events.wait("first plugin scratch cleanup", || !one_path.exists());
    assert!(two_path.exists());
    assert_eq!(
        std::fs::read_to_string(cache.join("saved")).unwrap(),
        "saved by BeforeClose"
    );
    test.bridge.cleanup(Instant::now() + Duration::from_secs(1));
    events.wait("final plugin scratch cleanup", || !two_path.exists());
    test.bridge
        .wait_for_cleanup(Instant::now() + Duration::from_secs(1));
    assert!(!root.exists());
    assert!(cache.join("saved").exists());
}

#[test]
fn reload_retains_temporary_data_and_old_users_cannot_remove_it() {
    let mut test = TestBridge::new();
    test.bridge
        .connected_clients
        .lock()
        .unwrap()
        .push(ClientId::new(1));
    let context = test.context(1, 1, SCRATCH_PLUGIN);
    let path = context.plugin_own_data_dir.clone();
    let events = FileEvents::new(path.parent().unwrap());
    std::fs::write(path.join("value"), "reload state").unwrap();
    test.load(context).unwrap();
    test.grant_files(1, 1);
    test.bridge.reload_plugin_with_id(PluginId::new(1)).unwrap();
    let done = zellij_test_support::Watched::new(false);
    let observed = done.clone();
    test.bridge.plugin_executor.execute_for_plugin_tagged(
        PluginId::new(1),
        "observe reload",
        false,
        move |_, _, _, _, _| *observed.lock() = true,
    );
    done.wait_until("reload completion", Duration::from_secs(30), |done| {
        done.then_some(())
    });
    assert_eq!(
        std::fs::read_to_string(path.join("closed")).unwrap(),
        "reload state"
    );
    let map = test.bridge.plugin_map.lock().unwrap();
    let (plugin, _) = map
        .get_running_plugin_and_subscriptions(PluginId::new(1), ClientId::new(1))
        .unwrap();
    assert_eq!(plugin.lock().unwrap().env().plugin_own_data_dir, path);
    drop(plugin);
    drop(map);
    test.bridge.unload_plugin(PluginId::new(1)).unwrap();
    events.wait("reloaded instance cleanup", || !path.exists());
}

#[test]
fn failed_loads_and_cancelled_loading_contexts_release_temporary_data() {
    let mut test = TestBridge::new();
    for source in [
        "not lua!",
        "return { load = function() error('load failed') end }",
    ] {
        let context = test.context(1, 1, source);
        let path = context.plugin_own_data_dir.clone();
        assert!(test.load(context).is_err());
        assert!(!path.exists());
    }
    let context = test.context(2, 1, "return {}");
    let path = context.plugin_own_data_dir.clone();
    drop(context); // A download/queued load cancelled before instantiation.
    assert!(!path.exists());
}

#[test]
fn departing_client_releases_its_data_while_another_client_stays() {
    let mut test = TestBridge::new();
    *test.bridge.connected_clients.lock().unwrap() = vec![ClientId::new(1), ClientId::new(2)];
    let one = test.context(1, 1, "return {}");
    let one_path = one.plugin_own_data_dir.clone();
    let events = FileEvents::new(one_path.parent().unwrap());
    test.load(one).unwrap();
    let two = test.context(1, 2, "return {}");
    let two_path = two.plugin_own_data_dir.clone();
    test.load(two).unwrap();
    test.bridge.remove_client(ClientId::new(1));
    events.wait("departing client scratch cleanup", || !one_path.exists());
    assert!(two_path.exists());
    test.bridge.cleanup(Instant::now() + Duration::from_secs(1));
    events.wait("remaining client scratch cleanup", || !two_path.exists());
}

#[test]
fn outstanding_work_keeps_data_alive_after_host_drop() {
    let mut test = TestBridge::new();
    let root = test.bridge.temporary_root.path().to_path_buf();
    let events = FileEvents::new(root.parent().unwrap());
    let context = test.context(1, 1, "return {}");
    let path = context.plugin_own_data_dir.clone();
    let in_flight = context.clone();
    test.load(context).unwrap();
    let started = zellij_test_support::Watched::new(false);
    let release = zellij_test_support::Watched::new(false);
    let observed = started.clone();
    let finish = release.clone();
    test.bridge.plugin_executor.execute_for_plugin_tagged(
        PluginId::new(1),
        "outstanding work",
        false,
        move |_, _, _, _, _| {
            let _in_flight = in_flight;
            *observed.lock() = true;
            finish.wait_until(
                "release of outstanding work",
                Duration::from_secs(30),
                |done| done.then_some(()),
            );
        },
    );
    started.wait_until("work to start", Duration::from_secs(30), |started| {
        started.then_some(())
    });
    let deadline = Instant::now();
    test.bridge.cleanup(deadline);
    test.bridge.wait_for_cleanup(deadline);
    assert!(path.exists());
    assert!(root.exists());
    *release.lock() = true;
    events.wait("last work owner and host root cleanup", || !root.exists());
}

#[test]
fn wasm_workers_release_their_shared_temporary_data_after_unload() {
    let mut test = TestBridge::new();
    let mut context = test.context(1, 1, "return {}");
    let path = context.plugin_own_data_dir.clone();
    let events = FileEvents::new(path.parent().unwrap());
    let wasm = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../target/wasm32-wasip1/debug/fixture-plugin-for-tests.wasm");
    context.plugin_config = PluginConfig::from_run_plugin(&RunPlugin {
        location: RunPluginLocation::File(wasm),
        ..Default::default()
    })
    .unwrap();
    test.load(context).unwrap();
    test.bridge.unload_plugin(PluginId::new(1)).unwrap();
    events.wait("wasm instance and worker cleanup", || !path.exists());
}

#[test]
fn unload_cancels_a_queued_reload_and_client_load() {
    let mut test = TestBridge::new();
    test.bridge
        .connected_clients
        .lock()
        .unwrap()
        .push(ClientId::new(1));
    let context = test.context(1, 1, "return {}");
    let path = context.plugin_own_data_dir.clone();
    let events = FileEvents::new(path.parent().unwrap());
    test.load(context).unwrap();
    let started = zellij_test_support::Watched::new(false);
    let release = zellij_test_support::Watched::new(false);
    let observed = started.clone();
    let finish = release.clone();
    test.bridge.plugin_executor.execute_for_plugin_tagged(
        PluginId::new(1),
        "hold executor",
        false,
        move |_, _, _, _, _| {
            *observed.lock() = true;
            finish.wait_until("release executor", Duration::from_secs(30), |done| {
                done.then_some(())
            });
        },
    );
    started.wait_until("executor to start", Duration::from_secs(30), |started| {
        started.then_some(())
    });
    test.bridge.reload_plugin_with_id(PluginId::new(1)).unwrap();
    test.bridge.add_client(ClientId::new(2)).unwrap();
    test.bridge.unload_plugin(PluginId::new(1)).unwrap();
    *release.lock() = true;
    events.wait("unloaded instance cleanup", || !path.exists());
    assert!(test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .plugin_ids()
        .is_empty());
    assert_eq!(
        std::fs::read_dir(test.bridge.temporary_root.path())
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn reconnecting_before_client_cleanup_keeps_the_live_instance() {
    let mut test = TestBridge::new();
    *test.bridge.connected_clients.lock().unwrap() = vec![ClientId::new(1), ClientId::new(2)];
    let one = test.context(1, 1, "return {}");
    let path = one.plugin_own_data_dir.clone();
    test.load(one).unwrap();
    let two = test.context(1, 2, "return {}");
    test.load(two).unwrap();
    let started = zellij_test_support::Watched::new(false);
    let release = zellij_test_support::Watched::new(false);
    let observed = started.clone();
    let finish = release.clone();
    test.bridge.plugin_executor.execute_for_plugin_tagged(
        PluginId::new(1),
        "hold executor",
        false,
        move |_, _, _, _, _| {
            *observed.lock() = true;
            finish.wait_until("release executor", Duration::from_secs(30), |done| {
                done.then_some(())
            });
        },
    );
    started.wait_until("executor to start", Duration::from_secs(30), |started| {
        started.then_some(())
    });
    test.bridge.remove_client(ClientId::new(1));
    test.bridge.add_client(ClientId::new(1)).unwrap();
    let done = zellij_test_support::Watched::new(false);
    let observed = done.clone();
    test.bridge.plugin_executor.execute_for_plugin_tagged(
        PluginId::new(1),
        "observe cleanup",
        false,
        move |_, _, _, _, _| *observed.lock() = true,
    );
    *release.lock() = true;
    done.wait_until("client cleanup", Duration::from_secs(30), |done| {
        done.then_some(())
    });
    assert!(path.exists());
    assert!(test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .contains(PluginId::new(1), ClientId::new(1)));
}
