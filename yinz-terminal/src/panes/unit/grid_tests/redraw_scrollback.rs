use super::*;

#[test]
fn erased_wrapped_rows_redraw_as_separate_scrollback_lines() {
    for redraw in [
        "\x1b[H\x1b[Jfirst\r\nsecond\r\nthird",
        "\x1b[H\x1b[Kfirst\r\n\x1b[Ksecond\r\n\x1b[Kthird",
        "\x1b[H\x1b[2Kfirst\x1b[2;1H\x1b[2Ksecond\x1b[3;1H\x1b[2Kthird",
    ] {
        let mut grid = create_grid_with_size_and_raw(3, 8, b"12345678abcdefghABCDEFGH");
        assert!(!grid.viewport[1].is_canonical);
        assert!(!grid.viewport[2].is_canonical);

        feed_bytes(&mut grid, redraw.as_bytes());
        assert_eq!(grid.dump_screen(true), "first\nsecond\nthird", "{redraw:?}");

        // Once the redrawn rows leave the screen, the history must keep their boundaries.
        feed_bytes(&mut grid, b"\r\nnext\r\nlast\r\n");
        assert_eq!(scrollback_texts(&grid), ["first", "second", "third"]);
        grid.move_viewport_up(3);
        assert_eq!(viewport_texts(&grid), ["first", "second", "third"]);
        grid.move_viewport_down(3);
        grid.change_size(3, 16);
        assert_eq!(grid.dump_screen(true), "first\nsecond\nthird\nnext\nlast\n");
    }
}

#[test]
fn erase_line_only_breaks_the_wrap_after_the_erased_right_edge() {
    for (erase, breaks_wrap) in [
        ("\x1b[1;3H\x1b[K", true),
        ("\x1b[1;3H\x1b[2K", true),
        ("\x1b[1;3H\x1b[1K", false),
        ("\x1b[1;8H\x1b[1K", true),
        ("\x1b[1;3H\x1b[2X", false),
        ("\x1b[1;3H\x1b[6X", true),
        ("\x1b[1;3H\x1b[P", true),
    ] {
        let mut grid = create_grid_with_size_and_raw(4, 8, b"12345678abcdefghABCDEFGH");
        feed_bytes(&mut grid, erase.as_bytes());
        assert_eq!(grid.viewport[1].is_canonical, breaks_wrap, "{erase:?}");
        assert!(!grid.viewport[2].is_canonical, "{erase:?}");
    }
}

#[test]
fn erase_display_above_breaks_wraps_only_after_erased_right_edges() {
    let mut grid = create_grid_with_size_and_raw(4, 8, b"12345678abcdefghABCDEFGHijklmnop");
    feed_bytes(&mut grid, b"\x1b[3;3H\x1b[1J");
    assert!(grid.viewport[1].is_canonical);
    assert!(grid.viewport[2].is_canonical);
    assert!(!grid.viewport[3].is_canonical);
}

#[test]
fn erase_inside_wrapped_line_keeps_its_prefix_and_allows_fresh_wraps() {
    let mut grid = create_grid_with_size_and_raw(4, 8, b"12345678abcdefghABCDEFGH");
    feed_bytes(&mut grid, b"\x1b[2;3H\x1b[JuvwxyzABCD");
    assert!(!grid.viewport[1].is_canonical);
    assert!(!grid.viewport[2].is_canonical);
    assert_eq!(grid.dump_screen(true), "12345678abuvwxyzABCD");
    grid.change_size(4, 20);
    assert_eq!(grid.dump_screen(true).trim_end(), "12345678abuvwxyzABCD");
}

#[test]
fn breaking_wrap_repaints_highlights_on_the_untouched_continuation() {
    let mut grid = create_grid_with_size_and_raw(4, 8, b"abcdefghijklmnopxyz");
    grid.set_plugin_regex_highlights(
        yinz_types::internal_ids::PluginId::new(1),
        vec![create_highlight(
            "^xyz",
            false,
            true,
            false,
            false,
            yinz_types::data::HighlightLayer::Hint,
        )],
        &Style::default(),
    );
    assert!(grid.plugin_highlight_at(&Position::new(2, 0)).is_none());
    grid.read_changes(0, 0);

    feed_bytes(&mut grid, b"\x1b[2;8H\x1b[K");

    assert!(grid.plugin_highlight_at(&Position::new(2, 0)).is_some());
    let (chunks, _, _) = grid.read_changes(0, 0);
    assert!(chunks.iter().any(|chunk| chunk.y == 2));
}

#[test]
fn erasing_wraps_keeps_images_anchored_to_their_text_after_resize() {
    let (mut grid, _store) = new_kitty_grid(4, 8);
    let mut parser = vte::Parser::new();
    let mut interceptor = KittyApcInterceptor::new();
    feed_bytes(&mut grid, b"12345678abcdefghIMAGE");
    feed_bytes(&mut grid, b"\x1b[3;1H");
    feed_kitty_bytes(
        &mut grid,
        &mut parser,
        &mut interceptor,
        &kitty_apc("a=T,f=24,s=10,v=20,i=1,C=1", &rgb_raster(10, 20)),
    );
    let original_rect = grid.kitty_placements()[0].display_rect;

    // Kitty: "the other commands to erase text must have no effect on graphics".
    feed_bytes(&mut grid, b"\x1b[H\x1b[K\x1b[2;1H\x1b[K");
    assert_eq!(grid.kitty_placements()[0].display_rect, original_rect);
    grid.change_size(4, 16);
    assert_eq!(grid.kitty_placements()[0].display_rect, original_rect);
    assert_eq!(grid.viewport[2].columns[0].character, 'I');
}
