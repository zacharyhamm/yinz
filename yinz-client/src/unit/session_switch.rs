use super::*;
use crate::test_support::FakeClientOsApi;
use interprocess::local_socket::{GenericFilePath, ListenerOptions};

#[test]
fn enclosing_local_session_is_rejected_before_preparation_even_from_a_remote_session() {
    use yinz_process::pty_host::{protocol, PANE_REF_ENV_KEY};
    if yinz_test_support::reexec_test() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("YINZ_SOCKET_DIR", dir.path());
        std::env::set_var("YINZ_TEST_CACHE_DIR", dir.path().join("cache"));
        std::env::set_var(PANE_REF_ENV_KEY, "abcdef012345");
        std::env::set_var("YINZ_SESSION_NAME", "stale-owner");
        // Accepted remote requests must not contact a real SSH host.
        std::env::set_var("PATH", dir.path());
    }
    std::fs::create_dir_all(protocol::socket_dir()).unwrap();
    let listener = std::os::unix::net::UnixListener::bind(protocol::socket_path(
        &yinz_types::internal_ids::SupervisorId::new("abcdef012345"),
    ))
    .unwrap();
    let peer = std::thread::spawn(move || {
        for _ in 0..5 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            assert!(matches!(
                protocol::read_msg::<protocol::ServerToHostMsg, _>(&mut stream).unwrap(),
                Some(protocol::ServerToHostMsg::Query)
            ));
            let status = serde_json::from_value(serde_json::json!({
                "host_id": "abcdef012345", "session_name": "enclosing", "terminal_id": 0,
                "command": "yinz", "cwd": null, "child_pid": null, "host_pid": 1,
                "started_at_unix": 0, "attached": true, "child_exit_status": null, "buffered_bytes": 0
            }))
            .unwrap();
            protocol::write_msg(&mut stream, &protocol::HostToServerMsg::Status(status)).unwrap();
        }
    });
    let os = FakeClientOsApi::new();
    let args = Default::default();
    let mut local_config = LocalConfig::default();
    let mut remote = Sessions::new(
        Some("host-a".into()),
        "source".into(),
        &args,
        &mut local_config,
    );
    assert_eq!(
        remote
            .handle(
                prepare_request(Destination::CurrentServer, "enclosing"),
                &os
            )
            .0
            .unwrap(),
        ClientReply::Pending
    );
    remote.cancel();
    for (host, destination) in [
        (None, Destination::Local),
        (None, Destination::CurrentServer),
        (Some("host-a".into()), Destination::Local),
    ] {
        let args = Default::default();
        let mut local_config = LocalConfig::default();
        let mut sessions = Sessions::new(host, "source".into(), &args, &mut local_config);
        let (result, prepared) = sessions.handle(prepare_request(destination, "enclosing"), &os);
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("nesting a session in itself"));
        assert!(prepared.is_none());
        assert!(sessions.preparation.is_none());
    }
    let target = |name: &str| yinz_types::data::SessionConnection {
        name: Some(name.into()),
        ..Default::default()
    };
    let error = legacy_target(
        target("enclosing"),
        None,
        &Default::default(),
        Default::default(),
    )
    .err()
    .unwrap();
    assert!(error.to_string().contains("nesting a session in itself"));
    assert!(!YINZ_SOCK_DIR.join("enclosing").exists());
    assert!(matches!(
        legacy_target(
            target("stale-owner"),
            None,
            &Default::default(),
            Default::default()
        )
        .unwrap(),
        crate::ClientInfo::New(..)
    ));
    let error = legacy_target(
        target("enclosing"),
        Some("host-a".into()),
        &Default::default(),
        Default::default(),
    )
    .err()
    .unwrap();
    assert!(!format!("{error:#}").contains("nesting a session in itself"));
    assert!(format!("{error:#}").contains("bootstrap"));
    peer.join().unwrap();

    unsafe { std::env::set_var(PANE_REF_ENV_KEY, "unavailable") };
    let args = Default::default();
    let mut local_config = LocalConfig::default();
    let mut local = Sessions::new(None, "source".into(), &args, &mut local_config);
    assert_eq!(
        local
            .handle(prepare_request(Destination::Local, "enclosing"), &os)
            .0
            .unwrap(),
        ClientReply::Pending
    );
    local.cancel();
}

fn prepare_request(destination: Destination, name: &str) -> ClientRequest {
    ClientRequest::Switch(SwitchRequest::Prepare {
        id: "switch".into(),
        target: SwitchTarget {
            destination,
            name: name.into(),
            saved: false,
        },
    })
}

#[test]
fn resurrection_forwards_retained_paths_and_options_as_separate_arguments() {
    let assets = CliAssets {
        config_file_path: Some("/local config/custom config.kdl".into()),
        config_dir: Some("/local config".into()),
        data_dir: Some("/local data".into()),
        configuration_options: Some(yinz_types::input::options::Options {
            scroll_buffer_size: Some(42),
            default_shell: Some("/local shell".into()),
            ..Default::default()
        }),
        terminal_window_size: yinz_types::pane_size::Size {
            rows: 60,
            cols: 120,
        },
        max_panes: Some(3),
        is_debug: true,
        ..Default::default()
    };
    let pty = nix::pty::openpty(None, None).unwrap();
    let termios = Some(WireTermios::from_termios(
        &nix::sys::termios::tcgetattr(&pty.slave).unwrap(),
    ));
    let command = resurrection_command(
        "saved session",
        &assets,
        assets.config_file_path.as_deref(),
        &termios,
    )
    .unwrap();
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(
        &args[..10],
        [
            "--config",
            "/local config/custom config.kdl",
            "--config-dir",
            "/local config",
            "--data-dir",
            "/local data",
            "--max-panes",
            "3",
            "--debug",
            "remote-session"
        ]
    );
    let options = args
        .windows(2)
        .find(|pair| pair[0] == "--configuration-options")
        .unwrap()[1];
    assert_eq!(
        serde_json::from_str::<yinz_types::input::options::Options>(options.to_str().unwrap())
            .unwrap(),
        assets.configuration_options.unwrap()
    );
    assert_eq!(&args[args.len() - 2..], ["--", "saved session"]);
    let snapshot = args
        .windows(2)
        .find(|pair| pair[0] == "--client-termios")
        .unwrap()[1];
    assert_eq!(
        serde_json::from_str::<Option<WireTermios>>(snapshot.to_str().unwrap()).unwrap(),
        termios
    );
    let size = args
        .windows(2)
        .find(|pair| pair[0] == "--terminal-window-size")
        .unwrap()[1];
    assert_eq!(
        serde_json::from_str::<yinz_types::pane_size::Size>(size.to_str().unwrap()).unwrap(),
        assets.terminal_window_size
    );
    let defaults = resurrection_command("saved", &CliAssets::default(), None, &None).unwrap();
    assert_eq!(defaults.get_args().next().unwrap(), "remote-session");
    assert!(!defaults
        .get_args()
        .any(|arg| arg == "--configuration-options" || arg == "--max-panes" || arg == "--debug"));
    assert!(defaults
        .get_args()
        .collect::<Vec<_>>()
        .windows(2)
        .any(|pair| pair == ["--client-termios", "null"]));
}

