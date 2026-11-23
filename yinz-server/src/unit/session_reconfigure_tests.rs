use super::*;
use std::str::FromStr;
use yinz_config::screen_config::ScreenConfig;
use yinz_types::cli::{PluginOperation, PluginResponse, PluginState, PluginStatus};

const CLIENT: ClientId = ClientId::new(1);
const RECONFIGURE_TEST_TIMEOUT: Duration = Duration::from_secs(10);

fn scheduled_plugins() -> PluginResponse {
    PluginResponse {
        plugins: [PluginState::Running, PluginState::Loading]
            .into_iter()
            .enumerate()
            .map(|(id, state)| PluginStatus {
                id: PluginId::new(id as u32),
                location: "file:/background.scm".into(),
                configuration: Default::default(),
                background: true,
                shared: true,
                clients: vec![],
                state,
                pending_permissions: vec![],
                latest_error: None,
                pane: None,
            })
            .collect(),
    }
}

fn assert_reconfiguration_returns_before_reply(server_message_count: usize) {
    let (plugin_tx, plugin_rx) = channels::unbounded();
    let (server_tx, server_rx) = channels::bounded(50);
    let server_tx = SenderWithContext::new(server_tx);
    let (returned_tx, returned_rx) = channels::bounded(1);
    let (ready_tx, ready_rx) = channels::bounded(1);
    let (drain_tx, drain_rx) = channels::bounded(1);
    let (reply_tx, reply_rx) = channels::bounded(1);
    let (completion_tx, mut completion_rx) = tokio::sync::oneshot::channel();
    let server = thread::spawn(move || {
        let session = reconfiguration_session(ThreadSenders {
            to_plugin: Some(SenderWithContext::new(plugin_tx)),
            should_silently_fail: true,
            ..Default::default()
        });
        reconfigure_all_clients(
            "mouse_mode false\nload_plugins clear-defaults=true {}".into(),
            false,
            Some(NotificationEnd::new(completion_tx)),
            &Arc::new(RwLock::new(SessionState::new())),
            &Arc::new(RwLock::new(Some(session))),
        );
        returned_tx.send(()).unwrap();
        drain_rx.recv_timeout(RECONFIGURE_TEST_TIMEOUT).unwrap();
        for _ in 0..server_message_count {
            assert!(matches!(
                server_rx.recv_timeout(RECONFIGURE_TEST_TIMEOUT).unwrap().0,
                ServerInstruction::EventUpdateToClient(..)
            ));
        }
    });
    let plugin = thread::spawn(move || {
        let event =
            ServerInstruction::EventUpdateToClient(CLIENT, "ModeUpdate".into(), "{}".into());
        for _ in 0..server_message_count.min(50) {
            server_tx.send(event.clone()).unwrap();
        }
        ready_tx.send(()).unwrap();
        // The 51st send cannot finish until dispatch returns and the server drains its queue.
        for _ in 50..server_message_count {
            server_tx.send(event.clone()).unwrap();
        }
        loop {
            if let PluginInstruction::PluginRequest { request, reply, .. } =
                plugin_rx.recv_timeout(RECONFIGURE_TEST_TIMEOUT).unwrap().0
            {
                assert_eq!(request.operation, PluginOperation::List);
                reply_rx.recv_timeout(RECONFIGURE_TEST_TIMEOUT).unwrap();
                reply.send(Ok(scheduled_plugins())).unwrap();
                break;
            }
        }
    });
    returned_rx.recv_timeout(RECONFIGURE_TEST_TIMEOUT).unwrap();
    ready_rx.recv_timeout(RECONFIGURE_TEST_TIMEOUT).unwrap();
    assert!(matches!(
        completion_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    drain_tx.send(()).unwrap();
    reply_tx.send(()).unwrap();
    let result = global_async_runtime::get_tokio_runtime().block_on(async {
        tokio::time::timeout(RECONFIGURE_TEST_TIMEOUT, completion_rx)
            .await
            .unwrap()
            .unwrap()
    });
    assert!(result.confirmed);
    assert_eq!(result.exit_status, None);
    assert_eq!(result.error_message, None);
    assert_eq!(
        result.stdout_message.as_deref(),
        Some("Scheduled plugin state: plugin_0 Running, plugin_1 Loading")
    );
    server.join().unwrap();
    plugin.join().unwrap();
}

#[test]
fn background_reconfiguration_returns_with_completion_pending_until_plugin_reply() {
    assert_reconfiguration_returns_before_reply(0);
}

#[test]
fn background_reconfiguration_drains_a_full_server_queue_before_plugin_reply() {
    assert_reconfiguration_returns_before_reply(51);
}

#[test]
fn background_reconfiguration_reports_reply_errors_disconnects_and_timeouts() {
    for failure in ["plugin", "disconnected", "timeout"] {
        let (reply, receiver) = std::sync::mpsc::channel();
        let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
        let expected_error = match failure {
            "plugin" => {
                reply.send(Err("plugin query failed".into())).unwrap();
                "plugin query failed".to_owned()
            },
            "disconnected" => {
                drop(reply);
                format!(
                    "background plugin scheduling timed out: {}",
                    std::sync::mpsc::RecvTimeoutError::Disconnected
                )
            },
            _ => format!(
                "background plugin scheduling timed out: {}",
                std::sync::mpsc::RecvTimeoutError::Timeout
            ),
        };
        complete_plugin_reconfiguration(
            receiver,
            Some(NotificationEnd::new(completion_tx)),
            Duration::ZERO,
        );
        let result = global_async_runtime::get_tokio_runtime().block_on(async {
            tokio::time::timeout(RECONFIGURE_TEST_TIMEOUT, completion_rx)
                .await
                .unwrap()
                .unwrap()
        });
        assert!(result.confirmed);
        assert_eq!(result.exit_status, Some(1));
        assert_eq!(result.error_message, Some(expected_error));
        assert_eq!(result.stdout_message, None);
    }
}

#[test]
fn background_reconfiguration_validation_and_enqueue_failures_complete_synchronously() {
    for (config, error) in [
        ("load_plugins {", "Deserialization error"),
        (
            "load_plugins clear-defaults=true {}",
            "query scheduled background plugins",
        ),
    ] {
        let (plugin_tx, plugin_rx) = channels::unbounded();
        drop(plugin_rx);
        let session = reconfiguration_session(ThreadSenders {
            to_plugin: Some(SenderWithContext::new(plugin_tx)),
            ..Default::default()
        });
        let (completion_tx, mut completion_rx) = tokio::sync::oneshot::channel();
        reconfigure_all_clients(
            config.into(),
            false,
            Some(NotificationEnd::new(completion_tx)),
            &Arc::new(RwLock::new(SessionState::new())),
            &Arc::new(RwLock::new(Some(session))),
        );
        let result = completion_rx.try_recv().unwrap();
        assert!(result.confirmed);
        assert_eq!(result.exit_status, Some(1));
        assert!(
            result.error_message.as_ref().unwrap().contains(error),
            "{result:?}"
        );
        assert_eq!(result.stdout_message, None);
    }
}

fn reconfiguration_session(senders: ThreadSenders) -> SessionMetaData {
    SessionMetaData {
        shutdown: Default::default(),
        senders,
        data_access: std::sync::Arc::new(yinz_storage::data_access::DataAccess::sqlite(
            std::path::Path::new("/unused"),
        )),
        default_shell: None,
        current_input_modes: HashMap::new(),
        session_configuration: Default::default(),
        effective_layout: Default::default(),
        key_passthrough_clients: HashMap::new(),
        locked_escapes: HashMap::new(),
        command_palette_restore_modes: HashMap::new(),
        pending_mouse_updates: Vec::new(),
        screen_thread: None,
        pty_thread: None,
        plugin_thread: None,
        web_server: None,
        pty_writer_thread: None,
        background_jobs_thread: None,
        config_file_path: None,
        config_dir: Some(PathBuf::from("/yinz-config")),
    }
}

fn normal_mode_keybinds(bindings: &[(&str, Action)]) -> Keybinds {
    let mut keybinds = Keybinds::default();
    let normal = keybinds.0.entry(InputMode::Normal).or_default();
    for (key, action) in bindings {
        normal.insert(
            KeyWithModifier::from_str(key).unwrap(),
            vec![action.clone()],
        );
    }
    keybinds
}

fn new_pane() -> Action {
    Action::NewPane {
        direction: None,
        pane_name: None,
        start_suppressed: false,
    }
}

/// What the server sends every thread when a client's config changes: the client's effective
/// config, with the plugins' default keybinds filled into the slots it leaves free. The screen
/// tests reconfigure under the default configuration, where no plugin has registered defaults,
/// so the merge is only checked here.
#[test]
fn runtime_repository_aliases_resolve_before_replacing_configuration() {
    use yinz_types::input::layout::RunPluginLocation;
    let directory = tempfile::tempdir().unwrap();
    let mut configuration = SessionConfiguration::default();
    let (resolved, changed) = configuration
        .reconfigure_runtime_config(
            &CLIENT,
            "plugins { panel repo=\"tools\"; }".into(),
            Some(Path::new("/elsewhere/config.kdl")),
            Some(directory.path()),
        )
        .unwrap();
    assert!(changed);
    let resolved = resolved.unwrap();
    let RunPluginLocation::Git(plugin) = &resolved.plugins.aliases["panel"].location else {
        panic!("Git alias")
    };
    assert_eq!(Path::new(&plugin.repo), directory.path().join("tools"));
    let (again, changed) = configuration
        .reconfigure_runtime_config(
            &CLIENT,
            "plugins { panel repo=\"tools\"; }".into(),
            None,
            Some(directory.path()),
        )
        .unwrap();
    assert!(!changed);
    assert_eq!(again, Some(resolved.clone()));
    // Resolution also validates aliases inherited from the previous configuration.
    let mut invalid = resolved.clone();
    let RunPluginLocation::Git(plugin) =
        &mut invalid.plugins.aliases.get_mut("panel").unwrap().location
    else {
        panic!("Git alias")
    };
    plugin.repo = "ext::invalid".into();
    configuration.set_client_runtime_configuration(CLIENT, invalid.clone());
    assert!(configuration
        .reconfigure_runtime_config(
            &CLIENT,
            "mouse_mode false".into(),
            None,
            Some(directory.path())
        )
        .is_err());
    assert_eq!(configuration.get_client_configuration(&CLIENT), invalid);
}

#[test]
fn a_config_change_reconfigures_every_thread_with_plugin_default_keybinds() {
    for was_written_to_disk in [false, true] {
        let (to_screen, screen_rx) = channels::unbounded();
        let (to_plugin, plugin_rx) = channels::unbounded();
        let (to_pty, pty_rx) = channels::unbounded();
        let mut session = reconfiguration_session(ThreadSenders {
            to_screen: Some(SenderWithContext::new(to_screen)),
            to_plugin: Some(SenderWithContext::new(to_plugin)),
            to_pty: Some(SenderWithContext::new(to_pty)),
            ..Default::default()
        });
        let saved = Config {
            keybinds: normal_mode_keybinds(&[("Ctrl g", Action::Quit)]),
            ..Default::default()
        };
        let configuration = &mut session.session_configuration;
        configuration.set_saved_configuration(saved.clone());
        configuration.set_client_runtime_configuration(CLIENT, saved.clone());
        // One slot the config already binds, which the config keeps, and one it leaves free.
        configuration.register_plugin_default_keybinds(
            PluginId::new(2),
            normal_mode_keybinds(&[("Ctrl g", Action::NoOp), ("Alt x", new_pane())]),
        );
        let mut reloaded = saved;
        reloaded.options.default_shell = Some(PathBuf::from("/bin/fish"));
        reloaded.options.default_cwd = Some(PathBuf::from("/work"));
        reloaded.options.scrollback_editor = Some(PathBuf::from("/usr/bin/vi"));
        let changes = configuration.change_saved_config(reloaded.clone());
        session.propagate_configuration_changes(changes, was_written_to_disk);

        let effective_keybinds =
            normal_mode_keybinds(&[("Ctrl g", Action::Quit), ("Alt x", new_pane())]);
        let shell = RunCommand {
            command: PathBuf::from("/bin/fish"),
            cwd: Some(PathBuf::from("/work")),
            use_terminal_title: true,
            ..Default::default()
        };
        let screen: Vec<_> = screen_rx.try_iter().map(|(message, _)| message).collect();
        match screen.as_slice() {
            [ScreenInstruction::Reconfigure {
                client_id,
                keybinds,
                config,
            }] => {
                assert_eq!(*client_id, CLIENT);
                assert_eq!(keybinds, &effective_keybinds);
                let mut expected = ScreenConfig::from_config(&reloaded);
                expected.layout_dir = Some(PathBuf::from("/yinz-config/layouts"));
                assert_eq!(config, &expected);
            },
            other => panic!("expected one screen Reconfigure, got {other:?}"),
        }
        let mut plugin = plugin_rx.try_iter().map(|(message, _)| message);
        match plugin.next() {
            Some(PluginInstruction::Reconfigure {
                client_id,
                keybinds,
                default_shell: Some(TerminalAction::RunCommand(default_shell)),
                layout_dir,
                user_plugin_dir,
                was_written_to_disk: reported_written,
                ..
            }) => {
                assert_eq!(client_id, CLIENT);
                assert_eq!(keybinds.as_ref(), Some(&effective_keybinds));
                assert_eq!(default_shell, shell);
                assert_eq!(layout_dir, Some(PathBuf::from("/yinz-config/layouts")));
                assert_eq!(user_plugin_dir, Some(PathBuf::from("/yinz-config/plugins")));
                assert_eq!(reported_written, was_written_to_disk);
            },
            other => panic!("expected a plugin Reconfigure with the new shell, got {other:?}"),
        }
        assert!(matches!(
            plugin.next(),
            Some(PluginInstruction::ReconcileBackgroundPlugins {
                client_id: CLIENT,
                ..
            })
        ));
        if was_written_to_disk {
            match plugin.next() {
                Some(PluginInstruction::DetectPluginConfigChanges(plugins)) => {
                    assert_eq!(plugins, reloaded.plugins)
                },
                other => panic!("expected the plugins to look for config changes, got {other:?}"),
            }
        }
        let rest: Vec<_> = plugin.collect();
        assert!(rest.is_empty(), "unexpected plugin instructions: {rest:?}");
        let pty: Vec<_> = pty_rx.try_iter().map(|(message, _)| message).collect();
        match pty.as_slice() {
            [PtyInstruction::Reconfigure {
                client_id,
                default_editor,
                post_command_discovery_hook: None,
            }] => {
                assert_eq!(*client_id, CLIENT);
                assert_eq!(default_editor, &Some(PathBuf::from("/usr/bin/vi")));
            },
            other => panic!("expected one pty Reconfigure, got {other:?}"),
        }
        assert!(
            matches!(&session.default_shell, Some(TerminalAction::RunCommand(s)) if *s == shell),
            "{:?}",
            session.default_shell
        );

        // A reload that changed no client's config tells no thread anything and keeps the shell.
        session.propagate_configuration_changes(vec![], was_written_to_disk);
        assert!(screen_rx.try_recv().is_err());
        assert!(plugin_rx.try_recv().is_err());
        assert!(pty_rx.try_recv().is_err());
        assert!(session.default_shell.is_some());

        if was_written_to_disk {
            // A command applies its initiating client's runtime config before persisting it.
            // With only that client, the saved-config diff is empty; it still needs notifying.
            let directory = tempfile::tempdir().unwrap();
            session.config_dir = Some(directory.path().to_path_buf());
            let fragment = "mouse_mode false".to_owned();
            let (config, changed) = session
                .session_configuration
                .reconfigure_runtime_config(&CLIENT, fragment.clone(), None, Some(directory.path()))
                .unwrap();
            assert!(changed);
            let session = Arc::new(RwLock::new(Some(session)));
            update_new_saved_config(
                config,
                ConfigPersistence::Fragment(fragment),
                changed,
                &session,
                CLIENT,
                None,
            )
            .unwrap();
            assert!(screen_rx.try_iter().any(|(message, _)| matches!(
                message,
                ScreenInstruction::Reconfigure { client_id, .. } if client_id == CLIENT
            )));
            assert!(plugin_rx.try_iter().any(|(message, _)| matches!(
                message,
                PluginInstruction::DetectPluginConfigChanges(_)
            )));
        }
    }
}

#[test]
fn reconfigure_resolves_layout_overrides_and_resets_from_the_active_config_directory() {
    let (to_screen, screen_rx) = channels::unbounded();
    let (to_plugin, plugin_rx) = channels::unbounded();
    let senders = ThreadSenders {
        to_screen: Some(SenderWithContext::new(to_screen)),
        to_plugin: Some(SenderWithContext::new(to_plugin)),
        should_silently_fail: true,
        ..Default::default()
    };
    let cwd = std::env::current_dir().unwrap();
    for (configured, config_dir, expected) in [
        (
            Some("/explicit-layouts"),
            "/active-config",
            PathBuf::from("/explicit-layouts"),
        ),
        (
            None,
            "/active-config",
            PathBuf::from("/active-config/layouts"),
        ),
        (
            Some("custom/../layouts"),
            "/active-config",
            cwd.join("custom/../layouts"),
        ),
        (
            None,
            "relative/./config",
            cwd.join("relative/./config/layouts"),
        ),
    ] {
        let mut config = Config::default();
        config.options.layout_dir = configured.map(PathBuf::from);
        senders.reconfigure_threads(vec![(CLIENT, config)], Some(Path::new(config_dir)), false);
        let screen_layout_dir = screen_rx.try_iter().find_map(|(message, _)| match message {
            ScreenInstruction::Reconfigure { config, .. } => Some(config.layout_dir),
            _ => None,
        });
        let plugin_layout_dir = plugin_rx.try_iter().find_map(|(message, _)| match message {
            PluginInstruction::Reconfigure { layout_dir, .. } => Some(layout_dir),
            _ => None,
        });
        assert!(expected.is_absolute());
        let expected = Some(Some(expected));
        assert_eq!(screen_layout_dir, expected);
        assert_eq!(plugin_layout_dir, expected);
    }
}

#[test]
fn session_handoff_loads_custom_layout_from_relative_directories() {
    use yinz_types::data::{LayoutInfo, SessionConnection};
    use yinz_types::input::cli_assets::CliAssets;

    let cwd = std::env::current_dir().unwrap();
    let root = tempfile::tempdir_in(&cwd).unwrap();
    let relative_root = root.path().strip_prefix(&cwd).unwrap();
    let config_dir = relative_root.join("config");
    let explicit_dir = relative_root.join("custom-layouts");
    for configured in [None, Some(explicit_dir)] {
        let layout_dir =
            layout_dir_or_default(configured.clone(), Some(config_dir.clone())).unwrap();
        std::fs::create_dir_all(&layout_dir).unwrap();
        std::fs::write(
            layout_dir.join("custom.kdl"),
            "layout { tab name=\"Custom handoff tab\" { pane; }; }",
        )
        .unwrap();
        let mut connection = SessionConnection {
            layout: Some(LayoutInfo::File("custom".into(), Default::default())),
            ..Default::default()
        };
        connection.apply_layout_dir(&layout_dir);
        let (_, layout) = yinz_config::cli_assets::load_config_and_layout(&CliAssets {
            config_dir: Some(config_dir.clone()),
            configuration_options: Some(Options {
                layout_dir: configured,
                ..Default::default()
            }),
            layout: connection.layout,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(layout.tabs.len(), 1);
        assert_eq!(layout.tabs[0].0.as_deref(), Some("Custom handoff tab"));
    }
}

#[test]
fn stale_client_rebinding_preserves_session_background_reconciliation() {
    let mut configuration = SessionConfiguration::default();
    let other = ClientId::new(2);
    let saved = Config::from_kdl("load_plugins { \"file:/first.scm\"; }", None).unwrap();
    configuration.set_saved_configuration(saved.clone());
    configuration.set_client_runtime_configuration(CLIENT, saved.clone());
    configuration.set_client_runtime_configuration(other, saved);
    let (to_plugin, plugin_rx) = channels::unbounded();
    let senders = ThreadSenders {
        to_plugin: Some(SenderWithContext::new(to_plugin)),
        should_silently_fail: true,
        ..Default::default()
    };
    for (fragment, action) in [
        ("load_plugins { \"file:/second.scm\"; }", Action::Quit),
        ("load_plugins clear-defaults=true {}", Action::NoOp),
    ] {
        let (current, changed) = configuration
            .reconfigure_runtime_config(&CLIENT, fragment.into(), None, None)
            .unwrap();
        assert!(changed);
        let current = current.unwrap().background_plugins;
        let (rebound, changed) = configuration.rebind_keys(
            &other,
            vec![(
                InputMode::Normal,
                KeyWithModifier::from_str("Ctrl g").unwrap(),
                vec![action],
            )],
            vec![],
        );
        assert!(changed);
        let rebound = rebound.unwrap();
        assert_eq!(rebound.background_plugins, current);
        senders.reconfigure_threads(vec![(other, rebound)], None, false);
        let reconciliations: Vec<_> = plugin_rx
            .try_iter()
            .filter_map(|(instruction, _)| match instruction {
                PluginInstruction::ReconcileBackgroundPlugins { plugins, client_id } => {
                    Some((client_id, plugins))
                },
                _ => None,
            })
            .collect();
        assert_eq!(reconciliations, vec![(other, current)]);
    }
}

#[test]
fn background_declarations_are_session_wide_and_preserve_kdl_merge_rules() {
    let mut configuration = SessionConfiguration::default();
    let other = ClientId::new(2);
    configuration.set_client_runtime_configuration(other, Config::default());
    let apply = |configuration: &mut SessionConfiguration, client, fragment: &str| {
        configuration
            .reconfigure_runtime_config(&client, fragment.into(), None, None)
            .unwrap();
    };
    apply(
        &mut configuration,
        CLIENT,
        "load_plugins { \"file:/first.scm\"; }",
    );
    let first = configuration
        .get_client_configuration(&other)
        .background_plugins;
    assert_eq!(first.len(), 1);
    apply(&mut configuration, other, "mouse_mode false");
    assert_eq!(
        configuration
            .get_client_configuration(&CLIENT)
            .background_plugins,
        first
    );
    apply(
        &mut configuration,
        other,
        "load_plugins { \"file:/second.scm\"; }",
    );
    assert_eq!(
        configuration
            .get_client_configuration(&CLIENT)
            .background_plugins
            .len(),
        2
    );
    apply(
        &mut configuration,
        CLIENT,
        "load_plugins clear-defaults=true { \"file:/third.scm\"; }",
    );
    assert_eq!(
        configuration
            .get_client_configuration(&other)
            .background_plugins
            .len(),
        1
    );
    assert!(configuration
        .reconfigure_runtime_config(&CLIENT, "load_plugins {".into(), None, None)
        .is_err());
    for invalid in [
        "load_plugins { missing-alias; }",
        "load_plugins { \"yinz:missing-builtin\"; }",
    ] {
        let before = configuration.get_client_configuration(&CLIENT);
        assert!(configuration
            .reconfigure_runtime_config(&CLIENT, invalid.into(), None, None)
            .is_err());
        assert_eq!(configuration.get_client_configuration(&CLIENT), before);
    }
}

#[test]
fn unsaved_background_aliases_survive_attachment_and_staged_reconfiguration() {
    let saved = Config::default();
    let other = ClientId::new(2);
    let mut configuration = SessionConfiguration::default();
    configuration.set_saved_configuration(saved.clone());
    configuration.set_client_runtime_configuration(CLIENT, saved.clone());
    configuration
        .reconfigure_runtime_config(
            &CLIENT,
            "plugins { background location=\"file:/unsaved.scm\"; }\nload_plugins { background; }"
                .into(),
            None,
            None,
        )
        .unwrap();
    configuration.set_client_runtime_configuration(other, saved);
    let expected = configuration.get_client_configuration(&CLIENT);
    for client in [other, CLIENT] {
        let (updated, changed) = configuration
            .reconfigure_runtime_config(&client, "mouse_mode false".into(), None, None)
            .unwrap();
        assert!(changed);
        let updated = updated.unwrap();
        assert_eq!(updated.background_plugins, expected.background_plugins);
        assert_eq!(updated.plugins, expected.plugins);
    }
    // The all-client command stages successive updates on one cloned session.
    for clients in [[CLIENT, other], [other, CLIENT]] {
        let mut staged = configuration.clone();
        for client in clients {
            let (_, changed) = staged
                .reconfigure_runtime_config(&client, "pane_frames false".into(), None, None)
                .unwrap();
            assert!(changed);
        }
        for client in clients {
            assert_eq!(
                staged.get_client_configuration(&client).background_plugins,
                expected.background_plugins
            );
        }
    }
    for invalid in [
        "plugins { background location=\"yinz:missing-builtin\"; }",
        "load_plugins clear-defaults=true { missing-alias; }",
    ] {
        let before = [CLIENT, other].map(|id| configuration.get_client_configuration(&id));
        assert!(configuration
            .reconfigure_runtime_config(&other, invalid.into(), None, None)
            .is_err());
        assert_eq!(
            [CLIENT, other].map(|id| configuration.get_client_configuration(&id)),
            before
        );
    }
}

#[test]
fn background_aliases_keep_overrides_when_rebinding_and_reset_on_saved_replacement() {
    let saved = Config::from_kdl(
        r#"
            plugins {
                background location="file:/saved.scm"
                unrelated location="file:/saved-only.scm"
            }
            load_plugins { background; }
        "#,
        None,
    )
    .unwrap();
    let other = ClientId::new(2);
    let mut configuration = SessionConfiguration::default();
    configuration.set_saved_configuration(saved.clone());
    configuration.set_client_runtime_configuration(CLIENT, saved.clone());
    configuration
        .reconfigure_runtime_config(
            &CLIENT,
            r#"
                plugins {
                    background location="file:/unsaved.scm" {
                        cwd "/alias cwd"
                        inherited "alias value"
                        override "alias value"
                    }
                    unrelated location="file:/first-client.scm"
                }
                load_plugins clear-defaults=true {
                    background {
                        cwd "/declaration cwd"
                        override "declaration value"
                    }
                }
            "#
            .into(),
            None,
            None,
        )
        .unwrap();
    // The attached client's saved alias is stale; its unrelated alias remains its own.
    configuration.set_client_runtime_configuration(other, saved.clone());
    let expected = configuration.get_client_configuration(&CLIENT);
    for client in [CLIENT, other] {
        let (rebound, changed) = configuration.rebind_keys(
            &client,
            vec![(
                InputMode::Normal,
                KeyWithModifier::from_str("Ctrl g").unwrap(),
                vec![Action::Quit],
            )],
            vec![],
        );
        assert!(changed);
        let rebound = rebound.unwrap();
        let reloaded =
            Config::from_kdl(&rebound.to_string(false).unwrap(), Some(saved.clone())).unwrap();
        assert_eq!(reloaded.background_plugins, expected.background_plugins);
        assert!(reloaded.background_plugins_cleared);
        assert_eq!(
            reloaded.plugins.aliases["background"],
            expected.plugins.aliases["background"]
        );
        assert_eq!(
            reloaded.plugins.aliases["unrelated"],
            if client == CLIENT { &expected } else { &saved }
                .plugins
                .aliases["unrelated"]
        );
        let mut declaration = reloaded.background_plugins.first().unwrap().clone();
        assert!(matches!(declaration, RunPluginOrAlias::Alias(_)));
        declaration.populate_run_plugin_if_needed(&reloaded.plugins);
        let run = declaration.get_run_plugin().unwrap();
        assert_eq!(run.location.display(), "file:/unsaved.scm");
        assert_eq!(run.initial_cwd, Some(PathBuf::from("/declaration cwd")));
        assert_eq!(run.configuration.inner()["inherited"], "alias value");
        assert_eq!(run.configuration.inner()["override"], "declaration value");
    }
    assert_eq!(configuration.change_saved_config(saved.clone()).len(), 2);
    for client in [CLIENT, other] {
        assert_eq!(configuration.get_client_configuration(&client), saved);
    }
    // Initialization also captures required aliases, even over a differing client config.
    configuration.set_saved_configuration(expected.clone());
    let current = configuration.get_client_configuration(&other);
    assert_eq!(current.background_plugins, expected.background_plugins);
    assert_eq!(
        current.background_plugins_cleared,
        expected.background_plugins_cleared
    );
    assert_eq!(
        current.plugins.aliases["background"],
        expected.plugins.aliases["background"]
    );
    assert_eq!(
        current.plugins.aliases["unrelated"],
        saved.plugins.aliases["unrelated"]
    );
}

#[test]
fn unsaved_background_clear_survives_attachment_keybinding_save_and_default_reload() {
    let saved = Config::from_default_assets().unwrap();
    assert!(!saved.background_plugins.is_empty());
    let other = ClientId::new(2);
    let mut configuration = SessionConfiguration::default();
    configuration.set_saved_configuration(saved.clone());
    configuration.set_client_runtime_configuration(CLIENT, saved.clone());
    configuration
        .reconfigure_runtime_config(
            &CLIENT,
            "load_plugins clear-defaults=true {}".into(),
            None,
            None,
        )
        .unwrap();
    configuration.set_client_runtime_configuration(other, saved.clone());
    let (rebound, changed) = configuration.rebind_keys(
        &other,
        vec![(
            InputMode::Normal,
            KeyWithModifier::from_str("Ctrl g").unwrap(),
            vec![Action::Quit],
        )],
        vec![],
    );
    assert!(changed);
    let rebound = rebound.unwrap();
    assert!(rebound.background_plugins.is_empty());
    assert!(rebound.background_plugins_cleared);
    let reloaded = Config::from_kdl(&rebound.to_string(false).unwrap(), Some(saved)).unwrap();
    assert!(reloaded.background_plugins_cleared);
    assert!(reloaded.background_plugins.is_empty());
}

#[test]
fn theme_save_escapes_names_preserves_other_settings_and_stages_until_written() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("config.kdl");
    std::fs::write(&main, "mouse_mode false\n").unwrap();
    let overlay = dir.path().join("config.d");
    Config::write_managed_fragment(&overlay, "pane_frames false\n").unwrap();
    let name = "custom \"quoted\" \\ theme";
    let mut config = Config::from_kdl("mouse_mode false\npane_frames false\n", None).unwrap();
    config.themes.insert(
        name.into(),
        yinz_config::theme::Theme {
            terminal: None,
            palette: Default::default(),
            sourced_from_external_file: true,
        },
    );
    let mut configuration = SessionConfiguration::default();
    configuration.set_client_runtime_configuration(CLIENT, config.clone());
    let saved = save_palette_theme(
        &configuration,
        CLIENT,
        name,
        Some(&main),
        Some(dir.path()),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(configuration.get_client_configuration(&CLIENT), config);
    assert_eq!(saved.options.theme.as_deref(), Some(name));
    assert_eq!(saved.options.theme_dark, None);
    assert_eq!(saved.options.theme_light, None);
    assert_eq!(saved.options.mouse_mode, Some(false));
    assert_eq!(std::fs::read_to_string(main).unwrap(), "mouse_mode false\n");
    let text = std::fs::read_to_string(overlay.join("yinz-managed.kdl")).unwrap();
    let loaded = Config::from_kdl(&text, Some(config)).unwrap();
    assert_eq!(loaded.options, saved.options);
    assert!(text.contains("pane_frames false"));
    assert!(!text.contains("keybinds"));
    assert!(!text.contains("themes {"));
}

#[test]
fn theme_save_preserves_configured_variants_after_reload() {
    for variants in [
        "",
        "theme_dark \"nightfox\"\n",
        "theme_light \"dayfox\"\n",
        "theme_dark \"nightfox\"\ntheme_light \"dayfox\"\n",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let themes = Config::from_kdl(
            include_str!("../../../yinz-config/assets/themes/dracula.kdl"),
            None,
        )
        .unwrap();
        let config = Config::from_kdl(variants, Some(themes)).unwrap();
        let mut configuration = SessionConfiguration::default();
        configuration.set_client_runtime_configuration(CLIENT, config.clone());
        let saved = save_palette_theme(
            &configuration,
            CLIENT,
            "dracula",
            None,
            Some(dir.path()),
            &Default::default(),
        )
        .unwrap();
        let text = std::fs::read_to_string(dir.path().join("config.d/yinz-managed.kdl")).unwrap();
        let loaded = Config::from_kdl(&text, Some(config.clone())).unwrap();
        assert_eq!(loaded.options, saved.options);
        assert_eq!(loaded.options.theme.as_deref(), Some("dracula"));
        assert_eq!(
            loaded.options.theme_dark.as_deref(),
            config.options.theme_dark.as_ref().map(|_| "dracula")
        );
        assert_eq!(
            loaded.options.theme_light.as_deref(),
            config.options.theme_light.as_ref().map(|_| "dracula")
        );
    }
}

#[test]
fn theme_save_rejects_stale_names_and_write_failure_without_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.themes.insert(
        "custom".into(),
        yinz_config::theme::Theme {
            terminal: None,
            palette: Default::default(),
            sourced_from_external_file: false,
        },
    );
    let mut configuration = SessionConfiguration::default();
    configuration.set_client_runtime_configuration(CLIENT, config.clone());
    assert!(save_palette_theme(
        &configuration,
        CLIENT,
        "stale",
        None,
        Some(dir.path()),
        &Default::default()
    )
    .is_err());
    assert!(save_palette_theme(
        &configuration,
        CLIENT,
        "custom",
        None,
        Some(dir.path()),
        &std::sync::atomic::AtomicBool::new(true)
    )
    .is_err());
    assert!(!dir.path().join("config.d").exists());
    std::fs::write(dir.path().join("config.d"), "not a directory").unwrap();
    assert!(save_palette_theme(
        &configuration,
        CLIENT,
        "custom",
        None,
        Some(dir.path()),
        &Default::default()
    )
    .is_err());
    assert_eq!(configuration.get_client_configuration(&CLIENT), config);
}

#[test]
fn detached_runtime_changes_accumulate_without_client_entries() {
    let mut configuration = SessionConfiguration::default();
    configuration
        .reconfigure_runtime_config(&ClientId::NONE, "mouse_mode false".into(), None, None)
        .unwrap();
    for key in ["Ctrl g", "Ctrl h"] {
        let (_, changed) = configuration.rebind_keys(
            &ClientId::NONE,
            vec![(InputMode::Normal, key.parse().unwrap(), vec![Action::NoOp])],
            vec![],
        );
        assert!(changed);
    }
    let (config, changed) = configuration
        .reconfigure_runtime_config(&ClientId::NONE, "pane_frames false".into(), None, None)
        .unwrap();
    assert!(changed);
    let config = config.unwrap();
    assert_eq!(config.options.mouse_mode, Some(false));
    assert_eq!(config.options.pane_frames, Some(false));
    assert_eq!(
        config.keybinds,
        normal_mode_keybinds(&[("Ctrl g", Action::NoOp), ("Ctrl h", Action::NoOp)])
    );
    assert_eq!(
        configuration.get_client_keybinds(&ClientId::NONE),
        &config.keybinds
    );
    assert!(configuration.runtime_config.is_empty());
    assert!(configuration.editor_effective.is_empty());
    assert!(configuration.effective_keybinds.is_empty());
    assert_eq!(
        configuration.get_client_configuration(&CLIENT),
        Config::default()
    );
}

#[test]
fn saved_configuration_resets_detached_overrides_even_if_disk_is_unchanged() {
    for disk_changed in [false, true] {
        let mut configuration = SessionConfiguration::default();
        configuration
            .reconfigure_runtime_config(&ClientId::NONE, "mouse_mode false".into(), None, None)
            .unwrap();
        let mut saved = Config::default();
        if disk_changed {
            saved.options.pane_frames = Some(false);
        }
        let changes = configuration.change_saved_config(saved.clone());
        assert_eq!(changes, vec![(ClientId::NONE, saved.clone())]);
        assert_eq!(
            configuration.get_client_configuration(&ClientId::NONE),
            saved
        );
        assert!(configuration.detached_runtime_config.is_none());
        assert!(configuration.change_saved_config(saved).is_empty());
    }
}

#[test]
fn the_last_client_leaves_a_baseline_without_leaving_its_identity() {
    let mut configuration = SessionConfiguration::default();
    let mut runtime = Config::default();
    runtime.options.mouse_mode = Some(false);
    configuration.set_client_runtime_configuration(CLIENT, runtime.clone());
    configuration.remove_client(CLIENT);
    assert!(configuration.runtime_config.is_empty());
    assert_eq!(
        configuration.get_client_configuration(&ClientId::NONE),
        runtime
    );
    assert_eq!(
        configuration.get_client_configuration(&CLIENT),
        Config::default()
    );
    configuration.set_client_runtime_configuration(ClientId::new(2), Config::default());
    configuration.remove_client(ClientId::new(2));
    assert_eq!(
        configuration.get_client_configuration(&ClientId::NONE),
        Config::default()
    );
}

/// Saved key rebindings: written as per-binding edits to `config.d/yinz-managed.kdl` over the
/// first-run bootstrap `config.kdl`, which is never rewritten.
mod rebind_saves {
    use super::*;
    use yinz_config::config::{FIRST_RUN_CONFIG, MANAGED_CONFIG_FILE_NAME};

    type PluginReconfigure = (ClientId, Option<Keybinds>, bool);
    type WriteFailure = (Option<PathBuf>, Option<String>);

    struct Fixture {
        dir: tempfile::TempDir,
        session: Arc<RwLock<Option<SessionMetaData>>>,
        state: Arc<RwLock<SessionState>>,
        screen: channels::Receiver<(ScreenInstruction, yinz_types::diagnostics::ErrorContext)>,
        plugin: channels::Receiver<(PluginInstruction, yinz_types::diagnostics::ErrorContext)>,
    }

    impl Fixture {
        /// A session over a first-run config directory, with `attached` clients attached.
        fn new(attached: &[ClientId]) -> Self {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("config.kdl"), FIRST_RUN_CONFIG).unwrap();
            let (to_screen, screen) = channels::unbounded();
            let (to_plugin, plugin) = channels::unbounded();
            let mut session = reconfiguration_session(ThreadSenders {
                to_screen: Some(SenderWithContext::new(to_screen)),
                to_plugin: Some(SenderWithContext::new(to_plugin)),
                should_silently_fail: true,
                ..Default::default()
            });
            session.config_dir = Some(dir.path().to_path_buf());
            let saved = Self::load(&dir);
            session
                .session_configuration
                .set_saved_configuration(saved.clone());
            let mut state = SessionState::new();
            for client in attached {
                state.clients.insert(*client);
                state.interactive_clients.insert(*client);
                session
                    .session_configuration
                    .set_client_runtime_configuration(*client, saved.clone());
            }
            Fixture {
                dir,
                session: Arc::new(RwLock::new(Some(session))),
                state: Arc::new(RwLock::new(state)),
                screen,
                plugin,
            }
        }

        /// What the config watcher loads after a write.
        fn load(dir: &tempfile::TempDir) -> Config {
            Config::load(
                Some(&dir.path().join("config.kdl")),
                Some(dir.path()),
                false,
            )
            .unwrap()
        }

        fn reload(&self) -> Config {
            Self::load(&self.dir)
        }

        fn managed(&self) -> PathBuf {
            self.dir
                .path()
                .join("config.d")
                .join(MANAGED_CONFIG_FILE_NAME)
        }

        fn with<R>(&self, f: impl FnOnce(&mut SessionMetaData) -> R) -> R {
            with_session_mut(&self.session, f).unwrap().unwrap()
        }

        fn client_config(&self, client: ClientId) -> Config {
            self.with(|session| {
                session
                    .session_configuration
                    .get_client_configuration(&client)
            })
        }

        fn rebind(
            &self,
            client: ClientId,
            rebind: Vec<(InputMode, KeyWithModifier, Vec<Action>)>,
            unbind: Vec<(InputMode, KeyWithModifier)>,
            save: bool,
        ) -> Result<()> {
            rebind_client_keys(client, rebind, unbind, save, &self.state, &self.session)
        }

        fn screen_reconfigures(&self) -> Vec<(ClientId, Keybinds)> {
            self.screen
                .try_iter()
                .filter_map(|(instruction, _)| match instruction {
                    ScreenInstruction::Reconfigure {
                        client_id,
                        keybinds,
                        ..
                    } => Some((client_id, keybinds)),
                    _ => None,
                })
                .collect()
        }

        /// The plugin `Reconfigure`s (client, keybinds, written to disk) and write failures.
        fn plugin_messages(&self) -> (Vec<PluginReconfigure>, Vec<WriteFailure>) {
            let mut reconfigures = vec![];
            let mut failures = vec![];
            for (instruction, _) in self.plugin.try_iter() {
                match instruction {
                    PluginInstruction::Reconfigure {
                        client_id,
                        keybinds,
                        was_written_to_disk,
                        ..
                    } => reconfigures.push((client_id, keybinds, was_written_to_disk)),
                    PluginInstruction::FailedToWriteConfigToDisk { file_path, reason } => {
                        failures.push((file_path, reason))
                    },
                    _ => {},
                }
            }
            (reconfigures, failures)
        }

        fn drain(&self) {
            while self.screen.try_recv().is_ok() {}
            while self.plugin.try_recv().is_ok() {}
        }
    }

    fn key(chord: &str) -> KeyWithModifier {
        KeyWithModifier::from_str(chord).unwrap()
    }

    fn bound(keybinds: &Keybinds, mode: InputMode, chord: &str) -> Option<Vec<Action>> {
        keybinds
            .get_actions_for_key_in_mode(&mode, &key(chord))
            .cloned()
    }

    #[test]
    fn a_saved_rebind_writes_only_binding_edits_and_reaches_the_client() {
        let client = ClientId::new(1);
        let fixture = Fixture::new(&[client]);
        let config_file = fixture.dir.path().join("config.kdl");
        fixture
            .rebind(
                client,
                vec![(InputMode::Normal, key("Ctrl Alt y"), vec![Action::Quit])],
                vec![],
                true,
            )
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(&config_file).unwrap(),
            FIRST_RUN_CONFIG
        );
        // One edits block holding the one binding: no defaults or other settings.
        let managed = std::fs::read_to_string(fixture.managed()).unwrap();
        assert!(
            managed
                .trim_start()
                .starts_with(yinz_config::editor::EDITS_BLOCK),
            "{managed}"
        );
        assert_eq!(managed.matches(yinz_config::editor::EDITS_BLOCK).count(), 1);
        assert_eq!(managed.matches("set ").count(), 1, "{managed}");
        assert_eq!(managed.matches("bind ").count(), 1, "{managed}");
        assert!(managed.contains("\"Ctrl Alt y\""), "{managed}");

        let reloaded = fixture.reload();
        assert_eq!(
            bound(&reloaded.keybinds, InputMode::Normal, "Ctrl Alt y"),
            Some(vec![Action::Quit])
        );
        assert_eq!(
            Config::load(None, Some(fixture.dir.path()), false)
                .unwrap()
                .keybinds,
            reloaded.keybinds
        );
        // The client's screen and plugins hear of the binding, and that it was saved.
        let screen = fixture.screen_reconfigures();
        assert!(
            screen.iter().any(|(id, keybinds)| *id == client
                && bound(keybinds, InputMode::Normal, "Ctrl Alt y").is_some()),
            "{screen:?}"
        );
        let (plugin, failures) = fixture.plugin_messages();
        assert!(failures.is_empty(), "{failures:?}");
        assert!(
            plugin.iter().any(|(id, keybinds, written)| *id == client
                && *written
                && keybinds
                    .as_ref()
                    .is_some_and(|k| bound(k, InputMode::Normal, "Ctrl Alt y").is_some())),
            "{plugin:?}"
        );
        // The config marked saved is the one on disk, so the watcher's reload changes nothing.
        let echo = fixture.with(|session| {
            assert_eq!(
                session.session_configuration.saved_config.keybinds,
                reloaded.keybinds
            );
            session.session_configuration.change_saved_config(reloaded)
        });
        assert!(echo.is_empty(), "{echo:?}");
    }

    /// The rebind screen applies new leader keys with Enter, unsaved, then saves with Ctrl+A a
    /// request built from the keymap it now sees. The save must record the keys the first
    /// apply unbound, or the watcher's reload brings them back.
    #[test]
    fn saving_after_an_unsaved_rebind_persists_the_whole_keybinding_difference() {
        let client = ClientId::new(1);
        let fixture = Fixture::new(&[client]);
        let defaults = fixture.reload().keybinds;
        let moved: Vec<_> = defaults
            .0
            .iter()
            .filter_map(|(mode, bindings)| {
                bindings
                    .get(&key("Ctrl g"))
                    .map(|actions| (*mode, actions.clone()))
            })
            .collect();
        assert!(moved.len() > 1, "Ctrl g is bound in several modes");
        fixture
            .rebind(
                client,
                moved
                    .iter()
                    .map(|(mode, actions)| (*mode, key("Ctrl Alt g"), actions.clone()))
                    .collect(),
                moved
                    .iter()
                    .map(|(mode, _)| (*mode, key("Ctrl g")))
                    .collect(),
                false,
            )
            .unwrap();
        reconfigure_client(
            client,
            "pane_frames false".into(),
            false,
            &fixture.state,
            &fixture.session,
        )
        .unwrap();
        assert!(!fixture.managed().exists(), "nothing was saved yet");
        fixture.drain();

        fixture
            .rebind(
                client,
                moved
                    .iter()
                    .map(|(mode, actions)| (*mode, key("Ctrl Alt g"), actions.clone()))
                    .collect(),
                moved
                    .iter()
                    .map(|(mode, _)| (*mode, key("Ctrl Alt g")))
                    .collect(),
                true,
            )
            .unwrap();

        let running = fixture.client_config(client);
        let reloaded = fixture.reload();
        assert_eq!(reloaded.keybinds, running.keybinds);
        for (mode, actions) in &moved {
            assert_eq!(bound(&reloaded.keybinds, *mode, "Ctrl g"), None);
            assert_eq!(
                bound(&reloaded.keybinds, *mode, "Ctrl Alt g"),
                Some(actions.clone())
            );
        }
        // The unsaved option stays applied, and unsaved.
        assert_eq!(running.options.pane_frames, Some(false));
        assert_ne!(reloaded.options.pane_frames, Some(false));
        assert!(!std::fs::read_to_string(fixture.managed())
            .unwrap()
            .contains("pane_frames"));
        let (plugin, _) = fixture.plugin_messages();
        assert!(
            plugin
                .iter()
                .any(|(id, _, written)| *id == client && *written),
            "{plugin:?}"
        );

        let echo = fixture.with(|session| {
            assert_eq!(
                session.session_configuration.saved_config.keybinds,
                running.keybinds
            );
            session.session_configuration.change_saved_config(reloaded)
        });
        assert!(echo.is_empty(), "the reload restored saved keys: {echo:?}");
        assert_eq!(fixture.client_config(client), running);
    }

    #[test]
    fn an_unwritable_action_refuses_the_save_and_changes_nothing() {
        let client = ClientId::new(1);
        let fixture = Fixture::new(&[client]);
        let before = fixture.with(|session| session.session_configuration.clone());
        let unwritable = Action::NewPane {
            direction: None,
            pane_name: Some("named".into()),
            start_suppressed: false,
        };
        let saved = fixture.rebind(
            client,
            vec![(InputMode::Normal, key("Ctrl Alt y"), vec![unwritable])],
            vec![],
            true,
        );
        assert!(saved.is_err());
        let (plugin, failures) = fixture.plugin_messages();
        assert!(plugin.is_empty(), "{plugin:?}");
        match failures.as_slice() {
            [(None, Some(reason))] => assert!(reason.contains("Ctrl Alt y"), "{reason}"),
            other => panic!("expected one reported failure, got {other:?}"),
        }
        assert!(fixture.screen_reconfigures().is_empty());
        assert!(!fixture.dir.path().join("config.d").exists());
        assert_eq!(
            std::fs::read_to_string(fixture.dir.path().join("config.kdl")).unwrap(),
            FIRST_RUN_CONFIG
        );
        fixture.with(|session| {
            let after = &session.session_configuration;
            assert_eq!(after.saved_config, before.saved_config);
            assert_eq!(after.runtime_config, before.runtime_config);
            assert_eq!(after.pending_editor_reload, None);
        });
    }

    /// As with any failed fragment save, the rebinding stays applied, unsaved.
    #[test]
    fn a_failed_write_keeps_the_rebinding_applied_and_unsaved() {
        let client = ClientId::new(1);
        let fixture = Fixture::new(&[client]);
        std::fs::create_dir(fixture.dir.path().join("config.d")).unwrap();
        std::fs::write(fixture.managed(), "configuration_edits {\n").unwrap();
        let saved_before =
            fixture.with(|session| session.session_configuration.saved_config.clone());

        let saved = fixture.rebind(
            client,
            vec![(InputMode::Normal, key("Ctrl Alt y"), vec![Action::Quit])],
            vec![],
            true,
        );
        assert!(saved.is_err());
        let (plugin, failures) = fixture.plugin_messages();
        match failures.as_slice() {
            [(Some(path), Some(reason))] => {
                assert_eq!(path, &fixture.managed());
                assert!(reason.contains("not valid KDL"), "{reason}");
            },
            other => panic!("expected one reported failure, got {other:?}"),
        }
        assert!(
            plugin.iter().any(|(id, keybinds, written)| *id == client
                && !*written
                && keybinds
                    .as_ref()
                    .is_some_and(|k| bound(k, InputMode::Normal, "Ctrl Alt y").is_some())),
            "{plugin:?}"
        );
        assert!(plugin.iter().all(|(_, _, written)| !*written));
        assert_eq!(
            std::fs::read_to_string(fixture.managed()).unwrap(),
            "configuration_edits {\n"
        );
        assert_eq!(
            bound(
                &fixture.client_config(client).keybinds,
                InputMode::Normal,
                "Ctrl Alt y"
            ),
            Some(vec![Action::Quit])
        );
        fixture.with(|session| {
            assert_eq!(session.session_configuration.saved_config, saved_before);
            assert_eq!(session.session_configuration.pending_editor_reload, None);
        });
    }

    /// A departed client's save while another client is attached writes the edits and gives
    /// the attached client the saved bindings, keeping its other runtime settings.
    #[test]
    fn a_departed_clients_saved_rebind_updates_the_attached_clients_bindings() {
        let attached = ClientId::new(1);
        let departed = ClientId::new(2);
        let fixture = Fixture::new(&[attached]);
        reconfigure_client(
            attached,
            "pane_frames false".into(),
            false,
            &fixture.state,
            &fixture.session,
        )
        .unwrap();
        fixture.drain();

        fixture
            .rebind(
                departed,
                vec![(InputMode::Normal, key("Ctrl Alt y"), vec![Action::Quit])],
                vec![],
                true,
            )
            .unwrap();

        assert_eq!(
            bound(&fixture.reload().keybinds, InputMode::Normal, "Ctrl Alt y"),
            Some(vec![Action::Quit])
        );
        let running = fixture.client_config(attached);
        assert_eq!(
            bound(&running.keybinds, InputMode::Normal, "Ctrl Alt y"),
            Some(vec![Action::Quit])
        );
        assert_eq!(running.options.pane_frames, Some(false));
        let screen = fixture.screen_reconfigures();
        assert!(
            screen.iter().all(|(id, _)| *id == attached),
            "only the attached client hears of it: {screen:?}"
        );
        assert!(!screen.is_empty());
        fixture.with(|session| {
            let configured: Vec<_> = session
                .session_configuration
                .runtime_config
                .keys()
                .copied()
                .collect();
            assert_eq!(configured, vec![attached]);
            assert_eq!(session.session_configuration.detached_runtime_config, None);
        });
    }

    fn binding_edit() -> String {
        yinz_config::editor::keybinding_edits(&[yinz_config::editor::KeybindingChange {
            mode: InputMode::Normal,
            key: key("Ctrl Alt y"),
            actions: Some(vec![Action::Quit]),
        }])
        .unwrap()
    }

    /// A session's own config dir is the target; the default dir is not even looked up.
    #[test]
    fn a_fragment_goes_to_the_sessions_config_dir() {
        let dir = tempfile::tempdir().unwrap();
        write_fragment_in_config_dir(
            Some(dir.path().to_path_buf()),
            || panic!("the default config dir was looked up"),
            &binding_edit(),
        )
        .unwrap();
        let managed = dir.path().join("config.d").join(MANAGED_CONFIG_FILE_NAME);
        assert!(std::fs::read_to_string(managed)
            .unwrap()
            .contains("\"Ctrl Alt y\""));
    }

    /// A `--config <file>` session has no config dir: the save goes to the (created) default
    /// one, which is where its next start and the watcher read `config.d` from, and the
    /// explicit file is left alone.
    #[test]
    fn an_explicit_config_sessions_fragment_goes_to_the_default_config_dir() {
        let explicit_dir = tempfile::tempdir().unwrap();
        let explicit = explicit_dir.path().join("my.kdl");
        std::fs::write(&explicit, "mouse_mode false\n").unwrap();
        let home = tempfile::tempdir().unwrap();
        let created = home.path().join("yinz");

        write_fragment_in_config_dir(
            None,
            || {
                std::fs::create_dir(&created).unwrap();
                Some(created.clone())
            },
            &binding_edit(),
        )
        .unwrap();

        assert!(created
            .join("config.d")
            .join(MANAGED_CONFIG_FILE_NAME)
            .exists());
        assert!(!explicit_dir.path().join("config.d").exists());
        assert_eq!(
            std::fs::read_to_string(&explicit).unwrap(),
            "mouse_mode false\n"
        );
        let next_start = Config::load(Some(&explicit), Some(&created), false).unwrap();
        assert_eq!(
            bound(&next_start.keybinds, InputMode::Normal, "Ctrl Alt y"),
            Some(vec![Action::Quit])
        );
        assert_eq!(next_start.options.mouse_mode, Some(false));
    }

    /// With no config dir to be found or made, the reason says what to do, and fits.
    #[test]
    fn a_fragment_with_no_config_dir_fails_with_an_actionable_reason() {
        let result = write_fragment_in_config_dir(None, || None, &binding_edit());
        assert_eq!(result, Err((None, NO_CONFIG_DIR_REASON.to_owned())));
        assert!(NO_CONFIG_DIR_REASON.contains("--config-dir"));
    }
}

mod ligature_reconfigure {
    use super::*;

    pub(super) struct Fixture {
        pub(super) dir: tempfile::TempDir,
        pub(super) data: Arc<RwLock<Option<SessionMetaData>>>,
        pub(super) state: Arc<RwLock<SessionState>>,
        pub(super) screen:
            channels::Receiver<(ScreenInstruction, yinz_types::diagnostics::ErrorContext)>,
        pub(super) plugin:
            channels::Receiver<(PluginInstruction, yinz_types::diagnostics::ErrorContext)>,
        pub(super) pty: channels::Receiver<(PtyInstruction, yinz_types::diagnostics::ErrorContext)>,
    }

    impl Fixture {
        pub(super) fn new(clients: &[ClientId], enabled: Option<bool>) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let main = dir.path().join("config.kdl");
            std::fs::write(&main, "// unchanged main\n").unwrap();
            let (screen_tx, screen) = channels::unbounded();
            let (plugin_tx, plugin) = channels::unbounded();
            let (pty_tx, pty) = channels::unbounded();
            let mut session = reconfiguration_session(ThreadSenders {
                to_screen: Some(SenderWithContext::new(screen_tx)),
                to_plugin: Some(SenderWithContext::new(plugin_tx)),
                to_pty: Some(SenderWithContext::new(pty_tx)),
                should_silently_fail: true,
                ..Default::default()
            });
            session.config_dir = Some(dir.path().to_path_buf());
            session.config_file_path = Some(main);
            let mut saved = Config::default();
            saved.terminal.font_ligatures = enabled;
            session
                .session_configuration
                .set_saved_configuration(saved.clone());
            let mut state = SessionState::new();
            for client in clients {
                state.clients.insert(*client);
                state.interactive_clients.insert(*client);
                session
                    .session_configuration
                    .set_client_runtime_configuration(*client, saved.clone());
            }
            session.session_configuration.pending_editor_reload = Some(saved);
            session.pending_mouse_updates.push((CLIENT, true));
            Self {
                dir,
                data: Arc::new(RwLock::new(Some(session))),
                state: Arc::new(RwLock::new(state)),
                screen,
                plugin,
                pty,
            }
        }

        pub(super) fn with<R>(&self, f: impl FnOnce(&mut SessionMetaData) -> R) -> R {
            with_session_mut(&self.data, f).unwrap().unwrap()
        }

        pub(super) fn snapshot(&self) -> String {
            self.with(|session| {
                let mut runtime = session.session_configuration.clone();
                runtime.startup_axes_save = None;
                format!("{runtime:?} {:?}", session.pending_mouse_updates)
            })
        }

        pub(super) fn all(
            &self,
            fragment: &str,
            write: bool,
        ) -> yinz_bus::route::ActionCompletionResult {
            let (tx, mut rx) = tokio::sync::oneshot::channel();
            reconfigure_all_clients(
                fragment.into(),
                write,
                Some(NotificationEnd::new(tx)),
                &self.state,
                &self.data,
            );
            rx.try_recv().unwrap()
        }

        pub(super) fn assert_no_propagation(&self) {
            self.assert_no_instructions();
            assert!(!self.dir.path().join("config.d").exists());
        }

        pub(super) fn assert_no_instructions(&self) {
            assert!(self.screen.is_empty());
            assert!(self.plugin.is_empty());
            assert!(self.pty.is_empty());
        }

        pub(super) fn assert_only_write_failure(&self, managed: &Path, error: &str) {
            assert!(self.screen.is_empty());
            assert!(self.pty.is_empty());
            let plugin: Vec<_> = self
                .plugin
                .try_iter()
                .map(|(instruction, _)| instruction)
                .collect();
            assert!(
                matches!(
                    plugin.as_slice(),
                    [PluginInstruction::FailedToWriteConfigToDisk {
                        file_path: Some(path),
                        reason: Some(reason),
                        ..
                    }] if path == managed && error.contains(reason)
                ),
                "{plugin:?}"
            );
        }

        pub(super) fn drain_instructions(
            &self,
        ) -> (Vec<ScreenInstruction>, Vec<PluginInstruction>) {
            self.pty.try_iter().for_each(drop);
            (
                self.screen
                    .try_iter()
                    .map(|(instruction, _)| instruction)
                    .collect(),
                self.plugin
                    .try_iter()
                    .map(|(instruction, _)| instruction)
                    .collect(),
            )
        }
    }

    fn guidance(error: &str) {
        for word in [
            "ligatures/font_ligatures",
            "--write",
            "configuration/drop-in",
            "entire fragment",
        ] {
            assert!(error.contains(word), "{error}");
        }
    }

    #[test]
    fn unsaved_changes_refuse_both_aliases_atomically_at_both_entry_points() {
        for clients in [vec![CLIENT], vec![]] {
            for enabled in [true, false] {
                for alias in ["ligatures", "font_ligatures"] {
                    let fixture = Fixture::new(&clients, Some(enabled));
                    let before = fixture.snapshot();
                    let fragment = format!(
                        "terminal {{ {alias} {}; }}\nmouse_mode false\nkeybinds {{ normal {{ bind \"Ctrl y\" {{ NewPane; }}; }}; }}\nplugins {{ background location=\"file:/background.scm\"; }}\nload_plugins {{ background; }}", !enabled
                    );
                    for _ in 0..2 {
                        let result = fixture.all(&fragment, false);
                        assert_eq!(result.exit_status, Some(1));
                        guidance(result.error_message.as_deref().unwrap());
                        assert_eq!(fixture.snapshot(), before);
                        fixture.assert_no_propagation();
                        let error = reconfigure_client(
                            CLIENT,
                            fragment.clone(),
                            false,
                            &fixture.state,
                            &fixture.data,
                        )
                        .unwrap_err();
                        guidance(&format!("{error:#}"));
                        assert_eq!(fixture.snapshot(), before);
                        fixture.assert_no_propagation();
                    }
                }
            }
        }
    }

    #[test]
    fn differing_client_later_in_batch_refuses_without_installing_earlier_changes() {
        let fixture = Fixture::new(&[CLIENT, ClientId::new(2)], Some(true));
        let order = fixture.state.read().unwrap().interactive_client_ids();
        fixture.with(|session| {
            let mut first = session
                .session_configuration
                .get_client_configuration(&order[0]);
            first.terminal.font_ligatures = Some(false);
            session
                .session_configuration
                .set_client_runtime_configuration(order[0], first);
        });
        let before = fixture.snapshot();
        let result = fixture.all("terminal { ligatures false; }\nmouse_mode false", false);
        assert_eq!(result.exit_status, Some(1));
        guidance(result.error_message.as_deref().unwrap());
        assert_eq!(fixture.snapshot(), before);
        fixture.assert_no_propagation();
    }

    #[test]
    fn equivalent_defaults_aliases_and_omissions_keep_runtime_changes_available() {
        for clients in [vec![CLIENT], vec![]] {
            for (enabled, fragment) in [
                (None, "terminal { ligatures true; }"),
                (None, "terminal { font_ligatures true; }"),
                (Some(false), "terminal { ligatures false; }"),
                (Some(false), "terminal { font_ligatures false; }"),
                (Some(false), "terminal { padding 0; }"),
                (Some(true), "pane_frames false"),
            ] {
                for all_clients in [true, false] {
                    let fixture = Fixture::new(&clients, enabled);
                    let fragment = format!("{fragment}\nmouse_mode false");
                    if all_clients {
                        let result = fixture.all(&fragment, false);
                        assert_eq!(result.exit_status, None, "{result:?}");
                        assert_eq!(result.error_message, None);
                    } else {
                        reconfigure_client(CLIENT, fragment, false, &fixture.state, &fixture.data)
                            .unwrap();
                    }
                    let client = if clients.is_empty() {
                        ClientId::NONE
                    } else {
                        CLIENT
                    };
                    fixture.with(|session| {
                        let config = session
                            .session_configuration
                            .get_client_configuration(&client);
                        assert_eq!(config.options.mouse_mode, Some(false));
                        assert_eq!(
                            config.terminal.font_ligatures.unwrap_or(true),
                            enabled.unwrap_or(true)
                        );
                    });
                    assert!(!fixture.dir.path().join("config.d").exists());
                }
            }
        }
    }

    #[test]
    fn parse_failures_leave_full_session_unchanged_and_saved_changes_report_write_outcomes() {
        for alias in ["ligatures", "font_ligatures"] {
            for all_clients in [true, false] {
                let fixture = Fixture::new(&[CLIENT], None);
                let before = fixture.snapshot();
                let invalid = format!("terminal {{ {alias} \"false\"; }}\nmouse_mode false");
                if all_clients {
                    let result = fixture.all(&invalid, false);
                    assert_eq!(result.exit_status, Some(1));
                } else {
                    assert!(reconfigure_client(
                        CLIENT,
                        invalid,
                        false,
                        &fixture.state,
                        &fixture.data
                    )
                    .is_err());
                }
                assert_eq!(fixture.snapshot(), before);
                fixture.assert_no_propagation();
                let fragment = format!("terminal {{ {alias} false; }}");
                if all_clients {
                    assert_eq!(fixture.all(&fragment, true).exit_status, None);
                } else {
                    reconfigure_client(CLIENT, fragment, true, &fixture.state, &fixture.data)
                        .unwrap();
                }
                let managed = fixture.dir.path().join("config.d/yinz-managed.kdl");
                let text = std::fs::read_to_string(managed).unwrap();
                assert_eq!(
                    Config::from_kdl(&text, None)
                        .unwrap()
                        .terminal
                        .font_ligatures,
                    Some(false)
                );
                assert_eq!(
                    std::fs::read_to_string(fixture.dir.path().join("config.kdl")).unwrap(),
                    "// unchanged main\n"
                );

                let failed = Fixture::new(&[CLIENT], None);
                std::fs::write(failed.dir.path().join("config.d"), "not a directory").unwrap();
                let fragment = format!("terminal {{ {alias} false; }}");
                if all_clients {
                    let result = failed.all(&fragment, true);
                    assert_eq!(result.exit_status, Some(1));
                    assert!(result.error_message.is_some());
                } else {
                    assert!(reconfigure_client(
                        CLIENT,
                        fragment,
                        true,
                        &failed.state,
                        &failed.data
                    )
                    .is_err());
                }
                assert_eq!(
                    std::fs::read_to_string(failed.dir.path().join("config.d")).unwrap(),
                    "not a directory"
                );
            }
        }
    }

    #[test]
    fn departed_client_unsaved_requests_keep_existing_drop_policy() {
        let fixture = Fixture::new(&[ClientId::new(2)], None);
        let before = fixture.snapshot();
        reconfigure_client(
            CLIENT,
            "terminal { ligatures false; }".into(),
            false,
            &fixture.state,
            &fixture.data,
        )
        .unwrap();
        assert_eq!(fixture.snapshot(), before);
        fixture.assert_no_propagation();
    }
}

