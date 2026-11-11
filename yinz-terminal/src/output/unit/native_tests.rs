use super::super::*;
use yinz_types::structured_render::{decode, WireColor, ATTR_BOLD};

#[test]
fn native_palette_overrides_apply_to_all_cell_colors() {
    use crate::panes::terminal_character::NamedColor;

    let replacement = AnsiCode::RgbCode((10, 20, 30));
    let mut palette = [None; 256];
    for index in [1, 9, 42] {
        palette[index] = Some(replacement);
    }
    let cases = [
        (
            Some(AnsiCode::NamedColor(NamedColor::Red)),
            WireColor::Named(1),
            true,
        ),
        (
            Some(AnsiCode::NamedColor(NamedColor::BrightRed)),
            WireColor::Named(9),
            true,
        ),
        (Some(AnsiCode::ColorIndex(42)), WireColor::Indexed(42), true),
        (
            Some(AnsiCode::NamedColor(NamedColor::Green)),
            WireColor::Named(2),
            false,
        ),
        (
            Some(AnsiCode::ColorIndex(43)),
            WireColor::Indexed(43),
            false,
        ),
        (
            Some(AnsiCode::RgbCode((1, 2, 3))),
            WireColor::Rgb(1, 2, 3),
            false,
        ),
        (Some(AnsiCode::Reset), WireColor::Default, false),
        (None, WireColor::Default, false),
    ];
    for changed_colors in [None, Some(palette)] {
        let client = ClientId::new(1);
        let mut output = Output::default();
        let characters = cases
            .iter()
            .map(|(color, _, _)| {
                TerminalCharacter::new_styled(
                    'x',
                    CharacterStyles {
                        foreground: *color,
                        background: *color,
                        underline_color: *color,
                        ..DEFAULT_STYLES
                    }
                    .into(),
                )
            })
            .collect();
        let mut chunk = CharacterChunk::new(characters, 0, 0);
        chunk.add_changed_colors(changed_colors);
        output.client_character_chunks.insert(client, vec![chunk]);
        let mut states = HashMap::from([(client, NativeRenderState::default())]);
        let frames = output.serialize_native(&mut states, &HashMap::new(), &HashMap::new());
        let frame = decode(&frames[&client]).unwrap();
        let (_, cells) = frame.rows().next().unwrap();
        assert_eq!(cells.len(), cases.len());
        for (cell, (color, original, overridden)) in cells.iter().zip(cases) {
            let expected = if changed_colors.is_some() && overridden {
                WireColor::Rgb(10, 20, 30)
            } else {
                original
            };
            for actual in [cell.fg, cell.bg, cell.underline_color] {
                assert_eq!(WireColor::unpack(actual), expected, "color {color:?}");
            }
        }
    }
}

#[test]
fn native_and_ansi_clients_receive_the_same_composed_cells() {
    let native = ClientId::new(1);
    let ansi = ClientId::new(2);
    let mut output = Output::default();
    output.add_clients(
        &HashSet::from([native, ansi]),
        Rc::new(RefCell::new(LinkHandler::new())),
        None,
    );
    let styles = CharacterStyles {
        foreground: Some(AnsiCode::RgbCode((10, 20, 30))),
        bold: Some(AnsiCode::On),
        ..DEFAULT_STYLES
    };
    let characters = vec![
        TerminalCharacter::new_styled('界', styles.into()),
        TerminalCharacter::new('!'),
    ];
    output
        .add_character_chunks_to_multiple_clients(
            vec![CharacterChunk::new(characters, 1, 2)],
            [native, ansi].into_iter(),
            None,
        )
        .unwrap();
    output.add_post_vte_instruction_to_client(native, "\x1b[3;2H\x1b[?25h");
    let mut states = HashMap::from([(native, NativeRenderState::default())]);
    let frames = output.serialize_native(
        &mut states,
        &HashMap::from([(native, Size { cols: 8, rows: 4 })]),
        &HashMap::new(),
    );
    let frame = decode(&frames[&native]).unwrap();
    assert!(frame.header().full_repaint());
    let rows: Vec<_> = frame.rows().collect();
    assert_eq!((rows[0].0.x0, rows[0].0.y), (1, 2));
    assert_eq!(rows[0].1.len(), 3);
    assert_eq!(rows[0].1[0].character(), '界');
    assert_eq!(rows[0].1[0].width, 2);
    assert!(rows[0].1[0].has(ATTR_BOLD));
    assert_eq!(
        WireColor::unpack(rows[0].1[0].fg),
        WireColor::Rgb(10, 20, 30)
    );
    assert!(frame.header().cursor_visible);
    let ansi_output = output.serialize().unwrap();
    assert!(!ansi_output.contains_key(&native));
    assert!(ansi_output[&ansi].contains('界'));
}

