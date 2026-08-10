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

/// A toast is three rows: top border, text, bottom border.
pub const NOTIFICATION_ROWS: usize = 3;
/// The widest a toast gets - borders and padding included - before its text is truncated.
const MAX_NOTIFICATION_COLS: usize = 50;
/// One column of breathing room between the toast and the viewport edge it's anchored to.
const EDGE_MARGIN: usize = 1;

/// How wide the toast for `text` should be, borders and padding included, clamped to what
/// `viewport_cols` can hold. Returns `None` if the viewport is too narrow for even a minimal box.
pub fn notification_width(text: &str, viewport_cols: usize) -> Option<usize> {
    // 2 border columns + 2 padding columns
    let chrome = 4;
    let smallest = chrome + 1;
    let available = viewport_cols.checked_sub(EDGE_MARGIN * 2)?;
    if available < smallest {
        return None;
    }
    let natural = text.width().saturating_add(chrome);
    Some(natural.clamp(smallest, available.min(MAX_NOTIFICATION_COLS)))
}

/// The top-left corner of each visible notification, index 0 nearest `position`'s corner.
///
/// Stacks grow away from the edge they're anchored to: downwards from a top corner, upwards from a
/// bottom one. Notifications that would not fit inside `viewport` are dropped, so the result may be
/// shorter than `count`.
pub fn notification_origins(
    viewport: Viewport,
    position: NotificationPosition,
    count: usize,
    width: usize,
) -> Vec<(usize, usize)> {
    if width + EDGE_MARGIN * 2 > viewport.cols {
        return vec![];
    }
    let visible = count.min(viewport.rows / NOTIFICATION_ROWS);
    let x = if position.is_left() {
        viewport.x + EDGE_MARGIN
    } else {
        viewport.x + viewport.cols - width - EDGE_MARGIN
    };
    (0..visible)
        .map(|i| {
            let offset = i * NOTIFICATION_ROWS;
            let y = if position.is_top() {
                viewport.y + offset
            } else {
                viewport.y + viewport.rows - NOTIFICATION_ROWS - offset
            };
            (x, y)
        })
        .collect()
}

/// The chunks for one toast, its top-left corner at (`x`, `y`) and `width` columns wide.
pub fn notification_chunks(
    text: &str,
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

    let mut text_row = Vec::with_capacity(width);
    text_row.push(TerminalCharacter::new_styled('│', border.clone()));
    text_row.push(TerminalCharacter::new_singlewidth_styled(' ', fill.clone()));
    // ponytail: single line, truncated with an ellipsis. Wrap into a taller box if messages turn
    // out to be long in practice.
    let text_width = inner_width.saturating_sub(2);
    let mut used = 0;
    let truncated = text.width() > text_width;
    let budget = if truncated {
        text_width.saturating_sub(1)
    } else {
        text_width
    };
    for character in text.chars() {
        let character_width = character.width().unwrap_or(0).max(1);
        if used + character_width > budget {
            break;
        }
        text_row.push(TerminalCharacter::new_styled(character, fill.clone()));
        used += character_width;
    }
    if truncated {
        text_row.push(TerminalCharacter::new_styled('…', fill.clone()));
        used += 1;
    }
    while used < text_width {
        text_row.push(TerminalCharacter::new_singlewidth_styled(' ', fill.clone()));
        used += 1;
    }
    text_row.push(TerminalCharacter::new_singlewidth_styled(' ', fill.clone()));
    text_row.push(TerminalCharacter::new_styled('│', border.clone()));

    vec![
        CharacterChunk::new(border_row('╭', '╮'), x, y),
        CharacterChunk::new(text_row, x, y + 1),
        CharacterChunk::new(border_row('╰', '╯'), x, y + 2),
    ]
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
            let origins = notification_origins(viewport, position, 3, width);
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
                    viewport.y + viewport.rows - NOTIFICATION_ROWS,
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
        let origins = notification_origins(short, NotificationPosition::TopLeft, 5, 20);
        assert_eq!(origins.len(), 2);

        let narrow = Viewport {
            x: 0,
            y: 0,
            rows: 30,
            cols: 10,
        };
        assert!(notification_origins(narrow, NotificationPosition::TopLeft, 1, 20).is_empty());
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

    #[test]
    fn chunks_are_three_rows_of_exactly_the_requested_width() {
        let style = Style::default();
        let chunks = notification_chunks("hello", 4, 9, 20, &style);
        assert_eq!(chunks.len(), NOTIFICATION_ROWS);
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
    fn long_text_is_truncated_with_an_ellipsis_and_still_fits() {
        let style = Style::default();
        let chunks = notification_chunks("a message far too long for this box", 0, 0, 20, &style);
        let text_row: String = chunks[1]
            .terminal_characters
            .iter()
            .map(|c| c.character)
            .collect();
        assert_eq!(text_row.width(), 20);
        assert!(text_row.contains('…'), "got {:?}", text_row);
    }
}
