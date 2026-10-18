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
    let result = session.configuration_editor(Some(Edit {
        revision: snapshot["revision"].as_str().unwrap().into(),
        destination,
        patches,
        reset,
    }));
    if result.is_ok() {
        assert_effective_configuration_matches_recompute(&session.session_configuration);
    }
    result
}

fn assert_effective_configuration_matches_recompute(state: &SessionConfiguration) {
    let mut recomputed = state.clone();
    recomputed.recompute_effective_keybinds();
    assert_eq!(state.editor_effective, recomputed.editor_effective);
    assert_eq!(state.effective_keybinds, recomputed.effective_keybinds);
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

fn loaded_theme_config(dir: &Path) -> Config {
    yinz_config::cli_assets::load_config_and_layout(&yinz_types::input::cli_assets::CliAssets {
        config_file_path: Some(dir.join("config.kdl")),
        config_dir: Some(dir.into()),
        ..Default::default()
    })
    .unwrap()
    .0
}

#[test]
fn palette_theme_transaction_preserves_overrides_variants_and_future_clients() {
    for permanent in [false, true] {
        for variants in ["", "theme_dark \"nightfox\"\ntheme_light \"dayfox\"\n"] {
            let dir = tempfile::tempdir().unwrap();
            let original = format!("theme \"default\"\nmouse_mode true\n{variants}");
            std::fs::write(dir.path().join("config.kdl"), &original).unwrap();
            let base = loaded_theme_config(dir.path());
            let (mut session, screen_rx) = session(dir.path());
            session
                .session_configuration
                .set_saved_configuration(base.clone());
            let plugin_bindings = Config::from_kdl(
                r#"keybinds clear-defaults=true { normal { bind "Ctrl Alt F12" { Quit; }; }; }"#,
                None,
            )
            .unwrap()
            .keybinds;
            let plugin_key = plugin_bindings.0[&InputMode::Normal]
                .keys()
                .next()
                .unwrap()
                .clone();
            session
                .session_configuration
                .register_plugin_default_keybinds(PluginId::new(1), plugin_bindings);
            for id in [1, 2] {
                let mut client = base.clone();
                client.options.scroll_buffer_size = Some(1000 * id as usize);
                if id == 1 {
                    client
                        .keybinds
                        .0
                        .entry(InputMode::Normal)
                        .or_default()
                        .insert(plugin_key.clone(), vec![Action::Copy]);
                }
                session
                    .session_configuration
                    .set_client_runtime_configuration(ClientId::new(id), client);
            }
            session
                .session_configuration
                .editor_overrides
                .push(mouse(false));
            let request = uuid::Uuid::new_v4();
            session
                .apply_palette_theme(
                    ClientId::new(1),
                    request,
                    "dracula",
                    permanent,
                    &Default::default(),
                )
                .unwrap();
            assert_effective_configuration_matches_recompute(&session.session_configuration);
            for id in [1, 2] {
                assert_eq!(
                    session
                        .session_configuration
                        .get_client_keybinds(&ClientId::new(id))
                        .get_actions_for_key_in_mode(&InputMode::Normal, &plugin_key),
                    Some(&vec![if id == 1 { Action::Copy } else { Action::Quit }]),
                );
            }
            let instructions: Vec<_> = screen_rx
                .try_iter()
                .map(|(instruction, _)| instruction)
                .collect();
            assert!(
                matches!(instructions.first(), Some(ScreenInstruction::PaletteThemeSaved { client, request: actual, result: Ok(()) }) if *client == ClientId::new(1) && *actual == request)
            );
            assert_eq!(
                instructions
                    .iter()
                    .filter(|i| matches!(i, ScreenInstruction::Reconfigure { .. }))
                    .count(),
                2
            );
            for id in [1, 2] {
                let config = session
                    .session_configuration
                    .get_client_configuration(&ClientId::new(id));
                assert_eq!(config.options.theme.as_deref(), Some("dracula"));
                assert_eq!(
                    config.options.theme_dark.as_deref(),
                    (!variants.is_empty()).then_some("dracula")
                );
                assert_eq!(
                    config.options.theme_light.as_deref(),
                    (!variants.is_empty()).then_some("dracula")
                );
                assert_eq!(config.options.mouse_mode, Some(false));
                assert_eq!(config.options.scroll_buffer_size, Some(1000 * id as usize));
            }
            if !permanent {
                session
                    .apply_palette_theme(
                        ClientId::new(1),
                        uuid::Uuid::new_v4(),
                        "dracula",
                        false,
                        &Default::default(),
                    )
                    .unwrap();
                assert_effective_configuration_matches_recompute(&session.session_configuration);
                assert!(!screen_rx.try_iter().any(|(instruction, _)| matches!(
                    instruction,
                    ScreenInstruction::Reconfigure { .. }
                )));
            }
            let saved = loaded_theme_config(dir.path());
            assert_eq!(
                saved.options.theme.as_deref(),
                Some(if permanent { "dracula" } else { "default" })
            );
            assert_eq!(saved.options.mouse_mode, Some(true));
            assert_eq!(
                std::fs::read_to_string(dir.path().join("config.kdl")).unwrap(),
                original
            );
            assert_eq!(
                dir.path().join("config.d/yinz-managed.kdl").exists(),
                permanent
            );
            session
                .session_configuration
                .change_saved_config(saved.clone());
            session
                .session_configuration
                .set_client_runtime_configuration(ClientId::new(3), saved);
            let future = session
                .session_configuration
                .get_client_configuration(&ClientId::new(3));
            assert_eq!(future.options.theme.as_deref(), Some("dracula"));
            assert_eq!(future.options.mouse_mode, Some(false));
        }
    }
}

#[test]
fn palette_theme_transaction_cancellation_and_failed_write_leave_state_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.kdl"), "theme \"default\"\n").unwrap();
    let base = loaded_theme_config(dir.path());
    let (mut session, screen_rx) = session(dir.path());
    session
        .session_configuration
        .set_saved_configuration(base.clone());
    session
        .session_configuration
        .set_client_runtime_configuration(ClientId::new(1), base.clone());
    let disk = Disk::read(None, Some(dir.path())).unwrap();
    let revision = session.session_configuration.editor_revision(&disk);
    for permanent in [false, true] {
        let error = session
            .apply_palette_theme(
                ClientId::new(1),
                uuid::Uuid::new_v4(),
                "dracula",
                permanent,
                &std::sync::atomic::AtomicBool::new(true),
            )
            .unwrap_err();
        assert!(error.contains("cancelled"), "{error}");
        assert_eq!(
            session.session_configuration.editor_revision(&disk),
            revision
        );
        assert!(!dir.path().join("config.d").exists());
        assert!(screen_rx.try_recv().is_err());
    }
    let overlay = dir.path().join("config.d");
    std::fs::create_dir(&overlay).unwrap();
    let lock = std::fs::File::create(overlay.join(".yinz-managed.kdl.lock")).unwrap();
    lock.lock().unwrap();
    let error = session
        .apply_palette_theme(
            ClientId::new(1),
            uuid::Uuid::new_v4(),
            "dracula",
            true,
            &Default::default(),
        )
        .unwrap_err();
    assert!(error.contains("busy"), "{error}");
    assert_eq!(
        session.session_configuration.editor_revision(&disk),
        revision
    );
    assert_eq!(loaded_theme_config(dir.path()), base);
    assert!(!overlay.join("yinz-managed.kdl").exists());
    assert!(screen_rx.try_recv().is_err());
    drop(lock);
    session
        .apply_palette_theme(
            ClientId::new(1),
            uuid::Uuid::new_v4(),
            "dracula",
            true,
            &Default::default(),
        )
        .unwrap();
    assert_eq!(
        Config::load(None, Some(dir.path()), false)
            .unwrap()
            .options
            .theme
            .as_deref(),
        Some("dracula")
    );
}

