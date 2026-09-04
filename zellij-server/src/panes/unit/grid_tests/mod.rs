use super::super::Grid;
use super::MAX_DCS_PAYLOAD_BYTES;
use crate::panes::grid::SixelImageStore;
use crate::panes::kitty_graphics::KittyImageStore;
use crate::panes::link_handler::LinkHandler;
use base64::engine::general_purpose::STANDARD as BASE64_ENCODER;
use base64::engine::Engine as _;
use insta::assert_snapshot;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use vte;
use zellij_utils::consts::SCROLL_BUFFER_SIZE;
use zellij_utils::{
    data::{Palette, Style},
    input::options::DEFAULT_WORD_SEPARATORS,
    pane_size::SizeInPixels,
    position::Position,
};

use std::fmt::Write;

fn read_fixture(fixture_name: &str) -> Vec<u8> {
    let mut path_to_file = std::path::PathBuf::new();
    path_to_file.push("../src");
    path_to_file.push("tests");
    path_to_file.push("fixtures");
    path_to_file.push(fixture_name);
    std::fs::read(path_to_file)
        .unwrap_or_else(|_| panic!("could not read fixture {:?}", fixture_name))
}

fn create_grid_with_content(content: &str) -> Grid {
    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let mut grid = Grid::new(
        20,
        80,
        Rc::new(RefCell::new(Palette::default())),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        false,
        true,
        true,
        true,
        false,
    );
    vte_parser.advance(&mut grid, content.as_bytes());
    grid
}

fn select_with_click_count(grid: &mut Grid, position: Position, click_count: usize) -> String {
    for _ in 0..click_count {
        grid.start_selection(&position);
    }
    grid.end_selection(&position);
    grid.get_selected_text().unwrap()
}

fn viewport_position_of(grid: &Grid, needle: &str) -> Position {
    grid.viewport
        .iter()
        .enumerate()
        .find_map(|(line, row)| {
            let text: String = row.columns.iter().map(|c| c.character).collect();
            text.find(needle)
                .map(|column| Position::new(line as i32, column as u16))
        })
        .unwrap_or_else(|| panic!("{needle:?} not found in viewport"))
}

// =====================================================================
// Plugin Highlight Engine Tests
// =====================================================================

use crate::panes::grid::MouseTracking;
use crate::panes::terminal_character::AnsiCode;
use std::collections::BTreeMap;
use zellij_utils::data::{Direction, HighlightLayer, HighlightStyle, RegexHighlight};

#[test]
fn keyboard_selection_remembers_copies_until_select_mode_finishes() {
    let mut grid = create_grid_with_content("abcdef");

    grid.start_text_selection(Position::new(0, 0));
    for _ in 0..3 {
        grid.move_text_selection(Direction::Right);
    }
    assert_eq!(grid.get_selected_text().as_deref(), Some("abc"));

    grid.remember_copied_text_selection();
    grid.move_text_selection(Direction::Right);
    assert_eq!(grid.get_selected_text().as_deref(), Some("abcd"));
    grid.remember_copied_text_selection();
    grid.move_text_selection(Direction::Right);
    assert_eq!(grid.get_selected_text().as_deref(), Some("abcde"));

    grid.finish_text_selection();
    assert_eq!(grid.get_selected_text().as_deref(), Some("abcd"));
    assert!(!grid.selection.is_active());

    grid.start_text_selection(Position::new(0, 0));
    grid.move_text_selection(Direction::Right);
    grid.finish_text_selection();
    assert_eq!(grid.get_selected_text(), None);
}

