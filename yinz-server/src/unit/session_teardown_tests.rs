use super::*;
use crate::plugins::PluginInstruction;
use crate::thread_bus::ThreadSenders;
use std::collections::HashMap;
use yinz_runtime::channels::{self, SenderWithContext};
use yinz_types::diagnostics::ErrorContext;
use yinz_types::internal_ids::ClientId;

/// A session whose screen and plugin channels are the given ones, and whose sends fail loudly.
fn session_with(
    to_screen: SenderWithContext<ScreenInstruction>,
    to_plugin: SenderWithContext<PluginInstruction>,
) -> SessionMetaData {
    let senders = ThreadSenders {
        to_screen: Some(to_screen),
        to_plugin: Some(to_plugin),
        should_silently_fail: false,
        ..Default::default()
    };
    SessionMetaData {
        senders,
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
        config_dir: None,
        shutdown: Default::default(),
    }
}

#[test]
fn detaching_a_client_after_the_screen_and_plugin_threads_exited_does_not_panic() {
    let (screen_tx, screen_rx) = channels::unbounded();
    let (plugin_tx, plugin_rx) = channels::unbounded();
    let session_data = Arc::new(RwLock::new(Some(session_with(
        SenderWithContext::new(screen_tx),
        SenderWithContext::new(plugin_tx),
    ))));
    drop(screen_rx);
    drop(plugin_rx);

    let ran = with_session(&session_data, |session| {
        session.detach_client(ClientId::new(1))
    });

    assert_eq!(
        ran,
        Some(()),
        "the session was there, the sends just failed"
    );
}

#[test]
fn detaching_a_client_before_the_session_exists_is_a_no_op() {
    let session_data: Arc<RwLock<Option<SessionMetaData>>> = Arc::new(RwLock::new(None));

    let ran = with_session(&session_data, |session| {
        session.detach_client(ClientId::new(1))
    });

    assert_eq!(ran, None);
}

#[test]
fn mouse_updates_reach_every_interactive_client_with_a_full_server_queue() {
    let (screen_tx, _screen_rx) = channels::unbounded();
    let (plugin_tx, _plugin_rx) = channels::unbounded();
    let (server_tx, server_rx) = channels::bounded(50);
    for _ in 0..50 {
        server_tx
            .send((
                ServerInstruction::ConnStatus(ClientId::new(1)),
                ErrorContext::default(),
            ))
            .unwrap();
    }
    let mut session = session_with(
        SenderWithContext::new(screen_tx),
        SenderWithContext::new(plugin_tx),
    );
    session.senders.to_server = Some(SenderWithContext::new(server_tx));
    session.senders.should_silently_fail = true;
    let (done_tx, done_rx) = channels::bounded(1);
    let propagation = std::thread::spawn(move || {
        session.propagate_configuration_changes(
            (1..=64)
                .map(|id| {
                    let mut config = Config::default();
                    config.options.mouse_mode = Some(id % 2 == 0);
                    (ClientId::new(id), config)
                })
                .collect(),
            false,
        );
        done_tx.send(()).unwrap();
        session
    });
    if let Err(error) = done_rx.recv_timeout(Duration::from_secs(10)) {
        // Release a regressed self-send before joining so failure cannot strand a test thread.
        drop(server_rx);
        propagation.join().unwrap();
        panic!("configuration propagation blocked on its own full queue: {error}");
    }
    let session = propagation.join().unwrap();
    let session = Arc::new(RwLock::new(Some(session)));
    let state = Arc::new(RwLock::new(SessionState::new()));
    for _ in 1..=64 {
        let mut state = state.write().unwrap();
        let client = state.new_client().unwrap();
        state.interactive_clients.insert(client);
    }
    let capture = crate::capturing_os::CapturingOs::default();
    let mut os: Box<dyn ServerOsApi> = Box::new(capture.clone());
    flush_pending_mouse_updates(&mut os, &state, &session).unwrap();
    flush_pending_mouse_updates(&mut os, &state, &session).unwrap();
    let messages = capture.server_to_client_messages.lock();
    assert_eq!(messages.len(), 64);
    for id in 1..=64 {
        assert!(matches!(
            messages[&ClientId::new(id)].as_slice(),
            [ServerToClientMsg::SetMouseMode { enabled }] if *enabled == (id % 2 == 0)
        ));
    }
    assert!(with_session(&session, |session| session.pending_mouse_updates.is_empty()).unwrap());
    assert_eq!(server_rx.len(), 50);
}

