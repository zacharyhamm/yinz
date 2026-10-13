//! Native command palette. The screen owns its lifetime and input capture.

use std::cell::{Cell, RefCell};

#[path = "command_palette_agent.rs"]
mod agent;

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
    Agent,
    Mother,
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
    Theme,
}

impl EntryKind {
    fn parent_label(self) -> Option<&'static str> {
        match self {
            Self::WarpTabSession => Some("Warp tab · Source session"),
            Self::WarpPaneSession => Some("Warp pane · Source session"),
            Self::WarpTab => Some("Warp tab"),
            Self::WarpPane => Some("Warp pane"),
            Self::Theme => Some("Change theme"),
            Self::Project => Some("Switch current project"),
            Self::Command | Self::Plugin => None,
            Self::Agent => Some("Agents"),
            Self::Mother => Some("Launch mother"),
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
    FocusAgent(String),
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
    pub actions: Vec<(String, String)>,
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
            actions: vec![],
            selected: 0,
        }
    }

    fn rows(&self) -> usize {
        self.fields.len() + 1 + usize::from(self.confirmation) + self.actions.len()
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
    result_width: Cell<usize>,
    pub agent_metadata:
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
    pub metadata: Option<String>,
    agent: Option<RefCell<agent::AgentView>>,
    metadata_offset: usize,
    metadata_focus: u8,
    metadata_rows: Cell<usize>,
    metadata_total: Cell<usize>,
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
    selection_explicit: bool,
    entries: Vec<Entry>,
    entry_sections: Vec<Option<usize>>,
    matches: Vec<usize>,
    matcher: Matcher,
    picker: Option<EntryKind>,
    saved_search: Vec<(Option<EntryKind>, TextInput, Option<String>)>,
    form: Option<Form>,
    choice_input: TextInput,
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
    metadata_lines: Vec<String>,
    metadata_y: usize,
    agent: Option<agent::AgentLayout>,
}

enum ResultRow {
    Heading(String),
    Status(usize, String),
    Entry(usize, usize),
}

impl CommandPalette {
    pub fn set_agent_history(
        &mut self,
        header: yinz_bus::command_palette::HistoryHeader,
        events: Vec<yinz_bus::command_palette::HistoryEvent>,
        actions: Vec<Entry>,
        status: String,
    ) {
        self.metadata = None;
        let agent = self
            .agent
            .get_or_insert_with(|| RefCell::new(agent::AgentView::new(header.clone())));
        agent.get_mut().newest =
            self.input.get_text().is_empty() && !actions.iter().any(|a| a.id.ends_with(":newer"));
        agent.get_mut().update(header, events, actions, status);
    }
    pub fn open_agent_reader(&mut self, header: String, markdown: String, tool: String) {
        let agent = self.agent.get_or_insert_with(|| {
            RefCell::new(agent::AgentView::new(
                yinz_bus::command_palette::HistoryHeader {
                    name: String::new(),
                    status: String::new(),
                    location: String::new(),
                    started: String::new(),
                    details: String::new(),
                },
            ))
        });
        agent.get_mut().reader(header, markdown, tool);
    }
    pub fn agent_following(&self) -> bool {
        self.agent.as_ref().is_some_and(|a| a.borrow().following)
    }
    pub fn agent_first_revision(&self) -> Option<u64> {
        self.agent
            .as_ref()
            .and_then(|a| a.borrow().first_revision())
    }
    pub fn agent_page_request(&mut self, following: bool) {
        if let Some(agent) = &mut self.agent {
            agent.get_mut().page_request(following);
        }
    }
    pub fn select_agent_revision(&mut self, revision: u64) {
        if let Some(agent) = &mut self.agent {
            agent.get_mut().select_revision(revision);
        }
    }
    pub fn scroll_agent(&mut self, down: bool) -> bool {
        if self.busy {
            return self.agent.is_some();
        }
        if let Some(agent) = &mut self.agent {
            agent.get_mut().scroll(down, 3);
            return true;
        }
        false
    }

    pub fn action_label(&self, id: &str) -> String {
        self.form
            .as_ref()
            .filter(|form| form.id == id)
            .map(|form| form.title.clone())
            .or_else(|| {
                self.entries
                    .iter()
                    .find(|entry| entry.id == id)
                    .map(|entry| entry.label.clone())
            })
            .unwrap_or_else(|| "Palette action".into())
    }

