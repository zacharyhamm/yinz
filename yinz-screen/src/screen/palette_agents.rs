use super::Screen;
use crate::ui::command_palette::{
    escape_literal, Entry, EntryKind, Field, Form, Outcome, PluginSection,
};
use crate::ui::prompt_picker;
use std::collections::{BTreeMap, HashMap};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use yinz_bus::command_palette::{
    AgentAction, AgentLabelSource, AgentLabels, AgentLabelsRequest, AgentReply, AgentRequest,
    Instruction, PromptRecord,
};
use yinz_bus::plugins::PluginInstruction;
use yinz_runtime::errors::prelude::*;
use yinz_types::{agent::control::AgentOutcome, data::PaneInfo, internal_ids::ClientId};

static OPENING: AtomicU64 = AtomicU64::new(1);
const FIRST_VIEW: u64 = 1 << 63;

/// The past-prompt picker's list, freed with its view. Scoring runs off the screen thread.
struct PromptPicker {
    view: u64,
    /// `None` until the archive read returns.
    prompts: Option<Arc<[PromptRecord]>>,
    generation: u64,
    /// The generation whose results are on screen.
    shown: u64,
    /// Clears the search in flight when a newer query or the picker's end replaces it.
    active: Arc<AtomicBool>,
}

impl PromptPicker {
    fn cancel(&self) {
        self.active.store(false, Ordering::Release);
    }
}

impl Drop for PromptPicker {
    fn drop(&mut self) {
        self.cancel();
    }
}

const PROMPTS_UNREADABLE: &str = "Could not read past prompts";

pub(super) struct State {
    pub(super) opening: u64,
    pub(super) step: u64,
    source: PaneInfo,
    pending: Option<(u64, Arc<AtomicBool>, AgentAction)>,
    pub(super) actions: HashMap<String, AgentAction>,
    pub(super) browsers: HashMap<u64, super::history::Browser>,
    submitted: bool,
    pub(super) values: HashMap<String, Vec<String>>,
    pub(super) labels: BTreeMap<String, AgentLabels>,
    prompt_picker: Option<PromptPicker>,
    label_sources: Vec<AgentLabelSource>,
    label_revision: u64,
    labels_dirty: bool,
    labels_inflight: Option<u64>,
}

impl State {
    pub(super) fn new(source: PaneInfo) -> Self {
        Self {
            opening: OPENING.fetch_add(1, Ordering::Relaxed),
            step: 0,
            source,
            pending: None,
            actions: HashMap::new(),
            browsers: HashMap::new(),
            submitted: false,
            values: HashMap::new(),
            labels: BTreeMap::new(),
            prompt_picker: None,
            label_sources: Vec::new(),
            label_revision: 0,
            labels_dirty: true,
            labels_inflight: None,
        }
    }
    pub(super) fn refresh_labels(
        &mut self,
        sources: Vec<AgentLabelSource>,
        client: ClientId,
        senders: &yinz_bus::thread_bus::ThreadSenders,
    ) {
        if self.label_sources != sources {
            self.label_sources = sources;
            self.labels.clear();
            self.invalidate_labels();
        }
        if self.labels_dirty && self.labels_inflight.is_none() {
            self.labels_dirty = false;
            if self.label_sources.is_empty() {
                return;
            }
            let revision = self.label_revision;
            senders
                .send_to_plugin(PluginInstruction::PaletteAgentLabels(AgentLabelsRequest {
                    client,
                    opening: self.opening,
                    revision,
                    sources: self.label_sources.clone(),
                }))
                .context("requesting palette agent labels")
                .non_fatal();
            self.labels_inflight = Some(revision);
        }
    }

    pub(super) fn invalidate_labels(&mut self) {
        self.label_revision += 1;
        self.labels_dirty = true;
    }

    pub(super) fn apply_labels(
        &mut self,
        opening: u64,
        revision: u64,
        labels: Result<BTreeMap<String, AgentLabels>, String>,
    ) {
        if self.opening != opening || self.labels_inflight != Some(revision) {
            return;
        }
        self.labels_inflight = None;
        if self.label_revision == revision {
            match labels {
                Ok(labels) => self.labels = labels,
                Err(error) => log::error!("Could not read palette agent labels: {error}"),
            }
        }
    }

