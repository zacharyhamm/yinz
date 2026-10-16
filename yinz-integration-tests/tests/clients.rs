#![cfg(unix)]

use insta::assert_snapshot;
use yinz_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, col, keys, normalized, start_yinz,
    tab_bar_shows_no_lone_pane_id, GridSnapshot, TestRunner, PROMPT, TERMINAL_SIZE,
};

fn session_name_rendered(grid_snapshot: &GridSnapshot) -> bool {
    grid_snapshot.contains("(test-")
}

#[test]
fn mirrored_sessions() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("mirror_session true")
        .start();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    let second_client = yinz.attach_client(TERMINAL_SIZE);
    second_client.wait_until("second client loaded on the first tab", |grid_snapshot| {
        grid_snapshot.tab_bar_appears()
            && grid_snapshot.status_bar_appears()
            && grid_snapshot.cursor_is_at(col(2).row(1))
    });

    second_client.send_stdin(&keys::ctrl('p'));
    second_client.send_stdin(&keys::key('r'));
    let right_terminal = yinz.expect_pty_spawn();
    right_terminal.output(PROMPT);

    let mirror_focused_right_pane = |grid_snapshot: &GridSnapshot| {
        grid_snapshot.tab_bar_appears()
            && tab_bar_shows_no_lone_pane_id(grid_snapshot)
            && session_name_rendered(grid_snapshot)
            && grid_snapshot.status_bar_appears()
            && grid_snapshot.contains("Pane #2")
            && grid_snapshot.cursor_is_at(col(62).row(2))
    };
    let main_grid = yinz.wait_until(
        "main client follows the second client's focus into the split it never asked for",
        mirror_focused_right_pane,
    );
    let second_grid = second_client.wait_until(
        "second client shows the split it created",
        mirror_focused_right_pane,
    );
    assert_eq!(
        normalized(&main_grid),
        normalized(&second_grid),
        "mirrored clients must render an identical view"
    );
    assert_snapshot!(normalized(&main_grid));
    second_client.quit();
    yinz.quit();
}

#[test]
fn multiple_users_in_same_pane_and_tab() {
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config("pane_frame_style \"full\"")
        .start();
    let terminal = yinz.expect_pty_spawn();
    terminal.output(PROMPT);
    yinz.wait_until("first terminal prompt rendered", |grid_snapshot| {
        grid_snapshot.tab_bar_appears()
            && grid_snapshot.status_bar_appears()
            && grid_snapshot.contains("$ ")
    });

    let second_client = yinz.attach_client(TERMINAL_SIZE);
    let second_grid =
        second_client.wait_until("second client shares the focused pane", |grid_snapshot| {
            grid_snapshot.tab_bar_appears()
                && grid_snapshot.contains("Tab #1 [ ]")
                && session_name_rendered(grid_snapshot)
                && grid_snapshot.status_bar_appears()
                && grid_snapshot.contains("MY FOCUS")
                && grid_snapshot.cursor_is_at(col(3).row(2))
        });
    let main_grid = yinz.wait_until(
        "main client shows the shared-focus indicator",
        |grid_snapshot| {
            grid_snapshot.tab_bar_appears()
                && grid_snapshot.contains("Tab #1 [ ]")
                && session_name_rendered(grid_snapshot)
                && grid_snapshot.status_bar_appears()
                && grid_snapshot.contains("MY FOCUS")
                && grid_snapshot.cursor_is_at(col(3).row(2))
        },
    );
    assert_snapshot!(normalized(&main_grid));
    assert_snapshot!(normalized(&second_grid));
    second_client.quit();
    yinz.quit();
}

#[test]
fn multiple_users_in_different_panes_and_same_tab() {
    let mut yinz = start_yinz();
    let left_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);

    let second_client = yinz.attach_client(TERMINAL_SIZE);
    second_client.wait_until("second client loaded on the first tab", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && grid_snapshot.cursor_is_at(col(2).row(1))
    });

    second_client.send_stdin(&keys::ctrl('p'));
    second_client.send_stdin(&keys::key('r'));
    let right_terminal = yinz.expect_pty_spawn();
    right_terminal.output(PROMPT);

    let second_grid = second_client.wait_until(
        "second client focused the new right pane",
        |grid_snapshot| {
            grid_snapshot.tab_bar_appears()
                && tab_bar_shows_no_lone_pane_id(grid_snapshot)
                && grid_snapshot.contains("Tab #1 [ ]")
                && session_name_rendered(grid_snapshot)
                && grid_snapshot.status_bar_appears()
                && grid_snapshot.cursor_is_at(col(62).row(2))
        },
    );
    let main_grid = yinz.wait_until(
        "main client sees the second client's split while staying on the left pane",
        |grid_snapshot| {
            grid_snapshot.tab_bar_appears()
                && tab_bar_shows_no_lone_pane_id(grid_snapshot)
                && grid_snapshot.contains("Tab #1 [ ]")
                && session_name_rendered(grid_snapshot)
                && grid_snapshot.contains("Pane #2")
                && grid_snapshot.status_bar_appears()
                && grid_snapshot.cursor_is_at(col(2).row(2))
        },
    );
    assert_snapshot!(normalized(&main_grid));
    assert_snapshot!(normalized(&second_grid));
    yinz.send_stdin(b"left-client;");
    left_terminal.wait_for_stdin("main client's input reached the left pane", |bytes| {
        bytes.ends_with(b";")
    });
    second_client.send_stdin(b"right-client;");
    right_terminal.wait_for_stdin("second client's input reached the right pane", |bytes| {
        bytes.ends_with(b";")
    });
    assert_eq!(left_terminal.stdin_bytes(), b"left-client;");
    assert_eq!(right_terminal.stdin_bytes(), b"right-client;");
    second_client.quit();
    yinz.quit();
}

