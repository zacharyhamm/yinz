//! `reset_terminal_state_preserving_scrollback`: the reset a command-pane respawn uses when the
//! next command should append to the pane's transcript rather than erase it.

use super::*;

#[test]
fn a_preserving_reset_scrolls_the_screen_into_history() {
    let mut grid = create_grid_with_content("step one says hello\r\nand a second line");
    grid.reset_terminal_state_preserving_scrollback();
    assert_eq!(
        scrollback_texts(&grid),
        vec!["step one says hello", "and a second line"],
        "what was on screen is history now, with the trailing blank rows trimmed"
    );
    assert!(
        viewport_texts(&grid).iter().all(|line| line.is_empty()),
        "the screen itself starts clean, got: {:?}",
        viewport_texts(&grid)
    );
}

#[test]
fn a_preserving_reset_appends_to_history_already_there() {
    let mut grid = create_grid_with_content("step one\r\n");
    grid.reset_terminal_state_preserving_scrollback();
    feed_bytes(&mut grid, b"step two");
    grid.reset_terminal_state_preserving_scrollback();
    assert_eq!(
        scrollback_texts(&grid),
        vec!["step one", "step two"],
        "each step's output lands below the one before it"
    );
}

#[test]
fn a_preserving_reset_keeps_the_primary_screen_not_the_alternate() {
    // A step that died inside vim: the alternate screen is live, hiding the primary screen.
    // The transcript worth keeping is the primary one the full-screen app was drawn over.
    let mut grid = create_grid_with_content("shell output before vim\r\n");
    feed_bytes(&mut grid, b"\x1b[?1049h");
    feed_bytes(&mut grid, b"vim contents");
    grid.reset_terminal_state_preserving_scrollback();
    let scrollback = scrollback_texts(&grid).join("\n");
    assert!(
        scrollback.contains("shell output before vim"),
        "the primary screen is the transcript, got: {scrollback:?}"
    );
    assert!(
        !scrollback.contains("vim contents"),
        "alternate-screen contents are not transcript, got: {scrollback:?}"
    );
    assert!(
        !grid.is_alternate_mode_active(),
        "the reset leaves the pane on the primary screen"
    );
}
