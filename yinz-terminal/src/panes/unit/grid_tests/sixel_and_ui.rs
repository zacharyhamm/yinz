use super::*;

#[test]
fn oversized_sixel_dcs_is_discarded_and_terminal_output_recovers() {
    let mut grid = create_grid_with_content("");
    *grid.character_cell_size.borrow_mut() = Some(SizeInPixels {
        width: 8,
        height: 6,
    });
    feed_bytes(&mut grid, b"\x1bPq\"1;1;1;99999~ignored\x1b\\recovered");
    assert_eq!(grid.sixel_grid.sixel_image_store.borrow().image_count(), 0);
    assert_eq!(viewport_texts(&grid)[0].trim_end(), "recovered");
    feed_bytes(&mut grid, b"\x1bPq~\x1b\\");
    assert_eq!(grid.sixel_grid.sixel_image_store.borrow().image_count(), 1);
    let (_, _, chunks, _) = grid.render(0, 0, &Style::default()).unwrap().unwrap();
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].sixel_image_pixel_width, 1);
    assert_eq!(chunks[0].sixel_image_pixel_height, 6);
}

fn serialize_sixel_chunk(
    store: &mut SixelImageStore,
    chunk: &crate::output::SixelImageChunk,
) -> String {
    store
        .serialize_image(
            chunk.sixel_image_id,
            chunk.sixel_image_pixel_x,
            chunk.sixel_image_pixel_y,
            chunk.sixel_image_pixel_width,
            chunk.sixel_image_pixel_height,
        )
        .unwrap()
}

#[test]
fn locked_repaints_preserve_completed_sixel_pixels_after_incremental_render_and_erasure() {
    let mut grid = create_grid_with_size_and_raw(8, 10, b"");
    *grid.character_cell_size.borrow_mut() = Some(SizeInPixels {
        width: 8,
        height: 6,
    });
    let store = grid.sixel_grid.sixel_image_store.clone();
    feed_bytes(
        &mut grid,
        b"\x1b[8;1H\x1b[2;2H\x1bPq\"1;1;16;12#1;2;100;0;0#1!16~-!16~\x1b\\",
    );
    let original = grid.render(10, 20, &Style::default()).unwrap().unwrap().2;
    assert_eq!(original.len(), 1);
    let original = original[0];
    let full_pixels = serialize_sixel_chunk(&mut store.borrow_mut(), &original);
    let cropped_pixels = store
        .borrow_mut()
        .serialize_image(original.sixel_image_id, 0, 0, 8, 6)
        .unwrap();

    // Only a row below the image is damaged. The completed cache must still include the image.
    feed_bytes(&mut grid, b"\x1b[7;1Hfinished");
    let incremental = grid.render(10, 20, &Style::default()).unwrap().unwrap();
    assert_eq!(incremental.0.len(), 1);
    assert!(incremental.2.is_empty());
    assert_eq!(store.borrow().image_count(), 2);
    feed_bytes(&mut grid, b"\x1b[?2026h\x1b[2;2HX");
    assert!(grid.render(0, 0, &Style::default()).unwrap().is_none());
    let pending_pixels = serialize_sixel_chunk(&mut store.borrow_mut(), &original);
    assert_ne!(
        pending_pixels, full_pixels,
        "the live image has lost pixels under X"
    );

    for (rows, cols, visible_size) in [
        (8, 10, Some((16, 12))),
        (2, 2, Some((8, 6))),
        (8, 1, None),
        (1, 10, None),
        (8, 10, Some((16, 12))),
    ] {
        grid.change_size(rows, cols);
        for (x, y) in [(2, 3), (5, 7)] {
            grid.render_full_viewport();
            let (_, raw, chunks, _) = grid.render(x, y, &Style::default()).unwrap().unwrap();
            assert!(raw.is_none());
            if let Some((width, height)) = visible_size {
                assert_eq!(chunks.len(), 1);
                let chunk = chunks[0];
                assert_eq!((chunk.cell_x, chunk.cell_y), (x + 1, y + 1));
                assert_eq!(
                    (chunk.sixel_image_pixel_x, chunk.sixel_image_pixel_y),
                    (0, 0)
                );
                assert_eq!(
                    (
                        chunk.sixel_image_pixel_width,
                        chunk.sixel_image_pixel_height
                    ),
                    (width, height)
                );
                assert_ne!(chunk.sixel_image_id, original.sixel_image_id);
                assert_eq!(
                    serialize_sixel_chunk(&mut store.borrow_mut(), &chunk),
                    if width == 8 {
                        &cropped_pixels
                    } else {
                        &full_pixels
                    }
                    .as_str(),
                );
            } else {
                assert!(chunks.is_empty());
            }
            assert!(grid.render(x, y, &Style::default()).unwrap().is_none());
        }
    }
    feed_bytes(&mut grid, b"\x1b[?2026l");
    let unlocked = grid.render(0, 0, &Style::default()).unwrap().unwrap().2;
    assert_eq!(unlocked.len(), 1);
    assert_eq!(
        serialize_sixel_chunk(&mut store.borrow_mut(), &unlocked[0]),
        pending_pixels
    );
    feed_bytes(&mut grid, b"\x1b[?2026h");
    grid.render_full_viewport();
    let replay = grid.render(0, 0, &Style::default()).unwrap().unwrap().2;
    assert_eq!(
        serialize_sixel_chunk(&mut store.borrow_mut(), &replay[0]),
        pending_pixels
    );
}

