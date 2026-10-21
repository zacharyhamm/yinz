use super::*;
use std::str::FromStr;
use yinz_config::screen_config::ScreenConfig;

const CLIENT: ClientId = ClientId::new(1);

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
        let mut session = SessionMetaData {
            shutdown: Default::default(),
            senders: ThreadSenders {
                to_screen: Some(SenderWithContext::new(to_screen)),
                to_plugin: Some(SenderWithContext::new(to_plugin)),
                to_pty: Some(SenderWithContext::new(to_pty)),
                ..Default::default()
            },
            data_access: std::sync::Arc::new(yinz_storage::data_access::DataAccess::sqlite(
                std::path::Path::new("/unused"),
            )),
            default_shell: None,
            current_input_modes: HashMap::new(),
            session_configuration: Default::default(),
            key_passthrough_clients: HashMap::new(),
            locked_escapes: HashMap::new(),
            command_palette_restore_modes: HashMap::new(),
            pending_mouse_updates: Vec::new(),
            screen_thread: None,
            pty_thread: None,
            plugin_thread: None,
            pty_writer_thread: None,
            background_jobs_thread: None,
            config_file_path: None,
            config_dir: Some(PathBuf::from("/yinz-config")),
        };
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