    pub fn update_form_values(&mut self, values: &[String]) {
        if let Some(form) = &mut self.form {
            for (field, value) in form.fields.iter_mut().zip(values) {
                if let FieldValue::Text(input) = &mut field.value {
                    *input = TextInput::new(value.clone());
                }
            }
        }
    }

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
        let selected = self
            .matches
            .get(self.selected)
            .filter(|_| self.selection_explicit)
            .map(|i| self.entries[*i].id.clone());
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
        let selected =
            selected.and_then(|id| self.matches.iter().position(|i| self.entries[*i].id == id));
        self.selection_explicit = selected.is_some();
        self.selected = selected.unwrap_or(0);
        self.normalize_region_selection();
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

    pub fn select_id(&mut self, id: &str) {
        if let Some(index) = self.matches.iter().position(|i| self.entries[*i].id == id) {
            self.selected = index;
            self.selected_status = None;
            self.selection_explicit = true;
        }
    }

    pub fn set_query(&mut self, query: String) {
        self.input = TextInput::new(query);
        self.rematch_query();
    }

    pub fn theme_picker(&self) -> bool {
        self.picker == Some(EntryKind::Theme)
    }

    pub fn agent_picker(&self) -> bool {
        self.picker == Some(EntryKind::Agent)
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        if self.selected_status.is_some() {
            return None;
        }
        self.matches
            .get(self.selected)
            .map(|i| &self.entries[*i])
            .filter(|entry| self.in_focused_region(entry))
    }

    fn in_focused_region(&self, entry: &Entry) -> bool {
        self.metadata.is_none()
            || (self.metadata_focus != 0
                && entry.id.contains("event:") == (self.metadata_focus == 1))
    }

    fn normalize_region_selection(&mut self) {
        if self.metadata.is_none() || self.metadata_focus == 0 {
            return;
        }
        if self.selected_entry().is_none() {
            self.selected = self
                .matches
                .iter()
                .position(|i| self.in_focused_region(&self.entries[*i]))
                .unwrap_or(self.matches.len());
        }
        self.selected_status = None;
    }

    fn scored_section(&self, section: usize) -> bool {
        self.is_root()
            && !self.input.get_text().trim().is_empty()
            && !self.sections[section].semantic
    }

