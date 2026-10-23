use super::super::{Grid, Row};
use super::MAX_DCS_PAYLOAD_BYTES;
use crate::panes::grid::SixelImageStore;
use crate::panes::kitty_graphics::KittyImageStore;
use crate::panes::link_handler::LinkHandler;
use crate::panes::terminal_character::TerminalCharacter;
use base64::engine::general_purpose::STANDARD as BASE64_ENCODER;
use base64::engine::Engine as _;
use insta::assert_snapshot;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use vte;
use yinz_runtime::consts::SCROLL_BUFFER_SIZE;
use yinz_types::input::options::DEFAULT_WORD_SEPARATORS;
use yinz_types::pane_size::SizeInPixels;
use yinz_types::position::Position;
use yinz_types::style::{Palette, Style};
use yinz_types::ui_components::UI_COMPONENT_DCS_PREFIX;

use std::fmt::Write;

fn read_fixture(fixture_name: &str) -> Vec<u8> {
    let mut path_to_file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(fixture_name);
    if !path_to_file.exists() {
        // Replays also used by server orchestration tests remain shared.
        path_to_file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../src/tests/fixtures")
            .join(fixture_name);
    }
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

#[test]
fn palette_register_changes_invalidate_rows_but_queries_and_invalid_input_do_not() {
    let mut grid = create_grid_with_content("\x1b[31mindexed");
    let mut parser = vte::Parser::new();
    grid.read_changes(0, 0);
    for (osc, dirty) in [
        ("4;1;rgb:ff/00/ff", true),
        ("4;1;?", false),
        ("4;1;invalid", false),
        ("4;256;rgb:ff/00/ff", false),
        ("4;1", false),
        ("104;invalid", false),
        ("104;1", true),
        ("4;1;rgb:ff/00/ff", true),
        ("104", true),
    ] {
        parser.advance(&mut grid, format!("\x1b]{osc}\x07").as_bytes());
        let chunks = grid.read_changes(0, 0).0;
        assert_eq!(chunks.len(), if dirty { grid.height } else { 0 }, "{osc}");
        assert!(grid.read_changes(0, 0).0.is_empty());
    }
}

#[test]
fn full_dumps_include_output_below_the_viewport_without_moving_it() {
    let lines: Vec<_> = (1..=60).map(|n| n.to_string()).collect();
    let mut grid = create_grid_with_content(&format!("\x1b[31m{}", lines.join("\r\n")));
    let full_ansi = grid.dump_screen_with_ansi(true);
    let ansi_escape = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
    assert!(full_ansi.contains("\x1b[31m"));
    assert_eq!(ansi_escape.replace_all(&full_ansi, ""), lines.join("\n"));

    for scroll in [0, 20] {
        grid.move_viewport_up(scroll);
        grid.selection
            .set_start_and_end_positions(Position::new(1, 0), Position::new(2, 2));
        let selection = grid.selection;
        let viewport = grid.dump_screen(false);
        let viewport_ansi = grid.dump_screen_with_ansi(false);
        let position = grid.scrollback_position_and_length();
        let expected_viewport = lines[(40 - scroll)..(60 - scroll)].join("\n");
        assert_eq!(viewport, expected_viewport);
        assert_eq!(
            ansi_escape.replace_all(&viewport_ansi, ""),
            expected_viewport
        );
        assert_eq!(grid.dump_screen(true), lines.join("\n"));
        assert_eq!(grid.dump_screen_with_ansi(true), full_ansi);
        assert_eq!(grid.dump_screen(false), viewport);
        assert_eq!(grid.dump_screen_with_ansi(false), viewport_ansi);
        assert_eq!(grid.selection, selection);
        assert_eq!(grid.scrollback_position_and_length(), position);
    }
}

#[test]
fn full_dumps_join_wrapped_rows_across_both_viewport_boundaries() {
    let mut grid = create_grid_with_size_and_raw(
        3,
        4,
        b"\x1b[31maaaabbbbccccddddeeeeffff\x1b[0m\r\nZ\x1b[44m  \x1b[0m ",
    );
    let expected = "aaaabbbbccccddddeeeeffff\nZ";
    let ansi_escape = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
    let full_ansi = grid.dump_screen_with_ansi(true);
    assert_eq!(grid.dump_screen(true), expected);
    assert_eq!(
        ansi_escape.replace_all(&full_ansi, ""),
        format!("{expected}  ")
    );
    assert!(full_ansi.contains("\x1b[31m"));
    assert!(full_ansi.contains("\x1b[44m"));

    grid.move_viewport_up(2);
    assert!(!grid.viewport[0].is_canonical);
    assert!(!grid.lines_below[0].is_canonical);
    assert_eq!(grid.dump_screen(false), "ccccddddeeee");
    assert_eq!(grid.dump_screen(true), expected);
    assert_eq!(grid.dump_screen_with_ansi(true), full_ansi);
}

#[test]
fn short_scrolled_rows_release_capacity_without_losing_content_or_metadata() {
    for text in ["", "x", "界e\u{0301}"] {
        let mut grid = create_grid_with_size_and_raw(2, 80, b"\x1b[44m\r\n\r\n");
        feed_bytes(
            &mut grid,
            format!("\x1b]133;A\x07\x1b]8;;https://example.com\x07{text}\x1b]8;;\x07").as_bytes(),
        );
        let row = grid.viewport.back_mut().unwrap();
        assert!(row.columns.capacity() < 40);
        // Rows can retain capacity after erasing longer content or shrinking the terminal.
        row.columns.reserve(80);
        let expected = row.clone();
        feed_bytes(&mut grid, b"\r\n\r\n");

        let scrolled = grid.lines_above.back().unwrap();
        assert!(scrolled.columns.capacity() < 40);
        assert_eq!(scrolled.columns, expected.columns);
        assert_eq!(scrolled.bg_color, expected.bg_color);
        assert_eq!(scrolled.is_canonical, expected.is_canonical);
        assert_eq!(scrolled.osc133_markers, expected.osc133_markers);
        assert_eq!(scrolled.width(), expected.width());

        let dump = grid.dump_screen_with_ansi(true);
        grid.move_viewport_up(2);
        grid.reset_viewport();
        grid.change_size(4, 40);
        grid.change_size(2, 80);
        assert_eq!(grid.dump_screen_with_ansi(true), dump);
    }
}

#[test]
fn scrolling_blank_lines_does_not_allocate_cells() {
    let grid = create_grid_with_size_and_raw(2, 300, &b"\r\n".repeat(100));
    assert!(grid
        .viewport
        .iter()
        .chain(&grid.lines_above)
        .all(|row| { row.columns.is_empty() && row.columns.capacity() == 0 }));
}

#[test]
fn manual_scrolling_releases_spare_row_capacity() {
    let mut grid = create_grid_with_size_and_raw(2, 300, b"first\r\nsecond\r\nthird");
    grid.scroll_up_one_line();
    let row = grid.viewport.front_mut().unwrap();
    row.columns.reserve(300);
    assert_eq!(row_text(row), "first");
    grid.scroll_down_one_line();
    let row = grid.lines_above.back().unwrap();
    assert_eq!(row_text(row), "first");
    assert!(row.columns.capacity() < 150);
    assert_eq!(grid.dump_screen(true), "first\nsecond\nthird");
}

#[test]
fn a_non_scrolling_newline_does_not_paint_or_materialize_padding() {
    let mut grid = create_grid_with_content("\u{1b}[1;31;44m\r\n");
    let row = &grid.viewport[1];
    assert!(
        row.columns.is_empty(),
        "implicit blanks must not become text"
    );
    let background = None;
    assert_eq!(row.bg_color, background);
    let (chunks, _, _) = grid.read_changes(0, 0);
    let new_line = chunks.iter().find(|chunk| chunk.y == 1).unwrap();
    assert!(new_line
        .terminal_characters
        .iter()
        .all(|c| c.character == ' '
            && c.styles.background.filter(|bg| *bg != AnsiCode::Reset) == background
            && c.styles.bold != Some(AnsiCode::On)));
}

#[test]
fn scroll_region_edits_only_damage_the_rows_they_move() {
    use crate::panes::terminal_character::EMPTY_TERMINAL_CHARACTER;

    for operation in 0..4 {
        let mut grid = create_grid_with_content(&"line\r\n".repeat(19));
        grid.set_scroll_region(3, Some(8));
        grid.move_cursor_to(0, 5, EMPTY_TERMINAL_CHARACTER);
        grid.read_changes(0, 0);
        let first_changed = match operation {
            0 => {
                grid.rotate_scroll_region_up(1);
                3
            },
            1 => {
                grid.rotate_scroll_region_down(1);
                3
            },
            2 => {
                grid.delete_lines_in_scroll_region(1, EMPTY_TERMINAL_CHARACTER);
                5
            },
            _ => {
                grid.add_empty_lines_in_scroll_region(1, EMPTY_TERMINAL_CHARACTER);
                5
            },
        };
        let (chunks, _, _) = grid.read_changes(0, 0);
        assert_eq!(
            chunks.iter().map(|chunk| chunk.y).collect::<Vec<_>>(),
            (first_changed..=8).collect::<Vec<_>>(),
            "operation {operation}",
        );
    }

    // Moving rows into scrollback also moves selection feedback, even below the region.
    for flash in [false, true] {
        let mut grid = create_grid_with_content(&"line\r\n".repeat(19));
        grid.set_scroll_region(0, Some(2));
        let start = Position::new(10, 0);
        let end = Position::new(11, 3);
        if flash {
            grid.set_command_output_flash(start, end);
        } else {
            grid.selection.set_start_and_end_positions(start, end);
        }
        grid.read_changes(0, 0);
        grid.rotate_scroll_region_down(1);
        let (chunks, _, _) = grid.read_changes(0, 0);
        assert_eq!(
            chunks.iter().map(|chunk| chunk.y).collect::<Vec<_>>(),
            vec![0, 1, 2, 9, 10, 11],
        );
    }
}

#[test]
fn scrolling_repaints_an_empty_active_selection_that_becomes_nonempty() {
    let mut grid = create_grid_with_content(&"line\r\n".repeat(19));
    grid.set_scroll_region(0, Some(2));
    grid.selection.start(Position::new(10, 0));
    assert!(grid.selection.is_empty());
    grid.read_changes(0, 0);
    grid.rotate_scroll_region_down(1);
    assert!(!grid.selection.is_empty());
    let (chunks, _, _) = grid.read_changes(0, 0);
    assert_eq!(
        chunks.iter().map(|chunk| chunk.y).collect::<Vec<_>>(),
        vec![0, 1, 2, 9, 10]
    );
}

#[test]
fn scroll_region_edits_repaint_wrapped_highlights_across_both_boundaries() {
    use crate::panes::terminal_character::EMPTY_TERMINAL_CHARACTER;
    use yinz_types::data::{HighlightLayer, HighlightStyle, RegexHighlight};
    use yinz_types::internal_ids::PluginId;

    for operation in 0..4 {
        // One logical line spans rows 0–5, crossing both boundaries of rows 2–3.
        let mut grid = create_grid_with_content(&"a".repeat(480));
        grid.set_plugin_regex_highlights(
            PluginId::new(1),
            vec![RegexHighlight {
                pattern: "^a{480}$".into(),
                style: HighlightStyle::Emphasis0,
                layer: HighlightLayer::Hint,
                context: Default::default(),
                on_hover: false,
                bold: false,
                italic: false,
                underline: true,
                tooltip_text: None,
            }],
            &Style::default(),
        );
        assert!(grid.plugin_highlight_at(&Position::new(0, 0)).is_some());
        assert!(grid.plugin_highlight_at(&Position::new(5, 0)).is_some());
        grid.set_scroll_region(2, Some(3));
        grid.move_cursor_to(0, 2, EMPTY_TERMINAL_CHARACTER);
        grid.read_changes(0, 0);
        match operation {
            0 => grid.rotate_scroll_region_up(1),
            1 => grid.rotate_scroll_region_down(1),
            2 => grid.delete_lines_in_scroll_region(1, EMPTY_TERMINAL_CHARACTER),
            _ => grid.add_empty_lines_in_scroll_region(1, EMPTY_TERMINAL_CHARACTER),
        }
        assert!(grid.plugin_highlight_at(&Position::new(0, 0)).is_none());
        assert!(grid.plugin_highlight_at(&Position::new(5, 0)).is_none());
        let (chunks, _, _) = grid.read_changes(0, 0);
        assert_eq!(
            chunks.iter().map(|chunk| chunk.y).collect::<Vec<_>>(),
            (0..6).collect::<Vec<_>>(),
            "operation {operation}"
        );
    }
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
use yinz_types::geometry::Direction;
use {
    yinz_types::data::HighlightLayer, yinz_types::data::HighlightStyle,
    yinz_types::data::RegexHighlight,
};

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
fn swapping_the_ends_moves_the_start_of_a_keyboard_selection() {
    let mut grid = create_grid_with_content("abcdef");

    grid.start_text_selection(Position::new(0, 2));
    grid.move_text_selection(Direction::Right);
    grid.move_text_selection(Direction::Right);
    assert_eq!(grid.get_selected_text().as_deref(), Some("cd"));
    assert_eq!(grid.cursor_coordinates(), Some((4, 0, true)));

    grid.swap_text_selection_ends();
    assert_eq!(
        grid.get_selected_text().as_deref(),
        Some("cd"),
        "the swap itself selects nothing new"
    );
    assert_eq!(
        grid.cursor_coordinates(),
        Some((2, 0, true)),
        "the cursor marks the end that is now being moved"
    );

    grid.move_text_selection(Direction::Left);
    grid.move_text_selection(Direction::Left);
    assert_eq!(
        grid.get_selected_text().as_deref(),
        Some("abcd"),
        "movement now grows the selection from its start"
    );

    grid.swap_text_selection_ends();
    grid.move_text_selection(Direction::Right);
    assert_eq!(
        grid.get_selected_text().as_deref(),
        Some("abcde"),
        "swapping back returns movement to the original end"
    );

    grid.finish_text_selection();
    assert_eq!(
        grid.cursor_coordinates(),
        Some((6, 0, true)),
        "leaving the selection hands the cursor back to the terminal"
    );
}

#[test]
fn a_swapped_keyboard_selection_scrolls_from_its_start() {
    let mut grid = create_grid_with_scrollback();
    let lines_above = grid.lines_above.len();

    grid.start_text_selection(Position::new(1, 1));
    grid.move_text_selection(Direction::Right);
    grid.move_text_selection(Direction::Right);
    grid.swap_text_selection_ends();

    grid.move_text_selection(Direction::Up);
    assert_eq!(grid.lines_above.len(), lines_above);
    assert_eq!(grid.selection.start, Position::new(1, 3));
    assert_eq!(grid.selection.end, Position::new(0, 1));

    grid.move_text_selection(Direction::Up);
    assert_eq!(grid.lines_above.len(), lines_above - 1);
    assert_eq!(
        grid.selection.start,
        Position::new(2, 3),
        "the end that is not being moved rides the buffer"
    );
    assert_eq!(
        grid.selection.end,
        Position::new(0, 1),
        "the end being moved stays pinned to the viewport edge"
    );
}

#[test]
fn the_cell_under_the_keyboard_selection_cursor_is_the_one_being_drawn_on() {
    let mut grid = create_grid_with_content("abcdef");

    // the pty cursor sits past the end of the text, the selection cursor on the live end
    assert!(grid.get_character_under_cursor().is_none());

    grid.start_text_selection(Position::new(0, 2));
    let (cursor_x, cursor_y, _) = grid.cursor_coordinates().unwrap();
    assert_eq!(
        grid.get_character_at(cursor_x, cursor_y).unwrap().character,
        'c',
        "the fake cursor paints the character it is drawn over, not the one under the pty cursor"
    );
}

#[test]
fn a_mouse_selection_hands_the_cursor_back_to_the_pty() {
    let mut grid = create_grid_with_content("abcdef");
    let pty_cursor = (6, 0, true);

    grid.start_text_selection(Position::new(0, 2));
    grid.move_text_selection(Direction::Right);
    assert_eq!(grid.cursor_coordinates(), Some((3, 0, true)));

    grid.start_selection(&Position::new(0, 0));
    assert_eq!(
        grid.cursor_coordinates(),
        Some(pty_cursor),
        "a mouse selection takes over from the keyboard one"
    );
    grid.update_selection(&Position::new(0, 3));
    grid.end_selection(&Position::new(0, 3));
    assert_eq!(grid.cursor_coordinates(), Some(pty_cursor));
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

fn nested_announce_message() -> yinz_ipc::nested_session::NestedSessionMessage {
    yinz_ipc::nested_session::NestedSessionMessage::Announce {
        session_name: "guest-session".to_owned().into(),
        capabilities: vec![yinz_ipc::nested_session::NestedSessionCapability::NestedControl],
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
    let dcs = format!(
        "{UI_COMPONENT_DCS_PREFIX}{};{}\u{1b}\\",
        component_name, serialized_params
    );
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
            InterceptorResult::Swallow | InterceptorResult::CaptureStarted => {},
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

use crate::panes::kitty_test_support::{self as test_support, KittyReply};

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
    assert_dcs_payload_is_bounded(UI_COMPONENT_DCS_PREFIX.as_bytes(), BufferedDcs::UiComponent);
}

#[test]
fn nested_frame_dcs_payload_is_bounded_and_recovers() {
    let prefix = format!("\x1bP{}n", yinz_ipc::nested_session::NESTED_DCS_PARAM);
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
mod hyperlinks;
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

#[test]
fn search_retries_overlapping_prefixes_in_rows_and_wrapped_tails() {
    for (content, needle, width, count, start, end) in [
        ("aaab", "aab", 8, 1, (0, 1), (0, 4)),
        ("aaab", "aab", 2, 1, (0, 1), (1, 2)),
        ("ababc", "abc", 8, 1, (0, 2), (0, 5)),
        ("ababc", "abc", 3, 1, (0, 2), (1, 2)),
        ("aab aab", "aab", 4, 2, (0, 0), (0, 3)),
        ("aabaab", "aab", 3, 2, (0, 0), (0, 3)),
    ] {
        let mut grid = create_grid_with_size_and_raw(4, width, content.as_bytes());
        grid.set_search_string(needle);
        let selections = &grid.search_results.selections;
        assert_eq!(selections.len(), count, "{needle:?} in {content:?}");
        assert_eq!(selections[0].start, Position::new(start.0, start.1));
        assert_eq!(selections[0].end, Position::new(end.0, end.1));
    }
}

#[test]
fn whole_word_search_retries_wrapped_tail_after_rejecting_a_match() {
    let mut grid = create_grid_with_size_and_raw(3, 3, b"a a a ");
    grid.search_results.whole_word_only = true;
    grid.set_search_string("a a ");

    assert_eq!(grid.search_results.selections.len(), 1);
    assert_eq!(grid.search_results.selections[0].start, Position::new(0, 2));
    assert_eq!(grid.search_results.selections[0].end, Position::new(1, 3));
}

#[test]
fn searching_erased_wrapped_rows_does_not_index_empty_continuations() {
    for content in ["abcdx\x1b[2K", "abcdefghx\x1b[2K\x1b[A\x1b[2K"] {
        let mut grid = create_grid_with_size_and_raw(4, 4, content.as_bytes());

        grid.set_search_string("dx");

        assert!(grid.search_results.selections.is_empty());
        feed_bytes(&mut grid, b"\rxy");
        grid.set_search_string("dx");
        assert_eq!(grid.search_results.selections.len(), 1);
        assert_eq!(grid.search_results.selections[0].start, Position::new(0, 3));
        assert_eq!(grid.search_results.selections[0].end, Position::new(1, 1));
    }
}

#[test]
fn unicode_search_uses_character_indexes_and_terminal_columns() {
    for (content, needle, start, end) in [
        ("界 café 東京 🙂 end".to_owned(), "café", (0, 3), (0, 7)),
        ("界 café 東京 🙂 end".to_owned(), "東京", (0, 8), (0, 12)),
        ("界 café 東京 🙂 end".to_owned(), "🙂", (0, 13), (0, 15)),
        (
            "界 café 東京 🙂 end".to_owned(),
            "café 東京 🙂",
            (0, 3),
            (0, 15),
        ),
        (
            format!("{}界é🙂z", " ".repeat(77)),
            "界é🙂",
            (0, 77),
            (1, 2),
        ),
    ] {
        let mut grid = create_grid_with_content(&content);
        grid.set_search_string(needle);
        let selections = &grid.search_results.selections;
        assert_eq!(selections.len(), 1, "{needle:?} in {content:?}");
        assert_eq!(selections[0].start, Position::new(start.0, start.1));
        assert_eq!(selections[0].end, Position::new(end.0, end.1));
    }

    let mut grid = create_grid_with_content("界 café 東京 🙂 end");
    grid.set_search_string("café 東京 🙂");
    assert!(format!("{grid:?}").contains("界 ############ end"));

    let mut grid = create_grid_with_content("界 éx é ");
    grid.search_results.whole_word_only = true;
    grid.set_search_string("é");
    assert_eq!(grid.search_results.selections.len(), 1);
    assert_eq!(grid.search_results.selections[0].start, Position::new(0, 6));
    assert_eq!(grid.search_results.selections[0].end, Position::new(0, 7));
}

#[test]
fn dense_ascii_search_finds_every_cell_in_a_large_grid() {
    let mut grid = create_grid_with_content("");
    grid.width = 640;
    grid.viewport = (0..100)
        .map(|_| {
            Row::from_columns(
                (0..640)
                    .map(|_| TerminalCharacter::new_singlewidth('x'))
                    .collect(),
            )
            .canonical()
        })
        .collect();

    grid.set_search_string("x");

    assert_eq!(grid.search_results.selections.len(), 640 * 100);
    for (index, selection) in grid.search_results.selections.iter().enumerate() {
        let line = index / 640;
        let column = index % 640;
        assert_eq!(selection.start, Position::new(line as i32, column as u16));
        assert_eq!(
            selection.end,
            Position::new(line as i32, (column + 1) as u16)
        );
    }
}

#[test]
fn search_match_ending_on_a_full_tail_row_below_a_wide_char_wrap_stays_on_that_row() {
    // "xy" ends row 0 at column 79; the wide character does not fit in the last cell, so it
    // wraps and leaves the main row one column short of the grid. Row 1 is exactly 80 wide
    // and the match ends on its last cell, which must not be reflowed onto a third row.
    let tail = format!("界{}z", "a".repeat(77));
    let content = format!("{}xy{tail}", " ".repeat(77));
    let mut grid = create_grid_with_content(&content);
    assert_eq!(grid.viewport[0].width(), 79);
    assert_eq!(grid.viewport[1].width(), 80);
    grid.set_search_string(&format!("xy{tail}"));
    let selections = &grid.search_results.selections;
    assert_eq!(selections.len(), 1);
    assert_eq!(selections[0].start, Position::new(0, 77));
    assert_eq!(selections[0].end, Position::new(1, 80));
}

#[test]
fn mouse_button_reports_preserve_wire_bytes_and_tracking_modes() {
    for (position, encoded_coordinates) in [
        (Position::new(0, 0), "!!"),
        (Position::new(4, 6), "'%"),
        (Position::new(300, 500), "\u{215}\u{14d}"),
    ] {
        for encoding in ["", "\u{1b}[?1005h", "\u{1b}[?1006h"] {
            for tracking in [0, 1000, 1002, 1003] {
                let tracking_sequence = if tracking == 0 {
                    String::new()
                } else {
                    format!("\u{1b}[?{tracking}h")
                };
                let grid = create_grid_with_content(&format!("{tracking_sequence}{encoding}"));
                for (button, x10_button, suffix, is_motion, actual) in [
                    (
                        0,
                        ' ',
                        'M',
                        false,
                        grid.mouse_left_click_signal(&position, false),
                    ),
                    (
                        32,
                        '@',
                        'M',
                        true,
                        grid.mouse_left_click_signal(&position, true),
                    ),
                    (
                        0,
                        '#',
                        'm',
                        false,
                        grid.mouse_left_click_release_signal(&position),
                    ),
                    (
                        2,
                        '"',
                        'M',
                        false,
                        grid.mouse_right_click_signal(&position, false),
                    ),
                    (
                        34,
                        'B',
                        'M',
                        true,
                        grid.mouse_right_click_signal(&position, true),
                    ),
                    (
                        2,
                        '#',
                        'm',
                        false,
                        grid.mouse_right_click_release_signal(&position),
                    ),
                    (
                        1,
                        '!',
                        'M',
                        false,
                        grid.mouse_middle_click_signal(&position, false),
                    ),
                    (
                        33,
                        'A',
                        'M',
                        true,
                        grid.mouse_middle_click_signal(&position, true),
                    ),
                    (
                        1,
                        '#',
                        'm',
                        false,
                        grid.mouse_middle_click_release_signal(&position),
                    ),
                ] {
                    let expected = if tracking == 0 || (tracking == 1000 && is_motion) {
                        None
                    } else if encoding == "\u{1b}[?1006h" {
                        Some(format!(
                            "\u{1b}[<{button};{};{}{suffix}",
                            position.column() + 1,
                            position.line() + 1,
                        ))
                    } else {
                        Some(format!("\u{1b}[M{x10_button}{encoded_coordinates}"))
                    };
                    assert_eq!(
                        actual, expected,
                        "position={position:?}, encoding={encoding:?}, tracking={tracking}, \
                         button={button}, suffix={suffix}, motion={is_motion}"
                    );
                }
            }
        }
    }
}

#[test]
fn wheel_reports_share_encoding_and_preserve_modifiers() {
    use yinz_types::mouse::MouseEvent;

    let position = Position::new(4, 6);
    for encoding in ["", "\u{1b}[?1005h", "\u{1b}[?1006h"] {
        for tracking in [1000, 1002, 1003] {
            let grid = create_grid_with_content(&format!("\u{1b}[?{tracking}h{encoding}"));
            for (event, base, specialized) in [
                (
                    MouseEvent::new_scroll_up_event(position),
                    64,
                    grid.mouse_scroll_up_signal(&position),
                ),
                (
                    MouseEvent::new_scroll_down_event(position),
                    65,
                    grid.mouse_scroll_down_signal(&position),
                ),
            ] {
                assert_eq!(specialized, grid.mouse_event_signal(&event));
                for modifiers in 0..8 {
                    let mut event = event;
                    event.shift = modifiers & 1 != 0;
                    event.alt = modifiers & 2 != 0;
                    event.ctrl = modifiers & 4 != 0;
                    let button = base | (modifiers << 2);
                    let expected = if encoding == "\u{1b}[?1006h" {
                        format!("\u{1b}[<{button};7;5M")
                    } else {
                        format!("\u{1b}[M{}'%", char::from(button + 32))
                    };
                    assert_eq!(grid.mouse_event_signal(&event), Some(expected));
                }
            }
        }
        let grid = create_grid_with_content(encoding);
        assert_eq!(grid.mouse_scroll_up_signal(&position), None);
        assert_eq!(grid.mouse_scroll_down_signal(&position), None);
        assert_eq!(
            grid.mouse_event_signal(&MouseEvent::new_alt_scroll_up_event(position)),
            None
        );
    }
}

#[test]
fn translucency_repaints_completed_rows_during_synchronized_output() {
    use crate::panes::AnsiCode;
    let mut grid = create_grid_with_size_and_raw(3, 8, b"\x1b[31mfirst\r\nsecond\r\nthird");
    grid.selection
        .set_start_and_end_positions(Position::new(0, 1), Position::new(0, 3));
    grid.set_pane_default_colors(Some("#123456".into()), Some("#654321".into()));
    let mut colors = [None; 256];
    colors[1] = Some(AnsiCode::RgbCode((10, 20, 30)));
    grid.changed_colors = Some(colors);
    let original = grid.render(10, 20, &Style::default()).unwrap().unwrap().0;
    feed_bytes(&mut grid, b"\x1b[2;1HSECOND");
    let incremental = grid.render(10, 20, &Style::default()).unwrap().unwrap().0;
    assert_eq!(incremental.len(), 1);
    feed_bytes(&mut grid, b"\x1b[?2026h\x1b[HWRITING");
    assert!(grid.render(10, 20, &Style::default()).unwrap().is_none());
    // Live metadata may change too; the completed rows must keep their original metadata.
    grid.changed_colors = None;
    grid.selection.reset();
    for (rows, cols) in [(3, 8), (2, 4), (4, 10)] {
        grid.change_size(rows, cols);
        for _ in 0..2 {
            grid.render_full_viewport();
            let (chunks, raw, sixel, kitty) =
                grid.render(2, 3, &Style::default()).unwrap().unwrap();
            assert!(raw.is_none() && sixel.is_empty() && kitty.is_empty());
            assert_eq!(chunks.len(), rows);
            for (y, chunk) in chunks.iter().enumerate() {
                assert_eq!((chunk.x, chunk.y, chunk.width()), (2, 3 + y, cols));
                let expected = ["first", "SECOND", "third", ""][y];
                let expected = format!("{expected:<cols$}")
                    .chars()
                    .take(cols)
                    .collect::<String>();
                assert_eq!(
                    chunk
                        .terminal_characters
                        .iter()
                        .map(|c| c.character)
                        .collect::<String>(),
                    expected
                );
            }
            assert_eq!(chunks[0].changed_colors, original[0].changed_colors);
            assert_eq!(chunks[0].pane_default_fg, original[0].pane_default_fg);
            assert_eq!(chunks[0].pane_default_bg, original[0].pane_default_bg);
            assert_eq!(
                chunks[0].terminal_characters[0].styles,
                original[0].terminal_characters[0].styles
            );
            assert_eq!(
                chunks[0].selection_and_colors()[0].selection.start,
                Position::new(3, 3)
            );
            assert!(grid.output_buffer.should_update_all_lines);
            assert!(grid.render(2, 3, &Style::default()).unwrap().is_none());
        }
    }
    feed_bytes(&mut grid, b"\x1b[HFINISHED\x1b[?2026l");
    let unlocked = grid.render(2, 3, &Style::default()).unwrap().unwrap().0;
    assert!(unlocked[0]
        .terminal_characters
        .iter()
        .map(|c| c.character)
        .collect::<String>()
        .starts_with("FINISHED"));
    assert!(!grid.output_buffer.should_update_all_lines);
}

mod copy_whitespace;

mod agent_theme;
