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
    fn receiver<T: 'static>(
        &self,
    ) -> channels::Receiver<(T, zellij_types::diagnostics::ErrorContext)> {
        self._receivers.iter().find_map(|receiver| receiver.downcast_ref::<channels::Receiver<(T, zellij_types::diagnostics::ErrorContext)>>()).unwrap().clone()
    }

    fn settle(&self, plugin_id: PluginId) {
        let (send, receive) = std::sync::mpsc::channel();
        self.bridge
            .plugin_executor
            .execute_for_plugin(plugin_id, move |_, _, _, _, _| {
                send.send(()).unwrap();
            });
        receive
            .recv_timeout(zellij_test_support::default_timeout())
            .unwrap();
    }

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
            session_checkpoint: Default::default(),
            should_silently_fail: false,
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

#[test]
fn deno_startup_instance_is_cancelled_and_recreated_on_reattach() {
    let mut test = TestBridge::new();
    let client = ClientId::new(1);
    let path = test._dirs[0].path().join("startup.js");
    std::fs::write(
        &path,
        r#"
let keys = 0;
export default {
  load() { zellij.subscribe(['Key']); },
  update(event) { if (event.Key) keys++; return true; },
  render() { return `keys=${keys}`; }
};
"#,
    )
    .unwrap();
    let run =
        zellij_runtime::plugin_location::run_plugin_from_url(&format!("file:{}", path.display()))
            .unwrap();
    let (id, _) = test
        .bridge
        .load_plugin(
            &Some(run),
            None,
            Size { rows: 10, cols: 80 },
            None,
            false,
            Some(client),
        )
        .unwrap();
    test.settle(id);
    let original = test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .get_running_plugin(id, Some(client))
        .unwrap();
    let generation = original.lock().unwrap().deno_generation().unwrap();
    test.bridge.add_client(client).unwrap();
    let cancel = test.bridge.client_cancellations.lock().unwrap()[&client].clone();
    assert!(test.bridge.plugin_map.lock().unwrap().contains(id, client));
    test.bridge.remove_client(client);
    assert!(cancel.load(Ordering::Acquire));
    // contains reads the instance's token without waiting for executor disposal.
    assert!(!test.bridge.plugin_map.lock().unwrap().contains(id, client));
    test.bridge.add_client(client).unwrap();
    test.settle(id);
    assert!(original.lock().unwrap().is_disposed());
    let replacement = test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .get_running_plugin(id, Some(client))
        .unwrap();
    assert_ne!(
        replacement.lock().unwrap().deno_generation().unwrap(),
        generation
    );
    let fresh_cancel = test.bridge.client_cancellations.lock().unwrap()[&client].clone();
    assert!(!Arc::ptr_eq(&cancel, &fresh_cancel));
    assert!(!fresh_cancel.load(Ordering::Acquire));
    let (shutdown, _) = tokio::sync::mpsc::channel(1);
    test.bridge
        .apply_cached_events(vec![id], false, shutdown.clone())
        .unwrap();
    test.bridge
        .update_plugins(
            vec![(
                Some(id),
                Some(client),
                Event::Key(zellij_types::keys::KeyWithModifier::new(
                    zellij_types::keys::BareKey::Char('x'),
                )),
            )],
            shutdown,
        )
        .unwrap();
    test.settle(id);
    assert!(test.receiver::<ScreenInstruction>().try_iter().any(|(instruction, _)| {
        matches!(instruction, ScreenInstruction::PluginBytes(assets) if assets.iter().any(|asset|
            asset.client_id == client && String::from_utf8_lossy(&asset.bytes).contains("keys=1")))
    }), "the replacement must receive events");
    let scratch = replacement
        .lock()
        .unwrap()
        .env()
        .plugin_own_data_dir
        .clone();
    let events = FileEvents::new(scratch.parent().unwrap());
    drop(original);
    drop(replacement);
    test.bridge.unload_plugin(id).unwrap();
    events.wait("replacement plugin cleanup", || !scratch.exists());
}

