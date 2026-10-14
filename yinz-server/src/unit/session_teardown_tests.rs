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
fn mouse_updates_reach_every_client_with_a_full_server_queue() {
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
    let capture = crate::capturing_os::CapturingOs::default();
    let mut os: Box<dyn ServerOsApi> = Box::new(capture.clone());
    flush_pending_mouse_updates(&mut os, &state, &session);
    flush_pending_mouse_updates(&mut os, &state, &session);
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
fn removal_queues_cleanup_before_reusing_the_connection_id() {
    let (screen_tx, screen_rx) = channels::unbounded();
    // Pause cleanup at the plugin queue, after the screen has been notified.
    let (plugin_tx, plugin_rx) = channels::bounded(0);
    let session = Arc::new(RwLock::new(Some(session_with(
        SenderWithContext::new(screen_tx),
        SenderWithContext::new(plugin_tx),
    ))));
    let state = Arc::new(RwLock::new(SessionState::new()));
    let old = state.write().unwrap().new_client();
    let removing_state = state.clone();
    let removing_session = session.clone();
    let removing = std::thread::spawn(move || {
        let mut os: Box<dyn ServerOsApi> = Box::new(crate::capturing_os::CapturingOs::default());
        remove_client_and_flush_forwards(old, &mut os, &removing_state, &removing_session);
    });
    let cleanup = screen_rx.recv_timeout(Duration::from_secs(2));
    let replacement = state.write().unwrap().new_client();
    // Always release the sender before asserting, including on a regression.
    let plugin_cleanup = plugin_rx.recv_timeout(Duration::from_secs(2));
    drop(plugin_rx);
    removing.join().unwrap();
    assert!(matches!(cleanup.unwrap().0, ScreenInstruction::RemoveClient(id) if id == old));
    assert_ne!(replacement, old, "cleanup still owns the old connection ID");
    assert!(matches!(plugin_cleanup.unwrap().0, PluginInstruction::RemoveClient(id) if id == old));
    state.write().unwrap().release_client_id(old);
    assert_eq!(state.write().unwrap().new_client(), old);
}

#[test]
fn removed_client_id_stays_reserved_until_its_router_finishes() {
    let state = Arc::new(RwLock::new(SessionState::new()));
    let session = Arc::new(RwLock::new(None));
    let mut os: Box<dyn ServerOsApi> = Box::new(crate::capturing_os::CapturingOs::default());
    let old = state.write().unwrap().new_client();
    // A send failure removes the connection before its router queues RemoveClient.
    remove_client_and_flush_forwards(old, &mut os, &state, &session);
    let attached = state.write().unwrap().new_client();
    assert_ne!(attached, old, "the old router still owns a pending removal");
    remove_client_and_flush_forwards(old, &mut os, &state, &session);
    assert!(state.read().unwrap().client_ids().contains(&attached));
    state.write().unwrap().release_client_id(old);
    assert_eq!(state.write().unwrap().new_client(), old);
}
