use super::*;
use yinz_types::structured_render::{CursorState, FrameBuilder, WireCell, CURSOR_SHAPE_BEAM};

fn run(text: &str) -> Vec<WireCell> {
    text.chars()
        .map(|character| WireCell {
            ch: character as u32,
            ..WireCell::BLANK
        })
        .collect()
}

fn frame_of(cols: u16, rows: u16, seq: u64, sideband: &[u8]) -> Vec<u8> {
    let mut builder = FrameBuilder::new(cols, rows, seq);
    builder.sideband_mut().extend_from_slice(sideband);
    builder.finish()
}

fn sixel_image(width: usize, bands: usize) -> Vec<u8> {
    let mut body = format!("\u{1b}P0;1;0q\"1;1;{};{}", width, bands * 6);
    for _ in 0..bands {
        body.push_str(&format!("#0;2;100;0;0!{}~-", width));
    }
    body.push_str("\u{1b}\\");
    body.into_bytes()
}

fn row_text(state: &TerminalState, row: usize) -> String {
    (0..state.size().cols)
        .map(|col| state.screen().cell(row, col).character())
        .collect()
}

#[test]
fn a_frame_paints_the_grid_and_reports_its_sequence_number() {
    let mut state = TerminalState::new(2, 8);
    let mut builder = FrameBuilder::new(8, 2, 11);
    builder.push_row(0, 0, &run("hello"));
    builder.set_cursor(CursorState {
        x: 5,
        y: 0,
        shape: CURSOR_SHAPE_BEAM,
        visible: true,
    });
    assert_eq!(state.apply_frame(&builder.finish()).unwrap().seq, 11);
    assert_eq!(row_text(&state, 0), "hello   ");
    assert_eq!(state.cursor_position(), (0, 5));
    assert!(state.cursor_is_visible());
    assert_eq!(state.cursor_shape(), CursorShape::Beam);
}

#[test]
fn a_malformed_frame_is_refused_rather_than_partly_applied() {
    let mut state = TerminalState::new(2, 8);
    let mut builder = FrameBuilder::new(8, 2, 1);
    builder.push_row(0, 0, &run("kept"));
    state.apply_frame(&builder.finish()).unwrap();

    assert!(matches!(
        state.apply_frame(b"nonsense"),
        Err(FrameError::Undecodable(_))
    ));
    let mut reshaped = FrameBuilder::new(4, 1, 2);
    reshaped.push_row(0, 0, &run("no"));
    assert!(matches!(
        state.apply_frame(&reshaped.finish()),
        Err(FrameError::Unapplicable { seq: 2, .. })
    ));
    assert_eq!(row_text(&state, 0), "kept    ");
}

#[test]
fn the_frame_header_rather_than_the_client_decides_the_viewport() {
    let mut state = TerminalState::new(2, 8);
    let mut builder = FrameBuilder::new(20, 5, 0);
    builder.set_full_repaint(true);
    builder.push_row(4, 0, &run("wider"));
    state.apply_frame(&builder.finish()).unwrap();

    assert_eq!(state.size(), TermSize { rows: 5, cols: 20 });
    assert_eq!(row_text(&state, 4), "wider               ");
}

#[test]
fn a_frame_claiming_an_empty_viewport_is_dropped_rather_than_collapsing_the_grid() {
    let mut state = TerminalState::new(2, 8);
    let mut builder = FrameBuilder::new(0, 0, 4);
    builder.set_full_repaint(true);
    assert!(matches!(
        state.apply_frame(&builder.finish()),
        Err(FrameError::Unapplicable { seq: 4, .. })
    ));
    assert_eq!(state.size(), TermSize { rows: 2, cols: 8 });
}

#[test]
fn a_title_reaches_the_signals_through_the_sideband() {
    let mut state = TerminalState::new(2, 8);
    state
        .apply_frame(&frame_of(8, 2, 0, b"\x1b]0;yinz - fish\x07"))
        .unwrap();
    assert_eq!(state.take_title(), Some(Some("yinz - fish".to_owned())));
    assert_eq!(state.take_title(), None);
}

#[test]
fn an_empty_title_resets_it() {
    let mut state = TerminalState::new(2, 8);
    state
        .apply_frame(&frame_of(8, 2, 0, b"\x1b]2;\x1b\\"))
        .unwrap();
    assert_eq!(state.take_title(), Some(None));
}

#[test]
fn bells_are_counted_and_drained() {
    let mut state = TerminalState::new(2, 8);
    state.apply_frame(&frame_of(8, 2, 0, b"\x07\x07")).unwrap();
    assert_eq!(state.take_bells(), 2);
    assert_eq!(state.take_bells(), 0);
}

#[test]
fn osc_9_and_osc_99_notifications_are_captured_with_their_bodies() {
    let mut state = TerminalState::new(2, 8);
    state
        .apply_frame(&frame_of(8, 2, 0, b"\x1b]9;a job finished\x07"))
        .unwrap();
    state
        .apply_frame(&frame_of(8, 2, 1, b"\x1b]99;i=1:p=body;the body\x1b\\"))
        .unwrap();
    assert_eq!(
        state.take_notifications(),
        vec![
            Notification {
                title: None,
                body: "a job finished".to_owned()
            },
            Notification {
                title: None,
                body: "the body".to_owned()
            }
        ]
    );
    assert!(state.take_notifications().is_empty());
}

