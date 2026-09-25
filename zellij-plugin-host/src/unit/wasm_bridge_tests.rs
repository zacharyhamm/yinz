//! Bookkeeping around plugins that are still *loading* - in `loading_plugins`, not yet in the
//! `PluginMap`. These are the states no end-to-end test can observe: whether a cancellation mark
//! was left for a loader that will run, or for one that already gave up.

use super::*;

use notify_debouncer_full::notify::RecommendedWatcher;
use std::collections::BTreeMap;
use tempfile::tempdir;
use zellij_runtime::channels::{self, SenderWithContext};
use zellij_types::data::PluginTag;

/// Senders nothing listens to. The receivers are returned so the channels stay connected for as
/// long as the bridge does.
struct TestBridge {
    bridge: WasmBridge,
    _receivers: Vec<Box<dyn std::any::Any>>,
    _dirs: Vec<tempfile::TempDir>,
}

impl TestBridge {
    fn hold_executor(&self, plugin: PluginId) -> std::sync::mpsc::SyncSender<()> {
        let (entered, started) = std::sync::mpsc::sync_channel(1);
        let (release, gate) = std::sync::mpsc::sync_channel(1);
        self.bridge
            .plugin_executor
            .execute_for_plugin(plugin, move |_, _, _, _, _| {
                entered.send(()).unwrap();
                gate.recv_timeout(zellij_test_support::default_timeout())
                    .unwrap();
            });
        started
            .recv_timeout(zellij_test_support::default_timeout())
            .unwrap();
        release
    }

    fn pipe(&mut self, plugin: PluginId, name: &str) {
        let (shutdown, _) = tokio::sync::mpsc::channel(1);
        self.bridge
            .pipe_messages(
                vec![(
                    Some(plugin),
                    None,
                    PipeMessage::new(PipeSource::Cli(name.into()), name, &None, &None, true),
                )],
                shutdown,
                None,
            )
            .unwrap();
    }

    fn pipe_outcomes(&mut self) -> PipeOutcomes {
        let mut outcomes = vec![];
        for (instruction, _) in self.receiver::<PluginInstruction>().try_iter() {
            match instruction {
                PluginInstruction::PipeCompleted(completion) => {
                    outcomes.extend(self.bridge.update_cli_pipe_state(vec![completion]))
                },
                PluginInstruction::ReleasePluginInstancePipes(plugin, instance) => {
                    outcomes.extend(self.bridge.release_plugin_instance_pipes(plugin, instance))
                },
                _ => {},
            }
        }
        outcomes.extend(self.receiver::<ServerInstruction>().try_iter().filter_map(
            |(instruction, _)| match instruction {
                ServerInstruction::UnblockCliPipeInput(pipe) => Some((pipe, Ok(()))),
                ServerInstruction::CliPipeError(pipe, error) => Some((pipe, Err(error))),
                _ => None,
            },
        ));
        outcomes
    }

    fn pipe_acknowledgements(&mut self) -> Vec<String> {
        self.pipe_outcomes()
            .into_iter()
            .map(|(pipe, result)| {
                result.unwrap();
                pipe
            })
            .collect()
    }

    fn pipe_failures(&mut self) -> Vec<String> {
        self.pipe_outcomes()
            .into_iter()
            .map(|(pipe, result)| {
                assert!(result.is_err(), "unexpected success for {pipe}");
                pipe
            })
            .collect()
    }

    fn complete(&mut self, completions: Vec<PipeCompletion>) -> Vec<String> {
        let outcomes = self.bridge.update_cli_pipe_state(completions);
        self.successes(outcomes)
    }

    fn retire(&mut self, plugin: PluginId, instance: PluginInstanceId) -> Vec<String> {
        let outcomes = self.bridge.release_plugin_instance_pipes(plugin, instance);
        self.successes(outcomes)
    }

    fn successes(&self, outcomes: PipeOutcomes) -> Vec<String> {
        let mut successes = vec![];
        for (pipe, result) in outcomes {
            match result {
                Ok(()) => successes.push(pipe),
                Err(error) => self.bridge.send_pipe_outcomes(vec![(pipe, Err(error))]),
            }
        }
        successes
    }

    fn errors(&self) -> Vec<String> {
        self.receiver::<ServerInstruction>()
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                ServerInstruction::CliPipeError(pipe, _) => Some(pipe),
                _ => None,
            })
            .collect()
    }

    fn instance_id(&self, plugin: PluginId, client: ClientId) -> PluginInstanceId {
        self.bridge
            .plugin_map
            .lock()
            .unwrap()
            .pipe_instances()
            .into_iter()
            .find(|((id, owner, _, _), _)| *id == plugin && *owner == client)
            .unwrap()
            .1
    }

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
fn cancelled_deno_load_never_leaves_a_cached_pipe_recipient() {
    for pipe_before_disconnect in [false, true] {
        let mut test = TestBridge::new();
        let plugin = PluginId::new(1);
        let first = ClientId::new(1);
        let second = ClientId::new(2);
        test.bridge.add_client(first).unwrap();
        let context = test.context(1, 1, "export default { pipe() { return false; } };");
        test.load(context).unwrap();
        let release = test.hold_executor(plugin);
        test.bridge.add_client(second).unwrap();
        if pipe_before_disconnect {
            test.pipe(plugin, "cancelled-load");
        }
        test.bridge.remove_client(second);
        if !pipe_before_disconnect {
            test.pipe(plugin, "cancelled-load");
        }
        assert!(test.pipe_acknowledgements().is_empty());
        release.send(()).unwrap();
        test.settle(plugin);
        assert!(!test
            .bridge
            .plugin_map
            .lock()
            .unwrap()
            .contains(plugin, second));
        test.finish_reload(plugin);
        assert_eq!(test.pipe_acknowledgements(), ["cancelled-load"]);
        assert!(test.pipe_acknowledgements().is_empty());
        assert!(test.bridge.pending_pipes.take_ready().is_empty());
    }
}

