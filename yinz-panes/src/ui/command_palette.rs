//! Native command palette prototype. The screen owns its lifetime and input capture.

use yinz_types::keys::{BareKey, KeyModifier, KeyWithModifier};
use yinz_types::pane_size::Viewport;
use yinz_types::position::Position;
use yinz_types::style::Style;
use yinz_types::text_input::viewport::{take_columns, viewport};
use yinz_types::text_input::{InputAction, TextInput};

use crate::output::CharacterChunk;
use crate::panes::terminal_character::{RcCharacterStyles, TerminalCharacter, RESET_STYLES};
use crate::ui::damage::CellRect;

const OPTIONS: [&str; 6] = [
    "New pane",
    "New tab",
    "Find file",
    "Switch session",
    "Open settings",
    "Help",
];

#[derive(Default)]
pub struct CommandPalette {
    input: TextInput,
    selected: usize,
    feedback: Option<&'static str>,
}

pub struct CommandPaletteLayout {
    pub rect: CellRect,
    border: usize,
    input_y: usize,
    options_y: usize,
    options_rows: usize,
    first_option: usize,
    footer_y: Option<usize>,
}

impl CommandPalette {
    fn matches(&self) -> Vec<&'static str> {
        let query = self.input.get_text().to_lowercase();
        OPTIONS
            .into_iter()
            .filter(|label| label.to_lowercase().contains(&query))
            .collect()
    }

    /// Returns true only when Esc dismisses the palette. All other keys stay captured.
    pub fn handle_key(&mut self, key: KeyWithModifier) -> bool {
        match key.bare_key {
            BareKey::Esc => return true,
            BareKey::Up if key.has_no_modifiers() => {
                self.selected = self.selected.saturating_sub(1);
            },
            BareKey::Down if key.has_no_modifiers() => {
                self.selected = (self.selected + 1).min(self.matches().len().saturating_sub(1));
            },
            // TextInput's Alt-letter fallback inserts text. These are shortcuts here.
            BareKey::Char(_)
                if key.has_modifiers(&[KeyModifier::Alt])
                    || key.has_modifiers(&[KeyModifier::Super]) => {},
            _ => {
                let before = self.input.get_text().to_owned();
                if self.input.handle_key(key) == InputAction::Submit {
                    self.feedback = self.matches().get(self.selected).copied();
                }
                if self.input.get_text() != before {
                    self.selected = 0;
                    self.feedback = None;
                }
            },
        }
        false
    }

    pub fn insert_text(&mut self, text: &str) {
        let text: String = text
            .chars()
            .map(|c| if c.is_whitespace() { ' ' } else { c })
            .collect();
        self.input.insert_text(&text);
        self.selected = 0;
        self.feedback = None;
    }

    pub fn layout(&self, surface: Viewport) -> CommandPaletteLayout {
        let cols = surface.cols.min(64);
        let rows = surface.rows.min(11);
        let rect = CellRect {
            x: surface.x + (surface.cols - cols) / 2,
            y: surface.y
                + (surface.rows / 3)
                    .saturating_sub(rows / 2)
                    .min(surface.rows - rows),
            cols,
            rows,
        };
        let border = usize::from(cols >= 3 && rows >= 4);
        let input_y = rect.y + border;
        let options_y = input_y + 1 + usize::from(rows >= 6);
        let footer_y = (rows >= 5).then_some(rect.y + rows.saturating_sub(border + 1));
        let options_rows = footer_y
            .unwrap_or(rect.y + rows - border)
            .saturating_sub(options_y);
        CommandPaletteLayout {
            rect,
            border,
            input_y,
            options_y,
            options_rows,
            first_option: (self.selected + 1).saturating_sub(options_rows),
            footer_y,
        }
    }

    pub fn click(&mut self, layout: &CommandPaletteLayout, position: Position) {
        let Ok(y) = usize::try_from(position.line()) else {
            return;
        };
        if !layout.rect.contains(position.column(), y) {
            return;
        }
        if (layout.options_y..layout.options_y + layout.options_rows).contains(&y) {
            let index = layout.first_option + y - layout.options_y;
            if index < self.matches().len() {
                self.selected = index;
            }
        }
    }

    pub fn cursor(&self, layout: &CommandPaletteLayout) -> Option<(usize, usize)> {
        if layout.rect.is_empty() {
            return None;
        }
        let (_, column) = self.input_line(layout);
        Some((layout.rect.x + layout.border + column, layout.input_y))
    }

    fn input_line(&self, layout: &CommandPaletteLayout) -> (String, usize) {
        viewport(
            "> ",
            self.input.get_text(),
            self.input.get_cursor_position(),
            layout.rect.cols.saturating_sub(2 * layout.border),
        )
    }

    pub fn chunks(&self, layout: &CommandPaletteLayout, style: &Style) -> Vec<CharacterChunk> {
        let rect = layout.rect;
        if rect.is_empty() {
            return vec![];
        }
        // Default-background spaces let the compositor reveal the tinted text underneath.
        let fill: RcCharacterStyles = RESET_STYLES
            .foreground(Some(style.colors.text_unselected.base.into()))
            .into();
        let border: RcCharacterStyles = RESET_STYLES
            .foreground(Some(style.colors.text_unselected.emphasis_0.into()))
            .into();
        let selected: RcCharacterStyles = RESET_STYLES
            .foreground(Some(style.colors.text_selected.base.into()))
            .background(Some(style.colors.text_selected.background.into()))
            .into();
        let matches = self.matches();
        let width = rect.cols - 2 * layout.border;
        (rect.y..rect.y + rect.rows)
            .map(|y| {
                let (text, styles) = if layout.border == 1 && y == rect.y {
                    (format!("╭{}╮", fit(" Do ", width, '─')), border.clone())
                } else if layout.border == 1 && y == rect.y + rect.rows - 1 {
                    (format!("╰{}╯", "─".repeat(width)), border.clone())
                } else {
                    let (text, styles) = if y == layout.input_y {
                        (self.input_line(layout).0, fill.clone())
                    } else if Some(y) == layout.footer_y {
                        (
                            self.feedback
                                .map(|label| format!("Selected: {label}"))
                                .unwrap_or_else(|| "↑↓ Select  Enter Choose  Esc Close".into()),
                            fill.clone(),
                        )
                    } else if (layout.options_y..layout.options_y + layout.options_rows)
                        .contains(&y)
                    {
                        let index = layout.first_option + y - layout.options_y;
                        match matches.get(index) {
                            Some(label) => (
                                format!(
                                    "{} {label}",
                                    if index == self.selected { '›' } else { ' ' }
                                ),
                                if index == self.selected {
                                    selected.clone()
                                } else {
                                    fill.clone()
                                },
                            ),
                            None if matches.is_empty() && y == layout.options_y => {
                                ("No matches".into(), fill.clone())
                            },
                            None => (String::new(), fill.clone()),
                        }
                    } else {
                        (String::new(), fill.clone())
                    };
                    let text = fit(&text, width, ' ');
                    if layout.border == 1 {
                        let mut characters =
                            vec![TerminalCharacter::new_styled('│', border.clone())];
                        characters.extend(
                            text.chars()
                                .map(|c| TerminalCharacter::new_styled(c, styles.clone())),
                        );
                        characters.push(TerminalCharacter::new_styled('│', border.clone()));
                        return CharacterChunk::new(characters, rect.x, y);
                    }
                    (text, styles)
                };
                CharacterChunk::new(
                    text.chars()
                        .map(|c| TerminalCharacter::new_styled(c, styles.clone()))
                        .collect(),
                    rect.x,
                    y,
                )
            })
            .collect()
    }
}

fn fit(text: &str, cols: usize, pad: char) -> String {
    use unicode_width::UnicodeWidthChar;
    let mut text = take_columns(text, cols);
    let width: usize = text.chars().map(|c| c.width().unwrap_or(0)).sum();
    text.extend(std::iter::repeat_n(pad, cols.saturating_sub(width)));
    text
}

#[cfg(test)]
#[path = "unit/command_palette_tests.rs"]
mod tests;
