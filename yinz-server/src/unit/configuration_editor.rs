use super::*;
use serde_json::json;

fn mouse(value: bool) -> Patch {
    Patch {
        reset: false,
        path: vec!["mouse_mode".into()],
        node: Some(yinz_types::configuration::Node {
            name: "mouse_mode".into(),
            arguments: vec![json!(value)],
            properties: Default::default(),
            children: vec![],
        }),
    }
}

#[test]
fn session_layer_survives_reload_and_later_clients() {
    let mut state = SessionConfiguration::default();
    let base = Config::from_kdl("mouse_mode true\npane_frames true", None).unwrap();
    state.set_saved_configuration(base.clone());
    state.set_client_runtime_configuration(ClientId::new(1), base.clone());
    state.editor_overrides.push(mouse(false));
    state.recompute_effective_keybinds();
    assert_eq!(
        state
            .get_client_configuration(&ClientId::new(1))
            .options
            .mouse_mode,
        Some(false)
    );
    state.change_saved_config(base.clone());
    state.set_client_runtime_configuration(ClientId::new(2), base);
    assert_eq!(
        state
            .get_client_configuration(&ClientId::new(2))
            .options
            .mouse_mode,
        Some(false)
    );
    state.editor_overrides.clear();
    state.recompute_effective_keybinds();
    assert_eq!(
        state
            .get_client_configuration(&ClientId::new(1))
            .options
            .mouse_mode,
        Some(true)
    );
}

#[test]
fn revisions_cover_other_clients_and_external_writes() {
    let dir = tempfile::tempdir().unwrap();
    let disk = Disk::read(None, Some(dir.path())).unwrap();
    let mut state = SessionConfiguration::default();
    let revision = state.editor_revision(&disk);
    state.set_client_runtime_configuration(ClientId::new(1), Config::default());
    assert_ne!(revision, state.editor_revision(&disk));
    let revision = state.editor_revision(&disk);
    std::fs::write(dir.path().join("config.kdl"), "mouse_mode false").unwrap();
    assert_ne!(
        revision,
        state.editor_revision(&Disk::read(None, Some(dir.path())).unwrap())
    );
}

#[test]
fn configuration_transactions_preserve_temporary_edits_and_reject_failed_or_stale_writes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.kdl"),
        "mouse_mode true\npane_frames true\n",
    )
    .unwrap();
    let (mut session, screen_rx) = session(dir.path());
    let base = Config::load(None, Some(dir.path()), false).unwrap();
    session
        .session_configuration
        .set_saved_configuration(base.clone());
    for id in [1, 2] {
        session
            .session_configuration
            .set_client_runtime_configuration(ClientId::new(id), base.clone());
    }
    submit(
        &mut session,
        Destination::Session,
        vec![mouse(false)],
        vec![],
    )
    .unwrap();
    assert_eq!(
        screen_rx
            .try_iter()
            .filter(|(i, _)| matches!(i, ScreenInstruction::Reconfigure { .. }))
            .count(),
        2
    );
    let mut frames = mouse(false);
    frames.path[0] = "pane_frames".into();
    frames.node.as_mut().unwrap().name = "pane_frames".into();
    submit(&mut session, Destination::Permanent, vec![frames], vec![]).unwrap();
    let saved = Config::load(None, Some(dir.path()), false).unwrap();
    assert_eq!(saved.options.mouse_mode, Some(true));
    assert_eq!(saved.options.pane_frames, Some(false));
    assert_eq!(
        session
            .session_configuration
            .get_client_configuration(&ClientId::new(1))
            .options
            .mouse_mode,
        Some(false)
    );
    session
        .session_configuration
        .change_saved_config(saved.clone());
    session
        .session_configuration
        .set_client_runtime_configuration(ClientId::new(3), saved);
    assert_eq!(
        session
            .session_configuration
            .get_client_configuration(&ClientId::new(3))
            .options
            .mouse_mode,
        Some(false)
    );
    let stale = session.configuration_editor(None).unwrap()["revision"]
        .as_str()
        .unwrap()
        .to_owned();
    std::fs::write(
        dir.path().join("config.kdl"),
        "mouse_mode true\npane_frames true\n// changed\n",
    )
    .unwrap();
    assert!(session
        .configuration_editor(Some(Edit {
            revision: stale,
            destination: Destination::Permanent,
            patches: vec![mouse(true)],
            reset: vec![]
        }))
        .unwrap_err()
        .contains("Reload"));
    let overlay = dir.path().join("config.d");
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .open(overlay.join(".yinz-managed.kdl.lock"))
        .unwrap();
    lock.lock().unwrap();
    assert!(submit(
        &mut session,
        Destination::Permanent,
        vec![mouse(true)],
        vec![]
    )
    .unwrap_err()
    .contains("busy"));
    assert_eq!(
        session
            .session_configuration
            .get_client_configuration(&ClientId::new(1))
            .options
            .mouse_mode,
        Some(false)
    );
    drop(lock);
    submit(
        &mut session,
        Destination::Session,
        vec![],
        vec![vec!["mouse_mode".into()]],
    )
    .unwrap();
    assert_eq!(
        session
            .session_configuration
            .get_client_configuration(&ClientId::new(1))
            .options
            .mouse_mode,
        Some(true)
    );
    let mut host = mouse(true);
    host.path[0] = "host_id".into();
    host.node.as_mut().unwrap().name = "host_id".into();
    host.node.as_mut().unwrap().arguments = vec![json!("host")];
    assert!(
        submit(&mut session, Destination::Session, vec![host], vec![])
            .unwrap_err()
            .contains("next session")
    );
}