#[test]
fn native_delta_sequence_and_resize_require_a_repaint() {
    let client = ClientId::new(1);
    let mut states = HashMap::from([(client, NativeRenderState::default())]);
    for (sequence, cols, full) in [(1, 8, true), (2, 8, false), (3, 10, true)] {
        let mut output = Output::default();
        output.add_pre_vte_instruction_to_client(client, "");
        let frames = output.serialize_native(
            &mut states,
            &HashMap::from([(client, Size { cols, rows: 4 })]),
            &HashMap::new(),
        );
        let frame = decode(&frames[&client]).unwrap();
        assert_eq!(frame.header().seq, sequence);
        assert_eq!(frame.header().full_repaint(), full);
    }
}

#[test]
fn native_graphics_transmit_pixels_and_remove_stale_placements() {
    use super::output_tests::{create_test_kitty_parts, kitty_chunk, store_test_kitty_image};
    use yinz_types::structured_render::GraphicsRecord;
    let client = ClientId::new(1);
    let parts = create_test_kitty_parts();
    let image = store_test_kitty_image(&parts.0, 30, 40);
    let mut states = HashMap::from([(client, NativeRenderState::default())]);
    let sizes = HashMap::from([(client, Size { cols: 80, rows: 24 })]);
    for step in 0..4 {
        let present = step != 3;
        let presentation = Presentation {
            terminal: sizes[&client],
            content: Size {
                cols: if step < 2 { 60 } else { 70 },
                rows: 20,
            },
            background: yinz_types::data::PaletteColor::EightBit(0),
        };
        let (dx, dy) = presentation.offset();
        let mut output = Output {
            kitty_image_store: parts.0.clone(),
            kitty_host_state: parts.2.clone(),
            ..Default::default()
        };
        output.add_clients(
            &HashSet::from([client]),
            Rc::new(RefCell::new(LinkHandler::new())),
            None,
        );
        output.add_kitty_image_chunks_to_client(
            client,
            PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1)),
            if present {
                vec![kitty_chunk(image, 1, 2, 3)]
            } else {
                vec![]
            },
            None,
        );
        let frames = output.serialize_native(
            &mut states,
            &sizes,
            &HashMap::from([(client, presentation)]),
        );
        let frame = decode(&frames[&client]).unwrap();
        let records = frame.graphics();
        if step == 1 {
            assert!(
                records.is_empty(),
                "unchanged placements need no delta record"
            );
        } else if present {
            assert!(records.iter().any(|record| matches!(record, GraphicsRecord::Residency { width:30,height:40,data,.. } if data.len()==30*40*4)));
            assert!(records.iter().any(|record| matches!(
                record,
                GraphicsRecord::Placement {
                    cell_x,
                    cell_y,
                    ..
                } if *cell_x == 2 + dx as u32 && *cell_y == 3 + dy as u32
            )));
        } else {
            assert!(records
                .iter()
                .any(|record| matches!(record, GraphicsRecord::DeletePlacement { .. })));
        }
    }
}

