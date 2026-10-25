use super::generated_api::api::style::{
    color::Payload as ProtobufColorPayload, Color as ProtobufColor, ColorType as ProtobufColorType,
    Palette as ProtobufPalette, RgbColorPayload as ProtobufRgbColorPayload, Style as ProtobufStyle,
    Styling as ProtobufStyling, ThemeHue as ProtobufThemeHue,
};
use yinz_types::data::PaletteColor;
use yinz_types::style::{MultiplayerColors, Palette, Style, StyleDeclaration, Styling, ThemeHue};

use std::convert::TryFrom;

impl TryFrom<ProtobufStyle> for Style {
    type Error = &'static str;
    fn try_from(protobuf_style: ProtobufStyle) -> Result<Self, &'static str> {
        let s = protobuf_style
            .styling
            .ok_or("malformed style payload")?
            .try_into()?;
        Ok(Style {
            agent_theme_mapping: Default::default(),
            terminal: None,
            colors: s,
            rounded_corners: protobuf_style.rounded_corners,
            hide_session_name: protobuf_style.hide_session_name,
        })
    }
}

#[allow(deprecated)]
impl TryFrom<Style> for ProtobufStyle {
    type Error = &'static str;
    fn try_from(style: Style) -> Result<Self, &'static str> {
        let s = ProtobufStyling::try_from(style.colors)?;
        let palette = Palette::from(style.colors);
        Ok(ProtobufStyle {
            palette: Some(palette.try_into()?),
            rounded_corners: style.rounded_corners,
            hide_session_name: style.hide_session_name,
            styling: Some(s),
        })
    }
}

fn to_array<T, const N: usize>(v: Vec<T>) -> std::result::Result<[T; N], &'static str> {
    v.try_into()
        .map_err(|_| "Could not obtain array from protobuf field")
}

fn to_style_declaration(
    parsed_array: Result<[PaletteColor; 6], &'static str>,
) -> Result<StyleDeclaration, &'static str> {
    parsed_array.map(|arr| StyleDeclaration {
        base: arr[0],
        background: arr[1],
        emphasis_0: arr[2],
        emphasis_1: arr[3],
        emphasis_2: arr[4],
        emphasis_3: arr[5],
    })
}

fn to_multiplayer_colors(
    parsed_array: Result<[PaletteColor; 10], &'static str>,
) -> Result<MultiplayerColors, &'static str> {
    parsed_array.map(|arr| MultiplayerColors {
        player_1: arr[0],
        player_2: arr[1],
        player_3: arr[2],
        player_4: arr[3],
        player_5: arr[4],
        player_6: arr[5],
        player_7: arr[6],
        player_8: arr[7],
        player_9: arr[8],
        player_10: arr[9],
    })
}

#[macro_export]
macro_rules! color_definitions {
    ($proto:expr_2021, $declaration:ident, $size:expr_2021) => {
        to_style_declaration(to_array::<PaletteColor, $size>(
            $proto
                .$declaration
                .into_iter()
                .map(PaletteColor::try_from)
                .collect::<Result<Vec<PaletteColor>, _>>()?,
        ))?
    };
}

#[macro_export]
macro_rules! multiplayer_colors {
    ($proto:expr_2021, $size: expr_2021) => {
        to_multiplayer_colors(to_array::<PaletteColor, $size>(
            $proto
                .multiplayer_user_colors
                .into_iter()
                .map(PaletteColor::try_from)
                .collect::<Result<Vec<PaletteColor>, _>>()?,
        ))?
    };
}

impl TryFrom<ProtobufStyling> for Styling {
    type Error = &'static str;

    fn try_from(proto: ProtobufStyling) -> std::result::Result<Self, Self::Error> {
        let frame_unselected = if !proto.frame_unselected.is_empty() {
            Some(color_definitions!(proto, frame_unselected, 6))
        } else {
            None
        };

        Ok(Styling {
            pane_surround: proto
                .pane_surround
                .map(PaletteColor::try_from)
                .transpose()?,
            text_unselected: color_definitions!(proto, text_unselected, 6),
            text_selected: color_definitions!(proto, text_selected, 6),
            ribbon_unselected: color_definitions!(proto, ribbon_unselected, 6),
            ribbon_selected: color_definitions!(proto, ribbon_selected, 6),
            table_title: color_definitions!(proto, table_title, 6),
            table_cell_unselected: color_definitions!(proto, table_cell_unselected, 6),
            table_cell_selected: color_definitions!(proto, table_cell_selected, 6),
            list_unselected: color_definitions!(proto, list_unselected, 6),
            list_selected: color_definitions!(proto, list_selected, 6),
            frame_unselected,
            frame_selected: color_definitions!(proto, frame_selected, 6),
            frame_highlight: color_definitions!(proto, frame_highlight, 6),
            exit_code_success: color_definitions!(proto, exit_code_success, 6),
            exit_code_error: color_definitions!(proto, exit_code_error, 6),
            multiplayer_user_colors: multiplayer_colors!(proto, 10),
        })
    }
}