#[test]
fn an_osc_99_title_and_body_arrive_as_one_notification() {
    let mut state = TerminalState::new(2, 8);
    state
        .apply_frame(&frame_of(
            8,
            2,
            0,
            b"\x1b]99;i=7:d=0:p=title;Build\x1b\\\x1b]99;i=7:p=body;finished\x1b\\",
        ))
        .unwrap();
    assert_eq!(
        state.take_notifications(),
        vec![Notification {
            title: Some("Build".to_owned()),
            body: "finished".to_owned()
        }],
        "the two chunks of one identifier are one notification, not two"
    );
}

#[test]
fn an_unfinished_osc_99_notification_waits_for_its_last_chunk() {
    let mut state = TerminalState::new(2, 8);
    state
        .apply_frame(&frame_of(8, 2, 0, b"\x1b]99;i=3:d=0:p=body;half \x1b\\"))
        .unwrap();
    assert!(
        state.take_notifications().is_empty(),
        "a chunk that says more is coming must not be shown on its own"
    );
    state
        .apply_frame(&frame_of(8, 2, 1, b"\x1b]99;i=3:p=body;done\x1b\\"))
        .unwrap();
    assert_eq!(
        state.take_notifications(),
        vec![Notification {
            title: None,
            body: "half done".to_owned()
        }]
    );
}

#[test]
fn an_osc_99_body_encoded_in_base64_is_decoded() {
    let mut state = TerminalState::new(2, 8);
    state
        .apply_frame(&frame_of(8, 2, 0, b"\x1b]99;i=1:e=1:p=body;ZG9uZQ==\x1b\\"))
        .unwrap();
    assert_eq!(
        state.take_notifications(),
        vec![Notification {
            title: None,
            body: "done".to_owned()
        }]
    );
}

#[test]
fn an_osc_99_notification_that_is_only_a_title_is_shown_as_a_body() {
    let mut state = TerminalState::new(2, 8);
    state
        .apply_frame(&frame_of(8, 2, 0, b"\x1b]99;;Build: finished\x1b\\"))
        .unwrap();
    assert_eq!(
        state.take_notifications(),
        vec![Notification {
            title: None,
            body: "Build: finished".to_owned()
        }],
        "the server joins title and body when it re-encodes, and there is nothing to split"
    );
}

#[test]
fn an_osc_99_control_request_is_not_a_notification() {
    let mut state = TerminalState::new(2, 8);
    state
        .apply_frame(&frame_of(8, 2, 0, b"\x1b]99;i=1:p=close;\x1b\\"))
        .unwrap();
    assert!(
        state.take_notifications().is_empty(),
        "a close or query is a protocol request, not something to show"
    );
}

#[test]
fn an_osc_52_store_reaches_the_clipboard_and_drains_once() {
    let mut state = TerminalState::new(2, 8);
    state
        .apply_frame(&frame_of(8, 2, 0, b"\x1b]52;c;Y29waWVk\x1b\\"))
        .unwrap();
    assert_eq!(state.take_copied_text().as_deref(), Some("copied"));
    assert_eq!(state.take_copied_text(), None);
}

#[test]
fn an_osc_52_read_request_is_not_mistaken_for_a_store() {
    let mut state = TerminalState::new(2, 8);
    state
        .apply_frame(&frame_of(8, 2, 0, b"\x1b]52;c;?\x1b\\"))
        .unwrap();
    assert_eq!(state.take_copied_text(), None);
}

fn rgba(pixel: [u8; 4], count: usize) -> Vec<u8> {
    pixel.repeat(count)
}

fn residency(id: u64, medium: GraphicsMedium, data: Vec<u8>) -> GraphicsRecord {
    GraphicsRecord::Residency {
        id,
        width: 2,
        height: 2,
        format: GRAPHICS_FORMAT_RGBA8,
        medium,
        byte_len: 16,
        data,
    }
}

fn placement(image_id: u64, placement_id: u64) -> GraphicsRecord {
    GraphicsRecord::Placement {
        image_id,
        placement_id,
        cell_x: 2,
        cell_y: 3,
        offset_x: 0,
        offset_y: 0,
        source_x: 0,
        source_y: 0,
        source_width: 2,
        source_height: 2,
        z: 0,
    }
}

fn apply_graphics(state: &mut TerminalState, records: &[GraphicsRecord]) {
    let size = state.size();
    let mut builder = FrameBuilder::new(size.cols as u16, size.rows as u16, 0);
    builder.extend_graphics(records.iter());
    state
        .apply_frame(&builder.finish())
        .expect("a usable frame");
}

#[test]
fn an_inline_residency_and_its_placement_reach_the_window_as_pixels() {
    let mut state = TerminalState::new(6, 12);
    let pixels = rgba([200, 10, 10, 255], 4);
    apply_graphics(
        &mut state,
        &[
            residency(7, GraphicsMedium::Inline, pixels.clone()),
            placement(7, 1),
        ],
    );

    let image = state.graphics().image(7).expect("no resident image");
    assert_eq!((image.width, image.height), (2, 2));
    assert_eq!(image.pixels, pixels);
    let placed = *state.graphics().placements().next().expect("no placement");
    assert_eq!((placed.cell_y, placed.cell_x), (3, 2));
    assert_eq!((placed.source_width, placed.source_height), (2, 2));
}