#[test]
fn resurrection_preserves_optional_and_explicit_missing_config() {
    use yinz_cli::{args::CliArgs, setup::Setup};

    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("config.kdl");
    assert!(!config_path.exists());
    for explicit in [false, true] {
        let cli_args = CliArgs {
            config: explicit.then(|| config_path.clone()),
            config_dir: Some(directory.path().to_owned()),
            ..Default::default()
        };
        let options = Default::default();
        let assets = crate::startup::StartupContext::detached(&cli_args, &options)
            .build_cli_assets(&options, None, false, None);
        assert_eq!(assets.config_file_path.as_ref(), Some(&config_path));
        let command =
            resurrection_command("saved", &assets, cli_args.config.as_deref(), &None).unwrap();
        let args: Vec<_> = command.get_args().collect();
        let bootstrap_args = CliArgs {
            config: args
                .windows(2)
                .find(|pair| pair[0] == "--config")
                .map(|pair| pair[1].into()),
            config_dir: args
                .windows(2)
                .find(|pair| pair[0] == "--config-dir")
                .map(|pair| pair[1].into()),
            ..Default::default()
        };
        assert_eq!(bootstrap_args.config, cli_args.config);
        assert_eq!(bootstrap_args.config_dir, cli_args.config_dir);
        assert_eq!(
            Setup::config_without_layout(&bootstrap_args).is_ok(),
            !explicit
        );
    }
}

#[test]
fn client_loop_rename_updates_current_identity_without_cancelling_preparation() {
    use crate::{
        client_loop::ClientLoop, command_is_executing::CommandIsExecuting,
        nested_reannounce::NestedReannounce, render_budget::RenderBudget,
        stdin_ansi_parser::StdinAnsiParser, terminal_escapes::ClientTeardown, ClientInstruction,
    };
    use yinz_runtime::channels::{self, SenderWithContext};
    if yinz_test_support::reexec_test() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("PATH", dir.path());
        std::env::set_var(yinz_process::pty_host::PANE_REF_ENV_KEY, "unavailable");
    }
    for host in [None, Some("host-a".into())] {
        let args = Default::default();
        let mut local_config = LocalConfig::default();
        let mut sessions = Sessions::new(host.clone(), "alpha".into(), &args, &mut local_config);
        let machine = sessions.machine.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        sessions.preparation = Some(Preparation {
            id: "pending".into(),
            cancelled: cancelled.clone(),
            receiver: mpsc::channel().1,
            result: None,
        });
        let os = FakeClientOsApi::new();
        let (tx, rx) = channels::unbounded();
        let tx = SenderWithContext::new(tx);
        tx.send(ClientInstruction::RenamedSession("beta".into()))
            .unwrap();
        tx.send(ClientInstruction::Exit(yinz_ipc::ipc::ExitReason::Normal))
            .unwrap();
        let (input_tx, _input_rx) = channels::unbounded();
        let nested = NestedReannounce::spawn(Box::new(os.clone()), "rename-test".into());
        nested.stop();
        let outcome = ClientLoop {
            sessions: &mut sessions,
            remote_host: host.as_deref(),
            os_input: &os,
            command_is_executing: &mut CommandIsExecuting::new(),
            stdin_ansi_parser: &Arc::new(std::sync::Mutex::new(StdinAnsiParser::new())),
            send_input_instructions: &SenderWithContext::new(input_tx),
            nested_reannounce: &nested,
            teardown: ClientTeardown::new(24, false),
            synchronised_output: None,
            render_budget: &RenderBudget::new(100),
            startup_started: Instant::now(),
            splash_deadline: None,
        }
        .run(&rx);
        assert_eq!(outcome.exit_status, 0);
        assert_eq!(sessions.machine, machine);
        assert_eq!(sessions.preparation.as_ref().unwrap().id, "pending");
        assert!(!cancelled.load(Ordering::Acquire));
        sessions.cancel();
        let rejected = sessions
            .handle(prepare_request(Destination::CurrentServer, "beta"), &os)
            .0;
        assert_eq!(
            rejected.unwrap_err().to_string(),
            "Already attached to this session"
        );
        assert!(sessions.preparation.is_none());
        assert_eq!(
            sessions
                .handle(prepare_request(Destination::CurrentServer, "alpha"), &os)
                .0
                .unwrap(),
            ClientReply::Pending
        );
        sessions.cancel();
    }
}

fn peer(
    mode: &'static str,
) -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::thread::JoinHandle<()>,
    Arc<AtomicBool>,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("destination");
    let listener = ListenerOptions::new()
        .name(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
        .create_sync()
        .unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let stop = cancelled.clone();
    let worker = std::thread::spawn(move || {
        let mut sender = IpcSenderWithContext::<ServerToClientMsg>::new(listener.accept().unwrap());
        let mut receiver = sender.get_receiver::<ClientToServerMsg>().unwrap();
        receiver
            .set_recv_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        assert!(matches!(
            receiver.recv_client_msg(),
            IpcRecv::Msg(ClientToServerMsg::AttachClient { .. }, _)
        ));
        match mode {
            "ready" | "close-after-ready" | "close-after-large" | "reject-after-large" => {
                sender
                    .send_server_msg(ServerToClientMsg::Render {
                        content: if mode.ends_with("large") {
                            "x".repeat(5 * 1024 * 1024)
                        } else {
                            "DESTINATION FRAME".into()
                        },
                    })
                    .unwrap();
                if mode.starts_with("close-after-") {
                    return;
                }
                if mode == "reject-after-large" {
                    sender
                        .send_server_msg(ServerToClientMsg::Exit {
                            exit_reason: yinz_ipc::ipc::ExitReason::Error(
                                "destination rejected".into(),
                            ),
                        })
                        .unwrap();
                    return;
                }
            },
            "startup-overflow" => sender
                .send_server_msg(ServerToClientMsg::Log {
                    lines: vec!["x".repeat(STARTUP_OUTPUT_BUDGET + 1)],
                })
                .unwrap(),
            "reject" => sender
                .send_server_msg(ServerToClientMsg::Exit {
                    exit_reason: yinz_ipc::ipc::ExitReason::Error("attachment rejected".into()),
                })
                .unwrap(),
            "cancel" => stop.store(true, Ordering::Release),
            "hangup" => return,
            _ => unreachable!(),
        }
        assert!(matches!(
            receiver.recv_client_msg(),
            IpcRecv::Msg(ClientToServerMsg::ClientExited, _)
        ));
    });
    (dir, path, worker, cancelled)
}