#[test]
fn legacy_reconfigure_does_not_copy_the_session_layer_into_client_base() {
    let mut state = SessionConfiguration::default();
    let base = Config::from_kdl("mouse_mode true\npane_frames true", None).unwrap();
    state.set_saved_configuration(base.clone());
    state.set_client_runtime_configuration(ClientId::new(1), base);
    state.editor_overrides.push(mouse(false));
    state.recompute_effective_keybinds();
    state
        .reconfigure_runtime_config(&ClientId::new(1), "pane_frames false".into(), None, None)
        .unwrap();
    state.editor_overrides.clear();
    state.recompute_effective_keybinds();
    let config = state.get_client_configuration(&ClientId::new(1));
    assert_eq!(config.options.mouse_mode, Some(true));
    assert_eq!(config.options.pane_frames, Some(false));
}

fn session(
    dir: &Path,
) -> (
    SessionMetaData,
    channels::Receiver<(ScreenInstruction, yinz_types::diagnostics::ErrorContext)>,
) {
    let (screen_tx, screen_rx) = channels::unbounded();
    let session = SessionMetaData {
        shutdown: Default::default(),
        senders: ThreadSenders {
            to_screen: Some(SenderWithContext::new(screen_tx)),
            should_silently_fail: true,
            ..Default::default()
        },
        data_access: Arc::new(yinz_storage::data_access::DataAccess::sqlite(Path::new(
            "/unused",
        ))),
        default_shell: None,
        current_input_modes: Default::default(),
        session_configuration: Default::default(),
        key_passthrough_clients: Default::default(),
        locked_escapes: Default::default(),
        command_palette_restore_modes: Default::default(),
        pending_mouse_updates: Vec::new(),
        screen_thread: None,
        pty_thread: None,
        plugin_thread: None,
        pty_writer_thread: None,
        background_jobs_thread: None,
        config_file_path: None,
        config_dir: Some(dir.into()),
    };
    (session, screen_rx)
}