#[test]
fn keyboard_selection_scrolls_past_both_viewport_boundaries() {
    let mut grid = create_grid_with_scrollback();
    let lines_above = grid.lines_above.len();

    grid.start_text_selection(Position::new(0, 1));
    grid.move_text_selection(Direction::Up);
    assert_eq!(grid.lines_above.len(), lines_above - 1);
    assert_eq!(grid.selection.start, Position::new(1, 1));
    assert_eq!(grid.selection.end, Position::new(0, 1));
    assert!(grid.selection.contains(0, 1));

    grid.finish_text_selection();
    grid.start_text_selection(Position::new(grid.height.saturating_sub(1) as i32, 1));
    grid.move_text_selection(Direction::Down);
    assert_eq!(grid.lines_above.len(), lines_above);
    assert_eq!(
        grid.selection.start,
        Position::new(grid.height.saturating_sub(2) as i32, 1)
    );
    assert_eq!(
        grid.selection.end,
        Position::new(grid.height.saturating_sub(1) as i32, 1)
    );
    assert!(grid.selection.contains(grid.height.saturating_sub(1), 0));
}

#[test]
fn keyboard_selection_preserves_the_cursor_row_at_the_right_margin() {
    let mut grid = create_grid_with_content(&format!("short\r\n{}", "x".repeat(80)));

    assert_eq!(grid.cursor_coordinates(), None);
    let (column, line) = grid.selection_cursor_coordinates().unwrap();
    assert_eq!((column, line), (80, 1));

    grid.start_text_selection(Position::new(line as i32, column as u16));
    grid.move_text_selection(Direction::Left);
    assert_eq!(grid.get_selected_text().as_deref(), Some("x"));
}

fn create_highlight(
    pattern: &str,
    on_hover: bool,
    bold: bool,
    italic: bool,
    underline: bool,
    layer: HighlightLayer,
) -> RegexHighlight {
    RegexHighlight {
        pattern: pattern.to_string(),
        style: HighlightStyle::Emphasis0,
        layer,
        context: BTreeMap::new(),
        on_hover,
        bold,
        italic,
        underline,
        tooltip_text: None,
    }
}

fn create_grid_with_scrollback() -> Grid {
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let mut grid = Grid::new(
        5,
        40,
        Rc::new(RefCell::new(Palette::default())),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        false,
        true,
        true,
        true,
        false,
    );
    let mut parser = vte::Parser::new();
    for i in 0..25 {
        let line = format!("scrollback line {}\r\n", i);
        parser.advance(&mut grid, line.as_bytes());
    }
    grid
}

/// What a plugin reading a pane needs in order to tell "the process wrote something" from
/// "the user is looking at what it wrote earlier".
#[test]
fn pane_contents_says_whether_the_viewport_is_scrolled_back() {
    let mut grid = create_grid_with_scrollback();
    assert!(
        !grid.pane_contents(false, None).is_scrolled,
        "sitting on the live tail"
    );

    grid.move_viewport_up(3);
    assert!(grid.pane_contents(false, None).is_scrolled);
    assert!(
        grid.pane_contents_with_ansi(false, None).is_scrolled,
        "and the same either way it is read"
    );

    grid.reset_viewport();
    assert!(!grid.pane_contents(false, None).is_scrolled);
}

// =====================================================================
// pane_contents_with_ansi Tests
// =====================================================================

fn create_grid_with_colored_scrollback() -> Grid {
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let mut grid = Grid::new(
        5,
        40,
        Rc::new(RefCell::new(Palette::default())),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        false,
        true,
        true,
        true,
        false,
    );
    let mut parser = vte::Parser::new();
    for i in 0..25 {
        let line = format!("\x1b[31mred line {}\x1b[0m\r\n", i);
        parser.advance(&mut grid, line.as_bytes());
    }
    grid
}

fn row_text(row: &super::super::Row) -> String {
    row.columns
        .iter()
        .map(|c| c.character)
        .collect::<String>()
        .trim_end()
        .to_string()
}

fn scrollback_texts(grid: &Grid) -> Vec<String> {
    grid.lines_above.iter().map(row_text).collect()
}

fn viewport_texts(grid: &Grid) -> Vec<String> {
    grid.viewport.iter().map(row_text).collect()
}

fn create_grid_with_size_and_raw(rows: usize, cols: usize, content: &[u8]) -> Grid {
    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let mut grid = Grid::new(
        rows,
        cols,
        Rc::new(RefCell::new(Palette::default())),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        false,
        true,
        true,
        true,
        false,
    );
    vte_parser.advance(&mut grid, content);
    grid
}