#[test]
fn old_cleanup_and_render_acknowledgements_cannot_release_a_reconnected_instance() {
    for source in [
        RECOVERABLE_WASM,
        "export default { pipe() { return false; } };",
    ] {
        let mut test = TestBridge::new();
        let plugin = PluginId::new(1);
        let client = ClientId::new(1);
        test.bridge.add_client(client).unwrap();
        test.bridge.add_client(ClientId::new(2)).unwrap();
        for client in [1, 2] {
            let context = test.context(1, client, source);
            test.load(context).unwrap();
        }
        let old_instance = test.instance_id(plugin, client);
        test.pipe(plugin, "reused-pipe");
        test.settle(plugin);
        let old_completions: Vec<_> = test
            .receiver::<PluginInstruction>()
            .try_iter()
            .filter_map(|(instruction, _)| {
                if let PluginInstruction::PipeCompleted(completion) = instruction {
                    Some(vec![completion])
                } else {
                    None
                }
            })
            .flatten()
            .collect();
        assert_eq!(test.complete(old_completions.clone()), ["reused-pipe"]);
        test.bridge.remove_client(client);
        test.settle(plugin);
        let removals: Vec<_> = test
            .receiver::<PluginInstruction>()
            .try_iter()
            .filter_map(|(instruction, _)| {
                if let PluginInstruction::ReleasePluginInstancePipes(id, instance) = instruction {
                    Some((id, instance))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(removals, [(plugin, old_instance)]);
        test.bridge.add_client(client).unwrap();
        test.settle(plugin);
        let replacement = test.instance_id(plugin, client);
        assert_ne!(replacement, old_instance);
        // Hold cached replay, then deliver the old instance's deferred messages.
        test.pipe(plugin, "reused-pipe");
        assert!(test.retire(plugin, old_instance).is_empty());
        assert!(test.complete(old_completions.clone()).is_empty());
        let release = test.hold_executor(plugin);
        let (shutdown, _) = tokio::sync::mpsc::channel(1);
        test.bridge
            .apply_cached_events(vec![plugin], true, shutdown)
            .unwrap();
        assert!(test.retire(plugin, old_instance).is_empty());
        assert!(test.complete(old_completions).is_empty());
        assert!(test.pipe_acknowledgements().is_empty());
        release.send(()).unwrap();
        test.settle(plugin);
        assert_eq!(test.pipe_acknowledgements(), ["reused-pipe"]);
    }
}

#[test]
fn cached_pipe_without_an_instance_fails_on_replay_or_load_failure() {
    for failed in [false, true] {
        let mut test = TestBridge::new();
        let plugin = PluginId::new(1);
        test.bridge.loading_plugins.insert((plugin, run_plugin()));
        test.bridge.cache_plugin_events(plugin);
        test.pipe(plugin, "no-instance");
        assert!(test.pipe_acknowledgements().is_empty());
        if failed {
            test.bridge
                .queue_plugin_failed(plugin, None, None, None, "load failed".into(), true)
                .unwrap();
        } else {
            let (shutdown, _) = tokio::sync::mpsc::channel(1);
            test.bridge
                .apply_cached_events(vec![plugin], true, shutdown)
                .unwrap();
        }
        assert_eq!(test.pipe_failures(), ["no-instance"]);
        assert!(test.bridge.pending_pipes.take_ready().is_empty());
        test.pipe(plugin, "no-instance");
        assert_eq!(test.pipe_failures(), ["no-instance"]);
    }
}

#[test]
fn load_failure_reports_each_cached_request_once_without_acknowledging() {
    for queued_job in [false, true] {
        let mut test = TestBridge::new();
        let plugin = PluginId::new(1);
        if queued_job {
            launch_job(&mut test.bridge, "load-failure", plugin);
        }
        let load = test.bridge.begin_plugin_load(plugin);
        test.bridge.loading_plugins.insert((plugin, run_plugin()));
        test.bridge.cache_plugin_events(plugin);
        for pipe in ["first", "second", "first"] {
            test.pipe(plugin, pipe);
        }
        assert!(test.receiver::<ServerInstruction>().try_recv().is_err());
        test.bridge
            .queue_plugin_failed(plugin, Some(load), None, None, "load failed".into(), true)
            .unwrap();
        let messages: Vec<_> = test
            .receiver::<ServerInstruction>()
            .try_iter()
            .map(|(instruction, _)| instruction)
            .collect();
        for pipe in ["first", "second"] {
            let replies: Vec<_> = messages
                .iter()
                .filter_map(|instruction| match instruction {
                    ServerInstruction::CliPipeError(id, error) if id == pipe => {
                        assert_eq!(error, "load failed");
                        Some("error")
                    },
                    ServerInstruction::UnblockCliPipeInput(id) if id == pipe => Some("ack"),
                    _ => None,
                })
                .collect();
            assert_eq!(
                replies,
                vec!["error"; if pipe == "first" { 2 } else { 1 }],
                "{pipe}: {messages:?}"
            );
        }
        assert!(test.bridge.pending_pipes.take_ready().is_empty());
    }
}

#[test]
fn resolution_failure_reports_each_cached_request_without_acknowledging() {
    for (run, diagnostic) in [
        (
            Some(RunPlugin {
                location: RunPluginLocation::Zellij(PluginTag::new("not-a-builtin")),
                ..Default::default()
            }),
            "Failed to resolve plugin: not-a-builtin",
        ),
        (None, "Failed to resolve plugin alias"),
    ] {
        let mut test = TestBridge::new();
        let ui_client = ClientId::new(1);
        test.bridge.add_client(ui_client).unwrap();
        let (plugin, client) = test
            .bridge
            .load_plugin(
                &run,
                None,
                Size::default(),
                None,
                false,
                Some(ClientId::new(2)),
            )
            .unwrap();
        assert_eq!(client, ui_client);
        for _ in 0..2 {
            test.pipe(plugin, "resolution-failure");
        }
        assert!(
            !test
                .receiver::<ServerInstruction>()
                .try_iter()
                .any(|(instruction, _)| {
                    matches!(
                        instruction,
                        ServerInstruction::CliPipeError(..)
                            | ServerInstruction::UnblockCliPipeInput(_)
                    )
                }),
            "pipe completed before PluginFailed: {diagnostic}"
        );
        assert_eq!(
            test.bridge.cached_events_for_pending_plugins[&plugin].len(),
            2
        );

        let PluginInstruction::PluginFailed {
            plugin_id,
            load_id,
            deno_generation,
            instance_id,
            error,
            failed_to_load,
            ..
        } = test.receiver::<PluginInstruction>().try_recv().unwrap().0
        else {
            panic!("expected PluginFailed for {diagnostic}");
        };
        assert_eq!(plugin_id, plugin);
        assert!(failed_to_load);
        assert!(error.contains(diagnostic), "{error}");
        test.bridge
            .queue_plugin_failed(
                plugin_id,
                load_id,
                deno_generation,
                instance_id,
                error.clone(),
                failed_to_load,
            )
            .unwrap();
        let replies: Vec<_> = test
            .receiver::<ServerInstruction>()
            .try_iter()
            .map(|(instruction, _)| match instruction {
                ServerInstruction::CliPipeError(pipe, message) => {
                    assert_eq!(pipe, "resolution-failure");
                    assert_eq!(message, error);
                    "error"
                },
                ServerInstruction::UnblockCliPipeInput(pipe) => {
                    assert_eq!(pipe, "resolution-failure");
                    "ack"
                },
                other => panic!("unexpected reply: {other:?}"),
            })
            .collect();
        assert_eq!(replies, ["error", "error"]);
        assert!(test.bridge.cached_events_for_pending_plugins.is_empty());
        assert!(test.bridge.pending_pipes.take_ready().is_empty());
    }
}

#[test]
fn reload_and_runtime_failure_release_explicit_holds_after_output() {
    for reload in [false, true] {
        let mut test = TestBridge::new();
        let plugin = PluginId::new(1);
        let client = ClientId::new(1);
        test.bridge.add_client(client).unwrap();
        let context = test.context(
            1,
            1,
            r#"
export default {
  load() { zellij.subscribe(['CustomMessage']); },
  pipe(message) {
    zellij.command('BlockCliPipeInput', message.source.Cli);
    zellij.command('CliPipeOutput', [message.source.Cli, 'reply before retirement']);
    return false;
  },
  update(event) { if (event.CustomMessage) throw Error('retire this runtime'); return false; }
};
"#,
        );
        test.load(context).unwrap();
        let instance = test
            .bridge
            .plugin_map
            .lock()
            .unwrap()
            .get_running_plugin(plugin, Some(client))
            .unwrap();
        *instance.lock().unwrap().env().permissions.lock().unwrap() =
            Some(HashSet::from([PermissionType::ReadCliPipes]));
        test.pipe(plugin, "held");
        test.settle(plugin);
        assert!(test.receiver::<ServerInstruction>().try_iter().any(|(instruction, _)| {
            matches!(instruction, ServerInstruction::CliPipeOutput(_, ref output) if output == "reply before retirement")
        }));
        assert!(
            test.pipe_acknowledgements().is_empty(),
            "an explicit hold must survive callback completion"
        );
        let original = test.instance_id(plugin, client);
        if reload {
            test.bridge.reload_plugin_with_id(plugin).unwrap();
            test.finish_reload(plugin);
            assert_ne!(test.instance_id(plugin, client), original);
        } else {
            let (shutdown, _) = tokio::sync::mpsc::channel(1);
            test.bridge
                .update_plugins(
                    vec![(
                        Some(plugin),
                        Some(client),
                        Event::CustomMessage("fail".into(), String::new()),
                    )],
                    shutdown,
                )
                .unwrap();
            test.settle(plugin);
            let mut failed = false;
            for (instruction, _) in test
                .receiver::<PluginInstruction>()
                .try_iter()
                .collect::<Vec<_>>()
            {
                if let PluginInstruction::PluginFailed {
                    plugin_id,
                    deno_generation,
                    instance_id,
                    error,
                    failed_to_load,
                    ..
                } = instruction
                {
                    failed = true;
                    test.bridge
                        .queue_plugin_failed(
                            plugin_id,
                            None,
                            deno_generation,
                            instance_id,
                            error,
                            failed_to_load,
                        )
                        .unwrap();
                }
            }
            assert!(failed);
            test.settle(plugin);
        }
        assert_eq!(test.pipe_failures(), ["held"]);
        assert!(test.retire(plugin, original).is_empty());
    }
}

#[test]
fn stale_loader_results_cannot_drop_or_replay_a_replacements_cached_pipe() {
    let mut test = TestBridge::new();
    let plugin = PluginId::new(1);
    let old_load = test.bridge.begin_plugin_load(plugin);
    let new_load = test.bridge.begin_plugin_load(plugin);
    test.bridge.loading_plugins.insert((plugin, run_plugin()));
    test.bridge.cache_plugin_events(plugin);
    test.pipe(plugin, "replacement");
    let (shutdown, _) = tokio::sync::mpsc::channel(1);
    test.bridge
        .apply_cached_events_for_load(old_load, vec![plugin], true, shutdown.clone())
        .unwrap();
    test.bridge
        .queue_plugin_failed(
            plugin,
            Some(old_load),
            None,
            None,
            "obsolete failure".into(),
            true,
        )
        .unwrap();
    assert!(test.receiver::<ServerInstruction>().try_recv().is_err());
    assert!(test.pipe_acknowledgements().is_empty());
    assert_eq!(
        test.bridge.cached_events_for_pending_plugins[&plugin].len(),
        1
    );
    let context = test.context(1, 1, RECOVERABLE_WASM);
    test.load(context).unwrap();
    test.bridge
        .apply_cached_events_for_load(new_load, vec![plugin], true, shutdown)
        .unwrap();
    test.settle(plugin);
    assert_eq!(test.pipe_acknowledgements(), ["replacement"]);
}

#[test]
fn eof_lookup_finds_loading_plugins_without_starting_a_replacement() {
    let mut test = TestBridge::new();
    let run = RunPlugin {
        location: RunPluginLocation::File(PathBuf::from("/pipe-recipient.wasm")),
        ..Default::default()
    };
    let id = PluginId::new(7);
    for loading in [false, true] {
        if loading {
            test.bridge.loading_plugins.insert((id, run.clone()));
        }
        let recipients = test
            .bridge
            .get_or_load_plugins(
                RunPluginOrAlias::RunPlugin(run.clone()),
                Size::default(),
                None,
                false,
                true,
                false,
                None,
                None,
                Some(ClientId::new(1)),
                None,
                false,
                false,
            )
            .unwrap();
        assert_eq!(recipients, if loading { vec![(id, None)] } else { vec![] });
        assert_eq!(test.bridge.next_plugin_id, 0);
        assert!(test.receiver::<ScreenInstruction>().is_empty());
        assert!(test.receiver::<ServerInstruction>().is_empty());
    }
    test.bridge.loading_plugins.clear();
}

#[test]
fn detached_cached_cli_pipe_during_loading_is_acknowledged_once() {
    detached_cached_cli_pipe(true, false);
}

#[test]
fn detached_cached_cli_pipe_during_permission_caching_is_acknowledged_once() {
    detached_cached_cli_pipe(false, false);
}

#[test]
fn detached_cached_cli_pipe_unloaded_during_loading_fails_once() {
    detached_cached_cli_pipe(true, true);
}

#[test]
fn detached_cached_cli_pipe_unloaded_during_permission_caching_fails_once() {
    detached_cached_cli_pipe(false, true);
}

fn detached_cached_cli_pipe(loading: bool, unload: bool) {
    for shared in [false, true] {
        let mut test = TestBridge::new();
        let owner = ClientId::new(7);
        test.bridge.add_client(owner).unwrap();
        let id = PluginId::new(test.bridge.next_plugin_id);
        let path = test._dirs[0].path().join("pipe.wasm");
        std::fs::write(
            &path,
            r#"(module
                (func (export "_start"))
                (func (export "load"))
                (func (export "pipe") (result i32) i32.const 0)
                (func (export "render") (param i32 i32))
            )"#,
        )
        .unwrap();
        let mut run = RunPlugin {
            location: RunPluginLocation::File(path),
            ..Default::default()
        };
        if shared {
            run.configuration.insert("shared", "true");
        }

        // Hold the loader before it can create a PluginMap entry.
        test.bridge.plugin_executor.register_plugin(id);
        let (entered, started) = std::sync::mpsc::sync_channel(1);
        let (release, gate) = std::sync::mpsc::sync_channel(1);
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
        assert_eq!(
            test.bridge
                .load_plugin(
                    &Some(run),
                    None,
                    Size { rows: 5, cols: 20 },
                    None,
                    false,
                    Some(owner),
                )
                .unwrap(),
            (id, owner)
        );
        if !loading {
            release.send(()).unwrap();
            test.finish_reload(id);

            test.bridge.cache_plugin_events(id);
        }
        test.bridge.remove_client(owner);
        assert!(test.bridge.connected_clients.lock().unwrap().is_empty());
        assert_eq!(test.bridge.all_plugin_ids().is_empty(), loading);
        let plugin = test.receiver::<PluginInstruction>();
        plugin.try_iter().for_each(drop);
        let (shutdown, _) = tokio::sync::mpsc::channel(1);
        let pipe = "detached_pipe";
        // CliPipe addressed to plugin_<id> routes with a plugin ID and no client target.
        test.bridge
            .pipe_messages(
                vec![(
                    Some(id),
                    None,
                    PipeMessage::new(PipeSource::Cli(pipe.into()), "cached", &None, &None, true),
                )],
                shutdown.clone(),
                None,
            )
            .unwrap();
        assert_eq!(test.bridge.cached_events_for_pending_plugins[&id].len(), 1);
        assert!(!plugin
            .try_iter()
            .any(|(instruction, _)| matches!(instruction, PluginInstruction::PipeCompleted(_))));

        let mut acknowledgements = vec![];
        if unload {
            test.bridge.unload_plugin(id).unwrap();
            assert_eq!(test.pipe_failures(), [pipe]);
        }
        if loading {
            release.send(()).unwrap();
        }
        if !unload {
            test.settle(id);
            if !loading {
                test.bridge
                    .apply_cached_events(vec![id], false, shutdown.clone())
                    .unwrap();
                assert!(test
                    .bridge
                    .cached_events_for_pending_plugins
                    .contains_key(&id));
            }
        }
        test.bridge
            .apply_cached_events(vec![id], true, shutdown)
            .unwrap();

        if !unload {
            test.settle(id);
            let completions: Vec<_> = plugin
                .try_iter()
                .flat_map(|(instruction, _)| match instruction {
                    PluginInstruction::PipeCompleted(completion) => vec![completion],
                    _ => vec![],
                })
                .collect();
            assert_eq!(completions.len(), 1);
            assert_eq!(completions[0].plugin_id, id);
            acknowledgements.extend(test.complete(completions));
        }
        assert_eq!(acknowledgements, if unload { vec![] } else { vec![pipe] });
        // A completion arriving after replay or unload must not acknowledge again.
        let late = pipe_completion(
            id,
            PluginInstanceId::default(),
            1,
            PipeStateChange::NoChange,
        );
        assert!(test.complete(vec![late]).is_empty());
        if !unload {
            test.bridge.unload_plugin(id).unwrap();
        }
    }
}

#[test]
fn cached_cli_pipe_after_instance_removal_completes_without_unloading() {
    let mut test = TestBridge::new();
    let first = ClientId::new(7);
    let second = ClientId::new(8);
    test.bridge.add_client(first).unwrap();
    test.bridge.add_client(second).unwrap();
    let path = test._dirs[0].path().join("pipe.wasm");
    std::fs::write(
        &path,
        r#"(module
            (func (export "_start"))
            (func (export "load"))
            (func (export "pipe") (result i32) i32.const 0)
            (func (export "render") (param i32 i32))
        )"#,
    )
    .unwrap();
    let (id, _) = test
        .bridge
        .load_plugin(
            &Some(RunPlugin {
                location: RunPluginLocation::File(path),
                ..Default::default()
            }),
            None,
            Size { rows: 5, cols: 20 },
            None,
            false,
            Some(first),
        )
        .unwrap();
    // Keep the initial loader metadata and cached events until permissions arrive.
    test.bridge.cache_plugin_events(id);
    test.finish_reload(id);
    assert!(test.bridge.plugin_map.lock().unwrap().contains(id, first));
    assert!(test.bridge.plugin_map.lock().unwrap().contains(id, second));

    let first_instance = test.instance_id(id, first);
    test.bridge.remove_client(first);
    test.settle(id);
    assert_eq!(
        test.bridge
            .plugin_map
            .lock()
            .unwrap()
            .owner_client_ids_of_plugin_id(id),
        [second]
    );
    let plugin = test.receiver::<PluginInstruction>();
    let removals: Vec<_> = plugin
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::ReleasePluginInstancePipes(id, client) => Some((id, client)),
            _ => None,
        })
        .collect();
    assert_eq!(removals, [(id, first_instance)]);
    for (id, client) in removals {
        assert!(test.retire(id, client).is_empty());
    }

    let pipe = "after_instance_removal";
    let (shutdown, _) = tokio::sync::mpsc::channel(1);
    test.bridge
        .pipe_messages(
            vec![(
                Some(id),
                None,
                PipeMessage::new(PipeSource::Cli(pipe.into()), "cached", &None, &None, true),
            )],
            shutdown.clone(),
            None,
        )
        .unwrap();
    assert_eq!(test.bridge.cached_events_for_pending_plugins[&id].len(), 1);
    test.bridge
        .apply_cached_events(vec![id], true, shutdown)
        .unwrap();
    test.settle(id);
    let completions: Vec<_> = plugin
        .try_iter()
        .flat_map(|(instruction, _)| match instruction {
            PluginInstruction::PipeCompleted(completion) => vec![completion],
            _ => vec![],
        })
        .collect();
    assert_eq!(completions.len(), 1);
    assert_eq!(completions[0].instance_id, test.instance_id(id, second));
    assert_eq!(test.complete(completions.clone()), [pipe]);
    assert!(test.complete(completions).is_empty());
    assert_eq!(test.bridge.all_plugin_ids(), [(id, second)]);
    test.bridge.unload_plugin(id).unwrap();
}