#[test]
fn permanent_binding_reconciliation_keeps_both_modes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.kdl"), "keybinds clear-defaults=true { normal { bind \"Ctrl a\" { Quit; }; }; locked { bind \"Ctrl a\" { Copy; }; }; }").unwrap();
    let base = Config::load(None, Some(dir.path()), false).unwrap();
    let (mut session, _) = session(dir.path());
    session
        .session_configuration
        .set_saved_configuration(base.clone());
    session
        .session_configuration
        .set_client_runtime_configuration(ClientId::new(1), base);
    let snapshot = session.configuration_editor(None).unwrap();
    let path = vec![
        "keybinds".into(),
        "normal".into(),
        "bind:[\"Ctrl a\"]".into(),
    ];
    let nodes = editor::document(&session.session_configuration.saved_config).unwrap();
    let mut binding = find_node(&nodes, &path).unwrap().clone();
    binding.children[0].name = "Copy".into();
    let result = session
        .configuration_editor(Some(Edit {
            revision: snapshot["revision"].as_str().unwrap().into(),
            destination: Destination::Permanent,
            patches: vec![Patch {
                path: path.clone(),
                node: Some(binding.clone()),
                reset: false,
            }],
            reset: vec![],
        }))
        .unwrap();
    assert_eq!(result["notice"], "Saved permanently");
    let disk = Disk::read(None, Some(dir.path()))
        .unwrap()
        .preview(&[], &[])
        .unwrap()
        .1;
    let state = &mut session.session_configuration;
    assert!(state.change_saved_config(disk.clone()).is_empty());
    for config in [disk, state.get_client_configuration(&ClientId::new(1))] {
        let nodes = editor::document(&config).unwrap();
        for mode in ["normal", "locked"] {
            assert_eq!(
                find_node(&nodes, &["keybinds".into(), mode.into(), path[2].clone()]),
                Some(&binding)
            );
        }
    }
    for destination in [Destination::Session, Destination::Permanent] {
        let snapshot = session.configuration_editor(None).unwrap();
        session
            .configuration_editor(Some(Edit {
                revision: snapshot["revision"].as_str().unwrap().into(),
                destination,
                patches: vec![Patch {
                    path: path.clone(),
                    node: None,
                    reset: false,
                }],
                reset: vec![],
            }))
            .unwrap();
        let config = session
            .session_configuration
            .get_client_configuration(&ClientId::new(1));
        assert!(config.keybinds.0[&InputMode::Normal].is_empty());
        assert_eq!(config.keybinds.0[&InputMode::Locked].len(), 1);
    }
    let disk = Disk::read(None, Some(dir.path()))
        .unwrap()
        .preview(&[], &[])
        .unwrap()
        .1;
    assert!(disk.keybinds.0[&InputMode::Normal].is_empty());
    assert_eq!(disk.keybinds.0[&InputMode::Locked].len(), 1);
}

#[test]
fn permanent_preview_matches_watcher_layout_precedence_and_preserves_client_values() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("layouts")).unwrap();
    std::fs::write(
        dir.path().join("config.kdl"),
        "default_layout \"conflict\"\nmouse_mode true\npane_frames true\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("layouts/conflict.kdl"),
        "mouse_mode true\npane_frames false\nlayout { pane; }\n",
    )
    .unwrap();
    let disk = Disk::read(None, Some(dir.path())).unwrap();
    let (_, base, underlay) = disk.preview(&[], &[]).unwrap();
    assert_eq!(underlay.options.pane_frames, Some(true));
    assert_eq!(base.options.pane_frames, Some(false));
    let (mut session, _) = session(dir.path());
    session
        .session_configuration
        .set_saved_configuration(base.clone());
    session
        .session_configuration
        .set_client_runtime_configuration(ClientId::new(2), base.clone());
    let mut client = base;
    client.options.pane_frames = Some(true);
    session
        .session_configuration
        .set_client_runtime_configuration(ClientId::new(1), client);
    let snapshot = session.configuration_editor(None).unwrap();
    let result = session
        .configuration_editor(Some(Edit {
            revision: snapshot["revision"].as_str().unwrap().into(),
            destination: Destination::Permanent,
            patches: vec![mouse(false)],
            reset: vec![],
        }))
        .unwrap();
    assert!(result["notice"]
        .as_str()
        .unwrap()
        .contains("another configuration layer"));
    let loaded = Config::load(None, Some(dir.path()), false).unwrap();
    assert_eq!(loaded.options.mouse_mode, Some(false));
    let (_, mut watched) = yinz_config::config::load_layout_and_config(
        None,
        loaded,
        Some(dir.path().into()),
        Some(&dir.path().join("config.kdl")),
        None,
        None,
        ".".into(),
    )
    .unwrap();
    yinz_config::config::apply_themes(&mut watched, None, Some(dir.path().into())).unwrap();
    let preview = Disk::read(None, Some(dir.path()))
        .unwrap()
        .preview(&[], &[])
        .unwrap()
        .1;
    assert_eq!(preview, watched);
    let state = &mut session.session_configuration;
    assert_eq!(state.pending_editor_reload.as_ref(), Some(&watched));
    for _ in 0..3 {
        assert!(state.change_saved_config(watched.clone()).is_empty());
    }
    let client = state.get_client_configuration(&ClientId::new(1));
    assert_eq!(client.options.mouse_mode, Some(true));
    assert_eq!(client.options.pane_frames, Some(true));
    assert_eq!(
        state
            .get_client_configuration(&ClientId::new(2))
            .options
            .pane_frames,
        Some(false)
    );
    watched.options.copy_command = Some("external-copy".into());
    assert!(!state.change_saved_config(watched.clone()).is_empty());
    assert!(state.pending_editor_reload.is_none());
    assert_eq!(
        state
            .get_client_configuration(&ClientId::new(1))
            .options
            .pane_frames,
        Some(false)
    );
}