#[test]
fn terminal_notifications_exclude_noninteractive_clients_even_with_configuration_entries() {
    let (screen_tx, _screen_rx) = channels::unbounded();
    let (plugin_tx, _plugin_rx) = channels::unbounded();
    let mut session = session_with(
        SenderWithContext::new(screen_tx),
        SenderWithContext::new(plugin_tx),
    );
    session.senders.should_silently_fail = true;
    let mut state = SessionState::new();
    let interactive = state.new_client().unwrap();
    let cli = state.new_client().unwrap();
    let watcher = state.new_client().unwrap();
    let detached = state.new_client().unwrap();
    let also_detached = state.new_client().unwrap();
    state
        .interactive_clients
        .extend([interactive, watcher, detached, also_detached]);
    state.convert_client_to_watcher(watcher);
    state.remove_client(detached);
    state.remove_client(also_detached);

    for client in [interactive, cli, watcher, detached, also_detached] {
        session
            .session_configuration
            .set_client_runtime_configuration(client, Config::default());
    }
    let mut config = Config::default();
    config.options.mouse_mode = Some(false);
    let changes = session.session_configuration.change_saved_config(config);
    assert_eq!(changes.len(), 5);
    session.propagate_configuration_changes(changes, true);
    assert_eq!(state.interactive_client_ids(), vec![interactive]);
    let session = Arc::new(RwLock::new(Some(session)));
    let state = Arc::new(RwLock::new(state));
    let capture = crate::capturing_os::CapturingOs::default();
    let mut os: Box<dyn ServerOsApi> = Box::new(capture.clone());

    flush_pending_mouse_updates(&mut os, &state, &session).unwrap();

    assert_eq!(
        *capture.server_to_client_messages.lock(),
        HashMap::from([(
            interactive,
            vec![ServerToClientMsg::SetMouseMode { enabled: false }]
        )])
    );
    assert!(with_session(&session, |session| session.pending_mouse_updates.is_empty()).unwrap());
}

#[test]
fn failed_mouse_delivery_removes_the_client_without_holding_session_locks() {
    let (screen_tx, _screen_rx) = channels::unbounded();
    let (plugin_tx, _plugin_rx) = channels::unbounded();
    let mut session = session_with(
        SenderWithContext::new(screen_tx),
        SenderWithContext::new(plugin_tx),
    );
    let mut state = SessionState::new();
    let failed = state.new_client().unwrap();
    let connected = state.new_client().unwrap();
    state.interactive_clients.extend([failed, connected]);
    session.pending_mouse_updates = vec![(failed, true), (connected, false)];
    let session = Arc::new(RwLock::new(Some(session)));
    let state = Arc::new(RwLock::new(state));
    let capture = crate::capturing_os::CapturingOs {
        fail_client: Some(failed),
        ..Default::default()
    };
    let mut os: Box<dyn ServerOsApi> = Box::new(capture.clone());
    let (done, finished) = channels::bounded(1);
    let flushing = {
        let state = state.clone();
        std::thread::spawn(move || {
            flush_pending_mouse_updates(&mut os, &state, &session).unwrap();
            done.send(()).unwrap();
        })
    };
    finished
        .recv_timeout(Duration::from_secs(10))
        .expect("mouse delivery deadlocked");
    flushing.join().unwrap();
    assert_eq!(
        state.read().unwrap().interactive_client_ids(),
        vec![connected]
    );
    assert_eq!(
        *capture.server_to_client_messages.lock(),
        HashMap::from([(
            connected,
            vec![ServerToClientMsg::SetMouseMode { enabled: false }]
        )])
    );
}

