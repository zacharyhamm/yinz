use yinz_types::structured_render::{
    self, DecodeError, GeometryRecord, GraphicsMedium, GraphicsRecord, GRAPHICS_FORMAT_RGBA8,
};

use std::collections::HashMap;

use crate::emulation::damage::Damage;
use crate::emulation::kitty::{
    self, ApcFilter, Graphics, Image, Kind, Notification, NotificationChunk, OscSignal, Placement,
};
use crate::emulation::screen_buffer::TermSize;
use crate::emulation::screen_buffer::{ApplyError, Blink, CursorShape, Occupancy, ScreenBuffer};
use crate::emulation::sixel::{CellSize, Sixels};

const DEFAULT_CELL: CellSize = CellSize {
    width: 10,
    height: 20,
};

#[derive(Debug)]
pub enum FrameError {
    Undecodable(DecodeError),
    Unapplicable { seq: u64, error: ApplyError },
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::Undecodable(e) => write!(f, "{}", e),
            FrameError::Unapplicable { error, .. } => write!(f, "{}", error),
        }
    }
}

impl std::error::Error for FrameError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameApplied {
    pub seq: u64,
    pub damage: Damage,
}

const MAX_PENDING_NOTIFICATIONS: usize = 64;
const MAX_NOTIFICATION_BYTES: usize = 4096;

#[derive(Default)]
struct Signals {
    clipboard: Option<String>,
    title: Option<Option<String>>,
    bells: usize,
    notifications: Vec<Notification>,
}

#[derive(Default)]
struct NotificationAssembly {
    pending: HashMap<String, (String, String)>,
    order: Vec<String>,
}

impl NotificationAssembly {
    fn feed(&mut self, chunk: NotificationChunk) -> Option<Notification> {
        let Some(id) = chunk.id else {
            return assembled(String::new(), chunk.text);
        };
        if !self.pending.contains_key(&id) {
            if self.order.len() >= MAX_PENDING_NOTIFICATIONS {
                let oldest = self.order.remove(0);
                self.pending.remove(&oldest);
            }
            self.order.push(id.clone());
        }
        let entry = self.pending.entry(id.clone()).or_default();
        let destination = match chunk.title {
            true => &mut entry.0,
            false => &mut entry.1,
        };
        let room = MAX_NOTIFICATION_BYTES.saturating_sub(destination.len());
        destination.push_str(&truncated(&chunk.text, room));
        if !chunk.done {
            return None;
        }
        let (title, body) = self.pending.remove(&id)?;
        self.order.retain(|pending| pending != &id);
        assembled(title, body)
    }
}