#[test]
fn a_shared_file_residency_is_read_from_the_path_it_names() {
    let directory = std::env::temp_dir();
    let path = directory.join(format!("yinz-terminal-test-{}", std::process::id()));
    let pixels = rgba([0, 0, 255, 255], 4);
    std::fs::write(&path, &pixels).unwrap();

    let mut state = TerminalState::new(6, 12);
    apply_graphics(
        &mut state,
        &[
            residency(
                9,
                GraphicsMedium::SharedFile,
                path.to_string_lossy().into_owned().into_bytes(),
            ),
            placement(9, 1),
        ],
    );
    let _ = std::fs::remove_file(&path);

    assert_eq!(state.graphics().image(9).unwrap().pixels, pixels);
    assert_eq!(state.graphics().placements().count(), 1);
}

#[test]
fn a_shared_file_residency_is_refused_when_shared_media_is_off() {
    let directory = std::env::temp_dir();
    let path = directory.join(format!("yinz-terminal-remote-test-{}", std::process::id()));
    let pixels = rgba([0, 0, 255, 255], 4);
    std::fs::write(&path, &pixels).unwrap();

    let mut state = TerminalState::new(6, 12);
    state.set_shared_media(false);
    apply_graphics(
        &mut state,
        &[
            residency(
                9,
                GraphicsMedium::SharedFile,
                path.to_string_lossy().into_owned().into_bytes(),
            ),
            placement(9, 1),
        ],
    );
    let _ = std::fs::remove_file(&path);

    assert!(state.graphics().image(9).is_none());
}

#[test]
fn a_residency_naming_a_path_outside_the_media_directories_is_refused() {
    let mut state = TerminalState::new(6, 12);
    apply_graphics(
        &mut state,
        &[residency(
            9,
            GraphicsMedium::SharedFile,
            b"/etc/passwd".to_vec(),
        )],
    );
    assert!(state.graphics().image(9).is_none());
}

#[test]
fn a_placement_of_an_image_that_never_arrived_is_dropped() {
    let mut state = TerminalState::new(6, 12);
    apply_graphics(&mut state, &[placement(3, 1)]);
    assert_eq!(state.graphics().placements().count(), 0);
}

#[test]
fn deletions_retire_placements_and_images_independently() {
    let mut state = TerminalState::new(6, 12);
    apply_graphics(
        &mut state,
        &[
            residency(1, GraphicsMedium::Inline, rgba([1, 2, 3, 255], 4)),
            placement(1, 1),
            placement(1, 2),
        ],
    );
    assert_eq!(state.graphics().placements().count(), 2);

    apply_graphics(
        &mut state,
        &[GraphicsRecord::DeletePlacement {
            image_id: 1,
            placement_id: 2,
        }],
    );
    assert_eq!(state.graphics().placements().count(), 1);
    assert!(state.graphics().image(1).is_some());

    apply_graphics(&mut state, &[GraphicsRecord::DeleteImage { id: 1 }]);
    assert_eq!(state.graphics().placements().count(), 0);
    assert!(state.graphics().image(1).is_none());
}

#[test]
fn a_clear_record_retires_every_image_and_placement() {
    let mut state = TerminalState::new(6, 12);
    apply_graphics(
        &mut state,
        &[
            residency(1, GraphicsMedium::Inline, rgba([1, 2, 3, 255], 4)),
            placement(1, 1),
        ],
    );
    apply_graphics(&mut state, &[GraphicsRecord::Clear]);
    assert!(state.graphics().resident_ids().is_empty());
    assert_eq!(state.graphics().placements().count(), 0);
}

#[test]
fn a_typed_sixel_chunk_lands_at_the_cell_its_record_names() {
    let mut state = TerminalState::new(6, 12);
    state.set_cell_size(8, 20);
    apply_graphics(
        &mut state,
        &[GraphicsRecord::SixelChunk {
            cell_x: 3,
            cell_y: 2,
            pixel_x: 0,
            pixel_y: 0,
            pixel_width: 16,
            pixel_height: 6,
            payload: sixel_image(16, 1),
        }],
    );

    let chunk = state.sixels().chunks().next().expect("no chunk");
    assert_eq!((chunk.cell_y, chunk.cell_x), (2, 3));
    assert_eq!((chunk.width, chunk.height), (16, 6));
}

#[test]
fn a_residency_whose_pixels_fall_short_of_its_dimensions_is_refused() {
    let mut state = TerminalState::new(6, 12);
    apply_graphics(
        &mut state,
        &[GraphicsRecord::Residency {
            id: 5,
            width: 8,
            height: 8,
            format: GRAPHICS_FORMAT_RGBA8,
            medium: GraphicsMedium::Inline,
            byte_len: 16,
            data: rgba([1, 2, 3, 255], 4),
        }],
    );
    assert!(state.graphics().image(5).is_none());
}

