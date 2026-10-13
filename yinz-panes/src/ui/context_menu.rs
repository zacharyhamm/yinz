//! Right-click context menus: a rectangle the server draws over everything and hit-tests.
//!
//! A menu is deliberately **not a pane**. It has no layout participation, no focus, no frame
//! chrome, no resize and no serialization - the only things it needs from the rest of the session
//! are where to draw and what its z-order is. Two consequences are worth stating, because both are
//! the reason this module exists rather than a `NewPanePlacement::Overlay`:
//!
//! - **The clicked pane keeps focus.** Every item's actions run against the pane the user
//!   right-clicked, so `Copy` and `CloseFocus` mean what they look like they mean. A pane-shaped
//!   menu would take the focus and would have to hand it back, in order, before acting.
//! - **A menu flips at a screen edge, it does not shrink.** `PaneGeom::adjust_coordinates` clamps
//!   an out-of-bounds pane by taking columns away from it, which for a menu three columns from the
//!   right edge would mean a three-column menu. [`context_menu_layout`] flips the anchor instead.
//!
//! Everything here is pure. [`context_menu_layout`] is the single source of geometry, and both
//! [`context_menu_chunks`] (drawing) and [`context_menu_item_at`] (hit-testing) are written against
//! its output - the same discipline `ui::notification` and `panes::nested_session_modal` keep, and
//! for the same reason: two functions that compute the same rectangle separately eventually
//! disagree, and the symptom is a menu that highlights one row and runs another.

use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;
use yinz_types::data::ContextMenuItem;
use yinz_types::pane_size::Viewport;
use yinz_types::position::Position;
use yinz_types::style::Style;

use crate::output::CharacterChunk;
use crate::panes::terminal_character::{
    AnsiCode, RcCharacterStyles, TerminalCharacter, RESET_STYLES,
};

/// Left border, one space of padding, then the label. Mirrored on the right.
const CHROME_COLS: usize = 4;
/// Top and bottom border.
const BORDER_ROWS: usize = 2;
/// Columns between a label and its shortcut hint, when a row has both.
const SHORTCUT_GAP: usize = 3;
/// The widest a menu grows before labels are truncated.
const MAX_COLS: usize = 60;
/// Below this there is no room for a border, a padding column and a single character of label.
const MIN_COLS: usize = CHROME_COLS + 1;

/// One laid-out row, in the order the menu draws them.
///
/// `item_index` indexes back into the caller's `items` slice. It is not the row number: a menu
/// windowed to fit a short viewport starts partway down the list, and hit-testing has to answer
/// with the item the user actually pointed at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuRow {
    pub item_index: usize,
    pub label: String,
    pub shortcut: Option<String>,
    pub disabled: bool,
    pub separator: bool,
}

/// Where a menu goes and what is on each of its rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextMenuLayout {
    /// Top-left corner of the box, including its border, in surface coordinates.
    pub x: usize,
    pub y: usize,
    /// Total width including borders and padding.
    pub width: usize,
    pub rows: Vec<MenuRow>,
    /// The first item index the window starts at, for a list too long to fit.
    pub window_start: usize,
    /// Whether items were dropped above/below the window, so the caller can draw a hint.
    pub more_above: bool,
    pub more_below: bool,
}

impl ContextMenuLayout {
    /// Total height including borders.
    pub fn height(&self) -> usize {
        self.rows.len() + BORDER_ROWS
    }
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
    /// The row of the box `item_index` is drawn on, if it is inside the window.
    ///
    /// The inverse of `context_menu_item_at`, and only tests need it: drawing walks the rows in
    /// order and hit-testing goes the other way, so nothing in production has an item and wants a
    /// row. It exists so a test can aim at an item without duplicating the layout arithmetic it is
    /// supposed to be checking.
    pub fn row_of_item(&self, item_index: usize) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| row.item_index == item_index)
            .map(|offset| self.y + 1 + offset)
    }
}

/// The display width of a row's content, padding and borders excluded.
fn content_width(item: &ContextMenuItem) -> usize {
    if item.separator {
        return 0;
    }
    let label = item.label.width();
    match item.shortcut.as_deref() {
        Some(shortcut) if !shortcut.is_empty() => label + SHORTCUT_GAP + shortcut.width(),
        _ => label,
    }
}

