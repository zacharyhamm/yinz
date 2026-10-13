//! Bookkeeping around plugins that are still *loading* - in `loading_plugins`, not yet in the
//! `PluginMap`. These are the states no end-to-end test can observe: whether a cancellation mark
//! was left for a loader that will run, or for one that already gave up.

use super::*;

use notify_debouncer_full::notify::RecommendedWatcher;
use std::collections::BTreeMap;
use tempfile::tempdir;
use yinz_runtime::channels::{self, SenderWithContext};
use yinz_types::data::PluginTag;

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
                gate.recv_timeout(yinz_test_support::default_timeout())
                    .unwrap();
            });
        started
            .recv_timeout(yinz_test_support::default_timeout())
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
    ) -> channels::Receiver<(T, yinz_types::diagnostics::ErrorContext)> {
        self._receivers
            .iter()
            .find_map(|receiver| {
                receiver
                    .downcast_ref::<channels::Receiver<(T, yinz_types::diagnostics::ErrorContext)>>(
                    )
            })
            .unwrap()
            .clone()
    }

    fn settle(&self, plugin_id: PluginId) {
        let (send, receive) = std::sync::mpsc::channel();
        self.bridge
            .plugin_executor
            .execute_for_plugin(plugin_id, move |_, _, _, _, _| {
                send.send(()).unwrap();
            });
        receive
            .recv_timeout(yinz_test_support::default_timeout())
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
            session_checkpoint: Default::default(),
            session_ownership: None,
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
            crate::ServerSession::Fixed(Default::default()),
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
fn unload_watchdog_does_not_keep_disposed_instances_alive() {
    let mut test = TestBridge::new();
    let plugin = PluginId::new(1);
    let client = ClientId::new(1);
    test.bridge.add_client(client).unwrap();
    let context = test.context(1, 1, RECOVERABLE_WASM);
    test.load(context).unwrap();
    let instance = test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .get_running_plugin(plugin, Some(client))
        .unwrap();
    let owners = Arc::strong_count(&instance);
    arm_unload_watchdog(
        plugin,
        Arc::new(AtomicBool::new(false)),
        vec![instance.clone()],
    );
    assert_eq!(
        Arc::strong_count(&instance),
        owners,
        "an observer must not retain a disposed plugin or its temporary directory"
    );
    test.bridge.unload_plugin(plugin).unwrap();
}

#[test]
fn pane_render_history_survives_deferred_clients() {
    use yinz_types::{data::PaneId, internal_ids::TerminalId};

    let mut test = TestBridge::new();
    let first = ClientId::new(1);
    let second = ClientId::new(2);
    let pane = PaneId::Terminal(TerminalId::new(1));
    let closed_pane = PaneId::Terminal(TerminalId::new(2));
    let contents = PaneContents {
        viewport: vec!["unchanged".into()],
        ..Default::default()
    };
    let changed = PaneContents {
        viewport: vec!["changed".into()],
        ..Default::default()
    };
    let (shutdown, _receiver) = tokio::sync::mpsc::channel(1);
    let mut report = PaneRenderReport::default();
    report.add_pane_contents(&[first, second], pane, contents.clone());
    report.add_pane_contents(&[first], closed_pane, contents);
    test.bridge
        .handle_pane_render_report(report.clone(), shutdown.clone())
        .unwrap();

    // All clients can be deferred, or just one while another continues rendering.
    test.bridge
        .handle_pane_render_report(PaneRenderReport::default(), shutdown.clone())
        .unwrap();
    let mut partial = PaneRenderReport::default();
    partial.add_pane_contents(&[second], pane, changed.clone());
    test.bridge
        .handle_pane_render_report(partial, shutdown.clone())
        .unwrap();
    report.add_pane_contents(&[second], pane, changed.clone());
    assert!(test
        .bridge
        .get_changed_panes_per_client(
            &report.all_pane_contents,
            test.bridge
                .previous_pane_render_report
                .as_ref()
                .map(|r| &r.all_pane_contents),
        )
        .is_empty());

    // Resuming replaces this client's pane map, so closed panes disappear from history.
    report
        .all_pane_contents
        .get_mut(&first)
        .unwrap()
        .remove(&closed_pane);
    test.bridge
        .handle_pane_render_report(report.clone(), shutdown)
        .unwrap();
    assert_eq!(
        test.bridge.previous_pane_render_report.as_ref(),
        Some(&report)
    );

    report.add_pane_contents(&[first], pane, changed.clone());
    let expected = HashMap::from([(first, HashMap::from([(pane, changed)]))]);
    assert_eq!(
        test.bridge.get_changed_panes_per_client(
            &report.all_pane_contents,
            test.bridge
                .previous_pane_render_report
                .as_ref()
                .map(|r| &r.all_pane_contents),
        ),
        expected,
    );
    test.bridge.remove_client(first);
    assert!(!test
        .bridge
        .previous_pane_render_report
        .as_ref()
        .unwrap()
        .all_pane_contents
        .contains_key(&first));
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
                location: RunPluginLocation::Yinz(PluginTag::new("not-a-builtin")),
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
  load() { yinz.subscribe(['CustomMessage']); },
  pipe(message) {
    yinz.command('BlockCliPipeInput', message.source.Cli);
    yinz.command('CliPipeOutput', [message.source.Cli, 'reply before retirement']);
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
                gate.recv_timeout(yinz_test_support::default_timeout())
                    .unwrap();
            });
        started
            .recv_timeout(yinz_test_support::default_timeout())
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
fn duplicate_agent_has_one_instance_across_client_attachments() {
    let mut test = TestBridge::new();
    let first = ClientId::new(1);
    test.bridge.add_client(first).unwrap();
    test.bridge.add_client(ClientId::new(2)).unwrap();
    let mut run = RunPlugin {
        location: RunPluginLocation::Yinz(PluginTag::new("new-agent")),
        ..Default::default()
    };
    run.configuration.insert("duplicate_focused", "true");
    run.configuration.insert("caller_terminal_pane_id", "7");
    let (id, _) = test
        .bridge
        .load_plugin(
            &Some(run),
            None,
            Size { rows: 5, cols: 20 },
            None,
            false,
            Some(first),
        )
        .unwrap();
    test.finish_reload(id);
    assert_eq!(
        test.bridge.all_plugin_ids(),
        [(id, first)],
        "each client instance would launch its own duplicate on the agent snapshot"
    );

    test.bridge.add_client(ClientId::new(3)).unwrap();
    test.settle(id);
    assert_eq!(test.bridge.all_plugin_ids(), [(id, first)]);
    test.bridge.unload_plugin(id).unwrap();
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
fn background_subscription_updates_preserve_other_events_and_remove_unsubscribed_events() {
    let mut test = TestBridge::new();
    let context = test.context(
        1,
        1,
        "import z from 'yinz:plugin'; export default { load() {
            z.subscribe(['PaneUpdate']);
            z.subscribe(['TabUpdate']);
            z.unsubscribe(['PaneUpdate']);
            z.unsubscribe(['TabUpdate']);
        } };",
    );
    test.load(context).unwrap();
    let registrations = test
        .receiver::<PluginInstruction>()
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::PluginSubscribedToEvents(_, _, requested, Some(current)) => {
                Some((requested, current))
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    let pane = HashSet::from([EventType::PaneUpdate]);
    let tab = HashSet::from([EventType::TabUpdate]);
    assert_eq!(
        registrations,
        [
            (pane.clone(), pane),
            (
                tab.clone(),
                HashSet::from([EventType::PaneUpdate, EventType::TabUpdate])
            ),
            (HashSet::new(), tab),
            (HashSet::new(), HashSet::new()),
        ]
    );
    test.bridge.unload_plugin(PluginId::new(1)).unwrap();
}

#[test]
fn shared_background_subscriptions_follow_the_client_after_detach() {
    for reattach in [false, true] {
        let mut test = TestBridge::new();
        let plugin = PluginId::new(1);
        let owner = ClientId::new(1);
        let next_owner = ClientId::new(2);
        test.bridge.add_client(owner).unwrap();
        let mut context = test.context(
            1,
            1,
            "import z from 'yinz:plugin'; export default { load() { z.subscribe(['PaneUpdate', 'TabUpdate']); } };",
        );
        context
            .plugin_config
            .initial_userspace_configuration
            .insert("shared", "true");
        test.bridge.shared_plugin_ids.insert(plugin);
        test.load(context).unwrap();
        let receiver = test.receiver::<PluginInstruction>();
        let registrations = || {
            receiver
                .try_iter()
                .filter_map(|(instruction, _)| match instruction {
                    PluginInstruction::PluginSubscribedToEvents(id, client, _, Some(events))
                        if id == plugin =>
                    {
                        Some((client, events))
                    },
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let events = HashSet::from([EventType::PaneUpdate, EventType::TabUpdate]);
        assert_eq!(registrations(), [(owner, events.clone())]);

        if !reattach {
            test.bridge.add_client(next_owner).unwrap();
        }
        test.bridge.remove_client(owner);
        if reattach {
            test.bridge.add_client(next_owner).unwrap();
        }
        test.settle(plugin);
        assert_eq!(
            registrations(),
            [(owner, HashSet::new()), (next_owner, events)]
        );
        test.bridge.unload_plugin(plugin).unwrap();
    }
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
  workers: {probe() {yinz.host.setTimeout(0);}},
  load() {
    yinz.subscribe(['HostFolderChanged', 'FailedToChangeHostFolder', 'CustomMessage']);
    yinz.postMessageTo('probe', 'start');
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
    let pty = test.receiver::<yinz_bus::pty::PtyInstruction>();
    let worker_generation = loop {
        let (instruction, _) = instructions
            .recv_timeout(yinz_test_support::default_timeout())
            .unwrap();
        if let PluginInstruction::DenoReply(_, _, generation, Event::Timer(_), _) = instruction {
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
                PluginInstruction::DenoReply(
                    _,
                    _,
                    reply_generation,
                    event @ Event::HostFolderChanged(_),
                    _,
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
                yinz_bus::pty::PtyInstruction::ReportPluginCwd(plugin, path) => {
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
                .recv_timeout(yinz_test_support::default_timeout())
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
        PluginInstruction::DenoReply(
            _,
            _,
            _,
            Event::HostFolderChanged(_) | Event::FailedToChangeHostFolder(_),
            _
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
  load() { yinz.subscribe(['Key']); },
  update(event) { if (event.Key) keys++; return true; },
  render() { return `keys=${keys}`; }
};
"#,
    )
    .unwrap();
    let run =
        yinz_runtime::plugin_location::run_plugin_from_url(&format!("file:{}", path.display()))
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
                Event::Key(yinz_types::keys::KeyWithModifier::new(
                    yinz_types::keys::BareKey::Char('x'),
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
    let terminal = yinz_types::internal_ids::TerminalId::new(3);
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
  load() { yinz.subscribe(['RunCommandOutput']); },
  update(event) {
    if (!event.RunCommandOutput) return false;
    const payload = event.RunCommandOutput;
    if (!(payload.data instanceof Uint8Array)) throw Error('expected bytes');
    chunks.push(`${yinz.getPluginIds().client_id}:${payload.data[0]}`);
    return true;
  },
  render() { return chunks.join(','); }
};
"#,
    )
    .unwrap();
    let mut run =
        yinz_runtime::plugin_location::run_plugin_from_url(&format!("file:{}", path.display()))
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
            .recv_timeout(yinz_test_support::default_timeout())
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
    if (yinz.getPluginIds().client_id !== 3) throw Error('wrong acting client');
    if (message.name === 'error') throw Error('pipe callback failed');
    return message.name === 'render';
  },
  render() { return `rendered for ${yinz.getPluginIds().client_id}`; }
};
"#,
    )
    .unwrap();
    let mut run =
        yinz_runtime::plugin_location::run_plugin_from_url(&format!("file:{}", path.display()))
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
            gate.recv_timeout(yinz_test_support::default_timeout())
                .unwrap();
        });
    started
        .recv_timeout(yinz_test_support::default_timeout())
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
    yinz_runtime::plugin_location::run_plugin_from_url("file:/plugins/job.ts")
        .expect("a valid plugin url")
}

/// Put `plugin_id` on `queue` as a running job, the way `pump_queue` would have.
fn launch_job(bridge: &mut WasmBridge, queue: &str, plugin_id: PluginId) {
    let mut queues = bridge.queues.lock().unwrap();
    queues.enqueue(
        queue,
        (crate::PluginId::new(1), crate::ClientId::new(1)),
        None,
        yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
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
        receiver.downcast_ref::<channels::Receiver<(ScreenInstruction, yinz_types::diagnostics::ErrorContext)>>()
    }).unwrap();
    let plugin = test._receivers.iter().find_map(|receiver| {
        receiver.downcast_ref::<channels::Receiver<(PluginInstruction, yinz_types::diagnostics::ErrorContext)>>()
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
    changes: yinz_test_support::Watched<usize>,
}

impl FileEvents {
    fn new(path: &std::path::Path) -> Self {
        use notify_debouncer_full::notify::{self, Watcher};
        let changes = yinz_test_support::Watched::new(0);
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
import z from 'yinz:plugin';
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
        let done = yinz_test_support::Watched::new(None);
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
        let done = yinz_test_support::Watched::new(false);
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
            .is::<channels::Receiver<(ScreenInstruction, yinz_types::diagnostics::ErrorContext)>>()
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
    let mut config = context.plugin_config.clone();
    let cache = YINZ_CACHE_DIR
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
        config.location = RunPluginLocation::File(
            config
                .path
                .canonicalize()
                .unwrap_or_else(|_| config.path.clone()),
        );
        assert_eq!(
            running.env().resolved_source_path,
            Some(source.canonicalize().unwrap())
        );
        assert_eq!(running.env().plugin_config, config);
        assert_eq!(map.plugin_config_of_plugin_id(id), Some(config.clone()));
        assert_eq!(
            map.run_plugin_of_plugin_id(id).unwrap().location,
            config.location
        );
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
        map.all_plugin_ids_for_plugin_location(
            &location,
            &PluginUserConfiguration::default(),
            Clone::clone
        )
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
    drop(context); // A queued load cancelled before instantiation.
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
    let started = yinz_test_support::Watched::new(false);
    let release = yinz_test_support::Watched::new(false);
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
    let started = yinz_test_support::Watched::new(false);
    let release = yinz_test_support::Watched::new(false);
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
    let done = yinz_test_support::Watched::new(false);
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
    let started = yinz_test_support::Watched::new(false);
    let release = yinz_test_support::Watched::new(false);
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
    let done = yinz_test_support::Watched::new(false);
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
                        let run = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
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
  pipe(message) { stream = message.source.Cli; yinz.command('CliPipeOutput', [stream, 'callback']); return true; },
  render() { if (stream) yinz.command('CliPipeOutput', [stream, 'render']); return 'frame'; }
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
        yinz_runtime::plugin_location::run_plugin_from_url(&format!("file:{}", path.display()))
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

#[test]
fn client_grid_reset_is_targeted_subscribed_ungated_and_keeps_identity_on_replay() {
    let mut test = TestBridge::new();
    let owner = ClientId::new(1);
    let other = ClientId::new(2);
    for client in [owner, other] {
        test.bridge.add_client(client).unwrap();
    }
    let source = r#"
        let resets: number[] = [];
        export default {
            update(event) {
                if (typeof event === 'object' && 'ClientGridReset' in event) {
                    resets.push(event.ClientGridReset);
                    return true;
                }
                return false;
            },
            render() { return `size ${resets.join(',')};`; }
        };
    "#;
    let plugin = PluginId::new(1);
    for id in [1, 2] {
        for client in [1, 2] {
            let context = test.context(id, client, source);
            test.load(context).unwrap();
        }
    }
    // Subscribe directly, as the plugin thread would after the SDK's subscribe command.
    for (_, _, instance, subscriptions) in test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .running_plugins_and_subscriptions()
    {
        subscriptions
            .lock()
            .unwrap()
            .insert(EventType::ClientGridReset);
        assert_eq!(
            check_event_permission(
                instance.lock().unwrap().env(),
                &Event::ClientGridReset(other)
            ),
            (PermissionStatus::Granted, None)
        );
    }
    let _ = frames(&mut test, plugin);
    let (shutdown, _receiver) = tokio::sync::mpsc::channel(1);
    test.bridge
        .update_plugins(
            vec![(Some(plugin), Some(other), Event::ClientGridReset(other))],
            shutdown.clone(),
        )
        .unwrap();
    test.settle(plugin);
    test.settle(PluginId::new(2));
    // All instances subscribe, but only the addressed plugin/client receives the event.
    let deliveries: Vec<_> = test
        .receiver::<ScreenInstruction>()
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            ScreenInstruction::PluginBytes(assets) => Some(assets),
            _ => None,
        })
        .flatten()
        .map(|asset| (asset.plugin_id, asset.client_id))
        .collect();
    assert_eq!(deliveries, [(plugin, other)]);

    test.bridge
        .plugin_map
        .lock()
        .unwrap()
        .get_running_plugin_and_subscriptions(PluginId::new(2), other)
        .unwrap()
        .1
        .lock()
        .unwrap()
        .remove(&EventType::ClientGridReset);
    // Subscription still matters for lifecycle events with no permission requirement.
    test.bridge
        .update_plugins(
            vec![(
                Some(PluginId::new(2)),
                Some(other),
                Event::ClientGridReset(other),
            )],
            shutdown.clone(),
        )
        .unwrap();
    test.settle(PluginId::new(2));
    assert!(frames(&mut test, PluginId::new(2)).is_empty());

    test.bridge.cache_plugin_events(plugin);
    test.bridge
        .update_plugins(
            vec![(Some(plugin), Some(other), Event::ClientGridReset(other))],
            shutdown.clone(),
        )
        .unwrap();
    assert!(
        matches!(&test.bridge.cached_events_for_pending_plugins[&plugin][0], EventOrPipeMessage::Event(Event::ClientGridReset(client)) if *client == other)
    );
    test.bridge
        .apply_cached_events(vec![plugin], true, shutdown)
        .unwrap();
    test.settle(plugin);
    let mut replayed = frames_by_client(&mut test, plugin);
    replayed.sort_by_key(|(client, _)| *client);
    assert_eq!(
        replayed,
        [(owner, "size 2".into()), (other, "size 2,2".into())]
    );
}

#[test]
fn queued_deno_reply_survives_map_removal_and_cannot_reach_replacement() {
    for cached in [false, true] {
        let mut test = TestBridge::new();
        let id = PluginId::new(1);
        let client = ClientId::new(1);
        let context = test.context(
            1,
            1,
            r#"
            let pending;
            export default {
              load() { pending = yinz.sleep(0); },
              async unload() { await pending; yinz.host.setSelectable(false); }
            };
        "#,
        );
        test.load(context).unwrap();
        {
            let map = test.bridge.plugin_map.lock().unwrap();
            let plugin = map.get_running_plugin(id, Some(client)).unwrap();
            *plugin.lock().unwrap().env().permissions.lock().unwrap() =
                Some(HashSet::from([PermissionType::ChangeApplicationState]));
        }
        let instructions = test.receiver::<PluginInstruction>();
        let (generation, event, reply) = loop {
            if let PluginInstruction::DenoReply(_, _, generation, event @ Event::Timer(_), reply) =
                instructions
                    .recv_timeout(yinz_test_support::default_timeout())
                    .unwrap()
                    .0
            {
                break (generation, event, reply);
            }
        };
        let release = test.hold_executor(id);
        let (shutdown, _) = tokio::sync::mpsc::channel(1);
        if cached {
            test.bridge
                .cached_events_for_pending_plugins
                .insert(id, vec![]);
        }
        test.bridge
            .deno_replies(
                id,
                generation,
                vec![(client, event.clone(), Some(reply))],
                shutdown.clone(),
            )
            .unwrap();
        if cached {
            test.bridge
                .apply_cached_events(vec![id], false, shutdown.clone())
                .unwrap();
        }
        // Both normal and cached notifications are queued behind the channel gate.
        test.bridge.unload_plugin(id).unwrap();
        assert!(!test.bridge.plugin_map.lock().unwrap().contains(id, client));
        let (done, finished) = std::sync::mpsc::channel();
        test.bridge
            .plugin_executor
            .execute_for_plugin(id, move |_, _, _, _, _| {
                done.send(()).unwrap();
            });
        release.send(()).unwrap();
        finished
            .recv_timeout(yinz_test_support::default_timeout())
            .unwrap();
        assert!(test
            .receiver::<ScreenInstruction>()
            .try_iter()
            .any(|(instruction, _)| matches!(
                instruction,
                ScreenInstruction::SetSelectable(_, false)
            )));
        assert!(reply.claim().is_none());

        let context = test.context(
            1,
            1,
            r#"
            export default {
              load() { yinz.subscribe(['Timer']); },
              update() { throw Error('stale notification reached replacement'); }
            };
        "#,
        );
        test.load(context).unwrap();
        test.bridge
            .deno_replies(id, generation, vec![(client, event, Some(reply))], shutdown)
            .unwrap();
        test.settle(id);
        let map = test.bridge.plugin_map.lock().unwrap();
        let plugin = map.get_running_plugin(id, Some(client)).unwrap();
        assert!(!plugin.lock().unwrap().is_unavailable());
        drop(map);
        assert!(!instructions
            .try_iter()
            .any(|(instruction, _)| matches!(instruction, PluginInstruction::PluginFailed { .. })));
    }
}

#[test]
fn pipe_launch_focus_preserves_ui_ownership() {
    for focus in [false, true] {
        let mut test = TestBridge::new();
        let ui = ClientId::new(1);
        let cli = ClientId::new(2);
        test.bridge.add_client(ui).unwrap();
        let path = test._dirs[0].path().join("pipe-focus.ts");
        std::fs::write(&path, "export default { render() { return 'ready'; } };").unwrap();
        let run = RunPluginOrAlias::RunPlugin(RunPlugin {
            location: RunPluginLocation::File(path),
            ..Default::default()
        });
        let recipients = test
            .bridge
            .get_or_load_plugins(
                run.clone(),
                Size::default(),
                None,
                false,
                true,
                false,
                None,
                None,
                Some(cli),
                None,
                focus,
                true,
            )
            .unwrap();
        let (id, owner) = recipients[0];
        assert_eq!(owner, Some(ui));
        let added: Vec<_> = test
            .receiver::<ScreenInstruction>()
            .try_iter()
            .filter_map(|(instruction, _)| {
                if let ScreenInstruction::AddPlugin(
                    floating,
                    _,
                    _,
                    _,
                    _,
                    _,
                    plugin,
                    _,
                    _,
                    suppressed,
                    _,
                    focused,
                    client,
                    _,
                    _,
                ) = instruction
                {
                    Some((floating, plugin, suppressed, focused, client))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(added, vec![(Some(true), id, false, Some(focus), Some(ui))]);
        // The loading entry is retained until the plugin thread handles completion.
        assert_eq!(
            test.bridge
                .get_or_load_plugins(
                    run,
                    Size::default(),
                    None,
                    false,
                    true,
                    false,
                    None,
                    None,
                    Some(cli),
                    None,
                    true,
                    true,
                )
                .unwrap()[0]
                .0,
            id
        );
        assert!(!test
            .receiver::<ScreenInstruction>()
            .try_iter()
            .any(|(i, _)| matches!(i, ScreenInstruction::AddPlugin(..))));
        test.settle(id);
    }
}

#[test]
fn plugin_url_query_uses_metadata_while_guest_is_locked() {
    let mut test = TestBridge::new();
    let context = test.context(1, 1, "export default {}");
    let location = context.plugin_config.location.clone();
    test.load(context).unwrap();
    let id = PluginId::new(1);
    let plugin = test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .get_running_plugin(id, None)
        .unwrap();
    let _guest = plugin.lock().unwrap();
    assert_eq!(
        test.bridge
            .get_plugin_ids_for_url(&location.display())
            .unwrap(),
        vec![id]
    );
    assert!(test
        .bridge
        .get_plugin_ids_for_url("file:/missing-plugin.ts")
        .unwrap()
        .is_empty());
    assert!(test
        .bridge
        .get_plugin_ids_for_url("unknown-alias")
        .unwrap_err()
        .contains("known alias"));
    assert!(test
        .bridge
        .get_plugin_ids_for_url("file:/unsupported.lua")
        .is_err());
}

fn lifecycle_request(
    operation: yinz_types::cli::PluginOperation,
    run: &RunPlugin,
) -> yinz_types::cli::PluginRequest {
    yinz_types::cli::PluginRequest {
        operation,
        target: Some(run.location.to_string()),
        configuration: None,
        fallback: Some(run.clone()),
        candidates: vec![],
        caller: None,
    }
}

#[test]
fn deferred_lifecycle_reload_keeps_empty_configuration_exact() {
    use yinz_types::cli::PluginOperation as Op;
    for outcome in ["loaded", "failed", "stopped"] {
        let mut test = TestBridge::new();
        let client = ClientId::new(1);
        let source = test._dirs[0].path().join("deferred.ts");
        let good = "export default {};";
        std::fs::write(&source, good).unwrap();
        let run = RunPlugin {
            location: RunPluginLocation::File(source.clone()),
            ..Default::default()
        };
        let mut start = lifecycle_request(Op::Start, &run);
        start.configuration = Some(BTreeMap::from([("setting".into(), "configured".into())]));
        let sibling = test.bridge.plugin_request(start, client).unwrap().plugins[0].id;
        test.finish_reload(sibling);
        let sibling_instance = test.instance_id(sibling, client);
        let sibling_load = test.bridge.load_ids[&sibling];
        if outcome == "failed" {
            std::fs::write(
                &source,
                "export default { load() { throw Error('load failed'); } };",
            )
            .unwrap();
        }
        let mut start = lifecycle_request(Op::Start, &run);
        start.configuration = Some(BTreeMap::new());
        let id = test.bridge.plugin_request(start, client).unwrap().plugins[0].id;
        test.wait_for_executor(id);
        let old_load = test.bridge.load_ids[&id];
        // Hold completion notifications and the executor independently: the retry cannot
        // finish while we check which generation was scheduled by those notifications.
        let instructions: Vec<_> = test.receiver::<PluginInstruction>().try_iter().collect();
        let release = test.hold_executor(id);
        for _ in 0..2 {
            let mut reload = lifecycle_request(Op::Reload, &run);
            reload.configuration = Some(BTreeMap::new());
            assert_eq!(
                test.bridge.plugin_request(reload, client).unwrap().plugins[0].id,
                id
            );
        }
        assert_eq!(test.bridge.load_ids[&id], old_load);
        assert!(test.bridge.pending_plugin_reloads.is_empty());
        assert_eq!(test.bridge.pending_plugin_id_reloads, HashSet::from([id]));
        if outcome == "stopped" {
            let mut stop = lifecycle_request(Op::Stop, &run);
            stop.target = Some(format!("plugin_{id}"));
            test.bridge.plugin_request(stop, client).unwrap();
            assert!(test.bridge.pending_plugin_id_reloads.is_empty());
        }
        std::fs::write(&source, good).unwrap();
        let (shutdown, _) = tokio::sync::mpsc::channel(1);
        let mut failed = false;
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
                    failed = true;
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
                },
                PluginInstruction::ApplyCachedEvents {
                    load_id,
                    plugin_ids,
                    done_receiving_permissions,
                } => {
                    test.bridge
                        .apply_cached_events_for_load(
                            load_id,
                            plugin_ids,
                            done_receiving_permissions,
                            shutdown.clone(),
                        )
                        .unwrap();
                },
                _ => {},
            }
        }
        assert_eq!(failed, outcome == "failed");
        assert!(test.bridge.pending_plugin_id_reloads.is_empty());
        assert_eq!(test.bridge.load_ids[&sibling], sibling_load);
        assert_eq!(test.instance_id(sibling, client), sibling_instance);
        release.send(()).unwrap();
        if outcome == "stopped" {
            assert!(!test.bridge.load_ids.contains_key(&id));
            assert!(test.bridge.run_plugin_of_plugin_id(id).is_none());
        } else {
            let replacement_load = test.bridge.load_ids[&id];
            assert_eq!(replacement_load, old_load + 1, "requests must coalesce");
            test.finish_reload(id);
            assert_eq!(test.bridge.load_ids[&id], replacement_load);
            assert!(test.bridge.loading_plugins.is_empty());
            assert!(test
                .bridge
                .plugin_map
                .lock()
                .unwrap()
                .get_running_plugin(id, Some(client))
                .is_some());
            test.bridge.unload_plugin(id).unwrap();
        }
        test.bridge.unload_plugin(sibling).unwrap();
    }
}

#[test]
fn background_lifecycle_deduplicates_pending_and_selects_configuration_exactly() {
    use yinz_types::cli::{PluginOperation as Op, PluginState};
    let mut test = TestBridge::new();
    let source = test._dirs[0].path().join("palette.ts");
    std::fs::write(&source, "export default {};").unwrap();
    let run = RunPlugin {
        location: RunPluginLocation::File(source.clone()),
        ..Default::default()
    };
    let client = ClientId::new(1);
    let mut start = lifecycle_request(Op::Start, &run);
    let mut shared = run.clone();
    shared.configuration.insert("shared", "true");
    start.candidates.push(shared.clone());
    let first = test
        .bridge
        .plugin_request(start.clone(), client)
        .unwrap()
        .plugins
        .remove(0);
    assert!(first.shared);
    assert_eq!(first.state, PluginState::Loading);
    assert_eq!(
        test.bridge.plugin_request(start, client).unwrap().plugins[0].id,
        first.id
    );
    assert_eq!(test.bridge.next_plugin_id, 1);
    test.finish_reload(first.id);
    let mut second = lifecycle_request(Op::Start, &run);
    second.configuration = Some(BTreeMap::new());
    let second = test.bridge.plugin_request(second, client).unwrap().plugins[0].id;
    assert_ne!(second, first.id);
    let error = test
        .bridge
        .plugin_request(lifecycle_request(Op::Reload, &run), client)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("ambiguous") && error.contains("shared"),
        "{error}"
    );
    let mut exact = lifecycle_request(Op::Reload, &run);
    exact.configuration = Some(shared.configuration.into_inner());
    assert_eq!(
        test.bridge.plugin_request(exact, client).unwrap().plugins[0].id,
        first.id
    );
    test.finish_reload(first.id);
    std::fs::remove_file(source).unwrap();
    let mut stop = lifecycle_request(Op::Stop, &run);
    stop.target = Some(format!("plugin_{}", first.id));
    assert_eq!(
        test.bridge
            .plugin_request(stop.clone(), client)
            .unwrap()
            .plugins[0]
            .state,
        PluginState::Stopped
    );
    stop.operation = Op::Reload;
    assert!(test
        .bridge
        .plugin_request(stop, client)
        .unwrap_err()
        .to_string()
        .contains("not loaded; use plugins start"));
    test.bridge.unload_plugin(second).unwrap();
}

#[test]
fn background_configuration_removal_respects_explicit_launches_and_stops() {
    let mut test = TestBridge::new();
    let source = test._dirs[0].path().join("configured.ts");
    std::fs::write(&source, "export default {};").unwrap();
    let run = RunPlugin {
        location: RunPluginLocation::File(source),
        ..Default::default()
    };
    let declarations = std::collections::BTreeSet::from([RunPluginOrAlias::RunPlugin(run.clone())]);
    let client = ClientId::new(1);
    test.bridge
        .reconcile_background_plugins(declarations.clone(), client)
        .unwrap();
    let id = PluginId::new(0);
    test.finish_reload(id);
    test.bridge
        .plugin_request(
            lifecycle_request(yinz_types::cli::PluginOperation::Stop, &run),
            client,
        )
        .unwrap();
    test.bridge
        .reconcile_background_plugins(declarations.clone(), client)
        .unwrap();
    assert_eq!(
        test.bridge.next_plugin_id, 1,
        "identical reconfiguration must not revive a stopped entry"
    );
    test.bridge
        .reconcile_background_plugins(Default::default(), client)
        .unwrap();
    test.bridge
        .reconcile_background_plugins(declarations, client)
        .unwrap();
    let id = test
        .bridge
        .plugin_request(
            lifecycle_request(yinz_types::cli::PluginOperation::Start, &run),
            client,
        )
        .unwrap()
        .plugins[0]
        .id;
    test.bridge
        .reconcile_background_plugins(Default::default(), client)
        .unwrap();
    assert!(
        test.bridge.run_plugin_of_plugin_id(id).is_some(),
        "explicit launch retains ownership"
    );
    test.bridge.unload_plugin(id).unwrap();
}

#[test]
fn background_permission_answers_cannot_grant_replacements_or_stopped_instances() {
    let mut test = TestBridge::new();
    let id = PluginId::new(1);
    let client = ClientId::new(1);
    let context = test.context(1, 1, "export default {};");
    test.load(context).unwrap();
    let instance = test.instance_id(id, client);
    let (shutdown, _receiver) = tokio::sync::mpsc::channel(1);
    test.bridge
        .request_permissions(
            id,
            instance,
            client,
            vec![PermissionType::RunCommands],
            shutdown.clone(),
        )
        .unwrap();
    let token = *test.bridge.permission_requests.keys().next().unwrap();
    test.bridge.reload_plugin_with_id(id).unwrap();
    test.finish_reload(id);
    let cache = test._dirs[0].path().join("permissions.kdl");
    test.bridge
        .answer_permissions(
            token,
            PermissionStatus::Granted,
            Some(cache.clone()),
            shutdown.clone(),
        )
        .unwrap();
    assert!(!cache.exists());
    let instance = test.instance_id(id, client);
    test.bridge
        .request_permissions(
            id,
            instance,
            client,
            vec![PermissionType::RunCommands],
            shutdown.clone(),
        )
        .unwrap();
    let token = *test.bridge.permission_requests.keys().next().unwrap();
    test.bridge.unload_plugin(id).unwrap();
    test.bridge
        .answer_permissions(
            token,
            PermissionStatus::Granted,
            Some(cache.clone()),
            shutdown,
        )
        .unwrap();
    assert!(!cache.exists());
    assert!(test.bridge.permission_requests.is_empty());
}

#[test]
fn background_identity_does_not_read_source_pipes() {
    let test = TestBridge::new();
    for name in ["source.ts", "source"] {
        let path = test._dirs[0].path().join(name);
        // Opening a FIFO without a writer would block identity lookup indefinitely.
        nix::unistd::mkfifo(&path, nix::sys::stat::Mode::S_IRUSR).unwrap();
        let expected = if name == "source" {
            let wasm = path.with_extension("wasm");
            std::fs::write(&wasm, b"wasm").unwrap();
            wasm
        } else {
            path.clone()
        };
        let run = RunPlugin {
            location: RunPluginLocation::File(path),
            ..Default::default()
        };
        assert_eq!(
            test.bridge.normalize_plugin(run).location,
            RunPluginLocation::File(expected.canonicalize().unwrap())
        );
    }
}

#[test]
fn background_identity_resolves_script_paths_without_validating_source() {
    let mut test = TestBridge::new();
    let cwd = test._dirs[0].path().join("cwd");
    let plugins = test._dirs[0].path().join("plugins");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&plugins).unwrap();
    test.bridge.user_plugin_dir = Some(plugins.clone());
    let path = cwd.join("source.ts");
    let fallback = plugins.join("source.ts");
    std::fs::write(&fallback, [0xff]).unwrap();
    let run = RunPlugin {
        location: RunPluginLocation::File(path.clone()),
        relative_path: Some("source.ts".into()),
        ..Default::default()
    };
    assert_eq!(
        test.bridge.normalize_plugin(run.clone()).location,
        RunPluginLocation::File(fallback.canonicalize().unwrap())
    );
    // A present declaration wins even when its contents cannot be loaded.
    std::fs::write(&path, [0xff]).unwrap();
    assert_eq!(
        test.bridge.normalize_plugin(run.clone()).location,
        RunPluginLocation::File(path.canonicalize().unwrap())
    );
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert_eq!(
        test.bridge.normalize_plugin(run).location,
        RunPluginLocation::File(path.canonicalize().unwrap())
    );
}

#[test]
fn background_identity_survives_url_spelling_and_deleted_symlinks() {
    use yinz_types::cli::PluginOperation as Op;
    let mut test = TestBridge::new();
    let source = test._dirs[0].path().join("space #%.ts");
    std::fs::write(&source, "export default {};").unwrap();
    let link = test._dirs[0].path().join("linked.ts");
    std::os::unix::fs::symlink(&source, &link).unwrap();
    let run = RunPlugin {
        location: RunPluginLocation::File(link.clone()),
        ..Default::default()
    };
    let declarations = std::collections::BTreeSet::from([RunPluginOrAlias::RunPlugin(run.clone())]);
    let client = ClientId::new(1);
    test.bridge
        .reconcile_background_plugins(declarations, client)
        .unwrap();
    let url = url::Url::from_file_path(&source).unwrap();
    let encoded = yinz_runtime::plugin_location::run_plugin_from_url(url.as_str()).unwrap();
    let first = test
        .bridge
        .plugin_request(lifecycle_request(Op::Start, &encoded), client)
        .unwrap()
        .plugins[0]
        .id;
    assert_eq!(test.bridge.next_plugin_id, 1);
    test.bridge.reload_plugin(&encoded).unwrap();
    test.finish_reload(first);
    test.finish_reload(first);
    assert!(test.bridge.pending_plugin_reloads.is_empty());
    assert_eq!(
        test.bridge.run_plugin_of_plugin_id(first).unwrap().location,
        RunPluginLocation::File(source.canonicalize().unwrap())
    );
    assert_eq!(
        test.bridge
            .run_plugin_of_plugin_id(first)
            .unwrap()
            .declared_path,
        Some(link.clone())
    );
    std::fs::remove_file(&link).unwrap();
    std::fs::remove_file(&source).unwrap();
    for (location, url) in [
        (&run.location, url::Url::from_file_path(&link).unwrap()),
        (&encoded.location, url),
    ] {
        assert_eq!(
            test.bridge.get_plugin_ids_for_url(url.as_str()).unwrap(),
            vec![first]
        );
        assert_eq!(
            test.bridge
                .all_plugin_and_client_ids_for_plugin_location(location, &run.configuration),
            vec![(first, Some(client))]
        );
    }
    assert_eq!(
        test.bridge
            .plugin_request(lifecycle_request(Op::Inspect, &run), client)
            .unwrap()
            .plugins[0]
            .id,
        first
    );
    test.bridge
        .plugin_request(lifecycle_request(Op::Stop, &run), client)
        .unwrap();
    assert!(test
        .bridge
        .plugin_request(lifecycle_request(Op::Inspect, &encoded), client)
        .unwrap()
        .plugins
        .is_empty());
}

#[test]
fn background_identity_resolves_repointed_symlink_after_last_stop() {
    use yinz_types::cli::{PluginOperation as Op, PluginState};
    let mut test = TestBridge::new();
    let first = test._dirs[0].path().join("a.ts");
    let second = test._dirs[0].path().join("b.ts");
    for source in [&first, &second] {
        std::fs::write(source, "export default {};").unwrap();
    }
    let link = test._dirs[0].path().join("link.ts");
    std::os::unix::fs::symlink(&first, &link).unwrap();
    let run = RunPlugin {
        location: RunPluginLocation::File(link.clone()),
        ..Default::default()
    };
    let client = ClientId::new(1);
    // A stopped configured declaration must not retain the old source identity.
    test.bridge
        .reconcile_background_plugins(
            std::collections::BTreeSet::from([RunPluginOrAlias::RunPlugin(run.clone())]),
            client,
        )
        .unwrap();
    test.finish_reload(PluginId::new(0));
    test.bridge
        .plugin_request(lifecycle_request(Op::Stop, &run), client)
        .unwrap();
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&second, &link).unwrap();
    let status = test
        .bridge
        .plugin_request(lifecycle_request(Op::Start, &run), client)
        .unwrap()
        .plugins
        .remove(0);
    let canonical = second.canonicalize().unwrap();
    let url = url::Url::from_file_path(&canonical).unwrap().to_string();
    assert_eq!(status.location, url);
    assert_eq!(status.state, PluginState::Loading);
    test.finish_reload(status.id);
    let status = test
        .bridge
        .plugin_request(lifecycle_request(Op::Inspect, &run), client)
        .unwrap()
        .plugins
        .remove(0);
    assert_eq!(status.location, url);
    assert_eq!(status.state, PluginState::Running);

    let instance = test.instance_id(status.id, client);
    let (shutdown, _receiver) = tokio::sync::mpsc::channel(1);
    test.bridge
        .request_permissions(
            status.id,
            instance,
            client,
            vec![PermissionType::RunCommands],
            shutdown,
        )
        .unwrap();
    assert!(test
        .receiver::<ScreenInstruction>()
        .try_iter()
        .any(|(instruction, _)| {
            matches!(instruction, ScreenInstruction::RequestPluginPermissions(id, permission)
            if id == status.id && permission.name == canonical.to_string_lossy())
        }));
    let resolved = RunPlugin {
        location: RunPluginLocation::File(canonical),
        ..Default::default()
    };
    let reloaded = test
        .bridge
        .plugin_request(lifecycle_request(Op::Reload, &resolved), client)
        .unwrap();
    assert_eq!(reloaded.plugins[0].id, status.id);
    assert_eq!(reloaded.plugins[0].location, url);
    test.finish_reload(status.id);
    let stopped = test
        .bridge
        .plugin_request(lifecycle_request(Op::Stop, &resolved), client)
        .unwrap();
    assert_eq!(stopped.plugins[0].id, status.id);
    assert_eq!(stopped.plugins[0].location, url);
    assert_eq!(stopped.plugins[0].state, PluginState::Stopped);
}

#[test]
fn background_identity_refreshes_retargeted_symlink_after_successful_reload() {
    use yinz_types::cli::PluginOperation as Op;
    for (parsed, fail_first) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut test = TestBridge::new();
        let first = test._dirs[0].path().join("a.ts");
        let second = test._dirs[0].path().join("b.ts");
        for source in [&first, &second] {
            std::fs::write(source, "export default {};").unwrap();
        }
        let link = test._dirs[0].path().join("link.ts");
        std::os::unix::fs::symlink(&first, &link).unwrap();
        let mut run = RunPlugin {
            location: RunPluginLocation::File(link.clone()),
            ..Default::default()
        };
        let declaration = run.clone();
        let url = Url::from_file_path(&link).unwrap();
        if parsed {
            run = yinz_runtime::plugin_location::run_plugin_from_url(url.as_str()).unwrap();
        }
        let client = ClientId::new(1);
        let id = test
            .bridge
            .plugin_request(lifecycle_request(Op::Start, &run), client)
            .unwrap()
            .plugins[0]
            .id;
        test.finish_reload(id);
        let original = RunPluginLocation::File(first.canonicalize().unwrap());
        let replacement = RunPluginLocation::File(second.canonicalize().unwrap());
        let mut sibling = RunPlugin {
            location: original.clone(),
            ..Default::default()
        };
        sibling.configuration.insert("setting", "sibling");
        let mut start = lifecycle_request(Op::Start, &sibling);
        start.configuration = Some(sibling.configuration.clone().into_inner());
        let sibling_id = test.bridge.plugin_request(start, client).unwrap().plugins[0].id;
        test.finish_reload(sibling_id);
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&second, &link).unwrap();
        if parsed {
            run = yinz_runtime::plugin_location::run_plugin_from_url(url.as_str()).unwrap();
            assert_eq!(run.location, replacement);
            let next_id = test.bridge.next_plugin_id;
            for operation in [Op::Inspect, Op::Start] {
                let response = test
                    .bridge
                    .plugin_request(lifecycle_request(operation, &run), client)
                    .unwrap();
                assert_eq!(response.plugins.len(), 1);
                assert_eq!(response.plugins[0].id, id);
            }
            assert_eq!(test.bridge.next_plugin_id, next_id);
        }

        if fail_first {
            std::fs::write(
                &second,
                "export default { load() { throw Error('retry'); } };",
            )
            .unwrap();
            if parsed {
                test.bridge
                    .plugin_request(lifecycle_request(Op::Reload, &run), client)
                    .unwrap();
            } else {
                test.bridge.reload_plugin_with_id(id).unwrap();
            }
            test.finish_reload(id);
            assert!(test
                .bridge
                .plugin_map
                .lock()
                .unwrap()
                .get_running_plugin(id, None)
                .is_none());
            assert_eq!(
                test.bridge.normalize_plugin(declaration.clone()).location,
                original
            );
            std::fs::write(&second, "export default {};").unwrap();
        }

        if parsed {
            test.bridge
                .plugin_request(lifecycle_request(Op::Reload, &run), client)
                .unwrap();
        } else {
            test.bridge.reload_plugin_with_id(id).unwrap();
        }
        test.finish_reload(id);
        for _ in 0..2 {
            assert_eq!(
                test.bridge.normalize_plugin(run.clone()).location,
                replacement
            );
            assert_eq!(
                test.bridge.normalize_plugin(sibling.clone()).location,
                original
            );
            assert_eq!(
                test.bridge
                    .loaded_plugin_identity(sibling_id, sibling.clone())
                    .location,
                original
            );
            let loaded = test.bridge.run_plugin_of_plugin_id(id).unwrap();
            assert_eq!(loaded.location, replacement);
            assert_eq!(loaded.declared_path, Some(link.clone()));
            let next_id = test.bridge.next_plugin_id;
            for operation in [Op::Inspect, Op::Start, Op::Reload] {
                let response = test
                    .bridge
                    .plugin_request(lifecycle_request(operation, &run), client)
                    .unwrap();
                assert_eq!(response.plugins.len(), 1);
                assert_eq!(response.plugins[0].id, id);
            }
            assert_eq!(test.bridge.next_plugin_id, next_id);
            test.finish_reload(id);
        }
        let stopped = test
            .bridge
            .plugin_request(lifecycle_request(Op::Stop, &run), client)
            .unwrap();
        assert_eq!(stopped.plugins[0].id, id);
        assert!(test.bridge.run_plugin_of_plugin_id(id).is_none());
        assert_eq!(
            test.bridge
                .loaded_plugin_identity(sibling_id, sibling)
                .location,
            original
        );
        test.bridge.unload_plugin(sibling_id).unwrap();
    }
}

