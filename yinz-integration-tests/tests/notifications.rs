#![cfg(unix)]

use yinz_cli::args::CliAction;
use yinz_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, col, keys, start_yinz, TestRunner, TestSession,
    TERMINAL_SIZE,
};

const TOAST_TOP_LEFT_CORNER: &str = "╭";
const TOAST_BOTTOM_LEFT_CORNER: &str = "╰";

fn notify(yinz: &TestSession, text: &str) {
    let exit_code = yinz.run_cli_action(CliAction::Notify {
        text: text.to_owned(),
    });
    assert_eq!(exit_code, 0, "notify exited cleanly");
}

/// Wait for `expected` toasts, then report the row each one's top border sits on, top-to-bottom.
fn toast_border_rows(yinz: &TestSession, what: &str, expected: usize) -> Vec<usize> {
    wait_for_toasts(yinz, what, expected).0
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
fn wait_for_toasts(yinz: &TestSession, what: &str, expected: usize) -> (Vec<usize>, Vec<usize>) {
    let rows_with =
        |grid_snapshot: &yinz_integration_tests::GridSnapshot, marker: &str| -> Vec<usize> {
            grid_snapshot
                .lines()
                .iter()
                .enumerate()
                .filter(|(_, line)| line.contains(marker))
                .map(|(row, _)| row)
                .collect()
        };
    let snapshot = yinz.wait_until(what, |grid_snapshot| {
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
fn a_toast_is_fully_revealed(grid_snapshot: &yinz_integration_tests::GridSnapshot) -> bool {
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
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("notification_position \"top-right\"\nnotification_timeout 1")
        .start();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    notify(&yinz, "build finished");

    let snapshot = yinz.wait_until("notification is drawn", |grid_snapshot| {
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

    yinz.wait_until("notification dismisses itself", |grid_snapshot| {
        !grid_snapshot.contains("build finished")
            && !grid_snapshot.contains(TOAST_TOP_LEFT_CORNER)
            // whatever the toast covered has been repainted
            && grid_snapshot.status_bar_appears()
    });

    yinz.quit();
}

#[test]
fn alt_a_dismisses_newest_first_and_shares_consumption_between_clients() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("notification_timeout 30")
        .start();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    notify(&yinz, "older pending");
    notify(&yinz, "newer pending");
    wait_for_toasts(&yinz, "both pending toasts are settled", 2);
    let other = yinz.attach_client(TERMINAL_SIZE);
    other.wait_until("second client sees the activation stack", |grid| {
        grid.contains("older pending") && grid.contains("newer pending")
    });

    yinz.send_stdin(&keys::alt('a'));
    other.wait_until(
        "hint transfers to the older toast for both clients",
        |grid| {
            grid.contains("older pending")
                && grid.contains("Alt+a to dismiss")
                && !grid.contains("newer pending")
        },
    );
    other.send_stdin(&keys::alt('a'));
    yinz.wait_until("second client consumes the remaining toast", |grid| {
        !grid.contains("older pending")
            && !grid.contains("newer pending")
            && !grid.contains("Alt+a to dismiss")
    });
    yinz.send_stdin(&keys::ctrl('o'));
    yinz.send_stdin(&keys::key('n'));
    yinz.wait_until("consumed notifications remain in history", |grid| {
        grid.contains("older pending") && grid.contains("newer pending")
    });
    other.quit();
    yinz.quit();
}

#[test]
fn notifications_stack_downwards_from_a_top_corner() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("notification_position \"top-left\"\nnotification_timeout 30")
        .start();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    notify(&yinz, "first");
    let first = toast_border_rows(&yinz, "one notification is drawn", 1);
    notify(&yinz, "second");
    let both = toast_border_rows(&yinz, "two notifications are drawn", 2);

    assert_eq!(
        both[0], first[0],
        "the newest toast takes the corner, pushing the older one away from it"
    );
    assert!(
        both[1] > both[0],
        "a top-anchored stack grows downwards, got rows {:?}",
        both
    );

    let snapshot = yinz.wait_until("both messages visible", |grid_snapshot| {
        grid_snapshot.contains("first") && grid_snapshot.contains("second")
    });
    assert_eq!(
        snapshot
            .lines()
            .iter()
            .filter(|line| line.contains("Alt+a to dismiss"))
            .count(),
        1
    );
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

    yinz.quit();
}

#[test]
fn notifications_stack_upwards_from_a_bottom_corner() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("notification_position \"bottom-right\"\nnotification_timeout 30")
        .start();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    notify(&yinz, "first");
    let first = toast_border_rows(&yinz, "one notification is drawn", 1);
    notify(&yinz, "second");
    let both = toast_border_rows(&yinz, "two notifications are drawn", 2);

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

    yinz.quit();
}

#[test]
fn notification_paints_over_a_floating_pane() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("notification_position \"top-right\"\nnotification_timeout 30")
        .start();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    // a floating pane covers most of the viewport, including the top-right corner
    let exit_code = yinz.run_cli_action(CliAction::NewPane {
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
        configuration_json: None,
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
    let floating = yinz.expect_pty_spawn();
    floating.output(b"$ ");

    notify(&yinz, "over the float");
    yinz.wait_until(
        "notification is not clipped behind the floating pane",
        |grid_snapshot| grid_snapshot.contains("over the float"),
    );

    yinz.quit();
}

#[test]
fn notification_survives_a_tab_switch_without_leaving_residue() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("notification_position \"top-right\"\nnotification_timeout 30")
        .start();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    let exit_code = yinz.run_cli_action(CliAction::NewTab {
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
    let second_tab_terminal = yinz.expect_pty_spawn();
    second_tab_terminal.output(b"$ ");
    yinz.wait_until("second tab is up", |grid_snapshot| {
        grid_snapshot.contains("Tab #2")
    });

    notify(&yinz, "still here");
    yinz.wait_until("notification drawn on the second tab", |grid_snapshot| {
        grid_snapshot.contains("still here")
    });

    let exit_code = yinz.run_cli_action(CliAction::GoToPreviousTab);
    assert_eq!(exit_code, 0, "go-to-previous-tab exited cleanly");
    yinz.wait_until(
        "notification follows the client to the first tab",
        |grid_snapshot| grid_snapshot.contains("Tab #1") && grid_snapshot.contains("still here"),
    );

    yinz.quit();
}

#[test]
fn notification_through_a_keybinding() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
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
    claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&[0x19]); // Ctrl-y
    yinz.wait_until("notification drawn from the keybinding", |grid_snapshot| {
        grid_snapshot.contains("from a keybinding")
    });

    yinz.quit();
}

#[test]
fn notification_browser_keeps_dismissed_session_history() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("notification_timeout 1")
        .start();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    notify(&yinz, "history first");
    let newest = format!("{} detail tail", "newest ".repeat(20));
    notify(&yinz, &newest);
    yinz.wait_until("newest notification appears", |grid_snapshot| {
        grid_snapshot.contains("newest")
    });
    yinz.wait_until("notifications dismiss", |grid_snapshot| {
        !grid_snapshot.contains("history first") && !grid_snapshot.contains("newest")
    });

    yinz.send_stdin(b"\x1b[112;7u");
    yinz.wait_until("command palette opens", |grid_snapshot| {
        grid_snapshot.contains("Do · Project:")
    });
    yinz.send_stdin("Notifications…".as_bytes());
    yinz.wait_until("notification action is ready", |grid_snapshot| {
        grid_snapshot.contains("› Notifications…")
    });
    yinz.send_stdin(&keys::ENTER);
    let list = yinz.wait_until("notification history opens", |grid_snapshot| {
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

    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("full notification opens", |grid_snapshot| {
        grid_snapshot.contains("detail tail")
    });

    yinz.quit();
}

#[test]
fn session_palette_notifications_refresh_on_each_visit() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("notification_timeout 1")
        .start();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    for message in ["first visit", "second visit"] {
        notify(&yinz, message);
        yinz.wait_until("toast appears", |snapshot| snapshot.contains(message));
        yinz.wait_until("toast dismisses", |snapshot| !snapshot.contains(message));
        yinz.send_stdin(b"\x1b[112;7u");
        yinz.wait_until("command palette opens", |snapshot| {
            snapshot.contains("Do · Project:") && snapshot.contains("│> █")
        });
        yinz.send_stdin("Notifications…".as_bytes());
        yinz.wait_until("notification action is ready", |snapshot| {
            snapshot.contains("› Notifications…")
        });
        yinz.send_stdin(&keys::ENTER);
        yinz.wait_until("notifications refresh on entry", |snapshot| {
            snapshot.contains("Received") && snapshot.contains(message)
        });
        yinz.send_stdin(b"\x1b[112;7u");
        yinz.wait_until("command palette closes", |snapshot| {
            !snapshot.contains(message) && snapshot.status_bar_appears()
        });
    }
    yinz.quit();
}

/// Notifications mode lists the session's notifications where the toasts render, and keeps them
/// actionable after the toasts have timed out.
#[test]
fn notifications_mode_browses_dismissed_toasts_in_place() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("notification_position \"top-right\"\nnotification_timeout 1")
        .start();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    notify(&yinz, "browse older");
    let newest = format!("{} detail tail", "newest ".repeat(20));
    notify(&yinz, &newest);
    yinz.wait_until("the newest toast appears", |grid_snapshot| {
        grid_snapshot.contains("newest")
    });
    yinz.wait_until("the toasts dismiss", |grid_snapshot| {
        !grid_snapshot.contains("browse older") && !grid_snapshot.contains("newest")
    });

    yinz.send_stdin(&keys::ctrl('o'));
    yinz.send_stdin(&keys::key('n'));
    let list = yinz.wait_until("the browser lists both notifications", |grid_snapshot| {
        grid_snapshot.contains("browse older") && grid_snapshot.contains("newest")
    });
    let row_of = |text: &str| {
        list.lines()
            .iter()
            .position(|line| line.contains(text))
            .unwrap_or_else(|| panic!("{text:?} is listed"))
    };
    assert!(
        row_of("newest") < row_of("browse older"),
        "the newest notification sits nearest the top corner"
    );
    let text_column = list.lines()[row_of("browse older")]
        .find("browse older")
        .unwrap();
    assert!(
        text_column > TERMINAL_SIZE.cols / 2,
        "the list renders where the top-right toasts do"
    );
    assert!(
        !list.contains("detail tail"),
        "a long notification is elided in the list"
    );

    yinz.send_stdin(&keys::SPACE);
    yinz.wait_until(
        "the selected notification shows its full text",
        |grid_snapshot| grid_snapshot.contains("detail tail"),
    );

    yinz.send_stdin(&keys::key('j'));
    yinz.wait_until("moving collapses the entry it left", |grid_snapshot| {
        !grid_snapshot.contains("detail tail") && grid_snapshot.contains("browse older")
    });

    yinz.send_stdin(&keys::ENTER);
    yinz.wait_until("activating leaves the mode", |grid_snapshot| {
        !grid_snapshot.contains("browse older")
            && !grid_snapshot.contains("newest")
            && grid_snapshot.status_bar_appears()
    });

    yinz.send_stdin(&keys::ctrl('o'));
    yinz.send_stdin(&keys::key('n'));
    yinz.wait_until(
        "the browser reopens with activated history",
        |grid_snapshot| grid_snapshot.contains("newest") && grid_snapshot.contains("browse older"),
    );

    yinz.send_stdin(&keys::alt('a'));
    yinz.wait_until(
        "Alt+A leaves browsing with no pending entries",
        |grid_snapshot| {
            !grid_snapshot.contains("newest")
                && !grid_snapshot.contains("browse older")
                && grid_snapshot.status_bar_appears()
        },
    );
    yinz.send_stdin(&keys::ctrl('o'));
    yinz.send_stdin(&keys::key('n'));
    yinz.wait_until("Alt+A preserves notification history", |grid_snapshot| {
        grid_snapshot.contains("newest") && grid_snapshot.contains("browse older")
    });

    yinz.send_stdin(&keys::ESC);
    yinz.wait_until("Esc leaves the mode", |grid_snapshot| {
        !grid_snapshot.contains("newest")
            && !grid_snapshot.contains("browse older")
            && grid_snapshot.status_bar_appears()
    });

    yinz.quit();
}

/// Text wraps into a taller box now, so it takes a message past the row cap - not merely past one
/// row - to reach the ellipsis.
#[test]
fn notification_text_past_the_row_cap_is_truncated_rather_than_overflowing() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    let mut message =
        "this notification message is far longer than any toast box has room to show ".repeat(7);
    message.push_str("and this tail is well past the last row");

    notify(&yinz, &message);

    let snapshot = yinz.wait_until("truncated notification is drawn", |grid_snapshot| {
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

    yinz.quit();
}

#[test]
fn multi_line_notification_text_wraps_into_a_taller_box() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("notification_position \"top-right\"\nnotification_timeout 30")
        .start();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    notify(&yinz, "agent needs confirmation\n  pane_id: 3");

    let snapshot = yinz.wait_until("both rows of the message are drawn", |grid_snapshot| {
        grid_snapshot.contains("agent needs confirmation")
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
    assert_eq!(bottom - top, 4, "two message rows and the dismissal footer");
    assert!(lines[top + 3].contains("Alt+a to dismiss"));
    assert!(lines[top + 1].contains("agent needs confirmation"));
    assert!(
        lines[top + 2].contains("  pane_id: 3"),
        "the indentation survives, got {:?}",
        lines[top + 2]
    );

    yinz.quit();
}

#[test]
fn a_taller_toast_pushes_the_next_one_further_from_the_corner() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("notification_position \"top-left\"\nnotification_timeout 30")
        .start();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    notify(&yinz, "short");
    let first = toast_border_rows(&yinz, "one notification is drawn", 1);
    notify(&yinz, "one\ntwo\nthree");
    let both = toast_border_rows(&yinz, "two notifications are drawn", 2);

    assert_eq!(both[0], first[0], "the newest toast takes the corner");
    assert_eq!(
        both[1] - both[0],
        6,
        "the older toast starts below the message and footer, got {:?}",
        both
    );

    yinz.quit();
}

/// The case the stacking maths is easiest to get wrong: growing upwards, a toast is placed by its
/// bottom edge, so a taller one must not hang off the end of the viewport.
#[test]
fn a_tall_toast_at_a_bottom_corner_still_ends_on_the_last_viewport_row() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("notification_position \"bottom-right\"\nnotification_timeout 30")
        .start();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    notify(&yinz, "short");
    let (_, short_bottoms) = wait_for_toasts(&yinz, "one notification is drawn", 1);

    notify(&yinz, "one\ntwo\nthree");
    let (tops, bottoms) = wait_for_toasts(&yinz, "two notifications are drawn", 2);

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
        5,
        "the three message rows and footer fit inside the borders, got tops {:?} bottoms {:?}",
        tops,
        bottoms
    );

    yinz.quit();
}

/// A toast repaints itself on every frame of its slide, and on those frames nothing underneath it
/// is dirty - so the frame's only content is the toast's own chunks, each preceded by an absolute
/// goto. The cursor used to be left at the end of the last one, blinking in the bottom-right of the
/// box, because the frame decided it had nothing to re-place and emitted no goto at all.
#[test]
fn a_notification_does_not_steal_the_cursor() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("notification_position \"top-right\"\nnotification_timeout 30")
        .start();
    // asserts the prompt's cursor is at col(2).row(1) to begin with
    claim_first_terminal_and_wait_for_prompt(&yinz);

    notify(&yinz, "build finished");

    // the toast settles and the animation clock stops, so a frame that misplaced the cursor is
    // the last word on where it is - nothing comes along afterwards to correct it
    yinz.wait_until(
        "toast is up and the cursor stayed on the prompt",
        |snapshot| {
            a_toast_is_fully_revealed(snapshot)
                && snapshot.contains("build finished")
                && snapshot.cursor_is_at(col(2).row(1))
        },
    );

    yinz.quit();
}

