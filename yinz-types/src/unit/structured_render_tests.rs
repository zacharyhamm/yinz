use super::*;

fn cell(character: char) -> WireCell {
    WireCell {
        ch: character as u32,
        ..WireCell::BLANK
    }
}

fn run(text: &str) -> Vec<WireCell> {
    text.chars().map(cell).collect()
}

fn grid(cols: usize, rows: usize, frame: &[u8]) -> Vec<WireCell> {
    let view = decode(frame).expect("the frame should decode");
    let mut destination = vec![WireCell::BLANK; cols * rows];
    view.apply(&mut destination, cols, rows);
    destination
}

fn error_of(frame: &[u8]) -> Option<DecodeError> {
    decode(frame).err()
}

fn text_of(cells: &[WireCell], cols: usize, row: usize) -> String {
    cells[row * cols..(row + 1) * cols]
        .iter()
        .map(WireCell::character)
        .collect()
}

#[test]
fn a_frame_round_trips_its_header_rows_and_sideband() {
    let mut builder = FrameBuilder::new(8, 3, 42);
    builder.set_full_repaint(true);
    builder.set_cursor(CursorState {
        x: 3,
        y: 1,
        shape: CURSOR_SHAPE_BEAM | CURSOR_BLINKING,
        visible: true,
    });
    builder.push_row(1, 2, &run("abc"));
    builder.sideband_mut().extend_from_slice(b"\x1b_Gq=2\x1b\\");
    let frame = builder.finish();

    let view = decode(&frame).unwrap();
    let header = *view.header();
    assert_eq!(header.version, WIRE_VERSION);
    assert_eq!((header.cols, header.rows), (8, 3));
    assert_eq!(header.seq, 42);
    assert!(header.full_repaint());
    assert!(!header.clear());
    assert_eq!(
        view.cursor(),
        CursorState {
            x: 3,
            y: 1,
            shape: CURSOR_SHAPE_BEAM | CURSOR_BLINKING,
            visible: true
        }
    );
    assert!(view.cursor().blinking());
    assert_eq!(view.cursor().shape_kind(), CURSOR_SHAPE_BEAM);
    assert_eq!(view.sideband(), b"\x1b_Gq=2\x1b\\");

    let rows: Vec<_> = view.rows().collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].0,
        RowRecord {
            y: 1,
            x0: 2,
            len: 3
        }
    );
    assert_eq!(rows[0].1, run("abc"));
    assert_eq!(text_of(&grid(8, 3, &frame), 8, 1), "  abc   ");
}

#[test]
fn the_cell_array_starts_on_a_four_byte_boundary() {
    for record_count in 0..5u16 {
        let mut builder = FrameBuilder::new(4, 8, 0);
        for y in 0..record_count {
            builder.push_row(y, 0, &run("z"));
        }
        let frame = builder.finish();
        let view = decode(&frame).unwrap();
        assert_eq!(view.header().cells_offset % 4, 0);
    }
}

#[test]
fn every_style_field_survives_the_wire() {
    let styled = WireCell {
        ch: '\u{4f60}' as u32,
        fg: WireColor::Rgb(1, 2, 3).pack(),
        bg: WireColor::Indexed(208).pack(),
        underline_color: WireColor::Named(4).pack(),
        attrs: ATTR_BOLD | ATTR_ITALIC | ATTR_FAST_BLINK | (UNDERLINE_CURLY << UNDERLINE_SHIFT),
        width: 2,
        link: 17,
    };
    let mut builder = FrameBuilder::new(4, 1, 1);
    builder.push_row(0, 0, &[styled, WireCell::BLANK]);
    let frame = builder.finish();
    let cells = grid(4, 1, &frame);

    assert_eq!(cells[0], styled);
    assert_eq!(cells[0].character(), '\u{4f60}');
    assert!(cells[0].is_wide_head());
    assert!(cells[0].has(ATTR_BOLD) && cells[0].has(ATTR_ITALIC));
    assert!(!cells[0].has(ATTR_SLOW_BLINK) && cells[0].has(ATTR_FAST_BLINK));
    assert_eq!(cells[0].underline_style(), UNDERLINE_CURLY);
    assert_eq!(WireColor::unpack(cells[0].fg), WireColor::Rgb(1, 2, 3));
    assert_eq!(WireColor::unpack(cells[0].bg), WireColor::Indexed(208));
    assert_eq!(
        WireColor::unpack(cells[0].underline_color),
        WireColor::Named(4)
    );
    assert_eq!(WireColor::unpack(COLOR_DEFAULT), WireColor::Default);
}

#[test]
fn a_row_may_carry_several_partial_records_and_later_ones_win() {
    let mut builder = FrameBuilder::new(6, 1, 0);
    builder.push_row(0, 0, &run("aaaa"));
    builder.push_row(0, 2, &run("bb"));
    let frame = builder.finish();
    assert_eq!(text_of(&grid(6, 1, &frame), 6, 0), "aabb  ");
}

#[test]
fn a_run_that_would_overrun_the_right_edge_is_truncated_rather_than_emitted() {
    let mut builder = FrameBuilder::new(4, 1, 0);
    builder.push_row(0, 2, &run("abcd"));
    let frame = builder.finish();
    let view = decode(&frame).unwrap();
    assert_eq!(
        view.rows().next().unwrap().0,
        RowRecord {
            y: 0,
            x0: 2,
            len: 2
        }
    );
    assert_eq!(text_of(&grid(4, 1, &frame), 4, 0), "  ab");
}

#[test]
fn a_run_outside_the_viewport_is_dropped_entirely() {
    let mut builder = FrameBuilder::new(4, 2, 0);
    builder.push_row(2, 0, &run("gone"));
    builder.push_row(0, 4, &run("gone"));
    builder.push_row(0, 0, &[]);
    let frame = builder.finish();
    assert_eq!(decode(&frame).unwrap().header().row_record_count, 0);
}

#[test]
fn a_truncated_buffer_is_rejected() {
    let mut builder = FrameBuilder::new(4, 1, 0);
    builder.push_row(0, 0, &run("ab"));
    let frame = builder.finish();
    assert_eq!(
        error_of(&frame[..HEADER_LEN - 1]),
        Some(DecodeError::TooShort)
    );
    for cut in [HEADER_LEN, HEADER_LEN + 4, frame.len() - 1] {
        assert!(
            decode(&frame[..cut]).is_err(),
            "a frame cut at {} decoded",
            cut
        );
    }
}

#[test]
fn a_foreign_wire_version_is_rejected() {
    let mut frame = FrameBuilder::new(4, 1, 0).finish();
    frame[0] = WIRE_VERSION + 1;
    assert_eq!(
        error_of(&frame),
        Some(DecodeError::UnsupportedVersion(WIRE_VERSION + 1))
    );
    frame[0] = OLDEST_WIRE_VERSION - 1;
    assert_eq!(
        error_of(&frame),
        Some(DecodeError::UnsupportedVersion(OLDEST_WIRE_VERSION - 1))
    );
}