fn feed_bytes(grid: &mut Grid, bytes: &[u8]) {
    let mut vte_parser = vte::Parser::new();
    vte_parser.advance(grid, bytes);
}

// All tests below use a 10-row, 40-col grid with scroll region 1;8
// (0-based: rows 0-7), leaving rows 8-9 outside the region.
const PARTIAL_SR: &[u8] = b"\x1b[1;8r";
const FILL_8_LINES: &[u8] = b"AAA\r\nBBB\r\nCCC\r\nDDD\r\nEEE\r\nFFF\r\nGGG\r\nHHH";

fn new_grid_for_forwarding_test() -> Grid {
    Grid::new(
        10,
        20,
        Rc::new(RefCell::new(Palette::default())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(Some(SizeInPixels {
            width: 8,
            height: 16,
        }))),
        Rc::new(RefCell::new(SixelImageStore::default())),
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        false,
        true,
        true,
        true,
        false,
    )
}

fn new_grid_for_nested_frames() -> Grid {
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    Grid::new(
        41,
        110,
        Rc::new(RefCell::new(Palette::default())),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        false,
        true,
        true,
        true,
        false,
    )
}

fn nested_announce_message() -> zellij_utils::nested_session::NestedSessionMessage {
    zellij_utils::nested_session::NestedSessionMessage::Announce {
        session_name: "guest-session".to_owned(),
        capabilities: vec![zellij_utils::nested_session::NestedSessionCapability::NestedControl],
    }
}

