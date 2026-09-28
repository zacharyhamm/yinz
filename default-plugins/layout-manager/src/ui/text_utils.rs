use crate::DisplayLayout;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use yinz_tile::prelude::*;

// Every budget in this module is in terminal columns, because that is what the caller has: a
// pane width, or a column of a table. A `char` count is not that (CJK and emoji take two
// columns, a combining mark none), and a byte count is further still (a single `é` is two).
// `unicode-width` answers the column question; these helpers just stay in that unit.

const ELLIPSIS: &str = "...";

/// Longest prefix of `chars` that fits in `budget` columns.
fn take_columns(chars: impl Iterator<Item = char>, budget: usize) -> String {
    let mut taken = String::new();
    let mut used = 0;
    for ch in chars {
        let ch_width = ch.width().unwrap_or(0);
        if used + ch_width > budget {
            break;
        }
        taken.push(ch);
        used += ch_width;
    }
    taken
}

/// Longest suffix of `text` that fits in `budget` columns.
fn take_columns_from_end(text: &str, budget: usize) -> String {
    take_columns(text.chars().rev(), budget)
        .chars()
        .rev()
        .collect()
}

/// `text` cut to `max_width` columns, with `...` standing in for what was dropped.
///
/// Unchanged when it already fits - and zero-width text fits every budget, a budget of zero
/// included. Below four columns there is no room for the ellipsis, so the text is simply cut;
/// at zero columns there is no room for anything, and taking a prefix of a budget that small
/// would hand back a column the caller does not have.
pub fn truncate_with_ellipsis(text: &str, max_width: usize) -> String {
    if text.width() <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width <= ELLIPSIS.len() {
        return take_columns(text.chars(), max_width);
    }
    let kept = take_columns(text.chars(), max_width - ELLIPSIS.len());
    format!("{kept}{ELLIPSIS}")
}

/// `truncate_with_ellipsis` keeping the end of `text` instead of its start.
pub fn truncate_with_ellipsis_start(text: &str, max_width: usize) -> String {
    if text.width() <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width <= ELLIPSIS.len() {
        return take_columns_from_end(text, max_width);
    }
    let kept = take_columns_from_end(text, max_width - ELLIPSIS.len());
    format!("{ELLIPSIS}{kept}")
}

struct AnsiString<'a> {
    content: &'a str,
}

impl<'a> AnsiString<'a> {
    fn new(content: &'a str) -> Self {
        Self { content }
    }

    fn stripped_content(&self) -> String {
        strip_ansi_escapes::strip_str(self.content)
    }

    fn display_width(&self) -> usize {
        let stripped = self.stripped_content();
        UnicodeWidthStr::width(stripped.as_str())
    }

    fn fits_within(&self, max_width: usize) -> bool {
        self.display_width() <= max_width
    }

    fn truncate_to_width(&self, max_width: usize) -> String {
        if max_width == 0 {
            return String::new();
        }

        if self.fits_within(max_width) {
            return self.content.to_string();
        }

        let mut truncated = String::new();
        for ch in self.content.chars() {
            let test_string = format!("{}{}", truncated, ch);
            let test_ansi = AnsiString::new(&test_string);

            if test_ansi.display_width() > max_width {
                break;
            }

            truncated.push(ch);
        }

        truncated
    }

    fn with_reset_code(content: String) -> String {
        format!("{}\u{1b}[0m", content)
    }
}

pub fn truncate_line_with_ansi(line: &str, max_width: usize) -> String {
    let ansi_line = AnsiString::new(line);
    let truncated = ansi_line.truncate_to_width(max_width);
    AnsiString::with_reset_code(truncated)
}

pub fn wrap_text_to_width(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return Vec::new();
    }
    if max_width < 3 {
        return text.chars().map(|c| c.to_string()).collect();
    }

    let words: Vec<&str> = text.split_whitespace().collect();
    let mut lines = Vec::new();
    let mut current_line = String::new();

    for word in words.iter() {
        let word_to_add = if word.width() > max_width {
            truncate_with_ellipsis(word, max_width)
        } else {
            word.to_string()
        };

        if current_line.is_empty() {
            current_line = word_to_add;
        } else {
            let test_line = format!("{} {}", current_line, word_to_add);
            if test_line.width() <= max_width {
                current_line = test_line;
            } else {
                lines.push(current_line);
                current_line = word_to_add;
            }
        }
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    lines
}

pub fn get_layout_display_info(layout: &DisplayLayout) -> (String, Option<&LayoutMetadata>) {
    match layout {
        DisplayLayout::Valid(info) => match info {
            LayoutInfo::BuiltIn(name) => (name.clone(), None),
            LayoutInfo::File(path, metadata) => {
                let name = path.split('/').next_back().unwrap_or(path).to_string();
                (name, Some(metadata))
            },
            LayoutInfo::Stringified(_) => ("raw".to_string(), None),
        },
        DisplayLayout::Error { name, .. } => (name.clone(), None),
    }
}