fn sixel_at(cell_y: u32, cell_x: u32, width: u32, bands: u32) -> GraphicsRecord {
    GraphicsRecord::SixelChunk {
        cell_x,
        cell_y,
        pixel_x: 0,
        pixel_y: 0,
        pixel_width: width,
        pixel_height: bands * 6,
        payload: sixel_image(width as usize, bands as usize),
    }
}

#[test]
fn a_full_repaint_retires_the_graphics_the_server_stopped_referencing() {
    let mut state = TerminalState::new(6, 12);
    apply_graphics(
        &mut state,
        &[
            residency(1, GraphicsMedium::Inline, rgba([1, 2, 3, 255], 4)),
            placement(1, 1),
        ],
    );
    assert_eq!(state.graphics().resident_ids(), vec![1]);

    let mut second = FrameBuilder::new(12, 6, 1);
    second.set_full_repaint(true);
    second.extend_graphics(
        [
            residency(2, GraphicsMedium::Inline, rgba([4, 5, 6, 255], 4)),
            placement(2, 1),
        ]
        .iter(),
    );
    state.apply_frame(&second.finish()).unwrap();

    assert_eq!(state.graphics().resident_ids(), vec![2]);
    let placements: Vec<_> = state.graphics().placements().collect();
    assert_eq!(placements.len(), 1);
    assert_eq!((placements[0].cell_y, placements[0].cell_x), (3, 2));
}

#[test]
fn a_row_record_erases_the_part_of_a_sixel_it_covers() {
    let mut state = TerminalState::new(6, 12);
    state.set_cell_size(8, 20);
    apply_graphics(&mut state, &[sixel_at(0, 0, 16, 7)]);
    let before = state.sixels().chunks().next().unwrap().revision;

    let mut builder = FrameBuilder::new(12, 6, 1);
    builder.push_row(1, 0, &run("covered"));
    state.apply_frame(&builder.finish()).unwrap();

    let chunk = state.sixels().chunks().next().expect("the chunk vanished");
    assert!(chunk.revision > before, "the band was not erased");
    let stride = (chunk.width * 4) as usize;
    assert_eq!(&chunk.image.pixels[..4], &[255, 0, 0, 255]);
    assert_eq!(&chunk.image.pixels[stride * 20..stride * 20 + 4], &[0; 4]);
}

#[test]
fn a_row_record_beside_a_sixel_leaves_it_alone() {
    let mut state = TerminalState::new(6, 20);
    state.set_cell_size(8, 20);
    apply_graphics(&mut state, &[sixel_at(0, 0, 16, 7)]);
    let before = state.sixels().chunks().next().unwrap().revision;

    let mut builder = FrameBuilder::new(20, 6, 1);
    builder.push_row(1, 4, &run("neighbour"));
    state.apply_frame(&builder.finish()).unwrap();

    assert_eq!(
        state.sixels().chunks().next().unwrap().revision,
        before,
        "a write in the next pane's columns erased the image"
    );
}

#[test]
fn a_sixel_carried_by_the_frame_that_repaints_around_it_survives() {
    let mut state = TerminalState::new(6, 20);
    state.set_cell_size(8, 20);
    let mut builder = FrameBuilder::new(20, 6, 0);
    builder.push_row(0, 0, &run("status"));
    builder.extend_graphics([sixel_at(0, 0, 16, 2)].iter());
    state.apply_frame(&builder.finish()).unwrap();

    assert_eq!(state.sixels().chunks().count(), 1);
}

#[test]
fn a_reshaping_frame_retires_the_sixel_positioned_against_the_old_grid() {
    let mut state = TerminalState::new(6, 12);
    state.set_cell_size(8, 20);
    apply_graphics(&mut state, &[sixel_at(1, 0, 16, 1)]);
    assert_eq!(state.sixels().chunks().count(), 1);

    let mut reshaped = FrameBuilder::new(20, 8, 1);
    reshaped.set_full_repaint(true);
    state.apply_frame(&reshaped.finish()).unwrap();
    assert!(state.sixels().chunks().next().is_none());
    assert_eq!(state.size(), TermSize { rows: 8, cols: 20 });
}

#[test]
fn a_clear_from_a_smaller_client_attaching_leaves_nothing_outside_the_new_area() {
    let mut state = TerminalState::new(4, 10);
    let mut before = FrameBuilder::new(10, 4, 0);
    before.set_full_repaint(true);
    for row in 0..4 {
        before.push_row(row, 0, &run("wide-pane!"));
    }
    state.apply_frame(&before.finish()).unwrap();

    let mut shrunk = FrameBuilder::new(10, 4, 1);
    shrunk.set_clear(true);
    shrunk.push_row(0, 0, &run("small"));
    shrunk.push_row(1, 0, &run("small"));
    state.apply_frame(&shrunk.finish()).unwrap();

    let text = |row: usize| -> String {
        (0..10)
            .map(|col| state.screen().cell(row, col).character())
            .collect()
    };
    assert_eq!(text(0), "small     ");
    assert_eq!(text(1), "small     ");
    assert_eq!(text(2), "          ");
    assert_eq!(text(3), "          ");
}

#[test]
fn degenerate_dimensions_are_clamped_rather_than_panicking() {
    let state = TerminalState::new(0, 0);
    assert_eq!(state.size(), TermSize { rows: 1, cols: 1 });
}