fn serialize_text_bytes(text: &str) -> String {
    text.as_bytes()
        .iter()
        .map(|byte| byte.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn render_ui_component(component_name: &str, serialized_params: &str) -> Grid {
    let mut vte_parser = vte::Parser::new();
    let mut grid = new_grid_for_nested_frames();
    let dcs = format!("\u{1b}Pz{};{}\u{1b}\\", component_name, serialized_params);
    vte_parser.advance(&mut grid, dcs.as_bytes());
    grid
}

fn styled_cells_of(
    grid: &Grid,
    needle: char,
) -> Vec<crate::panes::terminal_character::CharacterStyles> {
    let mut styles = vec![];
    for line in grid.as_character_lines() {
        for terminal_character in line {
            if terminal_character.character == needle {
                styles.push(*terminal_character.styles);
            }
        }
    }
    styles
}

use crate::panes::kitty_graphics::{InterceptorResult, KittyApcInterceptor, KittyHostSupport};
use crate::panes::sixel::PixelRect;

const KITTY_PNG_2X2: [u8; 75] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x06, 0x00, 0x00, 0x00, 0x72, 0xb6, 0x0d,
    0x24, 0x00, 0x00, 0x00, 0x12, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x0c, 0x81, 0x34, 0x18, 0x00, 0x00, 0x49, 0xc8, 0x09, 0xf7, 0x03, 0xd9, 0x64, 0xf1, 0x00,
    0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

fn new_kitty_grid(rows: usize, columns: usize) -> (Grid, Rc<RefCell<KittyImageStore>>) {
    new_kitty_grid_with_store(rows, columns, KittyImageStore::default())
}

fn new_kitty_grid_with_store(
    rows: usize,
    columns: usize,
    store: KittyImageStore,
) -> (Grid, Rc<RefCell<KittyImageStore>>) {
    let kitty_image_store = Rc::new(RefCell::new(store));
    let grid = Grid::new(
        rows,
        columns,
        Rc::new(RefCell::new(Palette::default())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(Some(SizeInPixels {
            width: 10,
            height: 20,
        }))),
        Rc::new(RefCell::new(SixelImageStore::default())),
        kitty_image_store.clone(),
        Style::default(),
        false,
        true,
        true,
        true,
        false,
    );
    (grid, kitty_image_store)
}

fn feed_kitty_bytes(
    grid: &mut Grid,
    vte_parser: &mut vte::Parser,
    interceptor: &mut KittyApcInterceptor,
    bytes: &[u8],
) {
    for byte in bytes {
        match interceptor.advance(*byte) {
            InterceptorResult::Forward(fwd) => {
                vte_parser.advance(grid, fwd.as_slice());
            },
            InterceptorResult::Swallow => {},
            InterceptorResult::Captured(cmd) => {
                grid.handle_kitty_apc(&cmd);
            },
        }
    }
}

fn kitty_apc(control: &str, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"\x1b_G");
    out.extend_from_slice(control.as_bytes());
    if !payload.is_empty() {
        out.push(b';');
        out.extend_from_slice(BASE64_ENCODER.encode(payload).as_bytes());
    }
    out.extend_from_slice(b"\x1b\\");
    out
}

fn rgb_raster(width: usize, height: usize) -> Vec<u8> {
    (0..width * height * 3).map(|i| (i % 251) as u8).collect()
}

fn rgba_raster(width: usize, height: usize) -> Vec<u8> {
    (0..width * height * 4).map(|i| (i % 251) as u8).collect()
}

fn kitty_placement_id_set(grid: &mut Grid) -> Vec<(u32, u32)> {
    let mut ids: Vec<(u32, u32)> = grid
        .kitty_placements()
        .iter()
        .map(|placement| (placement.image_id, placement.placement_id))
        .collect();
    ids.sort();
    ids
}

fn build_three_kitty_placements(
    grid: &mut Grid,
    vte_parser: &mut vte::Parser,
    interceptor: &mut KittyApcInterceptor,
) {
    feed_kitty_bytes(
        grid,
        vte_parser,
        interceptor,
        &kitty_apc("a=T,f=24,s=20,v=40,i=1,C=1", &rgb_raster(20, 40)),
    );
    feed_kitty_bytes(grid, vte_parser, interceptor, b"\x1b[6;11H");
    feed_kitty_bytes(
        grid,
        vte_parser,
        interceptor,
        &kitty_apc("a=T,f=24,s=20,v=40,i=2,z=1,C=1", &rgb_raster(20, 40)),
    );
    feed_kitty_bytes(grid, vte_parser, interceptor, b"\x1b[11;21H");
    feed_kitty_bytes(
        grid,
        vte_parser,
        interceptor,
        &kitty_apc("a=T,f=32,s=10,v=20,i=5,z=-1,C=1", &rgba_raster(10, 20)),
    );
    assert_eq!(grid.kitty_placement_count(), 3);
}

use crate::panes::kitty_graphics::test_support::{self, KittyReply};

/// A terminal grid wired up the way `TerminalPane` wires one, so a conformance test can send
/// graphics commands and read back the replies the application would see.
///
/// Cells are 10x20 pixels, matching [`new_kitty_grid`].
pub struct KittySession {
    pub grid: Grid,
    pub store: Rc<RefCell<KittyImageStore>>,
    vte_parser: vte::Parser,
    interceptor: KittyApcInterceptor,
}

#[allow(dead_code)]
impl KittySession {
    pub fn new(rows: usize, columns: usize) -> Self {
        Self::from_parts(new_kitty_grid(rows, columns))
    }

    /// A session whose image store is limited to `quota_bytes`, for exercising eviction.
    pub fn with_quota(rows: usize, columns: usize, quota_bytes: usize) -> Self {
        Self::from_parts(new_kitty_grid_with_store(
            rows,
            columns,
            KittyImageStore::with_quota(quota_bytes),
        ))
    }

    fn from_parts((grid, store): (Grid, Rc<RefCell<KittyImageStore>>)) -> Self {
        KittySession {
            grid,
            store,
            vte_parser: vte::Parser::new(),
            interceptor: KittyApcInterceptor::new(),
        }
    }

    /// Send a graphics command, returning the replies it produced.
    pub fn send(&mut self, control: &str, payload: &[u8]) -> Vec<KittyReply> {
        test_support::send_kitty(
            &mut self.grid,
            &mut self.vte_parser,
            &mut self.interceptor,
            control,
            payload,
        )
    }

    /// Send a graphics command whose payload is already base64-encoded, for chunked uploads that
    /// have to split the encoded stream rather than the raw one.
    pub fn send_raw(&mut self, control: &str, payload_b64: &str) -> Vec<KittyReply> {
        test_support::send_kitty_raw(
            &mut self.grid,
            &mut self.vte_parser,
            &mut self.interceptor,
            control,
            payload_b64,
        )
    }

    /// Send a graphics command that should produce exactly one reply.
    pub fn reply(&mut self, control: &str, payload: &[u8]) -> KittyReply {
        self.send(control, payload)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("expected a reply to {:?}", control))
    }

    /// Send a graphics command expected to succeed, returning the image id reported back.
    pub fn ok(&mut self, control: &str, payload: &[u8]) -> Option<u32> {
        let replies = self.send(control, payload);
        for reply in &replies {
            assert!(
                reply.is_ok(),
                "expected {:?} to succeed, got {}:{}",
                control,
                reply.code,
                reply.message
            );
        }
        replies.first().and_then(|reply| reply.image_id)
    }

    /// Send a graphics command expected to fail, returning the error reply.
    pub fn err(&mut self, control: &str, payload: &[u8]) -> KittyReply {
        let reply = self.reply(control, payload);
        assert!(
            !reply.is_ok(),
            "expected {:?} to be rejected, got OK",
            control
        );
        reply
    }

    /// Feed arbitrary terminal input: text, CSI sequences, anything that is not a command whose
    /// reply we want to inspect.
    pub fn feed(&mut self, bytes: &[u8]) {
        test_support::feed_bytes(
            &mut self.grid,
            &mut self.vte_parser,
            &mut self.interceptor,
            bytes,
        );
    }

    pub fn change_character_cell_size(&mut self, size: SizeInPixels) {
        *self.grid.character_cell_size.borrow_mut() = Some(size);
        self.grid.character_cell_size_possibly_changed();
        self.grid.render_full_viewport();
    }

    pub fn placement_count(&mut self) -> usize {
        self.grid.kitty_placement_count()
    }

    pub fn image_count(&self) -> usize {
        self.store.borrow().image_count()
    }

    /// The (image id, placement id) pairs on screen, sorted for stable comparison.
    pub fn placement_ids(&mut self) -> Vec<(u32, u32)> {
        kitty_placement_id_set(&mut self.grid)
    }

    /// How many placements currently overlap the viewport. Placements that have scrolled into
    /// the scrollback still exist — they come back when the user scrolls up — but they are not
    /// on screen, which is the distinction most of the spec's wording is about.
    pub fn visible_placement_count(&mut self) -> usize {
        let cell_height = 20isize;
        let viewport_top = self.grid.lines_above.len() as isize * cell_height;
        let viewport_bottom = viewport_top + self.grid.height as isize * cell_height;
        self.grid
            .kitty_placements()
            .iter()
            .filter(|placement| {
                let top = placement.display_rect.y;
                let bottom = top + placement.display_rect.height as isize;
                bottom > viewport_top && top < viewport_bottom
            })
            .count()
    }

    /// The row the placement sits on, counted from the top of the viewport. Negative once it has
    /// scrolled off the top.
    pub fn placement_viewport_row(&mut self, index: usize) -> isize {
        let cell_height = 20isize;
        let viewport_top = self.grid.lines_above.len() as isize * cell_height;
        (self.grid.kitty_placements()[index].display_rect.y - viewport_top).div_euclid(cell_height)
    }

    pub fn cursor(&self) -> (usize, usize) {
        self.grid
            .cursor_coordinates()
            .map(|(x, y, _)| (x, y))
            .expect("the cursor is visible")
    }

    /// Whether the image can still be displayed, which is how a client observes whether its data
    /// survived a delete.
    ///
    /// The probe is a real `a=p`, so a successful one **leaves a placement behind**. Call it after
    /// any assertion on placement counts, not before.
    pub fn can_still_display_by_placing_it(&mut self, image_id: u32) -> bool {
        self.reply(&format!("a=p,i={},C=1", image_id), &[]).is_ok()
    }
}

