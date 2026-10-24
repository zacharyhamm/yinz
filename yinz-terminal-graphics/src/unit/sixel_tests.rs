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
    let first = first_pane.new_sixel_image(image.clone(), &[]).unwrap();
    let second = second_pane.new_sixel_image(image.clone(), &[]).unwrap();

    first_pane.reap_images(vec![first]);
    let third = first_pane.new_sixel_image(image, &[]).unwrap();

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
        grid.sixel_image_store.borrow_mut().limits.images = 2;
        for _ in 0..20 {
            grid.start_image(None, vec![], vec![]);
            for byte in b"\"1;1;16;12#1;2;100;0;0#1!16~-!16~" {
                grid.handle_byte(*byte);
            }
            assert_eq!(grid.end_image(x, y), Some((12, 16)));
        }
        assert_eq!(grid.image_coordinates().count(), 1, "position ({x}, {y})");
        assert!(grid.drain_image_ids_to_reap().is_none());
        assert_eq!(grid.sixel_image_store.borrow().image_count(), 1);
        assert!(grid
            .sixel_image_store
            .borrow()
            .sixel_images
            .contains_key(&38));
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
    grid.new_sixel_image(image, &[]).unwrap();
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
    let cache = &store.sixel_images[&0].cache;
    assert_eq!(cache.len(), 2);
    assert_eq!(
        cache.get(&right).map(AsRef::as_ref),
        before.get(&right).map(String::as_str)
    );
    assert_eq!(
        cache.get(&below).map(AsRef::as_ref),
        before.get(&below).map(String::as_str)
    );
    assert_eq!(store.cache_entries, 2);
    assert_eq!(
        store.cache_bytes,
        cache.values().map(|cached| cached.len()).sum::<usize>()
    );
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
fn unchanged_cuts_preserve_cached_serialization_and_shared_pixels() {
    let mut image = stored_image();
    image.cut_out(4, 3, 12, 9);
    let mut store = SixelImageStore::default();
    let live = store.insert_image(image, &[]).unwrap();
    let retained = store.retain_image(live).unwrap();
    let full = PixelRect::new(0, 0, 12, 16);
    let before = store.serialize_image(live, 0, 0, 16, 12).unwrap();

    for rect in [
        PixelRect::new(4, 3, 6, 8),
        PixelRect::new(12, 9, 6, 8),
        PixelRect::new(0, 0, 6, 0),
        PixelRect::new(0, 0, 0, 8),
        PixelRect::new(16, 0, 6, 8),
        PixelRect::new(0, 12, 6, 8),
    ] {
        assert!(!store.remove_pixels_from_image(live, rect));
        assert!(Rc::ptr_eq(
            &store.sixel_images[&live].image,
            &store.sixel_images[&retained].image
        ));
        assert_eq!(store.sixel_images[&live].cache[&full].as_ref(), before);
        assert_eq!((store.cache_bytes, store.cache_entries), (before.len(), 1));
    }
    assert!(!store.remove_pixels_from_image(usize::MAX, full));
    assert_eq!(store.serialize_image(live, 0, 0, 16, 12).unwrap(), before);
}