fn format_elapsed(unix_epoch_str: &str) -> String {
    let Ok(timestamp) = unix_epoch_str.parse::<i64>() else {
        return " ".to_string();
    };

    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let diff = now - timestamp;

    if diff < 60 {
        "now".to_string()
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86400 {
        format!("{}h ago", diff / 3600)
    } else {
        format!("{}d ago", diff / 86400)
    }
}

pub fn get_last_modified_string(metadata: Option<&LayoutMetadata>, is_builtin: bool) -> String {
    if let Some(metadata) = metadata {
        format_elapsed(&metadata.update_time)
    } else if is_builtin {
        "Built-in".to_string()
    } else {
        " ".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hegel::generators;

    #[test]
    fn ascii_truncation_is_unchanged() {
        assert_eq!(truncate_with_ellipsis("abcdef", 6), "abcdef");
        assert_eq!(truncate_with_ellipsis("abcdef", 5), "ab...");
        assert_eq!(truncate_with_ellipsis("abcdef", 3), "abc");
        assert_eq!(truncate_with_ellipsis("abcdef", 1), "a");
        assert_eq!(truncate_with_ellipsis("abcdef", 0), "");
        assert_eq!(truncate_with_ellipsis_start("abcdef", 5), "...ef");
        assert_eq!(truncate_with_ellipsis_start("abcdef", 3), "def");
        assert_eq!(truncate_with_ellipsis_start("abcdef", 0), "");
    }

    #[test]
    fn wide_characters_are_budgeted_by_column() {
        // seven characters, fourteen columns: a char count would call this a fit in ten
        let name = "日本語テキスト";
        assert_eq!(name.width(), 14);
        let truncated = truncate_with_ellipsis(name, 10);
        assert_eq!(truncated, "日本語...");
        assert!(truncated.width() <= 10);
        let from_end = truncate_with_ellipsis_start(name, 10);
        assert_eq!(from_end, "...キスト");
        assert!(from_end.width() <= 10);
        // an odd budget cannot be filled exactly by two-column characters
        assert_eq!(truncate_with_ellipsis(name, 8), "日本...");
        assert_eq!(truncate_with_ellipsis(name, 3), "日");
    }

    #[test]
    fn emoji_and_combining_marks_count_as_their_columns() {
        assert_eq!(truncate_with_ellipsis("🦀🦀🦀🦀", 8), "🦀🦀🦀🦀");
        assert_eq!(truncate_with_ellipsis("🦀🦀🦀🦀", 7), "🦀🦀...");
        // a decomposed `é` is two chars and one column
        let decomposed = "cafe\u{301} au lait";
        assert_eq!(decomposed.chars().count(), 13);
        assert_eq!(decomposed.width(), 12);
        assert_eq!(truncate_with_ellipsis(decomposed, 12), decomposed);
        assert_eq!(truncate_with_ellipsis(decomposed, 8), "cafe\u{301} ...");
    }

    #[test]
    fn multibyte_latin_wraps_by_column_not_by_byte() {
        // 17 columns, 21 bytes: byte counting broke this into three lines at width 17
        let text = "café crème brûlée";
        assert_eq!(wrap_text_to_width(text, 17), vec![text.to_string()]);
        assert_eq!(
            wrap_text_to_width(text, 10),
            vec!["café crème".to_string(), "brûlée".to_string()]
        );
    }

    #[test]
    fn cjk_wraps_by_column() {
        let lines = wrap_text_to_width("日本語 テキスト です", 9);
        assert_eq!(
            lines,
            vec![
                "日本語".to_string(),
                "テキスト".to_string(),
                "です".to_string()
            ]
        );
        // a word wider than the budget is cut to it rather than overflowing
        let lines = wrap_text_to_width("日本語テキスト", 6);
        assert_eq!(lines, vec!["日...".to_string()]);
    }

    #[test]
    fn wrap_edges_are_unchanged() {
        assert!(wrap_text_to_width("anything", 0).is_empty());
        assert_eq!(
            wrap_text_to_width("ab", 2),
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(wrap_text_to_width("   ", 10).is_empty());
    }

    #[hegel::composite]
    fn any_text(tc: &hegel::TestCase) -> String {
        // a mix of narrow, wide, zero-width and multibyte-narrow characters plus spaces
        let alphabet = vec![
            'a', 'b', ' ', 'é', '日', '本', '🦀', '\u{301}', 'ß', 'z', ' ',
        ];
        let len = tc.draw(generators::integers::<usize>().max_value(24));
        (0..len)
            .map(|_| tc.draw(generators::sampled_from(alphabet.clone())))
            .collect()
    }

    #[hegel::test]
    fn truncation_never_exceeds_its_column_budget(tc: hegel::TestCase) {
        let text = tc.draw(any_text());
        let max_width = tc.draw(generators::integers::<usize>().max_value(20));
        for (name, truncated) in [
            ("start", truncate_with_ellipsis(&text, max_width)),
            ("end", truncate_with_ellipsis_start(&text, max_width)),
        ] {
            assert!(
                truncated.width() <= max_width,
                "{name}: {truncated:?} is {} columns, budget {max_width}, from {text:?}",
                truncated.width()
            );
            if text.width() <= max_width {
                assert_eq!(
                    truncated, text,
                    "{name}: text that fits must come back verbatim"
                );
            } else if max_width > ELLIPSIS.len() {
                assert!(
                    truncated.contains(ELLIPSIS),
                    "{name}: a cut with room for it carries the ellipsis: {truncated:?}"
                );
            }
        }
    }

    #[hegel::test]
    fn wrapped_lines_fit_and_keep_every_word(tc: hegel::TestCase) {
        let text = tc.draw(any_text());
        let max_width = tc.draw(generators::integers::<usize>().min_value(3).max_value(20));
        let lines = wrap_text_to_width(&text, max_width);
        for line in &lines {
            assert!(
                line.width() <= max_width,
                "{line:?} is {} columns, budget {max_width}, from {text:?}",
                line.width()
            );
        }
        let words: Vec<&str> = text.split_whitespace().collect();
        if words.iter().all(|w| w.width() <= max_width) {
            let rejoined: Vec<&str> = lines.iter().flat_map(|l| l.split(' ')).collect();
            assert_eq!(rejoined, words, "wrapping must keep every word in order");
        }
    }
}