#[test]
fn a_frame_of_the_previous_wire_version_still_decodes() {
    let mut builder = FrameBuilder::new(4, 1, 0);
    builder.push_row(0, 0, &run("ab"));
    builder.push_geometry(&GeometryRecord {
        panes: vec![pane(0, 0, 4, 1, 0)],
    });
    let mut frame = builder.finish();
    frame[0] = 2;
    let view = decode(&frame).unwrap();
    assert_eq!(view.header().version, 2);
    assert_eq!(view.geometry().unwrap().panes.len(), 1);
    assert!(view.scroll().is_empty());
}

#[test]
fn a_scroll_record_in_a_frame_of_the_previous_wire_version_is_rejected() {
    let mut builder = FrameBuilder::new(4, 1, 0);
    builder.push_scroll(&scroll(&[(0, 2)]));
    let mut frame = builder.finish();
    assert!(decode(&frame).is_ok());
    frame[0] = 2;
    assert_eq!(error_of(&frame), Some(DecodeError::BadGraphicsRecord));
}

#[test]
fn a_row_record_outside_the_viewport_is_rejected() {
    let mut builder = FrameBuilder::new(4, 2, 0);
    builder.push_row(0, 0, &run("ab"));
    let frame = builder.finish();

    let mut past_the_bottom = frame.clone();
    past_the_bottom[HEADER_LEN] = 9;
    assert_eq!(error_of(&past_the_bottom), Some(DecodeError::RowOutOfRange));

    let mut past_the_right = frame.clone();
    past_the_right[HEADER_LEN + 2] = 3;
    assert_eq!(error_of(&past_the_right), Some(DecodeError::RowOutOfRange));

    let mut overlong = frame;
    overlong[HEADER_LEN + 4] = 5;
    assert_eq!(error_of(&overlong), Some(DecodeError::RowOutOfRange));
}

#[test]
fn section_offsets_that_leave_the_frame_are_rejected() {
    let mut builder = FrameBuilder::new(4, 1, 0);
    builder.push_row(0, 0, &run("ab"));
    builder.sideband_mut().extend_from_slice(b"xy");
    let frame = builder.finish();

    let mut cells_past_the_end = frame.clone();
    cells_past_the_end[24..28].copy_from_slice(&(frame.len() as u32 + 8).to_le_bytes());
    assert_eq!(error_of(&cells_past_the_end), Some(DecodeError::BadSection));

    let mut cells_inside_the_records = frame.clone();
    cells_inside_the_records[24..28].copy_from_slice(&(HEADER_LEN as u32).to_le_bytes());
    assert_eq!(
        error_of(&cells_inside_the_records),
        Some(DecodeError::BadSection)
    );

    let mut graphics_past_the_end = frame.clone();
    graphics_past_the_end[36..40].copy_from_slice(&1024u32.to_le_bytes());
    assert_eq!(
        error_of(&graphics_past_the_end),
        Some(DecodeError::BadSection)
    );

    let mut sideband_past_the_end = frame.clone();
    sideband_past_the_end[44..48].copy_from_slice(&1024u32.to_le_bytes());
    assert_eq!(
        error_of(&sideband_past_the_end),
        Some(DecodeError::BadSection)
    );

    let mut ragged_cells = frame;
    ragged_cells[28..32].copy_from_slice(&(CELL_LEN as u32 * 2 - 1).to_le_bytes());
    assert!(decode(&ragged_cells).is_err());
}

#[test]
fn records_that_name_more_cells_than_the_frame_carries_are_rejected() {
    let mut builder = FrameBuilder::new(8, 1, 0);
    builder.push_row(0, 0, &run("ab"));
    let mut frame = builder.finish();
    frame[HEADER_LEN + 4] = 4;
    assert_eq!(error_of(&frame), Some(DecodeError::CellsExhausted));
}

#[test]
fn records_that_name_fewer_cells_than_the_frame_carries_are_rejected() {
    let mut builder = FrameBuilder::new(8, 1, 0);
    builder.push_row(0, 0, &run("ab"));
    let mut frame = builder.finish();
    frame[HEADER_LEN + 4] = 1;
    assert_eq!(error_of(&frame), Some(DecodeError::CellCountMismatch));
}

#[test]
fn an_overlay_replays_the_writes_it_absorbed_in_order() {
    let mut overlay = PendingOverlay::new(6, 2);
    assert!(!overlay.is_dirty());
    overlay.merge_row(0, 0, &run("aaaa"));
    overlay.merge_row(0, 2, &run("bb"));
    overlay.merge_row(1, 4, &run("zz"));
    assert!(overlay.is_dirty());

    let frame = overlay.drain(7);
    assert!(!overlay.is_dirty());
    let view = decode(&frame).unwrap();
    assert_eq!(view.header().seq, 7);
    let cells = grid(6, 2, &frame);
    assert_eq!(text_of(&cells, 6, 0), "aabb  ");
    assert_eq!(text_of(&cells, 6, 1), "    zz");
}

#[test]
fn overlapping_and_adjacent_spans_are_unioned_into_one_record() {
    let mut overlay = PendingOverlay::new(8, 1);
    overlay.merge_row(0, 0, &run("ab"));
    overlay.merge_row(0, 2, &run("cd"));
    overlay.merge_row(0, 1, &run("XY"));
    let frame = overlay.drain(0);
    let view = decode(&frame).unwrap();
    let rows: Vec<_> = view.rows().collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].0,
        RowRecord {
            y: 0,
            x0: 0,
            len: 4
        }
    );
    assert_eq!(text_of(&grid(8, 1, &frame), 8, 0), "aXYd    ");
}

#[test]
fn disjoint_spans_stay_separate_so_untouched_cells_are_never_repainted() {
    let mut overlay = PendingOverlay::new(8, 1);
    overlay.merge_row(0, 0, &run("ab"));
    overlay.merge_row(0, 6, &run("yz"));
    let frame = overlay.drain(0);
    let view = decode(&frame).unwrap();
    let records: Vec<RowRecord> = view.rows().map(|(record, _)| record).collect();
    assert_eq!(
        records,
        vec![
            RowRecord {
                y: 0,
                x0: 0,
                len: 2
            },
            RowRecord {
                y: 0,
                x0: 6,
                len: 2
            }
        ]
    );
}

#[test]
fn an_overlay_absorbs_whole_frames_including_flags_cursor_and_sideband() {
    let mut first = FrameBuilder::new(6, 2, 1);
    first.set_clear(true);
    first.push_row(0, 0, &run("one"));
    first.sideband_mut().extend_from_slice(b"AA");
    let first = first.finish();

    let mut second = FrameBuilder::new(6, 2, 2);
    second.set_full_repaint(true);
    second.push_row(1, 0, &run("two"));
    second.set_cursor(CursorState {
        x: 5,
        y: 1,
        shape: CURSOR_SHAPE_UNDERLINE,
        visible: true,
    });
    second.sideband_mut().extend_from_slice(b"BB");
    let second = second.finish();

    let mut overlay = PendingOverlay::new(6, 2);
    overlay.merge_frame(&decode(&first).unwrap());
    overlay.merge_frame(&decode(&second).unwrap());

    let drained = overlay.drain(9);
    let view = decode(&drained).unwrap();
    assert!(view.header().full_repaint());
    assert!(view.header().clear());
    assert_eq!(view.header().seq, 9);
    assert_eq!(view.sideband(), b"AABB");
    assert_eq!(
        view.cursor(),
        CursorState {
            x: 5,
            y: 1,
            shape: CURSOR_SHAPE_UNDERLINE,
            visible: true
        }
    );
    let cells = grid(6, 2, &drained);
    assert_eq!(text_of(&cells, 6, 0), "      ");
    assert_eq!(text_of(&cells, 6, 1), "two   ");
}