#[test]
fn deno_tagged_output_batch_renders_once_per_client_and_replays_cached_identities() {
    let mut test = TestBridge::new();
    let first = ClientId::new(1);
    let second = ClientId::new(2);
    test.bridge.add_client(first).unwrap();
    test.bridge.add_client(second).unwrap();
    let path = test._dirs[0].path().join("batch.js");
    std::fs::write(
        &path,
        r#"
let chunks = [];
export default {
  load() { zellij.subscribe(['RunCommandOutput']); },
  update(event) {
    if (!event.RunCommandOutput) return false;
    const payload = event.RunCommandOutput;
    if (!(payload.data instanceof Uint8Array)) throw Error('expected bytes');
    chunks.push(`${zellij.getPluginIds().client_id}:${payload.data[0]}`);
    return true;
  },
  render() { return chunks.join(','); }
};
"#,
    )
    .unwrap();
    let mut run =
        zellij_runtime::plugin_location::run_plugin_from_url(&format!("file:{}", path.display()))
            .unwrap();
    run.configuration.insert("shared", "true");
    let (id, _) = test
        .bridge
        .load_plugin(
            &Some(run),
            None,
            Size { rows: 10, cols: 80 },
            None,
            false,
            Some(first),
        )
        .unwrap();
    let instructions = test.receiver::<PluginInstruction>();
    loop {
        let (instruction, _) = instructions
            .recv_timeout(zellij_test_support::default_timeout())
            .unwrap();
        if matches!(instruction, PluginInstruction::ApplyCachedEvents { .. }) {
            break;
        }
    }
    let generation = test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .get_running_plugin_and_subscriptions(id, first)
        .unwrap()
        .0
        .lock()
        .unwrap()
        .deno_generation()
        .unwrap();
    let output = |byte| Event::RunCommandOutput {
        data: vec![byte],
        stderr: false,
        context: BTreeMap::new(),
    };
    let (shutdown, _) = tokio::sync::mpsc::channel(1);
    // Still pending: callbacks must retain the shared instance's acting client.
    test.bridge
        .deno_event(
            id,
            generation,
            vec![(second, output(1)), (first, output(2))],
            shutdown.clone(),
        )
        .unwrap();
    test.bridge
        .deno_event(
            id,
            generation + 1,
            vec![(first, output(99))],
            shutdown.clone(),
        )
        .unwrap();
    test.bridge
        .apply_cached_events(vec![id], false, shutdown.clone())
        .unwrap();
    test.settle(id);
    let screen = test.receiver::<ScreenInstruction>();
    screen.try_iter().for_each(drop);
    // This is one controlled bridge batch, so its render count does not depend on
    // how the plugin thread happens to drain concurrently arriving instructions.
    test.bridge
        .deno_event(
            id,
            generation,
            vec![(first, output(3)), (second, output(4)), (first, output(5))],
            shutdown.clone(),
        )
        .unwrap();
    test.bridge
        .deno_event(id, generation + 1, vec![(first, output(99))], shutdown)
        .unwrap();
    test.settle(id);
    let mut frames: Vec<_> = screen
        .try_iter()
        .flat_map(|(instruction, _)| match instruction {
            ScreenInstruction::PluginBytes(assets) => assets,
            _ => vec![],
        })
        .collect();
    frames.sort_by_key(|asset| asset.client_id);
    assert_eq!(frames.len(), 2, "one render per connected client");
    assert_eq!(
        frames
            .iter()
            .map(|asset| asset.client_id)
            .collect::<Vec<_>>(),
        [first, second]
    );
    for frame in frames {
        assert!(
            String::from_utf8_lossy(&frame.bytes).contains("2:1,1:2,1:3,2:4,1:5"),
            "{:?}",
            frame.bytes
        );
    }
    let scratch = test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .get_running_plugin(id, Some(first))
        .unwrap()
        .lock()
        .unwrap()
        .env()
        .plugin_own_data_dir
        .clone();
    let events = FileEvents::new(scratch.parent().unwrap());
    test.bridge.unload_plugin(id).unwrap();
    events.wait("tagged output plugin cleanup", || !scratch.exists());
}

#[test]
fn deno_cli_pipe_render_releases_its_registration_owner_after_a_queued_rekey() {
    cli_pipe_after_queued_rekey("render");
}

