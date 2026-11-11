use std::fmt::{self, Display, Formatter};

const WIRE_VERSION: u8 = 4;
const OLDEST_WIRE_VERSION: u8 = 2;
const FIRST_VERSION_WITH_SCROLL: u8 = 3;
const FIRST_VERSION_WITH_CLUSTERS: u8 = 4;

const HEADER_LEN: usize = 48;
const ROW_RECORD_LEN: usize = 8;
const CELL_LEN: usize = 20;
const GRAPHICS_RECORD_HEADER_LEN: usize = 8;

const GRAPHICS_KIND_RESIDENCY: u8 = 0;
const GRAPHICS_KIND_PLACEMENT: u8 = 1;
const GRAPHICS_KIND_DELETE_IMAGE: u8 = 2;
const GRAPHICS_KIND_DELETE_PLACEMENT: u8 = 3;
const GRAPHICS_KIND_CLEAR: u8 = 4;
const GRAPHICS_KIND_SIXEL: u8 = 5;
const RECORD_KIND_GEOMETRY: u8 = 6;
const RECORD_KIND_LINKS: u8 = 7;
const RECORD_KIND_SCROLL: u8 = 8;
const RECORD_KIND_CLUSTERS: u8 = 9;
const RECORD_KIND_SHIFT: u8 = 10;

const GEOMETRY_HEADER_LEN: usize = 8;
const GEOMETRY_ENTRY_LEN: usize = 16;

const LINKS_HEADER_LEN: usize = 8;
const LINK_ENTRY_HEADER_LEN: usize = 4;

const SCROLL_HEADER_LEN: usize = 8;

const CLUSTERS_HEADER_LEN: usize = 8;
const CLUSTER_ENTRY_HEADER_LEN: usize = 6;
pub const MAX_CLUSTER_ENTRY_LEN: usize = 4 * 1024;
pub const MAX_CLUSTER_RECORD_LEN: usize = 1024 * 1024;
pub const CLUSTER_CH_BASE: u32 = 0x110000;
const SCROLL_ENTRY_LEN: usize = 4;
const SHIFT_HEADER_LEN: usize = 8;
const SHIFT_ENTRY_LEN: usize = 10;

pub const LINK_NONE: u8 = 0;
pub const MAX_LINK_ID: u8 = u8::MAX;
pub const MAX_LINK_URI_LEN: usize = u16::MAX as usize;

const GRAPHICS_MEDIUM_INLINE: u8 = 0;
const GRAPHICS_MEDIUM_SHARED_FILE: u8 = 1;

pub const PANE_FRAMED: u8 = 1 << 0;
pub const PANE_FOCUSED: u8 = 1 << 1;
pub const PANE_WANTS_MOUSE: u8 = 1 << 2;
pub const PANE_SELECTABLE: u8 = 1 << 3;
pub const PANE_FLOATING: u8 = 1 << 4;
pub const PANE_POPUP: u8 = 1 << 5;
pub const PANE_PLUGIN_UI: u8 = 1 << 6;
pub const PANE_UNDER_FLOATING: u8 = 1 << 7;

pub const GRAPHICS_FORMAT_RGBA8: u32 = 0;

const FLAG_FULL_REPAINT: u8 = 1 << 0;
const FLAG_CLEAR: u8 = 1 << 1;

const COLOR_DEFAULT: u32 = 0;
const COLOR_TAG_DEFAULT: u8 = 0;
const COLOR_TAG_NAMED: u8 = 1;
const COLOR_TAG_INDEXED: u8 = 2;
const COLOR_TAG_RGB: u8 = 3;

pub const ATTR_BOLD: u16 = 1 << 0;
pub const ATTR_DIM: u16 = 1 << 1;
pub const ATTR_ITALIC: u16 = 1 << 2;
pub const ATTR_REVERSE: u16 = 1 << 3;
pub const ATTR_HIDDEN: u16 = 1 << 4;
pub const ATTR_STRIKE: u16 = 1 << 5;
pub const ATTR_SLOW_BLINK: u16 = 1 << 6;
pub const ATTR_FAST_BLINK: u16 = 1 << 7;
pub const ATTR_GROUP_CONTINUE: u16 = 1 << 11;
pub const UNDERLINE_SHIFT: u32 = 8;
const UNDERLINE_MASK: u16 = 0b111 << UNDERLINE_SHIFT;

pub const UNDERLINE_NONE: u16 = 0;
pub const UNDERLINE_STRAIGHT: u16 = 1;
pub const UNDERLINE_DOUBLE: u16 = 2;
pub const UNDERLINE_CURLY: u16 = 3;
pub const UNDERLINE_DOTTED: u16 = 4;
pub const UNDERLINE_DASHED: u16 = 5;

pub const CURSOR_SHAPE_BLOCK: u8 = 0;
pub const CURSOR_SHAPE_UNDERLINE: u8 = 1;
pub const CURSOR_SHAPE_BEAM: u8 = 2;
pub const CURSOR_SHAPE_DEFAULT: u8 = 3;
const CURSOR_SHAPE_MASK: u8 = 0b0111_1111;
pub const CURSOR_BLINKING: u8 = 1 << 7;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WireCell {
    pub ch: u32,
    pub fg: u32,
    pub bg: u32,
    pub underline_color: u32,
    pub attrs: u16,
    pub width: u8,
    pub link: u8,
}

const _: [(); CELL_LEN] = [(); std::mem::size_of::<WireCell>()];
const _: [(); 4] = [(); std::mem::align_of::<WireCell>()];

impl Default for WireCell {
    fn default() -> Self {
        Self::BLANK
    }
}

impl WireCell {
    pub const BLANK: WireCell = WireCell {
        ch: ' ' as u32,
        fg: COLOR_DEFAULT,
        bg: COLOR_DEFAULT,
        underline_color: COLOR_DEFAULT,
        attrs: 0,
        width: 1,
        link: LINK_NONE,
    };

    pub fn character(&self) -> char {
        char::from_u32(self.ch).unwrap_or(' ')
    }

    pub fn cluster_id(&self) -> Option<u16> {
        if self.ch >= CLUSTER_CH_BASE {
            u16::try_from(self.ch - CLUSTER_CH_BASE).ok()
        } else {
            None
        }
    }

    pub fn is_group_continuation(&self) -> bool {
        self.has(ATTR_GROUP_CONTINUE)
    }