#[test]
fn rows_absorbed_before_a_clear_are_not_replayed_after_it() {
    let mut first = FrameBuilder::new(6, 2, 1);
    first.push_row(0, 0, &run("stale"));
    first.push_row(1, 0, &run("old"));
    let first = first.finish();

    let mut second = FrameBuilder::new(6, 2, 2);
    second.set_clear(true);
    second.push_row(0, 0, &run("ab"));
    let second = second.finish();

    let mut overlay = PendingOverlay::new(6, 2);
    overlay.merge_frame(&decode(&first).unwrap());
    overlay.merge_frame(&decode(&second).unwrap());

    let drained = overlay.drain(3);
    let view = decode(&drained).unwrap();
    assert!(view.header().clear());
    let records: Vec<RowRecord> = view.rows().map(|(record, _)| record).collect();
    assert_eq!(
        records,
        vec![RowRecord {
            y: 0,
            x0: 0,
            len: 2
        }]
    );
}

#[test]
fn an_overlay_that_absorbs_a_reshaped_frame_adopts_the_new_dimensions() {
    let mut builder = FrameBuilder::new(3, 1, 1);
    builder.push_row(0, 0, &run("abc"));
    let frame = builder.finish();

    let mut overlay = PendingOverlay::new(8, 4);
    overlay.merge_row(3, 0, &run("stale"));
    overlay.merge_frame(&decode(&frame).unwrap());
    assert_eq!(overlay.dimensions(), (3, 1));

    let drained = overlay.drain(2);
    let view = decode(&drained).unwrap();
    assert_eq!((view.header().cols, view.header().rows), (3, 1));
    assert_eq!(text_of(&grid(3, 1, &drained), 3, 0), "abc");
}

fn residency(id: u64, medium: GraphicsMedium, data: &[u8]) -> GraphicsRecord {
    GraphicsRecord::Residency {
        id,
        width: 2,
        height: 1,
        format: GRAPHICS_FORMAT_RGBA8,
        medium,
        byte_len: 8,
        data: data.to_vec(),
    }
}

fn placement(image_id: u64, placement_id: u64, z: i32) -> GraphicsRecord {
    GraphicsRecord::Placement {
        image_id,
        placement_id,
        cell_x: 3,
        cell_y: 4,
        offset_x: 5,
        offset_y: 6,
        source_x: 7,
        source_y: 8,
        source_width: 9,
        source_height: 10,
        z,
    }
}

#[test]
fn every_graphics_record_round_trips_in_emission_order() {
    let records = vec![
        GraphicsRecord::Clear,
        residency(1, GraphicsMedium::Inline, &[1, 2, 3, 4, 5, 6, 7, 8]),
        residency(2, GraphicsMedium::SharedFile, b"/dev/shm/yinz-image"),
        placement(1, 7, -3),
        GraphicsRecord::DeletePlacement {
            image_id: 1,
            placement_id: 7,
        },
        GraphicsRecord::DeleteImage { id: 2 },
        GraphicsRecord::SixelChunk {
            cell_x: 11,
            cell_y: 12,
            pixel_x: 13,
            pixel_y: 14,
            pixel_width: 15,
            pixel_height: 16,
            payload: b"\x1bP0;1;0q#0;2;100;0;0~\x1b\\".to_vec(),
        },
    ];

    let mut builder = FrameBuilder::new(4, 2, 0);
    builder.push_row(0, 0, &run("ab"));
    builder.extend_graphics(records.iter());
    builder
        .sideband_mut()
        .extend_from_slice(b"\x1b]0;title\x07");
    let frame = builder.finish();

    let view = decode(&frame).unwrap();
    assert_eq!(view.graphics(), records);
    assert_eq!(view.sideband(), b"\x1b]0;title\x07");
    assert_eq!(text_of(&grid(4, 2, &frame), 4, 0), "ab  ");
}

#[test]
fn the_graphics_section_starts_on_a_four_byte_boundary() {
    for cells in 0..5u16 {
        let mut builder = FrameBuilder::new(8, 1, 0);
        if cells > 0 {
            builder.push_row(0, 0, &run(&"z".repeat(cells as usize)));
        }
        builder.push_graphics(&GraphicsRecord::DeleteImage { id: 9 });
        let frame = builder.finish();
        let view = decode(&frame).unwrap();
        assert_eq!(view.header().graphics_offset % 4, 0);
        assert_eq!(view.graphics(), vec![GraphicsRecord::DeleteImage { id: 9 }]);
    }
}

#[test]
fn a_graphics_record_that_overruns_its_section_is_rejected() {
    let mut builder = FrameBuilder::new(4, 1, 0);
    builder.push_graphics(&GraphicsRecord::DeleteImage { id: 1 });
    let mut frame = builder.finish();
    let at = decode(&frame).unwrap().header().graphics_offset as usize;
    frame[at + 4..at + 8].copy_from_slice(&4096u32.to_le_bytes());
    assert_eq!(error_of(&frame), Some(DecodeError::BadGraphicsRecord));
}

#[test]
fn a_graphics_record_of_an_unknown_kind_is_rejected() {
    let mut builder = FrameBuilder::new(4, 1, 0);
    builder.push_graphics(&GraphicsRecord::DeleteImage { id: 1 });
    let mut frame = builder.finish();
    let at = decode(&frame).unwrap().header().graphics_offset as usize;
    frame[at] = 99;
    assert_eq!(error_of(&frame), Some(DecodeError::BadGraphicsRecord));
}

#[test]
fn an_overlay_replays_the_graphics_records_it_absorbed() {
    let mut first = FrameBuilder::new(4, 1, 1);
    first.push_graphics(&GraphicsRecord::Clear);
    first.push_graphics(&residency(1, GraphicsMedium::Inline, &[9; 8]));
    let first = first.finish();

    let mut second = FrameBuilder::new(4, 1, 2);
    second.push_graphics(&placement(1, 1, 0));
    let second = second.finish();

    let mut overlay = PendingOverlay::new(4, 1);
    assert!(!overlay.is_dirty());
    overlay.merge_frame(&decode(&first).unwrap());
    overlay.merge_frame(&decode(&second).unwrap());
    assert!(overlay.is_dirty());

    let drained = overlay.drain(3);
    assert_eq!(
        decode(&drained).unwrap().graphics(),
        vec![
            GraphicsRecord::Clear,
            residency(1, GraphicsMedium::Inline, &[9; 8]),
            placement(1, 1, 0),
        ]
    );
    assert!(!overlay.is_dirty());
}

