//! Native command palette. The screen owns its lifetime and input capture.

use nucleo_matcher::{
    pattern::{AtomKind, CaseMatching, Normalization, Pattern},
    Matcher, Utf32Str,
};
use yinz_types::keys::{BareKey, KeyModifier, KeyWithModifier};
use yinz_types::pane_size::Viewport;
use yinz_types::position::Position;
use yinz_types::style::Style;
use yinz_types::text_input::viewport::{take_columns, viewport};
use yinz_types::text_input::TextInput;

use crate::output::CharacterChunk;
use crate::panes::terminal_character::{RcCharacterStyles, TerminalCharacter, RESET_STYLES};
use crate::ui::damage::CellRect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Command,
    Pane,
    Tab,
    Session,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub label: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub kind: EntryKind,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    None,
    Dismiss,
    Activate(String),
    Submit { id: String, values: Vec<String> },
}

pub enum FieldValue {
    Text(TextInput),
    Choice {
        options: Vec<String>,
        selected: usize,
    },
}

pub struct Field {
    pub label: String,
    pub value: FieldValue,
}

impl Field {
    pub fn text(label: &str, value: &str) -> Self {
        Self {
            label: label.into(),
            value: FieldValue::Text(TextInput::new(value.into())),
        }
    }

    pub fn choice(label: &str, options: &[&str]) -> Self {
        Self {
            label: label.into(),
            value: FieldValue::Choice {
                options: options.iter().map(|s| (*s).into()).collect(),
                selected: 0,
            },
        }
    }

    fn text_value(&self) -> String {
        match &self.value {
            FieldValue::Text(input) => input.get_text().into(),
            FieldValue::Choice { options, selected } => {
                options.get(*selected).cloned().unwrap_or_default()
            },
        }
    }
}

pub struct Form {
    pub id: String,
    pub title: String,
    pub fields: Vec<Field>,
    pub submit: String,
    pub confirmation: bool,
    selected: usize,
}

impl Form {
    pub fn new(
        id: String,
        title: String,
        fields: Vec<Field>,
        submit: &str,
        confirmation: bool,
    ) -> Self {
        Self {
            id,
            title,
            fields,
            submit: submit.into(),
            confirmation,
            selected: 0,
        }
    }

    fn rows(&self) -> usize {
        self.fields.len() + 1 + usize::from(self.confirmation)
    }
}

#[derive(Default)]
pub struct CommandPalette {
    input: TextInput,
    selected: usize,
    entries: Vec<Entry>,
    matches: Vec<usize>,
    matcher: Matcher,
    picker: Option<EntryKind>,
    saved_search: Option<(TextInput, Option<String>)>,
    form: Option<Form>,
    error: Option<String>,
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
    pub fn set_entries(&mut self, entries: Vec<Entry>) {
        if entries == self.entries {
            return;
        }
        let selected = self.selected_entry().map(|entry| entry.id.clone());
        self.entries = entries;
        self.rematch();
        self.selected = selected
            .and_then(|id| self.matches.iter().position(|i| self.entries[*i].id == id))
            .unwrap_or(0);
    }

    fn selected_entry(&self) -> Option<&Entry> {
        self.matches.get(self.selected).map(|i| &self.entries[*i])
    }

