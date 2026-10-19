#![cfg(unix)]

use insta::assert_snapshot;
use yinz_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, col, keys, normalized,
    split_right_and_wait_for_prompt, start_yinz,
};

#[test]
fn write_byte_to_focused_pane_via_tmux_mode() {
    let mut yinz = start_yinz();
    let terminal = claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('b'));
    yinz.send_stdin(&keys::ctrl('b'));

    let stdin_bytes = terminal.wait_for_stdin("ctrl-b byte reached the pane", |stdin_bytes| {
        stdin_bytes.contains(&0x02)
    });
    assert!(stdin_bytes.contains(&0x02));
    let grid_snapshot = yinz.wait_until("mode settled back to normal", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn focus_next_pane_via_tmux_mode() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('b'));
    yinz.send_stdin(&keys::key('o'));
    yinz.send_stdin(&keys::ENTER);

    let grid_snapshot = yinz.wait_until("focus cycled to the other pane", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && grid_snapshot.cursor_is_at(col(2).row(2))
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn toggle_group_marking() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::alt('P'));

    let grid_snapshot = yinz.wait_until("group marking engaged", |grid_snapshot| {
        grid_snapshot.contains("GROUP ACTIONS")
            && grid_snapshot.contains("SELECTED PANE")
            && grid_snapshot.contains("Follow Focus")
            && grid_snapshot.status_bar_appears()
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn toggle_pane_in_group() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::alt('p'));

    let grid_snapshot = yinz.wait_until("pane added to group", |grid_snapshot| {
        grid_snapshot.contains("GROUP ACTIONS")
            && grid_snapshot.contains("SELECTED PANE")
            && grid_snapshot.contains("Follow Focus")
            && grid_snapshot.status_bar_appears()
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn next_swap_layout() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);
    yinz.wait_until("swap layout at base", |grid_snapshot| {
        grid_snapshot.contains("BASE")
    });

    yinz.send_stdin(&keys::alt(']'));

    yinz.wait_until("swap layout advanced to vertical", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && grid_snapshot.contains("VERTICAL")
            && !grid_snapshot.contains("BASE")
    });
    yinz.send_stdin(&keys::alt(']'));
    yinz.wait_until("swap layout advanced to three columns", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && grid_snapshot.contains("THREE-COLUMNS")
            && !grid_snapshot.contains("VERTICAL")
    });
    yinz.send_stdin(&keys::alt(']'));
    yinz.wait_until("swap layout advanced to quad", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && grid_snapshot.contains("QUAD")
            && !grid_snapshot.contains("THREE-COLUMNS")
    });
    yinz.send_stdin(&keys::alt(']'));
    let grid_snapshot = yinz.wait_until("swap layout advanced to horizontal", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && !grid_snapshot.contains("QUAD")
            && !grid_snapshot.contains("VERTICAL")
            && grid_snapshot.contains("HORIZONTAL")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn previous_swap_layout() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);
    yinz.wait_until("swap layout at base", |grid_snapshot| {
        grid_snapshot.contains("BASE")
    });

    yinz.send_stdin(&keys::alt('['));

    let grid_snapshot = yinz.wait_until("swap layout moved back to stacked", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && grid_snapshot.contains("STACKED")
            && !grid_snapshot.contains("HALF-STACKED")
            && !grid_snapshot.contains("BASE")
            // The tab bar is a plugin; wait for it to name the visible stack member.
            && grid_snapshot
                .tab_bar_line()
                .is_some_and(|bar| bar.contains("[1] Tab #1"))
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}
