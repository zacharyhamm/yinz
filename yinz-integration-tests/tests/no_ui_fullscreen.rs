#![cfg(unix)]

use insta::assert_snapshot;
use yinz_cli::args::CliAction;
use yinz_integration_tests::{
    assert_same_rendered_grid, claim_first_terminal_and_wait_for_prompt, col, keys, normalized,
    split_right_and_wait_for_prompt, start_yinz, Size, PROMPT, TERMINAL_SIZE,
};
use yinz_types::input::options::PaneFrameStyle;

const FULL_FRAME_CORNER: &str = "┌";

fn toggle_no_ui_fullscreen_via_keybinding(yinz: &yinz_integration_tests::TestSession) {
    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('F'));
}

#[test]
fn no_ui_fullscreen_hides_ui_and_covers_display() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);

    toggle_no_ui_fullscreen_via_keybinding(&yinz);

    let grid_snapshot = yinz.wait_until(
        "focused pane covers the whole display without chrome",
        |grid_snapshot| {
            !grid_snapshot.tab_bar_appears()
                && !grid_snapshot.status_bar_appears()
                && grid_snapshot.cursor_is_at(col(2).row(0))
        },
    );
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn no_ui_fullscreen_toggle_off_restores_ui_and_layout() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);

    let before = yinz.wait_until("two panes settled with chrome", |grid_snapshot| {
        grid_snapshot.tab_bar_appears()
            && grid_snapshot.status_bar_appears()
            && grid_snapshot.cursor_is_at(col(62).row(2))
    });

    toggle_no_ui_fullscreen_via_keybinding(&yinz);
    yinz.wait_until("chrome hidden while no-ui fullscreen", |grid_snapshot| {
        !grid_snapshot.tab_bar_appears() && !grid_snapshot.status_bar_appears()
    });

    toggle_no_ui_fullscreen_via_keybinding(&yinz);
    let after = yinz.wait_until("chrome and layout restored", |grid_snapshot| {
        grid_snapshot.tab_bar_appears()
            && grid_snapshot.status_bar_appears()
            && !grid_snapshot.contains("(FULLSCREEN)")
            && grid_snapshot.cursor_is_at(col(62).row(2))
    });

    assert_same_rendered_grid(
        &after,
        &before,
        "layout, UI panes and frames are restored exactly",
    );
    yinz.quit();
}

#[test]
fn no_ui_fullscreen_works_with_single_pane() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    toggle_no_ui_fullscreen_via_keybinding(&yinz);

    let grid_snapshot = yinz.wait_until(
        "single pane covers the whole display without chrome",
        |grid_snapshot| {
            !grid_snapshot.tab_bar_appears()
                && !grid_snapshot.status_bar_appears()
                && grid_snapshot.cursor_is_at(col(2).row(0))
        },
    );
    assert_snapshot!(normalized(&grid_snapshot));

    toggle_no_ui_fullscreen_via_keybinding(&yinz);
    yinz.wait_until("chrome restored for single pane", |grid_snapshot| {
        grid_snapshot.tab_bar_appears()
            && grid_snapshot.status_bar_appears()
            && grid_snapshot.contains("Pane #1")
            && grid_snapshot.cursor_is_at(col(2).row(2))
    });
    yinz.quit();
}

#[test]
fn resize_terminal_while_no_ui_fullscreen_tracks_display() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    let right_terminal = split_right_and_wait_for_prompt(&yinz);

    toggle_no_ui_fullscreen_via_keybinding(&yinz);
    yinz.wait_until("chrome hidden while no-ui fullscreen", |grid_snapshot| {
        !grid_snapshot.tab_bar_appears()
            && !grid_snapshot.status_bar_appears()
            && grid_snapshot.cursor_is_at(col(2).row(0))
    });

    yinz.resize(Size {
        cols: 100,
        rows: 24,
    });
    right_terminal.wait_for_size(
        "no-ui fullscreen pane tracks the new display size",
        |cols, rows| cols == 100 && rows == 24,
    );
    let grid_snapshot = yinz.wait_until(
        "no-ui fullscreen still covers the resized display",
        |grid_snapshot| {
            grid_snapshot.contains("$ ")
                && !grid_snapshot.tab_bar_appears()
                && !grid_snapshot.status_bar_appears()
                && grid_snapshot.cursor_is_at(col(2).row(0))
        },
    );
    assert_snapshot!(normalized(&grid_snapshot));

    toggle_no_ui_fullscreen_via_keybinding(&yinz);
    yinz.wait_until("chrome restored at the new size", |grid_snapshot| {
        grid_snapshot.tab_bar_appears()
            && grid_snapshot.contains("Ctrl +")
            && grid_snapshot.cursor_is_at(col(52).row(2))
    });
    yinz.quit();
}

