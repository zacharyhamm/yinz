use super::parser::{DecodedImage, KittyError, KittyErrorCode, KittyFormat};
use crate::sixel::PixelRect;
use flate2::{write::ZlibEncoder, Compression};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::rc::Rc;

pub const DEFAULT_KITTY_STORE_QUOTA_BYTES: usize = 335_544_320;

pub type InternalImageId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScaledImageKey {
    pub source: PixelRect,
    pub size: (usize, usize),
}

impl ScaledImageKey {
    fn byte_len(self) -> Option<usize> {
        self.size
            .0
            .checked_mul(self.size.1)?
            .checked_mul(4)
            .filter(|bytes| *bytes <= isize::MAX as usize)
    }
}

/// One frame of an image's animation, beyond the root frame.
///
/// Spec, "Animation": frames are numbered from 1 and "the first frame is the base image", so this
/// is frame 2 and up. Every frame covers the whole image, the same size as the root, however small
/// the rectangle the client sent to build it.
#[derive(Debug, Clone, PartialEq)]
pub struct KittyFrame {
    /// Milliseconds to wait before moving to the next frame. Negative marks a gapless frame: base
    /// data for later frames that is never displayed on its own.
    pub gap_ms: i32,
    pub rgba: Vec<u8>,
}

/// Spec, "Animation": the default gap for a frame a client adds without saying otherwise. The root
/// frame's default is zero instead.
pub const DEFAULT_FRAME_GAP_MS: i32 = 40;

#[derive(Debug, Clone, PartialEq)]
pub struct KittyImage {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub scaled_variants: HashMap<ScaledImageKey, Vec<u8>>,
    /// Frames 2..=n. Frame 1 is [`Self::rgba`], the root image, so this is empty for the still
    /// image that every transmission starts as.
    pub frames: Vec<KittyFrame>,
    /// The root frame's own gap. Unlike the frames a client adds, it defaults to zero.
    pub root_frame_gap_ms: i32,
    /// Which frame is displayed, 1-based, so 1 is the root image. Nothing advances this on its own
    /// — this terminal has no playback clock, so it moves only when a client sends `a=a,c=<n>`.
    pub current_frame: usize,
    /// `a=a,s=`: 1 stopped, 2 running while waiting for more frames, 3 running normally. Recorded
    /// so a client can read its own state back, and inert without a clock.
    pub animation_state: u32,
    /// `a=a,v=`: how many times the animation should loop.
    pub animation_loop_count: u32,
}

impl KittyImage {
    /// The pixels of frame `frame_number`, 1-based, or `None` if there is no such frame.
    pub fn frame_rgba(&self, frame_number: u32) -> Option<&[u8]> {
        match frame_number {
            0 => None,
            1 => Some(&self.rgba),
            n => self
                .frames
                .get(n as usize - 2)
                .map(|frame| frame.rgba.as_slice()),
        }
    }
    /// How many frames the image has, counting the root.
    pub fn frame_count(&self) -> u32 {
        self.frames.len() as u32 + 1
    }
    /// The pixels currently displayed, which is the root image unless a client has moved the
    /// animation on with `a=a,c=<n>`.
    pub fn displayed_rgba(&self) -> &[u8] {
        self.frame_rgba(self.current_frame as u32)
            .unwrap_or(&self.rgba)
    }
    fn frame_bytes(&self) -> usize {
        self.frames.iter().map(|frame| frame.rgba.len()).sum()
    }
}

