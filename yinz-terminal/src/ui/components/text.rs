use super::{
    is_too_wide, parse_disabled, parse_indices, parse_opaque, parse_selected, Coordinates,
};
use crate::panes::terminal_character::RESET_STYLES;
use crate::panes::{terminal_character::CharacterStyles, AnsiCode};
use yinz_types::style::{Style, StyleDeclaration};
use {yinz_types::data::PaletteColor, yinz_types::text::ansi_len};

use unicode_width::UnicodeWidthChar;
use yinz_runtime::errors::prelude::*;

pub fn text(content: Text, style: &Style, component_coordinates: Option<Coordinates>) -> Vec<u8> {
    if content.disabled {
        let declaration = style.colors.text_unselected;
        let mut base_text_style = RESET_STYLES
            .foreground(Some(declaration.base.into()))
            .italic(Some(AnsiCode::On));
        if content.opaque || content.selected {
            base_text_style = base_text_style.background(Some(declaration.background.into()));
        }
        let disabled_content = content.into_disabled();
        let (text, _text_width) = stringify_text(
            &disabled_content,
            None,
            &component_coordinates,
            &declaration,
            &style.colors,
            base_text_style,
        );
        return match component_coordinates {
            Some(component_coordinates) => {
                format!("{}{}{}", component_coordinates, base_text_style, text)
                    .as_bytes()
                    .to_vec()
            },
            None => format!("{}{}", base_text_style, text).as_bytes().to_vec(),
        };
    }
    let declaration = if content.selected {
        style.colors.text_selected
    } else {
        style.colors.text_unselected
    };

    // Start with base style from declaration
    let base_text_style = CharacterStyles::from(declaration).bold(Some(AnsiCode::On));

    let (text, _text_width) = stringify_text(
        &content,
        None,
        &component_coordinates,
        &declaration,
        &style.colors,
        base_text_style,
    );
    match component_coordinates {
        Some(component_coordinates) => {
            format!("{}{}{}", component_coordinates, base_text_style, text)
                .as_bytes()
                .to_vec()
        },
        None => format!("{}{}", base_text_style, text).as_bytes().to_vec(),
    }
}

pub fn stringify_text(
    text: &Text,
    left_padding: Option<usize>,
    coordinates: &Option<Coordinates>,
    style: &StyleDeclaration,
    styling: &yinz_types::style::Styling,
    component_text_style: CharacterStyles,
) -> (String, usize) {
    let mut text_width = 0;
    let mut stringified = String::new();
    let base_text_style = if text.opaque || text.selected {
        component_text_style.background(Some(style.background.into()))
    } else {
        component_text_style
    };
    stringified.push_str(&format!("{}", base_text_style));
    for (i, character) in text.text.chars().enumerate() {
        let character_width = character.width().unwrap_or(0);
        if is_too_wide(
            character_width,
            left_padding.unwrap_or(0) + text_width,
            coordinates,
        ) {
            break;
        }
        text_width += character_width;

        if text.selected || text.opaque {
            // we do this so that selected text will appear selected
            // even if it does not have color indices
            stringified.push_str(&format!("{}", base_text_style));
        }

        if !text.indices.is_empty() || text.selected || text.opaque {
            let character_with_styling =
                color_index_character(character, i, text, style, styling, base_text_style);
            stringified.push_str(&character_with_styling);
        } else {
            stringified.push(character)
        }
    }
    let coordinates_width = coordinates.as_ref().and_then(|c| c.width);
    if let (Some(coordinates_width), Some(_background_style)) =
        (coordinates_width, base_text_style.background)
    {
        let text_width_with_left_padding = text_width + left_padding.unwrap_or(0);
        let background_padding_length =
            coordinates_width.saturating_sub(text_width_with_left_padding);
        if text_width_with_left_padding < coordinates_width {
            // here we pad the string with whitespace until the end so that the background
            // style will apply the whole length of the coordinates
            stringified.push_str(&format!(
                "{:width$}",
                " ",
                width = background_padding_length
            ));
        }
        text_width += background_padding_length;
    }
    (stringified, text_width)
}

pub fn color_index_character(
    character: char,
    index: usize,
    text: &Text,
    declaration: &StyleDeclaration,
    styling: &yinz_types::style::Styling,
    base_text_style: CharacterStyles,
) -> String {
    let mut character_style = text
        .style_of_index(index, declaration, styling)
        .map(|foreground_style| base_text_style.foreground(Some(foreground_style.into())))
        .unwrap_or(base_text_style);

    // Apply dim and unbold per-character based on index levels 4 and 5
    if text.is_unbold_at(index) {
        // Remove bold for this character
        character_style = character_style.bold(Some(AnsiCode::Reset));
    } else if text.is_dimmed_at(index) {
        character_style = character_style.dim(Some(AnsiCode::On));
    } else {
        character_style = character_style
            .bold(Some(AnsiCode::On))
            .dim(Some(AnsiCode::Reset)); // default, to reset any
                                         // possible dim/bold values
                                         // from previous indices
    }

    // Level 8, applied on top of the colour rather than instead of it: a struck run keeps
    // whatever `style_of_index` chose. No explicit reset is needed on the way out - the
    // base style is derived from `RESET_STYLES`, which carries `strike: Reset`, and it is
    // written after every character.
    if text.is_struck_at(index) {
        character_style = character_style.strike(Some(AnsiCode::On));
    }

    format!("{}{}{}", character_style, character, base_text_style)
}

