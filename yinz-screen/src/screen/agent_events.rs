//! Session-local, ordered lifecycle facts. Socket I/O remains in the client writers.
use super::Screen;
use crate::{ClientId, ServerInstruction};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use yinz_ipc::ipc::{EventSubscription, ServerToClientMsg};
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::control::{AgentOutcome, PromptObservation};
use yinz_types::agent::{AgentEvent, HookEvent};
use yinz_types::pane_id::PaneId;

const EVENTS: usize = 4096;
const BYTES: usize = 8 * 1024 * 1024;
const PAYLOAD_BYTES: usize = 64 * 1024;
const BATCH: usize = 32;

struct Subscriber {
    filters: EventSubscription,
    pending: VecDeque<Value>,
    bytes: usize,
}

struct Turn {
    identity: (Option<String>, Option<String>),
    observation: PromptObservation,
    submission: Option<String>,
    last: Option<AgentOutcome>,
    ended: bool,
}

pub(super) struct AgentEvents {
    pub(super) dirty: bool,
    epoch: String,
    sequence: u64,
    generation: u64,
    panes: BTreeMap<PaneId, Value>,
    turns: HashMap<PaneId, VecDeque<Turn>>,
    history: VecDeque<Value>,
    bytes: usize,
    subscribers: HashMap<ClientId, Subscriber>,
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
            epoch: uuid::Uuid::new_v4().to_string(),
            sequence: 0,
            generation: 0,
            panes: BTreeMap::new(),
            turns: HashMap::new(),
            history: VecDeque::new(),
            bytes: 0,
            subscribers: HashMap::new(),
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

pub(super) fn canonicalize(options: &mut EventSubscription) -> Result<()> {
    for values in [
        &mut options.panes,
        &mut options.projects,
        &mut options.kinds,
        &mut options.lifecycle_types,
    ] {
        if values.len() > 128 || values.iter().any(|v| v.len() > 1024 || v.is_empty()) {
            bail!("lifecycle filters are limited to 128 nonempty values of 1024 bytes");
        }
        values.sort();
        values.dedup();
    }
    for pane in &mut options.panes {
        *pane = pane
            .parse::<PaneId>()
            .map_err(|error| anyhow!(error.to_string()))
            .context("invalid lifecycle pane filter")?
            .to_string();
    }
    options.panes.sort();
    options.panes.dedup();
    let mut filters = options.clone();
    filters.cursor = None;
    if serde_json::to_string(&filters)
        .context("encode lifecycle filters")?
        .len()
        > 4096
    {
        bail!("lifecycle filters exceed 4096 bytes");
    }
    Ok(())
}

fn matches_identity(filters: &EventSubscription, event: &Value) -> bool {
    [&filters.panes, &filters.kinds, &filters.lifecycle_types]
        .into_iter()
        .zip(["pane_id", "kind", "event_type"])
        .all(|(values, key)| {
            values.is_empty()
                || event[key]
                    .as_str()
                    .is_some_and(|v| values.iter().any(|wanted| wanted == v))
        })
}

fn matches(filters: &EventSubscription, event: &Value) -> bool {
    matches_identity(filters, event)
        && (filters.projects.is_empty()
            || ["projects", "previous_projects"].iter().any(|key| {
                event[key].as_array().is_some_and(|projects| {
                    projects.iter().any(|p| {
                        p.as_str()
                            .is_some_and(|p| filters.projects.iter().any(|wanted| wanted == p))
                    })
                })
            }))
}

// Use the fact's recorded probe coverage, including during replay after a probe completes.
fn filtered_fact(filters: &EventSubscription, event: &Value) -> Option<Value> {
    if event["reason"] == "payload_limit" {
        return Some(event.clone());
    }
    if !matches_identity(filters, event) {
        return None;
    }
    if filters.projects.iter().any(|project| {
        !event["projects_resolved_for"]
            .as_array()
            .is_some_and(|resolved| resolved.iter().any(|value| value == project))
    }) {
        let mut uncertain = event.clone();
        uncertain["observed_event_type"] = event["event_type"].clone();
        uncertain["event_type"] = "observation_uncertain".into();
        uncertain["reason"] = "project_membership_unresolved".into();
        return Some(uncertain);
    }
    matches(filters, event).then(|| event.clone())
}

impl AgentEvents {
    fn cursor(
        &self,
        host: &Value,
        session: &str,
        sequence: u64,
        filters: &EventSubscription,
    ) -> String {
        json!({"v":1,"host":host,"session":session,"epoch":self.epoch,"sequence":sequence,"filters":filters}).to_string()
    }

