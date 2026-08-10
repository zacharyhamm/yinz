#![cfg(unix)]

use zellij_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, start_zellij, TestRunner, TestSession, TERMINAL_SIZE,
};
use zellij_utils::cli::CliAction;

const TOAST_TOP_LEFT_CORNER: &str = "╭";

fn notify(zellij: &TestSession, text: &str) {
    let exit_code = zellij.run_cli_action(CliAction::Notify {
        text: text.to_owned(),
    });
    assert_eq!(exit_code, 0, "notify exited cleanly");
}

/// The row each toast's top border sits on, top-to-bottom.
fn toast_border_rows(zellij: &TestSession, what: &str, expected: usize) -> Vec<usize> {
    let snapshot = zellij.wait_until(what, |grid_snapshot| {
        grid_snapshot
            .lines()
            .iter()
            .filter(|line| line.contains(TOAST_TOP_LEFT_CORNER))
            .count()
            == expected
    });
    snapshot
        .lines()
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains(TOAST_TOP_LEFT_CORNER))
        .map(|(row, _)| row)
        .collect()
}

#[test]
fn notification_appears_top_right_and_dismisses_itself() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config("notification_position \"top-right\"\nnotification_timeout 1")
        .start();
    claim_first_terminal_and_wait_for_prompt(&zellij);

    notify(&zellij, "build finished");

    let snapshot = zellij.wait_until("notification is drawn", |grid_snapshot| {
        grid_snapshot.contains("build finished")
    });
    let toast_row = snapshot
        .lines()
        .iter()
        .position(|line| line.contains("build finished"))
        .expect("notification text is on screen");
    let toast_line = &snapshot.lines()[toast_row];
    let text_column = toast_line.find("build finished").unwrap();
    assert!(
        text_column > TERMINAL_SIZE.cols / 2,
        "top-right notification should sit in the right half, was at column {} of {:?}",
        text_column,
        toast_line
    );

    zellij.wait_until("notification dismisses itself", |grid_snapshot| {
        !grid_snapshot.contains("build finished")
            && !grid_snapshot.contains(TOAST_TOP_LEFT_CORNER)
            // whatever the toast covered has been repainted
            && grid_snapshot.status_bar_appears()
    });

    zellij.quit();
}

#[test]
fn notifications_stack_downwards_from_a_top_corner() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config("notification_position \"top-left\"\nnotification_timeout 30")
        .start();
    claim_first_terminal_and_wait_for_prompt(&zellij);

    notify(&zellij, "first");
    let first = toast_border_rows(&zellij, "one notification is drawn", 1);
    notify(&zellij, "second");
    let both = toast_border_rows(&zellij, "two notifications are drawn", 2);

    assert_eq!(
        both[0], first[0],
        "the newest toast takes the corner, pushing the older one away from it"
    );
    assert!(
        both[1] > both[0],
        "a top-anchored stack grows downwards, got rows {:?}",
        both
    );

    let snapshot = zellij.wait_until("both messages visible", |grid_snapshot| {
        grid_snapshot.contains("first") && grid_snapshot.contains("second")
    });
    let newest_row = snapshot
        .lines()
        .iter()
        .position(|line| line.contains("second"))
        .unwrap();
    let oldest_row = snapshot
        .lines()
        .iter()
        .position(|line| line.contains("first"))
        .unwrap();
    assert!(
        newest_row < oldest_row,
        "the newest notification sits nearest the corner"
    );

    zellij.quit();
}

#[test]
fn notifications_stack_upwards_from_a_bottom_corner() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config("notification_position \"bottom-right\"\nnotification_timeout 30")
        .start();
    claim_first_terminal_and_wait_for_prompt(&zellij);

    notify(&zellij, "first");
    let first = toast_border_rows(&zellij, "one notification is drawn", 1);
    notify(&zellij, "second");
    let both = toast_border_rows(&zellij, "two notifications are drawn", 2);

    // rows come back top-to-bottom, so the corner-most toast is the *last* one here
    assert_eq!(
        both[1], first[0],
        "the newest toast takes the corner, pushing the older one away from it"
    );
    assert!(
        both[0] < both[1],
        "a bottom-anchored stack grows upwards, got rows {:?}",
        both
    );

    zellij.quit();
}