/// Available label width for rows without shortcuts, shared with callers that wrap labels.
pub fn context_menu_text_width(items: &[ContextMenuItem], viewport: Viewport) -> usize {
    let natural = items.iter().map(content_width).max().unwrap_or(0) + CHROME_COLS;
    natural.min(MAX_COLS).min(viewport.cols).max(MIN_COLS) - CHROME_COLS
}

/// Truncate to `max` columns of display width, respecting wide characters.
fn truncate_to_width(text: &str, max: usize) -> String {
    if text.width() <= max {
        return text.to_owned();
    }
    let mut used = 0;
    let mut out = String::new();
    for character in text.chars() {
        // Zero for a combining mark, which is what `UnicodeWidthStr::width` above and
        // `TerminalCharacter::new_styled` both say it is. Flooring it at one would disagree with
        // the width this menu was laid out to, and would drop an accent from the character it
        // belongs to while keeping that character.
        let character_width = character.width().unwrap_or(0);
        if used + character_width > max {
            break;
        }
        out.push(character);
        used += character_width;
    }
    out
}

/// Lay a menu out at `anchor`, inside `viewport`.
///
/// The anchor is the clicked cell; the menu's top-left border starts at that cell. It flips to the
/// other side of the anchor rather than being clipped when that would overflow, and only clamps
/// as a last resort when the menu is larger than the viewport in that axis.
///
/// A list too tall for the viewport is *windowed* rather than truncated, with `more_above` /
/// `more_below` saying so, because silently dropping the tail of a menu is indistinguishable to the
/// user from the item not existing.
pub fn context_menu_layout(
    items: &[ContextMenuItem],
    anchor: Position,
    viewport: Viewport,
    selected: Option<usize>,
) -> ContextMenuLayout {
    let empty = ContextMenuLayout {
        x: viewport.x,
        y: viewport.y,
        width: 0,
        rows: vec![],
        window_start: 0,
        more_above: false,
        more_below: false,
    };
    if items.is_empty() || viewport.cols < MIN_COLS || viewport.rows < BORDER_ROWS + 1 {
        return empty;
    }

    // One width for the whole menu - the widest row's content, chrome added, clamped to what the
    // viewport can hold. Clamping before truncation is what keeps a long label from forcing a menu
    // wider than the screen.
    let text_width = context_menu_text_width(items, viewport);
    let width = text_width + CHROME_COLS;

    // How many rows there is room for, once the borders have taken theirs.
    let max_rows = viewport.rows.saturating_sub(BORDER_ROWS);
    let (window_start, window_len) = window_for(items.len(), max_rows, selected);
    let more_above = window_start > 0;
    let more_below = window_start + window_len < items.len();

    let rows: Vec<MenuRow> = items
        .iter()
        .enumerate()
        .skip(window_start)
        .take(window_len)
        .map(|(item_index, item)| {
            // The shortcut is the first thing to go: a label that says what the row does is worth
            // more than a hint about how else to reach it.
            let shortcut = item
                .shortcut
                .as_deref()
                .filter(|shortcut| !shortcut.is_empty())
                .filter(|shortcut| shortcut.width() + SHORTCUT_GAP < text_width)
                .map(|shortcut| shortcut.to_owned());
            let label_room = match shortcut.as_deref() {
                Some(shortcut) => text_width.saturating_sub(shortcut.width() + SHORTCUT_GAP),
                None => text_width,
            };
            MenuRow {
                item_index,
                label: truncate_to_width(&item.label, label_room),
                shortcut,
                disabled: item.disabled,
                separator: item.separator,
            }
        })
        .collect();

    let height = rows.len() + BORDER_ROWS;
    let (x, y) = anchor_box(anchor, width, height, viewport);

    ContextMenuLayout {
        x,
        y,
        width,
        rows,
        window_start,
        more_above,
        more_below,
    }
}

/// Which slice of a too-long item list to show, keeping `selected` on screen.
///
/// The window is anchored at the top until the selection would fall off the bottom, then follows
/// it. Simpler than centring the selection, and it means a menu the user has not touched shows its
/// first item first - which is the one they are most likely to want.
fn window_for(item_count: usize, max_rows: usize, selected: Option<usize>) -> (usize, usize) {
    if max_rows == 0 {
        return (0, 0);
    }
    if item_count <= max_rows {
        return (0, item_count);
    }
    let start = match selected {
        Some(selected) if selected >= max_rows => selected + 1 - max_rows,
        _ => 0,
    };
    let start = start.min(item_count - max_rows);
    (start, max_rows)
}

