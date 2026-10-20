use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use sixel_image::{SixelDeserializer, SixelImage};
use sixel_tokenizer::SixelEvent;

use std::fmt::Debug;

use yinz_types::pane_size::SizeInPixels;

const MAX_SIXEL_PIXELS: usize = 64 * 1024 * 1024;
const MAX_SIXEL_DIMENSION: usize = 16_384;

#[derive(Debug)]
struct SixelBounds {
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    max_height: Option<usize>,
    transparent: bool,
    clipped: bool,
}

impl SixelBounds {
    fn new(max_height: Option<usize>) -> Self {
        Self {
            width: 0,
            height: 1,
            x: 0,
            y: 0,
            max_height,
            transparent: false,
            clipped: false,
        }
    }

    // Check before decoding: sixel-image's height limit does not cover raster allocation.
    fn advance(&mut self, event: SixelEvent) -> Option<()> {
        if self.clipped {
            return Some(());
        }
        match event {
            SixelEvent::RasterAttribute { ph, pv, .. } => {
                if ph.unwrap_or(0) > MAX_SIXEL_DIMENSION || pv.unwrap_or(0) > MAX_SIXEL_DIMENSION {
                    return None;
                }
                if !self.transparent {
                    if let Some(pv) = pv.filter(|pv| *pv > self.height) {
                        // sixel-image 0.2.1 pads new rows to pv pixels wide, even without ph.
                        self.width = self.width.max(pv);
                        self.height = pv;
                    }
                    self.width = self.width.max(ph.unwrap_or(0));
                }
            },
            SixelEvent::Data { .. } | SixelEvent::Repeat { .. } => {
                let count = match event {
                    SixelEvent::Repeat { repeat_count, .. } => repeat_count,
                    _ => 1,
                };
                self.x = self.x.checked_add(count)?;
                self.width = self.width.max(self.x);
                self.height = self.height.max(self.y.checked_add(6)?);
            },
            SixelEvent::GotoBeginningOfLine => self.x = 0,
            SixelEvent::GotoNextLine => {
                let bottom = self.y.checked_add(12)?;
                if self
                    .max_height
                    .is_some_and(|max_height| bottom > max_height)
                {
                    self.clipped = true;
                    return Some(());
                }
                if bottom > MAX_SIXEL_DIMENSION {
                    return None;
                }
                self.y = self.y.checked_add(6)?;
                self.x = 0;
            },
            SixelEvent::Dcs {
                transparent_background,
                ..
            } => self.transparent |= transparent_background == Some(1),
            _ => {},
        }
        (self.width <= MAX_SIXEL_DIMENSION
            && self.height <= MAX_SIXEL_DIMENSION
            && self.width.checked_mul(self.height)? <= MAX_SIXEL_PIXELS)
            .then_some(())
    }
}

#[derive(Debug)]
struct BoundedSixelDecoder {
    decoder: SixelDeserializer,
    bounds: SixelBounds,
}

impl BoundedSixelDecoder {
    fn handle_event(current: &mut Option<Self>, event: SixelEvent) {
        if let Some(currently_parsing) = current.as_mut() {
            if currently_parsing.bounds.advance(event).is_none() {
                *current = None;
            } else {
                let _ = currently_parsing.decoder.handle_event(event);
            }
        }
    }
}

