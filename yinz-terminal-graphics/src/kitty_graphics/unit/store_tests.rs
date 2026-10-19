use super::*;

fn key(width: u16, height: u16) -> ScaledImageKey {
    ScaledImageKey {
        source: PixelRect::new(0, 0, 1, 1),
        size: (width as usize, height as usize),
    }
}

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
fn single_image_exceeding_quota_is_refused_with_enospc_and_store_unchanged() {
    let mut store = KittyImageStore::with_quota(1000);
    let id_a = store.store_image(rgba_image(10, 10)).unwrap();
    let result = store.store_image(rgba_image(20, 20));
    match result {
        Err(err) => assert_eq!(err.code, KittyErrorCode::Enospc),
        Ok(_) => panic!("expected error"),
    }
    assert_eq!(store.image_count(), 1);
    assert_eq!(store.total_bytes(), 400);
    assert!(store.get(id_a).is_some());
    let id_small = store.store_image(rgba_image(2, 2)).unwrap();
    assert_eq!(id_small, id_a + 1);
}

#[test]
fn quality_pass_placement_scaling_checks_size_before_allocating() {
    let mut store = KittyImageStore::with_quota(20);
    let id = store.store_image(rgba_image(1, 1)).unwrap();
    store.add_placement_ref(id);
    assert!(store.ensure_scaled_variant(id, key(2, 2)));
    for size in [(usize::MAX / 4, 1), (usize::MAX, 2)] {
        let oversized = ScaledImageKey { size, ..key(1, 1) };
        assert!(!store.retain_scaled_variants(id, &HashSet::from([oversized]), Some(oversized),));
        assert_eq!(store.total_bytes(), 20);
        assert_eq!(store.scaled_variant(id, key(2, 2)).unwrap(), &[0; 16]);
    }
}

#[test]
fn quality_pass_raster_reconstruction_checks_size_before_allocating() {
    let mut store = KittyImageStore::with_quota(20);
    let id = store.store_image(rgba_image(1, 1)).unwrap();
    for size in [(usize::MAX / 4, 1), (usize::MAX, 2)] {
        let oversized = ScaledImageKey { size, ..key(1, 1) };
        assert!(!store.ensure_scaled_variant(id, oversized));
        assert_eq!(store.total_bytes(), 4);
        assert!(store.get(id).unwrap().scaled_variants.is_empty());
    }
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
    assert!(store.add_scaled_variant(id_a, key(3, 2), vec![0; 120]));
    assert!(store.add_scaled_variant(id_a, key(5, 4), vec![0; 60]));
    assert!(store.add_scaled_variant(id_a, key(3, 2), vec![0; 100]));
    assert_eq!(store.total_bytes(), 640);
    assert_eq!(store.scaled_variant(id_a, key(3, 2)).unwrap().len(), 100);
    store.free(id_a);
    assert_eq!(store.total_bytes(), 80);
    store.free(id_b);
    assert_eq!(store.total_bytes(), 0);
}

