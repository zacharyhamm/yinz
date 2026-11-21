use super::*;
use std::str::FromStr;

fn with_client(id: ClientId) -> SessionState {
    let mut s = SessionState::new();
    s.clients.insert(id);
    s
}

#[test]
fn pick_forward_target_prefers_last_active_when_still_connected() {
    let mut s = SessionState::new();
    s.clients.insert(crate::ClientId::new(1));
    s.clients.insert(crate::ClientId::new(2));
    s.interactive_clients.extend(s.clients.iter().copied());
    s.set_last_active_client(crate::ClientId::new(2));
    assert_eq!(s.pick_forward_target(), Some(ClientId::new(2)));
}

#[test]
fn plugin_default_keybinds_fill_only_free_slots_and_return_after_unbind() {
    let key = KeyWithModifier::from_str("Ctrl g").unwrap();
    let mut base = Config::default();
    base.keybinds
        .0
        .entry(InputMode::Normal)
        .or_default()
        .insert(key.clone(), vec![Action::Quit]);
    let mut configuration = SessionConfiguration::default();
    configuration.set_saved_configuration(base.clone());
    configuration.set_client_runtime_configuration(crate::ClientId::new(1), base);

    let mut defaults = Keybinds::default();
    defaults.0.entry(InputMode::Normal).or_default().insert(
        key.clone(),
        vec![Action::NewPane {
            direction: None,
            pane_name: None,
            start_suppressed: false,
        }],
    );
    configuration.register_plugin_default_keybinds(crate::PluginId::new(2), defaults);
    assert_eq!(
        configuration
            .get_client_keybinds(&ClientId::new(1))
            .get_actions_for_key_in_mode(&InputMode::Normal, &key),
        Some(&vec![Action::Quit])
    );

    let (_, changed) = configuration.rebind_keys(
        &ClientId::new(1),
        vec![],
        vec![(InputMode::Normal, key.clone())],
    );
    assert!(changed);
    assert_eq!(
        configuration
            .get_client_keybinds(&ClientId::new(1))
            .get_actions_for_key_in_mode(&InputMode::Normal, &key),
        Some(&vec![Action::NewPane {
            direction: None,
            pane_name: None,
            start_suppressed: false,
        }])
    );
}

#[test]
fn lower_plugin_id_wins_default_keybind_conflicts_until_it_unloads() {
    let key = KeyWithModifier::from_str("Alt x").unwrap();
    let mut configuration = SessionConfiguration::default();
    configuration.set_client_runtime_configuration(crate::ClientId::new(1), Config::default());
    for (plugin_id, action) in [(9, Action::Quit), (3, Action::NoOp)] {
        let mut defaults = Keybinds::default();
        defaults
            .0
            .entry(InputMode::Normal)
            .or_default()
            .insert(key.clone(), vec![action]);
        configuration.register_plugin_default_keybinds(crate::PluginId::new(plugin_id), defaults);
    }
    assert_eq!(
        configuration
            .get_client_keybinds(&ClientId::new(1))
            .get_actions_for_key_in_mode(&InputMode::Normal, &key),
        Some(&vec![Action::NoOp])
    );

    configuration.register_plugin_default_keybinds(crate::PluginId::new(3), Keybinds::default());
    assert_eq!(
        configuration
            .get_client_keybinds(&ClientId::new(1))
            .get_actions_for_key_in_mode(&InputMode::Normal, &key),
        Some(&vec![Action::Quit])
    );
}

#[test]
fn a_late_reply_detects_that_its_requester_stopped_waiting() {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let completion = NotificationEnd::new(sender);
    assert!(!completion.is_cancelled());
    drop(receiver);
    assert!(completion.is_cancelled());
}

#[test]
fn interactive_clients_exclude_cli_connections_watchers_and_departed_clients() {
    let mut session = SessionState::new();
    let cli = session.new_client().unwrap();
    let interactive = session.new_client().unwrap();
    let watcher = session.new_client().unwrap();
    session.interactive_clients.insert(watcher);
    session.convert_client_to_watcher(watcher);
    session.interactive_clients.insert(interactive);
    assert_eq!(session.interactive_client_ids(), vec![interactive]);
    session.remove_client(interactive);
    assert!(session.interactive_client_ids().is_empty());
    let next = session.new_client().unwrap();
    assert!(
        next > interactive,
        "{next} reuses or precedes {interactive}"
    );
    assert!(session.interactive_client_ids().is_empty());
    assert!(session.client_ids().contains(&cli));
}

