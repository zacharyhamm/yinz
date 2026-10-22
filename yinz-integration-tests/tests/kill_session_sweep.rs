#![cfg(unix)]

use yinz_integration_tests::{claim_first_terminal_and_wait_for_prompt, start_yinz};

/// A `kill-session` also reaps the supervisors an earlier incarnation of the
/// session left detached, and it does so under the session's own name.
///
/// The sweep is the last thing the server does with its panes, after its own
/// supervisors are terminated; `quit` sends the kill-session IPC the harness
/// tears down with and joins the server thread, so the list after it is final.
#[test]
fn a_kill_session_sweeps_the_orphans_of_its_own_name() {
    let mut yinz = start_yinz();
    let _first_pane = claim_first_terminal_and_wait_for_prompt(&yinz);
    assert!(
        yinz.orphan_sweeps().is_empty(),
        "orphans were swept while the session was still running"
    );
    let session_name = yinz.session_name().to_owned();

    yinz.quit();

    assert_eq!(
        yinz.orphan_sweeps(),
        vec![session_name],
        "a kill-session exit should sweep the orphans labelled with this session, once"
    );
}

/// Only a kill asks for the sweep. A server that exits because its last pane
/// closed was not told by anyone that the session's detached panes are done,
/// and must leave them for `pty-host list` to find.
#[test]
fn a_server_exiting_on_its_last_pane_does_not_sweep() {
    let mut yinz = start_yinz();
    let first_pane = claim_first_terminal_and_wait_for_prompt(&yinz);

    first_pane.exit(Some(0));
    yinz.wait_for_exit();

    assert!(
        yinz.orphan_sweeps().is_empty(),
        "a last-pane exit swept orphans: {:?}",
        yinz.orphan_sweeps()
    );
}

#[test]
fn a_kill_session_after_rename_sweeps_the_new_name() {
    let mut yinz = start_yinz();
    let _pane = claim_first_terminal_and_wait_for_prompt(&yinz);
    let new_name = format!("{}-renamed", yinz.session_name());
    let (code, output) =
        yinz.run_actions_reporting(vec![yinz_types::input::actions::Action::RenameSession {
            name: new_name.clone().into(),
        }]);
    assert_eq!(code, 0, "{output}");
    assert!(!output.contains("Failed"), "{output}");
    yinz.quit();
    assert_eq!(yinz.orphan_sweeps(), vec![new_name]);
}

#[test]
fn acknowledged_kill_replies_after_the_orphan_sweep() {
    let mut yinz = start_yinz();
    let _pane = claim_first_terminal_and_wait_for_prompt(&yinz);
    let name = yinz.session_name().to_owned();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            yinz_ipc::ipc::async_kill_session_with_completion(
                &yinz_runtime::consts::YINZ_SOCK_DIR.join(&name),
            ),
        )
        .await
        .unwrap()
        .unwrap();
    });
    assert_eq!(yinz.orphan_sweeps(), vec![name]);
    yinz.wait_for_exit();
}

#[test]
fn acknowledged_kill_reports_orphan_cleanup_failure() {
    let mut yinz = start_yinz();
    let _pane = claim_first_terminal_and_wait_for_prompt(&yinz);
    yinz.set_orphan_sweep_error("silent supervisor".to_owned());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let error = runtime.block_on(async {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            yinz_ipc::ipc::async_kill_session_with_completion(
                &yinz_runtime::consts::YINZ_SOCK_DIR.join(yinz.session_name()),
            ),
        )
        .await
        .unwrap()
        .unwrap_err()
    });
    assert!(error.to_string().contains("silent supervisor"), "{error}");
    yinz.wait_for_exit();
}

fn check_progress_opt_in(report_progress: bool) {
    use yinz_ipc::ipc::{
        ClientToServerMsg, IpcRecv, IpcSenderWithContext, IpcStream, ServerToClientMsg,
    };
    let mut yinz = start_yinz();
    let _pane = claim_first_terminal_and_wait_for_prompt(&yinz);
    let stream = yinz_ipc::consts::ipc_connect(
        &yinz_runtime::consts::YINZ_SOCK_DIR.join(yinz.session_name()),
    )
    .unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(stream);
    let mut receiver = sender.get_receiver::<ServerToClientMsg>().unwrap();
    sender
        .send_client_msg(ClientToServerMsg::KillSessionWithCompletion { report_progress })
        .unwrap();
    let mut progress = 0;
    loop {
        match receiver.recv_server_msg() {
            IpcRecv::Msg(ServerToClientMsg::KillSessionProgress, _) => progress += 1,
            IpcRecv::Msg(ServerToClientMsg::KillSessionComplete { error: None }, _) => break,
            IpcRecv::Msg(
                reply @ (ServerToClientMsg::KillSessionComplete { .. }
                | ServerToClientMsg::Exit { .. }),
                _,
            ) => {
                panic!("unexpected shutdown reply: {reply:?}");
            },
            // Broadcasts can already be queued when shutdown is accepted.
            IpcRecv::Msg(_, _) => {},
            _ => panic!("shutdown socket closed without completion"),
        }
    }
    assert_eq!(progress > 0, report_progress);
    assert_eq!(yinz.orphan_sweeps(), vec![yinz.session_name().to_owned()]);
    yinz.wait_for_exit();
    assert!(!yinz
        .received_server_messages()
        .iter()
        .any(|message| message == "KillSessionProgress"));
}

#[test]
fn shutdown_progress_is_sent_only_to_the_requesting_client() {
    check_progress_opt_in(true);
}

#[test]
fn completion_only_clients_do_not_receive_shutdown_progress() {
    check_progress_opt_in(false);
}

#[test]
fn a_disconnected_progress_client_does_not_cancel_cleanup() {
    use yinz_ipc::ipc::{ClientToServerMsg, IpcSenderWithContext};
    let mut yinz = start_yinz();
    let _pane = claim_first_terminal_and_wait_for_prompt(&yinz);
    let stream = yinz_ipc::consts::ipc_connect(
        &yinz_runtime::consts::YINZ_SOCK_DIR.join(yinz.session_name()),
    )
    .unwrap();
    let mut sender = IpcSenderWithContext::<ClientToServerMsg>::new(stream);
    sender
        .send_client_msg(ClientToServerMsg::KillSessionWithCompletion {
            report_progress: true,
        })
        .unwrap();
    drop(sender);
    yinz.wait_for_exit();
    assert_eq!(yinz.orphan_sweeps(), vec![yinz.session_name().to_owned()]);
}