fn pane(x: u16, y: u16, cols: u16, rows: u16, flags: u8) -> PaneRect {
    PaneRect {
        x,
        y,
        cols,
        rows,
        top: 1,
        bottom: 1,
        left: 1,
        right: 1,
        flags: flags | PANE_FRAMED | PANE_SELECTABLE,
    }
}

#[test]
fn a_geometry_record_round_trips_every_field_of_every_pane() {
    let record = GeometryRecord {
        panes: vec![
            pane(0, 0, 40, 20, PANE_FOCUSED),
            PaneRect {
                x: 40,
                y: 0,
                cols: 40,
                rows: 20,
                top: 0,
                bottom: 0,
                left: 0,
                right: 0,
                flags: PANE_WANTS_MOUSE,
            },
            PaneRect {
                x: 0,
                y: 20,
                cols: 80,
                rows: 1,
                top: 0,
                bottom: 0,
                left: 0,
                right: 0,
                flags: 0,
            },
        ],
    };

    let mut builder = FrameBuilder::new(80, 21, 0);
    builder.push_geometry(&record);
    let frame = builder.finish();

    let view = decode(&frame).unwrap();
    assert_eq!(view.geometry(), Some(record));
    assert!(view.graphics().is_empty());
}

#[test]
fn the_flags_that_tell_popups_floating_panes_and_plugins_apart_round_trip() {
    let record = GeometryRecord {
        panes: vec![
            pane(0, 0, 40, 20, PANE_UNDER_FLOATING),
            pane(10, 5, 20, 10, PANE_FLOATING | PANE_PLUGIN_UI),
            PaneRect {
                x: 2,
                y: 2,
                cols: 10,
                rows: 3,
                top: 0,
                bottom: 0,
                left: 0,
                right: 0,
                flags: PANE_POPUP | PANE_FOCUSED,
            },
        ],
    };
    let mut builder = FrameBuilder::new(40, 20, 0);
    builder.push_geometry(&record);
    let frame = builder.finish();
    let decoded = decode(&frame).unwrap().geometry().unwrap();
    assert_eq!(decoded, record);
    assert!(decoded.panes[0].under_floating() && !decoded.panes[0].floating());
    assert!(decoded.panes[1].floating() && decoded.panes[1].plugin_ui());
    assert!(decoded.panes[2].popup() && decoded.panes[2].focused());
    assert_eq!(decoded.focused_popup(), Some(&decoded.panes[2]));
}

#[test]
fn the_pin_button_sits_where_a_click_toggles_it() {
    let floating = pane(10, 5, 40, 10, PANE_FLOATING);
    let hits: Vec<u16> = (0..60)
        .filter(|x| floating.pin_button_contains(*x, 5))
        .collect();
    assert_eq!(hits, vec![45, 46, 47]);
    assert!(!floating.pin_button_contains(46, 6));
    let tiled = pane(10, 5, 40, 10, 0);
    assert!(!tiled.pin_button_contains(46, 5));
}

#[test]
fn only_an_unfocused_popup_leaves_the_rest_of_the_screen_alone() {
    let mut record = GeometryRecord {
        panes: vec![
            pane(0, 0, 40, 20, PANE_FOCUSED),
            PaneRect {
                x: 2,
                y: 2,
                cols: 10,
                rows: 3,
                flags: PANE_POPUP,
                ..PaneRect::default()
            },
        ],
    };
    assert!(record.claims_the_click(3, 3));
    assert!(!record.claims_the_click(20, 10));
    assert_eq!(record.pointer_at(3, 3, false), PointerKind::Arrow);
    assert_eq!(record.pointer_at(20, 10, false), PointerKind::Text);
    record.panes[1].flags |= PANE_FOCUSED;
    assert!(record.claims_the_click(20, 10));
    assert_eq!(record.pointer_at(20, 10, false), PointerKind::Arrow);
}

#[test]
fn a_geometry_record_shares_the_record_stream_with_graphics_records() {
    let record = GeometryRecord {
        panes: vec![pane(0, 0, 10, 5, PANE_FOCUSED)],
    };
    let mut builder = FrameBuilder::new(10, 5, 0);
    builder.push_graphics(&GraphicsRecord::Clear);
    builder.push_geometry(&record);
    builder.push_graphics(&GraphicsRecord::DeleteImage { id: 4 });
    let frame = builder.finish();

    let view = decode(&frame).unwrap();
    assert_eq!(view.geometry(), Some(record));
    assert_eq!(
        view.graphics(),
        vec![GraphicsRecord::Clear, GraphicsRecord::DeleteImage { id: 4 }]
    );
}

#[test]
fn a_frame_without_a_geometry_record_reports_none() {
    let mut builder = FrameBuilder::new(4, 1, 0);
    builder.push_row(0, 0, &run("ab"));
    builder.push_graphics(&GraphicsRecord::Clear);
    let frame = builder.finish();
    assert_eq!(decode(&frame).unwrap().geometry(), None);
}

#[test]
fn an_empty_geometry_record_survives_the_wire_as_an_empty_list() {
    let mut builder = FrameBuilder::new(4, 1, 0);
    builder.push_geometry(&GeometryRecord::default());
    let frame = builder.finish();
    assert_eq!(
        decode(&frame).unwrap().geometry(),
        Some(GeometryRecord::default())
    );
}

#[test]
fn a_geometry_record_that_names_more_panes_than_it_carries_is_rejected() {
    let mut builder = FrameBuilder::new(4, 1, 0);
    builder.push_geometry(&GeometryRecord {
        panes: vec![pane(0, 0, 4, 1, 0)],
    });
    let mut frame = builder.finish();
    let at = decode(&frame).unwrap().header().graphics_offset as usize;
    frame[at + GRAPHICS_RECORD_HEADER_LEN..at + GRAPHICS_RECORD_HEADER_LEN + 2]
        .copy_from_slice(&9u16.to_le_bytes());
    assert_eq!(error_of(&frame), Some(DecodeError::BadGraphicsRecord));
}

#[test]
fn a_geometry_record_declaring_a_shorter_entry_than_this_build_knows_is_rejected() {
    let mut builder = FrameBuilder::new(4, 1, 0);
    builder.push_geometry(&GeometryRecord {
        panes: vec![pane(0, 0, 4, 1, 0)],
    });
    let mut frame = builder.finish();
    let at = decode(&frame).unwrap().header().graphics_offset as usize;
    frame[at + GRAPHICS_RECORD_HEADER_LEN + 2..at + GRAPHICS_RECORD_HEADER_LEN + 4]
        .copy_from_slice(&8u16.to_le_bytes());
    assert_eq!(error_of(&frame), Some(DecodeError::BadGraphicsRecord));
}

