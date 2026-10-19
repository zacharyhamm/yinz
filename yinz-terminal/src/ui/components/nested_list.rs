use super::{
    is_too_high, is_too_wide, parse_disabled, parse_indices, parse_opaque, parse_selected,
    parse_text, stringify_text, Coordinates, Text,
};
use crate::panes::terminal_character::{AnsiCode, CharacterStyles, RESET_STYLES};
use yinz_types::style::Style;

use unicode_width::UnicodeWidthChar;
use yinz_runtime::errors::prelude::*;

#[derive(Debug, Clone)]
pub struct NestedListItem {
    pub text: Text,
    pub indentation_level: usize,
}

pub fn nested_list(
    mut contents: Vec<NestedListItem>,
    style: &Style,
    coordinates: Option<Coordinates>,
) -> Result<Vec<u8>> {
    let mut stringified = String::new();
    let max_width = coordinates
        .as_ref()
        .and_then(|c| c.width)
        .unwrap_or_else(|| max_nested_item_width(&contents));
    let max_rows = coordinates
        .as_ref()
        .and_then(|c| c.height)
        .unwrap_or(usize::MAX);
    if max_width.saturating_mul(contents.len().min(max_rows)) > super::MAX_PADDED_CELLS {
        bail!("list padding exceeds cell budget");
    }
    for (line_index, line_item) in contents.drain(..).enumerate() {
        if is_too_high(line_index + 1, &coordinates) {
            break;
        }
        let style_declaration = if line_item.text.selected && !line_item.text.disabled {
            style.colors.list_selected
        } else {
            style.colors.list_unselected
        };
        let padding = line_item.indentation_level * 2 + 1;
        let bulletin = if line_item.indentation_level % 2 == 0 {
            "> "
        } else {
            "- "
        };
        if is_too_wide(padding + bulletin.len(), 0, &coordinates) {
            continue;
        }
        let text_style = if line_item.text.disabled {
            let disabled_style = CharacterStyles::from(style_declaration)
                .bold(Some(AnsiCode::Reset))
                .italic(Some(AnsiCode::On));
            if line_item.text.opaque || line_item.text.selected {
                disabled_style.background(Some(style_declaration.background.into()))
            } else {
                disabled_style
            }
        } else if line_item.text.opaque || line_item.text.selected {
            CharacterStyles::from(style_declaration)
                .background(Some(style_declaration.background.into()))
                .bold(Some(AnsiCode::On))
        } else {
            CharacterStyles::from(style_declaration).bold(Some(AnsiCode::On))
        };
        let text = if line_item.text.disabled {
            line_item.text.into_disabled()
        } else {
            line_item.text
        };
        let (mut text, text_width) = stringify_text(
            &text,
            Some(padding + bulletin.len()),
            &coordinates,
            &style_declaration,
            &style.colors,
            text_style,
        );
        text = pad_line(text, max_width, padding, text_width);
        let go_to_row_instruction = coordinates
            .as_ref()
            .map(|c| c.stringify_with_y_offset(line_index))
            .unwrap_or_else(|| {
                if line_index != 0 {
                    "\n\r".to_string()
                } else {
                    "".to_owned()
                }
            });
        stringified.push_str(&format!(
            "{}{}{:padding$}{bulletin}{}{text}{}",
            go_to_row_instruction, text_style, " ", text_style, RESET_STYLES
        ));
    }
    Ok(stringified.into_bytes())
}

pub fn parse_nested_list_items<'a>(
    params_iter: impl Iterator<Item = &'a mut String>,
) -> Vec<NestedListItem> {
    params_iter
        .flat_map(|stringified| {
            let indentation_level = parse_indentation_level(stringified);
            let selected = parse_selected(stringified);
            let opaque = parse_opaque(stringified);
            let disabled = parse_disabled(stringified);
            let indices = parse_indices(stringified);
            let text = parse_text(stringified).map_err(|e| e.to_string())?;
            let text = Text {
                text,
                opaque,
                selected,
                disabled,
                indices,
            };
            Ok::<NestedListItem, String>(NestedListItem {
                text,
                indentation_level,
            })
        })
        .collect::<Vec<NestedListItem>>()
}

fn parse_indentation_level(stringified: &mut String) -> usize {
    let indentation_level = stringified.bytes().take_while(|byte| *byte == b'|').count();
    stringified.drain(..indentation_level);
    indentation_level
}

fn max_nested_item_width(contents: &[NestedListItem]) -> usize {
    let mut width_of_longest_line = 0;
    for line_item in contents.iter() {
        let mut line_item_text_width = 0;
        for character in line_item.text.text.chars() {
            let character_width = character.width().unwrap_or(0);
            line_item_text_width += character_width;
        }
        let bulletin_width = 2;
        let padding = line_item.indentation_level * 2 + 1;
        let total_width = line_item_text_width + bulletin_width + padding;
        if width_of_longest_line < total_width {
            width_of_longest_line = total_width;
        }
    }
    width_of_longest_line
}

fn pad_line(text: String, max_width: usize, padding: usize, text_width: usize) -> String {
    if max_width > text_width + padding + 2 {
        // 2 is the bulletin
        let end_padding = max_width.saturating_sub(text_width + padding + 2);
        return format!("{}{:end_padding$}", text, " ");
    }
    text
}