#[test]
fn completed_sixel_survives_deletion_and_releases_only_its_own_images() {
    let image = b"\x1bPq\"1;1;16;12#1;2;100;0;0#1!16~-!16~\x1b\\";
    let mut first = create_grid_with_content("");
    let mut second = create_grid_with_content("");
    let store = first.sixel_grid.sixel_image_store.clone();
    second.sixel_grid.sixel_image_store = store.clone();
    for grid in [&mut first, &mut second] {
        *grid.character_cell_size.borrow_mut() = Some(SizeInPixels {
            width: 8,
            height: 6,
        });
        feed_bytes(grid, image);
        grid.render(0, 0, &Style::default()).unwrap().unwrap();
    }
    assert_eq!(store.borrow().image_count(), 4);
    let original = first.completed_sixel[0];
    let pixels = serialize_sixel_chunk(&mut store.borrow_mut(), &original);
    let second_original = second.completed_sixel[0];
    feed_bytes(&mut first, b"\x1b[?2026h\x1b[2J\x1b[3J");
    assert!(first.sixel_grid.image_coordinates().next().is_none());
    for _ in 0..2 {
        first.render_full_viewport();
        let replay = first.render(0, 0, &Style::default()).unwrap().unwrap().2;
        assert_eq!(replay.len(), 1);
        assert_eq!(
            serialize_sixel_chunk(&mut store.borrow_mut(), &replay[0]),
            pixels
        );
    }
    feed_bytes(&mut first, b"\x1b[?2026l");
    assert!(first
        .render(0, 0, &Style::default())
        .unwrap()
        .unwrap()
        .2
        .is_empty());
    assert_eq!(store.borrow().image_count(), 2);
    assert!(store
        .borrow_mut()
        .serialize_image(original.sixel_image_id, 0, 0, 16, 12)
        .is_none());

    feed_bytes(&mut first, image);
    first.render(0, 0, &Style::default()).unwrap().unwrap();
    assert_eq!(store.borrow().image_count(), 4);
    drop(first);
    assert_eq!(store.borrow().image_count(), 2);
    assert_eq!(
        serialize_sixel_chunk(&mut store.borrow_mut(), &second_original),
        pixels
    );
    second.render_full_viewport();
    let remaining = second.render(0, 0, &Style::default()).unwrap().unwrap().2;
    assert_eq!(
        serialize_sixel_chunk(&mut store.borrow_mut(), &remaining[0]),
        pixels
    );
    assert_eq!(store.borrow().image_count(), 2);
    drop(second);
    assert_eq!(store.borrow().image_count(), 0);
}