#[test]
fn the_content_rectangle_is_the_outer_rectangle_less_its_frame_ring() {
    let framed = pane(10, 4, 20, 8, 0);
    assert_eq!(
        (
            framed.content_x(),
            framed.content_y(),
            framed.content_cols(),
            framed.content_rows()
        ),
        (11, 5, 18, 6)
    );
    assert!(framed.contains(10, 4));
    assert!(!framed.content_contains(10, 4));
    assert!(framed.content_contains(11, 5));
    assert!(framed.content_contains(28, 10));
    assert!(!framed.content_contains(29, 10));
    assert!(!framed.contains(30, 4));
}

#[test]
fn the_topmost_pane_owns_a_cell_two_panes_share() {
    let record = GeometryRecord {
        panes: vec![pane(0, 0, 20, 10, 0), pane(4, 2, 8, 4, PANE_FOCUSED)],
    };
    assert_eq!(record.content_pane_at(1, 1), Some(&record.panes[0]));
    assert_eq!(record.content_pane_at(6, 4), Some(&record.panes[1]));
    assert_eq!(record.content_pane_at(40, 40), None);
}

#[test]
fn clipping_a_pane_trims_the_rectangle_and_the_frame_ring_it_loses() {
    let framed = pane(0, 0, 20, 10, 0);
    let clipped = framed.clip_to(15, 10).unwrap();
    assert_eq!((clipped.x, clipped.cols), (0, 15));
    assert_eq!(clipped.right, 0);
    assert_eq!(clipped.left, 1);
    assert_eq!(clipped.content_cols(), 14);

    let clipped = framed.clip_to(20, 6).unwrap();
    assert_eq!((clipped.y, clipped.rows), (0, 6));
    assert_eq!(clipped.bottom, 0);
    assert_eq!(clipped.content_rows(), 5);

    assert_eq!(framed.clip_to(20, 10), Some(framed));
    assert_eq!(pane(30, 0, 10, 10, 0).clip_to(20, 10), None);
    assert_eq!(pane(0, 30, 10, 10, 0).clip_to(20, 10), None);
}

#[test]
fn clipping_a_record_drops_the_panes_the_viewport_cannot_show() {
    let record = GeometryRecord {
        panes: vec![
            pane(0, 0, 20, 10, 0),
            pane(20, 0, 20, 10, 0),
            pane(0, 10, 40, 10, 0),
        ],
    };
    let clipped = record.clip_to(30, 10);
    assert_eq!(clipped.panes.len(), 2);
    assert_eq!(clipped.panes[0].cols, 20);
    assert_eq!(clipped.panes[1].cols, 10);
}

#[test]
fn a_later_geometry_record_supersedes_an_earlier_one_in_the_overlay() {
    let first = GeometryRecord {
        panes: vec![pane(0, 0, 8, 4, PANE_FOCUSED)],
    };
    let second = GeometryRecord {
        panes: vec![pane(0, 0, 4, 4, 0), pane(4, 0, 4, 4, PANE_FOCUSED)],
    };

    let mut one = FrameBuilder::new(8, 4, 1);
    one.push_geometry(&first);
    one.push_graphics(&GraphicsRecord::Clear);
    let one = one.finish();

    let mut two = FrameBuilder::new(8, 4, 2);
    two.push_geometry(&second);
    let two = two.finish();

    let mut three = FrameBuilder::new(8, 4, 3);
    three.push_graphics(&GraphicsRecord::DeleteImage { id: 1 });
    let three = three.finish();

    let mut overlay = PendingOverlay::new(8, 4);
    assert!(!overlay.is_dirty());
    overlay.merge_frame(&decode(&one).unwrap());
    overlay.merge_frame(&decode(&two).unwrap());
    overlay.merge_frame(&decode(&three).unwrap());

    let drained = overlay.drain(4);
    let view = decode(&drained).unwrap();
    assert_eq!(view.geometry(), Some(second));
    assert_eq!(
        view.graphics(),
        vec![GraphicsRecord::Clear, GraphicsRecord::DeleteImage { id: 1 }]
    );
    assert!(!overlay.is_dirty());
}

#[test]
fn an_overlay_that_absorbed_only_a_geometry_record_is_dirty_and_drains_it_once() {
    let record = GeometryRecord {
        panes: vec![pane(0, 0, 8, 4, 0)],
    };
    let mut builder = FrameBuilder::new(8, 4, 1);
    builder.push_geometry(&record);
    let frame = builder.finish();

    let mut overlay = PendingOverlay::new(8, 4);
    overlay.merge_frame(&decode(&frame).unwrap());
    assert!(overlay.is_dirty());
    assert_eq!(decode(&overlay.drain(2)).unwrap().geometry(), Some(record));
    assert!(!overlay.is_dirty());
    assert_eq!(decode(&overlay.drain(3)).unwrap().geometry(), None);
}

fn link(id: u8, uri: &str) -> LinkEntry {
    LinkEntry {
        id,
        uri: uri.to_owned(),
    }
}

fn linked(character: char, id: u8) -> WireCell {
    WireCell {
        link: id,
        ..cell(character)
    }
}

#[test]
fn a_link_table_round_trips_every_entry_and_the_cells_that_name_them() {
    let record = LinkRecord {
        entries: vec![
            link(1, "https://example.com/one"),
            link(2, "mailto:someone@example.com"),
            link(255, ""),
        ],
    };
    let mut builder = FrameBuilder::new(6, 1, 7);
    builder.push_row(0, 0, &[linked('a', 1), linked('b', 2), cell('c')]);
    builder.push_links(&record);
    let frame = builder.finish();

    let view = decode(&frame).expect("the frame should decode");
    assert_eq!(view.links(), record);
    let cells = grid(6, 1, &frame);
    assert_eq!(cells[0].link, 1);
    assert_eq!(cells[1].link, 2);
    assert_eq!(cells[2].link, LINK_NONE);
    assert_eq!(cells[3].link, LINK_NONE);
}

#[test]
fn a_link_table_shares_the_record_stream_with_geometry_and_graphics() {
    let links = LinkRecord {
        entries: vec![link(3, "file:///tmp/x")],
    };
    let geometry = GeometryRecord {
        panes: vec![pane(0, 0, 8, 4, PANE_FRAMED)],
    };
    let mut builder = FrameBuilder::new(8, 4, 1);
    builder.push_geometry(&geometry);
    builder.push_links(&links);
    builder.extend_graphics([GraphicsRecord::Clear].iter());
    let frame = builder.finish();

    let view = decode(&frame).expect("the frame should decode");
    assert_eq!(view.links(), links);
    assert_eq!(view.geometry(), Some(geometry));
    assert_eq!(view.graphics(), vec![GraphicsRecord::Clear]);
}

#[test]
fn a_frame_without_a_link_table_reports_an_empty_one() {
    let mut builder = FrameBuilder::new(4, 1, 0);
    builder.push_row(0, 0, &run("abcd"));
    let frame = builder.finish();
    assert!(decode(&frame).unwrap().links().is_empty());
}

#[test]
fn an_empty_link_table_is_never_written_to_the_wire() {
    let mut builder = FrameBuilder::new(4, 1, 0);
    builder.push_links(&LinkRecord::default());
    let frame = builder.finish();
    assert_eq!(decode(&frame).unwrap().header().graphics_len, 0);
}