#[test]
fn detach_client_tells_both_threads_once() {
    let (screen_tx, screen_rx) = channels::unbounded::<(ScreenInstruction, ErrorContext)>();
    let (plugin_tx, plugin_rx) = channels::unbounded::<(PluginInstruction, ErrorContext)>();
    let session = session_with(
        SenderWithContext::new(screen_tx),
        SenderWithContext::new(plugin_tx),
    );

    session.detach_client(ClientId::new(7));

    let screen: Vec<_> = screen_rx.try_iter().map(|(i, _)| i).collect();
    let plugin: Vec<_> = plugin_rx.try_iter().map(|(i, _)| i).collect();
    assert!(
        matches!(screen.as_slice(), [ScreenInstruction::RemoveClient(id)] if *id == ClientId::new(7)),
        "screen got {screen:?}"
    );
    assert!(
        matches!(plugin.as_slice(), [PluginInstruction::RemoveClient(id)] if *id == ClientId::new(7)),
        "plugins got {plugin:?}"
    );
}

#[test]
fn removal_queues_screen_cleanup_before_plugin_cleanup() {
    let (screen_tx, screen_rx) = channels::unbounded();
    // Pause cleanup at the plugin queue, after the screen has been notified.
    let (plugin_tx, plugin_rx) = channels::bounded(0);
    let session = Arc::new(RwLock::new(Some(session_with(
        SenderWithContext::new(screen_tx),
        SenderWithContext::new(plugin_tx),
    ))));
    let state = Arc::new(RwLock::new(SessionState::new()));
    let old = state.write().unwrap().new_client().unwrap();
    let removing_state = state.clone();
    let removing_session = session.clone();
    let removing = std::thread::spawn(move || {
        let mut os: Box<dyn ServerOsApi> = Box::new(crate::capturing_os::CapturingOs::default());
        remove_client_and_flush_forwards(old, &mut os, &removing_state, &removing_session);
    });
    let cleanup = screen_rx.recv_timeout(Duration::from_secs(2));
    // Always release the sender before asserting, including on a regression.
    let plugin_cleanup = plugin_rx.recv_timeout(Duration::from_secs(2));
    drop(plugin_rx);
    removing.join().unwrap();
    assert!(matches!(cleanup.unwrap().0, ScreenInstruction::RemoveClient(id) if id == old));
    assert!(matches!(plugin_cleanup.unwrap().0, PluginInstruction::RemoveClient(id) if id == old));
}

#[test]
fn a_repeat_removal_leaves_newer_connections_alone() {
    let state = Arc::new(RwLock::new(SessionState::new()));
    let session = Arc::new(RwLock::new(None));
    let mut os: Box<dyn ServerOsApi> = Box::new(crate::capturing_os::CapturingOs::default());
    let old = state.write().unwrap().new_client().unwrap();
    // A send failure removes the connection before its router queues RemoveClient.
    remove_client_and_flush_forwards(old, &mut os, &state, &session);
    let attached = state.write().unwrap().new_client().unwrap();
    assert_ne!(attached, old);
    // The router's own removal follows.
    remove_client_and_flush_forwards(old, &mut os, &state, &session);
    assert!(state.read().unwrap().client_ids().contains(&attached));
}