#[test]
fn multiple_users_in_different_tabs() {
    let mut yinz = start_yinz();
    let first_tab_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);

    let second_client = yinz.attach_client(TERMINAL_SIZE);
    second_client.wait_until("second client loaded on the first tab", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && grid_snapshot.cursor_is_at(col(2).row(1))
    });

    second_client.send_stdin(&keys::ctrl('t'));
    second_client.send_stdin(&keys::key('n'));
    let second_tab_terminal = yinz.expect_pty_spawn();
    second_tab_terminal.output(PROMPT);

    let second_grid =
        second_client.wait_until("second client moved to the new tab", |grid_snapshot| {
            grid_snapshot.tab_bar_appears()
                && session_name_rendered(grid_snapshot)
                && grid_snapshot.status_bar_appears()
                && grid_snapshot.contains("[1] Tab #2")
                && grid_snapshot.cursor_is_at(col(2).row(1))
        });
    let main_grid = yinz.wait_until(
        "main client sees the new tab while staying on the first tab",
        |grid_snapshot| {
            grid_snapshot.tab_bar_appears()
                && session_name_rendered(grid_snapshot)
                && grid_snapshot.status_bar_appears()
                // The second client's `[1]` marker arrives in a later tab-bar frame than the
                // tab itself; the snapshot below asserts it, so wait for it.
                && grid_snapshot.contains("[1] Tab #2")
                && grid_snapshot.cursor_is_at(col(2).row(1))
        },
    );
    assert_snapshot!(normalized(&main_grid));
    assert_snapshot!(normalized(&second_grid));
    yinz.send_stdin(b"first-tab-client;");
    first_tab_terminal.wait_for_stdin("main client's input reached the first tab", |bytes| {
        bytes.ends_with(b";")
    });
    second_client.send_stdin(b"second-tab-client;");
    second_tab_terminal.wait_for_stdin("second client's input reached the second tab", |bytes| {
        bytes.ends_with(b";")
    });
    assert_eq!(first_tab_terminal.stdin_bytes(), b"first-tab-client;");
    assert_eq!(second_tab_terminal.stdin_bytes(), b"second-tab-client;");
    second_client.quit();
    yinz.quit();
}

#[test]
fn detach_and_attach_session() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('r'));
    let right_terminal = yinz.expect_pty_spawn();
    right_terminal.output(PROMPT);
    yinz.wait_until("right terminal prompt rendered", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && grid_snapshot.cursor_is_at(col(62).row(2))
            && tab_bar_shows_no_lone_pane_id(grid_snapshot)
    });

    right_terminal.output(b"I am some text");
    yinz.wait_until("text rendered in the right terminal", |grid_snapshot| {
        grid_snapshot.contains("I am some text")
    });

    yinz.detach_main_client();

    // Unspecified focus depends on whether attachment reuses the departed client's id.
    let reattached_client = yinz.attach_client_with_focus(
        TERMINAL_SIZE,
        Some(yinz_types::pane_id::PaneId::Terminal(
            right_terminal.terminal_id(),
        )),
    );
    let grid_snapshot = reattached_client.wait_until(
        "reattached client sees the restored split and text",
        |grid_snapshot| {
            grid_snapshot.tab_bar_appears()
                && session_name_rendered(grid_snapshot)
                && grid_snapshot.status_bar_appears()
                && grid_snapshot.contains("Pane #2")
                && grid_snapshot.contains("I am some text")
                && grid_snapshot.cursor_is_at(col(76).row(2))
        },
    );
    assert_snapshot!(normalized(&grid_snapshot));
    reattached_client.quit();
    yinz.quit();
}