fn one_link_frame() -> (Vec<u8>, usize) {
    let mut builder = FrameBuilder::new(4, 1, 0);
    builder.push_links(&LinkRecord {
        entries: vec![link(1, "https://example.com")],
    });
    let frame = builder.finish();
    let payload =
        decode(&frame).unwrap().header().graphics_offset as usize + GRAPHICS_RECORD_HEADER_LEN;
    (frame, payload)
}

#[test]
fn a_link_table_that_names_more_entries_than_it_carries_is_rejected() {
    let (mut frame, payload) = one_link_frame();
    frame[payload] = 2;
    assert_eq!(error_of(&frame), Some(DecodeError::BadGraphicsRecord));
}

#[test]
fn a_link_entry_header_shorter_than_this_build_knows_is_rejected() {
    let (mut frame, payload) = one_link_frame();
    frame[payload + 2] = (LINK_ENTRY_HEADER_LEN - 1) as u8;
    assert_eq!(error_of(&frame), Some(DecodeError::BadGraphicsRecord));
}

#[test]
fn a_link_entry_header_wider_than_this_build_knows_is_read_and_skipped() {
    let uri = "https://example.com/widened";
    let mut payload = Vec::new();
    payload.extend_from_slice(&1u16.to_le_bytes());
    payload.extend_from_slice(&((LINK_ENTRY_HEADER_LEN + 4) as u16).to_le_bytes());
    put_u32(&mut payload, 0);
    payload.push(9);
    payload.push(0);
    payload.extend_from_slice(&(uri.len() as u16).to_le_bytes());
    payload.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
    payload.extend_from_slice(uri.as_bytes());

    let record = LinkRecord::decode(&payload).expect("a widened entry should be readable");
    assert_eq!(record.entries, vec![link(9, uri)]);
}

#[test]
fn a_link_entry_claiming_the_no_link_id_is_rejected() {
    let (mut frame, payload) = one_link_frame();
    frame[payload + LINKS_HEADER_LEN] = LINK_NONE;
    assert_eq!(error_of(&frame), Some(DecodeError::BadGraphicsRecord));
}

#[test]
fn a_link_entry_whose_uri_is_not_text_is_rejected() {
    let mut payload = Vec::new();
    payload.extend_from_slice(&1u16.to_le_bytes());
    payload.extend_from_slice(&(LINK_ENTRY_HEADER_LEN as u16).to_le_bytes());
    put_u32(&mut payload, 0);
    payload.push(1);
    payload.push(0);
    payload.extend_from_slice(&2u16.to_le_bytes());
    payload.extend_from_slice(&[0xff, 0xfe]);
    assert_eq!(LinkRecord::decode(&payload), None);
}

#[test]
fn a_link_table_that_carries_no_entries_decodes_to_an_empty_one() {
    let mut payload = Vec::new();
    payload.extend_from_slice(&0u16.to_le_bytes());
    payload.extend_from_slice(&(LINK_ENTRY_HEADER_LEN as u16).to_le_bytes());
    put_u32(&mut payload, 0);
    assert_eq!(LinkRecord::decode(&payload), Some(LinkRecord::default()));
}

#[test]
fn an_overlay_merges_link_tables_and_a_later_entry_supersedes_an_earlier_one() {
    let mut first = FrameBuilder::new(8, 2, 1);
    first.push_links(&LinkRecord {
        entries: vec![link(1, "https://first"), link(2, "https://kept")],
    });
    let mut second = FrameBuilder::new(8, 2, 2);
    second.push_links(&LinkRecord {
        entries: vec![link(1, "https://second")],
    });

    let mut overlay = PendingOverlay::new(8, 2);
    overlay.merge_frame(&decode(&first.finish()).unwrap());
    overlay.merge_frame(&decode(&second.finish()).unwrap());
    assert!(overlay.is_dirty());

    let drained = overlay.drain(3);
    assert_eq!(
        decode(&drained).unwrap().links().entries,
        vec![link(1, "https://second"), link(2, "https://kept")]
    );
    assert!(!overlay.is_dirty());
    let again = overlay.drain(4);
    assert!(decode(&again).unwrap().links().is_empty());
}

#[test]
fn a_drained_frame_carries_the_entry_a_later_row_still_names() {
    let mut first = FrameBuilder::new(4, 2, 1);
    first.push_row(0, 0, &[linked('a', 1), linked('b', 1)]);
    first.push_links(&LinkRecord {
        entries: vec![link(1, "https://example.com/held")],
    });
    let mut second = FrameBuilder::new(4, 2, 2);
    second.push_row(1, 0, &[linked('c', 1), linked('d', 1)]);

    let mut overlay = PendingOverlay::new(4, 2);
    overlay.merge_frame(&decode(&first.finish()).unwrap());
    overlay.merge_frame(&decode(&second.finish()).unwrap());

    let drained = overlay.drain(3);
    let view = decode(&drained).unwrap();
    assert_eq!(
        view.links().entries,
        vec![link(1, "https://example.com/held")],
        "the row that arrived later names an id only the earlier frame stated"
    );
    let mut cells = vec![WireCell::BLANK; 8];
    view.apply(&mut cells, 4, 2);
    assert_eq!(cells[0].link, 1);
    assert_eq!(cells[4].link, 1);
}

#[test]
fn an_overlay_drops_the_links_it_holds_when_it_absorbs_a_full_repaint() {
    let mut first = FrameBuilder::new(8, 2, 1);
    first.push_links(&LinkRecord {
        entries: vec![link(1, "https://before"), link(2, "https://gone")],
    });
    let mut second = FrameBuilder::new(8, 2, 2);
    second.set_full_repaint(true);
    second.push_links(&LinkRecord {
        entries: vec![link(1, "https://after")],
    });

    let mut overlay = PendingOverlay::new(8, 2);
    overlay.merge_frame(&decode(&first.finish()).unwrap());
    overlay.merge_frame(&decode(&second.finish()).unwrap());

    let drained = overlay.drain(3);
    assert_eq!(
        decode(&drained).unwrap().links().entries,
        vec![link(1, "https://after")]
    );
}

#[test]
fn applying_a_frame_to_a_differently_shaped_grid_writes_nothing() {
    let mut builder = FrameBuilder::new(4, 1, 0);
    builder.push_row(0, 0, &run("abcd"));
    let frame = builder.finish();
    let view = decode(&frame).unwrap();
    let mut destination = vec![WireCell::BLANK; 16];
    view.apply(&mut destination, 8, 2);
    assert!(destination.iter().all(|cell| *cell == WireCell::BLANK));
}

fn scroll(entries: &[(u16, i16)]) -> ScrollRecord {
    ScrollRecord {
        entries: entries
            .iter()
            .map(|&(pane, lines)| ScrollEntry { pane, lines })
            .collect(),
    }
}