#[test]
fn cancellation_does_not_wait_for_a_blocked_worker_and_releases_its_late_result() {
    let (_dir, path, server, stop) = peer("ready");
    let prepared = attach_socket(
        Destination::Local,
        "destination".into(),
        None,
        &path,
        Default::default(),
        &stop,
        Instant::now() + Duration::from_secs(25),
    )
    .unwrap();
    let (release, blocked) = std::sync::mpsc::channel();
    let (sender, receiver) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        blocked.recv().unwrap();
        let _ = sender.send(Ok(prepared));
    });
    let (completed, completion) = std::sync::mpsc::channel();
    let (finish, finishing) = std::sync::mpsc::channel();
    let cancelled = stop.clone();
    let client = std::thread::spawn(move || {
        let args = Default::default();
        let mut config = LocalConfig::default();
        let mut sessions = Sessions::new(None, "source".into(), &args, &mut config);
        sessions.preparation = Some(Preparation {
            id: "switch".into(),
            cancelled,
            receiver,
            result: None,
        });
        sessions.workers.push(worker);
        let (result, prepared) = sessions.handle(
            ClientRequest::Switch(SwitchRequest::Cancel("switch".into())),
            &FakeClientOsApi::new(),
        );
        assert!(prepared.is_none());
        assert!(sessions.preparation.is_none());
        assert_eq!(sessions.workers.len(), 1);
        completed.send(result.unwrap()).unwrap();
        finishing.recv_timeout(Duration::from_secs(10)).unwrap();
    });
    // Cancellation must finish inside the plugin's three-second request timeout.
    let result = completion.recv_timeout(Duration::from_secs(2));
    let was_cancelled = stop.load(Ordering::Acquire);
    release.send(()).unwrap();
    // The peer requires ClientExited, proving the cancelled worker's result was dropped.
    server.join().unwrap();
    finish.send(()).unwrap();
    client.join().unwrap();
    assert_eq!(result.unwrap(), ClientReply::Complete);
    assert!(was_cancelled);
}

fn blocked_ssh(root: &std::path::Path, phase: &str) -> std::fs::File {
    use std::os::unix::fs::OpenOptionsExt;
    let ready = root.join("ready");
    nix::unistd::mkfifo(&ready, nix::sys::stat::Mode::S_IRWXU).unwrap();
    let fifo = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(nix::libc::O_NONBLOCK)
        .open(ready)
        .unwrap();
    let endpoint = serde_json::to_string(&remote_sessions::Endpoint {
        session_name: "target".into(),
        socket: "/remote/socket".into(),
        yinz_version: yinz_runtime::consts::VERSION.into(),
        contract_version: yinz_runtime::consts::CLIENT_SERVER_CONTRACT_VERSION,
    })
    .unwrap();
    yinz_test_support::write_warm_script(
        &root.join("ssh"),
        &format!(
            r#"
case " $* " in
    *" -L "*) ;;
    *) if [ "$YINZ_TEST_PREPARATION_PHASE" = forward ]; then
        printf '%s\n' '{endpoint}'
        exit 0
    fi ;;
esac
printf '%s\n' "$$" > "$YINZ_TEST_PREPARATION_ROOT/ready"
exec sleep 60
"#
        ),
    );
    let mut path = vec![root.to_owned()];
    path.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    unsafe {
        std::env::set_var("PATH", std::env::join_paths(path).unwrap());
        std::env::set_var("YINZ_TEST_PREPARATION_ROOT", root);
        std::env::set_var("YINZ_TEST_PREPARATION_PHASE", phase);
        std::env::set_var("YINZ_SOCKET_DIR", root.join("sessions"));
    }
    fifo
}

fn prepare_blocked_ssh(sessions: &mut Sessions<'_>, fifo: &std::fs::File) -> nix::unistd::Pid {
    use std::{io::BufRead, os::fd::AsFd};
    assert_eq!(
        sessions
            .handle(
                prepare_request(Destination::CurrentServer, "target"),
                &FakeClientOsApi::new(),
            )
            .0
            .unwrap(),
        ClientReply::Pending
    );
    let mut fds = [nix::poll::PollFd::new(
        fifo.as_fd(),
        nix::poll::PollFlags::POLLIN,
    )];
    assert_eq!(
        nix::poll::poll(&mut fds, 30000u16).unwrap(),
        1,
        "SSH preparation never started"
    );
    let mut pid = String::new();
    std::io::BufReader::new(fifo).read_line(&mut pid).unwrap();
    nix::unistd::Pid::from_raw(pid.trim().parse().unwrap())
}

fn ssh_child_is_reaped(pid: nix::unistd::Pid) -> bool {
    nix::sys::wait::waitpid(pid, Some(nix::sys::wait::WaitPidFlag::WNOHANG))
        == Err(nix::errno::Errno::ECHILD)
}

#[test]
fn shutdown_and_drop_reap_active_and_cancelled_preparation_children() {
    if yinz_test_support::reexec_test() {
        return;
    }
    for phase in ["bootstrap", "forward"] {
        let dir = tempfile::tempdir().unwrap();
        let fifo = blocked_ssh(dir.path(), phase);
        for drop_sessions in [false, true] {
            let args = Default::default();
            let mut local_config = LocalConfig::default();
            let mut sessions = Sessions::new(
                Some("host".into()),
                "source".into(),
                &args,
                &mut local_config,
            );
            let mut children = Vec::new();
            // Both cancellation requests must leave their workers owned until cleanup finishes.
            for request in [
                ClientRequest::CancelAll,
                ClientRequest::Switch(SwitchRequest::Cancel("switch".into())),
            ] {
                children.push(prepare_blocked_ssh(&mut sessions, &fifo));
                sessions.handle(request, &FakeClientOsApi::new()).0.unwrap();
                assert!(sessions.preparation.is_none());
            }
            children.push(prepare_blocked_ssh(&mut sessions, &fifo));
            if drop_sessions {
                drop(sessions);
            } else {
                sessions.shutdown();
                assert!(sessions.preparation.is_none());
                assert!(sessions.workers.is_empty());
                sessions.shutdown();
            }
            for pid in children {
                assert!(
                    ssh_child_is_reaped(pid),
                    "{phase}: SSH child {pid} was not reaped"
                );
            }
        }
    }
}

