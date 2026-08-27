#![cfg(unix)]

use zellij_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, keys, start_zellij, TestRunner, TestSession,
    TERMINAL_SIZE,
};
use zellij_utils::cli::CliAction;

const TOAST_TOP_LEFT_CORNER: &str = "╭";
const TOAST_BOTTOM_LEFT_CORNER: &str = "╰";

fn notify(zellij: &TestSession, text: &str) {
    let exit_code = zellij.run_cli_action(CliAction::Notify {
        text: text.to_owned(),
    });
    assert_eq!(exit_code, 0, "notify exited cleanly");
}

/// Wait for `expected` toasts, then report the row each one's top border sits on, top-to-bottom.
fn toast_border_rows(zellij: &TestSession, what: &str, expected: usize) -> Vec<usize> {
    wait_for_toasts(zellij, what, expected).0
}

const TOAST_TOP_RIGHT_CORNER: &str = "\u{256e}";
const TOAST_BOTTOM_RIGHT_CORNER: &str = "\u{256f}";

/// The `(top border rows, bottom border rows)` of the toasts on screen once there are `expected` of
/// them and they have stopped moving. Both are top-to-bottom, so a toast's own height is the
/// difference between the two at the same index - which is worth asking now that toasts are not all
/// the same height.
///
/// "Stopped moving" is asserted on the *content* rather than waited out on a timer. A mid-animation
/// stack is observably different from a settled one in two ways, both of which fall straight out of
/// how the animation works:
///
/// - **Not fully revealed.** The slide is a column clip that pins the edge nearest the anchored
///   corner, so a partly-revealed right-anchored toast has no `\u{256d}` and a left-anchored one has
///   no `\u{256e}`. Counting all four corners catches either anchor.
/// - **Not contiguous.** The stack has a gap or an overlap for exactly as long as a survivor is
///   gliding into its new slot; closing that gap *is* the animation.
///
/// This deliberately replaces a `sleep` tied to the animation's own constants. A stability
/// predicate ("the layout stopped changing") would be the wrong tool - it can read as settled on a
/// spurious wake against an unchanged snapshot - but neither of the properties above can be true
/// mid-animation, so there is nothing to race.
fn wait_for_toasts(zellij: &TestSession, what: &str, expected: usize) -> (Vec<usize>, Vec<usize>) {
    let rows_with =
        |grid_snapshot: &zellij_integration_tests::GridSnapshot, marker: &str| -> Vec<usize> {
            grid_snapshot
                .lines()
                .iter()
                .enumerate()
                .filter(|(_, line)| line.contains(marker))
                .map(|(row, _)| row)
                .collect()
        };
    let snapshot = zellij.wait_until(what, |grid_snapshot| {
        let tops = rows_with(grid_snapshot, TOAST_TOP_LEFT_CORNER);
        let bottoms = rows_with(grid_snapshot, TOAST_BOTTOM_LEFT_CORNER);
        tops.len() == expected
            && bottoms.len() == expected
            && rows_with(grid_snapshot, TOAST_TOP_RIGHT_CORNER).len() == expected
            && rows_with(grid_snapshot, TOAST_BOTTOM_RIGHT_CORNER).len() == expected
            && stack_is_contiguous(&tops, &bottoms)
    });
    (
        rows_with(&snapshot, TOAST_TOP_LEFT_CORNER),
        rows_with(&snapshot, TOAST_BOTTOM_LEFT_CORNER),
    )
}

/// Whether `grid_snapshot` shows a toast that has finished sliding in.
///
/// The slide is a column clip that pins the edge nearest the anchored corner, so a right-anchored
/// toast draws its text before it draws its left border. A test that waits only for the text and
/// then looks for `\u{256d}` is racing the reveal - it has to wait for the far edge too.
fn a_toast_is_fully_revealed(grid_snapshot: &zellij_integration_tests::GridSnapshot) -> bool {
    [
        TOAST_TOP_LEFT_CORNER,
        TOAST_TOP_RIGHT_CORNER,
        TOAST_BOTTOM_LEFT_CORNER,
        TOAST_BOTTOM_RIGHT_CORNER,
    ]
    .iter()
    .all(|corner| grid_snapshot.contains(corner))
}