#[test]
fn a_scroll_record_round_trips_every_entry() {
    let mut builder = FrameBuilder::new(8, 2, 1);
    builder.push_scroll(&scroll(&[(0, 5), (3, -2), (7, i16::MAX)]));
    let frame = builder.finish();
    let view = decode(&frame).unwrap();
    assert_eq!(view.header().version, WIRE_VERSION);
    assert_eq!(view.scroll(), scroll(&[(0, 5), (3, -2), (7, i16::MAX)]));
    assert_eq!(view.scroll().lines_for(3), Some(-2));
    assert_eq!(view.scroll().lines_for(1), None);
}

#[test]
fn a_scroll_record_shares_the_record_stream_with_geometry_and_links() {
    let mut builder = FrameBuilder::new(8, 2, 1);
    builder.push_geometry(&GeometryRecord {
        panes: vec![pane(0, 0, 8, 2, 0)],
    });
    builder.push_scroll(&scroll(&[(0, 1)]));
    builder.push_links(&LinkRecord {
        entries: vec![link(1, "https://example.com")],
    });
    let frame = builder.finish();
    let view = decode(&frame).unwrap();
    assert_eq!(view.geometry().unwrap().panes.len(), 1);
    assert_eq!(view.scroll(), scroll(&[(0, 1)]));
    assert_eq!(view.links().entries.len(), 1);
}

#[test]
fn an_empty_scroll_record_is_never_written_to_the_wire() {
    let mut builder = FrameBuilder::new(4, 1, 0);
    builder.push_scroll(&ScrollRecord::default());
    let frame = builder.finish();
    assert_eq!(decode(&frame).unwrap().header().graphics_len, 0);
}

#[test]
fn a_scroll_record_that_names_more_entries_than_it_carries_is_rejected() {
    let mut builder = FrameBuilder::new(4, 1, 0);
    builder.push_scroll(&scroll(&[(0, 1)]));
    let mut frame = builder.finish();
    let at = decode(&frame).unwrap().header().graphics_offset as usize;
    frame[at + GRAPHICS_RECORD_HEADER_LEN] = 2;
    assert_eq!(error_of(&frame), Some(DecodeError::BadGraphicsRecord));
}

#[test]
fn a_scroll_record_declaring_a_shorter_entry_than_this_build_knows_is_rejected() {
    let mut builder = FrameBuilder::new(4, 1, 0);
    builder.push_scroll(&scroll(&[(0, 1)]));
    let mut frame = builder.finish();
    let at = decode(&frame).unwrap().header().graphics_offset as usize;
    frame[at + GRAPHICS_RECORD_HEADER_LEN + 2] = 2;
    assert_eq!(error_of(&frame), Some(DecodeError::BadGraphicsRecord));
}

#[test]
fn adding_to_a_scroll_record_sums_per_pane_and_drops_a_zero_sum() {
    let mut record = ScrollRecord::default();
    record.add(1, 3);
    record.add(2, -1);
    record.add(1, 2);
    assert_eq!(record, scroll(&[(1, 5), (2, -1)]));
    record.add(2, 1);
    assert_eq!(record, scroll(&[(1, 5)]));
    record.add(1, 0);
    assert_eq!(record, scroll(&[(1, 5)]));
}

fn scroll_frame(seq: u64, entries: &[(u16, i16)]) -> Vec<u8> {
    let mut builder = FrameBuilder::new(4, 2, seq);
    builder.push_row(0, 0, &run("ab"));
    builder.push_scroll(&scroll(entries));
    builder.finish()
}

#[test]
fn an_overlay_sums_the_scroll_hints_of_the_frames_it_absorbs_and_drains_them_once() {
    let mut overlay = PendingOverlay::new(4, 2);
    overlay.merge_frame(&decode(&scroll_frame(1, &[(0, 2), (1, -1)])).unwrap());
    overlay.merge_frame(&decode(&scroll_frame(2, &[(0, 3)])).unwrap());
    let drained = overlay.drain(3);
    assert_eq!(
        decode(&drained).unwrap().scroll(),
        scroll(&[(0, 5), (1, -1)])
    );
    let again = overlay.drain(4);
    assert!(decode(&again).unwrap().scroll().is_empty());
}

#[test]
fn an_overlay_that_absorbed_only_a_scroll_hint_is_dirty() {
    let mut builder = FrameBuilder::new(4, 2, 1);
    builder.push_scroll(&scroll(&[(0, 1)]));
    let mut overlay = PendingOverlay::new(4, 2);
    let frame = builder.finish();
    let view = decode(&frame).unwrap();
    overlay.merge_frame(&view);
    assert!(overlay.is_dirty());
}

#[test]
fn an_overlay_drops_held_scroll_hints_when_it_absorbs_new_geometry() {
    let mut overlay = PendingOverlay::new(4, 2);
    overlay.merge_frame(&decode(&scroll_frame(1, &[(0, 2)])).unwrap());
    let mut reshaped = FrameBuilder::new(4, 2, 2);
    reshaped.push_geometry(&GeometryRecord {
        panes: vec![pane(0, 0, 4, 2, 0)],
    });
    overlay.merge_frame(&decode(&reshaped.finish()).unwrap());
    overlay.merge_frame(&decode(&scroll_frame(3, &[(0, 1)])).unwrap());
    let drained = overlay.drain(4);
    let view = decode(&drained).unwrap();
    assert!(view.geometry().is_some());
    assert!(view.scroll().is_empty());
}

#[test]
fn an_overlay_drops_held_scroll_hints_when_it_absorbs_a_full_repaint() {
    let mut overlay = PendingOverlay::new(4, 2);
    overlay.merge_frame(&decode(&scroll_frame(1, &[(0, 2)])).unwrap());
    let mut repaint = FrameBuilder::new(4, 2, 2);
    repaint.set_full_repaint(true);
    overlay.merge_frame(&decode(&repaint.finish()).unwrap());
    let drained = overlay.drain(3);
    assert!(decode(&drained).unwrap().scroll().is_empty());
}

fn cluster_cell(builder: &mut FrameBuilder, text: &str, width: u8, attrs: u16) -> WireCell {
    WireCell {
        ch: builder.cluster(text).unwrap(),
        width,
        attrs,
        ..WireCell::BLANK
    }
}

fn texts(view: &FrameView<'_>, cells: &[WireCell]) -> Vec<String> {
    let clusters = view.clusters();
    cells
        .iter()
        .map(|c| c.text(&clusters).into_owned())
        .collect()
}

fn graphics_payload_with_clusters(entries: &[(u16, &[u8])]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    payload.extend_from_slice(&(CLUSTER_ENTRY_HEADER_LEN as u16).to_le_bytes());
    put_u32(&mut payload, 0);
    for (id, text) in entries {
        payload.extend_from_slice(&id.to_le_bytes());
        payload.extend_from_slice(&0u16.to_le_bytes());
        payload.extend_from_slice(&(text.len() as u16).to_le_bytes());
        payload.extend_from_slice(text);
    }
    payload
}

