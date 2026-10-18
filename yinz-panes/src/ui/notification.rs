//! Toast notifications: short, self-dismissing messages drawn in a corner of the viewport, over
//! whatever happens to be there.
//!
//! Notifications are composited by `Screen::render_to_clients` in yinz-screen after every pane
//! (tiled and floating) has rendered, so they always paint on top - see `render_notifications`
//! there.

use std::time::Duration;
use yinz_types::internal_ids::NotificationId;

use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;
use yinz_types::animation::{blend, Easing};
use yinz_types::input::options::NotificationPosition;
use yinz_types::pane_size::Viewport;
use yinz_types::style::{Style, StyleDeclaration};

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
    notification_lines_capped(text, width, MAX_NOTIFICATION_TEXT_ROWS)
}

/// Wrap plain text to an exact content width without truncating rows.
pub fn wrapped_text_lines(text: &str, width: usize) -> Vec<String> {
    notification_lines_capped(text, width.saturating_add(CHROME_COLS), usize::MAX)
}

/// [`notification_lines`] with a caller-chosen row cap instead of [`MAX_NOTIFICATION_TEXT_ROWS`].
///
/// The notification browser uses it both ways: a collapsed entry is cut shorter than a toast, and
/// the entry whose full text was asked for is allowed as many rows as the viewport holds. A cap of
/// zero is treated as one, so the result is never empty.
pub fn notification_lines_capped(text: &str, width: usize, max_rows: usize) -> Vec<String> {
    let max_rows = max_rows.max(1);
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
            if rows.len() == max_rows {
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

/// How far toast `id`'s settled row moved when the stack went from `before` to `after`, in rows.
///
/// Positive means it moved *down* the screen. Returns `None` for a toast that is not in both
/// stacks - a new arrival has nowhere to have moved from.
///
/// Both stacks are `(id, height)` in stacking order, nearest the anchored corner first, each
/// measured at *its own* shared width: a wide toast arriving re-wraps every other toast and can
/// change their heights, and that is precisely the case this exists to handle.
///
/// The viewport cancels out, which is the point. Comparing [`notification_origins`] outputs
/// directly would tie the answer to one client's viewport, but every viewport-dependent term -
/// `viewport.y`, `viewport.rows` - is identical on both sides of the subtraction, so only the
/// cumulative heights survive. That is what lets one displacement be shared by clients whose
/// viewports differ. The two anchors measure from opposite edges, so they subtract differently:
/// a top-anchored toast is placed by its top edge and a bottom-anchored one by its bottom.
pub fn settled_row_delta(
    position: NotificationPosition,
    before: &[(NotificationId, usize)],
    after: &[(NotificationId, usize)],
    id: NotificationId,
) -> Option<isize> {
    // distance from the anchored edge to this toast's own anchored edge
    let distance = |stack: &[(NotificationId, usize)]| -> Option<isize> {
        let mut used = 0usize;
        for (candidate, height) in stack {
            if *candidate == id {
                let edge = if position.is_top() {
                    used
                } else {
                    used + height
                };
                return isize::try_from(edge).ok();
            }
            used += height;
        }
        None
    };
    let before = distance(before)?;
    let after = distance(after)?;
    // a top-anchored stack grows downwards, so a larger distance is a larger row; a bottom-anchored
    // one grows upwards, so a larger distance is a smaller row
    Some(if position.is_top() {
        after - before
    } else {
        before - after
    })
}

/// The most text rows a collapsed entry in the notification browser shows before it is elided.
///
/// Shorter than a toast's cap: the browser is a list to scan, and the full text of any one entry
/// is a keypress away.
pub const BROWSER_COLLAPSED_TEXT_ROWS: usize = 2;

/// One entry of the notification browser as it lands on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct BrowserEntryLayout {
    /// Index into the entries the layout was given.
    pub index: usize,
    pub lines: Vec<String>,
    pub x: usize,
    pub y: usize,
}

/// The notification browser laid out over `viewport`: which entries are visible, where, and at
/// what shared width.
#[derive(Debug, Clone, PartialEq)]
pub struct BrowserLayout {
    pub width: usize,
    /// The first entry shown. The caller keeps it and hands it back as the next `first` hint, so
    /// the list only scrolls when the selection would otherwise leave the screen.
    pub first: usize,
    pub entries: Vec<BrowserEntryLayout>,
}

/// Lay out the notification browser: `texts` newest first, stacked from `position`'s corner the
/// same way the toasts stack.
///
/// Every entry is cut to [`BROWSER_COLLAPSED_TEXT_ROWS`] except the selected one when `expanded`,
/// which gets as many rows as the viewport can hold. The window starts at `first` if the selection
/// fits from there, and otherwise moves the least distance that brings the whole selected box into
/// view. Returns `None` when the viewport cannot hold even a minimal box.
pub fn notification_browser_layout(
    viewport: Viewport,
    position: NotificationPosition,
    texts: &[&str],
    selected: usize,
    expanded: bool,
    first: usize,
) -> Option<BrowserLayout> {
    let width = texts
        .iter()
        .filter_map(|text| notification_width(text, viewport.cols))
        .max()?;
    let selected = selected.min(texts.len().saturating_sub(1));
    let expanded_rows = viewport.rows.saturating_sub(NOTIFICATION_BORDER_ROWS);
    let lines: Vec<Vec<String>> = texts
        .iter()
        .enumerate()
        .map(|(index, text)| {
            let max_rows = if expanded && index == selected {
                expanded_rows
            } else {
                BROWSER_COLLAPSED_TEXT_ROWS
            };
            notification_lines_capped(text, width, max_rows)
        })
        .collect();
    let heights: Vec<usize> = lines
        .iter()
        .map(|lines| notification_height(lines.len()))
        .collect();
    let first = browser_first_visible(&heights, viewport.rows, first, selected);
    let origins = notification_origins(viewport, position, &heights[first..], width);
    let entries = origins
        .into_iter()
        .enumerate()
        .map(|(offset, (x, y))| BrowserEntryLayout {
            index: first + offset,
            lines: lines[first + offset].clone(),
            x,
            y,
        })
        .collect();
    Some(BrowserLayout {
        width,
        first,
        entries,
    })
}

/// The first entry to show so that `selected` is wholly visible in `rows`, starting from `hint`.
///
/// Scrolls towards the selection only as far as needed: back to it when it sits before the
/// window, forward one entry at a time while it hangs off the far end. A selected box taller than
/// the viewport is shown from its own top edge.
fn browser_first_visible(heights: &[usize], rows: usize, hint: usize, selected: usize) -> usize {
    if heights.is_empty() {
        return 0;
    }
    let mut first = hint.min(selected);
    while first < selected && heights[first..=selected].iter().sum::<usize>() > rows {
        first += 1;
    }
    first
}

/// The chunks for one toast, its top-left corner at (`x`, `y`) and `width` columns wide.
///
/// `lines` comes from [`notification_lines`] called with the same `width`. An optional footer
/// adds one dim row, truncated to the content width without reducing the message row limit.
pub fn notification_chunks(
    lines: &[String],
    footer: Option<&str>,
    x: usize,
    y: usize,
    width: usize,
    style: &Style,
    fade: f32,
) -> Vec<CharacterChunk> {
    notification_box_chunks(
        lines,
        footer,
        x,
        y,
        width,
        style.colors.text_unselected,
        fade,
    )
}

/// [`notification_chunks`] painted in an explicit colour declaration, so the notification browser
/// can draw its selected entry in `text_selected` with the same box everything else uses.
pub fn notification_box_chunks(
    lines: &[String],
    footer: Option<&str>,
    x: usize,
    y: usize,
    width: usize,
    declaration: StyleDeclaration,
    fade: f32,
) -> Vec<CharacterChunk> {
    // The box paints as a solid slab of its own background from its very first frame, and its
    // border and text resolve out of that background as `fade` climbs - so a toast materialises
    // rather than snapping into existence, and dissolves rather than blinking out.
    //
    // Fading towards the box's own background rather than towards what is behind it is not a
    // shortcut, it is the only thing expressible: the server keeps no framebuffer of the panes a
    // toast covers, and there is no session background colour distinct from this one. What this
    // does *not* do is smooth the vertical glide, whose cells sit over content nothing here knows.
    //
    // At `fade == 1.0` `blend` hands back the original `PaletteColor` untouched, so a settled
    // toast is byte-identical to an unanimated one and a customised terminal palette is only ever
    // approximated during the handful of frames something is actually moving.
    let text_color = blend(declaration.background, declaration.base, fade);
    let border_color = blend(declaration.background, declaration.emphasis_0, fade);
    let fill: RcCharacterStyles = RESET_STYLES
        .foreground(Some(AnsiCode::from(text_color)))
        .background(Some(AnsiCode::from(declaration.background)))
        .into();
    let border: RcCharacterStyles = RESET_STYLES
        .foreground(Some(AnsiCode::from(border_color)))
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
    let text_row = |line: &str, fill: &RcCharacterStyles| -> Vec<TerminalCharacter> {
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

    let mut chunks =
        Vec::with_capacity(lines.len() + usize::from(footer.is_some()) + NOTIFICATION_BORDER_ROWS);
    chunks.push(CharacterChunk::new(border_row('╭', '╮'), x, y));
    for (offset, line) in lines.iter().enumerate() {
        chunks.push(CharacterChunk::new(
            text_row(line, &fill),
            x,
            y + 1 + offset,
        ));
    }
    if let Some(footer) = footer {
        let footer = sanitize(footer);
        let footer = if footer.width() > text_width {
            ellipsize(&footer, text_width)
        } else {
            footer
        };
        let subtle: RcCharacterStyles = fill.dim(Some(AnsiCode::On)).into();
        chunks.push(CharacterChunk::new(
            text_row(&footer, &subtle),
            x,
            y + 1 + lines.len(),
        ));
    }
    chunks.push(CharacterChunk::new(
        border_row('╰', '╯'),
        x,
        y + 1 + lines.len() + usize::from(footer.is_some()),
    ));
    chunks
}

/// How long a toast takes to slide in, and the curve it does it on.
///
/// `EaseOut` because it is arriving: on screen quickly, then settling without a bump. The easing
/// lives in the [`Tween`](yinz_types::animation::Tween) that drives `progress`, so nothing in
/// this module eases again - the reveal below is a straight multiply.
pub const ENTER_DURATION: Duration = Duration::from_millis(200);
pub const ENTER_EASING: Easing = Easing::EaseOut;
/// How long a toast takes to slide back out. Shorter than the entrance and `EaseIn`, because a
/// thing that is leaving should commit to going rather than linger.
pub const EXIT_DURATION: Duration = Duration::from_millis(160);
pub const EXIT_EASING: Easing = Easing::EaseIn;
/// How long a survivor takes to glide to its new place in the stack. `EaseInOut`: it is moving
/// from one resting place to another, so it should leave and arrive gently at both ends.
pub const DISPLACE_DURATION: Duration = Duration::from_millis(220);
/// `EaseOut` rather than `EaseInOut`: a displacement is almost always a *re-aim* of a tween that
/// is already moving, and easing back in from a non-zero velocity reads as a stall.
pub const DISPLACE_EASING: Easing = Easing::EaseOut;
/// The distance `DISPLACE_DURATION` is quoted for - about one short toast's height. Displacements
/// are scaled against it so speed, rather than duration, is what stays constant: see
/// [`yinz_types::animation::scaled_duration`].
pub const DISPLACE_SPAN: f32 = 4.0;

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
    let visible_cols = (((width as f32) * progress.clamp(0.0, 1.0)).round() as usize).min(width);
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
            notification_lines("agent needs confirmation\nsend_input", 40),
            vec!["agent needs confirmation", "send_input"]
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
        let chunks = notification_chunks(&["hello".to_owned()], None, 4, 9, 20, &style, 1.0);
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
        let chunks = notification_chunks(&lines, None, 0, 0, 20, &style, 1.0);

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

    /// The shape this whole change exists for: agent's confirmation block, rendered as written.
    #[test]
    fn the_agent_confirmation_block_renders_as_written() {
        let style = Style::default();
        let text = "agent needs confirmation\nsend_input\n  pane_id: 3\n  text: git push";
        let width = notification_width(text, 80).expect("wide enough");
        let lines = notification_lines(text, width);

        assert_eq!(
            lines,
            vec![
                "agent needs confirmation",
                "send_input",
                "  pane_id: 3",
                "  text: git push",
            ]
        );

        let painted = rows(&notification_chunks(&lines, None, 0, 0, width, &style, 1.0));
        assert_eq!(painted.len(), notification_height(4));
        assert!(painted[0].starts_with('╭'), "got {:?}", painted[0]);
        assert_eq!(painted[1], "│ agent needs confirmation │");
        assert_eq!(painted[3], "│   pane_id: 3             │");
        assert!(painted[5].starts_with('╰'), "got {:?}", painted[5]);
    }

    #[test]
    fn a_fully_hidden_reveal_paints_nothing() {
        let style = Style::default();
        let chunks = notification_chunks(&["hello".to_owned()], None, 4, 9, 20, &style, 1.0);
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
        let chunks = notification_chunks(&lines, None, 4, 9, 20, &style, 1.0);
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
        let chunks = notification_chunks(&["hello".to_owned()], None, 4, 9, 20, &style, 1.0);
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
        let chunks = notification_chunks(&["hello".to_owned()], None, 4, 9, 20, &style, 1.0);
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
        let chunks = notification_chunks(&lines, None, 0, 0, 12, &style, 1.0);
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

    /// The property the whole no-jump design rests on: for every survivor of a stack change, the
    /// row it was settled at equals the row it is settled at now, plus the delta. If that holds,
    /// adding the delta to the toast's displacement leaves the painted row exactly where it was.
    #[test]
    fn a_settled_row_delta_accounts_for_exactly_where_a_toast_moved_to() {
        let viewport = viewport();
        let width = 20;
        // ids 1..=3 at differing heights, then the newest (id 4) arrives at the corner and id 2 is
        // dismissed - so the survivors are re-ordered *and* pushed by a toast of a different size
        let before: Vec<(NotificationId, usize)> = vec![
            (NotificationId::new(3), 3),
            (NotificationId::new(2), 5),
            (NotificationId::new(1), 4),
        ];
        let after: Vec<(NotificationId, usize)> = vec![
            (NotificationId::new(4), 6),
            (NotificationId::new(3), 3),
            (NotificationId::new(1), 4),
        ];

        for position in [
            NotificationPosition::TopLeft,
            NotificationPosition::TopRight,
            NotificationPosition::BottomLeft,
            NotificationPosition::BottomRight,
        ] {
            let heights = |stack: &[(NotificationId, usize)]| -> Vec<usize> {
                stack.iter().map(|(_, height)| *height).collect()
            };
            let origins_before = notification_origins(viewport, position, &heights(&before), width);
            let origins_after = notification_origins(viewport, position, &heights(&after), width);

            for (index, (id, _)) in after.iter().enumerate() {
                let Some(before_index) = before.iter().position(|(candidate, _)| candidate == id)
                else {
                    // a new arrival has nowhere to have moved from
                    assert_eq!(settled_row_delta(position, &before, &after, *id), None);
                    continue;
                };
                let delta = settled_row_delta(position, &before, &after, *id)
                    .expect("a survivor has a delta");
                assert_eq!(
                    origins_before[before_index].1 as isize + delta,
                    origins_after[index].1 as isize,
                    "{position:?}: toast {id} should land exactly on its new row"
                );
            }
        }
    }

    #[test]
    fn a_settled_row_delta_is_zero_when_nothing_moved() {
        let stack: Vec<(NotificationId, usize)> =
            vec![(NotificationId::new(1), 3), (NotificationId::new(2), 4)];
        for position in [
            NotificationPosition::TopLeft,
            NotificationPosition::BottomRight,
        ] {
            assert_eq!(
                settled_row_delta(position, &stack, &stack, NotificationId::new(1)),
                Some(0)
            );
            assert_eq!(
                settled_row_delta(position, &stack, &stack, NotificationId::new(2)),
                Some(0)
            );
        }
    }

    #[test]
    fn a_settled_row_delta_has_no_answer_for_a_toast_outside_either_stack() {
        let before: Vec<(NotificationId, usize)> = vec![(NotificationId::new(1), 3)];
        let after: Vec<(NotificationId, usize)> = vec![(NotificationId::new(2), 3)];
        let position = NotificationPosition::TopRight;
        assert_eq!(
            settled_row_delta(position, &before, &after, NotificationId::new(1)),
            None
        );
        assert_eq!(
            settled_row_delta(position, &before, &after, NotificationId::new(2)),
            None
        );
        assert_eq!(
            settled_row_delta(position, &before, &after, NotificationId::new(9)),
            None
        );
    }

    /// A toast growing taller pushes what is behind it away from the corner, in whichever
    /// direction that corner implies. This is the case a wide arrival creates: the shared width
    /// grows, everything re-wraps, and heights change under toasts that did not themselves move
    /// in the stack.
    #[test]
    fn a_height_change_displaces_what_is_stacked_behind_it() {
        let before: Vec<(NotificationId, usize)> =
            vec![(NotificationId::new(1), 3), (NotificationId::new(2), 4)];
        let after: Vec<(NotificationId, usize)> =
            vec![(NotificationId::new(1), 5), (NotificationId::new(2), 4)];
        assert_eq!(
            settled_row_delta(
                NotificationPosition::TopLeft,
                &before,
                &after,
                NotificationId::new(2)
            ),
            Some(2),
            "a top-anchored stack grows downwards, so the one behind moves down"
        );
        assert_eq!(
            settled_row_delta(
                NotificationPosition::BottomLeft,
                &before,
                &after,
                NotificationId::new(2)
            ),
            Some(-2),
            "a bottom-anchored stack grows upwards, so the one behind moves up"
        );
        assert_eq!(
            settled_row_delta(
                NotificationPosition::TopLeft,
                &before,
                &after,
                NotificationId::new(1)
            ),
            Some(0),
            "the toast in the corner does not move, however tall it gets"
        );
    }

    /// The colours a chunk's characters actually carry, as `(foreground, background)` pairs.
    fn colors(chunk: &CharacterChunk) -> Vec<(Option<AnsiCode>, Option<AnsiCode>)> {
        chunk
            .terminal_characters
            .iter()
            .map(|character| (character.styles.foreground, character.styles.background))
            .collect()
    }

    /// The guarantee that makes the fade invisible at rest: a fully-arrived toast carries the
    /// exact colours the theme declared, `PaletteColor` variant included, rather than an RGB
    /// resolution of them. A terminal with a customised 16-colour palette therefore only ever sees
    /// an approximation during the frames a toast is actually moving.
    #[test]
    fn a_fully_faded_in_toast_carries_the_declared_colours_exactly() {
        let style = Style::default();
        let declaration = style.colors.text_unselected;
        let background = Some(AnsiCode::from(declaration.background));
        let border = Some(AnsiCode::from(declaration.emphasis_0));
        let text = Some(AnsiCode::from(declaration.base));

        let lines = notification_lines("one\ntwo three", 20);
        let chunks = notification_chunks(&lines, None, 4, 9, 20, &style, 1.0);
        let (border_rows, text_rows) = chunks.split_at(1);

        for (foreground, chunk_background) in colors(&border_rows[0]) {
            assert_eq!(foreground, border, "the top border is the declared colour");
            assert_eq!(chunk_background, background);
        }
        // the last chunk is the bottom border; everything between is text framed by border pipes
        let (text_rows, bottom) = text_rows.split_at(text_rows.len() - 1);
        for (foreground, _) in colors(&bottom[0]) {
            assert_eq!(foreground, border);
        }
        for chunk in text_rows {
            let row = colors(chunk);
            assert_eq!(row.first().map(|(fg, _)| *fg), Some(border), "left pipe");
            assert_eq!(row.last().map(|(fg, _)| *fg), Some(border), "right pipe");
            for (foreground, chunk_background) in &row[1..row.len() - 1] {
                assert_eq!(*foreground, text, "the text is the declared colour");
                assert_eq!(*chunk_background, background);
            }
        }
    }

    #[test]
    fn a_toast_at_zero_fade_is_a_solid_slab_of_its_own_background() {
        let style = Style::default();
        let declaration = style.colors.text_unselected;
        let background = Some(AnsiCode::from(declaration.background));
        let lines = notification_lines("hidden", 20);
        for chunk in notification_chunks(&lines, None, 4, 9, 20, &style, 0.0) {
            for (foreground, chunk_background) in colors(&chunk) {
                assert_eq!(
                    foreground, background,
                    "the border and text have not resolved out of the slab yet"
                );
                assert_eq!(chunk_background, background);
            }
        }
    }

    #[test]
    fn a_partly_faded_toast_never_leaves_the_segment_between_its_endpoints() {
        let style = Style::default();
        let declaration = style.colors.text_unselected;
        let (from_r, from_g, from_b) = rgb_of(declaration.background);
        let lines = notification_lines("resolving", 20);
        for step in 1..10 {
            let fade = step as f32 / 10.0;
            for chunk in notification_chunks(&lines, None, 4, 9, 20, &style, fade) {
                for (foreground, _) in colors(&chunk) {
                    let Some(AnsiCode::RgbCode((r, g, b))) = foreground else {
                        panic!("a mid-fade colour is resolved rgb, got {foreground:?}");
                    };
                    // every channel is between the background it started at and one of the two
                    // endpoints it can be heading for
                    for (channel, start, ends) in [
                        (
                            r,
                            from_r,
                            [rgb_of(declaration.base).0, rgb_of(declaration.emphasis_0).0],
                        ),
                        (
                            g,
                            from_g,
                            [rgb_of(declaration.base).1, rgb_of(declaration.emphasis_0).1],
                        ),
                        (
                            b,
                            from_b,
                            [rgb_of(declaration.base).2, rgb_of(declaration.emphasis_0).2],
                        ),
                    ] {
                        assert!(
                            ends.iter().any(|end| {
                                channel >= start.min(*end) && channel <= start.max(*end)
                            }),
                            "channel {channel} outside [{start}, {ends:?}] at fade {fade}"
                        );
                    }
                }
            }
        }
    }

    const LONG: &str = "one two three four five six seven eight nine ten eleven twelve thirteen \
        fourteen fifteen sixteen seventeen eighteen nineteen twenty twenty-one twenty-two \
        twenty-three twenty-four twenty-five twenty-six twenty-seven twenty-eight";

    #[test]
    fn a_collapsed_browser_entry_is_cut_to_two_rows_and_elided() {
        let layout = notification_browser_layout(
            viewport(),
            NotificationPosition::TopRight,
            &[LONG, "short"],
            1,
            false,
            0,
        )
        .expect("the viewport holds the browser");
        let long = &layout.entries[0];
        assert_eq!(long.lines.len(), BROWSER_COLLAPSED_TEXT_ROWS);
        assert!(
            long.lines.last().unwrap().ends_with('…'),
            "{:?}",
            long.lines
        );
        assert_eq!(layout.entries[1].lines, vec!["short".to_owned()]);
    }

    #[test]
    fn the_expanded_browser_entry_shows_its_whole_text() {
        let layout = notification_browser_layout(
            viewport(),
            NotificationPosition::TopRight,
            &[LONG, "short"],
            0,
            true,
            0,
        )
        .expect("the viewport holds the browser");
        let expanded = &layout.entries[0];
        assert!(expanded.lines.len() > MAX_NOTIFICATION_TEXT_ROWS.min(3));
        assert!(!expanded.lines.iter().any(|line| line.ends_with('…')));
        let text: Vec<&str> = expanded
            .lines
            .iter()
            .flat_map(|line| line.split_whitespace())
            .collect();
        assert_eq!(text, LONG.split_whitespace().collect::<Vec<_>>());
    }

    #[test]
    fn the_browser_scrolls_only_as_far_as_the_selection_needs() {
        // every entry is one text row, so three rows tall; ten rows hold three of them
        let viewport = Viewport {
            x: 0,
            y: 0,
            rows: 10,
            cols: 80,
        };
        let texts = ["a", "b", "c", "d", "e", "f"];
        let shown = |selected: usize, first: usize| {
            let layout = notification_browser_layout(
                viewport,
                NotificationPosition::TopRight,
                &texts,
                selected,
                false,
                first,
            )
            .unwrap();
            let indices: Vec<usize> = layout.entries.iter().map(|entry| entry.index).collect();
            (layout.first, indices)
        };
        assert_eq!(shown(2, 0), (0, vec![0, 1, 2]));
        // moving past the bottom scrolls by one
        assert_eq!(shown(3, 0), (1, vec![1, 2, 3]));
        // moving back up inside the window keeps it where it is
        assert_eq!(shown(2, 1), (1, vec![1, 2, 3]));
        // moving above the window scrolls back to the selection
        assert_eq!(shown(0, 1), (0, vec![0, 1, 2]));
    }

    #[test]
    fn the_browser_stacks_from_the_configured_corner() {
        let viewport = viewport();
        for position in [
            NotificationPosition::TopLeft,
            NotificationPosition::BottomRight,
        ] {
            let layout =
                notification_browser_layout(viewport, position, &["a", "b"], 0, false, 0).unwrap();
            let (newest, older) = (&layout.entries[0], &layout.entries[1]);
            if position.is_top() {
                assert_eq!(newest.y, viewport.y);
                assert!(older.y > newest.y);
            } else {
                assert_eq!(newest.y, viewport.y + viewport.rows - 3);
                assert!(older.y < newest.y);
            }
        }
    }

    fn rgb_of(color: yinz_types::colors::PaletteColor) -> (u8, u8, u8) {
        match color {
            yinz_types::colors::PaletteColor::Rgb(rgb) => rgb,
            yinz_types::colors::PaletteColor::EightBit(index) => {
                yinz_types::colors::eightbit_to_rgb(index)
            },
        }
    }
}

#[cfg(test)]
#[path = "unit/notification_footer.rs"]
mod footer_tests;