mod baseline_offset_reconfigure {
    use super::ligature_reconfigure::Fixture;
    use super::*;

    fn guidance(error: &str, settings: &[&str]) {
        for word in
            settings
                .iter()
                .copied()
                .chain(["--write", "configuration/drop-in", "entire fragment"])
        {
            assert!(error.contains(word), "{error}");
        }
    }

    #[test]
    fn unsaved_metrics_and_combined_ligatures_refuse_whole_fragment_at_both_entry_points() {
        for clients in [vec![CLIENT], vec![]] {
            for (settings, names) in [
                ("cell_width 1.5;", vec!["cell_width"]),
                ("line_height 1.5;", vec!["line_height"]),
                ("baseline_offset 2;", vec!["baseline_offset"]),
                ("baseline_offset -2;", vec!["baseline_offset"]),
                (
                    "cell_width 1.5; line_height 0.75; baseline_offset 2; ligatures false;",
                    vec![
                        "cell_width",
                        "line_height",
                        "baseline_offset",
                        "ligatures/font_ligatures",
                    ],
                ),
            ] {
                let fixture = Fixture::new(&clients, None);
                let before = fixture.snapshot();
                let fragment = format!("terminal {{ {settings} }}\nmouse_mode false\nkeybinds {{ normal {{ bind \"Ctrl y\" {{ NewPane; }}; }}; }}\nplugins {{ background location=\"file:/background.scm\"; }}\nload_plugins {{ background; }}");
                let result = fixture.all(&fragment, false);
                assert_eq!(result.exit_status, Some(1));
                guidance(result.error_message.as_deref().unwrap(), &names);
                assert_eq!(fixture.snapshot(), before);
                fixture.assert_no_propagation();
                let error =
                    reconfigure_client(CLIENT, fragment, false, &fixture.state, &fixture.data)
                        .unwrap_err();
                guidance(&format!("{error:#}"), &names);
                assert_eq!(fixture.snapshot(), before);
                fixture.assert_no_propagation();
            }
        }
    }