#[test]
fn disconnect_retires_only_the_clients_cli_pipes() {
    let (screen_tx, _screen_rx) = channels::unbounded();
    let (plugin_tx, plugin_rx) = channels::unbounded();
    let session = Arc::new(RwLock::new(Some(session_with(
        SenderWithContext::new(screen_tx),
        SenderWithContext::new(plugin_tx),
    ))));
    let state = Arc::new(RwLock::new(SessionState::new()));
    let (disconnected, other) = {
        let mut state = state.write().unwrap();
        let disconnected = state.new_client().unwrap();
        let other = state.new_client().unwrap();
        state.associate_pipe_with_client("repl".into(), disconnected);
        state.associate_pipe_with_client("other".into(), other);
        (disconnected, other)
    };
    let mut os: Box<dyn ServerOsApi> = Box::new(crate::capturing_os::CapturingOs::default());
    remove_client_and_flush_forwards(disconnected, &mut os, &state, &session);
    let cleanup: Vec<_> = plugin_rx
        .try_iter()
        .map(|(instruction, _)| instruction)
        .collect();
    assert!(
        matches!(cleanup.as_slice(), [
        PluginInstruction::CliPipesDisconnected(pipes),
        PluginInstruction::RemoveClient(id),
    ] if pipes == &["repl"] && *id == disconnected),
        "{cleanup:?}"
    );
    assert_eq!(
        state.read().unwrap().pipes,
        HashMap::from([("other".into(), other)])
    );
    remove_client_and_flush_forwards(disconnected, &mut os, &state, &session);
    assert!(plugin_rx.try_iter().all(|(instruction, _)| !matches!(
        instruction,
        PluginInstruction::CliPipesDisconnected(_)
    )));
}

/// A client that leaves takes its passthrough, mode and configuration with it. A plugin unload
/// re-pushes configuration to every client the session still has one for, and a client left in
/// that list was sent a mouse mode it could no longer receive.
#[test]
fn a_departed_client_leaves_no_state_behind() {
    let (screen_tx, _screen_rx) = channels::unbounded();
    let (plugin_tx, _plugin_rx) = channels::unbounded();
    let mut session = session_with(
        SenderWithContext::new(screen_tx),
        SenderWithContext::new(plugin_tx),
    );
    session.senders.should_silently_fail = true;
    let state = Arc::new(RwLock::new(SessionState::new()));
    let (departed, attached) = {
        let mut state = state.write().unwrap();
        let clients = (state.new_client().unwrap(), state.new_client().unwrap());
        state.interactive_clients.extend([clients.0, clients.1]);
        clients
    };
    for (client, pane) in [(departed, 1), (attached, 2)] {
        session
            .session_configuration
            .set_client_runtime_configuration(client, Config::default());
        session
            .current_input_modes
            .insert(client, InputMode::Locked);
        session.key_passthrough_clients.insert(
            client,
            PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(pane)),
        );
    }
    let session = Arc::new(RwLock::new(Some(session)));
    let capture = crate::capturing_os::CapturingOs::default();
    let mut os: Box<dyn ServerOsApi> = Box::new(capture.clone());
    remove_client_and_flush_forwards(departed, &mut os, &state, &session);
    let cli = state.write().unwrap().new_client().unwrap();
    assert_ne!(
        cli, departed,
        "a new connection must not reuse a departed client's id"
    );
    with_session(&session, |session| {
        assert!(!session.key_passthrough_clients.contains_key(&departed));
        assert!(!session.current_input_modes.contains_key(&departed));
        let configured: Vec<_> = session
            .session_configuration
            .runtime_config
            .keys()
            .copied()
            .collect();
        assert_eq!(configured, vec![attached]);
        assert!(session.key_passthrough_clients.contains_key(&attached));
        assert!(session.current_input_modes.contains_key(&attached));
    })
    .unwrap();

    // What the server does with the `RebindKeys` every plugin unload sends.
    with_session_mut(&session, |session| {
        let changes = session
            .session_configuration
            .register_plugin_default_keybinds(PluginId::new(8), Keybinds::default());
        session.propagate_configuration_changes(changes, false);
    });
    flush_pending_mouse_updates(&mut os, &state, &session).unwrap();

    let messages = capture.server_to_client_messages.lock();
    for gone_or_cli in [departed, cli] {
        assert!(
            messages.get(&gone_or_cli).is_none_or(Vec::is_empty),
            "client {gone_or_cli} was sent {:?}",
            messages.get(&gone_or_cli)
        );
    }
    assert!(matches!(
        messages.get(&attached).map(Vec::as_slice),
        Some([ServerToClientMsg::SetMouseMode { .. }])
    ));
}