    pub(super) fn save_form(&mut self, widget: &crate::ui::command_palette::CommandPalette) {
        if let Some((id, values)) = widget.form_values()
            && let Some(action) = self.actions.get(id)
        {
            self.values.insert(format!("{action:?}"), values);
        }
    }
    pub(super) fn owns_view(&self, view: u64) -> bool {
        view >= FIRST_VIEW
    }
    pub(super) fn pending(&self) -> bool {
        self.pending.is_some()
    }
    fn prompt_pending(&self) -> bool {
        matches!(self.pending, Some((_, _, AgentAction::Prompt { .. })))
    }
    pub(super) fn cancel(&mut self) {
        if let Some((_, active, _)) = self.pending.take() {
            active.store(false, Ordering::Release);
        }
    }
}

impl Screen {
    /// Open the past-prompt picker over the agent view, which keeps its draft underneath.
    pub(super) fn open_prompt_picker(&mut self, client: ClientId) {
        let timezone = self
            .client_timezones
            .get(&client)
            .cloned()
            .unwrap_or_default();
        let Some(state) = self.command_palettes.get_mut(&client) else {
            return;
        };
        let Some(draft) = state.widget.agent_prompt_text() else {
            return;
        };
        // A poll still pending would otherwise be dropped once the view changes.
        state.agents.cancel();
        state.agents.step += 1;
        let view = FIRST_VIEW + state.agents.step;
        state
            .widget
            .push_view(view, prompt_picker::TITLE.to_owned());
        state.widget.set_query(prompt_picker::seed_query(&draft));
        state
            .widget
            .set_ranked_section(prompt_picker::section(&[], &[], "Loading prompts…"));
        state.agents.prompt_picker = Some(PromptPicker {
            view,
            prompts: None,
            generation: 0,
            shown: 0,
            active: Arc::new(AtomicBool::new(true)),
        });
        let request = AgentRequest {
            timezone,
            client,
            opening: state.agents.opening,
            step: view,
            source: state.agents.source.clone(),
            action: AgentAction::PromptLibrary,
            values: vec![],
            active: Arc::new(AtomicBool::new(true)),
        };
        if let Err(error) = self
            .bus
            .senders
            .send_to_plugin(PluginInstruction::PaletteAgent(Box::new(request)))
        {
            state
                .widget
                .set_ranked_section(prompt_picker::section(&[], &[], PROMPTS_UNREADABLE));
            state
                .widget
                .set_error(format!("{PROMPTS_UNREADABLE}: {error}"));
        }
    }

    pub(super) fn prompt_library_loaded(
        &mut self,
        client: ClientId,
        opening: u64,
        view: u64,
        prompts: Result<Arc<[PromptRecord]>, String>,
    ) {
        let Some(state) = self
            .command_palettes
            .get_mut(&client)
            .filter(|state| state.agents.opening == opening && state.widget.view == view)
        else {
            return;
        };
        let Some(picker) = state
            .agents
            .prompt_picker
            .as_mut()
            .filter(|picker| picker.view == view)
        else {
            return;
        };
        match prompts {
            Ok(prompts) => {
                picker.prompts = Some(prompts);
                self.search_prompt_picker(client);
            },
            Err(error) => {
                state.widget.set_ranked_section(prompt_picker::section(
                    &[],
                    &[],
                    PROMPTS_UNREADABLE,
                ));
                state.widget.set_error(error);
            },
        }
    }

    /// Follow a view or query change: score the new query, or drop a picker no longer on screen.
    pub(super) fn refresh_prompt_picker(&mut self, client: ClientId) {
        let Some(state) = self.command_palettes.get_mut(&client) else {
            return;
        };
        if state
            .agents
            .prompt_picker
            .as_ref()
            .is_some_and(|picker| picker.view != state.widget.view)
        {
            state.agents.prompt_picker = None;
            return;
        }
        self.search_prompt_picker(client);
    }

    fn search_prompt_picker(&mut self, client: ClientId) {
        let Some(state) = self.command_palettes.get_mut(&client) else {
            return;
        };
        let opening = state.agents.opening;
        let query = state.widget.query().to_owned();
        let Some(picker) = state.agents.prompt_picker.as_mut() else {
            return;
        };
        let Some(prompts) = picker.prompts.clone() else {
            return;
        };
        picker.cancel();
        picker.generation += 1;
        picker.active = Arc::new(AtomicBool::new(true));
        let (view, generation, active) = (picker.view, picker.generation, picker.active.clone());
        let senders = self.bus.senders.clone();
        crate::global_async_runtime::get_tokio_runtime().spawn_blocking(move || {
            let Some(matches) = prompt_picker::matches(&prompts, &query, &active) else {
                return;
            };
            senders
                .send_to_screen(yinz_bus::screen::ScreenInstruction::CommandPalette(
                    Instruction::PromptLibraryMatches {
                        client,
                        opening,
                        view,
                        generation,
                        matches,
                    },
                ))
                .context("replying with palette prompt matches")
                .non_fatal();
        });
    }