#[test]
fn captured_key_spellings_survive_save_remove_and_reset() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.kdl"),
        "keybinds clear-defaults=true { normal { bind \"esc\" { Quit; }; }; }",
    )
    .unwrap();
    let base = Config::load(None, Some(dir.path()), false).unwrap();
    let (mut session, _) = session(dir.path());
    session
        .session_configuration
        .set_saved_configuration(base.clone());
    session
        .session_configuration
        .set_client_runtime_configuration(ClientId::new(1), base);
    let path = vec!["keybinds".into(), "normal".into(), "bind:[\"ESC\"]".into()];
    let binding = yinz_types::configuration::Node {
        name: "bind".into(),
        arguments: vec![serde_json::json!("ESC")],
        properties: Default::default(),
        children: vec![yinz_types::configuration::Node {
            name: "Copy".into(),
            arguments: vec![],
            properties: Default::default(),
            children: vec![],
        }],
    };
    for operation in ["save", "remove", "reset"] {
        let snapshot = session.configuration_editor(None).unwrap();
        let result = session
            .configuration_editor(Some(Edit {
                revision: snapshot["revision"].as_str().unwrap().into(),
                destination: Destination::Permanent,
                patches: if operation == "reset" {
                    vec![]
                } else {
                    vec![Patch {
                        path: path.clone(),
                        node: (operation == "save").then_some(binding.clone()),
                        reset: false,
                    }]
                },
                reset: if operation == "reset" {
                    vec![path.clone()]
                } else {
                    vec![]
                },
            }))
            .unwrap();
        assert_eq!(result["notice"], "Saved permanently");
        let saved = Config::load(None, Some(dir.path()), false).unwrap();
        for config in [
            saved,
            session
                .session_configuration
                .get_client_configuration(&ClientId::new(1)),
        ] {
            let nodes = editor::document(&config).unwrap();
            let found = find_node(
                &nodes,
                &["keybinds".into(), "normal".into(), "bind:[\"esc\"]".into()],
            );
            assert_eq!(
                found.map(|n| n.children[0].name.as_str()),
                match operation {
                    "save" => Some("Copy"),
                    "reset" => Some("Quit"),
                    _ => None,
                }
            );
        }
    }
}

fn submit(
    session: &mut SessionMetaData,
    destination: Destination,
    patches: Vec<Patch>,
    reset: Vec<Vec<String>>,
) -> Result<serde_json::Value, String> {
    let snapshot = session.configuration_editor(None)?;
    session.configuration_editor(Some(Edit {
        revision: snapshot["revision"].as_str().unwrap().into(),
        destination,
        patches,
        reset,
    }))
}

#[test]
fn saving_disk_scalar_clears_conflicting_session_override() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.kdl"), "mouse_mode true").unwrap();
    let (mut session, _) = session(dir.path());
    let base = Config::load(None, Some(dir.path()), false).unwrap();
    session
        .session_configuration
        .set_saved_configuration(base.clone());
    session
        .session_configuration
        .set_client_runtime_configuration(ClientId::new(1), base);
    submit(
        &mut session,
        Destination::Session,
        vec![mouse(false)],
        vec![],
    )
    .unwrap();
    submit(
        &mut session,
        Destination::Permanent,
        vec![mouse(true)],
        vec![],
    )
    .unwrap();
    assert!(session.session_configuration.editor_overrides.is_empty());
    assert_eq!(
        session
            .session_configuration
            .get_client_configuration(&ClientId::new(1))
            .options
            .mouse_mode,
        Some(true)
    );
}