fn style_declaration_to_protobuf(
    colors: StyleDeclaration,
) -> Result<Vec<ProtobufColor>, &'static str> {
    Ok(vec![
        colors.base.try_into()?,
        colors.background.try_into()?,
        colors.emphasis_0.try_into()?,
        colors.emphasis_1.try_into()?,
        colors.emphasis_2.try_into()?,
        colors.emphasis_3.try_into()?,
    ])
}

fn multiplayer_colors_to_protobuf(
    value: MultiplayerColors,
) -> Result<Vec<ProtobufColor>, &'static str> {
    Ok(vec![
        value.player_1.try_into()?,
        value.player_2.try_into()?,
        value.player_3.try_into()?,
        value.player_4.try_into()?,
        value.player_5.try_into()?,
        value.player_6.try_into()?,
        value.player_7.try_into()?,
        value.player_8.try_into()?,
        value.player_9.try_into()?,
        value.player_10.try_into()?,
    ])
}

impl TryFrom<Styling> for ProtobufStyling {
    type Error = &'static str;

    fn try_from(style: Styling) -> std::result::Result<Self, Self::Error> {
        let frame_unselected_vec = match style.frame_unselected {
            None => Ok(Vec::new()),
            Some(frame_unselected) => style_declaration_to_protobuf(frame_unselected),
        };

        Ok(ProtobufStyling {
            pane_surround: style
                .pane_surround
                .map(ProtobufColor::try_from)
                .transpose()?,
            text_unselected: style_declaration_to_protobuf(style.text_unselected)?,
            text_selected: style_declaration_to_protobuf(style.text_selected)?,
            ribbon_unselected: style_declaration_to_protobuf(style.ribbon_unselected)?,
            ribbon_selected: style_declaration_to_protobuf(style.ribbon_selected)?,
            table_title: style_declaration_to_protobuf(style.table_title)?,
            table_cell_unselected: style_declaration_to_protobuf(style.table_cell_unselected)?,
            table_cell_selected: style_declaration_to_protobuf(style.table_cell_selected)?,
            list_unselected: style_declaration_to_protobuf(style.list_unselected)?,
            list_selected: style_declaration_to_protobuf(style.list_selected)?,
            frame_unselected: frame_unselected_vec?,
            frame_selected: style_declaration_to_protobuf(style.frame_selected)?,
            frame_highlight: style_declaration_to_protobuf(style.frame_highlight)?,
            exit_code_success: style_declaration_to_protobuf(style.exit_code_success)?,
            exit_code_error: style_declaration_to_protobuf(style.exit_code_error)?,
            multiplayer_user_colors: multiplayer_colors_to_protobuf(style.multiplayer_user_colors)?,
        })
    }
}

impl TryFrom<ProtobufPalette> for Palette {
    type Error = &'static str;
    fn try_from(protobuf_palette: ProtobufPalette) -> Result<Self, &'static str> {
        #[allow(clippy::needless_update)]
        Ok(Palette {
            theme_hue: ProtobufThemeHue::try_from(protobuf_palette.theme_hue)
                .ok()
                .ok_or("malformed theme_hue payload for Palette")?
                .try_into()?,
            fg: protobuf_palette
                .fg
                .ok_or("malformed palette payload")?
                .try_into()?,
            bg: protobuf_palette
                .bg
                .ok_or("malformed palette payload")?
                .try_into()?,
            black: protobuf_palette
                .black
                .ok_or("malformed palette payload")?
                .try_into()?,
            red: protobuf_palette
                .red
                .ok_or("malformed palette payload")?
                .try_into()?,
            green: protobuf_palette
                .green
                .ok_or("malformed palette payload")?
                .try_into()?,
            yellow: protobuf_palette
                .yellow
                .ok_or("malformed palette payload")?
                .try_into()?,
            blue: protobuf_palette
                .blue
                .ok_or("malformed palette payload")?
                .try_into()?,
            magenta: protobuf_palette
                .magenta
                .ok_or("malformed palette payload")?
                .try_into()?,
            cyan: protobuf_palette
                .cyan
                .ok_or("malformed palette payload")?
                .try_into()?,
            white: protobuf_palette
                .white
                .ok_or("malformed palette payload")?
                .try_into()?,
            orange: protobuf_palette
                .orange
                .ok_or("malformed palette payload")?
                .try_into()?,
            gray: protobuf_palette
                .gray
                .ok_or("malformed palette payload")?
                .try_into()?,
            purple: protobuf_palette
                .purple
                .ok_or("malformed palette payload")?
                .try_into()?,
            gold: protobuf_palette
                .gold
                .ok_or("malformed palette payload")?
                .try_into()?,
            silver: protobuf_palette
                .silver
                .ok_or("malformed palette payload")?
                .try_into()?,
            pink: protobuf_palette
                .pink
                .ok_or("malformed palette payload")?
                .try_into()?,
            brown: protobuf_palette
                .brown
                .ok_or("malformed palette payload")?
                .try_into()?,
            ..Default::default()
        })
    }
}