    #[test]
    fn all_clients_use_each_current_override_and_reject_without_partial_mutation() {
        for (metric, value) in [
            ("cell_width", "1.5"),
            ("line_height", "1.5"),
            ("baseline_offset", "-2"),
        ] {
            let fixture = Fixture::new(&[CLIENT, ClientId::new(2)], None);
            let order = fixture.state.read().unwrap().interactive_client_ids();
            fixture.with(|session| {
                let config = session
                    .session_configuration
                    .get_client_configuration(&order[0]);
                let config =
                    Config::from_kdl(&format!("terminal {{ {metric} {value}; }}"), Some(config))
                        .unwrap();
                session
                    .session_configuration
                    .set_client_runtime_configuration(order[0], config);
            });
            let before = fixture.snapshot();
            let result = fixture.all(
                &format!("terminal {{ {metric} {value}; }}\nmouse_mode false"),
                false,
            );
            assert_eq!(result.exit_status, Some(1));
            guidance(result.error_message.as_deref().unwrap(), &[metric]);
            assert_eq!(fixture.snapshot(), before);
            fixture.assert_no_propagation();
        }
    }

    #[test]
    fn omitted_equivalent_and_explicit_default_metrics_allow_unrelated_runtime_changes() {
        for clients in [vec![CLIENT], vec![]] {
            for current in ["", "cell_width 1.5; line_height 0.75; baseline_offset -2;"] {
                for all in [false, true] {
                    for requested in [
                        "",
                        current,
                        if current.is_empty() {
                            "cell_width 1; line_height 1; baseline_offset 0;"
                        } else {
                            current
                        },
                    ] {
                        let fixture = Fixture::new(&clients, None);
                        fixture.with(|session| {
                            let config =
                                Config::from_kdl(&format!("terminal {{ {current} }}"), None)
                                    .unwrap();
                            session
                                .session_configuration
                                .set_saved_configuration(config.clone());
                            if !clients.is_empty() {
                                session
                                    .session_configuration
                                    .set_client_runtime_configuration(CLIENT, config);
                            }
                        });
                        let fragment = format!("terminal {{ {requested} }}\nmouse_mode false");
                        if all {
                            let result = fixture.all(&fragment, false);
                            assert_eq!(result.exit_status, None, "{result:?}");
                        } else {
                            reconfigure_client(
                                CLIENT,
                                fragment,
                                false,
                                &fixture.state,
                                &fixture.data,
                            )
                            .unwrap();
                        }
                        fixture.with(|session| {
                            let config =
                                session
                                    .session_configuration
                                    .get_client_configuration(&if clients.is_empty() {
                                        ClientId::NONE
                                    } else {
                                        CLIENT
                                    });
                            assert_eq!(config.options.mouse_mode, Some(false));
                            assert_eq!(
                                config.terminal.baseline_offset.unwrap_or(0),
                                if current.is_empty() { 0 } else { -2 }
                            );
                            assert_eq!(
                                config.terminal.cell_width.unwrap_or(1.0),
                                if current.is_empty() { 1.0 } else { 1.5 }
                            );
                            assert_eq!(
                                config.terminal.line_height.unwrap_or(1.0),
                                if current.is_empty() { 1.0 } else { 0.75 }
                            );
                        });
                    }
                }
            }
        }
    }

