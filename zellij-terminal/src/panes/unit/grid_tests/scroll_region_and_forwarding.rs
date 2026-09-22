use super::*;

#[test]
fn repeated_alternate_screen_entry_preserves_primary_screen_and_scrollback() {
    let mut grid = create_grid_with_size_and_raw(3, 8, b"one\r\ntwo\r\nthree\r\nfour");
    let primary_viewport = viewport_texts(&grid);
    let primary_scrollback = scrollback_texts(&grid);
    let primary_cursor = (grid.cursor.x, grid.cursor.y);
    assert!(!primary_scrollback.is_empty());

    feed_bytes(&mut grid, b"\x1b[?1049halt\x1b[?25l");
    let alternate_viewport = viewport_texts(&grid);
    feed_bytes(&mut grid, b"\x1b[?1049;25h");
    assert_eq!(viewport_texts(&grid), alternate_viewport);
    assert!(!grid.cursor_is_hidden);

    feed_bytes(&mut grid, b"\x1b[?1049l");
    assert_eq!(viewport_texts(&grid), primary_viewport);
    assert_eq!(scrollback_texts(&grid), primary_scrollback);
    assert_eq!((grid.cursor.x, grid.cursor.y), primary_cursor);
    assert!(!grid.is_alternate_mode_active());
}

#[test]
fn reverse_index_moves_outside_scroll_margins_without_scrolling() {
    for (row, expected_y) in [(1, 0), (2, 0), (4, 2), (6, 4), (7, 5)] {
        let mut grid =
            create_grid_with_size_and_raw(7, 5, b"AAA\r\nBBB\r\nCCC\r\nDDD\r\nEEE\r\nFFF\r\nGGG");
        feed_bytes(&mut grid, format!("\x1b[3;5r\x1b[{row};2H").as_bytes());
        let viewport_before = viewport_texts(&grid);

        feed_bytes(&mut grid, b"\x1bM");

        assert_eq!((grid.cursor.x, grid.cursor.y), (1, expected_y));
        assert_eq!(viewport_texts(&grid), viewport_before);
    }
}

#[test]
fn reverse_index_scrolls_only_at_top_margin_and_preserves_background() {
    let mut grid =
        create_grid_with_size_and_raw(7, 5, b"AAA\r\nBBB\r\nCCC\r\nDDD\r\nEEE\r\nFFF\r\nGGG");
    feed_bytes(&mut grid, b"\x1b[3;5r\x1b[5;2H\x1b[48;5;2m");
    let viewport_before = viewport_texts(&grid);

    grid.move_cursor_up_with_scrolling(2);

    assert_eq!(viewport_texts(&grid), viewport_before);
    feed_bytes(&mut grid, b"\x1bM");

    assert_eq!((grid.cursor.x, grid.cursor.y), (1, 2));
    assert_eq!(
        viewport_texts(&grid),
        ["AAA", "BBB", "", "CCC", "DDD", "FFF", "GGG"]
    );
    let (chunks, _, _) = grid.read_changes(0, 0);
    assert!(chunks
        .iter()
        .find(|chunk| chunk.y == 2)
        .unwrap()
        .terminal_characters
        .iter()
        .all(|character| {
            character.character == ' '
                && character.styles.background == Some(AnsiCode::ColorIndex(2))
        }));
}

#[test]
fn partial_erase_redraws_only_changed_lines() {
    for (sequence, expected_lines, expected_text) in [
        (b"\x1b[0J", vec![2, 3, 4], ["AAA", "BBB", "C", "", ""]),
        (b"\x1b[1J", vec![0, 1, 2], ["", "", "  C", "DDD", "EEE"]),
    ] {
        let mut grid =
            create_grid_with_size_and_raw(5, 5, b"AAA\r\nBBB\r\nCCC\r\nDDD\r\nEEE\x1b[3;2H");
        grid.read_changes(0, 0);

        feed_bytes(&mut grid, sequence);

        let (chunks, _, _) = grid.read_changes(0, 0);
        assert_eq!(
            chunks.iter().map(|chunk| chunk.y).collect::<Vec<_>>(),
            expected_lines
        );
        assert_eq!(viewport_texts(&grid), expected_text);
    }
}

#[test]
fn partial_scroll_region_newline_transfers_to_scrollback() {
    let mut content: Vec<u8> = Vec::new();
    content.extend_from_slice(PARTIAL_SR);
    content.extend_from_slice(FILL_8_LINES);
    content.extend_from_slice(b"\r\nIII\r\nJJJ");

    let grid = create_grid_with_size_and_raw(10, 40, &content);

    assert_eq!(scrollback_texts(&grid), vec!["AAA", "BBB"]);
    let vp = viewport_texts(&grid);
    assert_eq!(vp[0], "CCC");
    assert_eq!(vp[1], "DDD");
    assert_eq!(vp[6], "III");
    assert_eq!(vp[7], "JJJ");
}

#[test]
fn partial_scroll_region_csi_s_transfers_to_scrollback() {
    let mut content: Vec<u8> = Vec::new();
    content.extend_from_slice(PARTIAL_SR);
    content.extend_from_slice(FILL_8_LINES);
    content.extend_from_slice(b"\x1b[3S");

    let grid = create_grid_with_size_and_raw(10, 40, &content);

    assert_eq!(scrollback_texts(&grid), vec!["AAA", "BBB", "CCC"]);
    let vp = viewport_texts(&grid);
    assert_eq!(vp[0], "DDD");
    assert_eq!(vp[4], "HHH");
    assert_eq!(vp[5], "");
    assert_eq!(vp[7], "");
}

