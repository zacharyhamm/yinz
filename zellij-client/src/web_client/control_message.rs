#![allow(clippy::large_enum_variant)]
use serde::{Deserialize, Serialize};
use zellij_utils::{
    input::{config::Config, options::Options, web_client::WebClientTheme},
    ipc::MobileStatePayload,
    pane_size::Size,
};

#[derive(Serialize, Deserialize, Debug, Clone)]

pub struct WebClientToWebServerControlMessage {
    pub web_client_id: String,
    pub payload: WebClientToWebServerControlMessagePayload,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum WebClientToWebServerControlMessagePayload {
    TerminalResize(Size),
    TerminalMetrics(TerminalMetricsPayload),
    SoftKeyboardVisibilityChanged {
        visible: bool,
    },
    NestedSessionFrameFromHost {
        payload_bytes: Vec<u8>,
    },
    RequestSessionList,
    FocusPane {
        pane_id: u32,
        is_plugin: bool,
    },
    NewPaneInTab {
        tab_id: usize,
    },
    NewTab,
    SetMobileRenderPreferences {
        single_pane: bool,
        fit: bool,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TerminalMetricsPayload {
    pub cell_pixel_width: usize,
    pub cell_pixel_height: usize,
    pub text_area_pixel_width: usize,
    pub text_area_pixel_height: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum WebServerToWebClientControlMessage {
    SetConfig(SetConfigPayload),
    QueryTerminalSize,
    Log { lines: Vec<String> },
    LogError { lines: Vec<String> },
    SwitchedSession { new_session_name: String },
    SetSoftKeyboard { on: bool },
    MobileState { payload: MobileStatePayload },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SetConfigPayload {
    pub font: String,
    pub theme: SetConfigPayloadTheme,
    pub cursor_blink: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_inactive_style: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_style: Option<String>,
    pub mac_option_is_meta: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_size: Option<u16>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct SetConfigPayloadTheme {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub foreground: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub black: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blue: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bright_black: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bright_blue: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bright_cyan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bright_green: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bright_magenta: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bright_red: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bright_white: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bright_yellow: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_accent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cyan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub green: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub magenta: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub red: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_background: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_foreground: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_inactive_background: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub white: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yellow: Option<String>,
}

/// The 16 ANSI colour overrides from `web_client.theme`, in palette-index
/// order (0-7 normal, 8-15 bright).
///
/// Shared so the payload sent to xterm.js and the OSC 4 replies synthesized
/// for the server are built from one list, and cannot disagree about which
/// index a colour belongs to.
pub fn indexed_theme_overrides(theme: Option<&WebClientTheme>) -> [Option<String>; 16] {
    let get = |f: fn(&WebClientTheme) -> Option<String>| theme.and_then(f);
    [
        get(|t| t.black.clone()),
        get(|t| t.red.clone()),
        get(|t| t.green.clone()),
        get(|t| t.yellow.clone()),
        get(|t| t.blue.clone()),
        get(|t| t.magenta.clone()),
        get(|t| t.cyan.clone()),
        get(|t| t.white.clone()),
        get(|t| t.bright_black.clone()),
        get(|t| t.bright_red.clone()),
        get(|t| t.bright_green.clone()),
        get(|t| t.bright_yellow.clone()),
        get(|t| t.bright_blue.clone()),
        get(|t| t.bright_magenta.clone()),
        get(|t| t.bright_cyan.clone()),
        get(|t| t.bright_white.clone()),
    ]
}

impl SetConfigPayload {
    /// Build the config the browser renders with.
    ///
    /// `config_options` is the CLI-merged view, not `config.options`, so a
    /// `--theme` override reaches the browser and stays consistent with the
    /// colours seeded into the server's host-query cache.
    pub fn new(config: &Config, config_options: &Options) -> Self {
        let font = config.web_client.font.clone();

        let palette = config.theme_config(config_options.theme.as_ref());
        let web_client_theme_from_config = config.web_client.theme.as_ref();

        let mut theme = SetConfigPayloadTheme::default();
        let indexed = indexed_theme_overrides(web_client_theme_from_config);

        theme.background = web_client_theme_from_config
            .and_then(|theme| theme.background.clone())
            .or_else(|| palette.map(|p| p.text_unselected.background.as_rgb_str()));
        theme.foreground = web_client_theme_from_config
            .and_then(|theme| theme.foreground.clone())
            .or_else(|| palette.map(|p| p.text_unselected.base.as_rgb_str()));
        theme.black = indexed[0].clone();
        theme.blue = indexed[4].clone();
        theme.bright_black = indexed[8].clone();
        theme.bright_blue = indexed[12].clone();
        theme.bright_cyan = indexed[14].clone();
        theme.bright_green = indexed[10].clone();
        theme.bright_magenta = indexed[13].clone();
        theme.bright_red = indexed[9].clone();
        theme.bright_white = indexed[15].clone();
        theme.bright_yellow = indexed[11].clone();
        theme.cursor = web_client_theme_from_config.and_then(|theme| theme.cursor.clone());
        theme.cursor_accent =
            web_client_theme_from_config.and_then(|theme| theme.cursor_accent.clone());
        theme.cyan = indexed[6].clone();
        theme.green = indexed[2].clone();
        theme.magenta = indexed[5].clone();
        theme.red = indexed[1].clone();
        theme.selection_background = web_client_theme_from_config
            .and_then(|theme| theme.selection_background.clone())
            .or_else(|| palette.map(|p| p.text_selected.background.as_rgb_str()));
        theme.selection_foreground = web_client_theme_from_config
            .and_then(|theme| theme.selection_foreground.clone())
            .or_else(|| palette.map(|p| p.text_selected.base.as_rgb_str()));
        theme.selection_inactive_background = web_client_theme_from_config
            .and_then(|theme| theme.selection_inactive_background.clone());
        theme.white = indexed[7].clone();
        theme.yellow = indexed[3].clone();

        let cursor_blink = config.web_client.cursor_blink;
        let mac_option_is_meta = config.web_client.mac_option_is_meta;
        let cursor_style = config
            .web_client
            .cursor_style
            .as_ref()
            .map(|s| s.to_string());
        let cursor_inactive_style = config
            .web_client
            .cursor_inactive_style
            .as_ref()
            .map(|s| s.to_string());

        let font_size = config.web_client.font_size;

        SetConfigPayload {
            font,
            theme,
            cursor_blink,
            mac_option_is_meta,
            cursor_style,
            cursor_inactive_style,
            font_size,
        }
    }
}
