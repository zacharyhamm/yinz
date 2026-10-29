use super::*;
use std::sync::atomic::AtomicBool;

#[test]
fn shutdown_keeps_workers_available_until_the_screen_finishes() {
    let shutdown = Shutdown::default();
    let (to_screen, screen_rx) = channels::unbounded();
    let to_screen = shutdown.sender(to_screen);
    let (to_screen_bounded, bounded_screen_rx) = channels::bounded(1);
    let to_screen_bounded = shutdown.sender(to_screen_bounded);
    let (ack, acknowledgements) = channels::unbounded();
    let (done, completed) = channels::unbounded();
    let screen_finished = Arc::new(AtomicBool::new(false));

    macro_rules! worker {
        ($work:pat, $exit:path) => {{
            let (sender, receiver) = channels::unbounded();
            let ack = ack.clone();
            let done = done.clone();
            let screen_finished = screen_finished.clone();
            let to_screen = to_screen_bounded.clone();
            let thread = thread::spawn(move || {
                let (instruction, _) = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
                assert!(
                    matches!(instruction, $work),
                    "worker exited before screen work"
                );
                ack.send(()).unwrap();
                let (instruction, _) = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
                assert!(matches!(instruction, $exit));
                assert!(screen_finished.load(Ordering::SeqCst));
                // Reverse traffic after the screen has dropped both of its receivers.
                to_screen.send(ScreenInstruction::Render).unwrap();
                done.send(()).unwrap();
            });
            (shutdown.sender(sender), Some(thread))
        }};
    }

    let (to_pty, pty_thread) = worker!(PtyInstruction::CloseTab(_), PtyInstruction::Exit);
    let (to_plugin, plugin_thread) = worker!(PluginInstruction::Update(_), PluginInstruction::Exit);
    let (to_pty_writer, pty_writer_thread) = worker!(
        PtyWriteInstruction::ApplyCachedResizes,
        PtyWriteInstruction::Exit
    );
    let (to_background_jobs, background_jobs_thread) =
        worker!(BackgroundJob::ReportPluginList(_), BackgroundJob::Exit);
    let senders = ThreadSenders {
        to_screen: Some(to_screen),
        to_pty: Some(to_pty),
        to_plugin: Some(to_plugin),
        to_pty_writer: Some(to_pty_writer),
        to_background_jobs: Some(to_background_jobs),
        ..Default::default()
    };
    let screen_senders = senders.clone();
    let screen_thread = thread::spawn(move || {
        assert!(matches!(
            screen_rx.recv_timeout(Duration::from_secs(5)).unwrap().0,
            ScreenInstruction::Exit
        ));
        screen_senders
            .send_to_pty(PtyInstruction::CloseTab(vec![]))
            .unwrap();
        screen_senders
            .send_to_plugin(PluginInstruction::Update(vec![]))
            .unwrap();
        screen_senders
            .send_to_pty_writer(PtyWriteInstruction::ApplyCachedResizes)
            .unwrap();
        screen_senders
            .send_to_background_jobs(BackgroundJob::ReportPluginList(BTreeMap::new()))
            .unwrap();
        for _ in 0..4 {
            acknowledgements
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
        }
        drop(screen_rx);
        drop(bounded_screen_rx);
        screen_finished.store(true, Ordering::SeqCst);
        done.send(()).unwrap();
    });

    drop(SessionMetaData {
        shutdown,
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
        screen_thread: Some(screen_thread),
        pty_thread,
        plugin_thread,
        pty_writer_thread,
        background_jobs_thread,
        config_file_path: None,
        config_dir: None,
    });
    // Drop ignores join errors, so require explicit success from every fake worker and screen.
    assert_eq!(completed.try_iter().count(), 5);
}

#[test]
fn shutdown_does_not_hide_missing_bus_senders() {
    let shutdown = Shutdown::default();
    let (sender, receiver) = channels::unbounded();
    let senders = ThreadSenders {
        to_screen: Some(shutdown.sender(sender)),
        ..Default::default()
    };
    drop(receiver);
    assert!(senders.send_to_screen(ScreenInstruction::Exit).is_err());
    shutdown.begin();
    senders.send_to_screen(ScreenInstruction::Exit).unwrap();
    assert!(senders.send_to_pty(PtyInstruction::Exit).is_err());
    assert!(senders.send_to_plugin(PluginInstruction::Exit).is_err());
    assert!(senders
        .send_to_pty_writer(PtyWriteInstruction::Exit)
        .is_err());
    assert!(senders
        .send_to_background_jobs(BackgroundJob::Exit)
        .is_err());
    assert!(senders
        .send_to_server(ServerInstruction::KillSession)
        .is_err());
}

