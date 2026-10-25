use super::super::{FloatingPanesStack, KittyImageChunk, SixelImageChunk};
use super::output_tests::kitty_chunk;
use crate::panes::sixel::PixelRect;
use std::collections::BTreeSet;
use yinz_types::pane_size::{Dimension, PaneGeom, SizeInPixels};

// Cell position, source pixel origin, and source pixel size.
type Geometry = [usize; 6];

const ORIGINAL: Geometry = [2, 3, 7, 11, 19, 23];
const CELL: SizeInPixels = SizeInPixels {
    width: 4,
    height: 6,
};

fn pane(x: usize, y: usize, cols: usize, rows: usize) -> PaneGeom {
    PaneGeom {
        x,
        y,
        cols: Dimension::fixed(cols),
        rows: Dimension::fixed(rows),
        ..Default::default()
    }
}

fn clip(layers: Vec<PaneGeom>, z_index: Option<usize>) -> (Vec<Geometry>, Vec<Geometry>) {
    let mut kitty = kitty_chunk(17, 23, ORIGINAL[0], ORIGINAL[1]);
    kitty.source_px_x = ORIGINAL[2];
    kitty.source_px_y = ORIGINAL[3];
    kitty.source_px_width = ORIGINAL[4];
    kitty.source_px_height = ORIGINAL[5];
    kitty.cell_offset_x = 2;
    kitty.cell_offset_y = 3;
    kitty.z_index = -7;
    kitty.dest_cells = (11, 13);
    kitty.variant_key.source = PixelRect::new(7, 11, 23, 19);
    kitty.variant_key.size = (37, 41);
    kitty.scaled_px = Some((37, 41));
    let sixel = SixelImageChunk {
        cell_x: kitty.cell_x,
        cell_y: kitty.cell_y,
        sixel_image_pixel_x: kitty.source_px_x,
        sixel_image_pixel_y: kitty.source_px_y,
        sixel_image_pixel_width: kitty.source_px_width,
        sixel_image_pixel_height: kitty.source_px_height,
        sixel_image_id: 29,
    };
    let stack = FloatingPanesStack { layers };
    let kitty = stack
        .visible_kitty_image_chunks(vec![kitty], z_index, &CELL)
        .iter()
        .map(|chunk| {
            assert_eq!(
                KittyImageChunk {
                    cell_x: kitty.cell_x,
                    cell_y: kitty.cell_y,
                    source_px_x: kitty.source_px_x,
                    source_px_y: kitty.source_px_y,
                    source_px_width: kitty.source_px_width,
                    source_px_height: kitty.source_px_height,
                    ..*chunk
                },
                kitty,
                "clipping must preserve all Kitty metadata"
            );
            [
                chunk.cell_x,
                chunk.cell_y,
                chunk.source_px_x,
                chunk.source_px_y,
                chunk.source_px_width,
                chunk.source_px_height,
            ]
        })
        .collect();
    let sixel = stack
        .visible_sixel_image_chunks(vec![sixel], z_index, &CELL)
        .iter()
        .map(|chunk| {
            assert_eq!(chunk.sixel_image_id, sixel.sixel_image_id);
            [
                chunk.cell_x,
                chunk.cell_y,
                chunk.sixel_image_pixel_x,
                chunk.sixel_image_pixel_y,
                chunk.sixel_image_pixel_width,
                chunk.sixel_image_pixel_height,
            ]
        })
        .collect();
    (sixel, kitty)
}

#[test]
fn image_clipping_preserves_order_rounding_origins_and_protocol_metadata() {
    let cases = [
        (
            "central",
            vec![pane(3, 4, 2, 2)],
            None,
            vec![
                [2, 3, 7, 11, 20, 6],
                [2, 6, 7, 29, 20, 6],
                [2, 4, 7, 17, 4, 12],
                [5, 4, 19, 17, 8, 12],
            ],
        ),
        (
            "corner",
            vec![pane(1, 2, 3, 3)],
            None,
            vec![[2, 5, 7, 23, 20, 12], [4, 3, 15, 11, 12, 12]],
        ),
        ("covering", vec![pane(1, 2, 8, 7)], None, vec![]),
        ("disjoint", vec![pane(9, 9, 2, 2)], None, vec![ORIGINAL]),
        (
            "touching bottom edge",
            vec![pane(3, 7, 2, 2)],
            None,
            // Sixel retains empty side fragments; Kitty removes them.
            vec![
                [2, 3, 7, 11, 20, 24],
                [2, 7, 7, 35, 4, 0],
                [5, 7, 19, 35, 8, 0],
            ],
        ),
        ("no layers", vec![], None, vec![ORIGINAL]),
        (
            "skipped covering layer",
            vec![pane(1, 2, 8, 7)],
            Some(1),
            vec![ORIGINAL],
        ),
    ];
    for (name, layers, z_index, expected) in cases {
        let (sixel, kitty) = clip(layers, z_index);
        assert_eq!(sixel, expected, "{name}: Sixel");
        let nonempty: Vec<_> = expected
            .into_iter()
            .filter(|geometry| geometry[4] > 0 && geometry[5] > 0)
            .collect();
        assert_eq!(kitty, nonempty, "{name}: Kitty");
    }
}

#[test]
fn image_clipping_matches_pixel_coverage_for_layers_and_partial_overlap() {
    let layers = vec![pane(1, 2, 8, 7), pane(3, 4, 2, 2), pane(4, 5, 2, 2)];
    for (layers, z_index) in [
        (layers.clone(), 0),
        (layers.clone(), 1),
        (layers, 2),
        (vec![pane(1, 2, 3, 3)], 0),
        (vec![pane(5, 2, 3, 3)], 0),
        (vec![pane(3, 2, 2, 3)], 0),
        (vec![pane(1, 5, 3, 3)], 0),
        (vec![pane(5, 5, 3, 3)], 0),
    ] {
        let (sixel, kitty) = clip(layers.clone(), Some(z_index));
        // Compare pixels against pane occupancy, independently of rectangle splitting.
        let expected: BTreeSet<_> = (8..28)
            .flat_map(|x| (18..42).map(move |y| (x, y)))
            .filter(|&(x, y)| {
                !layers.iter().skip(z_index).any(|pane| {
                    (pane.x * CELL.width..(pane.x + pane.cols.as_usize()) * CELL.width).contains(&x)
                        && (pane.y * CELL.height..(pane.y + pane.rows.as_usize()) * CELL.height)
                            .contains(&y)
                })
            })
            .collect();
        for (protocol, chunks) in [("Sixel", sixel), ("Kitty", kitty)] {
            let mut pixels = BTreeSet::new();
            for [cell_x, cell_y, source_x, source_y, width, height] in chunks {
                assert_eq!(source_x, ORIGINAL[2] + (cell_x - ORIGINAL[0]) * CELL.width);
                assert_eq!(source_y, ORIGINAL[3] + (cell_y - ORIGINAL[1]) * CELL.height);
                for x in cell_x * CELL.width..cell_x * CELL.width + width {
                    for y in cell_y * CELL.height..cell_y * CELL.height + height {
                        assert!(pixels.insert((x, y)), "{protocol}: overlapping fragments");
                    }
                }
            }
            assert_eq!(pixels, expected, "{protocol}: z-index {z_index}");
        }
    }
}