/// A plugin keeps the id of the client that loaded it, and an instruction can be queued before its
/// client leaves and handled after, so a `RebindKeys`, `Reconfigure`, `ChangeMode` or
/// `KeyPassthroughChanged` can name a client that has gone. Handling one put that client's state
/// back for good. A runtime-only change from it is dropped while another client is attached:
/// applied to the session, it would replace that client's settings with the saved config's.
#[test]
fn late_instructions_for_a_departed_client_leave_nothing_behind() {
    let (screen_tx, screen_rx) = channels::unbounded();
    let (plugin_tx, _plugin_rx) = channels::unbounded();
    let mut session = session_with(
        SenderWithContext::new(screen_tx),
        SenderWithContext::new(plugin_tx),
    );
    session.senders.should_silently_fail = true;
    let state = Arc::new(RwLock::new(SessionState::new()));
    let (departed, attached) = {
        let mut state = state.write().unwrap();
        let clients = (state.new_client().unwrap(), state.new_client().unwrap());
        state.interactive_clients.extend([clients.0, clients.1]);
        clients
    };
    for client in [departed, attached] {
        session
            .session_configuration
            .set_client_runtime_configuration(client, Config::default());
    }
    let session = Arc::new(RwLock::new(Some(session)));
    let capture = crate::capturing_os::CapturingOs::default();
    let mut os: Box<dyn ServerOsApi> = Box::new(capture.clone());
    remove_client_and_flush_forwards(departed, &mut os, &state, &session);
    let cli = state.write().unwrap().new_client().unwrap();
    while screen_rx.try_recv().is_ok() {}

    // What the arms do with a plugin's requests for each client.
    for (client, pane) in [(departed, 1), (attached, 2)] {
        rebind_client_keys(
            client,
            vec![(
                InputMode::Normal,
                "Ctrl g".parse::<KeyWithModifier>().unwrap(),
                vec![Action::NoOp],
            )],
            vec![],
            false,
            &state,
            &session,
        )
        .unwrap();
        reconfigure_client(
            client,
            "mouse_mode false".to_owned(),
            false,
            &state,
            &session,
        )
        .unwrap();
        change_client_mode(client, InputMode::Pane, None, &state, &session).unwrap();
        change_key_passthrough(
            client,
            PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(pane)),
            true,
            None,
            true,
            &state,
            &session,
        )
        .unwrap();
    }
    flush_pending_mouse_updates(&mut os, &state, &session).unwrap();

    let messages = capture.server_to_client_messages.lock();
    for gone_or_cli in [departed, cli] {
        assert!(
            messages.get(&gone_or_cli).is_none_or(Vec::is_empty),
            "client {gone_or_cli} was sent {:?}",
            messages.get(&gone_or_cli)
        );
    }
    assert_eq!(
        messages.get(&attached).and_then(|sent| sent.last()),
        Some(&ServerToClientMsg::SetMouseMode { enabled: false })
    );
    drop(messages);
    with_session(&session, |session| {
        assert!(!session.current_input_modes.contains_key(&departed));
        assert!(!session.key_passthrough_clients.contains_key(&departed));
        let configured: Vec<_> = session
            .session_configuration
            .runtime_config
            .keys()
            .copied()
            .collect();
        assert_eq!(configured, vec![attached]);
        assert_eq!(
            session.current_input_modes.get(&attached),
            Some(&InputMode::Pane)
        );
        assert!(session.key_passthrough_clients.contains_key(&attached));
    })
    .unwrap();
    while let Ok((instruction, _)) = screen_rx.try_recv() {
        match instruction {
            ScreenInstruction::ChangeMode(_, _, client, _)
            | ScreenInstruction::Reconfigure {
                client_id: client, ..
            } if client == departed || client == cli || client == ClientId::NONE => {
                panic!("the screen heard a change for client {client}: {instruction:?}")
            },
            _ => {},
        }
    }
}

