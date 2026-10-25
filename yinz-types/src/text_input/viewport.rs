//! Laying out a one-line prompt: which slice of the text is on screen, and
//! which column the cursor sits in.
//!
//! Pure: no host calls, so
//! the awkward cases — a pane narrower than the prompt, a cursor sitting past
//! the right edge, a double-width character straddling the boundary — are
//! testable without a running Yinz.
//!
//! This is where [`TextInput`](super::TextInput)'s char-indexed cursor becomes a
//! screen column. The conversion is measured in columns via `unicode-width`
//! rather than by counting chars, which is why a CJK or emoji character before
//! the cursor does not shift it off by one.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The visible line and the terminal column the cursor belongs in.
///
/// The box is a single row, so the text *scrolls sideways* rather than wrapping
/// the way agent's input box does. The prompt is pinned: it is the only thing
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
    let (line, column, _) = viewport_window(prompt, text, cursor, cols);
    (line, column)
}

/// As [`viewport`], plus the char index the visible window starts at.
///
/// The extra number is what turns a clicked column back into a cursor position:
/// without it a caller would have to re-derive the scroll offset, which is the
/// one part of this that is easy to get subtly wrong.
pub fn viewport_window(
    prompt: &str,
    text: &str,
    cursor: usize,
    cols: usize,
) -> (String, usize, usize) {
    if cols == 0 {
        return (String::new(), 0, 0);
    }
    let prompt_width = prompt.width();
    if prompt_width >= cols {
        // Narrower than the prompt itself. Nothing useful to do but show what
        // fits and park the cursor at the edge; the user cannot read their own
        // typing at this width either way.
        return (take_columns(prompt, cols), cols - 1, 0);
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

    (line, prompt_width + used, start)
}

/// Which char index a click at `column` lands on, for a row drawn by
/// [`viewport_window`] with the same arguments.
///
/// Clicking left of the text (on the prompt) puts the cursor at the start of
/// the visible window rather than at the start of the buffer — the buffer may
/// be scrolled, and jumping somewhere off screen is not what the click looked
/// like it would do. Clicking past the last character lands after it, which is
/// how clicking into the empty tail of a field appends.
pub fn column_to_cursor(
    prompt: &str,
    text: &str,
    cursor: usize,
    cols: usize,
    column: usize,
) -> usize {
    let (_, _, start) = viewport_window(prompt, text, cursor, cols);
    let prompt_width = prompt.width();
    if cols == 0 || prompt_width >= cols || column <= prompt_width {
        return start;
    }

    let target = column - prompt_width;
    let chars: Vec<char> = text.chars().collect();
    let available = cols - prompt_width;

    let mut index = start;
    let mut used = 0usize;
    while index < chars.len() {
        let width = char_width(chars[index]);
        // Stop once the next character would start past the click, or past the
        // row. Half a wide character counts as that character.
        if used + width > target || used + width > available {
            break;
        }
        used += width;
        index += 1;
    }
    index
}

/// As much of `text` as fits in `max` columns, never splitting a character.
pub fn take_columns(text: &str, max: usize) -> String {
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
#[path = "unit/viewport_tests.rs"]
mod tests;