#[test]
fn partial_scroll_region_csi_m_at_top_transfers_to_scrollback() {
    let mut content: Vec<u8> = Vec::new();
    content.extend_from_slice(PARTIAL_SR);
    content.extend_from_slice(FILL_8_LINES);
    content.extend_from_slice(b"\x1b[1;1H\x1b[2M");

    let grid = create_grid_with_size_and_raw(10, 40, &content);

    assert_eq!(scrollback_texts(&grid), vec!["AAA", "BBB"]);
    let vp = viewport_texts(&grid);
    assert_eq!(vp[0], "CCC");
    assert_eq!(vp[5], "HHH");
    assert_eq!(vp[6], "");
    assert_eq!(vp[7], "");
}

#[test]
fn partial_scroll_region_csi_m_mid_region_does_not_transfer() {
    let mut content: Vec<u8> = Vec::new();
    content.extend_from_slice(PARTIAL_SR);
    content.extend_from_slice(FILL_8_LINES);
    // Cursor to row 3, delete 2 lines
    content.extend_from_slice(b"\x1b[4;1H\x1b[2M");

    let grid = create_grid_with_size_and_raw(10, 40, &content);

    assert_eq!(grid.lines_above.len(), 0);
    let vp = viewport_texts(&grid);
    assert_eq!(vp[0], "AAA");
    assert_eq!(vp[2], "CCC");
    assert_eq!(vp[3], "FFF");
    assert_eq!(vp[5], "HHH");
    assert_eq!(vp[6], "");
    assert_eq!(vp[7], "");
}

#[test]
fn partial_scroll_region_does_not_transfer_on_alternate_screen() {
    let mut content: Vec<u8> = Vec::new();
    content.extend_from_slice(b"\x1b[?1049h");
    content.extend_from_slice(PARTIAL_SR);
    content.extend_from_slice(FILL_8_LINES);
    content.extend_from_slice(b"\x1b[3S");

    let grid = create_grid_with_size_and_raw(10, 40, &content);

    assert_eq!(grid.lines_above.len(), 0);
    let vp = viewport_texts(&grid);
    assert_eq!(vp[0], "DDD");
    assert_eq!(vp[4], "HHH");
}

#[test]
fn partial_scroll_region_nonzero_top_does_not_transfer() {
    let mut content: Vec<u8> = Vec::new();
    content.extend_from_slice(b"AAA\r\nBBB\r\nCCC\r\nDDD\r\nEEE\r\nFFF");
    // Scroll region rows 3-6 (1-based), so top is row 2, not 0
    content.extend_from_slice(b"\x1b[3;6r");
    content.extend_from_slice(b"\x1b[2S");

    let grid = create_grid_with_size_and_raw(10, 40, &content);

    assert_eq!(grid.lines_above.len(), 0);
    let vp = viewport_texts(&grid);
    assert_eq!(vp[0], "AAA");
    assert_eq!(vp[1], "BBB");
    assert_eq!(vp[2], "EEE");
    assert_eq!(vp[3], "FFF");
    assert_eq!(vp[4], "");
    assert_eq!(vp[5], "");
}

#[test]
fn partial_scroll_region_selection_adjusted_on_newline() {
    let mut grid = create_grid_with_size_and_raw(10, 40, FILL_8_LINES);
    feed_bytes(&mut grid, PARTIAL_SR);

    let pos = Position::new(3, 5);
    grid.start_selection(&pos);
    grid.end_selection(&pos);
    let start_before = grid.selection.start.line.get();
    let end_before = grid.selection.end.line.get();

    feed_bytes(&mut grid, b"\x1b[8;1H\r\nnew line");

    assert_eq!(grid.selection.start.line.get(), start_before - 1);
    assert_eq!(grid.selection.end.line.get(), end_before - 1);
}

#[test]
fn partial_scroll_region_selection_adjusted_on_csi_s() {
    let mut grid = create_grid_with_size_and_raw(10, 40, FILL_8_LINES);
    feed_bytes(&mut grid, PARTIAL_SR);

    let pos = Position::new(4, 2);
    grid.start_selection(&pos);
    grid.end_selection(&pos);
    let start_before = grid.selection.start.line.get();
    let end_before = grid.selection.end.line.get();

    feed_bytes(&mut grid, b"\x1b[2S");

    assert_eq!(grid.selection.start.line.get(), start_before - 2);
    assert_eq!(grid.selection.end.line.get(), end_before - 2);
}

#[test]
fn partial_scroll_region_selection_adjusted_on_csi_m() {
    let mut grid = create_grid_with_size_and_raw(10, 40, FILL_8_LINES);
    feed_bytes(&mut grid, PARTIAL_SR);

    let pos = Position::new(5, 2);
    grid.start_selection(&pos);
    grid.end_selection(&pos);
    let start_before = grid.selection.start.line.get();
    let end_before = grid.selection.end.line.get();

    feed_bytes(&mut grid, b"\x1b[3M");

    assert_eq!(grid.selection.start.line.get(), start_before - 3);
    assert_eq!(grid.selection.end.line.get(), end_before - 3);
}

#[test]
fn partial_scroll_region_all_three_mechanisms_transfer_in_order() {
    let mut content: Vec<u8> = Vec::new();
    content.extend_from_slice(PARTIAL_SR);
    content.extend_from_slice(FILL_8_LINES);
    // Newline scrolls AAA off, CSI S scrolls BBB off, CSI M scrolls CCC off
    content.extend_from_slice(b"\r\nIII");
    content.extend_from_slice(b"\x1b[1S");
    content.extend_from_slice(b"\x1b[1;1H\x1b[1M");

    let grid = create_grid_with_size_and_raw(10, 40, &content);

    assert_eq!(scrollback_texts(&grid), vec!["AAA", "BBB", "CCC"]);
}