#[test]
fn preparation_worker_panic_is_reported_and_shutdown_is_idempotent() {
    let args = Default::default();
    let mut local_config = LocalConfig::default();
    let mut sessions = Sessions::new(None, "source".into(), &args, &mut local_config);
    let (sender, receiver) = mpsc::channel();
    sessions.workers.push(std::thread::spawn(move || {
        let _sender = sender;
        panic!("preparation failed");
    }));
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(10)),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
    sessions.preparation = Some(Preparation {
        id: "switch".into(),
        cancelled: Default::default(),
        receiver,
        result: None,
    });
    let error = sessions
        .handle(
            ClientRequest::Switch(SwitchRequest::Status("switch".into())),
            &FakeClientOsApi::new(),
        )
        .0
        .unwrap_err();
    assert_eq!(error.to_string(), "Session preparation worker failed");
    sessions.shutdown();
    sessions.shutdown();
    assert!(sessions.workers.is_empty());
}

#[test]
fn fatal_client_exit_reaps_preparation_children() {
    use crate::{
        client_loop::ClientLoop, command_is_executing::CommandIsExecuting,
        nested_reannounce::NestedReannounce, render_budget::RenderBudget,
        stdin_ansi_parser::StdinAnsiParser, terminal_escapes::ClientTeardown, ClientInstruction,
    };
    use yinz_runtime::channels::{self, SenderWithContext};
    const CHILD: &str = "YINZ_TEST_FATAL_PREPARATION";
    if let Ok(phase) = std::env::var(CHILD) {
        static CHILDREN: std::sync::Mutex<Vec<nix::unistd::Pid>> =
            std::sync::Mutex::new(Vec::new());
        extern "C" fn check_reaped() {
            if CHILDREN
                .lock()
                .unwrap()
                .iter()
                .copied()
                .all(ssh_child_is_reaped)
            {
                println!("preparation children reaped");
            }
        }
        let root =
            std::path::PathBuf::from(std::env::var_os("YINZ_TEST_PREPARATION_ROOT").unwrap());
        let fifo = blocked_ssh(&root, &phase);
        let args = Default::default();
        let mut local_config = LocalConfig::default();
        let mut sessions = Sessions::new(
            Some("host".into()),
            "source".into(),
            &args,
            &mut local_config,
        );
        CHILDREN
            .lock()
            .unwrap()
            .push(prepare_blocked_ssh(&mut sessions, &fifo));
        sessions.cancel();
        CHILDREN
            .lock()
            .unwrap()
            .push(prepare_blocked_ssh(&mut sessions, &fifo));
        assert_eq!(unsafe { nix::libc::atexit(check_reaped) }, 0);
        let os = FakeClientOsApi::new().capturing_stdout();
        let (tx, rx) = channels::unbounded();
        let instruction = if std::env::var_os("YINZ_TEST_FATAL_EXIT_REASON").is_some() {
            ClientInstruction::Exit(yinz_ipc::ipc::ExitReason::Error(
                "fatal preparation test".into(),
            ))
        } else {
            ClientInstruction::Error("fatal preparation test".into())
        };
        SenderWithContext::new(tx).send(instruction).unwrap();
        let (input_tx, _input_rx) = channels::unbounded();
        let nested = NestedReannounce::spawn(Box::new(os.clone()), "shutdown-test".into());
        nested.stop();
        ClientLoop {
            sessions: &mut sessions,
            remote_host: Some("host"),
            os_input: &os,
            command_is_executing: &mut CommandIsExecuting::new(),
            stdin_ansi_parser: &Arc::new(std::sync::Mutex::new(StdinAnsiParser::new())),
            send_input_instructions: &SenderWithContext::new(input_tx),
            nested_reannounce: &nested,
            teardown: ClientTeardown::new(24, false),
            synchronised_output: None,
            render_budget: &RenderBudget::new(100),
            startup_started: Instant::now(),
            splash_deadline: None,
        }
        .run(&rx);
        panic!("fatal instruction returned");
    }
    for phase in ["bootstrap", "forward"] {
        for exit_reason in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "session_switch::tests::fatal_client_exit_reaps_preparation_children",
                    "--nocapture",
                ])
                .env(CHILD, phase)
                .env("YINZ_TEST_PREPARATION_ROOT", dir.path());
            if exit_reason {
                command.env("YINZ_TEST_FATAL_EXIT_REASON", "1");
            }
            let output = command.output().unwrap();
            assert_eq!(output.status.code(), Some(1), "{output:?}");
            assert!(
                String::from_utf8_lossy(&output.stdout).contains("preparation children reaped"),
                "{output:?}"
            );
        }
    }
}

#[test]
fn prepared_output_is_inactive_and_unused_attachments_are_detached() {
    let (_dir, path, worker, stop) = peer("ready");
    let prepared = attach_socket(
        Destination::Local,
        "same".into(),
        None,
        &path,
        Default::default(),
        &stop,
        Instant::now() + Duration::from_secs(25),
    )
    .unwrap();
    assert!(
        matches!(&prepared.buffered[0].0, ServerToClientMsg::Render { content } if content == "DESTINATION FRAME")
    );
    drop(prepared);
    worker.join().unwrap();
}

#[test]
fn rejected_disconnected_and_cancelled_attachments_preserve_source() {
    let source = FakeClientOsApi::new().capturing_stdout();
    for mode in ["reject", "cancel", "hangup", "startup-overflow"] {
        let (_dir, path, worker, stop) = peer(mode);
        assert!(attach_socket(
            Destination::Local,
            "same".into(),
            None,
            &path,
            Default::default(),
            &stop,
            Instant::now() + Duration::from_secs(25),
        )
        .is_err());
        assert!(source.stdout_bytes().is_empty());
        worker.join().unwrap();
    }
    let dir = tempfile::tempdir().unwrap();
    assert!(attach_socket(
        Destination::Local,
        "vanished".into(),
        None,
        &dir.path().join("absent"),
        Default::default(),
        &AtomicBool::new(false),
        Instant::now() + Duration::from_secs(25),
    )
    .is_err());
    assert!(!dir.path().join("absent").exists());
}

