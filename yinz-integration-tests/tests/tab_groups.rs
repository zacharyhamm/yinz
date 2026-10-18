#![cfg(unix)]

use yinz_integration_tests::{TestRunner, TERMINAL_SIZE};

#[test]
fn tab_groups_keyboard_mouse_and_last_pane_lifecycle() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("mouse_mode true")
        .start();
    let default = yinz.expect_pty_spawn();
    default.output(b"DEFAULT_CONTENT");
    let grid = yinz.wait_until("default group and buttons ready", |grid| {
        grid.contains("Default")
            && grid.contains("DEFAULT_CONTENT")
            && grid.lines()[0].contains('+')
            && grid.lines()[1].contains('+')
    });
    let group_row = &grid.lines()[0];
    let group_button_column = group_row[..group_row.find('+').unwrap()].chars().count() + 1;
    yinz.send_stdin(
        format!("\x1b[<0;{group_button_column};1M\x1b[<0;{group_button_column};1m").as_bytes(),
    );
    let first = yinz.expect_pty_spawn();
    first.output(b"GROUP_FIRST");
    let grid = yinz.wait_until("clicked new group focused", |grid| {
        grid.contains("Group 1")
            && grid.contains("GROUP_FIRST")
            && !grid.contains("DEFAULT_CONTENT")
            && grid.lines()[1].contains('+')
    });
    let tab_row = &grid.lines()[1];
    let tab_button_column = tab_row[..tab_row.find('+').unwrap()].chars().count() + 1;
    yinz.send_stdin(
        format!("\x1b[<0;{tab_button_column};2M\x1b[<0;{tab_button_column};2m").as_bytes(),
    );
    let second = yinz.expect_pty_spawn();
    second.output(b"GROUP_SECOND");
    yinz.wait_until("new tab in group ready", |grid| {
        grid.contains("GROUP_SECOND") && !grid.contains("Group 2")
    });
    yinz.send_stdin(b"\x1bH");
    yinz.wait_until("previous group focused", |grid| {
        grid.contains("DEFAULT_CONTENT") && !grid.contains("GROUP_SECOND")
    });
    yinz.send_stdin(b"\x1bL");
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
    yinz.send_stdin(b"\x1bL");
    yinz.wait_until("last group pane focused", |grid| {
        grid.contains("GROUP_FIRST")
    });
    first.exit(Some(0));
    yinz.wait_until("last pane exit removes group", |grid| {
        grid.contains("DEFAULT_CONTENT") && !grid.contains("Group 1")
    });
    yinz.quit();
}
