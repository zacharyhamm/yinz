use super::*;
use crate::testing::painter;
use yinz_types::structured_render::{decode, FrameBuilder, ATTR_BOLD, CURSOR_SHAPE_BLOCK};

fn run(text: &str) -> Vec<WireCell> {
    text.chars()
        .map(|character| WireCell {
            ch: character as u32,
            ..WireCell::BLANK
        })
        .collect()
}

fn row_text(buffer: &ScreenBuffer, row: usize) -> String {
    (0..buffer.size().cols)
        .map(|col| buffer.cell(row, col).character())
        .collect()
}

fn apply(buffer: &mut ScreenBuffer, frame: &[u8]) -> Result<Applied, ApplyError> {
    let view = decode(frame).expect("the frame should decode");
    buffer.apply(&view)
}

#[test]
fn a_new_buffer_is_blank_with_a_hidden_cursor() {
    let buffer = ScreenBuffer::new(2, 4);
    assert_eq!(buffer.size(), TermSize { rows: 2, cols: 4 });
    assert_eq!(row_text(&buffer, 0), "    ");
    assert!(!buffer.cursor_is_visible());
    assert!(!buffer.has_blinking_cells());
}

#[test]
fn a_degenerate_size_is_clamped_rather_than_producing_an_empty_grid() {
    let buffer = ScreenBuffer::new(0, 0);
    assert_eq!(buffer.size(), TermSize { rows: 1, cols: 1 });
}

#[test]
fn row_records_land_at_the_columns_they_name_and_report_their_spans() {
    let mut buffer = ScreenBuffer::new(2, 6);
    let mut builder = FrameBuilder::new(6, 2, 3);
    builder.push_row(0, 1, &run("abc"));
    builder.push_row(1, 4, &run("zz"));
    let applied = apply(&mut buffer, &builder.finish()).unwrap();

    assert_eq!(applied.seq, 3);
    assert!(!applied.cleared);
    assert_eq!(row_text(&buffer, 0), " abc  ");
    assert_eq!(row_text(&buffer, 1), "    zz");
    assert_eq!(
        applied.written,
        vec![Span::new(0, 1, 3), Span::new(1, 4, 5)]
    );
}

#[test]
fn a_partial_frame_leaves_the_cells_it_does_not_name_alone() {
    let mut buffer = ScreenBuffer::new(1, 6);
    let mut first = FrameBuilder::new(6, 1, 0);
    first.push_row(0, 0, &run("abcdef"));
    apply(&mut buffer, &first.finish()).unwrap();

    let mut second = FrameBuilder::new(6, 1, 1);
    second.push_row(0, 2, &run("XY"));
    apply(&mut buffer, &second.finish()).unwrap();
    assert_eq!(row_text(&buffer, 0), "abXYef");
}

#[test]
fn a_full_repaint_clears_before_it_paints() {
    let mut buffer = ScreenBuffer::new(1, 6);
    let mut first = FrameBuilder::new(6, 1, 0);
    first.push_row(0, 0, &run("abcdef"));
    apply(&mut buffer, &first.finish()).unwrap();

    let mut second = FrameBuilder::new(6, 1, 1);
    second.set_full_repaint(true);
    second.push_row(0, 0, &run("gh"));
    let applied = apply(&mut buffer, &second.finish()).unwrap();
    assert!(applied.cleared);
    assert_eq!(row_text(&buffer, 0), "gh    ");
}

#[test]
fn a_clear_without_a_full_repaint_blanks_every_cell_it_does_not_repaint() {
    let mut buffer = ScreenBuffer::new(2, 6);
    let mut first = FrameBuilder::new(6, 2, 0);
    first.push_row(0, 0, &run("abcdef"));
    first.push_row(1, 0, &run("ghijkl"));
    apply(&mut buffer, &first.finish()).unwrap();

    let mut second = FrameBuilder::new(6, 2, 1);
    second.set_clear(true);
    second.push_row(0, 0, &run("ab"));
    let applied = apply(&mut buffer, &second.finish()).unwrap();
    assert!(applied.cleared);
    assert!(!applied.reshaped);
    assert_eq!(row_text(&buffer, 0), "ab    ");
    assert_eq!(row_text(&buffer, 1), "      ");
}