#[test]
fn initially_empty_images_are_reaped_without_copying_or_invalidating_cache() {
    for retain_snapshot in [false, true] {
        let mut grid = SixelGrid::default();
        finish_image(&mut grid, b"!8?", 0, 0).unwrap();
        let store = grid.sixel_image_store.clone();
        let retained = retain_snapshot.then(|| store.borrow_mut().retain_image(0).unwrap());
        let image = store.borrow().sixel_images[&0].image.clone();
        let before = store.borrow_mut().serialize_image(0, 0, 0, 8, 6).unwrap();
        let bytes = store.borrow().reserved_bytes;

        for _ in 0..2 {
            grid.remove_pixels_from_image(0, PixelRect::new(0, 0, 6, 4));
            let store = store.borrow();
            assert!(Rc::ptr_eq(&image, &store.sixel_images[&0].image));
            assert_eq!(store.cache_entries, 1);
            assert_eq!(store.cache_bytes, before.len());
        }
        assert_eq!(grid.images_to_check_for_emptiness, HashSet::from([0]));
        let reaped = grid.drain_image_ids_to_reap().unwrap();
        assert_eq!(reaped, vec![0]);
        grid.reap_images(reaped);
        assert_eq!(grid.image_coordinates().count(), 0);
        assert!(grid.drain_image_ids_to_reap().is_none());

        let mut store = store.borrow_mut();
        if let Some(retained) = retained {
            assert_eq!(store.image_count(), 1);
            assert_eq!((store.reserved_bytes, store.reserved_images), (bytes, 2));
            assert_eq!(store.serialize_image(retained, 0, 0, 8, 6).unwrap(), before);
            store.remove_image(retained);
        }
        assert_eq!(store.image_count(), 0);
        assert_eq!((store.reserved_bytes, store.reserved_images), (0, 0));
        assert_eq!((store.cache_bytes, store.cache_entries), (0, 0));
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
    assert!(grid.images_to_check_for_emptiness.is_empty());
    grid.clear();
    assert!(grid.images_to_check_for_emptiness.is_empty());
}

fn stored_image() -> SixelImage {
    SixelImage::new(b"\x1bPq\"1;1;16;12#1;2;100;0;0#1!16~-!16~\x1b\\").unwrap()
}

fn finish_image(grid: &mut SixelGrid, body: &[u8], x: usize, y: usize) -> Option<(usize, usize)> {
    grid.start_image(None, vec![], vec![]);
    for byte in body {
        grid.handle_byte(*byte);
    }
    grid.end_image(x, y)
}

#[test]
fn image_accounting_includes_spare_pixel_capacity_and_palette_storage() {
    let mut image = stored_image();
    let before = image_storage_bytes(&image).unwrap();
    let old_rows = image.pixels.capacity();
    let old_pixels = image.pixels[0].capacity();
    image.pixels.reserve(100);
    image.pixels[0].reserve(100);
    let extra_bytes = (image.pixels.capacity() - old_rows) * size_of::<Vec<Pixel>>()
        + (image.pixels[0].capacity() - old_pixels) * size_of::<Pixel>();
    assert_eq!(image_storage_bytes(&image), Some(before + extra_bytes));
    image
        .color_registers
        .insert(2, sixel_image::SixelColor::Rgb(0, 100, 0));
    assert!(image_storage_bytes(&image).unwrap() >= before + extra_bytes + 256);

    let bytes = image_storage_bytes(&image).unwrap() * 2;
    let mut store = SixelImageStore::default();
    store.limits.image_bytes = bytes;
    let id = store.insert_image(image, &[]).unwrap();
    let retained = store.retain_image(id).unwrap();
    store.remove_pixels_from_image(id, PixelRect::new(0, 0, 6, 8));
    assert!(
        image_storage_bytes(&store.sixel_images[&id].image).unwrap()
            + image_storage_bytes(&store.sixel_images[&retained].image).unwrap()
            <= bytes
    );
    assert_eq!(store.reserved_bytes, bytes);
}

#[test]
fn admission_enforces_exact_byte_and_reservation_limits_and_recovers_after_removal() {
    let bytes = image_storage_bytes(&stored_image()).unwrap() * 2;
    for (byte_limit, slots, accepted) in
        [(bytes - 1, 2, false), (bytes, 1, false), (bytes, 2, true)]
    {
        let mut store = SixelImageStore::default();
        store.limits.image_bytes = byte_limit;
        store.limits.images = slots;
        let id = store.insert_image(stored_image(), &[]);
        assert_eq!(id.is_some(), accepted);
        assert_eq!(store.image_count(), usize::from(accepted));
        assert_eq!(store.reserved_bytes, if accepted { bytes } else { 0 });
        assert_eq!(store.reserved_images, if accepted { 2 } else { 0 });
        assert!(store.insert_image(stored_image(), &[]).is_none());
        if let Some(id) = id {
            store.remove_image(id);
            store.remove_image(id);
            assert_eq!((store.reserved_bytes, store.reserved_images), (0, 0));
            assert!(store.insert_image(stored_image(), &[]).is_some());
        }
    }
}

#[test]
fn admission_reserves_snapshot_ids_before_id_exhaustion() {
    let mut store = SixelImageStore {
        next_image_id: usize::MAX - 3,
        ..Default::default()
    };
    let id = store.insert_image(stored_image(), &[]).unwrap();
    assert!(store.insert_image(stored_image(), &[id]).is_none());
    assert_eq!(store.retain_image(id), Some(id + 1));
    assert_eq!(store.image_count(), 2);
}

#[test]
fn many_images_share_one_quota_across_panes_and_drop_releases_only_owned_images() {
    let store = Rc::new(RefCell::new(SixelImageStore::default()));
    store.borrow_mut().limits.images = 100;
    let mut first = SixelGrid::new(Rc::default(), store.clone());
    let mut second = SixelGrid::new(Rc::default(), store.clone());
    for x in 0..25 {
        assert_eq!(finish_image(&mut first, b"~", x * 8, 0), Some((6, 1)));
        assert_eq!(finish_image(&mut second, b"~", x * 8, 0), Some((6, 1)));
    }
    assert_eq!(store.borrow().reserved_images, 100);
    let bytes = store.borrow().reserved_bytes;
    for grid in [&mut first, &mut second] {
        assert_eq!(finish_image(grid, b"~", 200, 0), None);
        assert_eq!(grid.image_coordinates().count(), 25);
    }
    drop(first);
    assert_eq!(store.borrow().reserved_bytes, bytes / 2);
    assert_eq!(store.borrow().reserved_images, 50);
    assert_eq!(finish_image(&mut second, b"~", 200, 0), Some((6, 1)));
    drop(second);
    assert_eq!(store.borrow().image_count(), 0);
    assert_eq!(
        (
            store.borrow().reserved_bytes,
            store.borrow().reserved_images
        ),
        (0, 0)
    );
}

#[test]
fn failed_replacements_preserve_pixels_locations_and_cache_until_admission_succeeds() {
    let mut grid = SixelGrid::default();
    let store = grid.sixel_image_store.clone();
    assert_eq!(finish_image(&mut grid, b"~", 24, 12), Some((6, 1)));
    let bytes = store.borrow().reserved_bytes;
    store.borrow_mut().limits.image_bytes = bytes;
    store.borrow_mut().limits.images = 2;
    grid.remove_pixels_from_image(0, PixelRect::new(0, 0, 1, 1));
    let before = store.borrow_mut().serialize_image(0, 0, 0, 1, 6).unwrap();
    assert_eq!(finish_image(&mut grid, b"!100~", 24, 12), None);
    assert_eq!(
        grid.image_coordinates().collect::<Vec<_>>(),
        vec![(0, &PixelRect::new(24, 12, 6, 1))]
    );
    assert_eq!(store.borrow().cache_entries, 1);
    assert_eq!(
        store.borrow_mut().serialize_image(0, 0, 0, 1, 6).unwrap(),
        before
    );

    let retained = store.borrow_mut().retain_image(0).unwrap();
    assert_eq!(finish_image(&mut grid, b"~", 24, 12), None);
    assert_eq!(store.borrow().image_count(), 2);
    assert_eq!(store.borrow().reserved_bytes, bytes);
    store.borrow_mut().remove_image(retained);
    assert_eq!(finish_image(&mut grid, b"~", 24, 12), Some((6, 1)));
    assert_eq!(store.borrow().image_count(), 1);
    assert_eq!(store.borrow().cache_entries, 0);
    assert_eq!(store.borrow().cache_bytes, 0);
    assert_eq!(store.borrow().reserved_bytes, bytes);
}

#[test]
fn replacing_multiple_images_only_reclaims_reservations_without_snapshots() {
    let mut grid = SixelGrid::default();
    let store = grid.sixel_image_store.clone();
    store.borrow_mut().limits.images = 4;
    finish_image(&mut grid, b"~", 0, 0).unwrap();
    finish_image(&mut grid, b"~", 8, 0).unwrap();
    let original_bytes = store.borrow().reserved_bytes / 2;
    let retained = store.borrow_mut().retain_image(0).unwrap();
    let original = store
        .borrow_mut()
        .serialize_image(retained, 0, 0, 1, 6)
        .unwrap();

    assert_eq!(finish_image(&mut grid, b"!9~", 0, 0), Some((6, 9)));
    assert_eq!(grid.image_coordinates().count(), 1);
    assert_eq!(store.borrow().image_count(), 2);
    assert_eq!(store.borrow().reserved_images, 4);
    assert_eq!(
        store
            .borrow_mut()
            .serialize_image(retained, 0, 0, 1, 6)
            .unwrap(),
        original
    );
    drop(grid);
    assert_eq!(store.borrow().image_count(), 1);
    assert_eq!(store.borrow().reserved_bytes, original_bytes);
    store.borrow_mut().remove_image(retained);
    assert_eq!(store.borrow().reserved_bytes, 0);
    assert_eq!(store.borrow().reserved_images, 0);
}

#[test]
fn sixel_completions_defer_sparse_image_scans_when_replacements_fit() {
    for limit in ["none", "bytes", "images"] {
        let mut grid = SixelGrid::default();
        let body = format!("{}!2048~", "!2048?-".repeat(340));
        assert_eq!(
            finish_image(&mut grid, body.as_bytes(), 0, 0),
            Some((2046, 2048))
        );
        grid.remove_pixels_from_image(0, PixelRect::new(0, 2040, 6, 1));
        finish_image(&mut grid, b"~", 2048, 0).unwrap();
        assert!(grid.images_to_check_for_emptiness.contains(&0));
        let store = grid.sixel_image_store.clone();
        match limit {
            "bytes" => {
                let bytes = store.borrow().reserved_bytes;
                store.borrow_mut().limits.image_bytes = bytes;
            },
            "images" => store.borrow_mut().limits.images = 4,
            _ => {},
        }

        for update in 0..1_000 {
            grid.remove_pixels_from_image(0, PixelRect::new(0, 0, 6, 1));
            grid.remove_pixels_from_image(2 + update * 2, PixelRect::new(0, 0, 6, 1));
            assert_eq!(finish_image(&mut grid, b"~", 2048, 0), Some((6, 1)));
            assert_eq!(
                grid.images_to_check_for_emptiness,
                HashSet::from([0]),
                "{limit}"
            );
        }
        assert!(grid.drain_image_ids_to_reap().is_none());
        assert!(grid.images_to_check_for_emptiness.is_empty());
        assert_eq!(grid.image_coordinates().count(), 2);
        assert_eq!(store.borrow().image_count(), 2);
    }
}

#[test]
fn transparent_cuts_do_not_queue_sparse_image_scans_at_capacity() {
    for limit_bytes in [false, true] {
        let mut grid = SixelGrid::default();
        let body = format!("{}!2048~", "!2048?-".repeat(340));
        assert_eq!(
            finish_image(&mut grid, body.as_bytes(), 0, 0),
            Some((2046, 2048))
        );
        let store = grid.sixel_image_store.clone();
        let bytes = store.borrow().reserved_bytes;
        if limit_bytes {
            store.borrow_mut().limits.image_bytes = bytes;
        } else {
            store.borrow_mut().limits.images = 2;
        }

        for _ in 0..1_000 {
            grid.remove_pixels_from_image(0, PixelRect::new(0, 0, 6, 1));
            assert!(grid.images_to_check_for_emptiness.is_empty());
            assert_eq!(finish_image(&mut grid, b"~", 2048, 0), None);
            assert!(grid.images_to_check_for_emptiness.is_empty());
        }
        assert_eq!(grid.image_coordinates().count(), 1);
        assert_eq!(store.borrow().image_count(), 1);
        assert_eq!(store.borrow().reserved_bytes, bytes);
    }
}

#[test]
fn admission_defers_snapshot_backed_sparse_image_scans_until_render() {
    for limit_bytes in [false, true] {
        let mut grid = SixelGrid::default();
        let body = format!("{}!2048~", "!2048?-".repeat(340));
        assert_eq!(
            finish_image(&mut grid, body.as_bytes(), 0, 0),
            Some((2046, 2048))
        );
        let store = grid.sixel_image_store.clone();
        let retained = store.borrow_mut().retain_image(0).unwrap();
        let before = store
            .borrow_mut()
            .serialize_image(retained, 0, 2040, 2048, 6)
            .unwrap();
        let bytes = store.borrow().reserved_bytes;
        if limit_bytes {
            store.borrow_mut().limits.image_bytes = bytes;
        } else {
            store.borrow_mut().limits.images = 2;
        }

        for x in 0..1_000 {
            grid.remove_pixels_from_image(0, PixelRect::new(x, 2040, 6, 1));
            assert_eq!(finish_image(&mut grid, b"~", 2048, 0), None);
            assert_eq!(grid.images_to_check_for_emptiness, HashSet::from([0]));
        }
        assert!(grid.drain_image_ids_to_reap().is_none());
        assert!(grid.images_to_check_for_emptiness.is_empty());
        assert_eq!(grid.image_coordinates().count(), 1);

        grid.remove_pixels_from_image(0, PixelRect::new(1_000, 2040, 6, 1048));
        assert_eq!(finish_image(&mut grid, b"~", 2048, 0), None);
        assert_eq!(grid.images_to_check_for_emptiness, HashSet::from([0]));
        let reaped = grid.drain_image_ids_to_reap().unwrap();
        assert_eq!(reaped, vec![0]);
        grid.reap_images(reaped);
        assert_eq!(store.borrow().image_count(), 1);
        assert_eq!(store.borrow().reserved_bytes, bytes);
        assert_eq!(store.borrow().reserved_images, 2);
        assert_eq!(
            store
                .borrow_mut()
                .serialize_image(retained, 0, 2040, 2048, 6)
                .unwrap(),
            before
        );
        store.borrow_mut().remove_image(retained);
        assert_eq!(store.borrow().reserved_bytes, 0);
        assert_eq!(store.borrow().reserved_images, 0);
        assert_eq!(finish_image(&mut grid, b"~", 2048, 0), Some((6, 1)));
    }
}

#[test]
fn sixel_completions_reap_scrolled_images_below_capacity() {
    let cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 6,
    })));
    let mut grid = SixelGrid::new(cell_size, Rc::default());
    finish_image(&mut grid, b"~", 0, 0).unwrap();
    grid.remove_pixels_from_image(0, PixelRect::new(0, 0, 1, 1));
    grid.offset_grid_top();

    assert_eq!(finish_image(&mut grid, b"~", 8, 0), Some((6, 1)));
    assert_eq!(grid.sixel_image_store.borrow().image_count(), 1);
    assert!(grid.image_ids_to_reap.is_empty());
    assert!(grid.images_to_check_for_emptiness.is_empty());
}

