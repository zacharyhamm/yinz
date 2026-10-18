#![cfg(unix)]

use insta::assert_snapshot;
use yinz_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, col, keys, normalized, start_yinz,
    tab_bar_shows_no_lone_pane_id, FakePtyHandle, GridSnapshot, Size, TestSession, PROMPT,
    TERMINAL_SIZE,
};

fn tabs_in_order(grid_snapshot: &GridSnapshot, labels: &[&str]) -> bool {
    let mut search_from = 0;
    for label in labels {
        match grid_snapshot.text[search_from..].find(label) {
            Some(offset) => search_from += offset + label.len(),
            None => return false,
        }
    }
    true
}

fn open_new_tab_and_wait_for_prompt(yinz: &TestSession, expected_tab: &str) -> FakePtyHandle {
    yinz.send_stdin(&keys::ctrl('t'));
    yinz.send_stdin(&keys::key('n'));
    let terminal = yinz.expect_pty_spawn();
    terminal.output(PROMPT);
    let expected_tab = expected_tab.to_owned();
    yinz.wait_until("new tab opened with prompt", move |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && grid_snapshot.contains(&expected_tab)
            && grid_snapshot.cursor_is_at(col(2).row(2))
    });
    terminal
}

fn label_first_tab_pane(yinz: &TestSession, terminal: &FakePtyHandle, marker: &str) {
    terminal.output(marker.as_bytes());
    let marker = marker.to_owned();
    yinz.wait_until("first tab pane labelled", move |grid_snapshot| {
        grid_snapshot.contains(&marker)
    });
}

fn open_marked_tab(yinz: &TestSession, expected_tab: &str, marker: &str) -> FakePtyHandle {
    yinz.send_stdin(&keys::ctrl('t'));
    yinz.send_stdin(&keys::key('n'));
    let terminal = yinz.expect_pty_spawn();
    terminal.output(PROMPT);
    terminal.output(marker.as_bytes());
    let expected_tab = expected_tab.to_owned();
    let marker = marker.to_owned();
    yinz.wait_until("new marked tab opened", move |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && grid_snapshot.contains(&expected_tab)
            && grid_snapshot.contains(&marker)
    });
    terminal
}

