use super::*;
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGQUIT, SIGTERM};
use std::sync::atomic::{AtomicBool, Ordering};

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
        bootstrap_version: REMOTE_BOOTSTRAP_VERSION,
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
    e.bootstrap_version += 1;
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
fn endpoint_from_a_remote_without_bootstrap_versioning_is_a_mismatch() {
    // A contract-1 remote from before bootstrap versioning: same fields, no bootstrap version.
    let older = serde_json::json!({
        "session_name": "remote work",
        "socket": "/remote/run/cv1/remote work",
        "yinz_version": VERSION,
        "contract_version": CLIENT_SERVER_CONTRACT_VERSION,
    });
    let error = parse_endpoint(&serde_json::to_vec(&older).unwrap(), "remote work")
        .unwrap_err()
        .to_string();
    assert!(error.starts_with("Yinz compatibility mismatch"), "{error}");
    assert!(
        error.contains(&format!("remote bootstrap {REMOTE_BOOTSTRAP_VERSION}"))
            && error.contains("no remote bootstrap version"),
        "{error}"
    );
}

#[test]
fn remote_command_line_rejections_are_compatibility_mismatches() {
    use yinz_client::remote_sessions::bootstrap_failure;
    let rendered = |args: &[&str]| {
        let args = args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        // The derived CLI builder needs the same stack as the existing CLI tests.
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(move || {
                CliArgs::try_parse_from(args)
                    .unwrap_err()
                    .render()
                    .to_string()
            })
            .unwrap()
            .join()
            .unwrap()
    };
    // What clap prints for a flag or subcommand a remote does not know, as an older remote
    // does for --bootstrap-version or remote-session.
    let unknown_flag = rendered(&[
        "yinz",
        "remote-session",
        "--client-version",
        VERSION,
        "--contract-version",
        "1",
        "--from-a-newer-client",
        "1",
        "--",
        "work",
    ]);
    assert!(
        unknown_flag.contains("unexpected argument '--from-a-newer-client'"),
        "{unknown_flag}"
    );
    let ours = format!(
        "Yinz {VERSION}, client/server contract {CLIENT_SERVER_CONTRACT_VERSION}, remote bootstrap {REMOTE_BOOTSTRAP_VERSION}"
    );
    let unknown_bootstrap_flag =
        unknown_flag.replace("'--from-a-newer-client'", "'--bootstrap-version'");
    let unknown_subcommand =
        "error: unrecognized subcommand 'remote-session'\n\nUsage: yinz [OPTIONS] [COMMAND]\n";
    let mapped = |stderr: &str| {
        let error = bootstrap_failure("user@host", format!("Warning: banner\n{stderr}").as_bytes())
            .unwrap_or_else(|| panic!("not mapped: {stderr}"))
            .to_string();
        assert!(error.starts_with("Yinz compatibility mismatch"), "{error}");
        assert!(error.contains(&ours), "{error}");
        error
    };
    for stderr in [unknown_bootstrap_flag.as_str(), unknown_subcommand] {
        let error = mapped(stderr);
        assert!(
            error.contains("older Yinz on user@host with no remote bootstrap version"),
            "{error}"
        );
    }
    // Any other unknown flag comes from an older remote that may well be versioned.
    let error = mapped(&unknown_flag);
    assert!(
        error.contains("older Yinz on user@host (it reported:"),
        "{error}"
    );
    assert!(!error.contains("no remote bootstrap version"), "{error}");
    // A remote that understood the flags reports its own mismatch; keep it, naming the host.
    let reported = bootstrap_failure(
        "host",
        b"Error: Yinz compatibility mismatch: this machine has Yinz 9; peer has Yinz 1.\n",
    )
    .unwrap()
    .to_string();
    assert_eq!(
        reported,
        "host reported: Yinz compatibility mismatch: this machine has Yinz 9; peer has Yinz 1."
    );
    assert!(bootstrap_failure("host", b"Permission denied (publickey).\n").is_none());
}

#[test]
fn bootstrap_rejects_clients_without_a_matching_bootstrap_version() {
    for bootstrap in [0, REMOTE_BOOTSTRAP_VERSION + 1] {
        let error = check_versions(VERSION, CLIENT_SERVER_CONTRACT_VERSION, bootstrap)
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("Yinz compatibility mismatch"), "{error}");
    }
    check_versions(
        VERSION,
        CLIENT_SERVER_CONTRACT_VERSION,
        REMOTE_BOOTSTRAP_VERSION,
    )
    .unwrap();
}

#[test]
fn remote_attach_cli_combinations() {
    // The derived CLI builder needs the same stack as the existing CLI tests.
    std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(|| {
            for args in [
                vec!["yinz", "attach", "--remote", "host"],
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
                vec!["yinz", "attach", "--remote", "host", "--create"],
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
fn remote_discovery_selects_the_only_running_session_among_unreachable_entries() {
    use yinz_cli::sessions::SessionListing::{Denied, Running, Unresponsive};

    for interactive in [false, true] {
        for sessions in [
            vec![("work", Running)],
            vec![("busy", Unresponsive), ("work", Running)],
            vec![("work", Running), ("denied", Denied)],
            vec![
                ("busy", Unresponsive),
                ("work", Running),
                ("denied", Denied),
            ],
        ] {
            assert_eq!(
                select_discovered_session("host", discovery_rows(&sessions), interactive).unwrap(),
                Outcome::Picked(Destination::Remote {
                    host: "host".into(),
                    name: "work".into(),
                })
            );
        }
    }
}

#[test]
fn remote_discovery_reports_empty_ambiguous_and_unreachable_sessions() {
    use yinz_cli::sessions::SessionListing::{Denied, Running, Unresponsive};
    let error = select_discovered_session("host", Vec::new(), false)
        .unwrap_err()
        .to_string();
    assert_eq!(error, "No active yinz sessions found on host.");
    for sessions in [
        vec![("one", Running), ("two", Running)],
        vec![("busy", Unresponsive)],
        vec![("denied", Denied)],
    ] {
        let rows = discovery_rows(&sessions);
        let error = select_discovered_session("host", rows, false)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("yinz attach --remote host SESSION"),
            "{error}"
        );
        for (name, _) in sessions {
            assert!(error.contains(&format!("host / {name}")), "{error}");
        }
    }
}

fn discovery_rows(sessions: &[(&str, yinz_cli::sessions::SessionListing)]) -> Vec<PickerRow> {
    let response = serde_json::json!({
        "yinz_version": VERSION,
        "contract_version": CLIENT_SERVER_CONTRACT_VERSION,
        "bootstrap_version": REMOTE_BOOTSTRAP_VERSION,
        "sessions": sessions.iter().map(|(name, listing)| (name, Duration::ZERO, listing)).collect::<Vec<_>>(),
    });
    parse_sessions("host", &serde_json::to_vec(&response).unwrap()).unwrap()
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
        bootstrap_version: REMOTE_BOOTSTRAP_VERSION,
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
    response.bootstrap_version = 0;
    assert!(parse(&response)
        .unwrap_err()
        .to_string()
        .contains("Yinz compatibility mismatch"));
    response.bootstrap_version = REMOTE_BOOTSTRAP_VERSION;
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