struct SpawnFailure {
    name: &'static str,
    to_server: SenderWithContext<ServerInstruction>,
    completed: yinz_test_support::Watched<Vec<&'static str>>,
}

thread_local! {
    static SPAWN_FAILURE: std::cell::RefCell<Option<SpawnFailure>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn injected_worker_spawn(
    name: &'static str,
) -> Option<std::io::Result<thread::JoinHandle<()>>> {
    SPAWN_FAILURE.with(|injection| {
        let injection = injection.borrow();
        let injection = injection.as_ref()?;
        if name == injection.name {
            return Some(Err(std::io::Error::from_raw_os_error(nix::libc::EAGAIN)));
        }
        let to_server = injection.to_server.clone();
        let completed = injection.completed.clone();
        Some(thread::Builder::new().name(name.into()).spawn(move || {
            // Startup failure must return ownership before joining a worker whose server queue is full.
            assert!(to_server
                .send_required(ServerInstruction::KillSession)
                .is_err());
            completed.lock().push(name);
        }))
    })
}

#[test]
fn failed_worker_spawns_preserve_partial_startup_until_the_server_queue_disconnects() {
    for (index, name) in ["pty", "screen", "plugins", "pty_writer", "background_jobs"]
        .into_iter()
        .enumerate()
    {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().to_path_buf();
        let shutdown = Shutdown::default();
        let (to_server, server_receiver) = channels::bounded(1);
        let to_server = shutdown.sender(to_server);
        to_server.send(ServerInstruction::KillSession).unwrap();
        let completed = yinz_test_support::Watched::new(Vec::new());
        let worker_completed = completed.clone();
        let (result_tx, result_rx) = channels::bounded(1);
        let initializer = thread::spawn(move || {
            SPAWN_FAILURE.with(|injection| {
                injection.replace(Some(SpawnFailure {
                    name,
                    to_server: to_server.clone(),
                    completed: worker_completed,
                }));
            });
            let ownership = Arc::new(
                yinz_ipc::session_ownership::SessionOwnership::acquire_for_startup(
                    &path.join("session"),
                )
                .unwrap()
                .unwrap(),
            );
            let mut starting = None;
            let result = init_session(
                &mut starting,
                Box::new(crate::capturing_os::CapturingOs::default()),
                to_server,
                shutdown,
                ownership,
                envs::SessionState::new("startup-failure".into()),
                ClientAttributes::default(),
                Options::default(),
                Layout::default(),
                CliAssets {
                    data_dir: Some(path),
                    ..Default::default()
                },
                Config::default(),
                PluginAliases::default(),
                ClientId::new(1),
            );
            SPAWN_FAILURE.with(|injection| injection.take());
            result_tx.send((starting, result.unwrap_err())).unwrap();
        });
        let (starting, error) = result_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(format!("{error:#}").contains(&format!("start {name} thread")));
        assert!(starting.is_some(), "partial worker ownership was lost");
        assert!(
            completed.lock().is_empty(),
            "the bounded queue must stay full"
        );
        // This is the server shutdown order: disconnect the undrained server queue, then join.
        drop(server_receiver);
        drop(starting);
        initializer.join().unwrap();
        assert_eq!(
            completed.lock().len(),
            index,
            "not every started worker joined"
        );
    }
}

fn empty_session() -> SessionMetaData {
    SessionMetaData {
        shutdown: Shutdown::default(),
        senders: ThreadSenders::default(),
        data_access: Arc::new(DataAccess::sqlite(Path::new("/unused"))),
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
    }
}

fn poison<T>(lock: &RwLock<T>) {
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = lock.write().unwrap();
        panic!("injected host panic while changing session state");
    }))
    .is_err());
}

