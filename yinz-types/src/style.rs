//! Palettes, styles, and their pure conversions.

use crate::colors::{default_colors, PaletteColor};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HostTerminalThemeMode {
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub enum ThemeHue {
    #[serde(alias = "light")]
    Light,
    #[default]
    #[serde(alias = "dark")]
    Dark,
}

impl FromStr for ThemeHue {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "light" => Ok(ThemeHue::Light),
            "dark" => Ok(ThemeHue::Dark),
            e => Err(format!(
                "Unknown theme hue: '{}' (expected 'dark' or 'light')",
                e
            )),
        }
    }
}

impl fmt::Display for ThemeHue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ThemeHue::Light => write!(f, "light"),
            ThemeHue::Dark => write!(f, "dark"),
        }
    }
}

impl From<ThemeHue> for HostTerminalThemeMode {
    fn from(hue: ThemeHue) -> Self {
        match hue {
            ThemeHue::Light => HostTerminalThemeMode::Light,
            ThemeHue::Dark => HostTerminalThemeMode::Dark,
        }
    }
}

impl From<HostTerminalThemeMode> for ThemeHue {
    fn from(mode: HostTerminalThemeMode) -> Self {
        match mode {
            HostTerminalThemeMode::Light => ThemeHue::Light,
            HostTerminalThemeMode::Dark => ThemeHue::Dark,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub enum PaletteSource {
    #[default]
    Default,
    Xresources,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub struct Palette {
    pub source: PaletteSource,
    pub theme_hue: ThemeHue,
    pub fg: PaletteColor,
    pub bg: PaletteColor,
    pub black: PaletteColor,
    pub red: PaletteColor,
    pub green: PaletteColor,
    pub yellow: PaletteColor,
    pub blue: PaletteColor,
    pub magenta: PaletteColor,
    pub cyan: PaletteColor,
    pub white: PaletteColor,
    pub orange: PaletteColor,
    pub gray: PaletteColor,
    pub purple: PaletteColor,
    pub gold: PaletteColor,
    pub silver: PaletteColor,
    pub pink: PaletteColor,
    pub brown: PaletteColor,
}

/// Literal pane colors, independent of host colors and application overrides.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TerminalTheme {
    pub foreground: (u8, u8, u8),
    pub background: (u8, u8, u8),
    pub ansi: [(u8, u8, u8); 16],
}

impl TerminalTheme {
    pub const ANSI_NAMES: [&'static str; 16] = [
        "black",
        "red",
        "green",
        "yellow",
        "blue",
        "magenta",
        "cyan",
        "white",
        "bright_black",
        "bright_red",
        "bright_green",
        "bright_yellow",
        "bright_blue",
        "bright_magenta",
        "bright_cyan",
        "bright_white",
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Style {
    // Renderer policy is carried by Options, not the plugin styling contract.
    #[serde(skip)]
    pub agent_theme_mapping: crate::agent::theme::AgentThemeMapping,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<TerminalTheme>,
    pub colors: Styling,
    pub rounded_corners: bool,
    pub hide_session_name: bool,
}

impl Style {
    /// Margin color independent of pane-local application background overrides.
    pub fn pane_surround(&self) -> PaletteColor {
        self.colors.pane_surround.unwrap_or_else(|| {
            let background = self
                .terminal
                .map(|t| PaletteColor::Rgb(t.background))
                .unwrap_or(self.colors.text_unselected.background);
            let (r, g, b) = match background {
                PaletteColor::Rgb(rgb) => rgb,
                PaletteColor::EightBit(index @ 0..=15) => return PaletteColor::EightBit(index),
                PaletteColor::EightBit(index) => crate::colors::eightbit_to_rgb(index),
            };
            PaletteColor::Rgb((
                r.saturating_sub(12),
                g.saturating_sub(12),
                b.saturating_sub(12),
            ))
        })
    }

    /// Frame and divider defaults, independent of pane-local application colors.
    pub fn chrome_colors(&self) -> Option<(PaletteColor, PaletteColor)> {
        self.terminal.map(|terminal| {
            (
                PaletteColor::Rgb(terminal.foreground),
                PaletteColor::Rgb(terminal.background),
            )
        })
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub enum Coloration {
    NoStyling,
    Styled(StyleDeclaration),
}

impl Coloration {
    pub fn with_fallback(&self, fallback: StyleDeclaration) -> StyleDeclaration {
        match &self {
            Coloration::NoStyling => fallback,
            Coloration::Styled(style) => *style,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct Styling {
    #[serde(default)]
    pub pane_surround: Option<PaletteColor>,
    pub text_unselected: StyleDeclaration,
    pub text_selected: StyleDeclaration,
    pub ribbon_unselected: StyleDeclaration,
    pub ribbon_selected: StyleDeclaration,
    pub table_title: StyleDeclaration,
    pub table_cell_unselected: StyleDeclaration,
    pub table_cell_selected: StyleDeclaration,
    pub list_unselected: StyleDeclaration,
    pub list_selected: StyleDeclaration,
    pub frame_unselected: Option<StyleDeclaration>,
    pub frame_selected: StyleDeclaration,
    pub frame_highlight: StyleDeclaration,
    pub exit_code_success: StyleDeclaration,
    pub exit_code_error: StyleDeclaration,
    pub multiplayer_user_colors: MultiplayerColors,
}

#[derive(Debug, Copy, Default, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct StyleDeclaration {
    pub base: PaletteColor,
    pub background: PaletteColor,
    pub emphasis_0: PaletteColor,
    pub emphasis_1: PaletteColor,
    pub emphasis_2: PaletteColor,
    pub emphasis_3: PaletteColor,
}

#[derive(Debug, Copy, Default, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct MultiplayerColors {
    pub player_1: PaletteColor,
    pub player_2: PaletteColor,
    pub player_3: PaletteColor,
    pub player_4: PaletteColor,
    pub player_5: PaletteColor,
    pub player_6: PaletteColor,
    pub player_7: PaletteColor,
    pub player_8: PaletteColor,
    pub player_9: PaletteColor,
    pub player_10: PaletteColor,
}

pub const DEFAULT_STYLES: Styling = Styling {
    pane_surround: None,
    text_unselected: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::BRIGHT_GRAY),
        emphasis_0: PaletteColor::EightBit(default_colors::ORANGE),
        emphasis_1: PaletteColor::EightBit(default_colors::CYAN),
        emphasis_2: PaletteColor::EightBit(default_colors::GREEN),
        emphasis_3: PaletteColor::EightBit(default_colors::MAGENTA),
        background: PaletteColor::EightBit(default_colors::GRAY),
    },
    text_selected: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::BRIGHT_GRAY),
        emphasis_0: PaletteColor::EightBit(default_colors::ORANGE),
        emphasis_1: PaletteColor::EightBit(default_colors::CYAN),
        emphasis_2: PaletteColor::EightBit(default_colors::GREEN),
        emphasis_3: PaletteColor::EightBit(default_colors::MAGENTA),
        background: PaletteColor::EightBit(default_colors::GRAY),
    },
    ribbon_unselected: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::BLACK),
        emphasis_0: PaletteColor::EightBit(default_colors::RED),
        emphasis_1: PaletteColor::EightBit(default_colors::WHITE),
        emphasis_2: PaletteColor::EightBit(default_colors::BLUE),
        emphasis_3: PaletteColor::EightBit(default_colors::MAGENTA),
        background: PaletteColor::EightBit(default_colors::GRAY),
    },
    ribbon_selected: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::BLACK),
        emphasis_0: PaletteColor::EightBit(default_colors::RED),
        emphasis_1: PaletteColor::EightBit(default_colors::ORANGE),
        emphasis_2: PaletteColor::EightBit(default_colors::MAGENTA),
        emphasis_3: PaletteColor::EightBit(default_colors::BLUE),
        background: PaletteColor::EightBit(default_colors::GREEN),
    },
    exit_code_success: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::GREEN),
        emphasis_0: PaletteColor::EightBit(default_colors::CYAN),
        emphasis_1: PaletteColor::EightBit(default_colors::BLACK),
        emphasis_2: PaletteColor::EightBit(default_colors::MAGENTA),
        emphasis_3: PaletteColor::EightBit(default_colors::BLUE),
        background: PaletteColor::EightBit(default_colors::GRAY),
    },
    exit_code_error: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::RED),
        emphasis_0: PaletteColor::EightBit(default_colors::YELLOW),
        emphasis_1: PaletteColor::EightBit(default_colors::GOLD),
        emphasis_2: PaletteColor::EightBit(default_colors::SILVER),
        emphasis_3: PaletteColor::EightBit(default_colors::PURPLE),
        background: PaletteColor::EightBit(default_colors::GRAY),
    },
    frame_unselected: None,
    frame_selected: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::GREEN),
        emphasis_0: PaletteColor::EightBit(default_colors::ORANGE),
        emphasis_1: PaletteColor::EightBit(default_colors::CYAN),
        emphasis_2: PaletteColor::EightBit(default_colors::MAGENTA),
        emphasis_3: PaletteColor::EightBit(default_colors::BROWN),
        background: PaletteColor::EightBit(default_colors::GRAY),
    },
    frame_highlight: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::ORANGE),
        emphasis_0: PaletteColor::EightBit(default_colors::MAGENTA),
        emphasis_1: PaletteColor::EightBit(default_colors::PURPLE),
        emphasis_2: PaletteColor::EightBit(default_colors::GREEN),
        emphasis_3: PaletteColor::EightBit(default_colors::GREEN),
        background: PaletteColor::EightBit(default_colors::GREEN),
    },
    table_title: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::GREEN),
        emphasis_0: PaletteColor::EightBit(default_colors::ORANGE),
        emphasis_1: PaletteColor::EightBit(default_colors::CYAN),
        emphasis_2: PaletteColor::EightBit(default_colors::GREEN),
        emphasis_3: PaletteColor::EightBit(default_colors::MAGENTA),
        background: PaletteColor::EightBit(default_colors::GRAY),
    },
    table_cell_unselected: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::BRIGHT_GRAY),
        emphasis_0: PaletteColor::EightBit(default_colors::ORANGE),
        emphasis_1: PaletteColor::EightBit(default_colors::CYAN),
        emphasis_2: PaletteColor::EightBit(default_colors::GREEN),
        emphasis_3: PaletteColor::EightBit(default_colors::MAGENTA),
        background: PaletteColor::EightBit(default_colors::GRAY),
    },
    table_cell_selected: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::GREEN),
        emphasis_0: PaletteColor::EightBit(default_colors::ORANGE),
        emphasis_1: PaletteColor::EightBit(default_colors::CYAN),
        emphasis_2: PaletteColor::EightBit(default_colors::RED),
        emphasis_3: PaletteColor::EightBit(default_colors::MAGENTA),
        background: PaletteColor::EightBit(default_colors::GRAY),
    },
    list_unselected: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::BRIGHT_GRAY),
        emphasis_0: PaletteColor::EightBit(default_colors::ORANGE),
        emphasis_1: PaletteColor::EightBit(default_colors::CYAN),
        emphasis_2: PaletteColor::EightBit(default_colors::GREEN),
        emphasis_3: PaletteColor::EightBit(default_colors::MAGENTA),
        background: PaletteColor::EightBit(default_colors::GRAY),
    },
    list_selected: StyleDeclaration {
        base: PaletteColor::EightBit(default_colors::GREEN),
        emphasis_0: PaletteColor::EightBit(default_colors::ORANGE),
        emphasis_1: PaletteColor::EightBit(default_colors::CYAN),
        emphasis_2: PaletteColor::EightBit(default_colors::RED),
        emphasis_3: PaletteColor::EightBit(default_colors::MAGENTA),
        background: PaletteColor::EightBit(default_colors::GRAY),
    },
    multiplayer_user_colors: MultiplayerColors {
        player_1: PaletteColor::EightBit(default_colors::MAGENTA),
        player_2: PaletteColor::EightBit(default_colors::BLUE),
        player_3: PaletteColor::EightBit(default_colors::PURPLE),
        player_4: PaletteColor::EightBit(default_colors::YELLOW),
        player_5: PaletteColor::EightBit(default_colors::CYAN),
        player_6: PaletteColor::EightBit(default_colors::GOLD),
        player_7: PaletteColor::EightBit(default_colors::RED),
        player_8: PaletteColor::EightBit(default_colors::SILVER),
        player_9: PaletteColor::EightBit(default_colors::PINK),
        player_10: PaletteColor::EightBit(default_colors::BROWN),
    },
};