#[test]
fn cluster_cells_groups_and_wide_cells_round_trip() {
    let mut builder = FrameBuilder::new(10, 2, 1);
    let family_head = cluster_cell(&mut builder, "\u{1F468}\u{200D}", 2, 0);
    let family_middle = cluster_cell(&mut builder, "\u{1F469}\u{200D}", 2, ATTR_GROUP_CONTINUE);
    let family_tail = WireCell {
        ch: '\u{1F467}' as u32,
        width: 2,
        attrs: ATTR_GROUP_CONTINUE,
        ..WireCell::BLANK
    };
    let filler = WireCell::BLANK;
    let wide = WireCell {
        ch: '\u{17D8}' as u32,
        width: 3,
        ..WireCell::BLANK
    };
    let row = vec![
        family_head,
        filler,
        family_middle,
        filler,
        family_tail,
        filler,
        wide,
        filler,
        filler,
    ];
    builder.push_row(0, 0, &row);
    let frame = builder.finish();
    let view = decode(&frame).unwrap();
    assert_eq!(view.header().version, WIRE_VERSION);
    let (_, cells) = view.rows().next().unwrap();
    assert_eq!(
        texts(&view, &cells),
        vec![
            "\u{1F468}\u{200D}",
            " ",
            "\u{1F469}\u{200D}",
            " ",
            "\u{1F467}",
            " ",
            "\u{17D8}",
            " ",
            " "
        ]
    );
    let continuations: Vec<bool> = cells.iter().map(|c| c.is_group_continuation()).collect();
    assert_eq!(
        continuations,
        vec![false, false, true, false, true, false, false, false, false]
    );
    assert_eq!(cells[6].width, 3);
    assert_eq!(cells[0].character(), ' ');
}

#[test]
fn a_frame_without_clusters_carries_no_cluster_record() {
    let mut builder = FrameBuilder::new(4, 1, 1);
    builder.push_row(0, 0, &run("abcd"));
    let frame = builder.finish();
    let view = decode(&frame).unwrap();
    assert!(view.clusters().is_empty());
    assert_eq!(view.header().graphics_len, 0);
}

#[test]
fn identical_cluster_texts_share_one_id_within_a_frame() {
    let mut builder = FrameBuilder::new(4, 1, 1);
    let first = builder.cluster("e\u{0301}").unwrap();
    let second = builder.cluster("e\u{0301}").unwrap();
    let third = builder.cluster("a\u{0301}").unwrap();
    assert_eq!(first, second);
    assert_ne!(first, third);
    let cells: Vec<WireCell> = [first, second, third]
        .iter()
        .map(|ch| WireCell {
            ch: *ch,
            ..WireCell::BLANK
        })
        .collect();
    builder.push_row(0, 0, &cells);
    let frame = builder.finish();
    assert_eq!(decode(&frame).unwrap().clusters().entries.len(), 2);
}

#[test]
fn version_three_frames_decode_but_may_not_carry_clusters() {
    let mut builder = FrameBuilder::new(4, 1, 1);
    builder.push_row(0, 0, &run("abcd"));
    let mut frame = builder.finish();
    frame[0] = 3;
    assert!(decode(&frame).is_ok());

    let mut builder = FrameBuilder::new(4, 1, 1);
    let cell = cluster_cell(&mut builder, "e\u{0301}", 1, 0);
    builder.push_row(0, 0, &[cell, cell, cell, cell]);
    let mut frame = builder.finish();
    frame[0] = 3;
    assert!(decode(&frame).is_err());
}

#[test]
fn malformed_cluster_records_are_rejected() {
    assert!(ClusterRecord::decode(&graphics_payload_with_clusters(&[(0, b"\xff\xfe")])).is_none());
    assert!(ClusterRecord::decode(&graphics_payload_with_clusters(&[(0, b"")])).is_none());
    assert!(
        ClusterRecord::decode(&graphics_payload_with_clusters(&[(0, b"ab"), (0, b"cd")])).is_none()
    );
    let oversize = vec![b'a'; MAX_CLUSTER_ENTRY_LEN + 1];
    assert!(ClusterRecord::decode(&graphics_payload_with_clusters(&[(0, &oversize)])).is_none());
    let entry = vec![b'a'; MAX_CLUSTER_ENTRY_LEN];
    let many: Vec<(u16, &[u8])> = (0..300u16).map(|id| (id, entry.as_slice())).collect();
    assert!(ClusterRecord::decode(&graphics_payload_with_clusters(&many)).is_none());
    assert_eq!(
        ClusterRecord::decode(&graphics_payload_with_clusters(&[(3, b"ok")]))
            .unwrap()
            .get(3),
        Some("ok")
    );
}

#[test]
fn a_cell_referring_to_a_missing_cluster_is_rejected() {
    let mut builder = FrameBuilder::new(2, 1, 1);
    let real = cluster_cell(&mut builder, "e\u{0301}", 1, 0);
    let missing = WireCell {
        ch: CLUSTER_CH_BASE + 7,
        ..WireCell::BLANK
    };
    builder.push_row(0, 0, &[real, missing]);
    assert!(decode(&builder.finish()).is_err());
}

#[test]
fn the_overlay_remaps_cluster_ids_from_several_frames() {
    let mut first = FrameBuilder::new(4, 2, 1);
    let a = cluster_cell(&mut first, "a\u{0301}", 1, 0);
    let b = cluster_cell(&mut first, "b\u{0301}", 1, 0);
    first.push_row(0, 0, &[a, b]);
    let first = first.finish();

    let mut second = FrameBuilder::new(4, 2, 2);
    let c = cluster_cell(&mut second, "c\u{0301}", 1, 0);
    let b_again = cluster_cell(&mut second, "b\u{0301}", 1, 0);
    second.push_row(1, 0, &[c, b_again]);
    let second = second.finish();

    let mut overlay = PendingOverlay::new(4, 2);
    overlay.merge_frame(&decode(&first).unwrap());
    overlay.merge_frame(&decode(&second).unwrap());
    let drained = overlay.drain(3);
    let view = decode(&drained).unwrap();
    let rows: Vec<(RowRecord, Vec<WireCell>)> = view.rows().collect();
    let row0 = rows.iter().find(|(r, _)| r.y == 0).unwrap();
    let row1 = rows.iter().find(|(r, _)| r.y == 1).unwrap();
    assert_eq!(texts(&view, &row0.1), vec!["a\u{0301}", "b\u{0301}"]);
    assert_eq!(texts(&view, &row1.1), vec!["c\u{0301}", "b\u{0301}"]);
    assert_eq!(view.clusters().entries.len(), 3);
}

#[test]
fn oversized_link_uris_are_omitted_without_corrupting_the_record() {
    let maximum = "a".repeat(MAX_LINK_URI_LEN);
    let record = LinkRecord {
        entries: vec![
            link(1, &"a".repeat(MAX_LINK_URI_LEN + 1)),
            link(2, &("界".to_owned() + &"a".repeat(MAX_LINK_URI_LEN - 1))),
            link(3, &maximum),
            link(4, "https://example.com"),
        ],
    };
    let mut builder = FrameBuilder::new(4, 1, 0);
    builder.push_links(&record);
    let frame = builder.finish();
    assert_eq!(decode(&frame).unwrap().links().entries, record.entries[2..]);
}