#[test]
fn background_identity_mapping_survives_other_retained_identities() {
    use yinz_types::cli::PluginOperation as Op;
    for state in ["running", "loading", "retry"] {
        let mut test = TestBridge::new();
        let source = test._dirs[0].path().join("source.ts");
        std::fs::write(&source, "export default {};").unwrap();
        let link = test._dirs[0].path().join("link.ts");
        std::os::unix::fs::symlink(&source, &link).unwrap();
        let run = RunPlugin {
            location: RunPluginLocation::File(link.clone()),
            ..Default::default()
        };
        let client = ClientId::new(1);
        let first = test
            .bridge
            .plugin_request(lifecycle_request(Op::Start, &run), client)
            .unwrap()
            .plugins[0]
            .id;
        test.finish_reload(first);
        let mut sibling = RunPlugin {
            location: RunPluginLocation::File(source.canonicalize().unwrap()),
            ..Default::default()
        };
        sibling.configuration.insert("setting", "sibling");
        let second = PluginId::new(test.bridge.next_plugin_id);
        if state == "loading" {
            // Keep only the loading entry, before any runtime or retry metadata exists.
            test.bridge
                .loading_plugins
                .insert((second, sibling.clone()));
        } else {
            let mut start = lifecycle_request(Op::Start, &sibling);
            start.configuration = Some(sibling.configuration.clone().into_inner());
            assert_eq!(
                test.bridge.plugin_request(start, client).unwrap().plugins[0].id,
                second
            );
            test.finish_reload(second);
            if state == "retry" {
                std::fs::write(
                    &source,
                    "export default { load() { throw Error('retry'); } };",
                )
                .unwrap();
                test.bridge.reload_plugin_with_id(second).unwrap();
                test.finish_reload(second);
                let map = test.bridge.plugin_map.lock().unwrap();
                assert!(map.get_running_plugin(second, None).is_none());
                assert!(map.run_plugin_of_plugin_id(second).is_some());
            }
        }
        std::fs::remove_file(&link).unwrap();
        test.bridge.unload_plugin(first).unwrap();
        assert_eq!(
            test.bridge.normalize_plugin(run.clone()).location,
            sibling.location,
            "{state}"
        );
        test.bridge.unload_plugin(second).unwrap();
        assert!(
            !test
                .bridge
                .resolved_plugin_locations
                .contains_key(&run.location),
            "{state}"
        );
    }
}

