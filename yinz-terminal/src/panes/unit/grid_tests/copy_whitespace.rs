use super::*;

#[test]
fn copying_preserves_printed_whitespace_and_trims_only_implicit_blanks() {
    for (input, expected) in [
        ("text  \r\n   \r\n", "text  \n   \n"),
        ("text\u{a0}\u{2003}", "text\u{a0}\u{2003}"),
        ("text  \x1b[K", "text  "),
        ("text  \r\x1b[4C\x1b[K", "text"),
        ("text  \r\x1b[5C\x1b[X", "text "),
        ("text  \r\x1b[6C\x1b[2@", "text  "),
        ("text  \r\x1b[4C\x1b[P", "text "),
        ("text\x1b[3C", "text"),
        ("text\x1b[2C ", "text   "),
        ("界  ", "界  "),
        ("界\r ", " "),
    ] {
        let mut grid = create_grid_with_size_and_raw(4, 20, input.as_bytes());
        let end = if input.ends_with('\n') {
            Position::new(2, 20)
        } else {
            Position::new(0, 20)
        };
        grid.selection
            .set_start_and_end_positions(Position::new(0, 0), end);
        assert_eq!(
            grid.get_selected_text().as_deref(),
            Some(expected),
            "{input:?}"
        );
        assert_eq!(
            grid.text_in_range(Position::new(0, 0), end).as_deref(),
            Some(expected),
            "{input:?}"
        );
    }
}

#[test]
fn copy_provenance_survives_wrapping_scrollback_and_reflow() {
    let mut grid = create_grid_with_size_and_raw(3, 8, "界     x  ".as_bytes());
    assert_eq!(
        grid.text_in_range(Position::new(0, 0), Position::new(1, 8))
            .as_deref(),
        Some("界     x  ")
    );
    grid.change_size(3, 5);
    assert_eq!(
        grid.text_in_range(Position::new(0, 0), Position::new(1, 5))
            .as_deref(),
        Some("界     x  ")
    );
    grid.change_size(3, 12);
    assert_eq!(
        grid.text_in_range(Position::new(0, 0), Position::new(0, 12))
            .as_deref(),
        Some("界     x  ")
    );

    let mut grid = create_grid_with_size_and_raw(2, 8, b"old  \r\n   \r\nnew");
    assert_eq!(
        grid.text_in_range(Position::new(-1, 0), Position::new(0, 8))
            .as_deref(),
        Some("old  \n   ")
    );
    grid.scroll_up_one_line();
    assert_eq!(
        grid.text_in_range(Position::new(0, 0), Position::new(2, 8))
            .as_deref(),
        Some("old  \n   \nnew")
    );

    // An implicit gap at a wrap boundary stays interior to the logical line.
    let grid = create_grid_with_size_and_raw(2, 8, b"a\x1b[K\x1b[7Cb  ");
    assert_eq!(
        grid.text_in_range(Position::new(0, 0), Position::new(1, 8))
            .as_deref(),
        Some("a       b  ")
    );
}

