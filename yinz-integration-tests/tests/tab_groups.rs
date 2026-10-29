#![cfg(unix)]

use yinz_integration_tests::{TestRunner, TERMINAL_SIZE};

#[test]
fn tab_groups_keyboard_mouse_and_last_pane_lifecycle() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("mouse_mode true")
        .start();
    let default = yinz.expect_pty_spawn();
    default.output(b"DEFAULT_CONTENT");
    yinz.wait_until("single group collapses to one row", |grid| {
        !grid.lines()[0].contains("Default")
            && grid.lines()[0].contains('+')
            && grid.lines()[2].contains("DEFAULT_CONTENT")
    });
    yinz.send_stdin(b"\x14g");
    let first = yinz.expect_pty_spawn();
    first.output(b"GROUP_FIRST");
    let grid = yinz.wait_until("second group expands the bar and takes focus", |grid| {
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
        grid.lines()[2].contains("DEFAULT_CONTENT")
            && !grid.contains("Group 1")
            && !grid.lines()[0].contains("Default")
    });
    yinz.quit();
}

#[test]
fn tab_groups_can_start_disabled_and_toggle_live_without_losing_groups() {
    use yinz_types::input::actions::Action;
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("tab_groups false")
        .start();
    let first = yinz.expect_pty_spawn();
    first.output(b"FIRST_CONTENT");
    let session_name = yinz.session_name().to_owned();
    yinz.wait_until("single tab row with host/session chrome", |grid| {
        let lines = grid.lines();
        grid.contains("FIRST_CONTENT")
            && lines[0].contains(&session_name)
            && lines[0].contains("Tab #1")
            && !grid.contains("Default")
    });
    yinz.send_stdin(b"\x14n");
    let second = yinz.expect_pty_spawn();
    second.output(b"SECOND_CONTENT");
    yinz.wait_until("two tabs in the single row", |grid| {
        grid.contains("SECOND_CONTENT")
            && grid.lines()[0].contains("Tab #1")
            && grid.lines()[0].contains("Tab #2")
    });
    let configure = |yinz: &yinz_integration_tests::TestSession, enabled| {
        let (code, output) = yinz.run_actions_reporting(vec![Action::Reconfigure {
            config: format!("tab_groups {enabled}"),
            write_to_disk: false,
        }]);
        assert_eq!(code, 0, "{output}");
    };
    configure(&yinz, true);
    yinz.wait_until(
        "single group stays collapsed after enabling groups",
        |grid| {
            !grid.contains("Default")
                && grid.lines()[0].contains("Tab #2")
                && grid.lines()[2].contains("SECOND_CONTENT")
        },
    );
    yinz.send_stdin(b"\x14g");
    let third = yinz.expect_pty_spawn();
    third.output(b"THIRD_CONTENT");
    yinz.wait_until("second group focused", |grid| {
        grid.contains("Group 1") && grid.contains("THIRD_CONTENT")
    });
    configure(&yinz, false);
    yinz.wait_until("all groups flattened into one row", |grid| {
        let row = &grid.lines()[0];
        row.contains("Tab #1")
            && row.contains("Tab #2")
            && row.contains("Tab #3")
            && !grid.contains("Group 1")
    });
    let (code, output) = yinz.run_actions_reporting(vec![Action::NewTabGroup]);
    assert_eq!(code, 0, "{output}");
    yinz.send_stdin(b"\x141");
    yinz.wait_until("numbered shortcut crosses stored groups", |grid| {
        grid.contains("FIRST_CONTENT")
    });
    configure(&yinz, true);
    yinz.wait_until("saved groups return", |grid| {
        grid.lines()[0].contains("Default")
            && grid.lines()[0].contains("Group 1")
            && grid.contains("FIRST_CONTENT")
            && !grid.lines()[1].contains("Tab #3")
    });
    yinz.quit();
}