#[test]
fn go_to_previous_tab() {
    let mut yinz = start_yinz();
    let first_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    label_first_tab_pane(&yinz, &first_terminal, "oneone");
    open_marked_tab(&yinz, "Tab #2", "twotwo");

    yinz.send_stdin(&keys::ctrl('t'));
    yinz.send_stdin(&keys::key('h'));
    yinz.send_stdin(&keys::ENTER);

    let grid_snapshot = yinz.wait_until("previous tab focused", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && grid_snapshot.contains("[1] Tab #2")
            && grid_snapshot.contains("oneone")
            && !grid_snapshot.contains("twotwo")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn go_to_next_tab() {
    let mut yinz = start_yinz();
    let first_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    label_first_tab_pane(&yinz, &first_terminal, "oneone");
    open_marked_tab(&yinz, "Tab #2", "twotwo");

    yinz.send_stdin(&keys::ctrl('t'));
    yinz.send_stdin(&keys::key('h'));
    yinz.wait_until("previous tab focused", |grid_snapshot| {
        grid_snapshot.contains("oneone") && !grid_snapshot.contains("twotwo")
    });

    yinz.send_stdin(&keys::key('l'));
    yinz.send_stdin(&keys::ENTER);

    let grid_snapshot = yinz.wait_until("next tab focused again", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && grid_snapshot.contains("[1] Tab #2")
            && grid_snapshot.contains("twotwo")
            && !grid_snapshot.contains("oneone")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn go_to_tab_by_number() {
    let mut yinz = start_yinz();
    let first_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    label_first_tab_pane(&yinz, &first_terminal, "oneone");
    open_marked_tab(&yinz, "Tab #2", "twotwo");

    yinz.send_stdin(&keys::ctrl('t'));
    yinz.send_stdin(&keys::key('1'));

    let grid_snapshot = yinz.wait_until("first tab focused by number", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && grid_snapshot.contains("[1] Tab #2")
            && grid_snapshot.contains("oneone")
            && !grid_snapshot.contains("twotwo")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn toggle_tab_returns_to_last_used() {
    let mut yinz = start_yinz();
    let first_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    label_first_tab_pane(&yinz, &first_terminal, "oneone");
    open_marked_tab(&yinz, "Tab #2", "twotwo");

    yinz.send_stdin(&keys::ctrl('t'));
    yinz.send_stdin(&keys::TAB);
    yinz.send_stdin(&keys::ENTER);

    let grid_snapshot = yinz.wait_until("toggled back to the first tab", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && grid_snapshot.contains("[1] Tab #2")
            && grid_snapshot.contains("oneone")
            && !grid_snapshot.contains("twotwo")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn toggle_active_sync_tab() {
    let mut yinz = start_yinz();
    let first_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('n'));
    let second_terminal = yinz.expect_pty_spawn();
    second_terminal.output(PROMPT);
    yinz.wait_until("second pane spawned in first tab", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && grid_snapshot.contains("Pane #2")
    });

    yinz.send_stdin(&keys::ctrl('t'));
    yinz.send_stdin(&keys::key('s'));

    let grid_snapshot = yinz.wait_until("tab marked as syncing", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && grid_snapshot.contains("SYNC")
    });
    assert_snapshot!(normalized(&grid_snapshot));

    yinz.send_stdin(&keys::ENTER);
    yinz.send_stdin(b"synced-input");

    let first_stdin = first_terminal.wait_for_stdin("synced input reached first pane", |stdin| {
        stdin.windows(12).any(|window| window == b"synced-input")
    });
    let second_stdin = second_terminal
        .wait_for_stdin("synced input reached second pane", |stdin| {
            stdin.windows(12).any(|window| window == b"synced-input")
        });
    assert!(first_stdin
        .windows(12)
        .any(|window| window == b"synced-input"));
    assert!(second_stdin
        .windows(12)
        .any(|window| window == b"synced-input"));
    yinz.quit();
}

#[test]
fn break_pane_into_new_tab() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('n'));
    let second_terminal = yinz.expect_pty_spawn();
    second_terminal.output(PROMPT);
    yinz.wait_until("second pane spawned in first tab", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && grid_snapshot.contains("Pane #2")
    });

    yinz.send_stdin(&keys::ctrl('t'));
    yinz.send_stdin(&keys::key('b'));

    let grid_snapshot = yinz.wait_until("focused pane broken into a new tab", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && grid_snapshot.contains("Tab #2")
            && !grid_snapshot.contains("Pane #1")
            && grid_snapshot.cursor_is_at(col(2).row(2))
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn break_floating_pane_into_new_tab_resizes_its_pty_exactly_once() {
    // a pane that is moved into a new tab used to pass through intermediate sizes before that tab
    // settled, each of which reached its pty as a SIGWINCH - programs that coalesce SIGWINCHes
    // arriving in quick succession (vim among them) would then act on an intermediate size and
    // remain rendered in the wrong size until their next redraw
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('w'));
    let floating_terminal = yinz.expect_pty_spawn();
    floating_terminal.output(PROMPT);
    yinz.wait_until("floating pane spawned", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && grid_snapshot.contains("Pane #2")
    });
    let resizes_before_break = floating_terminal.size_history().len();

    yinz.send_stdin(&keys::ctrl('t'));
    yinz.send_stdin(&keys::key('b'));
    yinz.wait_until("pane broken into a new tab", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && grid_snapshot.contains("Tab #2")
    });

    // the pane now takes up the whole tab, with only the tab bar and status bar around it
    let expected_size = (TERMINAL_SIZE.cols as u16, TERMINAL_SIZE.rows as u16 - 3);
    floating_terminal.wait_for_size("pane resized to its new tab", move |cols, rows| {
        (cols, rows) == expected_size
    });
    // give any (unwanted) additional resize a chance to arrive before we assert
    std::thread::sleep(std::time::Duration::from_millis(300));
    let resizes_while_breaking = floating_terminal
        .size_history()
        .split_off(resizes_before_break);
    assert_eq!(
        resizes_while_breaking,
        vec![expected_size],
        "the pane's pty should have been resized exactly once, to the size of its new tab"
    );
    yinz.quit();
}

#[test]
fn break_pane_to_the_right() {
    let mut yinz = start_yinz();
    let first_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    label_first_tab_pane(&yinz, &first_terminal, "alpha");

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('n'));
    let beta_terminal = yinz.expect_pty_spawn();
    beta_terminal.output(PROMPT);
    beta_terminal.output(b"beta");
    yinz.wait_until("second pane spawned in first tab", |grid_snapshot| {
        grid_snapshot.contains("beta") && grid_snapshot.contains("Pane #2")
    });

    open_marked_tab(&yinz, "Tab #2", "gamma");

    yinz.send_stdin(&keys::ctrl('t'));
    yinz.send_stdin(&keys::key('h'));
    yinz.wait_until("back on the first tab with its panes", |grid_snapshot| {
        grid_snapshot.contains("beta") && !grid_snapshot.contains("gamma")
    });

    yinz.send_stdin(&keys::key(']'));

    let grid_snapshot = yinz.wait_until("beta pane moved into the right tab", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && grid_snapshot.contains("beta")
            && grid_snapshot.contains("gamma")
            && !grid_snapshot.contains("alpha")
            && grid_snapshot.contains("[0] Tab #1")
            && !grid_snapshot.contains("[2] Tab #2")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn break_pane_to_the_left() {
    let mut yinz = start_yinz();
    let first_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    label_first_tab_pane(&yinz, &first_terminal, "alpha");

    open_marked_tab(&yinz, "Tab #2", "beta");

    yinz.send_stdin(&keys::ctrl('t'));
    yinz.send_stdin(&keys::key('['));

    let grid_snapshot = yinz.wait_until("beta pane moved into the left tab", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && grid_snapshot.contains("alpha")
            && grid_snapshot.contains("beta")
            && !grid_snapshot.contains("Tab #2")
            && tab_bar_shows_no_lone_pane_id(grid_snapshot)
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn open_new_tab() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    open_new_tab_and_wait_for_prompt(&yinz, "Tab #2");
    let grid_snapshot = yinz.wait_until("second tab steady in normal mode", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && grid_snapshot.contains("Tab #2")
            && grid_snapshot.cursor_is_at(col(2).row(2))
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn close_tab() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    open_new_tab_and_wait_for_prompt(&yinz, "Tab #2");

    yinz.send_stdin(&keys::ctrl('t'));
    yinz.send_stdin(&keys::key('x'));

    let grid_snapshot = yinz.wait_until(
        "second tab closed, only first tab remains",
        |grid_snapshot| {
            grid_snapshot.status_bar_appears()
                && grid_snapshot.contains("Tab #1")
                && !grid_snapshot.contains("Tab #2")
                && grid_snapshot.cursor_is_at(col(2).row(2))
        },
    );
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn closing_a_tab_resizes_the_tab_it_returns_to() {
    let mut yinz = start_yinz();
    let first_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    label_first_tab_pane(&yinz, &first_terminal, "oneone");
    open_marked_tab(&yinz, "Tab #2", "twotwo");

    let larger_size = Size {
        cols: TERMINAL_SIZE.cols + 20,
        rows: TERMINAL_SIZE.rows + 6,
    };
    yinz.resize(larger_size);
    yinz.wait_until(
        "second tab re-rendered at the larger size",
        move |snapshot| snapshot.contains("twotwo") && snapshot.row_count() == larger_size.rows,
    );

    yinz.send_stdin(&keys::ctrl('t'));
    yinz.send_stdin(&keys::key('x'));

    let expected_size = (larger_size.cols as u16, larger_size.rows as u16 - 3);
    first_terminal.wait_for_size(
        "first tab resized to the enlarged window once it is focused again",
        move |cols, rows| (cols, rows) == expected_size,
    );
    let grid_snapshot = yinz.wait_until(
        "first tab repainted over the closed tab's ui",
        |grid_snapshot| {
            grid_snapshot.status_bar_appears()
                && grid_snapshot.contains("Tab #1")
                && !grid_snapshot.contains("Tab #2")
                && grid_snapshot.contains("oneone")
                && !grid_snapshot.contains("twotwo")
        },
    );
    assert_eq!(
        grid_snapshot.row_count(),
        larger_size.rows,
        "the restored tab must cover the whole enlarged display:\n{}",
        grid_snapshot
    );
    yinz.quit();
}

#[test]
fn undo_rename_tab() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('t'));
    yinz.send_stdin(&keys::key('r'));
    yinz.send_stdin(b"aa");

    yinz.wait_until("typed name applied in rename mode", |grid_snapshot| {
        grid_snapshot.contains("aa") && grid_snapshot.contains("RENAMING TAB")
    });

    yinz.send_stdin(&keys::ESC);
    yinz.wait_until("rename undone, back in tab mode", |grid_snapshot| {
        grid_snapshot.contains("Tab #1") && !grid_snapshot.contains("aa")
    });

    yinz.send_stdin(&keys::ESC);
    let grid_snapshot = yinz.wait_until("tab name reverted to default", |grid_snapshot| {
        grid_snapshot.contains("Tab #1")
            && !grid_snapshot.contains("aa")
            && grid_snapshot.contains("LOCK")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn move_tab_to_left() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    open_new_tab_and_wait_for_prompt(&yinz, "Tab #2");
    open_new_tab_and_wait_for_prompt(&yinz, "Tab #3");

    yinz.send_stdin(&keys::alt('i'));

    // See `move_tab_to_right`: the same race, and it only passes today by luck of scheduling.
    let grid_snapshot = yinz.wait_until("third tab moved one position left", |grid_snapshot| {
        tabs_in_order(grid_snapshot, &["Tab #1", "Tab #3", "Tab #2"])
            && grid_snapshot.contains("LOCK")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn move_tab_to_right() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    open_new_tab_and_wait_for_prompt(&yinz, "Tab #2");
    open_new_tab_and_wait_for_prompt(&yinz, "Tab #3");

    yinz.send_stdin(&keys::alt('h'));
    yinz.send_stdin(&keys::alt('o'));

    // Wait for the landed tab's status bar to settle before snapshotting the moved order.
    // `LOCK` avoids racing the TAB-mode help shown while opening the tabs.
    let grid_snapshot = yinz.wait_until("second tab moved one position right", |grid_snapshot| {
        tabs_in_order(grid_snapshot, &["Tab #1", "Tab #3", "Tab #2"])
            && grid_snapshot.status_bar_appears()
            && grid_snapshot.contains("LOCK")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn move_tab_to_left_until_it_wraps_around() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    open_new_tab_and_wait_for_prompt(&yinz, "Tab #2");
    open_new_tab_and_wait_for_prompt(&yinz, "Tab #3");

    yinz.send_stdin(&keys::alt('i'));
    yinz.wait_until("third tab moved one position left", |grid_snapshot| {
        tabs_in_order(grid_snapshot, &["Tab #1", "Tab #3", "Tab #2"])
    });
    yinz.send_stdin(&keys::alt('i'));
    yinz.wait_until("third tab moved to the beginning", |grid_snapshot| {
        tabs_in_order(grid_snapshot, &["Tab #3", "Tab #1", "Tab #2"])
    });
    yinz.send_stdin(&keys::alt('i'));

    let grid_snapshot = yinz.wait_until("third tab wrapped to the end", |grid_snapshot| {
        tabs_in_order(grid_snapshot, &["Tab #1", "Tab #2", "Tab #3"])
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn move_tab_to_right_until_it_wraps_around() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    open_new_tab_and_wait_for_prompt(&yinz, "Tab #2");
    open_new_tab_and_wait_for_prompt(&yinz, "Tab #3");

    yinz.send_stdin(&keys::alt('o'));

    let grid_snapshot = yinz.wait_until("third tab wrapped to the beginning", |grid_snapshot| {
        tabs_in_order(grid_snapshot, &["Tab #3", "Tab #1", "Tab #2"])
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}
