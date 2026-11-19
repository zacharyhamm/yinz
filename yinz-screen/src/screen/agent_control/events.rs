//! Session-local, ordered lifecycle facts. Socket I/O remains in the client writers.
use super::host::AgentHost;
use crate::ClientId;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, Instant};
use yinz_ipc::ipc::EventSubscription;
use yinz_types::agent::control::{AgentOutcome, PromptObservation};
use yinz_types::agent::{AgentEvent, HookEvent};
use yinz_types::pane_id::PaneId;

use yinz_agent_control::journal::{canonicalize, EventJournal, EVENTS};

use yinz_types::agent::tracking::{EventTrackingHandoff, ObservedAgentTurn as Turn};

pub(in crate::screen) struct AgentEvents {
    pub(in crate::screen) dirty: bool,
    journal: EventJournal,
    generation: u64,
    panes: BTreeMap<PaneId, Value>,
    turns: HashMap<PaneId, VecDeque<Turn>>,
    uncertain: HashMap<(PaneId, Option<String>), AgentOutcome>,
    project_pending: HashMap<ClientId, EventSubscription>,
    projects: Vec<String>,
    project_input: Value,
    project_version: u64,
    project_inflight: Option<u64>,
    project_ready: bool,
    project_retry: Option<Instant>,
}

impl Default for AgentEvents {
    fn default() -> Self {
        Self {
            dirty: true,
            journal: EventJournal::default(),
            generation: 0,
            panes: BTreeMap::new(),
            turns: HashMap::new(),
            uncertain: HashMap::new(),
            project_pending: HashMap::new(),
            projects: Vec::new(),
            project_input: Value::Null,
            project_version: 0,
            project_inflight: None,
            project_ready: false,
            project_retry: None,
        }
    }
}

impl AgentEvents {
    pub(in crate::screen) fn remove(&mut self, client: ClientId) {
        self.journal.remove(client);
        self.project_pending.remove(&client);
        self.prune_projects();
    }

    fn prune_projects(&mut self) {
        self.projects.retain(|project| {
            self.project_pending
                .values()
                .any(|s| s.projects.contains(project))
                || self
                    .journal
                    .subscribers_for_project(project)
                    .next()
                    .is_some()
        });
        if self.projects.is_empty() {
            self.project_retry = None;
        }
    }

    pub(in crate::screen) fn deadline(&self) -> Option<Duration> {
        self.journal
            .has_pending()
            .then_some(Duration::from_millis(10))
            .into_iter()
            .chain(
                self.project_retry
                    .map(|at| at.saturating_duration_since(Instant::now())),
            )
            .min()
    }
}

impl super::AgentController {
    pub(in crate::screen) fn invalidate_agent_event_projects(&mut self, host: &impl AgentHost) {
        self.agent_events.project_version += 1;
        self.agent_events.project_ready = false;
        self.refresh_agent_event_projects(host);
    }
}

impl super::AgentController {
    pub(in crate::screen) fn agent_event_tracking(&self, pane: PaneId) -> EventTrackingHandoff {
        let state = self.agent_events.panes.get(&pane);
        let mut uncertain: Vec<_> = self
            .agent_events
            .uncertain
            .iter()
            .filter(|((id, _), _)| *id == pane)
            .map(|((_, submission), outcome)| (submission.clone(), *outcome))
            .collect();
        uncertain.sort_by(|a, b| a.0.cmp(&b.0));
        EventTrackingHandoff {
            turns: self
                .agent_events
                .turns
                .get(&pane)
                .cloned()
                .unwrap_or_default(),
            uncertain,
            submission_id: state
                .and_then(|s| s["submission_id"].as_str())
                .map(str::to_owned),
            turn_id: state.and_then(|s| s["turn_id"].as_str()).map(str::to_owned),
        }
    }
}

