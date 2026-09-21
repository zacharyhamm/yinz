#![cfg(unix)]

use std::time::Duration;
use zellij_cli::args::CliAction;
use zellij_integration_tests::{LayoutInfo, TestRunner, TestSession, PROMPT, TERMINAL_SIZE};

fn start() -> TestSession {
    TestRunner::new(TERMINAL_SIZE)
        .skip_concurrency_slot()
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified(
            r#"layout {
            pane command="sh" firecracker_vm="api"
            pane command="sh"
        }"#
            .into(),
        ))
        .start()
}
fn pane_count(session: &TestSession, vm: &str) -> usize {
    let (code, lines) = session.run_cli_action_reporting(CliAction::ListPanes {
        tab: false,
        command: false,
        state: false,
        geometry: false,
        all: true,
        json: true,
    });
    assert_eq!(code, 0);
    let panes: Vec<zellij_types::data::PaneListEntry> = serde_json::from_str(&lines).unwrap();
    panes
        .iter()
        .filter(|entry| entry.pane_info.target.firecracker_vm() == Some(vm))
        .count()
}
#[test]
fn close_confirms_removal_in_the_named_session_and_preserves_other_sessions() {
    let mut other = start();
    for _ in 0..2 {
        other.expect_pty_spawn().output(PROMPT);
    }
    other.wait_until("shell rendered", |screen| screen.contains("$"));
    let mut owner = start();
    for _ in 0..2 {
        owner.expect_pty_spawn().output(PROMPT);
    }
    owner.wait_until("shell rendered", |screen| screen.contains("$"));
    assert_eq!(pane_count(&owner, "api"), 1);
    assert_eq!(pane_count(&other, "api"), 1);
    zellij_session::firecracker::close_panes(owner.session_name(), "api", Duration::from_secs(10))
        .unwrap();
    assert_eq!(pane_count(&owner, "api"), 0);
    assert_eq!(pane_count(&other, "api"), 1);
    owner.quit();
    other.quit();
}
#[test]
fn closing_the_final_pane_finishes_while_shutdown_waits_for_the_provider_lock() {
    let mut owner = TestRunner::new(TERMINAL_SIZE)
        .with_config("load_plugins clear-defaults=true {}")
        .with_layout(LayoutInfo::Stringified(
            r#"layout { pane command="sh" firecracker_vm="api"; }"#.into(),
        ))
        .start();
    // Wait for the deferred spawn, after the remote terminal has been reserved.
    owner.expect_pty_spawn().output(PROMPT);
    owner.wait_until("shell rendered", |screen| screen.contains("$"));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("provider.lock");
    let lock = std::fs::File::create(&path).unwrap();
    lock.lock().unwrap();
    owner.set_firecracker_provider_lock(path);

    // The provider holds its lock throughout close_panes. The server must remove its
    // socket before its own stop_session waits for that lock, even for the last pane.
    let result = zellij_session::firecracker::close_panes(
        owner.session_name(),
        "api",
        Duration::from_secs(15),
    );
    assert_eq!(owner.firecracker_session_stops(), 0);
    drop(lock);
    owner.wait_for_exit();
    result.unwrap();
    assert_eq!(owner.firecracker_session_stops(), 1);
}

#[test]
fn closure_timeout_returns_failure_without_requesting_pane_closure() {
    let mut owner = start();
    for _ in 0..2 {
        owner.expect_pty_spawn().output(PROMPT);
    }
    owner.wait_until("shell rendered", |screen| screen.contains("$"));
    let error =
        zellij_session::firecracker::close_panes(owner.session_name(), "api", Duration::ZERO)
            .unwrap_err();
    assert!(error.to_string().contains("lease was not released"));
    assert_eq!(pane_count(&owner, "api"), 1);
    owner.quit();
}
#[test]
fn an_absent_server_allows_orphan_cleanup() {
    let mut owner = start();
    for _ in 0..2 {
        owner.expect_pty_spawn().output(PROMPT);
    }
    owner.wait_until("shell rendered", |screen| screen.contains("$"));
    let name = owner.session_name().to_owned();
    owner.quit();
    zellij_session::firecracker::close_panes(&name, "api", Duration::from_secs(5)).unwrap();
}

#[test]
fn a_server_that_acknowledges_close_but_keeps_the_pane_hits_the_deadline() {
    use interprocess::local_socket::traits::Listener;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };
    use zellij_ipc::{
        consts::{ipc_bind, ipc_connect},
        ipc::{ClientToServerMsg, IpcReceiverWithContext, ServerToClientMsg},
    };
    use zellij_types::{
        data::{PaneInfo, PaneListEntry},
        input::actions::Action,
    };
    zellij_integration_tests::test_env::init();
    let session = "vm-close-stalled";
    let path = zellij_runtime::consts::ZELLIJ_SOCK_DIR.join(session);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let listener = ipc_bind(&path).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let closes = Arc::new(AtomicUsize::new(0));
    let worker_stop = stop.clone();
    let worker_closes = closes.clone();
    let mut pane = PaneInfo::default();
    pane.target = zellij_types::command::PaneTarget::Firecracker {
        vm: "api".into(),
        image: None,
        worktree: None,
    };
    let manifest = serde_json::to_string(&vec![PaneListEntry {
        pane_info: pane,
        tab_id: Default::default(),
        tab_position: Default::default(),
        tab_name: "work".into(),
        pane_command: None,
        pane_cwd: None,
    }])
    .unwrap();
    let worker = std::thread::spawn(move || loop {
        let stream = listener.accept().unwrap();
        if worker_stop.load(Ordering::SeqCst) {
            break;
        }
        let mut receiver = IpcReceiverWithContext::<ClientToServerMsg>::new(stream);
        receiver
            .set_recv_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let Some((message, _)) = receiver.recv_client_msg().msg() else {
            continue;
        };
        let response = match message {
            ClientToServerMsg::ConnStatus => ServerToClientMsg::Connected {
                warp_companions: true,
            },
            ClientToServerMsg::Action {
                action: Action::ListPanes { .. },
                ..
            } => ServerToClientMsg::Log {
                lines: vec![manifest.clone()],
            },
            ClientToServerMsg::Action {
                action: Action::CloseTerminalPane { .. },
                ..
            } => {
                worker_closes.fetch_add(1, Ordering::SeqCst);
                ServerToClientMsg::UnblockInputThread
            },
            _ => continue,
        };
        let _ = receiver
            .get_sender::<ServerToClientMsg>()
            .send_server_msg(response);
    });
    let result =
        zellij_session::firecracker::close_panes(session, "api", Duration::from_millis(500));
    stop.store(true, Ordering::SeqCst);
    let _wake = ipc_connect(&path).unwrap();
    worker.join().unwrap();
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("lease was not released"));
    assert_eq!(
        closes.load(Ordering::SeqCst),
        1,
        "a close acknowledgement is not proof of removal"
    );
}
