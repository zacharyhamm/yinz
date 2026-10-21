use super::*;

#[test]
fn startup_cancellation_unregisters_handlers_on_scope_exit() {
    if yinz_test_support::reexec_test() {
        return;
    }
    let first = StartupCancellation::new().unwrap();
    let unregistered = first.cancelled.clone();
    drop(first);
    let current = StartupCancellation::new().unwrap();
    for signal in [SIGINT, SIGTERM, SIGQUIT, SIGHUP] {
        current.cancelled.store(false, Ordering::Release);
        signal_hook::low_level::raise(signal).unwrap();
        assert!(current.cancelled.load(Ordering::Acquire));
        assert!(!unregistered.load(Ordering::Acquire));
    }
}

fn endpoint() -> Endpoint {
    Endpoint {
        session_name: "remote work".to_owned(),
        socket: "/remote/run/cv1/remote work".into(),
        yinz_version: VERSION.to_owned(),
        contract_version: CLIENT_SERVER_CONTRACT_VERSION,
    }
}

#[test]
fn remote_endpoint_requires_bounded_matching_identity_and_versions() {
    let mut e = endpoint();
    let parse = |e: &Endpoint| parse_endpoint(&serde_json::to_vec(e).unwrap(), "remote work");
    assert!(parse(&e).is_ok());
    e.yinz_version = "different".into();
    let error = parse(&e).unwrap_err().to_string();
    assert!(error.contains(VERSION) && error.contains("different"));
    e = endpoint();
    e.contract_version += 1;
    assert!(parse(&e).is_err());
    e = endpoint();
    e.session_name = "another session".into();
    assert!(parse(&e).is_err());
    for path in ["relative.sock", "/tmp/bad\npath", "/tmp/bad\0path"] {
        e = endpoint();
        e.socket = path.into();
        assert!(parse(&e).is_err());
    }
    assert!(parse_endpoint(&vec![b' '; ENDPOINT_LIMIT + 1], "remote work").is_err());
    assert!(parse_endpoint(b"login banner\n{}", "remote work").is_err());
}

