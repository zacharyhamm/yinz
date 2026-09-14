#![cfg(unix)]

use zellij_integration_tests::{claim_first_terminal_and_wait_for_prompt, start_zellij};

/// A `kill-session` also reaps the supervisors an earlier incarnation of the
/// session left detached, and it does so under the session's own name.
///
/// The sweep is the last thing the server does with its panes, after its own
/// supervisors are terminated; `quit` sends the kill-session IPC the harness
/// tears down with and joins the server thread, so the list after it is final.
#[test]
fn a_kill_session_sweeps_the_orphans_of_its_own_name() {
    let mut zellij = start_zellij();
    let _first_pane = claim_first_terminal_and_wait_for_prompt(&zellij);
    assert!(
        zellij.orphan_sweeps().is_empty(),
        "orphans were swept while the session was still running"
    );
    let session_name = zellij.session_name().to_owned();

    zellij.quit();

    assert_eq!(
        zellij.orphan_sweeps(),
        vec![session_name],
        "a kill-session exit should sweep the orphans labelled with this session, once"
    );
}

/// Only a kill asks for the sweep. A server that exits because its last pane
/// closed was not told by anyone that the session's detached panes are done,
/// and must leave them for `pty-host list` to find.
#[test]
fn a_server_exiting_on_its_last_pane_does_not_sweep() {
    let mut zellij = start_zellij();
    let first_pane = claim_first_terminal_and_wait_for_prompt(&zellij);

    first_pane.exit(Some(0));
    zellij.wait_for_exit();

    assert!(
        zellij.orphan_sweeps().is_empty(),
        "a last-pane exit swept orphans: {:?}",
        zellij.orphan_sweeps()
    );
}

#[test]
fn a_kill_session_after_rename_sweeps_the_new_name() {
    let mut zellij = start_zellij();
    let _pane = claim_first_terminal_and_wait_for_prompt(&zellij);
    let new_name = format!("{}-renamed", zellij.session_name());
    let (code, output) =
        zellij.run_actions_reporting(vec![zellij_types::input::actions::Action::RenameSession {
            name: new_name.clone().into(),
        }]);
    assert_eq!(code, 0, "{output}");
    assert!(!output.contains("Failed"), "{output}");
    zellij.quit();
    assert_eq!(zellij.orphan_sweeps(), vec![new_name]);
}

#[test]
fn acknowledged_kill_replies_after_the_orphan_sweep() {
    let mut zellij = start_zellij();
    let _pane = claim_first_terminal_and_wait_for_prompt(&zellij);
    let name = zellij.session_name().to_owned();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            zellij_ipc::ipc::async_kill_session_with_completion(
                &zellij_runtime::consts::ZELLIJ_SOCK_DIR.join(&name),
            ),
        )
        .await
        .unwrap()
        .unwrap();
    });
    assert_eq!(zellij.orphan_sweeps(), vec![name]);
    zellij.wait_for_exit();
}

#[test]
fn acknowledged_kill_reports_orphan_cleanup_failure() {
    let mut zellij = start_zellij();
    let _pane = claim_first_terminal_and_wait_for_prompt(&zellij);
    zellij.set_orphan_sweep_error("silent supervisor".to_owned());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let error = runtime.block_on(async {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            zellij_ipc::ipc::async_kill_session_with_completion(
                &zellij_runtime::consts::ZELLIJ_SOCK_DIR.join(zellij.session_name()),
            ),
        )
        .await
        .unwrap()
        .unwrap_err()
    });
    assert!(error.to_string().contains("silent supervisor"), "{error}");
    zellij.wait_for_exit();
}