#[test]
fn background_permissions_coalesce_an_identity_and_retire_only_removed_instances() {
    let mut test = TestBridge::new();
    let first = test.context(1, 1, "export default {};");
    let config = first.plugin_config.clone();
    test.load(first).unwrap();
    let mut second = test.context(2, 2, "export default {};");
    second.plugin_config = config;
    test.load(second).unwrap();
    let (shutdown, _receiver) = tokio::sync::mpsc::channel(1);
    for (id, client) in [
        (PluginId::new(1), ClientId::new(1)),
        (PluginId::new(2), ClientId::new(2)),
    ] {
        let instance = test.instance_id(id, client);
        test.bridge
            .request_permissions(
                id,
                instance,
                client,
                vec![PermissionType::RunCommands],
                shutdown.clone(),
            )
            .unwrap();
    }
    assert_eq!(test.bridge.permission_requests.len(), 1);
    let token = *test.bridge.permission_requests.keys().next().unwrap();
    test.bridge.unload_plugin(PluginId::new(1)).unwrap();
    assert_eq!(test.bridge.permission_requests.len(), 1);
    test.bridge
        .answer_permissions(
            token,
            PermissionStatus::Granted,
            Some(test._dirs[0].path().join("permissions.kdl")),
            shutdown,
        )
        .unwrap();
    assert!(test.bridge.permission_requests.is_empty());
    let map = test.bridge.plugin_map.lock().unwrap();
    let runtime = map
        .get_running_plugin(PluginId::new(2), Some(ClientId::new(2)))
        .unwrap();
    assert!(runtime
        .lock()
        .unwrap()
        .env()
        .permissions
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .contains(&PermissionType::RunCommands));
}

