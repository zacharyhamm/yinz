#![cfg(unix)]

use yinz_integration_tests::{claim_first_terminal_and_wait_for_prompt, start_yinz};

/// The server releases the session's Firecracker VMs exactly once, when it exits.
///
/// Whatever the exit reason, the pty thread has been joined and every pane closed
/// by the time the server asks for it, so a VM the session held is not left running
/// — only a server that never gets to exit on its own can do that. Nothing needs
/// waiting for: `quit` sends the kill-session IPC the harness tears down with and
/// joins the server thread, so the count after it is the count the server left.
#[test]
fn a_session_exit_stops_its_firecracker_vms_once() {
    let mut yinz = start_yinz();
    let _first_pane = claim_first_terminal_and_wait_for_prompt(&yinz);
    assert_eq!(
        yinz.firecracker_session_stops(),
        0,
        "the session's VMs were released while the session was still running"
    );

    yinz.quit();

    assert_eq!(
        yinz.firecracker_session_stops(),
        1,
        "the server exit should release the session's Firecracker VMs exactly once"
    );
    // And only after every pane was closed: a VM stopped under a live pane takes the
    // pane's process with it, which is the order `lib.rs` exists to keep.
    assert_eq!(
        yinz.firecracker_stops_with_live_panes(),
        0,
        "the session's VMs were released while a pane was still open"
    );
}