#[test]
fn combined_binding_save_reconciles_each_key_and_clears_overrides() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.kdl"),
        r#"keybinds clear-defaults=true {
        normal { bind "Ctrl a" "Ctrl b" "Ctrl c" { Quit; }; }
        locked { bind "Ctrl a" { Quit; }; }
    }"#,
    )
    .unwrap();
    let base = Config::load(None, Some(dir.path()), false).unwrap();
    let (mut session, _) = session(dir.path());
    session
        .session_configuration
        .set_saved_configuration(base.clone());
    session
        .session_configuration
        .set_client_runtime_configuration(ClientId::new(1), base.clone());
    let nodes = editor::document(&base).unwrap();
    let path = vec![
        "keybinds".into(),
        "normal".into(),
        r#"bind:["Ctrl a"]"#.into(),
    ];
    let mut binding = find_node(&nodes, &path).unwrap().clone();
    binding.children[0].name = "Copy".into();
    let single = Patch {
        path,
        node: Some(binding.clone()),
        reset: false,
    };
    submit(&mut session, Destination::Session, vec![single], vec![]).unwrap();
    binding.arguments = vec![json!("Ctrl a"), json!("Ctrl b")];
    binding.children[0].name = "ToggleFocusFullscreen".into();
    let combined = Patch {
        path: vec![
            "keybinds".into(),
            "normal".into(),
            editor::node_key(&binding),
        ],
        node: Some(binding),
        reset: false,
    };
    // Combined temporary edits must also clear when both keys are saved.
    submit(
        &mut session,
        Destination::Session,
        vec![combined.clone()],
        vec![],
    )
    .unwrap();
    let result = submit(&mut session, Destination::Permanent, vec![combined], vec![]).unwrap();
    assert_eq!(result["notice"], "Saved permanently");
    assert!(session.session_configuration.editor_overrides.is_empty());
    let live = session
        .session_configuration
        .get_client_configuration(&ClientId::new(1));
    let loaded = Config::load(None, Some(dir.path()), false).unwrap();
    for config in [&live, &loaded] {
        let nodes = editor::document(config).unwrap();
        for (mode, key, action) in [
            ("normal", "a", "ToggleFocusFullscreen"),
            ("normal", "b", "ToggleFocusFullscreen"),
            ("normal", "c", "Quit"),
            ("locked", "a", "Quit"),
        ] {
            let path = vec![
                "keybinds".into(),
                mode.into(),
                format!("bind:[\"Ctrl {key}\"]"),
            ];
            assert_eq!(find_node(&nodes, &path).unwrap().children[0].name, action);
        }
    }
    assert!(session
        .session_configuration
        .change_saved_config(loaded)
        .is_empty());
}

#[test]
fn input_options_use_effective_cache_and_uncached_fallback() {
    let mut state = SessionConfiguration::default();
    state.set_saved_configuration(
        Config::from_kdl(
            "default_mode \"locked\"\nlocked_escape_timeout_ms 450",
            None,
        )
        .unwrap(),
    );
    let cached = ClientId::new(1);
    let absent = ClientId::new(2);
    state.set_client_runtime_configuration(cached, state.saved_config.clone());
    let options = Config::from_kdl(
        "default_mode \"normal\"\nlocked_escape_timeout_ms 123",
        None,
    )
    .unwrap();
    let nodes = editor::document(&options).unwrap();
    state.editor_overrides = ["default_mode", "locked_escape_timeout_ms"]
        .into_iter()
        .map(|name| Patch {
            path: vec![name.into()],
            node: find_node(&nodes, &[name.into()]).cloned(),
            reset: false,
        })
        .collect();
    state.recompute_effective_keybinds();
    assert!(state.editor_effective.contains_key(&cached));
    assert!(!state.editor_effective.contains_key(&absent));
    for client in [cached, absent] {
        assert_eq!(
            state.get_client_default_input_mode(&client),
            InputMode::Normal
        );
        assert_eq!(
            state.get_client_locked_escape_timeout(&client),
            Duration::from_millis(123)
        );
    }
    state.editor_overrides.clear();
    state.recompute_effective_keybinds();
    for client in [cached, absent] {
        assert_eq!(
            state.get_client_default_input_mode(&client),
            InputMode::Locked
        );
        assert_eq!(
            state.get_client_locked_escape_timeout(&client),
            Duration::from_millis(450)
        );
    }
    let state = SessionConfiguration::default();
    assert_eq!(
        state.get_client_default_input_mode(&absent),
        InputMode::default()
    );
    assert_eq!(
        state.get_client_locked_escape_timeout(&absent),
        Duration::from_millis(locked_escape::DEFAULT_LOCKED_ESCAPE_TIMEOUT_MS)
    );
}