#[test]
fn cached_shared_cli_pipe_after_rekey_uses_only_the_current_owner() {
    let mut test = TestBridge::new();
    let owner = ClientId::new(7);
    let new_owner = ClientId::new(8);
    test.bridge.add_client(owner).unwrap();
    let path = test._dirs[0].path().join("pipe.wasm");
    std::fs::write(
        &path,
        r#"(module
            (func (export "_start"))
            (func (export "load"))
            (func (export "pipe") (result i32) i32.const 0)
            (func (export "render") (param i32 i32))
        )"#,
    )
    .unwrap();
    let mut run = RunPlugin {
        location: RunPluginLocation::File(path),
        ..Default::default()
    };
    run.configuration.insert("shared", "true");
    let (id, _) = test
        .bridge
        .load_plugin(
            &Some(run),
            None,
            Size { rows: 5, cols: 20 },
            None,
            false,
            Some(owner),
        )
        .unwrap();
    // Initial permissions keep the loader's owner metadata and event cache alive.
    test.bridge.cache_plugin_events(id);
    test.finish_reload(id);
    let instance_id = test.instance_id(id, owner);
    let request = test.bridge.pending_pipes.begin("blocked");
    test.bridge
        .pending_pipes
        .processing(request, id, instance_id);
    let blocked = pipe_completion(id, instance_id, request, PipeStateChange::Block);
    assert!(test.complete(vec![blocked]).is_empty());
    test.bridge.add_client(new_owner).unwrap();
    test.bridge.remove_client(owner);
    test.settle(id);
    assert_eq!(
        test.bridge
            .plugin_map
            .lock()
            .unwrap()
            .owner_client_ids_of_plugin_id(id),
        [new_owner]
    );
    assert!(!test
        .receiver::<ServerInstruction>()
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, ServerInstruction::UnblockCliPipeInput(_))));
    let unblocked = pipe_completion(id, instance_id, request, PipeStateChange::Unblock);
    assert_eq!(test.complete(vec![unblocked]), ["blocked"]);
    let pipe = "rekeyed_loading_pipe";
    let (shutdown, _) = tokio::sync::mpsc::channel(1);
    test.bridge
        .pipe_messages(
            vec![(
                Some(id),
                None,
                PipeMessage::new(PipeSource::Cli(pipe.into()), "cached", &None, &None, true),
            )],
            shutdown.clone(),
            None,
        )
        .unwrap();
    assert!(matches!(
        &test.bridge.cached_events_for_pending_plugins[&id][0],
        EventOrPipeMessage::PipeMessage(None, _, _, _)
    ));
    test.bridge
        .apply_cached_events(vec![id], true, shutdown)
        .unwrap();
    test.settle(id);
    let completions: Vec<_> = test
        .receiver::<PluginInstruction>()
        .try_iter()
        .flat_map(|(instruction, _)| match instruction {
            PluginInstruction::PipeCompleted(completion) => vec![completion],
            _ => vec![],
        })
        .collect();
    assert_eq!(completions.len(), 1);
    assert_eq!(completions[0].instance_id, instance_id);
    assert_eq!(test.complete(completions.clone()), [pipe]);
    assert!(test.complete(completions).is_empty());
    test.bridge.unload_plugin(id).unwrap();
}

#[test]
fn detaching_the_last_wasm_client_keeps_processing_and_explicit_pipe_holds() {
    let mut test = TestBridge::new();
    let plugin = PluginId::new(1);
    let client = ClientId::new(1);
    test.bridge.add_client(client).unwrap();
    let context = test.context(1, 1, RECOVERABLE_WASM);
    test.load(context).unwrap();
    let instance = test.instance_id(plugin, client);
    let request = test.bridge.pending_pipes.begin("held");
    test.bridge
        .pending_pipes
        .processing(request, plugin, instance);
    assert!(test
        .complete(vec![pipe_completion(
            plugin,
            instance,
            request,
            PipeStateChange::Block
        )])
        .is_empty());
    test.bridge.remove_client(client);
    test.settle(plugin);
    assert_eq!(test.instance_id(plugin, client), instance);
    assert!(test.pipe_acknowledgements().is_empty());
    assert!(test.errors().is_empty());
    assert_eq!(
        test.complete(vec![pipe_completion(
            plugin,
            instance,
            request,
            PipeStateChange::Unblock
        )]),
        ["held"]
    );
    test.bridge.unload_plugin(plugin).unwrap();
}

