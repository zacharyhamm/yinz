//! Bookkeeping around plugins that are still *loading* - in `loading_plugins`, not yet in the
//! `PluginMap`. These are the states no end-to-end test can observe: whether a cancellation mark
//! was left for a loader that will run, or for one that already gave up.

use super::*;

use std::collections::BTreeMap;
use tempfile::tempdir;
use zellij_runtime::channels::{self, SenderWithContext};
use zellij_utils::input::layout::RunPluginOrAlias;

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
        );
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
    RunPlugin::from_url("file:/plugins/job.lua").expect("a valid plugin url")
}

/// Put `plugin_id` on `queue` as a running job, the way `pump_queue` would have.
fn launch_job(bridge: &mut WasmBridge, queue: &str, plugin_id: PluginId) {
    let mut queues = bridge.queues.lock().unwrap();
    queues.enqueue(
        queue,
        (crate::PluginId(1), crate::ClientId(1)),
        RunPluginOrAlias::from_url("file:/plugins/job.lua", &None, None, None).unwrap(),
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
        .insert((crate::PluginId(7), run_plugin()));

    test.bridge
        .cleanup(std::time::Instant::now() + std::time::Duration::from_secs(1));

    assert_eq!(
        cancellation_marks(&test.bridge),
        vec![PluginId(7)],
        "the loader must find the plugin cancelled rather than instantiate it after shutdown"
    );
    assert!(test.bridge.loading_plugins.is_empty());
}

#[test]
fn a_job_whose_load_failed_leaves_no_cancellation_mark() {
    let mut test = TestBridge::new();
    launch_job(&mut test.bridge, "merge", crate::PluginId(7));
    test.bridge
        .loading_plugins
        .insert((crate::PluginId(7), run_plugin()));

    test.bridge
        .queue_plugin_failed(crate::PluginId(7), "no such file".to_owned(), true)
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
    launch_job(&mut test.bridge, "merge", crate::PluginId(7));
    test.bridge
        .loading_plugins
        .insert((crate::PluginId(7), run_plugin()));

    test.bridge
        .queue_plugin_failed(crate::PluginId(7), "boom".to_owned(), false)
        .unwrap();

    assert_eq!(cancellation_marks(&test.bridge), vec![PluginId(7)]);
    assert!(test.bridge.loading_plugins.is_empty());
}
