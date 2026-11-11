use super::*;

fn metrics(width: u32, height: u32) -> CellMetrics {
    CellMetrics {
        width,
        height,
        baseline: height * 3 / 4,
        underline_top: height - 2,
        strikeout_top: height / 2,
        stroke: 1,
    }
}

fn sprite(character: char, metrics: CellMetrics) -> GlyphBitmap {
    draw(character, metrics, 1).unwrap()
}

fn at(bitmap: &GlyphBitmap, x: u32, y: u32) -> u8 {
    bitmap.coverage[(y * bitmap.width + x) as usize]
}

#[test]
fn sprites_fill_exactly_one_cell() {
    let metrics = metrics(8, 20);
    for character in ('\u{2500}'..='\u{259f}').chain('\u{e0b0}'..='\u{e0b3}') {
        let bitmap = sprite(character, metrics);
        assert_eq!((bitmap.width, bitmap.height), (8, 20), "{:?}", character);
        assert_eq!(bitmap.left, 0);
        assert_eq!(bitmap.top, metrics.baseline as i32);
        assert!(
            bitmap.coverage.iter().any(|alpha| *alpha > 0),
            "{:?} drew nothing",
            character
        );
    }
}

#[test]
fn a_full_block_covers_every_pixel_at_awkward_sizes() {
    for (width, height) in [(7, 17), (8, 20), (13, 31), (6, 15)] {
        let bitmap = sprite('\u{2588}', metrics(width, height));
        assert!(
            bitmap.coverage.iter().all(|alpha| *alpha == OPAQUE),
            "a {}x{} full block has gaps",
            width,
            height
        );
    }
}

#[test]
fn partial_blocks_grow_from_the_left_with_hard_edges() {
    let metrics = metrics(8, 20);
    let mut previous = 0;
    for character in [
        '\u{258f}', '\u{258e}', '\u{258d}', '\u{258c}', '\u{258b}', '\u{258a}', '\u{2589}',
    ] {
        let bitmap = sprite(character, metrics);
        let filled = (0..bitmap.width)
            .filter(|x| at(&bitmap, *x, 10) == OPAQUE)
            .count();
        assert!(
            filled > previous,
            "{:?} is not wider than its predecessor",
            character
        );
        assert!(bitmap
            .coverage
            .iter()
            .all(|alpha| *alpha == 0 || *alpha == OPAQUE));
        assert_eq!(at(&bitmap, 0, 0), OPAQUE);
        previous = filled;
    }
}

#[test]
fn lower_blocks_reach_the_bottom_edge() {
    let metrics = metrics(8, 20);
    for character in '\u{2581}'..='\u{2587}' {
        let bitmap = sprite(character, metrics);
        assert_eq!(at(&bitmap, 4, 19), OPAQUE, "{:?}", character);
        assert_eq!(at(&bitmap, 4, 0), 0, "{:?}", character);
    }
}

#[test]
fn horizontal_lines_meet_their_neighbours_at_the_cell_edge() {
    let metrics = metrics(9, 21);
    for character in ['\u{2500}', '\u{2501}', '\u{2550}', '\u{253c}', '\u{256c}'] {
        let bitmap = sprite(character, metrics);
        let left: Vec<u8> = (0..bitmap.height).map(|y| at(&bitmap, 0, y)).collect();
        let right: Vec<u8> = (0..bitmap.height)
            .map(|y| at(&bitmap, bitmap.width - 1, y))
            .collect();
        assert_eq!(left, right, "{:?}", character);
        assert!(left.contains(&OPAQUE));
    }
}

#[test]
fn vertical_lines_meet_their_neighbours_at_the_cell_edge() {
    let metrics = metrics(9, 21);
    for character in ['\u{2502}', '\u{2503}', '\u{2551}', '\u{253c}', '\u{256c}'] {
        let bitmap = sprite(character, metrics);
        let top: Vec<u8> = (0..bitmap.width).map(|x| at(&bitmap, x, 0)).collect();
        let bottom: Vec<u8> = (0..bitmap.width)
            .map(|x| at(&bitmap, x, bitmap.height - 1))
            .collect();
        assert_eq!(top, bottom, "{:?}", character);
        assert!(top.contains(&OPAQUE));
    }
}