#[test]
fn background_storage_preserves_initial_owner_on_reload() {
    use yinz_types::plugin_data::{AttrDef, EntityDef, FetchRequest, Mutation};
    for case in ["absolute", "relative", "url", "url-retry"] {
        let mut test = TestBridge::new();
        let cwd = std::env::current_dir().unwrap();
        let dir = tempfile::tempdir_in(&cwd).unwrap();
        let source = dir.path().join("storage.ts");
        std::fs::write(&source, r#"
            let value = 'waiting';
            export default {
                update(event) {
                    if (!event.PermissionRequestResult) return false;
                    const def = {name: 'item', version: 1, attrs: [{name: 'text', kind: 'Text', indexed: false, full_text: false, unique: false, required: false, default: null}]};
                    yinz.command('DataDefineEntity', {namespace: null, def, allow_destructive: false});
                    const catalog = yinz.command('DataCatalog', {namespace: null});
                    if (catalog.length !== 1) throw Error('private catalog lost its entity');
                    const rows = yinz.command('DataFetch', {request: {namespace: null, entity: 'item', filter: null, search: null, sort: [], select: [], limit: 100, offset: 0, after_id: null}});
                    value = rows.rows[0][rows.columns.indexOf('text')].Text + '!';
                    yinz.command('DataApply', {namespace: null, mutations: [{Put: {entity: 'item', uid: 'row', values: [['text', {Text: value}]], expect_rev: null}}]});
                    return true;
                },
                render() { return `size ${value};`; }
            };
        "#).unwrap();
        let link = dir.path().join("legacy.ts");
        std::os::unix::fs::symlink(&source, &link).unwrap();
        let mut run = RunPlugin {
            location: RunPluginLocation::File(if case == "relative" {
                link.strip_prefix(&cwd).unwrap().to_path_buf()
            } else {
                link.clone()
            }),
            ..Default::default()
        };
        let parsed = case.starts_with("url");
        if parsed {
            run = yinz_runtime::plugin_location::run_plugin_from_url(
                Url::from_file_path(&link).unwrap().as_str(),
            )
            .unwrap();
        }
        let mut reloaded_source = source.clone();
        let owner = Url::from(&run.location).to_string();
        let namespace = yinz_storage::plugin_data_store::private_namespace(&owner);
        let store = test.bridge.data_access.plugin_data();
        store
            .define_entity(
                &namespace,
                &owner,
                &EntityDef::new("item", 1).attr(AttrDef::text("text")),
                false,
            )
            .unwrap();
        store
            .apply(
                &namespace,
                &[Mutation::put("item").uid("row").set("text", "seed").build()],
            )
            .unwrap();
        let client = ClientId::new(1);
        let (id, _) = test
            .bridge
            .load_plugin(
                &Some(run.clone()),
                None,
                Size { rows: 5, cols: 40 },
                None,
                false,
                Some(client),
            )
            .unwrap();
        let (shutdown, _) = tokio::sync::mpsc::channel(1);
        for expected in ["size seed!", "size seed!!"] {
            test.finish_reload(id);
            let loaded = test.bridge.run_plugin_of_plugin_id(id).unwrap();
            assert_eq!(
                loaded.location,
                if expected == "size seed!" {
                    run.location.clone()
                } else {
                    RunPluginLocation::File(reloaded_source.canonicalize().unwrap())
                }
            );
            assert_eq!(
                loaded.declared_path,
                PluginConfig::from_run_plugin(&run).map(|config| config.path)
            );
            assert_eq!(
                test.bridge
                    .get_plugin_ids_for_url(
                        &RunPluginLocation::File(reloaded_source.clone()).display()
                    )
                    .unwrap(),
                vec![id]
            );
            let instance = test.instance_id(id, client);
            test.bridge
                .request_permissions(
                    id,
                    instance,
                    client,
                    vec![PermissionType::PluginData],
                    shutdown.clone(),
                )
                .unwrap();
            let token = *test.bridge.permission_requests.keys().next().unwrap();
            test.bridge
                .answer_permissions(
                    token,
                    PermissionStatus::Granted,
                    Some(test._dirs[0].path().join("permissions.kdl")),
                    shutdown.clone(),
                )
                .unwrap();
            test.settle(id);
            assert!(frames(&mut test, id).iter().any(|frame| frame == expected));
            if expected == "size seed!" {
                if parsed {
                    reloaded_source = dir.path().join("replacement.ts");
                    std::fs::copy(&source, &reloaded_source).unwrap();
                    std::fs::remove_file(&link).unwrap();
                    std::os::unix::fs::symlink(&reloaded_source, &link).unwrap();
                    if case == "url-retry" {
                        std::fs::write(
                            &reloaded_source,
                            "export default { load() { throw Error('retry'); } };",
                        )
                        .unwrap();
                        test.bridge.reload_plugin_with_id(id).unwrap();
                        test.finish_reload(id);
                        assert!(test
                            .bridge
                            .plugin_map
                            .lock()
                            .unwrap()
                            .get_running_plugin(id, None)
                            .is_none());
                        std::fs::copy(&source, &reloaded_source).unwrap();
                    }
                }
                test.bridge.reload_plugin_with_id(id).unwrap();
            }
        }
        let rows = test
            .bridge
            .data_access
            .plugin_data()
            .fetch(&namespace, &FetchRequest::entity("item"))
            .unwrap();
        assert_eq!(rows.rows.len(), 1);
        let text = rows
            .columns
            .iter()
            .position(|column| column == "text")
            .unwrap();
        assert_eq!(
            rows.rows[0][text],
            yinz_types::plugin_data::DataValue::Text("seed!!".into())
        );
        test.bridge.unload_plugin(id).unwrap();
    }
}

#[test]
fn background_permissions_preserve_each_requesting_client_including_cached_grants() {
    let mut test = TestBridge::new();
    let id = PluginId::new(1);
    let owner = ClientId::new(1);
    let other = ClientId::new(2);
    let mut context = test.context(
        1,
        1,
        r#"
        let clients = [];
        export default {
            update(event) {
                if (event.PermissionRequestResult) clients.push(yinz.getPluginIds().client_id);
                return true;
            },
            render() { return `size ${clients.join(',')};`; }
        };
    "#,
    );
    context
        .plugin_config
        .initial_userspace_configuration
        .insert("shared", "true");
    test.load(context).unwrap();
    test.bridge.shared_plugin_ids.insert(id);
    *test.bridge.connected_clients.lock().unwrap() = vec![owner, other];
    let instance = test.instance_id(id, owner);
    let (shutdown, _) = tokio::sync::mpsc::channel(1);
    for client in [other, other, owner] {
        test.bridge
            .request_permissions(
                id,
                instance,
                client,
                vec![PermissionType::RunCommands],
                shutdown.clone(),
            )
            .unwrap();
    }
    assert_eq!(test.bridge.permission_requests.len(), 1);
    let token = *test.bridge.permission_requests.keys().next().unwrap();
    // The default cache is isolated by the xtask test environment; this fixture has a unique URL.
    test.bridge
        .answer_permissions(token, PermissionStatus::Granted, None, shutdown.clone())
        .unwrap();
    test.settle(id);
    let rendered = frames(&mut test, id);
    assert!(
        rendered
            .iter()
            .any(|frame| frame == "size 1,2" || frame == "size 2,1"),
        "{rendered:?}"
    );
    test.receiver::<ScreenInstruction>()
        .try_iter()
        .for_each(drop);
    test.bridge
        .request_permissions(
            id,
            instance,
            other,
            vec![PermissionType::RunCommands],
            shutdown,
        )
        .unwrap();
    assert!(
        test.bridge.permission_requests.is_empty(),
        "grant should be cached"
    );
    test.settle(id);
    let rendered = frames(&mut test, id);
    assert!(
        rendered
            .iter()
            .any(|frame| frame == "size 1,2,2" || frame == "size 2,1,2"),
        "{rendered:?}"
    );
    test.bridge.unload_plugin(id).unwrap();
}

#[test]
fn background_permission_cancellation_replays_in_order_and_rechecks_pending_requests() {
    for another_request in [false, true] {
        let mut test = TestBridge::new();
        let id = PluginId::new(1);
        let first = ClientId::new(1);
        let second = ClientId::new(2);
        *test.bridge.connected_clients.lock().unwrap() = vec![first, second];
        let load_id = test.bridge.begin_plugin_load(id);
        for client in [1, 2] {
            let context = test.context(
                1,
                client,
                r#"
                let events = [];
                export default {
                    update(event) {
                        if (event.ClientGridReset) events.push(event.ClientGridReset);
                        return true;
                    },
                    pipe() { events.push('pipe'); return true; },
                    render() { return `size ${events.join(',')};`; }
                };
            "#,
            );
            test.load(context).unwrap();
        }
        for (_, _, _, subscriptions) in test
            .bridge
            .plugin_map
            .lock()
            .unwrap()
            .running_plugins_and_subscriptions()
        {
            subscriptions
                .lock()
                .unwrap()
                .insert(EventType::ClientGridReset);
        }
        let (shutdown, _) = tokio::sync::mpsc::channel(1);
        let instance = test.instance_id(id, first);
        test.bridge
            .request_permissions(
                id,
                instance,
                first,
                vec![PermissionType::RunCommands],
                shutdown.clone(),
            )
            .unwrap();
        test.bridge
            .update_plugins(
                vec![(Some(id), Some(second), Event::ClientGridReset(first))],
                shutdown.clone(),
            )
            .unwrap();
        test.pipe(id, "cached-permission-pipe");
        test.bridge.remove_client(first);
        test.settle(id);
        let released = test.receiver::<PluginInstruction>().try_iter().any(|(event, _)| matches!(event, PluginInstruction::ReleasePluginInstancePipes(plugin, retired) if plugin == id && retired == instance));
        assert!(released);
        test.bridge.cancel_permission_requests(id, Some(instance));
        let replay = test
            .receiver::<PluginInstruction>()
            .try_iter()
            .find_map(|(event, _)| match event {
                PluginInstruction::ApplyCachedEvents {
                    load_id,
                    plugin_ids,
                    done_receiving_permissions,
                } => Some((load_id, plugin_ids, done_receiving_permissions)),
                _ => None,
            })
            .expect("cancelling the last recipient schedules replay");
        assert_eq!(replay.0, load_id);
        if another_request {
            let instance = test.instance_id(id, second);
            test.bridge
                .request_permissions(
                    id,
                    instance,
                    second,
                    vec![PermissionType::ReadFiles],
                    shutdown.clone(),
                )
                .unwrap();
        }
        test.bridge
            .apply_cached_events_for_load(replay.0, replay.1, replay.2, shutdown.clone())
            .unwrap();
        if another_request {
            assert_eq!(test.bridge.cached_events_for_pending_plugins[&id].len(), 2);
            let token = *test.bridge.permission_requests.keys().next().unwrap();
            test.bridge
                .answer_permissions(token, PermissionStatus::Denied, None, shutdown.clone())
                .unwrap();
        }
        test.bridge
            .update_plugins(
                vec![(Some(id), Some(second), Event::ClientGridReset(second))],
                shutdown,
            )
            .unwrap();
        test.settle(id);
        assert!(frames(&mut test, id)
            .iter()
            .any(|frame| frame == "size 1,pipe,2"));
        assert_eq!(test.pipe_acknowledgements(), vec!["cached-permission-pipe"]);
        assert!(!test
            .bridge
            .cached_events_for_pending_plugins
            .contains_key(&id));
        test.bridge.unload_plugin(id).unwrap();
    }
}

#[test]
fn background_permission_callback_and_render_failures_retire_exact_instances_and_jobs() {
    use yinz_types::cli::{PluginOperation, PluginState};
    for render_failure in [false, true] {
        for queued in [false, true] {
            let mut test = TestBridge::new();
            let id = PluginId::new(1);
            let client = ClientId::new(1);
            let source = if render_failure {
                "let answered = false; export default { update() { answered = true; return true; }, render() { if (answered) throw Error('permission render failed'); return ''; } };"
            } else {
                "export default { update() { throw Error('permission callback failed'); } };"
            };
            let context = test.context(1, 1, source);
            test.load(context).unwrap();
            let context = test.context(1, 2, "export default {};");
            test.load(context).unwrap();
            let runtime = test
                .bridge
                .plugin_map
                .lock()
                .unwrap()
                .get_running_plugin(id, Some(client))
                .unwrap();
            let generation = runtime.lock().unwrap().deno_generation();
            let instance = test.instance_id(id, client);
            if queued {
                launch_job(&mut test.bridge, "permissions", id);
            }
            let (shutdown, _) = tokio::sync::mpsc::channel(1);
            test.bridge
                .request_permissions(
                    id,
                    instance,
                    client,
                    vec![PermissionType::RunCommands],
                    shutdown.clone(),
                )
                .unwrap();
            let token = *test.bridge.permission_requests.keys().next().unwrap();
            test.receiver::<ScreenInstruction>()
                .try_iter()
                .for_each(drop);
            test.bridge
                .answer_permissions(token, PermissionStatus::Denied, None, shutdown)
                .unwrap();
            test.settle(id);
            assert!(!test
                .receiver::<ScreenInstruction>()
                .try_iter()
                .any(|(event, _)| matches!(event, ScreenInstruction::PluginBytes(_))));
            let failure = test
                .receiver::<PluginInstruction>()
                .try_iter()
                .find_map(|(event, _)| match event {
                    PluginInstruction::PluginFailed {
                        plugin_id,
                        deno_generation,
                        instance_id,
                        error,
                        failed_to_load,
                        load_id,
                        ..
                    } => Some((
                        plugin_id,
                        deno_generation,
                        instance_id,
                        error,
                        failed_to_load,
                        load_id,
                    )),
                    _ => None,
                })
                .expect("permission failures must reach host cleanup");
            assert_eq!(
                (failure.0, failure.1, failure.2),
                (id, generation, Some(instance))
            );
            assert!(failure.3.contains(if render_failure {
                "permission render failed"
            } else {
                "permission callback failed"
            }));
            test.bridge
                .queue_plugin_failed(
                    failure.0, failure.5, failure.1, failure.2, failure.3, failure.4,
                )
                .unwrap();
            if queued {
                assert!(test
                    .bridge
                    .queues
                    .lock()
                    .unwrap()
                    .queue_snapshot("permissions")
                    .unwrap()
                    .running
                    .is_empty());
                assert!(test.receiver::<PluginInstruction>().try_iter().any(|(event, _)| matches!(event, PluginInstruction::Update(events) if events.iter().any(|(_, _, event)| matches!(event, Event::QueueJobFinished { outcome: QueueJobOutcome::Failed, .. })))));
            } else {
                test.settle(id);
                assert!(runtime.lock().unwrap().is_disposed());
                assert!(!test
                    .bridge
                    .plugin_map
                    .lock()
                    .unwrap()
                    .get_running_plugin(id, Some(ClientId::new(2)))
                    .unwrap()
                    .lock()
                    .unwrap()
                    .is_unavailable());
                let run = test.bridge.run_plugin_of_plugin_id(id).unwrap();
                let status = test
                    .bridge
                    .plugin_request(lifecycle_request(PluginOperation::Inspect, &run), client)
                    .unwrap()
                    .plugins
                    .remove(0);
                assert_eq!(status.state, PluginState::Failed);
                assert!(status.latest_error.unwrap().contains("permission"));
                test.bridge.unload_plugin(id).unwrap();
            }
        }
    }
}

#[test]
fn background_reload_tracks_repointed_source_per_plugin() {
    use yinz_types::cli::PluginOperation as Op;
    for by_id in [false, true] {
        let mut test = TestBridge::new();
        let a = test._dirs[0].path().join("a.ts");
        let b = test._dirs[0].path().join("b.ts");
        for (path, name) in [(&a, "a"), (&b, "b")] {
            std::fs::write(
                path,
                format!("export default {{ render() {{ return 'size {name};'; }} }};"),
            )
            .unwrap();
        }
        let link = test._dirs[0].path().join("link.ts");
        std::os::unix::fs::symlink(&a, &link).unwrap();
        let run = RunPlugin {
            location: RunPluginLocation::File(link.clone()),
            ..Default::default()
        };
        let client = ClientId::new(1);
        let id = test
            .bridge
            .plugin_request(lifecycle_request(Op::Start, &run), client)
            .unwrap()
            .plugins[0]
            .id;
        test.finish_reload(id);
        let mut sibling = run.clone();
        sibling.configuration.insert("setting", "sibling");
        let mut start = lifecycle_request(Op::Start, &sibling);
        start.configuration = Some(sibling.configuration.clone().into_inner());
        let sibling_id = test.bridge.plugin_request(start, client).unwrap().plugins[0].id;
        test.finish_reload(sibling_id);
        let (shutdown, _) = tokio::sync::mpsc::channel(1);
        test.bridge
            .request_permissions(
                id,
                test.instance_id(id, client),
                client,
                vec![PermissionType::RunCommands],
                shutdown.clone(),
            )
            .unwrap();
        let token = *test.bridge.permission_requests.keys().next().unwrap();
        let cache_path = test._dirs[0].path().join("permissions.kdl");
        test.bridge
            .answer_permissions(
                token,
                PermissionStatus::Granted,
                Some(cache_path.clone()),
                shutdown.clone(),
            )
            .unwrap();
        test.settle(id);
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&b, &link).unwrap();
        let mut reload = lifecycle_request(Op::Reload, &run);
        reload.configuration = Some(BTreeMap::new());
        if by_id {
            reload.target = Some(format!("plugin_{id}"));
        }
        assert_eq!(
            test.bridge.plugin_request(reload, client).unwrap().plugins[0].id,
            id
        );
        test.finish_reload(id);
        for (plugin, expected) in [(id, "size b"), (sibling_id, "size a")] {
            test.bridge
                .resize_plugin(plugin, 40, 5, shutdown.clone())
                .unwrap();
            test.settle(plugin);
            assert!(frames(&mut test, plugin)
                .iter()
                .any(|frame| frame == expected));
        }
        let statuses = test
            .bridge
            .plugin_request(lifecycle_request(Op::List, &run), client)
            .unwrap()
            .plugins;
        for (plugin, source) in [(id, &b), (sibling_id, &a)] {
            assert_eq!(
                statuses
                    .iter()
                    .find(|status| status.id == plugin)
                    .unwrap()
                    .location,
                Url::from_file_path(source.canonicalize().unwrap())
                    .unwrap()
                    .to_string()
            );
            test.bridge
                .request_permissions(
                    plugin,
                    test.instance_id(plugin, client),
                    client,
                    vec![PermissionType::RunCommands],
                    shutdown.clone(),
                )
                .unwrap();
        }
        let prompts: Vec<_> = test
            .receiver::<ScreenInstruction>()
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                ScreenInstruction::RequestPluginPermissions(plugin, permission) => {
                    Some((plugin, permission.name))
                },
                _ => None,
            })
            .collect();
        let b_key = b.canonicalize().unwrap().to_string_lossy().into_owned();
        assert!(prompts.contains(&(id, b_key.clone())));
        assert!(prompts.contains(&(
            sibling_id,
            a.canonicalize().unwrap().to_string_lossy().into_owned()
        )));
        let cache = PermissionCache::from_path_or_default(Some(cache_path));
        assert!(cache.check_permissions(
            a.canonicalize().unwrap().to_string_lossy().into_owned(),
            &vec![PermissionType::RunCommands]
        ));
        assert!(!cache.check_permissions(b_key, &vec![PermissionType::RunCommands]));
        assert_eq!(
            test.bridge
                .get_plugin_ids_for_url(Url::from_file_path(&a).unwrap().as_str())
                .unwrap(),
            vec![sibling_id]
        );
        test.bridge
            .grant_permissions_to_location(
                Url::from_file_path(&a).unwrap().as_str(),
                vec![PermissionType::RunCommands],
                shutdown.clone(),
            )
            .unwrap();
        let runtime = test
            .bridge
            .plugin_map
            .lock()
            .unwrap()
            .get_running_plugin(id, Some(client))
            .unwrap();
        assert!(!runtime
            .lock()
            .unwrap()
            .env()
            .permissions
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|permissions| permissions.contains(&PermissionType::RunCommands)));
        let resolved = RunPlugin {
            location: RunPluginLocation::File(b),
            ..Default::default()
        };
        assert_eq!(
            test.bridge
                .plugin_request(lifecycle_request(Op::Inspect, &resolved), client)
                .unwrap()
                .plugins[0]
                .id,
            id
        );
        test.bridge
            .plugin_request(lifecycle_request(Op::Stop, &resolved), client)
            .unwrap();
        test.bridge.unload_plugin(sibling_id).unwrap();
    }
}