fn kitty_image_px_below_canonical_line(grid: &mut Grid, canonical_index: usize) -> isize {
    let placement_y = grid.kitty_placements()[0].display_rect.y;
    let cell_height = 20isize;
    let line_start = buffer_cell_row_of_canonical_line(grid, canonical_index);
    placement_y - line_start * cell_height
}

fn kitty_image_absolute_cell_row(grid: &mut Grid) -> isize {
    let placement = &grid.kitty_placements()[0];
    let cell_height = 20isize;
    placement.display_rect.y.div_euclid(cell_height)
}

fn buffer_cell_row_of_canonical_line(grid: &Grid, canonical_index: usize) -> isize {
    let mut canonical_seen = 0usize;
    for (wrapped_row, row) in grid
        .lines_above
        .iter()
        .chain(grid.viewport.iter())
        .enumerate()
    {
        if row.is_canonical {
            if canonical_seen == canonical_index {
                return wrapped_row as isize;
            }
            canonical_seen += 1;
        }
    }
    -1
}

fn kitty_row_starts_the_marker_line(row: &super::super::Row) -> bool {
    row.is_canonical && row_text(row).starts_with("MARKER")
}

fn kitty_marker_is_below_the_viewport(grid: &Grid) -> bool {
    grid.lines_below
        .iter()
        .any(kitty_row_starts_the_marker_line)
}