#[test]
fn wrap_at_bottom_of_partial_scroll_region_transfers_to_scrollback() {
    // A long line that soft-wraps while the cursor is on the scroll region's
    // bottom row must scroll the region, exactly like a newline there would
    let mut content: Vec<u8> = Vec::new();
    content.extend_from_slice(PARTIAL_SR);
    content.extend_from_slice(FILL_8_LINES);
    content.extend_from_slice(b"\r\n");
    content.extend_from_slice(&[b'I'; 90]); // 90 cols on a 40-col grid: wraps twice

    let grid = create_grid_with_size_and_raw(10, 40, &content);

    // the newline scrolls AAA off, the two wraps scroll BBB and CCC off
    assert_eq!(scrollback_texts(&grid), vec!["AAA", "BBB", "CCC"]);
    let vp = viewport_texts(&grid);
    assert_eq!(vp[0], "DDD");
    assert_eq!(vp[4], "HHH");
    assert_eq!(vp[5], "I".repeat(40));
    assert_eq!(vp[6], "I".repeat(40));
    assert_eq!(vp[7], "I".repeat(10));
    assert_eq!(vp.len(), 8, "rows below the region must be untouched");
    // the continuation rows are wrapped rows, so resize re-wraps them correctly
    assert!(grid.viewport[5].is_canonical);
    assert!(!grid.viewport[6].is_canonical);
    assert!(!grid.viewport[7].is_canonical);
}

#[test]
fn wrap_at_bottom_of_partial_scroll_region_keeps_cursor_inside_region() {
    // Before the fix, the cursor would escape below the region's bottom margin
    // on wrap, and no subsequent line could ever scroll into scrollback again
    let mut content: Vec<u8> = Vec::new();
    content.extend_from_slice(PARTIAL_SR);
    content.extend_from_slice(FILL_8_LINES);
    content.extend_from_slice(b"\r\n");
    content.extend_from_slice(&[b'I'; 90]);

    let grid = create_grid_with_size_and_raw(10, 40, &content);

    // region is rows 0-7; after two wraps the cursor must still be on row 7
    assert_eq!(
        grid.cursor_coordinates().map(|(x, y, _)| (x, y)),
        Some((10, 7))
    );
}

#[test]
fn wrap_at_bottom_of_nonzero_top_scroll_region_does_not_transfer() {
    let mut content: Vec<u8> = Vec::new();
    content.extend_from_slice(b"AAA\r\nBBB\r\nCCC\r\nDDD\r\nEEE\r\nFFF");
    // Scroll region rows 3-6 (1-based), so top is row 2, not 0
    content.extend_from_slice(b"\x1b[3;6r");
    content.extend_from_slice(b"\x1b[6;1H");
    content.extend_from_slice(&[b'X'; 50]); // wraps once at the region's bottom

    let grid = create_grid_with_size_and_raw(10, 40, &content);

    // regions not anchored at the top do not preserve scrolled-off lines
    assert_eq!(grid.lines_above.len(), 0);
    let vp = viewport_texts(&grid);
    assert_eq!(vp[0], "AAA", "rows above the region must be untouched");
    assert_eq!(vp[1], "BBB", "rows above the region must be untouched");
    assert_eq!(vp[2], "DDD", "the region's top row (CCC) is discarded");
    assert_eq!(vp[4], "X".repeat(40));
    assert_eq!(vp[5], "X".repeat(10));
}

#[test]
fn wrap_at_bottom_of_partial_scroll_region_does_not_transfer_on_alternate_screen() {
    let mut content: Vec<u8> = Vec::new();
    content.extend_from_slice(b"\x1b[?1049h");
    content.extend_from_slice(PARTIAL_SR);
    content.extend_from_slice(FILL_8_LINES);
    content.extend_from_slice(b"\r\n");
    content.extend_from_slice(&[b'I'; 90]);

    let grid = create_grid_with_size_and_raw(10, 40, &content);

    assert_eq!(grid.lines_above.len(), 0);
    let vp = viewport_texts(&grid);
    assert_eq!(vp[5], "I".repeat(40));
    assert_eq!(vp[6], "I".repeat(40));
    assert_eq!(vp[7], "I".repeat(10));
}

#[test]
fn wrap_at_bottom_of_explicit_full_screen_scroll_region_keeps_wrap_flag() {
    // CSI 1;10r on a 10-row grid covers the whole screen; wrap behavior must
    // be identical to having no region set at all (scroll + wrapped row)
    let mut content: Vec<u8> = Vec::new();
    content.extend_from_slice(b"\x1b[1;10r");
    for i in 1..=10u8 {
        if i > 1 {
            content.extend_from_slice(b"\r\n");
        }
        content.extend_from_slice(b"EARLY-");
        content.push(b'0' + i / 10);
        content.push(b'0' + i % 10);
    }
    content.extend_from_slice(b"\r\n");
    content.extend_from_slice(&[b'I'; 90]); // wraps twice on a 40-col grid

    let grid = create_grid_with_size_and_raw(10, 40, &content);

    // the newline and the two wraps each scrolled one line into scrollback
    assert_eq!(
        scrollback_texts(&grid),
        vec!["EARLY-01", "EARLY-02", "EARLY-03"]
    );
    let vp = viewport_texts(&grid);
    assert_eq!(vp[6], "EARLY-10");
    assert_eq!(vp[7], "I".repeat(40));
    assert_eq!(vp[8], "I".repeat(40));
    assert_eq!(vp[9], "I".repeat(10));
    assert!(grid.viewport[7].is_canonical);
    assert!(
        !grid.viewport[8].is_canonical,
        "continuation must stay a wrapped row"
    );
    assert!(
        !grid.viewport[9].is_canonical,
        "continuation must stay a wrapped row"
    );
}