#[test]
fn deeply_indented_list_items_are_clipped_without_scrolling_the_pane() {
    for indentation in [3, 4, 65_536] {
        let mut grid = create_grid_with_size_and_raw(5, 8, b"original");
        let command = format!(
            "{UI_COMPONENT_DCS_PREFIX}nested_list;0/4/8/1;{}65\x1b\\",
            "|".repeat(indentation)
        );

        feed_bytes(&mut grid, command.as_bytes());

        assert!(grid.lines_above.is_empty());
        assert_eq!(viewport_texts(&grid)[0], "original");
    }

    let mut grid = create_grid_with_size_and_raw(5, 8, b"original");
    let command = format!("{UI_COMPONENT_DCS_PREFIX}nested_list;0/1/8/2;||||65;66\x1b\\");
    feed_bytes(&mut grid, command.as_bytes());
    assert_eq!(viewport_texts(&grid)[0], "original");
    assert_eq!(row_text(&grid.viewport[2]), " > B");
    assert!(grid.lines_above.is_empty());
}

#[test]
fn malformed_unicode_component_indices_do_not_panic() {
    for component in [
        "text;€$65",
        "ribbon;€$65",
        "table;1;1;€$65",
        "nested_list;€$65",
    ] {
        let mut grid = create_grid_with_content("");
        crate::ui::components::UiComponentParser::new(&mut grid, Style::default(), true)
            .parse(component.as_bytes().to_vec())
            .unwrap();
        assert!(grid.dump_screen(false).contains('A'));
    }
}

#[test]
fn invalid_legacy_color_lengths_are_ignored() {
    for length in [0, 1, 2, 4, 5, 7, 8, 10, 11, 13, 24, 51] {
        let mut grid = create_grid_with_content("");
        let color = format!("#{}", "0".repeat(length));
        let commands = format!("\x1b]4;1;{color}\x07\x1b]10;{color}\x07\x1b]11;{color}\x07");
        vte::Parser::new().advance(&mut grid, commands.as_bytes());
        assert!(grid.changed_colors.is_none(), "length: {length}");
        assert!(grid.pane_default_fg.is_none(), "length: {length}");
        assert!(grid.pane_default_bg.is_none(), "length: {length}");
    }
    for (color, expected) in [
        ("#abc", (0xa0, 0xb0, 0xc0)),
        ("#aabbcc", (0xaa, 0xbb, 0xcc)),
        ("#aaabbbccc", (0xaa, 0xbb, 0xcc)),
        ("#aaaabbbbcccc", (0xaa, 0xbb, 0xcc)),
    ] {
        assert_eq!(
            crate::panes::alacritty_functions::xparse_color(color.as_bytes()),
            Some(AnsiCode::RgbCode(expected))
        );
    }
}

#[test]
fn component_coordinates_preserve_terminal_clamping_without_overflow() {
    for coordinates in [
        format!("{}/0//", usize::MAX),
        format!("0/{}//", usize::MAX),
        "80/0//".to_owned(),
        "0/20//".to_owned(),
    ] {
        for component in ["text", "ribbon", "nested_list", "table"] {
            let mut grid = create_grid_with_content("");
            let table_dimensions = if component == "table" { "1;1;" } else { "" };
            let command = format!(
                "{UI_COMPONENT_DCS_PREFIX}{component};{coordinates};{table_dimensions}65\x1b\\"
            );
            vte::Parser::new().advance(&mut grid, command.as_bytes());
            assert!(
                grid.dump_screen(false).contains('A'),
                "{component}: {coordinates}"
            );
        }
    }
    // Existing SDK callers place bottom help at y=rows and rely on the terminal's clamp.
    let grid = create_grid_with_content(&format!(
        "{UI_COMPONENT_DCS_PREFIX}text;0/20/80/20;65\x1b\\"
    ));
    assert_eq!(grid.viewport[19].columns[0].character, 'A');
}

