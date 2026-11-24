//! Settings shared with the native terminal client.
use serde::{Deserialize, Serialize};

/// Which layers `opacity` applies to. Unset means [`OpacityMode::Background`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpacityMode {
    /// Only the default background (and default padding) shows through, including
    /// a session theme's terminal background in the window's own background
    /// colour; text, coloured cells, the cursor and images stay solid.
    #[default]
    Background,
    /// The whole window fades evenly.
    Everything,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionAsAlt {
    #[default]
    None,
    Both,
    Left,
    Right,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, strum_macros::EnumString,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum StartupMode {
    #[default]
    Windowed,
    Maximized,
    Fullscreen,
    Remember,
}

/// What the window padding shows. Unset means [`PaddingColor::Background`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaddingColor {
    /// The window clear colour.
    #[default]
    Background,
    /// Continues the background of the nearest edge cell on every side.
    Extend,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalConfig {
    #[serde(rename = "font", alias = "font_family")]
    pub font_family: Option<String>,
    pub font_bold: Option<String>,
    pub font_italic: Option<String>,
    pub font_bold_italic: Option<String>,
    pub font_size: Option<f32>,
    pub font_weight: Option<u16>,
    #[serde(rename = "ligatures", alias = "font_ligatures")]
    pub font_ligatures: Option<bool>,
    /// OpenType features in list order: `None` inherits (no explicit features),
    /// `Some(empty)` explicitly requests none. See [`parse_font_feature`].
    pub font_features: Option<Vec<String>>,
    pub system_fonts: Option<bool>,
    pub line_height: Option<f32>,
    pub cell_width: Option<f32>,
    /// Logical pixels, positive downwards; KDL accepts -64..=64.
    pub baseline_offset: Option<i32>,
    /// Logical pixels, positive downwards; KDL accepts -64..=64.
    pub underline_offset: Option<i32>,
    /// Logical pixels added to the font stroke; KDL accepts -64..=64.
    pub underline_thickness: Option<i32>,
    /// Logical pixels on every side; KDL accepts 0..=256. A per-side value
    /// overrides it, see [`TerminalConfig::padding_sides`].
    pub padding: Option<f32>,
    pub padding_top: Option<f32>,
    pub padding_right: Option<f32>,
    pub padding_bottom: Option<f32>,
    pub padding_left: Option<f32>,
    /// Centers the grid in the space left after padding; unset means `false`.
    pub padding_balance: Option<bool>,
    /// Padding paint mode; unset means [`PaddingColor::Background`].
    pub padding_color: Option<PaddingColor>,
    /// Minimum WCAG contrast ratio between displayed text and its background;
    /// KDL accepts [`MIN_CONTRAST`]`..=`[`MAX_CONTRAST`]. Unset means 1.0 (off),
    /// see [`TerminalConfig::minimum_contrast_ratio`].
    pub minimum_contrast: Option<f32>,
    /// Window opacity; KDL accepts 0..=1. Unset means 1.0 (opaque), see
    /// [`TerminalConfig::opacity_value`].
    pub opacity: Option<f32>,
    /// Which layers `opacity` applies to; unset means
    /// [`OpacityMode::Background`], see [`TerminalConfig::opacity_mode_or_default`].
    pub opacity_mode: Option<OpacityMode>,
    pub blur: Option<bool>,
    pub startup_mode: Option<StartupMode>,
    pub initial_columns: Option<u16>,
    pub initial_rows: Option<u16>,
    pub cursor_style: Option<CursorStyle>,
    pub macos_option_as_alt: Option<OptionAsAlt>,
    pub cursor_blink: Option<bool>,
    pub cursor_trail: Option<bool>,
    /// Whether an unfocused window draws an outlined block cursor that does
    /// not blink. Unset means true, see
    /// [`TerminalConfig::cursor_unfocused_hollow_enabled`].
    pub cursor_unfocused_hollow: Option<bool>,
    pub open_links: Option<bool>,
    pub middle_click_paste: Option<bool>,
}

/// Smallest accepted `minimum_contrast`; it leaves every colour unchanged.
pub const MIN_CONTRAST: f32 = 1.0;
/// Largest accepted `minimum_contrast`: black on white.
pub const MAX_CONTRAST: f32 = 21.0;

/// Logical padding per side after precedence: side override, then the
/// `padding` shorthand, then 0.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PaddingSides {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl TerminalConfig {
    /// Resolves each side as `padding_<side>`, else `padding`, else 0. Call it
    /// on the merged configuration so a side from any layer beats a shorthand
    /// from any layer.
    pub fn padding_sides(&self) -> PaddingSides {
        let side = |value: Option<f32>| value.or(self.padding).unwrap_or(0.0);
        PaddingSides {
            top: side(self.padding_top),
            right: side(self.padding_right),
            bottom: side(self.padding_bottom),
            left: side(self.padding_left),
        }
    }

    pub fn padding_balance_enabled(&self) -> bool {
        self.padding_balance.unwrap_or(false)
    }

    pub fn padding_color_mode(&self) -> PaddingColor {
        self.padding_color.unwrap_or_default()
    }

    /// The effective minimum contrast ratio; unset means [`MIN_CONTRAST`] (off).
    pub fn minimum_contrast_ratio(&self) -> f32 {
        self.minimum_contrast.unwrap_or(MIN_CONTRAST)
    }

    /// Whether an unfocused window draws a hollow cursor; unset means true.
    pub fn cursor_unfocused_hollow_enabled(&self) -> bool {
        self.cursor_unfocused_hollow.unwrap_or(true)
    }

    /// The effective window opacity; unset means 1.0 (opaque).
    pub fn opacity_value(&self) -> f32 {
        self.opacity.unwrap_or(1.0)
    }

    /// The effective opacity mode; unset means [`OpacityMode::Background`].
    pub fn opacity_mode_or_default(&self) -> OpacityMode {
        self.opacity_mode.unwrap_or_default()
    }
}

/// Parses one `font_features` entry: an optional single leading `+` (on, the
/// default) or `-` (off), then exactly four printable ASCII bytes
/// (`0x21..=0x7e`). Values other than on/off (such as `cv01=2`) are not supported.
/// Returns the tag and its value (1 = on, 0 = off).
pub fn parse_font_feature(text: &str) -> Option<([u8; 4], u16)> {
    let (value, tag) = match text.as_bytes().first() {
        Some(b'-') => (0, &text[1..]),
        Some(b'+') => (1, &text[1..]),
        _ => (1, text),
    };
    let tag: [u8; 4] = tag.as_bytes().try_into().ok()?;
    tag.iter()
        .all(|byte| (0x21..=0x7e).contains(byte))
        .then_some((tag, value))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorStyle {
    Block,
    Bar,
    Underline,
}

#[cfg(test)]
#[path = "../unit/native_terminal.rs"]
mod tests;
