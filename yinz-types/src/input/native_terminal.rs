//! Settings shared with the native terminal client.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OpacityMode {
    #[default]
    Background,
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
    pub padding: Option<f32>,
    pub opacity: Option<f32>,
    pub blur: Option<bool>,
    pub startup_mode: Option<StartupMode>,
    pub initial_columns: Option<u16>,
    pub initial_rows: Option<u16>,
    pub cursor_style: Option<CursorStyle>,
    pub macos_option_as_alt: Option<OptionAsAlt>,
    pub cursor_blink: Option<bool>,
    pub cursor_trail: Option<bool>,
    pub open_links: Option<bool>,
    pub middle_click_paste: Option<bool>,
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
