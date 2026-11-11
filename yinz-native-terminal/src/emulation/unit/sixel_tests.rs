use super::*;

const OPAQUE_BACKGROUND: [u8; 4] = [
    color::DEFAULT_BACKGROUND[0],
    color::DEFAULT_BACKGROUND[1],
    color::DEFAULT_BACKGROUND[2],
    255,
];

fn sixel(body: &str) -> Vec<u8> {
    format!("\x1bP0;1;0q{}\x1b\\", body).into_bytes()
}

fn pixel(image: &Image, x: u32, y: u32) -> [u8; 4] {
    let start = ((y * image.width + x) * 4) as usize;
    let mut pixel = [0u8; 4];
    pixel.copy_from_slice(&image.pixels[start..start + 4]);
    pixel
}

#[test]
fn a_raster_attribute_sizes_the_canvas() {
    let image = decode(&sixel("\"1;1;4;12#0;2;100;0;0@")).unwrap();
    assert_eq!((image.width, image.height), (4, 12));
    assert_eq!(image.pixels.len(), 4 * 12 * 4);
}

#[test]
fn without_a_raster_attribute_the_data_sizes_the_canvas() {
    let image = decode(&sixel("#0;2;100;100;100~~~")).unwrap();
    assert_eq!((image.width, image.height), (3, 6));
}

#[test]
fn each_data_byte_paints_the_six_pixels_of_its_bits() {
    let image = decode(&sixel("\"1;1;1;6#0;2;100;0;0@")).unwrap();
    assert_eq!(pixel(&image, 0, 0), [255, 0, 0, 255]);
    for row in 1..6 {
        assert_eq!(pixel(&image, 0, row), [0, 0, 0, 0], "row {}", row);
    }
}

#[test]
fn a_data_byte_with_every_bit_set_paints_the_whole_band() {
    let image = decode(&sixel("\"1;1;1;6#0;2;0;100;0\x7e")).unwrap();
    for row in 0..6 {
        assert_eq!(pixel(&image, 0, row), [0, 255, 0, 255], "row {}", row);
    }
}

#[test]
fn a_repeat_paints_the_same_column_run() {
    let image = decode(&sixel("\"1;1;8;6#0;2;0;0;100!5@")).unwrap();
    for column in 0..5 {
        assert_eq!(pixel(&image, column, 0), [0, 0, 255, 255], "{}", column);
    }
    assert_eq!(pixel(&image, 5, 0), [0, 0, 0, 0]);
}

#[test]
fn a_carriage_return_paints_a_second_color_over_the_same_band() {
    let image = decode(&sixel("\"1;1;2;6#0;2;100;0;0@@$#1;2;0;0;100?@")).unwrap();
    assert_eq!(pixel(&image, 0, 0), [255, 0, 0, 255]);
    assert_eq!(pixel(&image, 1, 0), [0, 0, 255, 255]);
}

#[test]
fn a_new_line_advances_by_one_band() {
    let image = decode(&sixel("\"1;1;1;12#0;2;100;0;0@-#0;2;100;0;0@")).unwrap();
    assert_eq!(pixel(&image, 0, 0), [255, 0, 0, 255]);
    assert_eq!(pixel(&image, 0, 6), [255, 0, 0, 255]);
}

#[test]
fn a_transparent_background_leaves_unpainted_pixels_clear() {
    let image = decode(&sixel("\"1;1;2;6#0;2;100;100;100@")).unwrap();
    assert_eq!(pixel(&image, 1, 0), [0, 0, 0, 0]);
}

#[test]
fn an_opaque_background_fills_unpainted_pixels_with_the_default_background() {
    let stream = b"\x1bP0;0;0q\"1;1;2;6#0;2;100;100;100@\x1b\\";
    let image = decode(stream).unwrap();
    assert_eq!(pixel(&image, 0, 0), [255, 255, 255, 255]);
    assert_eq!(pixel(&image, 1, 0), OPAQUE_BACKGROUND);
}

#[test]
fn a_missing_transparency_parameter_is_opaque_too() {
    let image = decode(b"\x1bPq\"1;1;2;6#0;2;100;100;100@\x1b\\").unwrap();
    assert_eq!(pixel(&image, 1, 0), OPAQUE_BACKGROUND);
}

#[test]
fn rgb_registers_are_percentages_of_full_scale() {
    let image = decode(&sixel("\"1;1;1;6#7;2;50;25;0@")).unwrap();
    assert_eq!(pixel(&image, 0, 0), [128, 64, 0, 255]);
}

#[test]
fn an_hls_register_is_converted_with_the_dec_hue_origin() {
    let blue = decode(&sixel("\"1;1;1;6#1;1;0;50;100@")).unwrap();
    assert_eq!(pixel(&blue, 0, 0), [0, 0, 255, 255]);
    let red = decode(&sixel("\"1;1;1;6#1;1;120;50;100@")).unwrap();
    assert_eq!(pixel(&red, 0, 0), [255, 0, 0, 255]);
    let green = decode(&sixel("\"1;1;1;6#1;1;240;50;100@")).unwrap();
    assert_eq!(pixel(&green, 0, 0), [0, 255, 0, 255]);
}