#[test]
fn cached_cli_pipe_registration_preserves_client_targets() {
    for shared in [false, true] {
        let mut test = TestBridge::new();
        let id = PluginId::new(1);
        let first = ClientId::new(7);
        let second = ClientId::new(8);
        let mut context = test.context(1, 7, RECOVERABLE_WASM);
        if shared {
            context
                .plugin_config
                .initial_userspace_configuration
                .insert("shared", "true");
            test.bridge.shared_plugin_ids.insert(id);
        }
        test.load(context).unwrap();
        if !shared {
            let context = test.context(1, 8, RECOVERABLE_WASM);
            test.load(context).unwrap();
        }
        test.bridge.cache_plugin_events(id);
        let (shutdown, _) = tokio::sync::mpsc::channel(1);
        test.bridge
            .pipe_messages(
                [(first, "first"), (second, "second")]
                    .into_iter()
                    .map(|(client, pipe)| {
                        (
                            Some(id),
                            Some(client),
                            PipeMessage::new(
                                PipeSource::Cli(pipe.into()),
                                "targeted",
                                &None,
                                &None,
                                true,
                            ),
                        )
                    })
                    .collect(),
                shutdown.clone(),
                None,
            )
            .unwrap();
        test.bridge
            .apply_cached_events(vec![id], true, shutdown)
            .unwrap();
        test.settle(id);
        let completions: Vec<_> = test
            .receiver::<PluginInstruction>()
            .try_iter()
            .flat_map(|(instruction, _)| match instruction {
                PluginInstruction::PipeCompleted(completion) => vec![completion],
                _ => vec![],
            })
            .collect();
        assert_eq!(completions.len(), 2);
        for asset in &completions {
            let owner = if shared || asset.callback.as_ref().unwrap().0 == 1 {
                first
            } else {
                second
            };
            assert_eq!(asset.instance_id, test.instance_id(id, owner));
        }
        let mut acknowledgements = test.complete(completions);
        acknowledgements.sort();
        assert_eq!(acknowledgements, ["first", "second"]);
        test.bridge.unload_plugin(id).unwrap();
    }
}

fn pipe_completion(
    plugin: PluginId,
    instance: PluginInstanceId,
    request: PipeRequestId,
    change: PipeStateChange,
) -> PipeCompletion {
    PipeCompletion {
        plugin_id: plugin,
        instance_id: instance,
        changes: HashMap::from([(request, change)]),
        callback: Some((request, Ok(()))),
    }
}

#[test]
fn headless_instance_can_hold_its_pipe_until_async_output_finishes() {
    let mut test = TestBridge::new();
    let plugin = PluginId::new(1);
    let instance = PluginInstanceId::default();
    let request = test.bridge.pending_pipes.begin("pipe");
    test.bridge
        .pending_pipes
        .processing(request, plugin, instance);
    for change in [PipeStateChange::Block, PipeStateChange::NoChange] {
        assert!(test
            .complete(vec![pipe_completion(plugin, instance, request, change)])
            .is_empty());
    }
    assert_eq!(
        test.complete(vec![pipe_completion(
            plugin,
            instance,
            request,
            PipeStateChange::Unblock
        )]),
        ["pipe"]
    );
}

#[test]
fn removing_an_instance_fails_only_its_pipe_holds_and_ignores_late_completions() {
    for source in [RECOVERABLE_WASM, "export default {}"] {
        let mut test = TestBridge::new();
        let plugin = PluginId::new(1);
        let first = ClientId::new(1);
        let second = ClientId::new(2);
        *test.bridge.connected_clients.lock().unwrap() = vec![first, second];
        for client in [1, 2] {
            let context = test.context(1, client, source);
            test.load(context).unwrap();
        }
        let old = test.instance_id(plugin, first);
        let peer = test.instance_id(plugin, second);
        let held = test.bridge.pending_pipes.begin("held");
        let fanout = test.bridge.pending_pipes.begin("fanout");
        let healthy = test.bridge.pending_pipes.begin("healthy");
        for (request, instance) in [(held, old), (fanout, old), (fanout, peer), (healthy, peer)] {
            test.bridge
                .pending_pipes
                .processing(request, plugin, instance);
        }
        for (request, instance) in [(held, old), (fanout, old), (fanout, peer), (healthy, peer)] {
            assert!(test
                .complete(vec![pipe_completion(
                    plugin,
                    instance,
                    request,
                    PipeStateChange::Block
                )])
                .is_empty());
        }
        test.bridge.remove_client(first);
        test.settle(plugin);
        assert!(!test
            .bridge
            .plugin_map
            .lock()
            .unwrap()
            .contains(plugin, first));
        assert!(test
            .bridge
            .plugin_map
            .lock()
            .unwrap()
            .contains(plugin, second));
        assert_eq!(test.pipe_failures(), ["held"]);
        assert!(test
            .complete(vec![pipe_completion(
                plugin,
                peer,
                fanout,
                PipeStateChange::Unblock
            )])
            .is_empty());
        assert_eq!(test.errors(), ["fanout"]);
        assert_eq!(
            test.complete(vec![pipe_completion(
                plugin,
                peer,
                healthy,
                PipeStateChange::Unblock
            )]),
            ["healthy"]
        );
        assert!(test.retire(plugin, old).is_empty());
        assert!(test
            .complete(vec![pipe_completion(
                plugin,
                old,
                held,
                PipeStateChange::Unblock
            )])
            .is_empty());
        test.bridge.unload_plugin(plugin).unwrap();
    }
}