#[test]
fn wrap_at_bottom_of_nonzero_top_scroll_region_keeps_hyperlink_tracking() {
    // A plain-text URL that wraps at the bottom of a region not anchored at
    // the top: the auto-linkifier's tracked positions must follow the rows
    // as the region scrolls, or the URL loses its clickable anchors
    let mut content: Vec<u8> = Vec::new();
    content.extend_from_slice(b"AAA\r\nBBB\r\nCCC\r\nDDD\r\nEEE\r\nFFF");
    content.extend_from_slice(b"\x1b[3;6r\x1b[6;1H");
    content.extend_from_slice(b"https://example.com/");
    content.extend_from_slice(&[b'a'; 30]); // 50 chars: wraps at the region bottom
    content.extend_from_slice(b" ");

    let grid = create_grid_with_size_and_raw(10, 40, &content);

    let start_anchors_per_row: Vec<usize> = grid
        .viewport
        .iter()
        .map(|row| {
            row.columns
                .iter()
                .filter(|c| {
                    matches!(
                        c.styles.link_anchor,
                        Some(crate::panes::terminal_character::LinkAnchor::Start(_))
                    )
                })
                .count()
        })
        .collect();

    // rows 0-3: AAA, BBB, DDD, EEE (CCC scrolled off and discarded) - no anchors;
    // row 4 holds the URL's first 40 columns, row 5 the wrapped remainder
    assert_eq!(start_anchors_per_row, vec![0, 0, 0, 0, 40, 10]);
}

#[test]
fn scroll_region_newline_sets_bg_color_on_new_row() {
    // Set bg color, set scroll region 1-5, fill 5 lines, then newline to scroll
    let content = b"\x1b[48;2;26;26;26m\x1b[1;5r\
        AAA\r\nBBB\r\nCCC\r\nDDD\r\nEEE\r\nFFF";
    let grid = create_grid_with_size_and_raw(10, 40, content);
    // The new row at the bottom of the scroll region (row index 4) should have bg_color
    let new_row = &grid.viewport[4];
    assert_eq!(
        new_row.bg_color,
        Some(AnsiCode::RgbCode((26, 26, 26))),
        "scroll-created row should carry the cursor background color"
    );
}

#[test]
fn scroll_region_newline_bg_color_used_for_trailing_padding() {
    // Set bg, scroll region 1-5, fill lines, scroll, then write short text on new row
    let content = b"\x1b[48;2;26;26;26m\x1b[1;5rAAA\r\nBBB\r\nCCC\r\nDDD\r\nEEE\r\nhi";
    let mut grid = create_grid_with_size_and_raw(10, 40, content);
    // read_changes returns character chunks with padding applied
    let (chunks, _, _) = grid.read_changes(0, 0);
    // Find the chunk for row 4 (the scroll-created row with "hi")
    let row_4_chunk = chunks.iter().find(|c| c.y == 4).expect("row 4 chunk");
    // The trailing padding character (last column) should have the row's bg_color
    let pad_char = &row_4_chunk.terminal_characters[39];
    assert_eq!(
        pad_char.styles.background,
        Some(AnsiCode::RgbCode((26, 26, 26))),
        "trailing padding should use the row's background color"
    );
}

#[test]
fn scroll_region_newline_bg_color_used_for_cursor_forward_gaps() {
    // Set bg, scroll region 1-5, fill lines, scroll, then cursor-forward and write
    // This simulates vim's [12C behavior on a scroll-created row
    let content = b"\x1b[48;2;26;26;26m\x1b[1;5rAAA\r\nBBB\r\nCCC\r\nDDD\r\nEEE\
        \r\n\x1b[0m\x1b[10Cx";
    let grid = create_grid_with_size_and_raw(10, 40, content);
    let new_row = &grid.viewport[4];
    // Position 5 (within the gap created by cursor forward) should have the bg_color
    let gap_char = &new_row.columns[5];
    assert_eq!(
        gap_char.styles.background,
        Some(AnsiCode::RgbCode((26, 26, 26))),
        "cursor-forward gap should use the row's background color"
    );
}

#[test]
fn scroll_region_bg_color_does_not_override_explicit_background() {
    // Set bg, scroll region 1-5, fill lines, scroll, then write with different bg
    let content = b"\x1b[48;2;26;26;26m\x1b[1;5rAAA\r\nBBB\r\nCCC\r\nDDD\r\nEEE\
        \r\n\x1b[48;2;255;0;0mRED";
    let grid = create_grid_with_size_and_raw(10, 40, content);
    let new_row = &grid.viewport[4];
    // The 'R' character should have the explicitly set red background, not the row bg
    let r_char = &new_row.columns[0];
    assert_eq!(
        r_char.styles.background,
        Some(AnsiCode::RgbCode((255, 0, 0))),
        "explicitly set background should not be overridden by row bg_color"
    );
}

