use super::{Builtin, Screen};
use crate::ui::command_palette::{
    escape_literal, Entry, EntryKind, Field, Form, Outcome, PluginSection, PROMPT_LIBRARY_ACTION,
};
use crate::ui::prompt_picker;
use std::collections::{BTreeMap, HashMap};
use yinz_bus::command_palette::{
    query::{AgentQuery, ARCHIVE_FIELDS, LIVE_FIELDS},
    AgentAction, HistoryAction, HistoryHeader, HistoryReply, HistoryRow, Instruction,
};
use yinz_runtime::errors::prelude::*;
use yinz_types::{
    data::PaneInfo,
    input::actions::Action,
    internal_ids::{ClientId, SessionName},
};

#[derive(Clone)]
pub(in crate::screen) struct LiveAgent {
    pub(in crate::screen) session: SessionName,
    host: String,
    pub(in crate::screen) pane: PaneInfo,
    pub(in crate::screen) metadata: BTreeMap<String, String>,
}
impl LiveAgent {
    pub(in crate::screen) fn id(&self) -> String {
        format!(
            "live-agent:{}:{}:{}:{}",
            self.host.len(),
            self.host,
            self.session,
            self.pane.id
        )
    }
    pub(super) fn order<'a>(
        &'a self,
        labels: &'a BTreeMap<String, yinz_bus::command_palette::AgentLabels>,
    ) -> yinz_types::agent::LiveAgentOrder<'a> {
        let labels = labels.get(&self.id());
        yinz_types::agent::LiveAgentOrder::new(
            self.pane.agent.as_ref().map(|agent| agent.state),
            self.pane.last_hook_at_ms,
            labels.map(|labels| labels.project.as_str()),
            labels.map(|labels| labels.branch.as_str()),
            self.pane.id,
            self.session.as_str(),
        )
    }
    pub(in crate::screen) fn label_source(&self) -> yinz_bus::command_palette::AgentLabelSource {
        yinz_bus::command_palette::AgentLabelSource {
            id: self.id(),
            host: self.pane.host.clone(),
            cwd: self
                .pane
                .agent
                .as_ref()
                .and_then(|a| a.cwd.as_ref())
                .filter(|cwd| !cwd.is_empty())
                .or(self.pane.cwd.as_ref())
                .cloned(),
            run: self
                .run()
                .map(|id| yinz_bus::command_palette::AgentRunSource {
                    id,
                    host_id: yinz_runtime::host_identity::host_id(),
                    session: self.session.clone(),
                }),
        }
    }

    pub(in crate::screen) fn presentation(
        &self,
    ) -> Option<crate::ui::command_palette::AgentPresentation> {
        crate::ui::command_palette::AgentPresentation::from_pane(&self.pane)
    }

    pub(in crate::screen) fn usage_unavailable(
        &self,
        session: &SessionName,
    ) -> Option<&'static str> {
        match &self.pane.target {
            _ if self.session != *session => Some("Quota unavailable for peer-session agents"),
            yinz_types::command::PaneTarget::Ssh { .. } => Some("Quota unavailable for SSH agents"),
            yinz_types::command::PaneTarget::Bubblewrap { .. } => {
                Some("Quota unavailable for isolated agents")
            },
            _ => None,
        }
    }

    pub(in crate::screen) fn header_overlay(
        &self,
        header: &mut HistoryHeader,
        labels: Option<&yinz_bus::command_palette::AgentLabels>,
    ) {
        header.live = Some(self.pane.clone());
        if let Some(labels) = labels {
            header.project.clone_from(&labels.project);
            header.branch.clone_from(&labels.branch);
        }
        header.name = self.pane.title.clone();
        header.status = ["provider", "state"]
            .iter()
            .filter_map(|key| self.metadata.get(*key).cloned())
            .collect::<Vec<_>>()
            .join(" · ");
        header.location = ["session", "directory"]
            .iter()
            .filter_map(|key| self.metadata.get(*key).cloned())
            .collect::<Vec<_>>()
            .join(" · ");
        header.details = format!("{}\n{}", self.text(), header.details);
        header.task = self.pane.title.clone();
        if header.project.is_empty() {
            header.project = self.metadata.get("project").cloned().unwrap_or_default();
        }
        header.session_id = self
            .metadata
            .get("agent-session")
            .map(|id| format!("session {id}"))
            .unwrap_or_else(|| header.session_id.clone());
        if let Some(agent) = &self.pane.agent {
            header.status = format!(
                "[{}] · {} · {}",
                self.pane.id.legacy_parts().0,
                agent.kind,
                match agent.state {
                    yinz_types::agent::AgentState::Busy => "working",
                    yinz_types::agent::AgentState::Waiting => "needs input",
                    yinz_types::agent::AgentState::Idle => "idle",
                    yinz_types::agent::AgentState::Exited => "exited",
                }
            );
        }
    }

    pub(super) fn description(&self) -> String {
        ["provider", "state", "session", "directory"]
            .iter()
            .map(|k| {
                self.metadata
                    .get(*k)
                    .map(String::as_str)
                    .unwrap_or("Unavailable")
            })
            .collect::<Vec<_>>()
            .join(" · ")
    }
    fn same(&self, other: &Self) -> bool {
        self.id() == other.id()
            && self.pane.target == other.pane.target
            && self
                .pane
                .agent
                .as_ref()
                .zip(other.pane.agent.as_ref())
                .is_some_and(|(a, b)| {
                    a.kind == b.kind
                        && a.session_id == b.session_id
                        && a.run_id
                            .as_ref()
                            .is_none_or(|id| b.run_id.as_ref() == Some(id))
                })
    }
    pub(in crate::screen) fn run(&self) -> Option<String> {
        let a = self.pane.agent.as_ref()?;
        a.run_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                a.session_id
                    .as_ref()
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| format!("{}:{}", a.kind, s.trim()))
            })
    }
    pub(in crate::screen) fn text(&self) -> String {
        let mut text = LIVE_FIELDS
            .iter()
            .map(|k| {
                format!(
                    "{k}: {}",
                    self.metadata
                        .get(*k)
                        .map(String::as_str)
                        .unwrap_or("Unavailable")
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        if let Some(a) = &self.pane.agent {
            text.push_str(&format!("\nWait needs a person: {}", a.wait_needs_a_person));
        }
        text
    }
}

pub(super) struct Browser {
    live: Option<LiveAgent>,
    action: HistoryAction,
    rows: HashMap<String, HistoryAction>,
    generation: u64,
    history_scope: Option<(String, yinz_bus::command_palette::HistoryScope)>,
    continuation: Option<HistoryAction>,
    history: HistoryCache,
    cached_history: HashMap<(String, yinz_bus::command_palette::HistoryScope), HistoryCache>,
    usage_checked: Option<std::time::Instant>,
    usage: Option<yinz_types::agent::control::ProviderUsage>,
    prompt: Option<PromptSubmission>,
    search_return: Option<HistoryAction>,
}

#[derive(Default)]
struct HistoryCache {
    events: BTreeMap<u64, yinz_bus::command_palette::HistoryEvent>,
    earlier: Option<HistoryRow>,
    // Highest revision covered by the initial tail or live revision delivery.
    latest_received_revision: u64,
}

impl HistoryCache {
    fn merge(
        &mut self,
        activity_ranges: &[yinz_types::agent::history::Range],
        events: Vec<yinz_bus::command_palette::HistoryEvent>,
    ) {
        self.events.retain(|_, old| {
            old.range
                .as_ref()
                .is_none_or(|old| !activity_ranges.iter().any(|range| old.overlaps(range)))
        });
        self.events
            .extend(events.into_iter().map(|event| (event.revision, event)));
    }
}

struct PromptSubmission {
    live: LiveAgent,
    action: AgentAction,
    values: Vec<String>,
}

impl Browser {
    fn prompt_action(&mut self, session: &SessionName, values: &[String]) -> Option<AgentAction> {
        let live = self.live.as_ref()?;
        if let Some(prompt) = &self.prompt
            && prompt.live.same(live)
            && prompt.values == values
        {
            return Some(prompt.action.clone());
        }
        let action = AgentAction::Prompt {
            pane_id: live.pane.id,
            session_id: live.pane.agent.as_ref().and_then(|a| a.session_id.clone()),
            target_session: (live.session != *session).then(|| live.session.clone()),
            submission_id: uuid::Uuid::new_v4().to_string(),
        };
        self.prompt = Some(PromptSubmission {
            live: live.clone(),
            action: action.clone(),
            values: values.to_vec(),
        });
        Some(action)
    }

    pub(super) fn clear_prompt(&mut self) {
        self.prompt = None;
    }
}

impl Screen {
    pub(super) fn refresh_palette_history_titles(&mut self) {
        for state in self.command_palettes.values_mut() {
            state.widget.set_agent_header_flash(None, false);
        }
        if !self.command_palettes.values().any(|state| {
            state
                .agents
                .browsers
                .get(&state.widget.view)
                .is_some_and(|browser| browser.live.is_some())
        }) {
            return;
        }
        let live = self.live_palette_agents();
        let flashing = self.flashing_palette_agent_ids(self.frame_now());
        for state in self.command_palettes.values_mut() {
            let Some(browser) = state.agents.browsers.get_mut(&state.widget.view) else {
                continue;
            };
            let Some(old) = &browser.live else {
                continue;
            };
            let Some(current) = live.iter().find(|agent| old.same(agent)) else {
                continue;
            };
            let id = current.id();
            state
                .widget
                .set_agent_header_flash(Some(id.clone()), flashing.contains(&id));
            let labels = state.agents.labels.get(&id);
            state.widget.refresh_agent_title(
                current.pane.clone(),
                labels.map(|l| l.project.clone()).unwrap_or_else(|| {
                    current.metadata.get("project").cloned().unwrap_or_default()
                }),
                labels.map(|l| l.branch.clone()).unwrap_or_default(),
            );
            // Leave run changes to the history tick, which also updates the query.
        }
    }
    pub(in crate::screen) fn live_palette_agents(&self) -> Vec<LiveAgent> {
        let mut agents = Vec::new();
        let mut add = |session: &SessionName, pane: PaneInfo| {
            let Some(a) = &pane.agent else {
                return;
            };
            if pane.exited || a.state == yinz_types::agent::AgentState::Exited {
                return;
            }
            let host = yinz_runtime::host_identity::host_id().to_string();
            let mut metadata = BTreeMap::from([
                ("title".into(), pane.title.clone()),
                ("provider".into(), a.kind.clone()),
                ("state".into(), a.state.to_string().to_lowercase()),
                ("source".into(), format!("{:?}", a.source).to_lowercase()),
                ("session".into(), session.to_string()),
                ("host".into(), host.clone()),
                (
                    "target".into(),
                    pane.target.display().unwrap_or_else(|| "local".into()),
                ),
                ("pane".into(), pane.id.to_string()),
            ]);
            for (k, v) in [
                ("directory", a.cwd.as_ref().or(pane.cwd.as_ref())),
                ("agent-session", a.session_id.as_ref()),
                ("run", a.run_id.as_ref()),
                ("detail", a.detail.as_ref()),
                ("waiting", a.waiting_on.as_ref()),
            ] {
                if let Some(v) = v {
                    metadata.insert(k.into(), v.clone());
                }
            }
            if session == &self.session_name
                && let Some(project) = self.pane_projects.get(&pane.id)
            {
                metadata.insert("project".into(), project.clone());
            }
            agents.push(LiveAgent {
                session: session.clone(),
                host,
                pane,
                metadata,
            });
        };
        for tab in self.tabs.values() {
            for pane in tab.pane_infos() {
                add(&self.session_name, pane);
            }
        }
        for session in self
            .peer_sessions_cache
            .values()
            .filter(|s| s.name != self.session_name)
        {
            for pane in session.panes.panes.values().flatten() {
                add(&session.name, pane.clone());
            }
        }
        agents.sort_by_key(LiveAgent::id);
        agents
    }

    pub(in crate::screen) fn open_palette_history(
        &mut self,
        client: ClientId,
        live: Option<LiveAgent>,
        action: Option<HistoryAction>,
    ) {
        let Some(state) = self.command_palettes.get_mut(&client) else {
            return;
        };
        state.agents.cancel();
        state.agents.step += 1;
        let view = (1 << 63) + state.agents.step;
        let action = action.unwrap_or_else(|| {
            live.as_ref()
                .and_then(LiveAgent::run)
                .map(|id| HistoryAction::Run {
                    id,
                    from: None,
                    query: String::new(),
                })
                .unwrap_or_else(|| {
                    if live.is_some() {
                        HistoryAction::Run {
                            id: String::new(),
                            from: None,
                            query: String::new(),
                        }
                    } else {
                        HistoryAction::Search(AgentQuery::default())
                    }
                })
        });
        let title = if matches!(action, HistoryAction::Workers { .. }) {
            "Workers"
        } else if matches!(action, HistoryAction::Tools { .. }) {
            "Tool activity"
        } else if matches!(
            action,
            HistoryAction::Conversation {
                worker: Some(_),
                ..
            }
        ) {
            "Worker history"
        } else if live.is_some()
            || matches!(
                action,
                HistoryAction::Run { .. } | HistoryAction::Hit { .. }
            )
        {
            "Agent info"
        } else {
            "Archived agents"
        };
        state.widget.push_view(view, title.into());
        let entries: Vec<_> = live
            .as_ref()
            .map(|_| Entry {
                id: "history-focus".into(),
                label: "Focus".into(),
                description: String::new(),
                shortcut: String::new(),
                aliases: vec![],
                kind: EntryKind::Plugin,
            })
            .into_iter()
            .collect();
        if matches!(
            action,
            HistoryAction::Search(_) | HistoryAction::Workers { .. }
        ) {
            state.widget.set_sections(vec![PluginSection {
                title: title.into(),
                entries,
                semantic: true,
                status: Some("Loading…".into()),
            }]);
        } else {
            state.widget.set_agent_history(
                HistoryHeader {
                    live: live.as_ref().map(|agent| agent.pane.clone()),
                    name: live
                        .as_ref()
                        .map(|live| live.pane.title.clone())
                        .unwrap_or_else(|| "Agent info".into()),
                    status: live
                        .as_ref()
                        .map(LiveAgent::description)
                        .unwrap_or_default(),
                    started: "Unavailable".into(),
                    details: live.as_ref().map(LiveAgent::text).unwrap_or_default(),
                    ..Default::default()
                },
                vec![],
                entries,
                "Loading…".into(),
            );
        }
        let missing = live.as_ref().is_some_and(|live| live.run().is_none());
        state.agents.browsers.insert(
            view,
            Browser {
                live,
                action: action.clone(),
                rows: HashMap::new(),
                generation: 0,
                cached_history: HashMap::new(),
                history_scope: None,
                continuation: None,
                history: HistoryCache::default(),
                usage_checked: None,
                usage: None,
                prompt: None,
                search_return: None,
            },
        );
        if missing {
            self.palette_history_reply(
                client,
                HistoryReply::Run {
                    header: HistoryHeader {
                        name: "Recorded history unavailable".into(),
                        details: "No recorded history".into(),
                        ..Default::default()
                    },
                    activity_ranges: vec![],
                    events: vec![],
                    rows: vec![],
                    status: "No recorded history".into(),
                },
                None,
            );
        } else {
            self.request_palette_agent(client, AgentAction::History(action), vec![]);
        }
    }

    pub(super) fn palette_history_reply(
        &mut self,
        client: ClientId,
        reply: HistoryReply,
        request: Option<HistoryAction>,
    ) {
        let mut request = request;
        if matches!(request, Some(HistoryAction::Hit { .. }))
            && let HistoryReply::Run { rows, .. } = &reply
        {
            if let Some(context) = rows.iter().find(|r| r.id == "context") {
                request = Some(context.action.clone());
            } else if let Some(HistoryAction::Hit { id, revision }) = &request {
                request = Some(HistoryAction::Run {
                    id: id.clone(),
                    from: Some(*revision),
                    query: String::new(),
                });
            }
        }
        let Some(state) = self.command_palettes.get_mut(&client) else {
            return;
        };
        let Some(browser) = state.agents.browsers.get_mut(&state.widget.view) else {
            return;
        };
        let mut continuation = None;
        match reply {
            HistoryReply::Reader {
                header,
                markdown,
                tool,
            } => state.widget.open_agent_reader(header, markdown, tool),
            HistoryReply::Run {
                mut header,
                activity_ranges,
                events,
                rows,
                status,
            } => {
                let mut rows = rows;
                let mut events = events;
                if let Some((id, query)) = request.as_ref().and_then(HistoryAction::scope) {
                    let scope = (id.to_owned(), query.clone());
                    if browser.history_scope.as_ref() != Some(&scope) {
                        if let Some(old) = browser.history_scope.take() {
                            browser
                                .cached_history
                                .insert(old, std::mem::take(&mut browser.history));
                        }
                        browser.history = browser.cached_history.remove(&scope).unwrap_or_default();
                        browser.history_scope = Some(scope);
                        browser.continuation = None;
                    }
                    let older = matches!(
                        request,
                        Some(
                            HistoryAction::Conversation {
                                before: Some(_),
                                ..
                            } | HistoryAction::Tools {
                                before: Some(_),
                                ..
                            }
                        )
                    );
                    let mut earlier = None;
                    rows.retain(|row| match row.id.as_str() {
                        "continue" => {
                            continuation = Some(row.action.clone());
                            false
                        },
                        "earlier" => {
                            earlier = Some(row.clone());
                            false
                        },
                        "context" => false,
                        "received" => {
                            if !older
                                && let HistoryAction::Run {
                                    from: Some(next), ..
                                }
                                | HistoryAction::Conversation {
                                    after_revision: Some(next),
                                    ..
                                } = row.action
                            {
                                browser.history.latest_received_revision = browser
                                    .history
                                    .latest_received_revision
                                    .max(next.saturating_sub(1));
                            }
                            false
                        },
                        _ => true,
                    });
                    if older
                        || browser.history.events.is_empty()
                        || browser.history.earlier.is_none()
                    {
                        browser.history.earlier = earlier;
                    }
                    rows.extend(browser.history.earlier.clone());
                    browser.history.merge(&activity_ranges, events);
                    events = browser.history.events.values().cloned().collect();
                    events.sort_by_key(|e| (e.created_at_ms, e.revision));
                    browser.continuation = continuation.clone();
                    rows.retain(|r| !r.id.starts_with("event:"));
                    let searching = matches!(request,Some(HistoryAction::Run {ref query,..}) if !query.is_empty());
                    for event in &events {
                        rows.push(HistoryRow {
                            id: format!("event:{}", event.revision),
                            label: String::new(),
                            description: String::new(),
                            action: if searching {
                                HistoryAction::Hit {
                                    id: id.into(),
                                    revision: event.revision,
                                }
                            } else if let Some(range) = &event.range {
                                HistoryAction::Tools {
                                    id: id.into(),
                                    range: range.clone(),
                                    before: None,
                                }
                            } else {
                                HistoryAction::Event {
                                    id: id.into(),
                                    revision: event.revision,
                                }
                            },
                        });
                    }
                }
                if let Some(live) = &browser.live {
                    live.header_overlay(&mut header, state.agents.labels.get(&live.id()));
                    header.usage = browser.usage.clone();
                }
                let entry = |id: String, label: String| Entry {
                    id,
                    label,
                    description: String::new(),
                    shortcut: String::new(),
                    aliases: vec![],
                    kind: EntryKind::Plugin,
                };
                let mut actions = Vec::new();
                if browser.live.is_some() {
                    actions.push(entry("history-focus".into(), "Focus".into()));
                    actions.push(entry("history-prompt".into(), "Send prompt…".into()));
                }
                browser.rows.clear();
                for row in rows {
                    let id = format!("history:{}:{}", state.widget.view, row.id);
                    let event = matches!(
                        row.action,
                        HistoryAction::Event { .. } | HistoryAction::Hit { .. }
                    ) || row.id.starts_with("event:")
                        && matches!(row.action, HistoryAction::Tools { .. });
                    if !event {
                        actions.push(entry(id.clone(), row.label));
                    }
                    browser.rows.insert(id, row.action);
                }
                actions.push(entry("history-details".into(), "Details".into()));
                if status.contains("Partial") {
                    state.widget.set_error(status.clone());
                }
                state
                    .widget
                    .set_agent_history(header, events, actions, status);
                if let HistoryAction::Hit { revision, .. } = &browser.action {
                    state.widget.select_agent_revision(*revision);
                    if let Some(action) = request.clone() {
                        browser.action = action;
                    }
                }
            },
            HistoryReply::Rows { rows, status } => {
                browser.rows.clear();
                let mut entries = Vec::new();
                let entry = |id: String, label: String, description: String| Entry {
                    id,
                    label: escape_literal(&label),
                    description: escape_literal(&description),
                    shortcut: String::new(),
                    aliases: vec![],
                    kind: EntryKind::Plugin,
                };
                if matches!(browser.action, HistoryAction::Search(_)) {
                    entries.push(entry(
                        "history-filters".into(),
                        "Filters…".into(),
                        "provider project session host run since".into(),
                    ));
                }
                for r in rows {
                    let id = format!("history:{}:{}", state.widget.view, r.id);
                    browser.rows.insert(id.clone(), r.action);
                    entries.push(entry(id, r.label, r.description));
                }
                entries.push(entry(
                    "history-refresh".into(),
                    "Refresh".into(),
                    String::new(),
                ));
                if status.contains("Partial") {
                    state.widget.set_error(status.clone());
                }
                state.widget.set_sections(vec![PluginSection {
                    title: "Archived agents".into(),
                    entries,
                    semantic: true,
                    status: Some(status),
                }]);
            },
        }
        if let Some(action) = continuation {
            self.request_palette_agent(client, AgentAction::History(action), vec![]);
        }
        self.schedule_palette_history_tick(client);
        self.refresh_palette_usage(client);
    }

    pub(super) fn schedule_palette_history_tick(&mut self, client: ClientId) {
        let Some(state) = self.command_palettes.get_mut(&client) else {
            return;
        };
        let view = state.widget.view;
        let Some(browser) = state.agents.browsers.get_mut(&view) else {
            return;
        };
        if browser.live.is_none() {
            return;
        }
        browser.generation += 1;
        let generation = browser.generation;
        let senders = self.bus.senders.clone();
        let opening = state.agents.opening;
        crate::global_async_runtime::get_tokio_runtime().spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            senders
                .send_to_screen(yinz_bus::screen::ScreenInstruction::CommandPalette(
                    Instruction::HistoryTick {
                        client,
                        opening,
                        view,
                        generation,
                    },
                ))
                .non_fatal();
        });
    }

    fn refresh_palette_usage(&mut self, client: ClientId) {
        let Some(state) = self.command_palettes.get_mut(&client) else {
            return;
        };
        let view = state.widget.view;
        let Some(browser) = state.agents.browsers.get_mut(&view) else {
            return;
        };
        let Some(live) = &browser.live else {
            return;
        };
        if browser
            .usage_checked
            .is_some_and(|at| at.elapsed().as_secs() < 60)
        {
            return;
        }
        browser.usage_checked = Some(std::time::Instant::now());
        let Some(agent) = &live.pane.agent else {
            return;
        };
        let provider = agent.kind.replace('◆', "");
        let reason = live.usage_unavailable(&self.session_name);
        if let Some(reason) = reason {
            let usage = yinz_types::agent::control::ProviderUsage {
                provider,
                status: yinz_types::agent::control::ProviderUsageStatus::Unavailable,
                observed_at_ms: None,
                plan: None,
                windows: vec![],
                message: Some(reason.into()),
            };
            browser.usage = Some(usage.clone());
            state.widget.set_agent_usage(usage);
            return;
        }
        self.bus
            .senders
            .send_to_plugin(yinz_bus::plugins::PluginInstruction::PaletteAgent(
                Box::new(yinz_bus::command_palette::AgentRequest {
                    timezone: self
                        .client_timezones
                        .get(&client)
                        .cloned()
                        .unwrap_or_default(),
                    client,
                    opening: state.agents.opening,
                    step: view,
                    source: live.pane.clone(),
                    action: AgentAction::Usage { provider },
                    values: vec![],
                    active: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
                }),
            ))
            .context("requesting palette usage")
            .non_fatal();
    }

    pub(super) fn palette_usage_reply(
        &mut self,
        client: ClientId,
        opening: u64,
        view: u64,
        mut usage: yinz_types::agent::control::ProviderUsage,
    ) {
        let Some(state) = self
            .command_palettes
            .get_mut(&client)
            .filter(|s| s.agents.opening == opening)
        else {
            return;
        };
        let Some(browser) = state.agents.browsers.get_mut(&view) else {
            return;
        };
        if usage.status != yinz_types::agent::control::ProviderUsageStatus::Ok
            && let Some(old) = &browser.usage
        {
            usage.windows = old.windows.clone();
            usage.observed_at_ms = old.observed_at_ms;
        }
        browser.usage = Some(usage.clone());
        if state.widget.view == view {
            state.widget.set_agent_usage(usage);
        }
    }

    pub(super) fn refresh_palette_history(&mut self, client: ClientId, query_changed: bool) {
        let Some(state) = self.command_palettes.get_mut(&client) else {
            return;
        };
        if query_changed
            && state
                .agents
                .browsers
                .get(&state.widget.view)
                .is_some_and(|browser| {
                    matches!(
                        browser.action,
                        HistoryAction::Hit { .. } | HistoryAction::Event { .. }
                    )
                })
        {
            return;
        }
        state.agents.cancel();
        state.widget.busy = false;
        let Some(browser) = state.agents.browsers.get_mut(&state.widget.view) else {
            return;
        };
        if !query_changed {
            if browser.live.is_some() {
                let opening = state.agents.opening;
                let view = state.widget.view;
                let generation = browser.generation;
                self.palette_history_tick(client, opening, view, generation);
            } else if let Some(action) = browser.continuation.clone() {
                self.request_palette_agent(client, AgentAction::History(action), vec![]);
            }
            return;
        }
        state.widget.clear_error();
        if matches!(browser.action, HistoryAction::Workers { .. }) {
            let action = browser.action.clone();
            self.request_palette_agent(client, AgentAction::History(action), vec![]);
            return;
        }
        browser.rows.clear();
        state.widget.set_sections(vec![]);
        if state.widget.query().is_empty() {
            if let Some(action) = browser.search_return.take() {
                browser.action = action;
            }
        } else if let Some((id, _)) = browser.action.scope()
            && matches!(
                browser.action,
                HistoryAction::Conversation { .. } | HistoryAction::Tools { .. }
            )
        {
            let id = id.to_string();
            browser.search_return = Some(browser.action.clone());
            browser.action = HistoryAction::Run {
                id,
                from: None,
                query: state.widget.query().into(),
            };
        }
        match &mut browser.action {
            HistoryAction::Search(query) => {
                match AgentQuery::parse(state.widget.query(), ARCHIVE_FIELDS) {
                    Ok(parsed) => *query = parsed,
                    Err(error) => {
                        state.widget.set_error(error);
                        return;
                    },
                }
            },
            HistoryAction::Run { query, from, .. } => {
                *query = state.widget.query().into();
                *from = None;
            },
            HistoryAction::Conversation { .. } | HistoryAction::Tools { .. } => {},
            _ => return,
        }
        browser.continuation = None;
        let action = browser.action.clone();
        browser.rows.clear();
        state.widget.set_sections(vec![PluginSection {
            title: "Agents".into(),
            entries: vec![],
            semantic: true,
            status: Some("Loading…".into()),
        }]);
        self.request_palette_agent(client, AgentAction::History(action), vec![]);
    }

    pub(super) fn palette_history_tick(
        &mut self,
        client: ClientId,
        opening: u64,
        view: u64,
        generation: u64,
    ) {
        let live = self.live_palette_agents();
        let Some(state) = self.command_palettes.get_mut(&client).filter(|s| {
            s.agents.opening == opening && s.widget.view == view && !s.agents.pending()
        }) else {
            return;
        };
        let Some(browser) = state.agents.browsers.get_mut(&view) else {
            return;
        };
        if browser.generation != generation {
            return;
        }
        browser.generation += 1;
        let Some(old) = &browser.live else {
            return;
        };
        if let Some(current) = live.into_iter().find(|a| old.same(a)) {
            if current.run() != old.run()
                && let HistoryAction::Run { id, from, .. } = &mut browser.action
            {
                *id = current.run().unwrap_or_default();
                *from = None;
                browser.history = HistoryCache::default();
                browser.cached_history.clear();
                browser.continuation = None;
                browser.history_scope = None;
                state.widget.agent_retarget_run();
            }
            browser.live = Some(current);
        } else {
            state
                .widget
                .set_error("Agent disappeared or changed identity; Focus is unavailable".into());
            return;
        }
        let missing = browser.live.as_ref().is_some_and(|a| a.run().is_none());
        if missing {
            self.palette_history_reply(
                client,
                HistoryReply::Run {
                    header: HistoryHeader {
                        name: "Recorded history unavailable".into(),
                        details: "No recorded history".into(),
                        ..Default::default()
                    },
                    activity_ranges: vec![],
                    events: vec![],
                    rows: vec![],
                    status: "No recorded history".into(),
                },
                None,
            );
        } else {
            let mut action = browser.action.clone();
            let next = Some(browser.history.latest_received_revision.saturating_add(1));
            match &mut action {
                HistoryAction::Run { from, query, .. } if query.is_empty() => *from = next,
                HistoryAction::Conversation {
                    before: None,
                    after_revision,
                    ..
                } => *after_revision = next,
                _ => {},
            }
            let action = browser.continuation.clone().unwrap_or(action);
            self.request_palette_agent(client, AgentAction::History(action), vec![]);
        }
    }

    pub(in crate::screen) fn live_agent_focus_action(
        &self,
        target: &LiveAgent,
    ) -> Result<Action, &'static str> {
        if !self
            .live_palette_agents()
            .iter()
            .any(|a| target.same(a) && a.pane.is_selectable && !a.pane.is_suppressed)
        {
            return Err("Agent disappeared or changed identity; Focus is unavailable");
        }
        Ok(if target.session == self.session_name {
            Action::FocusPaneByPaneId {
                pane_id: target.pane.id,
            }
        } else {
            Action::SwitchSession {
                name: target.session.clone(),
                pane_id: Some(target.pane.id),
                tab_position: None,
                layout: None,
                cwd: None,
            }
        })
    }

    fn focus_palette_agent(&mut self, client: ClientId, target: LiveAgent) {
        let action = match self.live_agent_focus_action(&target) {
            Ok(action) => action,
            Err(message) => {
                if let Some(state) = self.command_palettes.get_mut(&client) {
                    super::notify(&self.bus.senders, message.into());
                    state.widget.set_error(message.into());
                }
                return;
            },
        };
        let Some(state) = self.command_palettes.get(&client) else {
            return;
        };
        let (pane, tab) = (state.context.pane_id, state.context.tab_id);
        self.close_command_palette(client);
        self.dispatch_command_palette_action(pane, tab, action, client, "Focus agent".into());
    }

    pub(super) fn open_agent_filters(&mut self, client: ClientId) {
        let Some(state) = self.command_palettes.get_mut(&client) else {
            return;
        };
        state.agents.cancel();
        let archive = state.agents.browsers.contains_key(&state.widget.view);
        let fields = if archive { ARCHIVE_FIELDS } else { LIVE_FIELDS };
        let query = match AgentQuery::parse(state.widget.query(), fields) {
            Ok(q) => q,
            Err(e) => {
                state.widget.set_error(e);
                return;
            },
        };
        state.agents.step += 1;
        state
            .widget
            .push_view((1 << 63) + state.agents.step, "Filters".into());
        state.widget.open_form(Form::new(
            if archive {
                "archive-filter-form"
            } else {
                "live-filter-form"
            }
            .into(),
            "Agent filters (blank removes filter)".into(),
            std::iter::once(Field::text("Search", &query.text))
                .chain(fields.iter().map(|key| {
                    Field::text(
                        key,
                        query.filters.get(*key).map(String::as_str).unwrap_or(""),
                    )
                }))
                .collect(),
            "Apply",
            false,
        ));
    }

    pub(super) fn history_palette_outcome(&mut self, client: ClientId, outcome: &Outcome) -> bool {
        let Some(state) = self.command_palettes.get_mut(&client) else {
            return false;
        };
        if let Outcome::Submit { id, values } = outcome
            && (id == "archive-filter-form" || id == "live-filter-form")
        {
            let fields = if id == "archive-filter-form" {
                ARCHIVE_FIELDS
            } else {
                LIVE_FIELDS
            };
            let query = AgentQuery {
                text: values.first().cloned().unwrap_or_default(),
                filters: fields
                    .iter()
                    .zip(values.iter().skip(1))
                    .filter(|(_, v)| !v.is_empty())
                    .map(|(k, v)| ((*k).into(), v.clone()))
                    .collect(),
            };
            if let Err(error) = AgentQuery::parse(&query.tokens(), fields) {
                state.widget.set_error(error);
                return true;
            }
            state.widget.back();
            state.widget.set_query(query.tokens());
            self.refresh_palette_history(client, true);
            self.refresh_plugin_palette(client, false);
            return true;
        }
        if let Outcome::FocusAgent(id) = outcome {
            if let Some(Builtin::Inspect(agent)) = state.actions.get(id) {
                let agent = agent.clone();
                self.focus_palette_agent(client, agent);
            }
            return true;
        }
        if let Outcome::Submit { id, values } = outcome
            && id == "history-prompt"
        {
            if let Some(action) = state
                .agents
                .browsers
                .get_mut(&state.widget.view)
                .and_then(|browser| browser.prompt_action(&self.session_name, values))
            {
                // A background history request may still be pending while composing.
                state.agents.cancel();
                self.request_palette_agent(client, action, values.clone());
            }
            return true;
        }
        let Outcome::Activate(id) = outcome else {
            return false;
        };
        if id == "history-refresh" {
            self.refresh_palette_history(client, true);
            return true;
        }
        if id == PROMPT_LIBRARY_ACTION {
            self.open_prompt_picker(client);
            return true;
        }
        if let Some(index) = id.strip_prefix(prompt_picker::ENTRY_PREFIX) {
            return self.accept_prompt_picker(client, index);
        }
        if id == "history-filters" {
            self.open_agent_filters(client);
            return true;
        }
        let Some(browser) = state.agents.browsers.get_mut(&state.widget.view) else {
            return false;
        };
        if id == "history-focus" {
            if let Some(live) = browser.live.clone() {
                self.focus_palette_agent(client, live);
            }
            return true;
        }
        let Some(action) = browser.rows.get(id).cloned() else {
            return false;
        };
        match &action {
            HistoryAction::Resume { .. } => {
                state.agents.cancel();
                self.request_palette_agent(client, AgentAction::History(action), vec![])
            },
            HistoryAction::Run { id, .. } if matches!(&browser.action, HistoryAction::Run { id: current, .. } if current == id) =>
            {
                state
                    .widget
                    .agent_page_request(matches!(action, HistoryAction::Run { from: None, .. }));
                browser.action = action.clone();
                state.widget.set_query(String::new());
                state.widget.clear_error();
                state.agents.cancel();
                self.request_palette_agent(client, AgentAction::History(action), vec![]);
            },
            HistoryAction::Conversation { .. } | HistoryAction::Tools { .. }
                if action
                    .scope()
                    .as_ref()
                    .map(|(id, q)| (id.to_string(), q.clone()))
                    == browser.history_scope =>
            {
                let latest = matches!(
                    action,
                    HistoryAction::Conversation { before: None, .. }
                        | HistoryAction::Tools { before: None, .. }
                );
                state.widget.agent_page_request(latest);
                if latest {
                    browser.action = action.clone();
                }
                state.widget.clear_error();
                state.agents.cancel();
                self.request_palette_agent(client, AgentAction::History(action), vec![]);
            },
            _ => {
                let live = browser.live.clone().filter(|live| match &action {
                    HistoryAction::Search(_) => false,
                    HistoryAction::Run { id, .. }
                    | HistoryAction::Conversation { id, .. }
                    | HistoryAction::Workers { id }
                    | HistoryAction::Tools { id, .. }
                    | HistoryAction::Hit { id, .. }
                    | HistoryAction::Event { id, .. }
                    | HistoryAction::Resume { id } => live.run().as_ref() == Some(id),
                });
                self.open_palette_history(client, live, Some(action));
            },
        }
        true
    }
}

#[cfg(test)]
#[path = "../unit/palette_history.rs"]
mod tests;
