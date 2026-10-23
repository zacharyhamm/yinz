use super::*;

#[test]
fn osc133_a_b_c_d_selects_command_and_output_without_prompt() {
    let mut grid = create_grid_with_content(
        "\x1b]133;A\x07$ \x1b]133;B\x07echo hi\x1b]133;C\x07\r\nhi\x1b]133;D;0\x07 suffix",
    );

    assert_eq!(
        select_with_click_count(&mut grid, Position::new(1, 0), 3),
        "echo hi\nhi"
    );
}

#[test]
fn osc133_p_i_aliases_select_command_and_output_without_prompt() {
    let mut grid = create_grid_with_content(
        "\x1b]133;P\x07> \x1b]133;I\x07pwd\x1b]133;C\x07\r\n/tmp\x1b]133;D\x07",
    );

    let position = viewport_position_of(&grid, "/tmp");
    assert_eq!(select_with_click_count(&mut grid, position, 3), "pwd\n/tmp");
}

#[test]
fn osc133_triple_click_outside_output_keeps_logical_line_selection() {
    let mut grid = create_grid_with_content(
        "\x1b]133;A\x07$ \x1b]133;B\x07echo hi\x1b]133;C\x07 output\x1b]133;D\x07",
    );

    assert_eq!(
        select_with_click_count(&mut grid, Position::new(0, 0), 3),
        "$ echo hi output"
    );
}

#[test]
fn osc133_double_click_keeps_word_selection() {
    let mut grid = create_grid_with_content(
        "\x1b]133;A\x07$ \x1b]133;B\x07echo\x1b]133;C\x07 result word\x1b]133;D\x07",
    );

    let position = viewport_position_of(&grid, "word");
    assert_eq!(select_with_click_count(&mut grid, position, 2), "word");
}

#[test]
fn osc133_wrapped_command_and_output_are_selected_together() {
    let mut grid = create_grid_with_size_and_raw(
        4,
        8,
        b"\x1b]133;A\x07$ \x1b]133;B\x07longcmd\x1b]133;C\x07-output\x1b]133;D\x07",
    );

    let position = viewport_position_of(&grid, "output");
    assert_eq!(
        select_with_click_count(&mut grid, position, 3),
        "longcmd-output"
    );
}

#[test]
fn osc133_running_command_without_end_boundary_falls_back_to_logical_line_selection() {
    let mut grid =
        create_grid_with_content("\x1b]133;A\x07$ \x1b]133;B\x07sleep\x1b]133;C\x07\r\npartial");

    let position = viewport_position_of(&grid, "partial");
    assert_eq!(select_with_click_count(&mut grid, position, 3), "partial");
}

#[test]
fn osc133_command_becomes_selectable_once_its_end_boundary_arrives() {
    let mut grid =
        create_grid_with_content("\x1b]133;A\x07$ \x1b]133;B\x07sleep\x1b]133;C\x07\r\npartial");
    let position = viewport_position_of(&grid, "partial");

    let mut vte_parser = vte::Parser::new();
    vte_parser.advance(&mut grid, b"\x1b]133;D;0\x07");

    assert_eq!(
        select_with_click_count(&mut grid, position, 3),
        "sleep\npartial"
    );
}

#[test]
fn osc133_markers_survive_scrollback() {
    let mut grid = create_grid_with_size_and_raw(
        3,
        20,
        b"\x1b]133;A\x07$ \x1b]133;B\x07cmd\x1b]133;C\x07\r\nRESULT\x1b]133;D\x07\r\ntail1\r\ntail2\r\ntail3\r\ntail4",
    );
    for _ in 0..3 {
        grid.scroll_up_one_line();
    }

    let position = viewport_position_of(&grid, "RESULT");
    assert_eq!(
        select_with_click_count(&mut grid, position, 3),
        "cmd\nRESULT"
    );
}

#[test]
fn osc133_markers_survive_resize_reflow() {
    let mut grid = create_grid_with_size_and_raw(
        5,
        20,
        b"\x1b]133;A\x07$ \x1b]133;B\x07echo hi\x1b]133;C\x07RESULT\x1b]133;D\x07",
    );
    grid.change_size(5, 6);

    let position = viewport_position_of(&grid, "RES");
    assert_eq!(
        select_with_click_count(&mut grid, position, 3),
        "echo hiRESULT"
    );
}

#[test]
fn osc133_command_selection_disabled_falls_back_to_logical_line_selection() {
    let mut grid = create_grid_with_content(
        "\x1b]133;A\x07$ \x1b]133;B\x07echo hi\x1b]133;C\x07\r\nhi\x1b]133;D;0\x07",
    );
    grid.set_selection_options(false, DEFAULT_WORD_SEPARATORS);

    assert_eq!(
        select_with_click_count(&mut grid, Position::new(1, 0), 3),
        "hi"
    );
}