    fn rematch(&mut self) {
        let agent_query = yinz_bus::command_palette::query::AgentQuery::parse(
            self.input.get_text(),
            yinz_bus::command_palette::query::LIVE_FIELDS,
        );
        let pattern = Pattern::new(
            self.input.get_text(),
            CaseMatching::Ignore,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );
        let agent_pattern = Pattern::new(
            agent_query.as_ref().map(|q| q.text.as_str()).unwrap_or(""),
            CaseMatching::Ignore,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );
        if (self.is_root() || self.agent_picker()) && self.form.is_none() {
            if let Err(error) = &agent_query {
                self.error = Some(error.clone());
            }
        }
        let mut buffer = Vec::new();
        let mut matches: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                self.picker.map_or(
                    !matches!(
                        entry.kind,
                        EntryKind::Theme
                            | EntryKind::Project
                            | EntryKind::Mother
                            | EntryKind::WarpTabSession
                            | EntryKind::WarpPaneSession
                            | EntryKind::WarpTab
                            | EntryKind::WarpPane
                    ),
                    |kind| {
                        entry.kind == kind
                            || (kind == EntryKind::Agent && entry.id == "agent-filters")
                            || (kind == EntryKind::Session
                                && entry.kind == EntryKind::ExitedSession)
                    },
                )
            })
            .filter_map(|(index, entry)| {
                if self.picker == Some(EntryKind::Agent) && entry.id == "agent-filters" {
                    return Some((index, u32::MAX));
                }
                if self.entry_sections[index].is_some_and(|i| self.sections[i].semantic) {
                    return Some((index, 0));
                }
                let pattern = if entry.kind == EntryKind::Agent {
                    let query = agent_query.as_ref().ok()?;
                    if !self
                        .agent_metadata
                        .get(&entry.id)
                        .is_some_and(|m| query.matches(m))
                    {
                        return None;
                    }
                    &agent_pattern
                } else {
                    &pattern
                };
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
                EntryKind::Plugin if section.is_some_and(|i| self.scored_section(i)) => 0,
                EntryKind::Plugin => 3 + section.unwrap_or(0),
                EntryKind::Session => 1,
                EntryKind::ExitedSession => 2,
                _ => 0,
            };
            (
                group,
                std::cmp::Reverse(if self.theme_picker() { 0 } else { *score }),
            )
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
        let selected = self
            .selected_entry()
            .filter(|_| self.selection_explicit)
            .map(|entry| entry.id.clone());
        self.saved_search
            .push((self.picker, std::mem::take(&mut self.input), selected));
        self.picker = Some(kind);
        self.selection_explicit = false;
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
        if self
            .agent
            .as_mut()
            .is_some_and(|agent| agent.get_mut().back())
        {
            return Outcome::None;
        }
        if let Some(mut previous) = self.history.pop() {
            previous.history = std::mem::take(&mut self.history);
            *self = previous;
            return Outcome::None;
        }
        if self.form.take().is_some() {
            self.metadata = None;
            return Outcome::None;
        }
        if let Some((picker, input, selected)) = self.saved_search.pop() {
            self.input = input;
            self.picker = picker;
            self.rematch();
            let selected =
                selected.and_then(|id| self.matches.iter().position(|i| self.entries[*i].id == id));
            self.selection_explicit = selected.is_some();
            self.selected = selected.unwrap_or(0);
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
        if let Some(agent) = &mut self.agent {
            if let Some(outcome) = agent.get_mut().key(&key, self.view) {
                return outcome;
            }
        }
        if self.form.is_none() && self.metadata.is_some() && key.bare_key == BareKey::Tab {
            self.metadata_focus = (self.metadata_focus
                + if key.has_modifiers(&[KeyModifier::Shift]) {
                    2
                } else {
                    1
                })
                % 3;
            self.normalize_region_selection();
            return Outcome::None;
        }
        if self.form.is_none() && self.metadata.is_some() && self.metadata_focus == 0 {
            let page = self.metadata_rows.get().max(1);
            self.metadata_offset = self
                .metadata_offset
                .min(self.metadata_total.get().saturating_sub(page));
            match key.bare_key {
                BareKey::Up => self.metadata_offset = self.metadata_offset.saturating_sub(1),
                BareKey::Down => self.metadata_offset = self.metadata_offset.saturating_add(1),
                BareKey::PageUp => self.metadata_offset = self.metadata_offset.saturating_sub(page),
                BareKey::PageDown => {
                    self.metadata_offset = self.metadata_offset.saturating_add(page)
                },
                BareKey::Home => self.metadata_offset = 0,
                BareKey::End => self.metadata_offset = usize::MAX,
                _ => {},
            }
            return Outcome::None;
        }
        if self.form.is_none()
            && self.metadata.is_some()
            && matches!(
                key.bare_key,
                BareKey::Up
                    | BareKey::Down
                    | BareKey::PageUp
                    | BareKey::PageDown
                    | BareKey::Home
                    | BareKey::End
            )
        {
            self.normalize_region_selection();
            if matches!(key.bare_key, BareKey::PageUp | BareKey::PageDown) {
                self.page_selection(key.bare_key == BareKey::PageDown);
                return Outcome::None;
            }
            let region: Vec<_> = self
                .matches
                .iter()
                .enumerate()
                .filter(|(_, i)| self.in_focused_region(&self.entries[**i]))
                .map(|(position, _)| position)
                .collect();
            if !region.is_empty() {
                let current = region.iter().position(|i| *i == self.selected).unwrap_or(0);
                let next = match key.bare_key {
                    BareKey::Up => current.saturating_sub(1),
                    BareKey::Down => current.saturating_add(1),
                    BareKey::Home => 0,
                    BareKey::End => region.len() - 1,
                    _ => current,
                };
                self.selection_explicit = true;
                self.selected = region[next.min(region.len() - 1)];
                self.selected_status = None;
            }
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
            if form.confirmation && matches!(key.bare_key, BareKey::PageUp | BareKey::PageDown) {
                let page = self.metadata_rows.get().max(1);
                self.metadata_offset = self
                    .metadata_offset
                    .min(self.metadata_total.get().saturating_sub(page));
                self.metadata_offset = if key.bare_key == BareKey::PageDown {
                    self.metadata_offset.saturating_add(page)
                } else {
                    self.metadata_offset.saturating_sub(page)
                };
                return Outcome::None;
            }
            if let Some(Field {
                value: FieldValue::Choice { options, selected },
                ..
            }) = form.fields.get_mut(form.selected)
            {
                if matches!(key.bare_key, BareKey::Char(_) | BareKey::Backspace) {
                    self.choice_input.handle_key(key);
                    let pattern = Pattern::new(
                        self.choice_input.get_text(),
                        CaseMatching::Ignore,
                        Normalization::Smart,
                        AtomKind::Fuzzy,
                    );
                    let mut buffer = Vec::new();
                    if let Some((index, _)) = options
                        .iter()
                        .enumerate()
                        .filter_map(|(i, value)| {
                            pattern
                                .score(Utf32Str::new(value, &mut buffer), &mut self.matcher)
                                .map(|score| (i, score))
                        })
                        .max_by_key(|(_, score)| *score)
                    {
                        *selected = index;
                    }
                    return Outcome::None;
                }
            }
            if matches!(
                key.bare_key,
                BareKey::Up | BareKey::Down | BareKey::Tab | BareKey::Enter
            ) {
                self.choice_input = TextInput::default();
            }
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
                            id: form
                                .actions
                                .get(form.selected.saturating_sub(
                                    form.fields.len() + 1 + usize::from(form.confirmation),
                                ))
                                .filter(|_| {
                                    form.selected
                                        > form.fields.len() + usize::from(form.confirmation)
                                })
                                .map(|a| a.0.clone())
                                .unwrap_or_else(|| form.id.clone()),
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
            BareKey::Enter if key.has_only_modifiers(&[KeyModifier::Ctrl]) => {
                if let Some(entry) = self
                    .selected_entry()
                    .filter(|entry| entry.kind == EntryKind::Agent)
                {
                    return Outcome::FocusAgent(entry.id.clone());
                }
            },
            BareKey::PageUp | BareKey::PageDown => {
                self.page_selection(key.bare_key == BareKey::PageDown);
            },
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
        if self.busy
            || self.text.is_some()
            || self.agent.as_ref().is_some_and(|a| a.borrow().is_reader())
        {
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
        self.selection_explicit = false;
        self.selected = 0;
        self.error = None;
        self.rematch();
        self.normalize_region_selection();
    }

    fn selection(&self) -> usize {
        self.form
            .as_ref()
            .map_or(self.selected, |form| form.selected)
    }

    fn row_selected(&self, row: &ResultRow) -> bool {
        match row {
            ResultRow::Entry(index, _) => {
                self.selected_entry().is_some() && *index == self.selected
            },
            ResultRow::Status(section, _) => self.selected_status == Some(*section),
            ResultRow::Heading(_) => false,
        }
    }

    fn select_row(&mut self, row: &ResultRow) {
        match row {
            ResultRow::Entry(index, _) => {
                self.selection_explicit = true;
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
        let mut previous = (None, None);
        for (index, entry_index) in self.matches.iter().enumerate() {
            let entry = &self.entries[*entry_index];
            let section = self.entry_sections[*entry_index];
            if entry.kind == EntryKind::Plugin && !section.is_some_and(|i| self.scored_section(i)) {
                continue;
            }
            let label = if let Some(section) = section {
                Some(self.sections[section].title.as_str())
            } else if self.picker == Some(EntryKind::Session) {
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
            if (section, label) != previous && capacity >= 2 {
                if let Some(label) = label {
                    rows.push(ResultRow::Heading(label.into()));
                }
            }
            rows.push(ResultRow::Entry(index, 0));
            previous = (section, label);
        }
        if self.picker.is_none() {
            for (section_index, section) in self.sections.iter().enumerate() {
                let entries: Vec<_> = self
                    .matches
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| {
                        !self.scored_section(section_index)
                            && self.entry_sections[**entry] == Some(section_index)
                    })
                    .map(|(index, _)| ResultRow::Entry(index, 0))
                    .collect();
                if entries.is_empty() && section.status.is_none() {
                    continue;
                }
                if capacity >= 2 {
                    rows.push(ResultRow::Heading(section.title.clone()));
                }
                rows.extend(entries);
                if let Some(status) = &section.status {
                    rows.push(ResultRow::Status(section_index, status.clone()));
                }
            }
        }
        rows
    }

    fn entry_lines(&self, index: usize, width: usize, max_lines: usize) -> Vec<String> {
        use unicode_width::UnicodeWidthStr;
        let entry = &self.entries[self.matches[index]];
        let shortcut = take_columns(&entry.shortcut, width.saturating_sub(3));
        let width = width.saturating_sub(shortcut.width() + usize::from(!shortcut.is_empty()));
        let width = width - width.saturating_sub(1).min(2);
        if width == 0 {
            return vec![String::new()];
        }
        let mut lines = crate::ui::notification::wrapped_text_lines(&entry.label, width);
        for line in lines.iter_mut().skip(1) {
            line.insert_str(0, &" ".repeat(2.min(width.saturating_sub(1))));
        }
        let truncated = lines.len() > max_lines || lines.iter().any(|line| line.width() > width);
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            if truncated {
                *last = format!("{}…", take_columns(last, width.saturating_sub(1)));
            } else if !entry.description.is_empty() {
                last.push_str(&format!(" — {}", entry.description));
            }
        }
        lines
    }

    fn entry_heading(&self, index: usize) -> Option<String> {
        let entry = &self.entries[self.matches[index]];
        if entry.kind == EntryKind::Plugin {
            self.entry_sections[self.matches[index]].map(|i| self.sections[i].title.clone())
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
    }

    fn wrapped_result_rows(&self, capacity: usize, width: usize) -> Vec<ResultRow> {
        let rows = self.result_rows(capacity);
        rows.into_iter()
            .flat_map(|row| {
                if let ResultRow::Entry(index, _) = row {
                    let heading = usize::from(self.entry_heading(index).is_some());
                    let max_lines = capacity.saturating_sub(heading).clamp(1, 2);
                    (0..self.entry_lines(index, width, max_lines).len())
                        .map(|line| ResultRow::Entry(index, line))
                        .collect()
                } else {
                    vec![row]
                }
            })
            .collect()
    }

    fn page_selection(&mut self, down: bool) {
        let capacity = self.page_rows.get().max(1);
        let mut rows = self.wrapped_result_rows(capacity, self.result_width.get());
        if self.metadata.is_some() {
            rows.retain(|row| {
                matches!(row, ResultRow::Entry(index, _)
                if self.in_focused_region(&self.entries[self.matches[*index]]))
            });
        }
        if rows.is_empty() {
            return;
        }
        let selected = rows
            .iter()
            .position(|row| self.row_selected(row))
            .unwrap_or(0);
        let heading = match &rows[selected] {
            ResultRow::Entry(index, _) => self.entry_heading(*index).is_some(),
            ResultRow::Status(_, _) => true,
            ResultRow::Heading(_) => false,
        };
        let distance = capacity.saturating_sub(usize::from(heading)).max(1);
        let target = if down {
            selected
                .saturating_add(distance)
                .min(rows.len().saturating_sub(1))
        } else {
            selected.saturating_sub(distance)
        };
        let row = if down {
            rows[target..]
                .iter()
                .find(|row| !matches!(row, ResultRow::Heading(_)))
        } else {
            rows[..=target]
                .iter()
                .rev()
                .find(|row| !matches!(row, ResultRow::Heading(_)))
                .or_else(|| {
                    rows.iter()
                        .find(|row| !matches!(row, ResultRow::Heading(_)))
                })
        };
        if let Some(row) = row {
            self.select_row(row);
        }
    }

    fn visible_result_rows(&self, capacity: usize, width: usize) -> Vec<ResultRow> {
        let rows = self.wrapped_result_rows(capacity, width);
        if rows.is_empty() {
            return rows;
        }
        let selected = rows
            .iter()
            .rposition(|row| self.row_selected(row))
            .unwrap_or(0);
        let mut start = (selected + 1).saturating_sub(capacity);
        let sticky_heading = |start: usize| {
            if let Some(ResultRow::Status(section, _)) = rows.get(start) {
                return Some(self.sections[*section].title.clone());
            }
            let Some(ResultRow::Entry(index, _)) = rows.get(start) else {
                return None;
            };
            self.entry_heading(*index)
        };
        // Keep whole entries below their repeated heading, including the selected title.
        loop {
            let heading = usize::from(capacity >= 2 && sticky_heading(start).is_some());
            if selected >= start + capacity - heading
                || matches!(rows.get(start), Some(ResultRow::Entry(_, line)) if *line > 0)
            {
                start += 1;
            } else {
                break;
            }
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
        let expanded = self.metadata.is_some() || self.text.is_some() || self.agent.is_some();
        let cols = surface.cols.min(if expanded { 110 } else { 64 });
        let rows = surface.rows.min(if expanded { 36 } else { 11 });
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
        let metadata_y = input_y + 1 + usize::from(rows >= 6);
        let mut metadata_lines = self
            .metadata
            .as_ref()
            .map(|text| {
                crate::ui::notification::wrapped_text_lines(text, cols.saturating_sub(2 * border))
            })
            .unwrap_or_default();
        let metadata_height = if self.metadata.is_some() {
            rows.saturating_sub(7).min(rows / 3)
        } else {
            0
        };
        self.metadata_total.set(metadata_lines.len());
        let offset = self
            .metadata_offset
            .min(metadata_lines.len().saturating_sub(metadata_height));
        self.metadata_rows.set(metadata_height);
        metadata_lines = metadata_lines
            .into_iter()
            .skip(offset)
            .take(metadata_height)
            .collect();
        let options_y = metadata_y + metadata_height;
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
        self.result_width.set(cols.saturating_sub(2 * border));
        let offset = self
            .text_offset
            .min(text_lines.len().saturating_sub(options_rows));
        CommandPaletteLayout {
            agent: self.agent.as_ref().map(|a| {
                a.borrow_mut().layout(
                    rect,
                    border,
                    &self.input,
                    self.error.as_deref(),
                    self.busy || self.pending,
                )
            }),
            rect,
            metadata_lines,
            metadata_y,
            border,
            input_y,
            options_y,
            options_rows,
            first_option: (self.selection() + 1).saturating_sub(options_rows),
            footer_y,
            picker_heading_y: (self.picker.is_some() && self.form.is_none() && rows >= 6)
                .then_some(input_y + 1),
            result_rows: self.visible_result_rows(options_rows, self.result_width.get()),
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
        if let (Some(agent), Some(agent_layout)) = (&mut self.agent, &layout.agent) {
            agent.get_mut().click(
                agent_layout,
                y.saturating_sub(layout.rect.y + layout.border),
            );
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
                if self.metadata.is_some() {
                    if let ResultRow::Entry(index, _) = row {
                        self.metadata_focus =
                            if self.entries[self.matches[*index]].id.contains("event:") {
                                1
                            } else {
                                2
                            };
                    }
                }
            }
        }
    }

    pub fn cursor(&self, layout: &CommandPaletteLayout) -> Option<(usize, usize)> {
        if self.agent.is_some() {
            return None;
        }
        if layout.rect.is_empty()
            || self.text.is_some()
            || self.busy
            || (self.metadata.is_some() && self.metadata_focus == 0)
        {
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
        if let Some(agent) = &layout.agent {
            return agent::chunks(layout, agent, style);
        }
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
            .background(Some(style.colors.text_unselected.background.into()))
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
                let mut label_span = 0..usize::MAX;
                let (text, styles) = if layout.border == 1 && y == rect.y {
                    (
                        format!(
                            "╭{}╮",
                            fit(
                                &if self.form.as_ref().is_some_and(|f| f.confirmation) {
                                    " Confirm · PgUp/PgDn scroll details ".into()
                                } else if self.metadata.is_some() {
                                    format!(" Agent info · {} ", ["Metadata", "Timeline", "Actions"][self.metadata_focus as usize])
                                } else {
                                    format!(" Do · Project: {} ", self.current_project.as_deref().unwrap_or("None"))
                                },
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
                                } else if self.form.as_ref().is_some_and(|f| f.confirmation) {
                                    "↑↓ Select  Enter Choose  PgUp/PgDn Details  Esc Back"
                                } else if self.metadata.is_some() {
                                    "Tab Metadata/Timeline/Actions · ↑↓ PgUp/PgDn · Enter Open · Esc Back"
                                } else if self.text.is_some() {
                                    "↑↓ Scroll  PgUp/PgDn Page  Home/End  Esc Back"
                                } else if self.form.is_some() {
                                    "Tab/↑↓/Ctrl+n/p Fields  Enter Submit  Esc Back"
                                } else if self.selected_entry().is_some_and(|entry| entry.kind == EntryKind::Agent) {
                                    "↑↓ Select · Enter Info · Ctrl+Enter Focus · Esc Back"
                                } else if self.theme_picker() {
                                    "↑↓ Preview · Enter Save · Esc Back"
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
                    } else if (layout.metadata_y..layout.options_y).contains(&y) {
                        (layout.metadata_lines.get(y - layout.metadata_y).cloned().unwrap_or_default(), if self.metadata_focus == 0 { fill.clone() } else { heading.clone() })
                    } else if (layout.options_y..layout.options_y + layout.options_rows)
                        .contains(&y)
                    {
                        let row = layout.result_rows.get(y - layout.options_y);
                        let index = if self.form.is_some() {
                            layout.first_option + y - layout.options_y
                        } else if let Some(ResultRow::Entry(index, _)) = row {
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
                                        format!("{}: ‹ {} ›{}", field.label, field.text_value(), if is_selected && !self.choice_input.get_text().is_empty() { format!(" (search: {})", self.choice_input.get_text()) } else { String::new() })
                                    },
                                }
                            } else if form.confirmation && index == form.fields.len() {
                                "Cancel".into()
                            } else if index == form.fields.len() + usize::from(form.confirmation) {
                                form.submit.clone()
                            } else {
                                form.actions.get(index - form.fields.len() - 1 - usize::from(form.confirmation)).map(|a| a.1.clone()).unwrap_or_default()
                            }
                        } else if let Some(
                            ResultRow::Heading(label) | ResultRow::Status(_, label),
                        ) = row
                        {
                            label.clone()
                        } else if let Some(ResultRow::Entry(index, line)) = row {
                            use unicode_width::UnicodeWidthStr;
                            let entry = &self.entries[self.matches[*index]];
                            let shortcut = take_columns(&entry.shortcut, width.saturating_sub(3));
                            let label_width = width.saturating_sub(shortcut.width() + usize::from(!shortcut.is_empty()));
                            let remaining = layout.result_rows.len() - (y - layout.options_y);
                            let max_lines = (line + remaining).min(2);
                            let lines = self.entry_lines(*index, width, max_lines);
                            let prefix = if is_selected && *line == 0 { "› " } else { "  " };
                            let prefix = take_columns(prefix, label_width.saturating_sub(1).min(2));
                            let label = format!("{prefix}{}",
                                lines.get(*line).map(String::as_str).unwrap_or(""));
                            let label = take_columns(&label, label_width);
                            let indent = if *line == 0 { prefix.chars().count() } else {
                                label.chars().take_while(|c| c.is_whitespace()).count()
                            };
                            label_span = indent..label.chars().count();
                            if *line == 0 && !shortcut.is_empty() {
                                let label = fit(&label, label_width, ' ');
                                shortcut_start = label.chars().count() + 1;
                                format!("{label} {shortcut}")
                            } else {
                                label
                            }
                        } else if self.matches.is_empty() && y == layout.options_y {
                            if self.picker == Some(EntryKind::Project)
                                && !self.entries.iter().any(|e| e.kind == EntryKind::Project)
                            {
                                "No projects".into()
                            } else if self.picker == Some(EntryKind::Mother)
                                && !self.entries.iter().any(|e| e.kind == EntryKind::Mother)
                            {
                                "No agents available".into()
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
                    let text_len = text.chars().count();
                    label_span.end = label_span.end.min(text_len);
                    let text = fit(
                        &text
                            .chars()
                            .map(|c| if c.is_control() { ' ' } else { c })
                            .collect::<String>(),
                        width,
                        ' ',
                    );
                    // Semantic spaces cover backing glyphs; alignment and fill stay translucent.
                    let text_styles: RcCharacterStyles = styles
                        .background(Some(match styles.background {
                            None | Some(AnsiCode::Reset) => style.colors.text_unselected.background.into(),
                            Some(background) => background,
                        }))
                        .into();
                    let subtle: RcCharacterStyles = text_styles.dim(Some(AnsiCode::On)).into();
                    let mut characters = Vec::new();
                    if layout.border == 1 {
                        characters.push(TerminalCharacter::new_styled('│', border.clone()));
                    }
                    characters.extend(text.chars().enumerate().map(|(index, c)| {
                        TerminalCharacter::new_styled(
                            c,
                            if (shortcut_start..text_len).contains(&index) {
                                subtle.clone()
                            } else if label_span.contains(&index) {
                                text_styles.clone()
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