#[test]
fn no_ui_fullscreen_with_full_frames() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    let right_terminal = split_right_and_wait_for_prompt(&yinz);

    let exit_code = yinz.run_cli_action(CliAction::SetPaneFrameStyle {
        style: PaneFrameStyle::Full,
    });
    assert_eq!(exit_code, 0, "set-pane-frame-style exited cleanly");
    yinz.wait_until("full frames drawn", |grid_snapshot| {
        grid_snapshot.contains(FULL_FRAME_CORNER)
    });

    toggle_no_ui_fullscreen_via_keybinding(&yinz);

    right_terminal.wait_for_size(
        "no-ui fullscreen pane forced frameless and sized to the exact display",
        |cols, rows| cols == TERMINAL_SIZE.cols as u16 && rows == TERMINAL_SIZE.rows as u16,
    );
    let grid_snapshot = yinz.wait_until(
        "frameless fullscreen pane covers the whole display without chrome",
        |grid_snapshot| {
            !grid_snapshot.tab_bar_appears()
                && !grid_snapshot.status_bar_appears()
                && !grid_snapshot.contains(FULL_FRAME_CORNER)
                && grid_snapshot.cursor_is_at(col(2).row(0))
        },
    );
    assert_snapshot!(normalized(&grid_snapshot));

    toggle_no_ui_fullscreen_via_keybinding(&yinz);
    yinz.wait_until("chrome and frames restored", |grid_snapshot| {
        grid_snapshot.tab_bar_appears()
            && grid_snapshot.status_bar_appears()
            && grid_snapshot.contains(FULL_FRAME_CORNER)
    });
    yinz.quit();
}

#[test]
fn no_ui_fullscreen_with_frameless_panes() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    let right_terminal = split_right_and_wait_for_prompt(&yinz);

    let exit_code = yinz.run_cli_action(CliAction::SetPaneFrameStyle {
        style: PaneFrameStyle::None,
    });
    assert_eq!(exit_code, 0, "set-pane-frame-style exited cleanly");
    yinz.wait_until("titles removed in none mode", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && !grid_snapshot.contains("Pane #1")
    });

    toggle_no_ui_fullscreen_via_keybinding(&yinz);

    right_terminal.wait_for_size(
        "frameless no-ui fullscreen pane sized to the exact display",
        |cols, rows| cols == TERMINAL_SIZE.cols as u16 && rows == TERMINAL_SIZE.rows as u16,
    );
    yinz.wait_until(
        "frameless pane covers the whole display without chrome",
        |grid_snapshot| {
            !grid_snapshot.tab_bar_appears()
                && !grid_snapshot.status_bar_appears()
                && grid_snapshot.cursor_is_at(col(2).row(0))
        },
    );

    toggle_no_ui_fullscreen_via_keybinding(&yinz);
    yinz.wait_until("chrome restored in none mode", |grid_snapshot| {
        grid_snapshot.tab_bar_appears() && grid_snapshot.status_bar_appears()
    });
    yinz.quit();
}

#[test]
fn no_ui_fullscreen_on_focused_floating_pane_covers_display() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('w'));
    let floating_terminal = yinz.expect_pty_spawn();
    floating_terminal.output(PROMPT);
    floating_terminal.output(b"floating_marker");
    yinz.wait_until(
        "floating pane rendered and focused in normal mode",
        |grid_snapshot| {
            grid_snapshot.contains(FULL_FRAME_CORNER)
                && grid_snapshot.contains("floating_marker")
                && grid_snapshot.status_bar_appears()
                && grid_snapshot.tab_bar_appears()
        },
    );

    let exit_code = yinz.run_cli_action(CliAction::ToggleNoUiFullscreen { pane_id: None });
    assert_eq!(exit_code, 0, "toggle-no-ui-fullscreen exited cleanly");

    yinz.wait_until(
        "focused floating pane goes no-ui fullscreen and hides the chrome",
        |grid_snapshot| {
            grid_snapshot.contains("floating_marker")
                && !grid_snapshot.tab_bar_appears()
                && !grid_snapshot.status_bar_appears()
                && !grid_snapshot.contains(FULL_FRAME_CORNER)
        },
    );

    let exit_code = yinz.run_cli_action(CliAction::ToggleNoUiFullscreen { pane_id: None });
    assert_eq!(exit_code, 0, "toggle-no-ui-fullscreen exited cleanly");
    yinz.wait_until(
        "toggling off restores the chrome and the floating frame",
        |grid_snapshot| {
            grid_snapshot.tab_bar_appears()
                && grid_snapshot.status_bar_appears()
                && grid_snapshot.contains(FULL_FRAME_CORNER)
        },
    );
    yinz.quit();
}

#[test]
fn regular_fullscreen_switches_to_no_ui_fullscreen() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('f'));
    yinz.wait_until("regular fullscreen keeps the chrome", |grid_snapshot| {
        grid_snapshot.contains("(FULLSCREEN)")
            && grid_snapshot.status_bar_appears()
            && grid_snapshot.cursor_is_at(col(2).row(2))
    });

    toggle_no_ui_fullscreen_via_keybinding(&yinz);
    yinz.wait_until(
        "switching kinds hides the chrome without leaving fullscreen",
        |grid_snapshot| {
            !grid_snapshot.tab_bar_appears()
                && !grid_snapshot.status_bar_appears()
                && grid_snapshot.cursor_is_at(col(2).row(0))
        },
    );

    toggle_no_ui_fullscreen_via_keybinding(&yinz);
    yinz.wait_until("fullscreen fully off, layout restored", |grid_snapshot| {
        grid_snapshot.tab_bar_appears()
            && grid_snapshot.status_bar_appears()
            && !grid_snapshot.contains("(FULLSCREEN)")
            && grid_snapshot.cursor_is_at(col(62).row(2))
    });
    yinz.quit();
}