#[test]
fn background_reconciliation_isolates_rejections_and_retries_corrected_alias() {
    use yinz_types::cli::PluginOperation as Op;
    use yinz_types::input::layout::PluginAlias;
    let mut test = TestBridge::new();
    let source = test._dirs[0].path().join("valid.ts");
    std::fs::write(&source, "export default {};").unwrap();
    let valid = RunPlugin {
        location: RunPluginLocation::File(source),
        ..Default::default()
    };
    let rejected = RunPlugin {
        location: RunPluginLocation::Yinz(PluginTag::new("unknown-builtin")),
        ..Default::default()
    };
    let alias = RunPluginOrAlias::Alias(PluginAlias::new("correctable", &None, None));
    test.bridge
        .plugin_aliases
        .lock()
        .unwrap()
        .aliases
        .insert("correctable".into(), rejected.clone());
    let declarations = std::collections::BTreeSet::from([
        RunPluginOrAlias::Alias(PluginAlias::new("unknown-alias", &None, None)),
        alias,
        RunPluginOrAlias::RunPlugin(valid.clone()),
    ]);
    let client = ClientId::new(1);
    test.bridge
        .reconcile_background_plugins(declarations.clone(), client)
        .unwrap();
    test.finish_reload(PluginId::new(0));
    assert_eq!(test.bridge.configured_plugins, vec![valid.clone()]);
    assert!(test
        .bridge
        .plugin_request(lifecycle_request(Op::Start, &rejected), client)
        .is_err());
    let mut corrected = valid.clone();
    corrected.configuration.insert("setting", "corrected");
    test.bridge
        .plugin_aliases
        .lock()
        .unwrap()
        .aliases
        .insert("correctable".into(), corrected);
    test.bridge
        .reconcile_background_plugins(declarations, client)
        .unwrap();
    test.finish_reload(PluginId::new(1));
    assert_eq!(test.bridge.configured_plugins.len(), 2);
    assert_eq!(
        test.bridge
            .plugin_request(lifecycle_request(Op::List, &valid), client)
            .unwrap()
            .plugins
            .len(),
        2
    );
    for id in [PluginId::new(0), PluginId::new(1)] {
        test.bridge.unload_plugin(id).unwrap();
    }
}