#[test]
fn all_referenced_images_reject_new_image_without_exceeding_quota() {
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
    let error = store.store_image(rgba_image(10, 10)).unwrap_err();
    assert_eq!(error.code, KittyErrorCode::Enospc);
    assert!(store.get(id_b).is_some());
    assert!(store.get(id_c).is_some());
    assert_eq!(store.total_bytes(), 800);
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

#[test]
fn base64_cache_is_charged_and_dropped_before_image_eviction() {
    let mut store = KittyImageStore::with_quota(128);
    let id_a = store.store_image(rgba_image(4, 4)).unwrap();
    store.add_placement_ref(id_a);
    let encoded = store.zlib_base64_for(id_a, None).unwrap().unwrap();
    assert_eq!(store.total_bytes(), 64 + encoded.len());

    let id_b = store.store_image(rgba_image(4, 4)).unwrap();
    assert!(store.get(id_a).is_some());
    assert!(store.get(id_b).is_some());
    assert_eq!(store.total_bytes(), 128);
}

#[test]
fn base64_is_returned_uncached_when_only_live_pixels_fit() {
    let mut store = KittyImageStore::with_quota(64);
    let id = store.store_image(rgba_image(4, 4)).unwrap();
    let first = store.zlib_base64_for(id, None).unwrap().unwrap();
    let second = store.zlib_base64_for(id, None).unwrap().unwrap();
    assert_eq!(first, second);
    assert_eq!(store.total_bytes(), 64);
}

#[test]
fn shared_base64_handles_survive_variant_frame_and_image_replacement() {
    fn decode(encoded: &str) -> Vec<u8> {
        use std::io::Read;
        let bytes = base64_simd::STANDARD.decode_to_vec(encoded).unwrap();
        let mut pixels = Vec::new();
        flate2::read::ZlibDecoder::new(bytes.as_slice())
            .read_to_end(&mut pixels)
            .unwrap();
        pixels
    }

    let mut store = KittyImageStore::default();
    let id = store.store_image(rgba_image(1, 1)).unwrap();
    let root = store.zlib_base64_for(id, None).unwrap().unwrap();
    assert!(Rc::ptr_eq(
        &root,
        &store.zlib_base64_for(id, None).unwrap().unwrap()
    ));

    assert!(store.add_scaled_variant(id, key(1, 1), vec![1; 4]));
    let scaled = store.zlib_base64_for(id, Some(key(1, 1))).unwrap().unwrap();
    assert!(store.add_scaled_variant(id, key(1, 1), vec![2; 4]));
    let replacement = store.zlib_base64_for(id, Some(key(1, 1))).unwrap().unwrap();
    assert_eq!(decode(&scaled), [1; 4]);
    assert_eq!(decode(&replacement), [2; 4]);

    store.write_frame(id, None, vec![3; 4], 40).unwrap();
    store.set_current_frame(id, 2).unwrap();
    assert!(store
        .zlib_base64_for(id, Some(key(1, 1)))
        .unwrap()
        .is_none());
    assert_eq!(store.total_bytes(), 8);
    let frame = store.zlib_base64_for(id, None).unwrap().unwrap();
    assert_eq!(decode(&frame), [3; 4]);
    store.write_frame(id, Some(2), vec![4; 4], 40).unwrap();
    let edited = store.zlib_base64_for(id, None).unwrap().unwrap();
    assert_eq!(decode(&edited), [4; 4]);

    let next_id = store.replace_image(id, rgba_image(1, 1)).unwrap();
    assert!(store.zlib_base64_for(id, None).unwrap().is_none());
    assert_eq!(store.total_bytes(), 4);
    assert_eq!(
        *store.zlib_base64_for(next_id, None).unwrap().unwrap(),
        *root
    );
    // A render holding an old encoding keeps those pixels after cache invalidation or deletion.
    assert_eq!(decode(&root), [0; 4]);
    assert_eq!(decode(&scaled), [1; 4]);
    assert_eq!(decode(&replacement), [2; 4]);
    assert_eq!(decode(&frame), [3; 4]);
    assert_eq!(decode(&edited), [4; 4]);
}

#[test]
fn scaled_variant_overflow_does_not_replace_the_previous_variant() {
    let mut store = KittyImageStore::with_quota(120);
    let id = store.store_image(rgba_image(4, 4)).unwrap();
    assert!(store.add_scaled_variant(id, key(2, 2), vec![1; 40]));

    assert!(!store.add_scaled_variant(id, key(2, 2), vec![2; 80]));
    assert_eq!(store.scaled_variant(id, key(2, 2)), Some(&[1; 40][..]));
    assert_eq!(store.total_bytes(), 104);
}

#[test]
fn animation_frame_overflow_is_atomic() {
    let mut store = KittyImageStore::with_quota(100);
    let id = store.store_image(rgba_image(4, 4)).unwrap();

    let error = store.write_frame(id, None, vec![1; 64], 40).unwrap_err();
    assert_eq!(error.code, KittyErrorCode::Enospc);
    assert_eq!(store.get(id).unwrap().frame_count(), 1);
    assert_eq!(store.total_bytes(), 64);
}

#[test]
fn deleting_frames_debits_frames_variants_and_base64() {
    let mut store = KittyImageStore::default();
    let id = store.store_image(rgba_image(4, 4)).unwrap();
    assert_eq!(
        store.write_frame(id, None, vec![1; 64], 40).unwrap(),
        Some(2)
    );
    store.set_current_frame(id, 2).unwrap();
    assert!(store.add_scaled_variant(id, key(2, 2), vec![2; 20]));
    let _ = store.zlib_base64_for(id, None).unwrap().unwrap();
    assert!(store.total_bytes() > 128);

    store.remove_frames(id, None).unwrap();
    assert_eq!(store.get(id).unwrap().frame_count(), 1);
    assert!(store.get(id).unwrap().scaled_variants.is_empty());
    assert_eq!(store.total_bytes(), 64);
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
        self.store
            .add_scaled_variant(id, key(cells.0, cells.1), bytes);
    }

    #[rule]
    fn retain_scaled_variants(&mut self, tc: hegel::TestCase) {
        let id = self.any_id(&tc);
        let mut live: HashSet<_> = self
            .store
            .get(id)
            .map(|image| {
                image
                    .scaled_variants
                    .keys()
                    .copied()
                    .filter(|_| tc.draw(generators::booleans()))
                    .collect()
            })
            .unwrap_or_default();
        // This model also stores empty images and partial frames, which cannot be placed.
        let placeable = self.store.get(id).is_some_and(|image| {
            image.width > 0 && image.height > 0 && image.displayed_rgba().len() >= 4
        });
        let required = (tc.draw(generators::booleans()) && placeable).then(|| {
            key(
                tc.draw(generators::integers::<u16>().min_value(1).max_value(4)),
                tc.draw(generators::integers::<u16>().min_value(1).max_value(4)),
            )
        });
        live.extend(required);
        self.store.retain_scaled_variants(id, &live, required);
    }

    #[rule]
    fn zlib_base64_for(&mut self, tc: hegel::TestCase) {
        let id = self.any_id(&tc);
        let variant = if tc.draw(generators::booleans()) {
            Some(key(
                tc.draw(generators::integers::<u16>().max_value(4)),
                tc.draw(generators::integers::<u16>().max_value(4)),
            ))
        } else {
            None
        };
        let _ = self.store.zlib_base64_for(id, variant);
    }

    #[rule]
    fn write_frame(&mut self, tc: hegel::TestCase) {
        let id = self.any_id(&tc);
        let bytes = tc.draw(generators::binary().max_size(64));
        let edit = tc
            .draw(generators::booleans())
            .then(|| tc.draw(generators::integers::<u32>().max_value(4)));
        let _ = self.store.write_frame(id, edit, bytes, 40);
    }

    #[rule]
    fn remove_frames(&mut self, tc: hegel::TestCase) {
        let id = self.any_id(&tc);
        let _ = self.store.remove_frames(id, None);
    }

    #[rule]
    fn set_current_frame(&mut self, tc: hegel::TestCase) {
        let id = self.any_id(&tc);
        let frame = tc.draw(generators::integers::<u32>().max_value(4));
        let _ = self.store.set_current_frame(id, frame);
    }

    #[rule]
    fn protect_completed_frame(&mut self, tc: hegel::TestCase) {
        let id = self.any_id(&tc);
        self.store
            .protect_completed_frame_image(id, tc.draw(generators::booleans()));
    }

    #[rule]
    fn release_completed_frame(&mut self, tc: hegel::TestCase) {
        let id = self.any_id(&tc);
        self.store.release_completed_frame_image(id);
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
        let expected: usize = self
            .store
            .images
            .values()
            .map(KittyImageStore::stored_image_bytes)
            .sum();
        assert_eq!(
            self.store.total_bytes(),
            expected,
            "total_bytes() is {} but the live images {:?} hold {} bytes",
            self.store.total_bytes(),
            live,
            expected
        );
        assert!(
            self.store.total_bytes() <= self.store.quota_bytes,
            "store retained {} bytes under a {} byte quota",
            self.store.total_bytes(),
            self.store.quota_bytes
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

#[test]
fn replacement_credits_obsolete_rasters_and_drops_their_compressed_caches() {
    let mut store = KittyImageStore::with_quota(256);
    let id = store.store_image(rgba_image(1, 1)).unwrap();
    store.add_placement_ref(id);
    assert!(store.add_scaled_variant(id, key(4, 4), vec![1; 64]));
    let retained = store.zlib_base64_for(id, Some(key(4, 4))).unwrap().unwrap();
    assert!(store.total_bytes() > 68);
    let replace = |key| HashMap::from([(id, HashSet::from([key]))]);
    assert!(store.replace_scaled_variants(replace(key(7, 7))));
    assert_eq!(store.total_bytes(), 200);
    assert!(store.scaled_variant(id, key(4, 4)).is_none());
    assert!(store
        .zlib_base64_for(id, Some(key(4, 4)))
        .unwrap()
        .is_none());
    assert!(!retained.is_empty());
    assert!(!store.replace_scaled_variants(replace(key(8, 8))));
    assert_eq!(store.total_bytes(), 200);
    assert_eq!(store.scaled_variant(id, key(7, 7)).unwrap(), &[0; 196]);
    store.write_frame(id, None, vec![3; 4], 40).unwrap();
    store.set_current_frame(id, 2).unwrap();
    assert_eq!(store.total_bytes(), 8);
    assert!(store.get(id).unwrap().scaled_variants.is_empty());
    assert_eq!(store.get(id).unwrap().displayed_rgba(), &[3; 4]);
}

#[test]
fn pruning_variants_preserves_surviving_caches_and_retained_encodings() {
    use std::io::Read;
    let mut store = KittyImageStore::default();
    let id = store.store_image(rgba_image(1, 1)).unwrap();
    assert!(store.add_scaled_variant(id, key(2, 2), vec![1; 16]));
    assert!(store.add_scaled_variant(id, key(3, 3), vec![2; 36]));
    let root = store.zlib_base64_for(id, None).unwrap().unwrap();
    let surviving = store.zlib_base64_for(id, Some(key(2, 2))).unwrap().unwrap();
    let obsolete = store.zlib_base64_for(id, Some(key(3, 3))).unwrap().unwrap();
    assert!(store.retain_scaled_variants(id, &HashSet::from([key(2, 2)]), None));
    assert_eq!(store.total_bytes(), 20 + root.len() + surviving.len());
    assert!(Rc::ptr_eq(
        &root,
        &store.zlib_base64_for(id, None).unwrap().unwrap()
    ));
    assert!(Rc::ptr_eq(
        &surviving,
        &store.zlib_base64_for(id, Some(key(2, 2))).unwrap().unwrap()
    ));
    assert!(store
        .zlib_base64_for(id, Some(key(3, 3)))
        .unwrap()
        .is_none());
    let compressed = base64_simd::STANDARD.decode_to_vec(&*obsolete).unwrap();
    let mut pixels = Vec::new();
    flate2::read::ZlibDecoder::new(compressed.as_slice())
        .read_to_end(&mut pixels)
        .unwrap();
    assert_eq!(pixels, vec![2; 36]);
    assert!(store.retain_scaled_variants(id, &HashSet::new(), None));
    assert_eq!(store.total_bytes(), 4 + root.len());
}

#[test]
fn placement_transaction_credits_obsolete_cache_and_preserves_caches_on_failure() {
    let mut store = KittyImageStore::with_quota(256);
    let id = store.store_image(rgba_image(1, 1)).unwrap();
    store.add_placement_ref(id);
    assert!(store.ensure_scaled_variant(id, key(2, 2)));
    let root = store.zlib_base64_for(id, None).unwrap().unwrap();
    store.zlib_base64_for(id, Some(key(2, 2))).unwrap();
    assert!(store.retain_scaled_variants(id, &HashSet::from([key(7, 7)]), Some(key(7, 7))));
    assert_eq!(store.total_bytes(), 200 + root.len());
    assert!(Rc::ptr_eq(
        &root,
        &store.zlib_base64_for(id, None).unwrap().unwrap()
    ));
    let cached = store.zlib_base64_for(id, Some(key(7, 7))).unwrap().unwrap();
    let total = store.total_bytes();
    assert!(!store.retain_scaled_variants(id, &HashSet::from([key(8, 8)]), Some(key(8, 8))));
    assert_eq!(store.total_bytes(), total);
    assert_eq!(store.get(id).unwrap().scaled_variants.len(), 1);
    assert!(Rc::ptr_eq(
        &cached,
        &store.zlib_base64_for(id, Some(key(7, 7))).unwrap().unwrap()
    ));
}

#[test]
fn placement_transaction_does_not_credit_caches_twice_when_making_room() {
    let mut store = KittyImageStore::with_quota(256);
    let id = store.store_image(rgba_image(1, 1)).unwrap();
    store.add_placement_ref(id);
    assert!(store.ensure_scaled_variant(id, key(2, 2)));
    store.zlib_base64_for(id, None).unwrap();
    store.zlib_base64_for(id, Some(key(2, 2))).unwrap();
    let placed = store.store_image(rgba_image(5, 5)).unwrap();
    store.add_placement_ref(placed);
    let unplaced = store.store_image(rgba_image(5, 5)).unwrap();
    assert!(store.total_bytes() > 220);
    assert!(store.retain_scaled_variants(id, &HashSet::from([key(2, 7)]), Some(key(2, 7))));
    assert!(store.get(unplaced).is_none());
    assert!(store.get(placed).is_some());
    assert_eq!(store.total_bytes(), 160);
}

#[test]
fn frame_writes_invalidate_only_successful_edits_of_the_displayed_frame() {
    let mut store = KittyImageStore::with_quota(100);
    let id = store.store_image(rgba_image(1, 1)).unwrap();
    store.write_frame(id, None, vec![1; 4], 40).unwrap();
    for displayed in [1, 2] {
        store.set_current_frame(id, displayed).unwrap();
        assert!(store.ensure_scaled_variant(id, key(2, 2)));
        let original = store.scaled_variant(id, key(2, 2)).unwrap().to_vec();
        store
            .write_frame(id, Some(3 - displayed), vec![2; 4], 40)
            .unwrap();
        assert_eq!(store.scaled_variant(id, key(2, 2)).unwrap(), original);
        assert_eq!(store.total_bytes(), 24);
        assert_eq!(
            store.write_frame(id, Some(3), vec![3; 4], 40).unwrap(),
            None
        );
        assert!(store
            .write_frame(id, Some(displayed), vec![3; 101], 40)
            .is_err());
        assert_eq!(store.scaled_variant(id, key(2, 2)).unwrap(), original);
        assert_eq!(store.total_bytes(), 24);
        store
            .write_frame(id, Some(displayed), vec![4; 4], 40)
            .unwrap();
        assert!(store.scaled_variant(id, key(2, 2)).is_none());
        assert_eq!(store.total_bytes(), 8);
        assert!(store.ensure_scaled_variant(id, key(2, 2)));
        assert_eq!(store.scaled_variant(id, key(2, 2)).unwrap(), &[4; 16]);
        assert_eq!(store.total_bytes(), 24);
    }
}

#[test]
fn batch_resize_checks_deduplicated_bytes_and_overflow_before_generating_pixels() {
    let mut store = KittyImageStore::with_quota(20);
    let id = store.store_image(rgba_image(1, 1)).unwrap();
    store.add_placement_ref(id);
    let keys = HashSet::from([key(2, 2), key(2, 2)]);
    assert!(store.replace_scaled_variants(HashMap::from([(id, keys)])));
    assert_eq!(store.total_bytes(), 20);
    // These sizes would overflow Vec's capacity limit or the byte count if rasterized.
    for size in [
        (usize::MAX / 4, 1),
        (usize::MAX, 2),
        (usize::MAX / 4 + 1, 1),
    ] {
        let oversized = ScaledImageKey { size, ..key(1, 1) };
        assert!(!store.replace_scaled_variants(HashMap::from([(id, HashSet::from([oversized]))])));
        assert_eq!(store.total_bytes(), 20);
        assert_eq!(store.scaled_variant(id, key(2, 2)).unwrap(), &[0; 16]);
    }
    // Each raster fits usize, but their combined byte count does not.
    let keys = [(usize::MAX / 4, 1), (usize::MAX / 4 - 1, 1)]
        .map(|size| ScaledImageKey { size, ..key(1, 1) });
    assert!(!store.replace_scaled_variants(HashMap::from([(id, HashSet::from(keys))])));
    assert_eq!(store.total_bytes(), 20);
}

#[test]
fn revisions_follow_displayed_pixels_and_not_cache_reconstruction() {
    let mut store = KittyImageStore::with_quota(100);
    let id = store.store_image(rgba_image(1, 1)).unwrap();
    store.write_frame(id, None, vec![1; 4], 40).unwrap();
    store.write_frame(id, None, vec![2; 4], 40).unwrap();
    assert_eq!(store.raster_revision(id), Some(0));
    store.set_current_frame(id, 2).unwrap().unwrap();
    assert_eq!(store.raster_revision(id), Some(1));
    assert!(store.ensure_scaled_variant(id, key(2, 2)));
    store.zlib_base64_for(id, None).unwrap();
    store.write_frame(id, Some(1), vec![3; 4], 40).unwrap();
    store.write_frame(id, Some(2), vec![1; 4], 80).unwrap();
    assert!(store.write_frame(id, Some(2), vec![4; 101], 40).is_err());
    assert_eq!(store.set_current_frame(id, 4).unwrap(), None);
    assert_eq!(store.remove_frames(id, Some(1)).unwrap(), None);
    assert_eq!(store.raster_revision(id), Some(1));
    // Deleting frame 2 shifts frame 3 into the displayed slot without changing its index.
    store.remove_frames(id, Some(2)).unwrap().unwrap();
    assert_eq!(store.get(id).unwrap().displayed_rgba(), &[2; 4]);
    assert_eq!(store.raster_revision(id), Some(2));
    assert!(store.scaled_variant(id, key(2, 2)).is_none());
    assert!(store.ensure_scaled_variant(id, key(2, 2)));
    assert_eq!(store.raster_revision(id), Some(2));
    store.remove_frames(id, None).unwrap().unwrap();
    assert_eq!(store.raster_revision(id), Some(3));
    assert_eq!(store.get(id).unwrap().displayed_rgba(), &[3; 4]);
    store.remove_frames(id, None).unwrap().unwrap();
    assert_eq!(store.raster_revision(id), Some(3));
}

#[test]
fn completed_pixels_survive_edits_selection_deletion_and_repeated_locks() {
    for unscaled in [false, true] {
        let mut store = KittyImageStore::with_quota(100);
        let id = store.store_image(rgba_image(1, 1)).unwrap();
        store.write_frame(id, None, vec![1; 4], 40).unwrap();
        assert!(store.ensure_scaled_variant(id, key(2, 2)));
        store.protect_completed_frame_image(id, unscaled);
        let original = store.zlib_base64_for(id, Some(key(2, 2))).unwrap().unwrap();
        let original_root = store.zlib_base64_for(id, None).unwrap().unwrap();
        for value in [2, 3] {
            store.write_frame(id, Some(1), vec![value; 4], 40).unwrap();
            store.protect_completed_frame_image(id, unscaled);
            assert_eq!(store.raster_revision(id), Some(0));
            assert_eq!(store.scaled_variant(id, key(2, 2)).unwrap(), &[0; 16]);
            assert_eq!(store.get(id).unwrap().displayed_rgba(), &[value; 4]);
        }
        store.set_current_frame(id, 2).unwrap().unwrap();
        store.remove_frames(id, Some(2)).unwrap().unwrap();
        assert!(store.write_frame(id, Some(1), vec![9; 101], 40).is_err());
        // Quota pressure may discard encodings; locked repaint must rebuild the original pixels.
        store.remove_all_base64_caches(id);
        assert_eq!(
            *store.zlib_base64_for(id, Some(key(2, 2))).unwrap().unwrap(),
            *original
        );
        if unscaled {
            assert_eq!(
                *store.zlib_base64_for(id, None).unwrap().unwrap(),
                *original_root
            );
        }
        store.remove_all_base64_caches(id);
        assert_eq!(store.total_bytes(), 20 + if unscaled { 4 } else { 0 });
        store.release_completed_frame_image(id);
        assert_eq!(store.total_bytes(), 4);
        assert_eq!(store.raster_revision(id), Some(1));
        assert!(store.ensure_scaled_variant(id, key(2, 2)));
        assert_eq!(store.scaled_variant(id, key(2, 2)).unwrap(), &[3; 16]);
        store.release_completed_frame_image(id);
        assert_eq!(store.raster_revision(id), Some(1));
    }
}

#[test]
fn unscaled_snapshot_reservation_is_atomic_and_deletion_credits_removed_frames() {
    let mut store = KittyImageStore::with_quota(8);
    let id = store.store_image(rgba_image(1, 1)).unwrap();
    store.write_frame(id, None, vec![1; 4], 40).unwrap();
    store.protect_completed_frame_image(id, true);
    assert_eq!(
        store
            .write_frame(id, Some(1), vec![2; 4], 40)
            .unwrap_err()
            .code,
        KittyErrorCode::Enospc
    );
    assert_eq!(
        store.set_current_frame(id, 2).unwrap_err().code,
        KittyErrorCode::Enospc
    );
    assert_eq!(store.total_bytes(), 8);
    assert_eq!(store.raster_revision(id), Some(0));
    assert_eq!(store.get(id).unwrap().displayed_rgba(), &[0; 4]);
    assert!(store.images[&id].completed_rgba.is_none());
    store.release_completed_frame_image(id);
    assert_eq!(store.raster_revision(id), Some(0));
    store.set_current_frame(id, 2).unwrap().unwrap();
    store.protect_completed_frame_image(id, true);
    store.remove_frames(id, None).unwrap().unwrap();
    assert_eq!(store.total_bytes(), 8);
    assert_eq!(
        store.images[&id].completed_rgba.as_deref(),
        Some([1; 4].as_slice())
    );
    assert_eq!(store.get(id).unwrap().displayed_rgba(), &[0; 4]);
    store.release_completed_frame_image(id);
    assert_eq!(store.total_bytes(), 4);
    assert_eq!(store.raster_revision(id), Some(2));
}
