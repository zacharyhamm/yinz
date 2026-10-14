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