    fn rematch(&mut self) {
        let pattern = Pattern::new(
            self.input.get_text(),
            CaseMatching::Ignore,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );
        let mut buffer = Vec::new();
        let mut matches: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| self.picker.is_none_or(|kind| entry.kind == kind))
            .filter_map(|(index, entry)| {
                std::iter::once(&entry.label)
                    .chain(entry.aliases.iter())
                    .filter_map(|text| {
                        pattern.score(Utf32Str::new(text, &mut buffer), &mut self.matcher)
                    })
                    .max()
                    .map(|score| (index, score))
            })
            .collect();
        matches.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
        self.matches = matches.into_iter().map(|(index, _)| index).collect();
        self.selected = self.selected.min(self.matches.len().saturating_sub(1));
    }

    pub fn open_picker(&mut self, kind: EntryKind) {
        let selected = self.selected_entry().map(|entry| entry.id.clone());
        self.saved_search = Some((std::mem::take(&mut self.input), selected));
        self.picker = Some(kind);
        self.selected = 0;
        self.error = None;
        self.rematch();
    }

    pub fn open_form(&mut self, form: Form) {
        self.form = Some(form);
        self.error = None;
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    fn back(&mut self) -> Outcome {
        self.error = None;
        if self.form.take().is_some() {
            return Outcome::None;
        }
        if let Some((input, selected)) = self.saved_search.take() {
            self.input = input;
            self.picker = None;
            self.rematch();
            self.selected = selected
                .and_then(|id| self.matches.iter().position(|i| self.entries[*i].id == id))
                .unwrap_or(0);
            Outcome::None
        } else {
            Outcome::Dismiss
        }
    }

    pub fn handle_key(&mut self, key: KeyWithModifier) -> Outcome {
        if key.bare_key == BareKey::Esc {
            return self.back();
        }
        // TextInput's Alt-letter fallback inserts text. These are shortcuts here.
        if matches!(key.bare_key, BareKey::Char(_))
            && (key.has_modifiers(&[KeyModifier::Alt]) || key.has_modifiers(&[KeyModifier::Super]))
        {
            return Outcome::None;
        }
        if let Some(form) = &mut self.form {
            match key.bare_key {
                BareKey::Up => form.selected = (form.selected + form.rows() - 1) % form.rows(),
                BareKey::Tab if key.has_modifiers(&[KeyModifier::Shift]) => {
                    form.selected = (form.selected + form.rows() - 1) % form.rows();
                },
                BareKey::Down | BareKey::Tab => form.selected = (form.selected + 1) % form.rows(),
                BareKey::Enter => {
                    if form.selected < form.fields.len() {
                        form.selected += 1;
                    } else if form.confirmation && form.selected == form.fields.len() {
                        return self.back();
                    } else {
                        return Outcome::Submit {
                            id: form.id.clone(),
                            values: form.fields.iter().map(Field::text_value).collect(),
                        };
                    }
                },
                _ => {
                    if let Some(field) = form.fields.get_mut(form.selected) {
                        match &mut field.value {
                            FieldValue::Text(input) => {
                                input.handle_key(key);
                            },
                            FieldValue::Choice { options, selected } if !options.is_empty() => {
                                match key.bare_key {
                                    BareKey::Left => {
                                        *selected = (*selected + options.len() - 1) % options.len()
                                    },
                                    BareKey::Right => *selected = (*selected + 1) % options.len(),
                                    _ => {},
                                }
                            },
                            _ => {},
                        }
                    }
                },
            }
            return Outcome::None;
        }
        match key.bare_key {
            BareKey::Up if key.has_no_modifiers() => {
                self.selected = self.selected.saturating_sub(1)
            },
            BareKey::Down if key.has_no_modifiers() => {
                self.selected = (self.selected + 1).min(self.matches.len().saturating_sub(1))
            },
            BareKey::Enter if key.has_no_modifiers() => {
                if let Some(entry) = self.selected_entry() {
                    return Outcome::Activate(entry.id.clone());
                }
            },
            _ => {
                let before = self.input.get_text().to_owned();
                self.input.handle_key(key);
                if self.input.get_text() != before {
                    self.selected = 0;
                    self.error = None;
                    self.rematch();
                }
            },
        }
        Outcome::None
    }

    pub fn insert_text(&mut self, text: &str) {
        let text: String = text
            .chars()
            .map(|c| if c.is_whitespace() { ' ' } else { c })
            .collect();
        if let Some(form) = &mut self.form {
            if let Some(Field {
                value: FieldValue::Text(input),
                ..
            }) = form.fields.get_mut(form.selected)
            {
                input.insert_text(&text);
            }
        } else {
            self.input.insert_text(&text);
            self.selected = 0;
            self.error = None;
            self.rematch();
        }
    }

    fn selection(&self) -> usize {
        self.form
            .as_ref()
            .map_or(self.selected, |form| form.selected)
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
            first_option: (self.selection() + 1).saturating_sub(options_rows),
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
            if let Some(form) = &mut self.form {
                if index < form.rows() {
                    form.selected = index;
                }
            } else if index < self.matches.len() {
                self.selected = index;
            }
        }
    }

    pub fn cursor(&self, layout: &CommandPaletteLayout) -> Option<(usize, usize)> {
        if layout.rect.is_empty() {
            return None;
        }
        if let Some(form) = &self.form {
            let field = form.fields.get(form.selected)?;
            let FieldValue::Text(input) = &field.value else {
                return None;
            };
            let y = layout.options_y + form.selected.saturating_sub(layout.first_option);
            if y >= layout.options_y + layout.options_rows {
                return None;
            }
            let (_, column) = viewport(
                &format!("{}: ", field.label),
                input.get_text(),
                input.get_cursor_position(),
                layout.rect.cols.saturating_sub(2 * layout.border),
            );
            return Some((layout.rect.x + layout.border + column, y));
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
        let width = rect.cols - 2 * layout.border;
        (rect.y..rect.y + rect.rows)
            .map(|y| {
                let (text, styles) = if layout.border == 1 && y == rect.y {
                    (format!("╭{}╮", fit(" Do ", width, '─')), border.clone())
                } else if layout.border == 1 && y == rect.y + rect.rows - 1 {
                    (format!("╰{}╯", "─".repeat(width)), border.clone())
                } else {
                    let (text, styles) = if y == layout.input_y {
                        (
                            self.form.as_ref().map_or_else(
                                || self.input_line(layout).0,
                                |form| form.title.clone(),
                            ),
                            fill.clone(),
                        )
                    } else if Some(y) == layout.footer_y {
                        (
                            self.error.clone().unwrap_or_else(|| {
                                if self.form.is_some() {
                                    "Tab/↑↓ Fields  Enter Next/Submit  Esc Back"
                                } else if self.picker.is_some() {
                                    "↑↓ Select  Enter Choose  Esc Back"
                                } else {
                                    "↑↓ Select  Enter Choose  Esc Close"
                                }
                                .into()
                            }),
                            fill.clone(),
                        )
                    } else if (layout.options_y..layout.options_y + layout.options_rows)
                        .contains(&y)
                    {
                        let index = layout.first_option + y - layout.options_y;
                        let text = if let Some(form) = &self.form {
                            if let Some(field) = form.fields.get(index) {
                                match &field.value {
                                    FieldValue::Text(input) => {
                                        viewport(
                                            &format!("{}: ", field.label),
                                            input.get_text(),
                                            input.get_cursor_position(),
                                            width,
                                        )
                                        .0
                                    },
                                    FieldValue::Choice { .. } => {
                                        format!("{}: ‹ {} ›", field.label, field.text_value())
                                    },
                                }
                            } else if form.confirmation && index == form.fields.len() {
                                "Cancel".into()
                            } else if index == form.rows() - 1 {
                                form.submit.clone()
                            } else {
                                String::new()
                            }
                        } else if let Some(entry) =
                            self.matches.get(index).map(|i| &self.entries[*i])
                        {
                            format!(
                                "{} {}{}",
                                if index == self.selected { '›' } else { ' ' },
                                entry.label,
                                if entry.description.is_empty() {
                                    String::new()
                                } else {
                                    format!(" — {}", entry.description)
                                }
                            )
                        } else if self.matches.is_empty() && index == 0 {
                            "No matches".into()
                        } else {
                            String::new()
                        };
                        (
                            text,
                            if index == self.selection() {
                                selected.clone()
                            } else {
                                fill.clone()
                            },
                        )
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
