use super::*;

fn rgba_image(width: u32, height: u32) -> DecodedImage {
    DecodedImage {
        bytes: vec![0; (width * height * 4) as usize],
        width,
        height,
        format: KittyFormat::Rgba32,
    }
}

#[test]
fn anonymous_images_receive_distinct_monotonically_increasing_internal_ids() {
    let mut store = KittyImageStore::default();
    let id_a = store.store_image(rgba_image(2, 2)).unwrap();
    let id_b = store.store_image(rgba_image(2, 2)).unwrap();
    assert!(id_b > id_a);
    assert_eq!(id_b, id_a + 1);
    assert_eq!(store.image_count(), 2);
    assert_eq!(store.total_bytes(), 32);
}

#[test]
fn lru_zero_refcount_image_is_evicted_first_on_quota_overflow() {
    let mut store = KittyImageStore::with_quota(1000);
    let id_a = store.store_image(rgba_image(10, 10)).unwrap();
    let id_b = store.store_image(rgba_image(10, 10)).unwrap();
    let id_c = store.store_image(rgba_image(10, 10)).unwrap();
    assert!(store.get(id_a).is_none());
    assert!(store.get(id_b).is_some());
    assert!(store.get(id_c).is_some());
    assert_eq!(store.image_count(), 2);
    assert_eq!(store.total_bytes(), 800);
}

#[test]
fn touch_updates_lru_order_for_eviction() {
    let mut store = KittyImageStore::with_quota(1000);
    let id_a = store.store_image(rgba_image(10, 10)).unwrap();
    let id_b = store.store_image(rgba_image(10, 10)).unwrap();
    store.touch(id_a);
    let id_c = store.store_image(rgba_image(10, 10)).unwrap();
    assert!(store.get(id_a).is_some());
    assert!(store.get(id_b).is_none());
    assert!(store.get(id_c).is_some());
    assert_eq!(store.image_count(), 2);
    assert_eq!(store.total_bytes(), 800);
}

#[test]
fn refcounted_image_is_never_evicted_even_when_oldest() {
    let mut store = KittyImageStore::with_quota(1000);
    let id_a = store.store_image(rgba_image(10, 10)).unwrap();
    store.add_placement_ref(id_a);
    let id_b = store.store_image(rgba_image(10, 10)).unwrap();
    let id_c = store.store_image(rgba_image(10, 10)).unwrap();
    assert!(store.get(id_a).is_some());
    assert!(store.get(id_b).is_none());
    assert!(store.get(id_c).is_some());
    assert_eq!(store.total_bytes(), 800);
}

#[test]
fn single_image_exceeding_quota_is_refused_with_einval_and_store_unchanged() {
    let mut store = KittyImageStore::with_quota(1000);
    let id_a = store.store_image(rgba_image(10, 10)).unwrap();
    let result = store.store_image(rgba_image(20, 20));
    match result {
        Err(err) => assert_eq!(err.code, KittyErrorCode::Einval),
        Ok(_) => panic!("expected error"),
    }
    assert_eq!(store.image_count(), 1);
    assert_eq!(store.total_bytes(), 400);
    assert!(store.get(id_a).is_some());
    let id_small = store.store_image(rgba_image(2, 2)).unwrap();
    assert_eq!(id_small, id_a + 1);
}

#[test]
fn free_with_positive_refcount_retains_data_and_removes_at_zero() {
    let mut store = KittyImageStore::default();
    let id_a = store.store_image(rgba_image(10, 10)).unwrap();
    store.add_placement_ref(id_a);
    store.add_placement_ref(id_a);
    assert_eq!(store.refcount(id_a), Some(2));
    store.free(id_a);
    assert_eq!(store.refcount(id_a), Some(1));
    assert!(store.get(id_a).is_some());
    assert_eq!(store.total_bytes(), 400);
    store.free(id_a);
    assert!(store.get(id_a).is_none());
    assert_eq!(store.image_count(), 0);
    assert_eq!(store.total_bytes(), 0);
}

#[test]
fn byte_accounting_matches_hand_computed_total_with_scaled_variants() {
    let mut store = KittyImageStore::default();
    let id_a = store.store_image(rgba_image(10, 10)).unwrap();
    let id_b = store.store_image(rgba_image(4, 5)).unwrap();
    store.add_scaled_variant(id_a, (3, 2), vec![0; 120]);
    store.add_scaled_variant(id_a, (5, 4), vec![0; 60]);
    store.add_scaled_variant(id_a, (3, 2), vec![0; 100]);
    assert_eq!(store.total_bytes(), 640);
    assert_eq!(store.scaled_variant(id_a, (3, 2)).unwrap().len(), 100);
    store.free(id_a);
    assert_eq!(store.total_bytes(), 80);
    store.free(id_b);
    assert_eq!(store.total_bytes(), 0);
}

#[test]
fn remove_placement_ref_keeps_data_but_makes_image_evictable() {
    let mut store = KittyImageStore::with_quota(1000);
    let id_a = store.store_image(rgba_image(10, 10)).unwrap();
    store.add_placement_ref(id_a);
    store.remove_placement_ref(id_a);
    assert!(store.get(id_a).is_some());
    assert_eq!(store.refcount(id_a), Some(0));
    let id_b = store.store_image(rgba_image(10, 10)).unwrap();
    store.add_placement_ref(id_b);
    let id_c = store.store_image(rgba_image(10, 10)).unwrap();
    assert!(store.get(id_a).is_none());
    assert!(store.get(id_b).is_some());
    assert!(store.get(id_c).is_some());
    store.add_placement_ref(id_c);
    let id_d = store.store_image(rgba_image(10, 10)).unwrap();
    assert!(store.get(id_d).is_some());
    assert_eq!(store.total_bytes(), 1200);
}

