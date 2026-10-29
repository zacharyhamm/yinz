//! Native command palette. The screen owns its lifetime and input capture.

use std::cell::{Cell, RefCell};
pub use yinz_tile_markdown::escape_literal;
use yinz_tile_markdown::{self as md, MdLine, MdStyle, Options};

#[path = "command_palette_agent.rs"]
mod agent;
#[path = "command_palette_live_agent.rs"]
mod live_agent;
#[path = "command_palette_quota.rs"]
mod quota;
pub use live_agent::AgentPresentation;
pub use quota::QuotaImage;

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
    TabGroup,
}

impl EntryKind {
    fn parent_label(self) -> Option<&'static str> {
        match self {
            Self::WarpTabSession => Some("Warp tab · Source session"),
            Self::WarpPaneSession => Some("Warp pane · Source session"),
            Self::WarpTab => Some("Warp tab"),
            Self::WarpPane => Some("Warp pane"),
            Self::TabGroup => Some("Switch tab group"),
            Self::Theme => Some("Change theme"),
            Self::Project => Some("Switch current project"),
            Self::Command | Self::Plugin => None,
            Self::Agent => Some("Agents"),
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
    pub input_options: Option<yinz_types::command_palette::InputOptions>,
    pub capturing: bool,
    pub label: String,
    pub value: FieldValue,
}

impl Field {
    pub fn text(label: &str, value: &str) -> Self {
        Self {
            input_options: None,
            capturing: false,
            label: label.into(),
            value: FieldValue::Text(TextInput::new(value.into())),
        }
    }

