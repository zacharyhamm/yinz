//! Toast notifications: short, self-dismissing messages drawn in a corner of the viewport, over
//! whatever happens to be there.
//!
//! Notifications are composited by [`crate::screen::Screen::render_to_clients`] after every pane
//! (tiled, floating and global) has rendered, so they always paint on top - see
//! `render_notifications` there.

use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;
use zellij_utils::data::Style;
use zellij_utils::input::options::NotificationPosition;
use zellij_utils::pane_size::Viewport;

use crate::output::CharacterChunk;
use crate::panes::terminal_character::{
    AnsiCode, RcCharacterStyles, TerminalCharacter, RESET_STYLES,
};

/// The top and bottom border. A toast is this plus however many rows its text wrapped into.
pub const NOTIFICATION_BORDER_ROWS: usize = 2;
/// The most text rows a toast grows to before the rest is truncated away.
///
/// A toast is an interruption, not a pager: past a handful of rows the message belongs in a pane.
/// Six keeps the tallest toast (eight rows with borders) inside even a short viewport.
pub const MAX_NOTIFICATION_TEXT_ROWS: usize = 6;
/// The widest a toast gets - borders and padding included - before its text is wrapped.
const MAX_NOTIFICATION_COLS: usize = 50;
/// One column of breathing room between the toast and the viewport edge it's anchored to.
const EDGE_MARGIN: usize = 1;
/// 2 border columns + 2 padding columns. The one place this arithmetic lives: callers pass the
/// whole box width around and let this module work out how much of it is text.
const CHROME_COLS: usize = 4;

/// How tall the box for `text_rows` rows of text is.
///
/// Trivial, but the height is computed by the caller (which wraps) and consumed by
/// [`notification_origins`] (which stacks), and those two must not drift.
pub fn notification_height(text_rows: usize) -> usize {
    text_rows + NOTIFICATION_BORDER_ROWS
}

/// How wide the toast for `text` should be, borders and padding included, clamped to what
/// `viewport_cols` can hold. Returns `None` if the viewport is too narrow for even a minimal box.
///
/// The natural width is the widest of the text's own `\n`-separated segments rather than the width
/// of the whole string: a multi-line message is never laid out end to end, so measuring it that way
/// would ask for a box far wider than any row will fill.
pub fn notification_width(text: &str, viewport_cols: usize) -> Option<usize> {
    let smallest = CHROME_COLS + 1;
    let available = viewport_cols.checked_sub(EDGE_MARGIN * 2)?;
    if available < smallest {
        return None;
    }
    // `split('\n')` rather than `lines()`: the latter drops a trailing empty segment, and the
    // wrapping below has to split the same way for the width and the rows to agree.
    let natural = text
        .split('\n')
        .map(|segment| sanitize(segment).width())
        .max()
        .unwrap_or(0)
        .saturating_add(CHROME_COLS);
    Some(natural.clamp(smallest, available.min(MAX_NOTIFICATION_COLS)))
}

/// The text rows for `text` inside a box `width` columns wide.
///
/// Wrapping lives out here rather than inside [`notification_chunks`] because the caller needs the
/// row count to know how tall the box is *before* anything is painted. Handing the rows back to
/// `notification_chunks` - rather than the string - is what stops the two from wrapping
/// independently and disagreeing about where the box ends.
///
/// An explicit `\n` starts a new row and leading indentation survives, including on the
/// continuation rows of a wrapped line, so a caller can send an indented `key: value` block and get
/// it back looking like one. Words wrap on whitespace; a word too wide for a row is broken at a
/// column boundary rather than allowed to straddle the border. Past
/// [`MAX_NOTIFICATION_TEXT_ROWS`] the last row ends in an ellipsis and the rest is dropped.
///
/// Never returns an empty `Vec` - empty text is one empty row, which paints the same box a
/// one-line message does.
pub fn notification_lines(text: &str, width: usize) -> Vec<String> {
    let text_width = width.saturating_sub(CHROME_COLS);
    // `notification_width` never returns a box this small, but a caller passing a raw width must
    // not be able to make the wrapping below loop forever.
    if text_width == 0 {
        return vec![String::new()];
    }

    let mut rows: Vec<String> = Vec::new();
    let mut overflowed = false;
    'segments: for segment in text.split('\n') {
        for row in wrap_segment(&sanitize(segment), text_width) {
            if rows.len() == MAX_NOTIFICATION_TEXT_ROWS {
                overflowed = true;
                break 'segments;
            }
            rows.push(row);
        }
    }

    if overflowed {
        if let Some(last) = rows.last_mut() {
            *last = ellipsize(last, text_width);
        }
    }
    if rows.is_empty() {
        rows.push(String::new());
    }
    rows
}