#[test]
fn component_dimensions_are_clipped_before_rendering() {
    for component in ["text", "ribbon", "nested_list", "table"] {
        let mut grid = create_grid_with_content("");
        let table_dimensions = if component == "table" { "1;1;" } else { "" };
        let command = format!(
            "{UI_COMPONENT_DCS_PREFIX}{component};1/1/{0}/{0};{table_dimensions}z65\x1b\\",
            usize::MAX
        );
        vte::Parser::new().advance(&mut grid, command.as_bytes());
        assert_eq!((grid.width, grid.height), (80, 20));
        assert!(
            grid.lines_above.is_empty(),
            "{component} must not scroll the pane"
        );
    }
}

#[test]
fn sixel_images_are_reaped_when_a_pane_exits_or_resets_in_the_alternate_screen() {
    for reset in [false, true] {
        let mut grid = create_grid_with_content("");
        *grid.character_cell_size.borrow_mut() = Some(SizeInPixels {
            width: 8,
            height: 6,
        });
        let store = grid.sixel_grid.sixel_image_store.clone();
        let image = b"\x1bPq\"1;1;16;12#1;2;100;0;0#1!16~-!16~\x1b\\";
        let mut parser = vte::Parser::new();
        parser.advance(&mut grid, image);
        parser.advance(&mut grid, b"\x1b[?1049h");
        parser.advance(&mut grid, image);
        assert_eq!(store.borrow().image_count(), 2);

        if reset {
            grid.reset_terminal_state();
        } else {
            drop(grid);
        }
        assert_eq!(store.borrow().image_count(), 0, "reset: {reset}");
    }
}

#[test]
pub fn sixel_images_are_reaped_when_scrolled_off() {
    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 21,
    })));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut grid = Grid::new(
        51,
        112,
        Rc::new(RefCell::new(Palette::default())),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        character_cell_size,
        sixel_image_store.clone(),
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
    );
    let pane_content = read_fixture("sixel-image-500px.six");
    vte_parser.advance(&mut grid, &pane_content);
    for _ in 0..10_051 {
        // scrollbuffer limit + viewport height
        grid.add_canonical_line();
    }
    let _ = grid.read_changes(0, 0); // we do this because this is where the images are reaped
    assert_eq!(
        sixel_image_store.borrow().image_count(),
        0,
        "all images were deleted from the store"
    );
}

#[test]
pub fn sixel_images_are_reaped_when_resetting() {
    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 21,
    })));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut grid = Grid::new(
        51,
        112,
        Rc::new(RefCell::new(Palette::default())),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        character_cell_size,
        sixel_image_store.clone(),
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
    );
    let pane_content = read_fixture("sixel-image-500px.six");
    vte_parser.advance(&mut grid, &pane_content);
    grid.reset_terminal_state();
    let _ = grid.read_changes(0, 0); // we do this because this is where the images are reaped
    assert_eq!(
        sixel_image_store.borrow().image_count(),
        0,
        "all images were deleted from the store"
    );
}

#[test]
pub fn sixel_image_in_alternate_buffer() {
    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 21,
    })));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut grid = Grid::new(
        30,
        112,
        Rc::new(RefCell::new(Palette::default())),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        character_cell_size,
        sixel_image_store.clone(),
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
    );

    let move_to_alternate_screen = "\u{1b}[?1049h";
    vte_parser.advance(&mut grid, move_to_alternate_screen.as_bytes());

    let pane_content = read_fixture("sixel-image-500px.six");
    vte_parser.advance(&mut grid, &pane_content);
    assert_snapshot!(format!("{:?}", grid)); // should include the image
                                             //
    let move_away_from_alternate_screen = "\u{1b}[?1049l";
    vte_parser.advance(&mut grid, move_away_from_alternate_screen.as_bytes());
    assert_snapshot!(format!("{:?}", grid)); // should note include the image
    assert_eq!(
        sixel_image_store.borrow().image_count(),
        0,
        "all images were deleted from the store when we moved back from alternate screen"
    );
}