#[test]
fn native_presentation_centers_cells_cursor_geometry_and_repaints_gutters() {
    use yinz_types::{
        data::PaletteColor,
        structured_render::{GeometryRecord, PaneRect},
    };
    let client = ClientId::new(1);
    let size = Size { cols: 13, rows: 9 };
    let mut presentations = HashMap::from([(
        client,
        Presentation {
            terminal: size,
            content: Size { cols: 8, rows: 4 },
            background: PaletteColor::Rgb((12, 34, 56)),
        },
    )]);
    let mut states = HashMap::from([(client, NativeRenderState::default())]);
    for step in 0..3 {
        if step == 2 {
            presentations.get_mut(&client).unwrap().content = size;
        }
        let (dx, dy) = presentations[&client].offset();
        let mut output = Output::default();
        output.add_clients(
            &HashSet::from([client]),
            Rc::new(RefCell::new(LinkHandler::new())),
            None,
        );
        output
            .add_character_chunks_to_multiple_clients(
                vec![CharacterChunk::new(
                    vec![TerminalCharacter::new('界')],
                    1,
                    2,
                )],
                [client].into_iter(),
                None,
            )
            .unwrap();
        if step != 1 {
            output.add_post_vte_instruction_to_client(client, "\x1b[3;2H\x1b[?25h");
        }
        output.set_native_geometry(
            client,
            GeometryRecord {
                panes: vec![PaneRect {
                    x: 1,
                    y: 2,
                    cols: 3,
                    rows: 2,
                    ..Default::default()
                }],
            },
        );
        let frames = output.serialize_native(
            &mut states,
            &HashMap::from([(client, size)]),
            &presentations,
        );
        let frame = decode(&frames[&client]).unwrap();
        assert_eq!((frame.header().cols, frame.header().rows), (13, 9));
        assert_eq!(
            (frame.header().cursor_x, frame.header().cursor_y),
            ((1 + dx) as u16, (2 + dy) as u16)
        );
        assert_eq!(frame.header().full_repaint(), step != 1);
        let geometry = frame.geometry().unwrap();
        assert_eq!(
            (geometry.panes[0].x, geometry.panes[0].y),
            ((1 + dx) as u16, (2 + dy) as u16)
        );
        let rows: Vec<_> = frame.rows().collect();
        let text = rows
            .iter()
            .find(|(_, cells)| cells.iter().any(|cell| cell.character() == '界'))
            .unwrap();
        assert_eq!((text.0.x0, text.0.y), ((1 + dx) as u16, (2 + dy) as u16));
        assert_eq!(text.1.len(), 2);
        if step == 0 {
            let gutter = rows.iter().find(|(row, _)| row.y == 0).unwrap();
            assert_eq!(gutter.1.len(), size.cols);
            assert!(gutter
                .1
                .iter()
                .all(|cell| WireColor::unpack(cell.bg) == WireColor::Rgb(12, 34, 56)));
        } else {
            assert_eq!(rows.len(), 1);
        }
    }
}

#[test]
fn native_sixel_origin_uses_presentation_offset() {
    use crate::panes::sixel::SixelGrid;
    use yinz_types::structured_render::GraphicsRecord;
    let client = ClientId::new(1);
    let mut output = Output::default();
    *output.character_cell_size.borrow_mut() = Some(SizeInPixels {
        width: 10,
        height: 20,
    });
    let mut grid = SixelGrid::new(
        output.character_cell_size.clone(),
        output.sixel_image_store.clone(),
    );
    grid.start_image(None, vec![], vec![]);
    grid.handle_byte(b'~');
    assert!(grid.end_image(0, 0).is_some());
    let image = grid.image_coordinates().next().unwrap().0;
    output.add_clients(
        &HashSet::from([client]),
        Rc::new(RefCell::new(LinkHandler::new())),
        None,
    );
    output.add_sixel_image_chunks_to_client(
        client,
        vec![SixelImageChunk {
            cell_x: 1,
            cell_y: 2,
            sixel_image_pixel_x: 0,
            sixel_image_pixel_y: 0,
            sixel_image_pixel_width: 1,
            sixel_image_pixel_height: 6,
            sixel_image_id: image,
        }],
        None,
    );
    let size = Size { cols: 20, rows: 10 };
    let frames = output.serialize_native(
        &mut HashMap::from([(client, NativeRenderState::default())]),
        &HashMap::from([(client, size)]),
        &HashMap::from([(
            client,
            Presentation {
                terminal: size,
                content: Size { cols: 10, rows: 4 },
                background: yinz_types::data::PaletteColor::EightBit(0),
            },
        )]),
    );
    let frame = decode(&frames[&client]).unwrap();
    assert!(frame.graphics().iter().any(|record| matches!(record,
        GraphicsRecord::SixelChunk { cell_x: 6, cell_y: 5, payload, .. } if !payload.is_empty())));
}