#[test]
fn deno_host_folder_changes_report_only_the_active_parent_cwd() {
    let mut test = TestBridge::new();
    let id = PluginId::new(1);
    let client = ClientId::new(1);
    let source = r#"
let replies = 0, fences = 0;
export default {
  workers: {probe() {zellij.host.setTimeout(0);}},
  load() {
    zellij.subscribe(['HostFolderChanged', 'FailedToChangeHostFolder', 'CustomMessage']);
    zellij.postMessageTo('probe', 'start');
  },
  update(event) {
    if (event.HostFolderChanged || event.FailedToChangeHostFolder) replies++;
    if (event.CustomMessage) fences++;
    return true;
  },
  render() {return `replies=${replies} fences=${fences}`;}
};
"#;
    let context = test.context(1, 1, source);
    test.load(context).unwrap();
    let instructions = test.receiver::<PluginInstruction>();
    let pty = test.receiver::<zellij_bus::pty::PtyInstruction>();
    let worker_generation = loop {
        let (instruction, _) = instructions
            .recv_timeout(zellij_test_support::default_timeout())
            .unwrap();
        if let PluginInstruction::DenoEvent(_, _, generation, Event::Timer(_)) = instruction {
            break generation;
        }
    };
    let parent_generation = test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .get_running_plugin(id, Some(client))
        .unwrap()
        .lock()
        .unwrap()
        .deno_generation()
        .unwrap();
    let original_cwd = test.bridge.get_plugin_cwd(id, client).unwrap();
    let moved = tempdir().unwrap();
    instructions.try_iter().for_each(drop);
    pty.try_iter().for_each(drop);
    let mut replies = vec![];
    for generation in [worker_generation, parent_generation] {
        test.bridge
            .change_plugin_host_dir(moved.path().to_path_buf(), id, client, Some(generation))
            .unwrap();
        test.settle(id);
        let reply = instructions
            .try_iter()
            .find_map(|(instruction, _)| match instruction {
                PluginInstruction::DenoEvent(
                    _,
                    _,
                    reply_generation,
                    event @ Event::HostFolderChanged(_),
                ) => {
                    assert_eq!(reply_generation, generation);
                    Some(event)
                },
                _ => None,
            })
            .expect("host folder change must produce a scoped reply");
        replies.push((generation, reply));
        let reports: Vec<_> = pty
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                zellij_bus::pty::PtyInstruction::ReportPluginCwd(plugin, path) => {
                    Some((plugin, path))
                },
                _ => None,
            })
            .collect();
        if generation == worker_generation {
            assert!(
                reports.is_empty(),
                "worker must not report the parent pane's cwd"
            );
            assert_eq!(
                test.bridge.get_plugin_cwd(id, client),
                Some(original_cwd.clone())
            );
        } else {
            assert_eq!(reports, vec![(id, moved.path().to_path_buf())]);
            assert_eq!(
                test.bridge.get_plugin_cwd(id, client),
                Some(moved.path().to_path_buf())
            );
        }
    }
    // Hold the executor so invalidation happens after submission, before validation or mutation.
    let (release, blocked) = std::sync::mpsc::channel();
    test.bridge
        .plugin_executor
        .execute_for_plugin(id, move |_, _, _, _, _| {
            blocked
                .recv_timeout(zellij_test_support::default_timeout())
                .unwrap();
        });
    for generation in [worker_generation, parent_generation] {
        test.bridge
            .change_plugin_host_dir(original_cwd.clone(), id, client, Some(generation))
            .unwrap();
    }
    test.bridge
        .plugin_map
        .lock()
        .unwrap()
        .invalidate_deno_generations(id);
    release.send(()).unwrap();
    test.settle(id);
    assert_eq!(
        test.bridge.get_plugin_cwd(id, client),
        Some(moved.path().to_path_buf())
    );
    assert!(instructions.try_iter().next().is_none());
    assert!(pty.try_iter().next().is_none());

    let context = test.context(1, 1, source);
    test.load(context).unwrap();
    let replacement_cwd = test.bridge.get_plugin_cwd(id, client).unwrap();
    let (shutdown, _) = tokio::sync::mpsc::channel(1);
    for (generation, reply) in replies {
        test.bridge
            .change_plugin_host_dir(moved.path().to_path_buf(), id, client, Some(generation))
            .unwrap();
        test.bridge
            .deno_event(id, generation, vec![(client, reply)], shutdown.clone())
            .unwrap();
    }
    test.bridge
        .update_plugins(
            vec![(
                Some(id),
                Some(client),
                Event::CustomMessage("fence".into(), String::new()),
            )],
            shutdown,
        )
        .unwrap();
    test.settle(id);
    assert_eq!(
        test.bridge.get_plugin_cwd(id, client),
        Some(replacement_cwd)
    );
    assert!(pty.try_iter().next().is_none());
    assert!(!instructions.try_iter().any(|(instruction, _)| matches!(
        instruction,
        PluginInstruction::DenoEvent(
            _,
            _,
            _,
            Event::HostFolderChanged(_) | Event::FailedToChangeHostFolder(_)
        )
    )));
    let frames: Vec<_> = test
        .receiver::<ScreenInstruction>()
        .try_iter()
        .flat_map(|(instruction, _)| match instruction {
            ScreenInstruction::PluginBytes(assets) => assets,
            _ => vec![],
        })
        .collect();
    assert!(frames
        .iter()
        .any(|asset| String::from_utf8_lossy(&asset.bytes).contains("replies=0 fences=1")));
    test.bridge.unload_plugin(id).unwrap();
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
fn deno_pane_events_reach_cli_subscribers_once_even_after_reload() {
    let mut test = TestBridge::new();
    let client = ClientId::new(1);
    let all_events = ClientId::new(7);
    let exits_only = ClientId::new(8);
    let unrelated = ClientId::new(9);
    test.bridge.add_client(client).unwrap();
    let path = test._dirs[0].path().join("cli-events.ts");
    std::fs::write(&path, "export default {};").unwrap();
    let run = RunPlugin {
        location: RunPluginLocation::File(path),
        ..Default::default()
    };
    let (plugin, _) = test
        .bridge
        .load_plugin(
            &Some(run),
            None,
            Size { rows: 5, cols: 20 },
            None,
            false,
            Some(client),
        )
        .unwrap();
    test.wait_for_executor(plugin);
    let generation = test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .get_running_plugin(plugin, Some(client))
        .unwrap()
        .lock()
        .unwrap()
        .deno_generation()
        .unwrap();
    test.bridge
        .subscribe_cli_client_to_events(all_events, vec![]);
    test.bridge.subscribe_cli_client_to_events(
        exits_only,
        vec!["CommandPaneExited".into(), "EditPaneExited".into()],
    );
    test.bridge
        .subscribe_cli_client_to_events(unrelated, vec!["CwdChanged".into()]);
    let terminal = zellij_types::internal_ids::TerminalId::new(3);
    let context = BTreeMap::from([("request".into(), "pane".into())]);
    let events = [
        Event::CommandPaneOpened(terminal, context.clone()),
        Event::CommandPaneReRun(terminal, context.clone()),
        Event::CommandPaneExited(terminal, Some(0), context.clone()),
        Event::EditPaneOpened(terminal, context.clone()),
        Event::EditPaneExited(terminal, Some(1), context),
        Event::ListClients(vec![]),
        Event::CustomMessage("private".into(), "reply".into()),
        Event::InputReceived,
    ];
    let (shutdown, _) = tokio::sync::mpsc::channel(1);
    // Forward while the load is pending, then replay the cache without duplicating CLI events.
    assert!(test
        .bridge
        .cached_events_for_pending_plugins
        .contains_key(&plugin));
    test.bridge
        .deno_event(
            plugin,
            generation,
            events.iter().cloned().map(|e| (client, e)).collect(),
            shutdown.clone(),
        )
        .unwrap();
    test.finish_reload(plugin);
    test.bridge.reload_plugin_with_id(plugin).unwrap();
    test.finish_reload(plugin);
    assert!(!test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .contains_deno_generation(plugin, generation));
    // These lifecycle events still belong on the CLI stream even though their owner is gone.
    test.bridge
        .deno_event(
            plugin,
            generation,
            events.iter().cloned().map(|e| (client, e)).collect(),
            shutdown,
        )
        .unwrap();
    test.wait_for_executor(plugin);
    let forwarded: Vec<_> = test
        .receiver::<ServerInstruction>()
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            ServerInstruction::EventUpdateToClient(client, kind, json) => {
                Some((client, kind, json))
            },
            _ => None,
        })
        .collect();
    for (subscriber, expected) in [
        (all_events, events[..5].iter().collect::<Vec<_>>()),
        (exits_only, vec![&events[2], &events[4]]),
        (unrelated, vec![]),
    ] {
        let expected: Vec<_> = expected
            .iter()
            .map(|event| {
                (
                    EventType::from(*event).to_string(),
                    serde_json::to_string(event).unwrap(),
                )
            })
            .collect();
        let received: Vec<_> = forwarded
            .iter()
            .filter(|(client, _, _)| *client == subscriber)
            .map(|(_, kind, json)| (kind.clone(), json.clone()))
            .collect();
        assert_eq!(received, [expected.clone(), expected].concat());
    }
    let scratch = test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .get_running_plugin(plugin, Some(client))
        .unwrap()
        .lock()
        .unwrap()
        .env()
        .plugin_own_data_dir
        .clone();
    let changes = FileEvents::new(scratch.parent().unwrap());
    test.bridge.unload_plugin(plugin).unwrap();
    changes.wait("plugin scratch cleanup", || !scratch.exists());
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
    cli_pipe_after_queued_rekey("render", false);
}

#[test]
fn deno_cli_pipe_without_render_releases_its_registration_owner_after_a_queued_rekey() {
    cli_pipe_after_queued_rekey("no_render", false);
}

#[test]
fn deno_cli_pipe_error_releases_its_registration_owner_after_a_queued_rekey() {
    cli_pipe_after_queued_rekey("error", false);
}

#[test]
fn cached_deno_cli_pipe_render_releases_its_registration_owner_after_a_queued_rekey() {
    cli_pipe_after_queued_rekey("render", true);
}

#[test]
fn cached_deno_cli_pipe_without_render_releases_its_registration_owner_after_a_queued_rekey() {
    cli_pipe_after_queued_rekey("no_render", true);
}

#[test]
fn cached_deno_cli_pipe_error_releases_its_registration_owner_after_a_queued_rekey() {
    cli_pipe_after_queued_rekey("error", true);
}

