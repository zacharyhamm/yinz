#![cfg(unix)]

use yinz_integration_tests::{claim_first_terminal_and_wait_for_prompt, keys, start_yinz};

#[test]
fn management_shortcuts_open_and_close_the_command_palette() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    for shortcut in ['w', 'l', 'p'] {
        yinz.send_stdin(&keys::ctrl('o'));
        yinz.send_stdin(&keys::key(shortcut));
        yinz.wait_until("command palette opened", |grid_snapshot| {
            grid_snapshot.contains("Do · Project:") && grid_snapshot.status_bar_appears()
        });
        yinz.send_stdin(&keys::ESC);
        yinz.wait_until("command palette closed", |grid_snapshot| {
            !grid_snapshot.contains("Do · Project:") && grid_snapshot.status_bar_appears()
        });
    }
    yinz.quit();
}