    pub fn text<'a>(&self, clusters: &'a ClusterRecord) -> std::borrow::Cow<'a, str> {
        match self.cluster_id() {
            Some(id) => match clusters.get(id) {
                Some(text) => std::borrow::Cow::Borrowed(text),
                None => std::borrow::Cow::Borrowed(" "),
            },
            None => std::borrow::Cow::Owned(self.character().to_string()),
        }
    }

    pub fn is_wide_head(&self) -> bool {
        self.width >= 2
    }

    pub fn has(&self, attr: u16) -> bool {
        self.attrs & attr != 0
    }

    pub fn underline_style(&self) -> u16 {
        (self.attrs & UNDERLINE_MASK) >> UNDERLINE_SHIFT
    }

    #[inline]
    fn to_bytes(self) -> [u8; CELL_LEN] {
        let [c0, c1, c2, c3] = self.ch.to_le_bytes();
        let [f0, f1, f2, f3] = self.fg.to_le_bytes();
        let [b0, b1, b2, b3] = self.bg.to_le_bytes();
        let [u0, u1, u2, u3] = self.underline_color.to_le_bytes();
        let [a0, a1] = self.attrs.to_le_bytes();
        [
            c0, c1, c2, c3, f0, f1, f2, f3, b0, b1, b2, b3, u0, u1, u2, u3, a0, a1, self.width,
            self.link,
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireColor {
    Default,
    Named(u8),
    Indexed(u8),
    Rgb(u8, u8, u8),
}

impl WireColor {
    pub fn pack(self) -> u32 {
        match self {
            WireColor::Default => (COLOR_TAG_DEFAULT as u32) << 24,
            WireColor::Named(index) => ((COLOR_TAG_NAMED as u32) << 24) | index as u32,
            WireColor::Indexed(index) => ((COLOR_TAG_INDEXED as u32) << 24) | index as u32,
            WireColor::Rgb(r, g, b) => {
                ((COLOR_TAG_RGB as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
            },
        }
    }

    pub fn unpack(packed: u32) -> WireColor {
        match (packed >> 24) as u8 {
            COLOR_TAG_NAMED => WireColor::Named(packed as u8),
            COLOR_TAG_INDEXED => WireColor::Indexed(packed as u8),
            COLOR_TAG_RGB => WireColor::Rgb(
                ((packed >> 16) & 0xff) as u8,
                ((packed >> 8) & 0xff) as u8,
                (packed & 0xff) as u8,
            ),
            _ => WireColor::Default,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct RowRecord {
    pub y: u16,
    pub x0: u16,
    pub len: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphicsMedium {
    Inline,
    SharedFile,
}

impl GraphicsMedium {
    fn encode(self) -> u8 {
        match self {
            GraphicsMedium::Inline => GRAPHICS_MEDIUM_INLINE,
            GraphicsMedium::SharedFile => GRAPHICS_MEDIUM_SHARED_FILE,
        }
    }

    fn decode(medium: u8) -> Option<Self> {
        match medium {
            GRAPHICS_MEDIUM_INLINE => Some(GraphicsMedium::Inline),
            GRAPHICS_MEDIUM_SHARED_FILE => Some(GraphicsMedium::SharedFile),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphicsRecord {
    Residency {
        id: u64,
        width: u32,
        height: u32,
        format: u32,
        medium: GraphicsMedium,
        byte_len: u32,
        data: Vec<u8>,
    },
    Placement {
        image_id: u64,
        placement_id: u64,
        cell_x: u32,
        cell_y: u32,
        offset_x: u32,
        offset_y: u32,
        source_x: u32,
        source_y: u32,
        source_width: u32,
        source_height: u32,
        z: i32,
    },
    DeleteImage {
        id: u64,
    },
    DeletePlacement {
        image_id: u64,
        placement_id: u64,
    },
    Clear,
    SixelChunk {
        cell_x: u32,
        cell_y: u32,
        pixel_x: u32,
        pixel_y: u32,
        pixel_width: u32,
        pixel_height: u32,
        payload: Vec<u8>,
    },
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn take_u32(payload: &[u8], at: usize) -> Option<u32> {
    let bytes = payload.get(at..at + 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn take_u64(payload: &[u8], at: usize) -> Option<u64> {
    let bytes = payload.get(at..at + 8)?;
    let mut buffer = [0u8; 8];
    buffer.copy_from_slice(bytes);
    Some(u64::from_le_bytes(buffer))
}

impl GraphicsRecord {
    fn kind(&self) -> u8 {
        match self {
            GraphicsRecord::Residency { .. } => GRAPHICS_KIND_RESIDENCY,
            GraphicsRecord::Placement { .. } => GRAPHICS_KIND_PLACEMENT,
            GraphicsRecord::DeleteImage { .. } => GRAPHICS_KIND_DELETE_IMAGE,
            GraphicsRecord::DeletePlacement { .. } => GRAPHICS_KIND_DELETE_PLACEMENT,
            GraphicsRecord::Clear => GRAPHICS_KIND_CLEAR,
            GraphicsRecord::SixelChunk { .. } => GRAPHICS_KIND_SIXEL,
        }
    }

    fn medium(&self) -> u8 {
        match self {
            GraphicsRecord::Residency { medium, .. } => medium.encode(),
            _ => GRAPHICS_MEDIUM_INLINE,
        }
    }

    fn encode_payload(&self, out: &mut Vec<u8>) {
        match self {
            GraphicsRecord::Residency {
                id,
                width,
                height,
                format,
                byte_len,
                data,
                ..
            } => {
                put_u64(out, *id);
                put_u32(out, *width);
                put_u32(out, *height);
                put_u32(out, *format);
                put_u32(out, *byte_len);
                put_u32(out, data.len() as u32);
                put_u32(out, 0);
                out.extend_from_slice(data);
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
                put_u64(out, *image_id);
                put_u64(out, *placement_id);
                for value in [
                    *cell_x,
                    *cell_y,
                    *offset_x,
                    *offset_y,
                    *source_x,
                    *source_y,
                    *source_width,
                    *source_height,
                ] {
                    put_u32(out, value);
                }
                out.extend_from_slice(&z.to_le_bytes());
                put_u32(out, 0);
            },
            GraphicsRecord::DeleteImage { id } => put_u64(out, *id),
            GraphicsRecord::DeletePlacement {
                image_id,
                placement_id,
            } => {
                put_u64(out, *image_id);
                put_u64(out, *placement_id);
            },
            GraphicsRecord::Clear => {},
            GraphicsRecord::SixelChunk {
                cell_x,
                cell_y,
                pixel_x,
                pixel_y,
                pixel_width,
                pixel_height,
                payload,
            } => {
                for value in [
                    *cell_x,
                    *cell_y,
                    *pixel_x,
                    *pixel_y,
                    *pixel_width,
                    *pixel_height,
                    payload.len() as u32,
                    0,
                ] {
                    put_u32(out, value);
                }
                out.extend_from_slice(payload);
            },
        }
    }

    fn decode(kind: u8, medium: u8, payload: &[u8]) -> Option<Self> {
        match kind {
            GRAPHICS_KIND_RESIDENCY => {
                let data_len = take_u32(payload, 24)? as usize;
                let data = payload.get(32..32 + data_len)?.to_vec();
                Some(GraphicsRecord::Residency {
                    id: take_u64(payload, 0)?,
                    width: take_u32(payload, 8)?,
                    height: take_u32(payload, 12)?,
                    format: take_u32(payload, 16)?,
                    medium: GraphicsMedium::decode(medium)?,
                    byte_len: take_u32(payload, 20)?,
                    data,
                })
            },
            GRAPHICS_KIND_PLACEMENT => Some(GraphicsRecord::Placement {
                image_id: take_u64(payload, 0)?,
                placement_id: take_u64(payload, 8)?,
                cell_x: take_u32(payload, 16)?,
                cell_y: take_u32(payload, 20)?,
                offset_x: take_u32(payload, 24)?,
                offset_y: take_u32(payload, 28)?,
                source_x: take_u32(payload, 32)?,
                source_y: take_u32(payload, 36)?,
                source_width: take_u32(payload, 40)?,
                source_height: take_u32(payload, 44)?,
                z: take_u32(payload, 48)? as i32,
            }),
            GRAPHICS_KIND_DELETE_IMAGE => Some(GraphicsRecord::DeleteImage {
                id: take_u64(payload, 0)?,
            }),
            GRAPHICS_KIND_DELETE_PLACEMENT => Some(GraphicsRecord::DeletePlacement {
                image_id: take_u64(payload, 0)?,
                placement_id: take_u64(payload, 8)?,
            }),
            GRAPHICS_KIND_CLEAR => Some(GraphicsRecord::Clear),
            GRAPHICS_KIND_SIXEL => {
                let payload_len = take_u32(payload, 24)? as usize;
                Some(GraphicsRecord::SixelChunk {
                    cell_x: take_u32(payload, 0)?,
                    cell_y: take_u32(payload, 4)?,
                    pixel_x: take_u32(payload, 8)?,
                    pixel_y: take_u32(payload, 12)?,
                    pixel_width: take_u32(payload, 16)?,
                    pixel_height: take_u32(payload, 20)?,
                    payload: payload.get(32..32 + payload_len)?.to_vec(),
                })
            },
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PaneRect {
    pub x: u16,
    pub y: u16,
    pub cols: u16,
    pub rows: u16,
    pub top: u8,
    pub bottom: u8,
    pub left: u8,
    pub right: u8,
    pub flags: u8,
}

impl PaneRect {
    pub fn framed(&self) -> bool {
        self.flags & PANE_FRAMED != 0
    }

    pub fn focused(&self) -> bool {
        self.flags & PANE_FOCUSED != 0
    }

    pub fn wants_mouse(&self) -> bool {
        self.flags & PANE_WANTS_MOUSE != 0
    }

    pub fn selectable(&self) -> bool {
        self.flags & PANE_SELECTABLE != 0
    }

    pub fn floating(&self) -> bool {
        self.flags & PANE_FLOATING != 0
    }

    pub fn popup(&self) -> bool {
        self.flags & PANE_POPUP != 0
    }

    pub fn plugin_ui(&self) -> bool {
        self.flags & PANE_PLUGIN_UI != 0
    }

    pub fn under_floating(&self) -> bool {
        self.flags & PANE_UNDER_FLOATING != 0
    }

    pub fn pin_button_contains(&self, x: u16, y: u16) -> bool {
        if !self.floating() || !self.framed() || !self.selectable() || self.top == 0 {
            return false;
        }
        if y as u32 + 1 != self.content_y() as u32 || x < self.content_x() {
            return false;
        }
        let column = (x - self.content_x()) as u32;
        let center = self.cols.saturating_sub(5) as u32;
        column + 1 >= center && column <= center + 1
    }

    fn edge_pointer(&self, x: u16, y: u16) -> PointerKind {
        let (x, y) = (x as u32, y as u32);
        let left = x < self.content_x() as u32;
        let right = x >= self.content_x() as u32 + self.content_cols() as u32;
        let above = y < self.content_y() as u32;
        let below = y >= self.content_y() as u32 + self.content_rows() as u32;
        match (left || right, above || below) {
            (true, true) if (left && above) || (right && below) => PointerKind::NwseResize,
            (true, true) => PointerKind::NeswResize,
            (true, false) => PointerKind::ColResize,
            (false, true) => PointerKind::RowResize,
            (false, false) => PointerKind::Arrow,
        }
    }

    pub fn content_x(&self) -> u16 {
        self.x.saturating_add(self.left as u16)
    }

    pub fn content_y(&self) -> u16 {
        self.y.saturating_add(self.top as u16)
    }

    pub fn content_cols(&self) -> u16 {
        self.cols
            .saturating_sub(self.left as u16)
            .saturating_sub(self.right as u16)
    }

    pub fn content_rows(&self) -> u16 {
        self.rows
            .saturating_sub(self.top as u16)
            .saturating_sub(self.bottom as u16)
    }

    pub fn contains(&self, x: u16, y: u16) -> bool {
        within(x, self.x, self.cols) && within(y, self.y, self.rows)
    }

    pub fn content_contains(&self, x: u16, y: u16) -> bool {
        within(x, self.content_x(), self.content_cols())
            && within(y, self.content_y(), self.content_rows())
    }

    pub fn clip_to(&self, cols: u16, rows: u16) -> Option<PaneRect> {
        if self.x >= cols || self.y >= rows || self.cols == 0 || self.rows == 0 {
            return None;
        }
        let width = self.cols.min(cols - self.x);
        let height = self.rows.min(rows - self.y);
        let visible_right = self.x.saturating_add(width);
        let visible_bottom = self.y.saturating_add(height);
        let content_right = self
            .x
            .saturating_add(self.cols)
            .saturating_sub(self.right as u16);
        let content_bottom = self
            .y
            .saturating_add(self.rows)
            .saturating_sub(self.bottom as u16);
        let right = visible_right.saturating_sub(content_right.min(visible_right));
        let bottom = visible_bottom.saturating_sub(content_bottom.min(visible_bottom));
        Some(PaneRect {
            x: self.x,
            y: self.y,
            cols: width,
            rows: height,
            top: (self.top as u16).min(height) as u8,
            bottom: bottom.min(u8::MAX as u16) as u8,
            left: (self.left as u16).min(width) as u8,
            right: right.min(u8::MAX as u16) as u8,
            flags: self.flags,
        })
    }

    fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.x.to_le_bytes());
        out.extend_from_slice(&self.y.to_le_bytes());
        out.extend_from_slice(&self.cols.to_le_bytes());
        out.extend_from_slice(&self.rows.to_le_bytes());
        out.push(self.top);
        out.push(self.bottom);
        out.push(self.left);
        out.push(self.right);
        out.push(self.flags);
        out.extend_from_slice(&[0u8; 3]);
    }

    fn decode(entry: &[u8]) -> Option<Self> {
        let entry = entry.get(..GEOMETRY_ENTRY_LEN)?;
        Some(PaneRect {
            x: u16::from_le_bytes([entry[0], entry[1]]),
            y: u16::from_le_bytes([entry[2], entry[3]]),
            cols: u16::from_le_bytes([entry[4], entry[5]]),
            rows: u16::from_le_bytes([entry[6], entry[7]]),
            top: entry[8],
            bottom: entry[9],
            left: entry[10],
            right: entry[11],
            flags: entry[12],
        })
    }
}

fn within(value: u16, start: u16, len: u16) -> bool {
    value >= start && (value as u32) < start as u32 + len as u32
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerKind {
    Arrow,
    Text,
    Hand,
    Move,
    ColResize,
    RowResize,
    NwseResize,
    NeswResize,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct GeometryRecord {
    pub panes: Vec<PaneRect>,
}

impl GeometryRecord {
    pub fn pane_at(&self, x: u16, y: u16) -> Option<&PaneRect> {
        self.panes.iter().rev().find(|pane| pane.contains(x, y))
    }

    pub fn content_pane_at(&self, x: u16, y: u16) -> Option<&PaneRect> {
        self.panes
            .iter()
            .rev()
            .find(|pane| pane.content_contains(x, y))
    }

    pub fn focused_popup(&self) -> Option<&PaneRect> {
        self.panes
            .iter()
            .rev()
            .find(|pane| pane.popup() && pane.focused())
    }

    pub fn claims_the_click(&self, x: u16, y: u16) -> bool {
        if self.focused_popup().is_some() {
            return true;
        }
        match self.pane_at(x, y) {
            Some(pane) if pane.popup() => true,
            Some(pane) => {
                pane.content_contains(x, y)
                    && pane.selectable()
                    && pane.wants_mouse()
                    && pane.focused()
            },
            None => false,
        }
    }

    pub fn pointer_at(&self, x: u16, y: u16, ctrl: bool) -> PointerKind {
        if self.panes.is_empty() {
            return PointerKind::Text;
        }
        if self.focused_popup().is_some() {
            return PointerKind::Arrow;
        }
        let top = self.pane_at(x, y);
        if let Some(pane) = top {
            if pane.popup() || !pane.selectable() {
                return PointerKind::Arrow;
            }
            if pane.content_contains(x, y) {
                if pane.plugin_ui() || (pane.wants_mouse() && pane.focused()) {
                    return PointerKind::Arrow;
                }
                return PointerKind::Text;
            }
            if pane.floating() {
                if ctrl {
                    return pane.edge_pointer(x, y);
                }
                if pane.pin_button_contains(x, y) {
                    return PointerKind::Hand;
                }
                return PointerKind::Move;
            }
        }
        let inert = match top {
            Some(pane) => pane.under_floating(),
            None => self.panes.iter().any(|pane| pane.under_floating()),
        };
        if inert && !ctrl {
            return PointerKind::Arrow;
        }
        self.border_pointer(x, y)
    }

    fn border_pointer(&self, x: u16, y: u16) -> PointerKind {
        let (x, y) = (x as u32, y as u32);
        let (mut left, mut right, mut above, mut below) = (false, false, false, false);
        let tiled = self
            .panes
            .iter()
            .filter(|pane| pane.selectable() && !pane.floating() && !pane.popup());
        for pane in tiled {
            let content_x = pane.content_x() as u32;
            let content_y = pane.content_y() as u32;
            let content_cols = pane.content_cols() as u32;
            let content_rows = pane.content_rows() as u32;
            if content_cols == 0 || content_rows == 0 {
                continue;
            }
            if y + 1 >= pane.y as u32 && y <= pane.y as u32 + pane.rows as u32 {
                right |= x + 1 == content_x;
                left |= x == content_x + content_cols;
            }
            if x + 1 >= pane.x as u32 && x <= pane.x as u32 + pane.cols as u32 {
                below |= y + 1 == content_y;
                above |= y == content_y + content_rows;
            }
        }
        match (left || right, above || below) {
            (true, true) => {
                if (left && right) || (above && below) {
                    PointerKind::Move
                } else if (left && above) || (right && below) {
                    PointerKind::NwseResize
                } else {
                    PointerKind::NeswResize
                }
            },
            (true, false) => PointerKind::ColResize,
            (false, true) => PointerKind::RowResize,
            (false, false) => PointerKind::Arrow,
        }
    }

    pub fn clip_to(&self, cols: u16, rows: u16) -> GeometryRecord {
        GeometryRecord {
            panes: self
                .panes
                .iter()
                .filter_map(|pane| pane.clip_to(cols, rows))
                .collect(),
        }
    }

    fn encode_payload(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.panes.len() as u16).to_le_bytes());
        out.extend_from_slice(&(GEOMETRY_ENTRY_LEN as u16).to_le_bytes());
        put_u32(out, 0);
        for pane in &self.panes {
            pane.encode(out);
        }
    }

    fn decode(payload: &[u8]) -> Option<Self> {
        let header = payload.get(..GEOMETRY_HEADER_LEN)?;
        let pane_count = u16::from_le_bytes([header[0], header[1]]) as usize;
        let entry_len = u16::from_le_bytes([header[2], header[3]]) as usize;
        if entry_len < GEOMETRY_ENTRY_LEN {
            return None;
        }
        let entries = payload.get(GEOMETRY_HEADER_LEN..)?;
        let mut panes = Vec::with_capacity(pane_count);
        for index in 0..pane_count {
            let at = index.checked_mul(entry_len)?;
            let end = at.checked_add(entry_len)?;
            panes.push(PaneRect::decode(entries.get(at..end)?)?);
        }
        Some(GeometryRecord { panes })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkEntry {
    pub id: u8,
    pub uri: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct LinkRecord {
    pub entries: Vec<LinkEntry>,
}

impl LinkRecord {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn encode_payload(&self, out: &mut Vec<u8>) {
        let entries = self
            .entries
            .iter()
            .filter_map(|entry| u16::try_from(entry.uri.len()).ok().map(|len| (entry, len)))
            .take(u16::MAX as usize);
        out.extend_from_slice(&(entries.clone().count() as u16).to_le_bytes());
        out.extend_from_slice(&(LINK_ENTRY_HEADER_LEN as u16).to_le_bytes());
        put_u32(out, 0);
        for (entry, uri_len) in entries {
            let uri = entry.uri.as_bytes();
            out.push(entry.id);
            out.push(0);
            out.extend_from_slice(&uri_len.to_le_bytes());
            out.extend_from_slice(uri);
        }
    }

    fn decode(payload: &[u8]) -> Option<Self> {
        let header = payload.get(..LINKS_HEADER_LEN)?;
        let entry_count = u16::from_le_bytes([header[0], header[1]]) as usize;
        let entry_header_len = u16::from_le_bytes([header[2], header[3]]) as usize;
        if entry_header_len < LINK_ENTRY_HEADER_LEN {
            return None;
        }
        let mut at = LINKS_HEADER_LEN;
        let mut entries = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            let entry_header = payload.get(at..at.checked_add(entry_header_len)?)?;
            let id = entry_header[0];
            let uri_len = u16::from_le_bytes([entry_header[2], entry_header[3]]) as usize;
            at += entry_header_len;
            let uri = payload.get(at..at.checked_add(uri_len)?)?;
            at += uri_len;
            if id == LINK_NONE {
                return None;
            }
            entries.push(LinkEntry {
                id,
                uri: std::str::from_utf8(uri).ok()?.to_owned(),
            });
        }
        Some(LinkRecord { entries })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollEntry {
    pub pane: u16,
    pub lines: i16,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ScrollRecord {
    pub entries: Vec<ScrollEntry>,
}

impl ScrollRecord {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn lines_for(&self, pane: u16) -> Option<i16> {
        self.entries
            .iter()
            .find(|entry| entry.pane == pane)
            .map(|entry| entry.lines)
    }

    pub fn add(&mut self, pane: u16, lines: i32) {
        if lines == 0 {
            return;
        }
        match self.entries.iter_mut().position(|entry| entry.pane == pane) {
            Some(index) => {
                let sum = (self.entries[index].lines as i32 + lines)
                    .clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                if sum == 0 {
                    self.entries.remove(index);
                } else {
                    self.entries[index].lines = sum;
                }
            },
            None => self.entries.push(ScrollEntry {
                pane,
                lines: lines.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
            }),
        }
    }

    fn encode_payload(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(SCROLL_ENTRY_LEN as u16).to_le_bytes());
        put_u32(out, 0);
        for entry in &self.entries {
            out.extend_from_slice(&entry.pane.to_le_bytes());
            out.extend_from_slice(&entry.lines.to_le_bytes());
        }
    }

    fn decode(payload: &[u8]) -> Option<Self> {
        let header = payload.get(..SCROLL_HEADER_LEN)?;
        let entry_count = u16::from_le_bytes([header[0], header[1]]) as usize;
        let entry_len = u16::from_le_bytes([header[2], header[3]]) as usize;
        if entry_len < SCROLL_ENTRY_LEN {
            return None;
        }
        let entries = payload.get(SCROLL_HEADER_LEN..)?;
        let mut record = ScrollRecord {
            entries: Vec::with_capacity(entry_count),
        };
        for index in 0..entry_count {
            let at = index.checked_mul(entry_len)?;
            let entry = entries.get(at..at.checked_add(entry_len)?)?;
            record.entries.push(ScrollEntry {
                pane: u16::from_le_bytes([entry[0], entry[1]]),
                lines: i16::from_le_bytes([entry[2], entry[3]]),
            });
        }
        Some(record)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShiftEntry {
    pub x: u16,
    pub y: u16,
    pub cols: u16,
    pub rows: u16,
    pub lines: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ShiftRecord {
    pub entries: Vec<ShiftEntry>,
}

impl ShiftEntry {
    pub fn fits(&self, cols: usize, rows: usize) -> bool {
        self.lines > 0
            && self.x as usize + self.cols as usize <= cols
            && self.y as usize + self.rows as usize <= rows
    }

    pub fn apply(&self, cells: &mut [WireCell], cols: usize, rows: usize) {
        if !self.fits(cols, rows) || cells.len() < cols * rows {
            return;
        }
        let (x, y) = (self.x as usize, self.y as usize);
        let (width, height, lines) = (self.cols as usize, self.rows as usize, self.lines as usize);
        for row in y..y + height {
            let target = row * cols + x;
            if row + lines < y + height {
                let source = (row + lines) * cols + x;
                cells.copy_within(source..source + width, target);
            } else {
                cells[target..target + width].fill(WireCell::BLANK);
            }
        }
    }

    pub fn moved_position(&self, at: usize, cols: usize) -> Option<Option<usize>> {
        let (row, col) = (at / cols, at % cols);
        let (x, y) = (self.x as usize, self.y as usize);
        if col < x || col >= x + self.cols as usize || row < y || row >= y + self.rows as usize {
            return None;
        }
        Some((row >= y + self.lines as usize).then(|| at - self.lines as usize * cols))
    }
}

impl ShiftRecord {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn encode_payload(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(SHIFT_ENTRY_LEN as u16).to_le_bytes());
        put_u32(out, 0);
        for entry in &self.entries {
            for value in [entry.x, entry.y, entry.cols, entry.rows, entry.lines] {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
    }

    fn decode(payload: &[u8]) -> Option<Self> {
        let header = payload.get(..SHIFT_HEADER_LEN)?;
        let entry_count = u16::from_le_bytes([header[0], header[1]]) as usize;
        let entry_len = u16::from_le_bytes([header[2], header[3]]) as usize;
        if entry_len < SHIFT_ENTRY_LEN {
            return None;
        }
        let entries = payload.get(SHIFT_HEADER_LEN..)?;
        let mut record = ShiftRecord {
            entries: Vec::with_capacity(entry_count),
        };
        for index in 0..entry_count {
            let at = index.checked_mul(entry_len)?;
            let entry = entries.get(at..at.checked_add(entry_len)?)?;
            let value = |i: usize| u16::from_le_bytes([entry[2 * i], entry[2 * i + 1]]);
            record.entries.push(ShiftEntry {
                x: value(0),
                y: value(1),
                cols: value(2),
                rows: value(3),
                lines: value(4),
            });
        }
        Some(record)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterEntry {
    pub id: u16,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ClusterRecord {
    pub entries: Vec<ClusterEntry>,
}

impl ClusterRecord {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, id: u16) -> Option<&str> {
        match self.entries.binary_search_by_key(&id, |entry| entry.id) {
            Ok(index) => Some(self.entries[index].text.as_str()),
            Err(_) => self
                .entries
                .iter()
                .find(|entry| entry.id == id)
                .map(|entry| entry.text.as_str()),
        }
    }

    fn encode_payload(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(CLUSTER_ENTRY_HEADER_LEN as u16).to_le_bytes());
        put_u32(out, 0);
        for entry in &self.entries {
            let text = entry.text.as_bytes();
            out.extend_from_slice(&entry.id.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&(text.len() as u16).to_le_bytes());
            out.extend_from_slice(text);
        }
    }

    fn decode(payload: &[u8]) -> Option<Self> {
        if payload.len() > MAX_CLUSTER_RECORD_LEN {
            return None;
        }
        let header = payload.get(..CLUSTERS_HEADER_LEN)?;
        let entry_count = u16::from_le_bytes([header[0], header[1]]) as usize;
        let entry_header_len = u16::from_le_bytes([header[2], header[3]]) as usize;
        if entry_header_len < CLUSTER_ENTRY_HEADER_LEN {
            return None;
        }
        let mut at = CLUSTERS_HEADER_LEN;
        let mut entries: Vec<ClusterEntry> = Vec::with_capacity(entry_count.min(4096));
        for _ in 0..entry_count {
            let entry_header = payload.get(at..at.checked_add(entry_header_len)?)?;
            let id = u16::from_le_bytes([entry_header[0], entry_header[1]]);
            let text_len = u16::from_le_bytes([entry_header[4], entry_header[5]]) as usize;
            at += entry_header_len;
            if text_len == 0 || text_len > MAX_CLUSTER_ENTRY_LEN {
                return None;
            }
            let text = payload.get(at..at.checked_add(text_len)?)?;
            at += text_len;
            let text = std::str::from_utf8(text).ok()?.to_owned();
            entries.push(ClusterEntry { id, text });
        }
        entries.sort_by_key(|entry| entry.id);
        if entries.windows(2).any(|pair| pair[0].id == pair[1].id) {
            return None;
        }
        Some(ClusterRecord { entries })
    }
}

#[derive(Default)]
pub struct ClusterInterner {
    by_text: std::collections::HashMap<String, u16>,
    record: ClusterRecord,
    bytes: usize,
}

impl ClusterInterner {
    pub fn intern(&mut self, text: &str) -> Option<u32> {
        if let Some(id) = self.by_text.get(text) {
            return Some(CLUSTER_CH_BASE + *id as u32);
        }
        if text.is_empty() || text.len() > MAX_CLUSTER_ENTRY_LEN {
            return None;
        }
        let cost = CLUSTER_ENTRY_HEADER_LEN + text.len();
        if self.bytes + cost + CLUSTERS_HEADER_LEN > MAX_CLUSTER_RECORD_LEN {
            return None;
        }
        let id = u16::try_from(self.record.entries.len()).ok()?;
        self.by_text.insert(text.to_owned(), id);
        self.record.entries.push(ClusterEntry {
            id,
            text: text.to_owned(),
        });
        self.bytes += cost;
        Some(CLUSTER_CH_BASE + id as u32)
    }

    pub fn is_empty(&self) -> bool {
        self.record.is_empty()
    }

    pub fn take(&mut self) -> ClusterRecord {
        self.by_text.clear();
        self.bytes = 0;
        std::mem::take(&mut self.record)
    }
}

fn record_is_decodable(version: u8, kind: u8, medium: u8, payload: &[u8]) -> bool {
    match kind {
        RECORD_KIND_CLUSTERS => {
            version >= FIRST_VERSION_WITH_CLUSTERS && ClusterRecord::decode(payload).is_some()
        },
        RECORD_KIND_GEOMETRY => GeometryRecord::decode(payload).is_some(),
        RECORD_KIND_LINKS => LinkRecord::decode(payload).is_some(),
        RECORD_KIND_SCROLL => {
            version >= FIRST_VERSION_WITH_SCROLL && ScrollRecord::decode(payload).is_some()
        },
        RECORD_KIND_SHIFT => ShiftRecord::decode(payload).is_some(),
        _ => GraphicsRecord::decode(kind, medium, payload).is_some(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameHeader {
    pub version: u8,
    pub flags: u8,
    pub cols: u16,
    pub rows: u16,
    pub seq: u64,
    pub cursor_x: u16,
    pub cursor_y: u16,
    pub cursor_shape: u8,
    pub cursor_visible: bool,
    pub row_record_count: u32,
    pub cells_offset: u32,
    pub cells_len: u32,
    pub graphics_offset: u32,
    pub graphics_len: u32,
    pub sideband_offset: u32,
    pub sideband_len: u32,
}

impl FrameHeader {
    pub fn full_repaint(&self) -> bool {
        self.flags & FLAG_FULL_REPAINT != 0
    }

    pub fn clear(&self) -> bool {
        self.flags & FLAG_CLEAR != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorState {
    pub x: u16,
    pub y: u16,
    pub shape: u8,
    pub visible: bool,
}

impl CursorState {
    pub fn blinking(&self) -> bool {
        self.shape & CURSOR_BLINKING != 0
    }

    pub fn shape_kind(&self) -> u8 {
        self.shape & CURSOR_SHAPE_MASK
    }
}

impl Default for CursorState {
    fn default() -> Self {
        CursorState {
            x: 0,
            y: 0,
            shape: CURSOR_SHAPE_DEFAULT,
            visible: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    TooShort,
    UnsupportedVersion(u8),
    BadSection,
    CellCountMismatch,
    RowOutOfRange,
    CellsExhausted,
    BadGraphicsRecord,
}

impl Display for DecodeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::TooShort => write!(f, "the frame is shorter than its header"),
            DecodeError::UnsupportedVersion(version) => {
                write!(f, "the frame declares wire version {}", version)
            },
            DecodeError::BadSection => write!(f, "a frame section runs outside the frame"),
            DecodeError::CellCountMismatch => {
                write!(f, "the cell section is not a whole number of cells")
            },
            DecodeError::RowOutOfRange => write!(f, "a row record runs outside the viewport"),
            DecodeError::CellsExhausted => {
                write!(f, "the row records name more cells than the frame carries")
            },
            DecodeError::BadGraphicsRecord => {
                write!(f, "a graphics record is malformed or runs past its section")
            },
        }
    }
}

impl std::error::Error for DecodeError {}

fn align_up(value: usize, alignment: usize) -> usize {
    value.div_ceil(alignment) * alignment
}

pub struct FrameBuilder {
    cols: u16,
    rows: u16,
    seq: u64,
    flags: u8,
    cursor: CursorState,
    records: Vec<RowRecord>,
    cells: Vec<u8>,
    graphics: Vec<u8>,
    sideband: Vec<u8>,
    clusters: ClusterInterner,
}

impl FrameBuilder {
    pub fn new(cols: u16, rows: u16, seq: u64) -> Self {
        FrameBuilder {
            cols,
            rows,
            seq,
            flags: 0,
            cursor: CursorState::default(),
            records: Vec::new(),
            cells: Vec::new(),
            graphics: Vec::new(),
            sideband: Vec::new(),
            clusters: ClusterInterner::default(),
        }
    }

    pub fn cluster(&mut self, text: &str) -> Option<u32> {
        self.clusters.intern(text)
    }

    pub fn push_clusters(&mut self, record: &ClusterRecord) {
        if record.is_empty() {
            return;
        }
        self.push_record(RECORD_KIND_CLUSTERS, GRAPHICS_MEDIUM_INLINE, |out| {
            record.encode_payload(out)
        });
    }

    pub fn set_full_repaint(&mut self, full_repaint: bool) {
        self.set_flag(FLAG_FULL_REPAINT, full_repaint);
    }

    pub fn set_clear(&mut self, clear: bool) {
        self.set_flag(FLAG_CLEAR, clear);
    }

    fn set_flag(&mut self, flag: u8, on: bool) {
        if on {
            self.flags |= flag;
        } else {
            self.flags &= !flag;
        }
    }

    pub fn set_cursor(&mut self, cursor: CursorState) {
        self.cursor = cursor;
    }

    pub fn is_full_repaint(&self) -> bool {
        self.flags & FLAG_FULL_REPAINT != 0
    }

    pub fn is_clear(&self) -> bool {
        self.flags & FLAG_CLEAR != 0
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }

    pub fn push_row(&mut self, y: u16, x0: u16, cells: &[WireCell]) {
        if y >= self.rows || x0 >= self.cols || cells.is_empty() {
            return;
        }
        let room = (self.cols - x0) as usize;
        let cells = &cells[..cells.len().min(room)];
        self.records.push(RowRecord {
            y,
            x0,
            len: cells.len() as u16,
        });
        let start = self.cells.len();
        self.cells.resize(start + cells.len() * CELL_LEN, 0);
        for (slot, cell) in self.cells[start..]
            .as_chunks_mut::<CELL_LEN>()
            .0
            .iter_mut()
            .zip(cells)
        {
            *slot = cell.to_bytes();
        }
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty() && self.graphics.is_empty() && self.sideband.is_empty()
    }

    pub fn sideband_mut(&mut self) -> &mut Vec<u8> {
        &mut self.sideband
    }

    fn push_record(&mut self, kind: u8, medium: u8, payload: impl FnOnce(&mut Vec<u8>)) {
        let start = self.graphics.len();
        self.graphics.push(kind);
        self.graphics.push(medium);
        self.graphics.extend_from_slice(&0u16.to_le_bytes());
        self.graphics.extend_from_slice(&0u32.to_le_bytes());
        payload(&mut self.graphics);
        let payload_len = self.graphics.len() - start - GRAPHICS_RECORD_HEADER_LEN;
        self.graphics[start + 4..start + 8].copy_from_slice(&(payload_len as u32).to_le_bytes());
        self.graphics.resize(align_up(self.graphics.len(), 4), 0);
    }

    fn push_graphics(&mut self, record: &GraphicsRecord) {
        self.push_record(record.kind(), record.medium(), |out| {
            record.encode_payload(out)
        });
    }

    pub fn push_geometry(&mut self, record: &GeometryRecord) {
        self.push_record(RECORD_KIND_GEOMETRY, GRAPHICS_MEDIUM_INLINE, |out| {
            record.encode_payload(out)
        });
    }

    pub fn push_links(&mut self, record: &LinkRecord) {
        if record.is_empty() {
            return;
        }
        self.push_record(RECORD_KIND_LINKS, GRAPHICS_MEDIUM_INLINE, |out| {
            record.encode_payload(out)
        });
    }

    pub fn push_shifts(&mut self, record: &ShiftRecord) {
        if record.is_empty() {
            return;
        }
        self.push_record(RECORD_KIND_SHIFT, GRAPHICS_MEDIUM_INLINE, |out| {
            record.encode_payload(out)
        });
    }

    pub fn push_scroll(&mut self, record: &ScrollRecord) {
        if record.is_empty() {
            return;
        }
        self.push_record(RECORD_KIND_SCROLL, GRAPHICS_MEDIUM_INLINE, |out| {
            record.encode_payload(out)
        });
    }

    pub fn extend_graphics<'a>(&mut self, records: impl IntoIterator<Item = &'a GraphicsRecord>) {
        for record in records {
            self.push_graphics(record);
        }
    }

    pub fn finish(mut self) -> Vec<u8> {
        if !self.clusters.is_empty() {
            let record = self.clusters.take();
            self.push_clusters(&record);
        }
        let records_offset = HEADER_LEN;
        let records_len = self.records.len() * ROW_RECORD_LEN;
        let cells_offset = align_up(records_offset + records_len, 4);
        let cells_len = self.cells.len();
        let graphics_offset = align_up(cells_offset + cells_len, 4);
        let graphics_len = self.graphics.len();
        let sideband_offset = graphics_offset + graphics_len;
        let total = sideband_offset + self.sideband.len();

        let mut frame = Vec::with_capacity(total);
        frame.push(WIRE_VERSION);
        frame.push(self.flags);
        frame.extend_from_slice(&self.cols.to_le_bytes());
        frame.extend_from_slice(&self.rows.to_le_bytes());
        frame.extend_from_slice(&self.seq.to_le_bytes());
        frame.extend_from_slice(&self.cursor.x.to_le_bytes());
        frame.extend_from_slice(&self.cursor.y.to_le_bytes());
        frame.push(self.cursor.shape);
        frame.push(u8::from(self.cursor.visible));
        frame.extend_from_slice(&(self.records.len() as u32).to_le_bytes());
        frame.extend_from_slice(&(cells_offset as u32).to_le_bytes());
        frame.extend_from_slice(&(cells_len as u32).to_le_bytes());
        frame.extend_from_slice(&(graphics_offset as u32).to_le_bytes());
        frame.extend_from_slice(&(graphics_len as u32).to_le_bytes());
        frame.extend_from_slice(&(sideband_offset as u32).to_le_bytes());
        frame.extend_from_slice(&(self.sideband.len() as u32).to_le_bytes());
        debug_assert_eq!(frame.len(), HEADER_LEN);

        for record in &self.records {
            frame.extend_from_slice(&record.y.to_le_bytes());
            frame.extend_from_slice(&record.x0.to_le_bytes());
            frame.extend_from_slice(&record.len.to_le_bytes());
            frame.extend_from_slice(&0u16.to_le_bytes());
        }
        frame.resize(cells_offset, 0);
        frame.extend_from_slice(&self.cells);
        frame.resize(graphics_offset, 0);
        frame.extend_from_slice(&self.graphics);
        frame.extend_from_slice(&self.sideband);
        frame
    }
}

pub struct FrameView<'a> {
    header: FrameHeader,
    buffer: &'a [u8],
}

pub fn decode(buffer: &[u8]) -> Result<FrameView<'_>, DecodeError> {
    if buffer.len() < HEADER_LEN {
        return Err(DecodeError::TooShort);
    }
    let u16_at = |offset: usize| u16::from_le_bytes([buffer[offset], buffer[offset + 1]]);
    let u32_at = |offset: usize| {
        u32::from_le_bytes([
            buffer[offset],
            buffer[offset + 1],
            buffer[offset + 2],
            buffer[offset + 3],
        ])
    };
    let version = buffer[0];
    if !(OLDEST_WIRE_VERSION..=WIRE_VERSION).contains(&version) {
        return Err(DecodeError::UnsupportedVersion(version));
    }
    let mut seq_bytes = [0u8; 8];
    seq_bytes.copy_from_slice(&buffer[6..14]);
    let header = FrameHeader {
        version,
        flags: buffer[1],
        cols: u16_at(2),
        rows: u16_at(4),
        seq: u64::from_le_bytes(seq_bytes),
        cursor_x: u16_at(14),
        cursor_y: u16_at(16),
        cursor_shape: buffer[18],
        cursor_visible: buffer[19] != 0,
        row_record_count: u32_at(20),
        cells_offset: u32_at(24),
        cells_len: u32_at(28),
        graphics_offset: u32_at(32),
        graphics_len: u32_at(36),
        sideband_offset: u32_at(40),
        sideband_len: u32_at(44),
    };

    let records_len = (header.row_record_count as usize)
        .checked_mul(ROW_RECORD_LEN)
        .ok_or(DecodeError::BadSection)?;
    let records_end = HEADER_LEN
        .checked_add(records_len)
        .ok_or(DecodeError::BadSection)?;
    if records_end > buffer.len() {
        return Err(DecodeError::BadSection);
    }

    let cells_offset = header.cells_offset as usize;
    let cells_len = header.cells_len as usize;
    let cells_end = cells_offset
        .checked_add(cells_len)
        .ok_or(DecodeError::BadSection)?;
    if cells_offset < records_end || cells_end > buffer.len() {
        return Err(DecodeError::BadSection);
    }
    if !cells_len.is_multiple_of(CELL_LEN) {
        return Err(DecodeError::CellCountMismatch);
    }

    let graphics_offset = header.graphics_offset as usize;
    let graphics_end = graphics_offset
        .checked_add(header.graphics_len as usize)
        .ok_or(DecodeError::BadSection)?;
    if graphics_offset < cells_end || graphics_end > buffer.len() {
        return Err(DecodeError::BadSection);
    }

    let sideband_offset = header.sideband_offset as usize;
    let sideband_end = sideband_offset
        .checked_add(header.sideband_len as usize)
        .ok_or(DecodeError::BadSection)?;
    if sideband_offset < graphics_end || sideband_end > buffer.len() {
        return Err(DecodeError::BadSection);
    }

    let mut at = graphics_offset;
    while at < graphics_end {
        let header_end = at
            .checked_add(GRAPHICS_RECORD_HEADER_LEN)
            .ok_or(DecodeError::BadGraphicsRecord)?;
        if header_end > graphics_end {
            return Err(DecodeError::BadGraphicsRecord);
        }
        let payload_len = u32::from_le_bytes([
            buffer[at + 4],
            buffer[at + 5],
            buffer[at + 6],
            buffer[at + 7],
        ]) as usize;
        let payload_end = header_end
            .checked_add(payload_len)
            .ok_or(DecodeError::BadGraphicsRecord)?;
        if payload_end > graphics_end {
            return Err(DecodeError::BadGraphicsRecord);
        }
        if !record_is_decodable(
            version,
            buffer[at],
            buffer[at + 1],
            &buffer[header_end..payload_end],
        ) {
            return Err(DecodeError::BadGraphicsRecord);
        }
        at = align_up(payload_end, 4);
    }

    let available_cells = cells_len / CELL_LEN;
    let mut named_cells = 0usize;
    for index in 0..header.row_record_count as usize {
        let base = HEADER_LEN + index * ROW_RECORD_LEN;
        let y = u16::from_le_bytes([buffer[base], buffer[base + 1]]);
        let x0 = u16::from_le_bytes([buffer[base + 2], buffer[base + 3]]);
        let len = u16::from_le_bytes([buffer[base + 4], buffer[base + 5]]);
        if y >= header.rows {
            return Err(DecodeError::RowOutOfRange);
        }
        if x0 as usize + len as usize > header.cols as usize {
            return Err(DecodeError::RowOutOfRange);
        }
        named_cells += len as usize;
        if named_cells > available_cells {
            return Err(DecodeError::CellsExhausted);
        }
    }
    if named_cells != available_cells {
        return Err(DecodeError::CellCountMismatch);
    }

    let view = FrameView { header, buffer };
    let mut clusters: Option<ClusterRecord> = None;
    let mut at = cells_offset;
    while at < cells_end {
        let ch = u32::from_le_bytes([buffer[at], buffer[at + 1], buffer[at + 2], buffer[at + 3]]);
        if ch >= CLUSTER_CH_BASE {
            let record = clusters.get_or_insert_with(|| view.clusters());
            let referenced = u16::try_from(ch - CLUSTER_CH_BASE)
                .ok()
                .and_then(|id| record.get(id));
            if referenced.is_none() {
                return Err(DecodeError::BadGraphicsRecord);
            }
        }
        at += CELL_LEN;
    }

    Ok(view)
}

impl<'a> FrameView<'a> {
    pub fn header(&self) -> &FrameHeader {
        &self.header
    }

    pub fn cursor(&self) -> CursorState {
        CursorState {
            x: self.header.cursor_x,
            y: self.header.cursor_y,
            shape: self.header.cursor_shape,
            visible: self.header.cursor_visible,
        }
    }

    pub fn sideband(&self) -> &'a [u8] {
        let start = self.header.sideband_offset as usize;
        &self.buffer[start..start + self.header.sideband_len as usize]
    }

    pub fn graphics(&self) -> Vec<GraphicsRecord> {
        let start = self.header.graphics_offset as usize;
        let end = start + self.header.graphics_len as usize;
        let mut records = Vec::new();
        let mut at = start;
        while at < end {
            let header_end = at + GRAPHICS_RECORD_HEADER_LEN;
            let payload_len = u32::from_le_bytes([
                self.buffer[at + 4],
                self.buffer[at + 5],
                self.buffer[at + 6],
                self.buffer[at + 7],
            ]) as usize;
            let payload_end = header_end + payload_len;
            if let Some(record) = GraphicsRecord::decode(
                self.buffer[at],
                self.buffer[at + 1],
                &self.buffer[header_end..payload_end],
            ) {
                records.push(record);
            }
            at = align_up(payload_end, 4);
        }
        records
    }

    pub fn geometry(&self) -> Option<GeometryRecord> {
        let start = self.header.graphics_offset as usize;
        let end = start + self.header.graphics_len as usize;
        let mut geometry = None;
        let mut at = start;
        while at < end {
            let header_end = at + GRAPHICS_RECORD_HEADER_LEN;
            let payload_len = u32::from_le_bytes([
                self.buffer[at + 4],
                self.buffer[at + 5],
                self.buffer[at + 6],
                self.buffer[at + 7],
            ]) as usize;
            let payload_end = header_end + payload_len;
            if self.buffer[at] == RECORD_KIND_GEOMETRY {
                geometry = GeometryRecord::decode(&self.buffer[header_end..payload_end]);
            }
            at = align_up(payload_end, 4);
        }
        geometry
    }

    pub fn links(&self) -> LinkRecord {
        let start = self.header.graphics_offset as usize;
        let end = start + self.header.graphics_len as usize;
        let mut links = LinkRecord::default();
        let mut at = start;
        while at < end {
            let header_end = at + GRAPHICS_RECORD_HEADER_LEN;
            let payload_len = u32::from_le_bytes([
                self.buffer[at + 4],
                self.buffer[at + 5],
                self.buffer[at + 6],
                self.buffer[at + 7],
            ]) as usize;
            let payload_end = header_end + payload_len;
            if self.buffer[at] == RECORD_KIND_LINKS
                && let Some(record) = LinkRecord::decode(&self.buffer[header_end..payload_end])
            {
                links.entries.extend(record.entries);
            }
            at = align_up(payload_end, 4);
        }
        links
    }

    pub fn clusters(&self) -> ClusterRecord {
        let start = self.header.graphics_offset as usize;
        let end = start + self.header.graphics_len as usize;
        let mut clusters = ClusterRecord::default();
        let mut at = start;
        while at < end {
            let header_end = at + GRAPHICS_RECORD_HEADER_LEN;
            let payload_len = u32::from_le_bytes([
                self.buffer[at + 4],
                self.buffer[at + 5],
                self.buffer[at + 6],
                self.buffer[at + 7],
            ]) as usize;
            let payload_end = header_end + payload_len;
            if self.buffer[at] == RECORD_KIND_CLUSTERS
                && let Some(record) = ClusterRecord::decode(&self.buffer[header_end..payload_end])
            {
                clusters.entries.extend(record.entries);
            }
            at = align_up(payload_end, 4);
        }
        clusters.entries.sort_by_key(|entry| entry.id);
        clusters
    }

    pub fn shifts(&self) -> ShiftRecord {
        let start = self.header.graphics_offset as usize;
        let end = start + self.header.graphics_len as usize;
        let mut shifts = ShiftRecord::default();
        let mut at = start;
        while at < end {
            let header_end = at + GRAPHICS_RECORD_HEADER_LEN;
            let payload_len = u32::from_le_bytes([
                self.buffer[at + 4],
                self.buffer[at + 5],
                self.buffer[at + 6],
                self.buffer[at + 7],
            ]) as usize;
            let payload_end = header_end + payload_len;
            if self.buffer[at] == RECORD_KIND_SHIFT
                && let Some(record) = ShiftRecord::decode(&self.buffer[header_end..payload_end])
            {
                shifts.entries.extend(record.entries);
            }
            at = align_up(payload_end, 4);
        }
        shifts
    }

    pub fn scroll(&self) -> ScrollRecord {
        let start = self.header.graphics_offset as usize;
        let end = start + self.header.graphics_len as usize;
        let mut scroll = ScrollRecord::default();
        let mut at = start;
        while at < end {
            let header_end = at + GRAPHICS_RECORD_HEADER_LEN;
            let payload_len = u32::from_le_bytes([
                self.buffer[at + 4],
                self.buffer[at + 5],
                self.buffer[at + 6],
                self.buffer[at + 7],
            ]) as usize;
            let payload_end = header_end + payload_len;
            if self.buffer[at] == RECORD_KIND_SCROLL
                && let Some(record) = ScrollRecord::decode(&self.buffer[header_end..payload_end])
            {
                for entry in record.entries {
                    scroll.add(entry.pane, entry.lines as i32);
                }
            }
            at = align_up(payload_end, 4);
        }
        scroll
    }

    pub fn record(&self, index: usize) -> Option<RowRecord> {
        (index < self.header.row_record_count as usize).then(|| self.record_at(index))
    }

    fn record_at(&self, index: usize) -> RowRecord {
        let base = HEADER_LEN + index * ROW_RECORD_LEN;
        RowRecord {
            y: u16::from_le_bytes([self.buffer[base], self.buffer[base + 1]]),
            x0: u16::from_le_bytes([self.buffer[base + 2], self.buffer[base + 3]]),
            len: u16::from_le_bytes([self.buffer[base + 4], self.buffer[base + 5]]),
        }
    }

    pub fn rows(&self) -> RowIter<'_, 'a> {
        RowIter {
            view: self,
            index: 0,
            cursor: self.header.cells_offset as usize,
        }
    }

    fn blit(&self, record: RowRecord, at: usize, destination: &mut [WireCell], cols: usize) {
        let start = record.y as usize * cols + record.x0 as usize;
        let len = record.len as usize;
        let bytes = len * CELL_LEN;
        if start + len > destination.len() {
            return;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.buffer.as_ptr().add(at),
                destination.as_mut_ptr().add(start) as *mut u8,
                bytes,
            );
        }
    }

    pub fn apply(&self, destination: &mut [WireCell], cols: usize, rows: usize) {
        if cols != self.header.cols as usize || rows != self.header.rows as usize {
            return;
        }
        let mut at = self.header.cells_offset as usize;
        for index in 0..self.header.row_record_count as usize {
            let record = self.record_at(index);
            self.blit(record, at, destination, cols);
            at += record.len as usize * CELL_LEN;
        }
    }

    pub fn cell(&self, at: usize) -> WireCell {
        let mut chunk = [0u8; CELL_LEN];
        chunk.copy_from_slice(&self.buffer[at..at + CELL_LEN]);
        WireCell {
            ch: u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]),
            fg: u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]),
            bg: u32::from_le_bytes([chunk[8], chunk[9], chunk[10], chunk[11]]),
            underline_color: u32::from_le_bytes([chunk[12], chunk[13], chunk[14], chunk[15]]),
            attrs: u16::from_le_bytes([chunk[16], chunk[17]]),
            width: chunk[18],
            link: chunk[19],
        }
    }
}

pub struct RowIter<'v, 'a> {
    view: &'v FrameView<'a>,
    index: usize,
    cursor: usize,
}

impl<'v, 'a> Iterator for RowIter<'v, 'a> {
    type Item = (RowRecord, Vec<WireCell>);

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.view.header.row_record_count as usize {
            return None;
        }
        let record = self.view.record_at(self.index);
        let mut cells = Vec::with_capacity(record.len as usize);
        for offset in 0..record.len as usize {
            cells.push(self.view.cell(self.cursor + offset * CELL_LEN));
        }
        self.index += 1;
        self.cursor += record.len as usize * CELL_LEN;
        Some((record, cells))
    }
}

#[derive(Debug)]
pub struct PendingOverlay {
    cols: usize,
    rows: usize,
    cells: Vec<WireCell>,
    spans: Vec<Vec<(u16, u16)>>,
    cursor: CursorState,
    cursor_seen: bool,
    full_repaint: bool,
    clear: bool,
    geometry: Option<GeometryRecord>,
    links: Vec<LinkEntry>,
    scroll: ScrollRecord,
    graphics: Vec<GraphicsRecord>,
    sideband: Vec<u8>,
    cluster_texts: Vec<String>,
    cluster_ids: std::collections::HashMap<String, u32>,
}

impl PendingOverlay {
    pub fn new(cols: usize, rows: usize) -> Self {
        PendingOverlay {
            cols,
            rows,
            cells: vec![WireCell::BLANK; cols * rows],
            spans: vec![Vec::new(); rows],
            cursor: CursorState::default(),
            cursor_seen: false,
            full_repaint: false,
            clear: false,
            geometry: None,
            links: Vec::new(),
            scroll: ScrollRecord::default(),
            graphics: Vec::new(),
            sideband: Vec::new(),
            cluster_texts: Vec::new(),
            cluster_ids: std::collections::HashMap::new(),
        }
    }

    fn overlay_cluster(&mut self, text: &str) -> u32 {
        if let Some(id) = self.cluster_ids.get(text) {
            return *id;
        }
        let id = self.cluster_texts.len() as u32;
        self.cluster_texts.push(text.to_owned());
        self.cluster_ids.insert(text.to_owned(), id);
        id
    }

    pub fn dimensions(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        *self = PendingOverlay::new(cols, rows);
    }

    pub fn is_dirty(&self) -> bool {
        self.cursor_seen
            || self.full_repaint
            || self.clear
            || self.geometry.is_some()
            || !self.links.is_empty()
            || !self.scroll.is_empty()
            || !self.graphics.is_empty()
            || !self.sideband.is_empty()
            || self.spans.iter().any(|spans| !spans.is_empty())
    }

    fn merge_row(&mut self, y: u16, x0: u16, cells: &[WireCell]) {
        let row = y as usize;
        if row >= self.rows || cells.is_empty() {
            return;
        }
        let start = x0 as usize;
        let end = (start + cells.len()).min(self.cols);
        if start >= end {
            return;
        }
        self.cells[row * self.cols + start..row * self.cols + end]
            .copy_from_slice(&cells[..end - start]);
        insert_span(&mut self.spans[row], start as u16, end as u16);
    }

    pub fn merge_frame(&mut self, frame: &FrameView<'_>) {
        let header = frame.header();
        if header.cols as usize != self.cols || header.rows as usize != self.rows {
            self.resize(header.cols as usize, header.rows as usize);
        }
        if header.full_repaint() {
            self.full_repaint = true;
            self.links.clear();
        }
        if header.clear() {
            self.clear = true;
        }
        if header.full_repaint() || header.clear() {
            self.scroll.entries.clear();
            self.cells
                .iter_mut()
                .for_each(|cell| *cell = WireCell::BLANK);
            self.spans.iter_mut().for_each(Vec::clear);
        }
        let clusters = frame.clusters();
        for (record, mut cells) in frame.rows() {
            if !clusters.is_empty() {
                for cell in cells.iter_mut() {
                    if let Some(id) = cell.cluster_id() {
                        let text = clusters.get(id).unwrap_or(" ").to_owned();
                        cell.ch = CLUSTER_CH_BASE + self.overlay_cluster(&text);
                    }
                }
            }
            self.merge_row(record.y, record.x0, &cells);
        }
        self.cursor = frame.cursor();
        self.cursor_seen = true;
        if let Some(geometry) = frame.geometry() {
            self.geometry = Some(geometry);
            self.scroll.entries.clear();
        }
        for entry in frame.scroll().entries {
            self.scroll.add(entry.pane, entry.lines as i32);
        }
        for entry in frame.links().entries {
            match self.links.iter_mut().find(|held| held.id == entry.id) {
                Some(held) => held.uri = entry.uri,
                None => self.links.push(entry),
            }
        }
        self.graphics.extend(frame.graphics());
        self.sideband.extend_from_slice(frame.sideband());
    }

    pub fn drain(&mut self, seq: u64) -> Vec<u8> {
        let mut builder = FrameBuilder::new(self.cols as u16, self.rows as u16, seq);
        builder.set_full_repaint(self.full_repaint);
        builder.set_clear(self.clear);
        if self.cursor_seen {
            builder.set_cursor(self.cursor);
        }
        let cluster_texts = std::mem::take(&mut self.cluster_texts);
        self.cluster_ids.clear();
        for row in 0..self.rows {
            for (start, end) in std::mem::take(&mut self.spans[row]) {
                let base = row * self.cols;
                let span = &self.cells[base + start as usize..base + end as usize];
                if span.iter().any(|cell| cell.ch >= CLUSTER_CH_BASE) {
                    let remapped: Vec<WireCell> = span
                        .iter()
                        .map(|cell| {
                            let mut cell = *cell;
                            if cell.ch >= CLUSTER_CH_BASE {
                                let text = cluster_texts
                                    .get((cell.ch - CLUSTER_CH_BASE) as usize)
                                    .map(|text| text.as_str())
                                    .unwrap_or(" ");
                                cell.ch = builder
                                    .cluster(text)
                                    .unwrap_or_else(|| text.chars().next().unwrap_or(' ') as u32);
                            }
                            cell
                        })
                        .collect();
                    builder.push_row(row as u16, start, &remapped);
                } else {
                    builder.push_row(row as u16, start, span);
                }
            }
        }
        let scroll = std::mem::take(&mut self.scroll);
        if let Some(geometry) = self.geometry.take() {
            builder.push_geometry(&geometry);
        } else if !self.full_repaint && !self.clear {
            builder.push_scroll(&scroll);
        }
        builder.push_links(&LinkRecord {
            entries: std::mem::take(&mut self.links),
        });
        builder.extend_graphics(std::mem::take(&mut self.graphics).iter());
        builder
            .sideband_mut()
            .extend_from_slice(&std::mem::take(&mut self.sideband));
        self.cursor_seen = false;
        self.full_repaint = false;
        self.clear = false;
        builder.finish()
    }
}

fn insert_span(spans: &mut Vec<(u16, u16)>, start: u16, end: u16) {
    let mut start = start;
    let mut end = end;
    let mut merged: Vec<(u16, u16)> = Vec::with_capacity(spans.len() + 1);
    let mut inserted = false;
    for (span_start, span_end) in spans.drain(..) {
        if span_end < start {
            merged.push((span_start, span_end));
        } else if span_start > end {
            if !inserted {
                merged.push((start, end));
                inserted = true;
            }
            merged.push((span_start, span_end));
        } else {
            start = start.min(span_start);
            end = end.max(span_end);
        }
    }
    if !inserted {
        merged.push((start, end));
    }
    merged.sort_unstable();
    *spans = merged;
}

#[cfg(test)]
#[path = "unit/structured_render_tests.rs"]
mod tests;