#[test]
fn picker_cwd_tracks_each_instance_without_locking_the_guest() {
    let mut test = TestBridge::new();
    let id = PluginId::new(99);
    let first = ClientId::new(1);
    let second = ClientId::new(2);
    let missing = ClientId::new(3);
    let original = tempdir().unwrap();
    let other = tempdir().unwrap();
    let moved = tempdir().unwrap();
    for (client, cwd) in [(1, original.path()), (2, other.path())] {
        let mut context = test.context(99, client, "export default {};");
        context.plugin_cwd = cwd.to_owned();
        test.load(context).unwrap();
    }
    let plugin = test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .get_running_plugin(id, Some(first))
        .unwrap();
    {
        let _guest = plugin.lock().unwrap();
        assert_eq!(
            test.bridge
                .pick_cwd(Some(id), first, PathBuf::new())
                .unwrap(),
            original.path()
        );
        assert_eq!(
            test.bridge
                .pick_cwd(Some(id), second, PathBuf::new())
                .unwrap(),
            other.path()
        );
        assert!(test
            .bridge
            .pick_cwd(Some(id), missing, PathBuf::new())
            .is_err());
        assert_eq!(
            test.bridge
                .pick_cwd(None, missing, moved.path().to_owned())
                .unwrap(),
            moved.path()
        );
    }
    test.bridge
        .change_plugin_host_dir(moved.path().to_owned(), id, second, None)
        .unwrap();
    test.settle(id);
    assert_eq!(
        test.bridge
            .pick_cwd(Some(id), second, PathBuf::new())
            .unwrap(),
        moved.path()
    );
    assert_eq!(
        test.bridge
            .pick_cwd(Some(id), first, PathBuf::new())
            .unwrap(),
        original.path()
    );
    test.bridge
        .change_plugin_host_dir(moved.path().join("absent"), id, second, None)
        .unwrap();
    test.settle(id);
    assert_eq!(
        test.bridge
            .pick_cwd(Some(id), second, PathBuf::new())
            .unwrap(),
        moved.path()
    );
    test.bridge
        .plugin_map
        .lock()
        .unwrap()
        .remove_instance(id, first);
    assert!(test
        .bridge
        .pick_cwd(Some(id), first, PathBuf::new())
        .is_err());
    test.bridge.shared_plugin_ids.insert(id);
    assert_eq!(
        test.bridge
            .pick_cwd(Some(id), first, PathBuf::new())
            .unwrap(),
        moved.path()
    );
    {
        let mut map = test.bridge.plugin_map.lock().unwrap();
        assert!(map.rekey_client(id, second, missing));
        assert!(map.instance_cwd(id, Some(second)).is_none());
        assert_eq!(map.instance_cwd(id, Some(missing)).unwrap(), moved.path());
    }
    test.bridge
        .change_plugin_host_dir(original.path().to_owned(), id, missing, None)
        .unwrap();
    test.settle(id);
    assert_eq!(
        test.bridge
            .pick_cwd(Some(id), first, PathBuf::new())
            .unwrap(),
        original.path()
    );
    test.bridge.unload_plugin(id).unwrap();
    assert!(test
        .bridge
        .pick_cwd(Some(id), missing, PathBuf::new())
        .is_err());
    test.settle(id);
    assert!(test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .instance_cwd(id, None)
        .is_none());
}

