mod csi;
mod highlights;
mod mouse;
mod osc;
mod row;
mod selection;
mod vt;

pub use highlights::CompiledHighlight;
#[cfg(test)]
use row::Osc133Marker;
use row::Osc133MarkerKind;
pub use row::Row;

use super::kitty_graphics::diacritic_value;
use super::kitty_graphics::format_kitty_error;
use super::kitty_graphics::format_kitty_reply;
use super::kitty_graphics::KittyAction;
use super::kitty_graphics::KittyCommand;
use super::kitty_graphics::KittyCommandParser;
use super::kitty_graphics::KittyError;
use super::kitty_graphics::KittyErrorCode;
use super::kitty_graphics::KittyGrid;
use super::kitty_graphics::KittyHostSupport;
use super::kitty_graphics::KittyImageChunk;
use super::kitty_graphics::KittyImageStore;
use super::kitty_graphics::KittyPlacement;
use super::kitty_graphics::KittyReplyData;
use super::kitty_graphics::KittyRowsBelowTheViewport;
use super::kitty_graphics::KittyVerticalAnchor;
use super::kitty_graphics::PLACEHOLDER;
use super::sixel::PixelRect;
use super::sixel::SixelGrid;
use super::sixel::SixelImageStore;
use base64::alphabet::STANDARD as BASE64_STANDARD_ALPHABET;
use base64::engine::general_purpose::GeneralPurpose;
use base64::engine::general_purpose::GeneralPurposeConfig;
use base64::engine::DecodePaddingMode;
use std::borrow::Cow;
use std::cell::RefCell;

use std::collections::HashMap;
use std::collections::HashSet;
use std::rc::Rc;

use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::PluginId;

use yinz_types::data::HighlightLayer;
use yinz_types::style::HostTerminalThemeMode;
use yinz_types::style::Style;

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::str;

use yinz_types::pane_size::SizeInPixels;
use yinz_types::position::Position;
use yinz_types::style::Palette;
use yinz_types::style::Styling;

use yinz_ipc::nested_session::NestedSessionMessage;
use yinz_runtime::consts::DEFAULT_SCROLL_BUFFER_SIZE;
use yinz_runtime::consts::SCROLL_BUFFER_SIZE;
use yinz_types::data::PaletteColor;
use yinz_types::input::options::DEFAULT_WORD_SEPARATORS;

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
use yinz_host_protocol::notifications::action_wants_report;
use yinz_host_protocol::notifications::Osc99PayloadType;
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

use crate::output::CharacterChunk;
use crate::output::HighlightSelection;
use crate::output::OutputBuffer;
use crate::output::SixelImageChunk;
use crate::panes::alacritty_functions::xparse_color;
use crate::panes::hyperlink_tracker::HyperlinkTracker;
use crate::panes::link_handler::LinkHandler;
use crate::panes::search::SearchResult;
use crate::panes::terminal_character::AnsiCode;
use crate::panes::terminal_character::CharsetIndex;
use crate::panes::terminal_character::Cursor;
use crate::panes::terminal_character::CursorShape;
use crate::panes::terminal_character::RcCharacterStyles;
use crate::panes::terminal_character::StandardCharset;
use crate::panes::terminal_character::TerminalCharacter;
use crate::panes::terminal_character::EMPTY_TERMINAL_CHARACTER;
use crate::panes::Selection;

