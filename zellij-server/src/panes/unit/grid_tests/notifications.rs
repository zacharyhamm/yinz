//! Desktop notifications parsed out of a pane's output: OSC 9, OSC 777 and kitty's OSC 99,
//! whose chunks the grid assembles per notification id.

use super::*;
use crate::panes::grid::PendingNotification;

fn osc99_display(grid: &Grid, index: usize) -> Option<(String, String)> {
    match grid.pending_desktop_notifications.get(index) {
        Some(PendingNotification::Osc99 { display, .. }) => display.clone(),
        other => panic!("expected an OSC 99 notification, got: {:?}", other),
    }
}

fn wants_report_flags(grid: &Grid) -> Vec<bool> {
    grid.pending_desktop_notifications
        .iter()
        .map(|notification| match notification {
            PendingNotification::Osc99 { wants_report, .. } => *wants_report,
            other => panic!("expected an OSC 99 notification, got: {:?}", other),
        })
        .collect()
}

#[test]
fn osc_9_notification_is_parsed() {
    let mut grid = new_grid_for_forwarding_test();
    feed_bytes(&mut grid, b"\x1b]9;the build finished\x07");
    assert_eq!(
        grid.pending_desktop_notifications,
        vec![PendingNotification::Osc9 {
            body: "the build finished".to_owned()
        }]
    );
}

#[test]
fn osc_9_notification_with_semicolons_keeps_the_whole_body() {
    let mut grid = new_grid_for_forwarding_test();
    feed_bytes(&mut grid, b"\x1b]9;done: 1;2;3\x07");
    assert_eq!(
        grid.pending_desktop_notifications,
        vec![PendingNotification::Osc9 {
            body: "done: 1;2;3".to_owned()
        }]
    );
}

#[test]
fn an_empty_osc_9_notification_is_ignored() {
    let mut grid = new_grid_for_forwarding_test();
    feed_bytes(&mut grid, b"\x1b]9;\x07");
    assert!(grid.pending_desktop_notifications.is_empty());
}

#[test]
fn an_osc_9_conemu_progress_report_is_not_a_notification() {
    let mut grid = new_grid_for_forwarding_test();
    feed_bytes(&mut grid, b"\x1b]9;4;0;0\x1b\\");
    feed_bytes(&mut grid, b"\x1b]9;4;1;50\x07");
    assert!(
        grid.pending_desktop_notifications.is_empty(),
        "progress reports are not desktop notifications, got: {:?}",
        grid.pending_desktop_notifications
    );
}

#[test]
fn other_osc_9_conemu_subcommands_are_not_notifications() {
    let mut grid = new_grid_for_forwarding_test();
    feed_bytes(&mut grid, b"\x1b]9;9;/home/user\x07");
    feed_bytes(&mut grid, b"\x1b]9;3;a tab title\x07");
    assert!(
        grid.pending_desktop_notifications.is_empty(),
        "ConEmu subcommands are not desktop notifications, got: {:?}",
        grid.pending_desktop_notifications
    );
}

#[test]
fn an_osc_9_notification_starting_with_a_number_is_still_a_notification() {
    let mut grid = new_grid_for_forwarding_test();
    feed_bytes(&mut grid, b"\x1b]9;4 tests failed;in 2 crates\x07");
    assert_eq!(
        grid.pending_desktop_notifications,
        vec![PendingNotification::Osc9 {
            body: "4 tests failed;in 2 crates".to_owned()
        }]
    );
}

#[test]
fn osc_777_notification_is_parsed() {
    let mut grid = new_grid_for_forwarding_test();
    feed_bytes(&mut grid, b"\x1b]777;notify;the title;the body\x07");
    assert_eq!(
        grid.pending_desktop_notifications,
        vec![PendingNotification::Osc777 {
            title: "the title".to_owned(),
            body: "the body".to_owned()
        }]
    );
}

#[test]
fn an_osc_777_notification_body_may_contain_semicolons() {
    let mut grid = new_grid_for_forwarding_test();
    feed_bytes(&mut grid, b"\x1b]777;notify;title;a;b;c\x07");
    assert_eq!(
        grid.pending_desktop_notifications,
        vec![PendingNotification::Osc777 {
            title: "title".to_owned(),
            body: "a;b;c".to_owned()
        }]
    );
}

#[test]
fn an_osc_777_non_notify_subcommand_is_ignored() {
    let mut grid = new_grid_for_forwarding_test();
    feed_bytes(&mut grid, b"\x1b]777;precmd;something\x07");
    assert!(grid.pending_desktop_notifications.is_empty());
}

#[test]
fn an_osc_777_notification_without_a_body_is_kept() {
    let mut grid = new_grid_for_forwarding_test();
    feed_bytes(&mut grid, b"\x1b]777;notify;just a title\x07");
    assert_eq!(
        grid.pending_desktop_notifications,
        vec![PendingNotification::Osc777 {
            title: "just a title".to_owned(),
            body: String::new()
        }]
    );
}

#[test]
fn a_notification_carries_its_title_and_body_across_protocols() {
    assert_eq!(
        PendingNotification::Osc9 {
            body: "body".to_owned()
        }
        .title_and_body(),
        (String::new(), "body".to_owned())
    );
    assert_eq!(
        PendingNotification::Osc777 {
            title: "title".to_owned(),
            body: "body".to_owned()
        }
        .title_and_body(),
        ("title".to_owned(), "body".to_owned())
    );
    assert_eq!(
        PendingNotification::Osc99 {
            payload: "i=1;the title".to_owned(),
            terminator: "\u{7}".to_owned(),
            wants_report: false,
            display: Some(("the title".to_owned(), "the body".to_owned())),
        }
        .title_and_body(),
        ("the title".to_owned(), "the body".to_owned())
    );
}