#[test]
fn osc133_command_selection_can_be_re_enabled_for_existing_markers() {
    let mut grid = create_grid_with_content(
        "\x1b]133;A\x07$ \x1b]133;B\x07echo hi\x1b]133;C\x07\r\nhi\x1b]133;D;0\x07",
    );
    grid.set_selection_options(false, DEFAULT_WORD_SEPARATORS);
    grid.set_selection_options(true, DEFAULT_WORD_SEPARATORS);

    assert_eq!(
        select_with_click_count(&mut grid, Position::new(1, 0), 3),
        "echo hi\nhi"
    );
}

#[test]
fn wrapped_word_selection_uses_terminal_columns() {
    for (text, width, separators, clicks, start, end, word) in [
        (
            "界abcd",
            4,
            "",
            vec![(0, 0), (0, 1), (0, 2), (1, 0)],
            (0, 0),
            (1, 2),
            "界abcd",
        ),
        (
            "界:abcd",
            5,
            ":",
            vec![(0, 3), (1, 0)],
            (0, 3),
            (1, 2),
            "abcd",
        ),
        (
            "ab界cd:ef",
            4,
            ":",
            vec![(0, 0), (1, 1)],
            (0, 0),
            (1, 2),
            "ab界cd",
        ),
        (
            "ab界界cdef",
            4,
            "",
            vec![(0, 0), (1, 0), (1, 1), (2, 0)],
            (0, 0),
            (2, 2),
            "ab界界cdef",
        ),
        (
            "界ab\r\ncd",
            4,
            "",
            vec![(0, 0), (0, 1)],
            (0, 0),
            (0, 4),
            "界ab",
        ),
        ("界ab\r\ncd", 4, "", vec![(1, 0)], (1, 0), (1, 2), "cd"),
    ] {
        for (line, column) in clicks {
            let mut grid = create_grid_with_size_and_raw(4, width, text.as_bytes());
            grid.set_selection_options(true, separators);
            let click = Position::new(line, column);
            assert_eq!(
                grid.word_around_position(&click),
                Some((Position::new(start.0, start.1), Position::new(end.0, end.1))),
                "{text:?} at {click:?}"
            );
            assert_eq!(select_with_click_count(&mut grid, click, 2), word);
        }
    }
}

#[test]
fn configured_word_separators_split_double_click_selection() {
    let mut grid = create_grid_with_content("foo:bar baz");
    grid.set_selection_options(true, ":");

    let position = viewport_position_of(&grid, "bar");
    assert_eq!(select_with_click_count(&mut grid, position, 2), "bar");
}

#[test]
fn default_word_separators_keep_bracket_boundaries() {
    let mut grid = create_grid_with_content("foo(bar) baz");
    grid.set_selection_options(true, DEFAULT_WORD_SEPARATORS);

    let position = viewport_position_of(&grid, "bar");
    assert_eq!(select_with_click_count(&mut grid, position, 2), "bar");
}

#[test]
fn osc133_backward_scan_finds_command_start_in_deep_scrollback() {
    let mut content = b"\x1b]133;A\x07$ \x1b]133;B\x07cmd\x1b]133;C\x07\r\nline00".to_vec();
    for i in 1..40 {
        content.extend_from_slice(format!("\r\nline{:02}", i).as_bytes());
    }
    content.extend_from_slice(b"\x1b]133;D\x07");
    let mut grid = create_grid_with_size_and_raw(10, 20, &content);

    let position = viewport_position_of(&grid, "line39");
    let expected = std::iter::once("cmd".to_string())
        .chain((0..40).map(|i| format!("line{:02}", i)))
        .collect::<Vec<String>>()
        .join("\n");
    assert_eq!(select_with_click_count(&mut grid, position, 3), expected);
}

#[test]
fn cursor_forward_over_unwritten_line_positions_character() {
    use crate::panes::terminal_character::AnsiCode;

    let content = "\u{1b}[1BABC \u{1b}[1B\r\u{1b}[3C\u{1b}[42mDEF\u{1b}[0m";
    let grid = create_grid_with_content(content);

    let row = &grid.viewport[2];

    assert_eq!(row.columns[0].character, ' ');
    assert_eq!(row.columns[1].character, ' ');
    assert_eq!(row.columns[2].character, ' ');
    assert_eq!(row.columns[3].character, 'D');
    assert_eq!(row.columns[4].character, 'E');
    assert_eq!(row.columns[5].character, 'F');

    let has_background = |c: &crate::panes::terminal_character::TerminalCharacter| {
        !matches!(c.styles.background, Some(AnsiCode::Reset) | None)
    };
    assert!(!has_background(&row.columns[0]));
    assert!(!has_background(&row.columns[1]));
    assert!(!has_background(&row.columns[2]));
    assert!(has_background(&row.columns[3]));
    assert!(has_background(&row.columns[4]));
    assert!(has_background(&row.columns[5]));
}

