//! Wrapping text to a terminal pane's width.
//!
//! Lived in `agent::ui` while the conversation was the only pane that wrapped anything. The
//! agents pane wraps too now - the idle-summary box under its list is drawn from these lines -
//! and one wrapper measuring one way is the whole point of moving it: a box whose rows disagree
//! about how wide a character is does not close.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Wrap `text` to `width` display columns.
///
/// Newlines are preserved (each one starts a new visual line, and blank lines
/// survive), widths are measured in terminal columns rather than bytes, and a
/// word too long to ever fit is hard-split at a column boundary rather than
/// truncated.
///
/// Control characters are dropped: model output is rendered as plain text, so an
/// escape sequence in a reply must not be able to repaint the pane. Tabs become
/// four spaces, since a tab measures zero columns here but eight on screen.
///
/// Never returns an empty `Vec` - empty input is one empty line.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let sanitized = sanitize(text);

    if width == 0 {
        // Nothing can fit; still honour the "one visual line per source line"
        // contract so callers can't divide by zero or loop forever.
        return sanitized.split('\n').map(|_| String::new()).collect();
    }

    let mut lines = Vec::new();
    for source_line in sanitized.split('\n') {
        wrap_line(source_line, width, &mut lines);
    }
    lines
}

/// Text as the wrapper will measure it: tabs expanded, control characters dropped.
///
/// Public because the markdown renderer in `yinz-tile-markdown` parses the sanitized string
/// rather than the raw one - an escape sequence in a reply must not reach the parser either.
pub fn sanitize(text: &str) -> String {
    // Append to one buffer to avoid a separate allocation for every character.
    let mut sanitized = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\t' => sanitized.push_str("    "),
            '\n' => sanitized.push('\n'),
            ch if ch.is_control() => {},
            ch => sanitized.push(ch),
        }
    }
    sanitized
}

/// One row of the input box, and where in the source string it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrappedLine {
    pub text: String,
    /// Char index (not byte offset) of this row's first character in the string
    /// that was wrapped. The cursor is located by comparing against this.
    pub start: usize,
}

/// Wrap `text` to `width` columns for the input box: break between words, but
/// without losing a single character.
///
/// The input box cannot use [`wrap`]. That one discards the spaces that caused a
/// break, so trailing spaces would be invisible and the cursor would stall on
/// them, and it gives the caller no way back from a char index to a cell - which
/// the cursor needs, because word wrap is not prefix-stable: a word under the
/// cursor can be pushed onto the next row by characters typed *after* it, so the
/// cursor's row cannot be derived by wrapping the text before it in isolation.
///
/// So this wrapper is lossless: every character belongs to exactly one row (bar
/// the newlines, which separate rows), rows are contiguous in char order, and
/// each row records where it starts. A run of spaces stays on the row it was
/// typed on and fills to the edge, splitting only where it overflows.
///
/// A line that is exactly full is followed by an empty one so the cursor has a
/// row to land on: without it the cursor would sit at column `width`, one past
/// the last column, which yinz hides.
pub fn wrap_input(text: &str, width: usize) -> Vec<WrappedLine> {
    let mut lines = Vec::new();
    // Char index of the current source line, walked forward by its length plus
    // the newline that ended it.
    let mut line_start = 0;
    for source_line in text.split('\n') {
        if width == 0 {
            // Nothing can fit; still honour the "one visual line per source
            // line" contract so callers can't divide by zero or loop forever.
            lines.push(WrappedLine {
                text: String::new(),
                start: line_start,
            });
        } else {
            wrap_input_line(source_line, width, line_start, &mut lines);
        }
        line_start += source_line.chars().count() + 1;
    }
    if width > 0 && lines.last().is_some_and(|line| line.text.width() == width) {
        lines.push(WrappedLine {
            text: String::new(),
            start: text.chars().count(),
        });
    }
    lines
}