fn cli_pipe_after_queued_rekey(outcome: &str, cached: bool) {
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
    let (entered, started) = std::sync::mpsc::sync_channel(1);
    let (release, gate) = std::sync::mpsc::sync_channel(1);
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
    if cached {
        test.bridge.cache_plugin_events(id);
    }
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
            shutdown.clone(),
            None,
        )
        .unwrap();
    if cached {
        assert_eq!(test.bridge.cached_events_for_pending_plugins[&id].len(), 1);
    }
    let instance_id = test.instance_id(id, owner);
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
    if cached {
        test.bridge
            .apply_cached_events(vec![id], true, shutdown)
            .unwrap();
        test.settle(id);
    }
    let mut completions = vec![];
    let mut errors = vec![];
    for (instruction, _) in plugin.try_iter() {
        match instruction {
            PluginInstruction::PipeCompleted(completion) => completions.push(completion),
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
        assert_eq!(
            completions.len(),
            1,
            "completion must precede screen processing"
        );
        frames.sort_by_key(|asset| asset.client_id);
        assert_eq!(frames.len(), 2);
        for (frame, client) in frames.iter().zip([new_owner, acting_client]) {
            assert_eq!(frame.client_id, client);
            assert!(
                String::from_utf8_lossy(&frame.bytes).contains(&format!("rendered for {client}"))
            );
        }
    } else {
        assert!(frames.is_empty());
    }
    assert_eq!(completions.len(), 1);
    assert_eq!(completions[0].instance_id, instance_id);
    let result = test.complete(completions.clone());
    if outcome == "error" {
        assert!(result.is_empty());
        assert_eq!(test.errors(), [pipe_id]);
    } else {
        assert_eq!(result, [pipe_id]);
    }
    assert!(test.complete(completions).is_empty());
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
        None,
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
            instance_id,
            error,
            failed_to_load,
            ..
        },
        _,
    ) = plugin.try_recv().unwrap()
    else {
        panic!("expected the failure report");
    };
    assert_eq!(deno_generation, Some(41));
    test.bridge
        .queue_plugin_failed(
            plugin_id,
            None,
            deno_generation,
            instance_id,
            error,
            failed_to_load,
        )
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
fn a_disconnected_deno_failure_neither_paints_an_error_nor_finishes_a_job() {
    let mut test = TestBridge::new();
    let plugin = PluginId::new(1);
    let departing = ClientId::new(1);
    let remaining = ClientId::new(2);
    *test.bridge.connected_clients.lock().unwrap() = vec![departing, remaining];
    for client in [1, 2] {
        let context = test.context(1, client, "export default {}");
        test.load(context).unwrap();
    }
    let generation = test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .get_running_plugin(plugin, Some(departing))
        .unwrap()
        .lock()
        .unwrap()
        .deno_generation()
        .unwrap();
    let instance = test.instance_id(plugin, departing);
    launch_job(&mut test.bridge, "merge", plugin);
    let screen = test.receiver::<ScreenInstruction>();
    screen.try_iter().for_each(drop);

    // Keep the cancelled attachment in the map until its queued failure is handled.
    let release = test.hold_executor(plugin);
    test.bridge.remove_client(departing);
    test.bridge
        .queue_plugin_failed(
            plugin,
            None,
            Some(generation),
            Some(instance),
            "departed instance failed".into(),
            false,
        )
        .unwrap();
    let painted = screen.try_recv().is_ok();
    release.send(()).unwrap();
    test.settle(plugin);

    assert!(
        !painted,
        "cancelled failures must not paint or close the pane"
    );
    assert!(test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .contains(plugin, remaining));
    assert_eq!(
        test.bridge
            .queues
            .lock()
            .unwrap()
            .queue_snapshot("merge")
            .unwrap()
            .running
            .len(),
        1,
        "cancelled failures must not finish the current job"
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
            None,
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
        .queue_plugin_failed(
            crate::PluginId::new(7),
            None,
            None,
            None,
            "boom".to_owned(),
            false,
        )
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
        // Live values must win over the original insertion snapshot - the size included,
        // asked and not yet drawn.
        running.resize.request(13, 37);
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
fn deferred_url_reload_recovers_after_load_failure() {
    for good in [RECOVERABLE_WASM, "export default {}"] {
        let mut test = TestBridge::new();
        let id = PluginId::new(1);
        let context = test.context(1, 1, good);
        let source = context.plugin_config.path.clone();
        test.load(context).unwrap();
        let run = test.bridge.run_plugin_of_plugin_id(id).unwrap();
        let bad = if good == RECOVERABLE_WASM {
            good.replace("(export \"load\")", "(export \"load\") unreachable")
        } else {
            "export default { load() { throw Error('load failed'); } }".to_owned()
        };
        std::fs::write(&source, bad).unwrap();
        test.bridge.reload_plugin_with_id(id).unwrap();
        test.wait_for_executor(id);
        let old_load = test.bridge.load_ids[&id];
        let instructions: Vec<_> = test.receiver::<PluginInstruction>().try_iter().collect();

        // The loader has failed, but its notification has not cleared the loading association.
        test.bridge.reload_plugin(&run).unwrap();
        assert!(test.bridge.pending_plugin_reloads.contains(&run));
        assert_eq!(test.bridge.load_ids[&id], old_load);
        assert!(test
            .bridge
            .plugin_map
            .lock()
            .unwrap()
            .get_running_plugin(id, None)
            .is_none());
        std::fs::write(&source, good).unwrap();
        let release = test.hold_executor(id);
        let mut failure_received = false;
        let mut trailing_events_received = false;
        let (shutdown, _) = tokio::sync::mpsc::channel(1);
        for (instruction, _) in instructions {
            match instruction {
                PluginInstruction::PluginFailed {
                    plugin_id,
                    load_id,
                    deno_generation,
                    instance_id,
                    error,
                    failed_to_load,
                    ..
                } => {
                    assert_eq!(plugin_id, id);
                    assert_eq!(load_id, Some(old_load));
                    assert!(failed_to_load);
                    test.bridge
                        .queue_plugin_failed(
                            plugin_id,
                            load_id,
                            deno_generation,
                            instance_id,
                            error,
                            failed_to_load,
                        )
                        .unwrap();
                    failure_received = true;
                    assert_ne!(test.bridge.load_ids[&id], old_load);
                    assert!(!test.bridge.pending_plugin_reloads.contains(&run));
                    test.pipe(id, "replacement");
                },
                PluginInstruction::ApplyCachedEvents {
                    load_id,
                    plugin_ids,
                    done_receiving_permissions,
                } => {
                    assert!(failure_received);
                    assert_eq!(load_id, old_load);
                    trailing_events_received = true;
                    test.bridge
                        .apply_cached_events_for_load(
                            load_id,
                            plugin_ids,
                            done_receiving_permissions,
                            shutdown.clone(),
                        )
                        .unwrap();
                    assert_eq!(test.bridge.run_plugin_of_loading_plugin_id(id), Some(&run));
                    assert_eq!(test.bridge.cached_events_for_pending_plugins[&id].len(), 1);
                    assert!(test
                        .bridge
                        .cached_resizes_for_pending_plugins
                        .contains_key(&id));
                    assert!(test.pipe_acknowledgements().is_empty());
                },
                _ => {},
            }
        }
        assert!(failure_received);
        assert!(trailing_events_received);
        release.send(()).unwrap();
        test.finish_reload(id);
        assert!(test
            .bridge
            .plugin_map
            .lock()
            .unwrap()
            .get_running_plugin(id, None)
            .is_some());
        assert!(test.bridge.loading_plugins.is_empty());
        assert_eq!(test.pipe_acknowledgements(), ["replacement"]);
        test.bridge.unload_plugin(id).unwrap();
    }
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
        // Asked and not yet drawn, as a resize is while it waits behind a busy render: the
        // drawn size is still the old one.
        running.resize.request(17, 42);
        assert_ne!((running.rows, running.columns), (17, 42));
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

#[test]
fn failed_wasm_and_deno_reject_repeated_id_url_and_alias_pipes_until_explicit_reload() {
    for (bad, good) in [
        (
            r#"(module (func (export "_start")) (func (export "load")) (func (export "pipe") (result i32) unreachable) (func (export "render") (param i32 i32)))"#,
            RECOVERABLE_WASM,
        ),
        (
            "export default { pipe() { throw Error('failed pipe'); } };",
            "export default { pipe() { return false; } };",
        ),
    ] {
        for failed_load in [false, true] {
            let bad = if failed_load {
                if good == RECOVERABLE_WASM {
                    good.replace("(export \"load\")", "(export \"load\") unreachable")
                } else {
                    "export default { load() { throw Error('failed load'); } };".to_owned()
                }
            } else {
                bad.to_owned()
            };
            let mut test = TestBridge::new();
            let plugin = PluginId::new(1);
            let client = ClientId::new(1);
            test.bridge.add_client(client).unwrap();
            let context = test.context(1, 1, &bad);
            let path = context.plugin_config.path.clone();
            let loaded = test.load(context);
            if failed_load {
                assert!(loaded.is_err());
            } else {
                loaded.unwrap();
            }
            let run = test.bridge.run_plugin_of_plugin_id(plugin).unwrap();
            let aliases =
                PluginAliases::from_data(BTreeMap::from([("recipient".into(), run.clone())]));
            test.pipe(plugin, "first");
            test.settle(plugin);
            assert_eq!(test.pipe_failures(), ["first"]);
            std::fs::write(&path, good).unwrap();
            test.bridge.cache_plugin_events(plugin);
            let next_plugin = test.bridge.next_plugin_id;
            for target in [
                format!("plugin_{plugin}"),
                format!("file:{}", path.display()),
                "recipient".into(),
            ] {
                for _ in 0..2 {
                    let recipients = if target.starts_with("plugin_") {
                        vec![(plugin, None)]
                    } else {
                        let run = zellij_runtime::plugin_location::run_plugin_or_alias_from_url(
                            &target,
                            &None,
                            Some(&aliases),
                            None,
                        )
                        .unwrap();
                        test.bridge
                            .get_or_load_plugins(
                                run,
                                Size::default(),
                                None,
                                false,
                                false,
                                false,
                                None,
                                None,
                                Some(client),
                                None,
                                false,
                                true,
                            )
                            .unwrap()
                    };
                    assert_eq!(recipients.len(), 1, "{target}");
                    let (shutdown, _) = tokio::sync::mpsc::channel(1);
                    test.bridge
                        .pipe_messages(
                            recipients
                                .into_iter()
                                .map(|(id, owner)| {
                                    (
                                        Some(id),
                                        owner,
                                        PipeMessage::new(
                                            PipeSource::Cli("retry".into()),
                                            "retry",
                                            &None,
                                            &None,
                                            true,
                                        ),
                                    )
                                })
                                .collect(),
                            shutdown,
                            None,
                        )
                        .unwrap();
                    test.settle(plugin);
                    assert!(
                        test.receiver::<PluginInstruction>().try_iter().all(
                            |(instruction, _)| !matches!(
                                instruction,
                                PluginInstruction::PipeCompleted(_)
                            )
                        ),
                        "unavailable recipients must not be invoked: {target}"
                    );
                    assert_eq!(test.pipe_failures(), ["retry"]);
                    assert_eq!(
                        test.bridge.next_plugin_id, next_plugin,
                        "a failed destination must not lazily reload"
                    );
                }
            }
            test.bridge.reload_plugin_with_id(plugin).unwrap();
            test.finish_reload(plugin);
            test.pipe(plugin, "recovered");
            test.settle(plugin);
            assert_eq!(test.pipe_acknowledgements(), ["recovered"]);
            test.bridge.unload_plugin(plugin).unwrap();
        }
    }
}

#[test]
fn runtime_failure_while_waiting_for_permissions_rejects_cached_and_new_pipes() {
    for source in [RECOVERABLE_WASM, "export default {}"] {
        for fail_before_load_report in [false, true] {
            let mut test = TestBridge::new();
            let plugin = PluginId::new(1);
            let context = test.context(1, 1, source);
            test.load(context).unwrap();
            let instance = test.instance_id(plugin, ClientId::new(1));
            let run = test.bridge.run_plugin_of_plugin_id(plugin).unwrap();
            test.bridge.loading_plugins.insert((plugin, run));
            test.bridge.cache_plugin_events(plugin);
            test.pipe(plugin, "cached");
            let (shutdown, _) = tokio::sync::mpsc::channel(1);
            if !fail_before_load_report {
                test.bridge
                    .apply_cached_events(vec![plugin], false, shutdown.clone())
                    .unwrap();
            }
            test.bridge
                .queue_plugin_failed(
                    plugin,
                    None,
                    None,
                    Some(instance),
                    "runtime failed while awaiting permissions".into(),
                    false,
                )
                .unwrap();
            if fail_before_load_report {
                test.bridge
                    .apply_cached_events(vec![plugin], false, shutdown)
                    .unwrap();
            }
            test.settle(plugin);
            assert_eq!(test.pipe_failures(), ["cached"]);
            test.pipe(plugin, "retry");
            assert_eq!(test.pipe_failures(), ["retry"]);
            test.bridge.unload_plugin(plugin).unwrap();
            assert!(test.pipe_outcomes().is_empty());
        }
    }
}

#[test]
fn executor_rejection_fails_delivery_and_retirement_once() {
    for (cached, reload) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut test = TestBridge::new();
        let plugin = PluginId::new(1);
        let context = test.context(1, 1, RECOVERABLE_WASM);
        test.load(context).unwrap();
        if cached {
            test.bridge.cache_plugin_events(plugin);
        }
        test.bridge.plugin_executor.unregister_plugin(plugin);
        test.pipe(plugin, "rejected");
        if cached {
            assert!(test.pipe_outcomes().is_empty());
            let (shutdown, _) = tokio::sync::mpsc::channel(1);
            test.bridge
                .apply_cached_events(vec![plugin], true, shutdown)
                .unwrap();
        }
        let outcomes = test.pipe_outcomes();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].0, "rejected");
        assert!(outcomes[0]
            .1
            .as_ref()
            .unwrap_err()
            .contains("executor rejected"));
        assert!(test.bridge.pending_pipes.take_ready().is_empty());
        let instance = test.instance_id(plugin, ClientId::new(1));
        let request = test.bridge.pending_pipes.begin("retirement");
        test.bridge
            .pending_pipes
            .processing(request, plugin, instance);
        assert!(test
            .complete(vec![pipe_completion(
                plugin,
                instance,
                request,
                PipeStateChange::Block
            )])
            .is_empty());
        if reload {
            test.bridge.reload_plugin_with_id(plugin).unwrap();
        } else {
            test.bridge.unload_plugin(plugin).unwrap();
        }
        assert_eq!(test.pipe_failures(), ["retirement"]);
        test.bridge.unload_plugin(plugin).unwrap();
        assert!(test.pipe_outcomes().is_empty());
    }
}

