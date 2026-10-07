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

#[test]
fn force_delete_waits_for_shutdown_before_removing_checkpoint() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("session");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("layout"), "floating-plugin-marker").unwrap();
    let (started, received) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let target = folder.clone();
    let deletion = std::thread::spawn(move || {
        delete_session_after_shutdown(&target, Ok(SessionProbe::Alive), || {
            started.send(()).unwrap();
            released.recv().unwrap();
            fs::write(target.join("layout"), "final checkpoint")?;
            Ok(())
        })
    });
    received.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        fs::read_to_string(folder.join("layout")).unwrap(),
        "floating-plugin-marker"
    );
    assert!(!deletion.is_finished());
    release.send(()).unwrap();
    deletion.join().unwrap().unwrap();
    assert!(!folder.exists());
}

#[test]
fn force_delete_preserves_checkpoint_on_shutdown_failure() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("session");
    fs::create_dir(&folder).unwrap();
    let layout = folder.join("layout");
    fs::write(&layout, "floating-plugin-marker").unwrap();
    for status in [Ok(SessionProbe::Alive), Ok(SessionProbe::Unresponsive)] {
        for kind in [
            io::ErrorKind::Other,
            io::ErrorKind::UnexpectedEof,
            io::ErrorKind::TimedOut,
            io::ErrorKind::NotFound,
        ] {
            assert!(delete_session_after_shutdown(&folder, status, || Err(kind.into())).is_err());
            assert_eq!(
                fs::read_to_string(&layout).unwrap(),
                "floating-plugin-marker"
            );
        }
    }
    for status in [
        Ok(SessionProbe::Denied),
        Err(io::ErrorKind::PermissionDenied),
    ] {
        assert_eq!(
            delete_session_after_shutdown(&folder, status, || panic!(
                "denied probe attempted shutdown"
            ))
            .unwrap_err()
            .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            fs::read_to_string(&layout).unwrap(),
            "floating-plugin-marker"
        );
    }
    delete_session_after_shutdown(&folder, Ok(SessionProbe::Absent), || {
        panic!("absent server needs no shutdown")
    })
    .unwrap();
    assert!(!folder.exists());
    assert_eq!(
        delete_session_after_shutdown(&folder, Ok(SessionProbe::Absent), || unreachable!())
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
    fs::write(&folder, "not a directory").unwrap();
    assert!(
        delete_session_after_shutdown(&folder, Ok(SessionProbe::Absent), || unreachable!())
            .is_err()
    );
    assert_eq!(fs::read_to_string(folder).unwrap(), "not a directory");
}
