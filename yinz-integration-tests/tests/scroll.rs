#![cfg(unix)]

use insta::assert_snapshot;
use yinz_cli::args::CliAction;
use yinz_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, keys, normalized, split_right_and_wait_for_prompt,
    start_yinz, FakePtyHandle, GridSnapshot, TestSession,
};

const LAST_LINE: &str = "line40";

fn fill_pane_past_viewport(yinz: &TestSession) -> FakePtyHandle {
    let terminal = claim_first_terminal_and_wait_for_prompt(yinz);
    for line in 1..=40 {
        terminal.output(format!("line{:02}\r\n", line).as_bytes());
    }
    yinz.wait_until("pane filled past its viewport", |grid_snapshot| {
        grid_snapshot.contains(LAST_LINE)
    });
    terminal
}

fn first_terminal_line_is(snapshot: &GridSnapshot, expected: &str) -> bool {
    snapshot
        .text
        .lines()
        .nth(2)
        .is_some_and(|line| line.trim_end() == expected)
}

#[test]
fn page_scroll_up() {
    let mut yinz = start_yinz();
    fill_pane_past_viewport(&yinz);

    yinz.send_stdin(&keys::ctrl('s'));
    yinz.send_stdin(&keys::ctrl('b'));

    let grid_snapshot = yinz.wait_until("scrolled up by a page", |grid_snapshot| {
        grid_snapshot.contains("PgDn|PgUp")
            && !grid_snapshot.contains(LAST_LINE)
            && first_terminal_line_is(grid_snapshot, "$ line01")
            && grid_snapshot.contains("SCROLL 20/")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn page_scroll_down() {
    let mut yinz = start_yinz();
    fill_pane_past_viewport(&yinz);

    yinz.send_stdin(&keys::ctrl('s'));
    yinz.send_stdin(&keys::ctrl('b'));
    yinz.wait_until("scrolled up by a page", |grid_snapshot| {
        grid_snapshot.contains("PgDn|PgUp")
            && !grid_snapshot.contains(LAST_LINE)
            && first_terminal_line_is(grid_snapshot, "$ line01")
            && grid_snapshot.contains("SCROLL 19/")
    });

    yinz.send_stdin(&keys::ctrl('f'));

    // Landing back at the bottom leaves Scroll mode: the mode follows the pane's scroll
    // position, so the status bar here is the normal-mode one and not the scroll hints.
    let grid_snapshot = yinz.wait_until("scrolled back down toward the bottom", |grid_snapshot| {
        grid_snapshot.contains(LAST_LINE)
            && grid_snapshot.status_bar_appears()
            && first_terminal_line_is(grid_snapshot, "line20")
            && grid_snapshot.contains("SCROLL 0/")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn half_page_scroll_up() {
    let mut yinz = start_yinz();
    fill_pane_past_viewport(&yinz);

    yinz.send_stdin(&keys::ctrl('s'));
    yinz.send_stdin(&keys::key('u'));

    let grid_snapshot = yinz.wait_until("scrolled up by half a page", |grid_snapshot| {
        grid_snapshot.contains("PgDn|PgUp")
            && !grid_snapshot.contains(LAST_LINE)
            && first_terminal_line_is(grid_snapshot, "line10")
            && grid_snapshot.contains("SCROLL 10/")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn half_page_scroll_down() {
    let mut yinz = start_yinz();
    fill_pane_past_viewport(&yinz);

    yinz.send_stdin(&keys::ctrl('s'));
    yinz.send_stdin(&keys::key('u'));
    yinz.wait_until("scrolled up by half a page", |grid_snapshot| {
        grid_snapshot.contains("PgDn|PgUp")
            && !grid_snapshot.contains(LAST_LINE)
            && first_terminal_line_is(grid_snapshot, "line10")
            && grid_snapshot.contains("SCROLL 10/")
    });

    yinz.send_stdin(&keys::key('d'));

    // As in `page_scroll_down`: back at the bottom is back in the default mode.
    let grid_snapshot = yinz.wait_until("scrolled back down toward the bottom", |grid_snapshot| {
        grid_snapshot.contains(LAST_LINE)
            && grid_snapshot.status_bar_appears()
            && first_terminal_line_is(grid_snapshot, "line20")
            && grid_snapshot.contains("SCROLL 0/")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn scroll_down_one_line() {
    let mut yinz = start_yinz();
    fill_pane_past_viewport(&yinz);

    yinz.send_stdin(&keys::ctrl('s'));
    yinz.send_stdin(&keys::key('k'));
    yinz.send_stdin(&keys::key('k'));
    yinz.wait_until("scrolled up two lines", |grid_snapshot| {
        first_terminal_line_is(grid_snapshot, "line18")
            && grid_snapshot.contains("PgDn|PgUp")
            && grid_snapshot.contains("SCROLL 2/")
    });

    yinz.send_stdin(&keys::key('j'));

    let grid_snapshot = yinz.wait_until("scrolled back down one line", |grid_snapshot| {
        first_terminal_line_is(grid_snapshot, "line19")
            && grid_snapshot.contains("PgDn|PgUp")
            && grid_snapshot.contains("SCROLL 1/")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn scroll_to_bottom() {
    let mut yinz = start_yinz();
    fill_pane_past_viewport(&yinz);

    yinz.send_stdin(&keys::ctrl('s'));
    yinz.send_stdin(&keys::ctrl('b'));
    yinz.wait_until("scrolled up by a page", |grid_snapshot| {
        grid_snapshot.contains("PgDn|PgUp")
            && !grid_snapshot.contains(LAST_LINE)
            && first_terminal_line_is(grid_snapshot, "$ line01")
            && grid_snapshot.contains("SCROLL 19/")
    });

    yinz.send_stdin(&keys::ctrl('c'));

    let grid_snapshot = yinz.wait_until("returned to the bottom in normal mode", |grid_snapshot| {
        grid_snapshot.status_bar_appears()
            && grid_snapshot.contains(LAST_LINE)
            && first_terminal_line_is(grid_snapshot, "line20")
            && grid_snapshot.contains("SCROLL 0/")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn targeted_writes_clear_only_the_target_panes_scrollback() {
    let mut yinz = start_yinz();
    let left = claim_first_terminal_and_wait_for_prompt(&yinz);
    let right = split_right_and_wait_for_prompt(&yinz);
    for (terminal, label) in [(&left, "left"), (&right, "right")] {
        terminal.disable_echo();
        let mut output = String::from("\x1b[?2004h");
        for line in 1..=40 {
            output.push_str(&format!("{label}{line:02}\r\n"));
        }
        terminal.output(output.as_bytes());
    }
    yinz.wait_until("both panes filled past their viewports", |snapshot| {
        snapshot.contains("left40") && snapshot.contains("right40")
    });
    let left_id = format!("terminal_{}", left.terminal_id());
    let right_id = format!("terminal_{}", right.terminal_id());
    assert_eq!(
        yinz.run_cli_action(CliAction::PageScrollUp {
            pane_id: Some(left_id.clone()),
        }),
        0
    );
    assert_eq!(
        yinz.run_cli_action(CliAction::FocusPaneId { pane_id: left_id }),
        0
    );
    yinz.wait_until("focused left pane remains in scrollback", |snapshot| {
        !snapshot.contains("left40") && snapshot.contains("PgDn|PgUp")
    });

    let mut expected_stdin = Vec::new();
    for (action, input) in [
        (
            CliAction::Write {
                bytes: b"bytes".to_vec(),
                pane_id: Some(right_id.clone()),
            },
            b"bytes".as_slice(),
        ),
        (
            CliAction::WriteChars {
                chars: "chars".into(),
                pane_id: Some(right_id.clone()),
            },
            b"chars".as_slice(),
        ),
        (
            CliAction::Paste {
                chars: "paste".into(),
                pane_id: Some(right_id.clone()),
            },
            b"\x1b[200~paste\x1b[201~".as_slice(),
        ),
    ] {
        assert_eq!(
            yinz.run_cli_action(CliAction::PageScrollUp {
                pane_id: Some(right_id.clone()),
            }),
            0
        );
        yinz.wait_until("both panes are scrolled before the write", |snapshot| {
            !snapshot.contains("left40")
                && !snapshot.contains("right40")
                && snapshot.contains("PgDn|PgUp")
        });
        assert_eq!(yinz.run_cli_action(action), 0);
        expected_stdin.extend_from_slice(input);
        assert_eq!(
            right.wait_for_stdin("targeted input reached the right pane", |stdin| {
                stdin.len() >= expected_stdin.len()
            }),
            expected_stdin
        );
        yinz.wait_until("only the target pane returned to the bottom", |snapshot| {
            snapshot.contains("right40")
                && !snapshot.contains("left40")
                && snapshot.contains("PgDn|PgUp")
        });
        assert!(left.stdin_bytes().is_empty());
    }
    yinz.quit();
}