#[test]
fn large_clipboard_updates_survive_fragmented_frame_sidebands() {
    use base64::Engine as _;
    for text in ["a".repeat(7000), "界\n".repeat(7000)] {
        for terminator in ["\x07", "\x1b\\"] {
            let encoded = base64::engine::general_purpose::STANDARD.encode(&text);
            let content = format!("\x1b]52;c;{encoded}{terminator}");
            let mut state = TerminalState::new(2, 8);
            let split = content.len() - terminator.len();
            state
                .apply_frame(&frame_of(8, 2, 0, &content.as_bytes()[..split]))
                .unwrap();
            assert_eq!(state.take_copied_text(), None);
            state
                .apply_frame(&frame_of(8, 2, 1, &content.as_bytes()[split..]))
                .unwrap();
            assert_eq!(state.take_copied_text(), Some(text.clone()));
        }
    }
}

#[test]
fn render_negotiation_commits_only_consumed_media_with_full_width_ids() {
    let mut state = TerminalState::new(6, 12);
    state.negotiate(caps::Agreement::default(), None);
    let high = (1u64 << 40) + 7;
    let mut baseline = FrameBuilder::new(12, 6, 5);
    baseline.set_full_repaint(true);
    baseline.push_geometry(&GeometryRecord {
        panes: vec![structured_render::PaneRect {
            cols: 12,
            rows: 6,
            ..Default::default()
        }],
    });
    baseline.extend_graphics(&[
        residency(7, GraphicsMedium::Inline, rgba([1, 2, 3, 255], 4)),
        placement(7, 9),
        residency(high, GraphicsMedium::Inline, rgba([9, 8, 7, 255], 4)),
        placement(high, 9 + (1 << 40)),
    ]);
    let frame = baseline.finish();
    let first = state.apply_frame(&frame).unwrap();
    assert_eq!(first.seq, 5);
    assert!(first.cleared);
    assert_eq!(state.graphics().resident_ids(), vec![7, high]);
    use crate::render::scene::{resident_images, ImageKey};
    assert_eq!(
        resident_images(&state),
        vec![ImageKey::Kitty(7), ImageKey::Kitty(high)]
    );
    assert_eq!(
        state.graphics().image(high).unwrap().pixels,
        rgba([9, 8, 7, 255], 4)
    );
    assert_eq!(state.graphics().placements().count(), 2);
    let stamp = state.graphics_stamp();
    let duplicate = state.apply_frame(&frame).unwrap();
    assert_eq!(duplicate.seq, 5);
    assert!(!duplicate.cleared);
    assert_eq!(duplicate.damage, Damage::Rows(vec![]));
    assert_eq!(stamp, state.graphics_stamp());

    let mut remove = FrameBuilder::new(12, 6, 6);
    remove.extend_graphics(&[
        GraphicsRecord::DeletePlacement {
            image_id: high,
            placement_id: 9 + (1 << 40),
        },
        GraphicsRecord::DeleteImage { id: high },
    ]);
    state.apply_frame(&remove.finish()).unwrap();
    for broken in [
        placement(high, 9),
        GraphicsRecord::SixelChunk {
            cell_x: 0,
            cell_y: 0,
            pixel_x: 0,
            pixel_y: 0,
            pixel_width: 1,
            pixel_height: 1,
            payload: vec![],
        },
        residency(8, GraphicsMedium::SharedFile, b"/tmp/unproven".to_vec()),
    ] {
        let mut frame = FrameBuilder::new(12, 6, 7);
        frame.push_row(0, 0, &run("bad"));
        frame.sideband_mut().push(7);
        frame.extend_graphics(&[GraphicsRecord::DeleteImage { id: 7 }, broken]);
        assert!(state.apply_frame(&frame.finish()).is_err());
        assert_eq!(state.applied_seq, Some(6));
        assert_eq!(state.take_bells(), 0);
        assert_eq!(state.graphics().resident_ids(), vec![7]);
        assert_eq!(state.graphics().placements().count(), 1);
        assert!(!row_text(&state, 0).contains("bad"));
    }
    assert!(state.apply_frame(&frame_of(12, 6, 8, b"")).is_err());
}

#[test]
fn render_negotiation_rejects_cancelling_scroll_records_before_apply() {
    let mut state = TerminalState::new(2, 8);
    state.negotiate(caps::Agreement::default(), None);
    let mut frame = FrameBuilder::new(8, 2, 1);
    frame.set_full_repaint(true);
    frame.push_row(0, 0, &run("changed"));
    for lines in [1, -1] {
        let mut scroll = structured_render::ScrollRecord::default();
        scroll.add(0, lines);
        frame.push_scroll(&scroll);
    }
    let frame = frame.finish();
    let view = structured_render::decode(&frame).unwrap();
    assert!(view.scroll().is_empty());
    assert!(view.has_scroll_record());
    assert!(
        matches!(state.apply_frame(&frame), Err(FrameError::Protocol(1, reason)) if reason.contains("unnegotiated structured section"))
    );
    assert_eq!(state.applied_seq, None);
    assert!(!row_text(&state, 0).contains("changed"));
}

