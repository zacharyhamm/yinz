use std::collections::BTreeMap;
use std::rc::Rc;

use sixel_tokenizer::{ColorCoordinateSystem, Parser, SixelEvent};

use crate::emulation::color;
use crate::emulation::damage::Span;
use crate::emulation::kitty::Image;

const MAX_DIMENSION: usize = 10_000;
const MAX_IMAGE_BYTES: usize = 104_857_600;
const MAX_CHUNKS: usize = 256;
const MAX_REPEAT: usize = MAX_DIMENSION;
const BAND: usize = 6;

const DEFAULT_REGISTERS: [[usize; 3]; 16] = [
    [0, 0, 0],
    [20, 20, 80],
    [80, 13, 13],
    [20, 80, 20],
    [80, 20, 80],
    [20, 80, 80],
    [80, 80, 20],
    [53, 53, 53],
    [26, 26, 26],
    [33, 33, 60],
    [60, 26, 26],
    [33, 60, 33],
    [60, 33, 60],
    [33, 60, 60],
    [60, 60, 33],
    [80, 80, 80],
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellSize {
    pub width: u32,
    pub height: u32,
}

impl CellSize {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width: width.max(1),
            height: height.max(1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl PixelRect {
    fn right(&self) -> u32 {
        self.x.saturating_add(self.width)
    }

    fn bottom(&self) -> u32 {
        self.y.saturating_add(self.height)
    }

    fn intersection(&self, other: &PixelRect) -> Option<PixelRect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        if right <= x || bottom <= y {
            return None;
        }
        Some(PixelRect {
            x,
            y,
            width: right - x,
            height: bottom - y,
        })
    }
}

impl Span {
    fn rect(&self, cell: CellSize) -> PixelRect {
        let x = (self.left as u32).saturating_mul(cell.width);
        let right = (self.right as u32)
            .saturating_add(1)
            .saturating_mul(cell.width);
        PixelRect {
            x,
            y: (self.line as u32).saturating_mul(cell.height),
            width: right.saturating_sub(x),
            height: cell.height,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Chunk {
    pub id: u64,
    pub cell_x: usize,
    pub cell_y: usize,
    pub width: u32,
    pub height: u32,
    pub revision: u64,
    pub image: Rc<Image>,
    painted: usize,
}

impl Chunk {
    pub fn rect(&self, cell: CellSize) -> PixelRect {
        PixelRect {
            x: (self.cell_x as u32).saturating_mul(cell.width),
            y: (self.cell_y as u32).saturating_mul(cell.height),
            width: self.width,
            height: self.height,
        }
    }

    fn erase(&mut self, overlap: PixelRect, origin: PixelRect, revision: u64) {
        let local_x = (overlap.x - origin.x) as usize;
        let local_y = (overlap.y - origin.y) as usize;
        let stride = self.width as usize;
        let image = Rc::make_mut(&mut self.image);

        for row in local_y..local_y + overlap.height as usize {
            let start = (row * stride + local_x) * 4;
            let end = start + overlap.width as usize * 4;
            for pixel in image.pixels[start..end].as_chunks_mut::<4>().0 {
                if pixel[3] != 0 {
                    self.painted -= 1;
                }
                pixel.copy_from_slice(&[0, 0, 0, 0]);
            }
        }
        self.revision = revision;
    }
}

pub struct Sixels {
    chunks: BTreeMap<u64, Chunk>,
    pending: Vec<(usize, usize, Image)>,
    cell: CellSize,
    next_id: u64,
    revisions: u64,
    stamp: u64,
}

impl Sixels {
    pub fn new(cell: CellSize) -> Self {
        Self {
            chunks: BTreeMap::new(),
            pending: Vec::new(),
            cell,
            next_id: 0,
            revisions: 0,
            stamp: 0,
        }
    }

    pub fn stamp(&self) -> u64 {
        self.stamp
    }

    pub fn set_cell_size(&mut self, cell: CellSize) {
        if cell == self.cell {
            return;
        }
        self.cell = cell;
        self.clear();
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty() && self.pending.is_empty()
    }

    pub fn chunks(&self) -> impl Iterator<Item = &Chunk> {
        self.chunks.values()
    }

    pub fn resident_ids(&self) -> Vec<u64> {
        self.chunks.keys().copied().collect()
    }

    pub fn clear(&mut self) {
        self.stamp += 1;
        self.chunks.clear();
        self.pending.clear();
    }

    pub fn execute(&mut self, payload: &[u8], cursor: (usize, usize)) {
        let Some(image) = decode(payload) else {
            return;
        };
        self.pending.push((cursor.1, cursor.0, image));
    }

    pub fn cut_spans(&mut self, spans: &[Span]) {
        if self.chunks.is_empty() {
            return;
        }
        for span in spans {
            self.cut(span.rect(self.cell));
        }
    }

    pub fn commit(&mut self) {
        let pending = std::mem::take(&mut self.pending);
        for (cell_x, cell_y, image) in pending {
            let painted = image
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|pixel| pixel[3] != 0)
                .count();
            self.next_id += 1;
            self.revisions += 1;
            self.stamp += 1;
            let chunk = Chunk {
                id: self.next_id,
                cell_x,
                cell_y,
                width: image.width,
                height: image.height,
                revision: self.revisions,
                image: Rc::new(image),
                painted,
            };
            self.cut(chunk.rect(self.cell));
            self.chunks.insert(chunk.id, chunk);
            while self.chunks.len() > MAX_CHUNKS {
                let oldest = *self.chunks.keys().next().unwrap_or(&0);
                self.chunks.remove(&oldest);
            }
        }
    }

    fn cut(&mut self, rect: PixelRect) {
        let cell = self.cell;
        let mut revisions = self.revisions;
        self.chunks.retain(|_, chunk| {
            let origin = chunk.rect(cell);
            let Some(overlap) = origin.intersection(&rect) else {
                return true;
            };
            revisions += 1;
            chunk.erase(overlap, origin, revisions);
            chunk.painted > 0
        });
        if revisions != self.revisions {
            self.stamp += 1;
        }
        self.revisions = revisions;
    }
}

pub fn decode(payload: &[u8]) -> Option<Image> {
    let mut raster = Raster::new();
    let mut parser = Parser::new();
    for byte in payload {
        parser.advance(byte, |event| raster.event(event));
    }
    for byte in b"\x1b\\" {
        parser.advance(byte, |event| raster.event(event));
    }
    raster.finish()
}

struct Raster {
    registers: BTreeMap<u16, [u8; 3]>,
    current: u16,
    x: usize,
    band_y: usize,
    rows: Vec<Vec<[u8; 4]>>,
    background: [u8; 4],
    declared: Option<(usize, usize)>,
    refused: bool,
}

impl Raster {
    fn new() -> Self {
        let background = {
            let [red, green, blue] = color::DEFAULT_BACKGROUND;
            [red, green, blue, 255]
        };
        Self {
            registers: default_registers(),
            current: 0,
            x: 0,
            band_y: 0,
            rows: Vec::new(),
            background,
            declared: None,
            refused: false,
        }
    }

    fn event(&mut self, event: SixelEvent) {
        if self.refused {
            return;
        }
        match event {
            SixelEvent::Dcs {
                transparent_background,
                ..
            } => {
                if transparent_background == Some(1) {
                    self.background = [0, 0, 0, 0];
                }
            },
            SixelEvent::RasterAttribute {
                ph: Some(width),
                pv: Some(height),
                ..
            } => self.declare(width, height),
            SixelEvent::RasterAttribute { .. } => {},
            SixelEvent::ColorIntroducer {
                color_number,
                color_coordinate_system,
            } => {
                if let Some(system) = color_coordinate_system {
                    self.registers.insert(color_number, resolve(system));
                }
                self.current = color_number;
            },
            SixelEvent::Data { byte } => self.data(byte, 1),
            SixelEvent::Repeat {
                repeat_count,
                byte_to_repeat,
            } => self.data(byte_to_repeat, repeat_count.min(MAX_REPEAT)),
            SixelEvent::GotoBeginningOfLine => self.x = 0,
            SixelEvent::GotoNextLine => {
                self.x = 0;
                self.band_y += BAND;
            },
            SixelEvent::End | SixelEvent::UnknownSequence(_) => {},
        }
    }

    fn declare(&mut self, width: usize, height: usize) {
        if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
            self.refuse(format!("a sixel image of {}x{} px", width, height));
            return;
        }
        if width * height * 4 > MAX_IMAGE_BYTES {
            self.refuse(format!("a sixel image of {} bytes", width * height * 4));
            return;
        }
        self.declared = Some((width, height));
    }

    fn refuse(&mut self, what: String) {
        report!("refusing {}", what);
        self.refused = true;
        self.rows.clear();
    }

    fn data(&mut self, byte: u8, count: usize) {
        if !(0x3f..=0x7e).contains(&byte) {
            return;
        }
        let bits = byte - 0x3f;
        let color = self.color();
        for _ in 0..count {
            if self.x >= MAX_DIMENSION {
                return;
            }
            for bit in 0..BAND {
                if bits & (1 << bit) != 0 {
                    self.paint(self.x, self.band_y + bit, color);
                }
            }
            self.x += 1;
        }
    }

    fn color(&self) -> [u8; 4] {
        let [red, green, blue] = self
            .registers
            .get(&self.current)
            .copied()
            .unwrap_or([0, 0, 0]);
        [red, green, blue, 255]
    }

    fn paint(&mut self, x: usize, y: usize, color: [u8; 4]) {
        if x >= MAX_DIMENSION || y >= MAX_DIMENSION {
            return;
        }
        if let Some((width, height)) = self.declared
            && (x >= width || y >= height)
        {
            return;
        }
        while self.rows.len() <= y {
            self.rows.push(Vec::new());
        }
        let row = &mut self.rows[y];
        if row.len() <= x {
            row.resize(x + 1, self.background);
        }
        row[x] = color;
    }

    fn finish(self) -> Option<Image> {
        if self.refused {
            return None;
        }
        let width = match self.declared {
            Some((width, _)) => width,
            None => self.rows.iter().map(|row| row.len()).max().unwrap_or(0),
        };
        let height = match self.declared {
            Some((_, height)) => height,
            None => self.rows.len(),
        };
        if width == 0 || height == 0 {
            return None;
        }
        if width * height * 4 > MAX_IMAGE_BYTES {
            report!("refusing a sixel image of {} bytes", width * height * 4);
            return None;
        }

        let mut pixels = Vec::with_capacity(width * height * 4);
        for row in 0..height {
            let painted = self.rows.get(row);
            for column in 0..width {
                let pixel = painted
                    .and_then(|row| row.get(column))
                    .copied()
                    .unwrap_or(self.background);
                pixels.extend_from_slice(&pixel);
            }
        }

        Some(Image {
            width: width as u32,
            height: height as u32,
            pixels,
        })
    }
}

fn default_registers() -> BTreeMap<u16, [u8; 3]> {
    DEFAULT_REGISTERS
        .iter()
        .enumerate()
        .map(|(register, [red, green, blue])| {
            (
                register as u16,
                [percent(*red), percent(*green), percent(*blue)],
            )
        })
        .collect()
}

fn resolve(system: ColorCoordinateSystem) -> [u8; 3] {
    match system {
        ColorCoordinateSystem::RGB(red, green, blue) => {
            [percent(red), percent(green), percent(blue)]
        },
        ColorCoordinateSystem::HLS(hue, lightness, saturation) => {
            hls_to_rgb(hue, lightness, saturation)
        },
    }
}

fn percent(value: usize) -> u8 {
    ((value.min(100) * 255 + 50) / 100) as u8
}

fn hls_to_rgb(hue: usize, lightness: usize, saturation: usize) -> [u8; 3] {
    let hue = ((hue % 360) + 240) % 360;
    let lightness = lightness.min(100) as f32 / 100.0;
    let saturation = saturation.min(100) as f32 / 100.0;

    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let sector = hue as f32 / 60.0;
    let second = chroma * (1.0 - (sector % 2.0 - 1.0).abs());
    let (red, green, blue) = match hue / 60 {
        0 => (chroma, second, 0.0),
        1 => (second, chroma, 0.0),
        2 => (0.0, chroma, second),
        3 => (0.0, second, chroma),
        4 => (second, 0.0, chroma),
        _ => (chroma, 0.0, second),
    };
    let base = lightness - chroma / 2.0;

    [
        channel(red + base),
        channel(green + base),
        channel(blue + base),
    ]
}

fn channel(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
#[path = "unit/sixel_tests.rs"]
mod tests;
