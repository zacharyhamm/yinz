//! Color values and pure conversion helpers.

use colorsys::Ansi256;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum PaletteColor {
    Rgb((u8, u8, u8)),
    EightBit(u8),
}
impl Default for PaletteColor {
    fn default() -> PaletteColor {
        PaletteColor::EightBit(0)
    }
}

// Colors
pub mod default_colors {
    pub const WHITE: u8 = 255;
    pub const GREEN: u8 = 154;
    pub const GRAY: u8 = 238;
    pub const BRIGHT_GRAY: u8 = 245;
    pub const RED: u8 = 124;
    pub const ORANGE: u8 = 166;
    pub const BLACK: u8 = 16;
    pub const MAGENTA: u8 = 201;
    pub const CYAN: u8 = 51;
    pub const YELLOW: u8 = 226;
    pub const BLUE: u8 = 45;
    pub const PURPLE: u8 = 99;
    pub const GOLD: u8 = 136;
    pub const SILVER: u8 = 245;
    pub const PINK: u8 = 207;
    pub const BROWN: u8 = 215;
}

/// Parse a `#RGB` or `#RRGGBB` color (leading `#` optional) into its channels.
///
/// `None` for anything that is not exactly 3 or 6 ASCII hex digits. Gating on
/// `is_ascii_hexdigit` before slicing is what makes the fixed byte offsets safe:
/// every accepted byte is a single-byte char, so a body like `é1` (3 bytes, 2
/// chars) is refused instead of being split mid-char. Both the KDL theme parser
/// and the serde `HexColor` visitor go through here so the rule lives once.
pub fn parse_hex_rgb(s: &str) -> Option<(u8, u8, u8)> {
    let body = s.strip_prefix('#').unwrap_or(s);
    if !body.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |range: std::ops::Range<usize>| u8::from_str_radix(&body[range], 16).ok();
    match body.len() {
        3 => Some((
            channel(0..1)? * 0x11,
            channel(1..2)? * 0x11,
            channel(2..3)? * 0x11,
        )),
        6 => Some((channel(0..2)?, channel(2..4)?, channel(4..6)?)),
        _ => None,
    }
}

pub fn eightbit_to_rgb(c: u8) -> (u8, u8, u8) {
    Ansi256::new(c).as_rgb().into()
}

#[cfg(test)]
#[path = "unit/colors_tests.rs"]
mod tests;