#[derive(Debug, Clone)]
struct StoredImage {
    image: KittyImage,
    refcount: usize,
    lru_stamp: u64,
    base64_cache: HashMap<Option<ScaledImageKey>, Rc<String>>,
    raster_revision: u64,
    pending_invalidation: bool,
    protect_unscaled: bool,
    completed_rgba: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct KittyImageStore {
    images: HashMap<InternalImageId, StoredImage>,
    next_image_id: InternalImageId,
    next_lru_stamp: u64,
    quota_bytes: usize,
    total_bytes: usize,
    next_placement_uid: u64,
    // A locked frame can still display an image after its last placement was deleted.
    completed_frame_images: HashSet<InternalImageId>,
}

impl Default for KittyImageStore {
    fn default() -> Self {
        Self::with_quota(DEFAULT_KITTY_STORE_QUOTA_BYTES)
    }
}

impl KittyImageStore {
    pub fn with_quota(quota_bytes: usize) -> Self {
        KittyImageStore {
            images: HashMap::new(),
            next_image_id: 1,
            next_lru_stamp: 0,
            quota_bytes,
            total_bytes: 0,
            next_placement_uid: 1,
            completed_frame_images: HashSet::new(),
        }
    }
    pub fn next_placement_uid(&mut self) -> u64 {
        let uid = self.next_placement_uid;
        self.next_placement_uid += 1;
        uid
    }
    pub fn store_image(&mut self, image: DecodedImage) -> Result<InternalImageId, KittyError> {
        self.store_image_replacing(image, None)
    }
    pub fn replace_image(
        &mut self,
        old_id: InternalImageId,
        image: DecodedImage,
    ) -> Result<InternalImageId, KittyError> {
        self.store_image_replacing(image, Some(old_id))
    }
    fn store_image_replacing(
        &mut self,
        image: DecodedImage,
        old_id: Option<InternalImageId>,
    ) -> Result<InternalImageId, KittyError> {
        let (rgba, width, height) = Self::normalize_to_rgba(image);
        let size = rgba.len();
        // A replacement may reclaim the old pixels, frames and scaled variants. Its base64 cache
        // is deliberately not credited: make_room may discard caches before deciding whether the
        // replacement fits.
        let replaced_bytes = old_id
            .and_then(|old_id| self.images.get(&old_id))
            .map(Self::stored_image_data_bytes)
            .unwrap_or(0);
        if !self.make_room(size.saturating_sub(replaced_bytes), old_id) {
            // No amount of eviction can make room for this one, so the client is being told about
            // storage pressure rather than about a malformed command: ENOSPC says "retry smaller"
            // where EINVAL would say "fix your control data".
            return Err(Self::quota_error("image exceeds storage quota"));
        }
        if let Some(old_id) = old_id {
            self.remove_entry(old_id);
        }
        let id = self.next_image_id;
        self.next_image_id += 1;
        let lru_stamp = self.bump_lru();
        self.images.insert(
            id,
            StoredImage {
                image: KittyImage {
                    rgba,
                    width,
                    height,
                    scaled_variants: HashMap::new(),
                    frames: Vec::new(),
                    root_frame_gap_ms: 0,
                    current_frame: 1,
                    animation_state: 0,
                    animation_loop_count: 0,
                },
                refcount: 0,
                lru_stamp,
                base64_cache: HashMap::new(),
                raster_revision: 0,
                pending_invalidation: false,
                protect_unscaled: false,
                completed_rgba: None,
            },
        );
        self.total_bytes += size;
        Ok(id)
    }
    pub fn get(&self, id: InternalImageId) -> Option<&KittyImage> {
        self.images.get(&id).map(|stored| &stored.image)
    }
    pub fn raster_revision(&self, id: InternalImageId) -> Option<u64> {
        self.images.get(&id).map(|stored| stored.raster_revision)
    }
    pub fn scaled_variant(&self, id: InternalImageId, key: ScaledImageKey) -> Option<&[u8]> {
        self.images
            .get(&id)
            .and_then(|stored| stored.image.scaled_variants.get(&key))
            .map(|bytes| bytes.as_slice())
    }
    /// Rebuild a raster invalidated by an animation frame change using its complete pixel key.
    pub fn ensure_scaled_variant(&mut self, id: InternalImageId, key: ScaledImageKey) -> bool {
        if self.scaled_variant(id, key).is_some() {
            return true;
        }
        if !self.images.contains_key(&id) {
            return false;
        }
        let Some(incoming) = key.byte_len() else {
            return false;
        };
        if !self.make_room(incoming, Some(id)) {
            return false;
        }
        let Some(bytes) = self.scaled_variant_pixels(id, key) else {
            return false;
        };
        self.add_scaled_variant(id, key, bytes)
    }
    fn scaled_variant_pixels(&self, id: InternalImageId, key: ScaledImageKey) -> Option<Vec<u8>> {
        let image = self.get(id)?;
        let source = key.source;
        let cropped = super::grid_state::crop_rgba(
            image.displayed_rgba(),
            image.width as usize,
            image.height as usize,
            source.x,
            source.y as usize,
            source.width,
            source.height,
        );
        Some(super::grid_state::scale_rgba(
            &cropped,
            source.width,
            source.height,
            key.size.0,
            key.size.1,
        ))
    }
    /// Commit the post-placement live keys and its required raster only after they fit.
    pub(super) fn retain_scaled_variants(
        &mut self,
        id: InternalImageId,
        live: &HashSet<ScaledImageKey>,
        required: Option<ScaledImageKey>,
    ) -> bool {
        let Some(stored) = self.images.get(&id) else {
            return false;
        };
        let obsolete_pixels: usize = stored
            .image
            .scaled_variants
            .iter()
            .filter(|(key, _)| !live.contains(key))
            .map(|(_, bytes)| bytes.len())
            .sum();
        let obsolete_caches: usize = stored
            .base64_cache
            .iter()
            .filter(|(key, _)| key.is_some_and(|key| !live.contains(&key)))
            .map(|(_, bytes)| bytes.len())
            .sum();
        let required = required.filter(|key| !stored.image.scaled_variants.contains_key(key));
        let incoming = match required {
            Some(key) => match key.byte_len() {
                Some(bytes) => bytes,
                None => return false,
            },
            None => 0,
        };
        let extra = incoming.saturating_sub(obsolete_pixels + obsolete_caches);
        if !self.has_room(extra) {
            // Preflight before make_room drops caches, so a failed placement keeps its encodings.
            let disposable: usize = self
                .images
                .iter()
                .map(|(other_id, stored)| {
                    if *other_id != id
                        && stored.refcount == 0
                        && !self.completed_frame_images.contains(other_id)
                    {
                        Self::stored_image_bytes(stored)
                    } else {
                        Self::base64_cache_bytes(stored)
                    }
                })
                .sum();
            if incoming.saturating_sub(obsolete_pixels)
                > self
                    .quota_bytes
                    .saturating_sub(self.total_bytes - disposable)
            {
                return false;
            }
            // make_room may discard the obsolete caches itself, so only credit pixels here.
            if !self.make_room(incoming.saturating_sub(obsolete_pixels), Some(id)) {
                return false;
            }
        }
        let staged = match required {
            Some(key) => match self.scaled_variant_pixels(id, key) {
                Some(bytes) => Some((key, bytes)),
                None => return false,
            },
            None => None,
        };
        let Some(stored) = self.images.get_mut(&id) else {
            return false;
        };
        let mut removed = 0;
        stored.image.scaled_variants.retain(|key, bytes| {
            if live.contains(key) {
                true
            } else {
                removed += bytes.len();
                false
            }
        });
        stored.base64_cache.retain(|key, bytes| {
            if key.is_some_and(|key| !live.contains(&key)) {
                removed += bytes.len();
                false
            } else {
                true
            }
        });
        if let Some((key, bytes)) = staged {
            stored.image.scaled_variants.insert(key, bytes);
        }
        self.total_bytes = self.total_bytes - removed + incoming;
        true
    }
    pub fn touch(&mut self, id: InternalImageId) {
        let lru_stamp = self.bump_lru();
        if let Some(stored) = self.images.get_mut(&id) {
            stored.lru_stamp = lru_stamp;
        }
    }
    pub fn add_placement_ref(&mut self, id: InternalImageId) {
        let lru_stamp = self.bump_lru();
        if let Some(stored) = self.images.get_mut(&id) {
            stored.refcount += 1;
            stored.lru_stamp = lru_stamp;
        }
    }
    pub fn remove_placement_ref(&mut self, id: InternalImageId) {
        if let Some(stored) = self.images.get_mut(&id) {
            stored.refcount = stored.refcount.saturating_sub(1);
        }
    }
    pub fn free(&mut self, id: InternalImageId) {
        let should_remove = match self.images.get_mut(&id) {
            Some(stored) => {
                stored.refcount = stored.refcount.saturating_sub(1);
                stored.refcount == 0
            },
            None => false,
        };
        if should_remove {
            self.remove_entry(id);
        }
    }
    pub fn zlib_base64_for(
        &mut self,
        id: InternalImageId,
        variant: Option<ScaledImageKey>,
    ) -> std::io::Result<Option<Rc<String>>> {
        // Shared rather than cloned: an image is megabytes of base64.
        let encoded = {
            let Some(stored) = self.images.get(&id) else {
                return Ok(None);
            };
            if let Some(cached) = stored.base64_cache.get(&variant) {
                return Ok(Some(cached.clone()));
            }
            let bytes = match variant {
                Some(key) => match stored.image.scaled_variants.get(&key) {
                    Some(bytes) => bytes,
                    None => return Ok(None),
                },
                None => stored
                    .completed_rgba
                    .as_deref()
                    .unwrap_or_else(|| stored.image.displayed_rgba()),
            };
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
            encoder.write_all(bytes)?;
            Rc::new(base64_simd::STANDARD.encode_to_string(encoder.finish()?))
        };
        // This cache is an optimization, not image data. Do not evict or reject anything merely
        // to retain a second representation of pixels that can be encoded again later.
        if self.has_room(encoded.len())
            && let Some(stored) = self.images.get_mut(&id)
        {
            stored.base64_cache.insert(variant, encoded.clone());
            self.total_bytes += encoded.len();
        }
        Ok(Some(encoded))
    }
    /// Cache the raster for a scaled placement. Returns `false` without replacing the previous
    /// raster when the new one cannot fit.
    pub fn add_scaled_variant(
        &mut self,
        id: InternalImageId,
        key: ScaledImageKey,
        bytes: Vec<u8>,
    ) -> bool {
        let old_len = match self.images.get(&id) {
            Some(stored) => stored
                .image
                .scaled_variants
                .get(&key)
                .map(Vec::len)
                .unwrap_or(0),
            None => return false,
        };
        self.remove_base64_cache(id, Some(key));
        if !self.make_room(bytes.len().saturating_sub(old_len), Some(id)) {
            return false;
        }
        let lru_stamp = self.bump_lru();
        let new_len = bytes.len();
        let Some(stored) = self.images.get_mut(&id) else {
            return false;
        };
        stored.image.scaled_variants.insert(key, bytes);
        stored.lru_stamp = lru_stamp;
        self.total_bytes = self.total_bytes - old_len + new_len;
        true
    }
    /// Replace a grid's rasters together, crediting obsolete pixels before checking the quota.
    pub(super) fn replace_scaled_variants(
        &mut self,
        replacements: HashMap<InternalImageId, HashSet<ScaledImageKey>>,
    ) -> bool {
        let mut reclaimed = 0;
        let mut incoming = 0usize;
        for (id, variants) in &replacements {
            let Some(stored) = self.images.get(id) else {
                return false;
            };
            reclaimed += stored
                .image
                .scaled_variants
                .values()
                .map(Vec::len)
                .sum::<usize>();
            for key in variants {
                let Some(bytes) = key.byte_len() else {
                    return false;
                };
                let Some(total) = incoming.checked_add(bytes) else {
                    return false;
                };
                incoming = total;
            }
        }
        // Affected images have placement references or completed-frame protection.
        if !self.make_room(incoming.saturating_sub(reclaimed), None) {
            return false;
        }
        for (id, keys) in replacements {
            let mut variants = HashMap::new();
            for key in keys {
                let pixels = self
                    .images
                    .get_mut(&id)
                    .and_then(|stored| stored.image.scaled_variants.remove(&key))
                    .or_else(|| self.scaled_variant_pixels(id, key));
                if let Some(pixels) = pixels {
                    variants.insert(key, pixels);
                }
            }
            self.remove_all_base64_caches(id);
            if let Some(stored) = self.images.get_mut(&id) {
                stored.image.scaled_variants = variants;
            }
        }
        self.total_bytes = self.total_bytes - reclaimed + incoming;
        true
    }
    /// Add `rgba` as a new frame, or replace the pixels of frame `edit` if one is named.
    ///
    /// Returns the 1-based number of the frame written, which for a new frame is the first free
    /// one: the root image is frame 1, so the first frame a client adds is frame 2.
    pub fn write_frame(
        &mut self,
        id: InternalImageId,
        edit: Option<u32>,
        rgba: Vec<u8>,
        gap_ms: i32,
    ) -> Result<Option<u32>, KittyError> {
        let old_len = {
            let stored = match self.images.get(&id) {
                Some(stored) => stored,
                None => return Ok(None),
            };
            match edit {
                Some(1) => stored.image.rgba.len(),
                Some(n) => match n
                    .checked_sub(2)
                    .and_then(|index| stored.image.frames.get(index as usize))
                {
                    Some(frame) => frame.rgba.len(),
                    None => return Ok(None),
                },
                None => 0,
            }
        };
        let displayed_changes = self.images.get(&id).is_some_and(|stored| {
            edit == Some(stored.image.current_frame as u32) && stored.image.displayed_rgba() != rgba
        });
        self.prepare_frame_mutation(id, displayed_changes, rgba.len(), old_len)?;
        let new_len = rgba.len();
        let Some(stored) = self.images.get_mut(&id) else {
            return Ok(None);
        };
        let frame_number = match edit {
            Some(1) => {
                // Editing frame 1 rewrites the root image itself.
                stored.image.rgba = rgba;
                stored.image.root_frame_gap_ms = gap_ms;
                1
            },
            Some(n) => {
                let Some(frame) = stored.image.frames.get_mut(n as usize - 2) else {
                    return Ok(None);
                };
                frame.rgba = rgba;
                frame.gap_ms = gap_ms;
                n
            },
            None => {
                stored.image.frames.push(KittyFrame { gap_ms, rgba });
                stored.image.frames.len() as u32 + 1
            },
        };
        self.total_bytes = self.total_bytes - old_len + new_len;
        if displayed_changes {
            self.invalidate_rasters(id);
        }
        let lru_stamp = self.bump_lru();
        if let Some(stored) = self.images.get_mut(&id) {
            stored.lru_stamp = lru_stamp;
        }
        Ok(Some(frame_number))
    }
    /// Set which frame is displayed. Changing it invalidates every cached raster derived from the
    /// old one — the scaled variants the render path reads and their base64 encodings.
    pub fn set_current_frame(
        &mut self,
        id: InternalImageId,
        frame_number: u32,
    ) -> Result<Option<()>, KittyError> {
        let Some(stored) = self.images.get(&id) else {
            return Ok(None);
        };
        let Some(pixels) = stored.image.frame_rgba(frame_number) else {
            return Ok(None);
        };
        let displayed_changes = stored.image.displayed_rgba() != pixels;
        self.prepare_frame_mutation(id, displayed_changes, 0, 0)?;
        if let Some(stored) = self.images.get_mut(&id) {
            stored.image.current_frame = frame_number as usize;
        }
        if displayed_changes {
            self.invalidate_rasters(id);
        }
        Ok(Some(()))
    }
    pub fn set_frame_gap(
        &mut self,
        id: InternalImageId,
        frame_number: u32,
        gap_ms: i32,
    ) -> Option<()> {
        let stored = self.images.get_mut(&id)?;
        match frame_number {
            0 => None,
            1 => {
                stored.image.root_frame_gap_ms = gap_ms;
                Some(())
            },
            n => {
                stored.image.frames.get_mut(n as usize - 2)?.gap_ms = gap_ms;
                Some(())
            },
        }
    }
    pub fn set_animation_state(
        &mut self,
        id: InternalImageId,
        state: Option<u32>,
        loops: Option<u32>,
    ) {
        if let Some(stored) = self.images.get_mut(&id) {
            if let Some(state) = state {
                stored.image.animation_state = state;
            }
            if let Some(loops) = loops {
                stored.image.animation_loop_count = loops;
            }
        }
    }
    /// Delete frame `frame_number`, or every frame when none is named. The root image is never
    /// removed this way — spec, `d=f`: it deletes "the animation frames" and leaves the image.
    pub fn remove_frames(
        &mut self,
        id: InternalImageId,
        frame_number: Option<u32>,
    ) -> Result<Option<()>, KittyError> {
        let Some(stored) = self.images.get(&id) else {
            return Ok(None);
        };
        let image = &stored.image;
        let (removed_bytes, remaining_count) = match frame_number {
            Some(0) | Some(1) => return Ok(None),
            Some(n) => {
                let Some(frame) = image.frames.get(n as usize - 2) else {
                    return Ok(None);
                };
                (frame.rgba.len(), image.frame_count() as usize - 1)
            },
            None => (image.frame_bytes(), 1),
        };
        let next_current = if image.current_frame > remaining_count {
            1
        } else {
            image.current_frame
        };
        let old_number = match frame_number {
            Some(n) if next_current >= n as usize => next_current + 1,
            _ => next_current,
        };
        let displayed_changes = image.frame_rgba(old_number as u32) != Some(image.displayed_rgba());
        self.prepare_frame_mutation(id, displayed_changes, 0, removed_bytes)?;
        if let Some(stored) = self.images.get_mut(&id) {
            match frame_number {
                Some(n) => {
                    stored.image.frames.remove(n as usize - 2);
                },
                None => stored.image.frames.clear(),
            }
            stored.image.current_frame = next_current;
        }
        self.total_bytes -= removed_bytes;
        if displayed_changes {
            self.invalidate_rasters(id);
        }
        Ok(Some(()))
    }
    // Preserve unscaled completed pixels only when a successful mutation needs a copy.
    fn prepare_frame_mutation(
        &mut self,
        id: InternalImageId,
        displayed_changes: bool,
        incoming: usize,
        reclaimed: usize,
    ) -> Result<(), KittyError> {
        let snapshot_bytes = self.images.get(&id).map_or(0, |stored| {
            if displayed_changes && stored.protect_unscaled && stored.completed_rgba.is_none() {
                stored.image.displayed_rgba().len()
            } else {
                0
            }
        });
        if !self.make_room(
            (incoming + snapshot_bytes).saturating_sub(reclaimed),
            Some(id),
        ) {
            return Err(Self::quota_error("animation frame exceeds storage quota"));
        }
        if snapshot_bytes > 0
            && let Some(stored) = self.images.get_mut(&id)
        {
            stored.completed_rgba = Some(stored.image.displayed_rgba().to_vec());
            self.total_bytes += snapshot_bytes;
        }
        Ok(())
    }
    fn invalidate_rasters(&mut self, id: InternalImageId) {
        let Some(stored) = self.images.get_mut(&id) else {
            return;
        };
        if self.completed_frame_images.contains(&id) {
            stored.pending_invalidation = true;
            return;
        }
        self.total_bytes -= stored
            .image
            .scaled_variants
            .values()
            .map(Vec::len)
            .sum::<usize>()
            + Self::base64_cache_bytes(stored);
        stored.image.scaled_variants.clear();
        stored.base64_cache.clear();
        stored.raster_revision += 1;
        stored.pending_invalidation = false;
    }
    /// Remove the image outright, whatever its refcount. `d=F` frees the data even while it is on
    /// screen, unlike the refcounted [`Self::free`].
    pub fn remove_image(&mut self, id: InternalImageId) {
        self.remove_entry(id);
    }
    pub fn refcount(&self, id: InternalImageId) -> Option<usize> {
        self.images.get(&id).map(|stored| stored.refcount)
    }
    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }
    pub fn image_count(&self) -> usize {
        self.images.len()
    }
    fn bump_lru(&mut self) -> u64 {
        self.next_lru_stamp += 1;
        self.next_lru_stamp
    }
    fn quota_error(message: &str) -> KittyError {
        KittyError {
            code: KittyErrorCode::Enospc,
            message: message.to_owned(),
            image_id: None,
            image_number: None,
            placement_id: None,
            quiet: 0,
        }
    }
    fn base64_cache_bytes(stored: &StoredImage) -> usize {
        stored.base64_cache.values().map(|b64| b64.len()).sum()
    }
    fn stored_image_data_bytes(stored: &StoredImage) -> usize {
        stored.image.rgba.len()
            + stored
                .image
                .scaled_variants
                .values()
                .map(|bytes| bytes.len())
                .sum::<usize>()
            + stored.image.frame_bytes()
            + stored.completed_rgba.as_ref().map_or(0, Vec::len)
    }
    fn stored_image_bytes(stored: &StoredImage) -> usize {
        Self::stored_image_data_bytes(stored) + Self::base64_cache_bytes(stored)
    }
    pub(super) fn remove_entry(&mut self, id: InternalImageId) {
        self.completed_frame_images.remove(&id);
        if let Some(stored) = self.images.remove(&id) {
            self.total_bytes -= Self::stored_image_bytes(&stored);
        }
    }
    fn has_room(&self, incoming_bytes: usize) -> bool {
        incoming_bytes <= self.quota_bytes.saturating_sub(self.total_bytes)
    }
    pub(super) fn protect_completed_frame_image(&mut self, id: InternalImageId, unscaled: bool) {
        self.completed_frame_images.insert(id);
        if let Some(stored) = self.images.get_mut(&id) {
            stored.protect_unscaled |= unscaled;
        }
    }
    pub(super) fn release_completed_frame_image(&mut self, id: InternalImageId) {
        self.completed_frame_images.remove(&id);
        if let Some(stored) = self.images.get_mut(&id) {
            stored.protect_unscaled = false;
            self.total_bytes -= stored
                .completed_rgba
                .take()
                .map_or(0, |pixels| pixels.len());
            if stored.pending_invalidation {
                self.invalidate_rasters(id);
            }
        }
    }
    fn remove_base64_cache(&mut self, id: InternalImageId, variant: Option<ScaledImageKey>) {
        let removed = self
            .images
            .get_mut(&id)
            .and_then(|stored| stored.base64_cache.remove(&variant))
            .map(|encoded| encoded.len())
            .unwrap_or(0);
        self.total_bytes -= removed;
    }
    fn remove_all_base64_caches(&mut self, id: InternalImageId) {
        let removed = match self.images.get_mut(&id) {
            Some(stored) => {
                let removed = Self::base64_cache_bytes(stored);
                stored.base64_cache.clear();
                removed
            },
            None => 0,
        };
        self.total_bytes -= removed;
    }
    fn remove_every_base64_cache(&mut self) {
        let removed: usize = self
            .images
            .values_mut()
            .map(|stored| {
                let removed = Self::base64_cache_bytes(stored);
                stored.base64_cache.clear();
                removed
            })
            .sum();
        self.total_bytes -= removed;
    }
    /// Make room without ever evicting a displayed image. Base64 encodings are disposable, while
    /// image entries are removed only after a preflight proves that the request can succeed.
    fn make_room(&mut self, incoming_bytes: usize, protected: Option<InternalImageId>) -> bool {
        if incoming_bytes > self.quota_bytes {
            return false;
        }
        if self.has_room(incoming_bytes) {
            return true;
        }
        self.remove_every_base64_cache();
        if self.has_room(incoming_bytes) {
            return true;
        }
        let reclaimable: usize = self
            .images
            .iter()
            .filter(|(id, stored)| {
                stored.refcount == 0
                    && Some(**id) != protected
                    && !self.completed_frame_images.contains(id)
            })
            .map(|(_, stored)| Self::stored_image_bytes(stored))
            .sum();
        let bytes_needed = incoming_bytes - self.quota_bytes.saturating_sub(self.total_bytes);
        if reclaimable < bytes_needed {
            return false;
        }
        while !self.has_room(incoming_bytes) {
            let candidate = self
                .images
                .iter()
                .filter(|(id, stored)| {
                    stored.refcount == 0
                        && Some(**id) != protected
                        && !self.completed_frame_images.contains(id)
                })
                .min_by_key(|(_, stored)| stored.lru_stamp)
                .map(|(id, _)| *id);
            match candidate {
                Some(id) => self.remove_entry(id),
                None => return false,
            }
        }
        true
    }
    /// Turn a decoded payload into the RGBA every raster in the store is held as. Shared by image
    /// transmission and by animation frame loading, which decode identically.
    pub fn normalize_to_rgba(image: DecodedImage) -> (Vec<u8>, u32, u32) {
        match image.format {
            KittyFormat::Rgb24 => {
                let pixels = image.bytes.as_chunks::<3>().0;
                // Opaque from the start, so each pixel is a single 3-byte copy.
                let mut rgba = vec![255; pixels.len() * 4];
                for (rgba_pixel, pixel) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(pixels) {
                    rgba_pixel[..3].copy_from_slice(pixel);
                }
                (rgba, image.width, image.height)
            },
            KittyFormat::Rgba32 | KittyFormat::Png => (image.bytes, image.width, image.height),
        }
    }
}

#[cfg(test)]
#[path = "./unit/store_tests.rs"]
mod store_tests;