#[test]
fn legacy_shared_binding_reset_restores_underlay_live_and_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.kdl"),
        r#"keybinds clear-defaults=true {
        normal { bind "Ctrl a" { Quit; }; }
    }"#,
    )
    .unwrap();
    Config::write_managed_fragment(
        &dir.path().join("config.d"),
        r#"keybinds {
        shared_among "normal" "locked" { bind "Ctrl a" "Ctrl b" { Copy; }; }
    }"#,
    )
    .unwrap();
    let base = Config::load(None, Some(dir.path()), false).unwrap();
    let (mut session, _) = session(dir.path());
    session
        .session_configuration
        .set_saved_configuration(base.clone());
    session
        .session_configuration
        .set_client_runtime_configuration(ClientId::new(1), base);
    submit(
        &mut session,
        Destination::Permanent,
        vec![],
        vec![vec![
            "keybinds".into(),
            "normal".into(),
            "bind:[\"Ctrl a\"]".into(),
        ]],
    )
    .unwrap();
    for config in [
        Config::load(None, Some(dir.path()), false).unwrap(),
        session
            .session_configuration
            .get_client_configuration(&ClientId::new(1)),
    ] {
        let nodes = editor::document(&config).unwrap();
        for (mode, key, action) in [
            ("normal", "a", "Quit"),
            ("normal", "b", "Copy"),
            ("locked", "a", "Copy"),
            ("locked", "b", "Copy"),
        ] {
            let path = vec![
                "keybinds".into(),
                mode.into(),
                format!("bind:[\"Ctrl {key}\"]"),
            ];
            assert_eq!(find_node(&nodes, &path).unwrap().children[0].name, action);
        }
    }
}

#[test]
fn bundled_theme_edits_survive_startup_loading_and_directory_precedence() {
    use yinz_types::{data::PaletteColor, input::cli_assets::CliAssets};
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.kdl");
    std::fs::write(&config_path, "theme \"dracula\"\n").unwrap();
    let disk = Disk::read(Some(&config_path), Some(dir.path())).unwrap();
    let (fragment, _, underlay) = disk
        .preview(
            &[Patch {
                path: vec![
                    "themes".into(),
                    "dracula".into(),
                    "text_unselected".into(),
                    "base".into(),
                ],
                node: Some(yinz_types::configuration::Node {
                    name: "base".into(),
                    arguments: vec![json!(7)],
                    properties: Default::default(),
                    children: vec![],
                }),
                reset: false,
            }],
            &[],
        )
        .unwrap();
    Config::try_write_editor_fragment(&disk.overlay, &fragment, underlay, || Ok(())).unwrap();
    let assets = CliAssets {
        config_file_path: Some(config_path),
        config_dir: Some(dir.path().into()),
        ..Default::default()
    };
    let (loaded, _) = yinz_config::cli_assets::load_config_and_layout(&assets).unwrap();
    assert_eq!(
        loaded
            .themes
            .get_theme("dracula")
            .unwrap()
            .palette
            .text_unselected
            .base,
        PaletteColor::EightBit(7)
    );
    let mut external = Config::default();
    let mut theme = loaded.themes.get_theme("dracula").unwrap().clone();
    theme.palette.text_unselected.base = PaletteColor::EightBit(9);
    external.themes.insert("dracula".into(), theme);
    std::fs::create_dir(dir.path().join("themes")).unwrap();
    std::fs::write(
        dir.path().join("themes/dracula.kdl"),
        external.to_string(false).unwrap(),
    )
    .unwrap();
    let (loaded, _) = yinz_config::cli_assets::load_config_and_layout(&assets).unwrap();
    assert_eq!(
        loaded
            .themes
            .get_theme("dracula")
            .unwrap()
            .palette
            .text_unselected
            .base,
        PaletteColor::EightBit(9)
    );
}

#[test]
fn detached_configuration_participates_in_editor_revisions_and_permanent_edits() {
    let dir = tempfile::tempdir().unwrap();
    let (mut session, screen_rx) = session(dir.path());
    let disk = Disk::read(None, Some(dir.path())).unwrap();
    let revision = session.session_configuration.editor_revision(&disk);
    session
        .session_configuration
        .reconfigure_runtime_config(
            &ClientId::NONE,
            "mouse_mode false\npane_frames false".into(),
            None,
            None,
        )
        .unwrap();
    assert_ne!(
        revision,
        session.session_configuration.editor_revision(&disk)
    );
    submit(
        &mut session,
        Destination::Permanent,
        vec![mouse(true)],
        vec![],
    )
    .unwrap();
    let detached = session
        .session_configuration
        .get_client_configuration(&ClientId::NONE);
    assert_eq!(detached.options.mouse_mode, Some(true));
    assert_eq!(detached.options.pane_frames, Some(false));
    assert!(screen_rx.try_iter().any(|(instruction, _)| matches!(
        instruction,
        ScreenInstruction::Reconfigure {
            client_id: ClientId::NONE,
            ..
        }
    )));
}