    #[test]
    fn saved_metrics_persist_reload_and_surface_parse_and_write_failures() {
        for all in [false, true] {
            for (metric, value) in [
                ("cell_width", "1.5"),
                ("line_height", "1.5"),
                ("baseline_offset", "-2"),
            ] {
                let fixture = Fixture::new(&[CLIENT], None);
                let before = fixture.snapshot();
                let bad = if metric == "baseline_offset" {
                    "65"
                } else {
                    "4"
                };
                let invalid = format!("terminal {{ {metric} {bad}; }}\nmouse_mode false");
                assert_eq!(fixture.all(&invalid, false).exit_status, Some(1));
                assert_eq!(fixture.snapshot(), before);
                fixture.assert_no_propagation();
                let fragment = format!("terminal {{ {metric} {value}; }}");
                if all {
                    assert_eq!(fixture.all(&fragment, true).exit_status, None);
                } else {
                    reconfigure_client(
                        CLIENT,
                        fragment.clone(),
                        true,
                        &fixture.state,
                        &fixture.data,
                    )
                    .unwrap();
                }
                let path = fixture.dir.path().join("config.d/yinz-managed.kdl");
                let saved =
                    Config::from_kdl(&std::fs::read_to_string(path).unwrap(), None).unwrap();
                let loaded =
                    Config::default().with_overlay_dir(Some(&fixture.dir.path().join("config.d")));
                assert_eq!(loaded.terminal, saved.terminal);
                assert_eq!(
                    serde_json::to_value(&loaded.terminal).unwrap()[metric],
                    value.parse::<f64>().unwrap()
                );
                assert_eq!(
                    std::fs::read_to_string(fixture.dir.path().join("config.kdl")).unwrap(),
                    "// unchanged main\n"
                );
                fixture.with(|session| {
                    session
                        .session_configuration
                        .change_saved_config(loaded.clone());
                    assert_eq!(
                        session
                            .session_configuration
                            .get_client_configuration(&CLIENT)
                            .terminal,
                        loaded.terminal
                    );
                });
                let failed = Fixture::new(&[CLIENT], None);
                std::fs::write(failed.dir.path().join("config.d"), "not a directory").unwrap();
                if all {
                    let result = failed.all(&fragment, true);
                    assert_eq!(result.exit_status, Some(1));
                    assert!(result.error_message.is_some());
                } else {
                    assert!(reconfigure_client(
                        CLIENT,
                        fragment,
                        true,
                        &failed.state,
                        &failed.data
                    )
                    .is_err());
                }
                assert_eq!(
                    std::fs::read_to_string(failed.dir.path().join("config.d")).unwrap(),
                    "not a directory"
                );
            }
        }
    }
    #[test]
    fn baseline_offset_unsaved_reset_and_clamped_values_still_require_saving() {
        for current in [-64, -2, 2, 64] {
            for clients in [vec![CLIENT], vec![]] {
                let fixture = Fixture::new(&clients, None);
                fixture.with(|session| {
                    let config = Config::from_kdl(
                        &format!("terminal {{ baseline_offset {current}; }}"),
                        None,
                    )
                    .unwrap();
                    session
                        .session_configuration
                        .set_saved_configuration(config.clone());
                    if !clients.is_empty() {
                        session
                            .session_configuration
                            .set_client_runtime_configuration(CLIENT, config);
                    }
                });
                let before = fixture.snapshot();
                for requested in [0, -current] {
                    let fragment =
                        format!("terminal {{ baseline_offset {requested}; }}\nmouse_mode false");
                    assert_eq!(fixture.all(&fragment, false).exit_status, Some(1));
                    assert!(reconfigure_client(
                        CLIENT,
                        fragment,
                        false,
                        &fixture.state,
                        &fixture.data
                    )
                    .is_err());
                    assert_eq!(fixture.snapshot(), before);
                    fixture.assert_no_propagation();
                }
            }
        }
    }