/// Control characters are dropped: a toast renders text a plugin or a CLI caller chose, and an
/// escape sequence in it must not be able to repaint the session. A bare carriage return is the
/// dangerous one - it would send the rest of the chunk back over the box's own border. Tabs become
/// four spaces, since a tab measures zero columns here but eight on screen.
fn sanitize(segment: &str) -> String {
    let mut sanitized = String::with_capacity(segment.len());
    for ch in segment.chars() {
        match ch {
            '\t' => sanitized.push_str("    "),
            ch if ch.is_control() => {},
            ch => sanitized.push(ch),
        }
    }
    sanitized
}

/// One `\n`-free segment as the rows it occupies. Always at least one row.
fn wrap_segment(segment: &str, text_width: usize) -> Vec<String> {
    if segment.width() <= text_width {
        return vec![segment.to_owned()];
    }

    let lead = &segment[..segment.len() - segment.trim_start().len()];
    // Continuation rows are re-indented to match, so a wrapped `  key: value` still reads as one
    // indented item. Clamped to half the row: a pathological indent must still leave room for text.
    let indent_width = lead.width().min(text_width / 2);
    let indent = " ".repeat(indent_width);
    // The first row keeps the real indentation, unless it alone would fill the row.
    let mut row = if lead.width() < text_width {
        lead.to_owned()
    } else {
        String::new()
    };
    let mut row_width = row.width();
    let mut has_word = false;
    let mut rows = Vec::new();

    for word in segment.split_whitespace() {
        let word_width = word.width();
        let separator = usize::from(has_word);
        if row_width + separator + word_width <= text_width {
            if has_word {
                row.push(' ');
                row_width += 1;
            }
            row.push_str(word);
            row_width += word_width;
            has_word = true;
            continue;
        }
        if has_word {
            rows.push(std::mem::replace(&mut row, indent.clone()));
            row_width = indent_width;
            has_word = false;
        }
        // The row is bare indentation now, so anything that still doesn't fit never will.
        for (index, piece) in hard_split(word, text_width - indent_width)
            .into_iter()
            .enumerate()
        {
            if index > 0 {
                rows.push(std::mem::replace(&mut row, indent.clone()));
                row_width = indent_width;
            }
            row_width += piece.width();
            row.push_str(&piece);
            has_word = true;
        }
    }

    if has_word || rows.is_empty() {
        rows.push(row);
    }
    rows
}

/// A word wider than a row, cut at column boundaries. Breaking *before* the character that would
/// overflow is what stops a double-width glyph from straddling the border.
fn hard_split(word: &str, budget: usize) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut piece = String::new();
    let mut used = 0;
    for ch in word.chars() {
        let ch_width = ch.width().unwrap_or(0).max(1);
        if used + ch_width > budget && !piece.is_empty() {
            pieces.push(std::mem::take(&mut piece));
            used = 0;
        }
        piece.push(ch);
        used += ch_width;
    }
    if !piece.is_empty() {
        pieces.push(piece);
    }
    pieces
}