#[test]
fn poisoned_session_locks_reject_live_access_and_retain_unpublished_workers() {
    for poison_metadata in [false, true] {
        let state = RwLock::new(SessionState::new());
        let data = Arc::new(RwLock::new(None));
        if poison_metadata {
            poison(&data);
        } else {
            poison(&state);
        }
        let mut starting = Some(empty_session());
        let error =
            publish_session(&mut starting, &state, &data, ClientId::new(1), false).unwrap_err();
        assert!(error.to_string().contains(if poison_metadata {
            "publish initialized session"
        } else {
            "attach first client"
        }));
        assert!(
            starting.is_some(),
            "failed publication must leave cleanup ownership outside the loop"
        );
        assert!(take_session_for_shutdown(&data).is_none());
        if poison_metadata {
            assert!(with_session(&data, |_| panic!("poisoned read ran live work")).is_err());
            assert!(with_session_mut(&data, |_| panic!("poisoned write ran live work")).is_err());
        }
    }
}

#[test]
fn poisoned_metadata_is_taken_for_shutdown_without_holding_its_lock_during_join() {
    let data = Arc::new(RwLock::new(None));
    let (to_screen, screen_rx) = channels::unbounded();
    let (done, completed) = channels::bounded(1);
    let worker_data = data.clone();
    let mut session = empty_session();
    session.senders.to_screen = Some(session.shutdown.sender(to_screen));
    session.screen_thread = Some(thread::spawn(move || {
        assert!(matches!(
            screen_rx.recv_timeout(Duration::from_secs(5)).unwrap().0,
            ScreenInstruction::Exit
        ));
        match worker_data.try_read() {
            Err(std::sync::TryLockError::Poisoned(error)) => assert!(error.into_inner().is_none()),
            _ => panic!("shutdown must release the poisoned metadata lock before joining"),
        }
        done.send(()).unwrap();
    }));
    *data.write().unwrap() = Some(session);
    poison(&data);
    drop(take_session_for_shutdown(&data));
    completed.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        data.is_poisoned(),
        "disposal must not re-enable live access"
    );
}

#[test]
fn poisoned_client_state_still_closes_clients_and_watchers_on_shutdown() {
    let state = RwLock::new(SessionState::new());
    let (client, watcher) = {
        let mut state = state.write().unwrap();
        let client = state.new_client().unwrap();
        let watcher = state.new_client().unwrap();
        state.convert_client_to_watcher(watcher);
        (client, watcher)
    };
    poison(&state);
    let capture = crate::capturing_os::CapturingOs::default();
    let mut os: Box<dyn ServerOsApi> = Box::new(capture.clone());
    notify_shutdown_error(&mut os, &state, "host worker failed");
    for id in [client, watcher] {
        assert!(capture.removed_clients.lock().contains(&id));
        assert_eq!(
            capture.server_to_client_messages.lock()[&id],
            vec![ServerToClientMsg::Exit {
                exit_reason: ExitReason::Error("host worker failed".into())
            }]
        );
    }
    assert!(state.is_poisoned());
}

#[test]
fn panic_reporting_releases_poisoned_locks_even_when_the_server_queue_is_full() {
    const CHILD: &str = "YINZ_TEST_PANIC_REPORT_QUEUE";
    const NAME: &str = "session_shutdown_tests::panic_reporting_releases_poisoned_locks_even_when_the_server_queue_is_full";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }
    let shutdown = Shutdown::default();
    let (to_server, server_receiver) = channels::bounded(1);
    shutdown
        .sender(to_server)
        .send(ServerInstruction::KillSession)
        .unwrap();
    let (to_panic, panic_receiver) = channels::unbounded();
    let original_hook = std::panic::take_hook();
    install_server_panic_hook(shutdown.sender(to_panic));
    let state = Arc::new(RwLock::new(SessionState::new()));
    let worker_state = state.clone();
    let (done, completed) = channels::bounded(1);
    let worker = thread::Builder::new()
        .name("poisoned-session-worker".into())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(|| {
                let _guard = worker_state.write().unwrap();
                panic!("injected panic while holding the session lock");
            });
            done.send(outcome.is_err()).unwrap();
        })
        .unwrap();
    let released = completed.recv_timeout(Duration::from_secs(5));
    // Release even a regressed bounded hook before assertions, so the test cannot strand it.
    drop(server_receiver);
    worker.join().unwrap();
    std::panic::set_hook(original_hook);
    assert!(released.unwrap());
    assert!(state.is_poisoned());
    assert!(
        matches!(panic_receiver.try_recv().unwrap().0, ServerInstruction::Error(message) if message.contains("injected panic"))
    );
}