#[test]
fn a_full_repaint_may_reshape_the_screen() {
    let mut buffer = ScreenBuffer::new(1, 6);
    let mut builder = FrameBuilder::new(3, 2, 0);
    builder.set_full_repaint(true);
    builder.push_row(1, 0, &run("xyz"));
    let applied = apply(&mut buffer, &builder.finish()).unwrap();

    assert!(applied.reshaped);
    assert_eq!(buffer.size(), TermSize { rows: 2, cols: 3 });
    assert_eq!(row_text(&buffer, 0), "   ");
    assert_eq!(row_text(&buffer, 1), "xyz");
}

#[test]
fn a_partial_frame_of_the_wrong_shape_is_a_protocol_error() {
    let mut buffer = ScreenBuffer::new(1, 6);
    let mut builder = FrameBuilder::new(3, 2, 0);
    builder.push_row(0, 0, &run("xyz"));
    assert_eq!(
        apply(&mut buffer, &builder.finish()),
        Err(ApplyError::ReshapeWithoutFullRepaint {
            expected: (1, 6),
            found: (2, 3)
        })
    );
    assert_eq!(buffer.size(), TermSize { rows: 1, cols: 6 });
}

#[test]
fn a_frame_with_no_cells_at_all_is_refused_rather_than_emptying_the_buffer() {
    let mut buffer = ScreenBuffer::new(2, 4);
    let mut builder = FrameBuilder::new(0, 0, 0);
    builder.set_full_repaint(true);
    assert_eq!(
        apply(&mut buffer, &builder.finish()),
        Err(ApplyError::DegenerateViewport { found: (0, 0) })
    );
    assert_eq!(buffer.size(), TermSize { rows: 2, cols: 4 });
}

#[test]
fn the_cursor_comes_from_the_frame_header() {
    let mut buffer = ScreenBuffer::new(4, 8);
    let mut builder = FrameBuilder::new(8, 4, 0);
    builder.set_cursor(CursorState {
        x: 5,
        y: 2,
        shape: CURSOR_SHAPE_BEAM | yinz_types::structured_render::CURSOR_BLINKING,
        visible: true,
    });
    builder.push_row(0, 0, &run("a"));
    apply(&mut buffer, &builder.finish()).unwrap();

    assert!(buffer.cursor_is_visible());
    assert_eq!(buffer.cursor_position(), (2, 5));
    assert_eq!(buffer.cursor_shape(), CursorShape::Beam);
    assert!(buffer.cursor_is_blinking());
}

#[test]
fn cursor_shapes_map_to_their_wire_encoding() {
    for (encoded, expected) in [
        (CURSOR_SHAPE_BLOCK, CursorShape::Block),
        (CURSOR_SHAPE_UNDERLINE, CursorShape::Underline),
        (CURSOR_SHAPE_BEAM, CursorShape::Beam),
    ] {
        let mut buffer = ScreenBuffer::new(1, 2);
        let mut builder = FrameBuilder::new(2, 1, 0);
        builder.set_cursor(CursorState {
            x: 0,
            y: 0,
            shape: encoded,
            visible: true,
        });
        builder.push_row(0, 0, &run("a"));
        apply(&mut buffer, &builder.finish()).unwrap();
        assert_eq!(buffer.cursor_shape(), expected);
        assert!(!buffer.cursor_is_blinking());
    }
}

#[test]
fn a_wide_head_is_followed_by_a_tail_the_buffer_recognizes() {
    let mut buffer = ScreenBuffer::new(1, 4);
    let mut builder = FrameBuilder::new(4, 1, 0);
    let wide = WireCell {
        ch: '\u{4f60}' as u32,
        width: 2,
        ..WireCell::BLANK
    };
    builder.push_row(0, 0, &[wide, WireCell::BLANK, WireCell::BLANK]);
    apply(&mut buffer, &builder.finish()).unwrap();

    assert_eq!(buffer.occupancy(0, 0), Occupancy::WideHead);
    assert_eq!(buffer.occupancy(0, 1), Occupancy::WideTail);
    assert_eq!(buffer.occupancy(0, 2), Occupancy::Single);
}