    fn decorate(
        &self,
        mut value: Value,
        host: &Value,
        session: &str,
        filters: &EventSubscription,
    ) -> Value {
        let sequence = value["sequence"].as_u64().unwrap_or(self.sequence);
        value["cursor"] = self.cursor(host, session, sequence, filters).into();
        value
    }

    fn control(
        &self,
        event_type: &str,
        host: &Value,
        session: &str,
        filters: &EventSubscription,
    ) -> Value {
        self.decorate(json!({"schema_version":1,"event_type":event_type,
            "host_id":host,"session":session,"server_epoch":self.epoch,"sequence":self.sequence,
            "observed_at_ms":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),
            "pane_id":null,"pane_generation":null,"pane_sequence":null,"kind":null,
            "conversation_id":null,"submission_id":null,"turn_id":null,"run_id":null}), host, session, filters)
    }

    pub(super) fn remove(&mut self, client: ClientId) {
        self.subscribers.remove(&client);
        self.project_pending.remove(&client);
        self.prune_projects();
    }

    fn prune_projects(&mut self) {
        self.projects.retain(|project| {
            self.project_pending
                .values()
                .any(|s| s.projects.contains(project))
                || self
                    .subscribers
                    .values()
                    .any(|s| s.filters.projects.contains(project))
        });
        if self.projects.is_empty() {
            self.project_retry = None;
        }
    }

    pub(super) fn deadline(&self) -> Option<Duration> {
        self.subscribers
            .values()
            .any(|s| !s.pending.is_empty())
            .then_some(Duration::from_millis(10))
            .into_iter()
            .chain(
                self.project_retry
                    .map(|at| at.saturating_duration_since(Instant::now())),
            )
            .min()
    }
}

impl Screen {
    fn lifecycle_owner(&self) -> (Value, String) {
        (
            json!(yinz_runtime::host_identity::host_id()),
            self.session_name.to_string(),
        )
    }

