#![cfg(unix)]

use insta::assert_snapshot;
use yinz_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, col, keys, normalized,
    split_right_and_wait_for_prompt, TestRunner, TestSession, PROMPT, TERMINAL_SIZE,
};

fn start_yinz() -> TestSession {
    TestRunner::new(TERMINAL_SIZE)
        .with_config("mouse_mode true")
        .start()
}

fn sgr_mouse_report(column: usize, line: usize, button: u8) -> Vec<u8> {
    format!("\u{1b}[<{};{};{}M", button, column, line).into_bytes()
}

#[test]
fn focus_pane_with_mouse() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&sgr_mouse_report(2, 5, 0));
    let grid_snapshot = yinz.wait_until("focus moved back to the left pane", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && grid_snapshot.cursor_is_at(col(2).row(2))
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn scrolling_inside_a_pane_with_mouse() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    let right_terminal = split_right_and_wait_for_prompt(&yinz);

    for line in 1..=21 {
        right_terminal.output(format!("line{}\r\n", line).as_bytes());
    }
    yinz.wait_until("right pane filled past its viewport", |grid_snapshot| {
        grid_snapshot.contains("line21")
    });

    yinz.send_stdin(&sgr_mouse_report(64, 4, 64));
    // Scrolling the pane puts the client in Scroll mode, so the status bar is the scroll hints
    // rather than the normal-mode bar.
    let grid_snapshot = yinz.wait_until("scrolled up inside the right pane", |grid_snapshot| {
        grid_snapshot.contains("PgDn|PgUp")
            && grid_snapshot.contains("SCROLL:")
            && grid_snapshot.contains("line1 ")
    });
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn pin_floating_panes() {
    let mut yinz = start_yinz();
    let tiled_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('w'));
    let floating_terminal = yinz.expect_pty_spawn();
    floating_terminal.output(PROMPT);
    yinz.wait_until("floating pane shows the unpinned button", |grid_snapshot| {
        grid_snapshot.contains("PIN [ ]")
    });

    yinz.send_stdin(&sgr_mouse_report(87, 8, 0));
    yinz.wait_until("floating pane became pinned", |grid_snapshot| {
        grid_snapshot.contains("PIN [+]")
    });

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('w'));
    yinz.wait_until(
        "focus settled on the tiled pane while the pinned pane stays up",
        |grid_snapshot| {
            grid_snapshot.status_bar_appears()
                && grid_snapshot.contains("PIN [+]")
                && grid_snapshot.cursor_is_at(col(2).row(1))
        },
    );

    for line in 1..=13 {
        tiled_terminal.output(format!("line{}{}\r\n", line, "a".repeat(112)).as_bytes());
    }

    let grid_snapshot = yinz.wait_until(
        "pinned pane stays visible over the filled tiled pane",
        |grid_snapshot| {
            grid_snapshot.status_bar_appears()
                && grid_snapshot.contains("PIN [+]")
                && grid_snapshot.contains("line13")
        },
    );
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}