#[test]
fn blink_is_read_off_the_cells_rather_than_tracked_alongside_them() {
    let mut buffer = ScreenBuffer::new(1, 4);
    let mut builder = FrameBuilder::new(4, 1, 0);
    builder.push_row(
        0,
        0,
        &[
            WireCell {
                ch: 'a' as u32,
                attrs: ATTR_SLOW_BLINK | ATTR_BOLD,
                ..WireCell::BLANK
            },
            WireCell {
                ch: 'b' as u32,
                attrs: ATTR_FAST_BLINK,
                ..WireCell::BLANK
            },
            WireCell {
                ch: 'c' as u32,
                ..WireCell::BLANK
            },
        ],
    );
    apply(&mut buffer, &builder.finish()).unwrap();

    assert_eq!(
        buffer.blink_at(0, 0),
        Blink {
            slow: true,
            fast: false
        }
    );
    assert_eq!(
        buffer.blink_at(0, 1),
        Blink {
            slow: false,
            fast: true
        }
    );
    assert_eq!(buffer.blink_at(0, 2), Blink::default());
    assert!(buffer.has_blinking_cells());
}

#[test]
fn a_repaint_that_removes_the_last_blinking_cell_stops_the_blink_timer() {
    let mut buffer = ScreenBuffer::new(1, 4);
    let mut first = FrameBuilder::new(4, 1, 0);
    first.push_row(
        0,
        0,
        &[WireCell {
            ch: 'a' as u32,
            attrs: ATTR_SLOW_BLINK,
            ..WireCell::BLANK
        }],
    );
    apply(&mut buffer, &first.finish()).unwrap();
    assert!(buffer.has_blinking_cells());

    let mut second = FrameBuilder::new(4, 1, 1);
    second.push_row(0, 0, &run("a"));
    apply(&mut buffer, &second.finish()).unwrap();
    assert!(!buffer.has_blinking_cells());
}

#[test]
fn reads_outside_the_grid_answer_blank_rather_than_panicking() {
    let buffer = ScreenBuffer::new(2, 2);
    assert_eq!(buffer.cell(9, 9), WireCell::BLANK);
    assert_eq!(buffer.occupancy(9, 9), Occupancy::Single);
    assert_eq!(buffer.blink_at(9, 9), Blink::default());
}

#[test]
fn a_cell_resolves_its_url_through_the_table_the_frame_carried() {
    let mut buffer = ScreenBuffer::new(1, 6);
    let frame = painter::linked_frame(
        1,
        6,
        0,
        true,
        &[(0, 0, "ab cd ", &[3, 3, 0, 0, 0, 0])],
        &[(3, "https://example.com/three")],
    );
    apply(&mut buffer, &frame).unwrap();
    assert_eq!(buffer.link_at(0, 0), Some("https://example.com/three"));
    assert_eq!(buffer.link_at(0, 2), None);
    assert_eq!(buffer.link_id_at(0, 1), 3);
}

#[test]
fn a_delta_row_resolves_against_the_table_the_buffer_already_holds() {
    let mut buffer = ScreenBuffer::new(2, 6);
    let first = painter::linked_frame(
        2,
        6,
        0,
        true,
        &[(0, 0, "ab    ", &[4, 4, 0, 0, 0, 0])],
        &[(4, "https://example.com/four")],
    );
    apply(&mut buffer, &first).unwrap();

    let delta = painter::linked_frame(2, 6, 1, false, &[(1, 0, "cd", &[4, 4])], &[]);
    apply(&mut buffer, &delta).unwrap();
    assert_eq!(buffer.link_at(1, 0), Some("https://example.com/four"));
    assert_eq!(buffer.link_at(0, 0), Some("https://example.com/four"));
}