#[test]
fn admission_reaps_empty_and_scrolled_images_without_waiting_for_render() {
    let cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 6,
    })));
    for (body, erase) in [(b"~", false), (b"~", true), (b"?", true)] {
        for limit_bytes in [false, true] {
            let mut grid = SixelGrid::new(cell_size.clone(), Rc::default());
            finish_image(&mut grid, body, 0, 0).unwrap();
            let store = grid.sixel_image_store.clone();
            let bytes = store.borrow().reserved_bytes;
            if limit_bytes {
                store.borrow_mut().limits.image_bytes = bytes;
            } else {
                store.borrow_mut().limits.images = 2;
            }
            if erase {
                grid.remove_pixels_from_image(0, PixelRect::new(0, 0, 6, 1));
            } else {
                grid.offset_grid_top();
            }
            assert_eq!(finish_image(&mut grid, b"~", 8, 0), Some((6, 1)));
            assert_eq!(store.borrow().image_count(), 1);
            assert_eq!(store.borrow().reserved_images, 2);
            assert_eq!(store.borrow().reserved_bytes, bytes);
        }
    }
}

#[test]
fn snapshot_and_copy_on_write_fit_at_capacity_until_both_references_are_released() {
    for live_first in [false, true] {
        let mut store = SixelImageStore::default();
        let bytes = image_storage_bytes(&stored_image()).unwrap() * 2;
        store.limits.image_bytes = bytes;
        store.limits.images = 2;
        let live = store.insert_image(stored_image(), &[]).unwrap();
        assert!(store.insert_image(stored_image(), &[]).is_none());
        let retained = store.retain_image(live).unwrap();
        assert!(store.retain_image(live).is_none());
        assert!(store.retain_image(retained).is_none());
        assert!(Rc::ptr_eq(
            &store.sixel_images[&live].image,
            &store.sixel_images[&retained].image
        ));
        let before = store.serialize_image(retained, 0, 0, 16, 12).unwrap();
        store.remove_pixels_from_image(live, PixelRect::new(0, 0, 6, 8));
        assert!(!Rc::ptr_eq(
            &store.sixel_images[&live].image,
            &store.sixel_images[&retained].image
        ));
        assert_ne!(store.serialize_image(live, 0, 0, 16, 12).unwrap(), before);
        assert_eq!(
            store.serialize_image(retained, 0, 0, 16, 12).unwrap(),
            before
        );
        let (first, last) = if live_first {
            (live, retained)
        } else {
            (retained, live)
        };
        store.remove_image(first);
        assert_eq!((store.reserved_bytes, store.reserved_images), (bytes, 2));
        assert!(store.insert_image(stored_image(), &[]).is_none());
        if !live_first {
            let refreshed = store.retain_image(live).unwrap();
            assert_eq!(
                store.serialize_image(refreshed, 0, 0, 16, 12),
                store.serialize_image(live, 0, 0, 16, 12)
            );
            store.remove_image(refreshed);
        }
        store.remove_image(last);
        assert_eq!((store.reserved_bytes, store.reserved_images), (0, 0));
        assert_eq!((store.cache_bytes, store.cache_entries), (0, 0));
        assert!(store.insert_image(stored_image(), &[]).is_some());
    }
}

