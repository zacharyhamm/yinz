use super::*;
use std::sync::mpsc::{self, Receiver, SyncSender};

const SOURCE: &str = r#"
(define state "ready")
(define (load config) (yinz-command (hash "Subscribe" '("ConfigWasWrittenToDisk"))))
(define (update event) (set! state "updated") #t)
(define (render rows columns) (string-append "size " state ";"))
"#;

struct HeldLoad {
    release: SyncSender<()>,
    done: Receiver<(Result<()>, usize)>,
}

impl HeldLoad {
    fn start(test: &TestBridge, context: LoadingContext) -> Self {
        Self::start_with_hook(test, context, |loader, hook| {
            loader.before_register = Some(hook);
        })
    }

    fn start_with_hook(
        test: &TestBridge,
        context: LoadingContext,
        install: fn(&mut PluginLoader, Box<dyn FnOnce() + Send>),
    ) -> Self {
        let (entered, started) = mpsc::sync_channel(1);
        let (release, gate) = mpsc::sync_channel(1);
        let (done, completion) = mpsc::channel();
        test.bridge.plugin_executor.execute_for_plugin_tagged(
            context.plugin_id,
            "load",
            false,
            move |senders, map, clients| {
                let mut loader = PluginLoader::new(context, senders, map, clients);
                install(
                    &mut loader,
                    Box::new(move || {
                        entered.send(()).unwrap();
                        gate.recv_timeout(yinz_test_support::default_timeout())
                            .unwrap();
                    }),
                );
                let result = loader.start_plugin();
                drop(loader);
                done.send((result, crate::steel::runtime_count())).unwrap();
            },
        );
        started
            .recv_timeout(yinz_test_support::default_timeout())
            .unwrap();
        Self {
            release,
            done: completion,
        }
    }

    fn finish(self) -> (Result<()>, usize) {
        self.release.send(()).unwrap();
        self.wait()
    }

    fn wait(self) -> (Result<()>, usize) {
        self.done
            .recv_timeout(yinz_test_support::default_timeout())
            .unwrap()
    }
}

impl Drop for HeldLoad {
    fn drop(&mut self) {
        // Release the executor even if a regression assertion fails while initialization is held.
        let _ = self.release.try_send(());
    }
}

fn initial_ticks(test: &TestBridge) {
    let (shutdown, _) = tokio::sync::mpsc::channel(1);
    for (instruction, _) in test.receiver::<PluginInstruction>().try_iter() {
        if let PluginInstruction::PluginTick(id, _, generation) = instruction {
            test.bridge.plugin_tick(id, generation, shutdown.clone());
        }
    }
}

#[test]
fn initialization_does_not_block_another_plugins_event_render_or_load() {
    let mut test = TestBridge::new();
    let first = test.context(1, 1, SOURCE);
    test.load(first).unwrap();
    initial_ticks(&test);
    test.settle(PluginId::new(1));
    assert_eq!(frames(&mut test, PluginId::new(1)), ["size ready"]);

    let pending = test.context(2, 1, SOURCE);
    let held = HeldLoad::start(&test, pending);
    assert!(
        test.bridge.plugin_map.try_lock().is_ok(),
        "initialization holds the session map"
    );
    let third = test.context(3, 1, SOURCE);
    test.load(third).unwrap();
    initial_ticks(&test);
    test.settle(PluginId::new(3));
    assert_eq!(frames(&mut test, PluginId::new(3)), ["size ready"]);

    let (shutdown, _) = tokio::sync::mpsc::channel(1);
    test.bridge
        .update_plugins(
            vec![(
                Some(PluginId::new(1)),
                Some(ClientId::new(1)),
                Event::ConfigWasWrittenToDisk,
            )],
            shutdown,
        )
        .unwrap();
    test.settle(PluginId::new(1));
    assert_eq!(frames(&mut test, PluginId::new(1)), ["size updated"]);
    assert!(!test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .contains(PluginId::new(2), ClientId::new(1)));

    let (loaded, runtimes) = held.finish();
    loaded.unwrap();
    assert_eq!(runtimes, 1);
    initial_ticks(&test);
    test.settle(PluginId::new(2));
    assert_eq!(frames(&mut test, PluginId::new(2)), ["size ready"]);
}

#[test]
fn unload_during_initialization_cannot_publish_or_resurrect_metadata() {
    for reload_first in [false, true] {
        let mut test = TestBridge::new();
        let context = test.context(1, 1, SOURCE);
        let id = context.plugin_id;
        let url = Url::from(&context.plugin_config.location).to_string();
        let path = context.plugin_own_data_dir.clone();
        test.bridge.loading_plugins.insert((id, run_plugin()));
        let held = HeldLoad::start(&test, context);
        if reload_first {
            test.bridge.reload_plugin_with_id(id).unwrap();
        }
        test.bridge.unload_plugin(id).unwrap();
        // Queue the barrier before releasing initialization, while the executor is still assigned.
        let (done, finished) = mpsc::channel();
        assert!(test.bridge.plugin_executor.execute_for_plugin_tagged(
            id,
            "after unload",
            false,
            move |_, _, _| done.send(crate::steel::runtime_count()).unwrap(),
        ));
        let (loaded, runtimes) = held.finish();
        let remaining = finished
            .recv_timeout(yinz_test_support::default_timeout())
            .unwrap();
        assert!(loaded
            .unwrap_err()
            .to_string()
            .contains(crate::plugin_map::CANCELLED_BY_HOST));
        assert_eq!(runtimes, 0, "cancelled initialization retained a worker");
        assert_eq!(remaining, 0, "queued reload retained a worker");
        assert!(!path.exists());
        assert!(test
            .bridge
            .plugin_map
            .lock()
            .unwrap()
            .plugin_ids()
            .is_empty());
        assert!(test.bridge.all_plugin_ids().is_empty());
        assert!(test.bridge.get_plugin_ids_for_url(&url).unwrap().is_empty());
        assert!(!test
            .receiver::<PluginInstruction>()
            .try_iter()
            .any(|(i, _)| matches!(i, PluginInstruction::PluginTick(..))));
    }
}

#[test]
fn reload_during_initialization_rejects_the_superseded_loader() {
    let mut test = TestBridge::new();
    test.bridge.add_client(ClientId::new(1)).unwrap();
    let context = test.context(1, 1, SOURCE);
    let id = context.plugin_id;
    test.bridge.loading_plugins.insert((id, run_plugin()));
    let held = HeldLoad::start(&test, context);
    test.bridge.reload_plugin_with_id(id).unwrap();
    let (loaded, runtimes) = held.finish();
    assert!(loaded
        .unwrap_err()
        .to_string()
        .contains(crate::plugin_map::CANCELLED_BY_HOST));
    assert_eq!(runtimes, 0, "superseded initialization retained a worker");
    test.finish_reload(id);
    assert_eq!(test.bridge.all_plugin_ids(), [(id, ClientId::new(1))]);
    assert_eq!(frames(&mut test, id), ["size ready"]);
}

#[test]
fn stale_reload_cannot_dispose_a_newer_runtime() {
    let mut test = TestBridge::new();
    let id = PluginId::new(1);
    let client = ClientId::new(1);
    test.bridge.add_client(client).unwrap();
    let original = test.context(1, 1, SOURCE);
    drop(original);
    let (entered, started) = mpsc::sync_channel(1);
    let (replace, replacement) = mpsc::sync_channel(1);
    let (loaded, current) = mpsc::sync_channel(1);
    test.bridge.plugin_executor.execute_for_plugin_tagged(
        id,
        "newer prepared load",
        false,
        move |senders, map, clients| {
            entered.send(()).unwrap();
            let context = replacement
                .recv_timeout(yinz_test_support::default_timeout())
                .unwrap();
            PluginLoader::new(context, senders, map.clone(), clients)
                .start_plugin()
                .unwrap();
            loaded
                .send(
                    map.lock()
                        .unwrap()
                        .get_running_plugin(id, Some(client))
                        .unwrap(),
                )
                .unwrap();
        },
    );
    started
        .recv_timeout(yinz_test_support::default_timeout())
        .unwrap();
    // Repository preparation can enqueue an older reload after a newer load already owns the executor.
    test.bridge.reload_plugin_with_id(id).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    test.bridge.cancel_tokens.insert(id, cancel.clone());
    test.bridge
        .plugin_map
        .lock()
        .unwrap()
        .set_load_token(id, cancel);
    test.bridge.begin_plugin_load(id);
    let context = test.context(
        1,
        1,
        r#"
(define state "ready")
(define (load config) (yinz-command (hash "Subscribe" '("BeforeClose"))))
(define (update event) (set! state "closed") #t)
(define (unload) (set! state "unloaded"))
(define (render rows columns) (string-append "size " state ";"))
"#,
    );
    replace.send(context).unwrap();
    let current = current
        .recv_timeout(yinz_test_support::default_timeout())
        .unwrap();
    test.settle(id);

    assert!(!current.lock().unwrap().is_disposed());
    let published = test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .get_running_plugin(id, Some(client))
        .unwrap();
    assert!(Arc::ptr_eq(&current, &published));
    assert!(!test
        .receiver::<PluginInstruction>()
        .try_iter()
        .any(|(instruction, _)| {
            matches!(
                instruction,
                PluginInstruction::ReleasePluginInstancePipes(..)
            )
        }));
    test.finish_reload(id);
    assert_eq!(frames(&mut test, id), ["size ready"]);
}

fn configured_keybinds(mode: InputMode) -> Keybinds {
    let mut keybinds = Keybinds::default();
    keybinds.get_input_mode_mut(&mode).insert(
        yinz_types::keys::KeyWithModifier::new(yinz_types::keys::BareKey::Char('x')),
        vec![yinz_types::input::actions::Action::NoOp],
    );
    keybinds
}

#[test]
fn reconfigure_snapshot_and_publication_exclude_instance_registration() {
    let mut test = TestBridge::new();
    let client = ClientId::new(1);
    let running = test.context(1, 1, SOURCE);
    test.load(running).unwrap();
    let pending = test.context(2, 1, SOURCE);
    let held = HeldLoad::start(&test, pending);
    let map = test.bridge.plugin_map.clone();
    let release = held.release.clone();
    test.bridge.after_reconfigure_snapshot = Some(Box::new(move || {
        assert!(
            matches!(map.try_lock(), Err(std::sync::TryLockError::WouldBlock)),
            "registration can slip between the snapshot and configuration publication"
        );
        release.send(()).unwrap();
    }));
    test.bridge
        .reconfigure(
            client,
            Some(configured_keybinds(InputMode::Locked)),
            Some(InputMode::Locked),
            Some(TerminalAction::RunCommand(
                yinz_types::input::command::RunCommand {
                    command: PathBuf::from("/usr/bin/true"),
                    ..Default::default()
                },
            )),
            Some("updated-layouts".into()),
            Some("updated-plugins".into()),
        )
        .unwrap();
    held.wait().0.unwrap();

    // The first instance registered before publication; the second registers after it.
    for id in [PluginId::new(1), PluginId::new(2)] {
        test.settle(id);
        let plugin = test
            .bridge
            .plugin_map
            .lock()
            .unwrap()
            .get_running_plugin(id, Some(client))
            .unwrap();
        let plugin = plugin.lock().unwrap();
        let env = plugin.env();
        assert_eq!(env.keybinds, configured_keybinds(InputMode::Locked));
        assert_eq!(env.default_mode, InputMode::Locked);
        assert!(
            matches!(&env.default_shell, Some(TerminalAction::RunCommand(command)) if command.command == std::path::Path::new("/usr/bin/true"))
        );
        assert_eq!(env.layout_dir, Some("updated-layouts".into()));
        assert_eq!(env.user_plugin_dir, Some("updated-plugins".into()));
    }
}

#[test]
fn reconfigure_during_initialization_reaches_each_client() {
    let mut test = TestBridge::new();
    let id = PluginId::new(1);
    let first = ClientId::new(1);
    let second = ClientId::new(2);
    test.connect(&[first, second]);
    let context = test.context(
        1,
        1,
        &format!("{SOURCE}\n(yinz-register-workers (hash \"worker\" (lambda (name payload) #f)))"),
    );
    let held = HeldLoad::start(&test, context);
    for (client, mode) in [(first, InputMode::Locked), (second, InputMode::Pane)] {
        test.bridge
            .reconfigure(
                client,
                Some(configured_keybinds(mode)),
                Some(mode),
                Some(TerminalAction::RunCommand(
                    yinz_types::input::command::RunCommand {
                        command: PathBuf::from("/usr/bin/true"),
                        ..Default::default()
                    },
                )),
                Some(PathBuf::from(format!("layouts-{client}"))),
                Some(PathBuf::from(format!("plugins-{client}"))),
            )
            .unwrap();
    }
    // A later context must not overwrite the first loader's pending per-client updates.
    let later_context = test.context(2, 1, SOURCE);
    held.finish().0.unwrap();
    for (client, mode) in [(first, InputMode::Locked), (second, InputMode::Pane)] {
        let plugin = test
            .bridge
            .plugin_map
            .lock()
            .unwrap()
            .get_running_plugin(id, Some(client))
            .unwrap();
        let plugin = plugin.lock().unwrap();
        let env = plugin.env();
        assert_eq!(env.default_mode, mode);
        assert_eq!(env.keybinds, configured_keybinds(mode));
        assert!(
            matches!(&env.default_shell, Some(TerminalAction::RunCommand(command)) if command.command == std::path::Path::new("/usr/bin/true"))
        );
        assert_eq!(
            env.layout_dir,
            Some(PathBuf::from(format!("layouts-{client}")))
        );
        assert_eq!(
            env.user_plugin_dir,
            Some(PathBuf::from(format!("plugins-{client}")))
        );
    }
    // New contexts use bridge defaults even when this client has an older retained snapshot.
    test.load(later_context).unwrap();
    let plugin = test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .get_running_plugin(PluginId::new(2), Some(first))
        .unwrap();
    let plugin = plugin.lock().unwrap();
    assert_eq!(plugin.env().default_mode, InputMode::Locked);
    assert_eq!(
        plugin.env().layout_dir,
        Some(PathBuf::from(format!("layouts-{second}")))
    );
}

#[test]
fn reconfigure_merges_partial_updates_and_clears_optional_settings() {
    let mut test = TestBridge::new();
    let id = PluginId::new(1);
    let client = ClientId::new(1);
    let context = test.context(1, 1, SOURCE);
    let held = HeldLoad::start(&test, context);
    test.bridge
        .reconfigure(
            client,
            Some(configured_keybinds(InputMode::Locked)),
            Some(InputMode::Locked),
            Some(TerminalAction::RunCommand(Default::default())),
            Some("layouts".into()),
            Some("plugins".into()),
        )
        .unwrap();
    test.bridge
        .reconfigure(client, None, None, None, None, None)
        .unwrap();
    held.finish().0.unwrap();
    let plugin = test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .get_running_plugin(id, Some(client))
        .unwrap();
    {
        let plugin = plugin.lock().unwrap();
        let env = plugin.env();
        assert_eq!(env.default_mode, InputMode::Locked);
        assert_eq!(env.keybinds, configured_keybinds(InputMode::Locked));
        assert!(env.default_shell.is_none());
        assert!(env.layout_dir.is_none());
        assert!(env.user_plugin_dir.is_none());
    }
    test.bridge
        .reconfigure(
            client,
            Some(Keybinds::default()),
            Some(InputMode::Normal),
            None,
            None,
            None,
        )
        .unwrap();
    test.settle(id);
    let plugin = plugin.lock().unwrap();
    assert_eq!(plugin.env().default_mode, InputMode::Normal);
    assert_eq!(plugin.env().keybinds, Keybinds::default());
}

#[test]
fn reconfigure_while_reload_is_queued_reaches_the_replacement() {
    let mut test = TestBridge::new();
    let id = PluginId::new(1);
    let client = ClientId::new(1);
    test.bridge.add_client(client).unwrap();
    let context = test.context(1, 1, SOURCE);
    test.load(context).unwrap();
    let release = test.hold_executor(id);
    test.bridge.reload_plugin_with_id(id).unwrap();
    test.bridge
        .reconfigure(
            client,
            Some(configured_keybinds(InputMode::Locked)),
            Some(InputMode::Locked),
            None,
            Some("reloaded-layouts".into()),
            None,
        )
        .unwrap();
    release.send(()).unwrap();
    test.finish_reload(id);
    let plugin = test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .get_running_plugin(id, Some(client))
        .unwrap();
    let plugin = plugin.lock().unwrap();
    assert_eq!(plugin.env().default_mode, InputMode::Locked);
    assert_eq!(
        plugin.env().keybinds,
        configured_keybinds(InputMode::Locked)
    );
    assert_eq!(plugin.env().layout_dir, Some("reloaded-layouts".into()));
}

#[test]
fn initial_load_after_owner_attachment_initializes_missing_peers_once() {
    let mut test = TestBridge::new();
    let id = PluginId::new(1);
    let owner = ClientId::new(1);
    let peer = ClientId::new(2);
    test.connect(&[owner, peer]);
    let initial = test.context(1, 1, SOURCE);
    let attachment = test.context(1, 1, SOURCE);
    test.load(attachment).unwrap();
    let owner_instance = test.instance_id(id, owner);
    assert_eq!(test.bridge.all_plugin_ids(), [(id, owner)]);

    let mut peer_instances = Vec::new();
    for context in [initial, test.context(1, 1, SOURCE)] {
        let done = yinz_test_support::Watched::new(None);
        let observed = done.clone();
        test.bridge.plugin_executor.execute_for_plugin_tagged(
            id,
            "initial load after attachment",
            false,
            move |senders, map, clients| {
                *observed.lock() =
                    Some(PluginLoader::new(context, senders, map, clients).start_plugin());
            },
        );
        done.wait_until(
            "initial load after attachment",
            yinz_test_support::default_timeout(),
            |result| result.as_ref().map(|_| ()),
        );
        done.lock().take().unwrap().unwrap();

        let mut instances = test.bridge.all_plugin_ids();
        instances.sort();
        assert_eq!(instances, [(id, owner), (id, peer)]);
        assert_eq!(test.instance_id(id, owner), owner_instance);
        peer_instances.push(test.instance_id(id, peer));
    }
    assert_eq!(peer_instances[0], peer_instances[1]);
}

#[test]
fn attachment_during_initialization_loads_each_client_once_and_keeps_first_renders() {
    let mut test = TestBridge::new();
    test.bridge.add_client(ClientId::new(1)).unwrap();
    let context = test.context(1, 1, SOURCE);
    let held = HeldLoad::start(&test, context);
    test.bridge.add_client(ClientId::new(2)).unwrap();
    let (loaded, runtimes) = held.finish();
    loaded.unwrap();
    assert_eq!(runtimes, 2);
    test.settle(PluginId::new(1));
    let ticks: Vec<_> = test
        .receiver::<PluginInstruction>()
        .try_iter()
        .filter_map(|(i, _)| match i {
            PluginInstruction::PluginTick(id, client, generation) => Some((id, client, generation)),
            _ => None,
        })
        .collect();
    // Only the two successful load callbacks produce ticks; this fixture has no timers or jobs.
    assert_eq!(
        ticks.len(),
        2,
        "attachment loaded an already cloned client again"
    );
    let (shutdown, _) = tokio::sync::mpsc::channel(1);
    for (id, _, generation) in ticks {
        test.bridge.plugin_tick(id, generation, shutdown.clone());
    }
    test.settle(PluginId::new(1));
    let mut rendered = frames_by_client(&mut test, PluginId::new(1));
    rendered.sort();
    assert_eq!(
        rendered,
        [
            (ClientId::new(1), "size ready".into()),
            (ClientId::new(2), "size ready".into())
        ]
    );
}

#[test]
fn disconnect_during_initialization_keeps_only_the_connected_clients_instance() {
    let mut test = TestBridge::new();
    test.bridge.add_client(ClientId::new(1)).unwrap();
    let context = test.context(1, 1, SOURCE);
    let held = HeldLoad::start(&test, context);
    test.bridge.add_client(ClientId::new(2)).unwrap();
    test.bridge.remove_client(ClientId::new(1));
    assert!(test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .client_configurations
        .contains_key(&ClientId::new(1)));
    let (loaded, runtimes) = held.finish();
    loaded.unwrap();
    assert_eq!(
        runtimes, 1,
        "the disconnected client's worker survived initialization"
    );
    test.settle(PluginId::new(1));
    assert!(!test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .client_configurations
        .contains_key(&ClientId::new(1)));
    assert_eq!(
        test.bridge.all_plugin_ids(),
        [(PluginId::new(1), ClientId::new(2))]
    );
    initial_ticks(&test);
    test.settle(PluginId::new(1));
    assert_eq!(
        frames_by_client(&mut test, PluginId::new(1)),
        [(ClientId::new(2), "size ready".into())]
    );
}

#[test]
fn peer_cloning_skips_clients_disconnected_after_its_snapshot() {
    let mut test = TestBridge::new();
    let id = PluginId::new(1);
    let owner = ClientId::new(1);
    let departed = ClientId::new(2);
    let remaining = ClientId::new(3);
    test.connect(&[owner, departed, remaining]);
    let context = test.context(1, 1, SUBSCRIBES_TO_INITIAL_KEYBINDS);
    let held = HeldLoad::start_with_hook(&test, context, |loader, hook| {
        loader.after_peer_snapshot = Some(hook);
    });
    test.bridge.remove_client(departed);
    let (loaded, runtimes) = held.finish();
    loaded.unwrap();
    assert_eq!(runtimes, 2, "the departed peer retained a runtime");
    test.settle(id);

    let mut instances = test.bridge.all_plugin_ids();
    instances.sort();
    assert_eq!(instances, [(id, owner), (id, remaining)]);
    let mut subscribed = Vec::new();
    for (instruction, _) in test.receiver::<PluginInstruction>().try_iter() {
        if let PluginInstruction::PluginSubscribedToEvents(plugin, client, events, _) = instruction
            && plugin == id
            && events.contains(&EventType::InitialKeybinds)
        {
            subscribed.push(client);
        }
    }
    subscribed.sort();
    assert_eq!(
        subscribed,
        [owner, remaining],
        "only connected clients may issue the load hook's Subscribe command"
    );
}

#[test]
fn metadata_lookup_does_not_wait_for_a_running_guest() {
    let mut test = TestBridge::new();
    let context = test.context(1, 1, SOURCE);
    test.load(context).unwrap();
    let directory = tempdir().unwrap();
    let moved = directory.path().canonicalize().unwrap();
    test.bridge
        .change_plugin_host_dir(moved.clone(), PluginId::new(1), ClientId::new(1), None)
        .unwrap();
    test.settle(PluginId::new(1));
    let mut map = test.bridge.plugin_map.lock().unwrap();
    let plugin = map.get_running_plugin(PluginId::new(1), None).unwrap();
    let running = plugin.lock().unwrap();
    running.resize.request(19, 81);
    let metadata = map.reload_metadata(PluginId::new(1)).unwrap();
    assert_eq!(metadata.size, (19, 81));
    assert_eq!(metadata.cwd, moved);
    map.retire_instances(PluginId::new(1));
    let retired = map.reload_metadata(PluginId::new(1)).unwrap();
    assert_eq!(retired.size, (19, 81));
    assert_eq!(retired.cwd, moved);
}

#[test]
fn reload_of_busy_plugin_keeps_the_changed_host_folder() {
    let mut test = TestBridge::new();
    let id = PluginId::new(1);
    let client = ClientId::new(1);
    test.bridge.add_client(client).unwrap();
    let original = tempdir().unwrap();
    let moved = tempdir().unwrap();
    std::fs::write(original.path().join("marker"), "original").unwrap();
    std::fs::write(moved.path().join("marker"), "moved").unwrap();
    let source = r#"
(define state "ready")
(define (load config) (yinz-command (hash "Subscribe" '("ConfigWasWrittenToDisk"))))
(define (update event)
  (set! state (yinz-decode-utf8 (yinz-command
    (hash "FsRead" (hash "path" "/host/marker" "max_bytes" json-null "offset" json-null))))) #t)
(define (render rows columns) (string-append "size " state ";"))
"#;
    let mut context = test.context(1, 1, source);
    context.plugin_cwd = original.path().canonicalize().unwrap();
    test.load(context).unwrap();
    let moved_path = moved.path().canonicalize().unwrap();
    test.bridge
        .change_plugin_host_dir(moved_path.clone(), id, client, None)
        .unwrap();
    test.settle(id);
    let plugin = test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .get_running_plugin(id, None)
        .unwrap();
    let running = plugin.lock().unwrap();
    test.bridge.reload_plugin_with_id(id).unwrap();
    drop(running);
    test.finish_reload(id);
    frames(&mut test, id);
    test.grant_files(1, 1);
    let (shutdown, _) = tokio::sync::mpsc::channel(1);
    test.bridge
        .update_plugins(
            vec![(Some(id), Some(client), Event::ConfigWasWrittenToDisk)],
            shutdown,
        )
        .unwrap();
    test.settle(id);
    assert_eq!(frames(&mut test, id), ["size moved"]);
    assert_eq!(test.bridge.get_plugin_cwd(id, client), Some(moved_path));
    test.bridge.unload_plugin(id).unwrap();
    assert!(test.bridge.get_plugin_cwd(id, client).is_none());
}

#[test]
fn failed_load_disposes_workers_and_keeps_only_retry_metadata() {
    let mut test = TestBridge::new();
    let context = test.context(
        1,
        1,
        r#"
(yinz-register-workers (hash "worker" (lambda (name payload) #f)))
(define (load config) (error "fixture load failure"))
"#,
    );
    let held = HeldLoad::start(&test, context);
    let (loaded, runtimes) = held.finish();
    assert!(format!("{:#}", loaded.unwrap_err()).contains("fixture load failure"));
    assert!(test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .client_configurations
        .is_empty());
    assert_eq!(runtimes, 0);
    assert!(test.bridge.all_plugin_ids().is_empty());
    assert_eq!(
        test.bridge.plugin_map.lock().unwrap().plugin_ids(),
        [PluginId::new(1)]
    );
}

#[test]
fn permission_answer_keeps_events_cached_until_every_client_is_initialized() {
    let mut test = TestBridge::new();
    let id = PluginId::new(1);
    let first = ClientId::new(1);
    let second = ClientId::new(2);
    test.connect(&[first, second]);
    let context = test.context(1, 1, SOURCE);
    test.load(context).unwrap();
    let context = test.context(1, 2, SOURCE);
    test.bridge.loading_plugins.insert((id, run_plugin()));
    let held = HeldLoad::start(&test, context);
    let instance = test.instance_id(id, first);
    let (shutdown, _) = tokio::sync::mpsc::channel(1);
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
            vec![(Some(id), None, Event::ConfigWasWrittenToDisk)],
            shutdown.clone(),
        )
        .unwrap();
    let token = *test.bridge.permission_requests.keys().next().unwrap();
    test.bridge
        .answer_permissions(token, PermissionStatus::Denied, None, shutdown.clone())
        .unwrap();
    assert!(test.bridge.run_plugin_of_loading_plugin_id(id).is_some());
    assert_eq!(test.bridge.cached_events_for_pending_plugins[&id].len(), 1);
    assert!(test
        .bridge
        .plugin_ids_waiting_for_permission_request
        .contains(&id));
    assert!(test.bridge.permission_requests.contains_key(&token));
    held.finish().0.unwrap();
    test.bridge
        .request_permissions(
            id,
            test.instance_id(id, second),
            second,
            vec![PermissionType::RunCommands],
            shutdown.clone(),
        )
        .unwrap();
    test.bridge
        .apply_cached_events(vec![id], false, shutdown.clone())
        .unwrap();
    test.settle(id);
    assert!(test.bridge.run_plugin_of_loading_plugin_id(id).is_none());
    assert_eq!(test.bridge.cached_events_for_pending_plugins[&id].len(), 1);
    let answers: Vec<_> = test
        .receiver::<PluginInstruction>()
        .try_iter()
        .filter_map(|(instruction, _)| match instruction {
            PluginInstruction::PermissionAnswer {
                request_id,
                status,
                cache_path,
            } => Some((request_id, status, cache_path)),
            _ => None,
        })
        .collect();
    assert_eq!(answers.len(), 1);
    for (request_id, status, cache_path) in answers {
        test.bridge
            .answer_permissions(request_id, status, cache_path, shutdown.clone())
            .unwrap();
    }
    test.settle(id);
    assert!(test.bridge.permission_requests.is_empty());
    let rendered = frames_by_client(&mut test, id);
    for client in [first, second] {
        assert!(
            rendered.contains(&(client, "size updated".into())),
            "{rendered:?}"
        );
    }
}

#[test]
fn configuration_cache_retires_departed_and_headless_owners() {
    for attached in [false, true] {
        let mut test = TestBridge::new();
        for id in 1..=4 {
            let client = ClientId::new(id);
            let plugin = PluginId::new(id);
            if attached {
                test.bridge.add_client(client).unwrap();
            }
            let context = test.context(id, id, "(void)");
            test.load(context).unwrap();
            test.bridge.remove_client(client);
            test.settle(plugin);
            if attached {
                assert!(test
                    .bridge
                    .plugin_map
                    .lock()
                    .unwrap()
                    .client_configurations
                    .is_empty());
            } else {
                assert!(test
                    .bridge
                    .plugin_map
                    .lock()
                    .unwrap()
                    .client_configurations
                    .contains_key(&client));
            }
            test.bridge.unload_plugin(plugin).unwrap();
            assert!(test
                .bridge
                .plugin_map
                .lock()
                .unwrap()
                .client_configurations
                .is_empty());
        }
        test.bridge
            .reconfigure(ClientId::NONE, None, None, None, None, None)
            .unwrap();
        assert!(test
            .bridge
            .plugin_map
            .lock()
            .unwrap()
            .client_configurations
            .is_empty());
    }
}

#[test]
fn unused_loading_context_clones_release_their_configuration() {
    let mut test = TestBridge::new();
    let context = test.context(1, 1, "(void)");
    let cloned = context.clone();
    let overlapping = test.context(2, 1, "(void)");
    drop(context);
    drop(overlapping);
    assert!(test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .client_configurations
        .contains_key(&ClientId::new(1)));
    drop(cloned);
    assert!(test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .client_configurations
        .is_empty());
}

#[test]
fn disconnected_shared_initializer_keeps_configuration_until_unloaded() {
    let mut test = TestBridge::new();
    let client = ClientId::new(1);
    let id = PluginId::new(1);
    test.bridge.add_client(client).unwrap();
    let mut context = test.context(1, 1, SOURCE);
    context
        .plugin_config
        .initial_userspace_configuration
        .insert("shared", "true");
    test.bridge.shared_plugin_ids.insert(id);
    let held = HeldLoad::start(&test, context);
    test.bridge.remove_client(client);
    test.bridge
        .reconfigure(
            ClientId::NONE,
            Some(configured_keybinds(InputMode::Locked)),
            Some(InputMode::Locked),
            None,
            None,
            None,
        )
        .unwrap();
    held.finish().0.unwrap();
    let plugin = test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .get_running_plugin(id, Some(client))
        .unwrap();
    assert_eq!(
        plugin.lock().unwrap().env().keybinds,
        configured_keybinds(InputMode::Locked)
    );
    test.bridge.unload_plugin(id).unwrap();
    assert!(test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .client_configurations
        .is_empty());
}

#[test]
fn disposed_instances_keep_retry_metadata_without_retaining_configuration() {
    let mut test = TestBridge::new();
    let client = ClientId::new(5);
    for id in [1, 2] {
        let context = test.context(id, client.get(), "(void)");
        test.load(context).unwrap();
    }
    for id in [1, 2] {
        let plugin = PluginId::new(id);
        let instance = test.instance_id(plugin, client);
        test.bridge
            .plugin_failed(
                plugin,
                None,
                None,
                Some(instance),
                "fixture failure".into(),
                false,
            )
            .unwrap();
        test.settle(plugin);
        test.pipe_outcomes();
        assert_eq!(
            test.bridge
                .plugin_map
                .lock()
                .unwrap()
                .client_configurations
                .contains_key(&client),
            id == 1,
            "a healthy peer still needs the configuration"
        );
    }
    assert_eq!(
        test.bridge.plugin_map.lock().unwrap().plugin_ids(),
        vec![PluginId::new(1), PluginId::new(2)]
    );
    test.bridge
        .reconfigure(
            ClientId::NONE,
            None,
            Some(InputMode::Locked),
            None,
            None,
            None,
        )
        .unwrap();
    assert!(test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .client_configurations
        .is_empty());
    test.bridge.reload_plugin_with_id(PluginId::new(1)).unwrap();
    test.finish_reload(PluginId::new(1));
    let plugin = test
        .bridge
        .plugin_map
        .lock()
        .unwrap()
        .get_running_plugin(PluginId::new(1), Some(client))
        .unwrap();
    assert!(!plugin.lock().unwrap().is_disposed());
    assert_eq!(plugin.lock().unwrap().env().default_mode, InputMode::Locked);
    for id in [1, 2] {
        test.bridge.unload_plugin(PluginId::new(id)).unwrap();
    }
}
