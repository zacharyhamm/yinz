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
    pub font_family: Option<String>,
    pub font_size: Option<f32>,
    pub font_weight: Option<u16>,
    pub font_ligatures: Option<bool>,
    pub system_fonts: Option<bool>,
    pub line_height: Option<f32>,
    pub padding: Option<f32>,
    pub opacity: Option<f32>,
    pub blur: Option<bool>,
    pub startup_mode: Option<StartupMode>,
    pub cursor_style: Option<CursorStyle>,
    pub macos_option_as_alt: Option<OptionAsAlt>,
    pub cursor_blink: Option<bool>,
    pub open_links: Option<bool>,
    pub middle_click_paste: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorStyle {
    Block,
    Bar,
    Underline,
}
