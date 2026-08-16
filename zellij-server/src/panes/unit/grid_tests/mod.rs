use super::super::Grid;
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
use zellij_utils::data::{HighlightLayer, HighlightStyle, RegexHighlight};

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
    grid.lines_above.iter().map(|r| row_text(r)).collect()
}

fn viewport_texts(grid: &Grid) -> Vec<String> {
    grid.viewport.iter().map(|r| row_text(r)).collect()
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
    vte_parser.advance(&mut grid, &content);
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
    let kitty_image_store = Rc::new(RefCell::new(KittyImageStore::default()));
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

fn kitty_placement_id_set(grid: &Grid) -> Vec<(u32, u32)> {
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

fn kitty_image_px_below_canonical_line(grid: &Grid, canonical_index: usize) -> isize {
    let placement = &grid.kitty_placements()[0];
    let cell_height = 20isize;
    let line_start = buffer_cell_row_of_canonical_line(grid, canonical_index);
    placement.display_rect.y - line_start * cell_height
}

fn kitty_image_absolute_cell_row(grid: &Grid) -> isize {
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
        Some(kitty_image_absolute_cell_row(&grid)),
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
    assert_eq!(kitty_image_absolute_cell_row(&grid), 0);
    (grid, kitty_image_store, vte_parser, interceptor)
}

mod app_replay;
mod highlights;
mod host_capabilities;
mod kitty_graphics;
mod osc133_selection;
mod scroll_region_and_forwarding;
mod sixel_and_ui;
mod vttest;
mod wide_chars;
