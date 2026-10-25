use super::*;
use hegel::generators;

const PROMPT: &str = "ask> ";

#[test]
fn a_short_question_is_shown_whole_with_the_cursor_after_it() {
    let (line, column) = viewport(PROMPT, "why", 3, 40);
    assert_eq!(line, "ask> why");
    assert_eq!(column, 8);
}

#[test]
fn an_empty_prompt_puts_the_cursor_straight_after_the_marker() {
    let (line, column) = viewport(PROMPT, "", 0, 40);
    assert_eq!(line, "ask> ");
    assert_eq!(column, 5);
}

/// The whole point of scrolling: typing past the right edge keeps the
/// cursor visible rather than walking off the pane.
#[test]
fn a_long_question_scrolls_so_the_cursor_is_the_last_column() {
    let text = "abcdefghij";
    let (line, column) = viewport(PROMPT, text, text.len(), 10);
    // Five columns of prompt leave five for text, one of them the cursor's.
    assert_eq!(line, "ask> ghij");
    assert_eq!(column, 9);
}

/// And moving back to the start rewinds the window, rather than stranding
/// the user looking at the tail of a line they are editing the head of.
#[test]
fn moving_the_cursor_home_rewinds_the_window() {
    let (line, column) = viewport(PROMPT, "abcdefghij", 0, 10);
    assert_eq!(line, "ask> abcde");
    assert_eq!(column, 5);
}

#[test]
fn a_pane_narrower_than_the_prompt_does_not_panic() {
    let (line, column) = viewport(PROMPT, "why", 3, 3);
    assert_eq!(line, "ask");
    assert_eq!(column, 2);
}

#[test]
fn a_single_column_pane_does_not_panic() {
    let (line, column) = viewport(PROMPT, "why", 3, 1);
    assert_eq!(line, "a");
    assert_eq!(column, 0);
}

/// A double-width character costs two columns, so the same char count
/// scrolls sooner than it would in Latin text.
#[test]
fn a_wide_character_takes_two_columns() {
    let (line, column) = viewport("> ", "日本", 2, 8);
    assert_eq!(line, "> 日本");
    assert_eq!(column, 6);
}

/// It also must not be split down the middle to fill a leftover column.
#[test]
fn a_wide_character_is_never_half_shown() {
    let (line, _) = viewport("> ", "日本", 0, 5);
    assert_eq!(line, "> 日");
    assert_eq!(line.width(), 4);
}

/// A pasted stack trace arrives with its newlines and tabs still in it. The
/// box is one row, so they become spaces rather than control bytes written
/// straight into the grid.
#[test]
fn a_pasted_newline_is_drawn_as_a_space() {
    let (line, column) = viewport(PROMPT, "why\nnot\there", 12, 40);
    assert_eq!(line, "ask> why not here");
    assert_eq!(column, 17);
}

#[test]
fn a_cursor_past_the_end_of_the_text_is_clamped() {
    let (line, column) = viewport(PROMPT, "why", 99, 40);
    assert_eq!(line, "ask> why");
    assert_eq!(column, 8);
}

/// The invariant a one-line prompt depends on at every pane size. Break it and the
/// render bleeds outside the box instead of merely looking wrong.
///
/// The text generator is deliberately unconstrained: a question is whatever
/// the user typed or pasted, wide characters and combining marks included.
#[hegel::test]
fn a_rendered_line_always_fits_the_pane(tc: hegel::TestCase) {
    let text = tc.draw(generators::text());
    let cols = tc.draw(generators::integers::<usize>().min_value(1).max_value(200));
    let cursor = tc.draw(
        generators::integers::<usize>()
            .min_value(0)
            .max_value(text.chars().count()),
    );

    let (line, column) = viewport(PROMPT, &text, cursor, cols);
    assert!(
        line.width() <= cols,
        "line {line:?} is {} columns wide, pane is {cols}",
        line.width()
    );
    assert!(column < cols, "cursor at column {column}, pane is {cols}");
}