#[test]
fn destination_identity_and_stale_commits_are_client_scoped() {
    for source in [None, Some("host-a".into()), Some("host-b".into())] {
        let args = Default::default();
        let mut local_config = LocalConfig::default();
        let mut sessions = Sessions::new(source, "same".into(), &args, &mut local_config);
        let os = FakeClientOsApi::new();
        for destination in [
            Destination::CurrentServer,
            Destination::Remote("-oProxyCommand=bad".into()),
        ] {
            let (result, prepared) = sessions.handle(
                ClientRequest::Switch(SwitchRequest::Prepare {
                    id: "invalid".into(),
                    target: SwitchTarget {
                        destination,
                        name: "same".into(),
                        saved: false,
                    },
                }),
                &os,
            );
            assert!(result.is_err());
            assert!(prepared.is_none());
        }
        assert!(sessions.commit("other-client").is_err());
    }
    let (_dir, path, worker, stop) = peer("ready");
    let ready = attach_socket(
        Destination::Local,
        "same".into(),
        None,
        &path,
        Default::default(),
        &stop,
        Instant::now() + Duration::from_secs(25),
    )
    .unwrap();
    let args = Default::default();
    let mut local_config = LocalConfig::default();
    let mut remote = Sessions::new(
        Some("host-a".into()),
        "same".into(),
        &args,
        &mut local_config,
    );
    remote.preparation = Some(Preparation {
        id: "wanted".into(),
        cancelled: stop,
        receiver: mpsc::channel().1,
        result: Some(Ok(ready)),
    });
    assert!(remote.commit("stale").is_err());
    let prepared = remote.commit("wanted").unwrap();
    assert_eq!(prepared.machine, Destination::Local);
    assert_eq!(prepared.name, "same");
    assert!(remote.commit("wanted").is_err());
    drop(prepared);
    worker.join().unwrap();
}

#[test]
fn disconnect_between_preparation_and_commit_keeps_source_attached() {
    for mode in [
        "close-after-ready",
        "close-after-large",
        "reject-after-large",
    ] {
        let (_dir, path, worker, stop) = peer(mode);
        let ready = attach_socket(
            Destination::Local,
            "target".into(),
            None,
            &path,
            Default::default(),
            &stop,
            Instant::now() + Duration::from_secs(25),
        )
        .unwrap();
        worker.join().unwrap();
        let args = Default::default();
        let mut local_config = LocalConfig::default();
        let mut sessions = Sessions::new(None, "source".into(), &args, &mut local_config);
        sessions.preparation = Some(Preparation {
            id: "pending".into(),
            cancelled: stop,
            receiver: mpsc::channel().1,
            result: Some(Ok(ready)),
        });
        assert!(sessions.commit("pending").is_err());
        assert_eq!(sessions.name, "source");
        assert!(sessions.preparation.is_none());
    }
}

#[test]
fn large_renders_preserve_order_and_leave_output_beyond_the_commit_budget_on_the_transport() {
    for first_size in [5 * 1024 * 1024, STARTUP_OUTPUT_BUDGET - 1024] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("destination");
        let listener = yinz_ipc::consts::ipc_bind(&path).unwrap();
        let messages = vec![
            ServerToClientMsg::Log {
                lines: vec!["before first render".into()],
            },
            ServerToClientMsg::Render {
                content: "a".repeat(first_size),
            },
            ServerToClientMsg::Render {
                content: "b".repeat(2048),
            },
            ServerToClientMsg::Log {
                lines: vec!["after budget".into()],
            },
            ServerToClientMsg::Render {
                content: "final render".into(),
            },
        ];
        let output = messages.clone();
        let (sent, sent_all) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let stream = listener.accept().unwrap();
            stream
                .set_send_timeout(Some(Duration::from_secs(30)))
                .unwrap();
            stream
                .set_recv_timeout(Some(Duration::from_secs(30)))
                .unwrap();
            let mut sender = IpcSenderWithContext::<ServerToClientMsg>::new(stream);
            let mut receiver = sender.get_receiver::<ClientToServerMsg>().unwrap();
            assert!(matches!(
                receiver.recv_client_msg(),
                IpcRecv::Msg(ClientToServerMsg::AttachClient { .. }, _)
            ));
            for message in output {
                sender.send_server_msg(message).unwrap();
            }
            sent.send(()).unwrap();
            assert!(matches!(
                receiver.recv_client_msg(),
                IpcRecv::Msg(ClientToServerMsg::ClientExited, _)
            ));
        });
        let ready = attach_socket(
            Destination::Local,
            "target".into(),
            None,
            &path,
            Default::default(),
            &AtomicBool::new(false),
            Instant::now() + Duration::from_secs(30),
        )
        .unwrap();
        assert_eq!(ready.buffered.len(), 2);
        sent_all.recv_timeout(Duration::from_secs(30)).unwrap();
        let args = Default::default();
        let mut local_config = LocalConfig::default();
        let mut sessions = Sessions::new(None, "source".into(), &args, &mut local_config);
        sessions.preparation = Some(Preparation {
            id: "pending".into(),
            cancelled: Default::default(),
            receiver: mpsc::channel().1,
            result: Some(Ok(ready)),
        });
        let mut prepared = sessions.commit("pending").unwrap();
        // All messages are already sent: only the frame crossing the budget may be drained.
        assert_eq!(prepared.buffered.len(), 3);
        for expected in messages {
            let actual = prepared
                .buffered
                .pop_front()
                .or_else(|| prepared.receiver.as_mut().unwrap().recv_server_msg().msg())
                .unwrap()
                .0;
            assert_eq!(actual, expected);
        }
        drop(prepared);
        worker.join().unwrap();
    }
}

#[cfg(unix)]
#[test]
fn commit_deadline_preserves_a_frame_that_keeps_arriving() {
    use interprocess::local_socket::Stream;
    use std::io::Write;

    let (socket, mut peer) = std::os::unix::net::UnixStream::pair().unwrap();
    let sender = IpcSenderWithContext::<ClientToServerMsg>::new(Stream::UdSocket(
        socket.try_clone().unwrap().into(),
    ));
    let receiver = sender.get_receiver::<ServerToClientMsg>().unwrap();
    receiver
        .set_recv_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let args = Default::default();
    let mut local_config = LocalConfig::default();
    let mut sessions = Sessions::new(None, "source".into(), &args, &mut local_config);
    sessions.preparation = Some(Preparation {
        id: "pending".into(),
        cancelled: Default::default(),
        receiver: mpsc::channel().1,
        result: Some(Ok(PreparedConnection {
            sender: Some(sender),
            receiver: Some(receiver),
            buffered: Default::default(),
            forward: None,
            machine: Destination::Local,
            name: "destination".into(),
        })),
    });
    let frame = render_frame();
    peer.write_all(&frame[..2]).unwrap();
    let (finish, finished) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        let mut sent = 2;
        // Timing is under test: each chunk arrives within the old 100ms per-read timeout.
        while sent < frame.len() {
            if finished.recv_timeout(Duration::from_millis(20)).is_ok() {
                break;
            }
            peer.write_all(&frame[sent..sent + 1]).unwrap();
            sent += 1;
        }
        peer.write_all(&frame[sent..]).unwrap();
        peer
    });
    let started = Instant::now();
    let mut prepared = sessions.commit("pending").unwrap();
    let elapsed = started.elapsed();
    let _ = finish.send(());
    let _peer = writer.join().unwrap();
    assert!(elapsed < Duration::from_secs(2), "commit took {elapsed:?}");
    assert!(
        prepared.buffered.is_empty(),
        "commit waited for the whole frame"
    );
    assert_eq!(socket.read_timeout().unwrap(), None);
    assert!(
        matches!(prepared.receiver.as_mut().unwrap().recv_server_msg(), IpcRecv::Msg(
        ServerToClientMsg::Render { content }, _
    ) if content == "fragmented render")
    );
}

