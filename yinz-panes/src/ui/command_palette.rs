//! Native command palette. The screen owns its lifetime and input capture.

use std::cell::Cell;

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
use crate::panes::terminal_character::{
    AnsiCode, RcCharacterStyles, TerminalCharacter, RESET_STYLES,
};
use crate::ui::damage::CellRect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    ControlAgent,
    SetControlAgent,
    Command,
    Plugin,
    Pane,
    Tab,
    Session,
    ExitedSession,
    WarpTabSession,
    WarpPaneSession,
    WarpTab,
    WarpPane,
    Project,
}

impl EntryKind {
    fn parent_label(self) -> Option<&'static str> {
        match self {
            Self::WarpTabSession => Some("Warp tab · Source session"),
            Self::WarpPaneSession => Some("Warp pane · Source session"),
            Self::WarpTab => Some("Warp tab"),
            Self::WarpPane => Some("Warp pane"),
            Self::Project => Some("Switch current project"),
            Self::Command | Self::Plugin => None,
            Self::ControlAgent => Some("Focus control agent"),
            Self::SetControlAgent => Some("Set control agent"),
            Self::Pane => Some("Focus pane"),
            Self::Tab => Some("Switch tab"),
            Self::Session => Some("Switch session"),
            Self::ExitedSession => Some("Exited sessions"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub label: String,
    pub description: String,
    pub shortcut: String,
    pub aliases: Vec<String>,
    pub kind: EntryKind,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    None,
    Dismiss,
    Cancel,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginSection {
    pub title: String,
    pub entries: Vec<Entry>,
    pub semantic: bool,
    pub status: Option<String>,
}

#[derive(Default)]
pub struct CommandPalette {
    pub view: u64,
    pub busy: bool,
    pub pending: bool,
    history: Vec<CommandPalette>,
    native_entries: Vec<Entry>,
    sections: Vec<PluginSection>,
    title: Option<String>,
    text: Option<String>,
    text_offset: usize,
    text_rows: Cell<usize>,
    page_rows: Cell<usize>,
    pub current_project: Option<String>,
    input: TextInput,
    selected: usize,
    selected_status: Option<usize>,
    entries: Vec<Entry>,
    entry_sections: Vec<Option<usize>>,
    matches: Vec<usize>,
    matcher: Matcher,
    picker: Option<EntryKind>,
    saved_search: Vec<(Option<EntryKind>, TextInput, Option<String>)>,
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
    picker_heading_y: Option<usize>,
    result_rows: Vec<ResultRow>,
    text_lines: Vec<String>,
}

enum ResultRow {
    Heading(String),
    Status(usize, String),
    Entry(usize),
}

impl CommandPalette {
    pub fn form_values(&self) -> Option<(&str, Vec<String>)> {
        self.form.as_ref().map(|form| {
            (
                form.id.as_str(),
                form.fields.iter().map(Field::text_value).collect(),
            )
        })
    }

    pub fn query(&self) -> &str {
        self.input.get_text()
    }

    pub fn is_root(&self) -> bool {
        self.view == 0 && self.picker.is_none() && self.form.is_none()
    }

    pub fn set_entries(&mut self, entries: Vec<Entry>) {
        if self.view != 0 || self.native_entries == entries {
            return;
        }
        self.native_entries = entries;
        self.rebuild_entries();
    }

    pub fn set_sections(&mut self, sections: Vec<PluginSection>) {
        if self.sections == sections {
            return;
        }
        self.sections = sections;
        self.rebuild_entries();
    }

    pub fn set_section_status(&mut self, status: Option<String>) {
        if let Some(section) = self.sections.first_mut() {
            section.status = status;
            self.rebuild_entries();
        }
    }

    fn rebuild_entries(&mut self) {
        let selected = {
            self.matches
                .get(self.selected)
                .map(|i| self.entries[*i].id.clone())
        };
        self.entries = self
            .native_entries
            .iter()
            .chain(self.sections.iter().flat_map(|s| &s.entries))
            .cloned()
            .collect();
        self.entry_sections = std::iter::repeat_n(None, self.native_entries.len())
            .chain(
                self.sections
                    .iter()
                    .enumerate()
                    .flat_map(|(i, section)| std::iter::repeat_n(Some(i), section.entries.len())),
            )
            .collect();
        self.rematch();
        if let Some(id) = selected {
            if let Some(index) = self.matches.iter().position(|i| self.entries[*i].id == id) {
                self.selected = index;
            } else {
                self.selected = 0;
            }
        } else {
            self.selected = 0;
        }
    }

    pub fn push_view(&mut self, id: u64, title: String) {
        let mut history = std::mem::take(&mut self.history);
        let mut previous = std::mem::take(self);
        previous.busy = false;
        previous.pending = false;
        self.current_project.clone_from(&previous.current_project);
        history.push(previous);
        self.history = history;
        self.view = id;
        self.title = Some(title);
    }

    pub fn open_text(&mut self, text: String) {
        self.text = Some(text);
    }

    pub fn reset_to_root(&mut self) {
        while self.view != 0 {
            self.busy = false;
            self.pending = false;
            self.back();
        }
        self.busy = false;
        self.pending = false;
    }

    fn selected_entry(&self) -> Option<&Entry> {
        if self.selected_status.is_some() {
            return None;
        }
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
            .filter(|(_, entry)| {
                self.picker.map_or(
                    !matches!(
                        entry.kind,
                        EntryKind::Project
                            | EntryKind::ControlAgent
                            | EntryKind::SetControlAgent
                            | EntryKind::WarpTabSession
                            | EntryKind::WarpPaneSession
                            | EntryKind::WarpTab
                            | EntryKind::WarpPane
                    ),
                    |kind| {
                        entry.kind == kind
                            || (kind == EntryKind::Session
                                && entry.kind == EntryKind::ExitedSession)
                    },
                )
            })
            .filter_map(|(index, entry)| {
                if self.entry_sections[index].is_some_and(|i| self.sections[i].semantic) {
                    return Some((index, 0));
                }
                std::iter::once(&entry.label)
                    .chain(entry.aliases.iter())
                    .filter_map(|text| {
                        pattern.score(Utf32Str::new(text, &mut buffer), &mut self.matcher)
                    })
                    .max()
                    .map(|score| (index, score))
            })
            .collect();
        matches.sort_by_key(|(index, score)| {
            let section = self.entry_sections[*index];
            let group = match self.entries[*index].kind {
                EntryKind::Plugin => 3 + section.unwrap_or(0),
                EntryKind::Session => 1,
                EntryKind::ExitedSession => 2,
                _ => 0,
            };
            (group, std::cmp::Reverse(*score))
        });
        self.matches = matches.into_iter().map(|(index, _)| index).collect();
        self.selected = self.selected.min(self.matches.len().saturating_sub(1));
        self.selected_status = if self.matches.is_empty() && self.picker.is_none() {
            self.sections
                .iter()
                .position(|section| section.status.is_some())
        } else {
            None
        };
    }

    pub fn open_picker(&mut self, kind: EntryKind) {
        let selected = self.selected_entry().map(|entry| entry.id.clone());
        self.saved_search
            .push((self.picker, std::mem::take(&mut self.input), selected));
        self.picker = Some(kind);
        self.selected = 0;
        self.error = None;
        self.rematch();
    }

    pub fn open_form(&mut self, form: Form) {
        self.form = Some(form);
        self.error = None;
    }

    pub fn clear_error(&mut self) {
        self.error = None;
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn back(&mut self) -> Outcome {
        self.error = None;
        if self.busy || (self.pending && !self.is_root()) {
            return Outcome::Cancel;
        }
        if let Some(mut previous) = self.history.pop() {
            previous.history = std::mem::take(&mut self.history);
            *self = previous;
            return Outcome::None;
        }
        if self.form.take().is_some() {
            return Outcome::None;
        }
        if let Some((picker, input, selected)) = self.saved_search.pop() {
            self.input = input;
            self.picker = picker;
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
        let key = match key.bare_key {
            BareKey::Char('n') if key.has_only_modifiers(&[KeyModifier::Ctrl]) => {
                KeyWithModifier::new(BareKey::Down)
            },
            BareKey::Char('p') if key.has_only_modifiers(&[KeyModifier::Ctrl]) => {
                KeyWithModifier::new(BareKey::Up)
            },
            _ => key,
        };
        if key.bare_key == BareKey::Esc {
            return self.back();
        }
        if self.busy {
            return Outcome::None;
        }
        if self.text.is_some() {
            let page = self.page_rows.get().max(1);
            let end = self.text_rows.get().saturating_sub(page);
            self.text_offset = self.text_offset.min(end);
            match key.bare_key {
                BareKey::Up => self.text_offset = self.text_offset.saturating_sub(1),
                BareKey::Down => self.text_offset = (self.text_offset + 1).min(end),
                BareKey::PageUp => self.text_offset = self.text_offset.saturating_sub(page),
                BareKey::PageDown => {
                    self.text_offset = self.text_offset.saturating_add(page).min(end)
                },
                BareKey::Home => self.text_offset = 0,
                BareKey::End => self.text_offset = end,
                _ => {},
            }
            return Outcome::None;
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
            BareKey::Up | BareKey::Down if key.has_no_modifiers() => {
                let rows = self.result_rows(1);
                let selected = rows
                    .iter()
                    .position(|row| self.row_selected(row))
                    .unwrap_or(0);
                let next = if key.bare_key == BareKey::Up {
                    selected.saturating_sub(1)
                } else {
                    (selected + 1).min(rows.len().saturating_sub(1))
                };
                if let Some(row) = rows.get(next) {
                    self.select_row(row);
                }
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
                    self.rematch_query();
                }
            },
        }
        Outcome::None
    }

    pub fn insert_text(&mut self, text: &str) {
        if self.busy || self.text.is_some() {
            return;
        }
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
            self.rematch_query();
        }
    }

    fn rematch_query(&mut self) {
        let selected = self.matches.get(self.selected).copied().filter(|index| {
            self.selected_status.is_none()
                && self.entry_sections[*index].is_some_and(|i| self.sections[i].semantic)
        });
        self.selected = 0;
        self.error = None;
        self.rematch();
        if let Some(index) = selected {
            self.selected = self.matches.iter().position(|i| *i == index).unwrap_or(0);
        }
    }

    fn selection(&self) -> usize {
        self.form
            .as_ref()
            .map_or(self.selected, |form| form.selected)
    }

    fn row_selected(&self, row: &ResultRow) -> bool {
        match row {
            ResultRow::Entry(index) => self.selected_status.is_none() && *index == self.selected,
            ResultRow::Status(section, _) => self.selected_status == Some(*section),
            ResultRow::Heading(_) => false,
        }
    }

    fn select_row(&mut self, row: &ResultRow) {
        match row {
            ResultRow::Entry(index) => {
                self.selected = *index;
                self.selected_status = None;
            },
            ResultRow::Status(section, _) => self.selected_status = Some(*section),
            ResultRow::Heading(_) => (),
        }
    }

    fn result_rows(&self, capacity: usize) -> Vec<ResultRow> {
        if capacity == 0 || self.form.is_some() || self.text.is_some() {
            return vec![];
        }
        let mut rows = Vec::new();
        let mut previous = None;
        for (index, entry_index) in self.matches.iter().enumerate() {
            let entry = &self.entries[*entry_index];
            if entry.kind == EntryKind::Plugin {
                continue;
            }
            let label = if self.picker == Some(EntryKind::Session) {
                match entry.kind {
                    EntryKind::Session => Some("Live sessions"),
                    EntryKind::ExitedSession => Some("Exited sessions"),
                    _ => None,
                }
            } else if self.picker.is_none() {
                entry.kind.parent_label()
            } else {
                None
            };
            if label != previous && capacity >= 2 {
                if let Some(label) = label {
                    rows.push(ResultRow::Heading(label.into()));
                }
            }
            rows.push(ResultRow::Entry(index));
            previous = label;
        }
        if self.picker.is_none() {
            for (section_index, section) in self.sections.iter().enumerate() {
                if capacity >= 2 {
                    rows.push(ResultRow::Heading(section.title.clone()));
                }
                for (index, entry) in self.matches.iter().enumerate() {
                    if self.entry_sections[*entry] == Some(section_index) {
                        rows.push(ResultRow::Entry(index));
                    }
                }
                if let Some(status) = &section.status {
                    rows.push(ResultRow::Status(section_index, status.clone()));
                }
            }
        }
        rows
    }

    fn visible_result_rows(&self, capacity: usize) -> Vec<ResultRow> {
        let rows = self.result_rows(capacity);
        if rows.is_empty() {
            return rows;
        }
        let selected = rows
            .iter()
            .position(|row| self.row_selected(row))
            .unwrap_or(0);
        let mut start = (selected + 1).saturating_sub(capacity);
        let sticky_heading = |start: usize| {
            if let Some(ResultRow::Status(section, _)) = rows.get(start) {
                return Some(self.sections[*section].title.clone());
            }
            let Some(ResultRow::Entry(index)) = rows.get(start) else {
                return None;
            };
            let entry = &self.entries[self.matches[*index]];
            if entry.kind == EntryKind::Plugin {
                self.entry_sections[self.matches[*index]].map(|i| self.sections[i].title.clone())
            } else if self.picker == Some(EntryKind::Session) {
                match entry.kind {
                    EntryKind::Session => Some("Live sessions".into()),
                    EntryKind::ExitedSession => Some("Exited sessions".into()),
                    _ => None,
                }
            } else if self.picker.is_none() {
                entry.kind.parent_label().map(String::from)
            } else {
                None
            }
        };
        // Account for a repeated section heading before choosing the visible entry window.
        if capacity >= 2 && sticky_heading(start).is_some() && selected >= start + capacity - 1 {
            start += 1;
        }
        let mut visible = Vec::new();
        if capacity >= 2 {
            if let Some(heading) = sticky_heading(start) {
                visible.push(ResultRow::Heading(heading));
            }
        }
        let remaining = capacity - visible.len();
        visible.extend(rows.into_iter().skip(start).take(remaining));
        if matches!(visible.last(), Some(ResultRow::Heading(_))) {
            visible.pop();
        }
        visible
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
        let text_lines = self
            .text
            .as_ref()
            .map(|text| {
                crate::ui::notification::wrapped_text_lines(text, cols.saturating_sub(2 * border))
            })
            .unwrap_or_default();
        self.text_rows.set(text_lines.len());
        self.page_rows.set(options_rows);
        let offset = self
            .text_offset
            .min(text_lines.len().saturating_sub(options_rows));
        CommandPaletteLayout {
            rect,
            border,
            input_y,
            options_y,
            options_rows,
            first_option: (self.selection() + 1).saturating_sub(options_rows),
            footer_y,
            picker_heading_y: (self.picker.is_some() && self.form.is_none() && rows >= 6)
                .then_some(input_y + 1),
            result_rows: self.visible_result_rows(options_rows),
            text_lines: text_lines
                .into_iter()
                .skip(offset)
                .take(options_rows)
                .collect(),
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
            } else if let Some(row) = layout.result_rows.get(y - layout.options_y) {
                self.select_row(row);
            }
        }
    }

    pub fn cursor(&self, layout: &CommandPaletteLayout) -> Option<(usize, usize)> {
        if layout.rect.is_empty() || self.text.is_some() || self.busy {
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
        let heading: RcCharacterStyles = RESET_STYLES
            .foreground(Some(style.colors.text_unselected.base.into()))
            .dim(Some(AnsiCode::On))
            .into();
        let width = rect.cols - 2 * layout.border;
        (rect.y..rect.y + rect.rows)
            .map(|y| {
                let mut shortcut_start = usize::MAX;
                let (text, styles) = if layout.border == 1 && y == rect.y {
                    (
                        format!(
                            "╭{}╮",
                            fit(
                                &format!(
                                    " Do · Project: {} ",
                                    self.current_project.as_deref().unwrap_or("None")
                                ),
                                width,
                                '─'
                            )
                        ),
                        border.clone(),
                    )
                } else if layout.border == 1 && y == rect.y + rect.rows - 1 {
                    (format!("╰{}╯", "─".repeat(width)), border.clone())
                } else {
                    let (text, styles) = if y == layout.input_y {
                        (
                            self.form.as_ref().map_or_else(
                                || {
                                    if self.text.is_some() {
                                        self.title.clone().unwrap_or_default()
                                    } else {
                                        self.input_line(layout).0
                                    }
                                },
                                |form| form.title.clone(),
                            ),
                            fill.clone(),
                        )
                    } else if Some(y) == layout.picker_heading_y {
                        (
                            self.picker
                                .and_then(EntryKind::parent_label)
                                .unwrap_or_default()
                                .into(),
                            heading.clone(),
                        )
                    } else if Some(y) == layout.footer_y {
                        (
                            self.error.clone().unwrap_or_else(|| {
                                if self.busy {
                                    "Working…  Esc Cancel"
                                } else if self.text.is_some() {
                                    "↑↓ Scroll  PgUp/PgDn Page  Home/End  Esc Back"
                                } else if self.form.is_some() {
                                    "Tab/↑↓/Ctrl+n/p Fields  Enter Submit  Esc Back"
                                } else if self.picker.is_some() || self.view != 0 {
                                    "↑↓/Ctrl+n/p Select  Enter Choose  Esc Back"
                                } else {
                                    "↑↓/Ctrl+n/p Select  Enter Choose  Esc Close"
                                }
                                .into()
                            }),
                            if self.error.is_some() {
                                fill.clone()
                            } else {
                                heading.clone()
                            },
                        )
                    } else if (layout.options_y..layout.options_y + layout.options_rows)
                        .contains(&y)
                    {
                        let row = layout.result_rows.get(y - layout.options_y);
                        let index = if self.form.is_some() {
                            layout.first_option + y - layout.options_y
                        } else if let Some(ResultRow::Entry(index)) = row {
                            *index
                        } else {
                            usize::MAX
                        };
                        let is_selected = if self.form.is_some() {
                            index == self.selection()
                        } else {
                            row.is_some_and(|row| self.row_selected(row))
                        };
                        let text = if self.text.is_some() {
                            layout
                                .text_lines
                                .get(y - layout.options_y)
                                .cloned()
                                .unwrap_or_default()
                        } else if let Some(form) = &self.form {
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
                        } else if let Some(
                            ResultRow::Heading(label) | ResultRow::Status(_, label),
                        ) = row
                        {
                            label.clone()
                        } else if let Some(entry) =
                            self.matches.get(index).map(|i| &self.entries[*i])
                        {
                            let label = format!(
                                "{} {}{}",
                                if is_selected { '›' } else { ' ' },
                                entry.label,
                                if entry.description.is_empty() {
                                    String::new()
                                } else {
                                    format!(" — {}", entry.description)
                                }
                            );
                            if entry.shortcut.is_empty() {
                                label
                            } else {
                                use unicode_width::UnicodeWidthStr;
                                let shortcut =
                                    take_columns(&entry.shortcut, width.saturating_sub(3));
                                let label =
                                    fit(&label, width.saturating_sub(shortcut.width() + 1), ' ');
                                shortcut_start = label.chars().count() + 1;
                                format!("{label} {shortcut}")
                            }
                        } else if self.matches.is_empty() && y == layout.options_y {
                            if self.picker == Some(EntryKind::Project)
                                && !self.entries.iter().any(|e| e.kind == EntryKind::Project)
                            {
                                "No projects".into()
                            } else if self.picker == Some(EntryKind::ControlAgent)
                                && !self
                                    .entries
                                    .iter()
                                    .any(|e| e.kind == EntryKind::ControlAgent)
                            {
                                "No control agents".into()
                            } else if self.picker == Some(EntryKind::SetControlAgent)
                                && !self
                                    .entries
                                    .iter()
                                    .any(|e| e.kind == EntryKind::SetControlAgent)
                            {
                                "No agent panes".into()
                            } else {
                                "No matches".into()
                            }
                        } else {
                            String::new()
                        };
                        (
                            text,
                            if is_selected {
                                selected.clone()
                            } else if matches!(
                                row,
                                Some(ResultRow::Heading(_) | ResultRow::Status(_, _))
                            ) {
                                heading.clone()
                            } else {
                                fill.clone()
                            },
                        )
                    } else {
                        (String::new(), fill.clone())
                    };
                    let text = fit(
                        &text
                            .chars()
                            .map(|c| if c.is_control() { ' ' } else { c })
                            .collect::<String>(),
                        width,
                        ' ',
                    );
                    let subtle: RcCharacterStyles = styles.dim(Some(AnsiCode::On)).into();
                    let mut characters = Vec::new();
                    if layout.border == 1 {
                        characters.push(TerminalCharacter::new_styled('│', border.clone()));
                    }
                    characters.extend(text.chars().enumerate().map(|(index, c)| {
                        TerminalCharacter::new_styled(
                            c,
                            if index >= shortcut_start {
                                subtle.clone()
                            } else {
                                styles.clone()
                            },
                        )
                    }));
                    if layout.border == 1 {
                        characters.push(TerminalCharacter::new_styled('│', border.clone()));
                    }
                    return CharacterChunk::new(characters, rect.x, y);
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