#[test]
fn a_full_repaint_drops_the_table_the_buffer_was_holding() {
    let mut buffer = ScreenBuffer::new(1, 6);
    let first = painter::linked_frame(
        1,
        6,
        0,
        true,
        &[(0, 0, "ab    ", &[5, 5, 0, 0, 0, 0])],
        &[
            (5, "https://example.com/before"),
            (6, "https://example.com/x"),
        ],
    );
    apply(&mut buffer, &first).unwrap();

    let second = painter::linked_frame(
        1,
        6,
        1,
        true,
        &[(0, 0, "cdef  ", &[5, 5, 6, 6, 0, 0])],
        &[(5, "https://example.com/after")],
    );
    apply(&mut buffer, &second).unwrap();
    assert_eq!(buffer.link_at(0, 0), Some("https://example.com/after"));
    assert_eq!(buffer.link_id_at(0, 2), 6);
    assert_eq!(buffer.link_at(0, 2), None);
}

fn painted(buffer: &mut ScreenBuffer, paint: impl FnOnce(&mut painter::Painter)) {
    let size = buffer.size();
    let frame = painter::Painter::frame(size.rows, size.cols, paint);
    apply(buffer, &frame).unwrap();
}

const FAMILY_MAN: &str = "\u{1f468}\u{200d}";
const FAMILY_WOMAN: &str = "\u{1f469}\u{200d}";
const FAMILY_GIRL: &str = "\u{1f467}";

#[test]
fn a_cluster_cell_carries_its_whole_text_and_its_first_code_point() {
    let mut buffer = ScreenBuffer::new(1, 6);
    painted(&mut buffer, |p| {
        p.cluster(0, 0, "e\u{301}", 1);
        p.text(0, 1, "x");
    });
    assert_eq!(buffer.cell(0, 0).character(), 'e');
    assert_eq!(buffer.text_at(0, 0), "e\u{301}");
    assert_eq!(
        buffer.cluster_at(0, 0).map(|text| text.as_ref()),
        Some("e\u{301}")
    );
    assert_eq!(buffer.text_at(0, 1), "x");
    assert!(buffer.cluster_at(0, 1).is_none());
    assert_eq!(buffer.cluster_count(), 1);
}

#[test]
fn a_group_keeps_each_member_text_and_marks_the_members_after_the_first() {
    let mut buffer = ScreenBuffer::new(1, 8);
    painted(&mut buffer, |p| {
        p.group(
            0,
            0,
            &[(FAMILY_MAN, 2), (FAMILY_WOMAN, 2), (FAMILY_GIRL, 2)],
        );
        p.text(0, 6, "z");
    });
    assert_eq!(buffer.text_at(0, 0), FAMILY_MAN);
    assert_eq!(buffer.text_at(0, 2), FAMILY_WOMAN);
    assert_eq!(buffer.text_at(0, 4), FAMILY_GIRL);
    assert!(!buffer.is_group_continuation(0, 0));
    assert!(!buffer.is_group_continuation(0, 1));
    assert!(buffer.is_group_continuation(0, 2));
    assert!(!buffer.is_group_continuation(0, 3));
    assert!(buffer.is_group_continuation(0, 4));
    for col in 0..6 {
        assert_eq!(
            buffer.unit_span(0, col),
            UnitSpan { start: 0, end: 6 },
            "column {}",
            col
        );
    }
    assert_eq!(buffer.unit_span(0, 6), UnitSpan { start: 6, end: 7 });
    assert_eq!(
        buffer.unit_text(0, buffer.unit_span(0, 3)),
        format!("{}{}{}", FAMILY_MAN, FAMILY_WOMAN, FAMILY_GIRL)
    );
}

