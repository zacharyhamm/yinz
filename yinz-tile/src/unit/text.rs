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
