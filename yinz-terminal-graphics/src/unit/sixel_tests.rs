use super::*;

fn raster(ph: Option<usize>, pv: Option<usize>) -> SixelEvent {
    SixelEvent::RasterAttribute {
        pan: 1,
        pad: 1,
        ph,
        pv,
    }
}

#[test]
fn sixel_allocation_bounds_cover_rasters_and_exact_limits_without_allocating() {
    for event in [
        raster(Some(MAX_SIXEL_DIMENSION + 1), None),
        raster(None, Some(MAX_SIXEL_DIMENSION + 1)),
        raster(Some(1), Some(8193)),
        raster(None, Some(8193)),
        raster(Some(usize::MAX), Some(usize::MAX)),
    ] {
        assert!(SixelBounds::new(None).advance(event).is_none(), "{event:?}");
    }
    let mut bounds = SixelBounds::new(None);
    assert!(bounds.advance(raster(Some(8192), Some(8192))).is_some());
    assert_eq!(bounds.width * bounds.height, MAX_SIXEL_PIXELS);
    assert!(bounds.advance(raster(Some(8193), None)).is_none());

    let mut bounds = SixelBounds::new(None);
    assert!(bounds
        .advance(raster(Some(MAX_SIXEL_DIMENSION), Some(4096)))
        .is_some());
    assert!(bounds.advance(raster(None, Some(4097))).is_none());

    let mut bounds = SixelBounds::new(None);
    assert!(bounds
        .advance(raster(Some(MAX_SIXEL_DIMENSION), None))
        .is_some());
    assert!(bounds
        .advance(SixelEvent::Repeat {
            repeat_count: MAX_SIXEL_DIMENSION,
            byte_to_repeat: b'~',
        })
        .is_some());
    assert!(bounds.advance(SixelEvent::Data { byte: b'~' }).is_none());
}

#[test]
fn sixel_bounds_cover_accumulated_data_cursor_resets_and_overflow() {
    let mut bounds = SixelBounds::new(None);
    let repeat = SixelEvent::Repeat {
        repeat_count: 8192,
        byte_to_repeat: b'~',
    };
    assert!(bounds.advance(repeat).is_some());
    assert!(bounds.advance(SixelEvent::GotoBeginningOfLine).is_some());
    assert!(bounds.advance(repeat).is_some());
    assert_eq!(bounds.width, 8192);
    for _ in 0..1364 {
        assert!(bounds.advance(SixelEvent::GotoNextLine).is_some());
        assert!(bounds.advance(repeat).is_some());
    }
    assert_eq!(bounds.height, 8190);
    assert!(bounds.advance(SixelEvent::GotoNextLine).is_some());
    assert!(bounds.advance(repeat).is_none());

    let mut bounds = SixelBounds::new(None);
    assert!(bounds.advance(SixelEvent::Data { byte: b'~' }).is_some());
    assert!(bounds
        .advance(SixelEvent::Repeat {
            repeat_count: usize::MAX,
            byte_to_repeat: b'~',
        })
        .is_none());

    let mut bounds = SixelBounds::new(None);
    for _ in 0..2729 {
        assert!(bounds.advance(SixelEvent::GotoNextLine).is_some());
    }
    assert!(bounds.advance(SixelEvent::GotoNextLine).is_none());
}

#[test]
fn oversized_sixel_is_dropped_before_decoding_and_next_image_recovers() {
    for max_height in [None, Some(12)] {
        for direct in [false, true] {
            let mut grid = SixelGrid::default();
            grid.start_image(None, vec![], vec![]);
            grid.handle_byte(b'~');
            assert_eq!(grid.end_image(0, 0), Some((6, 1)));
            grid.start_image(max_height, vec![], vec![]);
            grid.handle_byte(b'~');
            if direct {
                grid.handle_event(raster(Some(1), Some(99999)));
            } else {
                for byte in b"\"1;1;1;99999~" {
                    grid.handle_byte(*byte);
                }
            }
            assert!(grid.currently_parsing.is_none());
            assert!(grid.is_parsing());
            for byte in b"#1;2;100;0;0!99999~-~" {
                grid.handle_byte(*byte);
            }
            assert_eq!(grid.end_image(0, 0), None);
            assert!(!grid.is_parsing());
            assert_eq!(grid.sixel_image_store.borrow().image_count(), 1);
            assert_eq!(grid.image_coordinates().count(), 1);
            grid.start_image(None, vec![], vec![]);
            grid.handle_byte(b'~');
            assert_eq!(grid.end_image(8, 0), Some((6, 1)));
            assert_eq!(grid.sixel_image_store.borrow().image_count(), 2);
        }
    }
}