/// `yinz action switch-mode` with nobody attached has no client to switch, and says so instead
/// of reporting success.
#[test]
fn a_mode_change_for_a_client_that_is_not_attached_fails() {
    let (screen_tx, screen_rx) = channels::unbounded();
    let (plugin_tx, _plugin_rx) = channels::unbounded();
    let session = Arc::new(RwLock::new(Some(session_with(
        SenderWithContext::new(screen_tx),
        SenderWithContext::new(plugin_tx),
    ))));
    let state = Arc::new(RwLock::new(SessionState::new()));
    let cli = state.write().unwrap().new_client().unwrap();
    let (completion_tx, mut completion_rx) = tokio::sync::oneshot::channel();
    change_client_mode(
        cli,
        InputMode::Locked,
        Some(yinz_bus::route::NotificationEnd::new(completion_tx)),
        &state,
        &session,
    )
    .unwrap();
    let result = completion_rx
        .try_recv()
        .expect("the request was not completed");
    assert_eq!(
        result.error_message.as_deref(),
        Some("No attached client to switch mode for")
    );
    assert!(screen_rx.try_recv().is_err(), "no mode was changed");
    with_session(&session, |session| {
        assert!(session.current_input_modes.is_empty())
    })
    .unwrap();
}

/// With nobody attached there is no client's settings to replace, so a departed client's
/// runtime-only change still reaches the session-wide state the threads keep.
#[test]
fn a_departed_clients_runtime_change_reaches_the_session_with_no_client_attached() {
    let (screen_tx, screen_rx) = channels::unbounded();
    let (plugin_tx, plugin_rx) = channels::unbounded();
    let mut session = session_with(
        SenderWithContext::new(screen_tx),
        SenderWithContext::new(plugin_tx),
    );
    session.senders.should_silently_fail = true;
    let state = Arc::new(RwLock::new(SessionState::new()));
    let departed = {
        let mut state = state.write().unwrap();
        let client = state.new_client().unwrap();
        state.interactive_clients.insert(client);
        client
    };
    session
        .session_configuration
        .set_client_runtime_configuration(departed, Config::default());
    let session = Arc::new(RwLock::new(Some(session)));
    let mut os: Box<dyn ServerOsApi> = Box::new(crate::capturing_os::CapturingOs::default());
    remove_client_and_flush_forwards(departed, &mut os, &state, &session);
    while screen_rx.try_recv().is_ok() {}
    while plugin_rx.try_recv().is_ok() {}

    reconfigure_client(
        departed,
        "mouse_mode false".to_owned(),
        false,
        &state,
        &session,
    )
    .unwrap();

    assert!(screen_rx.try_iter().any(|(instruction, _)| matches!(
        instruction,
        ScreenInstruction::Reconfigure { client_id, .. } if client_id == ClientId::NONE
    )));
    assert!(plugin_rx.try_iter().any(|(instruction, _)| matches!(
        instruction,
        PluginInstruction::Reconfigure { client_id, .. } if client_id == ClientId::NONE
    )));
    with_session(&session, |session| {
        assert!(session.session_configuration.runtime_config.is_empty());
    })
    .unwrap();
}