    pub(super) fn prompt_library_matches(
        &mut self,
        client: ClientId,
        opening: u64,
        view: u64,
        generation: u64,
        matches: Vec<usize>,
    ) {
        let Some(state) = self
            .command_palettes
            .get_mut(&client)
            .filter(|state| state.agents.opening == opening && state.widget.view == view)
        else {
            return;
        };
        let Some(picker) = state
            .agents
            .prompt_picker
            .as_mut()
            .filter(|picker| picker.view == view && picker.generation == generation)
        else {
            return;
        };
        let Some(prompts) = picker.prompts.as_ref() else {
            return;
        };
        picker.shown = generation;
        state.widget.set_ranked_section(prompt_picker::section(
            prompts,
            &matches,
            "No matching prompts",
        ));
    }

    /// Put the chosen prompt in the draft for editing. Nothing is sent until Enter there.
    pub(super) fn accept_prompt_picker(&mut self, client: ClientId, index: &str) -> bool {
        let Some(state) = self.command_palettes.get_mut(&client) else {
            return false;
        };
        let Some(picker) = &state.agents.prompt_picker else {
            return false;
        };
        // The rows still rank an older query until its results arrive, so a quick Enter after
        // typing would otherwise overwrite the draft with a prompt nobody searched for.
        if picker.shown != picker.generation {
            return true;
        }
        let text = index.parse::<usize>().ok().and_then(|index| {
            let prompts = picker.prompts.as_ref()?;
            Some(prompts.get(index)?.text.clone())
        });
        let Some(text) = text else {
            return false;
        };
        state.agents.prompt_picker = None;
        state.widget.back();
        state.widget.set_agent_prompt(&text);
        self.refresh_palette_history(client, false);
        true
    }

