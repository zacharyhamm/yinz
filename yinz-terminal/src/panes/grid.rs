use super::kitty_graphics::diacritic_value;
use super::kitty_graphics::{
    format_kitty_error, format_kitty_reply, KittyAction, KittyCommand, KittyCommandParser,
    KittyError, KittyErrorCode, KittyGrid, KittyHostSupport, KittyImageChunk, KittyImageStore,
    KittyPlacement, KittyReplyData, KittyRowsBelowTheViewport, KittyVerticalAnchor,
    PlaceholderMark, PLACEHOLDER,
};
use super::sixel::{PixelRect, SixelGrid, SixelImageStore};
use base64::alphabet::STANDARD as BASE64_STANDARD_ALPHABET;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::{DecodePaddingMode, Engine as _};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::rc::Rc;
use unicode_width::UnicodeWidthChar;
use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::PluginId;
use yinz_types::mouse::{MouseEvent, MouseEventType};
use yinz_types::style::{HostTerminalThemeMode, Style};
use {
    yinz_types::data::HighlightLayer, yinz_types::data::HighlightStyle,
    yinz_types::data::RegexHighlight,
};

use std::{
    cmp::Ordering,
    collections::{BTreeSet, VecDeque},
    fmt::{self, Debug, Formatter},
    str,
};

use vte;
use yinz_types::pane_size::SizeInPixels;
use yinz_types::position::Position;
use yinz_types::style::{Palette, Styling};
use yinz_types::ui_components::UI_COMPONENT_DCS_FINAL;
use {
    yinz_ipc::nested_session, yinz_ipc::nested_session::NestedSessionMessage,
    yinz_runtime::consts::DEFAULT_SCROLL_BUFFER_SIZE, yinz_runtime::consts::SCROLL_BUFFER_SIZE,
    yinz_types::data::PaletteColor, yinz_types::input::options::DEFAULT_WORD_SEPARATORS,
};

/// The ECMA-48 default; HTS/TBC (`set_horizontal_tabstop`, `clear_tabstop_at_position`)
/// let an application move them.
const TABSTOP_WIDTH: usize = 8;
pub const MAX_TITLE_STACK_SIZE: usize = 1000;

const BASE64_DECODER: GeneralPurpose = GeneralPurpose::new(
    &BASE64_STANDARD_ALPHABET,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

const MAX_TRACKED_NOTIFICATION_IDS: usize = 256;
const MAX_NOTIFICATION_ASSEMBLY_BYTES: usize = 4096;
const MAX_DCS_PAYLOAD_BYTES: usize = 1024 * 1024;

// The notification wire types live with the rest of the pane/host protocol. The pub re-exports
// keep `panes::grid::{namespace_notification_id, PendingNotification}` working for the pane layer,
// the screen and the tab tests; the rest is only used here, by the OSC 99 dispatch and the `NotificationTracker`.
use yinz_host_protocol::notifications::{
    action_wants_report, parse_osc99_metadata, split_osc99_payload, Osc99PayloadType,
};
pub use yinz_host_protocol::notifications::{namespace_notification_id, PendingNotification};

#[derive(Debug, Clone, Default)]
struct NotificationAssembly {
    title: String,
    body: String,
}

/// Appends `payload` to `destination` without letting it grow past
/// `MAX_NOTIFICATION_ASSEMBLY_BYTES`, truncating on a character boundary.
fn append_bounded(destination: &mut String, payload: &str) {
    let remaining = MAX_NOTIFICATION_ASSEMBLY_BYTES.saturating_sub(destination.len());
    if remaining == 0 {
        return;
    }
    if payload.len() <= remaining {
        destination.push_str(payload);
        return;
    }
    let mut truncate_at = remaining;
    while truncate_at > 0 && !payload.is_char_boundary(truncate_at) {
        truncate_at -= 1;
    }
    destination.push_str(&payload[..truncate_at]);
}

fn append_dcs_payload(payload: &mut Option<Vec<u8>>, byte: u8) {
    match payload {
        Some(bytes) if bytes.len() < MAX_DCS_PAYLOAD_BYTES => bytes.push(byte),
        Some(_) => *payload = None,
        None => {},
    }
}

/// What one pane's application said about its OSC 99 notifications: whether each id asked for
/// an activation report, and the title/body of notifications still being sent in chunks
/// (`d=0`). Bounded to the last `MAX_TRACKED_NOTIFICATION_IDS` ids.
#[derive(Debug, Clone, Default)]
pub struct NotificationTracker {
    wants_report: HashMap<String, bool>,
    assemblies: HashMap<String, NotificationAssembly>,
    known_ids: VecDeque<String>,
}

impl NotificationTracker {
    fn remember(&mut self, id: &str) {
        if self.known_ids.iter().any(|known| known == id) {
            return;
        }
        self.known_ids.push_back(id.to_owned());
        while self.known_ids.len() > MAX_TRACKED_NOTIFICATION_IDS {
            if let Some(evicted) = self.known_ids.pop_front() {
                self.wants_report.remove(&evicted);
                self.assemblies.remove(&evicted);
            }
        }
    }
    /// Whether the escape wants an activation report: its own `a=` when it states one
    /// (remembered from then on), otherwise what the notification with this id asked for.
    fn wants_report(&mut self, id: &str, action: Option<&str>) -> bool {
        match action {
            Some(action_value) => {
                let wants_report = action_wants_report(action_value);
                if !id.is_empty() {
                    self.remember(id);
                    self.wants_report.insert(id.to_owned(), wants_report);
                }
                wants_report
            },
            None => self.wants_report.get(id).copied().unwrap_or(false),
        }
    }
    /// Adds a chunk to the notification with this id and returns the assembled title and body
    /// once it is complete. Control requests show nothing; a close drops the assembly.
    fn assemble(
        &mut self,
        id: &str,
        payload_type: Osc99PayloadType,
        is_done: bool,
        payload: String,
    ) -> Option<(String, String)> {
        if payload_type == Osc99PayloadType::Close {
            self.assemblies.remove(id);
            return None;
        }
        if payload_type.is_control_request() {
            return None;
        }
        if !payload.is_empty() {
            self.remember(id);
            let assembly = self.assemblies.entry(id.to_owned()).or_default();
            match payload_type {
                Osc99PayloadType::Title => append_bounded(&mut assembly.title, &payload),
                Osc99PayloadType::Body => append_bounded(&mut assembly.body, &payload),
                _ => {},
            }
        }
        if !is_done {
            return None;
        }
        let assembly = self.assemblies.remove(id)?;
        if assembly.title.is_empty() && assembly.body.is_empty() {
            None
        } else {
            Some((assembly.title, assembly.body))
        }
    }
}

use vte::{Params, Perform};
use {yinz_runtime::consts::VERSION, yinz_runtime::shared::version_number};

use crate::output::{CharacterChunk, HighlightSelection, OutputBuffer, SixelImageChunk};
use crate::panes::alacritty_functions::{parse_number, xparse_color};
use crate::panes::hyperlink_tracker::HyperlinkTracker;
use crate::panes::link_handler::LinkHandler;
use crate::panes::search::SearchResult;
use crate::panes::terminal_character::{
    AnsiCode, CharsetIndex, Cursor, CursorShape, RcCharacterStyles, StandardCharset,
    TerminalCharacter, EMPTY_TERMINAL_CHARACTER,
};
use crate::panes::Selection;
use crate::ui::components::UiComponentParser;
use yinz_types::data::PaneContents;
use yinz_types::geometry::Direction;

fn get_top_non_canonical_rows(rows: &mut VecDeque<Row>) -> Vec<Row> {
    let mut index_of_last_non_canonical_row = None;
    for (i, row) in rows.iter().enumerate() {
        if row.is_canonical {
            break;
        } else {
            index_of_last_non_canonical_row = Some(i);
        }
    }
    match index_of_last_non_canonical_row {
        Some(index_of_last_non_canonical_row) => {
            rows.drain(..=index_of_last_non_canonical_row).collect()
        },
        None => vec![],
    }
}

fn get_lines_above_bottom_canonical_row_and_wraps(rows: &mut VecDeque<Row>) -> Vec<Row> {
    let mut index_of_last_non_canonical_row = None;
    for (i, row) in rows.iter().enumerate().rev() {
        index_of_last_non_canonical_row = Some(i);
        if row.is_canonical {
            break;
        }
    }
    match index_of_last_non_canonical_row {
        Some(index_of_last_non_canonical_row) => {
            rows.drain(index_of_last_non_canonical_row..).collect()
        },
        None => vec![],
    }
}

fn get_viewport_bottom_canonical_row_and_wraps(viewport: &mut VecDeque<Row>) -> Vec<Row> {
    let mut index_of_last_non_canonical_row = None;
    for (i, row) in viewport.iter().enumerate().rev() {
        index_of_last_non_canonical_row = Some(i);
        if row.is_canonical {
            break;
        }
    }
    match index_of_last_non_canonical_row {
        Some(index_of_last_non_canonical_row) => {
            viewport.drain(index_of_last_non_canonical_row..).collect()
        },
        None => vec![],
    }
}

fn get_top_canonical_row_and_wraps(rows: &mut VecDeque<Row>) -> Vec<Row> {
    let mut index_of_first_non_canonical_row = None;
    let mut end_index_of_first_canonical_line = None;
    for (i, row) in rows.iter().enumerate() {
        if row.is_canonical && end_index_of_first_canonical_line.is_none() {
            index_of_first_non_canonical_row = Some(i);
            end_index_of_first_canonical_line = Some(i);
            continue;
        }
        if row.is_canonical && end_index_of_first_canonical_line.is_some() {
            break;
        }
        if index_of_first_non_canonical_row.is_some() {
            end_index_of_first_canonical_line = Some(i);
            continue;
        }
    }
    match (
        index_of_first_non_canonical_row,
        end_index_of_first_canonical_line,
    ) {
        (Some(first_index), Some(last_index)) => rows.drain(first_index..=last_index).collect(),
        (Some(first_index), None) => rows.drain(first_index..).collect(),
        _ => vec![],
    }
}

fn transfer_rows_from_lines_above_to_viewport(
    lines_above: &mut VecDeque<Row>,
    viewport: &mut VecDeque<Row>,
    sixel_grid: &mut SixelGrid,
    kitty_grid: &mut KittyGrid,
    count: usize,
    max_viewport_width: usize,
) -> usize {
    let mut next_lines: Vec<Row> = vec![];
    let mut lines_added_to_viewport: isize = 0;
    loop {
        if lines_added_to_viewport as usize == count {
            break;
        }
        if next_lines.is_empty() {
            match lines_above.pop_back() {
                Some(next_line) => {
                    let mut top_non_canonical_rows_in_dst = get_top_non_canonical_rows(viewport);
                    let merged_row_count = top_non_canonical_rows_in_dst.len();
                    lines_added_to_viewport -= merged_row_count as isize;
                    next_lines.push(next_line);
                    next_lines.append(&mut top_non_canonical_rows_in_dst);
                    next_lines =
                        Row::from_rows(next_lines).split_to_rows_of_length(max_viewport_width);
                    if next_lines.is_empty() {
                        // no more lines at lines_above, the line we popped was probably empty
                        break;
                    }
                    let row_count_delta = next_lines.len() as isize - 1 - merged_row_count as isize;
                    if row_count_delta > 0 {
                        kitty_grid.split_line_start_into_rows(
                            lines_above.len(),
                            row_count_delta as usize,
                        );
                    } else if row_count_delta < 0 {
                        kitty_grid.merge_rows_into_line_start(
                            lines_above.len(),
                            row_count_delta.unsigned_abs(),
                        );
                    }
                },
                None => break, // no more rows
            }
        }
        viewport.push_front(next_lines.pop().unwrap());
        lines_added_to_viewport += 1;
    }
    if !next_lines.is_empty() {
        kitty_grid.merge_rows_into_line_start(lines_above.len(), next_lines.len() - 1);
        let excess_row = Row::from_rows(next_lines);
        bounded_push(lines_above, sixel_grid, kitty_grid, excess_row);
    }
    usize::try_from(lines_added_to_viewport).unwrap_or_default()
}

fn transfer_rows_from_viewport_to_lines_above(
    viewport: &mut VecDeque<Row>,
    lines_above: &mut VecDeque<Row>,
    sixel_grid: &mut SixelGrid,
    kitty_grid: &mut KittyGrid,
    count: usize,
    max_viewport_width: usize,
) -> isize {
    let mut transferred_rows_count: isize = 0;
    let drained_lines = std::cmp::min(count, viewport.len());
    for next_line in viewport.drain(..drained_lines) {
        let mut next_lines: Vec<Row> = vec![];
        transferred_rows_count +=
            calculate_row_display_height(next_line.width(), max_viewport_width) as isize;
        if !next_line.is_canonical {
            let mut bottom_canonical_row_and_wraps_in_dst =
                get_lines_above_bottom_canonical_row_and_wraps(lines_above);
            kitty_grid.merge_rows_into_line_start(
                lines_above.len(),
                bottom_canonical_row_and_wraps_in_dst.len(),
            );
            next_lines.append(&mut bottom_canonical_row_and_wraps_in_dst);
        }
        next_lines.push(next_line);
        let dropped_line_width = bounded_push(
            lines_above,
            sixel_grid,
            kitty_grid,
            Row::from_rows(next_lines),
        );
        if let Some(width) = dropped_line_width {
            transferred_rows_count -=
                calculate_row_display_height(width, max_viewport_width) as isize;
        }
    }
    transferred_rows_count
}

fn transfer_rows_from_lines_below_to_viewport(
    lines_below: &mut VecDeque<Row>,
    viewport: &mut VecDeque<Row>,
    count: usize,
    max_viewport_width: usize,
) {
    let mut next_lines: Vec<Row> = vec![];
    for _ in 0..count {
        let mut lines_pulled_from_viewport = 0;
        if next_lines.is_empty() {
            if !lines_below.is_empty() {
                let mut top_non_canonical_rows_in_lines_below =
                    get_top_non_canonical_rows(lines_below);
                if !top_non_canonical_rows_in_lines_below.is_empty() {
                    let mut canonical_line = get_viewport_bottom_canonical_row_and_wraps(viewport);
                    lines_pulled_from_viewport += canonical_line.len();
                    canonical_line.append(&mut top_non_canonical_rows_in_lines_below);
                    next_lines =
                        Row::from_rows(canonical_line).split_to_rows_of_length(max_viewport_width);
                } else {
                    let canonical_row = get_top_canonical_row_and_wraps(lines_below);
                    next_lines =
                        Row::from_rows(canonical_row).split_to_rows_of_length(max_viewport_width);
                }
            } else {
                break; // no more rows
            }
        }
        for _ in 0..(lines_pulled_from_viewport + 1) {
            if !next_lines.is_empty() {
                viewport.push_back(next_lines.remove(0));
            }
        }
    }
    if !next_lines.is_empty() {
        let excess_row = Row::from_rows(next_lines);
        lines_below.push_front(excess_row);
    }
}

fn bounded_push(
    vec: &mut VecDeque<Row>,
    sixel_grid: &mut SixelGrid,
    kitty_grid: &mut KittyGrid,
    mut value: Row,
) -> Option<usize> {
    // Every path into scrollback releases spare viewport capacity, including manual scrolling.
    if value.columns.len() < value.columns.capacity() / 2 {
        value.columns.shrink_to_fit();
    }
    let mut dropped_line_width = None;
    if vec.len() >= *SCROLL_BUFFER_SIZE.get().unwrap() {
        let line = vec.pop_front();
        if let Some(line) = line {
            sixel_grid.offset_grid_top();
            kitty_grid.offset_grid_top();
            if line.is_canonical {
                let first_canonical_row =
                    if kitty_grid.has_placement_anchored_to_first_canonical_line() {
                        vec.iter()
                            .position(|row| row.is_canonical)
                            .unwrap_or(vec.len())
                    } else {
                        0
                    };
                kitty_grid.drop_first_canonical_anchor_line(first_canonical_row);
            }
            dropped_line_width = Some(line.width());
        }
    }
    vec.push_back(value);
    dropped_line_width
}

pub fn create_horizontal_tabstops(columns: usize) -> BTreeSet<usize> {
    let mut i = TABSTOP_WIDTH;
    let mut horizontal_tabstops = BTreeSet::new();
    loop {
        if i > columns {
            break;
        }
        horizontal_tabstops.insert(i);
        i += TABSTOP_WIDTH;
    }
    horizontal_tabstops
}

fn calculate_row_display_height(row_width: usize, viewport_width: usize) -> usize {
    if row_width <= viewport_width {
        return 1;
    }
    (row_width as f64 / viewport_width as f64).ceil() as usize
}

fn subtract_isize_from_usize(u: usize, i: isize) -> usize {
    if i.is_negative() {
        u - i.unsigned_abs()
    } else {
        u + i as usize
    }
}

pub fn parse_osc7_path(raw: &[u8]) -> Option<std::path::PathBuf> {
    let s = std::str::from_utf8(raw).ok()?;
    let rest = s.strip_prefix("file://")?;
    let slash_idx = rest.find('/')?;
    let encoded_path = &rest[slash_idx..];
    let mut out = Vec::with_capacity(encoded_path.len());
    let bytes = encoded_path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let end = i.checked_add(3)?;
            if end > bytes.len() {
                return None;
            }
            let hex = std::str::from_utf8(&bytes[i + 1..end]).ok()?;
            let byte = u8::from_str_radix(hex, 16).ok()?;
            out.push(byte);
            i = end;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let decoded = String::from_utf8(out).ok()?;
    Some(std::path::PathBuf::from(decoded))
}

macro_rules! dump_screen {
    ($lines:expr) => {{
        let mut is_first = true;
        let lines = $lines;
        let mut buf = String::with_capacity(lines.clone().map(|l| l.len()).sum());

        for line in lines {
            if line.is_canonical && !is_first {
                buf.push_str("\n");
            }
            let s: String = (&line.columns).into_iter().map(|x| x.character).collect();
            // Replace the spaces at the end of the line. Sometimes, the lines are
            // collected with spaces until the end of the panel.
            buf.push_str(&s.trim_end_matches(' '));
            is_first = false;
        }
        buf
    }};
}

macro_rules! dump_screen_with_ansi {
    ($lines:expr) => {{
        use std::fmt::Write;
        let mut is_first = true;
        let mut buf = String::new();
        let mut last_styles: Option<RcCharacterStyles> = None;

        for line in $lines {
            if line.is_canonical && !is_first {
                buf.push_str("\n");
                last_styles = None;
            }

            let last_non_space = line
                .columns
                .iter()
                .rposition(|tc| {
                    let space = tc.character == ' ';
                    let styled = !matches!(tc.styles.background, Some(AnsiCode::Reset) | None);
                    !space || styled // it's, something drawable
                })
                .map(|i| i + 1)
                .unwrap_or(0);

            for tc in line.columns.iter().take(last_non_space) {
                // Only output style codes if style changed
                if last_styles.as_ref() != Some(&tc.styles) {
                    write!(buf, "{}", tc.styles).unwrap();
                    last_styles = Some(tc.styles.clone());
                }
                buf.push(tc.character);
            }
            is_first = false;
        }
        buf.push_str("\u{1b}[m");
        buf
    }};
}

fn utf8_mouse_coordinates(column: usize, line: isize) -> Vec<u8> {
    let mut coordinates = vec![];
    let mouse_pos_encode = |pos: usize| -> Vec<u8> {
        let pos = 32 + pos;
        let first = 0xC0 + pos / 64;
        let second = 0x80 + (pos & 63);
        vec![first as u8, second as u8]
    };

    if column > 95 {
        coordinates.append(&mut mouse_pos_encode(column));
    } else {
        coordinates.push(32 + column as u8);
    }
    if line > 95 {
        coordinates.append(&mut mouse_pos_encode(line as usize));
    } else {
        coordinates.push(32 + line as u8);
    }
    coordinates
}

/// A logical line rebuilt from wrapped rows: `(canonical row index, row count, text, per-row
/// byte-offset boundaries)`.
type LogicalLine = (usize, usize, String, Vec<(usize, usize)>);

/// Find the canonical root row for a logical line group containing `row_idx`,
/// collect the non-canonical tail rows, and build the concatenated text with
/// a byte-offset boundary table.
///
/// Returns `(canonical_idx, group_len, text, boundaries)` where `group_len`
/// is the total number of viewport rows in the group (1 + tail count).
/// Returns `None` if `row_idx` is out of bounds.
fn collect_and_build_logical_line(viewport: &VecDeque<Row>, row_idx: usize) -> Option<LogicalLine> {
    if row_idx >= viewport.len() {
        return None;
    }
    // Walk backward to find the canonical root.
    let mut canonical = row_idx;
    while canonical > 0 {
        match viewport.get(canonical) {
            Some(r) if !r.is_canonical => canonical -= 1,
            _ => break,
        }
    }
    let canonical_row = viewport.get(canonical)?;
    // Collect non-canonical tail rows.
    let mut tail_count = 0;
    loop {
        let tail_idx = canonical + tail_count + 1;
        match viewport.get(tail_idx) {
            Some(r) if !r.is_canonical => tail_count += 1,
            _ => break,
        }
    }
    let group_len = 1 + tail_count;
    // Build concatenated text and boundary table.
    let mut text = String::new();
    let mut boundaries: Vec<(usize, usize)> = Vec::with_capacity(group_len);
    boundaries.push((canonical, 0));
    for ch in &canonical_row.columns {
        text.push(ch.character);
    }
    for i in 0..tail_count {
        let idx = canonical + 1 + i;
        boundaries.push((idx, text.len()));
        if let Some(row) = viewport.get(idx) {
            for ch in &row.columns {
                text.push(ch.character);
            }
        }
    }
    Some((canonical, group_len, text, boundaries))
}

/// Map a byte offset in the concatenated logical-line string back to
/// (viewport_row_idx, display_column).
///
/// display_column is the sum of character.width() for all characters
/// before the target character in that row — NOT a char count or byte count.
/// This correctly handles wide characters (CJK, emoji).
///
/// `boundaries` is the table produced by `collect_and_build_logical_line`.
/// `viewport` is `&self.viewport`.
fn byte_offset_to_display_col(
    byte_offset: usize,
    boundaries: &[(usize, usize)],
    viewport: &VecDeque<Row>,
) -> Option<(usize, usize)> {
    if boundaries.is_empty() {
        return None;
    }
    // Find which row the byte offset falls in (last boundary whose byte_start <= offset).
    let boundary_idx = boundaries
        .partition_point(|&(_, byte_start)| byte_start <= byte_offset)
        .saturating_sub(1);
    let &(row_idx, row_byte_start) = boundaries.get(boundary_idx)?;
    let intra_byte_offset = byte_offset - row_byte_start;

    // Count display columns up to (but not including) the character at intra_byte_offset.
    let row = viewport.get(row_idx)?;
    let mut display_col = 0usize;
    let mut bytes_seen = 0usize;
    for ch in &row.columns {
        if bytes_seen >= intra_byte_offset {
            break;
        }
        bytes_seen += ch.character.len_utf8();
        display_col += ch.character.width().unwrap_or(1);
    }
    Some((row_idx, display_col))
}

/// Convert a regex match into a `Selection` spanning the matched display region.
/// Returns `None` if the boundary table or viewport lookup fails.
fn match_to_selection(
    mat: &regex::Match,
    boundaries: &[(usize, usize)],
    viewport: &VecDeque<Row>,
) -> Option<(Selection, usize, usize, usize, usize)> {
    let (start_row, start_col) = byte_offset_to_display_col(mat.start(), boundaries, viewport)?;
    let (end_row, end_col) = byte_offset_to_display_col(mat.end(), boundaries, viewport)?;
    let mut sel = Selection::default();
    sel.set_start_and_end_positions(
        Position::new(start_row as i32, start_col as u16),
        Position::new(end_row as i32, end_col as u16),
    );
    Some((sel, start_row, start_col, end_row, end_col))
}

/// Extract the effective match from a set of captures.
/// If capture group 1 exists, it is used (allowing patterns to include
/// context such as surrounding whitespace in the full match while
/// highlighting only the content in group 1). Otherwise the full match
/// (group 0) is returned.
fn highlight_match<'t>(captures: &regex::Captures<'t>) -> Option<regex::Match<'t>> {
    captures.get(1).or_else(|| captures.get(0))
}

fn highlight_matches<'r, 't>(
    regex: &'r regex::Regex,
    text: &'t str,
) -> impl Iterator<Item = regex::Match<'t>> + 'r
where
    't: 'r,
{
    let mut pos = 0;
    std::iter::from_fn(move || {
        while pos <= text.len() {
            let captures = regex.captures_at(text, pos)?;
            let mat = highlight_match(&captures);
            let end = match (mat, captures.get(0)) {
                (Some(m), _) => m.end(),
                (None, Some(whole)) => whole.end(),
                (None, None) => return None,
            };
            pos = if end > pos && mat.is_none_or(|m| !m.is_empty()) {
                end
            } else {
                text[end..]
                    .chars()
                    .next()
                    .map(|c| end + c.len_utf8())
                    .unwrap_or(text.len() + 1)
            };
            if mat.is_some() {
                return mat;
            }
        }
        None
    })
}