/// A departed client's key rebinding is a runtime change like any other: with nobody attached it
/// reaches the session-wide state the threads keep, as `reconfigure_client`'s does.
#[test]
fn a_departed_clients_unsaved_rebind_reaches_the_session_with_no_client_attached() {
    let (screen_tx, screen_rx) = channels::unbounded();
    let (plugin_tx, plugin_rx) = channels::unbounded();
    let mut session = session_with(
        SenderWithContext::new(screen_tx),
        SenderWithContext::new(plugin_tx),
    );
    session.senders.should_silently_fail = true;
    let state = Arc::new(RwLock::new(SessionState::new()));
    let departed = {
        let mut state = state.write().unwrap();
        let client = state.new_client().unwrap();
        state.interactive_clients.insert(client);
        client
    };
    session
        .session_configuration
        .set_client_runtime_configuration(departed, Config::default());
    let session = Arc::new(RwLock::new(Some(session)));
    let mut os: Box<dyn ServerOsApi> = Box::new(crate::capturing_os::CapturingOs::default());
    remove_client_and_flush_forwards(departed, &mut os, &state, &session);
    while screen_rx.try_recv().is_ok() {}
    while plugin_rx.try_recv().is_ok() {}

    let key = "Ctrl g".parse::<KeyWithModifier>().unwrap();
    rebind_client_keys(
        departed,
        vec![(InputMode::Normal, key.clone(), vec![Action::NoOp])],
        vec![],
        false,
        &state,
        &session,
    )
    .unwrap();

    let rebound = |keybinds: &Keybinds| {
        keybinds
            .0
            .get(&InputMode::Normal)
            .and_then(|bindings| bindings.get(&key))
            == Some(&vec![Action::NoOp])
    };
    assert!(screen_rx.try_iter().any(|(instruction, _)| matches!(
        instruction,
        ScreenInstruction::Reconfigure { client_id, ref keybinds, .. }
            if client_id == ClientId::NONE && rebound(keybinds)
    )));
    assert!(plugin_rx.try_iter().any(|(instruction, _)| matches!(
        instruction,
        PluginInstruction::Reconfigure { client_id, keybinds: Some(ref keybinds), .. }
            if client_id == ClientId::NONE && rebound(keybinds)
    )));
    with_session(&session, |session| {
        assert!(session.session_configuration.runtime_config.is_empty());
    })
    .unwrap();
}

/// A departed client's save that fails while another client is attached changes nothing: no
/// thread hears of it, and the session's configuration, background plugins included, stays as
/// the attached client is running it. The plugins hear that the write failed.
#[test]
fn a_departed_clients_failed_save_changes_nothing_while_another_client_is_attached() {
    let (screen_tx, screen_rx) = channels::unbounded();
    let (plugin_tx, plugin_rx) = channels::unbounded();
    let mut session = session_with(
        SenderWithContext::new(screen_tx),
        SenderWithContext::new(plugin_tx),
    );
    session.senders.should_silently_fail = true;
    // A config directory that is a file: the managed fragment's folder cannot be made in it.
    let temp = tempfile::tempdir().unwrap();
    let not_a_directory = temp.path().join("config");
    std::fs::write(&not_a_directory, "").unwrap();
    session.config_dir = Some(not_a_directory);
    let state = Arc::new(RwLock::new(SessionState::new()));
    let (departed, attached) = {
        let mut state = state.write().unwrap();
        let clients = (state.new_client().unwrap(), state.new_client().unwrap());
        state.interactive_clients.extend([clients.0, clients.1]);
        clients
    };
    for client in [departed, attached] {
        session
            .session_configuration
            .set_client_runtime_configuration(client, Config::default());
    }
    let session = Arc::new(RwLock::new(Some(session)));
    let mut os: Box<dyn ServerOsApi> = Box::new(crate::capturing_os::CapturingOs::default());
    remove_client_and_flush_forwards(departed, &mut os, &state, &session);
    while screen_rx.try_recv().is_ok() {}
    while plugin_rx.try_recv().is_ok() {}
    let running = |session: &Arc<RwLock<Option<SessionMetaData>>>| {
        with_session(session, |session| {
            session
                .session_configuration
                .get_client_configuration(&attached)
        })
        .unwrap()
    };
    let before = running(&session);

    let saved = reconfigure_client(
        departed,
        "mouse_mode false\nload_plugins { \"file:/unsaved.scm\"; }".to_owned(),
        true,
        &state,
        &session,
    );

    assert!(saved.is_err(), "the fragment could not be written");
    for (instruction, _) in screen_rx.try_iter() {
        assert!(
            !matches!(instruction, ScreenInstruction::Reconfigure { .. }),
            "the screen heard an unsaved change: {instruction:?}"
        );
    }
    let mut reported = false;
    for (instruction, _) in plugin_rx.try_iter() {
        match instruction {
            PluginInstruction::FailedToWriteConfigToDisk { .. } => reported = true,
            PluginInstruction::Reconfigure { .. }
            | PluginInstruction::ReconcileBackgroundPlugins { .. } => {
                panic!("the plugins heard an unsaved change: {instruction:?}")
            },
            _ => {},
        }
    }
    assert!(reported, "the plugins were not told the write failed");
    assert_eq!(running(&session), before);
    with_session(&session, |session| {
        let configured: Vec<_> = session
            .session_configuration
            .runtime_config
            .keys()
            .copied()
            .collect();
        assert_eq!(configured, vec![attached]);
    })
    .unwrap();
}