impl Default for Styling {
    fn default() -> Self {
        DEFAULT_STYLES
    }
}

impl From<Styling> for Palette {
    fn from(styling: Styling) -> Self {
        Palette {
            theme_hue: ThemeHue::Dark,
            source: PaletteSource::Default,
            fg: styling.ribbon_unselected.background,
            bg: styling.text_unselected.background,
            red: styling.exit_code_error.base,
            green: styling.text_unselected.emphasis_2,
            yellow: styling.exit_code_error.emphasis_0,
            blue: styling.ribbon_unselected.emphasis_2,
            magenta: styling.text_unselected.emphasis_3,
            orange: styling.text_unselected.emphasis_0,
            cyan: styling.text_unselected.emphasis_1,
            black: styling.ribbon_unselected.base,
            white: styling.ribbon_unselected.emphasis_1,
            gray: styling.list_unselected.background,
            purple: styling.multiplayer_user_colors.player_3,
            gold: styling.multiplayer_user_colors.player_6,
            silver: styling.multiplayer_user_colors.player_8,
            pink: styling.multiplayer_user_colors.player_9,
            brown: styling.multiplayer_user_colors.player_10,
        }
    }
}

impl From<Palette> for Styling {
    fn from(palette: Palette) -> Self {
        let (fg, bg) = match palette.theme_hue {
            ThemeHue::Light => (palette.black, palette.white),
            ThemeHue::Dark => (palette.white, palette.black),
        };
        Styling {
            pane_surround: None,
            text_unselected: StyleDeclaration {
                base: fg,
                emphasis_0: palette.orange,
                emphasis_1: palette.cyan,
                emphasis_2: palette.green,
                emphasis_3: palette.magenta,
                background: bg,
            },
            text_selected: StyleDeclaration {
                base: fg,
                emphasis_0: palette.orange,
                emphasis_1: palette.cyan,
                emphasis_2: palette.green,
                emphasis_3: palette.magenta,
                background: palette.bg,
            },
            ribbon_unselected: StyleDeclaration {
                base: palette.black,
                emphasis_0: palette.red,
                emphasis_1: palette.white,
                emphasis_2: palette.blue,
                emphasis_3: palette.magenta,
                background: palette.fg,
            },
            ribbon_selected: StyleDeclaration {
                base: palette.black,
                emphasis_0: palette.red,
                emphasis_1: palette.orange,
                emphasis_2: palette.magenta,
                emphasis_3: palette.blue,
                background: palette.green,
            },
            exit_code_success: StyleDeclaration {
                base: palette.green,
                emphasis_0: palette.cyan,
                emphasis_1: palette.black,
                emphasis_2: palette.magenta,
                emphasis_3: palette.blue,
                background: Default::default(),
            },
            exit_code_error: StyleDeclaration {
                base: palette.red,
                emphasis_0: palette.yellow,
                emphasis_1: palette.gold,
                emphasis_2: palette.silver,
                emphasis_3: palette.purple,
                background: Default::default(),
            },
            frame_unselected: None,
            frame_selected: StyleDeclaration {
                base: palette.green,
                emphasis_0: palette.orange,
                emphasis_1: palette.cyan,
                emphasis_2: palette.magenta,
                emphasis_3: palette.brown,
                background: Default::default(),
            },
            frame_highlight: StyleDeclaration {
                base: palette.orange,
                emphasis_0: palette.magenta,
                emphasis_1: palette.purple,
                emphasis_2: palette.orange,
                emphasis_3: palette.orange,
                background: Default::default(),
            },
            table_title: StyleDeclaration {
                base: palette.green,
                emphasis_0: palette.orange,
                emphasis_1: palette.cyan,
                emphasis_2: palette.green,
                emphasis_3: palette.magenta,
                background: palette.gray,
            },
            table_cell_unselected: StyleDeclaration {
                base: fg,
                emphasis_0: palette.orange,
                emphasis_1: palette.cyan,
                emphasis_2: palette.green,
                emphasis_3: palette.magenta,
                background: palette.black,
            },
            table_cell_selected: StyleDeclaration {
                base: fg,
                emphasis_0: palette.orange,
                emphasis_1: palette.cyan,
                emphasis_2: palette.green,
                emphasis_3: palette.magenta,
                background: palette.bg,
            },
            list_unselected: StyleDeclaration {
                base: palette.white,
                emphasis_0: palette.orange,
                emphasis_1: palette.cyan,
                emphasis_2: palette.green,
                emphasis_3: palette.magenta,
                background: palette.black,
            },
            list_selected: StyleDeclaration {
                base: palette.white,
                emphasis_0: palette.orange,
                emphasis_1: palette.cyan,
                emphasis_2: palette.green,
                emphasis_3: palette.magenta,
                background: palette.bg,
            },
            multiplayer_user_colors: MultiplayerColors {
                player_1: palette.magenta,
                player_2: palette.blue,
                player_3: palette.purple,
                player_4: palette.yellow,
                player_5: palette.cyan,
                player_6: palette.gold,
                player_7: palette.red,
                player_8: palette.silver,
                player_9: palette.pink,
                player_10: palette.brown,
            },
        }
    }
}