#[test]
fn serialization_preserves_copy_whitespace_through_replay_and_resize() {
    let copied = |grid: &mut Grid| {
        // Copy from the top as a user would: scrollback stores joined logical rows.
        while !grid.lines_above.is_empty() {
            grid.scroll_up_one_line();
        }
        let start = Position::new(0, 0);
        let end = Position::new(
            (grid.viewport.len() + grid.lines_below.len()) as i32 - 1,
            grid.width as u16,
        );
        grid.selection.set_start_and_end_positions(start, end);
        let selected = grid.get_selected_text().unwrap();
        assert_eq!(grid.text_in_range(start, end).unwrap(), selected);
        while !grid.lines_below.is_empty() {
            grid.scroll_down_one_line();
        }
        selected.trim_end_matches('\n').to_owned()
    };
    for (columns, input, expected) in [
        (20, "hello\x1b[K", "hello"),
        (20, "hello  \x1b[K", "hello  "),
        (20, "hello\x1b[41m  \x1b[K\x1b[0m", "hello  "),
        (20, "a\x1b[3Cb  ", "a   b  "),
        (20, "界\x1b[3Cb  ", "界   b  "),
        (8, "a\x1b[K\x1b[7Cb  ", "a       b  "),
        (8, "1234567\x1b[K\x1b[C界  ", "1234567 界  "),
        (
            8,
            "1234567界\x1b[3Cz  \r\nnext\r\nlast",
            "1234567界   z  \nnext\nlast",
        ),
        (8, "old  \r\n   \r\nnew\x1b[K", "old  \n   \nnew"),
        (
            8,
            "a\x1b[K\x1b[7Cb  \r\nnext\r\nlast",
            "a       b  \nnext\nlast",
        ),
        (1, "x\r\n\x1b[K\x1b[Cy", "x\n y"),
    ] {
        for scrollback in [false, true] {
            let rows = if scrollback { 2 } else { 8 };
            let mut original = create_grid_with_size_and_raw(rows, columns, input.as_bytes());
            assert_eq!(copied(&mut original), expected, "input {input:?}");
            let serialized = original.serialize(scrollback.then_some(0)).unwrap();
            for replay_columns in [columns, 7, 12, 30] {
                // Serialization starts each canonical line with CR/LF, including the first.
                let mut replay = create_grid_with_size_and_raw(
                    rows,
                    replay_columns,
                    serialized.strip_prefix("\n\r").unwrap().as_bytes(),
                );
                assert_eq!(
                    copied(&mut replay),
                    expected,
                    "replay {columns} -> {replay_columns}: {input:?}: {serialized:?}"
                );
                for width in [7, 30, columns] {
                    replay.change_size(rows, width);
                    assert_eq!(
                        copied(&mut replay),
                        expected,
                        "resize {replay_columns} -> {width}: {input:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn serialization_preserves_styles_on_printed_and_erased_spaces() {
    use crate::panes::terminal_character::AnsiCode;
    let original =
        create_grid_with_size_and_raw(16, 20, b"\x1b[1;31;44mhello  \x1b[K\x1b[0m\r\nnext\x1b[K");
    let serialized = original.serialize(None).unwrap();
    for columns in [7, 20, 30] {
        let replay = create_grid_with_size_and_raw(
            16,
            columns,
            serialized.strip_prefix("\n\r").unwrap().as_bytes(),
        );
        let original_cells: Vec<_> = original
            .viewport
            .iter()
            .flat_map(|row| &row.columns)
            .collect();
        let replay_cells: Vec<_> = replay
            .viewport
            .iter()
            .flat_map(|row| &row.columns)
            .collect();
        assert_eq!(original_cells.len(), replay_cells.len());
        for (original, replay) in original_cells.into_iter().zip(replay_cells) {
            assert_eq!(original.character, replay.character);
            assert_eq!(original.is_implicit_blank(), replay.is_implicit_blank());
            assert_eq!(
                original.styles.background.unwrap_or(AnsiCode::Reset),
                replay.styles.background.unwrap_or(AnsiCode::Reset)
            );
            assert_eq!(
                original.styles.foreground.unwrap_or(AnsiCode::Reset),
                replay.styles.foreground.unwrap_or(AnsiCode::Reset)
            );
            assert_eq!(
                original.styles.bold.unwrap_or(AnsiCode::Reset),
                replay.styles.bold.unwrap_or(AnsiCode::Reset)
            );
        }
    }
}

#[test]
fn implicit_blank_marker_only_marks_a_preceding_space_without_changing_cursor_or_styles() {
    let marker = b"\x1b]777;yinz;implicit-blank\x1b\\";
    for (columns, input) in [(1, " "), (3, "界 "), (3, "abc "), (8, "界 ")] {
        let mut grid = create_grid_with_size_and_raw(4, columns, input.as_bytes());
        feed_bytes(&mut grid, b"\x1b[1;31;44m");
        let cursor = grid.cursor.clone();
        let mut expected = grid.get_character_at(cursor.x - 1, cursor.y).unwrap();
        expected.mark_implicit_blank();
        feed_bytes(&mut grid, marker);
        assert_eq!((grid.cursor.x, grid.cursor.y), (cursor.x, cursor.y));
        assert_eq!(grid.cursor.pending_styles, cursor.pending_styles);
        assert_eq!(
            grid.get_character_at(cursor.x - 1, cursor.y),
            Some(expected)
        );
        // Pending wrap remains intact, including on a one-column terminal.
        feed_bytes(&mut grid, b"z");
        let control =
            create_grid_with_size_and_raw(4, columns, format!("{input}\x1b[1;31;44mz").as_bytes());
        assert_eq!(
            (grid.cursor.x, grid.cursor.y),
            (control.cursor.x, control.cursor.y)
        );
        assert_eq!(viewport_texts(&grid), viewport_texts(&control));
    }
    for input in ["", "x", "界", " \r", "\x1b[3C"] {
        let mut grid = create_grid_with_size_and_raw(4, 8, input.as_bytes());
        let before = grid.serialize(None);
        feed_bytes(&mut grid, marker);
        assert_eq!(grid.serialize(None), before, "{input:?}");
    }
    for malformed in [
        "\x1b]777;yinz\x1b\\",
        "\x1b]777;yinz;implicit-blank;extra\x1b\\",
        "\x1b]777;yinz;implicit-blankx\x1b\\",
        "\x1b]777;other;implicit-blank\x1b\\",
        "\x1b]777;yinz;implicit-blank\x07",
    ] {
        let grid = create_grid_with_size_and_raw(4, 8, format!(" {malformed}").as_bytes());
        assert!(!grid.get_character_at(0, 0).unwrap().is_implicit_blank());
    }
}
