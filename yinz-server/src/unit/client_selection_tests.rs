use super::{ClientId, SessionState};

fn noninteractive_connections() -> (SessionState, [ClientId; 5]) {
    let mut session = SessionState::new();
    let cli = session.new_client().unwrap();
    let pipe = session.new_client().unwrap();
    session.associate_pipe_with_client("test-pipe".into(), pipe);
    // Subscriptions keep their connection without registering an interactive client.
    let events = session.new_client().unwrap();
    let pane_renders = session.new_client().unwrap();
    let watcher = session.new_client().unwrap();
    session.interactive_clients.insert(watcher);
    session.convert_client_to_watcher(watcher);
    (session, [cli, pipe, events, pane_renders, watcher])
}

#[test]
fn only_interactive_clients_keep_the_session_active() {
    let (mut session, [cli, pipe, events, pane_renders, watcher]) = noninteractive_connections();
    assert!(!session.active_clients_are_connected());

    let first = session.new_client().unwrap();
    let second = session.new_client().unwrap();
    session.interactive_clients.extend([first, second]);
    assert!(session.active_clients_are_connected());
    session.remove_client(first);
    assert!(session.active_clients_are_connected());
    session.remove_client(second);
    assert!(!session.active_clients_are_connected());
    assert_eq!(session.clients, [cli, pipe, events, pane_renders].into());
    assert_eq!(session.watcher_client_ids(), vec![watcher]);
}

#[test]
fn host_queries_ignore_noninteractive_and_stale_last_active_clients() {
    let (mut session, noninteractive) = noninteractive_connections();
    let older = session.new_client().unwrap();
    let newer = session.new_client().unwrap();
    session.interactive_clients.extend([newer, older]);
    let stale = session.new_client().unwrap();
    session.remove_client(stale);

    for last_active in [None, Some(stale)]
        .into_iter()
        .chain(noninteractive.into_iter().map(Some))
    {
        session.last_active_client = last_active;
        assert_eq!(session.pick_forward_target(), Some(older));
    }
    session.set_last_active_client(newer);
    assert_eq!(session.pick_forward_target(), Some(newer));
    session.remove_client(newer);
    assert_eq!(session.pick_forward_target(), Some(older));
    session.remove_client(older);
    assert_eq!(session.pick_forward_target(), None);
    for client in noninteractive {
        session.set_last_active_client(client);
        assert_eq!(session.pick_forward_target(), None);
    }
}
