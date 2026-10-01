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