#[test]
fn callback_and_render_output_precede_success_without_processing_screen_messages() {
    let mut test = TestBridge::new();
    let plugin = PluginId::new(1);
    let context = test.context(1, 1, r#"
let stream;
export default {
  pipe(message) { stream = message.source.Cli; zellij.command('CliPipeOutput', [stream, 'callback']); return true; },
  render() { if (stream) zellij.command('CliPipeOutput', [stream, 'render']); return 'frame'; }
};"#);
    test.load(context).unwrap();
    let instance = test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .get_running_plugin(plugin, None)
        .unwrap();
    instance
        .lock()
        .unwrap()
        .env_mut()
        .set_permissions(HashSet::from([PermissionType::ReadCliPipes]));
    test.pipe(plugin, "ordered");
    test.settle(plugin);
    for (instruction, _) in test.receiver::<PluginInstruction>().try_iter() {
        if let PluginInstruction::PipeCompleted(completion) = instruction {
            let outcomes = test.bridge.update_cli_pipe_state(vec![completion]);
            test.bridge.send_pipe_outcomes(outcomes);
        }
    }
    let observed: Vec<_> = test
        .receiver::<ServerInstruction>()
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            ServerInstruction::CliPipeOutput(pipe, output) => {
                assert_eq!(pipe, "ordered");
                Some(output)
            },
            ServerInstruction::UnblockCliPipeInput(pipe) => {
                assert_eq!(pipe, "ordered");
                Some("success".into())
            },
            ServerInstruction::CliPipeError(_, error) => panic!("{error}"),
            _ => None,
        })
        .collect();
    assert_eq!(observed, ["callback", "render", "success"]);
    assert!(test
        .receiver::<ScreenInstruction>()
        .try_iter()
        .any(|(instruction, _)| matches!(instruction, ScreenInstruction::PluginBytes(_))));
    test.bridge.unload_plugin(plugin).unwrap();
}

#[test]
fn failed_replacement_retires_old_wasm_and_deno_holds() {
    for source in [RECOVERABLE_WASM, "export default {}"] {
        let mut test = TestBridge::new();
        let plugin = PluginId::new(1);
        let client = ClientId::new(1);
        let context = test.context(1, 1, source);
        let path = context.plugin_config.path.clone();
        test.load(context).unwrap();
        let instance = test.instance_id(plugin, client);
        let request = test.bridge.pending_pipes.begin("held");
        test.bridge
            .pending_pipes
            .processing(request, plugin, instance);
        assert!(test
            .complete(vec![pipe_completion(
                plugin,
                instance,
                request,
                PipeStateChange::Block
            )])
            .is_empty());
        std::fs::write(path, "invalid source").unwrap();
        test.bridge.reload_plugin_with_id(plugin).unwrap();
        test.settle(plugin);
        assert_eq!(test.pipe_failures(), ["held"]);
        assert!(test.pipe_outcomes().is_empty());
        test.bridge.unload_plugin(plugin).unwrap();
    }
}

#[test]
fn runtime_failure_keeps_the_healthy_peer_and_stale_failures_cannot_retire_a_reload() {
    for (bad, good) in [
        (
            r#"(module (func (export "_start")) (func (export "load")) (func (export "pipe") (result i32) unreachable) (func (export "render") (param i32 i32)))"#,
            RECOVERABLE_WASM,
        ),
        (
            "export default { pipe() { throw Error('failed pipe'); } };",
            "export default { pipe() { return false; } };",
        ),
    ] {
        let mut test = TestBridge::new();
        let plugin = PluginId::new(1);
        let first = ClientId::new(1);
        let peer = ClientId::new(2);
        test.bridge.add_client(first).unwrap();
        test.bridge.add_client(peer).unwrap();
        let context = test.context(1, 1, bad);
        test.load(context).unwrap();
        let old = test.instance_id(plugin, first);
        let mut context = test.context(1, 2, good);
        let peer_path = context
            .plugin_config
            .path
            .with_file_name(if good == RECOVERABLE_WASM {
                "peer.wasm"
            } else {
                "peer.js"
            });
        std::fs::write(&peer_path, good).unwrap();
        context.plugin_config.path = peer_path.clone();
        context.plugin_config.location = RunPluginLocation::File(peer_path);
        test.load(context).unwrap();
        let healthy = test.instance_id(plugin, peer);
        test.pipe(plugin, "fanout");
        test.settle(plugin);
        assert_eq!(test.pipe_failures(), ["fanout"]);
        test.bridge
            .queue_plugin_failed(
                plugin,
                None,
                None,
                Some(old),
                "failed instance".into(),
                false,
            )
            .unwrap();
        test.settle(plugin);
        test.pipe(plugin, "healthy-peer");
        test.settle(plugin);
        assert_eq!(test.pipe_acknowledgements(), ["healthy-peer"]);
        assert_eq!(test.instance_id(plugin, peer), healthy);
        test.bridge.reload_plugin_with_id(plugin).unwrap();
        test.finish_reload(plugin);
        test.bridge
            .queue_plugin_failed(plugin, None, None, Some(old), "stale failure".into(), false)
            .unwrap();
        test.pipe(plugin, "replacement");
        test.settle(plugin);
        assert_eq!(test.pipe_acknowledgements(), ["replacement"]);
        test.bridge.unload_plugin(plugin).unwrap();
    }
}

/// What each `PluginBytes` the bridge sent for `plugin` drew, in order.
fn frames(test: &mut TestBridge, plugin: PluginId) -> Vec<String> {
    frames_by_client(test, plugin)
        .into_iter()
        .map(|(_, drawn)| drawn)
        .collect()
}

/// [`frames`], with the client each one was drawn for.
fn frames_by_client(test: &mut TestBridge, plugin: PluginId) -> Vec<(ClientId, String)> {
    test.receiver::<ScreenInstruction>()
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            ScreenInstruction::PluginBytes(assets) => Some(assets),
            _ => None,
        })
        .flatten()
        .filter(|asset| asset.plugin_id == plugin)
        .filter_map(|asset| {
            let drawn = String::from_utf8_lossy(&asset.bytes).into_owned();
            let start = drawn.find("size ")?;
            let end = start + drawn[start..].find(';')?;
            Some((asset.client_id, drawn[start..end].to_owned()))
        })
        .collect()
}

fn a_plugin_that_draws_its_size(test: &mut TestBridge) -> PluginId {
    let plugin = PluginId::new(1);
    let context = test.context(
        1,
        1,
        "export default { pipe() { return true; }, render(rows: number, columns: number) { return `size ${rows}x${columns};`; } };",
    );
    test.load(context).unwrap();
    test.settle(plugin);
    let _ = frames(test, plugin);
    plugin
}

/// Resizes queued behind a busy plugin are drawn once, at the newest size, rather than replayed
/// one by one after the plugin frees up - which is what a pane-edge drag over a slow plugin
/// used to cost: seconds of stale frames ahead of the next key.
///
/// Counting frames is deliberate here and cannot vary: the executor is held while all four
/// resizes are queued, so every one of them is issued before any of them runs.
#[test]
fn resizes_queued_behind_a_busy_plugin_draw_only_the_newest_size() {
    let mut test = TestBridge::new();
    let plugin = a_plugin_that_draws_its_size(&mut test);
    let (shutdown, _shutdown_receiver) = tokio::sync::mpsc::channel(1);

    let release = test.hold_executor(plugin);
    for columns in [30, 31, 32, 33] {
        test.bridge
            .resize_plugin(plugin, columns, 10, shutdown.clone())
            .unwrap();
    }
    release.send(()).unwrap();
    test.settle(plugin);
    assert_eq!(frames(&mut test, plugin), ["size 10x33"]);

    // A drag that ends where it began still draws: the screen resized the pane to the size in
    // between, whose render was skipped.
    let release = test.hold_executor(plugin);
    for columns in [40, 33] {
        test.bridge
            .resize_plugin(plugin, columns, 10, shutdown.clone())
            .unwrap();
    }
    release.send(()).unwrap();
    test.settle(plugin);
    assert_eq!(frames(&mut test, plugin), ["size 10x33"]);

    // And a resize to the size it already has, with nothing skipped, still draws nothing.
    test.bridge
        .resize_plugin(plugin, 33, 10, shutdown.clone())
        .unwrap();
    test.settle(plugin);
    assert!(frames(&mut test, plugin).is_empty());
    test.bridge.unload_plugin(plugin).unwrap();
}

/// A shared instance rekeyed to another client when its owner leaves keeps its record of the
/// last resize it ran, so a drag that ends where it began still draws. Keyed by client, the
/// record stayed behind with the departed owner, and the rekeyed instance took the final resize
/// as its first: same size, nothing skipped, no frame.
///
/// The one frame counted cannot vary: the setup's frames are drained first, and the executor is
/// held while both resizes are queued, so both are issued before either runs.
#[test]
fn a_rekeyed_shared_plugin_still_draws_a_drag_that_ends_where_it_began() {
    let mut test = TestBridge::new();
    let owner = ClientId::new(1);
    let new_owner = ClientId::new(2);
    let plugin = a_shared_plugin_that_draws_its_size(&mut test, owner, &[owner, new_owner]);
    let (shutdown, _shutdown_receiver) = tokio::sync::mpsc::channel(1);
    test.bridge
        .resize_plugin(plugin, 30, 10, shutdown.clone())
        .unwrap();
    test.settle(plugin);
    test.bridge.remove_client(owner);
    test.settle(plugin);
    assert_eq!(
        test.bridge
            .plugin_map
            .lock()
            .unwrap()
            .owner_client_ids_of_plugin_id(plugin),
        [new_owner]
    );
    let _ = frames(&mut test, plugin);

    let release = test.hold_executor(plugin);
    for columns in [40, 30] {
        test.bridge
            .resize_plugin(plugin, columns, 10, shutdown.clone())
            .unwrap();
    }
    release.send(()).unwrap();
    test.settle(plugin);
    assert_eq!(frames(&mut test, plugin), ["size 10x30"]);
    test.bridge.unload_plugin(plugin).unwrap();
}