    #[test]
    fn baseline_offset_saved_sign_zero_and_invalid_fragments_at_both_entry_points() {
        for all in [false, true] {
            let fixture = Fixture::new(&[CLIENT], None);
            let request = |fragment: &str, write| {
                if all {
                    fixture.all(fragment, write).exit_status != Some(1)
                } else {
                    reconfigure_client(
                        CLIENT,
                        fragment.to_owned(),
                        write,
                        &fixture.state,
                        &fixture.data,
                    )
                    .is_ok()
                }
            };
            for bad in ["65", "-65", "2.0", "true", "(int)2"] {
                for write in [false, true] {
                    let before = fixture.snapshot();
                    assert!(!request(
                        &format!("terminal {{ baseline_offset {bad}; }}\nmouse_mode false"),
                        write
                    ));
                    assert_eq!(fixture.snapshot(), before);
                    fixture.assert_no_propagation();
                    assert!(!fixture
                        .dir
                        .path()
                        .join("config.d/yinz-managed.kdl")
                        .exists());
                }
            }
            for offset in [2, -2, 0] {
                assert!(request(
                    &format!("terminal {{ baseline_offset {offset}; }}"),
                    true
                ));
                let loaded =
                    Config::default().with_overlay_dir(Some(&fixture.dir.path().join("config.d")));
                assert_eq!(loaded.terminal.baseline_offset, Some(offset));
                fixture.with(|session| {
                    session
                        .session_configuration
                        .change_saved_config(loaded.clone());
                    assert_eq!(
                        session
                            .session_configuration
                            .get_client_configuration(&CLIENT)
                            .terminal
                            .baseline_offset,
                        Some(offset)
                    );
                });
            }
            assert_eq!(
                std::fs::read_to_string(fixture.dir.path().join("config.kdl")).unwrap(),
                "// unchanged main\n"
            );
        }
    }
}

mod padding_reconfigure {
    use super::ligature_reconfigure::Fixture;
    use super::*;

    fn with_current(clients: &[ClientId], current: &str) -> Fixture {
        let fixture = Fixture::new(clients, None);
        fixture.with(|session| {
            let config = Config::from_kdl(
                &format!("terminal {{ {current} }}"),
                Some(Config::default()),
            )
            .unwrap();
            session
                .session_configuration
                .set_saved_configuration(config.clone());
            for client in clients {
                session
                    .session_configuration
                    .set_client_runtime_configuration(*client, config.clone());
            }
            session.session_configuration.pending_editor_reload = Some(config);
        });
        fixture
    }

    /// The refused names, in order, from the save guidance.
    fn refused(error: &str) -> Vec<String> {
        for word in ["--write", "configuration/drop-in", "entire fragment"] {
            assert!(error.contains(word), "{error}");
        }
        let names = error
            .split("changing terminal ")
            .nth(1)
            .and_then(|rest| rest.split(" requires a saved configuration").next())
            .unwrap_or_else(|| panic!("{error}"));
        names.split(", ").map(str::to_owned).collect()
    }

    #[test]
    fn unsaved_padding_changes_refuse_whole_fragment_at_both_entry_points() {
        for clients in [vec![CLIENT], vec![]] {
            for (current, settings, names) in [
                ("", "padding 3;", vec!["padding"]),
                ("", "padding_left 2;", vec!["padding_left"]),
                ("padding 4;", "padding_top 1;", vec!["padding_top"]),
                // Sides from the shorthand change; the explicit left side does not.
                ("padding_left 2;", "padding 5;", vec!["padding"]),
                (
                    "padding 4; padding_left 6;",
                    "padding_left 0;",
                    vec!["padding_left"],
                ),
                ("", "padding_balance true;", vec!["padding_balance"]),
                (
                    "padding_balance true;",
                    "padding_balance false;",
                    vec!["padding_balance"],
                ),
                (
                    "",
                    "padding 2; padding_right 1; padding_balance true;",
                    vec!["padding", "padding_right", "padding_balance"],
                ),
                ("", "padding_color \"extend\";", vec!["padding_color"]),
                (
                    "padding_color \"extend\";",
                    "padding_color \"background\";",
                    vec!["padding_color"],
                ),
                (
                    "padding_color \"extend\";",
                    "padding_color \"Background\";",
                    vec!["padding_color"],
                ),
                (
                    "",
                    "padding 2; padding_color \"extend\";",
                    vec!["padding", "padding_color"],
                ),
                ("", "minimum_contrast 4.5;", vec!["minimum_contrast"]),
                (
                    "minimum_contrast 4.5;",
                    "minimum_contrast 7;",
                    vec!["minimum_contrast"],
                ),
                (
                    "minimum_contrast 4.5;",
                    "minimum_contrast 1;",
                    vec!["minimum_contrast"],
                ),
                (
                    "",
                    "padding 2; minimum_contrast 4.5;",
                    vec!["padding", "minimum_contrast"],
                ),
            ] {
                let fixture = with_current(&clients, current);
                let before = fixture.snapshot();
                let fragment = format!("terminal {{ {settings} }}\nmouse_mode false\nkeybinds {{ normal {{ bind \"Ctrl y\" {{ NewPane; }}; }}; }}");
                let result = fixture.all(&fragment, false);
                assert_eq!(result.exit_status, Some(1), "{current} -> {settings}");
                assert_eq!(refused(result.error_message.as_deref().unwrap()), names);
                assert_eq!(fixture.snapshot(), before);
                fixture.assert_no_propagation();
                let error =
                    reconfigure_client(CLIENT, fragment, false, &fixture.state, &fixture.data)
                        .unwrap_err();
                assert_eq!(refused(&format!("{error:#}")), names);
                assert_eq!(fixture.snapshot(), before);
                fixture.assert_no_propagation();
            }
        }
    }

    #[test]
    fn unsaved_equivalent_or_omitted_padding_keeps_runtime_changes_available() {
        for clients in [vec![CLIENT], vec![]] {
            for (current, requested) in [
                ("", ""),
                ("", "padding 0;"),
                ("", "padding_top 0; padding_balance false;"),
                ("padding 4;", "padding_left 4;"),
                ("padding 4;", "padding 4; padding_balance false;"),
                (
                    "padding_left 2; padding_balance true;",
                    "padding_left 2; padding_balance true;",
                ),
                ("padding 4; padding_left 6;", "padding 4;"),
                ("", "padding_color \"background\";"),
                ("padding_color \"extend\";", "padding_color \"extend\";"),
                ("padding_color \"extend\";", "padding_color \"EXTEND\";"),
                ("padding_color \"extend\";", "padding 0;"),
                ("", "minimum_contrast 1;"),
                ("", "minimum_contrast 1.0;"),
                ("minimum_contrast 4.5;", "minimum_contrast 4.5;"),
                ("minimum_contrast 4.5;", "padding 0;"),
            ] {
                for all in [true, false] {
                    let fixture = with_current(&clients, current);
                    let fragment = format!("terminal {{ {requested} }}\nmouse_mode false");
                    if all {
                        let result = fixture.all(&fragment, false);
                        assert_eq!(
                            result.exit_status, None,
                            "{current} -> {requested}: {result:?}"
                        );
                    } else {
                        reconfigure_client(CLIENT, fragment, false, &fixture.state, &fixture.data)
                            .unwrap();
                    }
                    let client = if clients.is_empty() {
                        ClientId::NONE
                    } else {
                        CLIENT
                    };
                    fixture.with(|session| {
                        let config = session
                            .session_configuration
                            .get_client_configuration(&client);
                        assert_eq!(config.options.mouse_mode, Some(false));
                    });
                    assert!(!fixture.dir.path().join("config.d").exists());
                }
            }
        }
    }

    #[test]
    fn written_padding_changes_save_instead_of_unsaved_refusal() {
        for all in [true, false] {
            let fixture = with_current(&[CLIENT], "padding 4;");
            let fragment = "terminal { padding_left 6; padding_balance true; \
                            padding_color \"extend\"; minimum_contrast 4.5; }";
            if all {
                assert_eq!(fixture.all(fragment, true).exit_status, None);
            } else {
                reconfigure_client(CLIENT, fragment.into(), true, &fixture.state, &fixture.data)
                    .unwrap();
            }
            let managed = fixture.dir.path().join("config.d/yinz-managed.kdl");
            let saved = Config::from_kdl(&std::fs::read_to_string(managed).unwrap(), None).unwrap();
            assert_eq!(saved.terminal.padding_left, Some(6.0));
            assert_eq!(saved.terminal.padding_balance, Some(true));
            assert_eq!(
                saved.terminal.padding_color,
                Some(yinz_types::input::native_terminal::PaddingColor::Extend)
            );
            assert_eq!(saved.terminal.minimum_contrast, Some(4.5));
            assert_eq!(
                std::fs::read_to_string(fixture.dir.path().join("config.kdl")).unwrap(),
                "// unchanged main\n"
            );
        }
    }
}