#[test]
fn client_ids_start_at_one_and_are_never_reused() {
    let mut session = SessionState::new();
    let first = session.new_client().unwrap();
    assert_eq!(first.get(), 1);
    let watcher = session.new_client().unwrap();
    session.convert_client_to_watcher(watcher);
    session.remove_client(first);
    session.remove_watcher(watcher);
    assert!(session.client_ids().is_empty());
    assert_eq!(session.new_client().map(ClientId::get), Some(3));
}

#[test]
fn a_session_that_has_used_every_client_id_refuses_more() {
    let mut session = SessionState::new();
    session.next_client_id = Some(u32::MAX - 1);
    assert_eq!(session.new_client().map(ClientId::get), Some(u32::MAX - 1));
    assert_eq!(session.new_client().map(ClientId::get), Some(u32::MAX));
    assert_eq!(session.new_client(), None);
    assert_eq!(session.new_client(), None);
    let mut client_ids = session.client_ids();
    client_ids.sort_unstable();
    assert_eq!(
        client_ids
            .into_iter()
            .map(ClientId::get)
            .collect::<Vec<_>>(),
        vec![u32::MAX - 1, u32::MAX]
    );
}

#[test]
fn a_cli_action_acts_for_an_attached_client_whenever_there_is_one() {
    let mut state = SessionState::new();
    let cli = state.new_client().unwrap();
    assert_eq!(
        state.cli_action_target(cli, None),
        cli,
        "nobody is attached"
    );
    let older = state.new_client().unwrap();
    let newer = state.new_client().unwrap();
    state.interactive_clients.extend([older, newer]);
    assert_eq!(
        state.cli_action_target(cli, None),
        older,
        "no key pressed yet: the longest-attached client"
    );
    assert_eq!(state.cli_action_target(cli, Some(newer)), newer);
    state.set_last_active_client(newer);
    assert_eq!(state.cli_action_target(cli, Some(older)), newer);
    state.interactive_clients.remove(&newer);
    assert_eq!(
        state.cli_action_target(cli, None),
        older,
        "a last-active client that is no longer attached does not count"
    );
}

#[test]
fn a_change_for_no_client_leaves_no_client_configuration() {
    let mut configuration = SessionConfiguration::default();
    let (config, changed) = configuration
        .reconfigure_runtime_config(&ClientId::NONE, "mouse_mode false".to_owned(), None, None)
        .unwrap();
    assert!(changed);
    assert_eq!(config.unwrap().options.mouse_mode, Some(false));
    let (_, rebound) = configuration.rebind_keys(
        &ClientId::NONE,
        vec![(
            InputMode::Normal,
            "Ctrl g".parse::<KeyWithModifier>().unwrap(),
            vec![Action::NoOp],
        )],
        vec![],
    );
    assert!(rebound);
    assert!(configuration.runtime_config.is_empty());
    assert!(configuration.editor_effective.is_empty());
    assert!(configuration.effective_keybinds.is_empty());

    let mut saved = Config::default();
    saved.options.mouse_mode = Some(false);
    let changes = configuration.change_saved_config(saved);
    assert_eq!(
        changes.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![ClientId::NONE]
    );
}

#[test]
fn pick_forward_target_falls_back_when_last_active_disconnected() {
    let mut s = SessionState::new();
    s.clients.insert(crate::ClientId::new(1));
    s.clients.insert(crate::ClientId::new(2));
    s.interactive_clients.extend(s.clients.iter().copied());
    // A stale last-active ID must fall back to the lowest interactive client ID.
    s.last_active_client = Some(crate::ClientId::new(3));
    assert_eq!(s.pick_forward_target(), Some(ClientId::new(1)));
}

#[test]
fn pick_forward_target_none_when_no_clients() {
    let s = SessionState::new();
    assert_eq!(s.pick_forward_target(), None);
}

#[test]
fn remove_client_returns_stuck_forward_tokens() {
    let mut s = with_client(crate::ClientId::new(1));
    s.mark_forward_in_flight(10, crate::ClientId::new(1));
    s.mark_forward_in_flight(11, crate::ClientId::new(1));
    // A forward dispatched to a *different* client — must not be
    // returned when we remove client 1.
    s.clients.insert(crate::ClientId::new(2));
    s.mark_forward_in_flight(12, crate::ClientId::new(2));

    let mut stuck = s.remove_client(crate::ClientId::new(1));
    stuck.sort_unstable();
    assert_eq!(stuck, vec![10, 11]);
    assert_eq!(s.forwards_in_flight.get(&12), Some(&ClientId::new(2)));
}

#[test]
fn remove_client_returns_empty_when_no_forwards_in_flight() {
    let mut s = with_client(crate::ClientId::new(1));
    assert!(s.remove_client(crate::ClientId::new(1)).is_empty());
}