#[test]
fn native_cursor_requests_preserve_default_and_explicit_styles() {
    use yinz_types::structured_render::*;
    let client = ClientId::new(1);
    let mut states = HashMap::from([(client, NativeRenderState::default())]);
    for (request, expected) in [
        ("0", CURSOR_SHAPE_DEFAULT),
        ("1", CURSOR_SHAPE_BLOCK | CURSOR_BLINKING),
        ("2", CURSOR_SHAPE_BLOCK),
        ("3", CURSOR_SHAPE_UNDERLINE | CURSOR_BLINKING),
        ("4", CURSOR_SHAPE_UNDERLINE),
        ("5", CURSOR_SHAPE_BEAM | CURSOR_BLINKING),
        ("6", CURSOR_SHAPE_BEAM),
        ("", CURSOR_SHAPE_DEFAULT),
    ] {
        let mut output = Output::default();
        output.client_character_chunks.insert(client, vec![]);
        output.add_post_vte_instruction_to_client(client, &format!("\x1b[{request} q"));
        let frames = output.serialize_native(&mut states, &HashMap::new(), &HashMap::new());
        assert_eq!(
            decode(&frames[&client]).unwrap().header().cursor_shape,
            expected,
            "style {request}"
        );
    }
}

#[test]
fn native_hyperlink_ids_are_reclaimed_on_full_repaint_and_stable_in_deltas() {
    let client = ClientId::new(1);
    let handler = Rc::new(RefCell::new(LinkHandler::new()));
    let mut output = Output::default();
    output.add_clients(&HashSet::from([client]), handler.clone(), None);
    let mut states = HashMap::from([(client, NativeRenderState::default())]);
    let mut last_anchor = None;
    // Each repaint starts a new table, even after more than 255 lifetime URIs.
    for index in 0..300 {
        let uri = format!("https://example.com/{index}");
        let anchor = handler.borrow_mut().new_link_from_url(uri.clone());
        last_anchor = Some(anchor);
        let character = TerminalCharacter::new_styled(
            'x',
            CharacterStyles {
                link_anchor: Some(anchor),
                ..DEFAULT_STYLES
            }
            .into(),
        );
        output
            .client_character_chunks
            .insert(client, vec![CharacterChunk::new(vec![character], 0, 0)]);
        output.mark_host_display_cleared_for_client(client);
        let frames = output.serialize_native(&mut states, &HashMap::new(), &HashMap::new());
        let frame = decode(&frames[&client]).unwrap();
        assert!(frame.header().full_repaint());
        assert_eq!(frame.rows().next().unwrap().1[0].link, 1);
        assert_eq!(frame.links().entries.len(), 1);
        assert_eq!(frame.links().entries[0].uri, uri);
    }
    let new_anchor = handler
        .borrow_mut()
        .new_link_from_url("https://example.com/new".into());
    for (anchor, expected_id) in [(new_anchor, 2), (last_anchor.unwrap(), 1)] {
        let character = TerminalCharacter::new_styled(
            'y',
            CharacterStyles {
                link_anchor: Some(anchor),
                ..DEFAULT_STYLES
            }
            .into(),
        );
        output
            .client_character_chunks
            .insert(client, vec![CharacterChunk::new(vec![character], 1, 0)]);
        let frames = output.serialize_native(&mut states, &HashMap::new(), &HashMap::new());
        let frame = decode(&frames[&client]).unwrap();
        assert!(!frame.header().full_repaint());
        assert_eq!(frame.rows().next().unwrap().1[0].link, expected_id);
        assert_eq!(frame.links().entries.len(), 2);
        assert_eq!(frame.links().entries[0].uri, "https://example.com/299");
    }
}