#[test]
fn deno_cli_pipe_without_render_releases_its_registration_owner_after_a_queued_rekey() {
    cli_pipe_after_queued_rekey("no_render");
}

#[test]
fn deno_cli_pipe_error_releases_its_registration_owner_after_a_queued_rekey() {
    cli_pipe_after_queued_rekey("error");
}

fn cli_pipe_after_queued_rekey(outcome: &str) {
    let mut test = TestBridge::new();
    let owner = ClientId::new(1);
    let new_owner = ClientId::new(2);
    let acting_client = ClientId::new(3);
    for client in [owner, new_owner, acting_client] {
        test.bridge.add_client(client).unwrap();
    }
    let path = test._dirs[0].path().join("pipe.js");
    std::fs::write(
        &path,
        r#"
export default {
  pipe(message) {
    if (zellij.getPluginIds().client_id !== 3) throw Error('wrong acting client');
    if (message.name === 'error') throw Error('pipe callback failed');
    return message.name === 'render';
  },
  render() { return `rendered for ${zellij.getPluginIds().client_id}`; }
};
"#,
    )
    .unwrap();
    let mut run =
        zellij_runtime::plugin_location::run_plugin_from_url(&format!("file:{}", path.display()))
            .unwrap();
    run.configuration.insert("shared", "true");
    let (id, _) = test
        .bridge
        .load_plugin(
            &Some(run),
            None,
            Size { rows: 10, cols: 80 },
            None,
            false,
            Some(owner),
        )
        .unwrap();
    test.settle(id);
    let (shutdown, _) = tokio::sync::mpsc::channel(1);
    test.bridge
        .apply_cached_events(vec![id], false, shutdown.clone())
        .unwrap();
    test.settle(id);
    let screen = test.receiver::<ScreenInstruction>();
    let plugin = test.receiver::<PluginInstruction>();
    screen.try_iter().for_each(drop);
    plugin.try_iter().for_each(drop);

    // Hold the executor so registration must happen after removal queues the rekey,
    // but before the rekey changes the instance's owner.
    let (entered, started) = std::sync::mpsc::channel();
    let (release, gate) = std::sync::mpsc::channel();
    test.bridge
        .plugin_executor
        .execute_for_plugin(id, move |_, _, _, _, _| {
            entered.send(()).unwrap();
            gate.recv_timeout(zellij_test_support::default_timeout())
                .unwrap();
        });
    started
        .recv_timeout(zellij_test_support::default_timeout())
        .unwrap();
    test.bridge.remove_client(owner);
    let pipe_id = "rekeyed_cli_pipe";
    test.bridge
        .pipe_messages(
            vec![(
                Some(id),
                Some(acting_client),
                PipeMessage {
                    source: PipeSource::Cli(pipe_id.into()),
                    name: outcome.into(),
                    payload: None,
                    args: BTreeMap::new(),
                    is_private: true,
                    pane_id: None,
                },
            )],
            shutdown,
            None,
        )
        .unwrap();
    let mut pending = test.bridge.pending_pipes.clone();
    assert!(pending
        .update_pipe_state_change(pipe_id, PipeStateChange::NoChange, &id, &new_owner)
        .is_empty());
    assert_eq!(
        pending.update_pipe_state_change(pipe_id, PipeStateChange::NoChange, &id, &owner),
        [pipe_id],
        "the pipe must be registered under the old owner"
    );
    release.send(()).unwrap();
    test.settle(id);
    assert_eq!(
        test.bridge
            .plugin_map
            .lock()
            .unwrap()
            .owner_client_ids_of_plugin_id(id),
        [new_owner],
        "the same loaded instance must finish under its new owner"
    );
    let mut completions = vec![];
    let mut errors = vec![];
    for (instruction, _) in plugin.try_iter() {
        match instruction {
            PluginInstruction::UnblockCliPipes(assets) => completions.extend(assets),
            PluginInstruction::PluginFailed { error, .. } => errors.push(error),
            _ => {},
        }
    }
    if outcome == "error" {
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("pipe callback failed"), "{errors:?}");
    } else {
        assert!(errors.is_empty(), "{errors:?}");
    }
    let mut frames: Vec<_> = screen
        .try_iter()
        .flat_map(|(instruction, _)| match instruction {
            ScreenInstruction::PluginBytes(assets) => assets,
            _ => vec![],
        })
        .collect();
    if outcome == "render" {
        assert!(
            completions.is_empty(),
            "release must wait for the screen acknowledgement"
        );
        frames.sort_by_key(|asset| asset.client_id);
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].client_id, owner);
        assert!(
            frames[0].bytes.is_empty(),
            "the departed owner needs only pipe state"
        );
        for (frame, client) in frames[1..].iter().zip([new_owner, acting_client]) {
            assert_eq!(frame.client_id, client);
            assert!(
                String::from_utf8_lossy(&frame.bytes).contains(&format!("rendered for {client}"))
            );
            assert!(frame.cli_pipes.is_empty());
        }
        // The screen returns these assets in UnblockCliPipes after consuming the bytes.
        completions = frames;
    } else {
        assert!(frames.is_empty());
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].client_id, owner);
    }
    assert_eq!(test.bridge.update_cli_pipe_state(completions), [pipe_id]);
    assert!(test
        .bridge
        .pending_pipes
        .clone()
        .unload_plugin(&id)
        .is_empty());
    let scratch = test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .get_running_plugin(id, Some(new_owner))
        .unwrap()
        .lock()
        .unwrap()
        .env()
        .plugin_own_data_dir
        .clone();
    let events = FileEvents::new(scratch.parent().unwrap());
    test.bridge.unload_plugin(id).unwrap();
    events.wait("rekeyed plugin cleanup", || !scratch.exists());
}