fn kitty_marker_absolute_cell_row(grid: &Grid) -> Option<isize> {
    grid.lines_above
        .iter()
        .chain(grid.viewport.iter())
        .position(kitty_row_starts_the_marker_line)
        .map(|row| row as isize)
}

fn kitty_marker_extended_absolute_cell_row(grid: &Grid) -> Option<isize> {
    grid.lines_above
        .iter()
        .chain(grid.viewport.iter())
        .chain(grid.lines_below.iter())
        .position(kitty_row_starts_the_marker_line)
        .map(|row| row as isize)
}

fn kitty_scrollback_has_merged_wrapped_rows(grid: &Grid) -> bool {
    grid.lines_above.iter().any(|row| row.width() > grid.width)
}

fn kitty_marker_viewport_row(grid: &Grid) -> Option<usize> {
    let marker_row = kitty_marker_absolute_cell_row(grid)?;
    let viewport_row = marker_row - grid.lines_above.len() as isize;
    if viewport_row >= 0 && (viewport_row as usize) < grid.viewport.len() {
        Some(viewport_row as usize)
    } else {
        None
    }
}

fn kitty_wrapped_scrollback_setup(
    wrapped_line_width: usize,
) -> (
    Grid,
    Rc<RefCell<KittyImageStore>>,
    vte::Parser,
    KittyApcInterceptor,
) {
    let (mut grid, kitty_image_store) = new_kitty_grid(5, 20);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    let wrapped_line = format!("{}\r\n", "A".repeat(wrapped_line_width));
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        wrapped_line.as_bytes(),
    );
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"MARKER");
    let marker_viewport_row = kitty_marker_viewport_row(&grid).expect("marker must be visible");
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        format!("\x1b[{};1H", marker_viewport_row + 1).as_bytes(),
    );
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=20,v=20,i=1,C=1", &rgb_raster(20, 20)),
    );
    assert_eq!(
        Some(kitty_image_absolute_cell_row(&mut grid)),
        kitty_marker_absolute_cell_row(&grid),
        "the image must start on the row of its marker line"
    );
    for _ in 0..5 {
        feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\r\nx");
    }
    assert!(
        kitty_scrollback_has_merged_wrapped_rows(&grid),
        "the wrapped line must have been merged into a single scrollback row"
    );
    (grid, kitty_image_store, vte_parser, interceptor)
}