#[test]
fn native_hyperlinks_are_bounded_before_assigning_ids() {
    use yinz_types::structured_render::MAX_LINK_URI_LEN;
    let client = ClientId::new(1);
    let handler = Rc::new(RefCell::new(LinkHandler::new()));
    let mut output = Output::default();
    output.add_clients(&HashSet::from([client]), handler.clone(), None);
    let mut states = HashMap::from([(client, NativeRenderState::default())]);
    let maximum = "a".repeat(MAX_LINK_URI_LEN);
    let uris = [
        "a".repeat(MAX_LINK_URI_LEN + 1),
        "界".to_owned() + &"a".repeat(MAX_LINK_URI_LEN - 1),
        maximum.clone(),
        "https://example.com".to_owned(),
    ];
    let characters = uris
        .iter()
        .map(|uri| {
            let anchor = handler.borrow_mut().new_link_from_url(uri.clone());
            TerminalCharacter::new_styled(
                'x',
                CharacterStyles {
                    link_anchor: Some(anchor),
                    ..DEFAULT_STYLES
                }
                .into(),
            )
        })
        .collect();
    output
        .client_character_chunks
        .insert(client, vec![CharacterChunk::new(characters, 0, 0)]);
    let frames = output.serialize_native(&mut states, &HashMap::new(), &HashMap::new());
    let frame = decode(&frames[&client]).unwrap();
    let cells = frame.rows().next().unwrap().1;
    assert_eq!(
        cells.iter().map(|cell| cell.link).collect::<Vec<_>>(),
        [0, 0, 1, 2]
    );
    assert!(cells.iter().all(|cell| cell.ch == 'x' as u32));
    assert_eq!(
        frame
            .links()
            .entries
            .iter()
            .map(|entry| entry.uri.as_str())
            .collect::<Vec<_>>(),
        [maximum.as_str(), "https://example.com"]
    );
}

fn linked_character(handler: &mut LinkHandler, uri: &str, ch: char) -> TerminalCharacter {
    let anchor = handler.new_link_from_url(uri.to_owned());
    TerminalCharacter::new_styled(
        ch,
        CharacterStyles {
            link_anchor: Some(anchor),
            ..DEFAULT_STYLES
        }
        .into(),
    )
}

#[test]
fn native_hyperlinks_reuse_ids_after_delta_replacements() {
    let client = ClientId::new(1);
    let handler = Rc::new(RefCell::new(LinkHandler::new()));
    let mut output = Output::default();
    output.add_clients(&HashSet::from([client]), handler.clone(), None);
    let mut states = HashMap::from([(client, NativeRenderState::default())]);
    let retained = linked_character(&mut handler.borrow_mut(), "retained", 'r');
    output
        .client_character_chunks
        .insert(client, vec![CharacterChunk::new(vec![retained], 1, 0)]);
    let frames = output.serialize_native(&mut states, &HashMap::new(), &HashMap::new());
    let retained_id = decode(&frames[&client]).unwrap().rows().next().unwrap().1[0].link;
    for index in 0..520 {
        let uri = format!("https://example.com/{index}");
        let character = linked_character(&mut handler.borrow_mut(), &uri, 'x');
        output
            .client_character_chunks
            .insert(client, vec![CharacterChunk::new(vec![character], 0, 0)]);
        let frames = output.serialize_native(&mut states, &HashMap::new(), &HashMap::new());
        let frame = decode(&frames[&client]).unwrap();
        assert!(!frame.header().clear());
        let id = frame.rows().next().unwrap().1[0].link;
        assert_ne!(id, 0);
        let links = frame.links();
        assert_eq!(
            links
                .entries
                .iter()
                .find(|entry| entry.id == id)
                .unwrap()
                .uri,
            uri
        );
        assert_eq!(
            links
                .entries
                .iter()
                .find(|entry| entry.id == retained_id)
                .unwrap()
                .uri,
            "retained"
        );
    }
}

