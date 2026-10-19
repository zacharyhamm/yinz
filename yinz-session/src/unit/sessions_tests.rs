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
    assert!(validate_session_name(yinz_ipc::consts::SESSION_OWNERSHIP_DIR).is_err());
}

#[test]
fn metadata_is_discovered_only_for_sessions_with_live_local_sockets() {
    use yinz_runtime::session_checkpoint::write_checkpoint;
    let sockets = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let session = SessionInfo {
        name: "controller".into(),
        connected_clients: 2,
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
    assert_eq!(live[&session.name].name, session.name);
    assert_eq!(live[&session.name].connected_clients, 2);
    drop(socket);
    assert!(read_live_session_states(&"current".into(), sockets.path(), cache.path()).is_empty());
    assert!(socket_path.exists());
}

#[test]
fn stale_socket_probes_do_not_delay_each_other() {
    use yinz_runtime::session_checkpoint::write_checkpoint;

    let sockets = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let live_name = SessionName::new("live-session");
    let mut live_listener = None;
    for name in (0..10)
        .map(|index| SessionName::new(format!("stale-{index}")))
        .chain(std::iter::once(live_name.clone()))
    {
        let session = SessionInfo {
            name: name.clone(),
            ..Default::default()
        };
        write_checkpoint(
            &cache.path().join(&name),
            &crate::metadata::session_info::to_string(&session).unwrap(),
            &("layout { pane; }".into(), Default::default()),
        )
        .unwrap();
        let listener = UnixListener::bind(sockets.path().join(&name)).unwrap();
        if name == live_name {
            live_listener = Some(listener);
        }
    }

    let started = Instant::now();
    let sessions = read_live_session_states(&"current".into(), sockets.path(), cache.path());
    let elapsed = started.elapsed();
    assert_eq!(sessions.keys().collect::<Vec<_>>(), vec![&live_name]);
    // BSD retries take 500 ms per stale socket: serial probes would take five seconds.
    assert!(
        elapsed < Duration::from_millis(2500),
        "scan took {elapsed:?}"
    );
    for index in 0..10 {
        assert!(sockets.path().join(format!("stale-{index}")).exists());
    }
    drop(live_listener);
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
    let socket = root.path().join("socket");
    let deletion = std::thread::spawn(move || {
        delete_session_after_shutdown(&target, &socket, Ok(SessionProbe::Alive), || {
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
    let socket = root.path().join("socket");
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
            assert!(
                delete_session_after_shutdown(&folder, &socket, status, || Err(kind.into()))
                    .is_err()
            );
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
            delete_session_after_shutdown(&folder, &socket, status, || panic!(
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
    delete_session_after_shutdown(&folder, &socket, Ok(SessionProbe::Absent), || {
        panic!("absent server needs no shutdown")
    })
    .unwrap();
    assert!(!folder.exists());
    assert_eq!(
        delete_session_after_shutdown(
            &folder,
            &socket,
            Ok(SessionProbe::Absent),
            || unreachable!()
        )
        .unwrap_err()
        .kind(),
        io::ErrorKind::NotFound
    );
    fs::write(&folder, "not a directory").unwrap();
    assert!(delete_session_after_shutdown(
        &folder,
        &socket,
        Ok(SessionProbe::Absent),
        || unreachable!()
    )
    .is_err());
    assert_eq!(fs::read_to_string(folder).unwrap(), "not a directory");
}

#[test]
fn deletion_preserves_owned_sessions_even_without_a_socket() {
    use yinz_ipc::session_ownership::SessionOwnership;
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("checkpoint");
    let socket = root.path().join("socket");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("layout"), "saved panes").unwrap();
    let owner = SessionOwnership::acquire(&socket).unwrap().unwrap();
    assert!(delete_session_folder(&folder, &socket, false).is_err());
    let listener = owner.bind().unwrap();
    assert!(delete_session_folder(&folder, &socket, false).is_err());
    owner.shutdown().unwrap();
    assert!(delete_session_folder(&folder, &socket, false).is_err());
    assert_eq!(
        fs::read_to_string(folder.join("layout")).unwrap(),
        "saved panes"
    );
    drop(listener);
    drop(owner);
    delete_session_folder(&folder, &socket, false).unwrap();
    assert!(!folder.exists());
}

#[test]
fn deletion_preserves_servers_without_claims_and_replacements_after_shutdown() {
    use yinz_ipc::session_ownership::SessionOwnership;
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("checkpoint");
    let socket = root.path().join("socket");
    fs::create_dir(&folder).unwrap();
    let listener = UnixListener::bind(&socket).unwrap();
    assert!(delete_session_folder(&folder, &socket, false).is_err());
    drop(listener);
    fs::remove_file(&socket).unwrap();
    let mut replacement = None;
    let result = delete_session_after_shutdown(&folder, &socket, Ok(SessionProbe::Alive), || {
        let owner = SessionOwnership::acquire(&socket).unwrap().unwrap();
        let listener = owner.bind().unwrap();
        replacement = Some((owner, listener));
        Ok(())
    });
    assert!(result.is_err());
    assert!(folder.exists());
    drop(replacement);
}

#[test]
fn discovery_leaves_stale_sockets_for_the_owner_to_reclaim() {
    use yinz_ipc::session_ownership::SessionOwnership;
    std::fs::create_dir_all(&*YINZ_SOCK_DIR).unwrap();
    let temporary = tempfile::NamedTempFile::new_in(&*YINZ_SOCK_DIR).unwrap();
    let socket = temporary.path().to_owned();
    let name = SessionName::new(socket.file_name().unwrap().to_str().unwrap());
    drop(temporary);
    drop(UnixListener::bind(&socket).unwrap());
    assert_eq!(session_status(&name).unwrap(), SessionProbe::Absent);
    assert!(socket.exists());
    let owner = SessionOwnership::acquire(&socket).unwrap().unwrap();
    let _listener = owner.bind().unwrap();
    let candidates = session_socket_candidates_in(&YINZ_SOCK_DIR).unwrap();
    assert!(candidates.iter().any(|(candidate, _)| candidate == &name));
    assert!(!candidates
        .iter()
        .any(|(candidate, _)| candidate.as_str() == ".session-locks"));
}