#[test]
fn palette_theme_permanent_save_respects_later_layers_and_clears_theme_override() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.kdl"), "theme \"default\"\n").unwrap();
    std::fs::create_dir(dir.path().join("config.d")).unwrap();
    std::fs::write(
        dir.path().join("config.d/zz-theme.kdl"),
        "theme \"nightfox\"\n",
    )
    .unwrap();
    let base = loaded_theme_config(dir.path());
    let (mut session, _screen_rx) = session(dir.path());
    session
        .session_configuration
        .set_saved_configuration(base.clone());
    session
        .session_configuration
        .set_client_runtime_configuration(ClientId::new(1), base);
    session
        .apply_palette_theme(
            ClientId::new(1),
            uuid::Uuid::new_v4(),
            "dracula",
            false,
            &Default::default(),
        )
        .unwrap();
    assert_eq!(
        session
            .session_configuration
            .get_client_configuration(&ClientId::new(1))
            .options
            .theme
            .as_deref(),
        Some("dracula")
    );
    session
        .apply_palette_theme(
            ClientId::new(1),
            uuid::Uuid::new_v4(),
            "dayfox",
            true,
            &Default::default(),
        )
        .unwrap();
    let saved = loaded_theme_config(dir.path());
    assert_eq!(saved.options.theme.as_deref(), Some("nightfox"));
    assert_eq!(
        session
            .session_configuration
            .get_client_configuration(&ClientId::new(1))
            .options
            .theme,
        saved.options.theme
    );
    assert!(session.session_configuration.editor_overrides.is_empty());
    std::fs::remove_file(dir.path().join("config.d/zz-theme.kdl")).unwrap();
    assert_eq!(
        Config::load(None, Some(dir.path()), false)
            .unwrap()
            .options
            .theme
            .as_deref(),
        Some("dayfox")
    );
}