fn wrap_input_line(line: &str, width: usize, base: usize, out: &mut Vec<WrappedLine>) {
    // `current` always holds exactly the characters `start..at` of the source,
    // which is what makes the recorded starts line up with the text.
    let mut current = String::new();
    let mut current_width = 0usize;
    let mut start = base;
    let mut at = base;

    for token in tokenize(line) {
        match token {
            // Spaces stay on the row that was being typed when they arrived:
            // in an input the cursor has to move as they are typed, so they
            // can't be held back the way `wrap` holds and drops them. Only the
            // ones that overflow the row break, and they break at the edge.
            Token::Spaces(spaces) => {
                for ch in spaces.chars() {
                    let ch_width = ch.width().unwrap_or(0);
                    if current_width + ch_width > width {
                        out.push(WrappedLine {
                            text: std::mem::take(&mut current),
                            start,
                        });
                        start = at;
                        current_width = 0;
                    }
                    current.push(ch);
                    current_width += ch_width;
                    at += 1;
                }
            },
            Token::Word(word) => {
                let word_width = word.width();
                if current_width + word_width <= width {
                    current.push_str(word);
                    current_width += word_width;
                    at += word.chars().count();
                } else if word_width <= width {
                    // The word wrap: the whole word moves down together.
                    out.push(WrappedLine {
                        text: std::mem::take(&mut current),
                        start,
                    });
                    start = at;
                    current.push_str(word);
                    current_width = word_width;
                    at += word.chars().count();
                } else {
                    // Wider than a whole row however we break it: flush, then
                    // split at column boundaries. No hyphenation - this is a
                    // terminal.
                    if !current.is_empty() {
                        out.push(WrappedLine {
                            text: std::mem::take(&mut current),
                            start,
                        });
                        start = at;
                    }
                    let mut pieces = hard_split(word, width);
                    let last = pieces.pop().unwrap_or_default();
                    for piece in pieces {
                        let chars = piece.chars().count();
                        out.push(WrappedLine { text: piece, start });
                        start += chars;
                        at += chars;
                    }
                    at += last.chars().count();
                    current_width = last.width();
                    current = last;
                }
            },
        }
    }

    // Always push, even when empty: a blank source line is a blank visual line.
    out.push(WrappedLine {
        text: current,
        start,
    });
}

fn wrap_line(line: &str, width: usize, out: &mut Vec<String>) {
    let mut current = String::new();
    let mut current_width = 0usize;
    let mut pending_spaces = String::new();

    for token in tokenize(line) {
        let word = match token {
            // Spaces are held back so a break can discard them: a wrapped line
            // should not start with the space that caused the wrap. Held rather
            // than collapsed so leading indentation survives, which matters for
            // code in model output.
            Token::Spaces(spaces) => {
                pending_spaces.push_str(spaces);
                continue;
            },
            Token::Word(word) => word,
        };

        let spaces_width = pending_spaces.width();
        let word_width = word.width();

        if current_width + spaces_width + word_width <= width {
            current.push_str(&pending_spaces);
            current.push_str(word);
            current_width += spaces_width + word_width;
        } else if word_width <= width {
            out.push(std::mem::take(&mut current));
            current.push_str(word);
            current_width = word_width;
        } else {
            // Wider than the whole line however we break it: flush, then split
            // at column boundaries. No hyphenation - this is a terminal.
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            let mut pieces = hard_split(word, width);
            let last = pieces.pop().unwrap_or_default();
            out.extend(pieces);
            current_width = last.width();
            current = last;
        }
        pending_spaces.clear();
    }

    // Always push, even when empty: a blank source line is a blank visual line.
    out.push(current);
}

fn hard_split(word: &str, width: usize) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut piece = String::new();
    let mut piece_width = 0usize;
    for ch in word.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if piece_width + ch_width > width && !piece.is_empty() {
            pieces.push(std::mem::take(&mut piece));
            piece_width = 0;
        }
        piece.push(ch);
        piece_width += ch_width;
    }
    pieces.push(piece);
    pieces
}