/// Check whether a (row, col) position falls within a display span.
/// The span is inclusive at start and exclusive at end.
fn position_in_span(
    row: usize,
    col: usize,
    start_row: usize,
    start_col: usize,
    end_row: usize,
    end_col: usize,
) -> bool {
    let after_start = row > start_row || (row == start_row && col >= start_col);
    let before_end = row < end_row || (row == end_row && col < end_col);
    after_start && before_end
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RegionRowsScrolled {
    IntoScrollback,
    Discarded,
    Nothing,
}

#[derive(Clone, Copy)]
struct KittyRowSnapshot {
    lines_above_len: usize,
    front_drops: u64,
    merged_rows: u64,
    split_rows: u64,
}

pub struct Grid {
    pub(crate) lines_above: VecDeque<Row>,
    pub(crate) viewport: VecDeque<Row>,
    pub(crate) lines_below: VecDeque<Row>,
    horizontal_tabstops: BTreeSet<usize>,
    alternate_screen_state: Option<AlternateScreenState>,
    cursor: Cursor,
    cursor_is_hidden: bool,
    saved_cursor_position: Option<Cursor>,
    scroll_region: (usize, usize),
    active_charset: CharsetIndex,
    preceding_char: Option<TerminalCharacter>,
    #[allow(dead_code)]
    terminal_emulator_colors: Rc<RefCell<Palette>>,
    #[allow(dead_code)]
    terminal_emulator_color_codes: Rc<RefCell<HashMap<usize, String>>>,
    pub(crate) output_buffer: OutputBuffer,
    title_stack: Vec<String>,
    character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
    sixel_grid: SixelGrid,
    kitty_grid: KittyGrid,
    kitty_parser: KittyCommandParser,
    kitty_host_support: KittyHostSupport,
    sixel_host_support: bool,
    pub changed_colors: Option<[Option<AnsiCode>; 256]>,
    pub should_render: bool,
    pub lock_renders: bool,
    // Completed content: synchronized output may mutate the viewport before a forced repaint.
    completed_text: Vec<Vec<CharacterChunk>>,
    completed_sixel: Vec<SixelImageChunk>,
    completed_kitty: Vec<KittyImageChunk>,
    repaint_completed_text: bool,
    pub cursor_key_mode: bool, // DECCKM - when set, cursor keys should send ANSI direction codes (eg. "OD") instead of the arrow keys (eg. "[D")
    pub bracketed_paste_mode: bool, // when set, paste instructions to the terminal should be escaped with a special sequence
    pub erasure_mode: bool,         // ERM
    pub sixel_scrolling: bool,      // DECSDM
    pub insert_mode: bool,
    pub disable_linewrap: bool,
    pub new_line_mode: bool, // Automatic newline LNM
    pub clear_viewport_before_rendering: bool,
    pub width: usize,
    pub height: usize,
    pub pending_messages_to_pty: Vec<Vec<u8>>,
    pub selection: Selection,
    agent_theme: Rc<crate::agent_theme::SourcePalette>,
    agent_palettes: Vec<std::rc::Weak<crate::agent_theme::SourcePalette>>,
    agent_launch: Option<yinz_types::agent::theme::AgentThemeLaunch>,
    pub pending_agent_theme_updates: Vec<yinz_types::agent::theme::AgentThemeEvent>,
    // The query owns this association, even if a different run starts before its reply.
    agent_color_queries: VecDeque<(u8, std::rc::Weak<crate::agent_theme::SourcePalette>)>,
    history_replay: Option<(
        yinz_types::agent::theme::AgentThemeHandoff,
        Vec<Rc<crate::agent_theme::SourcePalette>>,
    )>,
    copied_text_selection: Option<Selection>,
    text_selection_active: bool,
    pub title: Option<String>,
    pub is_scrolled: bool,
    pub link_handler: Rc<RefCell<LinkHandler>>,
    pub ring_bell: bool,
    scrollback_buffer_lines: usize,
    pub mouse_mode: MouseMode,
    pub mouse_tracking: MouseTracking,
    pub focus_event_tracking: bool,
    /// Has the app in this pane subscribed to host color-palette theme
    /// notifications via `CSI ? 2031 h`? When true, host-emitted DSR 997
    /// notifications (received by the client and forwarded as
    /// `ScreenInstruction::HostTerminalThemeChanged`) are pushed onto
    /// `pending_messages_to_pty` for this pane.
    pub color_palette_notification_enabled: bool,
    pub search_results: SearchResult,
    pub pending_clipboard_update: Option<String>,
    pub pending_osc7_cwd: Option<std::path::PathBuf>,
    pub pending_desktop_notifications: Vec<PendingNotification>,
    notification_tracker: NotificationTracker,
    /// Whitelisted host-terminal queries intercepted from the app running
    /// in this pane (CSI 14t / 16t pixel-dim queries, OSC 10;? / 11;? /
    /// 4;N;? color queries). Each entry is the raw byte sequence that
    /// Yinz should forward to the host terminal; the host's reply is
    /// later routed back to this pane's pty.
    pub pending_forwarded_queries: Vec<crate::host_query::HostQuery>,
    pub pending_nested_session_messages: Vec<NestedSessionMessage>,
    ui_component_bytes: Option<Vec<u8>>,
    nested_frame_bytes: Option<Vec<u8>>,
    xtgettcap_bytes: Option<Vec<u8>>,
    style: Style,
    debug: bool,
    arrow_fonts: bool,
    styled_underlines: bool,
    osc8_hyperlinks: bool,
    pub supports_kitty_keyboard_protocol: bool, // has the app requested kitty keyboard support?
    explicitly_disable_kitty_keyboard_protocol: bool, // has kitty keyboard support been explicitly
    // disabled by user config?
    click: Click,
    hyperlink_tracker: HyperlinkTracker,
    /// Pane-scoped override for the default foreground colour. Narrow
    /// to literal RGB by construction — the setters below refuse
    /// palette-indexed / named variants silently, so this field can be
    /// converted to an `OSC 10` reply without fallibility and the
    /// render path can wrap it unconditionally.
    pub pane_default_fg: Option<(u8, u8, u8)>,
    /// Pane-scoped override for the default background colour. Same
    /// invariant as `pane_default_fg`.
    pub pane_default_bg: Option<(u8, u8, u8)>,
    application_fg: Option<(u8, u8, u8)>,
    application_bg: Option<(u8, u8, u8)>,
    pub plugin_highlights: HashMap<PluginId, Vec<(String, CompiledHighlight)>>,
    // key: plugin_id (u32), inner vec: (pattern, compiled) pairs
    pub hover_position: Option<Position>, // pane-relative cursor cell; None when outside pane
    pub cached_hover_tooltip: Option<String>,
    osc133_markers_seen: bool,
    osc133_command_selection: bool,
    /// The output of the last copied command, painted for a moment as feedback for
    /// `CopyLastCommandOutput`; cleared by `BackgroundJob::ClearCommandOutputFlash`.
    command_output_flash: Option<Selection>,
    word_separators: String,
    osc7_payload: Option<String>,
}

impl Grid {
    pub fn host_colors(&self) -> (PaletteColor, PaletteColor) {
        let palette = self.terminal_emulator_colors.borrow();
        (palette.fg, palette.bg)
    }
    fn effective_foreground(&self) -> Option<(u8, u8, u8)> {
        self.application_fg
            .or(self.pane_default_fg)
            .or(self.style.terminal.map(|t| t.foreground))
    }
    fn effective_background(&self) -> Option<(u8, u8, u8)> {
        self.application_bg
            .or(self.pane_default_bg)
            .or(self.style.terminal.map(|t| t.background))
    }
    fn theme_background(&self) -> bool {
        self.style.terminal.is_some()
            && self.application_bg.is_none()
            && self.pane_default_bg.is_none()
    }
    fn effective_colors(&self) -> Option<[Option<AnsiCode>; 256]> {
        let Some(theme) = self.style.terminal else {
            return self.changed_colors;
        };
        let mut colors = self.changed_colors.unwrap_or([None; 256]);
        for (color, rgb) in colors.iter_mut().zip(theme.ansi) {
            *color = color.or(Some(AnsiCode::RgbCode(rgb)));
        }
        Some(colors)
    }
    pub fn update_terminal_theme(&mut self, theme: Option<yinz_types::style::TerminalTheme>) {
        if self.style.terminal == theme {
            return;
        }
        self.style.terminal = theme;
        self.recompile_agent_theme();
        let colors = self.effective_colors();
        let fg = self.effective_foreground().map(AnsiCode::RgbCode);
        let bg = self.effective_background().map(AnsiCode::RgbCode);
        let theme_background = self.theme_background();
        for chunk in self.completed_text.iter_mut().flatten() {
            chunk.theme_background = theme_background;
            chunk.add_changed_colors(colors);
            chunk.add_pane_defaults(fg, bg);
        }
        self.output_buffer.update_all_lines();
        self.repaint_completed_text = true;
        self.should_render = true;
    }
    pub fn set_pane_default_colors(&mut self, fg: Option<String>, bg: Option<String>) {
        // Parse inputs; anything that isn't literal RGB (palette
        // index / named colour / parse failure) is silently dropped so
        // the invariant on `pane_default_{fg,bg}` is preserved.
        self.pane_default_fg = fg
            .as_ref()
            .and_then(|s| xparse_color(s.as_bytes()))
            .and_then(rgb_of_ansi_code);
        self.pane_default_bg = bg
            .as_ref()
            .and_then(|s| xparse_color(s.as_bytes()))
            .and_then(rgb_of_ansi_code);
        self.output_buffer.update_all_lines();
    }
    pub fn get_pane_default_color_strings(&self) -> (Option<String>, Option<String>) {
        (
            self.pane_default_fg.map(rgb_to_hex_string),
            self.pane_default_bg.map(rgb_to_hex_string),
        )
    }
}

/// Extract the RGB triple from an `AnsiCode`, or `None` for any
/// other variant. Used at the ingress points that populate
/// `Grid::pane_default_{fg,bg}` to keep those fields narrow to
/// literal RGB.
fn rgb_of_ansi_code(code: AnsiCode) -> Option<(u8, u8, u8)> {
    match code {
        AnsiCode::RgbCode(rgb) => Some(rgb),
        _ => None,
    }
}

fn rgb_to_hex_string((r, g, b): (u8, u8, u8)) -> String {
    format!("#{:02x}{:02x}{:02x}", r, g, b)
}

/// Format an RGB triple as the body of an OSC 10/11 reply —
/// `rgb:RRRR/GGGG/BBBB` per xterm's ctlseqs, where each 8-bit
/// component is widened to the 16-bit hex form by repetition
/// (`0xAB` → `0xABAB`).
fn osc_color_reply_body((r, g, b): (u8, u8, u8)) -> String {
    let expand = |c: u8| (c as u16) * 0x0101;
    format!("rgb:{:04x}/{:04x}/{:04x}", expand(r), expand(g), expand(b))
}

fn decode_hex_ascii(hex: &[u8]) -> Option<String> {
    if hex.is_empty() || !hex.len().is_multiple_of(2) {
        return None;
    }
    let mut out = String::with_capacity(hex.len() / 2);
    for pair in hex.chunks(2) {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        let byte = (high * 16 + low) as u8;
        if !byte.is_ascii_graphic() {
            return None;
        }
        out.push(byte as char);
    }
    Some(out)
}

fn encode_hex_ascii(value: &str) -> String {
    value
        .bytes()
        .map(|byte| format!("{:02X}", byte))
        .collect::<String>()
}

/// A compiled highlight entry for one plugin/pattern combination.
#[derive(Clone)]
pub struct CompiledHighlight {
    pub regex: regex::Regex,
    pub fg: Option<AnsiCode>,
    pub bg: Option<AnsiCode>,
    pub context: BTreeMap<String, String>,
    pub on_hover: bool,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub tooltip_text: Option<String>,
    pub layer: HighlightLayer,
}

impl CompiledHighlight {
    /// Whether this highlight would produce any visible styling change.
    pub fn has_visual_effect(&self) -> bool {
        self.bg.is_some() || self.fg.is_some() || self.bold || self.italic || self.underline
    }
}

const CLICK_TIME_THRESHOLD: u128 = 400; // Doherty Threshold

#[derive(Clone, Debug, Default)]
struct Click {
    position_and_time: Option<(Position, std::time::Instant)>,
    count: usize,
}

impl Click {
    pub fn record_click(&mut self, position: Position) {
        let click_is_same_position_as_last_click = self
            .position_and_time
            .map(|(p, _t)| p == position)
            .unwrap_or(false);
        let click_is_within_time_threshold = self
            .position_and_time
            .map(|(_p, t)| t.elapsed().as_millis() <= CLICK_TIME_THRESHOLD)
            .unwrap_or(false);
        if click_is_same_position_as_last_click && click_is_within_time_threshold {
            self.count += 1;
        } else {
            self.count = 1;
        }
        self.position_and_time = Some((position, std::time::Instant::now()));
        if self.count == 4 {
            self.reset();
        }
    }
    pub fn is_double_click(&self) -> bool {
        self.count == 2
    }
    pub fn is_triple_click(&self) -> bool {
        self.count == 3
    }
    pub fn reset(&mut self) {
        self.count = 0;
    }
}

#[derive(Clone, Debug, Default)]
pub enum MouseMode {
    #[default]
    NoEncoding,
    Utf8,
    Sgr,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum MouseTracking {
    #[default]
    Off,
    Normal,
    ButtonEventTracking,
    AnyEventTracking,
}

impl Debug for Grid {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut buffer: Vec<Row> = Vec::from(self.viewport.clone());
        // pad buffer
        for _ in buffer.len()..self.height {
            buffer.push(Row::new().canonical());
        }

        // display sixel placeholder
        let sixel_indication_character = |x| {
            let sixel_indication_word = "Sixel";
            sixel_indication_word
                .chars()
                .nth(x % sixel_indication_word.len())
                .unwrap()
        };
        for image_coordinates in self
            .sixel_grid
            .image_cell_coordinates_in_viewport(self.height, self.lines_above.len())
        {
            let (image_top_edge, image_bottom_edge, image_left_edge, image_right_edge) =
                image_coordinates;
            for y in image_top_edge..image_bottom_edge {
                let row = buffer.get_mut(y).unwrap();
                for x in image_left_edge..image_right_edge {
                    let fake_sixel_terminal_character =
                        TerminalCharacter::new_singlewidth(sixel_indication_character(x));
                    row.add_character_at(fake_sixel_terminal_character, x);
                }
            }
        }

        let kitty_indication_character = |x| {
            let kitty_indication_word = "KittyImage";
            kitty_indication_word
                .chars()
                .nth(x % kitty_indication_word.len())
                .unwrap()
        };
        for image_coordinates in self
            .kitty_grid
            .image_cell_coordinates_in_viewport(self.height, self.lines_above.len())
        {
            let (image_top_edge, image_bottom_edge, image_left_edge, image_right_edge) =
                image_coordinates;
            for y in image_top_edge..image_bottom_edge {
                if let Some(row) = buffer.get_mut(y) {
                    for x in image_left_edge..image_right_edge {
                        let fake_kitty_terminal_character =
                            TerminalCharacter::new_singlewidth(kitty_indication_character(x));
                        row.add_character_at(fake_kitty_terminal_character, x);
                    }
                }
            }
        }

        // display terminal characters with stripped styles
        for (i, row) in buffer.iter().enumerate() {
            let mut cow_row = Cow::Borrowed(row);
            self.search_results
                .mark_search_results_in_row(&mut cow_row, i);
            if row.is_canonical {
                writeln!(f, "{:02?} (C): {:?}", i, cow_row)?;
            } else {
                writeln!(f, "{:02?} (W): {:?}", i, cow_row)?;
            }
        }
        Ok(())
    }
}

fn resolve_highlight_colors(
    style_decl: &HighlightStyle,
    style: &Style,
) -> (Option<AnsiCode>, Option<AnsiCode>) {
    let palette_to_ansi = |c: PaletteColor| -> AnsiCode {
        match c {
            PaletteColor::Rgb(rgb) => AnsiCode::RgbCode(rgb),
            PaletteColor::EightBit(i) => AnsiCode::ColorIndex(i),
        }
    };
    let tu = &style.colors.text_unselected;
    let emphasis = [tu.emphasis_0, tu.emphasis_1, tu.emphasis_2, tu.emphasis_3];
    match style_decl {
        HighlightStyle::None => (None, None),
        HighlightStyle::Emphasis0 => (Some(palette_to_ansi(emphasis[0])), None),
        HighlightStyle::Emphasis1 => (Some(palette_to_ansi(emphasis[1])), None),
        HighlightStyle::Emphasis2 => (Some(palette_to_ansi(emphasis[2])), None),
        HighlightStyle::Emphasis3 => (Some(palette_to_ansi(emphasis[3])), None),
        HighlightStyle::BackgroundEmphasis0 => (
            Some(palette_to_ansi(tu.background)),
            Some(palette_to_ansi(emphasis[0])),
        ),
        HighlightStyle::BackgroundEmphasis1 => (
            Some(palette_to_ansi(tu.background)),
            Some(palette_to_ansi(emphasis[1])),
        ),
        HighlightStyle::BackgroundEmphasis2 => (
            Some(palette_to_ansi(tu.background)),
            Some(palette_to_ansi(emphasis[2])),
        ),
        HighlightStyle::BackgroundEmphasis3 => (
            Some(palette_to_ansi(tu.background)),
            Some(palette_to_ansi(emphasis[3])),
        ),
        HighlightStyle::CustomRgb { fg, bg } => {
            (fg.map(AnsiCode::RgbCode), bg.map(AnsiCode::RgbCode))
        },
        HighlightStyle::CustomIndex { fg, bg } => {
            (fg.map(AnsiCode::ColorIndex), bg.map(AnsiCode::ColorIndex))
        },
    }
}

/// One rendered frame of a grid: its character chunks, the raw VTE output to pass through if
/// there is any, and its sixel and kitty image chunks.
pub type RenderedGridFrame = (
    Vec<CharacterChunk>,
    Option<String>,
    Vec<SixelImageChunk>,
    Vec<KittyImageChunk>,
);

/// The plugin highlight found under a position so far, ranked by its layer:
/// `(layer, plugin, pattern, matched text, context)`.
type RankedHighlight = (
    HighlightLayer,
    PluginId,
    String,
    String,
    BTreeMap<String, String>,
);

impl Grid {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        rows: usize,
        columns: usize,
        terminal_emulator_colors: Rc<RefCell<Palette>>,
        terminal_emulator_color_codes: Rc<RefCell<HashMap<usize, String>>>,
        link_handler: Rc<RefCell<LinkHandler>>,
        character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
        sixel_image_store: Rc<RefCell<SixelImageStore>>,
        kitty_image_store: Rc<RefCell<KittyImageStore>>,
        style: Style,
        debug: bool,
        arrow_fonts: bool,
        styled_underlines: bool,
        osc8_hyperlinks: bool,
        explicitly_disable_kitty_keyboard_protocol: bool,
    ) -> Self {
        let sixel_grid = SixelGrid::new(character_cell_size.clone(), sixel_image_store);
        let kitty_grid = KittyGrid::new(character_cell_size.clone(), kitty_image_store);
        // make sure this is initialized as it is used internally
        // if it was already initialized (which should happen normally unless this is a test or
        // something changed since this comment was written), we get an Error which we ignore
        // I don't know why this needs to be a OneCell, but whatevs
        let _ = SCROLL_BUFFER_SIZE.set(DEFAULT_SCROLL_BUFFER_SIZE);
        Grid {
            lines_above: VecDeque::new(),
            viewport: VecDeque::from(vec![Row::new().canonical()]),
            lines_below: VecDeque::new(),
            horizontal_tabstops: create_horizontal_tabstops(columns),
            cursor: Cursor::new(0, 0, styled_underlines),
            cursor_is_hidden: false,
            saved_cursor_position: None,
            scroll_region: (0, rows.saturating_sub(1)),
            preceding_char: None,
            width: columns,
            height: rows,
            should_render: true,
            cursor_key_mode: false,
            bracketed_paste_mode: false,
            erasure_mode: false,
            sixel_scrolling: false,
            insert_mode: false,
            disable_linewrap: false,
            new_line_mode: false,
            alternate_screen_state: None,
            clear_viewport_before_rendering: false,
            active_charset: Default::default(),
            pending_messages_to_pty: vec![],
            terminal_emulator_colors,
            terminal_emulator_color_codes,
            output_buffer: Default::default(),
            selection: Default::default(),
            agent_theme: Rc::new(crate::agent_theme::SourcePalette::new(None, style)),
            agent_palettes: vec![],
            agent_launch: None,
            pending_agent_theme_updates: vec![],
            agent_color_queries: VecDeque::new(),
            history_replay: None,
            title_stack: vec![],
            title: None,
            changed_colors: None,
            is_scrolled: false,
            link_handler,
            ring_bell: false,
            scrollback_buffer_lines: 0,
            mouse_mode: MouseMode::default(),
            mouse_tracking: MouseTracking::default(),
            focus_event_tracking: false,
            color_palette_notification_enabled: false,
            character_cell_size,
            search_results: Default::default(),
            sixel_grid,
            kitty_grid,
            kitty_parser: KittyCommandParser::new(),
            kitty_host_support: KittyHostSupport::Supported,
            sixel_host_support: true,
            pending_clipboard_update: None,
            pending_osc7_cwd: None,
            pending_desktop_notifications: Vec::new(),
            notification_tracker: NotificationTracker::default(),
            pending_forwarded_queries: Vec::new(),
            pending_nested_session_messages: Vec::new(),
            ui_component_bytes: None,
            nested_frame_bytes: None,
            xtgettcap_bytes: None,
            style,
            debug,
            arrow_fonts,
            styled_underlines,
            osc8_hyperlinks,
            lock_renders: false,
            completed_text: vec![],
            completed_sixel: vec![],
            completed_kitty: vec![],
            repaint_completed_text: false,
            supports_kitty_keyboard_protocol: false,
            explicitly_disable_kitty_keyboard_protocol,
            click: Click::default(),
            hyperlink_tracker: HyperlinkTracker::new(),
            pane_default_fg: None,
            pane_default_bg: None,
            application_fg: None,
            application_bg: None,
            plugin_highlights: HashMap::new(),
            hover_position: None,
            cached_hover_tooltip: None,
            osc133_markers_seen: false,
            osc133_command_selection: true,
            command_output_flash: None,
            copied_text_selection: None,
            text_selection_active: false,
            word_separators: DEFAULT_WORD_SEPARATORS.to_owned(),
            osc7_payload: None,
        }
    }
    pub fn set_selection_options(&mut self, osc133_command_selection: bool, word_separators: &str) {
        self.osc133_command_selection = osc133_command_selection;
        if self.word_separators != word_separators {
            self.word_separators = word_separators.to_owned();
        }
    }
    /// Last accepted OSC 7 URI, retained after the internal CWD update is drained.
    pub fn osc7_payload(&self) -> Option<&str> {
        self.osc7_payload.as_deref()
    }
    pub fn render_full_viewport(&mut self) {
        self.repaint_completed_text = true;
        self.output_buffer.update_all_lines();
    }
    /// Make the next render repeat what the last one sent, for watchers that mirror a
    /// client whose render has already consumed the changes. Only valid with no output in
    /// between.
    pub fn replay_last_render(&mut self) {
        if self.lock_renders {
            // A locked grid only ever renders its whole completed text, which leaves the
            // changed lines for after the unlock.
            self.repaint_completed_text = true;
        } else {
            self.output_buffer.replay_last_read();
        }
    }
    pub fn update_line_for_rendering(&mut self, line_index: usize) {
        self.output_buffer.update_line(line_index);
    }
    pub fn advance_to_next_tabstop(&mut self, styles: RcCharacterStyles) {
        let next_tabstop = self
            .horizontal_tabstops
            .iter()
            .copied()
            .find(|&tabstop| tabstop > self.cursor.x && tabstop < self.width);
        match next_tabstop {
            Some(tabstop) => {
                self.cursor.x = tabstop;
            },
            None => {
                self.cursor.x = self.width.saturating_sub(1);
            },
        }
        let mut empty_character = EMPTY_TERMINAL_CHARACTER;
        empty_character.styles = styles;
        self.pad_current_line_until(self.cursor.x, empty_character);
        self.output_buffer.update_line(self.cursor.y);
    }
    pub fn move_to_previous_tabstop(&mut self) {
        let previous_tabstop = self
            .horizontal_tabstops
            .iter()
            .rev()
            .copied()
            .find(|&tabstop| tabstop < self.cursor.x);
        match previous_tabstop {
            Some(tabstop) => {
                self.cursor.x = tabstop;
            },
            None => {
                self.cursor.x = 0;
            },
        }
    }
    pub fn cursor_shape(&self) -> CursorShape {
        self.cursor.get_shape()
    }
    pub fn scrollback_position_and_length(&self) -> (usize, usize) {
        // (position, length)
        (
            self.lines_below.len(),
            (self.scrollback_buffer_lines + self.lines_below.len()),
        )
    }

    fn recalculate_scrollback_buffer_count(&mut self) -> usize {
        let mut scrollback_buffer_count = 0;
        for row in &mut self.lines_above {
            let row_width = row.width_cached();
            // rows in lines_above are unwrapped, so we need to account for that
            if row_width > self.width {
                scrollback_buffer_count += calculate_row_display_height(row_width, self.width);
            } else {
                scrollback_buffer_count += 1;
            }
        }
        scrollback_buffer_count
    }

    fn set_horizontal_tabstop(&mut self) {
        self.horizontal_tabstops.insert(self.cursor.x);
    }
    fn clear_tabstop(&mut self, position: usize) {
        self.horizontal_tabstops.remove(&position);
    }
    fn clear_all_tabstops(&mut self) {
        self.horizontal_tabstops.clear();
    }
    fn save_cursor_position(&mut self) {
        self.saved_cursor_position = Some(self.cursor.clone());
    }
    fn restore_cursor_position(&mut self) {
        if let Some(saved_cursor_position) = &self.saved_cursor_position {
            self.cursor = saved_cursor_position.clone();
        }
    }
    fn configure_charset(&mut self, charset: StandardCharset, index: CharsetIndex) {
        self.cursor.charsets[index] = charset;
    }
    fn set_active_charset(&mut self, index: CharsetIndex) {
        self.active_charset = index;
    }
    fn cursor_canonical_line_index(&self) -> usize {
        let mut cursor_canonical_line_index = 0;
        let mut canonical_lines_traversed = 0;
        for (i, line) in self.viewport.iter().enumerate() {
            if line.is_canonical {
                cursor_canonical_line_index = canonical_lines_traversed;
                canonical_lines_traversed += 1;
            }
            if i == self.cursor.y {
                break;
            }
        }
        cursor_canonical_line_index
    }
    fn cursor_index_in_canonical_line(&self, cursor: &Cursor) -> usize {
        let mut cursor_canonical_line_index = 0;
        let mut cursor_index_in_canonical_line = 0;
        for (i, line) in self.viewport.iter().enumerate() {
            if line.is_canonical {
                cursor_canonical_line_index = i;
            }
            if i == cursor.y {
                let line_wraps = cursor.y.saturating_sub(cursor_canonical_line_index);
                cursor_index_in_canonical_line = (line_wraps * self.width) + cursor.x;
                break;
            }
        }
        cursor_index_in_canonical_line
    }
    fn canonical_line_y_coordinates(&self, canonical_line_index: usize) -> usize {
        let mut canonical_lines_traversed = 0;
        let mut y_coordinates = 0;
        for (i, line) in self.viewport.iter().enumerate() {
            if line.is_canonical {
                canonical_lines_traversed += 1;
                y_coordinates = i;
                if canonical_lines_traversed == canonical_line_index + 1 {
                    break;
                }
            }
        }
        y_coordinates
    }
    fn kitty_canonical_line_starts(&self) -> Vec<usize> {
        let mut starts = Vec::new();
        for (wrapped_row, row) in self
            .lines_above
            .iter()
            .chain(self.viewport.iter())
            .chain(self.lines_below.iter())
            .enumerate()
        {
            if row.is_canonical {
                starts.push(wrapped_row);
            }
        }
        starts
    }
    fn kitty_anchor_from_pixel_y(
        pixel_y: isize,
        cell_height: isize,
        starts: &[usize],
    ) -> KittyVerticalAnchor {
        let wrapped_row = pixel_y.div_euclid(cell_height);
        let canonical_line = match starts.binary_search(&wrapped_row.max(0).try_into().unwrap_or(0))
        {
            Ok(index) => index,
            Err(index) => index.saturating_sub(1),
        };
        let line_start_px = *starts.get(canonical_line).unwrap_or(&0) as isize * cell_height;
        KittyVerticalAnchor {
            canonical_line,
            offset_px_from_line_start: pixel_y - line_start_px,
        }
    }
    fn kitty_pixel_y_from_anchor(
        anchor: &KittyVerticalAnchor,
        cell_height: isize,
        starts: &[usize],
    ) -> Option<isize> {
        starts
            .get(anchor.canonical_line)
            .map(|line_start| *line_start as isize * cell_height + anchor.offset_px_from_line_start)
    }
    /// Rebuild the placements that unicode placeholder cells stand for.
    ///
    /// Spec, "Unicode placeholders": a virtual placement is a prototype, and the cells printing
    /// [`PLACEHOLDER`] are what instantiate it. Each cell names an image through its foreground
    /// colour, a placement of that image through its underline colour, and its own row and column
    /// within the image through combining marks - inheriting any of the three from the cell to its
    /// left when it does not spell them out.
    ///
    /// Derived wholesale from the viewport rather than maintained incrementally: the cells *are*
    /// the placement, so erasing a line, scrolling, resizing and reflowing all fall out correctly
    /// with no hook in any of them. Costs nothing until a client actually creates a virtual
    /// placement, which is the gate on the first line.
    fn kitty_refresh_placeholder_placements(&mut self) {
        // Relative placements are re-derived here too: their parents move with the grid, and a
        // virtual parent's position comes from the placeholder cells this rebuilds.
        if !self.kitty_grid.has_virtual_placements() {
            if let Some(cell) = *self.character_cell_size.borrow() {
                self.kitty_grid.resolve_relative_placements(cell);
            }
            return;
        }
        let cell = match *self.character_cell_size.borrow() {
            Some(cell) => cell,
            None => return,
        };
        let runs = self.kitty_placeholder_runs();
        let starts = self.kitty_canonical_line_starts();
        let scrollback_rows = self.lines_above.len();
        let mut live_uids = HashSet::with_capacity(runs.len());
        let mut derived = Vec::with_capacity(runs.len());
        for run in runs {
            // The raster is already fitted to the whole `c` x `r` box. Crop at cell
            // boundaries, matching the screen positions below; dividing it into `r`
            // equal strips would leave gaps whenever the fitted height is smaller.
            let (fit_w, fit_h) = run.fitted_px;
            let slice_x = run.column_in_image as usize * cell.width;
            let slice_y = run.row_in_image as usize * cell.height;
            let end_x = ((run.column_in_image as usize + run.span) * cell.width).min(fit_w);
            let end_y = ((run.row_in_image as usize + 1) * cell.height).min(fit_h);
            if slice_x >= end_x || slice_y >= end_y {
                continue;
            }
            let slice_w = end_x - slice_x;
            let slice_h = end_y - slice_y;
            let key = (
                run.image_id,
                run.placement_id,
                scrollback_rows + run.screen_row,
                run.screen_column,
            );
            live_uids.insert(key);
            let placement_uid = self.kitty_grid.placeholder_uid(key);
            let display_rect = PixelRect {
                x: run.screen_column * cell.width,
                y: ((scrollback_rows + run.screen_row) * cell.height) as isize,
                width: slice_w,
                height: slice_h,
            };
            let vertical_anchor =
                Self::kitty_anchor_from_pixel_y(display_rect.y, cell.height as isize, &starts);
            derived.push(KittyPlacement {
                image_id: run.image_id,
                placement_id: run.placement_id,
                placement_uid,
                internal_id: run.internal_id,
                display_rect,
                position_cell_size: cell,
                source_rect: PixelRect {
                    x: slice_x,
                    y: slice_y as isize,
                    width: slice_w,
                    height: slice_h,
                },
                emit_x: slice_x,
                emit_y: slice_y,
                scaled_px: Some(run.fitted_px),
                dest_cells: (run.span as u16, 1),
                variant_key: run.variant_key,
                cell_offset: (0, 0),
                z_index: run.z_index,
                vertical_anchor,
                is_placeholder: true,
                relative_to: None,
            });
        }
        self.kitty_grid.retain_placeholder_uids(&live_uids);
        self.kitty_grid.set_placeholder_placements(derived);
        self.kitty_grid.resolve_relative_placements(cell);
    }
    /// Walk the viewport and coalesce adjacent placeholder cells into the runs each of which
    /// becomes one placement. A run is broken by anything that is not the next column of the same
    /// row of the same placement - a gap, a different image, a jump in the image column.
    fn kitty_placeholder_runs(&self) -> Vec<KittyPlaceholderRun> {
        let mut runs: Vec<KittyPlaceholderRun> = Vec::new();
        for (screen_row, row) in self.viewport.iter().enumerate() {
            let mut previous: Option<ResolvedPlaceholderCell> = None;
            for screen_column in 0..self.width {
                let resolved = self.kitty_resolve_placeholder_cell(row, screen_column, &previous);
                if let Some(cell) = &resolved {
                    {
                        let continues_the_run = previous
                            .as_ref()
                            .zip(runs.last())
                            .map(|(previous, run)| {
                                previous.image_id == cell.image_id
                                    && previous.placement_id == cell.placement_id
                                    && previous.row_in_image == cell.row_in_image
                                    && previous.column_in_image + 1 == cell.column_in_image
                                    && run.screen_row == screen_row
                                    && run.screen_column + run.span == screen_column
                            })
                            .unwrap_or(false);
                        if continues_the_run {
                            if let Some(run) = runs.last_mut() {
                                run.span += 1;
                            }
                        } else if let Some(virtual_placement) = self
                            .kitty_grid
                            .virtual_placements()
                            .iter()
                            .find(|candidate| {
                                candidate.image_id == cell.image_id
                                    && candidate.placement_id == cell.placement_id
                            })
                        {
                            runs.push(KittyPlaceholderRun {
                                image_id: cell.image_id,
                                placement_id: cell.placement_id,
                                internal_id: virtual_placement.internal_id,
                                variant_key: virtual_placement.variant_key(),
                                fitted_px: virtual_placement.fitted_px,
                                z_index: virtual_placement.z_index,
                                row_in_image: cell.row_in_image,
                                column_in_image: cell.column_in_image,
                                screen_row,
                                screen_column,
                                span: 1,
                            });
                        }
                    }
                }
                previous = resolved;
            }
        }
        runs
    }
    /// Decode one cell, if it is a placeholder naming a virtual placement that exists.
    ///
    /// A stale placeholder left in the scrollback must not conjure up whatever image happens to
    /// hold its id now, so a cell whose image is unknown resolves to nothing.
    fn kitty_resolve_placeholder_cell(
        &self,
        row: &Row,
        screen_column: usize,
        previous: &Option<ResolvedPlaceholderCell>,
    ) -> Option<ResolvedPlaceholderCell> {
        let character = row.character_at(screen_column)?;
        if character.character != PLACEHOLDER {
            return None;
        }
        let foreground = character.styles.foreground;
        let underline_colour = character.styles.underline_color;
        let mark = row.placeholder_mark_at(screen_column).unwrap_or_default();
        // Spec: "If no diacritics are present, and the previous placeholder cell has the same
        // foreground and underline colors, then the row of the current cell will be the row of the
        // cell to the left, the column will be the column of the cell to the left plus one, and
        // the most significant image ID byte will be the most significant image ID byte of the
        // cell to the left."
        let inherited = previous.as_ref().filter(|previous| {
            previous.foreground == foreground && previous.underline_colour == underline_colour
        });
        let row_in_image = mark
            .row_in_image
            .or_else(|| inherited.map(|previous| previous.row_in_image))?;
        let column_in_image = match mark.column_in_image {
            Some(column) => column,
            // A cell with only a row mark takes the next column of that row.
            None => inherited
                .filter(|previous| previous.row_in_image == row_in_image)
                .map(|previous| previous.column_in_image + 1)?,
        };
        let id_most_significant_byte = mark
            .id_most_significant_byte
            .or_else(|| inherited.map(|previous| previous.id_most_significant_byte))
            .unwrap_or(0);
        let image_id = kitty_image_id_from_colour(foreground)?
            | ((id_most_significant_byte as u32 & 0xff) << 24);
        // Spec: "You can also specify a placement ID using the underline color (if it's omitted or
        // zero, the terminal may choose any virtual placement of the given image)."
        let requested_placement_id = kitty_image_id_from_colour(underline_colour).unwrap_or(0);
        let placement_id = self
            .kitty_grid
            .virtual_placements()
            .iter()
            .find(|candidate| {
                candidate.image_id == image_id
                    && (requested_placement_id == 0
                        || candidate.placement_id == requested_placement_id)
            })
            .map(|candidate| candidate.placement_id)?;
        Some(ResolvedPlaceholderCell {
            image_id,
            placement_id,
            row_in_image,
            column_in_image,
            id_most_significant_byte,
            foreground,
            underline_colour,
        })
    }
    fn kitty_rows_below_the_viewport(&self) -> Option<KittyRowsBelowTheViewport> {
        if self.lines_below.is_empty() {
            None
        } else {
            let first_row = self.lines_above.len() + self.viewport.len();
            Some(KittyRowsBelowTheViewport {
                first_row,
                total_rows: first_row + self.lines_below.len(),
            })
        }
    }
    fn kitty_settle_placements_below_the_viewport(&mut self) -> bool {
        let rows_below_the_viewport = self.kitty_rows_below_the_viewport();
        self.kitty_grid
            .settle_placements_below_the_viewport(rows_below_the_viewport)
    }
    fn kitty_reanchor_all_from_pixels(&mut self) {
        self.kitty_settle_placements_below_the_viewport();
        if self.kitty_grid.placement_count() == 0 {
            return;
        }
        let cell_height = match *self.character_cell_size.borrow() {
            Some(cell) => cell.height as isize,
            None => return,
        };
        let starts = self.kitty_canonical_line_starts();
        if starts.is_empty() {
            return;
        }
        for placement in self.kitty_grid.placements_mut() {
            placement.vertical_anchor =
                Self::kitty_anchor_from_pixel_y(placement.display_rect.y, cell_height, &starts);
        }
    }
    fn kitty_reproject_all_from_anchor(&mut self) {
        let rows_below_the_viewport = self.kitty_rows_below_the_viewport();
        self.kitty_grid
            .note_rows_below_the_viewport(rows_below_the_viewport);
        if self.kitty_grid.placement_count() == 0 {
            return;
        }
        let cell_height = match *self.character_cell_size.borrow() {
            Some(cell) => cell.height as isize,
            None => return,
        };
        let starts = self.kitty_canonical_line_starts();
        let line_count = starts.len();
        self.kitty_grid
            .retain_placements(|placement| placement.vertical_anchor.canonical_line < line_count);
        for placement in self.kitty_grid.placements_mut() {
            if let Some(pixel_y) =
                Self::kitty_pixel_y_from_anchor(&placement.vertical_anchor, cell_height, &starts)
            {
                placement.display_rect.y = pixel_y;
                placement.position_cell_size.height = cell_height as usize;
            }
        }
    }

    pub fn scroll_up_one_line(&mut self) -> bool {
        let mut found_something = false;
        if !self.lines_above.is_empty() && self.viewport.len() == self.height {
            self.is_scrolled = true;
            let line_to_push_down = self.viewport.pop_back().unwrap();
            self.lines_below.push_front(line_to_push_down);

            let transferred_rows_height = transfer_rows_from_lines_above_to_viewport(
                &mut self.lines_above,
                &mut self.viewport,
                &mut self.sixel_grid,
                &mut self.kitty_grid,
                1,
                self.width,
            );
            self.kitty_reanchor_all_from_pixels();
            self.scrollback_buffer_lines = self
                .scrollback_buffer_lines
                .saturating_sub(transferred_rows_height);

            self.selection.move_down(1);
            if let Some(copied_text_selection) = self.copied_text_selection.as_mut() {
                copied_text_selection.move_down(1);
            }
            if let Some(command_output_flash) = self.command_output_flash.as_mut() {
                command_output_flash.move_down(1);
            }
            // Move all search-selections down one line as well
            found_something =
                self.search_results
                    .move_down(1, &self.viewport, self.height, self.width);
        }
        self.output_buffer.update_all_lines();
        found_something
    }
    pub fn scroll_down_one_line(&mut self) -> bool {
        let mut found_something = false;
        if !self.lines_below.is_empty()
            && self.viewport.len() == self.height
            && !self.viewport.is_empty()
        {
            let mut line_to_push_up = self.viewport.pop_front().unwrap();

            self.scrollback_buffer_lines +=
                calculate_row_display_height(line_to_push_up.width(), self.width);

            let line_to_push_up = if line_to_push_up.is_canonical {
                line_to_push_up
            } else {
                match self.lines_above.pop_back() {
                    Some(mut last_line_above) => {
                        self.kitty_grid
                            .merge_rows_into_line_start(self.lines_above.len(), 1);
                        last_line_above.append(&mut line_to_push_up);
                        last_line_above
                    },
                    None => {
                        // in this case, this line was not canonical but its beginning line was
                        // dropped out of scope, so we make it canonical and push it up
                        self.kitty_grid.insert_canonical_anchor_line_at_front();
                        line_to_push_up.canonical()
                    },
                }
            };

            let dropped_line_width = bounded_push(
                &mut self.lines_above,
                &mut self.sixel_grid,
                &mut self.kitty_grid,
                line_to_push_up,
            );
            if let Some(width) = dropped_line_width {
                let dropped_line_height = calculate_row_display_height(width, self.width);

                self.scrollback_buffer_lines = self
                    .scrollback_buffer_lines
                    .saturating_sub(dropped_line_height);
            }

            transfer_rows_from_lines_below_to_viewport(
                &mut self.lines_below,
                &mut self.viewport,
                1,
                self.width,
            );
            self.kitty_reanchor_all_from_pixels();

            self.selection.move_up(1);
            if let Some(copied_text_selection) = self.copied_text_selection.as_mut() {
                copied_text_selection.move_up(1);
            }
            if let Some(command_output_flash) = self.command_output_flash.as_mut() {
                command_output_flash.move_up(1);
            }
            // Move all search-selections up one line as well
            found_something = self.search_results.move_up(
                1,
                &self.viewport,
                &self.lines_below,
                self.height,
                self.width,
            );
            self.output_buffer.update_all_lines();
        }
        if self.lines_below.is_empty() {
            self.is_scrolled = false;
        }
        found_something
    }
    pub fn force_change_size(&mut self, new_rows: usize, new_columns: usize) {
        // this is an ugly hack - it's here because sometimes we need to change_size to the
        // existing size (eg. when resizing an alternative_grid to the current height/width) and
        // the change_size method is a no-op in that case. Should be fixed by making the
        // change_size method atomic
        let intermediate_rows = if new_rows == self.height {
            new_rows + 1
        } else {
            new_rows
        };
        let intermediate_columns = if new_columns == self.width {
            new_columns + 1
        } else {
            new_columns
        };
        self.change_size(intermediate_rows, intermediate_columns);
        self.change_size(new_rows, new_columns);
    }
    pub fn character_cell_size_possibly_changed(&mut self) {
        self.sixel_grid.character_cell_size_possibly_changed();
        self.kitty_grid.character_cell_size_possibly_changed();
        self.kitty_reanchor_all_from_pixels();
    }
    pub fn change_size(&mut self, new_rows: usize, new_columns: usize) {
        // Do nothing if this pane hasn't been given a proper size yet
        if new_columns == 0 || new_rows == 0 {
            return;
        }
        if self.alternate_screen_state.is_some() {
            // in alternate screen we do nothing but log the new size, the program in the terminal
            // is in control now...
            self.height = new_rows;
            self.width = new_columns;
            self.set_scroll_region_to_viewport_size();
            self.output_buffer.update_all_lines();
            return;
        }
        self.selection.reset();
        self.command_output_flash = None;
        self.character_cell_size_possibly_changed();
        let cursors = if new_columns != self.width {
            self.horizontal_tabstops = create_horizontal_tabstops(new_columns);
            // Erasing scrollback can leave the viewport starting in a wrapped line. Its
            // visible remainder is now a new line, including for cursor bookkeeping.
            if self.lines_above.is_empty() {
                if let Some(first_row) = self.viewport.front_mut() {
                    first_row.writing(&self.agent_theme);
                    first_row.is_canonical = true;
                }
            }
            let mut cursor_canonical_line_index = self.cursor_canonical_line_index();
            let cursor_index_in_canonical_line = self.cursor_index_in_canonical_line(&self.cursor);
            let saved_cursor_index_in_canonical_line = self
                .saved_cursor_position
                .as_ref()
                .map(|cursor| self.cursor_index_in_canonical_line(cursor));
            let mut viewport_canonical_lines = vec![];
            for mut row in self.viewport.drain(..) {
                if row.is_canonical {
                    viewport_canonical_lines.push(row);
                } else if let Some(last_line) = viewport_canonical_lines.last_mut() {
                    last_line.append(&mut row);
                } else if let Some(mut first_line_above) = self.lines_above.pop_back() {
                    first_line_above.append(&mut row);
                    viewport_canonical_lines.push(first_line_above);
                    cursor_canonical_line_index += 1;
                }
            }

            // Remove unprinted padding before reflow, preserving explicitly printed spaces.
            for line in &mut viewport_canonical_lines {
                let mut trim_at = None;
                for (index, character) in line.columns.iter().enumerate() {
                    let is_trimmable_space = character.is_implicit_blank()
                        && matches!(character.styles.background, Some(AnsiCode::Reset) | None)
                        && matches!(line.bg_color, Some(AnsiCode::Reset) | None);

                    if !is_trimmable_space {
                        // we can't trim this character, meaning that if we had a previous
                        // character that we marked as the trim_at point, we need to clear it
                        trim_at = None;
                    } else if trim_at.is_none() {
                        // we CAN trim this character, set the trim_at point only if it's not set
                        // because we want the trim_at point to be the EARLIEST trimmable character
                        trim_at = Some(index);
                    }
                }
                if let Some(trim_at) = trim_at {
                    let excess_width_until_trim_at = line.excess_width_until(trim_at);
                    line.truncate(trim_at + excess_width_until_trim_at);
                }
            }

            let mut new_viewport_rows = vec![];
            for mut canonical_line in viewport_canonical_lines {
                let mut canonical_line_parts = canonical_line.split_to_rows_of_length(new_columns);
                // If a character is wider than the grid, split_to_rows_of_length returns an empty
                // vec — skip the line, matching the old `break` behavior
                if canonical_line_parts.is_empty() {
                    continue;
                }
                new_viewport_rows.append(&mut canonical_line_parts);
            }

            self.viewport = VecDeque::from(new_viewport_rows);

            let mut new_cursor_y = self.canonical_line_y_coordinates(cursor_canonical_line_index)
                + (cursor_index_in_canonical_line / new_columns);
            let mut saved_cursor_y_coordinates =
                self.saved_cursor_position.as_ref().map(|saved_cursor| {
                    self.canonical_line_y_coordinates(saved_cursor.y)
                        + saved_cursor_index_in_canonical_line.as_ref().unwrap() / new_columns
                });

            // A cursor at EOL has two equivalent positions - end of this line or beginning of
            // next. If not already at the beginning of line, bias to EOL so add character logic
            // doesn't create spurious canonical lines
            let mut new_cursor_x = cursor_index_in_canonical_line % new_columns;
            if self.cursor.x != 0 && new_cursor_x == 0 {
                new_cursor_y = new_cursor_y.saturating_sub(1);
                new_cursor_x = new_columns
            }
            let saved_cursor_x_coordinates = match (
                saved_cursor_index_in_canonical_line.as_ref(),
                self.saved_cursor_position.as_mut(),
                saved_cursor_y_coordinates.as_mut(),
            ) {
                (
                    Some(saved_cursor_index_in_canonical_line),
                    Some(saved_cursor_position),
                    Some(saved_cursor_y_coordinates),
                ) => {
                    let x = saved_cursor_position.x;
                    let mut new_x = *saved_cursor_index_in_canonical_line % new_columns;
                    let new_y = saved_cursor_y_coordinates;
                    if x != 0 && new_x == 0 {
                        *new_y = new_y.saturating_sub(1);
                        new_x = new_columns
                    }
                    Some(new_x)
                },
                _ => None,
            };
            Some((
                new_cursor_y,
                saved_cursor_y_coordinates,
                new_cursor_x,
                saved_cursor_x_coordinates,
            ))
        } else if new_rows != self.height {
            let saved_cursor_y_coordinates = self
                .saved_cursor_position
                .as_ref()
                .map(|saved_cursor| saved_cursor.y);
            let saved_cursor_x_coordinates = self
                .saved_cursor_position
                .as_ref()
                .map(|saved_cursor| saved_cursor.x);

            Some((
                self.cursor.y,
                saved_cursor_y_coordinates,
                self.cursor.x,
                saved_cursor_x_coordinates,
            ))
        } else {
            None
        };

        if let Some(cursors) = cursors {
            // At this point the x coordinates have been calculated, the y coordinates
            // will be updated within this block
            let (
                mut new_cursor_y,
                mut saved_cursor_y_coordinates,
                new_cursor_x,
                saved_cursor_x_coordinates,
            ) = cursors;

            let current_viewport_row_count = self.viewport.len();
            match current_viewport_row_count.cmp(&new_rows) {
                Ordering::Less => {
                    let row_count_to_transfer = new_rows - current_viewport_row_count;
                    transfer_rows_from_lines_above_to_viewport(
                        &mut self.lines_above,
                        &mut self.viewport,
                        &mut self.sixel_grid,
                        &mut self.kitty_grid,
                        row_count_to_transfer,
                        new_columns,
                    );
                    let rows_pulled = self.viewport.len() - current_viewport_row_count;
                    new_cursor_y += rows_pulled;
                    if let Some(saved_cursor_y_coordinates) = saved_cursor_y_coordinates.as_mut() {
                        *saved_cursor_y_coordinates += rows_pulled;
                    };
                },
                Ordering::Greater => {
                    let row_count_to_transfer = current_viewport_row_count - new_rows;
                    if row_count_to_transfer > new_cursor_y {
                        new_cursor_y = 0;
                    } else {
                        new_cursor_y -= row_count_to_transfer;
                    }
                    if let Some(saved_cursor_y_coordinates) = saved_cursor_y_coordinates.as_mut() {
                        if row_count_to_transfer > *saved_cursor_y_coordinates {
                            *saved_cursor_y_coordinates = 0;
                        } else {
                            *saved_cursor_y_coordinates -= row_count_to_transfer;
                        }
                    }
                    transfer_rows_from_viewport_to_lines_above(
                        &mut self.viewport,
                        &mut self.lines_above,
                        &mut self.sixel_grid,
                        &mut self.kitty_grid,
                        row_count_to_transfer,
                        new_columns,
                    );
                },
                Ordering::Equal => {},
            }
            self.cursor.y = new_cursor_y;
            self.cursor.x = new_cursor_x;
            if let Some(saved_cursor_position) = self.saved_cursor_position.as_mut() {
                match (saved_cursor_x_coordinates, saved_cursor_y_coordinates) {
                    (Some(saved_cursor_x_coordinates), Some(saved_cursor_y_coordinates)) => {
                        saved_cursor_position.x = saved_cursor_x_coordinates;
                        saved_cursor_position.y = saved_cursor_y_coordinates;
                    },
                    _ => log::error!(
                        "invalid state - cannot set saved cursor to {:?} {:?}",
                        saved_cursor_x_coordinates,
                        saved_cursor_y_coordinates
                    ),
                }
            };
        }
        self.kitty_reproject_all_from_anchor();
        self.height = new_rows;
        self.width = new_columns;
        self.set_scroll_region_to_viewport_size();
        self.scrollback_buffer_lines = self.recalculate_scrollback_buffer_count();
        self.search_results.selections.clear();
        self.search_viewport();
        // If we have thrown out the active element, set it to None
        self.search_results.unset_active_selection_if_nonexistent();
        self.output_buffer.update_all_lines();
    }
    pub fn as_character_lines(&self) -> Vec<Vec<TerminalCharacter>> {
        // this is only used in the tests
        // it's not part of testing the app, but rather is used to interpret the snapshots created
        // by it
        let mut lines: Vec<Vec<TerminalCharacter>> = self
            .viewport
            .iter()
            .map(|r| {
                let excess_width = r.excess_width();
                let mut line: Vec<TerminalCharacter> = r.columns.iter().cloned().collect();
                // pad line
                line.resize(
                    self.width.saturating_sub(excess_width),
                    EMPTY_TERMINAL_CHARACTER,
                );
                line
            })
            .collect();
        let empty_row = vec![EMPTY_TERMINAL_CHARACTER; self.width];
        for _ in lines.len()..self.height {
            lines.push(empty_row.clone());
        }
        lines
    }
    pub fn read_changes(
        &mut self,
        x_offset: usize,
        y_offset: usize,
    ) -> (
        Vec<CharacterChunk>,
        Vec<SixelImageChunk>,
        Vec<KittyImageChunk>,
    ) {
        self.kitty_refresh_placeholder_placements();
        let changed_character_chunks = self.output_buffer.changed_chunks_in_viewport(
            self.viewport.make_contiguous(),
            self.width,
            self.height,
            x_offset,
            y_offset,
        );
        let changed_rects = self
            .output_buffer
            .changed_rects_in_viewport(self.viewport.len());
        if let Some(image_ids_to_reap) = self.sixel_grid.drain_image_ids_to_reap() {
            self.sixel_grid.reap_images(image_ids_to_reap);
        }
        let changed_sixel_image_chunks = self.sixel_grid.changed_sixel_chunks_in_viewport(
            changed_rects,
            self.lines_above.len(),
            self.width,
            x_offset,
            y_offset,
        );
        if self.kitty_settle_placements_below_the_viewport() {
            self.kitty_reanchor_all_from_pixels();
        }
        let kitty_image_chunks = self.kitty_grid.viewport_kitty_chunks(
            self.height,
            self.lines_above.len(),
            self.width,
            x_offset,
            y_offset,
        );
        self.output_buffer.consume();

        (
            changed_character_chunks,
            changed_sixel_image_chunks,
            kitty_image_chunks,
        )
    }
    pub fn serialize(&self, scrollback_lines_to_serialize: Option<usize>) -> Option<String> {
        match scrollback_lines_to_serialize {
            Some(scrollback_lines_to_serialize) => {
                let first_index = if scrollback_lines_to_serialize == 0 {
                    0
                } else {
                    self.lines_above
                        .len()
                        .saturating_sub(scrollback_lines_to_serialize)
                };
                let mut to_serialize = vec![];
                for line in self.lines_above.iter().skip(first_index) {
                    to_serialize.push(line.clone());
                }
                for line in &self.viewport {
                    to_serialize.push(line.clone())
                }
                self.output_buffer
                    .serialize(to_serialize.as_slice(), self.osc8_hyperlinks, None)
                    .ok()
            },
            None => {
                let viewport_vec: Vec<Row> = self.viewport.iter().cloned().collect();
                self.output_buffer
                    .serialize(&viewport_vec, self.osc8_hyperlinks, None)
                    .ok()
            },
        }
    }
    pub fn serialize_handoff(
        &self,
        scrollback: Option<usize>,
        budget: usize,
    ) -> Option<(String, Option<yinz_types::agent::theme::AgentThemeHandoff>)> {
        use yinz_types::agent::theme::{AgentThemeHandoff, HandoffLaunch};
        if !self
            .agent_palettes
            .iter()
            .any(|p| p.upgrade().is_some_and(|p| p.has_snapshot()))
        {
            return self
                .serialize(scrollback)
                .map(|c| (crate::agent_theme::fit_legacy_contents(c, budget), None));
        }
        let first = match scrollback {
            None => self.lines_above.len(),
            Some(0) => 0,
            Some(n) => self.lines_above.len().saturating_sub(n),
        };
        let rows: Vec<_> = self
            .lines_above
            .iter()
            .skip(first)
            .chain(&self.viewport)
            .chain(&self.lines_below)
            .cloned()
            .collect();
        let mut metadata = AgentThemeHandoff::default();
        if let Some(active) = &self.agent_launch {
            metadata.active = Some(HandoffLaunch {
                launch_id: active.launch_id.clone(),
                palette: metadata.intern(&active.snapshot),
            });
        }
        let mut contents = self
            .output_buffer
            .serialize_handoff(&rows, self.width, self.osc8_hyperlinks, &mut metadata)
            .ok()?;
        if metadata.snapshots.is_empty() {
            return self
                .serialize(scrollback)
                .map(|c| (crate::agent_theme::fit_legacy_contents(c, budget), None));
        }
        // Cut at a serialized row boundary, never in an SGR/OSC or UTF-8 sequence.
        loop {
            let metadata_bytes = serde_json::to_vec(&metadata).ok()?.len();
            if contents.len() + metadata_bytes <= budget {
                return Some((contents, Some(metadata)));
            }
            if contents.is_empty() {
                return Some((contents, None));
            }
            let minimum = (contents.len() + metadata_bytes)
                .saturating_sub(budget)
                .max(1);
            let cut = contents
                .match_indices("\n\r")
                .map(|(i, _)| i)
                .find(|i| *i >= minimum)
                .unwrap_or(contents.len());
            metadata.trim_prefix(cut);
            contents = contents[cut..].to_owned();
        }
    }
    fn clear_completed_sixel(&mut self) {
        self.sixel_grid.reap_images(
            self.completed_sixel
                .drain(..)
                .map(|chunk| chunk.sixel_image_id)
                .collect(),
        );
    }
    pub fn render(
        &mut self,
        content_x: usize,
        content_y: usize,
        style: &Style,
    ) -> Result<Option<RenderedGridFrame>> {
        let forced_repaint = std::mem::take(&mut self.repaint_completed_text);
        if self.lock_renders {
            if !forced_repaint {
                return Ok(None);
            }
            let chunks = (0..self.height)
                .flat_map(|y| {
                    let mut row = self.completed_text.get(y).cloned().unwrap_or_default();
                    if row.is_empty() {
                        row.push(CharacterChunk::new(vec![], 0, y));
                    }
                    row.retain(|chunk| chunk.x < self.width);
                    for chunk in &mut row {
                        chunk.retain_by_width(self.width.saturating_sub(chunk.x));
                    }
                    if let Some(last) = row.last_mut() {
                        let padding = self.width.saturating_sub(last.x + last.width());
                        last.terminal_characters
                            .extend(std::iter::repeat_n(EMPTY_TERMINAL_CHARACTER, padding));
                    }
                    for chunk in &mut row {
                        chunk.offset(content_x, content_y);
                    }
                    row
                })
                .collect();
            let mut sixel = vec![];
            let mut kitty = vec![];
            if let Some(cell_size) = *self.character_cell_size.borrow() {
                for mut chunk in self.completed_sixel.iter().copied() {
                    chunk.sixel_image_pixel_width = chunk
                        .sixel_image_pixel_width
                        .min(self.width.saturating_sub(chunk.cell_x) * cell_size.width);
                    chunk.sixel_image_pixel_height = chunk
                        .sixel_image_pixel_height
                        .min(self.height.saturating_sub(chunk.cell_y) * cell_size.height);
                    if chunk.sixel_image_pixel_width == 0 || chunk.sixel_image_pixel_height == 0 {
                        continue;
                    }
                    chunk.cell_x += content_x;
                    chunk.cell_y += content_y;
                    sixel.push(chunk);
                }
                for mut chunk in self.completed_kitty.iter().copied() {
                    chunk.source_px_width = chunk.source_px_width.min(
                        (self.width.saturating_sub(chunk.cell_x) * cell_size.width)
                            .saturating_sub(chunk.cell_offset_x as usize),
                    );
                    chunk.source_px_height = chunk.source_px_height.min(
                        (self.height.saturating_sub(chunk.cell_y) * cell_size.height)
                            .saturating_sub(chunk.cell_offset_y as usize),
                    );
                    if chunk.source_px_width == 0 || chunk.source_px_height == 0 {
                        continue;
                    }
                    chunk.cell_x += content_x;
                    chunk.cell_y += content_y;
                    kitty.push(chunk);
                }
            }
            return Ok(Some((chunks, None, sixel, kitty)));
        }
        self.kitty_grid.release_completed_frame();
        if let Some(alternate) = self.alternate_screen_state.as_mut() {
            alternate.kitty_grid.release_completed_frame();
        }
        // Retry resizes that could not fit alongside the protected completed frame.
        self.kitty_grid.character_cell_size_possibly_changed();
        let raw_vte_output = String::new();

        let (mut character_chunks, sixel_image_chunks, kitty_image_chunks) =
            self.read_changes(content_x, content_y);

        let plugin_highlight_selections = self.compute_plugin_highlight_selections();

        for character_chunk in character_chunks.iter_mut() {
            // Store text and highlights relative to the pane.
            character_chunk.x -= content_x;
            character_chunk.y -= content_y;
            character_chunk.theme_background = self.theme_background();
            character_chunk.add_changed_colors(self.effective_colors());
            // CharacterChunk still carries `Option<AnsiCode>` because
            // `adjust_styles_for_custom_bg_fg` assigns the value into
            // `CharacterStyles.{foreground,background}` (themselves
            // `Option<AnsiCode>`). Re-wrap the narrow RGB at the
            // boundary so the downstream pipeline stays uniform.
            character_chunk.add_pane_defaults(
                self.effective_foreground().map(AnsiCode::RgbCode),
                self.effective_background().map(AnsiCode::RgbCode),
            );
            if self.selection.contains_row(character_chunk.y) {
                let background_color = match style.colors.text_selected.background {
                    PaletteColor::Rgb(rgb) => AnsiCode::RgbCode(rgb),
                    PaletteColor::EightBit(col) => AnsiCode::ColorIndex(col),
                };
                let foreground_color = match style.colors.text_selected.base {
                    PaletteColor::Rgb(rgb) => AnsiCode::RgbCode(rgb),
                    PaletteColor::EightBit(col) => AnsiCode::ColorIndex(col),
                };

                character_chunk.add_selection_and_colors(
                    HighlightSelection {
                        selection: self.selection,
                        bg: Some(background_color),
                        fg: Some(foreground_color),
                        bold: false,
                        italic: false,
                        underline: false,
                        layer: HighlightLayer::ActionFeedback,
                    },
                    0,
                    0,
                );
            } else if !self.search_results.selections.is_empty() {
                for res in self.search_results.selections.iter() {
                    if res.contains_row(character_chunk.y) {
                        let (select_background_palette, select_foreground_palette) =
                            if Some(res) == self.search_results.active.as_ref() {
                                (
                                    style.colors.text_unselected.emphasis_0,
                                    style.colors.text_unselected.background,
                                )
                            } else {
                                (
                                    style.colors.text_unselected.emphasis_2,
                                    style.colors.text_unselected.background,
                                )
                            };
                        let background_color = match select_background_palette {
                            PaletteColor::Rgb(rgb) => AnsiCode::RgbCode(rgb),
                            PaletteColor::EightBit(col) => AnsiCode::ColorIndex(col),
                        };
                        let foreground_color = match select_foreground_palette {
                            PaletteColor::Rgb(rgb) => AnsiCode::RgbCode(rgb),
                            PaletteColor::EightBit(col) => AnsiCode::ColorIndex(col),
                        };
                        character_chunk.add_selection_and_colors(
                            HighlightSelection {
                                selection: *res,
                                bg: Some(background_color),
                                fg: Some(foreground_color),
                                bold: false,
                                italic: false,
                                underline: false,
                                layer: HighlightLayer::ActionFeedback,
                            },
                            0,
                            0,
                        );
                    }
                }
            }
            if let Some(command_output_flash) = self.command_output_flash {
                if command_output_flash.contains_row(character_chunk.y) {
                    let foreground_color = match style.colors.text_unselected.emphasis_0 {
                        PaletteColor::Rgb(rgb) => AnsiCode::RgbCode(rgb),
                        PaletteColor::EightBit(col) => AnsiCode::ColorIndex(col),
                    };
                    character_chunk.add_selection_and_colors(
                        HighlightSelection {
                            selection: command_output_flash,
                            bg: None,
                            fg: Some(foreground_color),
                            bold: false,
                            italic: false,
                            underline: false,
                            layer: HighlightLayer::ActionFeedback,
                        },
                        0,
                        0,
                    );
                }
            }
            // Apply pre-computed plugin highlight selections to this chunk.
            for hs in &plugin_highlight_selections {
                if hs.selection.contains_row(character_chunk.y) {
                    character_chunk.add_selection_and_colors(*hs, 0, 0);
                }
            }
        }
        self.clear_completed_sixel();
        // Sixel damage is incremental, but a forced repaint needs every visible image.
        self.completed_sixel = self
            .sixel_grid
            .changed_sixel_chunks_in_viewport(
                HashMap::from([(0, self.height)]),
                self.lines_above.len(),
                self.width,
                0,
                0,
            )
            .into_iter()
            .filter_map(|mut chunk| {
                chunk.sixel_image_id = self
                    .sixel_grid
                    .sixel_image_store
                    .borrow_mut()
                    .retain_image(chunk.sixel_image_id)?;
                Some(chunk)
            })
            .collect();
        self.completed_kitty = kitty_image_chunks
            .iter()
            .map(|chunk| {
                let mut chunk = *chunk;
                chunk.cell_x -= content_x;
                chunk.cell_y -= content_y;
                chunk
            })
            .collect();
        // OutputBuffer emits complete rows. Cache all palette spans of each changed row.
        self.completed_text.resize_with(self.height, Vec::new);
        character_chunks = character_chunks
            .into_iter()
            .flat_map(|chunk| {
                let y = chunk.y;
                let chunks = self.viewport.get(y).map_or_else(
                    || vec![chunk.clone()],
                    |row| row.split_palette_chunks(chunk.clone()),
                );
                self.completed_text[y] = chunks.clone();
                chunks
            })
            .collect();
        for chunk in &mut character_chunks {
            chunk.offset(content_x, content_y);
        }
        Ok(Some((
            character_chunks,
            Some(raw_vte_output),
            sixel_image_chunks,
            kitty_image_chunks,
        )))
    }
    /// Returns the cursor position and whether it is visible.
    /// The position is returned unconditionally (as long as the cursor is within
    /// bounds) so that the host terminal can position the cursor for IME even
    /// when the app has hidden it. The bool is true when the cursor is visible.
    pub fn cursor_coordinates(&self) -> Option<(usize, usize, bool)> {
        if let Some((x, y)) = self.text_selection_cursor() {
            // in a keyboard selection the cursor marks the end being moved, so that swapping
            // the ends is visible before the next movement key
            return Some((x, y, true));
        }
        if self.cursor.x >= self.width || self.cursor.y >= self.height {
            None
        } else {
            Some((self.cursor.x, self.cursor.y, !self.cursor_is_hidden))
        }
    }
    /// While a keyboard selection is live, the cell of the end being moved - `None` otherwise.
    /// Panes render their cursor here so that swapping the ends is visible right away.
    pub fn text_selection_cursor(&self) -> Option<(usize, usize)> {
        if !self.text_selection_active {
            return None;
        }
        let end = self.selection.end;
        if end.line.get() < 0 || end.line.get() >= self.height as isize || self.width == 0 {
            return None;
        }
        let column = end.column.get().min(self.width.saturating_sub(1));
        Some((column, end.line.get() as usize))
    }
    pub fn selection_cursor_coordinates(&self) -> Option<(usize, usize)> {
        if self.cursor.x > self.width || self.cursor.y >= self.height {
            None
        } else {
            Some((self.cursor.x, self.cursor.y))
        }
    }
    pub fn is_mid_frame(&self) -> bool {
        self.lock_renders
    }
    /// Clears all buffers with text for a current screen
    pub fn clear_screen(&mut self) {
        if self.alternate_screen_state.is_some() {
            log::warn!("Tried to clear pane with alternate_screen_state");
            return;
        }
        self.reset_terminal_state();
        self.mark_for_rerender();
    }
    /// Dumps the viewport, or all retained lines including those below a scrolled viewport.
    pub fn dump_screen(&self, full: bool) -> String {
        if !full {
            return dump_screen!(self.viewport.iter());
        }
        dump_screen!(self
            .lines_above
            .iter()
            .chain(&self.viewport)
            .chain(&self.lines_below))
    }
    /// Dumps the viewport or all retained lines, with ANSI styling.
    pub fn dump_screen_with_ansi(&self, full: bool) -> String {
        if !full {
            return dump_screen_with_ansi!(self.viewport.iter());
        }
        dump_screen_with_ansi!(self
            .lines_above
            .iter()
            .chain(&self.viewport)
            .chain(&self.lines_below))
    }
    pub fn move_viewport_up(&mut self, count: usize) {
        for _ in 0..count {
            self.scroll_up_one_line();
        }
        self.output_buffer.update_all_lines();
    }
    pub fn move_viewport_down(&mut self, count: usize) {
        for _ in 0..count {
            self.scroll_down_one_line();
        }
        self.output_buffer.update_all_lines();
    }
    pub fn reset_viewport(&mut self) {
        let max_lines_to_scroll = *SCROLL_BUFFER_SIZE.get().unwrap() * 2; // while not very elegant, this can prevent minor bugs from becoming showstoppers by sticking the whole app display in an endless loop
        let mut lines_scrolled = 0;
        let should_clear_output_buffer = self.is_scrolled;
        while self.is_scrolled && lines_scrolled < max_lines_to_scroll {
            self.scroll_down_one_line();
            lines_scrolled += 1;
        }
        if should_clear_output_buffer {
            self.output_buffer.update_all_lines();
        }
    }
    pub fn rotate_scroll_region_up(&mut self, count: usize) {
        let (scroll_region_top, scroll_region_bottom) = self.scroll_region;
        self.pad_lines_until(scroll_region_bottom, EMPTY_TERMINAL_CHARACTER);
        for _ in 0..count {
            let mut pad_character = EMPTY_TERMINAL_CHARACTER;
            pad_character.styles = self.cursor.pending_styles.clone();
            let columns = VecDeque::from(vec![pad_character; self.width]);
            self.scroll_region_content_down(
                scroll_region_top,
                scroll_region_bottom,
                Row::from_columns(columns)
                    .canonical()
                    .with_agent_theme(Some(self.agent_theme.clone())),
            );
        }
        self.output_buffer
            .update_lines(scroll_region_top, scroll_region_bottom);
    }
    pub fn rotate_scroll_region_down(&mut self, count: usize) {
        let (scroll_region_top, scroll_region_bottom) = self.scroll_region;
        self.pad_lines_until(scroll_region_bottom, EMPTY_TERMINAL_CHARACTER);
        let mut pad_character = EMPTY_TERMINAL_CHARACTER;
        pad_character.styles = self.cursor.pending_styles.clone();
        for _ in 0..count {
            let columns = VecDeque::from(vec![pad_character.clone(); self.width]);
            self.scroll_region_content_up(
                scroll_region_top,
                scroll_region_top,
                scroll_region_bottom,
                Row::from_columns(columns)
                    .canonical()
                    .with_agent_theme(Some(self.agent_theme.clone())),
            );
        }
        self.output_buffer
            .update_lines(scroll_region_top, scroll_region_bottom);
    }
    pub fn fill_viewport(&mut self, character: TerminalCharacter) {
        if self.alternate_screen_state.is_some() {
            self.viewport.clear();
        } else {
            self.transfer_rows_to_lines_above(self.viewport.len())
        };

        for _ in 0..self.height {
            let columns = VecDeque::from(vec![character.clone(); self.width]);
            self.viewport.push_back(
                Row::from_columns(columns)
                    .canonical()
                    .with_agent_theme(Some(self.agent_theme.clone()))
                    .with_bg_color(
                        character
                            .styles
                            .background
                            .filter(|bg| *bg != AnsiCode::Reset),
                    ),
            );
        }
        self.output_buffer.update_all_lines();
    }
    fn scroll_region_content_down(&mut self, at: usize, region_bottom: usize, new_row: Row) {
        // Wrapped matches can change on either side of the moved rows, in both the old
        // and new logical lines. Ordinary scrolling still invalidates only its range.
        if !self.plugin_highlights.is_empty() {
            self.mark_logical_line_dirty(at);
            self.mark_logical_line_dirty(region_bottom);
        }
        let before = self.kitty_row_snapshot();
        if region_bottom < self.viewport.len() {
            self.viewport.remove(region_bottom);
        }
        self.viewport.insert(at, new_row);
        if !self.plugin_highlights.is_empty() {
            self.mark_logical_line_dirty(at);
            self.mark_logical_line_dirty(region_bottom);
        }
        self.kitty_region_scroll(at, region_bottom, -1, before, false);
    }
    fn scroll_region_content_up(
        &mut self,
        at: usize,
        region_top: usize,
        region_bottom: usize,
        new_row: Row,
    ) -> RegionRowsScrolled {
        if !self.plugin_highlights.is_empty() {
            self.mark_logical_line_dirty(at);
            self.mark_logical_line_dirty(region_bottom);
        }
        let before = self.kitty_row_snapshot();
        let outcome = if at == 0
            && region_top == 0
            && self.alternate_screen_state.is_none()
            && !self.viewport.is_empty()
        {
            self.transfer_rows_to_lines_above(1);
            let old_selection = self.selection;
            let old_command_output_flash = self.command_output_flash;
            self.selection.move_up(1);
            if let Some(command_output_flash) = self.command_output_flash.as_mut() {
                command_output_flash.move_up(1);
            }
            // Highlights can move outside a partial scroll region, whose rows alone are
            // invalidated by CSI S and CSI M.
            let current_selection = self.selection;
            if !old_selection.is_empty() || !current_selection.is_empty() {
                self.update_selected_lines(&old_selection, &current_selection);
            }
            if let (Some(old_flash), Some(current_flash)) =
                (old_command_output_flash, self.command_output_flash)
            {
                self.update_selected_lines(&old_flash, &current_flash);
            }
            RegionRowsScrolled::IntoScrollback
        } else if at < self.viewport.len() {
            self.viewport.remove(at);
            RegionRowsScrolled::Discarded
        } else {
            RegionRowsScrolled::Nothing
        };
        if self.viewport.len() >= region_bottom {
            self.viewport.insert(region_bottom, new_row);
        } else {
            self.viewport.push_back(new_row);
        }
        if !self.plugin_highlights.is_empty() {
            self.mark_logical_line_dirty(at);
            self.mark_logical_line_dirty(region_bottom);
        }
        self.kitty_region_scroll(
            at,
            region_bottom,
            1,
            before,
            outcome == RegionRowsScrolled::IntoScrollback,
        );
        outcome
    }
    fn kitty_row_snapshot(&mut self) -> KittyRowSnapshot {
        self.kitty_settle_placements_below_the_viewport();
        KittyRowSnapshot {
            lines_above_len: self.lines_above.len(),
            front_drops: self.kitty_grid.front_drops(),
            merged_rows: self.kitty_grid.merged_rows(),
            split_rows: self.kitty_grid.split_rows(),
        }
    }
    fn kitty_region_scroll(
        &mut self,
        region_top: usize,
        region_bottom: usize,
        n: isize,
        before: KittyRowSnapshot,
        preserve_above_region_top: bool,
    ) {
        let cell_size = { *self.character_cell_size.borrow() };
        if let Some(character_cell_size) = cell_size {
            let cell_h = character_cell_size.height as isize;
            let already_shifted_rows = (self.kitty_grid.front_drops() - before.front_drops)
                as isize
                + (self.kitty_grid.merged_rows() - before.merged_rows) as isize
                - (self.kitty_grid.split_rows() - before.split_rows) as isize;
            let la_delta = (self.lines_above.len() as isize - before.lines_above_len as isize)
                + already_shifted_rows;
            let la_delta_px = la_delta * cell_h;
            let viewport_top_row = before.lines_above_len as isize - already_shifted_rows;
            let region_top_px = (viewport_top_row + region_top as isize) * cell_h;
            let region_bottom_px = (viewport_top_row + region_bottom as isize + 1) * cell_h;
            let viewport_top_px = viewport_top_row * cell_h;
            self.kitty_grid.apply_region_scroll(
                region_top_px,
                region_bottom_px,
                n * cell_h,
                la_delta_px,
                viewport_top_px,
                preserve_above_region_top,
            );
            self.kitty_grid
                .note_rows_below_the_viewport_shifted(la_delta);
            self.kitty_reanchor_all_from_pixels();
        }
    }
    fn scroll_region_up_at_bottom(&mut self, new_row: Row, offset_hyperlinks: bool) {
        let (scroll_region_top, scroll_region_bottom) = self.scroll_region;
        if scroll_region_top >= self.viewport.len() {
            return;
        }
        if scroll_region_bottom == self.height.saturating_sub(1) && scroll_region_top == 0 {
            if self.alternate_screen_state.is_none() {
                self.transfer_rows_to_lines_above(1);
                if offset_hyperlinks {
                    self.hyperlink_tracker.offset_cursor_lines(1);
                }
            } else if !self.viewport.is_empty() {
                self.viewport.pop_front();
            }
            self.viewport.push_back(new_row);
            self.selection.move_up(1);
            if let Some(command_output_flash) = self.command_output_flash.as_mut() {
                command_output_flash.move_up(1);
            }
        } else {
            let outcome = self.scroll_region_content_up(
                scroll_region_top,
                scroll_region_top,
                scroll_region_bottom,
                new_row,
            );
            if offset_hyperlinks {
                match outcome {
                    RegionRowsScrolled::IntoScrollback => {
                        self.hyperlink_tracker.offset_cursor_lines(1)
                    },
                    RegionRowsScrolled::Discarded => {
                        self.hyperlink_tracker.offset_cursor_lines_in_range(
                            scroll_region_top as isize,
                            scroll_region_bottom as isize,
                            1,
                        )
                    },
                    RegionRowsScrolled::Nothing => {},
                }
            }
        }
        self.output_buffer.update_all_lines();
    }
    pub fn add_canonical_line(&mut self) {
        let (scroll_region_top, scroll_region_bottom) = self.scroll_region;
        self.hyperlink_tracker.update(
            '\n',
            &self.cursor,
            &mut self.viewport,
            &mut self.lines_above,
            &mut self.link_handler.borrow_mut(),
        );
        if self.cursor.y == scroll_region_bottom {
            if scroll_region_top >= self.viewport.len() {
                return;
            }
            let scroll_bg = self.cursor.pending_styles.background;
            let new_row = Row::new()
                .canonical()
                .with_bg_color(scroll_bg)
                .with_agent_theme(Some(self.agent_theme.clone()));
            self.scroll_region_up_at_bottom(new_row, false);
            self.kitty_reanchor_all_from_pixels();
            return;
        }
        if self.viewport.len() <= self.cursor.y + 1 {
            // Nothing scrolls here (the cursor is above the scroll region's bottom), so this
            // is a plain cursor move onto a screen row the sparse viewport had not
            // materialised yet. A linefeed that does not scroll paints nothing: the pending
            // background belongs to the scroll branch above and to the erase handlers.
            let new_row = Row::new().canonical();
            self.viewport.push_back(new_row);
        }
        if self.cursor.y == self.height.saturating_sub(1) {
            self.output_buffer.update_all_lines();
        } else {
            self.cursor.y += 1;
            self.output_buffer.update_line(self.cursor.y);
        }
    }
    pub fn move_cursor_to_beginning_of_line(&mut self) {
        self.cursor.x = 0;
    }
    pub fn add_character_at_cursor_position(
        &mut self,
        terminal_character: TerminalCharacter,
        should_insert_character: bool,
    ) {
        self.hyperlink_tracker.update(
            terminal_character.character,
            &self.cursor,
            &mut self.viewport,
            &mut self.lines_above,
            &mut self.link_handler.borrow_mut(),
        );
        // this function assumes the current line has enough room for terminal_character (that its
        // width has been checked beforehand)
        match self.viewport.get_mut(self.cursor.y) {
            Some(row) => {
                row.writing(&self.agent_theme);
                if self.insert_mode || should_insert_character {
                    row.insert_character_at(terminal_character, self.cursor.x);
                    if row.width() > self.width {
                        row.truncate(self.width);
                    }
                } else {
                    row.add_character_at(terminal_character, self.cursor.x);
                }
                if let Some(character_cell_size) = *self.character_cell_size.borrow() {
                    let scrollback_size_in_pixels =
                        self.lines_above.len() * character_cell_size.height;
                    let absolute_x_in_pixels = self.cursor.x * character_cell_size.width;
                    let absolute_y_in_pixels =
                        scrollback_size_in_pixels + (self.cursor.y * character_cell_size.height);
                    let rect_to_cut_out = PixelRect {
                        x: absolute_x_in_pixels,
                        y: absolute_y_in_pixels as isize,
                        width: character_cell_size.width,
                        height: character_cell_size.height,
                    };
                    if let Some(images_to_cut_out) =
                        self.sixel_grid.cut_off_rect_from_images(rect_to_cut_out)
                    {
                        for (image_id, rect_in_image_to_cut_out) in images_to_cut_out {
                            self.sixel_grid
                                .remove_pixels_from_image(image_id, rect_in_image_to_cut_out);
                        }
                    }
                }
                self.output_buffer.update_line(self.cursor.y);
            },
            None => {
                // pad lines until cursor if they do not exist
                for _ in self.viewport.len()..self.cursor.y {
                    self.viewport.push_back(Row::new().canonical());
                }
                let mut new_row = Row::new().canonical();
                new_row.writing(&self.agent_theme);
                new_row.add_character_at(terminal_character, self.cursor.x);
                self.viewport.push_back(new_row);
                self.output_buffer.update_line(self.cursor.y);
            },
        }
    }
    pub fn add_character(&mut self, terminal_character: TerminalCharacter) {
        let character_width = terminal_character.width();
        // Drop zero-width Unicode/UTF-8 codepoints, like for example Variation Selectors.
        // This breaks unicode grapheme segmentation, and is the reason why some characters
        // aren't displayed correctly. Refer to this issue for more information:
        //     https://github.com/zellij-org/zellij/issues/1538
        if character_width == 0 {
            // ...except for the row/column diacritics of a kitty unicode placeholder, which are
            // zero width by construction and carry the position of the cell before them inside its
            // image. Spec, "Unicode placeholders": "the row and column of the image can be
            // specified using diacritics". Anything else zero width, or one arriving after a cell
            // that is not a placeholder, falls through and is dropped as before.
            self.record_placeholder_diacritic(terminal_character.character);
            return;
        }
        if self.cursor.x + character_width > self.width {
            if self.disable_linewrap {
                return;
            }
            self.line_wrap();
        }
        self.add_character_at_cursor_position(terminal_character, false);
        self.move_cursor_forward_until_edge(character_width);
    }
    /// Attach a row/column diacritic to the placeholder cell the cursor has just moved past.
    ///
    /// The cell to the left is the one being annotated: printing a placeholder advances the cursor
    /// like any other single-width character, so by the time its marks arrive the cursor sits one
    /// column further on.
    fn record_placeholder_diacritic(&mut self, character: char) {
        let value = match diacritic_value(character) {
            Some(value) => value,
            None => return,
        };
        let column = match self.cursor.x.checked_sub(1) {
            Some(column) => column,
            None => return,
        };
        let row = match self.viewport.get_mut(self.cursor.y) {
            Some(row) => row,
            None => return,
        };
        if row.push_placeholder_mark_value(column, value) {
            self.output_buffer.update_line(self.cursor.y);
        }
    }
    pub fn get_character_under_cursor(&self) -> Option<TerminalCharacter> {
        self.get_character_at(self.cursor.x, self.cursor.y)
    }
    /// The character at a viewport cell - the cursor is not necessarily there, eg. when a
    /// keyboard selection has moved the rendered cursor to the end being moved.
    pub fn get_character_at(&self, x: usize, y: usize) -> Option<TerminalCharacter> {
        let absolute_x_in_line = self.get_absolute_character_index(x, y)?;
        self.viewport
            .get(y)
            .and_then(|current_line| current_line.columns.get(absolute_x_in_line))
            .cloned()
    }
    pub fn get_absolute_character_index(&self, x: usize, y: usize) -> Option<usize> {
        Some(self.viewport.get(y)?.absolute_character_index(x))
    }
    pub fn move_cursor_forward_until_edge(&mut self, count: usize) {
        let count_to_move = std::cmp::min(count, self.width.saturating_sub(self.cursor.x));
        self.cursor.x += count_to_move;
    }
    pub fn replace_characters_in_line_after_cursor(&mut self, replace_with: TerminalCharacter) {
        if let Some(row) = self.viewport.get_mut(self.cursor.y) {
            row.writing(&self.agent_theme);
            row.bg_color = replace_with
                .styles
                .background
                .filter(|bg| *bg != AnsiCode::Reset);
            row.replace_and_pad_end(self.cursor.x, self.width, replace_with);
        }
        self.output_buffer.update_line(self.cursor.y);
    }
    pub fn replace_characters_in_line_before_cursor(&mut self, replace_with: TerminalCharacter) {
        let row = self.viewport.get_mut(self.cursor.y).unwrap();
        row.writing(&self.agent_theme);
        // A prefix erase leaves the tail's padding colour intact unless it reaches the edge.
        if self.cursor.x >= self.width.saturating_sub(1) {
            row.bg_color = replace_with
                .styles
                .background
                .filter(|bg| *bg != AnsiCode::Reset);
        }
        row.replace_and_pad_beginning(self.cursor.x, replace_with);
        self.output_buffer.update_line(self.cursor.y);
    }
    pub fn clear_all_after_cursor(&mut self, replace_with: TerminalCharacter) {
        let erase_bg = replace_with
            .styles
            .background
            .filter(|bg| !matches!(bg, AnsiCode::Reset));
        if let Some(cursor_row) = self.viewport.get_mut(self.cursor.y) {
            cursor_row.writing(&self.agent_theme);
            cursor_row.truncate(self.cursor.x);
            // The erase's background is the padding colour of every row it touches from
            // now on, and a colourless erase takes an earlier one away: the rows are
            // filled to `width` here, but a widening or a DCH reopens the padding, and
            // `replace_columns` leaves `bg_color` alone.
            let replace_with_columns = VecDeque::from(vec![replace_with.clone(); self.width]);
            self.replace_characters_in_line_after_cursor(replace_with);
            for row in self.viewport.iter_mut().skip(self.cursor.y + 1) {
                row.writing(&self.agent_theme);
                row.replace_columns(replace_with_columns.clone());
                row.bg_color = erase_bg;
            }
            // The screen rows below the viewport are erased too. They render as default
            // until they exist, so only a coloured erase has to materialise them, and
            // `Row::bg_color` paints their padding without giving them a width (`ESC[2J`
            // goes through `fill_viewport` and already covers the whole screen).
            if erase_bg.is_some() {
                while self.viewport.len() < self.height {
                    self.viewport.push_back(
                        Row::new()
                            .canonical()
                            .with_bg_color(erase_bg)
                            .with_agent_theme(Some(self.agent_theme.clone())),
                    );
                }
            }
            self.output_buffer
                .update_lines(self.cursor.y, self.viewport.len() - 1);
        }
    }
    pub fn clear_all_before_cursor(&mut self, replace_with: TerminalCharacter) {
        if self.viewport.get(self.cursor.y).is_some() {
            let erase_bg = replace_with
                .styles
                .background
                .filter(|bg| *bg != AnsiCode::Reset);
            let replace_with_columns = VecDeque::from(vec![replace_with.clone(); self.width]);
            self.replace_characters_in_line_before_cursor(replace_with);
            for row in self.viewport.iter_mut().take(self.cursor.y) {
                row.writing(&self.agent_theme);
                row.replace_columns(replace_with_columns.clone());
                row.bg_color = erase_bg;
            }
            self.output_buffer.update_lines(0, self.cursor.y);
        }
    }
    pub fn clear_cursor_line(&mut self) {
        if let Some(viewport_line) = self.viewport.get_mut(self.cursor.y) {
            viewport_line.writing(&self.agent_theme);
            viewport_line.replace_columns(VecDeque::new());
            viewport_line.bg_color = self
                .cursor
                .pending_styles
                .background
                .filter(|bg| *bg != AnsiCode::Reset);
            self.output_buffer.update_line(self.cursor.y);
        }
    }
    pub fn clear_all(&mut self, replace_with: TerminalCharacter) {
        let erase_bg = replace_with
            .styles
            .background
            .filter(|bg| *bg != AnsiCode::Reset);
        let replace_with_columns = VecDeque::from(vec![replace_with.clone(); self.width]);
        self.replace_characters_in_line_after_cursor(replace_with);
        for row in &mut self.viewport {
            row.writing(&self.agent_theme);
            row.replace_columns(replace_with_columns.clone());
            row.bg_color = erase_bg;
        }
        self.output_buffer.update_all_lines();
    }
    fn line_wrap(&mut self) {
        self.cursor.x = 0;
        let (scroll_region_top, scroll_region_bottom) = self.scroll_region;
        if self.cursor.y == scroll_region_bottom {
            if scroll_region_top >= self.viewport.len() {
                return;
            }
            let wrapped_row = Row::new()
                .with_bg_color(self.cursor.pending_styles.background)
                .with_agent_theme(Some(self.agent_theme.clone()));
            self.scroll_region_up_at_bottom(wrapped_row, true);
        } else if self.cursor.y == self.height.saturating_sub(1) {
            // the cursor is on the last line of the screen but below the scroll region's
            // bottom margin: there is nowhere to scroll to, so we wrap onto the same line
            // (mirroring what add_canonical_line does in this situation)
            self.output_buffer.update_line(self.cursor.y);
        } else {
            self.cursor.y += 1;
            if self.viewport.len() <= self.cursor.y {
                // A wrap that does not scroll paints nothing but the wrapped character, so
                // the row it lands on takes no background (see `add_canonical_line`).
                let line_wrapped_row = Row::new();
                self.viewport.push_back(line_wrapped_row);
                self.output_buffer.update_line(self.cursor.y);
            } else if let Some(current_line) = self.viewport.get_mut(self.cursor.y) {
                current_line.writing(&self.agent_theme);
                current_line.is_canonical = false;
            }
        }
    }
    fn clear_lines_above(&mut self) {
        self.lines_above.clear();
        self.scrollback_buffer_lines = self.recalculate_scrollback_buffer_count();
    }

    fn pad_current_line_until(&mut self, position: usize, pad_character: TerminalCharacter) {
        if self.viewport.get(self.cursor.y).is_none() {
            self.pad_lines_until(self.cursor.y, pad_character.clone());
        }
        if let Some(current_row) = self.viewport.get_mut(self.cursor.y) {
            current_row.writing(&self.agent_theme);
            let mut effective_pad = pad_character;
            if let Some(bg_color) = current_row.bg_color {
                if matches!(
                    effective_pad.styles.background,
                    Some(AnsiCode::Reset) | None
                ) {
                    effective_pad
                        .styles
                        .update(|styles| styles.background = Some(bg_color));
                }
            }
            for _ in current_row.width()..position {
                let column = current_row.width();
                let existing = current_row.palettes.at(column).cloned();
                current_row.write_palette = existing;
                current_row.push(effective_pad.clone());
            }
            self.output_buffer.update_line(self.cursor.y);
        }
    }
    fn pad_lines_until(&mut self, position: usize, pad_character: TerminalCharacter) {
        for _ in self.viewport.len()..=position {
            let columns = VecDeque::from(vec![pad_character.clone(); self.width]);
            self.viewport.push_back(
                Row::from_columns(columns)
                    .canonical()
                    .with_agent_theme(Some(self.agent_theme.clone())),
            );
            self.output_buffer.update_line(self.viewport.len() - 1);
        }
    }
    pub fn move_cursor_to(&mut self, x: usize, y: usize, pad_character: TerminalCharacter) {
        let (scroll_region_top, scroll_region_bottom) = self.scroll_region;
        self.cursor.x = std::cmp::min(self.width - 1, x);
        let y_offset = if self.erasure_mode {
            scroll_region_top
        } else {
            0
        };
        if y >= scroll_region_top && y <= scroll_region_bottom {
            self.cursor.y = std::cmp::min(scroll_region_bottom, y + y_offset);
        } else {
            self.cursor.y = std::cmp::min(self.height.saturating_sub(1), y + y_offset);
        }
        self.pad_lines_until(self.cursor.y, pad_character.clone());
        self.pad_current_line_until(self.cursor.x, pad_character);
    }
    pub fn move_cursor_up(&mut self, count: usize) {
        let (scroll_region_top, scroll_region_bottom) = self.scroll_region;
        if self.cursor.y >= scroll_region_top && self.cursor.y <= scroll_region_bottom {
            self.cursor.y = std::cmp::max(self.cursor.y.saturating_sub(count), scroll_region_top);
            return;
        }
        self.cursor.y = self.cursor.y.saturating_sub(count);
    }
    pub fn move_cursor_up_with_scrolling(&mut self, count: usize) {
        let (scroll_region_top, scroll_region_bottom) = self.scroll_region;
        for _ in 0..count {
            let current_line_index = self.cursor.y;
            if current_line_index == scroll_region_top {
                // if we're at the top line, we create a new line and remove the last line that
                // would otherwise overflow
                self.scroll_region_content_down(
                    current_line_index,
                    scroll_region_bottom,
                    Row::new()
                        .canonical()
                        .with_bg_color(self.cursor.pending_styles.background),
                );
            } else {
                self.move_cursor_up(1);
            }
        }
        self.output_buffer.update_all_lines();
    }
    pub fn move_cursor_down_until_edge_of_screen(
        &mut self,
        count: usize,
        pad_character: TerminalCharacter,
    ) {
        let (scroll_region_top, scroll_region_bottom) = self.scroll_region;
        if self.cursor.y >= scroll_region_top && self.cursor.y <= scroll_region_bottom {
            self.cursor.y = std::cmp::min(self.cursor.y + count, scroll_region_bottom);
        } else {
            self.cursor.y = std::cmp::min(self.cursor.y + count, self.height - 1);
        }
        self.pad_lines_until(self.cursor.y, pad_character);
    }
    pub fn move_cursor_back(&mut self, count: usize) {
        if self.cursor.x == self.width {
            // on the rightmost screen edge, backspace skips one character
            self.cursor.x -= 1;
        }
        if self.cursor.x < count {
            self.cursor.x = 0;
        } else {
            self.cursor.x -= count;
        }
    }
    pub fn hide_cursor(&mut self) {
        self.cursor_is_hidden = true;
    }
    pub fn show_cursor(&mut self) {
        self.cursor_is_hidden = false;
    }
    pub fn set_scroll_region(&mut self, top_line_index: usize, bottom_line_index: Option<usize>) {
        let bottom_line_index = bottom_line_index.unwrap_or(self.height.saturating_sub(1));
        if top_line_index >= bottom_line_index || bottom_line_index >= self.height {
            return;
        }
        self.scroll_region = (top_line_index, bottom_line_index);
        let mut pad_character = EMPTY_TERMINAL_CHARACTER;
        pad_character.styles = self.cursor.pending_styles.clone();
        self.move_cursor_to(0, 0, pad_character); // DECSTBM moves the cursor to column 1 line 1 of the page
    }
    pub fn set_scroll_region_to_viewport_size(&mut self) {
        self.scroll_region = (0, self.height.saturating_sub(1));
    }
    pub fn delete_lines_in_scroll_region(
        &mut self,
        count: usize,
        pad_character: TerminalCharacter,
    ) {
        let (scroll_region_top, scroll_region_bottom) = self.scroll_region;
        let current_line_index = self.cursor.y;
        if current_line_index >= scroll_region_top && current_line_index <= scroll_region_bottom {
            // when deleting lines inside the scroll region, we must make sure it stays the
            // same size (and that other lines below it aren't shifted inside it)
            // so we delete the current line(s) and add an empty line at the end of the scroll
            // region
            for _ in 0..count {
                let columns = VecDeque::from(vec![pad_character.clone(); self.width]);
                self.scroll_region_content_up(
                    current_line_index,
                    scroll_region_top,
                    scroll_region_bottom,
                    Row::from_columns(columns)
                        .canonical()
                        .with_agent_theme(Some(self.agent_theme.clone())),
                );
            }
            self.output_buffer
                .update_lines(current_line_index, scroll_region_bottom);
        }
    }
    pub fn add_empty_lines_in_scroll_region(
        &mut self,
        count: usize,
        pad_character: TerminalCharacter,
    ) {
        let (scroll_region_top, scroll_region_bottom) = self.scroll_region;
        let current_line_index = self.cursor.y;
        if current_line_index >= scroll_region_top && current_line_index <= scroll_region_bottom {
            // when adding empty lines inside the scroll region, we must make sure it stays the
            // same size and that lines don't "leak" outside of it
            // so we add an empty line where the cursor currently is, and delete the last line
            // of the scroll region
            for _ in 0..count {
                let columns = VecDeque::from(vec![pad_character.clone(); self.width]);
                self.scroll_region_content_down(
                    current_line_index,
                    scroll_region_bottom,
                    Row::from_columns(columns)
                        .canonical()
                        .with_agent_theme(Some(self.agent_theme.clone())),
                );
            }
            self.output_buffer
                .update_lines(current_line_index, scroll_region_bottom);
        }
    }
    pub fn move_cursor_to_column(&mut self, column: usize) {
        self.cursor.x = column;
        let pad_character = EMPTY_TERMINAL_CHARACTER;
        self.pad_current_line_until(self.cursor.x, pad_character);
    }
    pub fn move_cursor_to_line(&mut self, line: usize, pad_character: TerminalCharacter) {
        self.cursor.y = std::cmp::min(self.height - 1, line);
        self.pad_lines_until(self.cursor.y, pad_character);
        let pad_character = EMPTY_TERMINAL_CHARACTER;
        self.pad_current_line_until(self.cursor.x, pad_character);
    }
    pub fn replace_with_empty_chars(&mut self, count: usize, empty_char_style: RcCharacterStyles) {
        let mut empty_character = EMPTY_TERMINAL_CHARACTER;
        empty_character.styles = empty_char_style;
        let pad_until = std::cmp::min(self.width, self.cursor.x + count);
        self.pad_current_line_until(pad_until, empty_character.clone());
        if let Some(current_row) = self.viewport.get_mut(self.cursor.y) {
            current_row.writing(&self.agent_theme);
            for i in 0..count {
                current_row.replace_character_at(empty_character.clone(), self.cursor.x + i);
            }
            self.output_buffer.update_line(self.cursor.y);
        }
    }
    fn erase_characters(&mut self, count: usize, empty_char_style: RcCharacterStyles) {
        let mut empty_character = EMPTY_TERMINAL_CHARACTER;
        empty_character.styles = empty_char_style;
        if let Some(current_row) = self.viewport.get_mut(self.cursor.y) {
            current_row.writing(&self.agent_theme);
            // pad row if needed
            if current_row.width_cached() < self.width {
                let padding_count = self.width - current_row.width_cached();
                let mut columns_padding =
                    VecDeque::from(vec![EMPTY_TERMINAL_CHARACTER; padding_count]);
                current_row.columns.append(&mut columns_padding);
            }
            for _ in 0..count {
                let deleted_character = current_row.delete_and_return_character(self.cursor.x);
                let excess_width = deleted_character
                    .map(|terminal_character| terminal_character.width())
                    .unwrap_or(0)
                    .saturating_sub(1);
                for _ in 0..excess_width {
                    current_row.insert_character_at(empty_character.clone(), self.cursor.x);
                }
                current_row.push(empty_character.clone());
            }
            self.output_buffer.update_line(self.cursor.y);
        }
    }
    fn add_newline(&mut self) {
        self.add_canonical_line();
        self.mark_for_rerender();
    }
    pub fn mark_for_rerender(&mut self) {
        self.should_render = true;
    }
    pub fn reset_terminal_state(&mut self) {
        if let Some(alternate_screen_state) = self.alternate_screen_state.as_mut() {
            alternate_screen_state.kitty_grid.teardown();
        }
        self.lines_above = VecDeque::new();
        self.lines_below = VecDeque::new();
        self.is_scrolled = false;
        self.viewport = VecDeque::from(vec![Row::new().canonical()]);
        self.alternate_screen_state = None;
        self.cursor_key_mode = false;
        self.clear_viewport_before_rendering = true;
        self.cursor = Cursor::new(0, 0, self.styled_underlines);
        self.saved_cursor_position = None;
        self.active_charset = Default::default();
        self.erasure_mode = false;
        self.disable_linewrap = false;
        self.new_line_mode = false;
        self.cursor.change_shape(CursorShape::Initial);
        self.output_buffer.update_all_lines();
        self.changed_colors = None;
        self.scrollback_buffer_lines = 0;
        self.search_results = Default::default();
        self.sixel_scrolling = false;
        self.mouse_mode = MouseMode::NoEncoding;
        self.mouse_tracking = MouseTracking::Off;
        self.focus_event_tracking = false;
        self.cursor_is_hidden = false;
        self.supports_kitty_keyboard_protocol = false;
        self.set_scroll_region_to_viewport_size();
        self.application_fg = None;
        self.application_bg = None;
        self.osc133_markers_seen = false;
        self.osc7_payload = None;
        if let Some(images_to_reap) = self.sixel_grid.clear() {
            self.sixel_grid.reap_images(images_to_reap);
        }
        self.kitty_grid.teardown();
        let kitty_image_store = self.kitty_grid.kitty_image_store.clone();
        self.kitty_grid = KittyGrid::new(self.character_cell_size.clone(), kitty_image_store);
        self.kitty_parser.abort_pending();
    }
    /// `reset_terminal_state`, except what is on screen is scrolled into `lines_above` and the
    /// scrollback survives, so a command respawned in this pane appends to the transcript
    /// instead of erasing it. A pane the user scrolled back through keeps the rows below its
    /// viewport too: the scroll position is normalized first. Alternate-screen contents are
    /// still discarded: the primary screen they were hiding is what gets preserved. Images
    /// anchored in the preserved lines are dropped with the rest of the graphics state; the
    /// text remains.
    pub fn reset_terminal_state_preserving_scrollback(&mut self) {
        if let Some(mut alternate_screen_state) = self.alternate_screen_state.take() {
            if let Some(image_ids_to_reap) = self.sixel_grid.clear() {
                self.sixel_grid.reap_images(image_ids_to_reap);
            }
            self.kitty_grid.teardown();
            alternate_screen_state.apply_contents_to(
                &mut self.lines_above,
                &mut self.viewport,
                &mut self.cursor,
                &mut self.sixel_grid,
                &mut self.kitty_grid,
                &mut self.supports_kitty_keyboard_protocol,
            );
        }
        // A scrolled-up pane keeps the rows newer than what is on screen in `lines_below`, which
        // the reset below would drop. Scroll back down first, so the whole transcript is in
        // `lines_above` + `viewport` by the time we take them. This has to happen after the
        // alternate screen was swapped back out: `lines_below` belongs to the primary screen
        // either way, because entering the alternate screen does not clear it.
        self.reset_viewport();
        let mut preserved_lines = std::mem::take(&mut self.lines_above);
        let mut screen_rows = std::mem::take(&mut self.viewport);
        // Trailing untouched rows are screen padding, not output.
        while screen_rows.back().is_some_and(|row| row.is_empty()) {
            screen_rows.pop_back();
        }
        self.reset_terminal_state();
        let screen_row_count = screen_rows.len();
        transfer_rows_from_viewport_to_lines_above(
            &mut screen_rows,
            &mut preserved_lines,
            &mut self.sixel_grid,
            &mut self.kitty_grid,
            screen_row_count,
            self.width,
        );
        self.lines_above = preserved_lines;
        self.scrollback_buffer_lines = self.recalculate_scrollback_buffer_count();
    }
    fn set_preceding_character(&mut self, terminal_character: TerminalCharacter) {
        self.preceding_char = Some(terminal_character);
    }
    /// Called by the server-side handler for SetPaneRegexHighlights.
    /// Upserts highlights keyed by pattern string for the given plugin.
    pub fn set_plugin_regex_highlights(
        &mut self,
        plugin_id: PluginId,
        highlights: Vec<RegexHighlight>,
        style: &Style,
    ) {
        let slot = self.plugin_highlights.entry(plugin_id).or_default();
        for h in highlights {
            let (fg, bg) = resolve_highlight_colors(&h.style, style);
            if let Ok(regex) = regex::Regex::new(&h.pattern) {
                // Upsert: replace existing entry with same pattern and on_hover flag, or push new
                let on_hover = h.on_hover;
                if let Some(existing) = slot
                    .iter_mut()
                    .find(|(p, c)| p == &h.pattern && c.on_hover == on_hover)
                {
                    existing.1 = CompiledHighlight {
                        regex,
                        fg,
                        bg,
                        context: h.context,
                        on_hover: h.on_hover,
                        bold: h.bold,
                        italic: h.italic,
                        underline: h.underline,
                        tooltip_text: h.tooltip_text.clone(),
                        layer: h.layer,
                    };
                } else {
                    slot.push((
                        h.pattern,
                        CompiledHighlight {
                            regex,
                            fg,
                            bg,
                            context: h.context,
                            on_hover: h.on_hover,
                            bold: h.bold,
                            italic: h.italic,
                            underline: h.underline,
                            tooltip_text: h.tooltip_text,
                            layer: h.layer,
                        },
                    ));
                }
            } else {
                log::warn!(
                    "Plugin {} supplied invalid regex: {:?}",
                    plugin_id,
                    h.pattern
                );
            }
        }
        self.output_buffer.update_all_lines();
        self.recompute_hover_tooltip();
    }

    /// Called by the server-side handler for ClearPaneHighlights.
    pub fn clear_plugin_highlights(&mut self, plugin_id: PluginId) {
        if self.plugin_highlights.remove(&plugin_id).is_some() {
            self.output_buffer.update_all_lines();
            self.recompute_hover_tooltip();
        }
    }

    /// Returns (plugin_id, pattern, matched_string, context) if the given
    /// pane-relative position falls inside any plugin highlight match, or None.
    /// Uses logical-line grouping so multi-line (wrapped) matches are detected.
    pub fn plugin_highlight_at(
        &self,
        position: &Position,
    ) -> Option<(PluginId, String, String, BTreeMap<String, String>)> {
        let click_row = position.line.get() as usize;
        let click_col = position.column.get();

        let (_canonical, _group_len, logical_text, boundaries) =
            collect_and_build_logical_line(&self.viewport, click_row)?;

        let mut best: Option<RankedHighlight> = None;
        for (plugin_id, pattern_map) in &self.plugin_highlights {
            for (pattern, compiled) in pattern_map {
                for mat in highlight_matches(&compiled.regex, &logical_text) {
                    if let Some((_sel, start_row, start_col, end_row, end_col)) =
                        match_to_selection(&mat, &boundaries, &self.viewport)
                    {
                        if position_in_span(
                            click_row, click_col, start_row, start_col, end_row, end_col,
                        ) {
                            let dominated = match &best {
                                Some((best_layer, ..)) => compiled.layer > *best_layer,
                                None => true,
                            };
                            if dominated {
                                best = Some((
                                    compiled.layer,
                                    *plugin_id,
                                    pattern.clone(),
                                    mat.as_str().to_string(),
                                    compiled.context.clone(),
                                ));
                            }
                        }
                    }
                }
            }
        }
        best.map(|(_layer, plugin_id, pattern, matched, ctx)| (plugin_id, pattern, matched, ctx))
    }

    pub fn set_hover_position(&mut self, new_pos: Option<Position>) -> bool {
        if self.hover_position == new_pos {
            return false;
        }

        let has_hover_consumer = self
            .plugin_highlights
            .values()
            .any(|highlights| highlights.iter().any(|(_, c)| c.on_hover));

        if !has_hover_consumer {
            self.hover_position = new_pos;
            return false;
        }

        // Mark the canonical group containing the old hover row dirty.
        if let Some(old_pos) = self.hover_position {
            self.mark_logical_line_dirty(old_pos.line.get() as usize);
        }
        // Mark the canonical group containing the new hover row dirty.
        if let Some(new_pos_inner) = new_pos {
            self.mark_logical_line_dirty(new_pos_inner.line.get() as usize);
        }

        self.hover_position = new_pos;
        self.recompute_hover_tooltip();
        true
    }

    /// Mark all physical rows belonging to the logical line group that contains
    /// `row_idx` as dirty in the output buffer.
    fn mark_logical_line_dirty(&mut self, row_idx: usize) {
        if let Some((canonical, group_len, _, _)) =
            collect_and_build_logical_line(&self.viewport, row_idx)
        {
            for r in canonical..canonical + group_len {
                self.output_buffer.update_line(r);
            }
        }
    }

    /// Recompute the cached hover tooltip from the current hover position and
    /// plugin highlights.  Called whenever the hover position, highlight set, or
    /// highlight clearing changes.
    fn recompute_hover_tooltip(&mut self) {
        self.cached_hover_tooltip = None;
        let hover_pos = match self.hover_position {
            Some(p) => p,
            None => return,
        };
        if self.mouse_tracking != MouseTracking::Off {
            return;
        }
        if self.plugin_highlights.is_empty() {
            return;
        }
        let hover_row = hover_pos.line.get() as usize;
        let hover_col = hover_pos.column.get();
        let (_canonical, _group_len, logical_text, boundaries) =
            match collect_and_build_logical_line(&self.viewport, hover_row) {
                Some(v) => v,
                None => return,
            };
        let mut best_layer: Option<HighlightLayer> = None;
        let mut best_tooltip: Option<String> = None;
        for pattern_map in self.plugin_highlights.values() {
            for (_pattern, compiled) in pattern_map {
                if !compiled.on_hover || compiled.tooltip_text.is_none() {
                    continue;
                }
                for mat in highlight_matches(&compiled.regex, &logical_text) {
                    if let Some((_sel, start_row, start_col, end_row, end_col)) =
                        match_to_selection(&mat, &boundaries, &self.viewport)
                    {
                        if position_in_span(
                            hover_row, hover_col, start_row, start_col, end_row, end_col,
                        ) {
                            let dominated = match best_layer {
                                Some(bl) => compiled.layer > bl,
                                None => true,
                            };
                            if dominated {
                                best_layer = Some(compiled.layer);
                                best_tooltip = compiled.tooltip_text.clone();
                            }
                        }
                    }
                }
            }
        }
        self.cached_hover_tooltip = best_tooltip;
    }

    /// Pre-compute plugin highlight selections across all logical line groups in
    /// the viewport.  Hover highlights are emitted first so they take priority
    /// when the cursor overlaps a match; non-hover highlights follow.
    fn compute_plugin_highlight_selections(&self) -> Vec<HighlightSelection> {
        if self.plugin_highlights.is_empty() {
            return vec![];
        }
        let mut selections = Vec::new();
        let viewport_len = self.viewport.len();
        let mut ridx = 0;
        while ridx < viewport_len {
            let (_canonical, group_len, logical_text, boundaries) =
                match collect_and_build_logical_line(&self.viewport, ridx) {
                    Some(v) => v,
                    None => break,
                };

            // Hover highlights are pushed first so that `.find()` in
            // `adjust_styles_for_possible_selection` returns the hover
            // style when the cursor overlaps the match.  They are suppressed
            // when mouse tracking is active (events pass through to the app)
            // and on unfocused panes (hover_position is not set for those).
            if self.mouse_tracking == MouseTracking::Off {
                if let Some(hover_pos) = self.hover_position {
                    let hover_row = hover_pos.line.get() as usize;
                    let group_end_row = ridx + group_len - 1;
                    if hover_row >= ridx && hover_row <= group_end_row {
                        let hover_col = hover_pos.column.get();
                        for pattern_map in self.plugin_highlights.values() {
                            for (_pattern, compiled) in pattern_map {
                                if !compiled.on_hover || !compiled.has_visual_effect() {
                                    continue;
                                }
                                for mat in highlight_matches(&compiled.regex, &logical_text) {
                                    if let Some((sel, start_row, start_col, end_row, end_col)) =
                                        match_to_selection(&mat, &boundaries, &self.viewport)
                                    {
                                        if position_in_span(
                                            hover_row, hover_col, start_row, start_col, end_row,
                                            end_col,
                                        ) {
                                            selections.push(HighlightSelection {
                                                selection: sel,
                                                bg: compiled.bg,
                                                fg: compiled.fg,
                                                bold: compiled.bold,
                                                italic: compiled.italic,
                                                underline: compiled.underline,
                                                layer: compiled.layer,
                                            });
                                            break; // only one match per pattern per hover
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Non-hover highlights are pushed after hover so that hover
            // takes precedence for overlapping regions.
            for pattern_map in self.plugin_highlights.values() {
                for (_pattern, compiled) in pattern_map {
                    if compiled.on_hover || !compiled.has_visual_effect() {
                        continue;
                    }
                    for mat in highlight_matches(&compiled.regex, &logical_text) {
                        if let Some((sel, _, _, _, _)) =
                            match_to_selection(&mat, &boundaries, &self.viewport)
                        {
                            selections.push(HighlightSelection {
                                selection: sel,
                                bg: compiled.bg,
                                fg: compiled.fg,
                                bold: compiled.bold,
                                italic: compiled.italic,
                                underline: compiled.underline,
                                layer: compiled.layer,
                            });
                        }
                    }
                }
            }

            ridx += group_len; // advance past this logical line group
        }
        // Sort by layer priority (highest first) and within the same layer,
        // hover highlights before non-hover (hover entries were pushed first,
        // so a stable sort preserves their relative order).
        selections.sort_by(|a, b| {
            use std::cmp::Reverse;
            Reverse(a.layer).cmp(&Reverse(b.layer))
        });
        selections
    }

    pub fn start_selection(&mut self, start: &Position) {
        let old_selection = self.selection;
        // a mouse selection takes over from a keyboard one, cursor included
        self.text_selection_active = false;
        self.click.record_click(*start);

        if self.click.is_double_click() {
            let Some((start_position, end_position)) = self.word_around_position(start) else {
                // no-op
                return;
            };
            self.selection
                .set_start_and_end_positions(start_position, end_position);
            for i in std::cmp::min(start_position.line.get(), end_position.line.get())
                ..=std::cmp::max(start_position.line.get(), end_position.line.get())
            {
                self.output_buffer.update_line(i as usize);
            }
            self.mark_for_rerender();
            return;
        } else if self.click.is_triple_click() {
            let Some((start_position, end_position)) = self
                .osc133_command_selection
                .then(|| self.osc133_command_around_position(start))
                .flatten()
                .or_else(|| self.canonical_line_around_position(start))
            else {
                // no-op
                return;
            };
            self.selection
                .set_start_and_end_positions(start_position, end_position);
            let current_selection = self.selection;
            self.update_selected_lines(&old_selection, &current_selection);
            self.mark_for_rerender();
            return;
        }

        self.selection.start(*start);
        self.update_selected_lines(&old_selection, &self.selection.clone());
        self.mark_for_rerender();
    }
    pub fn start_text_selection(&mut self, start: Position) {
        let old_selection = self.selection;
        self.copied_text_selection = None;
        self.text_selection_active = true;
        self.selection.start(start);
        self.update_selected_lines(&old_selection, &self.selection.clone());
        self.mark_for_rerender();
    }
    pub fn move_text_selection(&mut self, direction: Direction) {
        if !self.selection.is_active() || self.height == 0 || self.width == 0 {
            return;
        }
        let mut end = self.selection.end;
        match direction {
            Direction::Left => end.change_column(end.column.get().saturating_sub(1)),
            Direction::Right => end.change_column((end.column.get() + 1).min(self.width)),
            Direction::Up if end.line.get() > 0 => end.change_line(end.line.get() - 1),
            Direction::Down if end.line.get() < self.height.saturating_sub(1) as isize => {
                end.change_line(end.line.get() + 1);
            },
            Direction::Up => {
                self.scroll_up_one_line();
                return;
            },
            Direction::Down => {
                self.scroll_down_one_line();
                return;
            },
        }
        let old_selection = self.selection;
        self.selection.to(end);
        self.update_selected_lines(&old_selection, &self.selection.clone());
        self.mark_for_rerender();
    }
    pub fn swap_text_selection_ends(&mut self) {
        if !self.selection.is_active() {
            return;
        }
        self.selection.swap_ends();
        self.mark_for_rerender();
    }
    pub fn remember_copied_text_selection(&mut self) {
        let mut copied_text_selection = self.selection;
        copied_text_selection.finalize();
        self.copied_text_selection = Some(copied_text_selection);
    }
    pub fn finish_text_selection(&mut self) {
        let old_selection = self.selection;
        self.text_selection_active = false;
        if let Some(copied_text_selection) = self.copied_text_selection.take() {
            self.selection = copied_text_selection;
        } else if self.selection.is_active() {
            self.reset_selection();
            return;
        }
        self.update_selected_lines(&old_selection, &self.selection.clone());
        self.mark_for_rerender();
    }
    pub fn update_selection(&mut self, to: &Position) {
        let old_selection = self.selection;
        if &old_selection.end != to {
            if self.click.is_double_click() {
                let Some((word_start_position, word_end_position)) = self.word_around_position(to)
                else {
                    // no-op
                    return;
                };
                self.selection
                    .add_word_to_position(word_start_position, word_end_position);
                let current_selection = self.selection;
                self.update_selected_lines(&old_selection, &current_selection);
                self.mark_for_rerender();
            } else if self.click.is_triple_click() {
                let Some(last_index_in_line) = self.last_index_in_line(to) else {
                    return;
                };
                self.selection
                    .add_line_to_position(to.line.get(), last_index_in_line);
                let current_selection = self.selection;
                self.update_selected_lines(&old_selection, &current_selection);
                self.mark_for_rerender();
            } else {
                self.selection.to(*to);
                self.update_selected_lines(&old_selection, &self.selection.clone());
                self.mark_for_rerender();
            }
        }
    }

    pub fn end_selection(&mut self, end: &Position) {
        self.text_selection_active = false;
        if !self.click.is_double_click() && !self.click.is_triple_click() {
            let old_selection = self.selection;
            self.selection.end(*end);
            self.update_selected_lines(&old_selection, &self.selection.clone());
        } else {
            // we do this rather than using .end() so that the selection will be marked as inactive
            // (so we won't keep changing its start/end points as we scroll) but so we won't update
            // its end position to the above "end" position (which is incorrect behavior for
            // double/triple click - it will mean we won't mark until the end of the word/line)
            self.selection.finalize();
        }
        self.mark_for_rerender();
    }

    pub fn reset_selection(&mut self) {
        let old_selection = self.selection;
        self.text_selection_active = false;
        self.selection.reset();
        self.update_selected_lines(&old_selection, &self.selection.clone());
        self.mark_for_rerender();
    }
    pub fn get_selected_text(&self) -> Option<String> {
        if self.selection.is_empty() {
            return None;
        }
        self.text_for_selection(&self.selection)
    }
    /// The text between two positions, resolved the way a selection is (scrollback included).
    pub fn text_in_range(&self, start: Position, end: Position) -> Option<String> {
        let mut range_selection = Selection::default();
        range_selection.set_start_and_end_positions(start, end);
        if range_selection.is_empty() {
            return None;
        }
        self.text_for_selection(&range_selection)
    }
    /// The text a `Selection` covers, whether or not it is the selection the user is making.
    ///
    /// Split out of `get_selected_text` so that anything with a pair of positions - the word under
    /// the cursor, the logical line under it - can be turned into text without going through the
    /// grid's own selection state. Two things it carries that a naive row-slice would not: the
    /// lookup spans `lines_above` and `lines_below`, so a selection reaching into scrollback
    /// resolves; and a non-canonical row is joined onto the previous one, so a wrapped logical
    /// line comes back as the single line it is on screen.
    pub fn text_for_selection(&self, selection: &Selection) -> Option<String> {
        // Track the last explicit character's byte offset in each logical line.
        let mut lines: Vec<(String, usize)> = vec![];

        let sorted_selection = selection.sorted();
        let (start, end) = (sorted_selection.start, sorted_selection.end);

        for l in sorted_selection.line_indices() {
            // on the first line of the selection, use the selection start column
            // otherwise, start at the beginning of the line
            let start_column = if l == start.line.get() {
                start.column.get()
            } else {
                0
            };

            // same thing on the last line, but with the selection end column
            let end_column = if l == end.line.get() {
                end.column.get()
            } else {
                self.width
            };

            if start_column == end_column {
                continue;
            }

            let empty_row =
                Row::from_columns(VecDeque::from(vec![EMPTY_TERMINAL_CHARACTER; self.width]));

            // get the row from lines_above, viewport, or lines below depending on index
            let row = if l < 0 && self.lines_above.len() >= l.unsigned_abs() {
                let offset_from_end = l.abs();
                &self.lines_above[self
                    .lines_above
                    .len()
                    .saturating_sub(offset_from_end as usize)]
            } else if l >= 0 && (l as usize) < self.viewport.len() {
                &self.viewport[l as usize]
            } else if (l as usize) < self.height {
                // index is in viewport but there is no line
                &empty_row
            } else if self.lines_below.len() > (l as usize).saturating_sub(self.viewport.len()) {
                &self.lines_below[(l as usize) - self.viewport.len()]
            } else {
                // can't find the line, this probably it's on the pane border
                // is on the pane border
                continue;
            };

            if row.is_canonical || lines.is_empty() {
                lines.push((String::new(), 0));
            }
            let (line, explicit_end) = lines.last_mut()?;
            let mut terminal_col = 0;
            for terminal_character in &row.columns {
                if (start_column..end_column).contains(&terminal_col) {
                    line.push(terminal_character.character);
                    if !terminal_character.is_implicit_blank() {
                        *explicit_end = line.len();
                    }
                }

                terminal_col += terminal_character.width();
            }
        }

        // Trim padding only after joining wrapped rows, preserving interior gaps.
        let lines: Vec<_> = lines
            .into_iter()
            .map(|(mut line, end)| {
                line.truncate(end);
                line
            })
            .collect();

        if lines.is_empty() {
            None
        } else {
            Some(lines.join("\n"))
        }
    }
    pub fn absolute_position_in_scrollback(&self) -> usize {
        self.lines_above.len() + self.cursor.y
    }
    pub fn last_index_in_line(&self, position: &Position) -> Option<usize> {
        let position_row = self.viewport.get(position.line.get() as usize)?;
        Some(position_row.last_index_in_line())
    }
    pub fn word_around_position(&self, position: &Position) -> Option<(Position, Position)> {
        let position_row = self.viewport.get(position.line.get() as usize)?;
        let (index_start, index_end) = position_row
            .word_indices_around_character_index(position.column.get(), &self.word_separators)?;

        let mut position_start = Position::new(position.line.get() as i32, index_start as u16);
        let mut position_end = Position::new(position.line.get() as i32, index_end as u16);
        let mut position_row_is_canonical = position_row.is_canonical;

        while !position_row_is_canonical && position_start.column.get() == 0 {
            if let Some(position_row_above) = self
                .viewport
                .get(position_start.line.get().saturating_sub(1) as usize)
            {
                let new_start_index =
                    position_row_above.word_start_index_of_last_character(&self.word_separators);
                position_start = Position::new(
                    position_start.line.get().saturating_sub(1) as i32,
                    new_start_index as u16,
                );
                position_row_is_canonical = position_row_above.is_canonical;
            } else {
                break;
            }
        }

        let mut column_count_in_row = position_row.columns.len();
        while position_end.column.get() == column_count_in_row {
            if let Some(position_row_below) =
                self.viewport.get(position_end.line.get() as usize + 1)
            {
                if position_row_below.is_canonical {
                    break;
                }
                let new_end_index =
                    position_row_below.word_end_index_of_first_character(&self.word_separators);
                position_end =
                    Position::new(position_end.line.get() as i32 + 1, new_end_index as u16);
                column_count_in_row = position_row_below.columns.len();
            } else {
                break;
            }
        }

        Some((position_start, position_end))
    }
    pub fn canonical_line_around_position(
        &self,
        position: &Position,
    ) -> Option<(Position, Position)> {
        let position_row = self.viewport.get(position.line.get() as usize)?;

        let mut position_start = Position::new(position.line.get() as i32, 0);
        let mut position_end = Position::new(
            position.line.get() as i32,
            (position_row.columns.len() + position_row.excess_width()) as u16,
        );

        let mut found_canonical_row_start = position_row.is_canonical;
        while !found_canonical_row_start {
            if let Some(row_above) = self
                .viewport
                .get(position_start.line.get().saturating_sub(1) as usize)
            {
                position_start.change_line(position_start.line.get().saturating_sub(1));
                found_canonical_row_start = row_above.is_canonical;
            } else {
                break;
            }
        }

        let mut found_canonical_row_end = false;
        while !found_canonical_row_end {
            if let Some(row_below) = self.viewport.get(position_end.line.get() as usize + 1) {
                if row_below.is_canonical {
                    found_canonical_row_end = true;
                } else {
                    position_end = Position::new(
                        position_end.line.get() as i32 + 1,
                        row_below.columns.len() as u16,
                    );
                }
            } else {
                break;
            }
        }
        Some((position_start, position_end))
    }

    fn row_at(&self, line: isize) -> Option<&Row> {
        if line < 0 {
            let offset_from_end = line.unsigned_abs();
            if self.lines_above.len() >= offset_from_end {
                self.lines_above
                    .get(self.lines_above.len() - offset_from_end)
            } else {
                None
            }
        } else if (line as usize) < self.viewport.len() {
            self.viewport.get(line as usize)
        } else {
            self.lines_below.get((line as usize) - self.viewport.len())
        }
    }

    /// The command around `position`, from its `Input` (or `Output`) marker to the next marker.
    ///
    /// Not gated on `osc133_command_selection`: that option only opts out of triple-click grabbing
    /// a command, and the explicit prompt/command keybindings stay usable.
    fn osc133_command_around_position(&self, position: &Position) -> Option<(Position, Position)> {
        if !self.osc133_markers_seen {
            return None;
        }
        let first_line = -(self.lines_above.len() as isize);
        let last_line = (self.viewport.len() + self.lines_below.len()) as isize - 1;
        let clicked_line = position.line.get();
        let clicked_column = position.column.get();
        let marker_position = Self::osc133_marker_position;

        let mut latest_output = None;
        let mut selection_start = None;
        'backward: for line in (first_line..=clicked_line.min(last_line)).rev() {
            let Some(row) = self.row_at(line) else {
                continue;
            };
            for marker in row.osc133_markers.iter().rev() {
                if line == clicked_line && marker.column > clicked_column {
                    continue;
                }
                match marker.kind {
                    Osc133MarkerKind::Output => {
                        if latest_output.is_none() {
                            latest_output = Some(marker_position(line, marker.column));
                        }
                    },
                    Osc133MarkerKind::Input => {
                        latest_output?;
                        selection_start = Some(marker_position(line, marker.column));
                        break 'backward;
                    },
                    Osc133MarkerKind::Prompt | Osc133MarkerKind::End => {
                        latest_output?;
                        break 'backward;
                    },
                }
            }
        }
        let selection_start = selection_start.or(latest_output)?;

        let mut selection_end = None;
        'forward: for line in clicked_line.max(first_line)..=last_line {
            let Some(row) = self.row_at(line) else {
                continue;
            };
            for marker in row.osc133_markers.iter() {
                if line == clicked_line && marker.column <= clicked_column {
                    continue;
                }
                selection_end = Some(marker_position(line, marker.column));
                break 'forward;
            }
        }
        let selection_end = selection_end?;

        Some((selection_start, selection_end))
    }

    fn osc133_marker_position(line: isize, column: usize) -> Position {
        Position::new(line as i32, column.min(u16::MAX as usize) as u16)
    }

    /// How many display rows up the previous prompt is, or `None` when there is none above.
    ///
    /// Walks the scrollback upwards. A `Prompt` marker wins outright; an `Input` marker is
    /// remembered as a fallback for shells that only mark the command, and is used once the
    /// walk passes the `Output`/`End` of the command before it.
    fn previous_prompt_line_delta(&self) -> Option<usize> {
        if !self.osc133_markers_seen {
            return None;
        }
        let first_line = -(self.lines_above.len() as isize);
        let mut input_candidate: Option<usize> = None;
        let mut display_delta = 0;
        for line in (first_line..0).rev() {
            let Some(row) = self.row_at(line) else {
                continue;
            };
            display_delta += calculate_row_display_height(row.width(), self.width);
            for marker in row.osc133_markers.iter().rev() {
                match marker.kind {
                    Osc133MarkerKind::Prompt => return Some(display_delta),
                    Osc133MarkerKind::Input => {
                        if input_candidate.is_none() {
                            input_candidate = Some(display_delta);
                        }
                    },
                    Osc133MarkerKind::Output | Osc133MarkerKind::End => {
                        if let Some(candidate) = input_candidate {
                            return Some(candidate);
                        }
                    },
                }
            }
        }
        input_candidate
    }

    /// How many rows down the next prompt is, capped at what `lines_below` can still scroll.
    fn next_prompt_line_delta(&self) -> Option<usize> {
        if !self.osc133_markers_seen {
            return None;
        }
        let last_line = (self.viewport.len() + self.lines_below.len()) as isize - 1;
        for line in 1..=last_line {
            let Some(row) = self.row_at(line) else {
                continue;
            };
            for marker in row.osc133_markers.iter() {
                match marker.kind {
                    Osc133MarkerKind::Prompt | Osc133MarkerKind::Input => {
                        let reachable_delta = (line as usize).min(self.lines_below.len());
                        return (reachable_delta > 0).then_some(reachable_delta);
                    },
                    Osc133MarkerKind::Output | Osc133MarkerKind::End => {},
                }
            }
        }
        None
    }

    pub fn scroll_to_previous_prompt(&mut self) -> bool {
        match self.previous_prompt_line_delta() {
            Some(delta) => {
                self.move_viewport_up(delta);
                true
            },
            None => false,
        }
    }

    pub fn scroll_to_next_prompt(&mut self) -> bool {
        match self.next_prompt_line_delta() {
            Some(delta) => {
                self.move_viewport_down(delta);
                true
            },
            None => false,
        }
    }

    /// The command at the top of the viewport, or failing that the first command whose output
    /// starts somewhere in the viewport.
    fn osc133_command_at_scroll_position(&self) -> Option<(Position, Position)> {
        if let Some(command) = self.osc133_command_around_position(&Position::new(0, 0)) {
            return Some(command);
        }
        for line in 0..self.viewport.len() as isize {
            let Some(row) = self.row_at(line) else {
                continue;
            };
            for marker in row.osc133_markers.iter() {
                if marker.kind != Osc133MarkerKind::Output {
                    continue;
                }
                let anchor = Self::osc133_marker_position(line, marker.column);
                if let Some(command) = self.osc133_command_around_position(&anchor) {
                    return Some(command);
                }
            }
        }
        None
    }

    pub fn select_command_at_scroll_position(&mut self) -> bool {
        let Some((start_position, end_position)) = self.osc133_command_at_scroll_position() else {
            return false;
        };
        let old_selection = self.selection;
        self.selection
            .set_start_and_end_positions(start_position, end_position);
        self.selection.finalize();
        let current_selection = self.selection;
        self.update_selected_lines(&old_selection, &current_selection);
        self.mark_for_rerender();
        true
    }

    fn command_output_text(&self, start: Position, end: Position) -> Option<String> {
        let output = self.text_in_range(start, end)?;
        let output = output
            .strip_prefix('\n')
            .map(String::from)
            .unwrap_or(output);
        if output.trim().is_empty() {
            return None;
        }
        Some(output)
    }

    /// The output of the most recent command that has both an `Output` marker and a marker
    /// after it (its `End`, or the next prompt), with the positions the text spans.
    pub fn last_completed_command_output(&self) -> Option<(String, Position, Position)> {
        if !self.osc133_markers_seen {
            return None;
        }
        let first_line = -(self.lines_above.len() as isize);
        let last_line = (self.viewport.len() + self.lines_below.len()) as isize - 1;
        let mut command_boundary: Option<Position> = None;
        for line in (first_line..=last_line).rev() {
            let Some(row) = self.row_at(line) else {
                continue;
            };
            for marker in row.osc133_markers.iter().rev() {
                let marker_position = Self::osc133_marker_position(line, marker.column);
                if marker.kind == Osc133MarkerKind::Output {
                    if let Some(output_end) = command_boundary {
                        if let Some(output) = self.command_output_text(marker_position, output_end)
                        {
                            return Some((output, marker_position, output_end));
                        }
                    }
                }
                command_boundary = Some(marker_position);
            }
        }
        None
    }

    pub fn set_command_output_flash(&mut self, start: Position, end: Position) {
        let mut flash = Selection::default();
        flash.set_start_and_end_positions(start, end);
        flash.finalize();
        self.command_output_flash = Some(flash);
        self.output_buffer.update_all_lines();
        self.mark_for_rerender();
    }

    pub fn clear_command_output_flash(&mut self) -> bool {
        if self.command_output_flash.take().is_none() {
            return false;
        }
        self.output_buffer.update_all_lines();
        self.mark_for_rerender();
        true
    }

    #[cfg(test)]
    pub fn command_output_flash_is_shown(&self) -> bool {
        self.command_output_flash.is_some()
    }

    fn update_selected_lines(&mut self, old_selection: &Selection, new_selection: &Selection) {
        for l in old_selection.diff(new_selection, self.height) {
            self.output_buffer.update_line(l as usize);
        }
    }
    fn set_title(&mut self, title: String) {
        self.title = Some(title);
    }
    fn push_current_title_to_stack(&mut self) {
        if self.title_stack.len() > MAX_TITLE_STACK_SIZE {
            self.title_stack.remove(0);
        }
        if let Some(title) = &self.title {
            self.title_stack.push(title.clone());
        }
    }
    fn pop_title_from_stack(&mut self) {
        if let Some(popped_title) = self.title_stack.pop() {
            self.title = Some(popped_title);
        }
    }
    fn transfer_rows_to_lines_above(&mut self, count: usize) {
        self.kitty_settle_placements_below_the_viewport();
        let transferred_rows_count = transfer_rows_from_viewport_to_lines_above(
            &mut self.viewport,
            &mut self.lines_above,
            &mut self.sixel_grid,
            &mut self.kitty_grid,
            count,
            self.width,
        );
        self.kitty_reanchor_all_from_pixels();

        self.scrollback_buffer_lines =
            subtract_isize_from_usize(self.scrollback_buffer_lines, transferred_rows_count);
    }
    fn move_cursor_down_by_pixels(&mut self, pixel_count: usize) {
        if let Some(character_cell_size) = {
            let c = *self.character_cell_size.borrow();
            c
        } {
            // thanks borrow checker
            let pixel_height = character_cell_size.height;
            let to_move = (pixel_count as f64 / pixel_height as f64).ceil() as usize;
            for _ in 0..to_move {
                self.add_canonical_line();
            }
        }
    }
    fn current_cursor_pixel_coordinates(&self) -> Option<(usize, usize)> {
        // (x, y)
        if let Some(character_cell_size) = *self.character_cell_size.borrow() {
            let line_count_in_scrollback = self.lines_above.len();
            let y_coordinates =
                (line_count_in_scrollback + self.cursor.y) * character_cell_size.height;
            let x_coordinates = self.cursor.x * character_cell_size.width;
            Some((x_coordinates, y_coordinates))
        } else {
            None
        }
    }
    fn create_sixel_image(&mut self) {
        if let Some((x_pixel_coordinates, y_pixel_coordinates)) =
            self.current_cursor_pixel_coordinates()
        {
            let (x_pixel_coordinates, y_pixel_coordinates) = if self.sixel_scrolling {
                let scrollback_pixel_height =
                    self.lines_above.len() * self.character_cell_size.borrow().unwrap().height;
                (0, scrollback_pixel_height)
            } else {
                (x_pixel_coordinates, y_pixel_coordinates)
            };
            if let Some((image_pixel_height, _image_pixel_width)) = self
                .sixel_grid
                .end_image(x_pixel_coordinates, y_pixel_coordinates)
            {
                if !self.sixel_scrolling {
                    self.move_cursor_down_by_pixels(image_pixel_height);
                }
                self.render_full_viewport();
            }
        }
    }
    fn advance_cursor_after_kitty_placement(&mut self, cols: usize, rows: usize) {
        let mut down_steps = rows.saturating_sub(1);
        let target_x = self.cursor.x + cols;
        if target_x >= self.width {
            self.cursor.x = 0;
            down_steps += 1;
        } else {
            self.cursor.x = target_x;
        }
        for _ in 0..down_steps {
            self.add_canonical_line();
        }
    }
    pub fn handle_kitty_apc(&mut self, raw: &[u8]) -> Option<Result<KittyReplyData, KittyError>> {
        if !self.kitty_host_support.protocol_is_enabled() {
            return None;
        }
        self.kitty_grid.note_command_handled();
        let parsed = self.kitty_parser.parse(raw)?;
        let (result, was_query) = match parsed {
            Ok(command) => {
                let was_query = command.action == KittyAction::Query;
                (self.execute_kitty_command(command), was_query)
            },
            Err(e) => (Err(e), false),
        };
        let reply_bytes = match &result {
            Ok(reply) => format_kitty_reply(reply, was_query),
            Err(error) => format_kitty_error(error),
        };
        if let Some(reply_bytes) = reply_bytes {
            self.pending_messages_to_pty.push(reply_bytes);
        }
        Some(result)
    }
    fn execute_kitty_command(
        &mut self,
        command: KittyCommand,
    ) -> Result<KittyReplyData, KittyError> {
        match command.action {
            KittyAction::Query => {
                if self.kitty_host_support.host_supports_graphics() {
                    Ok(KittyReplyData::from_command(&command))
                } else {
                    Err(KittyError {
                        code: KittyErrorCode::Enotsupported,
                        message: "kitty graphics not supported by host terminal".to_owned(),
                        image_id: command.image_id,
                        image_number: command.image_number,
                        placement_id: command.placement_id,
                        quiet: command.quiet,
                    })
                }
            },
            KittyAction::Delete => {
                let cursor_cell = (self.cursor.x, self.cursor.y);
                let viewport_cells = (self.width, self.height);
                let scrollback_rows = self.lines_above.len();
                let result =
                    self.kitty_grid
                        .delete(&command, cursor_cell, viewport_cells, scrollback_rows);
                self.render_full_viewport();
                self.mark_for_rerender();
                result.map(|_| KittyReplyData::from_command(&command))
            },
            KittyAction::Frame => {
                let image = match command.image.clone() {
                    Some(image) => image,
                    None => {
                        return Err(KittyError {
                            code: KittyErrorCode::Einval,
                            message: "missing frame data".to_owned(),
                            image_id: command.image_id,
                            image_number: command.image_number,
                            placement_id: command.placement_id,
                            quiet: command.quiet,
                        });
                    },
                };
                let frame_number = self.kitty_grid.transmit_frame(&command, image)?;
                Ok(KittyReplyData {
                    frame_number: Some(frame_number),
                    ..KittyReplyData::from_command(&command)
                })
            },
            KittyAction::Animate | KittyAction::Compose => {
                if command.action == KittyAction::Animate {
                    self.kitty_grid.animate(&command)?;
                } else {
                    self.kitty_grid.compose_frames(&command)?;
                }
                // Making a frame current, or composing onto the one that is, changes the pixels
                // behind every placement of the image.
                self.render_full_viewport();
                self.mark_for_rerender();
                Ok(KittyReplyData::from_command(&command))
            },
            KittyAction::Transmit => {
                let image = match command.image.clone() {
                    Some(image) => image,
                    None => {
                        return Err(KittyError {
                            code: KittyErrorCode::Einval,
                            message: "missing image data".to_owned(),
                            image_id: command.image_id,
                            image_number: command.image_number,
                            placement_id: command.placement_id,
                            quiet: command.quiet,
                        });
                    },
                };
                match self.kitty_grid.transmit(&command, image) {
                    Ok(assigned_id) => Ok(KittyReplyData {
                        image_id: Some(assigned_id),
                        image_number: command.image_number,
                        placement_id: command.placement_id,
                        frame_number: None,
                        quiet: command.quiet,
                    }),
                    Err(e) => Err(e),
                }
            },
            KittyAction::TransmitAndDisplay | KittyAction::Display => {
                let resolved = if command.action == KittyAction::TransmitAndDisplay {
                    match command.image.clone() {
                        Some(image) => self.kitty_grid.transmit(&command, image).map(|id| {
                            let internal = *self
                                .kitty_grid
                                .pane_image_id_map()
                                .get(&id)
                                .expect("freshly transmitted image is mapped");
                            (id, internal)
                        }),
                        None => Err(KittyError {
                            code: KittyErrorCode::Einval,
                            message: "missing image data".to_owned(),
                            image_id: command.image_id,
                            image_number: command.image_number,
                            placement_id: command.placement_id,
                            quiet: command.quiet,
                        }),
                    }
                } else {
                    self.kitty_grid.resolve_display_target(&command)
                };
                let (pane_image_id, internal) = resolved?;
                // A relative placement can name a virtual parent, whose position exists only as
                // the placeholder cells standing for it. Those are derived from the grid on
                // demand, so they have to be up to date before the parent can be resolved.
                if command.is_relative() {
                    self.kitty_refresh_placeholder_placements();
                }
                let cell = *self.character_cell_size.borrow();
                // Virtual placements have no cursor anchor. Retain their source and requested
                // dimensions even before the host has supplied pixel geometry.
                let cursor_px = self.current_cursor_pixel_coordinates().unwrap_or((0, 0));
                let vertical_anchor = if let Some(cell) = cell {
                    let placement_top_px = (cursor_px.1
                        + std::cmp::min(command.cell_offset_y as usize, cell.height - 1))
                        as isize;
                    Self::kitty_anchor_from_pixel_y(
                        placement_top_px,
                        cell.height as isize,
                        &self.kitty_canonical_line_starts(),
                    )
                } else {
                    KittyVerticalAnchor {
                        canonical_line: 0,
                        offset_px_from_line_start: 0,
                    }
                };
                match self.kitty_grid.place(
                    pane_image_id,
                    internal,
                    &command,
                    cursor_px,
                    cell,
                    vertical_anchor,
                ) {
                    Ok(dest_cells) => {
                        // `None` covers the two placements that occupy no cells at the cursor: a
                        // virtual one, which draws nothing until placeholder cells instantiate it,
                        // and a relative one, which is positioned from its parent and so must
                        // leave the cursor alone whatever `C` says.
                        if let Some((cols, rows)) = dest_cells {
                            if !command.suppress_cursor_movement {
                                self.advance_cursor_after_kitty_placement(
                                    cols as usize,
                                    rows as usize,
                                );
                            }
                        }
                        self.kitty_reanchor_all_from_pixels();
                        self.render_full_viewport();
                        self.mark_for_rerender();
                        Ok(KittyReplyData {
                            image_id: Some(pane_image_id),
                            image_number: command.image_number,
                            placement_id: command.placement_id,
                            frame_number: None,
                            quiet: command.quiet,
                        })
                    },
                    Err(e) => Err(e),
                }
            },
        }
    }
    pub fn kitty_commands_handled(&self) -> u64 {
        self.kitty_grid.commands_handled()
    }
    /// Placements resolve lazily: unicode placeholder cells are read out of the grid on demand
    /// rather than tracked as they are printed, so both of these refresh before answering.
    pub fn kitty_placement_count(&mut self) -> usize {
        self.kitty_refresh_placeholder_placements();
        self.kitty_grid.placement_count()
    }
    pub fn kitty_placements(&mut self) -> &[KittyPlacement] {
        self.kitty_refresh_placeholder_placements();
        self.kitty_grid.placements()
    }
    fn mouse_buttons_value_x10(&self, event: &MouseEvent) -> u8 {
        let mut value = 35; // Default to no buttons down.
        if event.event_type == MouseEventType::Release {
            return value;
        }
        if event.left {
            value = 32;
        } else if event.middle {
            value = 33;
        } else if event.right {
            value = 34;
        } else if event.wheel_up {
            value = 96;
        } else if event.wheel_down {
            value = 97;
        }
        if event.event_type == MouseEventType::Motion {
            value += 32;
        }
        if event.shift {
            value |= 0x04;
        }
        if event.alt {
            value |= 0x08;
        }
        if event.ctrl {
            value |= 0x10;
        }
        value
    }
    fn mouse_buttons_value_sgr(&self, event: &MouseEvent) -> u8 {
        let mut value = 3; // Default to no buttons down.
        if event.left {
            value = 0;
        } else if event.middle {
            value = 1;
        } else if event.right {
            value = 2;
        } else if event.wheel_up {
            value = 64;
        } else if event.wheel_down {
            value = 65;
        }
        if event.event_type == MouseEventType::Motion {
            value += 32;
        }
        if event.shift {
            value |= 0x04;
        }
        if event.alt {
            value |= 0x08;
        }
        if event.ctrl {
            value |= 0x10;
        }
        value
    }
    pub fn mouse_event_signal(&self, event: &MouseEvent) -> Option<String> {
        let emit = match (&self.mouse_tracking, event.event_type) {
            (MouseTracking::Off, _) => false,
            (MouseTracking::AnyEventTracking, _) => true,
            (_, MouseEventType::Press | MouseEventType::Release) => true,
            (MouseTracking::ButtonEventTracking, MouseEventType::Motion) => {
                event.left | event.right | event.middle | event.wheel_up | event.wheel_down
            },
            (_, _) => false,
        };

        match (emit, &self.mouse_mode) {
            (true, MouseMode::NoEncoding | MouseMode::Utf8) => {
                let mut msg: Vec<u8> = vec![27, b'[', b'M', self.mouse_buttons_value_x10(event)];
                msg.append(&mut utf8_mouse_coordinates(
                    event.position.column() + 1,
                    event.position.line() + 1,
                ));
                Some(String::from_utf8_lossy(&msg).into())
            },
            (true, MouseMode::Sgr) => Some(format!(
                "\u{1b}[<{:?};{:?};{:?}{}",
                self.mouse_buttons_value_sgr(event),
                event.position.column() + 1,
                event.position.line() + 1,
                match event.event_type {
                    MouseEventType::Press | MouseEventType::Motion => 'M',
                    _ => 'm',
                }
            )),
            (_, _) => None,
        }
    }
    pub fn mouse_left_click_signal(&self, position: &Position, is_held: bool) -> Option<String> {
        let event = if is_held {
            MouseEvent::new_left_motion_event(*position)
        } else {
            MouseEvent::new_left_press_event(*position)
        };
        self.mouse_event_signal(&event)
    }
    pub fn mouse_left_click_release_signal(&self, position: &Position) -> Option<String> {
        self.mouse_event_signal(&MouseEvent::new_left_release_event(*position))
    }
    pub fn mouse_right_click_signal(&self, position: &Position, is_held: bool) -> Option<String> {
        let event = if is_held {
            MouseEvent::new_right_motion_event(*position)
        } else {
            MouseEvent::new_right_press_event(*position)
        };
        self.mouse_event_signal(&event)
    }
    pub fn mouse_right_click_release_signal(&self, position: &Position) -> Option<String> {
        self.mouse_event_signal(&MouseEvent::new_right_release_event(*position))
    }
    pub fn mouse_middle_click_signal(&self, position: &Position, is_held: bool) -> Option<String> {
        let event = if is_held {
            MouseEvent::new_middle_motion_event(*position)
        } else {
            MouseEvent::new_middle_press_event(*position)
        };
        self.mouse_event_signal(&event)
    }
    pub fn mouse_middle_click_release_signal(&self, position: &Position) -> Option<String> {
        self.mouse_event_signal(&MouseEvent::new_middle_release_event(*position))
    }
    pub fn mouse_scroll_up_signal(&self, position: &Position) -> Option<String> {
        self.mouse_event_signal(&MouseEvent::new_scroll_up_event(*position))
    }
    pub fn mouse_scroll_down_signal(&self, position: &Position) -> Option<String> {
        self.mouse_event_signal(&MouseEvent::new_scroll_down_event(*position))
    }
    pub fn is_alternate_mode_active(&self) -> bool {
        self.alternate_screen_state.is_some()
    }
    pub fn focus_event(&self) -> Option<String> {
        if self.focus_event_tracking {
            Some("\u{1b}[I".into())
        } else {
            None
        }
    }
    pub fn unfocus_event(&self) -> Option<String> {
        if self.focus_event_tracking {
            Some("\u{1b}[O".into())
        } else {
            None
        }
    }
    pub fn delete_viewport_and_scroll(&mut self) {
        self.lines_above.clear();
        self.viewport.clear();
        self.lines_below.clear();
    }
    pub fn reset_cursor_position(&mut self) {
        self.cursor = Cursor::new(0, 0, self.styled_underlines);
    }
    /// Queue a CSI ?997;{1|2}n DSR notification of host color-palette
    /// theme mode onto this grid's pty-write queue. No-op when the app
    /// has not opted in via `CSI ? 2031 h`.
    pub fn push_color_palette_dsr(&mut self, mode: HostTerminalThemeMode) {
        if !self.color_palette_notification_enabled {
            return;
        }
        let code = match mode {
            HostTerminalThemeMode::Dark => 1,
            HostTerminalThemeMode::Light => 2,
        };
        self.pending_messages_to_pty
            .push(format!("\u{1b}[?997;{}n", code).into_bytes());
    }
    fn answer_xtgettcap(&mut self, payload: &[u8]) {
        for name_hex in payload.split(|byte| *byte == b';') {
            let reply = match decode_hex_ascii(name_hex).as_deref() {
                Some("Ms") => format!(
                    "\u{1b}P1+r{}={}\u{1b}\\",
                    encode_hex_ascii("Ms"),
                    encode_hex_ascii("\u{1b}]52;%p1%s;%p2%s\u{7}"),
                ),
                _ => "\u{1b}P0+r\u{1b}\\".to_owned(),
            };
            self.pending_messages_to_pty.push(reply.into_bytes());
        }
    }
    pub fn lock_renders(&mut self) {
        self.kitty_grid
            .protect_completed_frame(&self.completed_kitty);
        if let Some(alternate) = self.alternate_screen_state.as_mut() {
            alternate
                .kitty_grid
                .protect_completed_frame(&self.completed_kitty);
        }
        self.lock_renders = true;
    }
    pub fn unlock_renders(&mut self) {
        self.lock_renders = false;
    }
    pub fn update_theme(&mut self, theme: Styling) {
        self.style.colors = theme;
        self.recompile_agent_theme();
    }
    pub fn set_agent_theme_mapping(
        &mut self,
        mapping: yinz_types::agent::theme::AgentThemeMapping,
    ) {
        if self.style.agent_theme_mapping != mapping {
            self.style.agent_theme_mapping = mapping;
            self.recompile_agent_theme();
        }
    }
    pub fn apply_agent_theme_event(&mut self, event: yinz_types::agent::theme::AgentThemeEvent) {
        let previous = self.agent_launch.clone();
        event.apply(&mut self.agent_launch);
        if self.agent_launch != previous {
            let snapshot = self.agent_launch.as_ref().map(|a| a.snapshot.clone());
            self.install_agent_palette(snapshot);
        }
    }
    fn install_agent_palette(
        &mut self,
        snapshot: Option<yinz_types::agent::theme::AgentThemeSnapshot>,
    ) {
        self.agent_theme = Rc::new(crate::agent_theme::SourcePalette::new(snapshot, self.style));
        self.agent_palettes.retain(|p| p.strong_count() > 0);
        self.agent_palettes.push(Rc::downgrade(&self.agent_theme));
    }
    pub fn begin_agent_run(&mut self) {
        self.agent_launch = None;
        self.install_agent_palette(None);
    }
    fn observe_agent_terminal_color(&mut self, code: u8, rgb: (u8, u8, u8)) {
        if self.agent_launch.is_some()
            && self
                .agent_theme
                .observe_terminal_color(code, rgb, self.style)
        {
            if let (Some(launch), Some(snapshot)) =
                (&mut self.agent_launch, self.agent_theme.snapshot())
            {
                launch.snapshot = snapshot.clone();
                self.pending_agent_theme_updates
                    .push(yinz_types::agent::theme::AgentThemeEvent {
                        version: yinz_types::agent::theme::EVENT_VERSION,
                        launch_id: launch.launch_id.clone(),
                        snapshot: Some(launch.snapshot.clone()),
                    });
            }
            self.output_buffer.update_all_lines();
            self.repaint_completed_text = true;
            self.should_render = true;
        }
    }
    /// Called only for a completed host query, after the screen's token guard and
    /// fallback synthesis. Clipboard replies cannot consume color observations.
    pub fn record_terminal_color_reply(
        &mut self,
        query: &crate::host_query::HostQuery,
        reply: &[u8],
    ) {
        use crate::host_query::HostQuery;
        let code = match query {
            HostQuery::DefaultForeground { .. } => 10,
            HostQuery::DefaultBackground { .. } => 11,
            _ => return,
        };
        let Some(index) = self
            .agent_color_queries
            .iter()
            .position(|(c, _)| *c == code)
        else {
            return;
        };
        let Some((_, palette)) = self.agent_color_queries.remove(index) else {
            return;
        };
        if palette.ptr_eq(&Rc::downgrade(&self.agent_theme)) {
            if let Some(rgb) = crate::agent_theme::terminal_color_reply(code, reply) {
                self.observe_agent_terminal_color(code, rgb);
            }
        }
    }
    fn recompile_agent_theme(&mut self) {
        self.agent_theme.recompile(self.style);
        self.agent_palettes.retain(|palette| {
            if let Some(palette) = palette.upgrade() {
                palette.recompile(self.style);
                true
            } else {
                false
            }
        });
        self.output_buffer.update_all_lines();
        self.repaint_completed_text = true;
        self.should_render = true;
    }

    pub fn update_arrow_fonts(&mut self, should_support_arrow_fonts: bool) {
        self.arrow_fonts = should_support_arrow_fonts;
    }
    /// The two rendering options a config reload can change. Links are serialized with the new
    /// setting from the next frame; cells already parsed keep the underline style they were
    /// parsed with, and the cursor's pending style picks the new one up at its next reset.
    pub fn update_render_options(&mut self, styled_underlines: bool, osc8_hyperlinks: bool) {
        self.styled_underlines = styled_underlines;
        self.osc8_hyperlinks = osc8_hyperlinks;
    }
    /// Turn the kitty keyboard protocol off, or back on, after the fact. Turning it off also
    /// forgets a negotiation the app has already made, so it stops receiving kitty-encoded keys
    /// at once; turning it back on waits for the app to ask again.
    pub fn set_kitty_keyboard_protocol_disabled(&mut self, disabled: bool) {
        self.explicitly_disable_kitty_keyboard_protocol = disabled;
        if disabled {
            self.supports_kitty_keyboard_protocol = false;
            if let Some(alternate_screen_state) = self.alternate_screen_state.as_mut() {
                alternate_screen_state.supports_kitty_keyboard_protocol = false;
            }
        }
    }
    pub fn update_kitty_host_support(&mut self, supported: KittyHostSupport) {
        self.kitty_host_support = supported;
    }
    pub fn update_sixel_host_support(&mut self, supported: bool) {
        self.sixel_host_support = supported;
    }
    pub fn has_selection(&self) -> bool {
        !self.selection.is_empty()
    }
    pub fn pane_contents(
        &self,
        get_full_scrollback: bool,
        max_scrollback_lines: Option<usize>,
    ) -> PaneContents {
        let mut viewport: Vec<String> = Vec::with_capacity(self.viewport.len());
        for row in &self.viewport {
            let s: String = row.columns.iter().map(|x| x.character).collect();
            viewport.push(s);
        }
        let mut contents = if get_full_scrollback {
            let mut lines_above_viewport: Vec<String> = Vec::with_capacity(self.lines_above.len());
            for row in &self.lines_above {
                let s: String = row.columns.iter().map(|x| x.character).collect();
                lines_above_viewport.push(s);
            }
            // Truncate to last N lines if max specified (Some(0) means "all" — no truncation)
            if let Some(max) = max_scrollback_lines {
                if max > 0 && lines_above_viewport.len() > max {
                    let start = lines_above_viewport.len() - max;
                    lines_above_viewport = lines_above_viewport.split_off(start);
                }
            }
            let mut lines_below_viewport: Vec<String> = Vec::with_capacity(self.lines_below.len());
            for row in &self.lines_below {
                let s: String = row.columns.iter().map(|x| x.character).collect();
                lines_below_viewport.push(s);
            }
            PaneContents::new_with_scrollback(
                viewport,
                self.selection.start,
                self.selection.end,
                lines_above_viewport,
                lines_below_viewport,
            )
        } else {
            PaneContents::new(viewport, self.selection.start, self.selection.end)
        };
        contents.cursor = self.visible_cursor_in_viewport().map(Into::into);
        // `lines_below` is the scroll offset: it is only non-empty once the viewport has been
        // moved up off the live tail.
        contents.is_scrolled = !self.lines_below.is_empty();
        contents
    }
    pub fn pane_contents_with_ansi(
        &self,
        get_full_scrollback: bool,
        max_scrollback_lines: Option<usize>,
    ) -> PaneContents {
        use std::fmt::Write;

        let extract_row_with_ansi = |row: &Row| -> String {
            let mut buf = String::new();
            let mut last_styles: Option<RcCharacterStyles> = None;

            let last_non_space = row
                .columns
                .iter()
                .rposition(|tc| {
                    let space = tc.character == ' ';
                    let styled = !matches!(tc.styles.background, Some(AnsiCode::Reset) | None);
                    !space || styled
                })
                .map(|i| i + 1)
                .unwrap_or(0);

            for tc in row.columns.iter().take(last_non_space) {
                if last_styles.as_ref() != Some(&tc.styles) {
                    write!(buf, "{}", tc.styles).unwrap();
                    last_styles = Some(tc.styles.clone());
                }
                buf.push(tc.character);
            }
            if last_styles.is_some() {
                buf.push_str("\u{1b}[m");
            }
            buf
        };

        let mut viewport: Vec<String> = Vec::with_capacity(self.viewport.len());
        for row in &self.viewport {
            viewport.push(extract_row_with_ansi(row));
        }

        let mut contents = if get_full_scrollback {
            let mut lines_above_viewport: Vec<String> = Vec::with_capacity(self.lines_above.len());
            for row in &self.lines_above {
                lines_above_viewport.push(extract_row_with_ansi(row));
            }
            if let Some(max) = max_scrollback_lines {
                if max > 0 && lines_above_viewport.len() > max {
                    let start = lines_above_viewport.len() - max;
                    lines_above_viewport = lines_above_viewport.split_off(start);
                }
            }
            let mut lines_below_viewport: Vec<String> = Vec::with_capacity(self.lines_below.len());
            for row in &self.lines_below {
                lines_below_viewport.push(extract_row_with_ansi(row));
            }
            PaneContents::new_with_scrollback(
                viewport,
                self.selection.start,
                self.selection.end,
                lines_above_viewport,
                lines_below_viewport,
            )
        } else {
            PaneContents::new(viewport, self.selection.start, self.selection.end)
        };
        contents.cursor = self.visible_cursor_in_viewport().map(Into::into);
        // `lines_below` is the scroll offset: it is only non-empty once the viewport has been
        // moved up off the live tail.
        contents.is_scrolled = !self.lines_below.is_empty();
        contents
    }

    fn visible_cursor_in_viewport(&self) -> Option<(usize, usize)> {
        self.cursor_coordinates()
            .and_then(|(x, y, is_visible)| if is_visible { Some((x, y)) } else { None })
    }
}

impl Drop for Grid {
    fn drop(&mut self) {
        self.clear_completed_sixel();
        self.kitty_grid.teardown();
        if let Some(alt) = self.alternate_screen_state.as_mut() {
            alt.kitty_grid.teardown();
        }
    }
}

impl Perform for Grid {
    fn print(&mut self, c: char) {
        let c = self.cursor.charsets[self.active_charset].map(c);

        // Printable ASCII is always one cell wide, and is the overwhelming majority of what a
        // program prints - take it without consulting the unicode-width tables.
        let terminal_character = if c.is_ascii_graphic() || c == ' ' {
            TerminalCharacter::new_singlewidth_styled(c, self.cursor.pending_styles.clone())
        } else {
            TerminalCharacter::new_styled(c, self.cursor.pending_styles.clone())
        };
        self.set_preceding_character(terminal_character.clone());
        self.add_character(terminal_character);
        if c == PLACEHOLDER {
            // The cell has been written and the cursor has moved past it; open its mark slot so
            // the diacritics that follow have somewhere to land. Opening it here rather than on
            // the first diacritic is what makes a placeholder with *no* marks - which inherits
            // everything from the cell to its left - distinguishable from an ordinary cell.
            let (x, y) = (self.cursor.x.saturating_sub(1), self.cursor.y);
            if let Some(row) = self.viewport.get_mut(y) {
                row.writing(&self.agent_theme);
                // Not unconditional: with linewrap disabled at the right edge `add_character`
                // drops the character on the floor, and there is no cell to annotate.
                if row.character_at(x).map(|character| character.character) == Some(PLACEHOLDER) {
                    row.begin_placeholder_marks(x);
                }
            }
        }
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            7 => {
                self.ring_bell = true;
            },
            8 => {
                // backspace
                self.move_cursor_back(1);
            },
            9 => {
                // tab
                self.advance_to_next_tabstop(self.cursor.pending_styles.clone());
            },
            10..=12 => {
                // 0a, newline
                // 0b, vertical tabulation
                // 0c, form feed
                self.add_newline();
            },
            13 => {
                // 0d, carriage return
                self.move_cursor_to_beginning_of_line();
            },
            14 => {
                self.set_active_charset(CharsetIndex::G1);
            },
            15 => {
                self.set_active_charset(CharsetIndex::G0);
            },
            _ => {
                if self.debug {
                    log::warn!("Unhandled execute: {:?}", byte);
                }
            },
        }
    }

    fn hook(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, c: char) {
        if c == 'q' && intermediates.first() == Some(&b'+') {
            self.xtgettcap_bytes = Some(vec![]);
        } else if c == 'q' && intermediates.is_empty() {
            // we only process sixel images if we know the pixel size of each character cell,
            // otherwise we can't reliably display them
            if self.current_cursor_pixel_coordinates().is_some() {
                let max_sixel_height_in_pixels = if self.sixel_scrolling {
                    let character_cell_height = self.character_cell_size.borrow().unwrap().height; // unwrap here is safe because `current_cursor_pixel_coordinates` above is only Some if it exists
                    Some(self.height * character_cell_height)
                } else {
                    None
                };
                self.sixel_grid.start_image(
                    max_sixel_height_in_pixels,
                    intermediates.iter().collect(),
                    params.iter().collect(),
                );
            }
        } else if c == UI_COMPONENT_DCS_FINAL {
            // UI-component (Yinz internal)
            self.ui_component_bytes = Some(vec![]);
        } else if c == 'n'
            && intermediates.is_empty()
            && params.len() == 1
            && params.iter().next() == Some(&[nested_session::NESTED_DCS_PARAM][..])
        {
            self.nested_frame_bytes = Some(vec![]);
        }
    }

    fn put(&mut self, byte: u8) {
        if self.sixel_grid.is_parsing() {
            self.sixel_grid.handle_byte(byte);
            // we explicitly set this to false here because in the context of Sixel, we only render the
            // image when it's done, i.e. in the unhook method
            self.should_render = false;
        } else if self.ui_component_bytes.is_some() {
            append_dcs_payload(&mut self.ui_component_bytes, byte);
        } else if self.nested_frame_bytes.is_some() {
            append_dcs_payload(&mut self.nested_frame_bytes, byte);
        } else if self.xtgettcap_bytes.is_some() {
            append_dcs_payload(&mut self.xtgettcap_bytes, byte);
        }
    }

    fn unhook(&mut self) {
        if self.sixel_grid.is_parsing() {
            self.create_sixel_image();
        } else if let Some(mut ui_component_bytes) = self.ui_component_bytes.take() {
            let component_bytes = ui_component_bytes.drain(..);
            let style = self.style;
            let arrow_fonts = self.arrow_fonts;
            UiComponentParser::new(self, style, arrow_fonts)
                .parse(component_bytes.collect())
                .non_fatal();
        } else if let Some(nested_frame_bytes) = self.nested_frame_bytes.take() {
            if let Some(message) = nested_session::decode_base64(&nested_frame_bytes)
                .and_then(|payload_bytes| nested_session::decode_payload(&payload_bytes))
            {
                self.pending_nested_session_messages.push(message);
            }
        } else if let Some(xtgettcap_bytes) = self.xtgettcap_bytes.take() {
            self.answer_xtgettcap(&xtgettcap_bytes);
        }
        self.mark_for_rerender();
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        let terminator = if bell_terminated { "\x07" } else { "\x1b\\" };

        if params.is_empty() || params[0].is_empty() {
            return;
        }

        match params[0] {
            // Set window title.
            b"0" | b"2" => {
                if params.len() >= 2 {
                    let title = params[1..]
                        .iter()
                        .flat_map(|x| str::from_utf8(x))
                        .collect::<Vec<&str>>()
                        .join(";")
                        .trim()
                        .to_owned();
                    self.set_title(title);
                }
            },

            b"7" => {
                if let Some(raw) = params.get(1) {
                    if let Some(path) = parse_osc7_path(raw) {
                        self.pending_osc7_cwd = Some(path);
                    }
                }
                // Keep the original URI for the host, including semicolons and percent encoding.
                if params.len() >= 2 {
                    let segments: Option<Vec<&str>> =
                        params[1..].iter().map(|x| str::from_utf8(x).ok()).collect();
                    if let Some(segments) = segments {
                        let uri = segments.join(";");
                        if !uri.is_empty() && !uri.chars().any(|c| c.is_control()) {
                            self.osc7_payload = Some(uri);
                        }
                    }
                }
            },

            // Set color index.
            b"4" => {
                for chunk in params[1..].chunks(2) {
                    let index = chunk.first().and_then(|index| parse_number(index));
                    let color = chunk.get(1).and_then(|color| xparse_color(color));
                    if let (Some(i), Some(c)) = (index, color) {
                        if self.changed_colors.is_none() {
                            self.changed_colors = Some([None; 256]);
                        }
                        self.changed_colors.as_mut().unwrap()[i as usize] = Some(c);
                        self.output_buffer.update_all_lines();
                    } else if chunk.get(1).as_ref().and_then(|c| c.first()) == Some(&b'?') {
                        if let Some(index) = index {
                            if let Some(rgb) = self
                                .effective_colors()
                                .and_then(|colors| colors[index as usize])
                                .and_then(rgb_of_ansi_code)
                            {
                                self.pending_messages_to_pty.push(
                                    format!(
                                        "\x1b]4;{index};{}{terminator}",
                                        osc_color_reply_body(rgb)
                                    )
                                    .into_bytes(),
                                );
                                continue;
                            }
                            self.pending_forwarded_queries.push(
                                crate::host_query::HostQuery::PaletteRegister {
                                    index,
                                    terminator:
                                        crate::host_query::OscTerminator::from_bell_terminated(
                                            bell_terminated,
                                        ),
                                },
                            );
                        }
                    }
                }
            },

            // define hyperlink
            b"8" => self.cursor.pending_styles.update(|styles| {
                styles.link_anchor = self.link_handler.borrow_mut().dispatch_osc8(params)
            }),

            // Get/set Foreground (b"10") or background (b"11") colors
            b"10" | b"11" => {
                if params.len() >= 2 {
                    if let Some(mut dynamic_code) = parse_number(params[0]) {
                        for param in &params[1..] {
                            if param == b"?" {
                                // If this pane has a local override for
                                // the channel being queried (set via
                                // `yinz action set-pane-color` or via
                                // a prior OSC 10;<rgb> / 11;<rgb> from
                                // inside the pane), answer with that
                                // override directly instead of
                                // forwarding to the host. Apps inside
                                // the pane must see the colors Yinz
                                // is actually rendering for them, not
                                // the host terminal's background.
                                let local_override = match dynamic_code {
                                    10 => self.effective_foreground(),
                                    11 => self.effective_background(),
                                    _ => None,
                                };
                                if let Some(rgb) = local_override {
                                    self.observe_agent_terminal_color(dynamic_code, rgb);
                                    let reply = format!(
                                        "\u{1b}]{};{}{}",
                                        dynamic_code,
                                        osc_color_reply_body(rgb),
                                        terminator
                                    );
                                    self.pending_messages_to_pty.push(reply.as_bytes().to_vec());
                                } else {
                                    // No local override — forward to
                                    // the host so the app observes the
                                    // terminal's actual color. Yinz's
                                    // cached copy is refreshed via the
                                    // double-dispatch on the reply.
                                    let term =
                                        crate::host_query::OscTerminator::from_bell_terminated(
                                            bell_terminated,
                                        );
                                    let query = match dynamic_code {
                                        10 => crate::host_query::HostQuery::DefaultForeground {
                                            terminator: term,
                                        },
                                        11 => crate::host_query::HostQuery::DefaultBackground {
                                            terminator: term,
                                        },
                                        _ => {
                                            // Out-of-range dynamic_code
                                            // (shouldn't happen since
                                            // the outer match pins it to
                                            // 10 or 11): skip.
                                            dynamic_code += 1;
                                            continue;
                                        },
                                    };
                                    self.agent_color_queries.push_back((
                                        dynamic_code,
                                        Rc::downgrade(&self.agent_theme),
                                    ));
                                    self.pending_forwarded_queries.push(query);
                                }
                            } else {
                                // Set: parse color and store as pane
                                // default. Only literal RGB is stored;
                                // palette-indexed / named variants (or
                                // a parse failure) are silently dropped
                                // to keep the pane-default fields
                                // narrow.
                                if let Some(rgb) = xparse_color(param).and_then(rgb_of_ansi_code) {
                                    if dynamic_code == 10 {
                                        self.application_fg = Some(rgb);
                                    } else if dynamic_code == 11 {
                                        self.application_bg = Some(rgb);
                                    }
                                    self.output_buffer.update_all_lines();
                                }
                            }
                            dynamic_code += 1;
                        }
                    }
                }
            },

            b"12" => {
                // get/set cursor color currently unimplemented
            },

            b"133" => {
                let marker = params.get(1).and_then(|subcommand| match *subcommand {
                    b"A" | b"P" => Some(Osc133MarkerKind::Prompt),
                    b"B" | b"I" => Some(Osc133MarkerKind::Input),
                    b"C" => Some(Osc133MarkerKind::Output),
                    b"D" => Some(Osc133MarkerKind::End),
                    _ => None,
                });
                if let (Some(marker), Some(row)) = (marker, self.viewport.get_mut(self.cursor.y)) {
                    row.add_osc133_marker(self.cursor.x, marker);
                    self.osc133_markers_seen = true;
                }
            },

            // Set cursor style.
            b"50" => {
                if params.len() >= 2
                    && params[1].len() >= 13
                    && params[1][0..12] == *b"CursorShape="
                {
                    let shape = match params[1][12] as char {
                        '0' => Some(CursorShape::Block),
                        '1' => Some(CursorShape::Beam),
                        '2' => Some(CursorShape::Underline),
                        _ => None,
                    };
                    if let Some(cursor_shape) = shape {
                        self.cursor.change_shape(cursor_shape);
                    }
                }
            },

            // Set clipboard.
            b"52" => {
                if params.len() < 3 {
                    return;
                }

                let clipboard = *params[1].first().unwrap_or(&b'c');
                match params[2] {
                    b"?" => {
                        self.pending_forwarded_queries.push(
                            crate::host_query::HostQuery::ClipboardContent {
                                selection: clipboard as char,
                                terminator: crate::host_query::OscTerminator::from_bell_terminated(
                                    bell_terminated,
                                ),
                            },
                        );
                    },
                    base64 => {
                        if let Ok(bytes) = BASE64_DECODER.decode(base64) {
                            if let Ok(string) = String::from_utf8(bytes) {
                                self.pending_clipboard_update = Some(string);
                            }
                        };
                    },
                }
            },

            // Reset color index.
            b"104" => {
                // Reset all color indexes when no parameters are given.
                if params.len() == 1 {
                    self.changed_colors = None;
                    self.output_buffer.update_all_lines();
                    return;
                }

                // Reset color indexes given as parameters.
                for param in &params[1..] {
                    if let Some(index) = parse_number(param) {
                        if let Some(changed_colors) = &mut self.changed_colors {
                            changed_colors[index as usize] = None;
                        }
                        self.output_buffer.update_all_lines();
                    }
                }

                // Reset all color indexes when no parameters are given.
                if params.len() == 1 {
                    // TBD - reset all color changes - currently unsupported
                    return;
                }

                // Reset color indexes given as parameters.
                for param in &params[1..] {
                    if let Some(_index) = parse_number(param) {
                        // TBD - reset color index - currently unimplemented
                    }
                }
            },

            // Reset foreground color.
            b"110" => {
                self.application_fg = None;
                self.output_buffer.update_all_lines();
            },

            // Reset background color.
            b"111" => {
                self.application_bg = None;
                self.output_buffer.update_all_lines();
            },

            // Reset text cursor color.
            b"112" => {
                // TBD - reset text cursor color - currently unimplemented
            },

            b"99" => {
                if params.len() > 1 {
                    let payload = params
                        .get(1..)
                        .unwrap_or_default()
                        .iter()
                        .flat_map(|x| str::from_utf8(x))
                        .collect::<Vec<&str>>()
                        .join(";");
                    if !payload.is_empty() {
                        let (metadata, rest) = split_osc99_payload(&payload);
                        let metadata = parse_osc99_metadata(metadata);
                        let id = metadata.get("i").copied().unwrap_or_default();
                        let payload_type = metadata
                            .get("p")
                            .map(|value| Osc99PayloadType::from_metadata_value(value))
                            .unwrap_or(Osc99PayloadType::Title);
                        let is_done = metadata.get("d").copied().unwrap_or("1") != "0";
                        let decoded = if metadata.get("e").copied() == Some("1") {
                            BASE64_DECODER
                                .decode(rest.as_bytes())
                                .map(|decoded| String::from_utf8_lossy(&decoded).into_owned())
                                .unwrap_or_else(|_| rest.to_owned())
                        } else {
                            rest.to_owned()
                        };
                        let wants_report = self
                            .notification_tracker
                            .wants_report(id, metadata.get("a").copied());
                        let display =
                            self.notification_tracker
                                .assemble(id, payload_type, is_done, decoded);
                        self.pending_desktop_notifications
                            .push(PendingNotification::Osc99 {
                                payload,
                                terminator: terminator.to_string(),
                                wants_report,
                                display,
                            });
                    }
                }
            },

            b"9" => {
                if params.len() > 1 {
                    // ConEmu reuses OSC 9 for its own subcommands. Only the two that other
                    // terminals emit and honour are recognised - `9;4;<state>;<pct>` progress
                    // and `9;9;<cwd>` - so a notification whose body happens to start with a
                    // small number (`9;3;builds remaining`) is still delivered.
                    let is_conemu_subcommand = params.len() > 2
                        && match params.get(1).copied() {
                            Some(b"4") => params
                                .get(2)
                                .and_then(|state| str::from_utf8(state).ok())
                                .map(|state| state.trim().parse::<u8>().is_ok())
                                .unwrap_or(false),
                            Some(b"9") => true,
                            _ => false,
                        };
                    let body = params
                        .get(1..)
                        .unwrap_or_default()
                        .iter()
                        .flat_map(|x| str::from_utf8(x))
                        .collect::<Vec<&str>>()
                        .join(";");
                    if !body.is_empty() && !is_conemu_subcommand {
                        self.pending_desktop_notifications
                            .push(PendingNotification::Osc9 { body });
                    }
                }
            },

            b"777" if params.get(1) == Some(&b"yinz-agent-history".as_slice()) => {
                use yinz_types::agent::theme::{AgentThemeHandoff, AgentThemeLaunch};
                match params.get(2).copied() {
                    Some(b"begin") => {
                        self.history_replay = None;
                        if let Some(encoded) = params.get(3).filter(|v| v.len() <= 8 * 1024 * 1024)
                        {
                            if let Ok(bytes) = BASE64_DECODER.decode(encoded) {
                                if let Ok(metadata) =
                                    serde_json::from_slice::<AgentThemeHandoff>(&bytes)
                                {
                                    if metadata.valid("") {
                                        let palettes: Vec<_> = metadata
                                            .snapshots
                                            .iter()
                                            .map(|s| {
                                                Rc::new(crate::agent_theme::SourcePalette::new(
                                                    Some(s.clone()),
                                                    self.style,
                                                ))
                                            })
                                            .collect();
                                        self.agent_palettes
                                            .extend(palettes.iter().map(Rc::downgrade));
                                        self.begin_agent_run();
                                        self.history_replay = Some((metadata, palettes));
                                    }
                                }
                            }
                        }
                    },
                    Some(b"select") => {
                        if let Some((_, palettes)) = &self.history_replay {
                            if params.get(3) == Some(&b"none".as_slice()) {
                                self.install_agent_palette(None);
                            } else if let Some(palette) = params
                                .get(3)
                                .and_then(|v| std::str::from_utf8(v).ok())
                                .and_then(|v| v.parse::<usize>().ok())
                                .and_then(|i| palettes.get(i))
                            {
                                self.agent_theme = palette.clone();
                            }
                        }
                    },
                    Some(b"end") => {
                        if let Some((metadata, palettes)) = self.history_replay.take() {
                            self.begin_agent_run();
                            if let Some(active) = metadata.active {
                                self.agent_theme = palettes[active.palette].clone();
                                self.agent_launch = Some(AgentThemeLaunch {
                                    launch_id: active.launch_id,
                                    snapshot: metadata.snapshots[active.palette].clone(),
                                });
                            }
                        }
                    },
                    _ => {},
                }
            },
            b"777" if params.get(1) == Some(&b"yinz-agent-theme".as_slice()) => {
                if let Some(encoded) = params
                    .get(2)
                    .filter(|value| value.len() <= yinz_types::agent::theme::MAX_SNAPSHOT_BYTES * 2)
                {
                    use base64::Engine;
                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded) {
                        if bytes.len() <= yinz_types::agent::theme::MAX_SNAPSHOT_BYTES {
                            if let Ok(event) = serde_json::from_slice(&bytes) {
                                self.apply_agent_theme_event(event);
                            }
                        }
                    }
                }
            },
            b"777"
                if !bell_terminated
                    && params == [b"777".as_slice(), b"yinz", b"implicit-blank"] =>
            {
                if let Some(x) = self.cursor.x.checked_sub(1) {
                    if let Some(row) = self.viewport.get_mut(self.cursor.y) {
                        row.writing(&self.agent_theme);
                        let index = row.absolute_character_index(x);
                        if let Some(character) = row.columns.get_mut(index) {
                            character.mark_implicit_blank();
                        }
                    }
                }
            },

            b"777" => {
                let parts: Vec<&str> = params
                    .get(1..)
                    .unwrap_or_default()
                    .iter()
                    .flat_map(|x| str::from_utf8(x))
                    .collect();
                if parts.first().copied() == Some("notify") {
                    let title = parts.get(1).map(|s| s.to_string()).unwrap_or_default();
                    let body = parts
                        .get(2..)
                        .map(|rest| rest.join(";"))
                        .unwrap_or_default();
                    if !title.is_empty() || !body.is_empty() {
                        self.pending_desktop_notifications
                            .push(PendingNotification::Osc777 { title, body });
                    }
                }
            },

            _ => {
                if self.debug {
                    log::warn!("Unhandled osc: {:?}", params);
                }
            },
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, c: char) {
        let mut params_iter = params.iter();
        let mut next_param_or = |default: u16| {
            params_iter
                .next()
                .map(|param| param[0])
                .filter(|&param| param != 0)
                .unwrap_or(default) as usize
        };
        if c == 'm' {
            if intermediates.is_empty() {
                self.cursor
                    .pending_styles
                    .update(|styles| styles.add_style_from_ansi_params(&mut params_iter))
            }
        } else if c == 'C' || c == 'a' {
            // move cursor forward
            let move_by = next_param_or(1);
            self.move_cursor_forward_until_edge(move_by);
        } else if c == 'K' {
            // clear line (0 => right, 1 => left, 2 => all)
            if let Some(clear_type) = params_iter.next().map(|param| param[0]) {
                let mut char_to_replace = EMPTY_TERMINAL_CHARACTER;
                if let Some(background_color) = self.cursor.pending_styles.background {
                    char_to_replace
                        .styles
                        .update(|styles| styles.background = Some(background_color));
                }
                if clear_type == 0 {
                    self.replace_characters_in_line_after_cursor(char_to_replace);
                } else if clear_type == 1 {
                    self.replace_characters_in_line_before_cursor(char_to_replace);
                } else if clear_type == 2 {
                    self.clear_cursor_line();
                }
            };
        } else if c == 'J' {
            // clear all (0 => below, 1 => above, 2 => all, 3 => saved)
            let mut char_to_replace = EMPTY_TERMINAL_CHARACTER;
            if let Some(background_color) = self.cursor.pending_styles.background {
                char_to_replace
                    .styles
                    .update(|styles| styles.background = Some(background_color));
            }
            if let Some(clear_type) = params_iter.next().map(|param| param[0]) {
                if clear_type == 0 {
                    self.clear_all_after_cursor(char_to_replace);
                } else if clear_type == 1 {
                    self.clear_all_before_cursor(char_to_replace);
                } else if clear_type == 2 {
                    self.set_scroll_region_to_viewport_size();
                    self.fill_viewport(char_to_replace);
                    if let Some(images_to_reap) = self.sixel_grid.clear() {
                        self.sixel_grid.reap_images(images_to_reap);
                    }
                    self.kitty_grid
                        .clear_visible_placements(self.lines_above.len());
                } else if clear_type == 3 {
                    let dropped_rows = self.lines_above.len();
                    self.clear_lines_above();
                    if let Some(images_to_reap) = self.sixel_grid.clear() {
                        self.sixel_grid.reap_images(images_to_reap);
                    }
                    // Only the images that lived in the scrollback go with it - the ones on screen
                    // are untouched, per "the other commands to erase text must have no effect on
                    // graphics".
                    self.kitty_grid.drop_scrollback_rows(dropped_rows);
                    self.kitty_reanchor_all_from_pixels();
                }
            };
        } else if c == 'H' || c == 'f' {
            // goto row/col
            // we subtract 1 from the row/column because these are 1 indexed
            let row = next_param_or(1).saturating_sub(1);
            let col = next_param_or(1).saturating_sub(1);
            self.move_cursor_to(col, row, EMPTY_TERMINAL_CHARACTER);
        } else if c == 'A' {
            // move cursor up until edge of screen
            let move_up_count = next_param_or(1);
            self.move_cursor_up(move_up_count);
        } else if c == 'B' || c == 'e' {
            // move cursor down until edge of screen
            let move_down_count = next_param_or(1);
            let pad_character = EMPTY_TERMINAL_CHARACTER;
            self.move_cursor_down_until_edge_of_screen(move_down_count, pad_character);
        } else if c == 'D' {
            let move_back_count = next_param_or(1);
            self.move_cursor_back(move_back_count);
        } else if c == 'l' {
            let first_intermediate_is_questionmark = match intermediates.first() {
                Some(b'?') => true,
                None => false,
                _ => false,
            };
            if first_intermediate_is_questionmark {
                for param in params_iter.map(|param| param[0]) {
                    match param {
                        2026 => {
                            self.unlock_renders();
                        },
                        2004 => {
                            self.bracketed_paste_mode = false;
                        },
                        1049 => {
                            if let Some(mut alternate_screen_state) =
                                self.alternate_screen_state.take()
                            {
                                self.kitty_grid.teardown();
                                alternate_screen_state.apply_contents_to(
                                    &mut self.lines_above,
                                    &mut self.viewport,
                                    &mut self.cursor,
                                    &mut self.sixel_grid,
                                    &mut self.kitty_grid,
                                    &mut self.supports_kitty_keyboard_protocol,
                                );
                            }
                            self.alternate_screen_state = None;
                            self.clear_viewport_before_rendering = true;
                            self.force_change_size(self.height, self.width); // the alternative_viewport might have been of a different size...
                            self.mark_for_rerender();
                        },
                        25 => {
                            self.hide_cursor();
                            self.mark_for_rerender();
                        },
                        1 => {
                            self.cursor_key_mode = false;
                        },
                        3 => {
                            // DECCOLM - only side effects
                            self.set_scroll_region_to_viewport_size();
                            self.clear_all(EMPTY_TERMINAL_CHARACTER);
                            self.cursor.x = 0;
                            self.cursor.y = 0;
                        },
                        6 => {
                            self.erasure_mode = false;
                        },
                        7 => {
                            self.disable_linewrap = true;
                        },
                        80 => {
                            self.sixel_scrolling = false;
                        },
                        1000 => {
                            self.mouse_tracking = MouseTracking::Off;
                        },
                        1002 => {
                            self.mouse_tracking = MouseTracking::Off;
                        },
                        1003 => {
                            self.mouse_tracking = MouseTracking::Off;
                        },
                        1004 => {
                            self.focus_event_tracking = false;
                        },
                        1005 => {
                            self.mouse_mode = MouseMode::NoEncoding;
                        },
                        1006 => {
                            self.mouse_mode = MouseMode::NoEncoding;
                        },
                        2031 => {
                            self.color_palette_notification_enabled = false;
                        },
                        _ => {},
                    };
                }
            } else {
                for param in params_iter.map(|param| param[0]) {
                    match param {
                        4 => {
                            self.insert_mode = false;
                        },
                        20 => {
                            self.new_line_mode = false;
                        },
                        _ => {},
                    }
                }
            }
        } else if c == 'h' {
            let first_intermediate_is_questionmark = match intermediates.first() {
                Some(b'?') => true,
                None => false,
                _ => false,
            };
            if first_intermediate_is_questionmark {
                for param in params_iter.map(|param| param[0]) {
                    match param {
                        25 => {
                            self.show_cursor();
                            self.mark_for_rerender();
                        },
                        2026 => {
                            self.lock_renders();
                        },
                        2004 => {
                            self.bracketed_paste_mode = true;
                        },
                        1049 => {
                            // enter alternate buffer
                            if self.alternate_screen_state.is_some() {
                                continue;
                            }
                            let current_lines_above = std::mem::take(&mut self.lines_above);
                            let current_viewport = std::mem::replace(
                                &mut self.viewport,
                                VecDeque::from(vec![Row::new().canonical()]),
                            );
                            let current_cursor = std::mem::replace(
                                &mut self.cursor,
                                Cursor::new(0, 0, self.styled_underlines),
                            );
                            let current_supports_kitty_keyboard_protocol = std::mem::replace(
                                &mut self.supports_kitty_keyboard_protocol,
                                false,
                            );
                            let sixel_image_store = self.sixel_grid.sixel_image_store.clone();
                            let alternate_sixelgrid = std::mem::replace(
                                &mut self.sixel_grid,
                                SixelGrid::new(self.character_cell_size.clone(), sixel_image_store),
                            );
                            let kitty_image_store = self.kitty_grid.kitty_image_store.clone();
                            let alternate_kittygrid = std::mem::replace(
                                &mut self.kitty_grid,
                                KittyGrid::new(self.character_cell_size.clone(), kitty_image_store),
                            );
                            self.alternate_screen_state = Some(AlternateScreenState::new(
                                current_lines_above,
                                current_viewport,
                                current_cursor,
                                alternate_sixelgrid,
                                alternate_kittygrid,
                                current_supports_kitty_keyboard_protocol,
                            ));
                            self.clear_viewport_before_rendering = true;
                            self.scrollback_buffer_lines =
                                self.recalculate_scrollback_buffer_count();
                            self.output_buffer.update_all_lines(); // make sure the screen gets cleared in the next render
                        },
                        1 => {
                            self.cursor_key_mode = true;
                        },
                        3 => {
                            // DECCOLM - only side effects
                            self.set_scroll_region_to_viewport_size();
                            self.clear_all(EMPTY_TERMINAL_CHARACTER);
                            self.cursor.x = 0;
                            self.cursor.y = 0;
                        },
                        6 => {
                            self.erasure_mode = true;
                        },
                        7 => {
                            self.disable_linewrap = false;
                        },
                        80 => {
                            self.sixel_scrolling = true;
                        },
                        1000 => {
                            self.mouse_tracking = MouseTracking::Normal;
                        },
                        1002 => {
                            self.mouse_tracking = MouseTracking::ButtonEventTracking;
                        },
                        1003 => {
                            self.mouse_tracking = MouseTracking::AnyEventTracking;
                        },
                        1004 => {
                            self.focus_event_tracking = true;
                        },
                        1005 => {
                            self.mouse_mode = MouseMode::Utf8;
                        },
                        1006 => {
                            self.mouse_mode = MouseMode::Sgr;
                        },
                        2031 => {
                            self.color_palette_notification_enabled = true;
                        },
                        _ => {},
                    }
                }
            } else {
                for param in params_iter.map(|param| param[0]) {
                    match param {
                        4 => {
                            self.insert_mode = true;
                        },
                        20 => {
                            self.new_line_mode = true;
                        },
                        _ => {},
                    }
                }
            }
        } else if c == 'p' {
            let first_intermediate_is_questionmark = match intermediates.first() {
                Some(b'?') => true,
                None => false,
                _ => false,
            };
            if first_intermediate_is_questionmark {
                for param in params_iter.map(|param| param[0]) {
                    match param {
                        2026 => {
                            let response = "\u{1b}[?2026;2$y";
                            self.pending_messages_to_pty
                                .push(response.as_bytes().to_vec());
                        },
                        2031 => {
                            let value = if self.color_palette_notification_enabled {
                                1
                            } else {
                                2
                            };
                            let response = format!("\u{1b}[?2031;{}$y", value);
                            self.pending_messages_to_pty.push(response.into_bytes());
                        },
                        _ => {},
                    }
                }
            }
        } else if c == 'r' {
            if params.len() > 1 {
                let top = next_param_or(1).saturating_sub(1);
                let bottom = params_iter
                    .next()
                    .map(|param| param[0] as usize)
                    .filter(|&param| param != 0)
                    .map(|bottom| {
                        std::cmp::min(self.height.saturating_sub(1), bottom.saturating_sub(1))
                    });
                self.set_scroll_region(top, bottom);
            } else {
                self.set_scroll_region_to_viewport_size();
            }
        } else if c == 'M' {
            // delete lines if currently inside scroll region, or otherwise
            // delete lines in the entire viewport
            let line_count_to_delete = next_param_or(1);
            let mut pad_character = EMPTY_TERMINAL_CHARACTER;
            pad_character.styles = self.cursor.pending_styles.clone();
            self.delete_lines_in_scroll_region(line_count_to_delete, pad_character);
        } else if c == 'L' {
            // insert blank lines if inside scroll region, or otherwise insert
            // blank lines in the entire viewport
            let line_count_to_add = next_param_or(1);
            let mut pad_character = EMPTY_TERMINAL_CHARACTER;
            pad_character.styles = self.cursor.pending_styles.clone();
            self.add_empty_lines_in_scroll_region(line_count_to_add, pad_character);
        } else if c == 'G' || c == '`' {
            let column = next_param_or(1).saturating_sub(1);
            let column = std::cmp::min(column, self.width.saturating_sub(1));
            self.move_cursor_to_column(column);
        } else if c == 'g' {
            let clear_type = next_param_or(0);
            if clear_type == 0 {
                self.clear_tabstop(self.cursor.x);
            } else if clear_type == 3 {
                self.clear_all_tabstops();
            }
        } else if c == 'd' {
            // goto line
            let line = next_param_or(1).saturating_sub(1);
            let pad_character = EMPTY_TERMINAL_CHARACTER;
            self.move_cursor_to_line(line, pad_character);
        } else if c == 'P' {
            // erase characters
            let count = next_param_or(1);
            self.erase_characters(count, self.cursor.pending_styles.clone());
        } else if c == 'X' {
            // erase characters and replace with empty characters of current style
            let count = next_param_or(1);
            self.replace_with_empty_chars(count, self.cursor.pending_styles.clone());
        } else if c == 'T' {
            /*
             * 124  54  T   SD
             * Scroll down, new lines inserted at top of screen
             * [4T = Scroll down 4, bring previous lines back into view
             */
            let line_count = next_param_or(1);
            self.rotate_scroll_region_up(line_count);
        } else if c == 'S' {
            let first_intermediate_is_questionmark = match intermediates.first() {
                Some(b'?') => true,
                None => false,
                _ => false,
            };
            if first_intermediate_is_questionmark {
                let query_type = params_iter.next();
                let is_query = matches!(params_iter.next(), Some(&[1]));
                if is_query {
                    // XTSMGRAPHICS
                    match query_type {
                        Some(&[1]) => {
                            // number of color registers
                            let response = if self.sixel_host_support {
                                "\u{1b}[?1;0;65536S"
                            } else {
                                "\u{1b}[?1;3;0S"
                            };
                            self.pending_messages_to_pty
                                .push(response.as_bytes().to_vec());
                        },
                        Some(&[2]) => {
                            // Sixel graphics geometry in pixels
                            if !self.sixel_host_support {
                                let response = "\u{1b}[?2;3;0S";
                                self.pending_messages_to_pty
                                    .push(response.as_bytes().to_vec());
                            } else if let Some(character_cell_size) =
                                *self.character_cell_size.borrow()
                            {
                                let sixel_area_geometry = format!(
                                    "\u{1b}[?2;0;{};{}S",
                                    character_cell_size.width * self.width,
                                    character_cell_size.height * self.height,
                                );
                                self.pending_messages_to_pty
                                    .push(sixel_area_geometry.as_bytes().to_vec());
                            }
                        },
                        _ => {
                            // unsupported (eg. ReGIS graphics geometry)
                        },
                    }
                }
            } else {
                // move scroll up
                let count = next_param_or(1);
                self.rotate_scroll_region_down(count);
            }
        } else if c == 's' {
            self.save_cursor_position();
        } else if c == 'u' && intermediates == b">" {
            // Yinz only supports the first "progressive enhancement" layer of the kitty keyboard
            // protocol
            // 0 disables, everything else enables.
            let count = next_param_or(0);
            if !self.explicitly_disable_kitty_keyboard_protocol {
                self.supports_kitty_keyboard_protocol = count > 0;
            }
        } else if c == 'u' && intermediates == b"<" {
            // Yinz only supports the first "progressive enhancement" layer of the kitty keyboard
            // protocol
            if !self.explicitly_disable_kitty_keyboard_protocol {
                self.supports_kitty_keyboard_protocol = false;
            }
        } else if c == 'u' && intermediates == b"?" {
            // Yinz only supports the first "progressive enhancement" layer of the kitty keyboard
            // protocol
            let reply = if self.supports_kitty_keyboard_protocol {
                "\u{1b}[?1u"
            } else {
                "\u{1b}[?0u"
            };
            self.pending_messages_to_pty.push(reply.as_bytes().to_vec());
        } else if c == 'u' && intermediates == b"=" {
            // kitty keyboard protocol without the stack, just setting.
            // 0 disables, everything else enables.
            let count = next_param_or(0);
            if !self.explicitly_disable_kitty_keyboard_protocol {
                self.supports_kitty_keyboard_protocol = count > 0;
            }
        } else if c == 'u' {
            self.restore_cursor_position();
        } else if c == '@' {
            let count = next_param_or(1);
            for _ in 0..count {
                let mut pad_character = EMPTY_TERMINAL_CHARACTER;
                pad_character.styles = self.cursor.pending_styles.clone();
                self.add_character_at_cursor_position(pad_character, true);
            }
        } else if c == 'b' {
            if let Some(c) = self.preceding_char.clone() {
                for _ in 0..next_param_or(1) {
                    self.add_character(c.clone());
                }
            }
        } else if c == 'E' {
            // Moves cursor to beginning of the line n (default 1) lines down.
            let count = next_param_or(1);
            let pad_character = EMPTY_TERMINAL_CHARACTER;
            self.move_cursor_down_until_edge_of_screen(count, pad_character);
            self.move_cursor_to_beginning_of_line();
        } else if c == 'F' {
            // Moves cursor to beginning of the line n (default 1) lines up.
            let count = next_param_or(1);
            self.move_cursor_up(count);
            self.move_cursor_to_beginning_of_line();
        } else if c == 'I' {
            for _ in 0..next_param_or(1) {
                self.advance_to_next_tabstop(self.cursor.pending_styles.clone());
            }
        } else if c == 'q' {
            let first_intermediate_is_space = matches!(intermediates.first(), Some(b' '));
            if first_intermediate_is_space {
                // DECSCUSR (CSI Ps SP q) -- Set Cursor Style.
                let cursor_style_id = next_param_or(0);
                let shape = match cursor_style_id {
                    0 => Some(CursorShape::Initial),
                    2 => Some(CursorShape::Block),
                    1 => Some(CursorShape::BlinkingBlock),
                    3 => Some(CursorShape::BlinkingUnderline),
                    4 => Some(CursorShape::Underline),
                    5 => Some(CursorShape::BlinkingBeam),
                    6 => Some(CursorShape::Beam),
                    _ => None,
                };
                if let Some(cursor_shape) = shape {
                    self.cursor.change_shape(cursor_shape);
                }
            } else if matches!(intermediates.first(), Some(b'>')) {
                let version = version_number(VERSION);
                let xtversion = format!("\u{1b}P>|Yinz({})\u{1b}\\", version);
                self.pending_messages_to_pty
                    .push(xtversion.as_bytes().to_vec());
            }
        } else if c == 'Z' {
            for _ in 0..next_param_or(1) {
                self.move_to_previous_tabstop();
            }
        } else if c == 'c' {
            // identify terminal
            // https://vt100.net/docs/vt510-rm/DA1.html
            match intermediates.first() {
                None | Some(0) => {
                    // primary device attributes - VT220 with OSC 52 clipboard, advertising
                    // sixel (attribute 4) only if the attached terminal supports it
                    let terminal_capabilities = if self.sixel_host_support {
                        "\u{1b}[?62;4;52c"
                    } else {
                        "\u{1b}[?62;52c"
                    };
                    self.pending_messages_to_pty
                        .push(terminal_capabilities.as_bytes().to_vec());
                },
                Some(b'>') => {
                    // secondary device attributes
                    let version = version_number(VERSION);
                    let text = format!("\u{1b}[>0;{};1c", version);
                    self.pending_messages_to_pty.push(text.as_bytes().to_vec());
                },
                _ => {},
            }
        } else if c == 'n' {
            // DSR - device status report
            // https://vt100.net/docs/vt510-rm/DSR.html
            let first_intermediate_is_questionmark = match intermediates.first() {
                Some(b'?') => true,
                None => false,
                _ => false,
            };
            if first_intermediate_is_questionmark {
                // CSI ? 996 n — query host terminal color-palette mode
                // (Contour spec; see contour-terminal.org). Yinz
                // short-circuits this query: we know the host's mode
                // from our own startup `\e[?996n` plus unsolicited DSR
                // 997 updates while `\e[?2031h` is enabled, so the
                // pane gets answered locally without a host round-trip.
                // Pushing onto `pending_forwarded_queries` enrols the
                // query in the existing Grid → Tab → Screen pipeline;
                // Screen recognises the variant and writes the reply
                // straight to the pane's pty.
                for param in params_iter.map(|param| param[0]) {
                    if param == 996 {
                        self.pending_forwarded_queries
                            .push(crate::host_query::HostQuery::ColorPaletteMode);
                    }
                }
            } else {
                match next_param_or(0) {
                    5 => {
                        // report terminal status
                        let all_good = "\u{1b}[0n";
                        self.pending_messages_to_pty
                            .push(all_good.as_bytes().to_vec());
                    },
                    6 => {
                        // CPR - cursor position report

                        // Note that this is relative to scrolling region.
                        let offset = self.scroll_region.0; // scroll_region_top
                        let position_report = format!(
                            "\u{1b}[{};{}R",
                            self.cursor.y + 1 - offset,
                            self.cursor.x + 1
                        );
                        self.pending_messages_to_pty
                            .push(position_report.as_bytes().to_vec());
                    },
                    _ => {},
                }
            }
        } else if c == 'x' {
            // DECREQTPARM - Request Terminal Parameters
            // https://vt100.net/docs/vt100-ug/chapter3.html#DECREQTPARM
            //
            // Respond with (same as xterm): Parity NONE, 8 bits,
            // xmitspeed 38400, recvspeed 38400.  (CLoCk MULtiplier =
            // 1, STP option flags = 0)
            //
            // (xterm used to respond to DECREQTPARM in all modes.
            // Now it seems to only do so when explicitly in VT100 mode.)
            let query = next_param_or(0);
            match query {
                0 | 1 => {
                    let response = format!("\u{1b}[{};1;1;128;128;1;0x", query + 2);
                    self.pending_messages_to_pty
                        .push(response.as_bytes().to_vec());
                },
                _ => {},
            }
        } else if c == 't' {
            match next_param_or(1) {
                14 => {
                    // Forward to host: apps asking for text-area pixels
                    // want the real window size, not Yinz's synthesised
                    // (cell_size * grid_size) value. The host's reply will
                    // be written back to this pane by the forwarding
                    // infrastructure on Screen.
                    self.pending_forwarded_queries
                        .push(crate::host_query::HostQuery::TextAreaPixelSize);
                },
                16 => {
                    // Forward to host: character-cell pixel size.
                    self.pending_forwarded_queries
                        .push(crate::host_query::HostQuery::CharacterCellPixelSize);
                },
                18 => {
                    // report text area
                    let text_area_report = format!("\x1b[8;{};{}t", self.height, self.width);
                    self.pending_messages_to_pty
                        .push(text_area_report.as_bytes().to_vec());
                },
                22 => {
                    self.push_current_title_to_stack();
                },
                23 => {
                    self.pop_title_from_stack();
                },
                _ => {},
            }
        } else {
            if self.debug {
                log::warn!("Unhandled csi: {}->{:?}", c, params);
            }
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        match (byte, intermediates.first()) {
            (b'A', charset_index_symbol) => {
                let charset_index: CharsetIndex = match charset_index_symbol {
                    Some(b'(') => CharsetIndex::G0,
                    Some(b')') => CharsetIndex::G1,
                    Some(b'*') => CharsetIndex::G2,
                    Some(b'+') => CharsetIndex::G3,
                    _ => {
                        // invalid, silently do nothing
                        return;
                    },
                };
                self.configure_charset(StandardCharset::UK, charset_index);
            },
            (b'B', charset_index_symbol) => {
                let charset_index: CharsetIndex = match charset_index_symbol {
                    Some(b'(') => CharsetIndex::G0,
                    Some(b')') => CharsetIndex::G1,
                    Some(b'*') => CharsetIndex::G2,
                    Some(b'+') => CharsetIndex::G3,
                    _ => {
                        // invalid, silently do nothing
                        return;
                    },
                };
                self.configure_charset(StandardCharset::Ascii, charset_index);
            },
            (b'0', charset_index_symbol) => {
                let charset_index: CharsetIndex = match charset_index_symbol {
                    Some(b'(') => CharsetIndex::G0,
                    Some(b')') => CharsetIndex::G1,
                    Some(b'*') => CharsetIndex::G2,
                    Some(b'+') => CharsetIndex::G3,
                    _ => {
                        // invalid, silently do nothing
                        return;
                    },
                };
                self.configure_charset(
                    StandardCharset::SpecialCharacterAndLineDrawing,
                    charset_index,
                );
            },
            (b'D', None) => {
                self.add_newline();
            },
            (b'E', None) => {
                self.add_newline();
                self.move_cursor_to_beginning_of_line();
            },
            (b'M', None) => {
                self.move_cursor_up_with_scrolling(1);
            },
            (b'c', None) => {
                self.reset_terminal_state();
            },
            (b'H', None) => {
                self.set_horizontal_tabstop();
            },
            (b'7', None) => {
                self.save_cursor_position();
            },
            (b'Z', None) => {
                let terminal_capabilities = "\u{1b}[?6c";
                self.pending_messages_to_pty
                    .push(terminal_capabilities.as_bytes().to_vec());
            },
            (b'8', None) => {
                self.restore_cursor_position();
            },
            (b'8', Some(b'#')) => {
                let mut fill_character = EMPTY_TERMINAL_CHARACTER;
                fill_character.character = 'E';
                self.fill_viewport(fill_character);
            },
            _ => {
                if self.debug {
                    log::warn!("Unhandled esc_dispatch: {}->{:?}", byte, intermediates);
                }
            },
        }
    }

    fn terminated(&self) -> bool {
        !self.pending_forwarded_queries.is_empty()
    }
}

pub struct AlternateScreenState {
    lines_above: VecDeque<Row>,
    viewport: VecDeque<Row>,
    cursor: Cursor,
    sixel_grid: SixelGrid,
    kitty_grid: KittyGrid,
    supports_kitty_keyboard_protocol: bool,
}
impl AlternateScreenState {
    pub fn new(
        lines_above: VecDeque<Row>,
        viewport: VecDeque<Row>,
        cursor: Cursor,
        sixel_grid: SixelGrid,
        kitty_grid: KittyGrid,
        supports_kitty_keyboard_protocol: bool,
    ) -> Self {
        AlternateScreenState {
            lines_above,
            viewport,
            cursor,
            sixel_grid,
            kitty_grid,
            supports_kitty_keyboard_protocol,
        }
    }
    pub fn apply_contents_to(
        &mut self,
        lines_above: &mut VecDeque<Row>,
        viewport: &mut VecDeque<Row>,
        cursor: &mut Cursor,
        sixel_grid: &mut SixelGrid,
        kitty_grid: &mut KittyGrid,
        supports_kitty_keyboard_protocol: &mut bool,
    ) {
        std::mem::swap(&mut self.lines_above, lines_above);
        std::mem::swap(&mut self.viewport, viewport);
        std::mem::swap(&mut self.cursor, cursor);
        std::mem::swap(&mut self.sixel_grid, sixel_grid);
        std::mem::swap(&mut self.kitty_grid, kitty_grid);
        std::mem::swap(
            &mut self.supports_kitty_keyboard_protocol,
            supports_kitty_keyboard_protocol,
        );
    }
}

#[derive(Clone)]
pub struct Row {
    palettes: super::palette_spans::PaletteSpans,
    write_palette: Option<Rc<crate::agent_theme::SourcePalette>>,
    pub columns: VecDeque<TerminalCharacter>,
    pub is_canonical: bool,
    width: Option<usize>,
    /// Whether every character in the row occupies exactly one cell. When it does, a cell's
    /// column index *is* its index into `columns`, which lets the width-aware lookups skip their
    /// O(row) scans. Cached alongside `width` and invalidated with it - `None` means "unknown",
    /// never "false".
    all_single_width: Option<bool>,
    pub bg_color: Option<AnsiCode>,
    osc133_markers: Vec<Osc133Marker>,
    /// The row/column diacritics of the kitty unicode placeholder cells in this row, sparse and
    /// keyed by column. They cannot live on `TerminalCharacter`, which is pinned at 16 bytes, and
    /// they do not need to: a cell *is* a placeholder when its character is
    /// [`kitty_graphics::PLACEHOLDER`], and its image and placement ids come from its foreground
    /// and underline colours, which the cell already carries. Only the marks are extra, and only
    /// for the cells that spell them out - a cell with no marks inherits from the one to its left.
    ///
    /// Maintained exactly like `osc133_markers`, alongside it in every mutation below.
    placeholder_marks: Vec<PlaceholderMark>,
}

/// One decoded unicode placeholder cell, carried left to right so the next cell can inherit from
/// it. The colours ride along because inheritance is only allowed between cells that share them.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ResolvedPlaceholderCell {
    image_id: u32,
    placement_id: u32,
    row_in_image: u16,
    column_in_image: u16,
    id_most_significant_byte: u16,
    foreground: Option<AnsiCode>,
    underline_colour: Option<AnsiCode>,
}

/// A maximal run of adjacent placeholder cells showing consecutive columns of one row of one
/// virtual placement. Each becomes a single [`KittyPlacement`].
#[derive(Clone, Copy, Debug, PartialEq)]
struct KittyPlaceholderRun {
    image_id: u32,
    placement_id: u32,
    internal_id: crate::panes::kitty_graphics::InternalImageId,
    variant_key: super::kitty_graphics::store::ScaledImageKey,
    fitted_px: (usize, usize),
    z_index: i32,
    row_in_image: u16,
    column_in_image: u16,
    screen_row: usize,
    screen_column: usize,
    span: usize,
}

/// The image or placement id a placeholder cell carries in one of its colours.
///
/// Spec, "Unicode placeholders": "the image ID is encoded in the foreground color of the cell",
/// as the low byte of a 256-colour index or the low three bytes of a 24-bit colour. A cell with no
/// colour set names no image, which is what makes an ordinary `U+10EEEE` in text inert.
fn kitty_image_id_from_colour(colour: Option<AnsiCode>) -> Option<u32> {
    match colour? {
        AnsiCode::ColorIndex(index) => Some(index as u32),
        AnsiCode::RgbCode((r, g, b)) => Some(((r as u32) << 16) | ((g as u32) << 8) | b as u32),
        AnsiCode::NamedColor(named) => Some(named as u32),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Osc133MarkerKind {
    Prompt,
    Input,
    Output,
    /// `OSC 133 ; D ; <exit code>` - the exit code is not tracked, nothing reads it yet.
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Osc133Marker {
    column: usize,
    kind: Osc133MarkerKind,
}

impl Debug for Row {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for character in &self.columns {
            write!(f, "{:?}", character)?;
        }
        Ok(())
    }
}

impl Row {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Row {
            palettes: Default::default(),
            write_palette: None,
            columns: VecDeque::new(),
            is_canonical: false,
            width: None,
            all_single_width: None,
            bg_color: None,
            osc133_markers: vec![],
            placeholder_marks: vec![],
        }
    }
    pub fn from_columns(columns: VecDeque<TerminalCharacter>) -> Self {
        Row {
            palettes: Default::default(),
            write_palette: None,
            columns,
            is_canonical: false,
            width: None,
            all_single_width: None,
            bg_color: None,
            osc133_markers: vec![],
            placeholder_marks: vec![],
        }
    }
    pub fn from_rows(mut rows: Vec<Row>) -> Self {
        if rows.is_empty() {
            Row::new()
        } else {
            let mut first_row = rows.remove(0);
            for row in &mut rows {
                first_row.append(row);
            }
            first_row
        }
    }
    pub fn with_character(mut self, terminal_character: TerminalCharacter) -> Self {
        self.columns.push_back(terminal_character);
        self.invalidate_width_cache();
        self
    }
    pub fn canonical(mut self) -> Self {
        self.is_canonical = true;
        self
    }
    fn with_agent_theme(mut self, palette: Option<Rc<crate::agent_theme::SourcePalette>>) -> Self {
        self.palettes.set(0..usize::MAX, palette.as_ref());
        self.write_palette = palette;
        self
    }
    fn writing(&mut self, palette: &Rc<crate::agent_theme::SourcePalette>) -> &mut Self {
        self.write_palette = palette.has_snapshot().then(|| palette.clone());
        self
    }
    pub(crate) fn split_palette_chunks(&self, mut chunk: CharacterChunk) -> Vec<CharacterChunk> {
        if self.palettes.is_empty() {
            return vec![chunk];
        }
        let mut chunks: Vec<CharacterChunk> = Vec::new();
        let mut x = chunk.x;
        let characters = std::mem::take(&mut chunk.terminal_characters);
        for character in characters {
            let palette = self.palettes.at(x);
            let same = chunks
                .last()
                .is_some_and(|c| match (&c.agent_theme, palette) {
                    (None, None) => true,
                    (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                    _ => false,
                });
            if !same {
                let mut next = chunk.clone();
                next.x = x;
                next.terminal_characters.clear();
                next.agent_theme = palette.cloned();
                chunks.push(next);
            }
            chunks
                .last_mut()
                .unwrap()
                .terminal_characters
                .push(character.clone());
            x += character.width();
        }
        if chunks.is_empty() {
            chunks.push(chunk);
        }
        chunks
    }
    pub fn with_bg_color(mut self, bg_color: Option<AnsiCode>) -> Self {
        self.bg_color = bg_color;
        self
    }
    /// Drop the cached row metrics. Called from every mutation that can change how wide the row
    /// is, or where a given column sits inside `columns`.
    fn invalidate_width_cache(&mut self) {
        self.width = None;
        self.all_single_width = None;
    }
    pub fn width_cached(&mut self) -> usize {
        if let Some(width) = self.width {
            width
        } else {
            let mut width = 0;
            let mut all_single_width = true;
            for terminal_character in &self.columns {
                let character_width = terminal_character.width();
                width += character_width;
                all_single_width &= character_width == 1;
            }
            self.width = Some(width);
            self.all_single_width = Some(all_single_width);
            width
        }
    }
    /// `true` only when every character in the row is exactly one cell wide, so column index and
    /// `columns` index coincide. Computed with (and cached alongside) the row width; a `None`
    /// cache means "not known yet", so this recomputes rather than assuming the slow path.
    fn all_single_width(&mut self) -> bool {
        if let Some(all_single_width) = self.all_single_width {
            return all_single_width;
        }
        // Computed here rather than by deferring to `width_cached`, which returns early when the
        // width alone is already cached and so would leave this `None` forever.
        let all_single_width = self
            .columns
            .iter()
            .all(|terminal_character| terminal_character.width() == 1);
        self.all_single_width = Some(all_single_width);
        all_single_width
    }
    pub fn width(&self) -> usize {
        let mut width = 0;
        for terminal_character in &self.columns {
            width += terminal_character.width();
        }
        width
    }
    pub fn excess_width(&self) -> usize {
        let mut acc = 0;
        for terminal_character in &self.columns {
            if terminal_character.width() > 1 {
                acc += terminal_character.width() - 1;
            }
        }
        acc
    }
    pub fn excess_width_until(&self, x: usize) -> usize {
        let mut acc = 0;
        for terminal_character in self.columns.iter().take(x) {
            if terminal_character.width() > 1 {
                acc += terminal_character.width() - 1;
            }
        }
        acc
    }
    pub fn absolute_character_index(&self, x: usize) -> usize {
        // return x's width aware index
        let mut absolute_index = x;
        for (i, terminal_character) in self.columns.iter().enumerate().take(x) {
            if i == absolute_index {
                break;
            }
            if terminal_character.width() > 1 {
                absolute_index = absolute_index.saturating_sub(1);
            }
        }
        absolute_index
    }
    pub fn absolute_character_index_and_position_in_char(&self, x: usize) -> (usize, usize) {
        // returns x's width aware index as well as its position inside the wide char (eg. 1 if
        // it's in the middle of a 2-char wide character)
        let mut accumulated_width = 0;
        let mut absolute_index = x;
        let mut position_inside_character = 0;
        for (i, terminal_character) in self.columns.iter().enumerate() {
            accumulated_width += terminal_character.width();
            absolute_index = i;
            if accumulated_width > x {
                let character_start_position = accumulated_width - terminal_character.width();
                position_inside_character = x - character_start_position;
                break;
            }
        }
        (absolute_index, position_inside_character)
    }
    pub fn add_character_at(&mut self, terminal_character: TerminalCharacter, x: usize) {
        let width = self.width_cached();
        let (start, end) =
            if x < width && !(terminal_character.width() == 1 && self.all_single_width()) {
                let (index, inside) = self.absolute_character_index_and_position_in_char(x);
                let start = x - inside;
                let mut end = start + terminal_character.width().max(self.columns[index].width());
                if terminal_character.width() > self.columns[index].width() {
                    end = end.max(
                        start
                            + self.columns[index].width()
                            + self.columns.get(index + 1).map_or(0, |c| c.width()),
                    );
                }
                (start, end)
            } else {
                (x, x + terminal_character.width())
            };
        self.palettes.set(start..end, self.write_palette.as_ref());
        match self.width_cached().cmp(&x) {
            Ordering::Equal => {
                // this is unwrapped because this always happens after self.width_cached()
                *self.width.as_mut().unwrap() += terminal_character.width();
                if terminal_character.width() != 1 {
                    // keep the companion cache honest: the width stays accurate above, so nothing
                    // else will invalidate it and recompute this
                    self.all_single_width = Some(false);
                }
                // adding the character at the end of the current line
                self.columns.push_back(terminal_character);
            },
            Ordering::Less => {
                // adding the character after the end of the current line
                // we pad the line up to the character and then add it
                let width_offset = self.excess_width_until(x);
                let mut gap_fill = EMPTY_TERMINAL_CHARACTER;
                if let Some(bg_color) = self.bg_color {
                    gap_fill
                        .styles
                        .update(|styles| styles.background = Some(bg_color));
                }
                self.columns
                    .resize(x.saturating_sub(width_offset), gap_fill);
                self.columns.push_back(terminal_character);
                self.invalidate_width_cache();
            },
            Ordering::Greater => {
                // adding the character in the middle of the line
                // we replace the character at its position

                // Fast path: in a row of single-width characters the column index is already the
                // index into `columns`, and swapping one single-width character for another can
                // neither shift the following characters nor change the row width. That skips the
                // O(row) index scan below *and* keeps the width cache warm, which matters because
                // a full-line redraw walks this branch once per column.
                if terminal_character.width() == 1 && self.all_single_width() {
                    if self.columns[x].character != ' ' {
                        self.osc133_markers.retain(|marker| marker.column != x);
                    }
                    self.discard_placeholder_marks_in(x..x + 1);
                    self.columns[x] = terminal_character;
                    return;
                }

                let (absolute_x_index, position_inside_character) =
                    self.absolute_character_index_and_position_in_char(x);
                let character_width = terminal_character.width();
                let overwrite_start = x.saturating_sub(position_inside_character);
                let overwrite_end =
                    overwrite_start + character_width.max(self.columns[absolute_x_index].width());
                let replacing_blank = self.columns[absolute_x_index].character == ' ';
                self.discard_placeholder_marks_in(overwrite_start..overwrite_end);
                self.osc133_markers.retain(|marker| {
                    marker.column < overwrite_start
                        || marker.column >= overwrite_end
                        || (replacing_blank && marker.column == overwrite_start)
                });
                let replaced_character =
                    std::mem::replace(&mut self.columns[absolute_x_index], terminal_character);
                match character_width.cmp(&replaced_character.width()) {
                    Ordering::Greater => {
                        // the replaced character is narrower than the current character
                        // (eg. we added a wide emoji in place of an English character)
                        // we remove the character after it to make room
                        let position_to_remove = absolute_x_index + 1;
                        if let Some(removed) = self.columns.remove(position_to_remove) {
                            if removed.width() > 1 {
                                // the character we removed is a wide character itself, so we add
                                // padding
                                self.columns
                                    .insert(position_to_remove, EMPTY_TERMINAL_CHARACTER);
                            }
                        }
                    },
                    Ordering::Less => {
                        // the replaced character is wider than the current character
                        // (eg. we added an English character in place of a wide emoji)
                        // we must make sure to add padding either before the character we added
                        // or after it, depending on our position inside said removed wide character
                        // (`unicode_width` never yields more than 2 for a single `char`)
                        if position_inside_character > 0 {
                            self.columns
                                .insert(absolute_x_index, EMPTY_TERMINAL_CHARACTER);
                        } else {
                            self.columns
                                .insert(absolute_x_index + 1, EMPTY_TERMINAL_CHARACTER);
                        }
                    },
                    _ => {},
                }
                self.invalidate_width_cache();
            },
        }
    }
    pub fn insert_character_at(&mut self, terminal_character: TerminalCharacter, x: usize) {
        let character_width = terminal_character.width();
        for marker in &mut self.osc133_markers {
            if marker.column > x {
                marker.column += character_width;
            }
        }
        for mark in &mut self.placeholder_marks {
            if mark.column >= x {
                mark.column += character_width;
            }
        }
        let insert_position = self.absolute_character_index(x);
        let column = self
            .columns
            .iter()
            .take(insert_position)
            .map(|c| c.width())
            .sum::<usize>()
            + insert_position.saturating_sub(self.columns.len());
        self.palettes
            .insert(column, character_width, self.write_palette.as_ref());
        match self.columns.len().cmp(&insert_position) {
            Ordering::Equal => self.columns.push_back(terminal_character),
            Ordering::Less => {
                self.columns
                    .resize(insert_position, EMPTY_TERMINAL_CHARACTER);
                self.columns.push_back(terminal_character);
            },
            Ordering::Greater => {
                self.columns.insert(insert_position, terminal_character);
            },
        }
        self.invalidate_width_cache();
    }
    pub fn replace_character_at(&mut self, terminal_character: TerminalCharacter, x: usize) {
        let absolute_x_index = self.absolute_character_index(x);
        let start = self
            .columns
            .iter()
            .take(absolute_x_index)
            .map(|c| c.width())
            .sum::<usize>();
        if let Some(character) = self.columns.get_mut(absolute_x_index) {
            let terminal_character_width = terminal_character.width();
            let overwrite_end = start + character.width().max(terminal_character_width);
            self.palettes
                .set(start..overwrite_end, self.write_palette.as_ref());
            // A narrower replacement leaves the wide character's trailing cells behind as
            // padding. That padding keeps the replacement's styles so an erased wide
            // character does not end up half pane-background, half terminal-default.
            let mut padding_character = EMPTY_TERMINAL_CHARACTER;
            padding_character.styles = terminal_character.styles.clone();
            let character = std::mem::replace(character, terminal_character);
            self.osc133_markers
                .retain(|marker| marker.column < x || marker.column >= overwrite_end);
            self.discard_placeholder_marks_in(x..overwrite_end);
            let excess_width = character.width().saturating_sub(terminal_character_width);
            for _ in 0..excess_width {
                self.columns
                    .insert(absolute_x_index, padding_character.clone());
            }
        }
        self.invalidate_width_cache();
    }
    pub fn replace_columns(&mut self, columns: VecDeque<TerminalCharacter>) {
        self.palettes
            .set(0..usize::MAX, self.write_palette.as_ref());
        self.columns = columns;
        self.osc133_markers.clear();
        self.placeholder_marks.clear();
        self.invalidate_width_cache();
    }
    pub fn push(&mut self, terminal_character: TerminalCharacter) {
        let start = self.width();
        self.palettes.set(
            start..start + terminal_character.width(),
            self.write_palette.as_ref(),
        );
        self.columns.push_back(terminal_character);
        self.invalidate_width_cache();
    }
    pub fn truncate(&mut self, x: usize) {
        let width_offset = self.excess_width_until(x);
        let truncate_position = x.saturating_sub(width_offset);
        if truncate_position < self.columns.len() {
            self.columns.truncate(truncate_position);
        }
        self.palettes.set(self.width()..usize::MAX, None);
        self.osc133_markers.retain(|marker| marker.column <= x);
        self.placeholder_marks.retain(|mark| mark.column < x);
        self.invalidate_width_cache();
    }
    pub fn position_accounting_for_widechars(&self, x: usize) -> usize {
        let mut position = x;
        for (index, terminal_character) in self.columns.iter().enumerate() {
            if index == position {
                break;
            }
            if terminal_character.width() > 1 {
                position = position.saturating_sub(terminal_character.width().saturating_sub(1));
            }
        }
        position
    }
    pub fn replace_and_pad_end(
        &mut self,
        from: usize,
        to: usize,
        terminal_character: TerminalCharacter,
    ) {
        self.osc133_markers.retain(|marker| marker.column <= from);
        self.placeholder_marks.retain(|mark| mark.column < from);
        let from_position_accounting_for_widechars = self.position_accounting_for_widechars(from);
        let to_position_accounting_for_widechars = self.position_accounting_for_widechars(to);
        let replacement_length = to_position_accounting_for_widechars
            .saturating_sub(from_position_accounting_for_widechars);
        let start = self
            .columns
            .iter()
            .take(from_position_accounting_for_widechars)
            .map(|c| c.width())
            .sum();
        self.palettes
            .set(start..usize::MAX, self.write_palette.as_ref());
        let mut replace_with = VecDeque::from(vec![terminal_character; replacement_length]);
        self.columns
            .truncate(from_position_accounting_for_widechars);
        self.columns.append(&mut replace_with);
        self.invalidate_width_cache();
    }
    pub fn append(&mut self, to_append: &mut Row) {
        let column_offset = self.width();
        self.palettes.append(&to_append.palettes, column_offset);
        self.columns.append(&mut to_append.columns);
        self.osc133_markers
            .extend(to_append.osc133_markers.drain(..).map(|mut marker| {
                marker.column += column_offset;
                marker
            }));
        self.placeholder_marks
            .extend(to_append.placeholder_marks.drain(..).map(|mut mark| {
                mark.column += column_offset;
                mark
            }));
        self.invalidate_width_cache();
    }
    pub fn drain_until(&mut self, x: usize) -> VecDeque<TerminalCharacter> {
        let mut drained_part_len = 0;
        let mut split_pos = 0;
        for next_character in self.columns.iter() {
            // drained_part_len == 0 here is so that if the grid is resized
            // to a size of 1, we won't drop wide characters
            if drained_part_len + next_character.width() <= x || drained_part_len == 0 {
                drained_part_len += next_character.width();
                split_pos += 1
            } else {
                break;
            }
        }
        // Can't use split_off because it doesn't reduce capacity, causing OOM with long lines
        let drained_part = self.columns.drain(..split_pos).collect();
        self.palettes = self.palettes.slice(drained_part_len..usize::MAX);
        self.placeholder_marks.retain_mut(|mark| {
            if mark.column < drained_part_len {
                false
            } else {
                mark.column -= drained_part_len;
                true
            }
        });
        self.invalidate_width_cache();
        drained_part
    }
    pub fn replace_and_pad_beginning(&mut self, to: usize, terminal_character: TerminalCharacter) {
        let to_position_accounting_for_widechars = self.position_accounting_for_widechars(to);
        let width_of_current_character = self
            .columns
            .get(to_position_accounting_for_widechars)
            .map(|character| character.width())
            .unwrap_or(1);
        let replaced_end = to + width_of_current_character;
        self.palettes
            .set(0..replaced_end, self.write_palette.as_ref());
        self.osc133_markers
            .retain(|marker| marker.column >= replaced_end);
        self.placeholder_marks
            .retain(|mark| mark.column >= replaced_end);
        let mut replace_with =
            VecDeque::from(vec![terminal_character; to + width_of_current_character]);
        if to_position_accounting_for_widechars > self.columns.len() {
            self.columns.clear();
        } else if to_position_accounting_for_widechars >= self.columns.len() {
            drop(self.columns.drain(0..to_position_accounting_for_widechars));
        } else {
            drop(self.columns.drain(0..=to_position_accounting_for_widechars));
        }
        replace_with.append(&mut self.columns);
        self.invalidate_width_cache();
        self.columns = replace_with;
    }
    pub fn len(&self) -> usize {
        self.columns.len()
    }
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }
    pub fn delete_and_return_character(&mut self, x: usize) -> Option<TerminalCharacter> {
        let erase_position = self.absolute_character_index(x);
        if erase_position < self.columns.len() {
            self.invalidate_width_cache();
            let deleted = self.columns.remove(erase_position);
            if let Some(deleted) = &deleted {
                let start = self
                    .columns
                    .iter()
                    .take(erase_position)
                    .map(|c| c.width())
                    .sum::<usize>();
                self.palettes.delete(start..start + deleted.width());
                let end = x + deleted.width();
                self.placeholder_marks.retain_mut(|mark| {
                    if (x..end).contains(&mark.column) {
                        false
                    } else {
                        if mark.column >= end {
                            mark.column -= deleted.width();
                        }
                        true
                    }
                });
                self.osc133_markers.retain_mut(|marker| {
                    if (x..end).contains(&marker.column) {
                        false
                    } else {
                        if marker.column >= end {
                            marker.column -= deleted.width();
                        }
                        true
                    }
                });
            }
            deleted
        } else {
            None
        }
    }
    pub fn split_to_rows_of_length(&mut self, max_row_length: usize) -> Vec<Row> {
        let markers = std::mem::take(&mut self.osc133_markers);
        let marks = std::mem::take(&mut self.placeholder_marks);
        let mut parts: Vec<Row> = vec![];
        let mut current_part: VecDeque<TerminalCharacter> = VecDeque::new();
        let mut current_part_len = 0;
        for character in self.columns.drain(..) {
            if current_part_len + character.width() > max_row_length {
                parts.push(Row::from_columns(current_part).with_bg_color(self.bg_color));
                current_part = VecDeque::new();
                current_part_len = 0;
            }
            current_part_len += character.width();
            current_part.push_back(character);
        }
        if !current_part.is_empty() {
            parts.push(Row::from_columns(current_part).with_bg_color(self.bg_color))
        };
        if !parts.is_empty() && self.is_canonical {
            if let Some(part) = parts.get_mut(0) {
                part.is_canonical = true;
            }
        }
        if parts.is_empty() {
            parts.push(self.clone());
        }
        let mut offset = 0;
        let last = parts.len().saturating_sub(1);
        for (index, part) in parts.iter_mut().enumerate() {
            let end = if index == last {
                usize::MAX
            } else {
                offset + part.width()
            };
            part.palettes = self.palettes.slice(offset..end);
            offset += part.width();
        }
        for mut marker in markers {
            let mut column_offset = 0;
            for part_index in 0..parts.len() {
                let part_width = parts[part_index].width();
                if marker.column <= column_offset + part_width || part_index + 1 == parts.len() {
                    marker.column = marker.column.saturating_sub(column_offset).min(part_width);
                    parts[part_index].osc133_markers.push(marker);
                    break;
                }
                column_offset += part_width;
            }
        }
        // The marks travel with the cell they annotate, so unlike an OSC-133 marker they are
        // placed by which part actually holds that column rather than clamped into the last one.
        for mut mark in marks {
            let mut column_offset = 0;
            for part in parts.iter_mut() {
                let part_width = part.width();
                if mark.column < column_offset + part_width {
                    mark.column -= column_offset;
                    part.placeholder_marks.push(mark);
                    break;
                }
                column_offset += part_width;
            }
        }
        self.invalidate_width_cache();
        parts
    }
    /// Forget the placeholder marks of every cell in `columns`, because those cells are being
    /// overwritten. Marks whose cell is gone are harmless - resolution only consults a mark when a
    /// placeholder character is actually sitting at that column - but leaving them would let a
    /// stale row/column reach a *later* placeholder printed at the same spot.
    fn discard_placeholder_marks_in(&mut self, columns: std::ops::Range<usize>) {
        if self.placeholder_marks.is_empty() {
            return;
        }
        self.placeholder_marks
            .retain(|mark| !columns.contains(&mark.column));
    }
    /// Start a placeholder cell at `column`, discarding whatever was recorded there before. The
    /// marks themselves arrive afterwards, as zero-width characters, through
    /// [`Row::push_placeholder_mark_value`].
    fn begin_placeholder_marks(&mut self, column: usize) {
        self.discard_placeholder_marks_in(column..column + 1);
        self.placeholder_marks.push(PlaceholderMark::new(column));
    }
    /// Record one more diacritic on the placeholder cell at `column`. Returns `false` when that
    /// cell never began one, which is how a combining mark that follows something other than a
    /// placeholder falls through to being dropped as before.
    fn push_placeholder_mark_value(&mut self, column: usize, value: u16) -> bool {
        match self
            .placeholder_marks
            .iter_mut()
            .find(|mark| mark.column == column)
        {
            Some(mark) => {
                mark.push_value(value);
                true
            },
            None => false,
        }
    }
    /// The character occupying screen column `x`, accounting for the wide characters before it.
    fn character_at(&self, x: usize) -> Option<&TerminalCharacter> {
        self.columns.get(self.absolute_character_index(x))
    }
    fn placeholder_mark_at(&self, column: usize) -> Option<PlaceholderMark> {
        self.placeholder_marks
            .iter()
            .find(|mark| mark.column == column)
            .copied()
    }
    fn add_osc133_marker(&mut self, column: usize, kind: Osc133MarkerKind) {
        self.osc133_markers.push(Osc133Marker { column, kind });
        self.osc133_markers.sort_by_key(|marker| marker.column);
    }
    pub fn last_index_in_line(&self) -> usize {
        self.columns.len()
    }
    pub fn word_indices_around_character_index(
        &self,
        index: usize,
        word_separators: &str,
    ) -> Option<(usize, usize)> {
        let absolute_character_index = self.absolute_character_index(index);
        let character_at_index = self.columns.get(absolute_character_index)?;
        if is_selection_boundary_character(character_at_index.character, word_separators) {
            return Some((index, index + 1));
        }
        let mut end_position = self
            .columns
            .iter()
            .enumerate()
            .skip(absolute_character_index)
            .find_map(|(i, t_c)| {
                if is_selection_boundary_character(t_c.character, word_separators) {
                    Some(i + self.excess_width_until(i))
                } else {
                    None
                }
            })
            .unwrap_or_else(|| self.columns.len() + self.excess_width());
        let start_position = self
            .columns
            .iter()
            .enumerate()
            .take(absolute_character_index)
            .rev()
            .find_map(|(i, t_c)| {
                if is_selection_boundary_character(t_c.character, word_separators) {
                    Some(i + 1 + self.excess_width_until(i))
                } else {
                    None
                }
            })
            .unwrap_or(0);
        if start_position == end_position {
            // so that if this is only one character, it'll still be marked
            end_position += 1;
        }
        Some((start_position, end_position))
    }
    pub fn word_start_index_of_last_character(&self, word_separators: &str) -> usize {
        self.columns
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, t_c)| {
                if is_selection_boundary_character(t_c.character, word_separators) {
                    Some(self.absolute_character_index(i + 1))
                } else {
                    None
                }
            })
            .unwrap_or(0)
    }
    pub fn word_end_index_of_first_character(&self, word_separators: &str) -> usize {
        self.columns
            .iter()
            .enumerate()
            .find_map(|(i, t_c)| {
                if is_selection_boundary_character(t_c.character, word_separators) {
                    Some(self.absolute_character_index(i))
                } else {
                    None
                }
            })
            .unwrap_or(self.columns.len())
    }
}

fn is_selection_boundary_character(character: char, word_separators: &str) -> bool {
    character.is_ascii_whitespace() || word_separators.contains(character)
}

#[cfg(test)]
#[path = "./unit/grid_tests/mod.rs"]
mod grid_tests;

#[cfg(test)]
#[path = "./unit/row_tests.rs"]
mod row_tests;

#[cfg(test)]
mod osc7_parser_tests {
    use super::parse_osc7_path;
    use std::path::PathBuf;

    #[test]
    fn osc7_parser_extracts_path() {
        let raw = b"file://host/home/user/foo";
        assert_eq!(parse_osc7_path(raw), Some(PathBuf::from("/home/user/foo")));
    }

    #[test]
    fn osc7_parser_decodes_percent_encoded_path() {
        let raw = b"file://localhost/tmp/with%20space";
        assert_eq!(parse_osc7_path(raw), Some(PathBuf::from("/tmp/with space")));
    }

    #[test]
    fn osc7_parser_rejects_missing_scheme() {
        assert_eq!(parse_osc7_path(b"/home/user"), None);
    }

    #[test]
    fn osc7_parser_rejects_missing_path() {
        assert_eq!(parse_osc7_path(b"file://host"), None);
    }

    #[test]
    fn osc7_parser_rejects_truncated_percent_escape() {
        assert_eq!(parse_osc7_path(b"file://host/foo%2"), None);
    }

    #[test]
    fn osc7_parser_rejects_non_utf8_input() {
        let raw = &[0xFFu8, 0xFE, 0xFD];
        assert_eq!(parse_osc7_path(raw), None);
    }
}
