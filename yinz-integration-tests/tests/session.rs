#![cfg(unix)]

use yinz_integration_tests::{claim_first_terminal_and_wait_for_prompt, keys, start_yinz};

#[test]
fn launch_session_manager_plugin() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('o'));
    yinz.send_stdin(&keys::key('w'));

    yinz.wait_until("session manager plugin opened", |grid_snapshot| {
        grid_snapshot.contains("Session Manager") && grid_snapshot.status_bar_appears()
    });
    yinz.quit();
}