enum Token<'a> {
    Spaces(&'a str),
    Word(&'a str),
}

/// Split into alternating runs of spaces and non-spaces.
fn tokenize(line: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut rest = line;
    while !rest.is_empty() {
        let is_space = rest.starts_with(' ');
        let end = rest
            .find(|ch: char| (ch == ' ') != is_space)
            .unwrap_or(rest.len());
        let (head, tail) = rest.split_at(end);
        tokens.push(if is_space {
            Token::Spaces(head)
        } else {
            Token::Word(head)
        });
        rest = tail;
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- wrap ---------------------------------------------------------------

    #[test]
    fn wrap_returns_one_empty_line_for_empty_input() {
        assert_eq!(wrap("", 10), vec![""]);
    }

    #[test]
    fn wrap_leaves_a_short_line_alone() {
        assert_eq!(wrap("hello there", 40), vec!["hello there"]);
    }

    #[test]
    fn wrap_breaks_on_word_boundaries() {
        assert_eq!(wrap("aaa bbb ccc", 7), vec!["aaa bbb", "ccc"]);
    }

    #[test]
    fn wrap_preserves_newlines_and_blank_lines() {
        assert_eq!(
            wrap("one\n\ntwo", 20),
            vec!["one".to_string(), String::new(), "two".to_string()]
        );
    }

    #[test]
    fn wrap_preserves_leading_indentation() {
        // Code in model output relies on this surviving.
        assert_eq!(wrap("    indented", 40), vec!["    indented"]);
    }

    #[test]
    fn wrap_drops_the_spaces_that_caused_a_break() {
        assert_eq!(wrap("aaaa bbbb", 4), vec!["aaaa", "bbbb"]);
    }

    #[test]
    fn wrap_measures_in_display_columns_not_bytes() {
        // Each CJK character is two columns wide, so only two fit in four.
        assert_eq!(wrap("日本語", 4), vec!["日本", "語"]);
    }

    #[test]
    fn wrap_hard_splits_a_word_longer_than_the_line() {
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn wrap_hard_splits_after_flushing_the_current_line() {
        assert_eq!(wrap("hi abcdefghij", 4), vec!["hi", "abcd", "efgh", "ij"]);
    }

    #[test]
    fn wrap_terminates_on_zero_width() {
        assert_eq!(wrap("anything at all", 0), vec![""]);
        assert_eq!(wrap("two\nlines", 0), vec!["", ""]);
    }

    #[test]
    fn wrap_expands_tabs_to_spaces() {
        // A tab measures zero columns but prints as several, which would
        // silently desync every width calculation downstream.
        assert_eq!(wrap("a\tb", 20), vec!["a    b"]);
    }

    #[test]
    fn wrap_strips_control_characters() {
        // Model output is rendered as plain text; an escape sequence in a reply
        // must not be able to repaint the pane.
        assert_eq!(wrap("safe\u{1b}[31mtext", 40), vec!["safe[31mtext"]);
    }

    // -- wrap_input (the input box) -----------------------------------------

    /// Rows as text, for the cases that don't care where each one starts.
    fn rows(text: &str, width: usize) -> Vec<String> {
        wrap_input(text, width)
            .into_iter()
            .map(|line| line.text)
            .collect()
    }

    #[test]
    fn input_wrap_breaks_between_words() {
        assert_eq!(rows("aaa bbb ccc", 8), vec!["aaa bbb ", "ccc"]);
    }

    #[test]
    fn input_wrap_moves_a_whole_word_down_rather_than_splitting_it() {
        assert_eq!(rows("ab cdef", 5), vec!["ab ", "cdef"]);
    }

    /// A space typed against a full row has nowhere to go but the next one: it
    /// keeps its cell there rather than vanishing, so the cursor still moves.
    #[test]
    fn input_wrap_carries_an_overflowing_space_onto_the_next_row() {
        assert_eq!(rows("aaa bbb ccc", 7), vec!["aaa bbb", " ccc"]);
    }

    #[test]
    fn input_wrap_hard_splits_a_word_longer_than_the_line() {
        // No break can make it fit, so it breaks at the column boundary.
        assert_eq!(rows("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }

    /// The cursor needs a row to sit on once a row is full, and that empty row
    /// is what makes the break land on the keystroke that filled the line.
    #[test]
    fn input_wrap_adds_a_row_when_the_last_row_is_full() {
        assert_eq!(rows("abcd", 4), vec!["abcd", ""]);
        assert_eq!(wrap_input("abcd", 4).last().unwrap().start, 4);
    }

    #[test]
    fn input_wrap_keeps_trailing_spaces() {
        // wrap() drops these; in an input the cursor has to move as you type them.
        assert_eq!(rows("ab ", 4), vec!["ab "]);
    }

    #[test]
    fn input_wrap_splits_spaces_that_overflow_the_row() {
        // The spaces can't move down whole - the cursor is sitting in them.
        assert_eq!(rows("ab    c", 4), vec!["ab  ", "  c"]);
    }

    #[test]
    fn input_wrap_preserves_newlines_and_blank_lines() {
        assert_eq!(
            rows("one\n\ntwo", 20),
            vec!["one".to_string(), String::new(), "two".to_string()]
        );
    }

    #[test]
    fn input_wrap_measures_in_display_columns_not_bytes() {
        assert_eq!(rows("日本語", 4), vec!["日本", "語"]);
    }

    #[test]
    fn input_wrap_terminates_on_zero_width() {
        assert_eq!(rows("two\nlines", 0), vec!["", ""]);
    }

    /// What the cursor mapping stands on: no character is dropped, and every
    /// row's `start` is exactly where its text sits in the input.
    #[test]
    fn input_wrap_covers_every_character() {
        let samples = [
            "",
            " ",
            "hello world",
            "aaa bbb ccc",
            "ab    c",
            "abcdefghij klm",
            "one\n\ntwo three four",
            "trailing   ",
            "日本語 のテキスト",
        ];
        for sample in samples {
            for width in 1..=8 {
                let lines = wrap_input(sample, width);
                let chars: Vec<char> = sample.chars().collect();
                for line in &lines {
                    let end = line.start + line.text.chars().count();
                    assert!(end <= chars.len(), "{sample:?}/{width}: row past the end");
                    assert_eq!(
                        line.text,
                        chars[line.start..end].iter().collect::<String>(),
                        "{sample:?}/{width}: row text disagrees with its start"
                    );
                }
                // Rows are contiguous: each starts where the previous ended, or
                // one further on when a newline was consumed between them.
                for pair in lines.windows(2) {
                    let end = pair[0].start + pair[0].text.chars().count();
                    let gap = pair[1].start - end;
                    assert!(
                        gap <= 1 && (gap == 0 || chars[end] == '\n'),
                        "{sample:?}/{width}: characters lost between rows"
                    );
                }
                let last = lines.last().expect("never empty");
                assert_eq!(
                    last.start + last.text.chars().count(),
                    chars.len(),
                    "{sample:?}/{width}: characters lost off the end"
                );
            }
        }
    }
}