pub fn default_palette() -> Palette {
    Palette {
        source: PaletteSource::Default,
        theme_hue: ThemeHue::Dark,
        fg: PaletteColor::EightBit(default_colors::BRIGHT_GRAY),
        bg: PaletteColor::EightBit(default_colors::GRAY),
        black: PaletteColor::EightBit(default_colors::BLACK),
        red: PaletteColor::EightBit(default_colors::RED),
        green: PaletteColor::EightBit(default_colors::GREEN),
        yellow: PaletteColor::EightBit(default_colors::YELLOW),
        blue: PaletteColor::EightBit(default_colors::BLUE),
        magenta: PaletteColor::EightBit(default_colors::MAGENTA),
        cyan: PaletteColor::EightBit(default_colors::CYAN),
        white: PaletteColor::EightBit(default_colors::WHITE),
        orange: PaletteColor::EightBit(default_colors::ORANGE),
        gray: PaletteColor::EightBit(default_colors::GRAY),
        purple: PaletteColor::EightBit(default_colors::PURPLE),
        gold: PaletteColor::EightBit(default_colors::GOLD),
        silver: PaletteColor::EightBit(default_colors::SILVER),
        pink: PaletteColor::EightBit(default_colors::PINK),
        brown: PaletteColor::EightBit(default_colors::BROWN),
    }
}

// Dark magic
pub fn detect_theme_hue(bg: PaletteColor) -> ThemeHue {
    match bg {
        PaletteColor::Rgb((r, g, b)) => {
            // HSP, P stands for perceived brightness
            let hsp: f64 = (0.299 * (r as f64 * r as f64)
                + 0.587 * (g as f64 * g as f64)
                + 0.114 * (b as f64 * b as f64))
                .sqrt();
            match hsp > 127.5 {
                true => ThemeHue::Light,
                false => ThemeHue::Dark,
            }
        },
        _ => ThemeHue::Dark,
    }
}

#[cfg(test)]
#[path = "unit/style_compatibility_tests.rs"]
mod compatibility_tests;

#[cfg(test)]
#[path = "unit/pane_surround.rs"]
mod pane_surround_tests;