#[test]
fn sixel_guard_preserves_transparency_and_viewport_clipping() {
    let mut grid = SixelGrid::default();
    grid.start_image(None, vec![], vec![&[0], &[1]]);
    grid.handle_event(raster(Some(1), Some(9000)));
    grid.handle_byte(b'~');
    assert_eq!(grid.end_image(0, 0), Some((6, 1)));

    grid.start_image(Some(6), vec![], vec![]);
    grid.handle_byte(b'~');
    grid.handle_byte(b'-');
    grid.handle_event(raster(Some(usize::MAX), Some(usize::MAX)));
    grid.handle_event(SixelEvent::Repeat {
        repeat_count: usize::MAX,
        byte_to_repeat: b'~',
    });
    assert_eq!(grid.end_image(0, 0), Some((6, 1)));
}

#[test]
fn sixel_bounds_cover_dependency_allocations_after_mixed_events() {
    let mut grid = SixelGrid::default();
    grid.start_image(None, vec![], vec![]);
    for event in [
        raster(None, Some(12)),
        raster(Some(2), Some(6)),
        SixelEvent::Repeat {
            repeat_count: 20,
            byte_to_repeat: b'~',
        },
        SixelEvent::GotoBeginningOfLine,
        SixelEvent::Data { byte: b'~' },
        SixelEvent::GotoNextLine,
        SixelEvent::GotoNextLine,
        SixelEvent::Data { byte: b'~' },
        raster(Some(24), None),
        raster(None, Some(30)),
    ] {
        grid.handle_event(event);
        let current = grid.currently_parsing.as_ref().unwrap();
        let image = current.decoder.clone().create_image().unwrap();
        assert!(image.pixels.len() <= current.bounds.height, "{event:?}");
        assert!(
            image
                .pixels
                .iter()
                .all(|row| row.len() <= current.bounds.width),
            "{event:?}"
        );
    }
}

#[test]
fn reaping_an_image_does_not_reuse_another_panes_live_image_id() {
    let image = SixelImage::new(b"\x1bPq\"1;1;16;12#1;2;100;0;0#1!16~-!16~\x1b\\").unwrap();
    let store = Rc::new(RefCell::new(SixelImageStore::default()));
    let mut first_pane = SixelGrid::new(Rc::default(), store.clone());
    let mut second_pane = SixelGrid::new(Rc::default(), store.clone());
    let first = first_pane.new_sixel_image(image.clone());
    let second = second_pane.new_sixel_image(image.clone());

    first_pane.reap_images(vec![first]);
    let third = first_pane.new_sixel_image(image);

    assert_ne!(third, second);
    assert_ne!(third, first);
    assert_eq!(store.borrow().image_count(), 2);
    second_pane.reap_images(vec![second]);
    assert!(store.borrow().sixel_images.contains_key(&third));
}

#[test]
fn a_fully_overwritten_image_is_reaped_at_any_position() {
    for (x, y) in [(0, 0), (24, 0), (0, 12), (24, 12)] {
        let mut grid = SixelGrid::default();
        for _ in 0..2 {
            grid.start_image(None, vec![], vec![]);
            for byte in b"\"1;1;16;12#1;2;100;0;0#1!16~-!16~" {
                grid.handle_byte(*byte);
            }
            assert_eq!(grid.end_image(x, y), Some((12, 16)));
        }
        assert_eq!(grid.image_coordinates().count(), 1, "position ({x}, {y})");
        let reaped = grid.drain_image_ids_to_reap().unwrap();
        assert_eq!(reaped, vec![0]);
        grid.reap_images(reaped);
        assert_eq!(grid.sixel_image_store.borrow().image_count(), 1);
        assert!(grid
            .sixel_image_store
            .borrow()
            .sixel_images
            .contains_key(&1));
    }
}