use yinz_types::data::PaneContents;

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
    ($lines:expr_2021) => {{
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
    ($lines:expr_2021) => {{
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

/// One rendered frame of a grid: its character chunks, the raw VTE output to pass through if
/// there is any, and its sixel and kitty image chunks.
pub type RenderedGridFrame = (
    Vec<CharacterChunk>,
    Option<String>,
    Vec<SixelImageChunk>,
    Vec<KittyImageChunk>,
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
    fn cursor_canonical_line_index(&self, cursor: &Cursor) -> usize {
        let mut cursor_canonical_line_index = 0;
        let mut canonical_lines_traversed = 0;
        for (i, line) in self.viewport.iter().enumerate() {
            // A leading continuation becomes a logical line when rejoined with scrollback.
            if i == 0 || line.is_canonical {
                cursor_canonical_line_index = canonical_lines_traversed;
                canonical_lines_traversed += 1;
            }
            if i == cursor.y {
                break;
            }
        }
        cursor_canonical_line_index
    }
    fn cursor_index_in_canonical_line(&self, cursor: &Cursor) -> usize {
        let mut cursor_canonical_line_index = 0;
        let mut cursor_index_in_canonical_line = 0;
        let mut scrollback_prefix_width = self.lines_above.back().map_or(0, Row::width);
        for (i, line) in self.viewport.iter().enumerate() {
            if line.is_canonical {
                cursor_canonical_line_index = i;
                scrollback_prefix_width = 0;
            }
            if i == cursor.y {
                let line_wraps = cursor.y.saturating_sub(cursor_canonical_line_index);
                cursor_index_in_canonical_line =
                    scrollback_prefix_width + (line_wraps * self.width) + cursor.x;
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
            if self.lines_above.is_empty()
                && let Some(first_row) = self.viewport.front_mut()
            {
                first_row.writing(&self.agent_theme);
                first_row.is_canonical = true;
            }
            let cursor_canonical_line_index = self.cursor_canonical_line_index(&self.cursor);
            let saved_cursor_canonical_line_index = self
                .saved_cursor_position
                .as_ref()
                .map(|cursor| self.cursor_canonical_line_index(cursor));
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
            let mut saved_cursor_y_coordinates = saved_cursor_canonical_line_index
                .zip(saved_cursor_index_in_canonical_line)
                .map(|(line_index, column_index)| {
                    self.canonical_line_y_coordinates(line_index) + column_index / new_columns
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
            if let Some(command_output_flash) = self.command_output_flash
                && command_output_flash.contains_row(character_chunk.y)
            {
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
        if let Some(rows) = self.hyperlink_tracker.update(
            '\n',
            &self.cursor,
            self.width,
            &mut self.viewport,
            &mut self.lines_above,
            &mut self.link_handler.borrow_mut(),
        ) {
            self.output_buffer.update_lines(*rows.start(), *rows.end());
        }
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
        if let Some(rows) = self.hyperlink_tracker.update(
            terminal_character.character,
            &self.cursor,
            self.width,
            &mut self.viewport,
            &mut self.lines_above,
            &mut self.link_handler.borrow_mut(),
        ) {
            self.output_buffer.update_lines(*rows.start(), *rows.end());
        }
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
        if let Some(row) = self.viewport.get_mut(self.cursor.y) {
            row.writing(&self.agent_theme);
            // A prefix erase leaves the tail's padding colour intact unless it reaches the edge.
            if self.cursor.x >= self.width.saturating_sub(1) {
                row.bg_color = replace_with
                    .styles
                    .background
                    .filter(|bg| *bg != AnsiCode::Reset);
            }
            row.replace_and_pad_beginning(self.cursor.x, replace_with);
        }
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
            if let Some(bg_color) = current_row.bg_color
                && matches!(
                    effective_pad.styles.background,
                    Some(AnsiCode::Reset) | None
                )
            {
                effective_pad
                    .styles
                    .update(|styles| styles.background = Some(bg_color));
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
        self.cursor.x = std::cmp::min(self.width.saturating_sub(1), x);
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
            self.cursor.y = std::cmp::min(self.cursor.y + count, self.height.saturating_sub(1));
        }
        self.pad_lines_until(self.cursor.y, pad_character);
    }
    pub fn move_cursor_back(&mut self, count: usize) {
        if self.cursor.x == self.width {
            // on the rightmost screen edge, backspace skips one character
            self.cursor.x = self.cursor.x.saturating_sub(1);
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
        self.cursor.y = std::cmp::min(self.height.saturating_sub(1), line);
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
        self.viewport
            .resize_with(self.height, || Row::new().canonical());
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
        if let Some(character_cell_size) = { *self.character_cell_size.borrow() } {
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
        match *self.character_cell_size.borrow() {
            Some(character_cell_size) => {
                let line_count_in_scrollback = self.lines_above.len();
                let y_coordinates =
                    (line_count_in_scrollback + self.cursor.y) * character_cell_size.height;
                let x_coordinates = self.cursor.x * character_cell_size.width;
                Some((x_coordinates, y_coordinates))
            },
            _ => None,
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
                        if let Some((cols, rows)) = dest_cells
                            && !command.suppress_cursor_movement
                        {
                            self.advance_cursor_after_kitty_placement(cols as usize, rows as usize);
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
        if palette.ptr_eq(&Rc::downgrade(&self.agent_theme))
            && let Some(rgb) = crate::agent_theme::terminal_color_reply(code, reply)
        {
            self.observe_agent_terminal_color(code, rgb);
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
            if let Some(max) = max_scrollback_lines
                && max > 0
                && lines_above_viewport.len() > max
            {
                let start = lines_above_viewport.len() - max;
                lines_above_viewport = lines_above_viewport.split_off(start);
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
            if let Some(max) = max_scrollback_lines
                && max > 0
                && lines_above_viewport.len() > max
            {
                let start = lines_above_viewport.len() - max;
                lines_above_viewport = lines_above_viewport.split_off(start);
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

#[cfg(test)]
#[path = "./unit/grid_tests/mod.rs"]
mod grid_tests;

#[cfg(test)]
#[path = "./unit/row_tests.rs"]
mod row_tests;

#[cfg(test)]
#[path = "unit/osc7_parser_tests.rs"]
mod osc7_parser_tests;