#[test]
fn cache_byte_limit_preserves_hits_and_returns_uncached_output_at_capacity() {
    let image = stored_image();
    let output = image.serialize_range(0, 0, 8, 6);
    for limit in [output.len() - 1, output.len()] {
        let mut store = SixelImageStore::default();
        store.limits.cache_bytes = limit;
        let first = store.insert_image(stored_image(), &[]).unwrap();
        let second = store.insert_image(stored_image(), &[]).unwrap();
        for _ in 0..3 {
            assert_eq!(store.serialize_image(first, 0, 0, 8, 6).unwrap(), output);
        }
        let cached = usize::from(limit == output.len());
        assert_eq!(store.cache_entries, cached);
        assert_eq!(store.cache_bytes, cached * output.len());
        assert_eq!(store.serialize_image(second, 0, 0, 8, 6).unwrap(), output);
        assert!(store.sixel_images[&second].cache.is_empty());
        store.remove_pixels_from_image(first, PixelRect::new(0, 0, 6, 8));
        assert_eq!((store.cache_bytes, store.cache_entries), (0, 0));
        assert_eq!(store.serialize_image(second, 0, 0, 8, 6).unwrap(), output);
        assert_eq!(store.cache_entries, cached);
        store.remove_image(first);
        store.remove_image(second);
        assert_eq!((store.cache_bytes, store.cache_entries), (0, 0));
    }
}

