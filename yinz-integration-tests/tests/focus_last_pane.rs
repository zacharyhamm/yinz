#![cfg(unix)]

use insta::assert_snapshot;
use yinz_integration_tests::{
    claim_first_terminal_and_wait_for_prompt, col, keys, normalized,
    split_down_and_wait_for_prompt, split_right_and_wait_for_prompt, FakePtyHandle, GridSnapshot,
    Size, TestRunner, TestSession, PROMPT, TERMINAL_SIZE,
};

const FOCUS_KEYS_CONFIG: &str = r#"
keybinds {
    normal {
        bind "Alt m" { FocusLastPane; }
        bind "Alt ." { FocusNextPane; }
    }
}
"#;

fn start_with_focus_keys(size: Size) -> TestSession {
    TestRunner::new(size).with_config(FOCUS_KEYS_CONFIG).start()
}

fn split_focused_pane_down(yinz: &TestSession) -> FakePtyHandle {
    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('d'));
    let terminal = yinz.expect_pty_spawn();
    terminal.output(PROMPT);
    yinz.wait_until(
        "newly split lower pane focused below the middle pane",
        |grid| grid.status_bar_appears() && grid.cursor.is_some_and(|cursor| cursor.y > 13),
    );
    terminal
}

#[test]
fn focus_last_pane_toggles_focus_back_to_the_previous_pane() {
    let mut yinz = start_with_focus_keys(TERMINAL_SIZE);
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::alt('m'));
    let grid = yinz.wait_until(
        "focus_last_pane moved focus back to the left pane",
        |grid| grid.status_bar_appears() && grid.cursor_is_at(col(2).row(3)),
    );
    assert_snapshot!(normalized(&grid));
    yinz.quit();
}

#[test]
fn focus_last_pane_returns_to_earlier_pane_after_closing_focused_pane() {
    let mut yinz = start_with_focus_keys(TERMINAL_SIZE);
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_down_and_wait_for_prompt(&yinz);
    let bottom_terminal = split_focused_pane_down(&yinz);

    bottom_terminal.exit(Some(0));
    yinz.wait_until(
        "closing the bottom pane returns focus to the middle pane",
        |grid| grid.status_bar_appears() && grid.cursor_is_at(col(2).row(14)),
    );

    yinz.send_stdin(&keys::alt('m'));
    yinz.wait_until(
        "focus_last_pane moves focus to the pane active before the closed one",
        |grid| grid.status_bar_appears() && grid.cursor_is_at(col(2).row(3)),
    );
    yinz.quit();
}

#[test]
fn focus_last_pane_refills_after_cycling_focus() {
    let mut yinz = start_with_focus_keys(TERMINAL_SIZE);
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::alt('.'));
    yinz.wait_until("focus cycled away from the right pane", |grid| {
        grid.status_bar_appears() && grid.cursor.is_some_and(|cursor| cursor.x < 60)
    });

    yinz.send_stdin(&keys::alt('m'));
    yinz.wait_until(
        "focus_last_pane returns to the pane focused before cycling",
        |grid| grid.status_bar_appears() && grid.cursor_is_at(col(62).row(3)),
    );
    yinz.quit();
}

/// Whether the focused pane's prompt has been drawn and the cursor has come to rest
/// after it.
///
/// Opening a pane and the pane's first output are two separate renders, coalesced by a
/// 10ms debounce on an idle machine and not coalesced at all on a loaded one. So there is
/// a frame in which the new pane's frame and title exist but its `$ ` does not, and the
/// cursor sits at the content origin — a position it leaves and never returns to. A wait
/// that stops on that frame reads a cursor which is about to become wrong.
///
/// `render_bytes` paints the cursor cell as `█`, so "the prompt is there and the cursor
/// is after it" is `"$ █"` ending at the cursor. That keeps this free of any hardcoded
/// geometry, which matters because floating panes are staggered.
fn prompt_cursor_settled(grid: &GridSnapshot) -> bool {
    grid.cursor.is_some_and(|cursor| {
        cursor.x >= 2
            && grid
                .lines()
                .get(cursor.y)
                .is_some_and(|line| line.chars().skip(cursor.x - 2).take(3).eq("$ █".chars()))
    })
}

fn open_floating_pane_below(
    yinz: &TestSession,
    previous_cursor_row: usize,
) -> (FakePtyHandle, usize) {
    yinz.send_stdin(&keys::alt('n'));
    let terminal = yinz.expect_pty_spawn();
    terminal.output(PROMPT);
    let grid = yinz.wait_until("new floating pane focused below the previous one", |grid| {
        prompt_cursor_settled(grid)
            && grid
                .cursor
                .is_some_and(|cursor| cursor.y > previous_cursor_row)
    });
    (terminal, grid.cursor.unwrap().y)
}

#[test]
fn focus_last_floating_pane_returns_to_earlier_pane_after_closing_focused_pane() {
    // Cursor coordinates identify focus here, so opening another float must not
    // move the earlier panes through an automatic swap layout.
    let mut yinz = TestRunner::new(TERMINAL_SIZE)
        .with_config(FOCUS_KEYS_CONFIG)
        .with_config("auto_layout false")
        .start();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('w'));
    let first_floating = yinz.expect_pty_spawn();
    first_floating.output(PROMPT);
    // This cursor is compared for exact equality at the end of the test, so it has to be
    // read from a settled frame. `cursor.is_some()` was no guard at all — the tiled
    // prompt already satisfies it — so the wait could stop the moment the `Pane #2` frame
    // was drawn and record a position the cursor was still on its way out of.
    let first_floating_cursor = yinz
        .wait_until("first floating pane focused with its prompt", |grid| {
            grid.contains("Pane #2")
                && !grid.cursor_is_at(col(2).row(2))
                && prompt_cursor_settled(grid)
        })
        .cursor
        .unwrap();

    let (_second_floating, second_floating_row) =
        open_floating_pane_below(&yinz, first_floating_cursor.y);
    let (third_floating, _third_floating_row) =
        open_floating_pane_below(&yinz, second_floating_row);

    third_floating.exit(Some(0));
    yinz.wait_until(
        "closing the focused floating pane returns focus to the previous floating pane",
        |grid| {
            grid.cursor
                .is_some_and(|cursor| cursor.y == second_floating_row)
        },
    );

    yinz.send_stdin(&keys::alt('m'));
    yinz.wait_until(
        "focus_last_pane moves focus to the floating pane active before the closed one",
        |grid| grid.cursor == Some(first_floating_cursor),
    );
    yinz.quit();
}

#[test]
fn focus_last_pane_with_floating_visible_leaves_tiled_focus_untouched() {
    let mut yinz = start_with_focus_keys(TERMINAL_SIZE);
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('w'));
    let floating_terminal = yinz.expect_pty_spawn();
    floating_terminal.output(PROMPT);
    // Same settling requirement as above: the `Alt m` below is answered against whatever
    // focus state exists when it lands, and the assertion after it is an exact position.
    yinz.wait_until("floating pane focused over the tiled panes", |grid| {
        grid.contains("Pane #3") && prompt_cursor_settled(grid)
    });

    yinz.send_stdin(&keys::alt('m'));

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('w'));
    yinz.wait_until(
        "the tiled pane focused before opening floating panes stays focused",
        |grid| grid.status_bar_appears() && grid.cursor_is_at(col(62).row(3)),
    );
    yinz.quit();
}