    pub(super) fn request_palette_agent(
        &mut self,
        client: ClientId,
        action: AgentAction,
        values: Vec<String>,
    ) {
        let Some(state) = self.command_palettes.get_mut(&client) else {
            return;
        };
        if state.agents.pending() {
            return;
        }
        if state.agents.submitted {
            let message = "Launch already submitted; inspect agents before reopening the palette";
            super::notify(&self.bus.senders, message.into());
            state.widget.set_error(message.into());
            return;
        }
        state.cancel_requests(&self.bus.senders, client);
        state.agents.submitted = matches!(
            &action,
            AgentAction::Current { .. }
                | AgentAction::Duplicate { .. }
                | AgentAction::Launch { .. }
                | AgentAction::History(yinz_bus::command_palette::HistoryAction::Resume { .. })
        ) || matches!(&action, AgentAction::Checkout { path, .. } if path.is_some() || !values.is_empty())
            || matches!(&action, AgentAction::NewProject { .. } if !values.is_empty());
        state.agents.step += 1;
        let active = Arc::new(AtomicBool::new(true));
        state.agents.pending = Some((state.widget.view, active.clone(), action.clone()));
        state.widget.busy = !matches!(action, AgentAction::History(ref a) if !matches!(a, yinz_bus::command_palette::HistoryAction::Resume { .. }));
        // Background history polls must leave submission errors visible.
        if state.widget.busy {
            state.widget.clear_error();
        }
        let prompt = state.agents.prompt_pending();
        let request = AgentRequest {
            timezone: self
                .client_timezones
                .get(&client)
                .cloned()
                .unwrap_or_default(),
            client,
            opening: state.agents.opening,
            step: state.agents.step,
            source: state.agents.source.clone(),
            action,
            values,
            active,
        };
        if matches!(&request.action, AgentAction::History(action) if !matches!(action, yinz_bus::command_palette::HistoryAction::Resume { .. }))
        {
            let senders = self.bus.senders.clone();
            crate::global_async_runtime::get_tokio_runtime().spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                if !request.active.load(Ordering::Acquire) {
                    return;
                }
                let (opening, step) = (request.opening, request.step);
                if let Err(error) =
                    senders.send_to_plugin(PluginInstruction::PaletteAgent(Box::new(request)))
                {
                    senders
                        .send_to_screen(yinz_bus::screen::ScreenInstruction::CommandPalette(
                            yinz_bus::command_palette::Instruction::AgentReply {
                                client,
                                opening,
                                step,
                                reply: Err(format!("Could not read archive: {error}")),
                            },
                        ))
                        .non_fatal();
                }
            });
            return;
        }
        if let Err(error) = self
            .bus
            .senders
            .send_to_plugin(PluginInstruction::PaletteAgent(Box::new(request)))
        {
            state.agents.cancel();
            state.agents.submitted = false;
            state.widget.busy = false;
            super::notify(
                &self.bus.senders,
                format!("Could not request agent action: {error}"),
            );
            state
                .widget
                .set_error(format!("Could not request agent action: {error}"));
            if prompt {
                self.schedule_palette_history_tick(client);
            }
        }
    }

    pub(super) fn agent_palette_outcome(&mut self, client: ClientId, outcome: &Outcome) -> bool {
        let Some(state) = self.command_palettes.get_mut(&client) else {
            return false;
        };
        if *outcome == Outcome::Cancel && state.agents.pending() {
            let prompt = state.agents.prompt_pending();
            state.agents.cancel();
            state.widget.busy = false;
            if state.agents.submitted {
                state.widget.set_error(
                    "Stopped waiting; launch may still complete. Inspect agents before retrying."
                        .into(),
                );
            }
            if prompt {
                self.schedule_palette_history_tick(client);
            }
            return true;
        }
        let (id, values) = match outcome {
            Outcome::Activate(id) => (id, state.widget.worktree_creation_values(id)),
            Outcome::Submit { id, values } => (id, values.clone()),
            _ => return false,
        };
        let Some(action) = state.agents.actions.get(id).cloned() else {
            return false;
        };
        self.request_palette_agent(client, action, values);
        true
    }

    pub(super) fn agent_palette_reply(
        &mut self,
        client: ClientId,
        opening: u64,
        step: u64,
        reply: Result<AgentReply, String>,
    ) {
        let Some(state) = self.command_palettes.get_mut(&client) else {
            return;
        };
        if state.agents.opening != opening
            || state.agents.step != step
            || !state
                .agents
                .pending
                .as_ref()
                .is_some_and(|(view, active, _)| {
                    *view == state.widget.view && active.load(Ordering::Acquire)
                })
        {
            return;
        }
        let history_action = state.agents.pending.as_ref().and_then(|(_, _, action)| {
            if let AgentAction::History(action) = action {
                Some(action.clone())
            } else {
                None
            }
        });
        let label = match state.agents.pending.as_ref().map(|(_, _, action)| action) {
            Some(AgentAction::History(yinz_bus::command_palette::HistoryAction::Resume {
                ..
            })) => "Resume agent",
            Some(AgentAction::Duplicate { .. }) => "Duplicate agent",
            Some(AgentAction::Fork) => "Fork agent",
            Some(AgentAction::Prompt { .. }) => "Send prompt",
            _ => "Launch agent",
        };
        let prompt = state.agents.prompt_pending();
        state.agents.cancel();
        state.widget.busy = false;
        match reply {
            Ok(AgentReply::Prompted(response)) => {
                if response.outcome == AgentOutcome::Ok {
                    if let Some(browser) = state.agents.browsers.get_mut(&state.widget.view) {
                        browser.clear_prompt();
                    }
                    if !state.widget.agent_prompt_sent() {
                        state.widget.back();
                    }
                    super::notify(&self.bus.senders, "Prompt sent".into());
                    self.refresh_palette_history(client, false);
                } else {
                    state.widget.set_error(
                        response
                            .message
                            .unwrap_or_else(|| format!("Prompt: {:?}", response.outcome)),
                    );
                    self.schedule_palette_history_tick(client);
                }
            },
            Err(error) => {
                if history_action.as_ref().is_none_or(|action| {
                    matches!(
                        action,
                        yinz_bus::command_palette::HistoryAction::Resume { .. }
                    )
                }) {
                    super::notify(
                        &self.bus.senders,
                        format!("Could not {}: {error}", label.to_lowercase()),
                    );
                }
                state.agents.submitted = false;
                state.widget.set_error(error);
                if prompt {
                    self.schedule_palette_history_tick(client);
                }
            },
            Ok(AgentReply::History(reply)) => {
                self.palette_history_reply(client, reply, history_action);
            },
            Ok(AgentReply::Choices { title, choices }) => {
                state.agents.submitted = false;
                state.widget.push_view(FIRST_VIEW + step, title.clone());
                let mut creation = None;
                let mut navigation = Vec::new();
                let entries = choices
                    .into_iter()
                    .enumerate()
                    .map(|(i, choice)| {
                        let id = format!("native-agent:{step}:{i}");
                        match &choice.action {
                            AgentAction::Checkout { path: None, .. } => creation = Some(id.clone()),
                            AgentAction::Projects { .. } => navigation.push(id.clone()),
                            _ => {},
                        }
                        state.agents.actions.insert(id.clone(), choice.action);
                        Entry {
                            id,
                            label: escape_literal(&choice.label),
                            description: escape_literal(&choice.description),
                            aliases: vec![choice.description],
                            shortcut: String::new(),
                            // Section rows use the widget’s grouped entry kind.
                            kind: EntryKind::Plugin,
                        }
                    })
                    .collect::<Vec<_>>();
                let status = entries.is_empty().then(|| "No projects available".into());
                state.widget.set_sections(vec![PluginSection {
                    title: escape_literal(&title),
                    entries,
                    semantic: false,
                    status,
                }]);
                if let Some(id) = creation {
                    state.widget.set_worktree_creation(id, navigation);
                }
            },
            Ok(AgentReply::Form {
                title,
                labels,
                submit,
                action,
            }) => {
                state.agents.submitted = false;
                state.widget.push_view(FIRST_VIEW + step, title.clone());
                let id = format!("native-agent:{step}:form");
                state.agents.actions.insert(id.clone(), action.clone());
                state.widget.open_form(Form::new(
                    id,
                    escape_literal(&title),
                    labels
                        .iter()
                        .enumerate()
                        .map(|(index, label)| {
                            Field::text(
                                label,
                                state
                                    .agents
                                    .values
                                    .get(&format!("{action:?}"))
                                    .and_then(|values| values.get(index))
                                    .map(String::as_str)
                                    .unwrap_or(""),
                            )
                        })
                        .collect(),
                    &submit,
                    false,
                ));
            },
            Ok(AgentReply::Launched(response)) => {
                if response.outcome == AgentOutcome::Rejected
                    && response.spawned.is_none()
                    && response.launch_resources.is_none()
                {
                    state.agents.submitted = false;
                }
                if response.outcome == AgentOutcome::Ok
                    && let Some(spawned) = response.spawned
                {
                    super::notify(&self.bus.senders, format!("Completed: {label}"));
                    self.close_command_palette(client);
                    self.focus_pane_with_id(spawned.pane_id, false, false, client)
                        .context("focusing palette agent")
                        .non_fatal();
                    self.sync_scroll_mode_on_focus(client)
                        .context("syncing palette agent focus")
                        .non_fatal();
                    self.log_and_report_session_state()
                        .context("reporting palette agent focus")
                        .non_fatal();
                    return;
                }
                let mut message = response
                    .message
                    .unwrap_or_else(|| "Agent launch did not complete".into());
                super::notify(
                    &self.bus.senders,
                    format!("Could not {}: {message}", label.to_lowercase()),
                );
                let partial = response.launch_resources.is_some() || response.spawned.is_some();
                if let Some(resources) = response.launch_resources {
                    if let Some(cwd) = resources.cwd {
                        message.push_str(&format!("\nCheckout: {}", cwd.display()));
                    }
                    if let Some(branch) = resources.branch {
                        message.push_str(&format!("\nBranch: {branch}"));
                    }
                    if let Some(tab) = resources.created_tab_id {
                        message.push_str(&format!("\nCreated tab: {tab}"));
                    }
                }
                if let Some(spawned) = response.spawned {
                    message.push_str(&format!("\nCreated pane: {}", spawned.pane_id));
                }
                if partial {
                    state
                        .widget
                        .push_view(FIRST_VIEW + step, "Agent launch failed".into());
                    state.widget.open_text(escape_literal(&message));
                } else {
                    state.widget.set_error(message);
                }
            },
        }
    }
}