#[cfg(unix)]
#[test]
fn commit_clears_both_startup_timeouts_and_keeps_input_usable() {
    use crate::os_input_output::{get_client_os_input, ServerConnection};
    use interprocess::local_socket::Stream;
    use yinz_types::input::actions::Action;

    let (socket, peer) = std::os::unix::net::UnixStream::pair().unwrap();
    let sender = IpcSenderWithContext::<ClientToServerMsg>::new(Stream::UdSocket(
        socket.try_clone().unwrap().into(),
    ));
    sender
        .set_send_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let receiver = sender.get_receiver::<ServerToClientMsg>().unwrap();
    receiver
        .set_recv_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    assert!(socket.read_timeout().unwrap().is_some());
    assert!(socket.write_timeout().unwrap().is_some());
    let args = Default::default();
    let mut local_config = LocalConfig::default();
    let mut sessions = Sessions::new(None, "source".into(), &args, &mut local_config);
    sessions.preparation = Some(Preparation {
        id: "pending".into(),
        cancelled: Default::default(),
        receiver: mpsc::channel().1,
        result: Some(Ok(PreparedConnection {
            sender: Some(sender),
            receiver: Some(receiver),
            buffered: Default::default(),
            forward: None,
            machine: Destination::Local,
            name: "destination".into(),
        })),
    });
    let mut prepared = sessions.commit("pending").unwrap();
    assert_eq!(socket.read_timeout().unwrap(), None);
    assert_eq!(socket.write_timeout().unwrap(), None);
    let os = get_client_os_input().unwrap();
    os.install_session_connection(
        prepared.sender.take().unwrap(),
        prepared.receiver.take().unwrap(),
    )
    .unwrap();
    os.send_to_server(ClientToServerMsg::Action {
        action: Action::WriteChars {
            chars: "input after handoff".into(),
        },
        terminal_id: None,
        client_id: None,
        is_cli_client: false,
    });
    let mut peer = IpcReceiverWithContext::<ClientToServerMsg>::new(Stream::UdSocket(peer.into()));
    peer.set_recv_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    assert!(matches!(peer.recv_client_msg(), IpcRecv::Msg(
        ClientToServerMsg::Action { action: Action::WriteChars { chars }, .. }, _
    ) if chars == "input after handoff"));
}

fn render_frame() -> Vec<u8> {
    use std::io::Read;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("encode");
    let listener = ListenerOptions::new()
        .name(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
        .create_sync()
        .unwrap();
    let mut sender = IpcSenderWithContext::<ServerToClientMsg>::new(
        yinz_ipc::consts::ipc_connect(&path).unwrap(),
    );
    sender
        .send_server_msg(ServerToClientMsg::Render {
            content: "fragmented render".into(),
        })
        .unwrap();
    drop(sender);
    let mut bytes = Vec::new();
    listener.accept().unwrap().read_to_end(&mut bytes).unwrap();
    bytes
}

#[test]
fn fragmented_attachment_and_commit_resume_on_the_active_connection() {
    use crate::os_input_output::{get_client_os_input, ServerConnection};
    use interprocess::TryClone;
    use std::io::Write;
    for split in [2, 5] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("destination");
        let listener = ListenerOptions::new()
            .name(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
            .create_sync()
            .unwrap();
        let (done, result) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            done.send(attach_socket(
                Destination::Local,
                "destination".into(),
                None,
                &path,
                Default::default(),
                &AtomicBool::new(false),
                Instant::now() + Duration::from_secs(25),
            ))
            .unwrap();
        });
        let mut socket = listener.accept().unwrap();
        let mut peer =
            IpcReceiverWithContext::<ClientToServerMsg>::new(socket.try_clone().unwrap());
        peer.set_recv_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        assert!(matches!(
            peer.recv_client_msg(),
            IpcRecv::Msg(ClientToServerMsg::AttachClient { .. }, _)
        ));
        let frame = render_frame();
        let mut start = 0;
        for end in [1, 3, 5, frame.len() - 1] {
            socket.write_all(&frame[start..end]).unwrap();
            // Attachment stays pending across at least two 100ms receive timeouts.
            assert!(matches!(
                result.recv_timeout(Duration::from_millis(250)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ));
            start = end;
        }
        socket.write_all(&frame[start..]).unwrap();
        let prepared = result
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        worker.join().unwrap();
        socket.write_all(&frame[..split]).unwrap();
        let args = Default::default();
        let mut local_config = LocalConfig::default();
        let mut sessions = Sessions::new(None, "source".into(), &args, &mut local_config);
        sessions.preparation = Some(Preparation {
            id: "pending".into(),
            cancelled: Default::default(),
            receiver: mpsc::channel().1,
            result: Some(Ok(prepared)),
        });
        let mut prepared = sessions.commit("pending").unwrap();
        assert_eq!(prepared.buffered.len(), 1);
        let os = get_client_os_input().unwrap();
        os.install_session_connection(
            prepared.sender.take().unwrap(),
            prepared.receiver.take().unwrap(),
        )
        .unwrap();
        socket.write_all(&frame[split..]).unwrap();
        assert!(
            matches!(os.recv_from_server(), IpcRecv::Msg(ServerToClientMsg::Render { content }, _) if content == "fragmented render")
        );
        os.send_to_server(ClientToServerMsg::ClientExited);
        assert!(matches!(
            peer.recv_client_msg(),
            IpcRecv::Msg(ClientToServerMsg::ClientExited, _)
        ));
    }
}