#[test]
fn clear_forward_in_flight_removes_entry() {
    let mut s = with_client(crate::ClientId::new(1));
    s.mark_forward_in_flight(42, crate::ClientId::new(1));
    assert!(!s.clear_forward_in_flight(42, ClientId::new(2)));
    assert!(s.clear_forward_in_flight(42, ClientId::new(1)));
    let old = s.new_client().unwrap();
    s.next_client_id = Some(old.get());
    let replacement = s.new_client().unwrap();
    s.mark_forward_in_flight(42, replacement);
    assert!(!s.clear_forward_in_flight(42, old));
    assert!(s.remove_client(old).is_empty());
    assert!(s.clear_forward_in_flight(42, replacement));
    // After clear, removing the client yields no stuck tokens.
    assert!(s.remove_client(crate::ClientId::new(1)).is_empty());
}
#[test]
fn disconnect_targets_exclude_origin_and_watchers_but_include_pipe_clients() {
    let mut state = SessionState::new();
    let origin = state.new_client().unwrap();
    state.watchers.insert(ClientId::new(8));
    assert!(state.disconnect_targets(origin).is_empty());
    let other = state.new_client().unwrap();
    let pipe = state.new_client().unwrap();
    state.associate_pipe_with_client("pipe".into(), pipe);
    assert_eq!(state.disconnect_targets(origin), vec![other, pipe]);
    for id in state.disconnect_targets(origin) {
        state.remove_client(id);
    }
    assert_eq!(state.client_ids(), vec![origin]);
    assert_eq!(state.watcher_client_ids(), vec![ClientId::new(8)]);
}

#[test]
fn attachment_generation_held_screen_output_cannot_ready_ansi_replacement() {
    use interprocess::local_socket::traits::Listener;
    use yinz_ipc::ipc::{ClientToServerMsg, IpcReceiverWithContext};
    let state = Arc::new(RwLock::new(SessionState::new()));
    let data = Arc::new(RwLock::new(None));
    let mut os: Box<dyn ServerOsApi> = Box::new(yinz_server_os::get_server_os_input().unwrap());
    let old = state.write().unwrap().new_client().unwrap();
    let sibling = state.write().unwrap().new_client().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("destination.sock");
    let listener = yinz_ipc::consts::ipc_bind(&path).unwrap();
    let old_stream = yinz_ipc::consts::ipc_connect(&path).unwrap();
    let _old_router = os.new_client(old, listener.accept().unwrap()).unwrap();
    let mut sibling_reader = IpcReceiverWithContext::<ServerToClientMsg>::new(
        yinz_ipc::consts::ipc_connect(&path).unwrap(),
    );
    sibling_reader
        .set_recv_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let _sibling_router = os.new_client(sibling, listener.accept().unwrap()).unwrap();
    let held = ServerInstruction::Render(Some(HashMap::from([
        (old, "old".into()),
        (sibling, "sibling".into()),
    ])));
    let replacement = {
        let mut state = state.write().unwrap();
        state.next_client_id = Some(old.get());
        state.new_client().unwrap()
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let preparing =
        std::thread::spawn(move || yinz_client::prepare_test_destination(&path, deadline));
    let mut router = os
        .new_client(replacement, listener.accept().unwrap())
        .unwrap();
    router
        .set_recv_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    assert!(matches!(
        router.recv_client_msg().msg(),
        Some((ClientToServerMsg::AttachClient { .. }, _))
    ));
    let mut pending = HashMap::from([(replacement, std::time::Instant::now())]);
    for instruction in [
        held,
        ServerInstruction::ClipboardUpdate(HashMap::from([(old, "old clipboard".into())])),
        ServerInstruction::DesktopNotification(HashMap::from([(old, "old notification".into())])),
        ServerInstruction::RepaintComplete(old),
    ] {
        publish_screen_output(instruction, &mut os, &state, &data, &mut pending).unwrap();
    }
    assert!(pending.contains_key(&replacement));
    assert!(
        matches!(sibling_reader.recv_server_msg().msg(), Some((ServerToClientMsg::Render { content }, _)) if content == "sibling")
    );
    publish_screen_output(
        ServerInstruction::Render(Some(HashMap::from([(replacement, "replacement".into())]))),
        &mut os,
        &state,
        &data,
        &mut pending,
    )
    .unwrap();
    assert!(pending.is_empty());
    assert_eq!(
        preparing.join().unwrap().unwrap(),
        vec![ServerToClientMsg::Render {
            content: "replacement".into()
        }]
    );
    drop(old_stream);
}
