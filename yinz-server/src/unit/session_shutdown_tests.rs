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