mod font_features_reconfigure {
    use super::ligature_reconfigure::Fixture;
    use super::*;

    const SIBLINGS: &str = "mouse_mode false\nkeybinds { normal { bind \"Ctrl y\" { NewPane; }; }; }\nplugins { background location=\"file:/background.scm\"; }\nload_plugins { background; }";

    fn guidance(error: &str) {
        for word in [
            "font_features",
            "--write",
            "configuration/drop-in",
            "entire fragment",
        ] {
            assert!(error.contains(word), "{error}");
        }
    }

    fn with_current(clients: &[ClientId], current: &str) -> Fixture {
        let fixture = Fixture::new(clients, None);
        fixture.with(|session| {
            let config = Config::from_kdl(&format!("terminal {{ {current} }}"), None).unwrap();
            session
                .session_configuration
                .set_saved_configuration(config.clone());
            for client in clients {
                session
                    .session_configuration
                    .set_client_runtime_configuration(*client, config.clone());
            }
        });
        fixture
    }

    fn client_of(clients: &[ClientId]) -> ClientId {
        if clients.is_empty() {
            ClientId::NONE
        } else {
            CLIENT
        }
    }

    #[test]
    fn unsaved_feature_changes_refuse_whole_fragment_at_both_entry_points() {
        for clients in [vec![CLIENT], vec![]] {
            for (current, requested) in [
                ("", "font_features \"ss01\";"),
                ("font_features;", "font_features \"-calt\";"),
                ("font_features \"ss01\";", "font_features;"),
                ("font_features \"ss01\";", "font_features \"ss02\";"),
                (
                    "font_features \"ss01\" \"zero\";",
                    "font_features \"zero\" \"ss01\";",
                ),
                ("font_features \"calt\";", "font_features \"-calt\";"),
                (
                    "font_features \"ss01\";",
                    "font_features \"ss01\" \"ss01\";",
                ),
            ] {
                let fixture = with_current(&clients, current);
                let before = fixture.snapshot();
                let fragment = format!("terminal {{ {requested} }}\n{SIBLINGS}");
                for _ in 0..2 {
                    let result = fixture.all(&fragment, false);
                    assert_eq!(result.exit_status, Some(1), "{current} -> {requested}");
                    guidance(result.error_message.as_deref().unwrap());
                    assert_eq!(fixture.snapshot(), before);
                    fixture.assert_no_propagation();
                    let error = reconfigure_client(
                        CLIENT,
                        fragment.clone(),
                        false,
                        &fixture.state,
                        &fixture.data,
                    )
                    .unwrap_err();
                    guidance(&format!("{error:#}"));
                    assert_eq!(fixture.snapshot(), before);
                    fixture.assert_no_propagation();
                }
            }
        }
        // Combined native changes name every refused setting.
        let fixture = with_current(&[CLIENT], "");
        let result = fixture.all(
            "terminal { ligatures false; cell_width 1.5; font_features \"ss01\"; }",
            false,
        );
        let error = result.error_message.unwrap();
        for word in ["ligatures/font_ligatures", "cell_width", "font_features"] {
            assert!(error.contains(word), "{error}");
        }
    }

    #[test]
    fn omitted_and_equivalent_feature_lists_keep_runtime_changes_available() {
        for clients in [vec![CLIENT], vec![]] {
            for (current, requested) in [
                ("", ""),
                ("", "font_features;"),
                ("font_features;", ""),
                ("font_features \"ss01\";", ""),
                ("font_features \"ss01\";", "font_features \"ss01\";"),
                ("font_features \"ss01\";", "font_features \"+ss01\";"),
                (
                    "font_features \"+calt\" \"-zero\";",
                    "font_features \"calt\" \"-zero\";",
                ),
                ("font_features \"ss01\";", "padding 0;"),
            ] {
                for all in [true, false] {
                    let fixture = with_current(&clients, current);
                    let fragment = format!("terminal {{ {requested} }}\nmouse_mode false");
                    if all {
                        let result = fixture.all(&fragment, false);
                        assert_eq!(
                            result.exit_status, None,
                            "{current} -> {requested}: {result:?}"
                        );
                    } else {
                        reconfigure_client(CLIENT, fragment, false, &fixture.state, &fixture.data)
                            .unwrap();
                    }
                    fixture.with(|session| {
                        let config = session
                            .session_configuration
                            .get_client_configuration(&client_of(&clients));
                        assert_eq!(config.options.mouse_mode, Some(false));
                    });
                    assert!(!fixture.dir.path().join("config.d").exists());
                }
            }
        }
    }

    #[test]
    fn saved_feature_lists_persist_and_invalid_lists_change_nothing() {
        for all in [false, true] {
            let fixture = Fixture::new(&[CLIENT], None);
            let before = fixture.snapshot();
            let invalid = "terminal { font_features \"ss1\"; }\nmouse_mode false";
            let result = fixture.all(invalid, false);
            assert_eq!(result.exit_status, Some(1));
            let message = result.error_message.unwrap();
            assert!(
                message.contains("\"ss1\" is not a font feature"),
                "{message}"
            );
            let error =
                reconfigure_client(CLIENT, invalid.into(), true, &fixture.state, &fixture.data)
                    .unwrap_err();
            let error = format!("{error:#}");
            assert!(error.contains("\"ss1\" is not a font feature"), "{error}");
            assert_eq!(fixture.snapshot(), before);
            fixture.assert_no_propagation();
            let path = fixture.dir.path().join("config.d/yinz-managed.kdl");
            for (fragment, expected) in [
                (
                    "terminal { font_features \"ss01\" \"zero\" \"-calt\"; padding 2; }",
                    vec!["ss01", "zero", "-calt"],
                ),
                ("terminal { font_features; }", vec![]),
            ] {
                if all {
                    assert_eq!(fixture.all(fragment, true).exit_status, None);
                } else {
                    reconfigure_client(
                        CLIENT,
                        fragment.into(),
                        true,
                        &fixture.state,
                        &fixture.data,
                    )
                    .unwrap();
                }
                let text = std::fs::read_to_string(&path).unwrap();
                assert_eq!(text.matches("font_features").count(), 1, "{text}");
                let saved = Config::from_kdl(&text, None).unwrap();
                let expected: Vec<String> = expected.iter().map(|tag| tag.to_string()).collect();
                assert_eq!(saved.terminal.font_features, Some(expected));
                assert_eq!(saved.terminal.padding, Some(2.0));
                let loaded =
                    Config::default().with_overlay_dir(Some(&fixture.dir.path().join("config.d")));
                assert_eq!(loaded.terminal, saved.terminal);
            }
            assert_eq!(
                std::fs::read_to_string(fixture.dir.path().join("config.kdl")).unwrap(),
                "// unchanged main\n"
            );
        }
    }
}

/// The session theme source behind shared application color answers changes only on
/// session-wide paths. See `docs/TERMINAL_COLORS.md`.
mod application_theme {
    use super::*;
    use yinz_config::screen_config::ApplicationThemeConfig;

    struct Fixture {
        dir: tempfile::TempDir,
        data: Arc<RwLock<Option<SessionMetaData>>>,
        state: Arc<RwLock<SessionState>>,
        screen: channels::Receiver<(ScreenInstruction, yinz_types::diagnostics::ErrorContext)>,
    }

    fn load(dir: &Path) -> Config {
        yinz_config::cli_assets::load_config_and_layout(&yinz_types::input::cli_assets::CliAssets {
            config_file_path: Some(dir.join("config.kdl")),
            config_dir: Some(dir.into()),
            ..Default::default()
        })
        .unwrap()
        .0
    }

    fn theme(config: &Config, kdl: &str) -> ApplicationThemeConfig {
        ApplicationThemeConfig::from_config(&Config::from_kdl(kdl, Some(config.clone())).unwrap())
    }

    impl Fixture {
        /// A session created with `theme "nightfox"`. Every attached client brings its own
        /// `theme "dayfox"` and light hue, merged the way the attach path merges them, so
        /// whichever client a batch picks first holds those options.
        fn new(attached: &[ClientId]) -> Self {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("config.kdl"), "theme \"nightfox\"\n").unwrap();
            let (to_screen, screen) = channels::unbounded();
            let mut session = reconfiguration_session(ThreadSenders {
                to_screen: Some(SenderWithContext::new(to_screen)),
                should_silently_fail: true,
                ..Default::default()
            });
            session.config_dir = Some(dir.path().to_path_buf());
            session.config_file_path = Some(dir.path().join("config.kdl"));
            let saved = load(dir.path());
            session
                .session_configuration
                .set_saved_configuration(saved.clone());
            session
                .session_configuration
                .initialize_application_theme(saved.clone());
            let mut state = SessionState::new();
            for client in attached {
                state.clients.insert(*client);
                state.interactive_clients.insert(*client);
                let mut runtime = saved.clone();
                runtime.options = saved.options.merge(yinz_types::input::options::Options {
                    theme: Some("dayfox".into()),
                    explicit_theme_hue: Some(yinz_types::style::ThemeHue::Light),
                    ..Default::default()
                });
                session
                    .session_configuration
                    .set_client_runtime_configuration(*client, runtime);
                let effective = session
                    .session_configuration
                    .get_client_configuration(client);
                session.propagate_configuration_changes(vec![(*client, effective)], false);
            }
            let fixture = Fixture {
                dir,
                data: Arc::new(RwLock::new(Some(session))),
                state: Arc::new(RwLock::new(state)),
                screen,
            };
            assert!(
                fixture.published().is_empty(),
                "attaching with its own theme options must not change the session theme"
            );
            fixture
        }

        fn with<R>(&self, f: impl FnOnce(&mut SessionMetaData) -> R) -> R {
            with_session_mut(&self.data, f).unwrap().unwrap()
        }

        fn instructions(&self) -> Vec<ScreenInstruction> {
            self.screen
                .try_iter()
                .map(|(instruction, _)| instruction)
                .collect()
        }

        fn published(&self) -> Vec<ApplicationThemeConfig> {
            self.instructions()
                .into_iter()
                .filter_map(|instruction| match instruction {
                    ScreenInstruction::SetApplicationThemeConfig(config) => Some(config),
                    _ => None,
                })
                .collect()
        }

        /// The session theme is published exactly once, ahead of every `Reconfigure`.
        fn assert_published_once_ahead_of_reconfigure(&self, expected: &ApplicationThemeConfig) {
            let instructions = self.instructions();
            let published: Vec<_> = instructions
                .iter()
                .enumerate()
                .filter_map(|(index, instruction)| match instruction {
                    ScreenInstruction::SetApplicationThemeConfig(config) => Some((index, config)),
                    _ => None,
                })
                .collect();
            assert_eq!(published.len(), 1, "{instructions:?}");
            assert_eq!(published[0].1, expected);
            assert!(instructions[..published[0].0]
                .iter()
                .all(|instruction| !matches!(instruction, ScreenInstruction::Reconfigure { .. })));
        }

        fn all(&self, fragment: &str, write: bool) -> yinz_bus::route::ActionCompletionResult {
            let (tx, mut rx) = tokio::sync::oneshot::channel();
            reconfigure_all_clients(
                fragment.into(),
                write,
                Some(NotificationEnd::new(tx)),
                &self.state,
                &self.data,
            );
            rx.try_recv().unwrap()
        }

        fn source(&self) -> ApplicationThemeConfig {
            self.with(|session| {
                session
                    .session_configuration
                    .application_theme_configuration()
            })
        }

        /// What `ServerInstruction::ConfigWrittenToDisk` does with the watcher's load.
        fn disk_reload(&self) {
            let disk = load(self.dir.path());
            self.with(|session| {
                let changes = session.session_configuration.reload_saved_config(disk);
                session.propagate_configuration_changes(changes, true);
            });
        }
    }

    #[test]
    fn application_theme_ignores_attach_and_unsaved_per_client_reconfigure() {
        let fixture = Fixture::new(&[CLIENT, ClientId::new(2)]);
        let before = fixture.source();
        reconfigure_client(
            ClientId::new(2),
            "theme \"dracula\"".into(),
            false,
            &fixture.state,
            &fixture.data,
        )
        .unwrap();
        let instructions = fixture.instructions();
        assert!(instructions
            .iter()
            .any(|instruction| matches!(instruction, ScreenInstruction::Reconfigure { .. })));
        assert!(!instructions
            .iter()
            .any(|i| matches!(i, ScreenInstruction::SetApplicationThemeConfig(_))));
        assert_eq!(fixture.source(), before);
    }

    #[test]
    fn application_theme_follows_session_wide_unsaved_reconfigure() {
        let fixture = Fixture::new(&[CLIENT, ClientId::new(2)]);
        let saved = load(fixture.dir.path());
        assert_eq!(fixture.all("theme \"dayfox\"", false).exit_status, None);
        fixture.assert_published_once_ahead_of_reconfigure(&theme(&saved, "theme \"dayfox\""));
        // Repeating it changes nothing.
        fixture.all("theme \"dayfox\"", false);
        assert!(fixture.published().is_empty());

        // With nobody attached, a client's unsaved change is session-wide.
        let fixture = Fixture::new(&[]);
        reconfigure_client(
            CLIENT,
            "theme \"dracula\"".into(),
            false,
            &fixture.state,
            &fixture.data,
        )
        .unwrap();
        fixture.assert_published_once_ahead_of_reconfigure(&theme(&saved, "theme \"dracula\""));
    }

    #[test]
    fn application_theme_takes_only_saved_fragments_and_disk_reloads() {
        let fixture = Fixture::new(&[CLIENT, ClientId::new(2)]);
        let saved = load(fixture.dir.path());
        let before = fixture.source();
        // A saved unrelated change from a client holding its own theme options.
        assert_eq!(fixture.all("pane_frames false", true).exit_status, None);
        assert!(fixture.published().is_empty());
        reconfigure_client(
            ClientId::new(2),
            "mouse_mode false".into(),
            true,
            &fixture.state,
            &fixture.data,
        )
        .unwrap();
        assert!(fixture.published().is_empty());
        assert_eq!(fixture.source(), before);
        // The watcher's reload of those writes still has the session's theme.
        fixture.disk_reload();
        assert!(fixture.published().is_empty());
        assert_eq!(fixture.source(), before);

        // A saved theme fragment is session-wide.
        assert_eq!(fixture.all("theme \"dayfox\"", true).exit_status, None);
        fixture.assert_published_once_ahead_of_reconfigure(&theme(&saved, "theme \"dayfox\""));
        fixture.disk_reload();
        assert!(fixture.published().is_empty());
    }

    #[test]
    fn application_theme_disk_reload_replaces_the_creators_options_once() {
        let fixture = Fixture::new(&[CLIENT]);
        let saved = load(fixture.dir.path());
        // The creating client started the session with its own theme option.
        fixture.with(|session| {
            session.session_configuration.initialize_application_theme(
                Config::from_kdl("theme \"dracula\"", Some(saved.clone())).unwrap(),
            )
        });
        std::fs::write(fixture.dir.path().join("config.kdl"), "theme \"dayfox\"\n").unwrap();
        fixture.disk_reload();
        fixture.assert_published_once_ahead_of_reconfigure(&theme(&saved, "theme \"dayfox\""));
        fixture.disk_reload();
        assert!(fixture.published().is_empty());
    }

    #[test]
    fn application_theme_failed_write_leaves_the_source_unchanged() {
        let fixture = Fixture::new(&[CLIENT]);
        let before = fixture.source();
        // A config dir that is a file cannot hold `config.d`.
        let blocked = fixture.dir.path().join("not-a-directory");
        std::fs::write(&blocked, "").unwrap();
        fixture.with(|session| session.config_dir = Some(blocked));
        let result = fixture.all("theme \"dayfox\"", true);
        assert_eq!(result.exit_status, Some(1));
        assert!(fixture.published().is_empty());
        assert_eq!(fixture.source(), before);
    }

    #[test]
    fn underline_metrics_deferred_save_publishes_theme_only_after_success() {
        for all in [false, true] {
            let fixture = Fixture::new(&[CLIENT]);
            let saved = load(fixture.dir.path());
            let before = fixture.source();
            let drop_ins = fixture.dir.path().join("config.d");
            std::fs::write(&drop_ins, "blocked").unwrap();
            let fragment = "theme \"dayfox\"\nterminal { underline_offset -2; }";
            let try_save = || {
                if all {
                    fixture.all(fragment, true).exit_status.is_none()
                } else {
                    reconfigure_client(CLIENT, fragment.into(), true, &fixture.state, &fixture.data)
                        .is_ok()
                }
            };
            assert!(!try_save());
            assert_eq!(fixture.source(), before);
            assert!(fixture.instructions().is_empty());
            std::fs::remove_file(drop_ins).unwrap();
            assert!(try_save());
            fixture.assert_published_once_ahead_of_reconfigure(&theme(&saved, "theme \"dayfox\""));
        }
    }
}