#[test]
pub fn sixel_with_image_scrolling_decsdm() {
    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 21,
    })));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut grid = Grid::new(
        30,
        112,
        Rc::new(RefCell::new(Palette::default())),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        character_cell_size,
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
    );

    // enter DECSDM
    let move_to_decsdm = "\u{1b}[?80h";
    vte_parser.advance(&mut grid, move_to_decsdm.as_bytes());

    // write some text
    let mut text_to_fill_pane = String::new();
    for i in 0..10 {
        writeln!(&mut text_to_fill_pane, "\rline {}", i + 1).unwrap();
    }
    vte_parser.advance(&mut grid, text_to_fill_pane.as_bytes());

    // render a sixel image (will appear on the top left and partially cover the text)
    let pane_content = read_fixture("sixel-image-100px.six");
    vte_parser.advance(&mut grid, &pane_content);
    // image should be on the top left corner of the grid
    assert_snapshot!(format!("{:?}", grid));

    // leave DECSDM
    let move_away_from_decsdm = "\u{1b}[?80l";
    vte_parser.advance(&mut grid, move_away_from_decsdm.as_bytes());

    // Go down to the beginning of the next line
    let mut go_down_once = String::new();
    writeln!(&mut go_down_once, "\n\r").unwrap();
    vte_parser.advance(&mut grid, go_down_once.as_bytes());

    // render another sixel image, should appear under the cursor
    let pane_content = read_fixture("sixel-image-100px.six");
    vte_parser.advance(&mut grid, &pane_content);

    // image should appear in cursor position
    assert_snapshot!(format!("{:?}", grid));
}

#[test]
pub fn osc_4_background_query() {
    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut grid = Grid::new(
        51,
        97,
        Rc::new(RefCell::new(Palette::default())),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
    );
    let content = "\u{1b}]10;?\u{1b}\\";
    vte_parser.advance(&mut grid, content.as_bytes());
    // Post-refactor: OSC 10;? is forwarded to the host, not answered
    // from Yinz's cached palette. pending_messages_to_pty must stay
    // empty.
    assert!(grid.pending_messages_to_pty.is_empty());
    let forwarded_string: String = grid
        .pending_forwarded_queries
        .iter()
        .map(|q| String::from_utf8(q.to_query_bytes()).unwrap())
        .collect();
    assert_eq!(forwarded_string, "\u{1b}]10;?\u{1b}\\");
}

#[test]
pub fn osc_4_foreground_query() {
    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut grid = Grid::new(
        51,
        97,
        Rc::new(RefCell::new(Palette::default())),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
    );
    let content = "\u{1b}]11;?\u{1b}\\";
    vte_parser.advance(&mut grid, content.as_bytes());
    assert!(grid.pending_messages_to_pty.is_empty());
    let forwarded_string: String = grid
        .pending_forwarded_queries
        .iter()
        .map(|q| String::from_utf8(q.to_query_bytes()).unwrap())
        .collect();
    assert_eq!(forwarded_string, "\u{1b}]11;?\u{1b}\\");
}

#[test]
pub fn osc_4_color_query() {
    let mut color_codes = HashMap::new();
    color_codes.insert(222, String::from("rgb:ffff/d7d7/8787"));
    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(color_codes));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut grid = Grid::new(
        51,
        97,
        Rc::new(RefCell::new(Palette::default())),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
    );
    let content = "\u{1b}]4;222;?\u{1b}\\";
    vte_parser.advance(&mut grid, content.as_bytes());
    // OSC 4;N;? is forwarded to the host for the real palette value.
    assert!(grid.pending_messages_to_pty.is_empty());
    let forwarded_string: String = grid
        .pending_forwarded_queries
        .iter()
        .map(|q| String::from_utf8(q.to_query_bytes()).unwrap())
        .collect();
    assert_eq!(forwarded_string, "\u{1b}]4;222;?\u{1b}\\");
}

