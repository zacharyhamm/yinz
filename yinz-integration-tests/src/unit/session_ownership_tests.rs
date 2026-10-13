use super::*;
use crate::TERMINAL_SIZE;
use std::sync::Barrier;
use yinz_ipc::session_ownership::SessionOwnership;
use yinz_runtime::consts::YINZ_SOCK_DIR;

#[test]
fn concurrent_creators_attach_without_reinitializing_panes() {
    test_env::init();
    let _slot = test_env::acquire_concurrency_slot();
    let session_name = test_env::unique_session_name();
    let cli_args = CliArgs {
        config: Some(test_env::write_config(
            &session_name,
            "pane_frame_style \"full\"",
        )),
        data_dir: Some(test_env::init().join("data")),
        ..Default::default()
    };
    let (config, _, options, _, _) = Setup::from_cli_args(&cli_args).unwrap();
    let os = FakeServerOsApi::default();
    let barrier = Arc::new(Barrier::new(2));
    let (returned, returns) = mpsc::channel();
    let mut servers = Vec::new();
    let mut clients = Vec::new();
    for name in ["creator-alpha", "creator-beta"] {
        let server = Arc::new(Mutex::new(None));
        let os = os.clone();
        let server_thread = server.clone();
        let barrier = barrier.clone();
        let returned = returned.clone();
        // Both clients have selected New, but neither server can claim the name yet.
        let spawner = Box::new(move |path| {
            barrier.wait();
            let thread = std::thread::spawn(move || {
                yinz_server::start_server_impl(Box::new(os), path, false);
                returned.send(()).unwrap();
            });
            *server_thread.lock().unwrap() = Some(thread);
        });
        let (client_os, handle) = FakeClientOsApi::new(TERMINAL_SIZE, Some(spawner));
        let thread = spawn_client_thread(
            client_os,
            cli_args.clone(),
            config.clone(),
            options.clone(),
            ClientInfo::New(
                session_name.clone().into(),
                Some(LayoutInfo::Stringified(format!(
                    "layout {{ pane name=\"{name}\"; }}"
                ))),
                None,
            ),
            None,
        );
        clients.push(TestClient {
            fake_client_handle: handle,
            thread: Some(thread),
        });
        servers.push(server);
    }
    // The losing server returns before any session is shut down.
    returns.recv_timeout(Duration::from_secs(10)).unwrap();
    let terminal_id = os
        .shared_ptys
        .wait_for("first creator's pane", |ptys| ptys.spawn_queue.pop_front());
    let terminal = FakePtyHandle {
        terminal_id,
        shared_ptys: os.shared_ptys.clone(),
    };
    terminal.output(b"ownership-marker\r\n");
    let mut initialized_layout = None;
    for client in &clients {
        let grid = client.wait_until("both creators share the initialized pane", |grid| {
            grid.contains("ownership-marker")
                && (grid.contains("creator-alpha") || grid.contains("creator-beta"))
        });
        let alpha = grid.contains("creator-alpha");
        assert_ne!(alpha, grid.contains("creator-beta"));
        if let Some(first) = initialized_layout {
            assert_eq!(
                first, alpha,
                "both clients must see the first creation layout"
            );
        }
        initialized_layout = Some(alpha);
    }
    assert!(
        returns.try_recv().is_err(),
        "the winning server must still be running"
    );
    assert!(os.shared_ptys.read(|ptys| ptys.spawn_queue.is_empty()));
    assert_eq!(os.shared_ptys.read(|ptys| ptys.fake_pty_states.len()), 1);
    let socket = YINZ_SOCK_DIR.join(&session_name);
    assert!(SessionOwnership::acquire(&socket).unwrap().is_none());

    let (client_os, handle) = FakeClientOsApi::new(TERMINAL_SIZE, None);
    let thread = spawn_client_thread(
        client_os,
        cli_args,
        config,
        options.clone(),
        ClientInfo::Attach(session_name.into(), options),
        None,
    );
    let reconnect = TestClient {
        fake_client_handle: handle,
        thread: Some(thread),
    };
    reconnect.wait_until("reconnect sees the original pane contents", |grid| {
        grid.contains("ownership-marker")
    });
    reconnect.send_stdin(b"same-pane;");
    terminal.wait_for_stdin("reconnect reaches the original terminal", |bytes| {
        bytes.ends_with(b"same-pane;")
    });
    assert_eq!(
        os.shared_ptys
            .read(|ptys| ptys.fake_pty_states.keys().copied().collect::<Vec<_>>()),
        vec![terminal_id]
    );
    clients[0].fake_client_handle.kill_session();
    for client in &mut clients {
        client.join();
    }
    let mut reconnect = reconnect;
    reconnect.join();
    for server in servers {
        join_server_thread_with_timeout(server.lock().unwrap().take().unwrap());
    }
    assert!(!socket.exists());
    assert!(SessionOwnership::acquire(&socket).unwrap().is_some());
    test_env::assert_no_background_panics();
}