mod underline_metrics_reconfigure {
    use super::ligature_reconfigure::Fixture;
    use super::*;
    use yinz_types::data::LayoutInfo;

    fn request(
        fixture: &Fixture,
        client: ClientId,
        all: bool,
        fragment: &str,
        write: bool,
    ) -> std::result::Result<(), String> {
        if all {
            let result = fixture.all(fragment, write);
            match result.exit_status {
                None => Ok(()),
                Some(1) => Err(result.error_message.unwrap()),
                status => panic!("unexpected exit status {status:?}"),
            }
        } else {
            reconfigure_client(
                client,
                fragment.into(),
                write,
                &fixture.state,
                &fixture.data,
            )
            .map(|_| ())
            .map_err(|error| format!("{error:#}"))
        }
    }

    fn load(fixture: &Fixture) -> Config {
        Config::load(
            Some(&fixture.dir.path().join("config.kdl")),
            Some(fixture.dir.path()),
            false,
        )
        .unwrap()
    }

    fn managed(fixture: &Fixture) -> PathBuf {
        fixture.dir.path().join("config.d/yinz-managed.kdl")
    }

    fn assert_refused(fixture: &Fixture, all: bool, fragment: &str, names: &[&str]) {
        let before = fixture.snapshot();
        let error = request(fixture, CLIENT, all, fragment, false).unwrap_err();
        for word in names.iter().copied().chain(["--write", "entire fragment"]) {
            assert!(error.contains(word), "{error}");
        }
        assert_eq!(fixture.snapshot(), before);
        fixture.assert_no_instructions();
    }