/// Whether the stack has neither a gap nor an overlap: every toast's bottom border is the row
/// immediately above the next one's top border. A stack mid-glide has one or the other.
fn stack_is_contiguous(tops: &[usize], bottoms: &[usize]) -> bool {
    tops.windows(2).zip(bottoms).all(|(pair, bottom)| {
        let [_, next_top] = pair else {
            return true;
        };
        *bottom + 1 == *next_top
    })
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
        remote_host: None,
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
        overlay: None,
        modal: None,
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
fn notification_browser_keeps_dismissed_session_history() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config("notification_timeout 1")
        .start();
    claim_first_terminal_and_wait_for_prompt(&zellij);

    notify(&zellij, "history first");
    let newest = format!("{} detail tail", "newest ".repeat(20));
    notify(&zellij, &newest);
    zellij.wait_until("notifications dismiss", |grid_snapshot| {
        !grid_snapshot.contains("history first") && !grid_snapshot.contains("detail tail")
    });

    zellij.send_stdin(&keys::ctrl('o'));
    zellij.send_stdin(&keys::key('w'));
    zellij.wait_until("session manager is ready", |grid_snapshot| {
        grid_snapshot.contains("Session Manager") && grid_snapshot.contains("Session: _")
    });
    zellij.send_stdin(&keys::ctrl('v'));
    let list = zellij.wait_until("notification history opens", |grid_snapshot| {
        grid_snapshot.contains("Received") && grid_snapshot.contains("history first")
    });
    let newest_row = list
        .lines()
        .iter()
        .position(|line| line.contains("newest"))
        .expect("newest notification is listed");
    let oldest_row = list
        .lines()
        .iter()
        .position(|line| line.contains("history first"))
        .expect("oldest notification is listed");
    assert!(
        newest_row < oldest_row,
        "newest notification should be first"
    );
    assert!(list.contains("…"), "the list truncates long notifications");

    zellij.send_stdin(&keys::key('\n'));
    zellij.wait_until("full notification opens", |grid_snapshot| {
        grid_snapshot.contains("detail tail")
    });

    zellij.quit();
}

/// Text wraps into a taller box now, so it takes a message past the row cap - not merely past one
/// row - to reach the ellipsis.
#[test]
fn notification_text_past_the_row_cap_is_truncated_rather_than_overflowing() {
    let mut zellij = start_zellij();
    claim_first_terminal_and_wait_for_prompt(&zellij);

    let mut message =
        "this notification message is far longer than any toast box has room to show ".repeat(7);
    message.push_str("and this tail is well past the last row");

    notify(&zellij, &message);

    let snapshot = zellij.wait_until("truncated notification is drawn", |grid_snapshot| {
        grid_snapshot.contains("…") && a_toast_is_fully_revealed(grid_snapshot)
    });
    assert!(
        !snapshot.contains("well past the last row"),
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

#[test]
fn multi_line_notification_text_wraps_into_a_taller_box() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config("notification_position \"top-right\"\nnotification_timeout 30")
        .start();
    claim_first_terminal_and_wait_for_prompt(&zellij);

    notify(&zellij, "hocus needs confirmation\n  pane_id: 3");

    let snapshot = zellij.wait_until("both rows of the message are drawn", |grid_snapshot| {
        grid_snapshot.contains("hocus needs confirmation")
            && grid_snapshot.contains("pane_id: 3")
            && a_toast_is_fully_revealed(grid_snapshot)
    });
    let lines = snapshot.lines();
    let top = lines
        .iter()
        .position(|line| line.contains(TOAST_TOP_LEFT_CORNER))
        .expect("a toast is drawn");
    let bottom = lines
        .iter()
        .position(|line| line.contains(TOAST_BOTTOM_LEFT_CORNER))
        .expect("the toast is closed");

    assert_eq!(
        lines
            .iter()
            .filter(|line| line.contains(TOAST_TOP_LEFT_CORNER))
            .count(),
        1,
        "one taller box, not one box per line, got {}",
        snapshot
    );
    assert_eq!(bottom - top, 3, "two text rows between the borders");
    assert!(lines[top + 1].contains("hocus needs confirmation"));
    assert!(
        lines[top + 2].contains("  pane_id: 3"),
        "the indentation survives, got {:?}",
        lines[top + 2]
    );

    zellij.quit();
}

#[test]
fn a_taller_toast_pushes_the_next_one_further_from_the_corner() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config("notification_position \"top-left\"\nnotification_timeout 30")
        .start();
    claim_first_terminal_and_wait_for_prompt(&zellij);

    notify(&zellij, "short");
    let first = toast_border_rows(&zellij, "one notification is drawn", 1);
    notify(&zellij, "one\ntwo\nthree");
    let both = toast_border_rows(&zellij, "two notifications are drawn", 2);

    assert_eq!(both[0], first[0], "the newest toast takes the corner");
    assert_eq!(
        both[1] - both[0],
        5,
        "the older toast starts below all five rows of the tall one, got {:?}",
        both
    );

    zellij.quit();
}

/// The case the stacking maths is easiest to get wrong: growing upwards, a toast is placed by its
/// bottom edge, so a taller one must not hang off the end of the viewport.
#[test]
fn a_tall_toast_at_a_bottom_corner_still_ends_on_the_last_viewport_row() {
    let mut zellij = TestRunner::new(TERMINAL_SIZE)
        .with_config("notification_position \"bottom-right\"\nnotification_timeout 30")
        .start();
    claim_first_terminal_and_wait_for_prompt(&zellij);

    notify(&zellij, "short");
    let (_, short_bottoms) = wait_for_toasts(&zellij, "one notification is drawn", 1);

    notify(&zellij, "one\ntwo\nthree");
    let (tops, bottoms) = wait_for_toasts(&zellij, "two notifications are drawn", 2);

    let newest_bottom = *bottoms
        .last()
        .expect("the newest toast is nearest the corner");
    assert_eq!(
        newest_bottom, short_bottoms[0],
        "the tall toast ends on the row the short one ended on, got {:?}",
        bottoms
    );
    assert_eq!(
        newest_bottom - tops[1],
        4,
        "and it is five rows tall, got tops {:?} bottoms {:?}",
        tops,
        bottoms
    );

    zellij.quit();
}