/// A saved config written while no client is attached still reaches the threads, which keep the
/// session's hooks, aliases and background plugins.
#[test]
fn a_saved_config_reaches_the_threads_with_no_client_attached() {
    let (screen_tx, screen_rx) = channels::unbounded();
    let (plugin_tx, plugin_rx) = channels::unbounded();
    let mut session = session_with(
        SenderWithContext::new(screen_tx),
        SenderWithContext::new(plugin_tx),
    );
    session.senders.should_silently_fail = true;
    let state = Arc::new(RwLock::new(SessionState::new()));
    let detached = {
        let mut state = state.write().unwrap();
        let client = state.new_client().unwrap();
        state.interactive_clients.insert(client);
        client
    };
    session
        .session_configuration
        .set_client_runtime_configuration(detached, Config::default());
    let session = Arc::new(RwLock::new(Some(session)));
    let mut os: Box<dyn ServerOsApi> = Box::new(crate::capturing_os::CapturingOs::default());
    remove_client_and_flush_forwards(detached, &mut os, &state, &session);
    while screen_rx.try_recv().is_ok() {}
    while plugin_rx.try_recv().is_ok() {}

    let mut config = Config::default();
    config.options.mouse_mode = Some(false);
    let changes = with_session_mut(&session, |session| {
        session
            .session_configuration
            .change_saved_config(config.clone())
    })
    .unwrap();
    assert_eq!(
        changes.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![ClientId::NONE]
    );
    with_session_mut(&session, |session| {
        session.propagate_configuration_changes(changes, true)
    });
    assert!(screen_rx.try_iter().any(|(instruction, _)| matches!(
        instruction,
        ScreenInstruction::Reconfigure { client_id, .. } if client_id == ClientId::NONE
    )));
    assert!(plugin_rx.try_iter().any(|(instruction, _)| matches!(
        instruction,
        PluginInstruction::Reconfigure { client_id, .. } if client_id == ClientId::NONE
    )));

    let unchanged = with_session_mut(&session, |session| {
        session.session_configuration.change_saved_config(config)
    })
    .unwrap();
    assert!(unchanged.is_empty(), "an unchanged config is no change");
}

/// A connection can pick up per-client state before it identifies itself as a watcher.
#[test]
fn removing_a_watcher_forgets_its_state() {
    let (screen_tx, _screen_rx) = channels::unbounded();
    let (plugin_tx, plugin_rx) = channels::unbounded();
    let mut session = session_with(
        SenderWithContext::new(screen_tx),
        SenderWithContext::new(plugin_tx),
    );
    session.senders.should_silently_fail = true;
    let state = Arc::new(RwLock::new(SessionState::new()));
    let watcher = state.write().unwrap().new_client().unwrap();
    session
        .session_configuration
        .set_client_runtime_configuration(watcher, Config::default());
    session
        .current_input_modes
        .insert(watcher, InputMode::Locked);
    session.pending_mouse_updates.push((watcher, true));
    state.write().unwrap().convert_client_to_watcher(watcher);
    let session = Arc::new(RwLock::new(Some(session)));
    let mut os: Box<dyn ServerOsApi> = Box::new(crate::capturing_os::CapturingOs::default());

    remove_watcher_client(watcher, &mut os, &state, &session);

    assert!(!state.read().unwrap().is_watcher(&watcher));
    with_session(&session, |session| {
        assert!(session.session_configuration.runtime_config.is_empty());
        assert!(session.current_input_modes.is_empty());
        assert!(session.pending_mouse_updates.is_empty());
    })
    .unwrap();
    assert!(plugin_rx.try_iter().any(|(instruction, _)| matches!(
        instruction,
        PluginInstruction::RemoveClient(id) if id == watcher
    )));
}