#[test]
fn double_click_selection_preserved_after_scroll() {
    let content = "line 0\nline 1\nline 2\nline 3\nline 4\nthis is a word test\nline 6\nline 7\nline 8\nline 9\nline 10\nline 11\nline 12\nline 13\nline 14\nline 15\nline 16\nline 17\nline 18\nline 19\n";
    let mut grid = create_grid_with_content(content);

    for _ in 0..20 {
        grid.add_canonical_line();
    }

    let word_position = Position::new(5, 10);
    grid.start_selection(&word_position);

    let selection_before = grid.get_selected_text();
    let word_start = grid.selection.start;
    let word_end = grid.selection.end;

    grid.end_selection(&word_position);

    grid.scroll_up_one_line();

    let selection_after_start = grid.selection.start;
    let selection_after_end = grid.selection.end;

    assert_eq!(selection_after_start.line.get(), word_start.line.get() + 1);
    assert_eq!(selection_after_end.line.get(), word_end.line.get() + 1);
    assert_eq!(selection_after_start.column, word_start.column);
    assert_eq!(selection_after_end.column, word_end.column);

    let text_after = grid.get_selected_text();
    assert_eq!(selection_before, text_after);
}

#[test]
fn triple_click_selection_preserved_after_scroll() {
    let content = "line 0\nline 1\nline 2\nline 3\nline 4\nthis is line five with some text\nline 6\nline 7\nline 8\nline 9\nline 10\nline 11\nline 12\nline 13\nline 14\nline 15\nline 16\nline 17\nline 18\nline 19\n";
    let mut grid = create_grid_with_content(content);

    for _ in 0..20 {
        grid.add_canonical_line();
    }

    let line_position = Position::new(5, 15);
    grid.start_selection(&line_position);
    grid.start_selection(&line_position);
    grid.start_selection(&line_position);

    let selection_before = grid.get_selected_text();
    let line_start = grid.selection.start;
    let line_end = grid.selection.end;

    grid.end_selection(&line_position);

    grid.scroll_up_one_line();

    let selection_after_start = grid.selection.start;
    let selection_after_end = grid.selection.end;

    assert_eq!(selection_after_start.line.get(), line_start.line.get() + 1);
    assert_eq!(selection_after_end.line.get(), line_end.line.get() + 1);
    assert_eq!(selection_after_start.column, line_start.column);
    assert_eq!(selection_after_end.column, line_end.column);

    let text_after = grid.get_selected_text();
    assert_eq!(selection_before, text_after);
}

#[test]
fn double_click_selection_moves_with_multiple_scrolls() {
    let content = "line 0\nline 1\nline 2\nline 3\nline 4\nthis is a word test\nline 6\nline 7\nline 8\nline 9\nline 10\nline 11\nline 12\nline 13\nline 14\nline 15\nline 16\nline 17\nline 18\nline 19\n";
    let mut grid = create_grid_with_content(content);

    for _ in 0..20 {
        grid.add_canonical_line();
    }

    let word_position = Position::new(5, 10);
    grid.start_selection(&word_position);

    let initial_start = grid.selection.start;
    let initial_end = grid.selection.end;

    grid.end_selection(&word_position);

    for _ in 0..5 {
        grid.scroll_up_one_line();
    }

    assert_eq!(
        grid.selection.start.line.get(),
        initial_start.line.get() + 5
    );
    assert_eq!(grid.selection.end.line.get(), initial_end.line.get() + 5);
    assert_eq!(grid.selection.start.column, initial_start.column);
    assert_eq!(grid.selection.end.column, initial_end.column);
}

#[test]
fn single_click_drag_selection_preserved_after_scroll() {
    let content = "line 0\nline 1\nline 2\nline 3\nline 4\nsome text here\nline 6\nline 7\nline 8\nline 9\nline 10\nline 11\nline 12\nline 13\nline 14\nline 15\nline 16\nline 17\nline 18\nline 19\n";
    let mut grid = create_grid_with_content(content);

    for _ in 0..20 {
        grid.add_canonical_line();
    }

    grid.start_selection(&Position::new(5, 5));
    grid.update_selection(&Position::new(5, 10));

    let start_before = grid.selection.start;
    let end_before = grid.selection.end;

    grid.end_selection(&Position::new(5, 10));

    grid.scroll_up_one_line();

    assert_eq!(grid.selection.start.line.get(), start_before.line.get() + 1);
    assert_eq!(grid.selection.end.line.get(), end_before.line.get() + 1);
    assert_eq!(grid.selection.start.column, start_before.column);
    assert_eq!(grid.selection.end.column, end_before.column);
}