#[test]
fn native_hyperlink_capacity_tracks_wide_cells_and_plain_overwrites() {
    let client = ClientId::new(1);
    let handler = Rc::new(RefCell::new(LinkHandler::new()));
    let mut output = Output::default();
    output.add_clients(&HashSet::from([client]), handler.clone(), None);
    let mut states = HashMap::from([(client, NativeRenderState::default())]);
    let sizes = HashMap::from([(client, Size { cols: 258, rows: 1 })]);
    let mut characters = vec![linked_character(&mut handler.borrow_mut(), "wide", '界')];
    for index in 1..255 {
        characters.push(linked_character(
            &mut handler.borrow_mut(),
            &format!("link/{index}"),
            'x',
        ));
    }
    output
        .client_character_chunks
        .insert(client, vec![CharacterChunk::new(characters, 0, 0)]);
    output.serialize_native(&mut states, &sizes, &HashMap::new());
    // Replacing only the leading cell must leave the continuation's ID reserved.
    for (col, expected) in [(0, 0), (1, 1)] {
        let new = linked_character(&mut handler.borrow_mut(), "new", 'n');
        output.client_character_chunks.insert(
            client,
            vec![
                CharacterChunk::new(vec![new], 256, 0),
                CharacterChunk::new(vec![TerminalCharacter::new(' ')], col, 0),
            ],
        );
        let frames = output.serialize_native(&mut states, &sizes, &HashMap::new());
        let frame = decode(&frames[&client]).unwrap();
        assert_eq!(frame.rows().next().unwrap().1[0].link, expected);
        let links = frame.links();
        assert_eq!(
            links
                .entries
                .iter()
                .find(|entry| entry.id == 1)
                .unwrap()
                .uri,
            if expected == 0 { "wide" } else { "new" }
        );
        assert_eq!(
            links
                .entries
                .iter()
                .find(|entry| entry.id == 255)
                .unwrap()
                .uri,
            "link/254"
        );
    }
    // A clipped wide character cannot allocate another ID in the full table.
    let clipped = linked_character(&mut handler.borrow_mut(), "clipped", '界');
    let new = linked_character(&mut handler.borrow_mut(), "overflow", 'o');
    output.client_character_chunks.insert(
        client,
        vec![
            CharacterChunk::new(vec![clipped], 257, 0),
            CharacterChunk::new(vec![new], 257, 0),
        ],
    );
    let frames = output.serialize_native(&mut states, &sizes, &HashMap::new());
    let frame = decode(&frames[&client]).unwrap();
    assert_eq!(frame.rows().last().unwrap().1[0].link, 0);
}

#[test]
fn native_hyperlink_reuse_pins_all_rows_emitted_in_the_frame() {
    let client = ClientId::new(1);
    let handler = Rc::new(RefCell::new(LinkHandler::new()));
    let mut output = Output::default();
    output.add_clients(&HashSet::from([client]), handler.clone(), None);
    let mut states = HashMap::from([(client, NativeRenderState::default())]);
    let chunks = (0..256)
        .map(|index| {
            CharacterChunk::new(
                vec![linked_character(
                    &mut handler.borrow_mut(),
                    &format!("link/{index}"),
                    'x',
                )],
                0,
                0,
            )
        })
        .collect();
    output.client_character_chunks.insert(client, chunks);
    let frames = output.serialize_native(&mut states, &HashMap::new(), &HashMap::new());
    let frame = decode(&frames[&client]).unwrap();
    let links = frame.links();
    for (index, (_, cells)) in frame.rows().enumerate() {
        if index == 255 {
            assert_eq!(cells[0].link, 0);
        } else {
            let entry = links
                .entries
                .iter()
                .find(|entry| entry.id == cells[0].link)
                .unwrap();
            assert_eq!(entry.uri, format!("link/{index}"));
        }
    }
    let new = linked_character(&mut handler.borrow_mut(), "next", 'n');
    output
        .client_character_chunks
        .insert(client, vec![CharacterChunk::new(vec![new], 0, 0)]);
    let frames = output.serialize_native(&mut states, &HashMap::new(), &HashMap::new());
    assert_ne!(
        decode(&frames[&client]).unwrap().rows().next().unwrap().1[0].link,
        0
    );
}