    pub fn choice(label: &str, options: &[&str]) -> Self {
        Self {
            input_options: None,
            capturing: false,
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
    pub agent_presentations: std::collections::BTreeMap<String, AgentPresentation>,
    pub metadata: Option<String>,
    pub description: String,
    pub columns: Vec<yinz_types::command_palette::Column>,
    pub table_cells: std::collections::BTreeMap<String, Vec<String>>,
    description_offset: Cell<usize>,
    description_rows: Cell<usize>,
    description_total: Cell<usize>,
    agent: Option<RefCell<agent::AgentView>>,
    metadata_offset: usize,
    metadata_focus: u8,
    metadata_rows: Cell<usize>,
    metadata_total: Cell<usize>,
    history: Vec<CommandPalette>,
    native_entries: Vec<Entry>,
    sections: Vec<PluginSection>,
    worktree_creation: Option<(String, String, Vec<String>)>,
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
    browsing: bool,
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

/// An optional quota raster, in absolute terminal cells. Text remains the fallback.
#[derive(Clone, Debug)]
pub struct QuotaPlacement {
    pub rect: CellRect,
    pub display: agent_proto::quota::Display,
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
    result_overflow: Option<(usize, usize)>,
    text_lines: Vec<MdLine>,
    metadata_lines: Vec<MdLine>,
    description_lines: Vec<MdLine>,
    description_y: usize,
    metadata_y: usize,
    agent: Option<agent::AgentLayout>,
}

impl CommandPaletteLayout {
    pub fn quota_image(&self) -> Option<&QuotaPlacement> {
        self.agent.as_ref()?.quota.as_ref()
    }
    /// Hide the ordinary percentage only after the managed image was prepared successfully.
    pub fn use_quota_image(&mut self) {
        if let Some(agent) = &mut self.agent
            && let Some(header) = agent.graphical_header.take()
        {
            for (line, graphical) in agent.lines.iter_mut().zip(header) {
                *line = graphical;
            }
        }
    }
}

enum ResultRow {
    Heading(String),
    Divider,
    Status(usize, String),
    Entry(usize, usize),
}

impl ResultRow {
    fn selectable(&self) -> bool {
        matches!(self, Self::Entry(_, _) | Self::Status(_, _))
    }
}

impl CommandPalette {
    /// Transient UI state, separate from the history reply and pane manifest.
    pub fn set_agent_header_flash(&mut self, live_id: Option<String>, flash: bool) {
        if let Some(agent) = &mut self.agent {
            agent.get_mut().live_id = live_id;
            agent.get_mut().hook_flash = flash;
        }
    }

    pub fn has_visible_flashing_agent(
        &self,
        surface: Viewport,
        arrow_fonts: bool,
        flashing_ids: &std::collections::HashSet<String>,
    ) -> bool {
        let layout = self.layout_with_arrow_fonts(surface, arrow_fonts);
        if let Some(agent_layout) = &layout.agent {
            return self.agent.as_ref().is_some_and(|agent| {
                let agent = agent.borrow();
                agent.shows_header()
                    && agent
                        .live_id
                        .as_ref()
                        .is_some_and(|id| flashing_ids.contains(id))
                    && agent_layout
                        .lines
                        .first()
                        .is_some_and(|line| line.text.starts_with('●'))
            });
        }
        if self.form.is_some() || self.text.is_some() {
            return false;
        }
        layout.result_rows.iter().any(|row| {
            let ResultRow::Entry(index, line) = row else {
                return false;
            };
            let entry = &self.entries[self.matches[*index]];
            flashing_ids.contains(&entry.id)
                && self.agent_presentation(*index).is_some_and(|agent| {
                    agent
                        .lines(
                            self.result_width.get(),
                            self.entry_line_limit(*index, layout.options_rows),
                        )
                        .get(*line)
                        .is_some_and(|line| line.text.starts_with('●'))
                })
        })
    }

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
        agent
            .get_mut()
            .search_mode(!self.input.get_text().is_empty());
        agent.get_mut().newest =
            self.input.get_text().is_empty() && !actions.iter().any(|a| a.id.ends_with(":newer"));
        agent.get_mut().update(header, events, actions, status);
    }
    pub fn refresh_agent_title(
        &mut self,
        pane: yinz_types::data::PaneInfo,
        project: String,
        branch: String,
    ) {
        if let Some(agent) = &mut self.agent {
            let header = &mut agent.get_mut().header;
            header.task.clone_from(&pane.title);
            if let Some(presentation) = AgentPresentation::from_pane(&pane) {
                header.status = format!(
                    "[{}] · {} · {}",
                    presentation.pane_id,
                    presentation.provider,
                    match presentation.state {
                        yinz_types::agent::AgentState::Busy => "working",
                        yinz_types::agent::AgentState::Waiting => "needs input",
                        yinz_types::agent::AgentState::Idle => "idle",
                        yinz_types::agent::AgentState::Exited => "exited",
                    }
                );
            }
            if let Some(session) = pane.agent.as_ref().and_then(|a| a.session_id.as_ref()) {
                header.session_id = format!("session {session}");
            }
            header.live = Some(pane);
            header.project = project;
            header.branch = branch;
        }
    }
    pub fn set_agent_usage(&mut self, usage: yinz_types::agent::control::ProviderUsage) {
        if let Some(agent) = &mut self.agent {
            agent.get_mut().header.usage = Some(usage);
        }
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
                    ..Default::default()
                },
            ))
        });
        agent.get_mut().reader(header, markdown, tool);
    }
    pub fn agent_following(&self) -> bool {
        self.agent.as_ref().is_some_and(|a| a.borrow().following)
    }
    pub fn agent_prompt_sent(&mut self) -> bool {
        if let Some(agent) = &mut self.agent {
            agent.get_mut().prompt_sent();
            self.error = None;
            true
        } else {
            false
        }
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
    pub fn agent_retarget_run(&mut self) {
        if let Some(agent) = &mut self.agent {
            agent.get_mut().retarget_run();
        }
    }
    pub fn select_agent_revision(&mut self, revision: u64) {
        if let Some(agent) = &mut self.agent {
            agent.get_mut().select_revision(revision);
        }
    }
    pub fn scroll(&mut self, down: bool) {
        if self.busy {
            return;
        }
        if let Some(agent) = &mut self.agent {
            agent.get_mut().scroll(down, 3);
            return;
        }
        // Wheel input must not edit form fields or capture a key binding.
        if self.form.is_some() {
            return;
        }
        let key = KeyWithModifier::new(if down { BareKey::Down } else { BareKey::Up });
        for _ in 0..3 {
            self.handle_key(key.clone());
        }
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
            .map(|label| plain_text(&label))
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

    fn is_compact(&self) -> bool {
        self.is_root()
            && self.query().is_empty()
            && !self.browsing
            && self.metadata.is_none()
            && self.description.is_empty()
            && self.text.is_none()
            && self.agent.is_none()
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

    pub fn clear_searches(&mut self) {
        self.set_query(String::new());
        for (_, input, selected) in &mut self.saved_search {
            *input = TextInput::default();
            *selected = None;
        }
        for previous in &mut self.history {
            previous.clear_searches();
        }
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

    fn ranked_root_search(&self) -> bool {
        self.is_root() && !self.input.get_text().trim().is_empty()
    }

    /// Configure the worktree view's creation row and navigation rows hidden during search.
    pub fn set_worktree_creation(&mut self, id: String, navigation: Vec<String>) {
        if let Some(entry) = self.entries.iter().find(|entry| entry.id == id) {
            self.worktree_creation = Some((id, entry.label.clone(), navigation));
            self.rematch();
        }
    }

    pub fn worktree_creation_values(&self, id: &str) -> Vec<String> {
        if self
            .worktree_creation
            .as_ref()
            .is_some_and(|(create, _, _)| create == id)
            && !self.query().trim().is_empty()
        {
            vec![self.query().to_owned()]
        } else {
            vec![]
        }
    }

    fn rematch(&mut self) {
        let worktree_search = self.worktree_creation.is_some() && !self.query().trim().is_empty();
        if let Some((id, label, _)) = &self.worktree_creation {
            let label = if worktree_search {
                escape_literal(&format!("Create worktree ‘{}’", self.query()))
            } else {
                label.clone()
            };
            if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == *id) {
                entry.label = label;
            }
        }
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
        if (self.is_root() || self.agent_picker())
            && self.form.is_none()
            && let Err(error) = &agent_query
        {
            self.error = Some(error.clone());
        }
        let ranked_root_search = self.ranked_root_search();
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
                            | EntryKind::TabGroup
                            | EntryKind::Project
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
                if let Some((create, _, navigation)) = &self.worktree_creation {
                    if worktree_search {
                        if entry.id == *create {
                            return Some((index, 0));
                        }
                        if navigation.contains(&entry.id) {
                            return None;
                        }
                    } else {
                        return Some((index, 0));
                    }
                }
                if self.picker == Some(EntryKind::Agent) && entry.id == "agent-filters" {
                    return Some((index, u32::MAX));
                }
                let section = self.entry_sections[index].map(|i| &self.sections[i]);
                let semantic = section.is_some_and(|s| s.semantic);
                if semantic && !ranked_root_search {
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
                std::iter::once(&inline(&entry.label).text)
                    .chain(entry.aliases.iter())
                    .chain(
                        self.agent_presentations
                            .get(&entry.id)
                            .filter(|_| entry.kind == EntryKind::Agent)
                            .into_iter()
                            .flat_map(|agent| [&agent.project, &agent.branch]),
                    )
                    .chain(section.filter(|_| ranked_root_search).map(|s| &s.title))
                    .filter_map(|text| {
                        pattern.score(Utf32Str::new(text, &mut buffer), &mut self.matcher)
                    })
                    .max()
                    .or_else(|| semantic.then_some(0))
                    .map(|score| (index, score))
            })
            .collect();
        // Search agents by relevance, with exact project/worktree matches in a
        // separate tier above every fuzzy score. Stable ties retain panel order.
        // Keep the agent slots so other kinds retain their ranking and placement.
        let mut ordered_agents: Vec<_> = matches
            .iter()
            .filter(|(index, _)| self.entries[*index].kind == EntryKind::Agent)
            .copied()
            .collect();
        if let Ok(query) = &agent_query
            && !query.text.trim().is_empty()
        {
            ordered_agents.sort_by_cached_key(|(index, score)| {
                std::cmp::Reverse((
                    self.agent_exact_location_match(&self.entries[*index], &query.text),
                    *score,
                ))
            });
        }
        matches.sort_by_key(|(index, score)| {
            let section = self.entry_sections[*index];
            let group = match self.entries[*index].kind {
                EntryKind::Plugin if ranked_root_search && *score > 0 => 0,
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
        let mut ordered_agents = ordered_agents.into_iter();
        self.matches = matches
            .into_iter()
            .map(|(index, _)| {
                if self.entries[index].kind == EntryKind::Agent {
                    ordered_agents.next().map_or(index, |(index, _)| index)
                } else {
                    index
                }
            })
            .collect();
        if worktree_search
            && let Some((create, _, _)) = &self.worktree_creation
            && let Some(position) = self
                .matches
                .iter()
                .position(|i| self.entries[*i].id == *create)
        {
            let index = self.matches.remove(position);
            self.matches.push(index);
        }
        self.selected = self.selected.min(self.matches.len().saturating_sub(1));
        self.selected_status = if self.matches.is_empty() && self.picker.is_none() {
            self.sections
                .iter()
                .position(|section| section.status.is_some())
        } else {
            None
        };
    }

    fn agent_exact_location_match(&self, entry: &Entry, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        let metadata = self.agent_metadata.get(&entry.id);
        let presentation = self.agent_presentations.get(&entry.id);
        metadata
            .into_iter()
            .flat_map(|metadata| [metadata.get("project"), metadata.get("directory")])
            .flatten()
            .map(String::as_str)
            .chain(
                presentation
                    .into_iter()
                    .flat_map(|agent| [agent.project.as_str(), agent.branch.as_str()]),
            )
            .any(|value| value.to_lowercase() == query)
            || metadata
                .and_then(|metadata| metadata.get("directory"))
                .and_then(|directory| directory.trim_end_matches('/').rsplit('/').next())
                .is_some_and(|name| name.to_lowercase() == query)
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
        let help = form
            .fields
            .iter()
            .filter_map(|f| {
                f.input_options
                    .as_ref()
                    .filter(|o| !o.help.is_empty() || !o.error.is_empty())
                    .map(|o| format!("{}: {} {}", f.label, o.help, o.error))
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        if !help.trim().is_empty() {
            self.metadata = Some(help);
        }
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
        if self.theme_picker() && self.form.is_some() {
            return Outcome::Cancel;
        }
        if self.busy || (self.pending && !self.is_root()) {
            return Outcome::Cancel;
        }
        if self
            .agent
            .as_mut()
            .is_some_and(|agent| agent.get_mut().back(&mut self.input))
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
        if !self.busy
            && let Some(field) = self
                .form
                .as_mut()
                .and_then(|f| f.fields.get_mut(f.selected))
        {
            if field.capturing {
                if let FieldValue::Text(input) = &mut field.value {
                    input.set_text(key.to_string());
                }
                field.capturing = false;
                return Outcome::None;
            }
            if field
                .input_options
                .as_ref()
                .is_some_and(|o| o.kind == "key")
                && key.bare_key == BareKey::F(2)
            {
                field.capturing = true;
                return Outcome::None;
            }
            if field
                .input_options
                .as_ref()
                .is_some_and(|o| o.kind == "multiline")
                && key.bare_key == BareKey::Enter
                && key.has_only_modifiers(&[KeyModifier::Shift])
            {
                if let FieldValue::Text(input) = &mut field.value {
                    insert_multiline(input, "\n");
                }
                return Outcome::None;
            }
        }
        if key.bare_key == BareKey::Esc {
            return self.back();
        }
        if self.busy {
            return Outcome::None;
        }
        if let Some(agent) = &mut self.agent {
            return agent.get_mut().key(&key, self.view, &mut self.input);
        }
        let key = match key.bare_key {
            BareKey::Char('n') if key.has_only_modifiers(&[KeyModifier::Ctrl]) => {
                KeyWithModifier::new(BareKey::Down)
            },
            BareKey::Char('p') if key.has_only_modifiers(&[KeyModifier::Ctrl]) => {
                KeyWithModifier::new(BareKey::Up)
            },
            _ => key,
        };
        if !self.description.is_empty()
            && key.has_only_modifiers(&[KeyModifier::Alt])
            && matches!(key.bare_key, BareKey::PageUp | BareKey::PageDown)
        {
            let page = self.description_rows.get().max(1);
            let end = self.description_total.get().saturating_sub(page);
            let offset = self.description_offset.get().min(end);
            self.description_offset
                .set(if key.bare_key == BareKey::PageDown {
                    offset.saturating_add(page).min(end)
                } else {
                    offset.saturating_sub(page)
                });
            return Outcome::None;
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
        // Leave Alt/Super character chords to palette shortcuts.
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
                && matches!(key.bare_key, BareKey::Char(_) | BareKey::Backspace)
            {
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
                        for (index, field) in form.fields.iter().enumerate() {
                            if let Some(options) = &field.input_options
                                && options.kind == "number"
                                && field.text_value().parse::<i64>().ok().is_none_or(|v| {
                                    options.minimum.is_some_and(|min| v < min)
                                        || options.maximum.is_some_and(|max| v > max)
                                })
                            {
                                self.error = Some(format!(
                                    "{}: enter a whole number within the displayed bounds",
                                    field.label
                                ));
                                form.selected = index;
                                return Outcome::None;
                            }
                        }
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
        if self.is_compact() {
            match key.bare_key {
                BareKey::Enter => return Outcome::None,
                BareKey::PageUp | BareKey::PageDown | BareKey::Up | BareKey::Down
                    if key.has_no_modifiers()
                        || matches!(key.bare_key, BareKey::PageUp | BareKey::PageDown) =>
                {
                    self.browsing = true;
                    if let Some(row) = self.result_rows(1).first() {
                        self.select_row(row);
                    }
                    return Outcome::None;
                },
                _ => {},
            }
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
                let rows: Vec<_> = self
                    .result_rows(1)
                    .into_iter()
                    .filter(ResultRow::selectable)
                    .collect();
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
        if let Some(agent) = &mut self.agent {
            agent.get_mut().insert_text(&mut self.input, text);
            return;
        }
        if let Some(field) = self
            .form
            .as_mut()
            .and_then(|f| f.fields.get_mut(f.selected))
            && field
                .input_options
                .as_ref()
                .is_some_and(|o| o.kind == "multiline" || o.kind == "text")
        {
            if let FieldValue::Text(input) = &mut field.value {
                insert_multiline(input, text);
            }
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
        if self.is_root() {
            self.browsing = false;
        }
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
            ResultRow::Heading(_) | ResultRow::Divider => false,
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
            ResultRow::Heading(_) | ResultRow::Divider => (),
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
            if entry.kind == EntryKind::Plugin && !self.ranked_root_search() {
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
            if (section, label) != previous
                && capacity >= 2
                && self.columns.is_empty()
                && let Some(label) = label
            {
                rows.push(ResultRow::Heading(label.into()));
            }
            // Only divide adjacent live-agent groups after filtering and ranking.
            // Waiting agents belong to the working group; exited agents stay unchanged.
            if (self.is_root() || self.agent_picker())
                && let Some(ResultRow::Entry(previous, _)) = rows.last()
                && matches!(
                    self.agent_presentation(*previous).map(|agent| agent.state),
                    Some(
                        yinz_types::agent::AgentState::Busy
                            | yinz_types::agent::AgentState::Waiting
                    )
                )
                && self.agent_presentation(index).map(|agent| agent.state)
                    == Some(yinz_types::agent::AgentState::Idle)
            {
                rows.push(ResultRow::Divider);
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
                        !self.ranked_root_search()
                            && self.entry_sections[**entry] == Some(section_index)
                    })
                    .map(|(index, _)| ResultRow::Entry(index, 0))
                    .collect();
                if entries.is_empty() && section.status.is_none() {
                    continue;
                }
                if capacity >= 2 && self.columns.is_empty() {
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

    /// Allocate in terminal cells. Collapse gaps first, then shrink preferred
    /// metadata widths so a flexible subject retains a useful share of the row.
    fn table_widths(&self, width: usize) -> (Vec<usize>, usize) {
        let count = self.columns.len();
        if count == 0 {
            return (vec![], 0);
        }
        let gap = usize::from(width >= count * 2 - 1);
        let available = width.saturating_sub(gap * (count - 1));
        if available == 0 {
            return (vec![0; count], gap);
        }
        let mut widths: Vec<_> = self.columns.iter().map(|c| c.width.unwrap_or(1)).collect();
        let flexible = self.columns.iter().position(|c| c.width.is_none());
        if let Some(index) = flexible {
            widths[index] = (available / 3).max(1);
        }
        if widths.iter().sum::<usize>() > available {
            // Find the common ceiling in logarithmic time instead of removing
            // thousands of cells individually on every measurement/render pass.
            let capped = |index: usize, width: usize, ceiling: usize| {
                width.min(ceiling).max(usize::from(Some(index) == flexible))
            };
            let mut low = 0;
            let mut high = widths.iter().copied().max().unwrap_or(0);
            while low < high {
                let ceiling = low + (high - low).div_ceil(2);
                let total: usize = widths
                    .iter()
                    .enumerate()
                    .map(|(i, width)| capped(i, *width, ceiling))
                    .sum();
                if total <= available {
                    low = ceiling;
                } else {
                    high = ceiling - 1;
                }
            }
            let total: usize = widths
                .iter()
                .enumerate()
                .map(|(i, width)| capped(i, *width, low))
                .sum();
            let mut remaining = available.saturating_sub(total);
            for (i, width) in widths.iter_mut().enumerate() {
                let reduced = capped(i, *width, low);
                // The old largest-first loop shrank later columns first on ties.
                let extra = usize::from(remaining > 0 && reduced == low && *width > reduced);
                *width = reduced + extra;
                remaining -= extra;
            }
        }
        if let Some(index) = flexible {
            widths[index] += available.saturating_sub(widths.iter().sum());
        }
        (widths, gap)
    }

    fn table_lines(&self, cells: &[String], width: usize, max_lines: usize) -> Vec<MdLine> {
        let (widths, gap) = self.table_widths(width);
        let columns: Vec<Vec<String>> = self
            .columns
            .iter()
            .zip(&widths)
            .enumerate()
            .map(|(i, (column, width))| {
                let width = *width;
                if width == 0 {
                    return vec![String::new()];
                }
                // Cells are plain text, never Markdown. Normalize permitted whitespace.
                let text = cells
                    .get(i)
                    .cloned()
                    .unwrap_or_default()
                    .replace(['\n', '\t'], " ");
                let limit = column.max_lines.min(max_lines).max(1);
                let mut lines = if limit == 1 {
                    vec![MdLine::plain(&text)]
                } else {
                    wrap_inline(&MdLine::plain(&text), width)
                };
                let truncated =
                    lines.len() > limit || lines.iter().any(|l| terminal_width(&l.text) > width);
                lines.truncate(limit);
                if truncated && let Some(last) = lines.last_mut() {
                    last.text = format!("{}…", take_columns(&last.text, width.saturating_sub(1)));
                }
                lines
                    .into_iter()
                    .map(|line| take_columns(&line.text, width))
                    .collect()
            })
            .collect();
        let height = columns.iter().map(Vec::len).max().unwrap_or(1);
        (0..height)
            .map(|line| {
                let mut text = String::new();
                for (i, (column, width)) in columns.iter().zip(&widths).enumerate() {
                    if i > 0 {
                        text.push_str(&" ".repeat(gap));
                    }
                    let cell = column.get(line).map(String::as_str).unwrap_or_default();
                    text.push_str(cell);
                    text.push_str(&" ".repeat(width.saturating_sub(terminal_width(cell))));
                }
                MdLine::plain(text)
            })
            .collect()
    }

    fn entry_lines(&self, index: usize, width: usize, max_lines: usize) -> Vec<MdLine> {
        use unicode_width::UnicodeWidthStr;
        let entry = &self.entries[self.matches[index]];
        if !self.columns.is_empty()
            && let Some(cells) = self.table_cells.get(&entry.id)
        {
            return self.table_lines(cells, width.saturating_sub(2), max_lines);
        }
        let shortcut = take_columns(&entry.shortcut, width.saturating_sub(3));
        let width = width.saturating_sub(shortcut.width() + usize::from(!shortcut.is_empty()));
        let width = width - width.saturating_sub(1).min(2);
        if width == 0 {
            return vec![MdLine::blank()];
        }
        if let Some(agent) = self.agent_presentation(index) {
            return agent.lines(width, max_lines);
        }
        let label = inline(&entry.label);
        let mut lines = wrap_inline(&label, width);
        for line in lines.iter_mut().skip(1) {
            *line = std::mem::take(line).indent(2.min(width.saturating_sub(1)));
        }
        let truncated =
            lines.len() > max_lines || lines.iter().any(|line| line.text.width() > width);
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            if truncated {
                last.text = take_columns(&last.text, width.saturating_sub(1));
                last.styles.truncate(last.text.chars().count());
                append(last, &MdLine::plain("…"));
            } else if !entry.description.is_empty() {
                append(last, &MdLine::plain(" — "));
                append(last, &inline(&entry.description));
            }
        }
        lines
    }

    fn agent_presentation(&self, index: usize) -> Option<&AgentPresentation> {
        let entry = &self.entries[self.matches[index]];
        (entry.kind == EntryKind::Agent)
            .then(|| self.agent_presentations.get(&entry.id))
            .flatten()
    }

    fn entry_line_limit(&self, index: usize, capacity: usize) -> usize {
        let heading = usize::from(self.entry_heading(index).is_some());
        let entry = &self.entries[self.matches[index]];
        let limit = if self.table_cells.contains_key(&entry.id) && !self.columns.is_empty() {
            self.columns.iter().map(|c| c.max_lines).max().unwrap_or(1)
        } else if self.agent_presentation(index).is_some() {
            5
        } else {
            2
        };
        capacity.saturating_sub(heading).clamp(1, limit)
    }

    fn entry_heading(&self, index: usize) -> Option<String> {
        if !self.columns.is_empty() {
            return None;
        }
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
                    let max_lines = self.entry_line_limit(index, capacity);
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
        let capacity = self
            .page_rows
            .get()
            .saturating_sub(usize::from(!self.columns.is_empty()))
            .max(1);
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
            ResultRow::Heading(_) | ResultRow::Divider => false,
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
            rows[target..].iter().find(|row| row.selectable())
        } else {
            rows[..=target]
                .iter()
                .rev()
                .find(|row| row.selectable())
                .or_else(|| rows.iter().find(|row| row.selectable()))
        };
        if let Some(row) = row {
            self.select_row(row);
        }
    }

    fn visible_result_rows(&self, capacity: usize, width: usize) -> Vec<ResultRow> {
        if self.columns.is_empty() || capacity < 2 {
            return self.visible_result_body(capacity, width);
        }
        let headers = self
            .columns
            .iter()
            .map(|c| c.header.clone())
            .collect::<Vec<_>>();
        let header = self
            .table_lines(&headers, width.saturating_sub(2), 1)
            .first()
            .map(|l| format!("  {}", l.text))
            .unwrap_or_default();
        let mut rows = vec![ResultRow::Heading(header)];
        rows.extend(self.visible_result_body(capacity - 1, width));
        rows
    }

    fn visible_result_body(&self, capacity: usize, width: usize) -> Vec<ResultRow> {
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
            if !self.columns.is_empty() {
                return None;
            }
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
                || matches!(rows.get(start), Some(ResultRow::Divider))
            {
                start += 1;
            } else {
                break;
            }
        }
        let mut visible = Vec::new();
        if capacity >= 2
            && let Some(heading) = sticky_heading(start)
        {
            visible.push(ResultRow::Heading(heading));
        }
        let remaining = capacity - visible.len();
        visible.extend(rows.into_iter().skip(start).take(remaining));
        if matches!(
            visible.last(),
            Some(ResultRow::Heading(_) | ResultRow::Divider)
        ) {
            visible.pop();
        }
        visible
    }

    fn result_overflow(&self, visible: &[ResultRow]) -> Option<(usize, usize)> {
        let visible_entries: std::collections::BTreeSet<_> = visible
            .iter()
            .filter_map(|row| match row {
                ResultRow::Entry(index, _) => Some(*index),
                _ => None,
            })
            .collect();
        if visible_entries.is_empty() {
            return None;
        }
        // Use display order, which can differ from match order for provider sections.
        // Count each entry once, even if it wraps or appears in multiple sections.
        let mut seen = std::collections::BTreeSet::new();
        let entries: Vec<_> = self
            .result_rows(1)
            .into_iter()
            .filter_map(|row| match row {
                ResultRow::Entry(index, _) => Some(index),
                _ => None,
            })
            .filter(|index| seen.insert(*index))
            .collect();
        let above = entries
            .iter()
            .position(|index| visible_entries.contains(index))?;
        let below = entries.len() - above - visible_entries.len();
        (above > 0 || below > 0).then_some((above, below))
    }

    pub fn layout(&self, surface: Viewport) -> CommandPaletteLayout {
        self.layout_with_arrow_fonts(surface, true)
    }

    pub fn layout_with_arrow_fonts(
        &self,
        surface: Viewport,
        arrow_fonts: bool,
    ) -> CommandPaletteLayout {
        let expanded = !self.columns.is_empty()
            || self.metadata.is_some()
            || !self.description.is_empty()
            || self.text.is_some()
            || self.agent.is_some();
        let cols = surface.cols.min(if expanded { 110 } else { 74 });
        let fit_results = !self.is_compact()
            && self.form.is_none()
            && self.text.is_none()
            && self.agent.is_none();
        let max_rows = if fit_results {
            (surface.rows * 3 / 5).max(1).min(surface.rows)
        } else {
            surface.rows.min(if expanded {
                36
            } else if self.is_compact() {
                3
            } else {
                11
            })
        };
        let border =
            usize::from(cols >= 3 && (max_rows >= 4 || (max_rows == 3 && self.is_compact())));
        let description_lines = if self.description.is_empty() {
            vec![]
        } else {
            md::render(
                &self.description,
                cols.saturating_sub(2 * border),
                Options::compact(),
            )
        };
        let mut metadata_lines = self
            .metadata
            .as_ref()
            .map(|text| md::render(text, cols.saturating_sub(2 * border), Options::compact()))
            .unwrap_or_default();
        let content_heights = |rows: usize| {
            let content_end = rows.saturating_sub(border + usize::from(rows >= 5));
            let description_y = border + 1 + usize::from(rows >= 6);
            let description_height = rows
                .saturating_sub(7)
                .min(rows / 3)
                .max(1)
                .min(content_end.saturating_sub(description_y).saturating_sub(1))
                .min(description_lines.len());
            let metadata_height = if self.metadata.is_some() {
                rows.saturating_sub(7 + description_height).min(rows / 3)
            } else {
                0
            };
            let options_rows =
                content_end.saturating_sub(description_y + description_height + metadata_height);
            (description_height, metadata_height, options_rows)
        };
        let rows = if fit_results {
            // Measure at the cap so shrinking does not hide wrapped lines or detail text.
            let (description_rows, metadata_rows, capacity) = content_heights(max_rows);
            let required = self
                .wrapped_result_rows(
                    capacity.saturating_sub(usize::from(!self.columns.is_empty())),
                    cols.saturating_sub(2 * border),
                )
                .len()
                .saturating_add(usize::from(!self.columns.is_empty()))
                .max(1);
            let metadata_rows = metadata_rows.min(metadata_lines.len());
            (max_rows.min(6)..=max_rows)
                .find(|rows| {
                    let (description_height, metadata_height, capacity) = content_heights(*rows);
                    required <= capacity
                        && description_height >= description_rows
                        && metadata_height >= metadata_rows
                })
                .unwrap_or(max_rows)
        } else {
            max_rows
        };
        let (description_height, metadata_height, options_rows) = content_heights(rows);
        let rect = CellRect {
            x: surface.x + (surface.cols - cols) / 2,
            y: surface.y + (surface.rows / 10).min(surface.rows - rows),
            cols,
            rows,
        };
        let input_y = rect.y + border;
        let footer_y = (rows >= 5).then_some(rect.y + rows.saturating_sub(border + 1));
        let description_y = input_y + 1 + usize::from(rows >= 6);
        self.description_total.set(description_lines.len());
        self.description_rows.set(description_height);
        let offset = self
            .description_offset
            .get()
            .min(description_lines.len().saturating_sub(description_height));
        self.description_offset.set(offset);
        let description_lines = description_lines
            .into_iter()
            .skip(offset)
            .take(description_height)
            .collect::<Vec<_>>();
        let metadata_y = description_y + description_lines.len();
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
        let text_lines = self
            .text
            .as_ref()
            .map(|text| md::render(text, cols.saturating_sub(2 * border), Options::compact()))
            .unwrap_or_default();
        self.text_rows.set(text_lines.len());
        self.page_rows.set(options_rows);
        self.result_width.set(cols.saturating_sub(2 * border));
        let offset = self
            .text_offset
            .min(text_lines.len().saturating_sub(options_rows));
        let result_rows = self.visible_result_rows(options_rows, self.result_width.get());
        let result_overflow = if footer_y.is_some() && self.agent.is_none() {
            self.result_overflow(&result_rows)
        } else {
            None
        };
        CommandPaletteLayout {
            agent: self.agent.as_ref().map(|a| {
                a.borrow_mut().layout(
                    rect,
                    border,
                    &self.input,
                    self.error.as_deref(),
                    self.busy || self.pending,
                    arrow_fonts,
                )
            }),
            rect,
            metadata_lines,
            metadata_y,
            description_lines,
            description_y,
            border,
            input_y,
            options_y,
            options_rows,
            first_option: (self.selection() + 1).saturating_sub(options_rows),
            footer_y,
            picker_heading_y: (self.picker.is_some() && self.form.is_none() && rows >= 6)
                .then_some(input_y + 1),
            result_rows,
            result_overflow,
            text_lines: text_lines
                .into_iter()
                .skip(offset)
                .take(options_rows)
                .collect(),
        }
    }

    pub fn click(&mut self, layout: &CommandPaletteLayout, position: Position) -> Outcome {
        let Ok(y) = usize::try_from(position.line()) else {
            return Outcome::None;
        };
        if !layout.rect.contains(position.column(), y) {
            return Outcome::None;
        }
        if let (Some(agent), Some(agent_layout)) = (&mut self.agent, &layout.agent) {
            let content = CellRect {
                x: layout.rect.x + layout.border + agent_layout.padding,
                y: layout.rect.y + layout.border,
                cols: layout
                    .rect
                    .cols
                    .saturating_sub(2 * (layout.border + agent_layout.padding)),
                rows: layout.rect.rows.saturating_sub(2 * layout.border),
            };
            if !content.contains(position.column(), y) {
                return Outcome::None;
            }
            return agent.get_mut().click(
                agent_layout,
                position.column() - content.x,
                y - content.y,
                &mut self.input,
            );
        }
        if (layout.options_y..layout.options_y + layout.options_rows).contains(&y) {
            let index = layout.first_option + y - layout.options_y;
            if let Some(form) = &mut self.form {
                if index < form.rows() {
                    form.selected = index;
                }
            } else if let Some(row) = layout.result_rows.get(y - layout.options_y) {
                self.select_row(row);
                if self.metadata.is_some()
                    && let ResultRow::Entry(index, _) = row
                {
                    self.metadata_focus =
                        if self.entries[self.matches[*index]].id.contains("event:") {
                            1
                        } else {
                            2
                        };
                }
            }
        }
        Outcome::None
    }

    pub fn cursor(&self, layout: &CommandPaletteLayout) -> Option<(usize, usize)> {
        if let Some(agent) = &layout.agent {
            return (!self.busy)
                .then_some(agent.cursor)
                .flatten()
                .map(|(x, y)| {
                    (
                        layout.rect.x + layout.border + agent.padding + x,
                        layout.rect.y + layout.border + y,
                    )
                });
        }
        if layout.rect.is_empty()
            || self.text.is_some()
            || self.busy
            || (self.form.is_none() && self.metadata.is_some() && self.metadata_focus == 0)
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
                &format!("{}: ", inline(&field.label).text),
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
                let mut markdown = MdLine::blank();
                let mut selected_agent = false;
                let mut shortcut_start = usize::MAX;
                let mut label_span = 0..usize::MAX;
                let (text, styles) = if layout.border == 1 && y == rect.y {
                    (
                        format!(
                            "╭{}╮",
                            fit(
                                &if self.form.as_ref().is_some_and(|f| f.confirmation) {
                                    " Confirm · PgUp/PgDn scroll details ".into()
                                } else if self.metadata.is_some() && self.form.is_none() {
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
                                let help = if self.busy {
                                    "Working…  Esc Cancel"
                                } else if self.form.as_ref().is_some_and(|f| f.confirmation) {
                                    "↑↓ Select  Enter Choose  PgUp/PgDn Details  Esc Back"
                                } else if self.metadata.is_some() && self.form.is_none() {
                                    "Tab Metadata/Timeline/Actions · ↑↓ PgUp/PgDn · Enter Open · Esc Back"
                                } else if self.text.is_some() {
                                    "↑↓ Scroll  PgUp/PgDn Page  Home/End  Esc Back"
                                } else if self.form.is_some() && self.metadata.is_some() {
                                    "Tab/↑↓ Fields  Enter Submit  PgUp/PgDn Details  Esc Back"
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
                                };
                                let help = if !self.busy && self.description_total.get() > layout.description_lines.len() {
                                    format!("Alt+PgUp/Dn Desc {}–{}/{} · {help}",
                                        self.description_offset.get() + 1,
                                        self.description_offset.get() + layout.description_lines.len(),
                                        self.description_total.get())
                                } else {
                                    help.into()
                                };
                                if !self.busy && let Some((above, below)) = layout.result_overflow {
                                    format!("↑ {above} above · ↓ {below} below · {help}")
                                } else {
                                    help
                                }
                            }),
                            if self.error.is_some() {
                                fill.clone()
                            } else {
                                heading.clone()
                            },
                        )
                    } else if (layout.description_y..layout.metadata_y).contains(&y) {
                        ({ markdown = layout.description_lines.get(y - layout.description_y).cloned().unwrap_or_default(); markdown.text.clone() }, heading.clone())
                    } else if (layout.metadata_y..layout.options_y).contains(&y) {
                        ({ markdown = layout.metadata_lines.get(y - layout.metadata_y).cloned().unwrap_or_default(); markdown.text.clone() }, if self.metadata_focus == 0 { fill.clone() } else { heading.clone() })
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
                            markdown = layout.text_lines.get(y - layout.options_y).cloned().unwrap_or_default();
                            markdown.text.clone()
                        } else if let Some(form) = &self.form {
                            if let Some(field) = form.fields.get(index) {
                                match &field.value {
                                    FieldValue::Text(input) => {
                                        markdown = inline(&field.label);
                                        viewport(
                                            &format!("{}: ", markdown.text),
                                            input.get_text(),
                                            input.get_cursor_position(),
                                            width,
                                        )
                                        .0
                                    },
                                    FieldValue::Choice { .. } => {
                                        markdown = inline(&field.label);
                                        format!("{}: ‹ {} ›{}", markdown.text, field.text_value(), if is_selected && !self.choice_input.get_text().is_empty() { format!(" (search: {})", self.choice_input.get_text()) } else { String::new() })
                                    },
                                }
                            } else if form.confirmation && index == form.fields.len() {
                                "Cancel".into()
                            } else if index == form.fields.len() + usize::from(form.confirmation) {
                                markdown = inline(&form.submit);
                                markdown.text.clone()
                            } else {
                                markdown = inline(form.actions.get(index - form.fields.len() - 1 - usize::from(form.confirmation)).map(|a| a.1.as_str()).unwrap_or_default());
                                markdown.text.clone()
                            }
                        } else if let Some(
                            ResultRow::Heading(label) | ResultRow::Status(_, label),
                        ) = row
                        {
                            markdown = if !self.columns.is_empty() && matches!(row, Some(ResultRow::Heading(_))) { MdLine::plain(label) } else { inline(label) };
                            markdown.text.clone()
                        } else if let Some(ResultRow::Divider) = row {
                            "─".repeat(width)
                        } else if let Some(ResultRow::Entry(index, line)) = row {
                            use unicode_width::UnicodeWidthStr;
                            let entry = &self.entries[self.matches[*index]];
                            let shortcut = take_columns(&entry.shortcut, width.saturating_sub(3));
                            let label_width = width.saturating_sub(shortcut.width() + usize::from(!shortcut.is_empty()));
                            let remaining = layout.result_rows.len() - (y - layout.options_y);
                            let max_lines = self.entry_line_limit(*index, layout.options_rows).min(line + remaining);
                            let lines = self.entry_lines(*index, width, max_lines);
                            selected_agent = is_selected && self.agent_presentation(*index).is_some();
                            let prefix = if is_selected && *line == 0 { "› " } else { "  " };
                            let prefix = take_columns(prefix, label_width.saturating_sub(1).min(2));
                            markdown = lines.get(*line).cloned().unwrap_or_default().prepend(&prefix, MdStyle::default());
                            let label = markdown.text.clone();
                            let label = take_columns(&label, label_width);
                            markdown.styles.truncate(label.chars().count());
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
                        } else if self.matches.is_empty()
                            && y == layout.options_y
                                + usize::from(!self.columns.is_empty() && layout.options_rows >= 2)
                        {
                            if self.picker == Some(EntryKind::Project)
                                && !self.entries.iter().any(|e| e.kind == EntryKind::Project)
                            {
                                "No projects".into()
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
                                Some(ResultRow::Heading(_) | ResultRow::Divider | ResultRow::Status(_, _))
                            ) {
                                heading.clone()
                            } else {
                                fill.clone()
                            },
                        )
                    } else {
                        (String::new(), fill.clone())
                    };
                    let text = if (y == layout.input_y && (self.form.is_some() || self.text.is_some()))
                        || (Some(y) == layout.footer_y && self.error.is_some()) {
                        markdown = inline(&text);
                        markdown.text.clone()
                    } else { text };
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
                            paint_markdown_with_selection(if (shortcut_start..text_len).contains(&index) {
                                subtle.clone()
                            } else if label_span.contains(&index) {
                                text_styles.clone()
                            } else {
                                styles.clone()
                            }, markdown.styles.get(index).copied().unwrap_or_default(), style, selected_agent),
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

/// Convert a Markdown palette label to the text shown in plain-text notifications.
pub fn plain_text(text: &str) -> String {
    inline(text).text
}

// Labels occupy one logical line; block bodies use the normal Markdown layout.
fn inline(text: &str) -> MdLine {
    let mut line = MdLine::blank();
    for block in md::parse(text) {
        if !line.is_empty() {
            append(&mut line, &MdLine::plain(" "));
        }
        append(&mut line, &MdLine::plain(block.prefix));
        if block.rule {
            append(&mut line, &MdLine::plain("───"));
        }
        // Flatten table cells without allocating a padded grid for a one-line preview.
        let chars = if let Some(table) = block.table {
            table
                .head
                .into_iter()
                .chain(table.rows.into_iter().flatten())
                .flat_map(|mut cell| {
                    cell.push(md::StyledChar {
                        ch: ' ',
                        style: MdStyle::default(),
                    });
                    cell
                })
                .collect()
        } else {
            block.chars
        };
        for styled in chars {
            line.text
                .push(if styled.ch == '\n' { ' ' } else { styled.ch });
            line.styles.push(styled.style);
        }
    }
    line
}

fn append(line: &mut MdLine, other: &MdLine) {
    if line.styles.is_empty() {
        line.styles
            .resize(line.text.chars().count(), MdStyle::default());
    }
    line.text.push_str(&other.text);
    line.styles.extend(
        other
            .text
            .chars()
            .enumerate()
            .map(|(i, _)| other.styles.get(i).copied().unwrap_or_default()),
    );
}

fn wrap_inline(line: &MdLine, width: usize) -> Vec<MdLine> {
    let block = md::MdBlock {
        chars: line
            .text
            .chars()
            .enumerate()
            .map(|(i, ch)| md::StyledChar {
                ch,
                style: line.styles.get(i).copied().unwrap_or_default(),
            })
            .collect(),
        ..Default::default()
    };
    md::render_block(&block, width)
}

fn paint_markdown_with_selection(
    base: RcCharacterStyles,
    md: MdStyle,
    style: &Style,
    selected: bool,
) -> RcCharacterStyles {
    if md == MdStyle::default() {
        return base;
    }
    let colors = if selected {
        style.colors.text_selected
    } else {
        style.colors.text_unselected
    };
    let mut painted = *base;
    if md.bold {
        painted = painted.bold(Some(AnsiCode::On));
    }
    if md.dim {
        painted = painted.dim(Some(AnsiCode::On));
    }
    if md.strike {
        painted = painted.strike(Some(AnsiCode::On));
    }
    if let Some(color) = md.color {
        painted = painted.foreground(Some(
            match color {
                0 => colors.emphasis_0,
                1 => colors.emphasis_1,
                2 => colors.emphasis_2,
                _ => colors.emphasis_3,
            }
            .into(),
        ));
    }
    if md.error {
        painted = painted.foreground(Some(style.colors.exit_code_error.base.into()));
    }
    painted.into()
}

// Match take_columns, Markdown wrapping, and emitted TerminalCharacters, which
// account for each character rather than treating an emoji sequence as one glyph.
fn terminal_width(text: &str) -> usize {
    use unicode_width::UnicodeWidthChar;
    text.chars().map(|c| c.width().unwrap_or(0)).sum()
}

fn fit(text: &str, cols: usize, pad: char) -> String {
    let mut text = take_columns(text, cols);
    let width = terminal_width(&text);
    text.extend(std::iter::repeat_n(pad, cols.saturating_sub(width)));
    text
}

#[cfg(test)]
#[path = "unit/command_palette_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "unit/command_palette_live_agent_tests.rs"]
mod live_agent_tests;

fn insert_multiline(input: &mut TextInput, text: &str) {
    let text: String = text
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    let cursor = input.get_cursor_position();
    let mut value = input.get_text().to_owned();
    let byte = value
        .char_indices()
        .nth(cursor)
        .map(|(i, _)| i)
        .unwrap_or(value.len());
    value.insert_str(byte, &text);
    input.replace_text(value, cursor + text.chars().count());
}