#[test]
fn render_negotiation_row_shifts_require_an_incremental_fitting_rectangle() {
    use structured_render::{ShiftEntry, ShiftRecord};
    let mut state = TerminalState::new(3, 6);
    let mut agreement = caps::Agreement::default();
    agreement.features |= caps::SHIFTS;
    state.negotiate(agreement, None);
    let mut base = FrameBuilder::new(6, 3, 1);
    base.set_full_repaint(true);
    for (i, row) in ["abcdef", "ghijkl", "mnopqr"].into_iter().enumerate() {
        base.push_row(i as u16, 0, &run(row));
    }
    state.apply_frame(&base.finish()).unwrap();
    let shift = ShiftEntry {
        x: 1,
        y: 0,
        cols: 3,
        rows: 2,
        lines: 1,
    };
    for (full, clear, width, entries) in [
        (true, false, 6, vec![shift]),
        (false, true, 6, vec![shift]),
        (false, false, 7, vec![shift]),
        (false, false, 6, vec![shift, ShiftEntry { x: 5, ..shift }]),
    ] {
        let mut frame = FrameBuilder::new(width, 3, 2);
        frame.set_full_repaint(full);
        frame.set_clear(clear);
        frame.push_shifts(&ShiftRecord { entries });
        assert!(state.apply_frame(&frame.finish()).is_err());
        assert_eq!(row_text(&state, 0), "abcdef");
    }
    let mut valid = FrameBuilder::new(6, 3, 2);
    valid.push_shifts(&ShiftRecord {
        entries: vec![shift],
    });
    valid.push_row(1, 2, &run("X"));
    let applied = state.apply_frame(&valid.finish()).unwrap();
    assert_eq!(applied.seq, 2);
    assert!(
        matches!(applied.damage, Damage::Rows(ref rows) if rows.contains(&0) && rows.contains(&1))
    );
    assert_eq!(row_text(&state, 0), "ahijef");
    assert_eq!(row_text(&state, 1), "g X kl");
    assert_eq!(row_text(&state, 2), "mnopqr");
}

mod transfers {
    use super::*;
    use yinz_runtime::native_media::{
        MAX_SHARED_IMAGE_BYTES, VIEWER_MEDIA_BYTES, VIEWER_MEDIA_ENTRIES,
    };

    fn chunked() -> caps::Agreement {
        caps::Agreement {
            media: caps::INLINE | caps::CHUNKED,
            ..caps::Agreement::default()
        }
    }

    fn chunk(transfer_id: u64, total: u32, offset: u32, data: &[u8]) -> GraphicsRecord {
        GraphicsRecord::TransferChunk {
            transfer_id,
            total,
            offset,
            data: data.to_vec(),
        }
    }

    /// A 2x2 RGBA residency naming transfer `transfer_id`.
    fn resident(id: u64, transfer_id: u64) -> GraphicsRecord {
        residency(
            id,
            GraphicsMedium::Transfer,
            transfer_id.to_le_bytes().to_vec(),
        )
    }

    /// A live client and the next frame sequence it expects.
    struct Live {
        state: TerminalState,
        seq: u64,
    }

    impl Live {
        fn new(agreement: caps::Agreement) -> Self {
            let mut state = TerminalState::new(6, 12);
            state.negotiate(agreement, None);
            let mut live = Self { state, seq: 0 };
            live.apply(true, &[]).unwrap();
            live
        }

        fn apply(&mut self, repaint: bool, records: &[GraphicsRecord]) -> Result<(), String> {
            self.seq += 1;
            let mut frame = FrameBuilder::new(12, 6, self.seq);
            frame.set_full_repaint(repaint);
            frame.extend_graphics(records.iter());
            self.state
                .apply_frame(&frame.finish())
                .map(drop)
                .map_err(|error| error.to_string())
        }

        /// A refused frame leaves the applied sequence where it was.
        fn rejects(&mut self, records: &[GraphicsRecord], reason: &str) {
            let error = self.apply(false, records).unwrap_err();
            self.seq -= 1;
            assert!(error.contains(reason), "{error} lacks {reason:?}");
        }

        fn pixels(&self, id: u64) -> Vec<u8> {
            self.state.graphics().image(id).unwrap().pixels.clone()
        }
    }

    #[test]
    fn render_negotiation_transfer_assembles_the_interleavings_a_server_produces() {
        let mut live = Live::new(chunked());
        let a = rgba([1, 2, 3, 255], 4);
        let b = rgba([4, 5, 6, 255], 4);
        // Two transfers start in one frame, with an id gap between them.
        live.apply(false, &[chunk(3, 16, 0, &a[..5]), chunk(9, 16, 0, &b[..7])])
            .unwrap();
        // A lower id continues after a higher one started, and both complete apart.
        live.apply(false, &[chunk(3, 16, 5, &a[5..]), resident(1, 3)])
            .unwrap();
        assert_eq!(live.pixels(1), a);
        live.apply(false, &[chunk(9, 16, 7, &b[7..12])]).unwrap();
        live.apply(false, &[chunk(9, 16, 12, &b[12..]), resident(2, 9)])
            .unwrap();
        assert_eq!(live.pixels(2), b);
        // A completed transfer is consumed, so its id cannot be named again.
        live.rejects(&[resident(3, 9)], "not complete");
    }