#[test]
fn an_unsaturated_hls_register_is_a_grey_of_its_lightness() {
    let image = decode(&sixel("\"1;1;1;6#1;1;0;50;0@")).unwrap();
    assert_eq!(pixel(&image, 0, 0), [128, 128, 128, 255]);
}

#[test]
fn an_undefined_register_falls_back_to_the_vt340_palette() {
    let image = decode(&sixel("\"1;1;1;6#3@")).unwrap();
    assert_eq!(pixel(&image, 0, 0), [51, 204, 51, 255]);
}

#[test]
fn a_register_redefined_mid_image_repaints_only_what_follows() {
    let image = decode(&sixel("\"1;1;2;6#0;2;100;0;0@#0;2;0;0;100@")).unwrap();
    assert_eq!(pixel(&image, 0, 0), [255, 0, 0, 255]);
    assert_eq!(pixel(&image, 1, 0), [0, 0, 255, 255]);
}

#[test]
fn data_beyond_a_declared_raster_is_clipped_rather_than_growing_it() {
    let image = decode(&sixel("\"1;1;2;6#0;2;100;0;0!8@--#0;2;100;0;0@")).unwrap();
    assert_eq!((image.width, image.height), (2, 6));
}

#[test]
fn an_oversized_raster_attribute_is_refused() {
    assert!(decode(&sixel("\"1;1;20000;20000#0;2;100;0;0@")).is_none());
}

#[test]
fn a_repeat_past_the_dimension_limit_is_clamped_rather_than_allocated() {
    let image = decode(&sixel("#0;2;100;0;0!99999~")).unwrap();
    assert_eq!(image.width as usize, MAX_DIMENSION);
    assert_eq!(image.height, 6);
}

#[test]
fn bands_past_the_dimension_limit_paint_nothing() {
    let mut body = String::from("#0;2;100;0;0@");
    body.push_str(&"-".repeat(MAX_DIMENSION / BAND + 4));
    body.push_str("#0;2;100;0;0@");
    let image = decode(&sixel(&body)).unwrap();
    assert!(image.height as usize <= MAX_DIMENSION, "{}", image.height);
}

#[test]
fn an_undeclared_image_over_the_byte_budget_is_refused() {
    let mut body = format!("#0;2;100;0;0!{}~", MAX_DIMENSION);
    for _ in 0..MAX_IMAGE_BYTES / (MAX_DIMENSION * 4 * BAND) + 1 {
        body.push_str(&format!("-!{}~", MAX_DIMENSION));
    }
    assert!(decode(&sixel(&body)).is_none());
}

#[test]
fn a_register_value_past_full_scale_is_clamped() {
    assert_eq!(percent(200), 255);
    assert_eq!(hls_to_rgb(0, 200, 200), [255, 255, 255]);
}

#[test]
fn an_empty_image_decodes_to_nothing() {
    assert!(decode(&sixel("")).is_none());
    assert!(decode(b"").is_none());
}

#[test]
fn a_malformed_payload_is_refused_rather_than_panicking() {
    for payload in [
        &b"\x1bP0;1;0q#not a register@\x1b\\"[..],
        &b"\x1bP99999999;1;0q@\x1b\\"[..],
        &b"\x1bP0;1;0q\"1;1;;;;;#\x1b\\"[..],
        &b"\x1bP0;1;0q\xff\xfe\x1b\\"[..],
    ] {
        let _ = decode(payload);
    }
}

fn cell() -> CellSize {
    CellSize::new(8, 20)
}

fn store() -> Sixels {
    Sixels::new(cell())
}

fn place(store: &mut Sixels, cell_x: usize, cell_y: usize, body: &str) {
    store.execute(&sixel(body), (cell_y, cell_x));
    store.commit();
}

fn opaque(width: usize, height: usize) -> String {
    format!("\"1;1;{};{}#0;2;100;0;0!{}~", width, height, width)
}

fn band(width: usize, bands: usize) -> String {
    let mut body = format!("\"1;1;{};{}", width, bands * 6);
    for _ in 0..bands {
        body.push_str(&format!("#0;2;100;0;0!{}~-", width));
    }
    body
}

#[test]
fn a_chunk_is_anchored_at_the_cursor_cell() {
    let mut store = store();
    place(&mut store, 3, 2, &opaque(16, 6));
    let chunk = store.chunks().next().unwrap();
    assert_eq!((chunk.cell_x, chunk.cell_y), (3, 2));
    assert_eq!((chunk.width, chunk.height), (16, 6));
    assert_eq!(
        chunk.rect(cell()),
        PixelRect {
            x: 24,
            y: 40,
            width: 16,
            height: 6
        }
    );
}