impl super::AgentController {
    pub(in crate::screen) fn refresh_agent_events(&mut self, host: &impl AgentHost) {
        self.agent_events.dirty = false;
        let current: BTreeMap<_, _> = host.pane_ids().into_iter()
            .filter(|pane| matches!(pane, PaneId::Terminal(_)))
            .map(|pane| {
                let agent = host.pane_agent(pane)
                    .or_else(|| self.detected_agent_status(host, pane).map(|status| status.agent));
                let background = self.agent_lifecycle
                    .get(&pane)
                    .is_some_and(|l| l.event_background());
                (
                    pane,
                    json!({"pane_id":pane.to_string(),"kind":agent.as_ref().map(|a| &a.kind),
                    "conversation_id":agent.as_ref().and_then(|a| a.session_id.as_ref()),
                    "run_id":agent.as_ref().and_then(|a| a.run_id.as_ref()),
                    "source":agent.as_ref().map(|a| a.source.to_string()),
                    "control":self.agent_status(host, pane, None).map(|s| json!({"ready":s.ready,"prompt_supported":s.prompt_supported,"status_available":s.status_available,"not_ready_reason":s.not_ready_reason})),
                    "state":agent.as_ref().map(|a| a.state.to_string().to_lowercase()),
                    "needs_input":agent.as_ref().is_some_and(|a| a.is_waiting_on_a_person()),
                    "submission_id":null,"turn_id":null,"background":background,"projects":[]}),
                )
            })
            .collect();
        let closed: Vec<_> = self
            .agent_events
            .panes
            .keys()
            .filter(|p| !current.contains_key(p))
            .copied()
            .collect();
        for pane in closed {
            self.retire_agent_event_pane(host, pane, "pane_closed");
        }
        for (pane, mut state) in current {
            let previous = self.agent_events.panes.get(&pane).cloned();
            if let Some(previous) = &previous {
                for key in [
                    "pane_generation",
                    "pane_sequence",
                    "projects",
                    "submission_id",
                    "turn_id",
                ] {
                    state[key] = previous[key].clone();
                }
                if previous == &state {
                    continue;
                }
                if previous["kind"] != state["kind"]
                    || previous["conversation_id"] != state["conversation_id"]
                {
                    self.publish_agent_event(host, pane, "conversation_replaced", json!({"next_kind":state["kind"],"next_conversation_id":state["conversation_id"]}));
                    self.agent_events.turns.remove(&pane);
                    state["submission_id"] = Value::Null;
                    state["turn_id"] = Value::Null;
                    state["pane_sequence"] =
                        self.agent_events.panes[&pane]["pane_sequence"].clone();
                }
            } else {
                self.agent_events.generation += 1;
                state["pane_generation"] = self.agent_events.generation.into();
                state["pane_sequence"] = 0.into();
            }
            self.agent_events.panes.insert(pane, state.clone());
            self.publish_agent_event(
                host,
                pane,
                if previous.is_none() {
                    "pane_spawned"
                } else {
                    "state_changed"
                },
                json!({}),
            );
            if state["needs_input"] == true
                && previous.as_ref().is_none_or(|p| p["needs_input"] != true)
            {
                self.publish_agent_event(host, pane, "needs_input", json!({}));
            }
            if previous
                .as_ref()
                .is_some_and(|p| p["background"] != state["background"])
            {
                self.publish_agent_event(host, pane, "background_changed", json!({}));
            }
        }
    }

    pub(in crate::screen) fn refresh_agent_event_projects(&mut self, host: &impl AgentHost) {
        if self.agent_events.projects.is_empty() {
            return;
        }
        let mut agents: Vec<_> = host
            .pane_ids()
            .into_iter()
            .filter_map(|pane| {
                self.agent_status(host, pane, None)
                    .or_else(|| self.detected_agent_status(host, pane))
            })
            .collect();
        agents.sort_by_key(|agent| agent.pane_id);
        let input = json!({"projects":self.agent_events.projects,"panes":agents.iter().map(|s| json!({
            "pane":s.pane_id.to_string(),"tab":s.tab_id,"cwd":s.cwd,"target":s.target,
            "generation":self.agent_events.panes.get(&s.pane_id).map(|p| &p["pane_generation"])
        })).collect::<Vec<_>>()});
        if input != self.agent_events.project_input {
            self.agent_events.project_input = input;
            self.agent_events.project_version += 1;
            self.agent_events.project_ready = false;
        }
        if !self.agent_events.project_ready
            && self.agent_events.project_inflight.is_none()
            && self.agent_events.project_retry.is_none()
        {
            let version = self.agent_events.project_version;
            self.agent_events.project_inflight = Some(version);
            host.resolve_event_projects(version, agents, self.agent_events.projects.clone());
        }
    }

