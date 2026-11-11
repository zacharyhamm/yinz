use super::super::*;
use yinz_types::structured_render::{decode, WireColor, ATTR_BOLD};

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
    for present in [true, false] {
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
        let frames = output.serialize_native(&mut states, &sizes);
        let frame = decode(&frames[&client]).unwrap();
        let records = frame.graphics();
        if present {
            assert!(records.iter().any(|record| matches!(record, GraphicsRecord::Residency { width:30,height:40,data,.. } if data.len()==30*40*4)));
            assert!(records.iter().any(|record| matches!(
                record,
                GraphicsRecord::Placement {
                    cell_x: 2,
                    cell_y: 3,
                    ..
                }
            )));
        } else {
            assert!(records
                .iter()
                .any(|record| matches!(record, GraphicsRecord::DeletePlacement { .. })));
        }
    }
}
