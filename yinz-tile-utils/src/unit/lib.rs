use super::*;

#[test]
fn ellipsize_leaves_a_fitting_string_alone() {
    assert_eq!(ellipsize("hello", 10), "hello");
    assert_eq!(ellipsize("hello", 5), "hello");
    assert_eq!(ellipsize("", 0), "");
}

#[test]
fn ellipsize_cuts_one_column_over() {
    assert_eq!(ellipsize("hello", 4), "hel…");
}

#[test]
fn ellipsize_result_never_exceeds_the_budget() {
    for max_width in 0..12 {
        assert!(ellipsize("a rather long title", max_width).width() <= max_width);
    }
}

#[test]
fn ellipsize_counts_wide_characters_by_their_columns() {
    // each of these is two columns wide, so only two of them fit beside the ellipsis
    assert_eq!(ellipsize("日本語です", 5), "日本…");
}

#[test]
fn ellipsize_makes_progress_through_combining_marks() {
    // a run of zero-width marks is no wider than what it decorates, so it is left alone
    assert_eq!(
        ellipsize("a\u{0301}\u{0301}\u{0301}\u{0301}", 3),
        "a\u{0301}\u{0301}\u{0301}\u{0301}"
    );
    // and when the string really is too wide, counting each mark as a column keeps the loop
    // advancing rather than spinning on characters that cost nothing
    let accented = "a\u{0301}b\u{0301}c\u{0301}d\u{0301}";
    assert!(ellipsize(accented, 3).width() <= 3);
}

#[test]
fn ellipsize_at_one_column_is_the_ellipsis_alone() {
    assert_eq!(ellipsize("hello", 1), "…");
}

#[test]
fn ellipsize_at_zero_columns_is_empty() {
    assert_eq!(ellipsize("hello", 0), "");
}