/// `row` cut to leave a column for the ellipsis that replaces what was dropped. At a one-column row
/// this is the ellipsis alone, which still fits.
fn ellipsize(row: &str, text_width: usize) -> String {
    let budget = text_width.saturating_sub(1);
    let mut truncated = String::new();
    let mut used = 0;
    for ch in row.chars() {
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

/// The top-left corner of each visible notification, index 0 nearest `position`'s corner.
///
/// Stacks grow away from the edge they're anchored to: downwards from a top corner, upwards from a
/// bottom one. `heights` is each notification's own height, newest first - toasts are no longer all
/// the same height, so a bottom-anchored one is placed by its *bottom* edge and its origin moves up
/// by its own height as well as by everything already stacked.
///
/// Notifications that would not fit inside `viewport` are dropped, so the result may be shorter
/// than `heights`. The first that doesn't fit ends the stack rather than being skipped over: the
/// stack has to stay contiguous and in arrival order, and letting a short toast leapfrog a dropped
/// tall one would make what's on screen stop matching what arrived. One consequence worth naming:
/// a newest toast taller than the whole viewport hides the ones behind it, which takes a viewport
/// under eight rows and degrades to the same nothing-fits-nothing-shows this has always done.
pub fn notification_origins(
    viewport: Viewport,
    position: NotificationPosition,
    heights: &[usize],
    width: usize,
) -> Vec<(usize, usize)> {
    if width + EDGE_MARGIN * 2 > viewport.cols {
        return vec![];
    }
    let x = if position.is_left() {
        viewport.x + EDGE_MARGIN
    } else {
        viewport.x + viewport.cols - width - EDGE_MARGIN
    };
    let mut used = 0;
    let mut origins = Vec::with_capacity(heights.len());
    for height in heights {
        if used + height > viewport.rows {
            break;
        }
        let y = if position.is_top() {
            viewport.y + used
        } else {
            viewport.y + viewport.rows - used - height
        };
        origins.push((x, y));
        used += height;
    }
    origins
}

/// The chunks for one toast, its top-left corner at (`x`, `y`) and `width` columns wide.
///
/// `lines` comes from [`notification_lines`] called with the same `width`.
pub fn notification_chunks(
    lines: &[String],
    x: usize,
    y: usize,
    width: usize,
    style: &Style,
) -> Vec<CharacterChunk> {
    let declaration = style.colors.text_unselected;
    let fill: RcCharacterStyles = RESET_STYLES
        .foreground(Some(AnsiCode::from(declaration.base)))
        .background(Some(AnsiCode::from(declaration.background)))
        .into();
    let border: RcCharacterStyles = RESET_STYLES
        .foreground(Some(AnsiCode::from(declaration.emphasis_0)))
        .background(Some(AnsiCode::from(declaration.background)))
        .into();

    let inner_width = width.saturating_sub(2);
    let border_row = |corner_left: char, corner_right: char| -> Vec<TerminalCharacter> {
        let mut characters = Vec::with_capacity(width);
        characters.push(TerminalCharacter::new_styled(corner_left, border.clone()));
        for _ in 0..inner_width {
            characters.push(TerminalCharacter::new_styled('─', border.clone()));
        }
        characters.push(TerminalCharacter::new_styled(corner_right, border.clone()));
        characters
    };

    let text_width = inner_width.saturating_sub(2);
    let text_row = |line: &str| -> Vec<TerminalCharacter> {
        let mut characters = Vec::with_capacity(width);
        characters.push(TerminalCharacter::new_styled('│', border.clone()));
        characters.push(TerminalCharacter::new_singlewidth_styled(' ', fill.clone()));
        let mut used = 0;
        for character in line.chars() {
            let character_width = character.width().unwrap_or(0).max(1);
            // `notification_lines` has already fitted the row; this only stops a caller that
            // wrapped to a different width from painting over the border.
            if used + character_width > text_width {
                break;
            }
            characters.push(TerminalCharacter::new_styled(character, fill.clone()));
            used += character_width;
        }
        while used < text_width {
            characters.push(TerminalCharacter::new_singlewidth_styled(' ', fill.clone()));
            used += 1;
        }
        characters.push(TerminalCharacter::new_singlewidth_styled(' ', fill.clone()));
        characters.push(TerminalCharacter::new_styled('│', border.clone()));
        characters
    };

    let mut chunks = Vec::with_capacity(lines.len() + NOTIFICATION_BORDER_ROWS);
    chunks.push(CharacterChunk::new(border_row('╭', '╮'), x, y));
    for (offset, line) in lines.iter().enumerate() {
        chunks.push(CharacterChunk::new(text_row(line), x, y + 1 + offset));
    }
    chunks.push(CharacterChunk::new(
        border_row('╰', '╯'),
        x,
        y + 1 + lines.len(),
    ));
    chunks
}

/// A smoothstep curve: eases in and out rather than moving at a constant rate. Used for both the
/// slide reveal and the displacement lerp so neither one looks mechanical.
///
/// `progress` is clamped to `[0.0, 1.0]` first, so a caller that overshoots (the last tick of an
/// animation commonly does) gets a safe `0.0`/`1.0` rather than a curve value outside that range.
pub fn ease(progress: f32) -> f32 {
    let t = progress.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `chunks` (as produced by [`notification_chunks`] for a box `width` columns wide with its
/// top-left corner at `x`) trimmed to the columns visible at slide `progress` - `0.0` is fully
/// hidden at the screen edge `position` is anchored to, `1.0` is fully in place.
///
/// The box's edge nearest that anchored corner stays fixed; the far edge is what moves, so a
/// left-anchored toast grows rightward from a fixed `x` and a right-anchored one grows leftward
/// from a fixed right edge, with `x` itself increasing as less of the box is hidden. Every
/// coordinate this produces stays inside `[x, x + width)`, so - unlike a literal slide off the
/// left of the screen - nothing here ever needs a negative column.
///
/// Returns the trimmed chunks and the `x` they should be painted at (unchanged from the input for
/// a left-anchored toast; increasing towards the input `x` for a right-anchored one).
pub fn notification_reveal(
    chunks: &[CharacterChunk],
    x: usize,
    width: usize,
    position: NotificationPosition,
    progress: f32,
) -> (Vec<CharacterChunk>, usize) {
    let (revealed_x, visible_cols) = notification_reveal_bounds(x, width, position, progress);
    let hidden_cols = width - visible_cols;

    let trimmed = chunks
        .iter()
        .map(|chunk| {
            let characters = if position.is_left() {
                take_columns(&chunk.terminal_characters, visible_cols)
            } else {
                drop_columns(&chunk.terminal_characters, hidden_cols)
            };
            CharacterChunk::new(characters, revealed_x, chunk.y)
        })
        .collect();
    (trimmed, revealed_x)
}

/// The horizontal bounds painted by [`notification_reveal`].
pub fn notification_reveal_bounds(
    x: usize,
    width: usize,
    position: NotificationPosition,
    progress: f32,
) -> (usize, usize) {
    let visible_cols = (((width as f32) * ease(progress)).round() as usize).min(width);
    let revealed_x = if position.is_left() {
        x
    } else {
        x + width - visible_cols
    };
    (revealed_x, visible_cols)
}

/// The leading `keep_cols` display columns of `characters`. Walks by [`TerminalCharacter::width`]
/// rather than by index, so a wide glyph is never split in half - it is kept whole if it fits
/// entirely within `keep_cols`, dropped otherwise.
fn take_columns(characters: &[TerminalCharacter], keep_cols: usize) -> Vec<TerminalCharacter> {
    let mut used = 0;
    let mut kept = Vec::new();
    for character in characters {
        let character_width = character.width();
        if used + character_width > keep_cols {
            break;
        }
        kept.push(character.clone());
        used += character_width;
    }
    kept
}

/// `characters` with the leading `drop_cols` display columns removed, same whole-glyph rule as
/// [`take_columns`].
fn drop_columns(characters: &[TerminalCharacter], drop_cols: usize) -> Vec<TerminalCharacter> {
    let mut used = 0;
    let mut start = 0;
    for (index, character) in characters.iter().enumerate() {
        if used >= drop_cols {
            break;
        }
        used += character.width();
        start = index + 1;
    }
    characters[start..].to_vec()
}

/// `before` lerped towards `after` by `progress` (`0.0` stays at `before`, `1.0` reaches `after`),
/// eased and rounded to the nearest cell.
pub fn lerp_origin(before: (usize, usize), after: (usize, usize), progress: f32) -> (usize, usize) {
    let t = ease(progress);
    let lerp = |from: usize, to: usize| -> usize {
        let from = from as f32;
        let to = to as f32;
        (from + (to - from) * t).round() as usize
    };
    (lerp(before.0, after.0), lerp(before.1, after.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewport() -> Viewport {
        Viewport {
            x: 10,
            y: 5,
            rows: 30,
            cols: 80,
        }
    }

    fn rows(chunks: &[CharacterChunk]) -> Vec<String> {
        chunks
            .iter()
            .map(|chunk| {
                chunk
                    .terminal_characters
                    .iter()
                    .map(|c| c.character)
                    .collect()
            })
            .collect()
    }

    #[test]
    fn notifications_stack_away_from_the_edge_they_are_anchored_to() {
        let viewport = viewport();
        let width = 20;
        for position in [
            NotificationPosition::TopLeft,
            NotificationPosition::TopRight,
            NotificationPosition::BottomLeft,
            NotificationPosition::BottomRight,
        ] {
            let origins = notification_origins(viewport, position, &[3, 3, 3], width);
            assert_eq!(origins.len(), 3, "{:?}", position);

            let expected_x = if position.is_left() {
                viewport.x + EDGE_MARGIN
            } else {
                viewport.x + viewport.cols - width - EDGE_MARGIN
            };
            assert!(
                origins.iter().all(|(x, _)| *x == expected_x),
                "{:?} should pin every toast to the same column",
                position
            );

            if position.is_top() {
                assert_eq!(origins[0].1, viewport.y, "{:?}", position);
                assert!(
                    origins[0].1 < origins[1].1 && origins[1].1 < origins[2].1,
                    "{:?} should grow downwards",
                    position
                );
            } else {
                assert_eq!(
                    origins[0].1,
                    viewport.y + viewport.rows - 3,
                    "{:?}",
                    position
                );
                assert!(
                    origins[0].1 > origins[1].1 && origins[1].1 > origins[2].1,
                    "{:?} should grow upwards",
                    position
                );
            }
        }
    }

    #[test]
    fn notifications_that_do_not_fit_are_dropped_rather_than_wrapping_around() {
        let short = Viewport {
            x: 0,
            y: 0,
            rows: 7, // room for 2 toasts, not 5
            cols: 80,
        };
        let origins = notification_origins(short, NotificationPosition::TopLeft, &[3; 5], 20);
        assert_eq!(origins.len(), 2);

        let narrow = Viewport {
            x: 0,
            y: 0,
            rows: 30,
            cols: 10,
        };
        assert!(notification_origins(narrow, NotificationPosition::TopLeft, &[3], 20).is_empty());
    }

    /// Heights differ now, so the stack is an accumulation rather than a multiplication.
    #[test]
    fn toasts_of_different_heights_stack_without_overlapping_or_gaps() {
        let viewport = viewport();
        let heights = [5, 3, 4];

        let top = notification_origins(viewport, NotificationPosition::TopLeft, &heights, 20);
        assert_eq!(top.len(), 3);
        for (index, height) in heights.iter().enumerate().take(2) {
            assert_eq!(
                top[index].1 + height,
                top[index + 1].1,
                "toast {index} should end exactly where the next one starts"
            );
        }

        let bottom = notification_origins(viewport, NotificationPosition::BottomLeft, &heights, 20);
        assert_eq!(bottom.len(), 3);
        for (index, _) in heights.iter().enumerate().take(2) {
            assert_eq!(
                bottom[index + 1].1 + heights[index + 1],
                bottom[index].1,
                "toast {index} should start exactly where the next one ends"
            );
        }
    }

    /// The easy one to get wrong: a taller toast at a bottom corner grows upwards, so its own
    /// height moves its origin rather than leaving it hanging off the bottom.
    #[test]
    fn a_bottom_anchored_toast_is_positioned_by_its_bottom_edge() {
        let viewport = viewport();
        for height in [3, 5, 8] {
            let origins =
                notification_origins(viewport, NotificationPosition::BottomRight, &[height], 20);
            assert_eq!(
                origins[0].1 + height,
                viewport.y + viewport.rows,
                "a {height}-row toast should end on the last viewport row"
            );
        }
    }

    #[test]
    fn a_toast_taller_than_the_viewport_is_dropped() {
        let short = Viewport {
            x: 0,
            y: 0,
            rows: 6,
            cols: 80,
        };
        assert!(notification_origins(short, NotificationPosition::TopLeft, &[8], 20).is_empty());
    }

    #[test]
    fn width_is_clamped_to_the_viewport_and_the_maximum() {
        assert_eq!(notification_width("hello", 80), Some(9));
        assert_eq!(
            notification_width(&"x".repeat(200), 80),
            Some(MAX_NOTIFICATION_COLS)
        );
        // a viewport narrower than the maximum clamps to what it can hold
        assert_eq!(notification_width(&"x".repeat(200), 20), Some(18));
        assert_eq!(notification_width("hello", 3), None);
    }

    /// Measuring the whole string would ask for a box as wide as every line laid end to end.
    #[test]
    fn width_is_the_widest_explicit_line_rather_than_the_whole_string() {
        assert_eq!(notification_width("hello\nhi\nthere", 80), Some(9));
    }

    #[test]
    fn a_short_single_line_message_wraps_to_exactly_one_row() {
        assert_eq!(
            notification_lines("build finished", 40),
            vec!["build finished"]
        );
    }

    #[test]
    fn explicit_newlines_start_a_new_row() {
        assert_eq!(
            notification_lines("hocus needs confirmation\nsend_input", 40),
            vec!["hocus needs confirmation", "send_input"]
        );
    }

    #[test]
    fn a_blank_line_between_paragraphs_is_kept_as_a_blank_row() {
        assert_eq!(notification_lines("one\n\ntwo", 40), vec!["one", "", "two"]);
    }

    #[test]
    fn words_wrap_on_whitespace_rather_than_mid_word() {
        // 14 columns of text: "the quick" fits, "brown" does not.
        assert_eq!(
            notification_lines("the quick brown fox", 18),
            vec!["the quick", "brown fox"]
        );
    }

    #[test]
    fn leading_indentation_survives_wrapping() {
        let lines = notification_lines("  text: git push --force --all", 18);
        assert_eq!(lines, vec!["  text: git", "  push --force", "  --all"]);
    }

    #[test]
    fn a_word_longer_than_a_row_is_hard_broken() {
        assert_eq!(
            notification_lines(&"x".repeat(20), 12),
            vec!["xxxxxxxx", "xxxxxxxx", "xxxx"]
        );
    }

    /// A double-width glyph that straddled the border would paint over the box's own frame.
    #[test]
    fn wide_characters_never_straddle_the_box_border() {
        // 7 columns of text, glyphs 2 columns each: three fit, the fourth starts a row.
        let lines = notification_lines(&"日".repeat(4), 11);
        assert_eq!(lines, vec!["日日日", "日"]);
        for line in &lines {
            assert!(line.width() <= 7, "{line:?} overflows the row");
        }
    }

    #[test]
    fn text_past_the_row_cap_is_truncated_with_an_ellipsis_on_the_last_row() {
        let many = (0..MAX_NOTIFICATION_TEXT_ROWS + 4)
            .map(|index| format!("row {index}"))
            .collect::<Vec<_>>()
            .join("\n");

        let lines = notification_lines(&many, 20);

        assert_eq!(lines.len(), MAX_NOTIFICATION_TEXT_ROWS);
        assert!(
            lines
                .last()
                .expect("capped at a non-zero cap")
                .ends_with('…'),
            "got {:?}",
            lines.last()
        );
    }

    /// Truncate-then-append is what makes the narrowest possible row work without a special case.
    #[test]
    fn the_ellipsis_still_fits_a_one_column_row() {
        let many = "a\n".repeat(MAX_NOTIFICATION_TEXT_ROWS + 2);
        let lines = notification_lines(&many, CHROME_COLS + 1);
        assert_eq!(lines.last().map(String::as_str), Some("…"));
    }

    #[test]
    fn control_characters_are_dropped_so_a_message_cannot_repaint_the_session() {
        assert_eq!(
            notification_lines("safe\u{1b}[2Jtext\rmore", 40),
            vec!["safe[2Jtextmore"]
        );
    }

    #[test]
    fn a_single_line_toast_is_still_three_rows_of_exactly_the_requested_width() {
        let style = Style::default();
        let chunks = notification_chunks(&["hello".to_owned()], 4, 9, 20, &style);
        assert_eq!(chunks.len(), notification_height(1));
        for (offset, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.x, 4);
            assert_eq!(chunk.y, 9 + offset);
            assert_eq!(
                chunk
                    .terminal_characters
                    .iter()
                    .map(|c| c.width())
                    .sum::<usize>(),
                20,
                "row {} should span the full width",
                offset
            );
        }
    }

    #[test]
    fn every_row_of_a_multi_line_toast_spans_the_full_width() {
        let style = Style::default();
        let lines = notification_lines("one\ntwo three\n  four", 20);
        let chunks = notification_chunks(&lines, 0, 0, 20, &style);

        assert_eq!(chunks.len(), notification_height(lines.len()));
        for (offset, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.y, offset, "rows should be contiguous");
            assert_eq!(
                chunk
                    .terminal_characters
                    .iter()
                    .map(|c| c.width())
                    .sum::<usize>(),
                20,
                "row {offset} should span the full width"
            );
        }
    }

    /// The shape this whole change exists for: hocus's confirmation block, rendered as written.
    #[test]
    fn the_hocus_confirmation_block_renders_as_written() {
        let style = Style::default();
        let text = "hocus needs confirmation\nsend_input\n  pane_id: 3\n  text: git push";
        let width = notification_width(text, 80).expect("wide enough");
        let lines = notification_lines(text, width);

        assert_eq!(
            lines,
            vec![
                "hocus needs confirmation",
                "send_input",
                "  pane_id: 3",
                "  text: git push",
            ]
        );

        let painted = rows(&notification_chunks(&lines, 0, 0, width, &style));
        assert_eq!(painted.len(), notification_height(4));
        assert!(painted[0].starts_with('╭'), "got {:?}", painted[0]);
        assert_eq!(painted[1], "│ hocus needs confirmation │");
        assert_eq!(painted[3], "│   pane_id: 3             │");
        assert!(painted[5].starts_with('╰'), "got {:?}", painted[5]);
    }

    #[test]
    fn ease_is_identity_at_the_endpoints_and_clamps_outside_them() {
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
        assert_eq!(ease(-0.5), 0.0);
        assert_eq!(ease(1.5), 1.0);
        // eased motion is slower than linear near the endpoints
        assert!(ease(0.1) < 0.1);
        assert!(ease(0.9) > 0.9);
    }

    #[test]
    fn a_fully_hidden_reveal_paints_nothing() {
        let style = Style::default();
        let chunks = notification_chunks(&["hello".to_owned()], 4, 9, 20, &style);
        for position in [
            NotificationPosition::TopLeft,
            NotificationPosition::TopRight,
        ] {
            let (revealed, _) = notification_reveal(&chunks, 4, 20, position, 0.0);
            for chunk in &revealed {
                assert!(
                    chunk.terminal_characters.is_empty(),
                    "{:?} at progress 0.0 should paint nothing",
                    position
                );
            }
        }
    }

    #[test]
    fn a_fully_shown_reveal_is_identical_to_the_unanimated_chunks() {
        let style = Style::default();
        let lines = notification_lines("one\ntwo three\n  four", 20);
        let chunks = notification_chunks(&lines, 4, 9, 20, &style);
        for position in [
            NotificationPosition::TopLeft,
            NotificationPosition::TopRight,
        ] {
            let (revealed, x) = notification_reveal(&chunks, 4, 20, position, 1.0);
            assert_eq!(x, 4);
            assert_eq!(revealed.len(), chunks.len(), "{:?}", position);
            for (revealed_chunk, chunk) in revealed.iter().zip(&chunks) {
                assert_eq!(revealed_chunk.x, chunk.x, "{:?}", position);
                assert_eq!(revealed_chunk.y, chunk.y, "{:?}", position);
                assert_eq!(
                    rows(std::slice::from_ref(revealed_chunk)),
                    rows(std::slice::from_ref(chunk)),
                    "{:?}",
                    position
                );
            }
        }
    }

    #[test]
    fn a_left_anchored_reveal_keeps_x_fixed_and_grows_rightward() {
        let style = Style::default();
        let chunks = notification_chunks(&["hello".to_owned()], 4, 9, 20, &style);
        for progress in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let (revealed, x) =
                notification_reveal(&chunks, 4, 20, NotificationPosition::TopLeft, progress);
            assert_eq!(
                x, 4,
                "left-anchored x should never move, progress {progress}"
            );
            for chunk in &revealed {
                let painted_width: usize =
                    chunk.terminal_characters.iter().map(|c| c.width()).sum();
                assert!(painted_width <= 20);
            }
        }
    }

    #[test]
    fn a_right_anchored_reveal_keeps_the_right_edge_fixed_and_grows_leftward() {
        let style = Style::default();
        let chunks = notification_chunks(&["hello".to_owned()], 4, 9, 20, &style);
        let mut previous_x = None;
        for progress in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let (revealed, x) =
                notification_reveal(&chunks, 4, 20, NotificationPosition::TopRight, progress);
            let painted_width: usize = revealed[0]
                .terminal_characters
                .iter()
                .map(|c| c.width())
                .sum();
            assert_eq!(
                x + painted_width,
                24,
                "the right edge should stay fixed at x + width, progress {progress}"
            );
            if let Some(previous_x) = previous_x {
                assert!(
                    x <= previous_x,
                    "x should shrink towards 4 as progress increases"
                );
            }
            previous_x = Some(x);
        }
        assert_eq!(previous_x, Some(4));
    }

    #[test]
    fn a_reveal_never_splits_a_wide_glyph() {
        let style = Style::default();
        // 4 double-width glyphs, 8 columns of text, box is CHROME_COLS(4) + 8 = 12 wide
        let lines = notification_lines(&"日".repeat(4), 12);
        let chunks = notification_chunks(&lines, 0, 0, 12, &style);
        for progress in 0..=12 {
            let progress = progress as f32 / 12.0;
            let (revealed, _) =
                notification_reveal(&chunks, 0, 12, NotificationPosition::TopLeft, progress);
            for chunk in &revealed {
                for character in &chunk.terminal_characters {
                    assert!(
                        character.width() == 1 || character.character == '日',
                        "a glyph must never be kept partially, got {:?}",
                        character.character
                    );
                }
                let painted_width: usize =
                    chunk.terminal_characters.iter().map(|c| c.width()).sum();
                assert!(painted_width <= 12);
            }
        }
    }

    #[test]
    fn lerp_origin_reaches_before_and_after_at_the_endpoints() {
        let before = (2, 3);
        let after = (10, 1);
        assert_eq!(lerp_origin(before, after, 0.0), before);
        assert_eq!(lerp_origin(before, after, 1.0), after);
    }

    #[test]
    fn lerp_origin_moves_monotonically_between_endpoints() {
        let before = (2, 20);
        let after = (10, 4);
        let mut previous = before;
        for progress in 1..=10 {
            let progress = progress as f32 / 10.0;
            let current = lerp_origin(before, after, progress);
            assert!(
                current.0 >= previous.0,
                "x should move monotonically towards `after`"
            );
            assert!(
                current.1 <= previous.1,
                "y should move monotonically towards `after`"
            );
            previous = current;
        }
        assert_eq!(previous, after);
    }
}
