use super::*;
use std::os::unix::net::UnixListener;

#[test]
fn the_pty_broker_socket_is_not_a_session_candidate() {
    let sock_dir = tempfile::tempdir().unwrap();
    let broker_name = yinz_ipc::consts::pty_broker_socket_name(8);
    let _broker = UnixListener::bind(sock_dir.path().join(&broker_name)).unwrap();
    let _session = UnixListener::bind(sock_dir.path().join("live-session")).unwrap();

    let names: Vec<_> = session_socket_candidates_in(sock_dir.path())
        .unwrap()
        .into_iter()
        .map(|(name, _)| name.as_str().to_owned())
        .collect();

    assert_eq!(names, vec!["live-session"]);
}

#[test]
fn the_pty_broker_socket_name_is_reserved() {
    let error = validate_session_name("pty-broker-v8.sock").unwrap_err();
    assert!(error.contains("pty broker"), "{error}");
    assert!(validate_session_name("pty-broker-v8").is_ok());
    assert!(validate_session_name("my-session").is_ok());
}

#[test]
fn control_agents_are_discovered_only_for_sessions_with_local_sockets() {
    use yinz_runtime::session_checkpoint::write_checkpoint;
    use yinz_types::internal_ids::TerminalId;
    let sockets = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let session = SessionInfo {
        name: "controller".into(),
        control_agents: vec![TerminalId::new(7)],
        ..Default::default()
    };
    let metadata = crate::metadata::session_info::to_string(&session).unwrap();
    write_checkpoint(
        &cache.path().join(&session.name),
        &metadata,
        &("layout { pane; }".into(), Default::default()),
    )
    .unwrap();
    assert!(read_live_session_states(&"current".into(), sockets.path(), cache.path()).is_empty());
    let socket_path = sockets.path().join(&session.name);
    let socket = UnixListener::bind(&socket_path).unwrap();
    let live = read_live_session_states(&"current".into(), sockets.path(), cache.path());
    assert_eq!(live[&session.name].control_agents, session.control_agents);
    drop(socket);
    std::fs::remove_file(socket_path).unwrap();
    assert!(read_live_session_states(&"current".into(), sockets.path(), cache.path()).is_empty());
}