#[test]
fn many_crop_rectangles_share_the_entry_limit_across_live_and_retained_images() {
    let image = stored_image();
    let mut store = SixelImageStore::default();
    store.limits.cache_entries = 10;
    let first = store.insert_image(stored_image(), &[]).unwrap();
    let second = store.insert_image(stored_image(), &[]).unwrap();
    let retained = store.retain_image(first).unwrap();
    for y in 0..12 {
        for x in 0..16 {
            for id in [first, second, retained] {
                let output = store.serialize_image(id, x, y, 16 - x, 12 - y).unwrap();
                assert_eq!(output, image.serialize_range(x, y, 16 - x, 12 - y));
                assert!(store.cache_entries <= 10);
                assert_eq!(
                    store.cache_entries,
                    store
                        .sixel_images
                        .values()
                        .map(|stored| stored.cache.len())
                        .sum::<usize>()
                );
                assert_eq!(
                    store.cache_bytes,
                    store
                        .sixel_images
                        .values()
                        .flat_map(|stored| stored.cache.values())
                        .map(|cached| cached.len())
                        .sum::<usize>()
                );
            }
        }
    }
    assert_eq!(store.cache_entries, 10);
    for mut copy in [store.clone(), store] {
        for id in [first, second, retained] {
            copy.remove_image(id);
        }
        assert_eq!((copy.cache_bytes, copy.cache_entries), (0, 0));
        assert_eq!((copy.reserved_bytes, copy.reserved_images), (0, 0));
    }
}