#[test]
fn a_shared_scheme_notification_policy_and_independent_observer_survive_detach() {
    use yinz_integration_tests::LayoutInfo;
    let dir = tempfile::tempdir().unwrap();
    let policy = dir.path().join("policy.scm");
    let observer = dir.path().join("observer.scm");
    std::fs::write(&policy, r#"
(define count 0)
(define (load configuration)
  (yinz-command (hash "Subscribe" '("NotificationRequested"))))
(define (update event)
  (when (and (hash? event) (hash-contains? event "NotificationRequested"))
    (define notification (hash-ref event "NotificationRequested"))
    (set! count (+ count 1))
    (yinz-command (hash "ResolveNotification"
      (hash "id" (hash-ref notification "id")
        "decision" (if (equal? (hash-ref notification "body") "hide") "Suppress"
          (hash "Replace" (hash "title" "policy"
            "body" (string-append "decision-" (number->string count))
            "delivery" (if (equal? (hash-ref notification "body") "desktop") "Desktop" "Both"))))))))
  #f)
"#).unwrap();
    std::fs::write(&observer, r#"
(define state "starting observer")
(define arrivals (hash))
(define results '())
(define (load configuration)
  (yinz-then (yinz-wait-for-permissions '("ReadApplicationState"))
    (lambda (_)
      (yinz-command (hash "Subscribe" '("NotificationReceived" "NotificationResolved" "ModeUpdate")))
      (set! state "observer ready")
      (yinz-request-render))))
(define (update event)
  (cond
    ((and (hash? event) (hash-contains? event "ModeUpdate")) #t)
    ((and (hash? event) (hash-contains? event "NotificationReceived"))
      (define notification (hash-ref event "NotificationReceived"))
      (set! arrivals (hash-insert arrivals (hash-ref notification "id") notification))
      #f)
    ((and (hash? event) (hash-contains? event "NotificationResolved"))
      (define result (hash-ref event "NotificationResolved"))
      (define original (hash-ref result "original"))
      (unless (and (hash-contains? arrivals (hash-ref original "id"))
                   (equal? (hash-ref (hash-ref arrivals (hash-ref original "id")) "body") (hash-ref original "body"))
                   (equal? (hash-ref (hash-ref arrivals (hash-ref original "id")) "source") (hash-ref original "source")))
        (error "arrival correlation"))
      (unless (equal? (hash-ref result "fallback_reason") json-null)
        (error (string-append "unexpected fallback: " (hash-ref result "fallback_reason"))))
      (set! results (append results (list (string-append
        (hash-ref original "body") ":"
        (if (hash-ref result "suppressed") "suppressed" (hash-ref result "body"))
        ":" (hash-ref result "delivery")))))
      (define history (yinz-command "ListNotifications"))
      (define latest (list-ref history (- (length history) 1)))
      (unless (and (equal? (hash-ref latest "id") (hash-ref original "id"))
                   (equal? (hash-ref (hash-ref latest "outcome") "suppressed") (hash-ref result "suppressed")))
        (error "history metadata"))
      (set! state (string-join results "\n"))
      #t)
    (else #f)))
(define (render rows columns) state)
"#).unwrap();
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config_dir(dir.path().to_owned())
        .with_config(&format!("load_plugins clear-defaults=true {{}}\nhost_notification_protocol \"osc99\"\nhooks {{ notification \"file:{}\"; }}", policy.display()))
        .with_layout(LayoutInfo::Stringified(format!("layout {{ pane {{ plugin location=\"file:{}\" {{ shared true; }}; }}; }}", observer.display())))
        .start();
    yinz.wait_until("observer's read-state permission", |grid| {
        grid.contains("Plugin permissions")
    });
    yinz.send_stdin(b"\t\r");
    yinz.wait_until("observer subscribed", |grid| {
        grid.contains("observer ready")
    });
    notify(&yinz, "first");
    yinz.wait_until(
        "rewritten outcome reached the independent observer",
        |grid| grid.contains("first:decision-1:Both"),
    );
    yinz.wait_until_raw_output("rewritten desktop notification", |raw| {
        String::from_utf8_lossy(raw).contains("i=yinz-notification-0")
    });
    let other = yinz.attach_client(TERMINAL_SIZE);
    other.wait_until("second client shares observer state", |grid| {
        grid.contains("first:decision-1:Both")
    });
    notify(&yinz, "hide");
    other.wait_until("one suppression decision across both clients", |grid| {
        grid.contains("hide:suppressed:Toast")
    });
    notify(&yinz, "desktop");
    yinz.wait_until("desktop-only outcome is recorded", |grid| {
        grid.contains("desktop:decision-3:Desktop")
    });
    other.quit();
    yinz.detach_main_client();
    notify(&yinz, "detached");
    let reattached = yinz.attach_client(TERMINAL_SIZE);
    reattached.wait_until(
        "the retained policy and observer handled the detached arrival",
        |grid| grid.contains("detached:decision-4:Both"),
    );
    assert!(
        !String::from_utf8_lossy(&reattached.raw_bytes()).contains("i=yinz-notification-3"),
        "desktop deliveries are not replayed on attach"
    );
    reattached.quit();
    yinz.quit();
}

#[test]
fn missing_failed_and_unresponsive_scheme_policies_deliver_original_notifications() {
    for source in [
        None,
        Some("(error \"policy failed to load\")"),
        Some("(define (load configuration) (yinz-command (hash \"Subscribe\" '(\"NotificationRequested\"))))"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let policy = directory.path().join("policy.scm");
        if let Some(source) = source {
            std::fs::write(&policy, source).unwrap();
        }
        let mut yinz = TestRunner::new(TERMINAL_SIZE)
            .with_config_dir(directory.path().to_owned())
            .with_config(&format!(
                "load_plugins clear-defaults=true {{}}\nhooks {{ notification \"file:{}\"; }}",
                policy.display()
            ))
            .start();
        claim_first_terminal_and_wait_for_prompt(&yinz);
        notify(&yinz, "original after policy fallback");
        yinz.wait_until(
            "original delivery after failed or missing policy decision",
            |grid| {
                grid.contains("original after policy fallback") && a_toast_is_fully_revealed(grid)
            },
        );
        yinz.quit();
    }
}