/// The pixel rectangle of a stored sixel image to draw at a viewport cell, as
/// [`SixelGrid::changed_sixel_chunks_in_viewport`] hands it to `yinz-terminal`'s output. Lives
/// here rather than in the output so this crate needs nothing from the terminal emulator.
#[derive(Debug, Clone, Copy, Default)]
pub struct SixelImageChunk {
    pub cell_x: usize,
    pub cell_y: usize,
    pub sixel_image_pixel_x: usize,
    pub sixel_image_pixel_y: usize,
    pub sixel_image_pixel_width: usize,
    pub sixel_image_pixel_height: usize,
    pub sixel_image_id: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct PixelRect {
    pub x: usize,
    pub y: isize, // this can potentially be negative (eg. when the image top has scrolled past the edge of the scrollbuffer)
    pub width: usize,
    pub height: usize,
}

impl PixelRect {
    pub fn new(x: usize, y: usize, height: usize, width: usize) -> Self {
        PixelRect {
            x,
            y: y as isize,
            width,
            height,
        }
    }
    pub fn intersecting_rect(&self, other: &PixelRect) -> Option<PixelRect> {
        // if the two rects intersect, this returns a PixelRect *relative to self*
        let self_top_edge = self.y;
        let self_bottom_edge = self.y + self.height as isize;
        let self_left_edge = self.x;
        let self_right_edge = self.x + self.width;
        let other_top_edge = other.y;
        let other_bottom_edge = other.y + other.height as isize;
        let other_left_edge = other.x;
        let other_right_edge = other.x + other.width;

        let absolute_x = std::cmp::max(self_left_edge, other_left_edge);
        let absolute_y = std::cmp::max(self_top_edge, other_top_edge);
        let absolute_right_edge = std::cmp::min(self_right_edge, other_right_edge);
        let absolute_bottom_edge = std::cmp::min(self_bottom_edge, other_bottom_edge);
        let width = absolute_right_edge.saturating_sub(absolute_x);
        let height = absolute_bottom_edge.saturating_sub(absolute_y);
        let x = absolute_x - self.x;
        let y = absolute_y - self.y;
        if width > 0 && height > 0 {
            Some(PixelRect {
                x,
                y,
                width,
                height: height as usize,
            })
        } else {
            None
        }
    }
}

#[derive(Debug, Default)]
pub struct SixelGrid {
    sixel_image_locations: HashMap<usize, PixelRect>,
    previous_cell_size: Option<SizeInPixels>,
    character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
    currently_parsing: Option<BoundedSixelDecoder>,
    image_ids_to_reap: Vec<usize>,
    images_to_check_for_emptiness: HashSet<usize>,
    sixel_parser: Option<sixel_tokenizer::Parser>,
    pub sixel_image_store: Rc<RefCell<SixelImageStore>>,
}

impl SixelGrid {
    pub fn new(
        character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
        sixel_image_store: Rc<RefCell<SixelImageStore>>,
    ) -> Self {
        let previous_cell_size = *character_cell_size.borrow();
        SixelGrid {
            previous_cell_size,
            character_cell_size,
            sixel_image_store,
            sixel_image_locations: HashMap::new(),
            currently_parsing: None,
            image_ids_to_reap: Vec::new(),
            images_to_check_for_emptiness: HashSet::new(),
            sixel_parser: None,
        }
    }
    pub fn handle_byte(&mut self, byte: u8) {
        self.sixel_parser
            .as_mut()
            .unwrap()
            .advance(&byte, |sixel_event| {
                BoundedSixelDecoder::handle_event(&mut self.currently_parsing, sixel_event);
            });
    }
    pub fn handle_event(&mut self, sixel_event: SixelEvent) {
        BoundedSixelDecoder::handle_event(&mut self.currently_parsing, sixel_event);
    }
    pub fn is_parsing(&self) -> bool {
        self.sixel_parser.is_some()
    }
    pub fn start_image(
        &mut self,
        max_height_in_pixels: Option<usize>,
        dcs_intermediates: Vec<&u8>,
        dcs_params: Vec<&[u16]>,
    ) {
        self.sixel_parser = Some(sixel_tokenizer::Parser::new());
        let decoder = SixelDeserializer::new();
        self.currently_parsing = Some(BoundedSixelDecoder {
            decoder: match max_height_in_pixels {
                Some(max_height) => decoder.max_height(max_height),
                None => decoder,
            },
            bounds: SixelBounds::new(max_height_in_pixels),
        });

        self.handle_byte(27);
        self.handle_byte(b'P');

        for byte in dcs_intermediates {
            self.handle_byte(*byte);
        }

        // send DCS event to parser
        for (i, param) in dcs_params.iter().enumerate() {
            if i != 0 {
                self.handle_byte(b';');
            }
            for subparam in param.iter() {
                let mut b = [0; 4];
                for digit in subparam.to_string().chars() {
                    let len = digit.encode_utf8(&mut b).len();
                    for byte in b.iter().take(len) {
                        self.handle_byte(*byte);
                    }
                }
            }
        }
        self.handle_byte(b'q');
    }
    /// Finish and store the current image, returning its pixel (height, width).
    pub fn end_image(
        &mut self,
        x_pixel_coordinates: usize,
        y_pixel_coordinates: usize,
    ) -> Option<(usize, usize)> {
        self.sixel_parser = None;
        let sixel_image = self.currently_parsing.take()?.decoder.create_image().ok()?;
        let image_pixel_size = sixel_image.pixel_size();
        let image_size_and_coordinates = PixelRect::new(
            x_pixel_coordinates,
            y_pixel_coordinates,
            image_pixel_size.0,
            image_pixel_size.1,
        );

        // Reclaim images fully covered by this one. Intersections are relative to the old image.
        for (image_id, pixel_rect) in &self.sixel_image_locations {
            if let Some(intersecting_rect) =
                pixel_rect.intersecting_rect(&image_size_and_coordinates)
                && intersecting_rect.x == 0
                && intersecting_rect.y == 0
                && intersecting_rect.height == pixel_rect.height
                && intersecting_rect.width == pixel_rect.width
            {
                self.image_ids_to_reap.push(*image_id);
            }
        }
        for image_id in &self.image_ids_to_reap {
            self.sixel_image_locations.remove(image_id);
        }

        let image_id = self.new_sixel_image(sixel_image);
        self.sixel_image_locations
            .insert(image_id, image_size_and_coordinates);
        Some(image_pixel_size)
    }
    pub fn image_coordinates(&self) -> impl Iterator<Item = (usize, &PixelRect)> {
        self.sixel_image_locations
            .iter()
            .map(|(image_id, pixel_rect)| (*image_id, pixel_rect))
    }
    pub fn cut_off_rect_from_images(
        &mut self,
        rect_to_cut_out: PixelRect,
    ) -> Option<Vec<(usize, PixelRect)>> {
        // if there is an image at this cursor location, this returns the image ID and the PixelRect inside the image to be removed
        let mut ret = None;
        for (image_id, pixel_rect) in &self.sixel_image_locations {
            if let Some(intersecting_rect) = pixel_rect.intersecting_rect(&rect_to_cut_out) {
                let ret = ret.get_or_insert(vec![]);
                ret.push((*image_id, intersecting_rect));
            }
        }
        ret
    }
    pub fn offset_grid_top(&mut self) {
        if let Some(character_cell_size) = *self.character_cell_size.borrow() {
            let height_to_reduce = character_cell_size.height as isize;
            for (sixel_image_id, pixel_rect) in self.sixel_image_locations.iter_mut() {
                pixel_rect.y -= height_to_reduce;
                if pixel_rect.y + pixel_rect.height as isize <= 0 {
                    self.image_ids_to_reap.push(*sixel_image_id);
                }
            }
            for image_id in &self.image_ids_to_reap {
                self.sixel_image_locations.remove(image_id);
            }
        }
    }
    pub fn drain_image_ids_to_reap(&mut self) -> Option<Vec<usize>> {
        let store = self.sixel_image_store.borrow();
        for image_id in self.images_to_check_for_emptiness.drain() {
            let has_lit_pixels = store
                .sixel_images
                .get(&image_id)
                .is_some_and(|(image, _)| image.pixels.iter().flatten().any(|pixel| pixel.on));
            if !has_lit_pixels && self.sixel_image_locations.remove(&image_id).is_some() {
                self.image_ids_to_reap.push(image_id);
            }
        }
        let images_to_reap = self.image_ids_to_reap.drain(..);
        if images_to_reap.len() > 0 {
            Some(images_to_reap.collect())
        } else {
            None
        }
    }
    pub fn character_cell_size_possibly_changed(&mut self) {
        if let (Some(previous_cell_size), Some(character_cell_size)) =
            (self.previous_cell_size, *self.character_cell_size.borrow())
            && previous_cell_size != character_cell_size
        {
            for pixel_rect in self.sixel_image_locations.values_mut() {
                pixel_rect.x =
                    (pixel_rect.x / previous_cell_size.width) * character_cell_size.width;
                pixel_rect.y = (pixel_rect.y / previous_cell_size.height as isize)
                    * character_cell_size.height as isize;
            }
        }
        self.previous_cell_size = *self.character_cell_size.borrow();
    }
    pub fn clear(&mut self) -> Option<Vec<usize>> {
        // returns image ids to reap
        let mut image_ids: Vec<usize> = self
            .sixel_image_locations
            .drain()
            .map(|(image_id, _image_rect)| image_id)
            .collect();
        image_ids.append(&mut self.image_ids_to_reap);
        self.images_to_check_for_emptiness.clear();
        if !image_ids.is_empty() {
            Some(image_ids)
        } else {
            None
        }
    }
    fn new_sixel_image(&mut self, sixel_image: SixelImage) -> usize {
        self.sixel_image_store
            .borrow_mut()
            .insert_image(Rc::new(sixel_image))
    }
    pub fn remove_pixels_from_image(&mut self, image_id: usize, pixel_rect: PixelRect) {
        self.images_to_check_for_emptiness.insert(image_id);
        if let Some((sixel_image, sixel_image_cache)) = self
            .sixel_image_store
            .borrow_mut()
            .sixel_images
            .get_mut(&image_id)
        {
            Rc::make_mut(sixel_image).cut_out(
                pixel_rect.x,
                pixel_rect.y as usize,
                pixel_rect.width,
                pixel_rect.height,
            );
            sixel_image_cache
                .retain(|cached_rect, _| cached_rect.intersecting_rect(&pixel_rect).is_none());
        }
    }
    pub fn reap_images(&mut self, ids_to_reap: Vec<usize>) {
        for id in ids_to_reap {
            drop(self.sixel_image_store.borrow_mut().sixel_images.remove(&id));
        }
    }
    pub fn image_cell_coordinates_in_viewport(
        &self,
        viewport_height: usize,
        scrollback_height: usize,
    ) -> Vec<(usize, usize, usize, usize)> {
        match *self.character_cell_size.borrow() {
            Some(character_cell_size) => self
                .sixel_image_locations
                .values()
                .filter_map(|pixel_rect| {
                    let scrollback_size_in_pixels = scrollback_height * character_cell_size.height;
                    let top = pixel_rect.y - scrollback_size_in_pixels as isize;
                    let bottom = (top + pixel_rect.height as isize).max(0) as usize;
                    let top = top.max(0) as usize;
                    let bottom = bottom.min(viewport_height * character_cell_size.height);
                    (top < bottom).then(|| {
                        (
                            top / character_cell_size.height,
                            bottom.div_ceil(character_cell_size.height),
                            pixel_rect.x / character_cell_size.width,
                            (pixel_rect.x + pixel_rect.width).div_ceil(character_cell_size.width),
                        )
                    })
                })
                .collect(),
            None => vec![],
        }
    }
    pub fn changed_sixel_chunks_in_viewport(
        &self,
        changed_rects: HashMap<usize, usize>,
        scrollback_size_in_lines: usize,
        viewport_width_in_cells: usize,
        viewport_x_offset: usize,
        viewport_y_offset: usize,
    ) -> Vec<SixelImageChunk> {
        let mut changed_sixel_image_chunks = vec![];
        let character_cell_size = *self.character_cell_size.borrow();
        if let Some(character_cell_size) = character_cell_size {
            for (sixel_image_id, sixel_image_pixel_rect) in self.image_coordinates() {
                for (line_index, line_count) in &changed_rects {
                    let changed_rect = PixelRect::new(
                        0,
                        (line_index + scrollback_size_in_lines) * character_cell_size.height,
                        line_count * character_cell_size.height,
                        viewport_width_in_cells * character_cell_size.width,
                    );
                    let Some(intersection) =
                        sixel_image_pixel_rect.intersecting_rect(&changed_rect)
                    else {
                        continue;
                    };
                    // Clip while coordinates are signed: an image can extend above scrollback.
                    let visible_top = (sixel_image_pixel_rect.y + intersection.y) as usize;
                    changed_sixel_image_chunks.push(SixelImageChunk {
                        cell_x: viewport_x_offset
                            + (sixel_image_pixel_rect.x + intersection.x)
                                / character_cell_size.width,
                        cell_y: viewport_y_offset + visible_top / character_cell_size.height
                            - scrollback_size_in_lines,
                        sixel_image_pixel_x: intersection.x,
                        sixel_image_pixel_y: intersection.y as usize,
                        sixel_image_pixel_width: intersection.width,
                        sixel_image_pixel_height: intersection.height,
                        sixel_image_id,
                    });
                }
            }
        }
        changed_sixel_image_chunks
    }
}

impl Drop for SixelGrid {
    fn drop(&mut self) {
        if let Some(image_ids) = self.clear() {
            self.reap_images(image_ids);
        }
    }
}

type SixelImageCache = HashMap<PixelRect, String>;
#[derive(Debug, Clone, Default)]
pub struct SixelImageStore {
    sixel_images: HashMap<usize, (Rc<SixelImage>, SixelImageCache)>,
    next_image_id: usize,
}

impl SixelImageStore {
    fn insert_image(&mut self, image: Rc<SixelImage>) -> usize {
        // The store is shared by panes; removing an image must never recycle another live ID.
        let image_id = self.next_image_id;
        self.next_image_id += 1;
        self.sixel_images.insert(image_id, (image, HashMap::new()));
        image_id
    }
    /// Retain the current pixels under a separate ID until the completed frame is released.
    pub fn retain_image(&mut self, image_id: usize) -> Option<usize> {
        let image = self.sixel_images.get(&image_id)?.0.clone();
        Some(self.insert_image(image))
    }
    pub fn serialize_image(
        &mut self,
        image_id: usize,
        pixel_x: usize,
        pixel_y: usize,
        pixel_width: usize,
        pixel_height: usize,
    ) -> Option<String> {
        self.sixel_images
            .get_mut(&image_id)
            .map(|(sixel_image, sixel_image_cache)| {
                if let Some(cached_image) = sixel_image_cache.get(&PixelRect::new(
                    pixel_x,
                    pixel_y,
                    pixel_height,
                    pixel_width,
                )) {
                    cached_image.clone()
                } else {
                    let serialized_image =
                        sixel_image.serialize_range(pixel_x, pixel_y, pixel_width, pixel_height);
                    sixel_image_cache.insert(
                        PixelRect::new(pixel_x, pixel_y, pixel_height, pixel_width),
                        serialized_image.clone(),
                    );
                    serialized_image
                }
            })
    }
    pub fn image_count(&self) -> usize {
        self.sixel_images.len()
    }
}

#[cfg(test)]
#[path = "./unit/sixel_tests.rs"]
mod sixel_tests;