#[test]
fn picker_ownership_busy_clients_and_startup_cancellation() {
    use yinz_types::internal_ids::TerminalId;
    use yinz_types::pick::{PickCommand as Pick, PickKind, PickOptions};
    let mut test = TestBridge::new();
    let first = ClientId::new(1);
    let second = ClientId::new(2);
    *test.bridge.connected_clients.lock().unwrap() = vec![first, second];
    let caller = PluginId::new(99);
    let context = test.context(99, 1, "export default {};");
    test.load(context).unwrap();
    let caller_instance = test.instance_id(caller, first);
    let finder_instance = PluginInstanceId::default();
    let start = |id: &str| Pick::Start {
        request_id: id.into(),
        options: PickOptions {
            kind: PickKind::File,
            roots: Some(vec!["/tmp".into()]),
            extensions: Some(vec!["pdf".into()]),
            hidden: None,
            recursive: None,
            timeout_ms: None,
        },
    };
    let finder = PluginId::new(0);
    test.bridge.plugin_executor.register_plugin(finder);
    let release = test.hold_executor(finder);
    test.bridge
        .pick_command(
            caller,
            caller_instance,
            Some(1),
            first,
            PathBuf::from("/tmp"),
            start("one"),
        )
        .unwrap();
    assert_eq!(
        test.bridge.plugin_roles.lock().unwrap().get(&finder),
        Some(&true)
    );
    assert!(test
        .bridge
        .pick_command(
            caller,
            caller_instance,
            Some(1),
            first,
            PathBuf::from("/tmp"),
            start("busy")
        )
        .unwrap_err()
        .contains("busy"));
    let request = test
        .bridge
        .pick_command(
            finder,
            finder_instance,
            None,
            first,
            PathBuf::new(),
            Pick::Request,
        )
        .unwrap()
        .unwrap();
    assert_eq!(request.options.extensions, Some(vec!["pdf".into()]));
    assert!(test
        .bridge
        .pick_command(
            finder,
            PluginInstanceId::default(),
            None,
            first,
            PathBuf::new(),
            Pick::Complete {
                result: Ok(Some("/tmp/wrong".into()))
            }
        )
        .is_err());
    test.bridge
        .pick_command(
            finder,
            finder_instance,
            None,
            first,
            PathBuf::new(),
            Pick::Attach {
                pane_id: TerminalId::new(12),
            },
        )
        .unwrap();
    test.receiver::<ScreenInstruction>()
        .try_iter()
        .for_each(drop);
    test.bridge
        .pick_command(
            caller,
            PluginInstanceId::default(),
            Some(2),
            first,
            PathBuf::new(),
            Pick::Cancel {
                request_id: "one".into(),
                timeout: false,
            },
        )
        .unwrap();
    assert_eq!(test.receiver::<ScreenInstruction>().try_iter().count(), 0);
    test.bridge
        .pick_command(
            finder,
            finder_instance,
            None,
            first,
            PathBuf::new(),
            Pick::Complete {
                result: Ok(Some("/tmp/a.pdf".into())),
            },
        )
        .unwrap();
    assert!(
        matches!(test.receiver::<ScreenInstruction>().recv().unwrap().0, ScreenInstruction::RetirePicker { finder: id, pane: Some(_), client } if id == finder && client == first)
    );
    assert!(test
        .bridge
        .pick_command(
            caller,
            caller_instance,
            Some(1),
            first,
            PathBuf::new(),
            start("still busy")
        )
        .is_err());
    test.bridge
        .pick_command(
            finder,
            finder_instance,
            None,
            first,
            PathBuf::new(),
            Pick::Complete { result: Ok(None) },
        )
        .unwrap();
    assert_eq!(test.receiver::<ScreenInstruction>().try_iter().count(), 0);
    let other = PluginId::new(1);
    test.bridge.plugin_executor.register_plugin(other);
    let release_other = test.hold_executor(other);
    test.bridge
        .pick_command(
            caller,
            caller_instance,
            Some(1),
            second,
            PathBuf::from("/tmp"),
            start("two"),
        )
        .unwrap();
    assert_eq!(test.bridge.picks.len(), 2);
    test.bridge.picks[&other]
        .opening
        .store(true, Ordering::SeqCst);
    test.bridge
        .pick_command(
            caller,
            caller_instance,
            Some(1),
            second,
            PathBuf::new(),
            Pick::Cancel {
                request_id: "two".into(),
                timeout: true,
            },
        )
        .unwrap();
    assert!(test
        .bridge
        .pick_command(
            other,
            PluginInstanceId::default(),
            None,
            second,
            PathBuf::new(),
            Pick::Request
        )
        .is_err());
    test.bridge.pick_retired(finder);
    assert_eq!(test.bridge.picks.len(), 1);
    test.bridge.unload_plugin(finder).unwrap();
    test.receiver::<ScreenInstruction>()
        .try_iter()
        .for_each(drop);
    test.bridge.unload_plugin(other).unwrap();
    assert!(!test.receiver::<ScreenInstruction>().try_iter().any(|(instruction, _)| matches!(instruction, ScreenInstruction::RetirePicker { finder, .. } if finder == other)));
    test.bridge
        .pick_command(
            other,
            PluginInstanceId::default(),
            None,
            second,
            PathBuf::new(),
            Pick::Attach {
                pane_id: TerminalId::new(16),
            },
        )
        .unwrap();
    assert!(test.receiver::<ScreenInstruction>().try_iter().any(|(instruction, _)| matches!(instruction, ScreenInstruction::RetirePicker { finder, pane: Some(_), .. } if finder == other)));
    release.send(()).unwrap();
    release_other.send(()).unwrap();
    test.bridge.pick_retired(other);
    assert!(test.bridge.picks.is_empty());
    assert!(test
        .bridge
        .pick_command(
            caller,
            caller_instance,
            None,
            ClientId::new(3),
            PathBuf::new(),
            start("disconnected")
        )
        .is_err());
}