    pub(in crate::screen) fn publish_agent_event(
        &mut self,
        host: &impl AgentHost,
        pane: PaneId,
        event_type: &str,
        extra: Value,
    ) {
        self.refresh_agent_event_projects(host);
        let events = &mut self.agent_events;
        let Some(state) = events.panes.get_mut(&pane) else {
            return;
        };
        let pane_sequence = state["pane_sequence"].as_u64().unwrap_or(0) + 1;
        state["pane_sequence"] = pane_sequence.into();
        let mut event = state.clone();
        for (key, value) in extra.as_object().into_iter().flatten() {
            event[key] = value.clone();
        }
        let projects = if events.project_ready {
            events.projects.as_slice()
        } else {
            &[]
        };
        events
            .journal
            .publish(host, pane, event_type, pane_sequence, event, projects);
    }

    pub(in crate::screen) fn retire_agent_event_pane(
        &mut self,
        host: &impl AgentHost,
        pane: PaneId,
        event_type: &str,
    ) {
        self.publish_agent_event(host, pane, event_type, json!({}));
        self.agent_events.panes.remove(&pane);
        self.agent_events.turns.remove(&pane);
        self.agent_events.uncertain.retain(|(id, _), _| *id != pane);
        self.agent_events.dirty = true;
    }

    pub(in crate::screen) fn agent_observation_uncertain(
        &mut self,
        host: &impl AgentHost,
        pane: PaneId,
        submission: Option<String>,
        outcome: AgentOutcome,
    ) {
        if self.agent_events.uncertain.get(&(pane, submission.clone())) == Some(&outcome) {
            return;
        }
        if self.agent_events.uncertain.len() >= EVENTS {
            self.agent_events.uncertain.clear();
        }
        self.agent_events
            .uncertain
            .insert((pane, submission.clone()), outcome);
        let turn = submission.as_ref().and_then(|submission| {
            self.agent_events
                .turns
                .get(&pane)?
                .iter()
                .find(|turn| turn.submission.as_ref() == Some(submission))
        });
        let extra = json!({
            "submission_id":submission,"reason":outcome,
            "turn_id":turn.and_then(|turn| turn.identity.0.as_ref()),
            "provider_prompt_id":turn.and_then(|turn| turn.identity.1.as_ref())
        });
        self.publish_agent_event(host, pane, "observation_uncertain", extra);
    }
}

impl super::AgentController {
    pub(in crate::screen) fn restore_agent_event_tracking(
        &mut self,
        host: &impl AgentHost,
        pane: PaneId,
        events: EventTrackingHandoff,
    ) {
        // Establish destination identity first; refreshing a changed conversation retires
        // its old observations. Session epochs and event cursors are never imported.
        self.refresh_agent_events(host);
        self.agent_events.turns.insert(pane, events.turns);
        self.agent_events.uncertain.extend(
            events
                .uncertain
                .into_iter()
                .map(|(id, outcome)| ((pane, id), outcome)),
        );
        if let Some(state) = self.agent_events.panes.get_mut(&pane) {
            state["submission_id"] = json!(events.submission_id);
            state["turn_id"] = json!(events.turn_id);
        }
    }

    pub(in crate::screen) fn subscribe_agent_events(
        &mut self,
        host: &impl AgentHost,
        client: ClientId,
        mut options: EventSubscription,
    ) {
        self.agent_events.remove(client);
        self.refresh_agent_events(host);
        if !options.projects.is_empty() {
            if let Err(error) = canonicalize(&mut options) {
                host.subscription_error(client, format!("{error:#}"));
                return;
            }
            self.agent_events
                .projects
                .extend(options.projects.iter().cloned());
            self.agent_events.projects.sort();
            self.agent_events.projects.dedup();
            self.agent_events.project_version += 1;
            self.agent_events.project_ready = false;
            self.agent_events.project_pending.insert(client, options);
            self.refresh_agent_event_projects(host);
            return;
        }
        self.start_agent_events(host, client, options);
    }

    fn start_agent_events(
        &mut self,
        host: &impl AgentHost,
        client: ClientId,
        options: EventSubscription,
    ) {
        self.agent_events.journal.subscribe(
            host,
            client,
            options,
            self.agent_events.panes.values(),
        );
    }

    pub(in crate::screen) fn flush_agent_events(&mut self, host: &impl AgentHost) {
        self.agent_events.journal.flush(host);
    }

