#![cfg(unix)]

use yinz_integration_tests::{TestRunner, TERMINAL_SIZE};

#[test]
fn tab_groups_keyboard_mouse_and_last_pane_lifecycle() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("mouse_mode true")
        .start();
    let default = yinz.expect_pty_spawn();
    default.output(b"DEFAULT_CONTENT");
    yinz.wait_until("default group and tab ready", |grid| {
        grid.contains("Default") && grid.contains("DEFAULT_CONTENT") && grid.tab_bar_appears()
    });
    yinz.send_stdin(b"\x14g");
    let first = yinz.expect_pty_spawn();
    first.output(b"GROUP_FIRST");
    yinz.wait_until("new group focused", |grid| {
        grid.contains("Group 1")
            && grid.contains("GROUP_FIRST")
            && !grid.contains("DEFAULT_CONTENT")
    });
    yinz.send_stdin(b"\x14n");
    let second = yinz.expect_pty_spawn();
    second.output(b"GROUP_SECOND");
    yinz.wait_until("new tab in group ready", |grid| {
        grid.contains("GROUP_SECOND")
    });
    yinz.send_stdin(b"\x14H\x1b");
    yinz.wait_until("previous group focused", |grid| {
        grid.contains("DEFAULT_CONTENT") && !grid.contains("GROUP_SECOND")
    });
    yinz.send_stdin(b"\x14L\x1b");
    yinz.wait_until("group restores last focused tab", |grid| {
        grid.contains("GROUP_SECOND") && !grid.contains("DEFAULT_CONTENT")
    });
    yinz.send_stdin(b"\x14x");
    yinz.wait_until("closing one tab retains group", |grid| {
        grid.contains("GROUP_FIRST") && grid.contains("Group 1")
    });
    let project_row = yinz.snapshot().lines().remove(0);
    let default_column = project_row[..project_row.find("Default").unwrap()]
        .chars()
        .count()
        + 1;
    yinz.send_stdin(format!("\x1b[<0;{default_column};1M\x1b[<0;{default_column};1m").as_bytes());
    yinz.wait_until("click selects default group", |grid| {
        grid.contains("DEFAULT_CONTENT")
    });
    yinz.send_stdin(b"\x14L\x1b");
    yinz.wait_until("last group pane focused", |grid| {
        grid.contains("GROUP_FIRST")
    });
    first.exit(Some(0));
    yinz.wait_until("last pane exit removes group", |grid| {
        grid.contains("DEFAULT_CONTENT") && !grid.contains("Group 1")
    });
    yinz.quit();
}