fn truncated(text: &str, room: usize) -> String {
    if text.len() <= room {
        return text.to_owned();
    }
    let mut end = room;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

fn assembled(title: String, body: String) -> Option<Notification> {
    match (title.is_empty(), body.is_empty()) {
        (true, true) => None,
        (false, true) => Some(Notification {
            title: None,
            body: title,
        }),
        (true, false) => Some(Notification { title: None, body }),
        (false, false) => Some(Notification {
            title: Some(title),
            body,
        }),
    }
}

pub struct TerminalState {
    screen: ScreenBuffer,
    filter: ApcFilter,
    graphics: Graphics,
    sixels: Sixels,
    signals: Signals,
    assembly: NotificationAssembly,
    geometry: GeometryRecord,
    geometry_stamp: u64,
    shared_media: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphicsStamp {
    geometry: u64,
    sixels: u64,
    kitty: u64,
}

impl TerminalState {
    pub fn new(rows: usize, cols: usize) -> Self {
        Self {
            screen: ScreenBuffer::new(rows, cols),
            filter: ApcFilter::new(),
            graphics: Graphics::new(),
            sixels: Sixels::new(DEFAULT_CELL),
            signals: Signals::default(),
            assembly: NotificationAssembly::default(),
            geometry: GeometryRecord::default(),
            geometry_stamp: 0,
            shared_media: true,
        }
    }

    pub fn set_shared_media(&mut self, allowed: bool) {
        self.shared_media = allowed;
    }

    pub fn geometry(&self) -> &GeometryRecord {
        &self.geometry
    }

    pub fn graphics_stamp(&self) -> GraphicsStamp {
        GraphicsStamp {
            geometry: self.geometry_stamp,
            sixels: self.sixels.stamp(),
            kitty: self.graphics.stamp(),
        }
    }

    pub fn take_copied_text(&mut self) -> Option<String> {
        self.signals.clipboard.take()
    }

    pub fn take_title(&mut self) -> Option<Option<String>> {
        self.signals.title.take()
    }

    pub fn take_bells(&mut self) -> usize {
        std::mem::take(&mut self.signals.bells)
    }

    pub fn take_notifications(&mut self) -> Vec<Notification> {
        std::mem::take(&mut self.signals.notifications)
    }

    pub fn apply_frame(&mut self, frame: &[u8]) -> Result<FrameApplied, FrameError> {
        self.apply_frame_with(frame, |_, _| {})
    }

    pub fn apply_frame_with(
        &mut self,
        frame: &[u8],
        before: impl FnOnce(&TerminalState, &structured_render::FrameView<'_>),
    ) -> Result<FrameApplied, FrameError> {
        let view = structured_render::decode(frame).map_err(FrameError::Undecodable)?;
        before(self, &view);
        let applied = self
            .screen
            .apply(&view)
            .map_err(|error| FrameError::Unapplicable {
                seq: view.header().seq,
                error,
            })?;
        if applied.cleared {
            self.graphics.clear();
            self.sixels.clear();
        }
        let mut everything = applied.cleared || applied.reshaped;
        if let Some(geometry) = view.geometry() {
            if geometry != self.geometry {
                everything = true;
                self.geometry_stamp += 1;
            }
            self.geometry = geometry;
        }
        self.consume_sideband(view.sideband());
        self.consume_graphics(&view.graphics());
        self.sixels.cut_spans(&applied.written);
        self.sixels.commit();

        let damage = if everything {
            Damage::Everything
        } else {
            let mut rows: Vec<usize> = applied.written.iter().map(|span| span.line).collect();
            if let Some((left, entered)) = applied.cursor_rows {
                rows.push(left);
                rows.push(entered);
            }
            Damage::Rows(rows)
        };
        Ok(FrameApplied {
            seq: applied.seq,
            damage,
        })
    }

    fn consume_graphics(&mut self, records: &[GraphicsRecord]) {
        for record in records {
            match record {
                GraphicsRecord::Clear => {
                    self.graphics.clear();
                    self.sixels.clear();
                },
                GraphicsRecord::Residency {
                    id,
                    width,
                    height,
                    format,
                    medium,
                    byte_len,
                    data,
                } => {
                    if *format != GRAPHICS_FORMAT_RGBA8 {
                        report!("image {} arrived in unknown format {}", id, format);
                        continue;
                    }
                    let expected = *byte_len as usize;
                    let pixels = match medium {
                        GraphicsMedium::Inline => {
                            (data.len() >= expected).then(|| data[..expected].to_vec())
                        },
                        GraphicsMedium::SharedFile if !self.shared_media => {
                            report!("refusing image {} carried in a local file", id);
                            None
                        },
                        GraphicsMedium::SharedFile => match std::str::from_utf8(data) {
                            Ok(path) => kitty::read_shared_media(path, expected),
                            Err(_) => None,
                        },
                    };
                    let Some(pixels) = pixels else {
                        continue;
                    };
                    if pixels.len() < *width as usize * *height as usize * 4 {
                        report!(
                            "image {} carried {} bytes for {}x{}",
                            id,
                            pixels.len(),
                            width,
                            height
                        );
                        continue;
                    }
                    self.graphics.insert_image(
                        *id as u32,
                        Image {
                            width: *width,
                            height: *height,
                            pixels,
                        },
                    );
                },
                GraphicsRecord::Placement {
                    image_id,
                    placement_id,
                    cell_x,
                    cell_y,
                    offset_x,
                    offset_y,
                    source_x,
                    source_y,
                    source_width,
                    source_height,
                    z,
                } => self.graphics.insert_placement(Placement {
                    image_id: *image_id as u32,
                    placement_id: *placement_id as u32,
                    cell_x: *cell_x as usize,
                    cell_y: *cell_y as usize,
                    offset_x: *offset_x,
                    offset_y: *offset_y,
                    source_x: *source_x,
                    source_y: *source_y,
                    source_width: *source_width,
                    source_height: *source_height,
                    z: *z,
                }),
                GraphicsRecord::DeleteImage { id } => self.graphics.remove_image(*id as u32),
                GraphicsRecord::DeletePlacement {
                    image_id,
                    placement_id,
                } => self
                    .graphics
                    .remove_placement(*image_id as u32, *placement_id as u32),
                GraphicsRecord::SixelChunk {
                    cell_x,
                    cell_y,
                    payload,
                    ..
                } => self
                    .sixels
                    .execute(payload, (*cell_y as usize, *cell_x as usize)),
            }
        }
    }

    fn consume_sideband(&mut self, sideband: &[u8]) {
        for byte in sideband {
            let step = self.filter.advance(*byte);
            match step.kind {
                Kind::Forward | Kind::Cleared => {},
                Kind::Boundary => {
                    if step.bytes.as_slice() == [0x07] {
                        self.signals.bells += 1;
                    }
                },
                Kind::Signal(signal) => match signal {
                    OscSignal::Notification(chunk) => {
                        if let Some(notification) = self.assembly.feed(chunk) {
                            self.signals.notifications.push(notification);
                        }
                    },
                    OscSignal::Title(title) => self.signals.title = Some(title),
                    OscSignal::Clipboard(text) => self.signals.clipboard = Some(text),
                },
            }
        }
    }

    pub fn set_cell_size(&mut self, width: u32, height: u32) {
        self.sixels.set_cell_size(CellSize::new(width, height));
    }

    pub fn size(&self) -> TermSize {
        self.screen.size()
    }

    pub fn screen(&self) -> &ScreenBuffer {
        &self.screen
    }

    pub fn cursor_is_visible(&self) -> bool {
        self.screen.cursor_is_visible()
    }

    pub fn cursor_position(&self) -> (usize, usize) {
        self.screen.cursor_position()
    }

    pub fn cursor_shape(&self) -> CursorShape {
        self.screen.cursor_shape()
    }

    pub fn cursor_is_blinking(&self) -> bool {
        self.screen.cursor_is_blinking()
    }

    pub fn occupancy(&self, row: usize, col: usize) -> Occupancy {
        self.screen.occupancy(row, col)
    }

    pub fn graphics(&self) -> &Graphics {
        &self.graphics
    }

    pub fn sixels(&self) -> &Sixels {
        &self.sixels
    }

    pub fn blink_at(&self, row: usize, col: usize) -> Blink {
        self.screen.blink_at(row, col)
    }

    pub fn has_blinking_cells(&self) -> bool {
        self.screen.has_blinking_cells()
    }
}

pub fn clamped(rows: usize, cols: usize) -> TermSize {
    TermSize {
        rows: rows.max(1),
        cols: cols.max(1),
    }
}

#[cfg(test)]
#[path = "unit/terminal_tests.rs"]
mod tests;