#[test]
fn osc_11_set_and_query_pane_default_bg() {
    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let mut grid = Grid::new(
        10,
        20,
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

    // Set background via OSC 11
    let set_bg = b"\x1b]11;#001a3a\x07";
    vte_parser.advance(&mut grid, set_bg);

    assert_eq!(grid.effective_background(), Some((0, 26, 58)));

    // Query background via OSC 11 — because a pane-scoped override is
    // in place, the query is short-circuited: apps inside the pane
    // must see what Yinz is actually rendering, not the host
    // terminal's bg. The reply uses xterm's canonical
    // `rgb:RRRR/GGGG/BBBB` form with each 8-bit channel widened by
    // repetition (0x00 → 0x0000, 0x1a → 0x1a1a, 0x3a → 0x3a3a).
    let query_bg = b"\x1b]11;?\x07";
    vte_parser.advance(&mut grid, query_bg);

    assert!(
        grid.pending_forwarded_queries.is_empty(),
        "OSC 11 query must not be forwarded when a pane override is set"
    );
    assert_eq!(grid.pending_messages_to_pty.len(), 1);
    let reply = String::from_utf8(grid.pending_messages_to_pty[0].clone()).unwrap();
    assert_eq!(reply, "\u{1b}]11;rgb:0000/1a1a/3a3a\u{7}",);
}

#[test]
fn osc_10_set_and_query_pane_default_fg() {
    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let mut grid = Grid::new(
        10,
        20,
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

    // Set foreground via OSC 10
    let set_fg = b"\x1b]10;#00e000\x07";
    vte_parser.advance(&mut grid, set_fg);

    assert_eq!(grid.effective_foreground(), Some((0, 224, 0)));

    // Query foreground via OSC 10 — pane-scoped override is in place,
    // so the query is answered locally (see OSC 11 equivalent test for
    // the short-circuit rationale).
    let query_fg = b"\x1b]10;?\x07";
    vte_parser.advance(&mut grid, query_fg);

    assert!(
        grid.pending_forwarded_queries.is_empty(),
        "OSC 10 query must not be forwarded when a pane override is set"
    );
    assert_eq!(grid.pending_messages_to_pty.len(), 1);
    let reply = String::from_utf8(grid.pending_messages_to_pty[0].clone()).unwrap();
    assert_eq!(reply, "\u{1b}]10;rgb:0000/e0e0/0000\u{7}",);
}

#[test]
fn osc_110_111_reset_pane_default_colors() {
    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let mut grid = Grid::new(
        10,
        20,
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

    // Set both fg and bg
    let set_fg = b"\x1b]10;#00e000\x07";
    vte_parser.advance(&mut grid, set_fg);
    let set_bg = b"\x1b]11;#001a3a\x07";
    vte_parser.advance(&mut grid, set_bg);

    assert_eq!(grid.effective_foreground(), Some((0, 224, 0)));
    assert_eq!(grid.effective_background(), Some((0, 26, 58)));

    // Reset foreground via OSC 110
    let reset_fg = b"\x1b]110\x07";
    vte_parser.advance(&mut grid, reset_fg);
    assert_eq!(grid.effective_foreground(), None);
    assert_eq!(grid.effective_background(), Some((0, 26, 58)));

    // Reset background via OSC 111
    let reset_bg = b"\x1b]111\x07";
    vte_parser.advance(&mut grid, reset_bg);
    assert_eq!(grid.effective_foreground(), None);
    assert_eq!(grid.effective_background(), None);
}

#[test]
fn osc_11_set_bg_produces_ansi_in_render_output() {
    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let mut grid = Grid::new(
        5,
        10,
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

    // Set background via OSC 11
    let set_bg = b"\x1b]11;#001a3a\x07";
    vte_parser.advance(&mut grid, set_bg);

    assert_eq!(grid.effective_background(), Some((0, 26, 58)));

    // Render the grid and check that the pane defaults are stamped on chunks
    let style = Style::default();
    let render_result = grid.render(0, 0, &style).unwrap();
    assert!(render_result.is_some(), "Expected render output");

    let (chunks, _, _, _) = render_result.unwrap();
    assert!(!chunks.is_empty(), "Expected at least one character chunk");

    // All chunks should carry the pane default bg
    for chunk in &chunks {
        assert_eq!(
            chunk.pane_default_bg,
            Some(AnsiCode::RgbCode((0, 26, 58))),
            "Chunk should carry pane default background"
        );
    }
}
