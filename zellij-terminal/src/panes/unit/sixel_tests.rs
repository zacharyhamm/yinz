use super::*;

#[test]
fn cutting_pixels_invalidates_only_overlapping_cached_rectangles() {
    let image = SixelImage::new(b"\x1bPq\"1;1;16;12#1;2;100;0;0#1!16~-!16~\x1b\\").unwrap();
    let mut grid = SixelGrid::default();
    grid.new_sixel_image(0, image);
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