#[test]
fn picker_lifecycle_retires_owned_panes_before_replying_to_surviving_callers() {
    use yinz_types::internal_ids::TerminalId;
    use yinz_types::pick::{PickCommand as Pick, PickKind, PickOptions};
    for (cause, stage) in [
        ("caller unload", "attached"),
        ("caller reload", "attached"),
        ("finder reload", "attached"),
        ("disconnect", "attached"),
        ("shared disconnect", "attached"),
        ("shared disconnect last client", "attached"),
        ("shared disconnect", "loading"),
        ("shared disconnect", "opening"),
        ("shared disconnect", "failed open"),
    ] {
        let mut test = TestBridge::new();
        let client = ClientId::new(1);
        *test.bridge.connected_clients.lock().unwrap() = vec![client];
        let caller = PluginId::new(99);
        let mut context = test.context(99, 1, "export default {};");
        let shared = cause.starts_with("shared");
        if shared {
            context
                .plugin_config
                .initial_userspace_configuration
                .insert("shared", "true");
            test.bridge.shared_plugin_ids.insert(caller);
            if cause == "shared disconnect" {
                test.bridge
                    .connected_clients
                    .lock()
                    .unwrap()
                    .push(ClientId::new(2));
            }
        }
        test.load(context).unwrap();
        let caller_instance = test.instance_id(caller, client);
        let finder = PluginId::new(0);
        test.bridge.plugin_executor.register_plugin(finder);
        let release = test.hold_executor(finder);
        test.bridge
            .pick_command(
                caller,
                caller_instance,
                Some(1),
                client,
                PathBuf::from("/tmp"),
                Pick::Start {
                    request_id: cause.into(),
                    options: PickOptions {
                        kind: PickKind::File,
                        roots: None,
                        extensions: None,
                        hidden: None,
                        recursive: None,
                        timeout_ms: None,
                    },
                },
            )
            .unwrap();
        let finder_instance = PluginInstanceId::default();
        if stage == "attached" {
            test.bridge
                .pick_command(
                    finder,
                    finder_instance,
                    None,
                    client,
                    PathBuf::new(),
                    Pick::Attach {
                        pane_id: TerminalId::new(12),
                    },
                )
                .unwrap();
        } else if stage != "loading" {
            test.bridge.picks[&finder]
                .opening
                .store(true, Ordering::SeqCst);
        }
        test.receiver::<ScreenInstruction>()
            .try_iter()
            .for_each(drop);
        test.receiver::<PluginInstruction>()
            .try_iter()
            .for_each(drop);
        match cause {
            "caller unload" => test.bridge.unload_plugin(caller).unwrap(),
            "caller reload" => test.bridge.reload_plugin_with_id(caller).unwrap(),
            "finder reload" => test.bridge.reload_plugin_with_id(finder).unwrap(),
            "disconnect" | "shared disconnect" | "shared disconnect last client" => {
                test.bridge.remove_client(client);
                test.settle(caller);
            },
            _ => unreachable!(),
        }
        assert!(test.bridge.picks[&finder].cancelled.load(Ordering::SeqCst));
        if stage == "opening" || stage == "failed open" {
            assert!(!test.receiver::<ScreenInstruction>().try_iter().any(
                |(instruction, _)| matches!(instruction, ScreenInstruction::RetirePicker { .. })
            ));
            assert!(!test.receiver::<PluginInstruction>().try_iter().any(
                |(instruction, _)| matches!(
                    instruction,
                    PluginInstruction::DenoEvent(_, _, _, Event::PickResult(_))
                )
            ));
            test.bridge
                .pick_command(
                    finder,
                    finder_instance,
                    None,
                    client,
                    PathBuf::new(),
                    if stage == "opening" {
                        Pick::Attach {
                            pane_id: TerminalId::new(12),
                        }
                    } else {
                        Pick::Complete {
                            result: Err("failed to open picker terminal".into()),
                        }
                    },
                )
                .unwrap();
        }
        let retirements: Vec<_> = test
            .receiver::<ScreenInstruction>()
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                ScreenInstruction::RetirePicker {
                    finder: id,
                    pane,
                    client: owner,
                } => {
                    assert_eq!(id, finder);
                    assert_eq!(owner, client);
                    Some(pane)
                },
                _ => None,
            })
            .collect();
        assert_eq!(
            retirements,
            vec![if stage == "attached" || stage == "opening" {
                Some(TerminalId::new(12))
            } else {
                None
            }],
            "{cause} {stage}"
        );
        assert!(
            !test
                .receiver::<PluginInstruction>()
                .try_iter()
                .any(|(instruction, _)| matches!(
                    instruction,
                    PluginInstruction::DenoEvent(_, _, _, Event::PickResult(_))
                )),
            "{cause}"
        );
        if cause == "shared disconnect" {
            assert_eq!(test.instance_id(caller, ClientId::new(2)), caller_instance);
        }
        test.bridge.pick_retired(finder);
        test.bridge.pick_retired(finder);
        let replies: Vec<_> = test
            .receiver::<PluginInstruction>()
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                PluginInstruction::DenoEvent(id, owner, 1, Event::PickResult(result))
                    if id == caller && owner == client =>
                {
                    Some(result)
                },
                _ => None,
            })
            .collect();
        assert_eq!(
            replies.len(),
            usize::from(cause == "finder reload" || shared),
            "{cause}"
        );
        if let Some(reply) = replies.first() {
            assert_eq!(reply.request_id, cause);
            assert!(reply.result.is_err());
            if shared {
                assert_eq!(reply.result, Err("picker client disconnected".into()));
            }
        }
        assert!(test.bridge.picks.is_empty());
        let driver_closures = test.receiver::<ScreenInstruction>().try_iter().filter(|(instruction, _)|
            matches!(instruction, ScreenInstruction::ClosePane(PaneId::Plugin(id), ..) if *id == finder)).count();
        assert_eq!(driver_closures, 1, "{cause} {stage}");
        let loading_cancel = test.bridge.cancel_token(finder);
        test.bridge.unload_plugin(finder).unwrap();
        assert!(loading_cancel.load(Ordering::Acquire));
        assert!(!test
            .bridge
            .loading_plugins
            .iter()
            .any(|(id, _)| *id == finder));
        assert!(!test
            .bridge
            .plugin_roles
            .lock()
            .unwrap()
            .contains_key(&finder));
        release.send(()).unwrap();
    }
}

#[test]
fn idle_cli_subscribers_get_snapshots_after_reconnect_and_peer_removal() {
    let mut test = TestBridge::new();
    let first = ClientId::new(7);
    let second = ClientId::new(8);
    for _ in 0..20 {
        for client in [first, second] {
            test.bridge
                .subscribe_cli_client_to_events(client, vec!["PaneUpdate".into()]);
            assert!(
                test.receiver::<ScreenInstruction>()
                    .try_iter()
                    .any(|(instruction, _)| matches!(
                        instruction,
                        ScreenInstruction::SetCliEventSubscribersPresent(true)
                    )),
                "each idle subscription requests its own snapshot"
            );
        }
        test.bridge.remove_client(first);
        assert!(!test
            .receiver::<ScreenInstruction>()
            .try_iter()
            .any(|(instruction, _)| matches!(
                instruction,
                ScreenInstruction::SetCliEventSubscribersPresent(false)
            )));
        test.bridge.forward_events_to_cli_subscribers(&[(
            None,
            None,
            Event::PaneUpdate(Default::default()),
        )]);
        let recipients: Vec<_> = test
            .receiver::<ServerInstruction>()
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                ServerInstruction::EventUpdateToClient(client, _, _) => Some(client),
                _ => None,
            })
            .collect();
        assert_eq!(recipients, [second]);
        test.bridge.remove_client(second);
        assert!(test
            .receiver::<ScreenInstruction>()
            .try_iter()
            .any(|(instruction, _)| matches!(
                instruction,
                ScreenInstruction::SetCliEventSubscribersPresent(false)
            )));
    }
}