/// Where the box's top-left corner goes: at the anchor, flipped at an edge, clamped
/// only when it cannot fit either way.
fn anchor_box(anchor: Position, width: usize, height: usize, viewport: Viewport) -> (usize, usize) {
    let max_x = viewport.x + viewport.cols;
    let max_y = viewport.y + viewport.rows;

    let preferred_x = anchor.column();
    let x = if preferred_x + width <= max_x {
        preferred_x
    } else {
        // Flip: the menu's right edge meets the anchor instead of its left.
        preferred_x.saturating_sub(width)
    };
    let x = x.clamp(viewport.x, max_x.saturating_sub(width).max(viewport.x));

    let preferred_y = anchor.line().max(0) as usize;
    let y = if preferred_y + height <= max_y {
        preferred_y
    } else {
        // Flip: the menu sits above the pointer, its bottom edge on the row above the anchor.
        (anchor.line().max(0) as usize).saturating_sub(height)
    };
    let y = y.clamp(viewport.y, max_y.saturating_sub(height).max(viewport.y));

    (x, y)
}

/// The item at `position`, if the pointer is on a selectable row of this menu.
///
/// `None` for a separator, a disabled row, the border, or anywhere outside the box. Callers that
/// need "is this inside the menu at all" - dismissing on an outside click - want
/// [`context_menu_contains`], not this.
pub fn context_menu_item_at(layout: &ContextMenuLayout, position: Position) -> Option<usize> {
    if !context_menu_contains(layout, position) {
        return None;
    }
    let line = usize::try_from(position.line()).ok()?;
    // The first row of the box is its top border.
    let row_offset = line.checked_sub(layout.y + 1)?;
    let row = layout.rows.get(row_offset)?;
    if row.separator || row.disabled {
        return None;
    }
    Some(row.item_index)
}

/// Whether `position` is anywhere inside the menu's box, borders included.
pub fn context_menu_contains(layout: &ContextMenuLayout, position: Position) -> bool {
    if layout.is_empty() {
        return false;
    }
    let Ok(line) = usize::try_from(position.line()) else {
        return false;
    };
    (layout.x..layout.x + layout.width).contains(&position.column())
        && (layout.y..layout.y + layout.height()).contains(&line)
}

struct MenuStyles {
    border: RcCharacterStyles,
    fill: RcCharacterStyles,
    shortcut: RcCharacterStyles,
    disabled: RcCharacterStyles,
    selected_fill: RcCharacterStyles,
    selected_shortcut: RcCharacterStyles,
}

fn menu_styles(style: &Style) -> MenuStyles {
    let base = style.colors.text_unselected;
    let selected = style.colors.text_selected;
    MenuStyles {
        border: RESET_STYLES
            .foreground(Some(AnsiCode::from(base.emphasis_0)))
            .background(Some(AnsiCode::from(base.background)))
            .into(),
        fill: RESET_STYLES
            .foreground(Some(AnsiCode::from(base.base)))
            .background(Some(AnsiCode::from(base.background)))
            .into(),
        shortcut: RESET_STYLES
            .foreground(Some(AnsiCode::from(base.emphasis_3)))
            .background(Some(AnsiCode::from(base.background)))
            .into(),
        // A disabled row is drawn in the border's colour rather than the label's: it is the one
        // colour in the declaration already established as "present but not the point".
        disabled: RESET_STYLES
            .foreground(Some(AnsiCode::from(base.emphasis_0)))
            .background(Some(AnsiCode::from(base.background)))
            .into(),
        selected_fill: RESET_STYLES
            .foreground(Some(AnsiCode::from(selected.base)))
            .background(Some(AnsiCode::from(selected.background)))
            .into(),
        selected_shortcut: RESET_STYLES
            .foreground(Some(AnsiCode::from(selected.emphasis_3)))
            .background(Some(AnsiCode::from(selected.background)))
            .into(),
    }
}