#[test]
fn box_lines_span_a_cell_adjusted_for_line_height_and_width() {
    use crate::text::font::{CellAdjust, FontOptions, FontStack};

    let metrics = FontStack::build(&FontOptions {
        system_fonts: false,
        cell: CellAdjust {
            line_height: 1.5,
            cell_width: 1.25,
            ..CellAdjust::default()
        },
        ..FontOptions::default()
    })
    .unwrap()
    .metrics();
    assert_eq!((metrics.width, metrics.height), (10, 30));

    let vertical = sprite('\u{2502}', metrics);
    assert_eq!((vertical.width, vertical.height), (10, 30));
    assert_eq!(vertical.top, metrics.baseline as i32);
    let column = (0..vertical.width)
        .find(|x| at(&vertical, *x, 0) == OPAQUE)
        .expect("the vertical line reaches the top edge");
    assert!(
        (0..vertical.height).all(|y| at(&vertical, column, y) == OPAQUE),
        "the vertical line has a gap inside the taller cell"
    );

    let horizontal = sprite('\u{2500}', metrics);
    let row = (0..horizontal.height)
        .find(|y| at(&horizontal, 0, *y) == OPAQUE)
        .expect("the horizontal line reaches the left edge");
    assert!(
        (0..horizontal.width).all(|x| at(&horizontal, x, row) == OPAQUE),
        "the horizontal line has a gap inside the wider cell"
    );

    let powerline = sprite('\u{e0b0}', metrics);
    assert_eq!((powerline.width, powerline.height), (10, 30));
}

#[test]
fn a_corner_lines_up_with_the_straight_lines_it_joins() {
    let metrics = metrics(9, 21);
    let corner = sprite('\u{250c}', metrics);
    let horizontal = sprite('\u{2500}', metrics);
    let vertical = sprite('\u{2502}', metrics);
    let right_edge = |bitmap: &GlyphBitmap| -> Vec<u8> {
        (0..bitmap.height)
            .map(|y| at(bitmap, bitmap.width - 1, y))
            .collect()
    };
    let bottom_edge = |bitmap: &GlyphBitmap| -> Vec<u8> {
        (0..bitmap.width)
            .map(|x| at(bitmap, x, bitmap.height - 1))
            .collect()
    };
    assert_eq!(right_edge(&corner), right_edge(&horizontal));
    assert_eq!(bottom_edge(&corner), bottom_edge(&vertical));
}

#[test]
fn a_rounded_corner_lines_up_with_the_straight_lines_it_joins() {
    let metrics = metrics(9, 21);
    let corner = sprite('\u{256d}', metrics);
    let horizontal = sprite('\u{2500}', metrics);
    let vertical = sprite('\u{2502}', metrics);
    let right: Vec<u8> = (0..21).map(|y| at(&corner, 8, y)).collect();
    let straight_right: Vec<u8> = (0..21).map(|y| at(&horizontal, 8, y)).collect();
    assert_eq!(right, straight_right);
    let bottom: Vec<u8> = (0..9).map(|x| at(&corner, x, 20)).collect();
    let straight_bottom: Vec<u8> = (0..9).map(|x| at(&vertical, x, 20)).collect();
    assert_eq!(bottom, straight_bottom);
}

#[test]
fn a_double_corner_leaves_the_inside_open() {
    let metrics = metrics(9, 21);
    let corner = sprite('\u{2554}', metrics);
    let columns = pair(9, 1);
    let rows = pair(21, 1);
    let inside_x = (columns.first.1 + columns.second.0) / 2;
    let inside_y = (rows.first.1 + rows.second.0) / 2;
    assert_eq!(at(&corner, inside_x as u32, inside_y as u32), 0);
    assert_eq!(at(&corner, 8, rows.first.0 as u32), OPAQUE);
    assert_eq!(at(&corner, 8, rows.second.0 as u32), OPAQUE);
    assert_eq!(at(&corner, columns.first.0 as u32, 20), OPAQUE);
    assert_eq!(at(&corner, columns.second.0 as u32, 20), OPAQUE);
}

#[test]
fn a_wide_budget_spans_both_cells() {
    let bitmap = draw('\u{2588}', metrics(8, 20), 2).unwrap();
    assert_eq!(bitmap.width, 16);
}

#[test]
fn other_characters_are_left_to_the_font() {
    assert!(draw('a', metrics(8, 20), 1).is_none());
    assert!(draw('\u{e0b4}', metrics(8, 20), 1).is_none());
    assert!(!is_sprite('\u{25a0}'));
}