#[test]
pub fn xtsmgraphics_color_register_count() {
    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut grid = Grid::new(
        51,
        97,
        Rc::new(RefCell::new(Palette::default())),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
    );
    let content = "\u{1b}[?1;1;S\u{1b}\\";
    vte_parser.advance(&mut grid, content.as_bytes());
    let message_string = grid
        .pending_messages_to_pty
        .iter()
        .map(|m| String::from_utf8_lossy(m))
        .fold(String::new(), |mut acc, s| {
            acc.push_str(&s);
            acc
        });
    assert_eq!(message_string, "\u{1b}[?1;0;65536S");
}

#[test]
pub fn xtsmgraphics_pixel_graphics_geometry() {
    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 21,
    })));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut grid = Grid::new(
        51,
        97,
        Rc::new(RefCell::new(Palette::default())),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        character_cell_size,
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
    );
    let content = "\u{1b}[?2;1;S\u{1b}\\";
    vte_parser.advance(&mut grid, content.as_bytes());
    let message_string = grid
        .pending_messages_to_pty
        .iter()
        .map(|m| String::from_utf8_lossy(m))
        .fold(String::new(), |mut acc, s| {
            acc.push_str(&s);
            acc
        });
    assert_eq!(message_string, "\u{1b}[?2;0;776;1071S");
}

#[test]
pub fn cursor_hide_persists_through_alternate_screen() {
    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 21,
    })));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut grid = Grid::new(
        30,
        112,
        Rc::new(RefCell::new(Palette::default())),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        character_cell_size,
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
    );

    let hide_cursor = "\u{1b}[?25l";
    vte_parser.advance(&mut grid, hide_cursor.as_bytes());
    assert!(
        matches!(grid.cursor_coordinates(), Some((_, _, false))),
        "Cursor hidden properly"
    );

    let move_to_alternate_screen = "\u{1b}[?1049h";
    vte_parser.advance(&mut grid, move_to_alternate_screen.as_bytes());
    assert!(
        matches!(grid.cursor_coordinates(), Some((_, _, false))),
        "Cursor still hidden in alternate screen"
    );

    let show_cursor = "\u{1b}[?25h";
    vte_parser.advance(&mut grid, show_cursor.as_bytes());
    assert!(
        matches!(grid.cursor_coordinates(), Some((_, _, true))),
        "Cursor shown"
    );

    let move_away_from_alternate_screen = "\u{1b}[?1049l";
    vte_parser.advance(&mut grid, move_away_from_alternate_screen.as_bytes());
    assert!(
        matches!(grid.cursor_coordinates(), Some((_, _, true))),
        "Cursor still shown away from alternate screen"
    );
}

#[test]
fn zero_column_tables_are_discarded_and_output_continues() {
    for params in ["0;0", "0;1;", "0;1;88"] {
        let input = format!(
            "{UI_COMPONENT_DCS_PREFIX}table;{params}\x1b\\after{UI_COMPONENT_DCS_PREFIX}table;1;1;79,75\x1b\\end"
        );
        for chunk_size in [input.len(), 1, 3] {
            let mut grid = create_grid_with_content("");
            let mut parser = vte::Parser::new();
            for chunk in input.as_bytes().chunks(chunk_size) {
                parser.advance(&mut grid, chunk);
            }
            let lines = viewport_texts(&grid);
            assert_eq!(
                lines[0].trim_end(),
                "afterOK",
                "{params}, chunks {chunk_size}"
            );
            assert_eq!(lines[1].trim_end(), "end", "{params}, chunks {chunk_size}");
        }
    }
}

#[test]
fn table_ui_component() {
    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut grid = Grid::new(
        41,
        110,
        Rc::new(RefCell::new(Palette::default())),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
    );
    let fixture_name = "table-ui-component";
    let content = read_fixture(fixture_name);
    vte_parser.advance(&mut grid, &content);
    assert_snapshot!(format!("{:?}", grid));
}