fn kitty_marker_canonical_line_index(grid: &Grid) -> Option<usize> {
    let mut canonical_lines_seen = 0usize;
    for row in grid.lines_above.iter().chain(grid.viewport.iter()) {
        if row.is_canonical {
            canonical_lines_seen += 1;
        }
        if row_text(row) == "MARKER" {
            return Some(canonical_lines_seen.saturating_sub(1));
        }
    }
    None
}

fn kitty_marker_with_image_on_first_row(
    rows: usize,
    columns: usize,
) -> (
    Grid,
    Rc<RefCell<KittyImageStore>>,
    vte::Parser,
    KittyApcInterceptor,
) {
    let (mut grid, kitty_image_store) = new_kitty_grid(rows, columns);
    let mut vte_parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[1;1H");
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"MARKER");
    feed_kitty_bytes(&mut grid, &mut vte_parser, &mut interceptor, b"\x1b[1;1H");
    feed_kitty_bytes(
        &mut grid,
        &mut vte_parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=20,v=20,i=1,C=1", &rgb_raster(20, 20)),
    );
    assert_eq!(grid.kitty_placement_count(), 1);
    assert_eq!(kitty_marker_absolute_cell_row(&grid), Some(0));
    assert_eq!(kitty_image_absolute_cell_row(&mut grid), 0);
    (grid, kitty_image_store, vte_parser, interceptor)
}

#[derive(Clone, Copy)]
enum BufferedDcs {
    UiComponent,
    NestedFrame,
    Xtgettcap,
}

fn buffered_dcs(grid: &Grid, kind: BufferedDcs) -> Option<&Vec<u8>> {
    match kind {
        BufferedDcs::UiComponent => grid.ui_component_bytes.as_ref(),
        BufferedDcs::NestedFrame => grid.nested_frame_bytes.as_ref(),
        BufferedDcs::Xtgettcap => grid.xtgettcap_bytes.as_ref(),
    }
}

fn assert_dcs_payload_is_bounded(prefix: &[u8], kind: BufferedDcs) {
    let mut grid = create_grid_with_content("");
    let mut parser = vte::Parser::new();
    parser.advance(&mut grid, prefix);
    parser.advance(&mut grid, &vec![b'a'; MAX_DCS_PAYLOAD_BYTES]);
    assert_eq!(
        buffered_dcs(&grid, kind).map(Vec::len),
        Some(MAX_DCS_PAYLOAD_BYTES),
        "the exact limit remains valid"
    );
    parser.advance(&mut grid, b"b");
    assert!(
        buffered_dcs(&grid, kind).is_none(),
        "byte MAX + 1 drops the whole payload"
    );
    parser.advance(&mut grid, b"\x1b\\");

    parser.advance(&mut grid, prefix);
    parser.advance(&mut grid, b"c");
    assert_eq!(
        buffered_dcs(&grid, kind).map(Vec::as_slice),
        Some(&b"c"[..])
    );
}

#[test]
fn ui_component_dcs_payload_is_bounded_and_recovers() {
    assert_dcs_payload_is_bounded(b"\x1bPz", BufferedDcs::UiComponent);
}

#[test]
fn nested_frame_dcs_payload_is_bounded_and_recovers() {
    let prefix = format!("\x1bP{}n", zellij_utils::nested_session::NESTED_DCS_PARAM);
    assert_dcs_payload_is_bounded(prefix.as_bytes(), BufferedDcs::NestedFrame);
}

#[test]
fn xtgettcap_dcs_payload_is_bounded_and_recovers() {
    assert_dcs_payload_is_bounded(b"\x1bP+q", BufferedDcs::Xtgettcap);
}

mod app_replay;
mod command_respawn;
mod highlights;
mod host_capabilities;
mod kitty_animation;
mod kitty_graphics;
mod kitty_properties;
mod kitty_relative_placement;
mod kitty_shared_memory;
mod kitty_spec;
mod kitty_unicode_placeholders;
mod notifications;
mod osc133_navigation;
mod osc133_selection;
mod scroll_region_and_forwarding;
mod sixel_and_ui;
mod vttest;
mod wide_chars;