    #[test]
    fn render_negotiation_transfer_rejects_every_rule_violation() {
        let pixels = rgba([7, 7, 7, 255], 4);
        let cases: Vec<(&str, Vec<GraphicsRecord>, &str)> = vec![
            (
                "continuation of an unknown id",
                vec![chunk(1, 16, 4, &pixels[4..])],
                "starts at offset",
            ),
            (
                "empty chunk",
                vec![chunk(1, 16, 0, &[])],
                "empty transfer chunk",
            ),
            ("zero total", vec![chunk(1, 0, 0, &[1])], "declares 0 bytes"),
            (
                "total above the decode limit",
                vec![chunk(1, MAX_SHARED_IMAGE_BYTES as u32 + 1, 0, &[1])],
                "declares",
            ),
            (
                "chunk past its total",
                vec![chunk(1, 2, 0, &[1, 2, 3])],
                "overruns",
            ),
            (
                "out-of-order offset",
                vec![chunk(1, 16, 0, &pixels[..4]), chunk(1, 16, 8, &pixels[8..])],
                "out of sequence",
            ),
            (
                "changed total",
                vec![
                    chunk(1, 16, 0, &pixels[..4]),
                    chunk(1, 15, 4, &pixels[4..15]),
                ],
                "out of sequence",
            ),
            (
                "cancel of an unstarted id",
                vec![GraphicsRecord::TransferCancel { transfer_id: 1 }],
                "cancel of unknown",
            ),
            (
                "residency of an unstarted id",
                vec![resident(1, 1)],
                "not complete",
            ),
            (
                "residency of an incomplete transfer",
                vec![chunk(1, 16, 0, &pixels[..4]), resident(1, 1)],
                "not complete",
            ),
            (
                "residency with a short id",
                vec![
                    chunk(1, 16, 0, &pixels),
                    residency(1, GraphicsMedium::Transfer, vec![1, 0, 0]),
                ],
                "8-byte id",
            ),
            (
                "length differing from the dimensions",
                vec![chunk(1, 12, 0, &pixels[..12]), resident(1, 1)],
                "differs from RGBA",
            ),
            (
                "completed without a residency",
                vec![chunk(1, 16, 0, &pixels)],
                "without a residency",
            ),
        ];
        for (case, records, reason) in cases {
            let mut live = Live::new(chunked());
            let error = live.apply(false, &records).unwrap_err();
            assert!(error.contains(reason), "{case}: {error}");
        }
        // Ids only increase: a late or reused id is refused.
        let mut live = Live::new(chunked());
        live.apply(false, &[chunk(5, 16, 0, &pixels[..4])]).unwrap();
        live.rejects(&[chunk(4, 16, 0, &pixels[..4])], "late or unknown");
        live.rejects(&[chunk(5, 16, 0, &pixels[..4])], "out of sequence");
        // Without the chunked bit every transfer record fails the frame.
        for record in [
            chunk(1, 16, 0, &pixels),
            GraphicsRecord::TransferCancel { transfer_id: 1 },
            resident(1, 1),
        ] {
            let mut live = Live::new(caps::Agreement::default());
            live.rejects(&[record], "chunked media was not agreed");
        }
        let mut text = Live::new(caps::Agreement {
            features: 0,
            media: 0,
            ..chunked()
        });
        text.rejects(&[chunk(1, 16, 0, &pixels)], "unnegotiated graphics record");
    }

    #[test]
    fn render_negotiation_transfer_bounds_concurrent_assemblies() {
        let mut live = Live::new(chunked());
        let starts: Vec<_> = (1..=VIEWER_MEDIA_ENTRIES as u64)
            .map(|id| chunk(id, 16, 0, &[0]))
            .collect();
        live.apply(false, &starts).unwrap();
        live.rejects(
            &[chunk(VIEWER_MEDIA_ENTRIES as u64 + 1, 16, 0, &[0])],
            "too many or too large",
        );
        // Declared totals are bounded before anything is allocated for them.
        let mut live = Live::new(chunked());
        let max = MAX_SHARED_IMAGE_BYTES as u32;
        live.apply(false, &[chunk(1, max, 0, &[0])]).unwrap();
        let rest = (VIEWER_MEDIA_BYTES - MAX_SHARED_IMAGE_BYTES) as u32;
        live.apply(false, &[chunk(2, rest, 0, &[0])]).unwrap();
        live.rejects(&[chunk(3, 1, 0, &[0])], "too many or too large");
        // A cancel returns its share.
        let mut live = Live::new(chunked());
        live.apply(false, &[chunk(1, max, 0, &[0])]).unwrap();
        live.apply(false, &[GraphicsRecord::TransferCancel { transfer_id: 1 }])
            .unwrap();
        live.apply(false, &[chunk(2, max, 0, &[0])]).unwrap();
        live.apply(false, &[chunk(3, rest, 0, &[0])]).unwrap();
    }