#[test]
fn table_ui_component_with_coordinates() {
    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut grid = Grid::new(
        41,
        110,
        Rc::new(RefCell::new(Palette::default())),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
    );
    let fixture_name = "table-ui-component-with-coordinates";
    let content = read_fixture(fixture_name);
    vte_parser.advance(&mut grid, &content);
    assert_snapshot!(format!("{:?}", grid));
}

#[test]
fn ribbon_ui_component() {
    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut grid = Grid::new(
        41,
        110,
        Rc::new(RefCell::new(Palette::default())),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
    );
    let fixture_name = "ribbon-ui-component";
    let content = read_fixture(fixture_name);
    vte_parser.advance(&mut grid, &content);
    assert_snapshot!(format!("{:?}", grid));
}

#[test]
fn ribbon_ui_component_with_coordinates() {
    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut grid = Grid::new(
        41,
        110,
        Rc::new(RefCell::new(Palette::default())),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
    );
    let fixture_name = "ribbon-ui-component-with-coordinates";
    let content = read_fixture(fixture_name);
    vte_parser.advance(&mut grid, &content);
    assert_snapshot!(format!("{:?}", grid));
}

#[test]
fn nested_list_ui_component() {
    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut grid = Grid::new(
        41,
        120,
        Rc::new(RefCell::new(Palette::default())),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
    );
    let fixture_name = "nested-list-ui-component";
    let content = read_fixture(fixture_name);
    vte_parser.advance(&mut grid, &content);
    assert_snapshot!(format!("{:?}", grid));
}

#[test]
fn nested_list_ui_component_with_coordinates() {
    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut grid = Grid::new(
        41,
        120,
        Rc::new(RefCell::new(Palette::default())),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
    );
    let fixture_name = "nested-list-ui-component-with-coordinates";
    let content = read_fixture(fixture_name);
    vte_parser.advance(&mut grid, &content);
    assert_snapshot!(format!("{:?}", grid));
}

#[test]
fn text_ui_component() {
    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut grid = Grid::new(
        41,
        120,
        Rc::new(RefCell::new(Palette::default())),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
    );
    let fixture_name = "text-ui-component";
    let content = read_fixture(fixture_name);
    vte_parser.advance(&mut grid, &content);
    assert_snapshot!(format!("{:?}", grid));
}

#[test]
fn text_ui_component_with_coordinates() {
    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut grid = Grid::new(
        41,
        120,
        Rc::new(RefCell::new(Palette::default())),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
    );
    let fixture_name = "text-ui-component-with-coordinates";
    let content = read_fixture(fixture_name);
    vte_parser.advance(&mut grid, &content);
    assert_snapshot!(format!("{:?}", grid));
}

#[test]
fn cannot_escape_scroll_region() {
    // this tests a fix for a bug where it would be possible to set the scroll region bounds beyond
    // the pane height, which would then allow a goto instruction beyond the scroll region to scape
    // the pane bounds and render content on other panes
    //
    // what we do here is set the scroll region beyond the terminal bounds (`<ESC>[1;42r` - whereas
    // the terminal is just 41 lines high), and then issue a goto instruction to line 42, one line
    // beyond the pane and scroll region bounds (`<ESC>[42;1H`) and then print text `Hi there!`.
    // This should be printed on the last line (zero indexed 40) of the terminal and not beyond it.
    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut grid = Grid::new(
        41,
        120,
        Rc::new(RefCell::new(Palette::default())),
        terminal_emulator_color_codes,
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Style::default(),
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
    );
    let content = "\u{1b}[1;42r\u{1b}[42;1HHi there!".as_bytes();
    vte_parser.advance(&mut grid, content);
    assert_snapshot!(format!("{:?}", grid));
}