#[test]
fn remote_attach_cli_combinations() {
    // The derived CLI builder needs the same stack as the existing CLI tests.
    std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(|| {
            for args in [
                vec!["yinz", "attach", "--remote", "host", "work"],
                vec![
                    "yinz",
                    "attach",
                    "--remote",
                    "user@host",
                    "work",
                    "--create",
                ],
                vec![
                    "yinz",
                    "--config",
                    "/local/terminal.kdl",
                    "attach",
                    "--remote",
                    "host",
                    "work",
                ],
            ] {
                let opts = CliArgs::try_parse_from(args).unwrap();
                validate_options(&opts).unwrap();
            }
            for args in [
                vec!["yinz", "attach", "--remote", "host"],
                vec!["yinz", "attach", "--remote", "host", "work", "--index", "1"],
                vec![
                    "yinz",
                    "attach",
                    "--remote",
                    "host",
                    "work",
                    "--create-background",
                ],
                vec![
                    "yinz",
                    "attach",
                    "--remote",
                    "host",
                    "work",
                    "--force-run-commands",
                ],
            ] {
                assert!(CliArgs::try_parse_from(args).is_err());
            }
            for args in [
                vec![
                    "yinz",
                    "--layout",
                    "/local/layout.kdl",
                    "attach",
                    "--remote",
                    "host",
                    "work",
                ],
                vec![
                    "yinz",
                    "--data-dir",
                    "/local/data",
                    "attach",
                    "--remote",
                    "host",
                    "work",
                ],
                vec![
                    "yinz",
                    "attach",
                    "--remote",
                    "host",
                    "work",
                    "options",
                    "--mouse-mode",
                    "false",
                ],
            ] {
                assert!(validate_options(&CliArgs::try_parse_from(args).unwrap()).is_err());
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn remote_switches_require_names_and_reject_path_overrides() {
    assert!(switch_target(SessionConnection::default()).is_err());
    let mut target = SessionConnection {
        name: Some(SessionName::new("other")),
        ..Default::default()
    };
    assert_eq!(switch_target(target.clone()).unwrap().0, "other");
    target.cwd = Some("/local/project".into());
    assert!(switch_target(target).is_err());
    for name in ["", ".", "..", "../session", "bad\0name", "bad\nname"] {
        assert!(validate_name(name).is_err(), "{name:?}");
    }
    validate_name("a 'quoted' session").unwrap();
}

#[test]
fn client_sockets_are_short_unique_and_outside_session_discovery() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let discovery = root.path().canonicalize().unwrap().join("cv1");
    let first = client_socket(&discovery).unwrap();
    let second = client_socket(&discovery).unwrap();
    assert_ne!(first, second);
    let directory = first.parent().unwrap();
    assert_eq!(directory.parent(), discovery.parent());
    assert_eq!(first.file_name().unwrap().len(), 29);
    assert_ne!(directory, discovery);
    assert_eq!(
        std::fs::metadata(directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[test]
fn remote_bootstrap_waits_for_a_concurrent_creator_to_initialize() {
    let dir = tempfile::Builder::new()
        .prefix("yrdy-")
        .tempdir_in("/tmp")
        .unwrap();
    let socket = dir.path().join("session");
    let listener = yinz_ipc::consts::ipc_bind(&socket).unwrap();
    let peer = std::thread::spawn(move || {
        for initialized in [false, true] {
            let stream = listener.accept().unwrap();
            stream
                .set_recv_timeout(Some(Duration::from_secs(30)))
                .unwrap();
            let mut sender = IpcSenderWithContext::<ServerToClientMsg>::new(stream);
            let mut receiver = sender.get_receiver::<ClientToServerMsg>().unwrap();
            assert!(matches!(
                receiver.recv_client_msg(),
                IpcRecv::Msg(
                    ClientToServerMsg::Action {
                        action: yinz_types::input::actions::Action::ListPanes { .. },
                        is_cli_client: true,
                        ..
                    },
                    _
                )
            ));
            if initialized {
                sender
                    .send_server_msg(ServerToClientMsg::Log {
                        lines: vec!["ready".into()],
                    })
                    .unwrap();
            }
        }
    });
    wait_until_ready(&socket).unwrap();
    peer.join().unwrap();
}

#[test]
fn discovery_checks_versions_names_and_keeps_unavailable_but_not_exited_sessions() {
    use yinz_cli::sessions::SessionListing;
    let mut response = SessionList {
        yinz_version: VERSION.into(),
        contract_version: CLIENT_SERVER_CONTRACT_VERSION,
        sessions: vec![
            (
                "work".into(),
                Duration::from_secs(12),
                SessionListing::Running,
            ),
            ("busy".into(), Duration::ZERO, SessionListing::Unresponsive),
            ("denied".into(), Duration::ZERO, SessionListing::Denied),
            ("gone".into(), Duration::ZERO, SessionListing::Exited),
        ],
    };
    let parse = |response: &SessionList| {
        parse_sessions("user@alias", &serde_json::to_vec(response).unwrap())
    };
    let rows = parse(&response).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].label.as_str(), "user@alias / work");
    assert_eq!(rows[0].age, Duration::from_secs(12));
    assert_eq!(rows[1].listing, SessionListing::Unresponsive);
    response.contract_version += 1;
    assert!(parse(&response).is_err());
    response.contract_version -= 1;
    response.sessions[0].0 = "bad\x1bname".into();
    assert!(parse(&response).is_err());
    assert!(parse_sessions("host", b"banner\n{}").is_err());
}

#[test]
fn discovery_deadline_kills_and_reaps_ssh() {
    if yinz_test_support::reexec_test() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let pid_file = root.path().join("pid");
    yinz_test_support::write_warm_script(
        &root.path().join("ssh"),
        "printf '%s' \"$$\" > \"$YINZ_TEST_QUERY_PID\"\nexec sleep 60",
    );
    let mut path = vec![root.path().to_path_buf()];
    path.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    unsafe {
        std::env::set_var("PATH", std::env::join_paths(path).unwrap());
        std::env::set_var("YINZ_TEST_QUERY_PID", &pid_file);
    }
    let error = discover("host", &AtomicBool::new(false)).unwrap_err();
    assert!(format!("{error:#}").contains("timed out"));
    let pid: i32 = std::fs::read_to_string(pid_file).unwrap().parse().unwrap();
    assert_eq!(
        nix::sys::wait::waitpid(
            nix::unistd::Pid::from_raw(pid),
            Some(nix::sys::wait::WaitPidFlag::WNOHANG)
        ),
        Err(nix::errno::Errno::ECHILD)
    );
}