#[test]
fn configuration_transaction_rechecks_disk_before_writing_without_a_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.kdl");
    std::fs::write(&path, "mouse_mode true\n").unwrap();
    let (mut session, screen_rx) = session(dir.path());
    let base = Config::load(None, Some(dir.path()), false).unwrap();
    session
        .session_configuration
        .set_saved_configuration(base.clone());
    session
        .session_configuration
        .set_client_runtime_configuration(ClientId::new(1), base);
    let disk = Disk::read(None, Some(dir.path())).unwrap();
    let revision = session.session_configuration.editor_revision(&disk);
    let changed = "mouse_mode true\n// external change\n";
    std::fs::write(&path, changed).unwrap();
    let error = session
        .apply_configuration_edit(
            &disk,
            Edit {
                revision: revision.clone(),
                destination: Destination::Permanent,
                patches: vec![mouse(false)],
                reset: vec![],
            },
            None,
            Some((ClientId::new(1), uuid::Uuid::new_v4())),
        )
        .unwrap_err();
    assert!(error.contains("changed on disk"), "{error}");
    assert_eq!(
        session.session_configuration.editor_revision(&disk),
        revision
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), changed);
    assert!(!dir.path().join("config.d/yinz-managed.kdl").exists());
    assert!(screen_rx.try_recv().is_err());
}

#[test]
fn editor_rejects_stale_draft_after_theme_confirmation() {
    for destination in [Destination::Session, Destination::Permanent] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.kdl"), "theme \"default\"\n").unwrap();
        let (mut session, screen_rx) = session(dir.path());
        let base = loaded_theme_config(dir.path());
        session
            .session_configuration
            .set_saved_configuration(base.clone());
        session
            .session_configuration
            .set_client_runtime_configuration(ClientId::new(1), base);
        let disk = Disk::read(None, Some(dir.path())).unwrap();
        let revision = session.session_configuration.editor_revision(&disk);
        session
            .apply_palette_theme(
                ClientId::new(1),
                uuid::Uuid::new_v4(),
                "dracula",
                false,
                &Default::default(),
            )
            .unwrap();
        screen_rx.try_iter().for_each(drop);
        let committed = session.session_configuration.editor_revision(&disk);
        let error = session
            .configuration_editor(Some(Edit {
                revision,
                destination,
                patches: vec![mouse(false)],
                reset: vec![],
            }))
            .unwrap_err();
        assert!(error.contains("Reload"), "{error}");
        assert_eq!(
            session.session_configuration.editor_revision(&disk),
            committed
        );
        assert!(screen_rx.try_recv().is_err());
        assert!(!dir.path().join("config.d/yinz-managed.kdl").exists());
    }
}