pub fn parse_text_params<'a>(params_iter: impl Iterator<Item = &'a mut String>) -> Vec<Text> {
    params_iter
        .flat_map(|stringified| {
            let selected = parse_selected(stringified);
            let opaque = parse_opaque(stringified);
            let disabled = parse_disabled(stringified);
            let indices = parse_indices(stringified);
            let text = parse_text(stringified).map_err(|e| e.to_string())?;
            Ok::<Text, String>(Text {
                text,
                opaque,
                selected,
                disabled,
                indices,
            })
        })
        .collect::<Vec<Text>>()
}

#[derive(Debug, Clone)]
pub struct Text {
    pub text: String,
    pub selected: bool,
    pub opaque: bool,
    pub disabled: bool,
    pub indices: Vec<Vec<usize>>,
}

impl Text {
    pub fn into_disabled(mut self) -> Self {
        self.selected = false;
        self.opaque = false;
        self.disabled = true;
        self.indices = vec![];
        self
    }

    pub fn pad_text(&mut self, max_column_width: usize) {
        for _ in ansi_len(&self.text)..max_column_width {
            self.text.push(' ');
        }
    }

    pub fn is_dimmed_at(&self, index: usize) -> bool {
        const DIM_LEVEL: usize = 4;
        self.indices
            .get(DIM_LEVEL)
            .map(|indices| indices.contains(&index))
            .unwrap_or(false)
    }

    pub fn is_unbold_at(&self, index: usize) -> bool {
        const UNBOLD_LEVEL: usize = 5;
        self.indices
            .get(UNBOLD_LEVEL)
            .map(|indices| indices.contains(&index))
            .unwrap_or(false)
    }

    /// Whether this character was asked for with `Text::strike_range` and friends.
    ///
    /// Level 8 rather than one of the levels below it because striking is orthogonal to
    /// colour: it is applied on top of whatever `style_of_index` chose, not instead of it.
    pub fn is_struck_at(&self, index: usize) -> bool {
        const STRIKE_LEVEL: usize = 8;
        self.indices
            .get(STRIKE_LEVEL)
            .map(|indices| indices.contains(&index))
            .unwrap_or(false)
    }

    pub fn style_of_index(
        &self,
        index: usize,
        style: &StyleDeclaration,
        styling: &yinz_types::style::Styling,
    ) -> Option<PaletteColor> {
        const ERROR_COLOR_LEVEL: usize = 6;
        const SUCCESS_COLOR_LEVEL: usize = 7;

        // Check error color first (highest precedence)
        if let Some(indices) = self.indices.get(ERROR_COLOR_LEVEL) {
            if indices.contains(&index) {
                return Some(styling.exit_code_error.base);
            }
        }

        // Check success color (second highest precedence)
        if let Some(indices) = self.indices.get(SUCCESS_COLOR_LEVEL) {
            if indices.contains(&index) {
                return Some(styling.exit_code_success.base);
            }
        }

        // Check regular emphasis levels (existing code)
        let index_variant_styles = [
            style.emphasis_0,
            style.emphasis_1,
            style.emphasis_2,
            style.emphasis_3,
        ];
        for i in (0..=3).rev() {
            // we do this in reverse to give precedence to the last applied
            // style
            if let Some(indices) = self.indices.get(i) {
                if indices.contains(&index) {
                    return Some(index_variant_styles[i]);
                }
            }
        }
        Some(style.base)
    }
}

pub fn parse_text(stringified: &str) -> Result<String> {
    // An empty param is a present-but-empty text (eg. a plugin rendering a blank
    // line as `Text::new("")`) — legal, and renders as nothing.
    if stringified.is_empty() {
        return Ok(String::new());
    }
    let mut utf8 = vec![];
    for stringified_character in stringified.split(',') {
        utf8.push(
            stringified_character
                .to_string()
                .parse::<u8>()
                .with_context(|| "Failed to parse utf8".to_string())?,
        );
    }
    Ok(String::from_utf8_lossy(&utf8).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_param_parses_as_empty_text() {
        assert_eq!(parse_text("").unwrap(), "");
    }

    #[test]
    fn empty_param_survives_parse_text_params() {
        let mut params = [String::new()];
        let parsed = parse_text_params(params.iter_mut());
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].text, "");
    }

    #[test]
    fn flags_only_param_parses_as_empty_text() {
        // "d" is the disabled flag with no text after it - eg. a plugin
        // rendering a blank line with styling flags attached
        let mut params = ["d".to_owned()];
        let parsed = parse_text_params(params.iter_mut());
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].text, "");
        assert!(parsed[0].disabled);
    }

    #[test]
    fn missing_param_still_yields_nothing() {
        let mut params: Vec<String> = vec![];
        assert!(parse_text_params(params.iter_mut()).is_empty());
    }

    /// Level 8 crosses a character out and leaves the levels below it alone: a struck run
    /// keeps whatever colour it was given, and the characters around it are not struck.
    #[test]
    fn the_strike_level_crosses_out_only_the_indices_it_names() {
        // Eight empty index groups, then level 8 naming characters 1 and 2, then the text
        // as the comma-joined bytes the plugin serializes it to.
        let mut params = ["$$$$$$$$1,2$97,98,99,100".to_owned()];
        let parsed = parse_text_params(params.iter_mut());
        assert_eq!(parsed.len(), 1);
        let text = &parsed[0];
        assert_eq!(text.text, "abcd");
        assert!(!text.is_struck_at(0));
        assert!(text.is_struck_at(1));
        assert!(text.is_struck_at(2));
        assert!(!text.is_struck_at(3));
        // and it is not read as one of the levels below it
        assert!(!text.is_dimmed_at(1));
        assert!(!text.is_unbold_at(1));
    }
}