    pub(in crate::screen) fn observe_agent_event(
        &mut self,
        host: &impl AgentHost,
        event: &AgentEvent,
        hook: Option<&HookEvent>,
        submission: Option<String>,
    ) {
        let provider = super::providers::Provider::for_kind(&event.kind);
        // A new acceptance supplies correlation before its busy-state edge is published.
        // Otherwise that edge would inherit the previous turn's submission ID.
        if let Some(hook) =
            hook.filter(|h| !h.is_worker() && h.hook_event_name == "UserPromptSubmit")
        {
            let pane = PaneId::Terminal(event.pane_id);
            let primary = provider.turn_key(&hook.turn_id, &hook.prompt_id);
            let repeated = self.agent_events.turns.get(&pane).is_some_and(|turns| {
                turns
                    .iter()
                    .any(|turn| provider.turn_key(&turn.identity.0, &turn.identity.1) == primary)
            });
            if !repeated
                && primary.as_deref().is_some_and(|id| !id.is_empty())
                && let Some(state) = self.agent_events.panes.get_mut(&pane).filter(|state| {
                    state["kind"] == event.kind && state["conversation_id"] == hook.session_id
                })
            {
                state["submission_id"] = json!(submission);
                state["turn_id"] = json!(hook.turn_id.as_deref().filter(|id| !id.is_empty()));
            }
        }
        self.refresh_agent_events(host);
        self.refresh_agent_event_projects(host);
        let Some(hook) = hook.filter(|h| !h.is_worker() && !h.session_id.is_empty()) else {
            return;
        };
        let pane = PaneId::Terminal(event.pane_id);
        let identity = (
            hook.turn_id.clone().filter(|id| !id.is_empty()),
            hook.prompt_id.clone().filter(|id| !id.is_empty()),
        );
        let key = provider.turn_key(&identity.0, &identity.1);
        let has_identity = key.is_some();
        if hook.hook_event_name == "UserPromptSubmit" && !has_identity {
            self.agent_observation_uncertain(
                host,
                pane,
                submission,
                AgentOutcome::StatusUnavailable,
            );
            return;
        }
        let turns = self.agent_events.turns.entry(pane).or_default();
        let same_turn = |turn: &Turn| provider.turn_key(&turn.identity.0, &turn.identity.1) == key;
        if hook.hook_event_name == "UserPromptSubmit" {
            if turns.iter().any(same_turn) {
                return;
            }
            turns.push_back(Turn {
                identity,
                observation: PromptObservation::from_accepted_hook(&event.kind, hook),
                submission: submission.clone(),
                last: None,
                ended: false,
            });
            // ponytail: reuse the submission-history window; increase its shared limit if
            // more concurrent background turns must remain correlated.
            let evicted = (turns.len() > super::SUBMISSION_HISTORY_LIMIT)
                .then(|| turns.pop_front())
                .flatten()
                .filter(|turn| !turn.ended);
            if let Some(evicted) = evicted {
                self.publish_agent_event(
                    host,
                    pane,
                    "observation_uncertain",
                    json!({
                        "submission_id":evicted.submission,"turn_id":evicted.identity.0,
                        "provider_prompt_id":evicted.identity.1,"reason":"turn_tracking_limit"
                    }),
                );
            }
            self.agent_events
                .uncertain
                .remove(&(pane, submission.clone()));
            if let Some(state) = self.agent_events.panes.get_mut(&pane) {
                state["submission_id"] = json!(submission);
                state["turn_id"] = json!(hook.turn_id.as_deref().filter(|id| !id.is_empty()));
            }
            self.publish_agent_event(host, pane, "submission_acknowledged", json!({"submission_id":submission,"turn_id":hook.turn_id.as_deref().filter(|id| !id.is_empty()),"provider_prompt_id":hook.prompt_id.as_deref().filter(|id| !id.is_empty())}));
            return;
        }
        let mut facts = Vec::new();
        for turn in turns {
            if has_identity && !same_turn(turn) {
                continue;
            }
            if turn.ended {
                continue;
            }
            let Some(outcome) = turn.observation.observe(event, Some(hook)) else {
                continue;
            };
            let event_type = match outcome {
                AgentOutcome::Completed => "turn_completed",
                AgentOutcome::Failed => "turn_failed",
                AgentOutcome::Interrupted if hook.hook_event_name == "Interrupt" => {
                    "turn_interrupted"
                },
                AgentOutcome::StatusUnavailable
                | AgentOutcome::Interrupted
                | AgentOutcome::TargetChanged => "observation_uncertain",
                _ => continue,
            };
            if turn.last == Some(outcome) {
                continue;
            }
            turn.last = Some(outcome);
            turn.ended = matches!(
                outcome,
                AgentOutcome::Completed | AgentOutcome::Failed | AgentOutcome::TargetChanged
            ) || event_type == "turn_interrupted";
            facts.push((event_type, json!({"submission_id":turn.submission,"turn_id":turn.identity.0,"provider_prompt_id":turn.identity.1,"outcome":outcome})));
        }
        for (event_type, extra) in facts {
            self.publish_agent_event(host, pane, event_type, extra);
        }
    }

