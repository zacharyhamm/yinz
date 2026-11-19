//! Ordered agent lifecycle events and resumable, per-client delivery.
use crate::host::AgentHost;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    time::{SystemTime, UNIX_EPOCH},
};
use yinz_ipc::ipc::EventSubscription;
use yinz_runtime::errors::prelude::*;
use yinz_types::{internal_ids::ClientId, pane_id::PaneId};

pub const EVENTS: usize = 4096;
const BYTES: usize = 8 * 1024 * 1024;
const PAYLOAD_BYTES: usize = 64 * 1024;
const BATCH: usize = 32;

struct Subscriber {
    filters: EventSubscription,
    pending: VecDeque<Value>,
    bytes: usize,
}

pub struct EventJournal {
    epoch: String,
    sequence: u64,
    history: VecDeque<Value>,
    bytes: usize,
    subscribers: HashMap<ClientId, Subscriber>,
}
impl Default for EventJournal {
    fn default() -> Self {
        Self {
            epoch: uuid::Uuid::new_v4().to_string(),
            sequence: 0,
            history: VecDeque::new(),
            bytes: 0,
            subscribers: HashMap::new(),
        }
    }
}

pub fn canonicalize(options: &mut EventSubscription) -> Result<()> {
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

impl EventJournal {
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

    pub fn remove(&mut self, client: ClientId) {
        self.subscribers.remove(&client);
    }

    pub fn subscribers_for_project<'a>(
        &'a self,
        project: &'a str,
    ) -> impl Iterator<Item = ClientId> + 'a {
        self.subscribers
            .iter()
            .filter(move |(_, sub)| sub.filters.projects.iter().any(|p| p == project))
            .map(|(client, _)| *client)
    }

    pub fn has_pending(&self) -> bool {
        self.subscribers.values().any(|sub| !sub.pending.is_empty())
    }

    pub fn publish(
        &mut self,
        host: &impl AgentHost,
        pane: PaneId,
        event_type: &str,
        pane_sequence: u64,
        mut event: Value,
        resolved_projects: &[String],
    ) {
        let host_id = json!(yinz_runtime::host_identity::host_id());
        let session = host.session_name();
        self.sequence += 1;
        event["schema_version"] = 1.into();
        event["event_type"] = event_type.into();
        event["sequence"] = self.sequence.into();
        event["host_id"] = host_id.clone();
        event["session"] = session.into();
        event["server_epoch"] = self.epoch.clone().into();
        event["observed_at_ms"] = json!(SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis());
        event["projects_resolved_for"] = json!(resolved_projects);
        if event.to_string().len() > PAYLOAD_BYTES / 2 {
            // Keep loss explicit and resumable; never truncate an identity into a different one.
            event = json!({"schema_version":1,"event_type":"observation_uncertain","reason":"payload_limit",
                "sequence":self.sequence,"pane_id":pane.to_string(),"pane_generation":event["pane_generation"],
                "pane_sequence":pane_sequence,"host_id":host_id,"session":session,"server_epoch":self.epoch,
                "projects_resolved_for":[],"observed_at_ms":event["observed_at_ms"],"kind":null,"conversation_id":null,"submission_id":null,"turn_id":null,"run_id":null});
        }
        let size = event.to_string().len();
        self.bytes += size;
        self.history.push_back(event.clone());
        while self.history.len() > EVENTS || self.bytes > BYTES {
            if let Some(old) = self.history.pop_front() {
                self.bytes -= old.to_string().len();
            }
        }
        let clients: Vec<_> = self.subscribers.keys().copied().collect();
        for client in clients {
            let filters = &self.subscribers[&client].filters;
            let record = if let Some(event) = filtered_fact(filters, &event) {
                self.decorate(event, &host_id, session, filters)
            } else {
                self.control("checkpoint", &host_id, session, filters)
            };
            let subscriber = self
                .subscribers
                .get_mut(&client)
                .expect("subscriber exists");
            subscriber.bytes += record.to_string().len();
            subscriber.pending.push_back(record);
            if subscriber.bytes > BYTES || subscriber.pending.len() > EVENTS + 3 {
                self.subscribers.remove(&client);
                host.remove_event_subscriber(client);
            }
        }
    }

    pub fn subscribe<'a>(
        &mut self,
        host: &impl AgentHost,
        client: ClientId,
        mut options: EventSubscription,
        panes: impl Iterator<Item = &'a Value>,
    ) {
        let host_id = json!(yinz_runtime::host_identity::host_id());
        let session = host.session_name();
        let result = (|| -> Result<Subscriber> {
            canonicalize(&mut options)?;
            let cursor = options.cursor.take();
            let mut pending = VecDeque::new();
            let mut fresh = true;
            if let Some(cursor) = cursor {
                if cursor.len() > PAYLOAD_BYTES {
                    bail!("lifecycle cursor is too large");
                }
                let cursor: Value =
                    serde_json::from_str(&cursor).context("invalid lifecycle cursor")?;
                if cursor["v"] != 1
                    || cursor["host"] != host_id
                    || cursor["session"] != session
                    || cursor["filters"] != json!(options)
                {
                    bail!("lifecycle cursor owner or filters do not match this subscription");
                }
                let sequence = cursor["sequence"]
                    .as_u64()
                    .context("invalid lifecycle cursor sequence")?;
                if cursor["epoch"] == self.epoch {
                    if sequence > self.sequence {
                        bail!("lifecycle cursor is in the future");
                    }
                    let oldest = self
                        .history
                        .front()
                        .and_then(|v| v["sequence"].as_u64())
                        .unwrap_or(self.sequence + 1);
                    if sequence + 1 >= oldest {
                        fresh = false;
                        for event in self
                            .history
                            .iter()
                            .filter(|v| v["sequence"].as_u64().is_some_and(|s| s > sequence))
                        {
                            if let Some(event) = filtered_fact(&options, event) {
                                pending
                                    .push_back(self.decorate(event, &host_id, session, &options));
                            }
                        }
                    }
                } else if !cursor["epoch"].is_string() {
                    bail!("invalid lifecycle cursor epoch");
                }
                if fresh {
                    let mut gap = self.control("gap", &host_id, session, &options);
                    gap["reason"] = if cursor["epoch"] == self.epoch {
                        "history_expired"
                    } else {
                        "server_restarted"
                    }
                    .into();
                    pending.push_back(gap);
                }
            }
            if fresh {
                pending.push_back(self.control("snapshot_begin", &host_id, session, &options));
                for pane in panes {
                    // Lifecycle filters apply to live facts, not the initial state needed to interpret them.
                    let mut snapshot_filters = options.clone();
                    snapshot_filters.lifecycle_types.clear();
                    if !pane["kind"].is_null() && matches(&snapshot_filters, pane) {
                        let mut snapshot = self.control("snapshot", &host_id, session, &options);
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
            pending.push_back(self.control("ready", &host_id, session, &options));
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
                self.subscribers.insert(client, subscriber);
            },
            Err(error) => {
                host.subscription_error(client, format!("{error:#}"));
            },
        }
        self.flush(host);
    }

    pub fn flush(&mut self, host: &impl AgentHost) {
        if !host.event_transport_available() {
            return;
        }
        self.subscribers.retain(|client, subscriber| {
            for _ in 0..BATCH {
                let Some(event) = subscriber.pending.pop_front() else {
                    break;
                };
                let event_json = event.to_string();
                subscriber.bytes -= event_json.len();
                if !host.send_event(*client, event_json) {
                    host.remove_event_subscriber(*client);
                    return false;
                }
            }
            true
        });
    }
}
