use super::Screen;
use crate::ui::command_palette::{
    escape_literal, Entry, EntryKind, Field, Form, Outcome, PluginSection,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use yinz_bus::command_palette::{
    AgentAction, AgentLabelSource, AgentLabels, AgentLabelsRequest, AgentReply, AgentRequest,
};
use yinz_bus::plugins::PluginInstruction;
use yinz_runtime::errors::prelude::*;
use yinz_types::{agent::control::AgentOutcome, data::PaneInfo, internal_ids::ClientId};

static OPENING: AtomicU64 = AtomicU64::new(1);
const FIRST_VIEW: u64 = 1 << 63;

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
            Outcome::Activate(id) => (id, vec![]),
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
                let entries = choices
                    .into_iter()
                    .enumerate()
                    .map(|(i, choice)| {
                        let id = format!("native-agent:{step}:{i}");
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