/// A shared plugin that draws its size, owned by `owner`, with every one of `clients` attached.
fn a_shared_plugin_that_draws_its_size(
    test: &mut TestBridge,
    owner: ClientId,
    clients: &[ClientId],
) -> PluginId {
    for client in clients {
        test.bridge.add_client(*client).unwrap();
    }
    let path = test._dirs[0].path().join("shared.ts");
    std::fs::write(
        &path,
        "export default { render(rows: number, columns: number) { return `size ${rows}x${columns};`; } };",
    )
    .unwrap();
    let mut run =
        zellij_runtime::plugin_location::run_plugin_from_url(&format!("file:{}", path.display()))
            .unwrap();
    run.configuration.insert("shared", "true");
    let (plugin, _) = test
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
    test.settle(plugin);
    let (shutdown, _shutdown_receiver) = tokio::sync::mpsc::channel(1);
    test.bridge
        .apply_cached_events(vec![plugin], false, shutdown)
        .unwrap();
    test.settle(plugin);
    plugin
}

/// A coalesced resize draws for the clients connected when it draws, not for those connected
/// when it was queued. The job that draws is the oldest one queued, so a client that attached
/// during the drag would otherwise get no frame at the final size, and one that left would
/// still be drawn for.
///
/// One frame per client cannot vary: the executor is held across both resizes and the change of
/// clients, so a single job draws, once for each client connected when it runs.
#[test]
fn a_coalesced_resize_draws_for_the_clients_connected_when_it_draws() {
    let mut test = TestBridge::new();
    let owner = ClientId::new(1);
    let leaving = ClientId::new(2);
    let joining = ClientId::new(3);
    let plugin = a_shared_plugin_that_draws_its_size(&mut test, owner, &[owner, leaving]);
    let (shutdown, _shutdown_receiver) = tokio::sync::mpsc::channel(1);
    let _ = frames(&mut test, plugin);

    let release = test.hold_executor(plugin);
    for columns in [30, 31] {
        test.bridge
            .resize_plugin(plugin, columns, 10, shutdown.clone())
            .unwrap();
    }
    test.bridge.remove_client(leaving);
    test.bridge.add_client(joining).unwrap();
    release.send(()).unwrap();
    test.settle(plugin);

    let mut drawn = frames_by_client(&mut test, plugin);
    drawn.sort_by_key(|(client, _)| *client);
    assert_eq!(
        drawn,
        [
            (owner, "size 10x31".to_owned()),
            (joining, "size 10x31".to_owned())
        ]
    );
    test.bridge.unload_plugin(plugin).unwrap();
}

/// A resize that queues nothing - here because the plugin is pending again, so the size is
/// cached for when it is ready - must not supersede one that is already queued: nothing would
/// draw in its place.
///
/// The one frame counted cannot vary: the executor is held while the first resize queues its
/// job, and the second queues none, so exactly one job runs once it is released.
#[test]
fn a_resize_that_queues_nothing_does_not_cancel_a_queued_one() {
    let mut test = TestBridge::new();
    let plugin = a_plugin_that_draws_its_size(&mut test);
    let (shutdown, _shutdown_receiver) = tokio::sync::mpsc::channel(1);

    let release = test.hold_executor(plugin);
    test.bridge
        .resize_plugin(plugin, 40, 10, shutdown.clone())
        .unwrap();
    test.bridge
        .cached_resizes_for_pending_plugins
        .insert(plugin, (10, 40));
    test.bridge
        .resize_plugin(plugin, 50, 10, shutdown.clone())
        .unwrap();
    release.send(()).unwrap();
    test.settle(plugin);
    assert_eq!(frames(&mut test, plugin), ["size 10x40"]);
    assert_eq!(
        test.bridge.cached_resizes_for_pending_plugins[&plugin],
        (10, 50)
    );
    test.bridge
        .cached_resizes_for_pending_plugins
        .remove(&plugin);
    test.bridge.unload_plugin(plugin).unwrap();
}

#[test]
fn a_resize_after_unload_leaves_no_resize_slot_behind() {
    let mut test = TestBridge::new();
    let plugin = a_plugin_that_draws_its_size(&mut test);
    let (shutdown, _shutdown_receiver) = tokio::sync::mpsc::channel(1);
    test.bridge
        .resize_plugin(plugin, 40, 10, shutdown.clone())
        .unwrap();
    assert!(test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .has_resize_slots(plugin));
    test.bridge.unload_plugin(plugin).unwrap();
    test.bridge.resize_plugin(plugin, 50, 10, shutdown).unwrap();
    assert!(!test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .has_resize_slots(plugin));
}

/// A frame is owed by any change of asked size, even back to the one last drawn, and by
/// nothing else; drawing pays it off, and a request after that owes the next one. An instance's
/// first resize draws whatever drew before it. A request queues a job only when one is due and
/// none is queued to draw it.
#[test]
fn a_resize_slot_owes_a_frame_for_each_change_of_size() {
    let slot = crate::plugin_map::ResizeSlot::new(10, 80);
    assert_eq!(slot.take_for_render(), (10, 80));
    assert!(
        slot.request(10, 80),
        "an instance's first resize queues a job even at the size it has"
    );
    assert!(slot.begin_job(), "and that job draws");
    slot.note_resize();
    assert_eq!(slot.take_for_render(), (10, 80));
    assert!(!slot.request(10, 80), "the same size owes nothing");
    assert!(!slot.resize_due());

    assert!(slot.request(10, 40));
    assert!(!slot.request(10, 80), "one job is queued already");
    assert!(slot.begin_job(), "a drag that ends where it began");
    assert_eq!(slot.take_for_render(), (10, 80));
    assert!(!slot.resize_due());

    assert!(slot.request(12, 80));
    assert_eq!(slot.wanted(), (12, 80));
    assert!(slot.resize_due(), "reading the size is not drawing it");
    // A render that is not a resize job pays the frame off, and the queued job is still
    // counted until it begins - when it finds nothing to draw.
    assert_eq!(slot.take_for_render(), (12, 80));
    assert!(!slot.request(14, 80), "the queued job will draw this");
    assert!(slot.begin_job());
    assert_eq!(slot.take_for_render(), (14, 80));
    assert!(
        slot.request(16, 80),
        "nothing is queued once the job has begun"
    );
}

/// A job the executor refused never begins, so cancelling it lets the next request queue one;
/// the frame it was to draw is still owed.
#[test]
fn a_resize_slot_queues_again_after_a_refused_job() {
    let slot = crate::plugin_map::ResizeSlot::new(10, 80);
    assert!(slot.request(10, 40));
    slot.cancel_job();
    assert!(
        slot.request(10, 40),
        "the same size is still owed, and nothing is queued to draw it"
    );
    assert!(!slot.request(12, 80), "a job is counted as queued again");
}

/// A resize the executor refuses - the instance has no thread, or its thread died - must not
/// leave the instance waiting on a job that will never run: the next resize still draws.
#[test]
fn a_refused_resize_job_does_not_block_later_resizes() {
    let mut test = TestBridge::new();
    let plugin = a_plugin_that_draws_its_size(&mut test);
    let (shutdown, _shutdown_receiver) = tokio::sync::mpsc::channel(1);

    test.bridge.plugin_executor.unregister_plugin(plugin);
    test.bridge
        .resize_plugin(plugin, 40, 10, shutdown.clone())
        .unwrap();
    test.bridge.plugin_executor.register_plugin(plugin);
    test.bridge
        .resize_plugin(plugin, 50, 10, shutdown.clone())
        .unwrap();
    test.settle(plugin);
    let frames = frames(&mut test, plugin);
    assert!(
        frames.iter().any(|frame| frame == "size 10x50"),
        "{frames:?}"
    );
    test.bridge.unload_plugin(plugin).unwrap();
}

/// A render that runs between queued resizes - a pipe, timer or key the plugin redraws for -
/// draws at the newest size asked for, not at the last one a resize job got to apply. It used
/// to draw several drag steps behind the screen: 30 columns into a pane about 55 wide.
#[test]
fn a_render_between_queued_resizes_draws_at_the_newest_size() {
    let mut test = TestBridge::new();
    let plugin = a_plugin_that_draws_its_size(&mut test);
    let (shutdown, _shutdown_receiver) = tokio::sync::mpsc::channel(1);

    let release = test.hold_executor(plugin);
    test.bridge
        .resize_plugin(plugin, 40, 10, shutdown.clone())
        .unwrap();
    test.pipe(plugin, "redraw");
    test.bridge
        .resize_plugin(plugin, 50, 10, shutdown.clone())
        .unwrap();
    release.send(()).unwrap();
    test.settle(plugin);
    let frames = frames(&mut test, plugin);
    assert!(!frames.is_empty());
    assert!(
        frames.iter().all(|frame| frame == "size 10x50"),
        "{frames:?}"
    );
    test.bridge.unload_plugin(plugin).unwrap();
}

/// An instance whose guest panicked while holding its lock is skipped by a resize, rather than
/// panicking every resize job queued for it after.
///
/// No frame at all is asserted, which no scheduling can change: a skipped instance never draws.
#[test]
fn a_resize_skips_an_instance_whose_lock_is_poisoned() {
    let mut test = TestBridge::new();
    let plugin = a_plugin_that_draws_its_size(&mut test);
    let (shutdown, _shutdown_receiver) = tokio::sync::mpsc::channel(1);
    let instance = test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .get_running_plugin(plugin, None)
        .unwrap();
    let poisoner = instance.clone();
    let _ = std::thread::spawn(move || {
        let _guard = poisoner.lock();
        panic!("a guest panicking while it holds its instance");
    })
    .join();
    assert!(instance.is_poisoned());
    drop(instance);

    for columns in [40, 50] {
        test.bridge
            .resize_plugin(plugin, columns, 10, shutdown.clone())
            .unwrap();
    }
    test.settle(plugin);
    assert!(frames(&mut test, plugin).is_empty());
    test.bridge.unload_plugin(plugin).unwrap();
}