    pub(in crate::screen) fn retry_agent_event_projects(
        &mut self,
        host: &impl AgentHost,
        now: Instant,
    ) {
        self.agent_events.prune_projects();
        if self.agent_events.project_retry.is_some_and(|at| now >= at) {
            self.agent_events.project_retry = None;
            self.refresh_agent_event_projects(host);
        }
    }

    pub(in crate::screen) fn apply_agent_event_projects(
        &mut self,
        host: &impl AgentHost,
        version: u64,
        result: yinz_bus::screen::AgentEventProjectMemberships,
    ) {
        self.refresh_agent_events(host);
        self.refresh_agent_event_projects(host);
        if self.agent_events.project_inflight != Some(version) {
            return;
        }
        self.agent_events.project_inflight = None;
        if version != self.agent_events.project_version {
            self.refresh_agent_event_projects(host);
            return;
        }
        let mut pending = Vec::new();
        let mut memberships: HashMap<PaneId, Vec<String>> = HashMap::new();
        for (project, result) in result {
            match result {
                Ok(None) => pending.push(project),
                Ok(Some(panes)) => {
                    for pane in panes {
                        memberships.entry(pane).or_default().push(project.clone());
                    }
                },
                Err(error) => {
                    // One invalid project must not poison other subscriptions or future probes.
                    self.agent_events.projects.retain(|name| name != &project);
                    let clients: Vec<_> = self
                        .agent_events
                        .project_pending
                        .iter()
                        .filter(|(_, options)| options.projects.contains(&project))
                        .map(|(c, _)| *c)
                        .chain(self.agent_events.journal.subscribers_for_project(&project))
                        .collect();
                    for client in clients {
                        self.agent_events.remove(client);
                        host.subscription_error(client, error.clone());
                    }
                },
            }
        }
        self.agent_events.prune_projects();
        self.agent_events.project_input["projects"] = json!(self.agent_events.projects);
        if pending
            .iter()
            .any(|project| self.agent_events.projects.contains(project))
        {
            self.agent_events.project_ready = false;
            self.agent_events.project_retry = Some(Instant::now() + Duration::from_millis(100));
            return;
        }
        self.agent_events.project_ready = true;
        self.agent_events.project_retry = None;
        let panes: Vec<_> = self.agent_events.panes.keys().copied().collect();
        for pane in panes {
            let projects = json!(memberships.get(&pane).cloned().unwrap_or_default());
            if self.agent_events.panes[&pane]["projects"] != projects {
                let previous = self.agent_events.panes[&pane]["projects"].clone();
                if let Some(state) = self.agent_events.panes.get_mut(&pane) {
                    state["projects"] = projects;
                }
                self.publish_agent_event(
                    host,
                    pane,
                    "project_membership_changed",
                    json!({"previous_projects":previous}),
                );
            }
        }
        for (client, options) in std::mem::take(&mut self.agent_events.project_pending) {
            self.start_agent_events(host, client, options);
        }
    }
}

impl super::AgentController {
    pub(in crate::screen) fn event_snapshot_dirty(&self) -> bool {
        self.agent_events.dirty
    }
    pub(in crate::screen) fn invalidate_event_snapshot(&mut self) {
        self.agent_events.dirty = true;
    }
    pub(in crate::screen) fn remove_subscriber(&mut self, client: ClientId) {
        self.agent_events.remove(client);
    }
}