#[test]
fn move_focus_while_no_ui_fullscreen_keeps_no_ui() {
    let mut yinz = start_yinz();
    let first_terminal = claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);

    toggle_no_ui_fullscreen_via_keybinding(&yinz);
    yinz.wait_until("right pane is no-ui fullscreen", |grid_snapshot| {
        !grid_snapshot.tab_bar_appears()
            && !grid_snapshot.status_bar_appears()
            && grid_snapshot.cursor_is_at(col(2).row(0))
    });

    yinz.send_stdin(&keys::alt('h'));
    first_terminal.output(b"left_marker");

    let grid_snapshot = yinz.wait_until(
        "focus moved to the left pane which is now no-ui fullscreen",
        |grid_snapshot| grid_snapshot.contains("left_marker"),
    );
    assert!(
        !grid_snapshot.tab_bar_appears() && !grid_snapshot.status_bar_appears(),
        "no-ui fullscreen kind survives a focus move"
    );
    yinz.quit();
}

#[test]
fn new_pane_while_no_ui_fullscreen_breaks_out_and_restores_chrome() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);

    toggle_no_ui_fullscreen_via_keybinding(&yinz);
    yinz.wait_until("chrome hidden while no-ui fullscreen", |grid_snapshot| {
        !grid_snapshot.tab_bar_appears() && !grid_snapshot.status_bar_appears()
    });

    yinz.send_stdin(&keys::alt('n'));
    let new_terminal = yinz.expect_pty_spawn();
    new_terminal.output(PROMPT);

    let grid_snapshot = yinz.wait_until(
        "all three panes tiled with chrome after breaking out of no-ui fullscreen",
        |grid_snapshot| {
            grid_snapshot.tab_bar_appears()
                && grid_snapshot.status_bar_appears()
                && !grid_snapshot.contains("(FULLSCREEN)")
                && grid_snapshot.contains("Pane #1")
                && grid_snapshot.contains("Pane #2")
                && grid_snapshot.contains("Pane #3")
                && grid_snapshot.cursor_is_at(col(62).row(13))
        },
    );
    assert_snapshot!(normalized(&grid_snapshot));
    yinz.quit();
}

#[test]
fn closing_the_no_ui_fullscreen_pane_restores_the_remaining_layout() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);
    let two_pane_state = yinz.wait_until("two panes settled", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && grid_snapshot.cursor_is_at(col(62).row(2))
    });

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('d'));
    let third_terminal = yinz.expect_pty_spawn();
    third_terminal.output(PROMPT);
    yinz.wait_until("third pane split below the right pane", |grid_snapshot| {
        grid_snapshot.status_bar_appears() && grid_snapshot.cursor_is_at(col(62).row(13))
    });

    toggle_no_ui_fullscreen_via_keybinding(&yinz);
    yinz.wait_until("third pane is no-ui fullscreen", |grid_snapshot| {
        !grid_snapshot.tab_bar_appears() && !grid_snapshot.status_bar_appears()
    });

    yinz.send_stdin(&keys::ctrl('p'));
    yinz.send_stdin(&keys::key('x'));
    let after_close = yinz.wait_until(
        "no-ui fullscreen pane closed, two panes and chrome restored",
        |grid_snapshot| {
            grid_snapshot.tab_bar_appears()
                && grid_snapshot.status_bar_appears()
                && !grid_snapshot.contains("(FULLSCREEN)")
                && grid_snapshot.cursor_is_at(col(62).row(2))
        },
    );

    assert_same_rendered_grid(
        &after_close,
        &two_pane_state,
        "closing the no-ui fullscreen pane restores the pre-split layout",
    );
    yinz.quit();
}

#[test]
fn no_ui_fullscreen_through_the_cli() {
    let mut yinz = start_yinz();
    claim_first_terminal_and_wait_for_prompt(&yinz);
    split_right_and_wait_for_prompt(&yinz);

    let exit_code = yinz.run_cli_action(CliAction::ToggleNoUiFullscreen { pane_id: None });
    assert_eq!(exit_code, 0, "toggle-no-ui-fullscreen exited cleanly");
    yinz.wait_until("cli action hid the chrome", |grid_snapshot| {
        !grid_snapshot.tab_bar_appears() && !grid_snapshot.status_bar_appears()
    });

    let exit_code = yinz.run_cli_action(CliAction::ToggleNoUiFullscreen { pane_id: None });
    assert_eq!(exit_code, 0, "toggle-no-ui-fullscreen exited cleanly");
    yinz.wait_until("cli action restored the chrome", |grid_snapshot| {
        grid_snapshot.tab_bar_appears()
            && grid_snapshot.status_bar_appears()
            && grid_snapshot.cursor_is_at(col(62).row(2))
    });
    yinz.quit();
}