#[test]
fn rgb24_images_are_normalized_to_rgba_at_store_time() {
    let mut store = KittyImageStore::default();
    let id = store
        .store_image(DecodedImage {
            bytes: (1..=12).collect(),
            width: 2,
            height: 2,
            format: KittyFormat::Rgb24,
        })
        .unwrap();
    let image = store.get(id).unwrap();
    assert_eq!(
        image.rgba,
        vec![1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]
    );
    assert_eq!(store.total_bytes(), 16);
}

// ---- property-based tests (hegel) ----

use hegel::generators;

#[hegel::composite]
fn any_decoded_image(tc: &hegel::TestCase) -> DecodedImage {
    let format = tc.draw(generators::sampled_from(vec![
        KittyFormat::Rgba32,
        KittyFormat::Rgb24,
        // Png here means "already decoded", `normalize_to_rgba` passes the bytes through
        KittyFormat::Png,
    ]));
    // capped to keep the run fast; 0-length is generated
    let bytes = tc.draw(generators::binary().max_size(64));
    DecodedImage {
        bytes,
        width: tc.draw(generators::integers::<u32>().max_value(8)),
        height: tc.draw(generators::integers::<u32>().max_value(8)),
        format,
    }
}

struct KittyImageStoreModel {
    store: KittyImageStore,
    /// every id `store_image` ever handed out, in order; ids are never reused
    known_ids: Vec<InternalImageId>,
}

impl KittyImageStoreModel {
    /// Draws from the ids the store actually issued *and* from junk ids that were never valid
    /// (or have since been evicted), so every entry point is exercised with both.
    fn any_id(&self, tc: &hegel::TestCase) -> InternalImageId {
        if !self.known_ids.is_empty() && tc.draw(generators::booleans()) {
            let index =
                tc.draw(generators::integers::<usize>().max_value(self.known_ids.len() - 1));
            self.known_ids[index]
        } else {
            tc.draw(generators::integers::<InternalImageId>().max_value(64))
        }
    }
}

#[hegel::state_machine]
impl KittyImageStoreModel {
    #[rule]
    fn store_image(&mut self, tc: hegel::TestCase) {
        let image = tc.draw(any_decoded_image());
        if let Ok(id) = self.store.store_image(image) {
            self.known_ids.push(id);
        }
    }

    #[rule]
    fn touch(&mut self, tc: hegel::TestCase) {
        let id = self.any_id(&tc);
        self.store.touch(id);
    }

    #[rule]
    fn add_placement_ref(&mut self, tc: hegel::TestCase) {
        let id = self.any_id(&tc);
        self.store.add_placement_ref(id);
    }

    #[rule]
    fn remove_placement_ref(&mut self, tc: hegel::TestCase) {
        let id = self.any_id(&tc);
        self.store.remove_placement_ref(id);
    }

    #[rule]
    fn free(&mut self, tc: hegel::TestCase) {
        let id = self.any_id(&tc);
        self.store.free(id);
    }

    #[rule]
    fn add_scaled_variant(&mut self, tc: hegel::TestCase) {
        let id = self.any_id(&tc);
        let cells = (
            tc.draw(generators::integers::<u16>().max_value(4)),
            tc.draw(generators::integers::<u16>().max_value(4)),
        );
        let bytes = tc.draw(generators::binary().max_size(64));
        self.store.add_scaled_variant(id, cells, bytes);
    }

    #[rule]
    fn base64_for(&mut self, tc: hegel::TestCase) {
        let id = self.any_id(&tc);
        let variant = if tc.draw(generators::booleans()) {
            Some((
                tc.draw(generators::integers::<u16>().max_value(4)),
                tc.draw(generators::integers::<u16>().max_value(4)),
            ))
        } else {
            None
        };
        let _ = self.store.base64_for(id, variant);
    }

    #[invariant]
    fn accounting_matches_the_live_images(&self, _: hegel::TestCase) {
        let live: Vec<InternalImageId> = self
            .known_ids
            .iter()
            .copied()
            .filter(|id| self.store.get(*id).is_some())
            .collect();
        assert_eq!(
            self.store.image_count(),
            live.len(),
            "image_count() is {} but {} of the issued ids {:?} are still reachable",
            self.store.image_count(),
            live.len(),
            self.known_ids
        );
        let expected: usize = live
            .iter()
            .map(|id| {
                let image = self.store.get(*id).unwrap();
                image.rgba.len()
                    + image
                        .scaled_variants
                        .values()
                        .map(|bytes| bytes.len())
                        .sum::<usize>()
            })
            .sum();
        assert_eq!(
            self.store.total_bytes(),
            expected,
            "total_bytes() is {} but the live images {:?} hold {} bytes",
            self.store.total_bytes(),
            live,
            expected
        );
    }
}

#[hegel::test]
fn store_accounting_survives_any_sequence_of_operations(tc: hegel::TestCase) {
    // tiny quotas make eviction (and the refcount/protected corner cases around it) the common
    // case rather than something that only happens at 320MiB
    let quota = tc.draw(hegel::one_of!(
        generators::just(0usize),
        generators::just(1usize),
        generators::integers::<usize>().max_value(256),
        generators::just(DEFAULT_KITTY_STORE_QUOTA_BYTES),
    ));
    let machine = KittyImageStoreModel {
        store: KittyImageStore::with_quota(quota),
        known_ids: vec![],
    };
    hegel::stateful::run(machine, tc);
}