#[test]
fn full_scroll_region_newline_sets_bg_color_on_new_row() {
    // Full scroll region (entire viewport), set bg, fill, then scroll
    let content = b"\x1b[48;2;26;26;26m\x1b[1;10r\
        L1\r\nL2\r\nL3\r\nL4\r\nL5\r\nL6\r\nL7\r\nL8\r\nL9\r\nL10\r\nL11";
    let grid = create_grid_with_size_and_raw(10, 40, content);
    // The new row at the bottom (row 9) should have bg_color
    let new_row = &grid.viewport[9];
    assert_eq!(
        new_row.bg_color,
        Some(AnsiCode::RgbCode((26, 26, 26))),
        "full scroll region: new row should carry the cursor background color"
    );
}

#[test]
fn row_without_scroll_has_no_bg_color() {
    // Normal content without scroll should not set bg_color on rows
    let content = b"\x1b[48;2;26;26;26mHello";
    let grid = create_grid_with_size_and_raw(10, 40, content);
    let row = &grid.viewport[0];
    assert_eq!(
        row.bg_color, None,
        "rows not created by scroll should have no bg_color"
    );
}

#[test]
fn csi_14t_forwards_to_host_not_local() {
    // CSI 14t used to synthesize a local "\x1b[4;H;Wt" reply; after the
    // refactor it must be forwarded to the host instead so apps observe
    // the terminal's real window pixel dimensions.
    let mut parser = vte::Parser::new();
    let mut grid = new_grid_for_forwarding_test();
    parser.advance(&mut grid, b"\x1b[14t");
    assert!(
        grid.pending_messages_to_pty.is_empty(),
        "local reply path must not fire for 14t"
    );
    assert_eq!(grid.pending_forwarded_queries.len(), 1);
    assert_eq!(
        grid.pending_forwarded_queries[0],
        crate::host_query::HostQuery::TextAreaPixelSize,
        "forwarded classification must be TextAreaPixelSize"
    );
}

#[test]
fn csi_16t_forwards_to_host_not_local() {
    let mut parser = vte::Parser::new();
    let mut grid = new_grid_for_forwarding_test();
    parser.advance(&mut grid, b"\x1b[16t");
    assert!(grid.pending_messages_to_pty.is_empty());
    assert_eq!(grid.pending_forwarded_queries.len(), 1);
    assert_eq!(
        grid.pending_forwarded_queries[0],
        crate::host_query::HostQuery::CharacterCellPixelSize,
    );
}

#[test]
fn csi_18t_still_answered_locally() {
    // 18 reports Zellij's own text-area size in cells — Zellij is
    // authoritative for this, do NOT forward.
    let mut parser = vte::Parser::new();
    let mut grid = new_grid_for_forwarding_test();
    parser.advance(&mut grid, b"\x1b[18t");
    assert_eq!(grid.pending_messages_to_pty.len(), 1);
    assert!(grid.pending_forwarded_queries.is_empty());
}

#[test]
fn osc_11_set_stays_local() {
    // OSC 11;<rgb> (set pane default bg) must stay local — Zellij needs
    // to track it for its own rendering.
    let mut parser = vte::Parser::new();
    let mut grid = new_grid_for_forwarding_test();
    parser.advance(&mut grid, b"\x1b]11;rgb:ffff/ffff/ffff\x07");
    assert!(grid.pending_messages_to_pty.is_empty());
    assert!(
        grid.pending_forwarded_queries.is_empty(),
        "set (not query) must not forward"
    );
    assert!(grid.pane_default_bg.is_some());
}

#[test]
fn osc_11_query_without_override_forwards_to_host() {
    // When no pane-local override is in place the query must still be
    // forwarded — the host's actual bg is what the app asked for.
    let mut parser = vte::Parser::new();
    let mut grid = new_grid_for_forwarding_test();
    assert!(grid.pane_default_bg.is_none());
    parser.advance(&mut grid, b"\x1b]11;?\x07");
    assert!(
        grid.pending_messages_to_pty.is_empty(),
        "no override → no local reply"
    );
    assert_eq!(grid.pending_forwarded_queries.len(), 1);
    assert!(matches!(
        grid.pending_forwarded_queries[0],
        crate::host_query::HostQuery::DefaultBackground { .. }
    ));
}

#[test]
fn osc_10_query_without_override_forwards_to_host() {
    let mut parser = vte::Parser::new();
    let mut grid = new_grid_for_forwarding_test();
    assert!(grid.pane_default_fg.is_none());
    parser.advance(&mut grid, b"\x1b]10;?\x07");
    assert!(grid.pending_messages_to_pty.is_empty());
    assert_eq!(grid.pending_forwarded_queries.len(), 1);
    assert!(matches!(
        grid.pending_forwarded_queries[0],
        crate::host_query::HostQuery::DefaultForeground { .. }
    ));
}

#[test]
fn set_pane_default_colors_short_circuits_osc_queries() {
    // The CLI path (`zellij action set-pane-color`) lands on
    // `Grid::set_pane_default_colors`. A later OSC 10/11 query must
    // read that override, not be forwarded — the entire point of the
    // CLI override is that apps see the color Zellij is painting, not
    // the underlying host's.
    let mut parser = vte::Parser::new();
    let mut grid = new_grid_for_forwarding_test();
    grid.set_pane_default_colors(Some("#ff8040".to_string()), Some("#102030".to_string()));

    parser.advance(&mut grid, b"\x1b]10;?\x07");
    parser.advance(&mut grid, b"\x1b]11;?\x07");

    assert!(
        grid.pending_forwarded_queries.is_empty(),
        "overrides set via set_pane_default_colors must suppress forwarding"
    );
    assert_eq!(grid.pending_messages_to_pty.len(), 2);
    let fg_reply = String::from_utf8(grid.pending_messages_to_pty[0].clone()).unwrap();
    let bg_reply = String::from_utf8(grid.pending_messages_to_pty[1].clone()).unwrap();
    assert_eq!(fg_reply, "\u{1b}]10;rgb:ffff/8080/4040\u{7}");
    assert_eq!(bg_reply, "\u{1b}]11;rgb:1010/2020/3030\u{7}");
}