#[test]
fn partial_attachment_obeys_cancellation_and_the_attachment_deadline() {
    use interprocess::TryClone;
    use std::io::Write;
    for cancel in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("destination");
        let listener = ListenerOptions::new()
            .name(path.as_path().to_fs_name::<GenericFilePath>().unwrap())
            .create_sync()
            .unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        let (done, result) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            done.send(attach_socket(
                Destination::Local,
                "destination".into(),
                None,
                &path,
                Default::default(),
                &cancelled,
                Instant::now() + Duration::from_secs(2),
            ))
            .unwrap();
        });
        let mut socket = listener.accept().unwrap();
        let mut peer =
            IpcReceiverWithContext::<ClientToServerMsg>::new(socket.try_clone().unwrap());
        peer.set_recv_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        assert!(matches!(
            peer.recv_client_msg(),
            IpcRecv::Msg(ClientToServerMsg::AttachClient { .. }, _)
        ));
        socket.write_all(&4096u32.to_le_bytes()).unwrap();
        assert!(matches!(
            result.recv_timeout(Duration::from_millis(250)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        stop.store(cancel, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(10);
        let error = loop {
            // Timing is under test: keep the frame arriving inside each per-read timeout.
            // The destination may close between publishing its error and this write.
            let _ = socket.write_all(&[0]);
            match result.recv_timeout(Duration::from_millis(20)) {
                Ok(result) => break result.err().unwrap(),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    assert!(
                        Instant::now() < deadline,
                        "attachment ignored cancellation/deadline"
                    );
                },
                Err(error) => panic!("attachment worker disconnected: {error}"),
            }
        };
        assert!(
            error.to_string().contains(if cancel {
                "Interrupted"
            } else {
                "did not finish attachment"
            }),
            "{error:#}"
        );
        assert!(matches!(
            peer.recv_client_msg(),
            IpcRecv::Msg(ClientToServerMsg::ClientExited, _)
        ));
        worker.join().unwrap();
    }
}

#[test]
fn local_switch_options_exclude_the_source_layout_and_reload_without_losing_cli_overrides() {
    use yinz_cli::args::{CliArgs, Command, SessionCommand, Sessions as CliSessions};
    use yinz_cli::options::CliOptions;
    use yinz_types::{
        data::{InputMode, SessionConnection},
        input::options::Options,
    };
    if yinz_test_support::reexec_test() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("YINZ_SOCKET_DIR", dir.path());
        std::env::set_var(yinz_process::pty_host::PANE_REF_ENV_KEY, "unavailable");
    }
    std::fs::create_dir_all(&*YINZ_SOCK_DIR).unwrap();
    let config = dir.path().join("config.kdl");
    let theme_dir = dir.path().join("themes");
    std::fs::create_dir(&theme_dir).unwrap();
    std::fs::write(
        theme_dir.join("custom.kdl"),
        include_str!("../../../yinz-config/assets/themes/noctis-uva.kdl")
            .replace("noctis-uva", "switch-theme"),
    )
    .unwrap();
    for command in [
        None,
        Some(Command::Options(Box::new(CliOptions {
            default_mode: Some(InputMode::Normal),
            ..Default::default()
        }))),
        Some(Command::Sessions(CliSessions::Attach {
            session_name: Some("source".into()),
            create: false,
            remote: None,
            index: None,
            create_background: false,
            force_run_commands: false,
            options: Some(Box::new(SessionCommand::Options(CliOptions {
                default_mode: Some(InputMode::Normal),
                ..Default::default()
            }))),
        })),
    ] {
        std::fs::write(&config, "").unwrap();
        let args = CliArgs {
            config: Some(config.clone()),
            config_dir: Some(dir.path().into()),
            command,
            layout_string: Some("default_mode \"locked\"\nlayout { pane; }".into()),
            ..Default::default()
        };
        let mut local = LocalConfig::load(&args).unwrap();
        let explicit = args.command.is_some();
        let expected = if explicit {
            Some(InputMode::Normal)
        } else {
            local.options.default_mode
        };
        assert_ne!(expected, Some(InputMode::Locked));
        assert_eq!(
            yinz_cli::setup::Setup::from_cli_args(&args)
                .unwrap()
                .2
                .default_mode,
            Some(if explicit {
                InputMode::Normal
            } else {
                InputMode::Locked
            })
        );
        assert!(local.config.themes.get_theme("switch-theme").is_some());
        std::fs::write(&config, "mouse_mode false\n").unwrap();
        let socket = YINZ_SOCK_DIR.join("target");
        let listener = ListenerOptions::new()
            .name(socket.as_path().to_fs_name::<GenericFilePath>().unwrap())
            .create_sync()
            .unwrap();
        let server = std::thread::spawn(move || {
            let mut sender =
                IpcSenderWithContext::<ServerToClientMsg>::new(listener.accept().unwrap());
            let mut receiver = sender.get_receiver::<ClientToServerMsg>().unwrap();
            receiver
                .set_recv_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let Some((ClientToServerMsg::AttachClient { cli_assets, .. }, _)) =
                receiver.recv_client_msg().msg()
            else {
                panic!("missing attachment")
            };
            assert!(cli_assets.layout.is_none());
            let options = cli_assets.configuration_options.unwrap();
            assert_eq!(options.default_mode, expected);
            assert_eq!(options.mouse_mode, Some(false));
            let destination = Options {
                default_mode: Some(InputMode::Pane),
                ..Default::default()
            }
            .merge(options);
            assert_eq!(destination.default_mode, expected.or(Some(InputMode::Pane)));
            sender
                .send_server_msg(ServerToClientMsg::Render {
                    content: "ready".into(),
                })
                .unwrap();
            assert!(matches!(
                receiver.recv_client_msg(),
                IpcRecv::Msg(ClientToServerMsg::ClientExited, _)
            ));
        });
        let mut sessions = Sessions::new(None, "source".into(), &args, &mut local);
        assert_eq!(
            sessions
                .handle(
                    prepare_request(Destination::Local, "target"),
                    &FakeClientOsApi::new()
                )
                .0
                .unwrap(),
            ClientReply::Pending
        );
        let ready = sessions
            .preparation
            .as_ref()
            .unwrap()
            .receiver
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        drop(ready);
        drop(sessions);
        server.join().unwrap();
        let valid_options = local.options.clone();
        std::fs::write(&config, "invalid { config").unwrap();
        local.reload(&args);
        assert_eq!(local.options, valid_options);
        assert!(local.config.themes.get_theme("switch-theme").is_some());
        let listener = ListenerOptions::new()
            .name(socket.as_path().to_fs_name::<GenericFilePath>().unwrap())
            .create_sync()
            .unwrap();
        let server = std::thread::spawn(move || {
            let mut sender =
                IpcSenderWithContext::<ServerToClientMsg>::new(listener.accept().unwrap());
            let mut receiver = sender.get_receiver::<ClientToServerMsg>().unwrap();
            assert!(matches!(
                receiver.recv_client_msg(),
                IpcRecv::Msg(ClientToServerMsg::ConnStatus, _)
            ));
            sender
                .send_server_msg(ServerToClientMsg::Connected {
                    warp_companions: false,
                    warp_held_terminals: false,
                })
                .unwrap();
        });
        let info = legacy_target(
            SessionConnection {
                name: Some("target".into()),
                ..Default::default()
            },
            None,
            &local.options,
            Default::default(),
        )
        .unwrap();
        let mut os: Box<dyn ClientOsApi> = Box::new(FakeClientOsApi::new());
        let (message, _) = crate::startup::StartupContext::detached(&args, &local.options)
            .first_message(&mut os, info, Default::default());
        let ClientToServerMsg::AttachClient { cli_assets, .. } = message else {
            panic!("expected legacy attachment")
        };
        assert_eq!(cli_assets.configuration_options, Some(valid_options));
        server.join().unwrap();
    }
}

