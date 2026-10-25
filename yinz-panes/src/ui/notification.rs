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

    if overflowed && let Some(last) = rows.last_mut() {
        *last = ellipsize(last, text_width);
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
#[path = "unit/notification_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "unit/notification_footer.rs"]
mod footer_tests;