#[test]
fn notification_paints_over_a_floating_pane() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config("notification_position \"top-right\"\nnotification_timeout 30")
        .start();
    claim_first_terminal_and_wait_for_prompt(&zellij);

    // a floating pane covers most of the viewport, including the top-right corner
    let exit_code = zellij.run_cli_action(CliAction::NewPane {
        direction: None,
        command: vec![],
        plugin: None,
        cwd: None,
        floating: true,
        in_place: false,
        stacked: false,
        name: None,
        close_on_exit: false,
        start_suspended: false,
        configuration: None,
        skip_plugin_cache: false,
        x: Some("0".to_owned()),
        y: Some("0".to_owned()),
        width: Some("100%".to_owned()),
        height: Some("100%".to_owned()),
        pinned: None,
        no_focus: false,
        tab_id: None,
        close_replaced_pane: false,
        pane_id: None,
        blocking: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        block_until_exit: false,
        unblock_condition: None,
        near_current_pane: false,
        borderless: None,
        global: false,
        edge: None,
        size: None,
    });
    assert_eq!(exit_code, 0, "new floating pane exited cleanly");
    let floating = zellij.expect_pty_spawn();
    floating.output(b"$ ");

    notify(&zellij, "over the float");
    zellij.wait_until(
        "notification is not clipped behind the floating pane",
        |grid_snapshot| grid_snapshot.contains("over the float"),
    );

    zellij.quit();
}

#[test]
fn notification_survives_a_tab_switch_without_leaving_residue() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config("notification_position \"top-right\"\nnotification_timeout 30")
        .start();
    claim_first_terminal_and_wait_for_prompt(&zellij);

    let exit_code = zellij.run_cli_action(CliAction::NewTab {
        name: None,
        layout: None,
        layout_string: None,
        layout_dir: None,
        cwd: None,
        initial_command: vec![],
        initial_plugin: None,
        close_on_exit: false,
        start_suspended: false,
        block_until_exit_success: false,
        block_until_exit_failure: false,
        block_until_exit: false,
        no_focus: false,
    });
    assert_eq!(exit_code, 0, "new-tab exited cleanly");
    let second_tab_terminal = zellij.expect_pty_spawn();
    second_tab_terminal.output(b"$ ");
    zellij.wait_until("second tab is up", |grid_snapshot| {
        grid_snapshot.contains("Tab #2")
    });

    notify(&zellij, "still here");
    zellij.wait_until("notification drawn on the second tab", |grid_snapshot| {
        grid_snapshot.contains("still here")
    });

    let exit_code = zellij.run_cli_action(CliAction::GoToPreviousTab);
    assert_eq!(exit_code, 0, "go-to-previous-tab exited cleanly");
    zellij.wait_until(
        "notification follows the client to the first tab",
        |grid_snapshot| grid_snapshot.contains("Tab #1") && grid_snapshot.contains("still here"),
    );

    zellij.quit();
}

#[test]
fn notification_through_a_keybinding() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config(
            r#"
notification_timeout 30
keybinds {
    normal {
        bind "Ctrl y" { Notify "from a keybinding"; }
    }
}
"#,
        )
        .start();
    claim_first_terminal_and_wait_for_prompt(&zellij);

    zellij.send_stdin(&[0x19]); // Ctrl-y
    zellij.wait_until("notification drawn from the keybinding", |grid_snapshot| {
        grid_snapshot.contains("from a keybinding")
    });

    zellij.quit();
}

#[test]
fn long_notification_text_is_truncated_rather_than_overflowing() {
    let mut zellij = start_zellij();
    claim_first_terminal_and_wait_for_prompt(&zellij);

    notify(
        &zellij,
        "this notification message is far longer than any toast box has room to show in full",
    );

    let snapshot = zellij.wait_until("truncated notification is drawn", |grid_snapshot| {
        grid_snapshot.contains("…")
    });
    assert!(
        !snapshot.contains("room to show in full"),
        "the tail should have been truncated away, got {}",
        snapshot
    );
    for line in snapshot.lines() {
        assert!(
            line.chars().count() <= TERMINAL_SIZE.cols,
            "notification must not overflow the viewport width: {:?}",
            line
        );
    }

    zellij.quit();
}