#[test]
fn sixel_damage_clips_signed_scrollback_coordinates_and_excludes_touching_edges() {
    let cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 6,
    })));
    let mut grid = SixelGrid::new(cell_size, Rc::default());
    // A three-row image loses its first row when the top of scrollback is evicted.
    grid.sixel_image_locations
        .insert(7, PixelRect::new(8, 0, 18, 16));
    grid.offset_grid_top();
    let chunks =
        grid.changed_sixel_chunks_in_viewport(HashMap::from([(0, 1), (1, 1), (2, 1)]), 0, 2, 4, 5);
    assert_eq!(
        chunks.len(),
        2,
        "the third row only touches the image's bottom edge"
    );
    let mut slices: Vec<_> = chunks
        .iter()
        .map(|chunk| {
            (
                chunk.cell_x,
                chunk.cell_y,
                chunk.sixel_image_pixel_x,
                chunk.sixel_image_pixel_y,
                chunk.sixel_image_pixel_width,
                chunk.sixel_image_pixel_height,
                chunk.sixel_image_id,
            )
        })
        .collect();
    slices.sort();
    assert_eq!(slices, vec![(5, 5, 0, 6, 8, 6, 7), (5, 6, 0, 12, 8, 6, 7)]);
    assert_eq!(
        grid.image_cell_coordinates_in_viewport(3, 0),
        vec![(0, 2, 1, 3)]
    );
    assert!(grid.image_cell_coordinates_in_viewport(3, 2).is_empty());
    assert!(grid
        .changed_sixel_chunks_in_viewport(HashMap::from([(0, 1)]), 2, 2, 0, 0)
        .is_empty());
    // Looking down into scrollback crops from the image, then places it relative to the viewport.
    let chunks = grid.changed_sixel_chunks_in_viewport(HashMap::from([(0, 1)]), 1, 3, 0, 0);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].cell_y, 0);
    assert_eq!(chunks[0].sixel_image_pixel_y, 12);
}

#[test]
fn cutting_pixels_invalidates_only_overlapping_cached_rectangles() {
    let image = SixelImage::new(b"\x1bPq\"1;1;16;12#1;2;100;0;0#1!16~-!16~\x1b\\").unwrap();
    let mut grid = SixelGrid::default();
    grid.new_sixel_image(image);
    let left = PixelRect::new(0, 0, 6, 8);
    let right = PixelRect::new(8, 0, 6, 8);
    let below = PixelRect::new(0, 6, 6, 8);
    let full = PixelRect::new(0, 0, 12, 16);
    let mut before = HashMap::new();
    for rect in [left, right, below, full] {
        let serialized = grid
            .sixel_image_store
            .borrow_mut()
            .serialize_image(0, rect.x, rect.y as usize, rect.width, rect.height)
            .unwrap();
        before.insert(rect, serialized);
    }

    grid.remove_pixels_from_image(0, left);

    let mut store = grid.sixel_image_store.borrow_mut();
    let cache = &store.sixel_images[&0].1;
    assert_eq!(cache.len(), 2);
    assert_eq!(cache.get(&right), before.get(&right));
    assert_eq!(cache.get(&below), before.get(&below));
    for rect in [left, full] {
        assert_ne!(
            store
                .serialize_image(0, rect.x, rect.y as usize, rect.width, rect.height)
                .as_ref(),
            before.get(&rect)
        );
    }
}

#[test]
fn multiple_cuts_reap_empty_images_once_and_preserve_retained_pixels() {
    let mut grid = SixelGrid::default();
    grid.start_image(None, vec![], vec![]);
    for byte in b"\"1;1;16;12#1;2;100;0;0#1!16~-!16~" {
        grid.handle_byte(*byte);
    }
    grid.end_image(24, 12);
    let retained = grid.sixel_image_store.borrow_mut().retain_image(0).unwrap();
    let before = grid
        .sixel_image_store
        .borrow_mut()
        .serialize_image(retained, 0, 0, 16, 12)
        .unwrap();
    grid.remove_pixels_from_image(0, PixelRect::new(0, 0, 12, 8));
    grid.remove_pixels_from_image(0, PixelRect::new(0, 0, 12, 8));
    assert_eq!(grid.images_to_check_for_emptiness.len(), 1);
    assert!(grid.drain_image_ids_to_reap().is_none());
    assert_eq!(grid.image_coordinates().count(), 1);
    grid.remove_pixels_from_image(0, PixelRect::new(8, 0, 12, 8));
    let reaped = grid.drain_image_ids_to_reap().unwrap();
    assert_eq!(reaped, vec![0]);
    assert_eq!(grid.image_coordinates().count(), 0);
    grid.reap_images(reaped);
    assert!(grid.drain_image_ids_to_reap().is_none());
    let mut store = grid.sixel_image_store.borrow_mut();
    assert_eq!(store.image_count(), 1);
    assert_eq!(
        store.serialize_image(retained, 0, 0, 16, 12).unwrap(),
        before
    );
    drop(store);
    grid.remove_pixels_from_image(0, PixelRect::default());
    grid.clear();
    assert!(grid.images_to_check_for_emptiness.is_empty());
}