    pub(super) fn subscribe_agent_events(
        &mut self,
        client: ClientId,
        mut options: EventSubscription,
    ) {
        self.agent_events.remove(client);
        self.refresh_agent_events();
        if !options.projects.is_empty() {
            if let Err(error) = canonicalize(&mut options) {
                self.bus
                    .senders
                    .send_to_server(ServerInstruction::LogError(
                        vec![format!("{error:#}")],
                        client,
                        None,
                    ))
                    .non_fatal();
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
            self.refresh_agent_event_projects();
            return;
        }
        self.start_agent_events(client, options);
    }

    fn start_agent_events(&mut self, client: ClientId, mut options: EventSubscription) {
        let (host, session) = self.lifecycle_owner();
        let result = (|| -> Result<Subscriber> {
            canonicalize(&mut options)?;
            let cursor = options.cursor.take();
            let events = &self.agent_events;
            let mut pending = VecDeque::new();
            let mut fresh = true;
            if let Some(cursor) = cursor {
                if cursor.len() > PAYLOAD_BYTES {
                    bail!("lifecycle cursor is too large");
                }
                let cursor: Value =
                    serde_json::from_str(&cursor).context("invalid lifecycle cursor")?;
                if cursor["v"] != 1
                    || cursor["host"] != host
                    || cursor["session"] != session
                    || cursor["filters"] != json!(options)
                {
                    bail!("lifecycle cursor owner or filters do not match this subscription");
                }
                let sequence = cursor["sequence"]
                    .as_u64()
                    .context("invalid lifecycle cursor sequence")?;
                if cursor["epoch"] == events.epoch {
                    if sequence > events.sequence {
                        bail!("lifecycle cursor is in the future");
                    }
                    let oldest = events
                        .history
                        .front()
                        .and_then(|v| v["sequence"].as_u64())
                        .unwrap_or(events.sequence + 1);
                    if sequence + 1 >= oldest {
                        fresh = false;
                        for event in events
                            .history
                            .iter()
                            .filter(|v| v["sequence"].as_u64().is_some_and(|s| s > sequence))
                        {
                            if let Some(event) = filtered_fact(&options, event) {
                                pending
                                    .push_back(events.decorate(event, &host, &session, &options));
                            }
                        }
                    }
                } else if !cursor["epoch"].is_string() {
                    bail!("invalid lifecycle cursor epoch");
                }
                if fresh {
                    let mut gap = events.control("gap", &host, &session, &options);
                    gap["reason"] = if cursor["epoch"] == events.epoch {
                        "history_expired"
                    } else {
                        "server_restarted"
                    }
                    .into();
                    pending.push_back(gap);
                }
            }
            if fresh {
                pending.push_back(events.control("snapshot_begin", &host, &session, &options));
                for pane in events.panes.values() {
                    // Lifecycle filters apply to live facts, not the initial state needed to interpret them.
                    let mut snapshot_filters = options.clone();
                    snapshot_filters.lifecycle_types.clear();
                    if !pane["kind"].is_null() && matches(&snapshot_filters, pane) {
                        let mut snapshot = events.control("snapshot", &host, &session, &options);
                        for (key, value) in pane.as_object().into_iter().flatten() {
                            snapshot[key] = value.clone();
                        }
                        snapshot["event_type"] = "snapshot".into();
                        if snapshot.to_string().len() > PAYLOAD_BYTES {
                            bail!("lifecycle snapshot payload exceeds 64 KiB");
                        }
                        pending.push_back(snapshot);
                    }
                }
            }
            pending.push_back(events.control("ready", &host, &session, &options));
            let bytes = pending.iter().map(|v| v.to_string().len()).sum();
            if bytes > BYTES || pending.len() > EVENTS + 3 {
                bail!("lifecycle startup exceeds subscription buffer; narrow the filters");
            }
            Ok(Subscriber {
                filters: options,
                pending,
                bytes,
            })
        })();
        match result {
            Ok(subscriber) => {
                self.agent_events.subscribers.insert(client, subscriber);
            },
            Err(error) => {
                self.bus
                    .senders
                    .send_to_server(ServerInstruction::LogError(
                        vec![format!("{error:#}")],
                        client,
                        None,
                    ))
                    .non_fatal();
            },
        }
        self.flush_agent_events();
    }

    pub(super) fn publish_agent_event(&mut self, pane: PaneId, event_type: &str, extra: Value) {
        self.refresh_agent_event_projects();
        let (host, session) = self.lifecycle_owner();
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
        events.sequence += 1;
        event["schema_version"] = 1.into();
        event["event_type"] = event_type.into();
        event["sequence"] = events.sequence.into();
        event["host_id"] = host.clone();
        event["session"] = session.clone().into();
        event["server_epoch"] = events.epoch.clone().into();
        event["observed_at_ms"] = json!(SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis());
        event["projects_resolved_for"] = if events.project_ready {
            json!(events.projects)
        } else {
            json!([])
        };
        if event.to_string().len() > PAYLOAD_BYTES / 2 {
            // Keep loss explicit and resumable; never truncate an identity into a different one.
            event = json!({"schema_version":1,"event_type":"observation_uncertain","reason":"payload_limit",
                "sequence":events.sequence,"pane_id":pane.to_string(),"pane_generation":event["pane_generation"],
                "pane_sequence":pane_sequence,"host_id":host,"session":session,"server_epoch":events.epoch,
                "projects_resolved_for":[],"observed_at_ms":event["observed_at_ms"],"kind":null,"conversation_id":null,"submission_id":null,"turn_id":null,"run_id":null});
        }
        let size = event.to_string().len();
        events.bytes += size;
        events.history.push_back(event.clone());
        while events.history.len() > EVENTS || events.bytes > BYTES {
            if let Some(old) = events.history.pop_front() {
                events.bytes -= old.to_string().len();
            }
        }
        let clients: Vec<_> = events.subscribers.keys().copied().collect();
        for client in clients {
            let filters = &events.subscribers[&client].filters;
            let record = if let Some(event) = filtered_fact(filters, &event) {
                events.decorate(event, &host, &session, filters)
            } else {
                events.control("checkpoint", &host, &session, filters)
            };
            let subscriber = events
                .subscribers
                .get_mut(&client)
                .expect("subscriber exists");
            subscriber.bytes += record.to_string().len();
            subscriber.pending.push_back(record);
            if subscriber.bytes > BYTES || subscriber.pending.len() > EVENTS + 3 {
                events.subscribers.remove(&client);
                self.bus
                    .senders
                    .send_to_server(ServerInstruction::RemoveClient(client))
                    .non_fatal();
            }
        }
    }

    pub(super) fn flush_agent_events(&mut self) {
        let Some(os) = self.bus.os_input.as_ref() else {
            return;
        };
        self.agent_events.subscribers.retain(|client, subscriber| {
            for _ in 0..BATCH {
                let Some(event) = subscriber.pending.pop_front() else {
                    break;
                };
                let event_json = event.to_string();
                subscriber.bytes -= event_json.len();
                if os
                    .send_to_client(
                        *client,
                        ServerToClientMsg::EventUpdate {
                            event_type: "agent_lifecycle".into(),
                            event_json,
                        },
                    )
                    .is_err()
                {
                    self.bus
                        .senders
                        .send_to_server(ServerInstruction::RemoveClient(*client))
                        .non_fatal();
                    return false;
                }
            }
            true
        });
    }

    pub(super) fn refresh_agent_events(&mut self) {
        self.agent_events.dirty = false;
        let current: BTreeMap<_, _> = self
            .tabs
            .values()
            .flat_map(|tab| tab.get_all_pane_ids())
            .filter(|pane| matches!(pane, PaneId::Terminal(_)))
            .map(|pane| {
                let agent = self
                    .tab_containing_pane(pane)
                    .and_then(|tab| tab.pane_agent_info(pane))
                    .or_else(|| self.detected_agent_status(pane).map(|status| status.agent));
                let tasks = self
                    .agent_lifecycle
                    .get(&pane)
                    .map(|l| l.event_background_tasks())
                    .unwrap_or_default();
                (
                    pane,
                    json!({"pane_id":pane.to_string(),"kind":agent.as_ref().map(|a| &a.kind),
                    "conversation_id":agent.as_ref().and_then(|a| a.session_id.as_ref()),
                    "run_id":agent.as_ref().and_then(|a| a.run_id.as_ref()),
                    "source":agent.as_ref().map(|a| a.source.to_string()),
                    "control":self.agent_status(pane, None).map(|s| json!({"prompt_supported":s.prompt_supported,"status_available":s.status_available,"not_ready_reason":s.not_ready_reason})),
                    "state":agent.as_ref().map(|a| a.state.to_string().to_lowercase()),
                    "needs_input":agent.as_ref().is_some_and(|a| a.is_waiting_on_a_person()),
                    "submission_id":null,"turn_id":null,"background_tasks":tasks,"projects":[]}),
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
            self.retire_agent_event_pane(pane, "pane_closed");
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
                    self.publish_agent_event(pane, "conversation_replaced", json!({"next_kind":state["kind"],"next_conversation_id":state["conversation_id"]}));
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
                self.publish_agent_event(pane, "needs_input", json!({}));
            }
            if previous
                .as_ref()
                .is_some_and(|p| p["background_tasks"] != state["background_tasks"])
            {
                self.publish_agent_event(pane, "background_tasks_changed", json!({}));
            }
        }
    }

    pub(super) fn retire_agent_event_pane(&mut self, pane: PaneId, event_type: &str) {
        self.publish_agent_event(pane, event_type, json!({}));
        self.agent_events.panes.remove(&pane);
        self.agent_events.turns.remove(&pane);
        self.agent_events.uncertain.retain(|(id, _), _| *id != pane);
        self.agent_events.dirty = true;
    }

    pub(super) fn observe_agent_event(
        &mut self,
        event: &AgentEvent,
        hook: Option<&HookEvent>,
        submission: Option<String>,
    ) {
        // A new acceptance supplies correlation before its busy-state edge is published.
        // Otherwise that edge would inherit the previous turn's submission ID.
        if let Some(hook) =
            hook.filter(|h| !h.is_worker() && h.hook_event_name == "UserPromptSubmit")
        {
            let pane = PaneId::Terminal(event.pane_id);
            let primary = if matches!(event.kind.as_str(), "claude" | "agy") {
                &hook.prompt_id
            } else {
                &hook.turn_id
            };
            let repeated = self.agent_events.turns.get(&pane).is_some_and(|turns| {
                turns.iter().any(|turn| {
                    if matches!(event.kind.as_str(), "claude" | "agy") {
                        &turn.identity.1 == primary
                    } else {
                        &turn.identity.0 == primary
                    }
                })
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
        self.refresh_agent_events();
        self.refresh_agent_event_projects();
        let Some(hook) = hook.filter(|h| !h.is_worker() && !h.session_id.is_empty()) else {
            return;
        };
        let pane = PaneId::Terminal(event.pane_id);
        let identity = (
            hook.turn_id.clone().filter(|id| !id.is_empty()),
            hook.prompt_id.clone().filter(|id| !id.is_empty()),
        );
        let has_identity = if matches!(event.kind.as_str(), "claude" | "agy") {
            identity.1.is_some()
        } else {
            identity.0.is_some()
        };
        if hook.hook_event_name == "UserPromptSubmit" && !has_identity {
            self.agent_observation_uncertain(pane, submission, AgentOutcome::StatusUnavailable);
            return;
        }
        let turns = self.agent_events.turns.entry(pane).or_default();
        let same_turn = |turn: &Turn| {
            if matches!(event.kind.as_str(), "claude" | "agy") {
                turn.identity.1 == identity.1
            } else {
                turn.identity.0 == identity.0
            }
        };
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
            let evicted = (turns.len() > super::agent_control::SUBMISSION_HISTORY_LIMIT)
                .then(|| turns.pop_front())
                .flatten()
                .filter(|turn| !turn.ended);
            if let Some(evicted) = evicted {
                self.publish_agent_event(
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
            self.publish_agent_event(pane, "submission_acknowledged", json!({"submission_id":submission,"turn_id":hook.turn_id.as_deref().filter(|id| !id.is_empty()),"provider_prompt_id":hook.prompt_id.as_deref().filter(|id| !id.is_empty())}));
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
            self.publish_agent_event(pane, event_type, extra);
        }
    }
}

impl Screen {
    pub(super) fn agent_observation_uncertain(
        &mut self,
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
        self.publish_agent_event(pane, "observation_uncertain", extra);
    }

    pub(super) fn invalidate_agent_event_projects(&mut self) {
        self.agent_events.project_version += 1;
        self.agent_events.project_ready = false;
        self.refresh_agent_event_projects();
    }

    pub(super) fn retry_agent_event_projects(&mut self, now: Instant) {
        self.agent_events.prune_projects();
        if self.agent_events.project_retry.is_some_and(|at| now >= at) {
            self.agent_events.project_retry = None;
            self.refresh_agent_event_projects();
        }
    }

    pub(super) fn refresh_agent_event_projects(&mut self) {
        if self.agent_events.projects.is_empty() {
            return;
        }
        let mut agents: Vec<_> = self
            .tabs
            .values()
            .flat_map(|tab| tab.get_all_pane_ids())
            .filter_map(|pane| {
                self.agent_status(pane, None)
                    .or_else(|| self.detected_agent_status(pane))
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
            self.bus
                .senders
                .send_to_plugin(
                    crate::plugins::PluginInstruction::ResolveAgentEventProjects {
                        version,
                        agents,
                        projects: self.agent_events.projects.clone(),
                    },
                )
                .non_fatal();
        }
    }

    pub(super) fn apply_agent_event_projects(
        &mut self,
        version: u64,
        result: yinz_bus::screen::AgentEventProjectMemberships,
    ) {
        self.refresh_agent_events();
        self.refresh_agent_event_projects();
        if self.agent_events.project_inflight != Some(version) {
            return;
        }
        self.agent_events.project_inflight = None;
        if version != self.agent_events.project_version {
            self.refresh_agent_event_projects();
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
                        .chain(
                            self.agent_events
                                .subscribers
                                .iter()
                                .filter(|(_, s)| s.filters.projects.contains(&project))
                                .map(|(c, _)| *c),
                        )
                        .collect();
                    for client in clients {
                        self.agent_events.remove(client);
                        self.bus
                            .senders
                            .send_to_server(ServerInstruction::LogError(
                                vec![error.clone()],
                                client,
                                None,
                            ))
                            .non_fatal();
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
                    pane,
                    "project_membership_changed",
                    json!({"previous_projects":previous}),
                );
            }
        }
        for (client, options) in std::mem::take(&mut self.agent_events.project_pending) {
            self.start_agent_events(client, options);
        }
    }
}