    #[test]
    fn render_negotiation_transfer_is_discarded_on_clear_repaint_and_negotiate() {
        let pixels = rgba([9, 9, 9, 255], 4);
        for discard in ["full repaint", "clear", "negotiate"] {
            let mut live = Live::new(chunked());
            live.apply(false, &[chunk(4, 16, 0, &pixels[..8])]).unwrap();
            match discard {
                "full repaint" => live.apply(true, &[]).unwrap(),
                "clear" => {
                    live.seq += 1;
                    let mut frame = FrameBuilder::new(12, 6, live.seq);
                    frame.set_clear(true);
                    live.state.apply_frame(&frame.finish()).unwrap();
                },
                _ => {
                    live.state.negotiate(chunked(), None);
                    live.seq = 0;
                    live.apply(true, &[]).unwrap();
                },
            }
            live.rejects(&[chunk(4, 16, 8, &pixels[8..])], "");
            // A new transfer starts fresh in the same frame as the discard.
            let mut live = Live::new(chunked());
            live.apply(false, &[chunk(4, 16, 0, &pixels[..8])]).unwrap();
            live.apply(true, &[chunk(5, 16, 0, &pixels), resident(1, 5)])
                .unwrap();
            assert_eq!(live.pixels(1), pixels, "{discard}");
        }
        // After negotiate, ids restart for the new attachment.
        let mut live = Live::new(chunked());
        live.apply(false, &[chunk(8, 16, 0, &pixels[..8])]).unwrap();
        live.state.negotiate(chunked(), None);
        live.seq = 0;
        live.apply(true, &[chunk(1, 16, 0, &pixels), resident(1, 1)])
            .unwrap();
    }

    #[test]
    fn render_negotiation_transfer_replays_from_a_recording() {
        let pixels = rgba([3, 1, 4, 255], 4);
        let mut state = TerminalState::new(6, 12);
        let frames: Vec<Vec<GraphicsRecord>> = vec![
            vec![chunk(2, 16, 0, &pixels[..6])],
            vec![
                chunk(2, 16, 6, &pixels[6..]),
                resident(7, 2),
                placement(7, 1),
            ],
            // Replay reports and skips what a live client would refuse.
            vec![
                GraphicsRecord::TransferCancel { transfer_id: 9 },
                resident(8, 9),
            ],
        ];
        for (seq, records) in frames.iter().enumerate() {
            let mut frame = FrameBuilder::new(12, 6, seq as u64 + 1);
            frame.set_full_repaint(seq == 0);
            frame.extend_graphics(records.iter());
            state.apply_frame(&frame.finish()).unwrap();
        }
        assert_eq!(state.graphics().image(7).unwrap().pixels, pixels);
        assert!(state.graphics().image(8).is_none());
        assert_eq!(state.graphics().placements().count(), 1);
    }
}

#[test]
fn live_images_beneath_a_popup_stay_owned_by_their_pane() {
    use structured_render::{PaneRect, PANE_FOCUSED, PANE_POPUP};
    let pane = PaneRect {
        cols: 12,
        rows: 6,
        ..Default::default()
    };
    // Covers both the Kitty placement at (2, 3) and the Sixel anchor at (3, 2).
    let palette = PaneRect {
        x: 1,
        y: 1,
        cols: 6,
        rows: 4,
        flags: PANE_POPUP | PANE_FOCUSED,
        ..Default::default()
    };
    let mut state = TerminalState::new(6, 12);
    state.set_cell_size(8, 20);
    state.negotiate(caps::Agreement::default(), None);
    let mut first = FrameBuilder::new(12, 6, 1);
    first.set_full_repaint(true);
    first.push_geometry(&GeometryRecord {
        panes: vec![pane, palette],
    });
    first.extend_graphics(&[
        residency(7, GraphicsMedium::Inline, rgba([1, 2, 3, 255], 4)),
        placement(7, 1),
        sixel_at(2, 3, 16, 1),
    ]);
    state.apply_frame(&first.finish()).unwrap();
    assert_eq!(state.graphics().placements().count(), 1);
    assert_eq!(state.sixels().chunks().count(), 1);
    assert_eq!(state.geometry().content_pane_at(2, 3), Some(&palette));
    assert_eq!(state.geometry().image_owner_at(2, 3), Some(&pane));
    assert_eq!(state.geometry().image_owner_at(3, 2), Some(&pane));

    // The popup closes: the pane still owns both images.
    let mut closed = FrameBuilder::new(12, 6, 2);
    closed.push_geometry(&GeometryRecord { panes: vec![pane] });
    state.apply_frame(&closed.finish()).unwrap();
    assert_eq!(state.graphics().placements().count(), 1);
    assert_eq!(state.sixels().chunks().count(), 1);

    // A popup is never an owner, so it cannot stand in for a pane that went away.
    let mut orphaned = FrameBuilder::new(12, 6, 3);
    orphaned.push_geometry(&GeometryRecord {
        panes: vec![palette],
    });
    assert!(matches!(
        state.apply_frame(&orphaned.finish()),
        Err(FrameError::Protocol(3, reason)) if reason.contains("without an owning pane")
    ));
    let mut placed = FrameBuilder::new(12, 6, 3);
    placed.push_geometry(&GeometryRecord {
        panes: vec![palette],
    });
    placed.extend_graphics(&[placement(7, 2)]);
    assert!(matches!(
        state.apply_frame(&placed.finish()),
        Err(FrameError::Protocol(3, reason)) if reason.contains("no owning pane")
    ));
    assert_eq!(state.applied_seq, Some(2));
}