#[test]
fn osc_11_override_short_circuits_only_the_overridden_channel() {
    // Setting only the bg must not short-circuit fg queries — each
    // channel's override is independent. If only `pane_default_bg` is
    // populated, an OSC 10 query (fg) still goes to the host.
    let mut parser = vte::Parser::new();
    let mut grid = new_grid_for_forwarding_test();
    parser.advance(&mut grid, b"\x1b]11;rgb:1010/2020/3030\x07");
    assert!(grid.pane_default_bg.is_some());
    assert!(grid.pane_default_fg.is_none());

    parser.advance(&mut grid, b"\x1b]10;?\x07");
    assert!(
        grid.pending_messages_to_pty.is_empty(),
        "fg has no override → must forward"
    );
    assert_eq!(grid.pending_forwarded_queries.len(), 1);
}

#[test]
fn csi_2026_dollar_p_stays_local() {
    // DECRQM mode 2026 (synchronised output) is emulated by Zellij
    // itself — never forwarded. The response is the DECRPM form
    // `\x1b[?2026;2$y` (2 = "reset but recognised"; Zellij brackets its
    // own frames, so individual panes are treated as "not enabled").
    let mut parser = vte::Parser::new();
    let mut grid = new_grid_for_forwarding_test();
    parser.advance(&mut grid, b"\x1b[?2026$p");
    assert!(
        grid.pending_forwarded_queries.is_empty(),
        "CSI ?2026$p is emulated locally, must not forward"
    );
    assert_eq!(grid.pending_messages_to_pty.len(), 1);
    assert_eq!(
        grid.pending_messages_to_pty[0], b"\x1b[?2026;2$y",
        "DECRPM reply must use the `$y` suffix per spec"
    );
}

#[test]
fn csi_2031_dollar_p_when_disabled_replies_reset() {
    // DECRQM mode 2031 (Application Theme Reporting) is per-pane state
    // tracked locally by Zellij. With no prior `CSI ? 2031 h`, the mode
    // is reset, so the DECRPM reply must report value=2.
    let mut parser = vte::Parser::new();
    let mut grid = new_grid_for_forwarding_test();
    parser.advance(&mut grid, b"\x1b[?2031$p");
    assert!(
        grid.pending_forwarded_queries.is_empty(),
        "CSI ?2031$p is answered locally, must not forward"
    );
    assert_eq!(grid.pending_messages_to_pty.len(), 1);
    assert_eq!(
        grid.pending_messages_to_pty[0], b"\x1b[?2031;2$y",
        "DECRPM reply must report value=2 (reset) when 2031 is disabled"
    );
}

#[test]
fn csi_2031_dollar_p_when_enabled_replies_set() {
    // After `CSI ? 2031 h` enables theme-change notifications on this
    // pane, a DECRQM probe must report value=1 (set).
    let mut parser = vte::Parser::new();
    let mut grid = new_grid_for_forwarding_test();
    parser.advance(&mut grid, b"\x1b[?2031h\x1b[?2031$p");
    assert!(
        grid.pending_forwarded_queries.is_empty(),
        "CSI ?2031$p is answered locally, must not forward"
    );
    assert_eq!(grid.pending_messages_to_pty.len(), 1);
    assert_eq!(
        grid.pending_messages_to_pty[0], b"\x1b[?2031;1$y",
        "DECRPM reply must report value=1 (set) when 2031 is enabled"
    );
}

#[test]
fn csi_22t_and_23t_stay_local() {
    // CSI 22t / 23t manipulate the pane's title stack — Zellij owns
    // that state, so forwarding would route an app's push/pop to the
    // host's unrelated title stack instead of the visible pane title.
    // They produce no reply; both queues must stay empty after each.
    let mut parser = vte::Parser::new();
    let mut grid = new_grid_for_forwarding_test();
    grid.set_title("hello".to_string());

    parser.advance(&mut grid, b"\x1b[22;0t");
    assert!(
        grid.pending_forwarded_queries.is_empty(),
        "22t is a stack op, not a query"
    );
    assert!(
        grid.pending_messages_to_pty.is_empty(),
        "22t produces no reply"
    );

    // The push actually landed on Zellij's per-pane stack: restore it.
    grid.set_title("different".to_string());
    parser.advance(&mut grid, b"\x1b[23;0t");
    assert!(
        grid.pending_forwarded_queries.is_empty(),
        "23t is a stack op, not a query"
    );
    assert!(
        grid.pending_messages_to_pty.is_empty(),
        "23t produces no reply"
    );
    assert_eq!(
        grid.title.as_deref(),
        Some("hello"),
        "pop should restore the previously-pushed title"
    );
}

