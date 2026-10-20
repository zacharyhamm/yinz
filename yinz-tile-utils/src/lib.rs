// test comment line 1 - this is just harmless placeholder text for a test change
// test comment line 2 - this is just harmless placeholder text for a test change
// test comment line 3 - this is just harmless placeholder text for a test change
// test comment line 4 - this is just harmless placeholder text for a test change
// test comment line 5 - this is just harmless placeholder text for a test change
// test comment line 6 - this is just harmless placeholder text for a test change
// test comment line 7 - this is just harmless placeholder text for a test change
// test comment line 8 - this is just harmless placeholder text for a test change
// test comment line 9 - this is just harmless placeholder text for a test change
// test comment line 10 - this is just harmless placeholder text for a test change
// test comment 1
// test comment 2
// test comment 3
// test comment 4
// test comment 5
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

/// `text` cut to `max_width` columns, with a trailing `…` standing in for what was dropped.
///
/// Returns `text` untouched when it already fits. The ellipsis costs a column of its own, so a
/// truncated result is at most `max_width` wide - at `max_width` of 1 that leaves the ellipsis
/// alone, and at 0 there is no room for even that.
pub fn ellipsize(text: &str, max_width: usize) -> String {
    if text.width() <= max_width {
        return text.to_owned();
    }
    if max_width == 0 {
        return String::new();
    }
    let budget = max_width - 1;
    let mut truncated = String::new();
    let mut used = 0;
    for ch in text.chars() {
        // a zero-width combining mark would never advance `used`, so count it as a column
        let ch_width = ch.width().unwrap_or(0).max(1);
        if used + ch_width > budget {
            break;
        }
        truncated.push(ch);
        used += ch_width;
    }
    truncated.push('…');
    truncated
}

#[macro_export]
macro_rules! rgb {
    ($a:expr_2021) => {
        ansi_term::Color::Rgb($a.0, $a.1, $a.2)
    };
}

#[macro_export]
macro_rules! palette_match {
    ($palette_color:expr_2021) => {
        match $palette_color {
            PaletteColor::Rgb((r, g, b)) => RGB(r, g, b),
            PaletteColor::EightBit(color) => Fixed(color),
        }
    };
}

#[macro_export]
macro_rules! style {
    ($fg:expr_2021, $bg:expr_2021) => {
        ansi_term::Style::new()
            .fg(match $fg {
                PaletteColor::Rgb((r, g, b)) => ansi_term::Color::RGB(r, g, b),
                PaletteColor::EightBit(color) => ansi_term::Color::Fixed(color),
            })
            .on(match $bg {
                PaletteColor::Rgb((r, g, b)) => ansi_term::Color::RGB(r, g, b),
                PaletteColor::EightBit(color) => ansi_term::Color::Fixed(color),
            })
    };
}

#[cfg(test)]
mod tests {
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
}