#[test]
fn a_chunk_that_covers_an_older_one_replaces_it() {
    let mut store = store();
    place(&mut store, 0, 0, &band(16, 4));
    place(&mut store, 0, 0, &band(16, 4));
    assert_eq!(store.chunks().count(), 1);
}

#[test]
fn a_chunk_that_covers_part_of_an_older_one_erases_only_that_part() {
    let mut store = store();
    place(&mut store, 0, 0, &band(16, 7));
    place(&mut store, 0, 1, &band(16, 1));

    let chunks: Vec<&Chunk> = store.chunks().collect();
    assert_eq!(chunks.len(), 2);
    let older = chunks[0];
    assert_eq!(pixel(&older.image, 0, 19), [255, 0, 0, 255]);
    assert_eq!(pixel(&older.image, 0, 20), [0, 0, 0, 0]);
    assert_eq!(pixel(&older.image, 0, 26), [255, 0, 0, 255]);
}

#[test]
fn an_erase_bumps_the_revision_so_the_texture_is_reuploaded() {
    let mut store = store();
    place(&mut store, 0, 0, &band(16, 7));
    let before = store.chunks().next().unwrap().revision;
    place(&mut store, 0, 1, &band(16, 1));
    let after = store.chunks().next().unwrap().revision;
    assert!(after > before, "{} is not newer than {}", after, before);
}

#[test]
fn a_damaged_line_erases_the_band_it_covers() {
    let mut store = store();
    place(&mut store, 0, 0, &band(16, 7));
    store.cut_spans(&[Span::new(1, 0, 7)]);

    let chunk = store.chunks().next().unwrap();
    assert_eq!(pixel(&chunk.image, 0, 19), [255, 0, 0, 255]);
    assert_eq!(pixel(&chunk.image, 0, 20), [0, 0, 0, 0]);
    assert_eq!(pixel(&chunk.image, 0, 39), [0, 0, 0, 0]);
    assert_eq!(pixel(&chunk.image, 0, 40), [255, 0, 0, 255]);
}

#[test]
fn a_line_outside_the_chunk_leaves_it_alone() {
    let mut store = store();
    place(&mut store, 0, 0, &band(16, 3));
    let revision = store.chunks().next().unwrap().revision;
    store.cut_spans(&[Span::new(4, 0, 7), Span::new(5, 0, 7)]);
    assert_eq!(store.chunks().next().unwrap().revision, revision);
}

#[test]
fn a_chunk_erased_to_nothing_is_dropped() {
    let mut store = store();
    place(&mut store, 0, 0, &band(16, 7));
    store.cut_spans(&[Span::new(0, 0, 7)]);
    assert!(store.chunks().next().is_some());
    store.cut_spans(&[Span::new(1, 0, 7), Span::new(2, 0, 7)]);
    assert!(store.is_empty(), "an emptied chunk stayed resident");
}

#[test]
fn a_transparent_chunk_is_dropped_by_the_lines_it_covers() {
    let mut store = store();
    place(&mut store, 0, 0, "\"1;1;16;20#0;2;100;0;0@");
    store.cut_spans(&[Span::new(0, 0, 1)]);
    assert!(store.is_empty());
}

#[test]
fn residency_is_reported_in_ascending_order() {
    let mut store = store();
    for row in [4, 0, 2] {
        place(&mut store, 0, row, &band(16, 1));
    }
    let ids = store.resident_ids();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted);
    assert_eq!(ids.len(), 3);
}

#[test]
fn a_cell_size_change_retires_the_chunks_positioned_against_the_old_one() {
    let mut store = store();
    place(&mut store, 0, 0, &band(16, 1));
    store.set_cell_size(CellSize::new(8, 20));
    assert_eq!(store.chunks().count(), 1);
    store.set_cell_size(CellSize::new(16, 40));
    assert!(store.is_empty());
}

#[test]
fn a_clear_drops_committed_and_pending_chunks_alike() {
    let mut store = store();
    place(&mut store, 0, 0, &band(16, 1));
    store.execute(&sixel(&band(16, 1)), (2, 0));
    store.clear();
    store.commit();
    assert!(store.is_empty());
}

#[test]
fn an_undecodable_payload_commits_nothing() {
    let mut store = store();
    store.execute(b"\x1bP0;1;0q\x1b\\", (0, 0));
    store.commit();
    assert!(store.is_empty());
}

#[test]
fn the_chunk_budget_retires_the_oldest_chunks() {
    let mut store = store();
    for row in 0..MAX_CHUNKS + 8 {
        place(&mut store, row % 4, row, &band(4, 1));
    }
    assert_eq!(store.chunks().count(), MAX_CHUNKS);
    assert_eq!(store.resident_ids()[0], 9);
}

#[test]
fn a_zero_cell_dimension_is_clamped_rather_than_dividing_by_it() {
    let cell = CellSize::new(0, 0);
    assert_eq!(cell, CellSize::new(1, 1));
}