#[test]
fn osc_4_set_stays_local() {
    // OSC 4;<index>;<rgb> writes to Zellij's in-memory palette; only
    // the query form (`OSC 4;N;?`) ever forwards to the host.
    let mut parser = vte::Parser::new();
    let mut grid = new_grid_for_forwarding_test();
    parser.advance(&mut grid, b"\x1b]4;5;rgb:ffff/0000/0000\x07");
    assert!(
        grid.pending_forwarded_queries.is_empty(),
        "OSC 4 set must not forward"
    );
    assert!(
        grid.pending_messages_to_pty.is_empty(),
        "OSC 4 set produces no reply"
    );
    let changed = grid
        .changed_colors
        .expect("changed_colors should be populated by OSC 4 set");
    assert!(changed[5].is_some(), "index 5 should have been written");
}

#[test]
fn decset_2031_enables_color_palette_notification() {
    let mut parser = vte::Parser::new();
    let mut grid = new_grid_for_forwarding_test();
    assert!(
        !grid.color_palette_notification_enabled,
        "default state must be disabled"
    );
    parser.advance(&mut grid, b"\x1b[?2031h");
    assert!(
        grid.color_palette_notification_enabled,
        "DECSET 2031 must enable the flag"
    );
    parser.advance(&mut grid, b"\x1b[?2031l");
    assert!(
        !grid.color_palette_notification_enabled,
        "DECRST 2031 must disable the flag"
    );
}

#[test]
fn push_color_palette_dsr_emits_when_enabled() {
    let mut grid = new_grid_for_forwarding_test();
    grid.color_palette_notification_enabled = true;
    grid.push_color_palette_dsr(zellij_types::style::HostTerminalThemeMode::Dark);
    assert_eq!(grid.pending_messages_to_pty.len(), 1);
    assert_eq!(
        grid.pending_messages_to_pty[0],
        b"\x1b[?997;1n".to_vec(),
        "Dark mode emits ?997;1n"
    );
    grid.push_color_palette_dsr(zellij_types::style::HostTerminalThemeMode::Light);
    assert_eq!(grid.pending_messages_to_pty.len(), 2);
    assert_eq!(
        grid.pending_messages_to_pty[1],
        b"\x1b[?997;2n".to_vec(),
        "Light mode emits ?997;2n"
    );
}

#[test]
fn push_color_palette_dsr_noop_when_disabled() {
    let mut grid = new_grid_for_forwarding_test();
    assert!(!grid.color_palette_notification_enabled);
    grid.push_color_palette_dsr(zellij_types::style::HostTerminalThemeMode::Dark);
    assert!(
        grid.pending_messages_to_pty.is_empty(),
        "no DSR is queued when the app has not opted in via CSI ?2031h"
    );
}

#[test]
fn csi_996n_pushes_color_palette_mode_query_to_forwarded_queries() {
    use crate::host_query::HostQuery;
    let mut parser = vte::Parser::new();
    let mut grid = new_grid_for_forwarding_test();
    parser.advance(&mut grid, b"\x1b[?996n");
    assert_eq!(
        grid.pending_forwarded_queries,
        vec![HostQuery::ColorPaletteMode],
        "CSI ?996n must push HostQuery::ColorPaletteMode for Screen to short-circuit"
    );
    assert!(
        grid.pending_messages_to_pty.is_empty(),
        "Grid must NOT answer locally — Screen owns the host_terminal_theme_mode cache"
    );
}

#[test]
fn csi_5n_status_query_still_handled_locally() {
    let mut parser = vte::Parser::new();
    let mut grid = new_grid_for_forwarding_test();
    // Plain DSR 5 (no `?` intermediate) is not the new theme query and
    // must continue to receive its `\e[0n` "all good" reply locally.
    parser.advance(&mut grid, b"\x1b[5n");
    assert!(
        grid.pending_forwarded_queries.is_empty(),
        "DSR 5 is not a forwarded query"
    );
    assert_eq!(
        grid.pending_messages_to_pty,
        vec![b"\x1b[0n".to_vec()],
        "DSR 5 must still produce its local 'all good' reply"
    );
}

#[test]
fn nested_session_frame_is_staged_for_dispatch() {
    let mut vte_parser = vte::Parser::new();
    let mut grid = new_grid_for_nested_frames();
    let message = nested_announce_message();
    vte_parser.advance(
        &mut grid,
        &zellij_ipc::nested_session::encode_frame(&message),
    );
    assert_eq!(grid.pending_nested_session_messages, vec![message]);
}

#[test]
fn nested_session_frame_split_across_feeds_is_staged() {
    let mut vte_parser = vte::Parser::new();
    let mut grid = new_grid_for_nested_frames();
    let message = nested_announce_message();
    let frame = zellij_ipc::nested_session::encode_frame(&message);
    let (first_half, second_half) = frame.split_at(frame.len() / 2);
    vte_parser.advance(&mut grid, first_half);
    assert!(grid.pending_nested_session_messages.is_empty());
    vte_parser.advance(&mut grid, second_half);
    assert_eq!(grid.pending_nested_session_messages, vec![message]);
}

#[test]
fn dcs_with_wrong_magic_param_is_not_staged() {
    let mut vte_parser = vte::Parser::new();
    let mut grid = new_grid_for_nested_frames();
    vte_parser.advance(&mut grid, b"\x1bP26662nAAAA\x1b\\");
    assert!(grid.pending_nested_session_messages.is_empty());
}

#[test]
fn dcs_with_intermediates_is_not_staged() {
    let mut vte_parser = vte::Parser::new();
    let mut grid = new_grid_for_nested_frames();
    vte_parser.advance(&mut grid, b"\x1bP26661$nAAAA\x1b\\");
    assert!(grid.pending_nested_session_messages.is_empty());
}