/// The chunks for one menu. `selected` is an index into the caller's `items`, not a row number.
pub fn context_menu_chunks(
    layout: &ContextMenuLayout,
    selected: Option<usize>,
    style: &Style,
) -> Vec<CharacterChunk> {
    if layout.is_empty() {
        return vec![];
    }
    let styles = menu_styles(style);
    let inner_width = layout.width.saturating_sub(2);
    let text_width = layout.width.saturating_sub(CHROME_COLS);

    let horizontal = |left: char, fill: char, right: char| -> Vec<TerminalCharacter> {
        let mut characters = Vec::with_capacity(layout.width);
        characters.push(TerminalCharacter::new_styled(left, styles.border.clone()));
        for _ in 0..inner_width {
            characters.push(TerminalCharacter::new_styled(fill, styles.border.clone()));
        }
        characters.push(TerminalCharacter::new_styled(right, styles.border.clone()));
        characters
    };

    let item_row = |row: &MenuRow, is_selected: bool| -> Vec<TerminalCharacter> {
        let (body, shortcut_style) = if is_selected {
            (
                styles.selected_fill.clone(),
                styles.selected_shortcut.clone(),
            )
        } else if row.disabled {
            (styles.disabled.clone(), styles.disabled.clone())
        } else {
            (styles.fill.clone(), styles.shortcut.clone())
        };
        let mut characters = Vec::with_capacity(layout.width);
        characters.push(TerminalCharacter::new_styled('│', styles.border.clone()));
        characters.push(TerminalCharacter::new_singlewidth_styled(' ', body.clone()));

        let mut used = 0;
        for character in row.label.chars() {
            // Zero-width for the same reason `truncate_to_width` says: `used` is compared against
            // widths the layout measured with `UnicodeWidthStr`, and the padding below is derived
            // from it, so a mark counted as a column here makes the row narrower than its border.
            let character_width = character.width().unwrap_or(0);
            if used + character_width > text_width {
                break;
            }
            characters.push(TerminalCharacter::new_styled(character, body.clone()));
            used += character_width;
        }
        // Right-align the shortcut against the far edge of the text area, padding the gap with the
        // row's own background so a selected row highlights as one unbroken block.
        let shortcut_width = row.shortcut.as_deref().map(|s| s.width()).unwrap_or(0);
        let pad = text_width.saturating_sub(used + shortcut_width);
        for _ in 0..pad {
            characters.push(TerminalCharacter::new_singlewidth_styled(' ', body.clone()));
        }
        if let Some(shortcut) = row.shortcut.as_deref() {
            for character in shortcut.chars() {
                characters.push(TerminalCharacter::new_styled(
                    character,
                    shortcut_style.clone(),
                ));
            }
        }

        characters.push(TerminalCharacter::new_singlewidth_styled(' ', body));
        characters.push(TerminalCharacter::new_styled('│', styles.border.clone()));
        characters
    };

    let mut chunks = Vec::with_capacity(layout.rows.len() + BORDER_ROWS);
    chunks.push(CharacterChunk::new(
        horizontal('╭', if layout.more_above { '┄' } else { '─' }, '╮'),
        layout.x,
        layout.y,
    ));
    for (offset, row) in layout.rows.iter().enumerate() {
        let characters = if row.separator {
            horizontal('├', '─', '┤')
        } else {
            item_row(row, Some(row.item_index) == selected)
        };
        chunks.push(CharacterChunk::new(
            characters,
            layout.x,
            layout.y + 1 + offset,
        ));
    }
    chunks.push(CharacterChunk::new(
        horizontal('╰', if layout.more_below { '┄' } else { '─' }, '╯'),
        layout.x,
        layout.y + 1 + layout.rows.len(),
    ));
    chunks
}

/// The next selectable item in `direction` from `from`, wrapping.
///
/// Separators and disabled rows are skipped rather than merely being unselectable on arrival -
/// otherwise Down onto a separator would look like the key did nothing. Returns `None` when the
/// menu has no selectable item at all, which is a menu of nothing but separators.
pub fn next_selectable(
    items: &[ContextMenuItem],
    from: Option<usize>,
    forward: bool,
) -> Option<usize> {
    let count = items.len();
    if count == 0 {
        return None;
    }
    let start = match from {
        Some(from) => from,
        // No selection yet: Down lands on the first selectable row, Up on the last.
        None => {
            return if forward {
                (0..count).find(|index| items[*index].is_selectable())
            } else {
                (0..count).rev().find(|index| items[*index].is_selectable())
            }
        },
    };
    (1..=count)
        .map(|step| {
            if forward {
                (start + step) % count
            } else {
                (start + count - (step % count)) % count
            }
        })
        .find(|index| items[*index].is_selectable())
}

#[cfg(test)]
#[path = "./unit/context_menu_tests.rs"]
mod context_menu_tests;
