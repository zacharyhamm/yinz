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
    let (resolved, changed) = configuration.reconfigure_runtime_config(
        &CLIENT,
        "plugins { panel repo=\"tools\"; }".into(),
        Some(Path::new("/elsewhere/config.kdl")),
        Some(directory.path()),
    );
    assert!(changed);
    let resolved = resolved.unwrap();
    let RunPluginLocation::Git(plugin) = &resolved.plugins.aliases["panel"].location else {
        panic!("Git alias")
    };
    assert_eq!(Path::new(&plugin.repo), directory.path().join("tools"));
    let (again, changed) = configuration.reconfigure_runtime_config(
        &CLIENT,
        "plugins { panel repo=\"tools\"; }".into(),
        None,
        Some(directory.path()),
    );
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
    assert_eq!(
        configuration.reconfigure_runtime_config(
            &CLIENT,
            "mouse_mode false".into(),
            None,
            Some(directory.path())
        ),
        (None, false)
    );
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
                assert_eq!(config, &ScreenConfig::from_config(&reloaded));
            },
            other => panic!("expected one screen Reconfigure, got {other:?}"),
        }
        let mut plugin = plugin_rx.try_iter().map(|(message, _)| message);
        match plugin.next() {
            Some(PluginInstruction::Reconfigure {
                client_id,
                keybinds,
                default_shell: Some(TerminalAction::RunCommand(default_shell)),
                user_plugin_dir,
                was_written_to_disk: reported_written,
                ..
            }) => {
                assert_eq!(client_id, CLIENT);
                assert_eq!(keybinds.as_ref(), Some(&effective_keybinds));
                assert_eq!(default_shell, shell);
                assert_eq!(user_plugin_dir, Some(PathBuf::from("/yinz-config/plugins")));
                assert_eq!(reported_written, was_written_to_disk);
            },
            other => panic!("expected a plugin Reconfigure with the new shell, got {other:?}"),
        }
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
            let (config, changed) = session.session_configuration.reconfigure_runtime_config(
                &CLIENT,
                fragment.clone(),
                None,
                Some(directory.path()),
            );
            assert!(changed);
            let session = Arc::new(RwLock::new(Some(session)));
            update_new_saved_config(
                config,
                ConfigPersistence::Fragment(fragment),
                changed,
                &session,
                CLIENT,
            );
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