#[test]
fn occupancy_covers_widths_one_to_three_and_continuation_cells() {
    let mut buffer = ScreenBuffer::new(1, 10);
    painted(&mut buffer, |p| {
        p.text(0, 0, "a");
        p.wide(0, 1, '\u{4f60}');
        p.cluster(0, 3, "\u{17d8}", 3);
        p.group(0, 6, &[("\u{1f1e9}", 1), ("\u{1f1ea}", 1)]);
    });
    let occupancy: Vec<Occupancy> = (0..9).map(|col| buffer.occupancy(0, col)).collect();
    assert_eq!(
        occupancy,
        vec![
            Occupancy::Single,
            Occupancy::WideHead,
            Occupancy::WideTail,
            Occupancy::WideHead,
            Occupancy::WideTail,
            Occupancy::WideTail,
            Occupancy::Single,
            Occupancy::Single,
            Occupancy::Single,
        ]
    );
    assert_eq!(buffer.unit_span(0, 5), UnitSpan { start: 3, end: 6 });
    assert_eq!(buffer.unit_span(0, 7), UnitSpan { start: 6, end: 8 });
    assert_eq!(buffer.unit_span(0, 8), UnitSpan { start: 8, end: 9 });
    assert!(buffer.is_group_continuation(0, 7));
}

#[test]
fn a_cluster_overwritten_by_a_later_frame_leaves_no_text_behind() {
    let mut buffer = ScreenBuffer::new(1, 6);
    painted(&mut buffer, |p| p.cluster(0, 0, "a\u{301}\u{302}", 1));
    assert_eq!(buffer.cluster_count(), 1);
    painted(&mut buffer, |p| p.text(0, 0, "b"));
    assert_eq!(buffer.text_at(0, 0), "b");
    assert_eq!(buffer.cluster_count(), 0);
}

#[test]
fn cluster_ids_are_read_against_the_frame_that_carried_them() {
    let mut buffer = ScreenBuffer::new(2, 6);
    painted(&mut buffer, |p| p.cluster(0, 0, "e\u{301}", 1));
    painted(&mut buffer, |p| p.cluster(1, 0, "o\u{308}", 1));
    assert_eq!(buffer.text_at(0, 0), "e\u{301}");
    assert_eq!(buffer.text_at(1, 0), "o\u{308}");
    assert_eq!(buffer.cluster_count(), 2);
}

#[test]
fn a_full_repaint_drops_every_cluster_text() {
    let mut buffer = ScreenBuffer::new(1, 6);
    painted(&mut buffer, |p| p.cluster(0, 2, "e\u{301}", 1));
    painted(&mut buffer, |p| {
        p.full_repaint();
        p.text(0, 0, "ab");
    });
    assert_eq!(buffer.cluster_count(), 0);
    assert_eq!(buffer.text_at(0, 2), " ");
}

#[test]
fn a_cluster_reference_missing_from_the_record_reads_as_a_blank() {
    let mut buffer = ScreenBuffer::new(1, 4);
    let mut builder = FrameBuilder::new(4, 1, 0);
    builder.push_row(
        0,
        0,
        &[WireCell {
            ch: yinz_types::structured_render::CLUSTER_CH_BASE + 7,
            ..WireCell::BLANK
        }],
    );
    if let Ok(view) = decode(&builder.finish()) {
        buffer.apply(&view).unwrap();
        assert_eq!(buffer.text_at(0, 0), " ");
        assert_eq!(buffer.cluster_count(), 0);
    }
}

#[test]
fn a_reshape_leaves_no_link_behind() {
    let mut buffer = ScreenBuffer::new(1, 6);
    let first = painter::linked_frame(
        1,
        6,
        0,
        true,
        &[(0, 0, "ab    ", &[5, 5, 0, 0, 0, 0])],
        &[(5, "https://example.com/before")],
    );
    apply(&mut buffer, &first).unwrap();

    let reshaped = painter::linked_frame(2, 4, 1, true, &[(0, 0, "abcd", &[5, 5, 0, 0])], &[]);
    apply(&mut buffer, &reshaped).unwrap();
    assert_eq!(buffer.link_id_at(0, 0), 5);
    assert_eq!(buffer.link_at(0, 0), None);
}
