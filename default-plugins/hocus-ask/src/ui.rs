//! Laying out the one-line prompt.
//!
//! Pure: no host calls, so the awkward cases — a pane narrower than the prompt,
//! a cursor sitting past the right edge, a double-width character straddling
//! the boundary — are testable without a running Zellij.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The visible line and the terminal column the cursor belongs in.
///
/// The box is a single row, so the text *scrolls sideways* rather than wrapping
/// the way hocus's input box does. The prompt is pinned: it is the only thing
/// telling the user what this pane is when frames are off, so it must not be
/// the first casualty of a long question.
///
/// `cursor` is a **char** index into `text`, as `TextInput::get_cursor_position`
/// returns. Columns are measured with `unicode-width`, which is where this
/// diverges from `TextInput`'s own char-counting cursor arithmetic — the one
/// place it is cheap to get right.
///
/// Guarantees, for every input: the returned line is at most `cols` columns
/// wide, and the returned column is inside `0..cols`. A render that breaks
/// either corrupts the grid rather than merely looking wrong.
pub fn viewport(prompt: &str, text: &str, cursor: usize, cols: usize) -> (String, usize) {
    if cols == 0 {
        return (String::new(), 0);
    }
    let prompt_width = prompt.width();
    if prompt_width >= cols {
        // Narrower than the prompt itself. Nothing useful to do but show what
        // fits and park the cursor at the edge; the user cannot read their own
        // typing at this width either way.
        return (take_columns(prompt, cols), cols - 1);
    }

    let available = cols - prompt_width;
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());

    // Scroll back from the cursor, keeping one column free for the cursor
    // itself so it stays on screen when it sits past the last character.
    let mut start = cursor;
    let mut used = 0usize;
    while start > 0 {
        let width = char_width(chars[start - 1]);
        if used + width > available - 1 {
            break;
        }
        used += width;
        start -= 1;
    }

    // Then forward as far as the row has room for.
    let mut line = String::with_capacity(cols);
    line.push_str(prompt);
    let mut shown = 0usize;
    for &c in &chars[start..] {
        let width = char_width(c);
        if shown + width > available {
            break;
        }
        shown += width;
        line.push(display_char(c));
    }

    (line, prompt_width + used)
}

/// As much of `text` as fits in `max` columns, never splitting a character.
fn take_columns(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut used = 0usize;
    for c in text.chars() {
        let width = char_width(c);
        if used + width > max {
            break;
        }
        used += width;
        out.push(display_char(c));
    }
    out
}

/// What a character is drawn as.
///
/// A pasted stack trace brings tabs and newlines with it, and a control byte
/// written into the grid is not a rendering bug but a corrupted pane. Standing
/// each one in as a space keeps the substitution one-for-one, so the cursor
/// arithmetic below needs no separate mapping between buffer and screen.
fn display_char(c: char) -> char {
    if c.is_control() {
        ' '
    } else {
        c
    }
}

/// How many columns a character is drawn in.
///
/// Measured on the substitution rather than the original, which is the whole
/// reason it agrees with `str::width` on the finished line: `unicode-width`
/// reports `None` for a control character but counts one column for it inside
/// a string, and a line built on the first number would overflow a pane sized
/// by the second.
fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(display_char(c)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
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

    /// The invariant the popup depends on at every pane size. Break it and the
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
}