#[test]
fn preserve_background_color_on_resize() {
    use crate::panes::terminal_character::{AnsiCode, EMPTY_TERMINAL_CHARACTER};

    let mut vte_parser = vte::Parser::new();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
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
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
    );

    let mut parse = |s, grid: &mut Grid| {
        vte_parser.advance(&mut *grid, &Vec::from(s));
    };

    // Write text with red background that extends to end of line
    // ESC[41m = red background
    // ESC[K = clear to end of line (fills with current background)
    // ESC[0m = reset
    let content = "test\x1b[41m\x1b[K\x1b[0m";
    parse(content, &mut grid);

    // Check that characters after "test" have red background before resize
    let first_row = &grid.viewport[0];
    let background_char_count_before = first_row
        .columns
        .iter()
        .enumerate()
        .filter(|(i, c)| *i >= 4 && c.styles.background != Some(AnsiCode::Reset))
        .count();
    assert!(
        background_char_count_before > 0,
        "Should have characters with background color before resize"
    );

    // Printed spaces survive, while the uncolored erase padding is trimmed.
    let content2 = "\r\n\rplain text with spaces    \x1b[K";
    parse(content2, &mut grid);

    // Resize the grid
    grid.change_size(10, 30);

    // Check that the background color is preserved after resize
    let first_row = &grid.viewport[0];
    let background_char_count_after = first_row
        .columns
        .iter()
        .enumerate()
        .filter(|(i, c)| *i >= 4 && c.styles.background != Some(AnsiCode::Reset))
        .count();
    assert_eq!(
        background_char_count_before, background_char_count_after,
        "Background colored characters should be preserved after resize"
    );

    // Only the four explicitly printed trailing spaces remain after reflow.
    let second_row = &grid.viewport[1];
    let trailing_spaces = second_row
        .columns
        .iter()
        .rev()
        .take_while(|c| c.character == EMPTY_TERMINAL_CHARACTER.character)
        .count();
    assert_eq!(
        trailing_spaces, 4,
        "Only printed trailing spaces should remain, but found {} trailing spaces",
        trailing_spaces
    );
}

#[test]
fn a_replayed_render_repeats_only_the_lines_the_last_render_read() {
    let rows = |grid: &mut Grid| -> Vec<usize> {
        grid.render(0, 0, &Style::default())
            .unwrap()
            .map(|(chunks, ..)| chunks.iter().map(|chunk| chunk.y).collect())
            .unwrap_or_default()
    };
    let all_rows: Vec<usize> = (0..8).collect();
    let mut grid = create_grid_with_size_and_raw(8, 10, b"");
    assert_eq!(rows(&mut grid), all_rows);
    grid.replay_last_render();
    assert_eq!(rows(&mut grid), all_rows);

    feed_bytes(&mut grid, b"\x1b[3;1Hthird\x1b[5;1Hfifth");
    let read = rows(&mut grid);
    assert!(read.contains(&2) && read.contains(&4), "{read:?}");
    assert!(!read.contains(&7), "{read:?}");
    grid.replay_last_render();
    assert_eq!(rows(&mut grid), read);
    assert_eq!(rows(&mut grid), Vec::<usize>::new());

    // Lines changed since the last render are sent along with the replay.
    feed_bytes(&mut grid, b"\x1b[7;1Hseventh");
    let read = rows(&mut grid);
    assert!(read.contains(&6), "{read:?}");
    feed_bytes(&mut grid, b"\x1b[2;1Hsecond");
    grid.replay_last_render();
    let replayed = rows(&mut grid);
    assert!(
        read.iter().all(|row| replayed.contains(row)),
        "{replayed:?}"
    );
    assert!(replayed.contains(&1), "{replayed:?}");
    assert!(!replayed.contains(&7), "{replayed:?}");

    // A locked grid replays its completed text and keeps its pending lines for the unlock.
    feed_bytes(&mut grid, b"\x1b[?2026h\x1b[8;1Hlocked");
    assert_eq!(rows(&mut grid), Vec::<usize>::new());
    grid.replay_last_render();
    assert_eq!(rows(&mut grid), all_rows);
    feed_bytes(&mut grid, b"\x1b[?2026l");
    assert!(rows(&mut grid).contains(&7));
}