impl TryFrom<Palette> for ProtobufPalette {
    type Error = &'static str;
    fn try_from(palette: Palette) -> Result<Self, &'static str> {
        let theme_hue: ProtobufThemeHue = palette
            .theme_hue
            .try_into()
            .map_err(|_| "malformed payload for palette")?;
        #[allow(clippy::needless_update)]
        Ok(ProtobufPalette {
            theme_hue: theme_hue as i32,
            fg: Some(palette.fg.try_into()?),
            bg: Some(palette.bg.try_into()?),
            black: Some(palette.black.try_into()?),
            red: Some(palette.red.try_into()?),
            green: Some(palette.green.try_into()?),
            yellow: Some(palette.yellow.try_into()?),
            blue: Some(palette.blue.try_into()?),
            magenta: Some(palette.magenta.try_into()?),
            cyan: Some(palette.cyan.try_into()?),
            white: Some(palette.white.try_into()?),
            orange: Some(palette.orange.try_into()?),
            gray: Some(palette.gray.try_into()?),
            purple: Some(palette.purple.try_into()?),
            gold: Some(palette.gold.try_into()?),
            silver: Some(palette.silver.try_into()?),
            pink: Some(palette.pink.try_into()?),
            brown: Some(palette.brown.try_into()?),
            ..Default::default()
        })
    }
}

impl TryFrom<ProtobufColor> for PaletteColor {
    type Error = &'static str;
    fn try_from(protobuf_color: ProtobufColor) -> Result<Self, &'static str> {
        match ProtobufColorType::try_from(protobuf_color.color_type) {
            Ok(ProtobufColorType::Rgb) => match protobuf_color.payload {
                Some(ProtobufColorPayload::RgbColorPayload(rgb_color_payload)) => {
                    Ok(PaletteColor::Rgb((
                        u8::try_from(rgb_color_payload.red)
                            .map_err(|_| "RGB color channel out of range")?,
                        u8::try_from(rgb_color_payload.green)
                            .map_err(|_| "RGB color channel out of range")?,
                        u8::try_from(rgb_color_payload.blue)
                            .map_err(|_| "RGB color channel out of range")?,
                    )))
                },
                _ => Err("malformed payload for Rgb color"),
            },
            Ok(ProtobufColorType::EightBit) => match protobuf_color.payload {
                Some(ProtobufColorPayload::EightBitColorPayload(eight_bit_payload)) => {
                    Ok(PaletteColor::EightBit(
                        u8::try_from(eight_bit_payload)
                            .map_err(|_| "8bit color index out of range")?,
                    ))
                },
                _ => Err("malformed payload for 8bit color"),
            },
            Err(_) => Err("malformed payload for Color"),
        }
    }
}

impl TryFrom<PaletteColor> for ProtobufColor {
    type Error = &'static str;
    fn try_from(color: PaletteColor) -> Result<Self, &'static str> {
        match color {
            PaletteColor::Rgb((red, green, blue)) => {
                let red = red as u32;
                let green = green as u32;
                let blue = blue as u32;
                Ok(ProtobufColor {
                    color_type: ProtobufColorType::Rgb as i32,
                    payload: Some(ProtobufColorPayload::RgbColorPayload(
                        ProtobufRgbColorPayload { red, green, blue },
                    )),
                })
            },
            PaletteColor::EightBit(color) => Ok(ProtobufColor {
                color_type: ProtobufColorType::EightBit as i32,
                payload: Some(ProtobufColorPayload::EightBitColorPayload(color as u32)),
            }),
        }
    }
}

impl TryFrom<ThemeHue> for ProtobufThemeHue {
    type Error = &'static str;
    fn try_from(theme_hue: ThemeHue) -> Result<Self, &'static str> {
        match theme_hue {
            ThemeHue::Light => Ok(ProtobufThemeHue::Light),
            ThemeHue::Dark => Ok(ProtobufThemeHue::Dark),
        }
    }
}

impl TryFrom<ProtobufThemeHue> for ThemeHue {
    type Error = &'static str;
    fn try_from(protobuf_theme_hue: ProtobufThemeHue) -> Result<Self, &'static str> {
        match protobuf_theme_hue {
            ProtobufThemeHue::Light => Ok(ThemeHue::Light),
            ProtobufThemeHue::Dark => Ok(ThemeHue::Dark),
        }
    }
}

#[cfg(test)]
#[path = "unit/style_color_tests.rs"]
mod color_tests;

#[cfg(test)]
#[path = "unit/style.rs"]
mod pane_surround_tests;
