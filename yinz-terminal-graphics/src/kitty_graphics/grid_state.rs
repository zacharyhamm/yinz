use super::parser::{DecodedImage, KittyCommand, KittyError, KittyErrorCode, MAX_RELATIVE_DEPTH};
use super::store::{InternalImageId, KittyImageStore, ScaledImageKey, DEFAULT_FRAME_GAP_MS};
use crate::sixel::PixelRect;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use yinz_types::pane_size::SizeInPixels;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KittyImageChunk {
    pub cell_x: usize,
    pub cell_y: usize,
    pub internal_image_id: InternalImageId,
    pub source_px_x: usize,
    pub source_px_y: usize,
    pub source_px_width: usize,
    pub source_px_height: usize,
    pub cell_offset_x: u32,
    pub cell_offset_y: u32,
    pub z_index: i32,
    pub dest_cells: (u16, u16),
    /// The source crop and pixel size of the raster, shared by all slices of a prototype.
    pub variant_key: ScaledImageKey,
    pub scaled_px: Option<(usize, usize)>,
    pub placement_uid: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KittyVerticalAnchor {
    pub canonical_line: usize,
    pub offset_px_from_line_start: isize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KittyPlacement {
    pub image_id: u32,
    pub placement_id: u32,
    pub placement_uid: u64,
    pub internal_id: InternalImageId,
    pub display_rect: PixelRect,
    /// Each axis can be reprojected independently while a raster resize is pending.
    pub position_cell_size: SizeInPixels,
    pub source_rect: PixelRect,
    pub emit_x: usize,
    pub emit_y: usize,
    pub scaled_px: Option<(usize, usize)>,
    pub dest_cells: (u16, u16),
    /// See [`KittyImageChunk::variant_key`].
    pub variant_key: ScaledImageKey,
    pub cell_offset: (u32, u32),
    pub z_index: i32,
    pub vertical_anchor: KittyVerticalAnchor,
    /// Derived from unicode placeholder cells rather than from a placement command, and therefore
    /// rebuilt wholesale from the grid on every refresh. The cells *are* the placement, so erasing
    /// them, scrolling them away or reflowing them needs no separate bookkeeping here.
    pub is_placeholder: bool,
    /// Set by a `P`/`Q` placement: this one has no position of its own, it sits at an offset from
    /// another placement. [`KittyGrid::resolve_relative_placements`] recomputes `display_rect` from
    /// the parent whenever anything moves.
    pub relative_to: Option<RelativeParent>,
}

/// Which placement a relative placement hangs off, and how far from its top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelativeParent {
    pub image_id: u32,
    /// `None` when the command left `Q` out, which selects any placement of the parent image. A
    /// client naming a virtual parent it never gave a placement id to relies on this.
    pub placement_id: Option<u32>,
    /// `H` and `V`, in cells, right and down positive.
    pub offset_cells: (i32, i32),
}

impl RelativeParent {
    /// Whether `placement` is the placement this parent reference selects.
    fn matches(&self, image_id: u32, placement_id: u32) -> bool {
        self.image_id == image_id
            && self
                .placement_id
                .map(|wanted| wanted == placement_id)
                .unwrap_or(true)
    }
}

/// A placement made with `U=1`: a prototype rather than something on screen.
///
/// Spec, "Unicode placeholders": "Virtual placements are invisible and only play the role of
/// prototypes for real images". It becomes visible only where the client prints
/// [`super::PLACEHOLDER`] cells naming it, and each run of those cells materialises into an
/// ordinary [`KittyPlacement`] showing its part of the image.
#[derive(Debug, Clone, PartialEq)]
pub struct VirtualPlacement {
    pub image_id: u32,
    pub placement_id: u32,
    pub internal_id: InternalImageId,
    /// The `c` x `r` cell box the image is fitted into.
    pub columns: u16,
    pub rows: u16,
    /// The rectangle of the original image fitted into the cell box.
    pub source: (usize, usize, usize, usize),
    /// The image is fitted into the box preserving its aspect ratio, so this is at most the box.
    pub fitted_px: (usize, usize),
    pending_geometry: Option<(u32, u32)>,
    pub z_index: i32,
}

impl VirtualPlacement {
    pub fn variant_key(&self) -> ScaledImageKey {
        let (x, y, width, height) = self.source;
        ScaledImageKey {
            source: PixelRect::new(x, y, height, width),
            size: self.fitted_px,
        }
    }
}

fn virtual_dimensions(
    columns: u32,
    rows: u32,
    source_w: usize,
    source_h: usize,
    cell: SizeInPixels,
) -> (usize, usize) {
    let ceil_div = |a: usize, b: usize| std::cmp::max(a.div_ceil(b), 1);
    // Spec, "Controlling displayed image layout": with only one of `c`/`r` given the other
    // follows the source aspect ratio; with neither, the image keeps its natural size.
    let (columns, rows) = match (columns as usize, rows as usize) {
        (0, 0) => (
            ceil_div(source_w, cell.width),
            ceil_div(source_h, cell.height),
        ),
        (c, 0) => (
            c,
            ceil_div(c * cell.width * source_h / source_w.max(1), cell.height),
        ),
        (0, r) => (
            ceil_div(r * cell.height * source_w / source_h.max(1), cell.width),
            r,
        ),
        (c, r) => (c, r),
    };
    (columns, rows)
}

fn fit_into_box(source: (usize, usize), box_px: (usize, usize)) -> (usize, usize) {
    let scale = f64::min(
        box_px.0 as f64 / source.0 as f64,
        box_px.1 as f64 / source.1 as f64,
    );
    (
        std::cmp::max((source.0 as f64 * scale).round() as usize, 1),
        std::cmp::max((source.1 as f64 * scale).round() as usize, 1),
    )
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KittyReplyData {
    pub image_id: Option<u32>,
    pub image_number: Option<u32>,
    pub placement_id: Option<u32>,
    /// The `r` key: which frame an `a=f` wrote. Only animation replies carry it.
    pub frame_number: Option<u32>,
    pub quiet: u8,
}

impl KittyReplyData {
    pub fn from_command(command: &KittyCommand) -> Self {
        KittyReplyData {
            image_id: command.image_id,
            image_number: command.image_number,
            placement_id: command.placement_id,
            frame_number: None,
            quiet: command.quiet,
        }
    }
}

#[derive(Debug, Clone)]
pub struct KittyGrid {
    character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
    pub kitty_image_store: Rc<RefCell<KittyImageStore>>,
    image_ids: HashMap<u32, InternalImageId>,
    anonymous_image_ids: Vec<InternalImageId>,
    image_numbers: HashMap<u32, Vec<u32>>,
    placements: Vec<KittyPlacement>,
    virtual_placements: Vec<VirtualPlacement>,
    /// Keeps a placeholder run's `placement_uid` stable across the rebuilds it goes through on
    /// every frame. The host-side diff in `output/mod.rs` treats a new uid as a new placement, so
    /// handing out a fresh one each time would retransmit every placeholder image continuously.
    placeholder_uids: HashMap<(u32, u32, usize, usize), u64>,
    next_synthetic_image_id: u32,
    front_drops: u64,
    merged_rows: u64,
    split_rows: u64,
    commands_handled: u64,
    previous_cell_size: Option<SizeInPixels>,
    completed_frame_keys: HashSet<(InternalImageId, Option<ScaledImageKey>)>,
    rows_below_the_viewport: Option<KittyRowsBelowTheViewport>,
}

#[derive(Clone, Copy, Debug)]
pub struct KittyRowsBelowTheViewport {
    pub first_row: usize,
    pub total_rows: usize,
}

fn einval(command: &KittyCommand, message: &str) -> KittyError {
    KittyError {
        code: KittyErrorCode::Einval,
        message: message.to_owned(),
        image_id: command.image_id,
        image_number: command.image_number,
        placement_id: command.placement_id,
        quiet: command.quiet,
    }
}

fn enoent(command: &KittyCommand, message: &str) -> KittyError {
    KittyError {
        code: KittyErrorCode::Enoent,
        message: message.to_owned(),
        image_id: command.image_id,
        image_number: command.image_number,
        placement_id: command.placement_id,
        quiet: command.quiet,
    }
}

fn enospc(command: &KittyCommand, message: &str) -> KittyError {
    KittyError {
        code: KittyErrorCode::Enospc,
        message: message.to_owned(),
        image_id: command.image_id,
        image_number: command.image_number,
        placement_id: command.placement_id,
        quiet: command.quiet,
    }
}

fn with_command_fields(mut error: KittyError, command: &KittyCommand) -> KittyError {
    error.image_id = command.image_id;
    error.image_number = command.image_number;
    error.placement_id = command.placement_id;
    error.quiet = command.quiet;
    error
}

/// The three ways a relative placement can be refused, all echoing the same fields as the errors
/// above so a client can tell which of several in-flight placements was rejected.
fn relative_error(command: &KittyCommand, code: KittyErrorCode, message: &str) -> KittyError {
    KittyError {
        code,
        message: message.to_owned(),
        image_id: command.image_id,
        image_number: command.image_number,
        placement_id: command.placement_id,
        quiet: command.quiet,
    }
}

impl KittyGrid {
    pub fn new(
        character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
        kitty_image_store: Rc<RefCell<KittyImageStore>>,
    ) -> Self {
        let previous_cell_size = *character_cell_size.borrow();
        KittyGrid {
            character_cell_size,
            kitty_image_store,
            image_ids: HashMap::new(),
            anonymous_image_ids: Vec::new(),
            image_numbers: HashMap::new(),
            placements: Vec::new(),
            virtual_placements: Vec::new(),
            placeholder_uids: HashMap::new(),
            next_synthetic_image_id: u32::MAX,
            front_drops: 0,
            merged_rows: 0,
            split_rows: 0,
            commands_handled: 0,
            previous_cell_size,
            completed_frame_keys: HashSet::new(),
            rows_below_the_viewport: None,
        }
    }
    pub fn note_command_handled(&mut self) {
        self.commands_handled += 1;
    }
    pub fn commands_handled(&self) -> u64 {
        self.commands_handled
    }
    pub fn front_drops(&self) -> u64 {
        self.front_drops
    }
    pub fn merged_rows(&self) -> u64 {
        self.merged_rows
    }
    pub fn split_rows(&self) -> u64 {
        self.split_rows
    }
    pub fn placements(&self) -> &[KittyPlacement] {
        &self.placements
    }
    pub fn placements_mut(&mut self) -> &mut [KittyPlacement] {
        &mut self.placements
    }
    pub fn protect_completed_frame(&mut self, chunks: &[KittyImageChunk]) {
        let owned: HashSet<_> = self
            .image_ids
            .values()
            .chain(&self.anonymous_image_ids)
            .copied()
            .collect();
        self.completed_frame_keys.extend(
            chunks
                .iter()
                .filter(|chunk| owned.contains(&chunk.internal_image_id))
                .map(|chunk| {
                    (
                        chunk.internal_image_id,
                        chunk.scaled_px.map(|_| chunk.variant_key),
                    )
                }),
        );
        let mut store = self.kitty_image_store.borrow_mut();
        for &(id, key) in &self.completed_frame_keys {
            store.protect_completed_frame_image(id, key.is_none());
        }
    }
    pub fn release_completed_frame(&mut self) {
        let affected: HashSet<_> = self
            .completed_frame_keys
            .drain()
            .map(|(id, _)| id)
            .collect();
        let mut store = self.kitty_image_store.borrow_mut();
        for &id in &affected {
            store.release_completed_frame_image(id);
        }
        drop(store);
        self.reclaim_scaled_variants(affected);
    }
    fn live_scaled_keys(
        &self,
        internal: InternalImageId,
        keep_placement: impl Fn(&KittyPlacement) -> bool,
        keep_virtual: impl Fn(&VirtualPlacement) -> bool,
    ) -> HashSet<ScaledImageKey> {
        self.placements
            .iter()
            .filter(|p| p.internal_id == internal && p.scaled_px.is_some() && keep_placement(p))
            .map(|p| p.variant_key)
            .chain(
                self.virtual_placements
                    .iter()
                    .filter(|p| {
                        p.internal_id == internal && p.pending_geometry.is_none() && keep_virtual(p)
                    })
                    .map(VirtualPlacement::variant_key),
            )
            .chain(
                self.completed_frame_keys
                    .iter()
                    .filter(|(id, _)| *id == internal)
                    .filter_map(|(_, key)| *key),
            )
            .collect()
    }
    fn reclaim_scaled_variants(&self, affected: HashSet<InternalImageId>) {
        let mut store = self.kitty_image_store.borrow_mut();
        for internal in affected {
            let live = self.live_scaled_keys(internal, |_| true, |_| true);
            store.retain_scaled_variants(internal, &live, None);
        }
    }
    pub fn retain_placements<F: FnMut(&KittyPlacement) -> bool>(&mut self, mut keep: F) {
        let mut store = self.kitty_image_store.borrow_mut();
        let mut affected = HashSet::new();
        let mut kept = Vec::with_capacity(self.placements.len());
        for placement in self.placements.drain(..) {
            if keep(&placement) {
                kept.push(placement);
            } else {
                affected.insert(placement.internal_id);
                store.free(placement.internal_id);
            }
        }
        self.placements = kept;
        drop(store);
        self.reclaim_scaled_variants(affected);
    }
    pub fn placement_count(&self) -> usize {
        self.placements.len()
    }
    pub fn pane_image_id_map(&self) -> &HashMap<u32, InternalImageId> {
        &self.image_ids
    }
    fn alloc_synthetic_image_id(&mut self) -> u32 {
        loop {
            let id = self.next_synthetic_image_id;
            self.next_synthetic_image_id = self.next_synthetic_image_id.wrapping_sub(1);
            if id != 0 && !self.image_ids.contains_key(&id) {
                return id;
            }
        }
    }
    fn drop_image_number_references(&mut self, pane_image_id: u32) {
        for ids in self.image_numbers.values_mut() {
            ids.retain(|id| *id != pane_image_id);
        }
        self.image_numbers.retain(|_, ids| !ids.is_empty());
    }
    pub fn transmit(
        &mut self,
        command: &KittyCommand,
        image: DecodedImage,
    ) -> Result<u32, KittyError> {
        let pane_image_id = if command.image_number.is_some() {
            self.alloc_synthetic_image_id()
        } else {
            command.image_id.unwrap_or(0)
        };
        let old_internal = if pane_image_id == 0 {
            None
        } else {
            self.image_ids.get(&pane_image_id).copied()
        };
        // Store the replacement before touching its placements or pane-local ids. The store can
        // credit the old image's bytes while preflighting, then swap it only after success.
        let internal = {
            let mut store = self.kitty_image_store.borrow_mut();
            let result = match old_internal {
                Some(old_internal) => store.replace_image(old_internal, image),
                None => store.store_image(image),
            };
            result.map_err(|error| with_command_fields(error, command))?
        };
        if old_internal.is_some() {
            self.remove_virtual_placements(|virtual_placement| {
                virtual_placement.image_id == pane_image_id
            });
            let mut store = self.kitty_image_store.borrow_mut();
            let mut kept = Vec::with_capacity(self.placements.len());
            for placement in self.placements.drain(..) {
                if placement.image_id == pane_image_id {
                    store.free(placement.internal_id);
                } else {
                    kept.push(placement);
                }
            }
            self.placements = kept;
            drop(store);
            self.image_ids.remove(&pane_image_id);
            self.drop_image_number_references(pane_image_id);
            self.cascade_relative_deletes(true);
        }
        if let Some(image_number) = command.image_number {
            self.image_numbers
                .entry(image_number)
                .or_default()
                .push(pane_image_id);
        }
        self.image_ids.insert(pane_image_id, internal);
        let mut store = self.kitty_image_store.borrow_mut();
        if pane_image_id == 0 {
            // All anonymous images belong to this pane, even after its id=0 lookup is replaced.
            if self.anonymous_image_ids.len() >= store.image_count().saturating_mul(2) {
                self.anonymous_image_ids
                    .retain(|id| store.get(*id).is_some());
            }
            self.anonymous_image_ids.push(internal);
        }
        store.touch(internal);
        Ok(pane_image_id)
    }
    pub fn resolve_display_target(
        &self,
        command: &KittyCommand,
    ) -> Result<(u32, InternalImageId), KittyError> {
        let pane_image_id = if let Some(image_id) = command.image_id {
            image_id
        } else if let Some(image_number) = command.image_number {
            *self
                .image_numbers
                .get(&image_number)
                .and_then(|ids| ids.last())
                .ok_or_else(|| enoent(command, "unknown image number"))?
        } else {
            return Err(enoent(command, "no image id specified"));
        };
        let internal = *self
            .image_ids
            .get(&pane_image_id)
            .ok_or_else(|| enoent(command, "unknown image id"))?;
        if self.kitty_image_store.borrow().get(internal).is_none() {
            return Err(enoent(command, "unknown image id"));
        }
        Ok((pane_image_id, internal))
    }
    /// `a=f`: build one animation frame and add it to an image, or rewrite an existing one.
    ///
    /// Spec, "Transferring animation frame data": the frame starts as a copy of the frame named by
    /// `c`, or as a flat field of the `Y` background colour when no base is given. The payload then
    /// covers the rectangle at `x`,`y` of size `s`x`v`, either alpha-blended onto the canvas or
    /// overwriting it when `X=1`. Returns the 1-based number of the frame written; the root image
    /// is frame 1, so the first frame a client adds is frame 2.
    pub fn transmit_frame(
        &mut self,
        command: &KittyCommand,
        image: DecodedImage,
    ) -> Result<u32, KittyError> {
        let (_, internal) = self.resolve_display_target(command)?;
        let (frame_rgba, frame_width, frame_height) = KittyImageStore::normalize_to_rgba(image);
        let mut store = self.kitty_image_store.borrow_mut();
        let stored = store
            .get(internal)
            .ok_or_else(|| enoent(command, "unknown image id"))?;
        let (image_width, image_height) = (stored.width as usize, stored.height as usize);
        let edit = command.frame_target();
        if let Some(edit) = edit
            && stored.frame_rgba(edit).is_none()
        {
            return Err(enoent(command, "no such frame"));
        }
        // The canvas the payload is drawn onto: a copy of the base frame, or the background colour.
        let mut canvas = match command.base_frame() {
            Some(base) => stored
                .frame_rgba(base)
                .ok_or_else(|| enoent(command, "no such base frame"))?
                .to_vec(),
            None => match edit {
                // Editing a frame in place patches what is already there.
                Some(edit) => stored
                    .frame_rgba(edit)
                    .ok_or_else(|| enoent(command, "no such frame"))?
                    .to_vec(),
                None => background_canvas(command.frame_background(), image_width, image_height),
            },
        };
        let (dest_x, dest_y) = (command.source_x as usize, command.source_y as usize);
        let (data_w, data_h) = (frame_width as usize, frame_height as usize);
        if dest_x + data_w > image_width || dest_y + data_h > image_height {
            return Err(einval(command, "frame data does not fit inside the image"));
        }
        blit_rgba(
            &mut canvas,
            image_width,
            (dest_x, dest_y),
            &frame_rgba,
            (data_w, data_h),
            (0, 0),
            (data_w, data_h),
            command.frame_overwrites(),
        );
        let gap_ms = match command.frame_gap_ms() {
            0 => DEFAULT_FRAME_GAP_MS,
            gap_ms => gap_ms,
        };
        store
            .write_frame(internal, edit, canvas, gap_ms)
            .map_err(|error| with_command_fields(error, command))?
            .ok_or_else(|| enoent(command, "no such frame"))
    }
    /// `a=a`: control playback. Nothing here advances the animation by itself — this terminal has
    /// no playback clock — but `c` makes a frame current, which is what a client driving its own
    /// animation uses, and the rest is recorded so it can be read back.
    pub fn animate(&mut self, command: &KittyCommand) -> Result<(), KittyError> {
        let (_, internal) = self.resolve_display_target(command)?;
        let mut store = self.kitty_image_store.borrow_mut();
        if let Some(gap_frame) = command.frame_target() {
            store
                .set_frame_gap(internal, gap_frame, command.frame_gap_ms())
                .ok_or_else(|| enoent(command, "no such frame"))?;
        }
        if let Some(current) = command.base_frame() {
            store
                .set_current_frame(internal, current)
                .map_err(|error| with_command_fields(error, command))?
                .ok_or_else(|| enoent(command, "no such frame"))?;
        }
        store.set_animation_state(
            internal,
            command.animation_state(),
            command.animation_loop_count(),
        );
        drop(store);
        if command.base_frame().is_some() {
            // The pixels behind every placement of this image just changed, so the host has to be
            // sent the new ones. A fresh uid is exactly the "this is different, retransmit it"
            // signal `output/mod.rs` reads - the opposite of a relative placement, which keeps its
            // uid because only its position moved.
            self.retransmit_placements_of(internal);
        }
        Ok(())
    }
    /// `a=c`: blit a rectangle of one frame onto another.
    ///
    /// Note the polarity is the opposite of frame loading: here `r` is the source and `c` the
    /// destination, and `x`,`y` locate the rectangle in the destination while `X`,`Y` locate it in
    /// the source.
    pub fn compose_frames(&mut self, command: &KittyCommand) -> Result<(), KittyError> {
        let (_, internal) = self.resolve_display_target(command)?;
        let mut store = self.kitty_image_store.borrow_mut();
        let stored = store
            .get(internal)
            .ok_or_else(|| enoent(command, "unknown image id"))?;
        let (image_width, image_height) = (stored.width as usize, stored.height as usize);
        let source_frame = command.frame_target().unwrap_or(1);
        let dest_frame = command.base_frame().unwrap_or(1);
        let source = stored
            .frame_rgba(source_frame)
            .ok_or_else(|| enoent(command, "no such source frame"))?
            .to_vec();
        let mut dest = stored
            .frame_rgba(dest_frame)
            .ok_or_else(|| enoent(command, "no such destination frame"))?
            .to_vec();
        // Spec: `w` and `h` default to the whole frame. Every offset is checked rather than
        // clamped - a client that sends a rectangle running off the edge has a bug, and silently
        // composing a different rectangle than it asked for would hide it.
        let width = match command.source_w {
            0 => image_width,
            w => w as usize,
        };
        let height = match command.source_h {
            0 => image_height,
            h => h as usize,
        };
        let dest_origin = (command.source_x as usize, command.source_y as usize);
        let source_origin = (
            command.cell_offset_x as usize,
            command.cell_offset_y as usize,
        );
        for (origin, what) in [(dest_origin, "destination"), (source_origin, "source")] {
            if origin.0.saturating_add(width) > image_width
                || origin.1.saturating_add(height) > image_height
            {
                return Err(einval(
                    command,
                    &format!("{} rectangle is outside the frame", what),
                ));
            }
        }
        blit_rgba(
            &mut dest,
            image_width,
            dest_origin,
            &source,
            (image_width, image_height),
            source_origin,
            (width, height),
            command.frame_overwrites(),
        );
        store
            .write_frame(internal, Some(dest_frame), dest, 0)
            .map_err(|error| with_command_fields(error, command))?
            .ok_or_else(|| enoent(command, "no such destination frame"))?;
        drop(store);
        self.retransmit_placements_of(internal);
        Ok(())
    }
    /// `d=f`/`d=F`: delete an image's animation frames, or the image itself.
    ///
    /// Spec: lowercase leaves the image displayable — it still has its root frame — while
    /// uppercase frees the image data, as everywhere else in the delete matrix.
    fn delete_frames(&mut self, command: &KittyCommand, uppercase: bool) -> Result<(), KittyError> {
        let (pane_image_id, internal) = match self.resolve_display_target(command) {
            Ok(resolved) => resolved,
            // A delete naming an image that is already gone has nothing to do and nothing to
            // report, which is how every other specifier treats an unknown target.
            Err(_) => return Ok(()),
        };
        if uppercase {
            self.retain_placements(|placement| placement.internal_id != internal);
            self.remove_virtual_placements(|virtual_placement| {
                virtual_placement.internal_id == internal
            });
            self.cascade_relative_deletes(true);
            self.kitty_image_store.borrow_mut().remove_image(internal);
            self.image_ids.remove(&pane_image_id);
            self.drop_image_number_references(pane_image_id);
            return Ok(());
        }
        self.kitty_image_store
            .borrow_mut()
            .remove_frames(internal, command.frame_target())
            .map_err(|error| with_command_fields(error, command))?
            .ok_or_else(|| enoent(command, "no such frame"))?;
        self.retransmit_placements_of(internal);
        Ok(())
    }
    /// Give every placement of an image a new uid, which is how `output/mod.rs` is told the pixels
    /// behind it changed and the image has to be sent to the host again.
    fn retransmit_placements_of(&mut self, internal: InternalImageId) {
        let mut store = self.kitty_image_store.borrow_mut();
        for placement in self.placements.iter_mut() {
            if placement.internal_id == internal {
                placement.placement_uid = store.next_placement_uid();
            }
        }
    }
    pub fn place(
        &mut self,
        pane_image_id: u32,
        internal: InternalImageId,
        command: &KittyCommand,
        cursor_px: (usize, usize),
        cell: Option<SizeInPixels>,
        vertical_anchor: KittyVerticalAnchor,
    ) -> Result<Option<(u16, u16)>, KittyError> {
        let (image_width, image_height) = {
            let store = self.kitty_image_store.borrow();
            let image = store
                .get(internal)
                .ok_or_else(|| enoent(command, "unknown image id"))?;
            (image.width as usize, image.height as usize)
        };
        let source_x = command.source_x as usize;
        let source_y = command.source_y as usize;
        if source_x >= image_width || source_y >= image_height {
            return Err(einval(command, "source rectangle outside image bounds"));
        }
        let source_w = if command.source_w == 0 {
            image_width - source_x
        } else {
            std::cmp::min(command.source_w as usize, image_width - source_x)
        };
        let source_h = if command.source_h == 0 {
            image_height - source_y
        } else {
            std::cmp::min(command.source_h as usize, image_height - source_y)
        };
        if source_w == 0 || source_h == 0 {
            return Err(einval(command, "empty source rectangle"));
        }
        // The id this placement will get, needed up front: the self-parent and cycle checks are
        // about the placement being created, and they have to run before the retain further down
        // removes the placement they are checking against.
        let placement_id = if pane_image_id != 0 {
            command.placement_id.unwrap_or(0)
        } else {
            0
        };
        let relative_to = self.validate_relative_parent(command, pane_image_id, placement_id)?;
        if command.virtual_placement {
            self.place_virtual(
                pane_image_id,
                internal,
                command,
                cell,
                (source_x, source_y, source_w, source_h),
            )?;
            // Spec: a virtual placement is "invisible" until placeholder cells instantiate it, so
            // it occupies no cells here and the cursor has nothing to move past.
            return Ok(None);
        }
        let cell = cell.ok_or_else(|| KittyError {
            code: KittyErrorCode::Enotsupported,
            message: "cell size unknown".to_owned(),
            image_id: command.image_id,
            image_number: command.image_number,
            placement_id: command.placement_id,
            quiet: command.quiet,
        })?;
        let off_x = std::cmp::min(command.cell_offset_x as usize, cell.width - 1);
        let off_y = std::cmp::min(command.cell_offset_y as usize, cell.height - 1);
        let ceil_div = |a: usize, b: usize| a.div_ceil(b);
        let (dst_w, dst_h, cols, rows, is_scaled) =
            match (command.columns as usize, command.rows as usize) {
                (0, 0) => (
                    source_w,
                    source_h,
                    ceil_div(source_w + off_x, cell.width),
                    ceil_div(source_h + off_y, cell.height),
                    false,
                ),
                // Spec, "Controlling displayed image layout": with only one of `c`/`r` given the
                // other follows the source aspect ratio, and "if you specify a start cell offset
                // via the X,Y keys, it is not added to the number of rows/columns" - so the
                // derived axis is the scaled size alone.
                (c, 0) => {
                    let dst_w = c * cell.width;
                    let dst_h = std::cmp::max(
                        ((dst_w as f64) * (source_h as f64) / (source_w as f64)).round() as usize,
                        1,
                    );
                    let rows = std::cmp::max(ceil_div(dst_h, cell.height), 1);
                    (dst_w, dst_h, c, rows, true)
                },
                (0, r) => {
                    let dst_h = r * cell.height;
                    let dst_w = std::cmp::max(
                        ((dst_h as f64) * (source_w as f64) / (source_h as f64)).round() as usize,
                        1,
                    );
                    let cols = std::cmp::max(ceil_div(dst_w, cell.width), 1);
                    (dst_w, dst_h, cols, r, true)
                },
                (c, r) => (c * cell.width, r * cell.height, c, r, true),
            };
        let dest_cells = (cols as u16, rows as u16);
        let variant_key = ScaledImageKey {
            source: PixelRect::new(source_x, source_y, source_h, source_w),
            size: (dst_w, dst_h),
        };
        let (origin_x, origin_y) = match &relative_to {
            // Spec: "a relative placement gets its position specified based on another placement,
            // instead of the cursor". `validate_relative_parent` has already established that the
            // parent resolves, so this cannot be `None`.
            Some(parent) => self.relative_origin_px(parent, cell).ok_or_else(|| {
                relative_error(
                    command,
                    KittyErrorCode::Enoparent,
                    "parent placement no longer exists",
                )
            })?,
            None => (cursor_px.0 + off_x, (cursor_px.1 + off_y) as isize),
        };
        let display_rect = PixelRect {
            x: origin_x,
            y: origin_y,
            width: dst_w,
            height: dst_h,
        };
        let mut live = self.live_scaled_keys(
            internal,
            |p| {
                !(pane_image_id != 0
                    && placement_id != 0
                    && p.image_id == pane_image_id
                    && p.placement_id == placement_id)
            },
            |_| true,
        );
        if is_scaled {
            live.insert(variant_key);
        }
        if !self.kitty_image_store.borrow_mut().retain_scaled_variants(
            internal,
            &live,
            is_scaled.then_some(variant_key),
        ) {
            return Err(enospc(command, "scaled image exceeds storage quota"));
        }
        let mut reused_uid: Option<u64> = None;
        let mut affected = HashSet::new();
        if pane_image_id != 0 && placement_id != 0 {
            let mut store = self.kitty_image_store.borrow_mut();
            self.placements.retain(|placement| {
                if placement.image_id == pane_image_id && placement.placement_id == placement_id {
                    store.remove_placement_ref(placement.internal_id);
                    affected.insert(placement.internal_id);
                    reused_uid = Some(placement.placement_uid);
                    false
                } else {
                    true
                }
            });
        }
        let placement_uid = match reused_uid {
            Some(uid) => uid,
            None => self.kitty_image_store.borrow_mut().next_placement_uid(),
        };
        self.placements.push(KittyPlacement {
            image_id: pane_image_id,
            placement_id,
            placement_uid,
            internal_id: internal,
            display_rect,
            position_cell_size: cell,
            source_rect: PixelRect {
                x: source_x,
                y: source_y as isize,
                width: source_w,
                height: source_h,
            },
            emit_x: 0,
            emit_y: 0,
            scaled_px: if is_scaled {
                Some((dst_w, dst_h))
            } else {
                None
            },
            dest_cells,
            variant_key,
            cell_offset: (off_x as u32, off_y as u32),
            z_index: command.z_index,
            vertical_anchor,
            is_placeholder: false,
            relative_to,
        });
        self.kitty_image_store
            .borrow_mut()
            .add_placement_ref(internal);
        self.reclaim_scaled_variants(affected);
        if relative_to.is_some() {
            // Spec, "Relative placements": "the cursor must not move after a relative position,
            // regardless of the value of the `C` key to control cursor movement". Reporting no
            // cells is how the caller is told there is nothing to advance past.
            return Ok(None);
        }
        Ok(Some(dest_cells))
    }
    /// Check a `P`/`Q` pair over against everything already placed, and turn it into the parent
    /// link the new placement will carry.
    ///
    /// Spec, "Relative placements": the four ways this can fail are a virtual placement asking to
    /// be relative and a placement made its own parent (both `EINVAL`), a parent that does not
    /// exist (`ENOPARENT`), a chain that loops (`ECYCLE`) and one that is too long (`ETOODEEP`).
    fn validate_relative_parent(
        &self,
        command: &KittyCommand,
        pane_image_id: u32,
        placement_id: u32,
    ) -> Result<Option<RelativeParent>, KittyError> {
        let parent_image_id = match command.parent_image_id {
            Some(parent_image_id) => parent_image_id,
            None => return Ok(None),
        };
        // Spec: "Virtual placements created for Unicode placeholder based images cannot also be
        // relative placements. [...] the terminal must respond with the EINVAL error code."
        if command.virtual_placement {
            return Err(relative_error(
                command,
                KittyErrorCode::Einval,
                "a virtual placement cannot be relative",
            ));
        }
        let parent = RelativeParent {
            image_id: parent_image_id,
            placement_id: command.parent_placement_id,
            offset_cells: command.relative_offset_cells,
        };
        if parent.matches(pane_image_id, placement_id) {
            return Err(relative_error(
                command,
                KittyErrorCode::Einval,
                "a placement cannot be its own parent",
            ));
        }
        if !self.relative_parent_exists(&parent) {
            return Err(relative_error(
                command,
                KittyErrorCode::Enoparent,
                "no such parent placement",
            ));
        }
        // Walk up from the parent. Reaching the placement being created closes a loop; running
        // past the depth limit means the chain is longer than this terminal resolves.
        let mut ancestor = self.relative_parent_of(&parent);
        for _ in 0..MAX_RELATIVE_DEPTH {
            let link = match ancestor {
                Some(link) => link,
                None => return Ok(Some(parent)),
            };
            if link.matches(pane_image_id, placement_id) {
                return Err(relative_error(
                    command,
                    KittyErrorCode::Ecycle,
                    "relative placements form a cycle",
                ));
            }
            ancestor = self.relative_parent_of(&link);
        }
        Err(relative_error(
            command,
            KittyErrorCode::Etoodeep,
            "relative placement chain is too deep",
        ))
    }
    /// Whether anything a `P`/`Q` pair could name is currently placed. A virtual placement counts
    /// even before any placeholder cells have instantiated it — it is a placement, it just has no
    /// position yet.
    fn relative_parent_exists(&self, parent: &RelativeParent) -> bool {
        self.placements
            .iter()
            .any(|placement| parent.matches(placement.image_id, placement.placement_id))
            || self.virtual_placements.iter().any(|virtual_placement| {
                parent.matches(virtual_placement.image_id, virtual_placement.placement_id)
            })
    }
    /// The parent of the placement `parent` names, for walking a chain upwards.
    fn relative_parent_of(&self, parent: &RelativeParent) -> Option<RelativeParent> {
        self.placements
            .iter()
            .find(|placement| parent.matches(placement.image_id, placement.placement_id))
            .and_then(|placement| placement.relative_to)
    }
    /// Where a placement hanging off `parent` starts, in absolute pixels.
    ///
    /// A virtual parent has no rect of its own. Spec: "When a virtual placement is the parent, its
    /// position is derived from all the actual Unicode placeholder images that refer to it. The x
    /// position is the minimum of all the placeholder x positions and the y position is the
    /// minimum of all the placeholder y positions." That is what makes a one-pixel placeholder a
    /// movable anchor for a real image.
    fn relative_origin_px(
        &self,
        parent: &RelativeParent,
        cell: SizeInPixels,
    ) -> Option<(usize, isize)> {
        let mut origin: Option<(usize, isize)> = None;
        for placement in self
            .placements
            .iter()
            .filter(|placement| parent.matches(placement.image_id, placement.placement_id))
        {
            let (x, y) = (placement.display_rect.x, placement.display_rect.y);
            origin = Some(match origin {
                Some((min_x, min_y)) => (std::cmp::min(min_x, x), std::cmp::min(min_y, y)),
                None => (x, y),
            });
        }
        let (parent_x, parent_y) = origin?;
        let (offset_columns, offset_rows) = parent.offset_cells;
        let x = (parent_x as isize + offset_columns as isize * cell.width as isize).max(0) as usize;
        let y = parent_y + offset_rows as isize * cell.height as isize;
        Some((x, y))
    }
    /// Reposition every relative placement from its parent.
    ///
    /// Parents move: the grid scrolls, lines reflow, and a virtual parent's position comes from
    /// placeholder cells that are rebuilt every frame. Rather than hook each of those, the whole
    /// set is re-derived, repeatedly so a chain settles from the root down. `placement_uid` is
    /// deliberately left alone — the host-side diff in `output/mod.rs` reads a new uid as "this is
    /// a different image, retransmit it", and a placement that merely moved is not.
    pub fn resolve_relative_placements(&mut self, cell: SizeInPixels) {
        if !self.has_relative_placements() {
            return;
        }
        for _ in 0..MAX_RELATIVE_DEPTH {
            let mut moved = false;
            for index in 0..self.placements.len() {
                let parent = match self.placements[index].relative_to {
                    Some(parent) => parent,
                    None => continue,
                };
                if let Some((x, y)) = self.relative_origin_px(&parent, cell) {
                    let rect = &mut self.placements[index].display_rect;
                    if rect.x != x || rect.y != y {
                        rect.x = x;
                        rect.y = y;
                        moved = true;
                    }
                }
            }
            if !moved {
                return;
            }
        }
    }
    /// Drop every placement whose parent has gone, and keep dropping until nothing more does.
    ///
    /// Spec, "Relative placements": "The lifetime of a relative placement is tied to the lifetime
    /// of its parent. If its parent is deleted, it is deleted as well." A child has no position of
    /// its own to fall back on, so leaving it behind would pin it wherever its parent last was.
    ///
    /// `free_image_data` follows the case of the delete specifier that started the cascade: an
    /// uppercase delete frees the pixels, so a child image left with no placements at all goes with
    /// them, while a lowercase one only drops the placement and leaves the image for re-display.
    fn cascade_relative_deletes(&mut self, free_image_data: bool) {
        if !self.has_relative_placements() {
            return;
        }
        for _ in 0..MAX_RELATIVE_DEPTH {
            let orphaned: Vec<usize> = self
                .placements
                .iter()
                .enumerate()
                .filter(|(_, placement)| match &placement.relative_to {
                    Some(parent) => !self.relative_parent_exists(parent),
                    None => false,
                })
                .map(|(index, _)| index)
                .collect();
            if orphaned.is_empty() {
                return;
            }
            let mut store = self.kitty_image_store.borrow_mut();
            let mut affected = HashSet::new();
            for index in orphaned.into_iter().rev() {
                let placement = self.placements.remove(index);
                affected.insert(placement.internal_id);
                if free_image_data {
                    store.free(placement.internal_id);
                } else {
                    store.remove_placement_ref(placement.internal_id);
                }
            }
            drop(store);
            self.reclaim_scaled_variants(affected);
        }
    }
    pub fn has_relative_placements(&self) -> bool {
        self.placements
            .iter()
            .any(|placement| placement.relative_to.is_some())
    }
    /// Create the prototype a `U=1` command asks for: crop the source rectangle, fit it into the
    /// `c` x `r` cell box preserving its aspect ratio. Runs share the crop and fitted-size key.
    fn place_virtual(
        &mut self,
        pane_image_id: u32,
        internal: InternalImageId,
        command: &KittyCommand,
        cell: Option<SizeInPixels>,
        source: (usize, usize, usize, usize),
    ) -> Result<(), KittyError> {
        let (_, _, source_w, source_h) = source;
        let (columns, rows, fitted_px) = if let Some(cell) = cell {
            let (columns, rows) =
                virtual_dimensions(command.columns, command.rows, source_w, source_h, cell);
            let box_px = (columns * cell.width, rows * cell.height);
            // Spec: "The image will eventually be fit to the specified rectangle, its aspect ratio
            // preserved" - so a square image in a wide box keeps its shape rather than stretching.
            let fitted_px = fit_into_box((source_w, source_h), box_px);
            (columns, rows, fitted_px)
        } else {
            (command.columns as usize, command.rows as usize, (0, 0))
        };
        let placement_id = if pane_image_id != 0 {
            command.placement_id.unwrap_or(0)
        } else {
            0
        };
        let prototype = VirtualPlacement {
            image_id: pane_image_id,
            placement_id,
            internal_id: internal,
            columns: columns as u16,
            rows: rows as u16,
            source,
            fitted_px,
            pending_geometry: cell.is_none().then_some((command.columns, command.rows)),
            z_index: command.z_index,
        };
        let mut live = self.live_scaled_keys(
            internal,
            |p| {
                !(p.is_placeholder && p.image_id == pane_image_id && p.placement_id == placement_id)
            },
            |p| !(p.image_id == pane_image_id && p.placement_id == placement_id),
        );
        let required = cell.map(|_| prototype.variant_key());
        live.extend(required);
        let mut store = self.kitty_image_store.borrow_mut();
        if !store.retain_scaled_variants(internal, &live, required) {
            return Err(enospc(command, "scaled image exceeds storage quota"));
        }
        let mut affected = HashSet::new();
        self.virtual_placements.retain(|p| {
            if p.image_id == pane_image_id && p.placement_id == placement_id {
                store.remove_placement_ref(p.internal_id);
                affected.insert(p.internal_id);
                false
            } else {
                true
            }
        });
        // The terminal grid refreshes these slices from the committed prototype.
        self.placements.retain(|p| {
            if p.is_placeholder && p.image_id == pane_image_id && p.placement_id == placement_id {
                store.remove_placement_ref(p.internal_id);
                affected.insert(p.internal_id);
                false
            } else {
                true
            }
        });
        self.virtual_placements.push(prototype);
        store.add_placement_ref(internal);
        drop(store);
        self.reclaim_scaled_variants(affected);
        Ok(())
    }
    pub fn virtual_placements(&self) -> &[VirtualPlacement] {
        &self.virtual_placements
    }
    pub fn has_virtual_placements(&self) -> bool {
        !self.virtual_placements.is_empty()
    }
    /// Drop the virtual placements matching `matches`, releasing the store reference each holds.
    fn remove_virtual_placements<F: FnMut(&VirtualPlacement) -> bool>(&mut self, mut matches: F) {
        if self.virtual_placements.is_empty() {
            return;
        }
        let mut store = self.kitty_image_store.borrow_mut();
        let mut affected = HashSet::new();
        let mut kept = Vec::with_capacity(self.virtual_placements.len());
        for virtual_placement in self.virtual_placements.drain(..) {
            if matches(&virtual_placement) {
                affected.insert(virtual_placement.internal_id);
                store.remove_placement_ref(virtual_placement.internal_id);
            } else {
                kept.push(virtual_placement);
            }
        }
        self.virtual_placements = kept;
        drop(store);
        self.reclaim_scaled_variants(affected);
    }
    /// Replace the placements derived from unicode placeholder cells with a freshly resolved set.
    ///
    /// The grid rebuilds these from its own contents rather than mutating them in step with every
    /// edit, so erasing a line, scrolling, resizing and reflowing all come out right without a
    /// hook in any of them: the cells are the state.
    pub fn set_placeholder_placements(&mut self, derived: Vec<KittyPlacement>) {
        if self.placements.iter().all(|p| !p.is_placeholder) && derived.is_empty() {
            return;
        }
        let mut store = self.kitty_image_store.borrow_mut();
        let mut affected = HashSet::new();
        let mut kept = Vec::with_capacity(self.placements.len());
        for placement in self.placements.drain(..) {
            if placement.is_placeholder {
                affected.insert(placement.internal_id);
                store.remove_placement_ref(placement.internal_id);
            } else {
                kept.push(placement);
            }
        }
        for placement in &derived {
            store.add_placement_ref(placement.internal_id);
        }
        drop(store);
        kept.extend(derived);
        self.placements = kept;
        self.reclaim_scaled_variants(affected);
    }
    /// A `placement_uid` that stays the same for as long as this run of placeholder cells does.
    pub fn placeholder_uid(&mut self, key: (u32, u32, usize, usize)) -> u64 {
        if let Some(uid) = self.placeholder_uids.get(&key) {
            return *uid;
        }
        let uid = self.kitty_image_store.borrow_mut().next_placement_uid();
        self.placeholder_uids.insert(key, uid);
        uid
    }
    /// Forget the uids of runs that no longer exist, so a pane that has scrolled through a lot of
    /// placeholder content does not accumulate them forever.
    pub fn retain_placeholder_uids(
        &mut self,
        live: &std::collections::HashSet<(u32, u32, usize, usize)>,
    ) {
        self.placeholder_uids.retain(|key, _| live.contains(key));
    }
    pub fn delete(
        &mut self,
        command: &KittyCommand,
        cursor_cell: (usize, usize),
        viewport_cells: (usize, usize),
        scrollback_rows: usize,
    ) -> Result<(), KittyError> {
        let specifier = command.delete_specifier.unwrap_or('a');
        let uppercase = specifier.is_ascii_uppercase();
        let lower = specifier.to_ascii_lowercase();
        // `f` names frames rather than a region of the screen, so none of the geometry below
        // applies to it.
        if lower == 'f' {
            return self.delete_frames(command, uppercase);
        }
        let cell = *self.character_cell_size.borrow();
        let geometry_needed = matches!(lower, 'a' | 'c' | 'p' | 'q' | 'x' | 'y');
        let cell = match (cell, geometry_needed) {
            (Some(cell), _) => cell,
            (None, true) => return Ok(()),
            (None, false) => SizeInPixels {
                width: 1,
                height: 1,
            },
        };
        let cell_w = cell.width;
        let cell_h = cell.height;
        let la_px = scrollback_rows * cell_h;
        let probe_rect = |x: usize, y: usize, width: usize, height: usize| PixelRect {
            x,
            y: y as isize,
            width,
            height,
        };
        let target_image_id: Option<u32> = match lower {
            'i' => Some(
                command
                    .image_id
                    .ok_or_else(|| einval(command, "missing image id for delete"))?,
            ),
            'n' => {
                let image_number = command
                    .image_number
                    .ok_or_else(|| einval(command, "missing image number for delete"))?;
                match self
                    .image_numbers
                    .get(&image_number)
                    .and_then(|ids| ids.last())
                {
                    Some(id) => Some(*id),
                    None => return Ok(()),
                }
            },
            _ => None,
        };
        let column_probe_x = |source_x: u32| -> Option<usize> {
            if source_x == 0 {
                None
            } else {
                Some((source_x as usize - 1) * cell_w)
            }
        };
        let row_probe_y = |source_y: u32| -> Option<usize> {
            if source_y == 0 {
                None
            } else {
                Some(la_px + (source_y as usize - 1) * cell_h)
            }
        };
        let matches_placement = |placement: &KittyPlacement| -> bool {
            match lower {
                'a' => placement
                    .display_rect
                    .intersecting_rect(&probe_rect(
                        0,
                        la_px,
                        viewport_cells.0 * cell_w,
                        viewport_cells.1 * cell_h,
                    ))
                    .is_some(),
                'i' | 'n' => {
                    placement.image_id == target_image_id.unwrap()
                        && command
                            .placement_id
                            .map(|placement_id| placement.placement_id == placement_id)
                            .unwrap_or(true)
                },
                'c' => placement
                    .display_rect
                    .intersecting_rect(&probe_rect(
                        cursor_cell.0 * cell_w,
                        la_px + cursor_cell.1 * cell_h,
                        cell_w,
                        cell_h,
                    ))
                    .is_some(),
                'p' => match (
                    column_probe_x(command.source_x),
                    row_probe_y(command.source_y),
                ) {
                    (Some(px), Some(py)) => placement
                        .display_rect
                        .intersecting_rect(&probe_rect(px, py, cell_w, cell_h))
                        .is_some(),
                    _ => false,
                },
                'q' => {
                    placement.z_index == command.z_index
                        && match (
                            column_probe_x(command.source_x),
                            row_probe_y(command.source_y),
                        ) {
                            (Some(px), Some(py)) => placement
                                .display_rect
                                .intersecting_rect(&probe_rect(px, py, cell_w, cell_h))
                                .is_some(),
                            _ => false,
                        }
                },
                'x' => match column_probe_x(command.source_x) {
                    Some(px) => placement
                        .display_rect
                        .intersecting_rect(&probe_rect(px, 0, cell_w, usize::MAX / 2))
                        .is_some(),
                    None => false,
                },
                'y' => match row_probe_y(command.source_y) {
                    Some(py) => placement
                        .display_rect
                        .intersecting_rect(&probe_rect(0, py, usize::MAX / 2, cell_h))
                        .is_some(),
                    None => false,
                },
                'z' => placement.z_index == command.z_index,
                'r' => {
                    placement.image_id >= command.source_x && placement.image_id <= command.source_y
                },
                _ => false,
            }
        };
        // Spec, "Unicode placeholders": "Virtual placements can be deleted by a deletion command
        // only when the `d` key is equal to `i`, `I`, `r`, `R`, `n` or `N`." The rest are
        // positional, and a virtual placement has no position to select it by.
        if matches!(lower, 'i' | 'n' | 'r') {
            let placement_id = command.placement_id;
            let (id_low, id_high) = (command.source_x, command.source_y);
            self.remove_virtual_placements(|virtual_placement| match lower {
                'r' => {
                    virtual_placement.image_id >= id_low && virtual_placement.image_id <= id_high
                },
                _ => match target_image_id {
                    Some(target) => {
                        virtual_placement.image_id == target
                            && placement_id
                                .map(|id| virtual_placement.placement_id == id)
                                .unwrap_or(true)
                    },
                    None => false,
                },
            });
        }
        let mut affected = HashSet::new();
        {
            let mut store = self.kitty_image_store.borrow_mut();
            let mut kept = Vec::with_capacity(self.placements.len());
            for placement in self.placements.drain(..) {
                if matches_placement(&placement) {
                    affected.insert(placement.internal_id);
                    if uppercase {
                        store.free(placement.internal_id);
                    } else {
                        store.remove_placement_ref(placement.internal_id);
                    }
                } else {
                    kept.push(placement);
                }
            }
            self.placements = kept;
        }
        self.cascade_relative_deletes(uppercase);
        self.reclaim_scaled_variants(affected);
        if uppercase {
            match lower {
                'i' | 'n' => {
                    let target = target_image_id.unwrap();
                    let no_surviving = !self
                        .placements
                        .iter()
                        .any(|placement| placement.image_id == target)
                        && !self
                            .virtual_placements
                            .iter()
                            .any(|virtual_placement| virtual_placement.image_id == target);
                    if no_surviving && let Some(internal) = self.image_ids.get(&target).copied() {
                        self.kitty_image_store.borrow_mut().free(internal);
                    }
                },
                'r' => {
                    let in_range: Vec<(u32, InternalImageId)> = self
                        .image_ids
                        .iter()
                        .filter(|(id, _)| **id >= command.source_x && **id <= command.source_y)
                        .map(|(id, internal)| (*id, *internal))
                        .collect();
                    let mut store = self.kitty_image_store.borrow_mut();
                    for (_, internal) in in_range {
                        if store.refcount(internal) == Some(0) {
                            store.free(internal);
                        }
                    }
                },
                _ => {},
            }
            let store = self.kitty_image_store.borrow();
            self.image_ids
                .retain(|_, internal| store.get(*internal).is_some());
            drop(store);
            let live_ids: Vec<u32> = self.image_ids.keys().copied().collect();
            self.image_numbers.values_mut().for_each(|ids| {
                ids.retain(|id| live_ids.contains(id));
            });
            self.image_numbers.retain(|_, ids| !ids.is_empty());
        }
        Ok(())
    }
    pub fn note_rows_below_the_viewport(&mut self, frame: Option<KittyRowsBelowTheViewport>) {
        self.rows_below_the_viewport = frame;
    }
    pub fn note_rows_below_the_viewport_shifted(&mut self, row_delta: isize) {
        self.rows_below_the_viewport =
            self.rows_below_the_viewport
                .map(|frame| KittyRowsBelowTheViewport {
                    first_row: std::cmp::max(0, frame.first_row as isize + row_delta) as usize,
                    total_rows: std::cmp::max(0, frame.total_rows as isize + row_delta) as usize,
                });
    }
    pub fn settle_placements_below_the_viewport(
        &mut self,
        frame: Option<KittyRowsBelowTheViewport>,
    ) -> bool {
        let previous = std::mem::replace(&mut self.rows_below_the_viewport, frame);
        let (previous, frame) = match (previous, frame) {
            (Some(previous), Some(frame)) => (previous, frame),
            _ => return false,
        };
        let row_delta = frame.total_rows as isize - previous.total_rows as isize;
        if row_delta == 0 || self.placements.is_empty() {
            return false;
        }
        let cell = match *self.character_cell_size.borrow() {
            Some(cell) => cell,
            None => return false,
        };
        let first_row_px = previous.first_row as isize * cell.height as isize;
        let delta_px = row_delta * cell.height as isize;
        let mut settled = false;
        for placement in self.placements.iter_mut() {
            if placement.display_rect.y >= first_row_px {
                placement.display_rect.y += delta_px;
                settled = true;
            }
        }
        settled
    }
    pub fn offset_grid_top(&mut self) {
        self.front_drops += 1;
        if let Some(cell) = *self.character_cell_size.borrow() {
            self.rows_below_the_viewport =
                self.rows_below_the_viewport
                    .map(|frame| KittyRowsBelowTheViewport {
                        first_row: std::cmp::max(0, frame.first_row as isize + -1) as usize,
                        total_rows: std::cmp::max(0, frame.total_rows as isize + -1) as usize,
                    });
            let height_to_reduce = cell.height as isize;
            let mut store = self.kitty_image_store.borrow_mut();
            let mut affected = HashSet::new();
            let mut kept = Vec::with_capacity(self.placements.len());
            for mut placement in self.placements.drain(..) {
                placement.display_rect.y -= height_to_reduce;
                if placement.display_rect.y + placement.display_rect.height as isize <= 0 {
                    affected.insert(placement.internal_id);
                    store.free(placement.internal_id);
                } else {
                    kept.push(placement);
                }
            }
            self.placements = kept;
            drop(store);
            self.reclaim_scaled_variants(affected);
        }
    }
    /// The scrollback has been discarded wholesale (`ESC[3J`). Placements that lived only up there
    /// have nowhere left to be anchored and are freed; everything still on screen simply moves up
    /// by the rows that went away.
    ///
    /// Spec, "Interaction with other terminal actions": erase commands other than `ESC[2J` "must
    /// have no effect on graphics", so this must not become a `clear_all_placements` in disguise.
    pub fn drop_scrollback_rows(&mut self, dropped_row_count: usize) {
        if dropped_row_count == 0 {
            return;
        }
        if let Some(cell) = *self.character_cell_size.borrow() {
            self.rows_below_the_viewport =
                self.rows_below_the_viewport
                    .map(|frame| KittyRowsBelowTheViewport {
                        first_row: frame.first_row.saturating_sub(dropped_row_count),
                        total_rows: frame.total_rows.saturating_sub(dropped_row_count),
                    });
            let height_to_reduce = (dropped_row_count * cell.height) as isize;
            let mut store = self.kitty_image_store.borrow_mut();
            let mut affected = HashSet::new();
            let mut kept = Vec::with_capacity(self.placements.len());
            for mut placement in self.placements.drain(..) {
                placement.display_rect.y -= height_to_reduce;
                if placement.display_rect.y + placement.display_rect.height as isize <= 0 {
                    affected.insert(placement.internal_id);
                    store.free(placement.internal_id);
                } else {
                    kept.push(placement);
                }
            }
            self.placements = kept;
            drop(store);
            self.reclaim_scaled_variants(affected);
        }
    }
    pub fn merge_rows_into_line_start(&mut self, line_start_row: usize, merged_row_count: usize) {
        if merged_row_count == 0 {
            return;
        }
        if let Some(cell) = *self.character_cell_size.borrow() {
            self.merged_rows += merged_row_count as u64;
            self.rows_below_the_viewport =
                self.rows_below_the_viewport
                    .map(|frame| KittyRowsBelowTheViewport {
                        first_row: std::cmp::max(
                            0,
                            frame.first_row as isize + -(merged_row_count as isize),
                        ) as usize,
                        total_rows: std::cmp::max(
                            0,
                            frame.total_rows as isize + -(merged_row_count as isize),
                        ) as usize,
                    });
            let line_start_px = line_start_row as isize * cell.height as isize;
            let merged_px = merged_row_count as isize * cell.height as isize;
            for placement in self.placements.iter_mut() {
                if placement.display_rect.y >= line_start_px {
                    placement.display_rect.y =
                        std::cmp::max(line_start_px, placement.display_rect.y - merged_px);
                }
            }
        }
    }
    pub fn split_line_start_into_rows(&mut self, line_start_row: usize, split_row_count: usize) {
        if split_row_count == 0 {
            return;
        }
        if let Some(cell) = *self.character_cell_size.borrow() {
            self.split_rows += split_row_count as u64;
            self.rows_below_the_viewport =
                self.rows_below_the_viewport
                    .map(|frame| KittyRowsBelowTheViewport {
                        first_row: std::cmp::max(
                            0,
                            frame.first_row as isize + split_row_count as isize,
                        ) as usize,
                        total_rows: std::cmp::max(
                            0,
                            frame.total_rows as isize + split_row_count as isize,
                        ) as usize,
                    });
            let line_start_px = line_start_row as isize * cell.height as isize;
            let split_px = split_row_count as isize * cell.height as isize;
            for placement in self.placements.iter_mut() {
                if placement.display_rect.y >= line_start_px + cell.height as isize {
                    placement.display_rect.y += split_px;
                }
            }
        }
    }
    pub fn has_placement_anchored_to_first_canonical_line(&self) -> bool {
        self.placements
            .iter()
            .any(|placement| placement.vertical_anchor.canonical_line == 0)
    }
    pub fn drop_first_canonical_anchor_line(&mut self, first_canonical_row: usize) {
        let cell_height = match *self.character_cell_size.borrow() {
            Some(cell) => cell.height as isize,
            None => return,
        };
        let first_line_start_px = first_canonical_row as isize * cell_height;
        for placement in self.placements.iter_mut() {
            if placement.vertical_anchor.canonical_line == 0 {
                placement.vertical_anchor.offset_px_from_line_start =
                    placement.display_rect.y - first_line_start_px;
            } else {
                placement.vertical_anchor.canonical_line -= 1;
            }
        }
    }
    pub fn insert_canonical_anchor_line_at_front(&mut self) {
        for placement in self.placements.iter_mut() {
            placement.vertical_anchor.canonical_line += 1;
        }
    }
    pub fn apply_region_scroll(
        &mut self,
        region_top_px: isize,
        region_bottom_px: isize,
        n_px: isize,
        la_delta_px: isize,
        viewport_top_px: isize,
        preserve_above_region_top: bool,
    ) {
        let delta_inside = la_delta_px - n_px;
        let delta_outside = la_delta_px;
        let shifted_region_top = region_top_px + la_delta_px;
        let shifted_region_bottom = region_bottom_px + la_delta_px;
        let mut store = self.kitty_image_store.borrow_mut();
        let mut affected = HashSet::new();
        let mut kept = Vec::with_capacity(self.placements.len());
        for mut placement in self.placements.drain(..) {
            let top = placement.display_rect.y;
            let bottom = top + placement.display_rect.height as isize;
            if bottom <= viewport_top_px {
                kept.push(placement);
                continue;
            }
            let entirely_inside = top >= region_top_px && bottom <= region_bottom_px;
            let intersects_region = top < region_bottom_px && bottom > region_top_px;
            if entirely_inside {
                placement.display_rect.y += delta_inside;
                let new_top = placement.display_rect.y;
                let new_bottom = new_top + placement.display_rect.height as isize;
                if new_top >= shifted_region_bottom
                    || (!preserve_above_region_top && new_bottom <= shifted_region_top)
                {
                    affected.insert(placement.internal_id);
                    store.free(placement.internal_id);
                    continue;
                }
                if !preserve_above_region_top && new_top < shifted_region_top {
                    let cut = (shifted_region_top - new_top) as usize;
                    placement.emit_y += cut;
                    placement.display_rect.y = shifted_region_top;
                    placement.display_rect.height -= cut;
                }
                let new_bottom = placement.display_rect.y + placement.display_rect.height as isize;
                if new_bottom > shifted_region_bottom {
                    placement.display_rect.height -= (new_bottom - shifted_region_bottom) as usize;
                }
                if placement.display_rect.height == 0 {
                    affected.insert(placement.internal_id);
                    store.free(placement.internal_id);
                    continue;
                }
                kept.push(placement);
            } else if intersects_region {
                if preserve_above_region_top && top < region_top_px {
                    placement.display_rect.y += delta_inside;
                    kept.push(placement);
                    continue;
                }
                placement.display_rect.y += delta_outside;
                let new_top = placement.display_rect.y;
                let new_bottom = new_top + placement.display_rect.height as isize;
                if new_top < shifted_region_top {
                    placement.display_rect.height = (shifted_region_top - new_top) as usize;
                } else if new_bottom > shifted_region_bottom {
                    let cut = (shifted_region_bottom - new_top) as usize;
                    placement.emit_y += cut;
                    placement.display_rect.y = shifted_region_bottom;
                    placement.display_rect.height -= cut;
                } else {
                    affected.insert(placement.internal_id);
                    store.free(placement.internal_id);
                    continue;
                }
                if placement.display_rect.height == 0 {
                    affected.insert(placement.internal_id);
                    store.free(placement.internal_id);
                    continue;
                }
                kept.push(placement);
            } else {
                placement.display_rect.y += delta_outside;
                kept.push(placement);
            }
        }
        self.placements = kept;
        drop(store);
        self.reclaim_scaled_variants(affected);
    }
    pub fn character_cell_size_possibly_changed(&mut self) {
        let Some(cell) = *self.character_cell_size.borrow() else {
            return;
        };
        if self.previous_cell_size == Some(cell) {
            return;
        }
        let mut placements = self.placements.clone();
        let mut virtual_placements = self.virtual_placements.clone();
        let mut keys: HashMap<InternalImageId, HashSet<ScaledImageKey>> = HashMap::new();
        for placement in &mut placements {
            if placement.is_placeholder {
                continue;
            }
            let previous = placement.position_cell_size;
            if previous.width != cell.width {
                placement.display_rect.x = placement.display_rect.x / previous.width * cell.width;
            }
            if previous.height != cell.height {
                placement.display_rect.y =
                    placement.display_rect.y / previous.height as isize * cell.height as isize;
            }
            placement.position_cell_size = cell;
            if placement.scaled_px.is_some() {
                let (Some(width), Some(height)) = (
                    (placement.dest_cells.0 as usize).checked_mul(cell.width),
                    (placement.dest_cells.1 as usize).checked_mul(cell.height),
                ) else {
                    return;
                };
                let size = (width, height);
                placement.variant_key = ScaledImageKey {
                    source: placement.source_rect,
                    size,
                };
                placement.scaled_px = Some(size);
                placement.display_rect.width = size.0;
                placement.display_rect.height = size.1;
                placement.emit_x = 0;
                placement.emit_y = 0;
                keys.entry(placement.internal_id)
                    .or_default()
                    .insert(placement.variant_key);
            }
        }
        for placement in &mut virtual_placements {
            let (_, _, width, height) = placement.source;
            if let Some((columns, rows)) = placement.pending_geometry.take() {
                let (columns, rows) = virtual_dimensions(columns, rows, width, height, cell);
                placement.columns = columns as u16;
                placement.rows = rows as u16;
            }
            let (Some(box_width), Some(box_height)) = (
                (placement.columns as usize).checked_mul(cell.width),
                (placement.rows as usize).checked_mul(cell.height),
            ) else {
                return;
            };
            placement.fitted_px = fit_into_box((width, height), (box_width, box_height));
            keys.entry(placement.internal_id)
                .or_default()
                .insert(placement.variant_key());
        }
        {
            let store = self.kitty_image_store.borrow();
            for &(id, key) in &self.completed_frame_keys {
                if let Some(key) = key.filter(|_| store.get(id).is_some()) {
                    keys.entry(id).or_default().insert(key);
                }
            }
        }
        if !self
            .kitty_image_store
            .borrow_mut()
            .replace_scaled_variants(keys)
        {
            return;
        }
        // Placeholder slices are rebuilt by the terminal grid from the committed prototypes.
        self.placements = placements;
        self.virtual_placements = virtual_placements;
        self.set_placeholder_placements(Vec::new());
        self.previous_cell_size = Some(cell);
    }
    /// Release this grid's namespace when it is permanently discarded. Clones are snapshots,
    /// so ownership cleanup is explicit rather than an implicit `Drop` implementation.
    pub fn teardown(&mut self) {
        self.release_completed_frame();
        self.clear_all_placements();
        self.remove_virtual_placements(|_| true);
        let mut store = self.kitty_image_store.borrow_mut();
        for (_, internal) in self.image_ids.drain() {
            store.remove_entry(internal);
        }
        for internal in self.anonymous_image_ids.drain(..) {
            store.remove_entry(internal);
        }
        self.image_numbers.clear();
        self.placeholder_uids.clear();
    }

    pub fn clear_all_placements(&mut self) {
        let mut store = self.kitty_image_store.borrow_mut();
        let mut affected = HashSet::new();
        for placement in self.placements.drain(..) {
            affected.insert(placement.internal_id);
            store.free(placement.internal_id);
        }
        drop(store);
        self.reclaim_scaled_variants(affected);
    }
    pub fn clear_visible_placements(&mut self, scrollback_size_in_lines: usize) {
        let cell_size = { *self.character_cell_size.borrow() };
        let cell_height = match cell_size {
            Some(cell) => cell.height as isize,
            None => {
                self.clear_all_placements();
                return;
            },
        };
        let scrollback_top = scrollback_size_in_lines as isize * cell_height;
        let mut store = self.kitty_image_store.borrow_mut();
        let mut affected = HashSet::new();
        let mut kept = Vec::with_capacity(self.placements.len());
        for placement in self.placements.drain(..) {
            let screen_relative_top_row =
                (placement.display_rect.y - scrollback_top).div_euclid(cell_height);
            let dest_rows = placement.dest_cells.1 as isize;
            if screen_relative_top_row + dest_rows > 0 {
                affected.insert(placement.internal_id);
                store.free(placement.internal_id);
            } else {
                kept.push(placement);
            }
        }
        self.placements = kept;
        drop(store);
        self.reclaim_scaled_variants(affected);
    }
    pub fn image_cell_coordinates_in_viewport(
        &self,
        viewport_height: usize,
        scrollback_height: usize,
    ) -> Vec<(usize, usize, usize, usize)> {
        match *self.character_cell_size.borrow() {
            Some(character_cell_size) => self
                .placements
                .iter()
                .map(|placement| {
                    let pixel_rect = &placement.display_rect;
                    let scrollback_size_in_pixels = scrollback_height * character_cell_size.height;
                    let y_pixel_coordinates_in_viewport =
                        pixel_rect.y - scrollback_size_in_pixels as isize;
                    let image_y = std::cmp::max(y_pixel_coordinates_in_viewport, 0) as usize
                        / character_cell_size.height;
                    let image_x = pixel_rect.x / character_cell_size.width;
                    let image_height_in_pixels = if y_pixel_coordinates_in_viewport < 0 {
                        pixel_rect.height as isize + y_pixel_coordinates_in_viewport
                    } else {
                        pixel_rect.height as isize
                    };
                    let image_height = image_height_in_pixels as usize / character_cell_size.height;
                    let image_width = pixel_rect.width / character_cell_size.width;
                    let height_remainder = if !(image_height_in_pixels as usize)
                        .is_multiple_of(character_cell_size.height)
                    {
                        1
                    } else {
                        0
                    };
                    let width_remainder = if pixel_rect.width % character_cell_size.width > 0 {
                        1
                    } else {
                        0
                    };
                    let image_top_edge = image_y;
                    let image_bottom_edge =
                        std::cmp::min(image_y + image_height + height_remainder, viewport_height);
                    let image_left_edge = image_x;
                    let image_right_edge = image_x + image_width + width_remainder;
                    (
                        image_top_edge,
                        image_bottom_edge,
                        image_left_edge,
                        image_right_edge,
                    )
                })
                .collect(),
            None => vec![],
        }
    }
    pub fn viewport_kitty_chunks(
        &self,
        viewport_height: usize,
        scrollback_size_in_lines: usize,
        viewport_width_in_cells: usize,
        viewport_x_offset: usize,
        viewport_y_offset: usize,
    ) -> Vec<KittyImageChunk> {
        let mut full = HashMap::new();
        full.insert(0, viewport_height);
        self.changed_kitty_chunks_in_viewport(
            full,
            scrollback_size_in_lines,
            viewport_width_in_cells,
            viewport_x_offset,
            viewport_y_offset,
        )
    }
    pub fn changed_kitty_chunks_in_viewport(
        &self,
        changed_rects: HashMap<usize, usize>,
        scrollback_size_in_lines: usize,
        viewport_width_in_cells: usize,
        viewport_x_offset: usize,
        viewport_y_offset: usize,
    ) -> Vec<KittyImageChunk> {
        let mut changed_kitty_image_chunks = vec![];
        let character_cell_size = *self.character_cell_size.borrow();
        if let Some(character_cell_size) = character_cell_size {
            for placement in &self.placements {
                let image_pixel_rect = &placement.display_rect;
                for (line_index, line_count) in &changed_rects {
                    let changed_rect_pixel_height = line_count * character_cell_size.height;
                    let changed_rect_top_edge = ((line_index + scrollback_size_in_lines)
                        * character_cell_size.height)
                        as isize;
                    let changed_rect_bottom_edge =
                        changed_rect_top_edge + changed_rect_pixel_height as isize;
                    let image_top_edge = image_pixel_rect.y;
                    let image_bottom_edge = image_pixel_rect.y + image_pixel_rect.height as isize;

                    let cell_x_in_current_pane = image_pixel_rect.x / character_cell_size.width;
                    let cell_x = viewport_x_offset + cell_x_in_current_pane;
                    let image_pixel_width = if image_pixel_rect.x + image_pixel_rect.width
                        <= (viewport_width_in_cells * character_cell_size.width)
                    {
                        image_pixel_rect.width
                    } else {
                        (viewport_width_in_cells * character_cell_size.width)
                            .saturating_sub(image_pixel_rect.x)
                    };
                    if image_pixel_width == 0 {
                        continue;
                    }

                    let image_cell_distance_from_scrollback_top = std::cmp::max(
                        image_top_edge.div_euclid(character_cell_size.height as isize),
                        0,
                    ) as usize;
                    let image_cell_distance_from_changed_rect_top =
                        image_cell_distance_from_scrollback_top
                            .saturating_sub(line_index + scrollback_size_in_lines);
                    let cell_y =
                        viewport_y_offset + line_index + image_cell_distance_from_changed_rect_top;
                    let source_px_x = placement.emit_x
                        + if placement.scaled_px.is_none() {
                            placement.source_rect.x
                        } else {
                            0
                        };
                    let source_px_y = placement.emit_y
                        + if placement.scaled_px.is_none() {
                            placement.source_rect.y as usize
                        } else {
                            0
                        }
                        + std::cmp::max(changed_rect_top_edge - image_top_edge, 0) as usize;
                    let source_px_height = std::cmp::min(
                        (std::cmp::min(changed_rect_bottom_edge, image_bottom_edge)
                            - std::cmp::max(changed_rect_top_edge, image_top_edge))
                            as usize,
                        image_pixel_rect.height,
                    );

                    if (image_top_edge >= changed_rect_top_edge
                        && image_top_edge <= changed_rect_bottom_edge)
                        || (image_bottom_edge >= changed_rect_top_edge
                            && image_bottom_edge <= changed_rect_bottom_edge)
                        || (image_bottom_edge >= changed_rect_bottom_edge
                            && image_top_edge <= changed_rect_top_edge)
                    {
                        changed_kitty_image_chunks.push(KittyImageChunk {
                            cell_x,
                            cell_y,
                            internal_image_id: placement.internal_id,
                            source_px_x,
                            source_px_y,
                            source_px_width: image_pixel_width,
                            source_px_height,
                            cell_offset_x: placement.cell_offset.0,
                            cell_offset_y: placement.cell_offset.1,
                            z_index: placement.z_index,
                            dest_cells: placement.dest_cells,
                            variant_key: placement.variant_key,
                            scaled_px: placement.scaled_px,
                            placement_uid: placement.placement_uid,
                        });
                    }
                }
            }
        }
        changed_kitty_image_chunks
    }
}

/// A frame-sized field of one colour, the canvas a new animation frame starts from.
///
/// Spec, `a=f` key `Y`: "the background colour to use for the frame, in 32-bit RGBA format. The
/// default background colour when none is specified is 0 i.e. a black, transparent pixel."
fn background_canvas(background: u32, width: usize, height: usize) -> Vec<u8> {
    let pixel = background.to_be_bytes();
    pixel.repeat(width * height)
}

/// Draw `src`'s rectangle onto `dest`, either overwriting the destination pixels or alpha-blending
/// onto them.
///
/// Both rasters are RGBA and both rectangles are the same size; the caller has already checked
/// that they fit, so anything running past an edge here is clipped rather than wrapped.
#[allow(clippy::too_many_arguments)]
fn blit_rgba(
    dest: &mut [u8],
    dest_width: usize,
    dest_origin: (usize, usize),
    src: &[u8],
    src_size: (usize, usize),
    src_origin: (usize, usize),
    size: (usize, usize),
    overwrite: bool,
) {
    let (width, height) = size;
    for row in 0..height {
        let src_row = src_origin.1 + row;
        let dest_row = dest_origin.1 + row;
        if src_row >= src_size.1 {
            break;
        }
        for column in 0..width {
            let src_column = src_origin.0 + column;
            let dest_column = dest_origin.0 + column;
            if src_column >= src_size.0 || dest_column >= dest_width {
                break;
            }
            let src_index = (src_row * src_size.0 + src_column) * 4;
            let dest_index = (dest_row * dest_width + dest_column) * 4;
            if src_index + 4 > src.len() || dest_index + 4 > dest.len() {
                break;
            }
            if overwrite {
                dest[dest_index..dest_index + 4].copy_from_slice(&src[src_index..src_index + 4]);
                continue;
            }
            let alpha = src[src_index + 3] as u32;
            for channel in 0..3 {
                let over = src[src_index + channel] as u32;
                let under = dest[dest_index + channel] as u32;
                dest[dest_index + channel] =
                    ((over * alpha + under * (255 - alpha)) / 255).min(255) as u8;
            }
            dest[dest_index + 3] =
                (alpha + (dest[dest_index + 3] as u32) * (255 - alpha) / 255).min(255) as u8;
        }
    }
}

pub fn crop_rgba(
    src: &[u8],
    src_w: usize,
    src_h: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    // `src` may be shorter than `src_w * src_h * 4` claims, and `x` may point past `src_w`;
    // clamp to whole pixels so a malformed image truncates instead of panicking
    let src_limit = src.len() - src.len() % 4;
    for row in y..y + h {
        if row >= src_h {
            break;
        }
        let row_start = (row * src_w + x) * 4;
        let row_end = std::cmp::min(row_start + w * 4, (row * src_w + src_w) * 4).min(src_limit);
        if row_start >= row_end {
            continue;
        }
        out.extend_from_slice(&src[row_start..row_end]);
    }
    out
}

pub fn scale_rgba(src: &[u8], src_w: usize, src_h: usize, dst_w: usize, dst_h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(dst_w * dst_h * 4);
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return out;
    }
    for dy in 0..dst_h {
        let fy = (dy as f64 + 0.5) * (src_h as f64) / (dst_h as f64) - 0.5;
        let y0 = fy.floor().max(0.0).min((src_h - 1) as f64) as usize;
        let y1 = std::cmp::min(y0 + 1, src_h - 1);
        let ty = (fy - y0 as f64).clamp(0.0, 1.0);
        for dx in 0..dst_w {
            let fx = (dx as f64 + 0.5) * (src_w as f64) / (dst_w as f64) - 0.5;
            let x0 = fx.floor().max(0.0).min((src_w - 1) as f64) as usize;
            let x1 = std::cmp::min(x0 + 1, src_w - 1);
            let tx = (fx - x0 as f64).clamp(0.0, 1.0);
            for channel in 0..4 {
                let p00 = src[(y0 * src_w + x0) * 4 + channel] as f64;
                let p10 = src[(y0 * src_w + x1) * 4 + channel] as f64;
                let p01 = src[(y1 * src_w + x0) * 4 + channel] as f64;
                let p11 = src[(y1 * src_w + x1) * 4 + channel] as f64;
                let value =
                    (1.0 - ty) * ((1.0 - tx) * p00 + tx * p10) + ty * ((1.0 - tx) * p01 + tx * p11);
                out.push(value.round().clamp(0.0, 255.0) as u8);
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "./unit/grid_state_tests.rs"]
mod grid_state_tests;

#[cfg(test)]
#[path = "./unit/grid_state_spec_tests.rs"]
mod grid_state_spec_tests;
