use super::geometry::{Sample, GLYPHS};
use super::outlines::OUTLINES;
use std::time::Duration;
use unicode_width::UnicodeWidthStr;
use yinz_types::colors::{eightbit_to_rgb, PaletteColor};
use yinz_types::pane_size::Size;
use yinz_types::style::StyleDeclaration;

const BITS: [u8; 8] = [1, 8, 2, 16, 4, 32, 64, 128];
const MIN_CONTRAST: f64 = 4.5;
const QUOTE: &str = "for if every tool could perform its own work when ordered, or by seeing what \
    to do in advance, like the statues of Daedalus in the story, or the tripods of Hephaestus \
    which the poet says 'enter self-moved the company divine.' If thus shuttles wove and quills \
    played harps of themselves, master-craftsmen would have no need of assistants and masters \
    no need of slaves.";
const ATTRIBUTION: &str = "- Aristotle, Politics 1253b";

fn quote_lines(width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in [QUOTE, ATTRIBUTION] {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && line.width() + 1 + word.width() > width {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    lines
}

type Rgb = [f32; 3];

#[derive(Clone, Copy)]
struct Dot {
    depth: f32,
    color: Rgb,
}

impl Default for Dot {
    fn default() -> Self {
        Self {
            depth: f32::NEG_INFINITY,
            color: [0.0; 3],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Stage {
    pub cols: usize,
    pub rows: usize,
    left: usize,
    top: usize,
    quote_rows: usize,
}

impl Stage {
    pub fn new(size: Size) -> Option<Self> {
        let scale = ((size.cols.saturating_sub(2)) as f32 / 52.0).min(1.0);
        if scale < 0.5 {
            return None;
        }
        let cols = (52.0 * scale).floor() as usize;
        // Trim the logo's vertical padding to leave room for the quotation,
        // without changing the letter proportions or the width of its stage.
        let rows = (12.0 * scale).ceil() as usize;
        let quote_rows = quote_lines(cols).len();
        let height = rows + 1 + quote_rows;
        if height <= size.rows.saturating_sub(2) {
            return Some(Self {
                cols,
                rows,
                left: (size.cols - cols) / 2 + 1,
                top: (size.rows - height) / 2 + 1,
                quote_rows,
            });
        }
        // Short terminals retain the logo rather than clipping the quotation.
        let scale = ((size.cols.saturating_sub(2)) as f32 / 52.0)
            .min((size.rows.saturating_sub(2)) as f32 / 16.0)
            .min(1.0);
        if scale < 0.5 {
            return None;
        }
        let cols = (52.0 * scale).floor() as usize;
        let rows = (16.0 * scale).floor() as usize;
        Some(Self {
            cols,
            rows,
            left: (size.cols - cols) / 2 + 1,
            top: (size.rows - rows) / 2 + 1,
            quote_rows: 0,
        })
    }
}

pub(crate) struct Renderer {
    pub stage: Stage,
    background: Rgb,
    background_luminance: f64,
    foreground: Rgb,
    highlight: Rgb,
    accent: Rgb,
    dots: Vec<Dot>,
    quote: Vec<String>,
}

impl Renderer {
    pub fn new(stage: Stage, style: StyleDeclaration, accent: PaletteColor) -> Self {
        let background = rgb(style.background);
        let background_luminance = luminance(background);
        let highlight = if contrast(0.0, background_luminance) > contrast(1.0, background_luminance)
        {
            [0.0; 3]
        } else {
            [255.0; 3]
        };
        Self {
            stage,
            background,
            background_luminance,
            foreground: rgb(style.base),
            highlight,
            accent: rgb(accent),
            dots: vec![Dot::default(); stage.cols * stage.rows * 8],
            quote: if stage.quote_rows > 0 {
                quote_lines(stage.cols)
            } else {
                Vec::new()
            },
        }
    }

    pub fn resize(&mut self, stage: Stage) {
        self.stage = stage;
        self.quote = if stage.quote_rows > 0 {
            quote_lines(stage.cols)
        } else {
            Vec::new()
        };
        self.dots
            .resize(stage.cols * stage.rows * 8, Dot::default());
    }

    pub fn frame(&mut self, elapsed: Duration) -> String {
        self.dots.fill(Dot::default());
        let width = OUTLINES
            .iter()
            .flat_map(|o| o.segments.iter().map(move |s| (s[0] + o.center).abs()))
            .fold(0.0, f32::max)
            * 2.0;
        let vertical_extent = if self.stage.quote_rows > 0 { 2.2 } else { 2.9 };
        let scale = (self.stage.cols as f32 * 2.0 / (width + 0.9))
            .min(self.stage.rows as f32 * 4.0 / vertical_extent);
        let (sin, cos) = angle(elapsed).sin_cos();
        for glyph in GLYPHS.iter() {
            for sample in &glyph.samples {
                self.project(sample, glyph.center, sin, cos, scale, elapsed, width);
            }
        }
        let [r, g, b] = self.background.map(|c| c.round() as u8);
        let mut output = format!("\x1b[m\x1b[48;2;{r};{g};{b}m\x1b[2J\x1b[H");
        for row in 0..self.stage.rows {
            output.push_str(&format!(
                "\x1b[{};{}H",
                self.stage.top + row,
                self.stage.left
            ));
            for col in 0..self.stage.cols {
                let mut dots = [Dot::default(); 8];
                for y in 0..4 {
                    for x in 0..2 {
                        dots[y * 2 + x] =
                            self.dots[(row * 4 + y) * self.stage.cols * 2 + col * 2 + x];
                    }
                }
                let (mask, color) = pack(&dots);
                if mask == 0 {
                    output.push(' ');
                } else {
                    let [r, g, b] =
                        ensure_contrast(color, self.background_luminance, self.highlight);
                    output.push_str(&format!(
                        "\x1b[38;2;{r};{g};{b}m{}",
                        char::from_u32(0x2800 + u32::from(mask)).unwrap_or(' ')
                    ));
                }
            }
        }
        if !self.quote.is_empty() {
            let [r, g, b] = ensure_contrast(
                self.foreground.map(|channel| channel.round() as u8),
                self.background_luminance,
                self.highlight,
            );
            output.push_str(&format!("\x1b[38;2;{r};{g};{b}m"));
            for (row, line) in self.quote.iter().enumerate() {
                output.push_str(&format!(
                    "\x1b[{};{}H{line}",
                    self.stage.top + self.stage.rows + 1 + row,
                    self.stage.left + (self.stage.cols - line.width()) / 2,
                ));
            }
        }
        output.push_str("\x1b[m");
        output
    }

    fn project(
        &mut self,
        sample: &Sample,
        center: f32,
        sin: f32,
        cos: f32,
        scale: f32,
        elapsed: Duration,
        width: f32,
    ) {
        let [x, y, z] = sample.position;
        let z = -sin * x + cos * z;
        let world_x = center + cos * x + sin * sample.position[2];
        let perspective = 8.0 / (8.0 - z);
        let dx = (self.stage.cols as f32 + world_x * perspective * scale).round() as isize;
        let dy = (self.stage.rows as f32 * 2.0 + y * perspective * scale).round() as isize;
        if dx < 0
            || dy < 0
            || dx >= (self.stage.cols * 2) as isize
            || dy >= (self.stage.rows * 4) as isize
        {
            return;
        }
        let index = dy as usize * self.stage.cols * 2 + dx as usize;
        if z <= self.dots[index].depth {
            return;
        }
        let [nx, ny, nz] = sample.normal;
        let normal = [cos * nx + sin * nz, ny, -sin * nx + cos * nz];
        let diffuse = (-0.41 * normal[0] - 0.56 * normal[1] + 0.72 * normal[2]).max(0.0);
        let gloss = (-0.22 * normal[0] - 0.30 * normal[1] + 0.93 * normal[2])
            .max(0.0)
            .powi(18);
        let base = mix(self.background, self.foreground, 0.45 + 0.55 * diffuse);
        let lit = mix(base, self.highlight, 0.22 * gloss);
        let color = mix(lit, self.accent, shimmer(elapsed, world_x, width) * 0.8);
        self.dots[index] = Dot { depth: z, color };
    }
}

pub(super) fn angle(elapsed: Duration) -> f32 {
    let t = elapsed.as_secs_f32().clamp(0.0, 1.0);
    // Return exactly to the resting pose after a full turn.
    if t == 1.0 {
        0.0
    } else {
        smooth(t) * std::f32::consts::TAU
    }
}

fn smooth(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

pub(super) fn shimmer(elapsed: Duration, x: f32, width: f32) -> f32 {
    let t = elapsed.as_secs_f32();
    if !(0.0..1.0).contains(&t) {
        return 0.0;
    }
    let center = (t - 0.5) * (width + 1.2);
    let distance = ((x - center) / 0.45).abs().min(1.0);
    1.0 - smooth(distance)
}

fn rgb(color: PaletteColor) -> Rgb {
    let (r, g, b) = match color {
        PaletteColor::Rgb(rgb) => rgb,
        PaletteColor::EightBit(index) => eightbit_to_rgb(index),
    };
    [r as f32, g as f32, b as f32]
}

fn mix(a: Rgb, b: Rgb, weight: f32) -> Rgb {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * weight)
}

fn luminance(color: Rgb) -> f64 {
    let [r, g, b] = color.map(|channel| {
        let srgb = f64::from(channel) / 255.0;
        if srgb <= 0.04045 {
            srgb / 12.92
        } else {
            ((srgb + 0.055) / 1.055).powf(2.4)
        }
    });
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn contrast(a: f64, b: f64) -> f64 {
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn ensure_contrast(color: [u8; 3], background_luminance: f64, highlight: Rgb) -> [u8; 3] {
    let readable = |color: [u8; 3]| {
        contrast(luminance(color.map(f32::from)), background_luminance) >= MIN_CONTRAST
    };
    if readable(color) {
        return color;
    }
    let color = color.map(f32::from);
    let blended = |weight| mix(color, highlight, weight).map(|channel| channel.round() as u8);
    let (mut low, mut high) = (0.0, 1.0);
    // Search the actual emitted bytes: rounding an otherwise readable float
    // color can drop its contrast below the floor. The endpoint chosen for
    // greater contrast always meets this floor.
    for _ in 0..16 {
        let middle = (low + high) / 2.0;
        if readable(blended(middle)) {
            high = middle;
        } else {
            low = middle;
        }
    }
    blended(high)
}

fn pack(dots: &[Dot; 8]) -> (u8, [u8; 3]) {
    let mut mask = 0;
    let mut sum = [0.0; 3];
    let mut count = 0;
    for (i, dot) in dots.iter().enumerate() {
        if dot.depth.is_finite() {
            mask |= BITS[i];
            for (sum, value) in sum.iter_mut().zip(dot.color) {
                *sum += value;
            }
            count += 1;
        }
    }
    (
        mask,
        sum.map(|value| (value / count.max(1) as f32).round() as u8),
    )
}

#[cfg(test)]
#[path = "../unit/splash_renderer_tests.rs"]
mod tests;