#[test]
fn legacy_remote_bootstrap_signals_restore_the_terminal_and_clean_up_children() {
    use crate::os_input_output::{get_client_os_input, TtyControl};
    use nix::sys::{
        signal::{kill, killpg, Signal},
        termios::{tcgetattr, LocalFlags},
        wait::{waitpid, WaitPidFlag},
    };
    use nix::unistd::Pid;
    use std::{
        fs::File,
        io::{BufRead, Read, Write},
        os::{
            fd::AsFd,
            unix::{fs::OpenOptionsExt, process::CommandExt},
        },
        process::{Command, Stdio},
    };
    const PHASE: &str = "YINZ_TEST_RECONNECT_PHASE";
    const TEST: &str = "session_switch::tests::legacy_remote_bootstrap_signals_restore_the_terminal_and_clean_up_children";
    if let Ok(phase) = std::env::var(PHASE) {
        let root = std::path::PathBuf::from(std::env::var_os("YINZ_TEST_RECONNECT_ROOT").unwrap());
        let socket = root.join("forward.sock");
        if phase == "forward" {
            let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
            std::fs::write(root.join("forward.pid"), std::process::id().to_string()).unwrap();
            // HostForward's readiness connection closes; the next accept waits until cleanup kills us.
            drop(listener.accept().unwrap());
            let _ = listener.accept();
            return;
        }
        let forward = yinz_process::ssh::HostForward::start(
            "host",
            &Default::default(),
            &socket,
            "/remote/socket",
            None,
        )
        .unwrap();
        remote_sessions::set_forward(Some(forward));
        let original = tcgetattr(std::io::stdin()).unwrap();
        let mut os = get_client_os_input().unwrap();
        os.set_raw_mode();
        assert!(!tcgetattr(std::io::stdin())
            .unwrap()
            .local_flags
            .contains(LocalFlags::ISIG));
        let outcome = legacy_reconnect(
            yinz_types::data::SessionConnection {
                name: Some("next".into()),
                ..Default::default()
            },
            Some("host".into()),
            &Default::default(),
            &os,
        )
        .err()
        .unwrap();
        assert_eq!(outcome.exit_status, crate::INTERRUPTED_EXIT_STATUS);
        assert_eq!(tcgetattr(std::io::stdin()).unwrap(), original);
        assert!(!socket.exists(), "source forward socket leaked");
        for name in ["forward.pid", "bootstrap.pid"] {
            let pid: i32 = std::fs::read_to_string(root.join(name))
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            assert_eq!(
                waitpid(Pid::from_raw(pid), Some(WaitPidFlag::WNOHANG)),
                Err(nix::errno::Errno::ECHILD),
                "SSH child was not reaped"
            );
        }
        std::process::exit(outcome.exit_status);
    }
    struct Group(Pid);
    impl Drop for Group {
        fn drop(&mut self) {
            let _ = killpg(self.0, Signal::SIGKILL);
        }
    }
    for signal in [
        None,
        Some(Signal::SIGTERM),
        Some(Signal::SIGHUP),
        Some(Signal::SIGQUIT),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let ready = dir.path().join("ready");
        nix::unistd::mkfifo(
            &ready,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .unwrap();
        let fifo = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(nix::libc::O_NONBLOCK)
            .open(&ready)
            .unwrap();
        yinz_test_support::write_warm_script(
            &dir.path().join("ssh"),
            &format!(
                r#"
case " $* " in
    *" -L "*) export {PHASE}=forward; exec "$YINZ_TEST_RECONNECT_EXE" --exact {TEST} --nocapture ;;
esac
printf '%s\n' "$$" > "$YINZ_TEST_RECONNECT_ROOT/bootstrap.pid"
printf 'ready\n' > "$YINZ_TEST_RECONNECT_ROOT/ready"
exec sleep 60
"#
            ),
        );
        let mut path = vec![dir.path().to_path_buf()];
        path.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let pty = nix::pty::openpty(None, None).unwrap();
        let exe = std::env::current_exe().unwrap();
        let mut command = Command::new(&exe);
        command
            .args(["--exact", TEST, "--nocapture"])
            .env(PHASE, "client")
            .env("YINZ_TEST_RECONNECT_ROOT", dir.path())
            .env("YINZ_TEST_RECONNECT_EXE", &exe)
            .env("PATH", std::env::join_paths(path).unwrap())
            .stdin(Stdio::from(pty.slave.try_clone().unwrap()))
            .stdout(Stdio::from(pty.slave.try_clone().unwrap()))
            .stderr(Stdio::from(pty.slave.try_clone().unwrap()));
        unsafe {
            command.pre_exec(|| {
                nix::unistd::setsid()?;
                if nix::libc::ioctl(0, nix::libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().unwrap();
        let pid = Pid::from_raw(child.id() as i32);
        let group = Group(pid);
        let mut fds = [nix::poll::PollFd::new(
            fifo.as_fd(),
            nix::poll::PollFlags::POLLIN,
        )];
        assert_eq!(
            nix::poll::poll(&mut fds, 30000u16).unwrap(),
            1,
            "SSH bootstrap never started"
        );
        let mut line = String::new();
        std::io::BufReader::new(fifo).read_line(&mut line).unwrap();
        assert_eq!(line, "ready\n");
        assert!(tcgetattr(&pty.slave)
            .unwrap()
            .local_flags
            .contains(LocalFlags::ISIG));
        if let Some(signal) = signal {
            kill(pid, signal).unwrap();
        } else {
            // Exercise the terminal driver, not only a synthetic SIGINT.
            (&File::from(pty.master.try_clone().unwrap()))
                .write_all(b"\x03")
                .unwrap();
        }
        let (done, result) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || done.send(child.wait().unwrap()).unwrap());
        let status = result
            .recv_timeout(Duration::from_secs(10))
            .expect("cancellation did not stop SSH bootstrap");
        waiter.join().unwrap();
        nix::fcntl::fcntl(
            &pty.master,
            nix::fcntl::FcntlArg::F_SETFL(nix::fcntl::OFlag::O_NONBLOCK),
        )
        .unwrap();
        let mut output = String::new();
        let _ = File::from(pty.master).read_to_string(&mut output);
        assert_eq!(
            status.code(),
            Some(crate::INTERRUPTED_EXIT_STATUS),
            "{signal:?}: {output}"
        );
        drop(group);
    }
}