fn run_plugin() -> RunPlugin {
    zellij_runtime::plugin_location::run_plugin_from_url("file:/plugins/job.ts")
        .expect("a valid plugin url")
}

/// Put `plugin_id` on `queue` as a running job, the way `pump_queue` would have.
fn launch_job(bridge: &mut WasmBridge, queue: &str, plugin_id: PluginId) {
    let mut queues = bridge.queues.lock().unwrap();
    queues.enqueue(
        queue,
        (crate::PluginId::new(1), crate::ClientId::new(1)),
        None,
        zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
            "file:/plugins/job.ts",
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
fn a_stale_deno_failure_neither_paints_an_error_nor_finishes_a_job() {
    let mut test = TestBridge::new();
    let plugin_id = PluginId::new(7);
    launch_job(&mut test.bridge, "merge", plugin_id);
    let screen = test._receivers.iter().find_map(|receiver| {
        receiver.downcast_ref::<channels::Receiver<(ScreenInstruction, zellij_types::diagnostics::ErrorContext)>>()
    }).unwrap();
    let plugin = test._receivers.iter().find_map(|receiver| {
        receiver.downcast_ref::<channels::Receiver<(PluginInstruction, zellij_types::diagnostics::ErrorContext)>>()
    }).unwrap();
    // Drain setup messages; every assertion below observes only failure handling.
    screen.try_iter().for_each(drop);
    plugin.try_iter().for_each(drop);
    // Generation 41 has already left the map. Its queued error must have no effects,
    // even though the plugin id still names a running queue job.
    handle_plugin_crash(
        plugin_id,
        Some(41),
        "old error".to_owned(),
        test.bridge.senders.clone(),
    );
    assert!(
        screen.try_recv().is_err(),
        "Deno failures must wait for generation validation before painting"
    );
    let (
        PluginInstruction::PluginFailed {
            plugin_id,
            deno_generation,
            error,
            failed_to_load,
        },
        _,
    ) = plugin.try_recv().unwrap()
    else {
        panic!("expected the failure report");
    };
    assert_eq!(deno_generation, Some(41));
    test.bridge
        .queue_plugin_failed(plugin_id, deno_generation, error, failed_to_load)
        .unwrap();
    assert!(
        screen.try_recv().is_err(),
        "stale failures must not paint or close the pane"
    );
    let snapshot = test
        .bridge
        .queues
        .lock()
        .unwrap()
        .queue_snapshot("merge")
        .unwrap();
    assert_eq!(
        snapshot.running.len(),
        1,
        "stale failures must not finish the current job"
    );
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
        .queue_plugin_failed(
            crate::PluginId::new(7),
            None,
            "no such file".to_owned(),
            true,
        )
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
        .queue_plugin_failed(crate::PluginId::new(7), None, "boom".to_owned(), false)
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
import z from 'zellij:plugin';
export default {
  load() { z.subscribe(['BeforeClose']); },
  update(event) {
    if (event === 'BeforeClose') {
      z.fs.write('/data/closed', z.fs.read('/data/value'));
      z.fs.write('/cache/saved', z.fs.read('/data/closed'));
    }
    return false;
  },
};
"#;

impl TestBridge {
    fn context(&mut self, id: u32, client: u16, source: &str) -> LoadingContext {
        let plugin_id = PluginId::new(id);
        let client_id = ClientId::new(client);
        self.bridge.cancel_token(plugin_id);
        self.bridge.plugin_executor.register_plugin(plugin_id);
        let path = self._dirs[0].path().join(format!(
            "{id}.{}",
            if source.starts_with("(module") {
                "wasm"
            } else {
                "ts"
            }
        ));
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
        let done = zellij_test_support::Watched::new(None);
        let observed = done.clone();
        self.bridge.plugin_executor.execute_for_plugin_tagged(
            context.plugin_id,
            "fixture load",
            false,
            move |senders, plugin_map, connected_clients, cache, engine| {
                let result = PluginLoader::new(
                    false,
                    context,
                    senders,
                    engine,
                    cache,
                    &mut plugin_map.lock().unwrap(),
                    connected_clients,
                )
                .without_connected_clients()
                .start_plugin();
                *observed.lock() = Some(result);
            },
        );
        done.wait_until("fixture load", Duration::from_secs(30), |result| {
            result.as_ref().map(|_| ())
        });
        let result = done.lock().take().unwrap();
        result
    }

    fn wait_for_executor(&self, plugin_id: PluginId) {
        let done = zellij_test_support::Watched::new(false);
        let observed = done.clone();
        self.bridge.plugin_executor.execute_for_plugin_tagged(
            plugin_id,
            "completion barrier",
            false,
            move |_, _, _, _, _| *observed.lock() = true,
        );
        done.wait_until("executor completion", Duration::from_secs(30), |done| {
            done.then_some(())
        });
    }

    fn finish_reload(&mut self, plugin_id: PluginId) {
        self.wait_for_executor(plugin_id);
        // This harness has no plugin thread to handle the loader's ApplyCachedEvents message.
        let (shutdown_sender, _receiver) = tokio::sync::mpsc::channel(1);
        self.bridge
            .apply_cached_events(vec![plugin_id], false, shutdown_sender)
            .unwrap();
        self.wait_for_executor(plugin_id);
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
    let two = test.context(2, 1, "export default {}");
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
    test.bridge
        .connected_clients
        .lock()
        .unwrap()
        .push(ClientId::new(2));
    let secondary = test.context(1, 2, SCRATCH_PLUGIN);
    let secondary_path = secondary.plugin_own_data_dir.clone();
    std::fs::write(secondary_path.join("value"), "other client").unwrap();
    test.load(secondary).unwrap();
    test.grant_files(1, 2);
    test.bridge.reload_plugin_with_id(PluginId::new(1)).unwrap();
    test.finish_reload(PluginId::new(1));
    events.wait("secondary reload data cleanup", || !secondary_path.exists());
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

// wasmi's existing test features accept WAT, so these fixtures need no compiler or dependency.
const RECOVERABLE_WASM: &str = r#"(module
    (func (export "_start"))
    (func (export "load"))
    (func (export "retry_worker"))
    (func (export "update") (result i32) i32.const 0)
    (func (export "render") (param i32 i32))
)"#;

#[test]
fn resizing_after_the_screen_exits_keeps_the_executor_and_plugin_usable() {
    let mut test = TestBridge::new();
    let plugin_id = PluginId::new(1);
    let client_id = ClientId::new(1);
    let context = test.context(1, 1, RECOVERABLE_WASM);
    let scratch = context.plugin_own_data_dir.clone();
    let events = FileEvents::new(scratch.parent().unwrap());
    test.load(context).unwrap();
    test.settle(plugin_id);

    // Simulate shutdown after the screen receiver exits but before queued plugin
    // resizes drain. Keep the other channels alive so the failed publication is real.
    test._receivers.retain(|receiver| {
        !receiver
            .is::<channels::Receiver<(ScreenInstruction, zellij_types::diagnostics::ErrorContext)>>(
            )
    });
    let (shutdown, _receiver) = tokio::sync::mpsc::channel(1);
    test.bridge
        .resize_plugin(plugin_id, 31, 9, shutdown)
        .unwrap();
    test.settle(plugin_id);

    let plugin = test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .get_running_plugin(plugin_id, Some(client_id))
        .unwrap();
    assert_eq!(
        {
            let plugin = plugin.lock().unwrap();
            (plugin.columns, plugin.rows)
        },
        (31, 9)
    );
    drop(plugin);
    test.bridge.unload_plugin(plugin_id).unwrap();
    events.wait("resized plugin cleanup", || !scratch.exists());
}

fn assert_reload_recovers(wasm: bool, by_url: bool) {
    let mut test = TestBridge::new();
    let id = PluginId::new(1);
    let owner = ClientId::new(7);
    let good = if wasm {
        RECOVERABLE_WASM
    } else {
        "export default {}"
    };
    let bad = if wasm {
        RECOVERABLE_WASM.replace("(export \"load\")", "(export \"load\") unreachable")
    } else {
        "export default { load() { throw Error('load failed'); } }".to_owned()
    };
    let mut context = test.context(1, 1, good);
    let source = context
        .plugin_config
        .path
        .with_extension(if wasm { "wasm" } else { "ts" });
    std::fs::write(&source, good).unwrap();
    context.plugin_config.path = source.clone();
    context.plugin_config.location = RunPluginLocation::File(source.clone());
    if !wasm {
        // A relative TypeScript URL resolves through the user's plugin folder. Recovery must also
        // recognize the full source path after the instance that resolved it has gone.
        let named = test._dirs[0].path().join("missing/1.ts");
        context.plugin_config.path = named.clone();
        context.plugin_config.location = RunPluginLocation::File(named);
        context.plugin_config.relative_path = Some(PathBuf::from("1.ts"));
        context.user_plugin_dir = Some(test._dirs[0].path().to_path_buf());
        test.bridge.user_plugin_dir = context.user_plugin_dir.clone();
    }
    context.plugin_config.initial_userspace_configuration = PluginUserConfiguration::new(
        BTreeMap::from([("setting".to_owned(), "preserved".to_owned())]),
    );
    let config = context.plugin_config.clone();
    let cache = ZELLIJ_CACHE_DIR
        .join(Url::from(&config.location).to_string())
        .join("plugin_cache");
    context.plugin_own_cache_dir = cache.clone();
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("saved"), "persistent state").unwrap();
    let scratch = context.plugin_own_data_dir.clone();
    let events = FileEvents::new(scratch.parent().unwrap());
    test.load(context).unwrap();
    let cwd = test._dirs[0].path().to_path_buf();
    let weak = {
        let mut map = test.bridge.plugin_map.lock().unwrap();
        assert!(map.rekey_client(id, ClientId::new(1), owner));
        let plugin = map.get_running_plugin(id, Some(owner)).unwrap();
        let mut running = plugin.lock().unwrap();
        // Live values must win over the original insertion snapshot.
        running.rows = 13;
        running.columns = 37;
        running.env_mut().plugin_cwd = cwd.clone();
        running.env_mut().tab_index = Some(TabId::new(4));
        Arc::downgrade(&plugin)
    };
    assert!(test.bridge.connected_clients.lock().unwrap().is_empty());
    for _ in 0..2 {
        let scratch = test
            .bridge
            .plugin_map
            .lock()
            .unwrap()
            .get_running_plugin(id, None)
            .unwrap()
            .lock()
            .unwrap()
            .env()
            .plugin_own_data_dir
            .clone();
        std::fs::write(&source, &bad).unwrap();
        test.bridge.reload_plugin_with_id(id).unwrap();
        test.finish_reload(id);
        assert!(weak.upgrade().is_none());
        events.wait("failed runtime and workers release /data", || {
            !scratch.exists()
        });
        assert!(test
            .bridge
            .plugin_map
            .lock()
            .unwrap()
            .get_running_plugin(id, None)
            .is_none());
        assert_eq!(
            test.bridge.plugin_config_of_plugin_id(id),
            Some(config.clone())
        );
        assert_eq!(test.bridge.size_of_plugin_id(id), Some((13, 37)));
        assert_eq!(test.bridge.cwd_of_plugin_id(id), Some(cwd.clone()));
        assert_eq!(test.bridge.tab_index_of_plugin_id(id), Some(TabId::new(4)));
        let location = RunPluginLocation::File(source.clone());
        assert_eq!(
            test.bridge
                .all_plugin_ids_for_plugin_location(&location, &PluginUserConfiguration::default())
                .unwrap(),
            vec![id]
        );
        assert!(test
            .bridge
            .all_plugin_ids_for_plugin_location(
                &location,
                &PluginUserConfiguration::new(BTreeMap::from([(
                    "setting".to_owned(),
                    "different".to_owned()
                )]))
            )
            .is_err());
        std::fs::write(&source, good).unwrap();
        if by_url {
            test.bridge
                .reload_plugin(&RunPlugin {
                    location,
                    ..Default::default()
                })
                .unwrap();
        } else {
            test.bridge.reload_plugin_with_id(id).unwrap();
        }
        test.finish_reload(id);
        let map = test.bridge.plugin_map.lock().unwrap();
        assert_eq!(map.all_plugin_ids(), vec![(id, owner)]);
        let plugin = map.get_running_plugin(id, Some(owner)).unwrap();
        let running = plugin.lock().unwrap();
        assert_ne!(running.env().plugin_own_data_dir, scratch);
        assert!(running.env().plugin_own_data_dir.exists());
        assert_eq!(running.env().plugin_own_cache_dir, cache);
        assert_eq!(
            std::fs::read_to_string(cache.join("saved")).unwrap(),
            "persistent state"
        );
        assert_eq!(running.env().plugin, config);
        assert_eq!((running.rows, running.columns), (13, 37));
        assert_eq!(running.env().plugin_cwd, cwd);
        assert_eq!(running.env().tab_index, Some(TabId::new(4)));
    }
    assert!(!scratch.exists());
    test.bridge.unload_plugin(id).unwrap();
    assert!(test.bridge.run_plugin_of_plugin_id(id).is_none());
    assert!(test.bridge.plugin_config_of_plugin_id(id).is_none());
    assert!(test
        .bridge
        .all_plugin_ids_for_plugin_location(
            &RunPluginLocation::File(source),
            &PluginUserConfiguration::default()
        )
        .is_err());
}

#[test]
fn deno_reload_recovers_by_id_after_load_failure() {
    assert_reload_recovers(false, false);
}

#[test]
fn deno_reload_recovers_by_url_after_load_failure() {
    assert_reload_recovers(false, true);
}

#[test]
fn wasm_reload_recovers_by_id_after_load_failure() {
    assert_reload_recovers(true, false);
}

#[test]
fn wasm_reload_recovers_by_url_after_load_failure() {
    assert_reload_recovers(true, true);
}

#[test]
fn removing_an_instance_retains_latest_values_and_url_lookup_deduplicates_ids() {
    let mut test = TestBridge::new();
    let one = test.context(1, 1, "export default {}");
    let location = one.plugin_config.location.clone();
    test.load(one).unwrap();
    let two = test.context(1, 2, "export default {}");
    test.load(two).unwrap();
    let id = PluginId::new(1);
    let mut map = test.bridge.plugin_map.lock().unwrap();
    assert_eq!(
        map.all_plugin_ids_for_plugin_location(&location, &PluginUserConfiguration::default())
            .unwrap(),
        vec![id]
    );
    let destinations = &map.clone_plugin_assets()[&location][&PluginUserConfiguration::default()];
    assert!(destinations.contains(&(id, ClientId::new(1))));
    assert!(destinations.contains(&(id, ClientId::new(2))));
    drop(map.remove_instance(id, ClientId::new(1)));
    let plugin = map.get_running_plugin(id, None).unwrap();
    let weak = Arc::downgrade(&plugin);
    {
        let mut running = plugin.lock().unwrap();
        running.rows = 17;
        running.columns = 42;
        running.env_mut().plugin_cwd = PathBuf::from("/changed");
        running.env_mut().tab_index = Some(TabId::new(9));
    }
    drop(plugin);
    drop(map.remove_instance(id, ClientId::new(2)));
    assert!(weak.upgrade().is_none());
    let retained = map.reload_metadata(id).unwrap();
    assert_eq!(retained.size, (17, 42));
    assert_eq!(retained.cwd, PathBuf::from("/changed"));
    assert_eq!(retained.tab_index, Some(TabId::new(9)));
    assert_eq!(retained.owner_client_id, ClientId::new(2));
    map.remove_plugins(id);
    assert!(map.reload_metadata(id).is_none());
}

#[test]
fn failed_loads_and_cancelled_loading_contexts_release_temporary_data() {
    let mut test = TestBridge::new();
    for source in [
        "not typescript!",
        "export default { load() { throw Error('load failed'); } }",
    ] {
        let context = test.context(1, 1, source);
        let path = context.plugin_own_data_dir.clone();
        assert!(test.load(context).is_err());
        assert!(!path.exists());
    }
    let context = test.context(2, 1, "export default {}");
    let path = context.plugin_own_data_dir.clone();
    drop(context); // A download/queued load cancelled before instantiation.
    assert!(!path.exists());
}

#[test]
fn departing_client_releases_its_data_while_another_client_stays() {
    let mut test = TestBridge::new();
    *test.bridge.connected_clients.lock().unwrap() = vec![ClientId::new(1), ClientId::new(2)];
    let one = test.context(1, 1, "export default {}");
    let one_path = one.plugin_own_data_dir.clone();
    let events = FileEvents::new(one_path.parent().unwrap());
    test.load(one).unwrap();
    let two = test.context(1, 2, "export default {}");
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
    let context = test.context(1, 1, "export default {}");
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
    let mut context = test.context(1, 1, "export default {}");
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

fn assert_unload_cancels_queued_reload(failed: bool) {
    let mut test = TestBridge::new();
    test.bridge
        .connected_clients
        .lock()
        .unwrap()
        .push(ClientId::new(1));
    let context = test.context(1, 1, "export default {}");
    let path = context.plugin_own_data_dir.clone();
    let events = FileEvents::new(path.parent().unwrap());
    let source = context.plugin_config.path.clone();
    test.load(context).unwrap();
    if failed {
        std::fs::write(
            &source,
            "export default { load() { throw Error('failed'); } }",
        )
        .unwrap();
        test.bridge.reload_plugin_with_id(PluginId::new(1)).unwrap();
        test.finish_reload(PluginId::new(1));
        assert!(!path.exists());
        std::fs::write(&source, "export default {}").unwrap();
    }
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
    assert!(test
        .bridge
        .run_plugin_of_plugin_id(PluginId::new(1))
        .is_none());
    let done = zellij_test_support::Watched::new(false);
    let observed = done.clone();
    // Queue before releasing the executor, while the live unload has not unregistered it.
    test.bridge.plugin_executor.execute_for_plugin_tagged(
        PluginId::new(1),
        "observe unloaded reload",
        false,
        move |_, _, _, _, _| *observed.lock() = true,
    );
    *release.lock() = true;
    done.wait_until(
        "queued reload completion",
        Duration::from_secs(30),
        |done| done.then_some(()),
    );
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
fn unload_cancels_a_queued_reload_and_client_load() {
    assert_unload_cancels_queued_reload(false);
}

#[test]
fn unload_cancels_a_queued_recovery_after_load_failure() {
    assert_unload_cancels_queued_reload(true);
}

#[test]
fn reconnecting_before_client_cleanup_keeps_the_live_instance() {
    let mut test = TestBridge::new();
    *test.bridge.connected_clients.lock().unwrap() = vec![ClientId::new(1), ClientId::new(2)];
    let one = test.context(1, 1, RECOVERABLE_WASM);
    let path = one.plugin_own_data_dir.clone();
    test.load(one).unwrap();
    let two = test.context(1, 2, RECOVERABLE_WASM);
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