    #[cfg(unix)]
    #[test]
    fn underline_metrics_reconfigure_ignores_unused_switch_swap() {
        use std::{fs, process::Command};

        let clients = [CLIENT, ClientId::new(2)];
        for all in [false, true] {
            for input in ["regular", "absent", "fifo"] {
                let fixture = Fixture::new(&clients, None);
                let layouts = fixture.dir.path().join("layouts");
                fs::create_dir(&layouts).unwrap();
                let layout = layouts.join("switch.kdl");
                let swap = layout.with_extension("swap.kdl");
                fs::write(&layout, r#"layout { tab name="Primary" { pane; }; }"#).unwrap();
                match input {
                    "regular" => fs::write(&swap, "swap_tiled_layout { tab { pane; }; }").unwrap(),
                    "fifo" => assert!(Command::new("mkfifo")
                        .arg(&swap)
                        .status()
                        .unwrap()
                        .success()),
                    _ => {},
                }
                let fragment = format!(
                    "layout_dir {:?}\ntheme \"dayfox\"\nterminal {{ underline_thickness 3; }}\n\
                     keybinds {{ normal {{ bind \"Ctrl y\" {{ SwitchSession {{ name \"other\"; layout \"switch\"; }}; }}; }}; }}",
                    layouts.to_str().unwrap(),
                );
                let assert_config = |config: &Config| {
                    assert_eq!(config.terminal.underline_thickness, Some(3));
                    assert_eq!(config.options.theme.as_deref(), Some("dayfox"));
                    let actions =
                        &config.keybinds.0[&InputMode::Normal][&"Ctrl y".parse().unwrap()];
                    let [Action::SwitchSession {
                        name,
                        layout: Some(LayoutInfo::File(path, metadata)),
                        ..
                    }] = actions.as_slice()
                    else {
                        panic!("expected an on-disk SwitchSession binding, got {actions:?}");
                    };
                    assert_eq!(name.as_str(), "other");
                    assert_eq!(Path::new(path), layout);
                    assert_eq!(metadata.tabs.len(), 1);
                    assert_eq!(metadata.tabs[0].name.as_deref(), Some("Primary"));
                };

                assert_refused(&fixture, all, &fragment, &["underline_thickness"]);
                assert!(!managed(&fixture).exists());
                request(&fixture, CLIENT, all, &fragment, true).unwrap();
                assert_config(&load(&fixture));
                fixture.with(|session| {
                    let config = &session.session_configuration;
                    assert_config(config.application_theme_source());
                    assert_config(&config.get_client_configuration(&CLIENT));
                    if all {
                        assert_config(&config.get_client_configuration(&clients[1]));
                    }
                });
                request(&fixture, CLIENT, all, "mouse_mode false", false).unwrap();
            }
        }
    }

    #[test]
    fn layout_values_cannot_authorize_unsaved_underline_metrics_at_either_entry_point() {
        let fixture = Fixture::new(&[CLIENT, ClientId::new(2)], None);
        let saved = load(&fixture);
        let (_, layout) = yinz_config::layout::from_layout_info_with_config(
            &None,
            &LayoutInfo::Stringified(
                "layout { pane; }\nterminal { underline_offset -2; underline_thickness 3; }".into(),
            ),
            Some(saved.clone()),
        )
        .unwrap();
        assert_eq!(
            (
                saved.terminal.underline_offset,
                saved.terminal.underline_thickness
            ),
            (None, None)
        );
        assert_eq!(
            (
                layout.terminal.underline_offset,
                layout.terminal.underline_thickness
            ),
            (Some(-2), Some(3))
        );
        fixture.with(|session| {
            session
                .session_configuration
                .set_saved_configuration(layout.clone());
            for client in [CLIENT, ClientId::new(2)] {
                session
                    .session_configuration
                    .set_client_runtime_configuration(client, layout.clone());
            }
        });
        for all in [false, true] {
            for (settings, names) in [
                ("underline_offset -2;", &["underline_offset"][..]),
                ("underline_thickness 3;", &["underline_thickness"][..]),
                (
                    "underline_offset -2; underline_thickness 3;",
                    &["underline_offset", "underline_thickness"][..],
                ),
            ] {
                assert_refused(&fixture, all, &format!(
                    "terminal {{ {settings} }}\nmouse_mode false\nkeybinds {{ normal {{ bind \"Ctrl y\" {{ NewPane; }}; }}; }}\nplugins {{ background location=\"file:/background.scm\"; }}\nload_plugins {{ background; }}"
                ), names);
            }
        }
        assert!(!managed(&fixture).exists());
        // A successful saved comparison authorizes the explicitly supplied zero only.
        request(
            &fixture,
            CLIENT,
            true,
            "terminal { underline_offset 0; }\nmouse_mode false",
            false,
        )
        .unwrap();
        fixture.with(|session| {
            for client in [CLIENT, ClientId::new(2)] {
                let terminal = session
                    .session_configuration
                    .get_client_configuration(&client)
                    .terminal;
                assert_eq!(
                    (terminal.underline_offset, terminal.underline_thickness),
                    (Some(0), Some(3))
                );
            }
        });
    }

    #[test]
    fn masked_managed_underline_metrics_compare_against_winning_disk_values() {
        for all in [false, true] {
            let fixture = Fixture::new(&[CLIENT], None);
            let overlays = fixture.dir.path().join("config.d");
            std::fs::create_dir(&overlays).unwrap();
            std::fs::write(
                overlays.join("zz-winning.kdl"),
                "terminal { underline_offset -64; underline_thickness 0; }",
            )
            .unwrap();
            let fragment = "terminal { underline_offset -63; underline_thickness 1; }";
            request(&fixture, CLIENT, all, fragment, true).unwrap();
            let before = std::fs::read_to_string(managed(&fixture)).unwrap();
            let managed_terminal = Config::from_kdl(&before, None).unwrap().terminal;
            assert_eq!(
                (
                    managed_terminal.underline_offset,
                    managed_terminal.underline_thickness
                ),
                (Some(-63), Some(1))
            );
            let disk = load(&fixture).terminal;
            assert_eq!(
                (disk.underline_offset, disk.underline_thickness),
                (Some(-64), Some(0))
            );
            fixture.with(|session| {
                let runtime = session
                    .session_configuration
                    .get_client_configuration(&CLIENT);
                assert_eq!(runtime.terminal, managed_terminal);
            });
            fixture.drain_instructions();
            assert_refused(
                &fixture,
                all,
                &format!("{fragment}\nmouse_mode false"),
                &["underline_offset", "underline_thickness"],
            );
            assert_refused(
                &fixture,
                all,
                "terminal { underline_offset 0; }",
                &["underline_offset"],
            );
            assert_eq!(std::fs::read_to_string(managed(&fixture)).unwrap(), before);
            request(
                &fixture,
                CLIENT,
                all,
                "terminal { underline_offset -64; }\nmouse_mode false",
                false,
            )
            .unwrap();
        }
    }

    #[test]
    fn failed_underline_metrics_save_is_atomic_even_when_runtime_already_matches() {
        for all in [false, true] {
            for existing_runtime in [false, true] {
                let fixture = Fixture::new(&[CLIENT], None);
                let fragment = "terminal { underline_thickness 2; }\nmouse_mode false";
                if existing_runtime {
                    fixture.with(|session| {
                        let mut config = session
                            .session_configuration
                            .get_client_configuration(&CLIENT);
                        config.terminal.underline_thickness = Some(2);
                        session
                            .session_configuration
                            .set_client_runtime_configuration(CLIENT, config);
                    });
                }
                let path = fixture.dir.path().join("config.d");
                std::fs::write(&path, "not a directory").unwrap();
                let before = fixture.snapshot();
                request(&fixture, CLIENT, all, fragment, true).unwrap_err();
                assert_eq!(fixture.snapshot(), before);
                assert_eq!(std::fs::read_to_string(&path).unwrap(), "not a directory");
                let (screen, plugin) = fixture.drain_instructions();
                assert!(screen.is_empty());
                assert!(matches!(
                    plugin.as_slice(),
                    [PluginInstruction::FailedToWriteConfigToDisk {
                        reason: Some(_),
                        ..
                    }]
                ));
                assert_refused(&fixture, all, fragment, &["underline_thickness"]);

                std::fs::remove_file(path).unwrap();
                request(&fixture, CLIENT, all, fragment, true).unwrap();
                assert_eq!(load(&fixture).terminal.underline_thickness, Some(2));
                let (screen, _) = fixture.drain_instructions();
                assert!(screen.iter().any(|instruction| matches!(instruction, ScreenInstruction::Reconfigure { client_id, .. } if *client_id == CLIENT)));
                request(&fixture, CLIENT, all, fragment, false).unwrap();
            }
        }
    }

    #[test]
    fn underline_metrics_saved_layer_errors_and_invalid_fragments_preserve_working_state() {
        for all in [false, true] {
            let fixture = Fixture::new(&[], None);
            let main = fixture.dir.path().join("config.kdl");
            std::fs::write(&main, "terminal { underline_offset 65; }").unwrap();
            let before = fixture.snapshot();
            let good = "terminal { underline_thickness 1; }";
            let error = request(&fixture, CLIENT, all, good, false).unwrap_err();
            for expected in [
                "underline_thickness",
                "underline_offset",
                "Repair or restore",
                "--write",
                "entire fragment",
            ] {
                assert!(error.contains(expected), "{error}");
            }
            let invalid = "terminal { underline_thickness 2.0; }\nmouse_mode false";
            for write in [false, true] {
                assert!(request(&fixture, CLIENT, all, invalid, write)
                    .unwrap_err()
                    .contains("underline_thickness"));
                assert_eq!(fixture.snapshot(), before);
                fixture.assert_no_propagation();
            }
            // Requests with no underline keys don't consult the broken file.
            request(&fixture, CLIENT, all, "mouse_mode false", false).unwrap();
            request(&fixture, CLIENT, all, good, true).unwrap();
            assert!(managed(&fixture).exists());
            assert!(Config::load(Some(&main), Some(fixture.dir.path()), false).is_err());
            std::fs::write(&main, "// restored").unwrap();
            assert_eq!(load(&fixture).terminal.underline_thickness, Some(1));
            request(&fixture, CLIENT, all, good, false).unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn underline_metrics_managed_save_rejects_fifo_atomically() {
        use std::os::unix::{
            ffi::OsStrExt,
            fs::{FileTypeExt, OpenOptionsExt},
        };
        use std::sync::mpsc;

        for all in [false, true] {
            for fifo_lock in [false, true] {
                let fixture = Fixture::new(&[CLIENT, ClientId::new(2)], None);
                let managed = managed(&fixture);
                let lock = managed.with_file_name(".yinz-managed.kdl.lock");
                let fifo = if fifo_lock { &lock } else { &managed };
                let main = fixture.dir.path().join("config.kdl");
                let main_before = std::fs::read(&main).unwrap();
                let original = "terminal { underline_thickness 3; }";
                std::fs::create_dir(managed.parent().unwrap()).unwrap();
                if fifo_lock {
                    std::fs::write(&managed, original).unwrap();
                }
                let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
                let before = fixture.snapshot();
                let fragment =
                    "theme \"dayfox\"\nterminal { underline_offset -2; }\nmouse_mode false";
                let fixture_ref = &fixture;
                let error = std::thread::scope(|scope| {
                    let (send, result) = mpsc::channel();
                    let worker = scope.spawn(move || {
                        send.send(request(fixture_ref, CLIENT, all, fragment, true))
                            .unwrap();
                    });
                    let outcome = result.recv_timeout(yinz_test_support::default_timeout());
                    let mut lock_reader = None;
                    if matches!(outcome, Err(mpsc::RecvTimeoutError::Timeout)) {
                        eprintln!("saved reconfigure blocked on {fifo:?}, all clients: {all}");
                        let mut options = std::fs::OpenOptions::new();
                        options.custom_flags(libc::O_NONBLOCK);
                        if fifo_lock {
                            // Keep the reader through join, including if the worker
                            // has yet to enter the write-only open.
                            lock_reader = Some(options.read(true).open(fifo).unwrap());
                        } else {
                            // A blocked read needs a writer that immediately sends EOF.
                            let _ = options.write(true).open(fifo);
                        }
                    }
                    worker.join().unwrap();
                    drop(lock_reader);
                    outcome
                        .expect("saved reconfigure must return promptly")
                        .unwrap_err()
                });
                let detail = if fifo_lock {
                    "regular lock file"
                } else {
                    "not a regular file"
                };
                for expected in [fifo.to_str().unwrap(), detail, "then retry"] {
                    assert!(error.contains(expected), "{error}");
                }
                assert_eq!(fixture.snapshot(), before);
                assert_eq!(std::fs::read(&main).unwrap(), main_before);
                assert!(fifo.symlink_metadata().unwrap().file_type().is_fifo());
                if fifo_lock {
                    assert_eq!(std::fs::read_to_string(&managed).unwrap(), original);
                }
                fixture.assert_only_write_failure(&managed, &error);

                std::fs::remove_file(fifo).unwrap();
                if !fifo_lock {
                    std::fs::write(&managed, original).unwrap();
                }
                request(&fixture, CLIENT, all, fragment, true).unwrap();
                let saved = load(&fixture);
                assert_eq!(saved.terminal.underline_offset, Some(-2));
                assert_eq!(saved.terminal.underline_thickness, Some(3));
                assert_eq!(saved.options.mouse_mode, Some(false));
                assert_eq!(saved.options.theme.as_deref(), Some("dayfox"));
                assert!(lock.symlink_metadata().unwrap().is_file());
            }
        }
    }

    #[test]
    fn underline_metrics_saved_reconfigure_identifies_invalid_saved_file_and_key() {
        let fragment = "terminal { underline_offset -2; }\nmouse_mode false";
        for all in [false, true] {
            for (write, edits) in [(false, false), (true, false), (true, true)] {
                let fixture = Fixture::new(&[CLIENT], None);
                let main = fixture.dir.path().join("config.kdl");
                let managed = managed(&fixture);
                let bad_path = if write { &managed } else { &main };
                if write {
                    std::fs::create_dir(managed.parent().unwrap()).unwrap();
                }
                let mut bad = "terminal { underline_thickness 65; padding 4; }\n".to_owned();
                if edits {
                    bad.push_str(r#"configuration_edits { remove "themes" "unused"; }"#);
                }
                std::fs::write(bad_path, &bad).unwrap();
                let before = fixture.snapshot();
                let error = request(&fixture, CLIENT, all, fragment, write).unwrap_err();
                for expected in [
                    bad_path.to_str().unwrap(),
                    "underline_thickness",
                    "between -64 and 64",
                ] {
                    assert!(error.contains(expected), "{error}");
                }
                assert_eq!(fixture.snapshot(), before);
                assert_eq!(std::fs::read_to_string(bad_path).unwrap(), bad);
                if write {
                    assert!(error.contains("then retry"), "{error}");
                    fixture.assert_only_write_failure(&managed, &error);
                } else {
                    for expected in [
                        "underline_offset",
                        "Repair or restore",
                        "--write",
                        "attempt",
                        "entire fragment",
                    ] {
                        assert!(error.contains(expected), "{error}");
                    }
                    fixture.assert_no_propagation();
                }

                std::fs::write(bad_path, bad.replace("65", "3")).unwrap();
                request(&fixture, CLIENT, all, fragment, true).unwrap();
                let saved = load(&fixture);
                assert_eq!(saved.terminal.underline_offset, Some(-2));
                assert_eq!(saved.terminal.underline_thickness, Some(3));
                assert_eq!(saved.terminal.padding, Some(4.0));
                assert_eq!(saved.options.mouse_mode, Some(false));
            }
        }
    }

    #[test]
    fn departed_client_underline_metrics_can_only_be_saved() {
        let fixture = Fixture::new(&[CLIENT], None);
        let before = fixture.snapshot();
        let fragment = "terminal { underline_offset -2; }\nmouse_mode false";
        let departed = ClientId::new(9);
        request(&fixture, departed, false, fragment, false).unwrap();
        assert_eq!(fixture.snapshot(), before);
        fixture.assert_no_propagation();
        request(&fixture, departed, false, fragment, true).unwrap();
        assert_eq!(load(&fixture).terminal.underline_offset, Some(-2));
    }

    #[cfg(unix)]
    #[test]
    fn underline_metrics_reject_a_saved_main_replaced_by_a_symlink_to_a_fifo() {
        use std::os::unix::{ffi::OsStrExt, fs::symlink};
        let fixture = Fixture::new(&[CLIENT], None);
        let dir = fixture.dir.path();
        let fifo = dir.join("pipe");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let main = dir.join("config.kdl");
        std::fs::remove_file(&main).unwrap();
        symlink(&fifo, &main).unwrap();
        for all in [false, true] {
            let before = fixture.snapshot();
            let error = request(
                &fixture,
                CLIENT,
                all,
                "terminal { underline_offset 0; }",
                false,
            )
            .unwrap_err();
            assert!(
                error.contains("not a regular file") && error.contains("--write"),
                "{error}"
            );
            assert_eq!(fixture.snapshot(), before);
            fixture.assert_no_propagation();
            request(&fixture, CLIENT, all, "mouse_mode false", false).unwrap();
            fixture.drain_instructions();
        }
    }

    #[cfg(unix)]
    #[test]
    fn underline_metrics_refuse_an_unsaved_reset_when_theme_preload_is_indeterminate() {
        use std::os::unix::ffi::OsStrExt;

        for all in [false, true] {
            let fixture = Fixture::new(&[CLIENT], None);
            let root = fixture.dir.path();
            let main = root.join("config.kdl");
            let themes = root.join("themes");
            let overlays = root.join("config.d");
            std::fs::create_dir(&themes).unwrap();
            std::fs::create_dir(&overlays).unwrap();
            let theme = themes.join("fixture.kdl");
            let theme_text = include_str!("../../../yinz-config/assets/themes/adwaita.kdl");
            std::fs::write(&theme, theme_text).unwrap();
            let managed = managed(&fixture);
            let managed_text = "terminal { padding 4; }";
            std::fs::write(&managed, managed_text).unwrap();
            let winning = overlays.join("zz-theme-edit.kdl");
            let winning_text = r#"
                configuration_edits { remove "themes" "unrelated"; }
                terminal { underline_thickness 2; }
            "#;
            std::fs::write(&winning, winning_text).unwrap();
            let main_text = std::fs::read_to_string(&main).unwrap();
            let saved = load(&fixture);
            assert_eq!(saved.terminal.underline_thickness, Some(2));
            fixture.with(|session| {
                session
                    .session_configuration
                    .set_client_runtime_configuration(CLIENT, saved);
            });

            std::fs::remove_file(&theme).unwrap();
            let fifo = std::ffi::CString::new(theme.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
            assert_refused(
                &fixture,
                all,
                "terminal { underline_thickness 0; }\nmouse_mode false",
                &[
                    "underline_thickness",
                    "Repair or restore",
                    theme.to_str().unwrap(),
                ],
            );
            assert_eq!(std::fs::read_to_string(&main).unwrap(), main_text);
            assert_eq!(std::fs::read_to_string(&managed).unwrap(), managed_text);
            assert_eq!(std::fs::read_to_string(&winning).unwrap(), winning_text);

            std::fs::remove_file(&theme).unwrap();
            std::fs::write(&theme, theme_text).unwrap();
            request(
                &fixture,
                CLIENT,
                all,
                "terminal { underline_thickness 2; }\nmouse_mode false",
                false,
            )
            .unwrap();
        }
    }
}

#[test]
fn native_startup_dimensions_unsaved_requires_write() {
    use ligature_reconfigure::Fixture;

    for clients in [&[CLIENT][..], &[][..]] {
        for all in [false, true] {
            let fixture = Fixture::new(clients, None);
            let main = fixture.dir.path().join("config.kdl");
            std::fs::write(&main, "terminal { initial_columns 80; }").unwrap();
            let active = clients.first().copied().unwrap_or(ClientId::NONE);
            fixture.with(|session| {
                let mut layout_config = session
                    .session_configuration
                    .get_client_configuration(&active);
                layout_config.terminal.initial_columns = Some(100);
                session
                    .session_configuration
                    .set_client_runtime_configuration(active, layout_config);
            });
            let apply = |fragment: &str, write| {
                if all {
                    fixture
                        .all(fragment, write)
                        .error_message
                        .map_or(Ok(()), Err)
                } else {
                    reconfigure_client(
                        CLIENT,
                        fragment.into(),
                        write,
                        &fixture.state,
                        &fixture.data,
                    )
                    .map(|_| ())
                    .map_err(|error| format!("{error:#}"))
                }
            };
            let before = fixture.snapshot();
            let equivalent = "terminal { initial_columns 100; }\nmouse_mode false";
            for (fragment, keys) in [
                (equivalent, &["initial_columns"][..]),
                ("terminal { initial_rows 40; }", &["initial_rows"][..]),
                (
                    "terminal { initial_columns 120; initial_rows 40; }",
                    &["initial_columns", "initial_rows"][..],
                ),
            ] {
                let error = apply(fragment, false).unwrap_err();
                for key in keys.iter().copied().chain([
                    "--write",
                    "configuration/drop-in",
                    "entire fragment",
                ]) {
                    assert!(error.contains(key), "{error}");
                }
                assert_eq!(fixture.snapshot(), before);
                fixture.assert_no_propagation();
            }
            let error =
                apply("terminal { initial_rows 1001; }\nmouse_mode false", false).unwrap_err();
            assert!(error.contains("between 2 and 1000"), "{error}");
            let blocked = fixture.dir.path().join("config.d");
            std::fs::write(&blocked, "not a directory").unwrap();
            assert!(apply("terminal { initial_columns 100; }", true).is_err());
            assert!(apply(equivalent, false).is_err());
            assert_eq!(fixture.snapshot(), before);
            assert!(matches!(
                fixture.plugin.try_recv().unwrap().0,
                PluginInstruction::FailedToWriteConfigToDisk { .. }
            ));
            std::fs::remove_file(&blocked).unwrap();

            apply("terminal { initial_columns 100; }", true).unwrap();
            let loaded = Config::load(Some(&main), Some(fixture.dir.path()), false).unwrap();
            assert_eq!(loaded.terminal.initial_columns, Some(100));
            assert_eq!(fixture.snapshot(), before);
            apply("terminal { ligatures true; }\nmouse_mode false", false).unwrap();
            fixture.with(|session| {
                assert_eq!(
                    session
                        .session_configuration
                        .get_client_configuration(&active)
                        .options
                        .mouse_mode,
                    Some(false)
                )
            });
        }
    }
}

#[test]
fn native_startup_dimensions_disk_publication() {
    use ligature_reconfigure::Fixture;
    use yinz_config::config::DiskConfigChange;

    for clients in [&[CLIENT][..], &[][..]] {
        let fixture = Fixture::new(clients, None);
        let before = fixture.snapshot();
        let mut disk = fixture.with(|session| session.session_configuration.saved_config.clone());
        disk.terminal.initial_columns = Some(100);
        assert_eq!(
            fixture
                .all("terminal { initial_columns 100; }", true)
                .exit_status,
            None
        );
        for _ in 0..2 {
            fixture.with(|session| {
                session.publish_disk_config(DiskConfigChange {
                    config: disk.clone(),
                    requires_live_reload: false,
                    unchanged_managed_revision: None,
                })
            });
            // A startup-only observation preserves live config, theme preview, and the pending editor echo.
            assert_eq!(fixture.snapshot(), before);
            assert!(
                fixture.screen.is_empty() && fixture.plugin.is_empty() && fixture.pty.is_empty()
            );
        }
        fixture.with(|session| {
            let revision = session.session_configuration.startup_axes_save.clone();
            assert!(revision.is_some());
            for _ in 0..2 {
                session.publish_disk_config(DiskConfigChange {
                    config: disk.clone(),
                    requires_live_reload: true,
                    unchanged_managed_revision: revision.clone(),
                });
            }
        });
        assert_eq!(fixture.snapshot(), before);
        assert!(fixture.screen.is_empty() && fixture.plugin.is_empty() && fixture.pty.is_empty());
        fixture.with(|session| {
            let ConfigurationTarget { client_id, .. } =
                configuration_target(CLIENT, true, "test", &fixture.state)
                    .unwrap()
                    .unwrap();
            let mut writer = session
                .session_configuration
                .get_client_configuration(&client_id);
            writer.options.mouse_mode = Some(true);
            session
                .session_configuration
                .set_client_runtime_configuration(client_id, writer);
        });
        // A coalesced rebind/editor echo may include new host axes. It still
        // matches the previously applied live remainder, including repeated delivery.
        let mut editor = disk.clone();
        editor.options.default_cwd = Some(fixture.dir.path().into());
        fixture.with(|session| {
            let mut expected = editor.clone();
            expected.terminal.initial_columns = None;
            session.session_configuration.pending_editor_reload = Some(expected.clone());
            for _ in 0..2 {
                session.publish_disk_config(DiskConfigChange {
                    config: editor.clone(),
                    requires_live_reload: true,
                    unchanged_managed_revision: None,
                });
            }
            assert_eq!(
                session.session_configuration.pending_editor_reload,
                Some(expected)
            );
        });
        assert!(fixture.screen.is_empty() && fixture.plugin.is_empty() && fixture.pty.is_empty());
        disk.options.mouse_mode = Some(false);
        fixture.with(|session| {
            session.publish_disk_config(DiskConfigChange {
                config: disk.clone(),
                requires_live_reload: true,
                unchanged_managed_revision: None,
            })
        });
        assert!(matches!(
            fixture.screen.try_recv().unwrap().0,
            ScreenInstruction::Reconfigure { .. }
        ));
        let client = clients.first().copied().unwrap_or(ClientId::NONE);
        fixture.with(|session| {
            assert_eq!(
                session
                    .session_configuration
                    .get_client_configuration(&client)
                    .options
                    .mouse_mode,
                Some(false)
            )
        });
        let mixed = fixture.all("terminal { initial_rows 33; }\nmouse_mode true", true);
        assert_eq!(mixed.exit_status, None);
        fixture.with(|session| {
            assert_eq!(
                session
                    .session_configuration
                    .get_client_configuration(&client)
                    .options
                    .mouse_mode,
                Some(true)
            )
        });
    }
}

mod cursor_trail_reconfigure {
    use super::ligature_reconfigure::Fixture;
    use super::*;

    #[test]
    fn failed_cursor_trail_save_leaves_runtime_intact_and_unsaved_retry_is_refused() {
        for all in [false, true] {
            for runtime_matches in [false, true] {
                let fixture = Fixture::new(&[CLIENT], None);
                if runtime_matches {
                    fixture.with(|session| {
                        let configuration = &mut session.session_configuration;
                        let mut runtime = configuration.get_client_configuration(&CLIENT);
                        runtime.terminal.cursor_trail = Some(false);
                        configuration.set_client_runtime_configuration(CLIENT, runtime);
                    });
                }
                let fragment = "terminal { cursor_trail false; }\nmouse_mode false";
                let request = |fragment: &str, write| {
                    if all {
                        let result = fixture.all(fragment, write);
                        match result.exit_status {
                            None => Ok(()),
                            Some(1) => Err(result.error_message.unwrap()),
                            status => panic!("unexpected exit status {status:?}"),
                        }
                    } else {
                        reconfigure_client(
                            CLIENT,
                            fragment.into(),
                            write,
                            &fixture.state,
                            &fixture.data,
                        )
                        .map(|_| ())
                        .map_err(|error| format!("{error:#}"))
                    }
                };
                let drop_ins = fixture.dir.path().join("config.d");
                std::fs::write(&drop_ins, "blocked").unwrap();
                let before = fixture.snapshot();
                let failure = request(fragment, true).unwrap_err();
                fixture.assert_only_write_failure(&drop_ins.join("yinz-managed.kdl"), &failure);
                assert_eq!(fixture.snapshot(), before);

                std::fs::remove_file(drop_ins).unwrap();
                let refusal = request(fragment, false).unwrap_err();
                assert!(
                    refusal.contains("cursor_trail") && refusal.contains("--write"),
                    "{refusal}"
                );
                fixture.assert_no_propagation();
                assert_eq!(fixture.snapshot(), before);

                request(fragment, true).unwrap();
                let saved = Config::load_saved(None, Some(fixture.dir.path())).unwrap();
                assert_eq!(saved.terminal.cursor_trail, Some(false));
                assert_eq!(saved.options.mouse_mode, Some(false));
                request("terminal { cursor_trail false; }\nmouse_mode true", false).unwrap();
                fixture.with(|session| {
                    let runtime = session
                        .session_configuration
                        .get_client_configuration(&CLIENT);
                    assert_eq!(runtime.terminal.cursor_trail, Some(false));
                    assert_eq!(runtime.options.mouse_mode, Some(true));
                });
            }
        }
    }

    #[test]
    fn unsaved_cursor_trail_changes_refuse_whole_fragment_and_write_persists() {
        for clients in [vec![CLIENT], vec![]] {
            let fixture = Fixture::new(&clients, None);
            let before = fixture.snapshot();
            let fragment = "terminal { cursor_trail false; }\nmouse_mode false";
            let result = fixture.all(fragment, false);
            assert_eq!(result.exit_status, Some(1));
            let error = result.error_message.as_deref().unwrap();
            for word in [
                "cursor_trail",
                "--write",
                "configuration/drop-in",
                "entire fragment",
            ] {
                assert!(error.contains(word), "{error}");
            }
            assert_eq!(fixture.snapshot(), before);
            fixture.assert_no_propagation();

            let err = reconfigure_client(
                CLIENT,
                fragment.to_owned(),
                false,
                &fixture.state,
                &fixture.data,
            )
            .unwrap_err();
            let msg = format!("{err:#}");
            assert!(msg.contains("cursor_trail"), "{msg}");
            assert_eq!(fixture.snapshot(), before);
            fixture.assert_no_propagation();

            // Equivalent unsaved value (true matches default true) is accepted.
            let equivalent =
                fixture.all("terminal { cursor_trail true; }\nmouse_mode false", false);
            assert_eq!(equivalent.exit_status, None);

            // Persisting with --write succeeds and saves cursor_trail to the managed drop-in.
            let saved = fixture.all("terminal { cursor_trail false; }", true);
            assert_eq!(saved.exit_status, None);
            let loaded = Config::load_saved(None, Some(fixture.dir.path())).unwrap();
            assert_eq!(loaded.terminal.cursor_trail, Some(false));

            // Now toggling back to true without --write is also refused.
            let revert_refused =
                fixture.all("terminal { cursor_trail true; }\nmouse_mode true", false);
            assert_eq!(revert_refused.exit_status, Some(1));
            assert!(revert_refused
                .error_message
                .as_deref()
                .unwrap()
                .contains("cursor_trail"));
        }
    }
}