#[test]
fn dcs_n_without_params_is_not_staged() {
    let mut vte_parser = vte::Parser::new();
    let mut grid = new_grid_for_nested_frames();
    vte_parser.advance(&mut grid, b"\x1bPnAAAA\x1b\\");
    assert!(grid.pending_nested_session_messages.is_empty());
}

#[test]
fn nested_session_frame_with_garbage_payload_is_dropped() {
    let mut vte_parser = vte::Parser::new();
    let mut grid = new_grid_for_nested_frames();
    vte_parser.advance(&mut grid, b"\x1bP26661n!!!not-base64!!!\x1b\\");
    assert!(grid.pending_nested_session_messages.is_empty());
}

#[test]
fn disabled_ribbon_renders_italic_non_bold_unselected() {
    use crate::panes::terminal_character::AnsiCode;
    let grid = render_ui_component("ribbon", &format!("d{}", serialize_text_bytes("RES")));
    let letter_styles = styled_cells_of(&grid, 'R');
    assert!(!letter_styles.is_empty());
    for style in letter_styles {
        assert_eq!(style.italic, Some(AnsiCode::On));
        assert_ne!(style.bold, Some(AnsiCode::On));
    }
}

#[test]
fn disabled_nested_list_item_renders_italic_non_bold() {
    use crate::panes::terminal_character::AnsiCode;
    let grid = render_ui_component("nested_list", &format!("d{}", serialize_text_bytes("RES")));
    let letter_styles = styled_cells_of(&grid, 'R');
    assert!(!letter_styles.is_empty());
    for style in letter_styles {
        assert_eq!(style.italic, Some(AnsiCode::On));
        assert_ne!(style.bold, Some(AnsiCode::On));
    }
}

#[test]
fn disabled_text_with_opaque_flag_keeps_a_background() {
    use crate::panes::terminal_character::AnsiCode;
    let grid = render_ui_component("text", &format!("zd{}", serialize_text_bytes("RES")));
    let letter_styles = styled_cells_of(&grid, 'R');
    assert!(!letter_styles.is_empty());
    for style in letter_styles {
        assert_eq!(style.italic, Some(AnsiCode::On));
        assert_ne!(style.bold, Some(AnsiCode::On));
        assert!(style.background.is_some());
    }
}

#[test]
fn disabled_text_renders_italic_non_bold() {
    use crate::panes::terminal_character::AnsiCode;
    let grid = render_ui_component("text", &format!("d{}", serialize_text_bytes("RES")));
    let letter_styles = styled_cells_of(&grid, 'R');
    assert!(!letter_styles.is_empty());
    for style in letter_styles {
        assert_eq!(style.italic, Some(AnsiCode::On));
        assert_ne!(style.bold, Some(AnsiCode::On));
    }
}

#[test]
fn ui_component_flag_prefixes_parse_order_independently() {
    let baseline = render_ui_component("text", &format!("xzd{}", serialize_text_bytes("RES")));
    let baseline_styles = styled_cells_of(&baseline, 'R');
    for prefix in ["xdz", "zxd", "zdx", "dxz", "dzx"] {
        let grid = render_ui_component(
            "text",
            &format!("{}{}", prefix, serialize_text_bytes("RES")),
        );
        assert_eq!(
            styled_cells_of(&grid, 'R'),
            baseline_styles,
            "prefix {prefix} should parse the same flags as xzd"
        );
    }
}

/// A fresh pane's viewport is shorter than the screen, so each newline materialises a row.
/// Nothing scrolls, so those rows are plain cursor moves and take no background: xterm leaves
/// them default after `printf '\e[44m\n\n\n'`. BCE is for erases and for scrolled-in lines.
#[test]
fn newline_past_the_viewport_takes_no_background() {
    let mut grid = create_grid_with_size_and_raw(5, 4, b"");
    grid.read_changes(0, 0);

    feed_bytes(&mut grid, b"\x1b[48;5;4m\n\n\n");

    for y in 1..=3 {
        assert_eq!(grid.viewport[y].bg_color, None, "row {y}");
    }
    let (chunks, _, _) = grid.read_changes(0, 0);
    for y in 1..=3 {
        let chunk = chunks
            .iter()
            .find(|chunk| chunk.y == y)
            .unwrap_or_else(|| panic!("row {y} was not rendered"));
        assert!(
            chunk.terminal_characters.iter().all(|character| {
                character.character == ' '
                    && matches!(character.styles.background, None | Some(AnsiCode::Reset))
            }),
            "row {y}"
        );
    }
}

#[test]
fn a_line_wrapped_past_the_viewport_takes_no_background() {
    let mut grid = create_grid_with_size_and_raw(5, 3, b"");

    feed_bytes(&mut grid, b"\x1b[48;5;4mABCD");

    assert_eq!(grid.viewport.len(), 2);
    assert_eq!(grid.viewport[1].bg_color, None);
    assert_eq!(viewport_texts(&grid), ["ABC", "D"]);
}

/// The wrap that *does* scroll is the one BCE reaches: the line it inserts at the bottom
/// margin takes the pending background, like the scrolling newline does.
#[test]
fn a_line_wrapped_at_the_scroll_region_bottom_takes_the_pending_background() {
    let mut grid = create_grid_with_size_and_raw(2, 3, b"");

    feed_bytes(&mut grid, b"\x1b[48;5;4mABCDEFG");

    assert_eq!(viewport_texts(&grid), ["DEF", "G"]);
    assert_eq!(
        grid.viewport[1].bg_color,
        Some(AnsiCode::ColorIndex(4)),
        "the row the wrap scrolled in"
    );
}