#[test]
fn an_osc_99_notification_is_assembled_from_its_chunks() {
    let mut grid = new_grid_for_forwarding_test();
    feed_bytes(&mut grid, b"\x1b]99;i=1:d=0;Hello world\x1b\\");
    feed_bytes(&mut grid, b"\x1b]99;i=1:p=body;This is cool\x1b\\");
    assert_eq!(
        osc99_display(&grid, 0),
        None,
        "an unfinished notification has nothing to show yet"
    );
    assert_eq!(
        osc99_display(&grid, 1),
        Some(("Hello world".to_owned(), "This is cool".to_owned())),
        "the finished notification carries all of its chunks"
    );
}

#[test]
fn an_osc_99_notification_payload_may_be_base64_encoded() {
    let mut grid = new_grid_for_forwarding_test();
    feed_bytes(&mut grid, b"\x1b]99;i=1:e=1;dGhlIGJ1aWxkIGZpbmlzaGVk\x1b\\");
    assert_eq!(
        osc99_display(&grid, 0),
        Some(("the build finished".to_owned(), String::new()))
    );
}

#[test]
fn osc_99_requests_that_display_nothing_are_not_downgraded() {
    let mut grid = new_grid_for_forwarding_test();
    feed_bytes(&mut grid, b"\x1b]99;i=1:p=close;\x1b\\");
    feed_bytes(&mut grid, b"\x1b]99;i=2:p=?;\x1b\\");
    feed_bytes(&mut grid, b"\x1b]99;i=3:p=alive;\x1b\\");
    assert_eq!(osc99_display(&grid, 0), None, "a close shows nothing");
    assert_eq!(osc99_display(&grid, 1), None, "a query shows nothing");
    assert_eq!(
        osc99_display(&grid, 2),
        None,
        "a liveness poll shows nothing"
    );
}

#[test]
fn the_report_intent_of_an_osc_99_notification_is_remembered() {
    let mut grid = new_grid_for_forwarding_test();
    feed_bytes(&mut grid, b"\x1b]99;i=1:a=report;the build finished\x1b\\");
    feed_bytes(&mut grid, b"\x1b]99;i=1:p=close;\x1b\\");
    assert_eq!(
        wants_report_flags(&grid),
        vec![true, true],
        "a close request inherits the intent of the notification it closes"
    );
}

#[test]
fn an_explicit_action_replaces_the_remembered_report_intent() {
    let mut grid = new_grid_for_forwarding_test();
    feed_bytes(&mut grid, b"\x1b]99;i=1:a=report;the build started\x1b\\");
    feed_bytes(&mut grid, b"\x1b]99;i=1:a=focus;the build finished\x1b\\");
    feed_bytes(&mut grid, b"\x1b]99;i=1:p=close;\x1b\\");
    assert_eq!(
        wants_report_flags(&grid),
        vec![true, false, false],
        "an escape that states its own action decides, and is remembered from then on"
    );
}

#[test]
fn an_osc_99_notification_is_finished_by_a_payload_it_cannot_show() {
    let mut grid = new_grid_for_forwarding_test();
    feed_bytes(&mut grid, b"\x1b]99;i=1:d=0;the build finished\x1b\\");
    feed_bytes(&mut grid, b"\x1b]99;i=1:p=buttons:d=1;retry\x1b\\");
    assert_eq!(
        osc99_display(&grid, 1),
        Some(("the build finished".to_owned(), String::new())),
        "the assembled text is shown even when the last chunk carries buttons"
    );
}

#[test]
fn the_notification_state_remembered_per_pane_is_bounded() {
    let mut grid = new_grid_for_forwarding_test();
    for index in 0..1000 {
        feed_bytes(
            &mut grid,
            format!("\x1b]99;i={}:a=report;notification\x1b\\", index).as_bytes(),
        );
    }
    assert!(
        grid.notification_tracker.known_ids.len() <= 256
            && grid.notification_tracker.wants_report.len() <= 256
            && grid.notification_tracker.assemblies.is_empty(),
        "an app sending endless notifications does not grow the pane's state without bound"
    );
}

#[test]
fn a_single_notification_being_assembled_is_bounded() {
    let mut grid = new_grid_for_forwarding_test();
    let chunk = "\u{5efa}".repeat(300);
    for _ in 0..100 {
        feed_bytes(
            &mut grid,
            format!("\x1b]99;i=1:d=0;{}\x1b\\", chunk).as_bytes(),
        );
    }

    let assembled_bytes = grid
        .notification_tracker
        .assemblies
        .get("1")
        .map(|assembly| assembly.title.len())
        .unwrap_or(0);
    assert!(
        assembled_bytes <= 4096,
        "an app streaming an endless single notification does not grow the pane's state without \
         bound, got {} bytes",
        assembled_bytes
    );

    feed_bytes(&mut grid, b"\x1b]99;i=1:d=1;\x1b\\");
    let last_notification = grid.pending_desktop_notifications.len() - 1;
    let (title, _body) =
        osc99_display(&grid, last_notification).expect("the truncated notification is still shown");
    assert_eq!(
        title,
        "\u{5efa}".repeat(1365),
        "the assembled text is truncated on a character boundary"
    );
}
