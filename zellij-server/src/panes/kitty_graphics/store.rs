use base64::engine::general_purpose::STANDARD as BASE64_ENCODER;
use base64::engine::Engine as _;

use super::parser::{DecodedImage, KittyError, KittyErrorCode, KittyFormat};
use std::collections::HashMap;

pub const DEFAULT_KITTY_STORE_QUOTA_BYTES: usize = 335_544_320;

pub type InternalImageId = u64;

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
    pub scaled_variants: HashMap<(u16, u16), Vec<u8>>,
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
    base64_cache: HashMap<Option<(u16, u16)>, String>,
}

#[derive(Debug, Clone)]
pub struct KittyImageStore {
    images: HashMap<InternalImageId, StoredImage>,
    next_image_id: InternalImageId,
    next_lru_stamp: u64,
    quota_bytes: usize,
    total_bytes: usize,
    next_placement_uid: u64,
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
        }
    }
    pub fn next_placement_uid(&mut self) -> u64 {
        let uid = self.next_placement_uid;
        self.next_placement_uid += 1;
        uid
    }
    pub fn store_image(&mut self, image: DecodedImage) -> Result<InternalImageId, KittyError> {
        let (rgba, width, height) = Self::normalize_to_rgba(image);
        let size = rgba.len();
        if size > self.quota_bytes {
            // No amount of eviction can make room for this one, so the client is being told about
            // storage pressure rather than about a malformed command: ENOSPC says "retry smaller"
            // where EINVAL would say "fix your control data".
            return Err(KittyError {
                code: KittyErrorCode::Enospc,
                message: "image exceeds storage quota".to_owned(),
                image_id: None,
                image_number: None,
                placement_id: None,
                quiet: 0,
            });
        }
        self.evict_to_fit(size, None);
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
            },
        );
        self.total_bytes += size;
        Ok(id)
    }
    pub fn get(&self, id: InternalImageId) -> Option<&KittyImage> {
        self.images.get(&id).map(|stored| &stored.image)
    }
    pub fn scaled_variant(&self, id: InternalImageId, cells: (u16, u16)) -> Option<&[u8]> {
        self.images
            .get(&id)
            .and_then(|stored| stored.image.scaled_variants.get(&cells))
            .map(|bytes| bytes.as_slice())
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
    pub fn base64_for(
        &mut self,
        id: InternalImageId,
        variant: Option<(u16, u16)>,
    ) -> Option<String> {
        let stored = self.images.get_mut(&id)?;
        if let Some(cached) = stored.base64_cache.get(&variant) {
            return Some(cached.clone());
        }
        let bytes = match variant {
            Some(cells) => stored.image.scaled_variants.get(&cells)?,
            None => stored.image.displayed_rgba(),
        };
        let encoded = BASE64_ENCODER.encode(bytes);
        stored.base64_cache.insert(variant, encoded.clone());
        Some(encoded)
    }
    pub fn add_scaled_variant(&mut self, id: InternalImageId, cells: (u16, u16), bytes: Vec<u8>) {
        let lru_stamp = self.bump_lru();
        let inserted = match self.images.get_mut(&id) {
            Some(stored) => {
                let new_len = bytes.len();
                let old_len = stored
                    .image
                    .scaled_variants
                    .insert(cells, bytes)
                    .map(|old| old.len())
                    .unwrap_or(0);
                stored.base64_cache.remove(&Some(cells));
                stored.lru_stamp = lru_stamp;
                Some((old_len, new_len))
            },
            None => None,
        };
        if let Some((old_len, new_len)) = inserted {
            self.total_bytes = self.total_bytes - old_len + new_len;
            if self.total_bytes > self.quota_bytes {
                self.evict_to_fit(0, Some(id));
            }
        }
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
    ) -> Option<u32> {
        let stored = self.images.get_mut(&id)?;
        let before = stored.image.rgba.len() + stored.image.frame_bytes();
        let frame_number = match edit {
            Some(1) => {
                // Editing frame 1 rewrites the root image itself.
                stored.image.rgba = rgba;
                stored.image.root_frame_gap_ms = gap_ms;
                1
            },
            Some(n) => {
                let frame = stored.image.frames.get_mut(n as usize - 2)?;
                frame.rgba = rgba;
                frame.gap_ms = gap_ms;
                n
            },
            None => {
                stored.image.frames.push(KittyFrame { gap_ms, rgba });
                stored.image.frames.len() as u32 + 1
            },
        };
        let after = stored.image.rgba.len() + stored.image.frame_bytes();
        stored.base64_cache.clear();
        self.total_bytes = self.total_bytes + after - before;
        let lru_stamp = self.bump_lru();
        if let Some(stored) = self.images.get_mut(&id) {
            stored.lru_stamp = lru_stamp;
        }
        Some(frame_number)
    }
    /// Set which frame is displayed. Changing it invalidates every cached raster derived from the
    /// old one — the scaled variants the render path reads and their base64 encodings.
    pub fn set_current_frame(&mut self, id: InternalImageId, frame_number: u32) -> Option<()> {
        let stored = self.images.get_mut(&id)?;
        stored.image.frame_rgba(frame_number)?;
        if stored.image.current_frame != frame_number as usize {
            stored.image.current_frame = frame_number as usize;
            let freed: usize = stored.image.scaled_variants.values().map(Vec::len).sum();
            stored.image.scaled_variants.clear();
            stored.base64_cache.clear();
            self.total_bytes -= freed;
        }
        Some(())
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
    pub fn remove_frames(&mut self, id: InternalImageId, frame_number: Option<u32>) -> Option<()> {
        let stored = self.images.get_mut(&id)?;
        let before = stored.image.frame_bytes();
        match frame_number {
            // Frame 1 is the root image, which `d=f` may not take away.
            Some(0) | Some(1) => return None,
            Some(n) => {
                let index = n as usize - 2;
                if index >= stored.image.frames.len() {
                    return None;
                }
                stored.image.frames.remove(index);
            },
            None => stored.image.frames.clear(),
        }
        // The frames after a removed one shift down, so a current frame past the end has to come
        // back to something that exists.
        if stored.image.current_frame > stored.image.frame_count() as usize {
            stored.image.current_frame = 1;
            stored.image.scaled_variants.clear();
        }
        stored.base64_cache.clear();
        let after = stored.image.frame_bytes();
        self.total_bytes = self.total_bytes + after - before;
        Some(())
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
    fn stored_image_bytes(stored: &StoredImage) -> usize {
        stored.image.rgba.len()
            + stored
                .image
                .scaled_variants
                .values()
                .map(|bytes| bytes.len())
                .sum::<usize>()
            + stored.image.frame_bytes()
    }
    fn remove_entry(&mut self, id: InternalImageId) {
        if let Some(stored) = self.images.remove(&id) {
            self.total_bytes -= Self::stored_image_bytes(&stored);
        }
    }
    fn evict_to_fit(&mut self, incoming_bytes: usize, protected: Option<InternalImageId>) {
        while self.total_bytes + incoming_bytes > self.quota_bytes {
            let candidate = self
                .images
                .iter()
                .filter(|(id, stored)| stored.refcount == 0 && Some(**id) != protected)
                .min_by_key(|(_, stored)| stored.lru_stamp)
                .map(|(id, _)| *id);
            match candidate {
                Some(id) => self.remove_entry(id),
                None => break,
            }
        }
    }
    /// Turn a decoded payload into the RGBA every raster in the store is held as. Shared by image
    /// transmission and by animation frame loading, which decode identically.
    pub fn normalize_to_rgba(image: DecodedImage) -> (Vec<u8>, u32, u32) {
        match image.format {
            KittyFormat::Rgb24 => {
                let mut rgba = Vec::with_capacity(image.bytes.len() / 3 * 4);
                for pixel in image.bytes.chunks_exact(3) {
                    rgba.extend_from_slice(pixel);
                    rgba.push(255);
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
