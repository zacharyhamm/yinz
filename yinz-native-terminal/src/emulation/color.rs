use yinz_types::structured_render::WireColor;

pub type Srgb = [u8; 3];

pub const DEFAULT_FOREGROUND: Srgb = [229, 229, 229];
pub const DEFAULT_BACKGROUND: Srgb = [0, 0, 0];

pub const ANSI_16: [Srgb; 16] = [
    [0, 0, 0],
    [205, 0, 0],
    [0, 205, 0],
    [205, 205, 0],
    [0, 0, 238],
    [205, 0, 205],
    [0, 205, 205],
    [229, 229, 229],
    [127, 127, 127],
    [255, 0, 0],
    [0, 255, 0],
    [255, 255, 0],
    [92, 92, 255],
    [255, 0, 255],
    [0, 255, 255],
    [255, 255, 255],
];

const DIM_NUMERATOR: u32 = 2;
const DIM_DENOMINATOR: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Paints {
    pub foreground: Srgb,
    pub background: Srgb,
    pub cursor: Option<Srgb>,
    pub ansi: [Srgb; 16],
}

impl Default for Paints {
    fn default() -> Self {
        Self {
            foreground: DEFAULT_FOREGROUND,
            background: DEFAULT_BACKGROUND,
            cursor: None,
            ansi: ANSI_16,
        }
    }
}

impl Paints {
    pub fn cursor_over(&self, cell_foreground: Srgb) -> Srgb {
        self.cursor.unwrap_or(cell_foreground)
    }

    pub fn beam_cursor(&self) -> Srgb {
        self.cursor.unwrap_or(self.foreground)
    }

    pub fn indexed(&self, index: u8) -> Srgb {
        match index {
            0..=15 => self.ansi[index as usize],
            _ => extended(index),
        }
    }

    pub fn foreground(&self, packed: u32) -> Srgb {
        self.wire(packed, self.foreground)
    }

    pub fn background(&self, packed: u32) -> Srgb {
        self.wire(packed, self.background)
    }

    fn wire(&self, packed: u32, default: Srgb) -> Srgb {
        match WireColor::unpack(packed) {
            WireColor::Default => default,
            WireColor::Named(index) => self.ansi[(index as usize).min(15)],
            WireColor::Indexed(index) => self.indexed(index),
            WireColor::Rgb(r, g, b) => [r, g, b],
        }
    }
}

fn extended(index: u8) -> Srgb {
    match index {
        0..=15 => ANSI_16[index as usize],
        16..=231 => {
            let offset = (index - 16) as u32;
            let level = |step: u32| {
                if step == 0 {
                    0u8
                } else {
                    (55 + step * 40) as u8
                }
            };
            [
                level(offset / 36),
                level((offset / 6) % 6),
                level(offset % 6),
            ]
        },
        232..=255 => {
            let level = 8u8 + 10 * (index - 232);
            [level, level, level]
        },
    }
}

pub fn dim(color: Srgb) -> Srgb {
    color.map(|channel| ((channel as u32 * DIM_NUMERATOR) / DIM_DENOMINATOR) as u8)
}

const WHITE: Srgb = [255, 255, 255];
const BLACK: Srgb = [0, 0, 0];
const BLEND_STEPS: u32 = 255;
const MAX_CACHED_PAIRS: usize = 4096;

fn linear(channel: u8) -> f64 {
    let value = channel as f64 / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

pub fn relative_luminance(color: Srgb) -> f64 {
    0.2126 * linear(color[0]) + 0.7152 * linear(color[1]) + 0.0722 * linear(color[2])
}

pub fn contrast_ratio(first: Srgb, second: Srgb) -> f64 {
    let (a, b) = (relative_luminance(first), relative_luminance(second));
    let (light, dark) = if a >= b { (a, b) } else { (b, a) };
    (light + 0.05) / (dark + 0.05)
}

fn blend(from: Srgb, to: Srgb, step: u32) -> Srgb {
    let mut out = from;
    for channel in 0..3 {
        let (start, end) = (from[channel] as i32, to[channel] as i32);
        out[channel] = (start
            + ((end - start) * step as i32 + BLEND_STEPS as i32 / 2) / BLEND_STEPS as i32)
            as u8;
    }
    out
}

fn steps_to_reach(foreground: Srgb, background: Srgb, target: Srgb, minimum: f64) -> Option<u32> {
    if contrast_ratio(target, background) < minimum {
        return None;
    }
    (0..=BLEND_STEPS)
        .find(|step| contrast_ratio(blend(foreground, target, *step), background) >= minimum)
}

pub fn with_minimum_contrast(foreground: Srgb, background: Srgb, minimum: f64) -> Srgb {
    if minimum <= 1.0 || contrast_ratio(foreground, background) >= minimum {
        return foreground;
    }
    let lighter = steps_to_reach(foreground, background, WHITE, minimum);
    let darker = steps_to_reach(foreground, background, BLACK, minimum);
    match (lighter, darker) {
        (Some(up), Some(down)) if down < up => blend(foreground, BLACK, down),
        (Some(up), _) => blend(foreground, WHITE, up),
        (None, Some(down)) => blend(foreground, BLACK, down),
        (None, None) => {
            if contrast_ratio(WHITE, background) >= contrast_ratio(BLACK, background) {
                WHITE
            } else {
                BLACK
            }
        },
    }
}

#[derive(Debug, Default)]
pub struct Contrast {
    minimum: f32,
    cache: std::cell::RefCell<std::collections::HashMap<(Srgb, Srgb), Srgb>>,
}

impl Contrast {
    pub fn new(minimum: f32) -> Self {
        Self {
            minimum,
            cache: Default::default(),
        }
    }

    pub fn minimum(&self) -> f32 {
        self.minimum
    }

    pub fn is_active(&self) -> bool {
        self.minimum > 1.0
    }

    pub fn apply(&self, foreground: Srgb, background: Srgb) -> Srgb {
        if !self.is_active() {
            return foreground;
        }
        let mut cache = self.cache.borrow_mut();
        if let Some(adjusted) = cache.get(&(foreground, background)) {
            return *adjusted;
        }
        if cache.len() >= MAX_CACHED_PAIRS {
            cache.clear();
        }
        let adjusted = with_minimum_contrast(foreground, background, self.minimum as f64);
        cache.insert((foreground, background), adjusted);
        adjusted
    }
}

#[cfg(test)]
pub(crate) mod test_helpers;

#[cfg(test)]
#[path = "unit/color_tests.rs"]
mod tests;
