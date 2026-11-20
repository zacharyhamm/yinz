use std::sync::Arc;
use yinz_runtime::native_media::MediaRoot;
use yinz_types::render_negotiation as caps;
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
    Protocol(u64, String),
    Unapplicable { seq: u64, error: ApplyError },
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::Protocol(seq, reason) => write!(f, "native frame {seq}: {reason}"),
            FrameError::Undecodable(e) => write!(f, "{}", e),
            FrameError::Unapplicable { error, .. } => write!(f, "{}", error),
        }
    }
}

impl std::error::Error for FrameError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameApplied {
    pub seq: u64,
    pub cleared: bool,
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
    live: Option<(caps::Agreement, Option<Arc<MediaRoot>>)>,
    applied_seq: Option<u64>,
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
            live: None,
            applied_seq: None,
        }
    }

    /// Install the validated attachment agreement before its first live frame.
    pub fn negotiate(&mut self, agreement: caps::Agreement, root: Option<Arc<MediaRoot>>) {
        self.live = Some((agreement, root));
        self.applied_seq = None;
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
        let h = view.header();
        let records = view.graphics();
        let mut staged = None;
        if let Some((agreement, root)) = &self.live {
            let invalid = |reason: &str| FrameError::Protocol(h.seq, reason.to_owned());
            if u32::from(h.version) != agreement.version {
                return Err(invalid("unselected codec version"));
            }
            if let Some(last) = self.applied_seq {
                if h.seq <= last {
                    return Ok(FrameApplied {
                        seq: last,
                        cleared: false,
                        damage: Damage::Rows(vec![]),
                    });
                }
                if last.checked_add(1) != Some(h.seq) {
                    return Err(invalid("missing predecessor"));
                }
            } else if !h.full_repaint() {
                return Err(invalid("first frame requires a full repaint"));
            }
            let features = agreement.features;
            let shifts = view.shifts();
            if !shifts.is_empty()
                && (features & caps::SHIFTS == 0
                    || self.applied_seq.is_none()
                    || h.full_repaint()
                    || h.clear()
                    || self.size().cols != h.cols as usize
                    || self.size().rows != h.rows as usize
                    || shifts.entries.iter().any(|s| {
                        s.cols == 0 || s.rows == 0 || !s.fits(self.size().cols, self.size().rows)
                    }))
            {
                return Err(invalid("invalid or unnegotiated row shift"));
            }
            let geometry = view.geometry();
            if view.has_scroll_record()
                || geometry.is_some() && features & caps::GEOMETRY == 0
                || features & caps::LINKS == 0
                    && (!view.links().entries.is_empty()
                        || view
                            .rows()
                            .any(|(_, cells)| cells.iter().any(|cell| cell.link != 0)))
            {
                return Err(invalid("unnegotiated structured section"));
            }
            let replacement = geometry
                .as_ref()
                .filter(|g| !h.full_repaint() && !h.clear() && **g != self.geometry);
            let validate_retained_sixels =
                replacement.is_some() && self.sixels.chunks().next().is_some();
            if !records.is_empty() || validate_retained_sixels {
                let mut graphics = self.graphics.clone();
                let mut sixels = self.sixels.clone();
                if h.full_repaint() || h.clear() {
                    graphics.clear();
                    sixels.clear();
                }
                let empty_geometry = GeometryRecord::default();
                let previous_geometry = if h.full_repaint() || h.clear() {
                    &empty_geometry
                } else {
                    &self.geometry
                };
                Self::consume_graphics(
                    &records,
                    &mut graphics,
                    &mut sixels,
                    false,
                    Some((
                        agreement,
                        root.as_deref(),
                        geometry.as_ref().unwrap_or(previous_geometry),
                    )),
                )
                .map_err(|reason| FrameError::Protocol(h.seq, reason))?;
                // New Sixels paint over row damage. Validate the chunks left by both operations.
                if sixels.chunks().next().is_some() {
                    sixels.cut_spans(&ScreenBuffer::written_spans(&view, &shifts.entries));
                }
                sixels.commit();
                staged = Some((graphics, sixels));
            }
            if let Some(geometry) = replacement {
                let (graphics, sixels) = staged
                    .as_ref()
                    .map(|(g, s)| (g, s))
                    .unwrap_or((&self.graphics, &self.sixels));
                let mut anchors = graphics
                    .placements()
                    .map(|p| (p.cell_x, p.cell_y))
                    .chain(sixels.chunks().map(|c| (c.cell_x, c.cell_y)));
                if anchors.any(|(x, y)| {
                    u16::try_from(x)
                        .ok()
                        .zip(u16::try_from(y).ok())
                        .and_then(|(x, y)| geometry.content_pane_at(x, y))
                        .is_none()
                }) {
                    return Err(invalid(
                        "replacement geometry leaves an image without an owning pane",
                    ));
                }
            }
        }
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
        if let Some(geometry) = view
            .geometry()
            .or_else(|| (self.live.is_some() && applied.cleared).then(GeometryRecord::default))
        {
            if geometry != self.geometry {
                everything = true;
                self.geometry_stamp += 1;
            }
            self.geometry = geometry;
        }
        self.consume_sideband(view.sideband());
        if let Some((graphics, sixels)) = staged {
            self.graphics = graphics;
            self.sixels = sixels;
        } else {
            if self.live.is_none() {
                let _ = Self::consume_graphics(
                    &records,
                    &mut self.graphics,
                    &mut self.sixels,
                    self.shared_media,
                    None,
                );
            }
            self.sixels.cut_spans(&applied.written);
            self.sixels.commit();
        }
        if self.live.is_some() {
            self.applied_seq = Some(applied.seq);
        }

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
            cleared: applied.cleared,
            damage,
        })
    }

    fn consume_graphics(
        records: &[GraphicsRecord],
        graphics: &mut Graphics,
        sixels: &mut Sixels,
        shared_media: bool,
        live: Option<(&caps::Agreement, Option<&MediaRoot>, &GeometryRecord)>,
    ) -> Result<(), String> {
        for record in records {
            if let Some((agreement, _, geometry)) = live {
                let feature = match record {
                    GraphicsRecord::SixelChunk { .. } => caps::SIXEL,
                    GraphicsRecord::Clear => caps::KITTY | caps::SIXEL,
                    _ => caps::KITTY,
                };
                if agreement.features & feature == 0 {
                    return Err("unnegotiated graphics record".into());
                }
                if let GraphicsRecord::Placement { cell_x, cell_y, .. }
                | GraphicsRecord::SixelChunk { cell_x, cell_y, .. } = record
                    && u16::try_from(*cell_x)
                        .ok()
                        .zip(u16::try_from(*cell_y).ok())
                        .and_then(|(x, y)| geometry.content_pane_at(x, y))
                        .is_none()
                {
                    return Err("image anchor has no owning pane".into());
                }
            }
            match record {
                GraphicsRecord::Clear => {
                    graphics.clear();
                    sixels.clear();
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
                        if live.is_some() {
                            return Err("unknown image format".into());
                        }
                        report!("image {} arrived in unknown format {}", id, format);
                        continue;
                    }
                    let expected = *byte_len as usize;
                    let size = (*width as usize)
                        .checked_mul(*height as usize)
                        .and_then(|n| n.checked_mul(4));
                    if live.is_some() && (size != Some(expected) || expected == 0) {
                        return Err("invalid RGBA dimensions".into());
                    }
                    let pixels = match medium {
                        GraphicsMedium::Inline => {
                            if live.is_some() && data.len() != expected {
                                return Err(
                                    "inline image length differs from RGBA dimensions".into()
                                );
                            }
                            (data.len() >= expected).then(|| data[..expected].to_vec())
                        },
                        GraphicsMedium::SharedFile if live.is_some() => {
                            let root = live
                                .and_then(|(agreement, root, _)| {
                                    root.filter(|_| agreement.media & caps::SHARED != 0)
                                })
                                .ok_or("shared media was not agreed")?;
                            Some(
                                root.read_image(data, expected)
                                    .map_err(|e| format!("read shared image: {e:#}"))?,
                            )
                        },
                        GraphicsMedium::SharedFile if !shared_media => {
                            report!("refusing image {} carried in a local file", id);
                            None
                        },
                        GraphicsMedium::SharedFile => match std::str::from_utf8(data) {
                            Ok(path) => kitty::read_shared_media(path, expected),
                            Err(_) => None,
                        },
                    };
                    let Some(pixels) = pixels else {
                        if live.is_some() {
                            return Err("incomplete native image".into());
                        }
                        continue;
                    };
                    if size.is_none_or(|len| pixels.len() < len) {
                        report!(
                            "image {} carried {} bytes for {}x{}",
                            id,
                            pixels.len(),
                            width,
                            height
                        );
                        continue;
                    }
                    graphics.insert_image(
                        *id,
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
                } => {
                    if live.is_some() && graphics.image(*image_id).is_none() {
                        return Err(format!(
                            "missing image {image_id} for placement {placement_id}"
                        ));
                    }
                    graphics.insert_placement(Placement {
                        image_id: *image_id,
                        placement_id: *placement_id,
                        cell_x: *cell_x as usize,
                        cell_y: *cell_y as usize,
                        offset_x: *offset_x,
                        offset_y: *offset_y,
                        source_x: *source_x,
                        source_y: *source_y,
                        source_width: *source_width,
                        source_height: *source_height,
                        z: *z,
                    });
                },
                GraphicsRecord::DeleteImage { id } => graphics.remove_image(*id),
                GraphicsRecord::DeletePlacement {
                    image_id,
                    placement_id,
                } => graphics.remove_placement(*image_id, *placement_id),
                GraphicsRecord::SixelChunk {
                    cell_x,
                    cell_y,
                    payload,
                    ..
                } => {
                    if !sixels.execute(payload, (*cell_y as usize, *cell_x as usize))
                        && live.is_some()
                    {
                        return Err("invalid Sixel raster".into());
                    }
                },
            }
        }
        Ok(())
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
