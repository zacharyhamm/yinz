//! One writer owns Mother's durable inbox; screen-thread guards own terminal input.
use super::{agent_control::Reply, Screen};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};
use yinz_bus::{screen::ScreenInstruction, thread_bus::ThreadSenders};
use yinz_ipc::ipc::{
    ClientToServerMsg, EventSubscription, IpcReceiverWithContext, IpcRecv, IpcSenderWithContext,
    ServerToClientMsg,
};
use yinz_runtime::errors::prelude::*;
use yinz_types::{
    agent::{
        control::{AgentOutcome, AgentRequest, AgentResponse},
        mother::{MotherRequest, MotherStatus},
    },
    pane_id::PaneId,
};

const JOURNAL_RECORDS: usize = 4096;
const JOURNAL_BYTES: usize = 8 * 1024 * 1024;
const TICK: Duration = Duration::from_millis(200);

pub(super) struct Supervisor {
    pub generation: String,
    pane: PaneId,
    session: String,
    commands: Option<mpsc::Sender<(MotherRequest, Reply, Option<String>)>>,
    thread: Option<std::thread::JoinHandle<()>>,
    stopped: Arc<AtomicBool>,
    observations: mpsc::SyncSender<Value>,
    observation_gap: Arc<AtomicBool>,
}

impl Supervisor {
    fn running(&self) -> bool {
        !self.stopped.load(Ordering::Acquire)
            && self
                .thread
                .as_ref()
                .is_none_or(|thread| !thread.is_finished())
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.commands.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Batch {
    id: String,
    summary: Vec<String>,
    events: usize,
    urgent: bool,
    attempted: bool,
    #[serde(default)]
    frozen: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Receipt {
    batch: String,
    conversation: String,
    submission: String,
    response: Option<AgentResponse>,
    settled: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Memory {
    revision: u64,
    checkpoint: Value,
    filters: EventSubscription,
    cursor: Option<String>,
    paused: bool,
    roster: BTreeMap<String, Value>,
    journal: VecDeque<Arc<Value>>,
    #[serde(default)]
    journal_sequence: u64,
    pending: VecDeque<Batch>,
    receipts: VecDeque<Receipt>,
    conversation: Option<String>,
    gap: bool,
    #[serde(default)]
    alerts: VecDeque<String>,
    #[serde(default)]
    snapshotting: bool,
    #[serde(default)]
    delivery_blocked: Option<String>,
}

impl Memory {
    fn subscription_filters(&self) -> EventSubscription {
        let mut filters = self.filters.clone();
        filters.lifecycle_types.clear();
        filters.cursor = None;
        filters
    }

    fn selects_wake(&self, kind: &str) -> bool {
        self.filters.lifecycle_types.is_empty()
            || self
                .filters
                .lifecycle_types
                .iter()
                .any(|wanted| wanted == kind)
    }

    fn load(path: &Path, session: &str) -> Result<Self> {
        let mut memory: Self = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("read Mother checkpoint"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).context("read Mother checkpoint"),
        }?;
        // Renames preserve operational memory, but lifecycle cursors belong to one session.
        if memory.snapshotting
            || memory.cursor.as_deref().is_some_and(|cursor| {
                serde_json::from_str::<Value>(cursor).map_or(true, |cursor| {
                    cursor["v"] != 1
                        || cursor["session"] != session
                        || !cursor["host"].is_string()
                        || !cursor["epoch"].is_string()
                        || cursor["sequence"].as_u64().is_none()
                        || cursor["filters"] != json!(memory.subscription_filters())
                })
            })
        {
            memory.cursor = None;
        }
        // Upgrade old journals once, retaining their complete event bodies.
        for event in &mut memory.journal {
            if let Some(cursor) = event["journal_cursor"].as_u64() {
                memory.journal_sequence = memory.journal_sequence.max(cursor);
            } else {
                memory.journal_sequence += 1;
                Arc::make_mut(event)["journal_cursor"] = json!(memory.journal_sequence);
            }
        }
        Ok(memory)
    }

    fn save(&self, path: &Path) -> Result<()> {
        yinz_runtime::mother::replace(
            path,
            &serde_json::to_vec(self).context("encode Mother checkpoint")?,
        )
    }

    fn batch(&mut self, summary: String, urgent: bool) {
        if self.pending.back().is_none_or(|b| b.attempted || b.frozen) {
            self.pending.push_back(Batch {
                id: uuid::Uuid::new_v4().to_string(),
                summary: vec![],
                events: 0,
                urgent,
                attempted: false,
                frozen: false,
            });
        }
        if let Some(batch) = self.pending.back_mut() {
            batch.events += 1;
            batch.urgent |= urgent;
            if batch.summary.len() < 32 {
                batch.summary.push(summary);
            }
        }
        // Unacknowledged old batches remain explicit in the journal, but cannot grow forever.
        if self.pending.len() > 256 {
            self.pending.pop_front();
            self.gap = true;
        }
    }

    fn event(&mut self, event: Value, mother: PaneId) -> bool {
        let cursor = event["cursor"].as_str().map(str::to_owned);
        let kind = event["event_type"].as_str().unwrap_or_default();
        if cursor == self.cursor && !matches!(kind, "snapshot" | "snapshot_begin" | "ready" | "gap")
        {
            return false;
        }
        if !matches!(kind, "snapshot" | "snapshot_begin" | "ready" | "gap") {
            if let Some(previous) = self
                .cursor
                .as_deref()
                .and_then(|c| serde_json::from_str::<Value>(c).ok())
            {
                if previous["epoch"] == event["server_epoch"]
                    && event["sequence"]
                        .as_u64()
                        .zip(previous["sequence"].as_u64())
                        .is_some_and(|(next, last)| next <= last)
                {
                    return false;
                }
            }
        }
        let own = event["pane_id"] == mother.to_string();
        match kind {
            "gap" => {
                self.gap = true;
                self.batch(
                    "Observation gap: reconcile live agents and retained results before acting"
                        .into(),
                    true,
                );
            },
            "snapshot_begin" => {
                self.roster.clear();
                self.snapshotting = true;
                self.cursor = None;
            },
            "ready" => {
                self.snapshotting = false;
            },
            "checkpoint" => {},
            _ if own => {
                let conversation = event["conversation_id"].as_str().map(str::to_owned);
                if kind != "conversation_replaced"
                    && conversation.is_some()
                    && conversation != self.conversation
                {
                    self.conversation = conversation;
                    self.batch(
                        "Bootstrap Mother: load durable memory and reconcile this session".into(),
                        false,
                    );
                }
            },
            _ => {
                if let Some(pane) = event["pane_id"].as_str() {
                    let observed_worker = self.roster.contains_key(pane);
                    let state_changed = self.roster.get(pane).is_none_or(|old| {
                        ["state", "conversation_id", "needs_input"]
                            .iter()
                            .any(|key| old[key] != event[key])
                    });
                    let left_projects = kind == "project_membership_changed"
                        && !self.filters.projects.is_empty()
                        && event["projects"].as_array().is_some_and(|projects| {
                            !self
                                .filters
                                .projects
                                .iter()
                                .any(|wanted| projects.iter().any(|project| project == wanted))
                        });
                    if matches!(
                        kind,
                        "conversation_replaced" | "pane_closed" | "pane_transferred"
                    ) || (kind == "state_changed" && event.get("kind") == Some(&Value::Null))
                        || left_projects
                    {
                        self.roster.remove(pane);
                    } else if !event["kind"].is_null() {
                        let mut row = event.clone();
                        if let Some(old) = self
                            .roster
                            .get(pane)
                            .filter(|old| old["conversation_id"] == event["conversation_id"])
                        {
                            if old["submission_id"] == row["submission_id"] {
                                row["retained_result"] = old["retained_result"].clone();
                            }
                            for key in [
                                "assignment",
                                "last_outcome",
                                "current_activity",
                                "waiting_on",
                                "last_message",
                                "control",
                            ] {
                                if row[key].is_null() {
                                    row[key] = old[key].clone();
                                }
                            }
                        }
                        if kind == "submission_acknowledged" {
                            row["last_outcome"] = Value::Null;
                            row["retained_result"] = Value::Null;
                        }
                        if kind == "observation_uncertain"
                            || kind == "turn_completed"
                            || kind == "turn_failed"
                            || kind == "turn_interrupted"
                        {
                            row["last_outcome"] = event.clone();
                            row["retained_result"] = Value::Null;
                        }
                        if !event["detail"].is_null() {
                            row["current_activity"] = event["detail"].clone();
                        }
                        self.roster.insert(pane.to_owned(), row);
                    }
                    let urgent = kind == "needs_input"
                        || kind == "turn_failed"
                        || kind == "observation_uncertain";
                    if (!event["kind"].is_null() || observed_worker)
                        && self.selects_wake(kind)
                        && (!matches!(kind, "snapshot" | "activity" | "state_changed")
                            || urgent
                            || (kind == "state_changed" && state_changed))
                    {
                        self.batch(
                            format!(
                                "{pane}: {kind} ({})",
                                event["kind"].as_str().unwrap_or("unknown provider")
                            ),
                            urgent,
                        );
                    }
                }
            },
        }
        if own {
            if !self.snapshotting {
                self.cursor = cursor.or(self.cursor.take());
            }
            return true;
        }
        self.record(event);
        let mut bytes: usize = self.journal.iter().map(|v| v.to_string().len()).sum();
        let overflow = self.journal.len() > JOURNAL_RECORDS || bytes > JOURNAL_BYTES;
        while overflow && (self.journal.len() > JOURNAL_RECORDS / 2 || bytes > JOURNAL_BYTES / 2) {
            if let Some(old) = self.journal.pop_front() {
                bytes -= old.to_string().len();
            }
            self.gap = true;
        }
        if overflow {
            self.record(json!({"event_type":"gap","reason":"mother_journal_overflow"}));
            self.batch(
                "Journal overflow: reconcile live agents and retained results".into(),
                true,
            );
        }
        if !self.snapshotting {
            self.cursor = cursor.or(self.cursor.take());
        }
        true
    }

    fn receipt_response(&mut self, mut result: AgentResponse, delivery: bool) {
        let Some(receipt) = self.receipts.back_mut() else {
            return;
        };
        if !delivery && result.submission_id.is_none() {
            result.submission_id = Some(receipt.submission.clone());
        }
        // Only the delivery path can prove rejection happened before any terminal write.
        if delivery && result.submission_id.is_none() && receipt.response.is_none() {
            self.delivery_blocked = result.message.clone();
            if let Some(batch) = self.pending.iter_mut().find(|b| b.id == receipt.batch) {
                batch.attempted = false;
            }
            self.receipts.pop_back();
        } else {
            self.delivery_blocked = None;
            receipt.settled = matches!(
                result.outcome,
                AgentOutcome::Completed
                    | AgentOutcome::TargetChanged
                    | AgentOutcome::TargetClosed
                    | AgentOutcome::TargetExited
            ) || (result.outcome == AgentOutcome::Failed && result.acknowledged)
                // WaitTurn reports Interrupted only for a retained terminal result.
                || (!delivery && result.outcome == AgentOutcome::Interrupted);
            receipt.response = Some(result);
        }
    }

    fn reconcile(&mut self, request: &AgentRequest, result: AgentResponse) -> bool {
        let AgentRequest::WaitTurn {
            pane_id,
            expected_session_id,
            submission_id,
            ..
        } = request
        else {
            return false;
        };
        let Some(row) = self.roster.get_mut(&pane_id.to_string()).filter(|row| {
            row["conversation_id"].as_str() == expected_session_id.as_deref()
                && row["submission_id"].as_str() == Some(submission_id.as_str())
                && result.submission_id.as_deref() == Some(submission_id.as_str())
        }) else {
            return false;
        };
        row["retained_result"] = json!({"outcome":result.outcome,"acknowledged":result.acknowledged,"submission_id":result.submission_id,"turn_id":result.turn_id,"message":result.message.as_ref().map(|s| s.chars().take(4096).collect::<String>()),"background_tasks":result.background_tasks});
        true
    }

    fn status(&self, ready: bool, error: bool) -> MotherStatus {
        let blockers = self
            .roster
            .values()
            .filter(|r| {
                r["needs_input"] == true
                    || r["last_outcome"]["event_type"] == "turn_failed"
                    || (r["last_outcome"].is_null()
                        && r["retained_result"]["outcome"] == "failed"
                        && r["retained_result"]["acknowledged"] == true
                        && r["retained_result"]["submission_id"] == r["submission_id"])
            })
            .count();
        let unsettled = self.receipts.back().is_some_and(|r| !r.settled);
        MotherStatus {
            state: if error || !ready {
                "degraded"
            } else if self.paused {
                "paused"
            } else if blockers > 0
                || self.gap
                || self.receipts.iter().any(|r| {
                    r.response
                        .as_ref()
                        .is_some_and(|result| result.outcome == AgentOutcome::Failed)
                        && self.pending.iter().any(|b| b.id == r.batch)
                })
                || self.receipts.back().is_some_and(|r| {
                    !r.settled
                        && r.response.as_ref().is_some_and(|result| {
                            matches!(
                                result.outcome,
                                AgentOutcome::Rejected
                                    | AgentOutcome::NeedsInput
                                    | AgentOutcome::Failed
                                    | AgentOutcome::Interrupted
                                    | AgentOutcome::StatusUnavailable
                            )
                        })
                })
            {
                "needs_attention"
            } else if unsettled {
                "processing"
            } else if self.pending.iter().any(|b| !b.attempted) {
                "queued"
            } else {
                "watching"
            }
            .into(),
            pending: self.pending.len(),
            blockers,
        }
    }

    fn record(&mut self, mut event: Value) {
        self.journal_sequence += 1;
        event["journal_cursor"] = json!(self.journal_sequence);
        self.journal.push_back(Arc::new(event));
    }

    fn journal_page(
        &self,
        after: Option<u64>,
        through: Option<u64>,
        limit: Option<usize>,
    ) -> Result<Value> {
        let limit = limit.unwrap_or(50);
        if !(1..=200).contains(&limit) {
            bail!("journal limit must be between 1 and 200");
        }
        let after = after.unwrap_or(0);
        let through = through.unwrap_or(self.journal_sequence);
        if through > self.journal_sequence || after > through {
            bail!("journal cursor is beyond the requested high-water boundary");
        }
        let oldest = self
            .journal
            .front()
            .and_then(|event| event["journal_cursor"].as_u64());
        let mut records = vec![];
        let mut bytes = 0;
        let mut cursor = after;
        let mut more = false;
        for event in &self.journal {
            let sequence = event["journal_cursor"].as_u64().unwrap_or(0);
            if sequence <= after || sequence > through {
                continue;
            }
            let size = event.to_string().len();
            if records.len() == limit || (!records.is_empty() && bytes + size > 256 * 1024) {
                more = true;
                break;
            }
            // An oversized record remains addressable without making a page unbounded.
            if size > 256 * 1024 {
                records.push(json!({"journal_cursor":sequence,"event_type":"evidence_unavailable","reason":"record exceeds page byte limit"}));
            } else {
                records.push(event.as_ref().clone());
            }
            bytes += size.min(256 * 1024);
            cursor = sequence;
        }
        Ok(
            json!({"records":records,"next_cursor":more.then_some(cursor),"cursor":cursor,"high_water":through,"oldest_cursor":oldest,"retention_gap":oldest.is_some_and(|oldest| after.saturating_add(1) < oldest)}),
        )
    }

    fn overview(&self, ready: bool, failed: bool) -> Value {
        json!({"status":self.status(ready, failed),"filters":self.filters,
            "checkpoint":{"revision":self.revision,"value":self.checkpoint},
            "roster":self.roster,"batches":self.pending,"receipts":self.receipts,
            "gap":self.gap,"delivery_blocked":self.delivery_blocked,
            "paused":self.paused, "observation_ready":ready && !failed, "journal_high_water":self.journal_sequence, "recent":self.journal_page(Some(self.journal_sequence.saturating_sub(50)), None, None).ok()})
    }

    fn persist_command(&mut self, request: MotherRequest, path: &Path) -> Result<Value> {
        if matches!(
            request,
            MotherRequest::Status
                | MotherRequest::Overview
                | MotherRequest::Journal { .. }
                | MotherRequest::Checkpoint { value: None, .. }
        ) {
            return self.command(request);
        }
        let mut next = self.clone();
        let value = next.command(request)?;
        next.save(path)?;
        *self = next;
        Ok(value)
    }

    fn command(&mut self, request: MotherRequest) -> Result<Value> {
        Ok(match request {
            MotherRequest::Status => json!(self.status(true, false)),
            MotherRequest::Overview => self.overview(true, false),
            MotherRequest::Journal {
                after,
                through,
                limit,
            } => self.journal_page(after, through, limit)?,
            MotherRequest::Resolve {
                submission_id,
                expect_rev,
            } => {
                if expect_rev != self.revision {
                    bail!("checkpoint revision conflict; review the current checkpoint before resolving");
                }
                let receipt = self
                    .receipts
                    .back_mut()
                    .filter(|r| r.submission == submission_id && !r.settled)
                    .context("no unresolved receipt with that submission identity")?;
                if !receipt.response.as_ref().is_some_and(|r| {
                    r.outcome == AgentOutcome::StatusUnavailable
                        && r.submission_id.as_deref() == Some(submission_id.as_str())
                }) {
                    bail!("only an explicitly unavailable result can be resolved after review");
                }
                receipt.settled = true;
                self.revision = self
                    .revision
                    .checked_add(1)
                    .context("checkpoint revision exhausted")?;
                self.record(json!({"event_type":"supervision_result_resolved","submission_id":submission_id,"outcome":"unknown","checkpoint_revision":self.revision}));
                self.batch("Reviewed unavailable supervision result: reconcile live agents and archives; never replay the original prompt".into(), true);
                json!({"resolved":submission_id,"outcome":"unknown","revision":self.revision})
            },
            MotherRequest::Inbox => {
                // An acknowledgement must cover only the events the reader saw.
                for batch in &mut self.pending {
                    batch.frozen = true;
                }
                json!({"batches":self.pending,"roster":self.roster,"journal":self.journal_page(None, None, None)?,"receipts":self.receipts,"gap":self.gap,"cursor":self.cursor,"delivery_blocked":self.delivery_blocked})
            },
            MotherRequest::Checkpoint { expect_rev, value } => {
                if let Some(value) = value {
                    if expect_rev != Some(self.revision) {
                        bail!("checkpoint revision conflict; read and merge before updating");
                    }
                    if !value.is_object() || value.to_string().len() > 1024 * 1024 {
                        bail!("checkpoint must be an object of at most 1 MiB");
                    }
                    self.checkpoint = value;
                    self.revision = self
                        .revision
                        .checked_add(1)
                        .context("checkpoint revision exhausted")?;
                } else if expect_rev.is_some() {
                    bail!("expect-rev requires a checkpoint value");
                }
                json!({"revision":self.revision,"value":self.checkpoint})
            },
            MotherRequest::Ack { batch } => {
                if !self.pending.iter().any(|b| b.id == batch) {
                    bail!("unknown batch ID");
                }
                self.pending.retain(|b| b.id != batch);
                json!({"acknowledged":batch})
            },
            MotherRequest::Configure {
                panes,
                projects,
                kinds,
                lifecycle_types,
            } => {
                let mut filters = EventSubscription {
                    panes,
                    projects,
                    kinds,
                    lifecycle_types,
                    cursor: None,
                };
                super::agent_events::canonicalize(&mut filters)?;
                self.filters = filters;
                self.cursor = None;
                json!(self.filters)
            },
            MotherRequest::Pause => {
                self.paused = true;
                json!({"paused":true})
            },
            MotherRequest::Resume => {
                self.paused = false;
                json!({"paused":false})
            },
        })
    }
}

struct Stream {
    sender: IpcSenderWithContext<ClientToServerMsg>,
    receiver: IpcReceiverWithContext<ServerToClientMsg>,
}

impl Stream {
    fn connect(session: &str, memory: &Memory) -> Result<Self> {
        let socket =
            yinz_ipc::consts::ipc_connect(&yinz_runtime::consts::YINZ_SOCK_DIR.join(session))
                .context("connect Mother lifecycle stream")?;
        let mut sender = IpcSenderWithContext::new(socket);
        let receiver = sender
            .get_receiver()
            .context("receive Mother lifecycle stream")?;
        receiver
            .set_recv_timeout(Some(TICK))
            .context("bound Mother lifecycle read")?;
        let mut options = memory.subscription_filters();
        options.cursor = memory.cursor.clone();
        sender
            .send_client_msg(ClientToServerMsg::SubscribeToEvents {
                event_types: vec!["agent_lifecycle".into()],
                options,
            })
            .context("subscribe Mother lifecycle stream")?;
        Ok(Self { sender, receiver })
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.sender.send_client_msg(ClientToServerMsg::ClientExited);
    }
}

fn reply(senders: &ThreadSenders, target: Reply, response: AgentResponse) {
    match target {
        Reply::Host(tx) => {
            let _ = tx.send(response);
        },
        Reply::Client(client) => {
            senders
                .send_to_screen(ScreenInstruction::MotherCommandReply(client, response))
                .context("reply to Mother command")
                .non_fatal();
        },
    }
}

// Hook prose stays private; lifecycle facts remain the authority for ordering and outcomes.
fn merge_activity(old: &mut Value, new: &Value) {
    if old["conversation_id"] != new["conversation_id"] || !old.is_object() {
        *old = new.clone();
    } else {
        if !new["assignment"].is_null() {
            old["last_message"] = Value::Null;
        }
        for key in ["assignment", "last_message"] {
            if !new[key].is_null() {
                old[key] = new[key].clone();
            }
        }
        old["current_activity"] = new["current_activity"].clone();
        old["waiting_on"] = new["waiting_on"].clone();
    }
}

fn enrich_row(row: &mut Value, observation: &Value) -> bool {
    if row["conversation_id"].is_null() || row["conversation_id"] != observation["conversation_id"]
    {
        return false;
    }
    let mut changed = false;
    for key in [
        "assignment",
        "current_activity",
        "waiting_on",
        "last_message",
    ] {
        if row[key] != observation[key] {
            row[key] = observation[key].clone();
            changed = true;
        }
    }
    changed
}

fn memory_path(cache: &Path, session: &str) -> Result<PathBuf> {
    Ok(
        yinz_config::project_config::bubblewrap::mother_directory(cache, session)?
            .parent()
            .context("Mother workspace has no parent")?
            .join("supervision.json"),
    )
}

fn writer_lock(path: &Path) -> Result<std::fs::File> {
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))
        .context("open Mother writer lock")?;
    lock.try_lock()
        .context("Mother supervisor already owns this workspace")?;
    Ok(lock)
}

pub(super) fn rename_memory(
    cache: &Path,
    old: &str,
    new: &str,
    rename: impl FnOnce() -> Result<()>,
) -> Result<()> {
    if old == new {
        return rename();
    }
    let source = memory_path(cache, old)?;
    let destination = memory_path(cache, new)?;
    let _source_lock = writer_lock(&source)?;
    let _destination_lock = writer_lock(&destination)?;
    match std::fs::symlink_metadata(&destination) {
        Ok(_) => bail!("Mother memory already exists for session {new}"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
        Err(error) => return Err(error).context("inspect destination Mother memory"),
    }
    let moved = match std::fs::rename(&source, &destination) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error).context("rename Mother memory"),
    };
    if let Err(error) = rename() {
        if moved {
            std::fs::rename(&destination, &source)
                .with_context(|| format!("restore Mother memory after rename failed: {error:#}"))?;
        }
        return Err(error);
    }
    Ok(())
}

fn run(
    session: String,
    pane: PaneId,
    generation: String,
    senders: ThreadSenders,
    commands: &mpsc::Receiver<(MotherRequest, Reply, Option<String>)>,
    observations: mpsc::Receiver<Value>,
    observation_gap: Arc<AtomicBool>,
) -> Result<()> {
    let path = memory_path(&yinz_runtime::consts::YINZ_CACHE_DIR, &session)?;
    let _lock = writer_lock(&path)?;
    let mut memory = Memory::load(&path, &session)?;
    memory.conversation = None;
    memory.save(&path)?;
    let mut stream = None;
    let mut ready = false;
    let mut due = Instant::now() + Duration::from_secs(2);
    let mut retry = Instant::now();
    let mut response: Option<mpsc::Receiver<AgentResponse>> = None;
    let mut status_reply: Option<mpsc::Receiver<AgentResponse>> = None;
    let mut status_due = Instant::now();
    let mut reconcile = VecDeque::new();
    let mut reconcile_reply: Option<(AgentRequest, mpsc::Receiver<AgentResponse>)> = None;
    let mut input_revision = None;
    let mut mother_agent: Option<yinz_types::agent::AgentInfo> = None;
    let mut response_delivery = false;
    let mut last_status = None;
    let mut failed = false;
    let mut activity = BTreeMap::<String, Value>::new();
    loop {
        match commands.try_recv() {
            Ok((request, target, delivery_blocked)) => {
                let reconnect = matches!(request, MotherRequest::Configure { .. });
                let result = if matches!(request, MotherRequest::Resolve { .. })
                    && response.is_some()
                {
                    Err(anyhow!("original supervision operation is still in flight; wait for its result before resolving"))
                } else if matches!(request, MotherRequest::Status) {
                    Ok(json!(memory.status(ready, failed)))
                } else if matches!(request, MotherRequest::Overview) {
                    let mut overview = memory.overview(ready, failed);
                    overview["mother"] = json!({
                        "pane_id": pane.to_string(),
                        "conversation_id": memory.conversation,
                        "host_id": yinz_runtime::host_identity::host_id(),
                        "yinz_session_name": session,
                        "kind": mother_agent.as_ref().map(|agent| &agent.kind),
                        "state": mother_agent.as_ref().map(|agent| agent.state),
                        "needs_input": mother_agent.as_ref().is_some_and(|agent| agent.is_waiting_on_a_person()),
                    });
                    overview["operation_in_flight"] = json!(response.is_some());
                    overview["delivery_blocked"] = json!(delivery_blocked);
                    Ok(overview)
                } else {
                    memory.persist_command(request, &path)
                };
                let mut response = AgentResponse::new(if result.is_ok() {
                    AgentOutcome::Ok
                } else {
                    AgentOutcome::Rejected
                });
                match result {
                    Ok(value) => {
                        response.output = Some(value.to_string());
                        if reconnect {
                            stream = None;
                            ready = false;
                        }
                    },
                    Err(error) => response.message = Some(format!("{error:#}")),
                }
                reply(&senders, target, response);
            },
            Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            Err(mpsc::TryRecvError::Empty) => {},
        }
        let mut enriched = false;
        while let Ok(observation) = observations.try_recv() {
            if let Some(pane) = observation["pane_id"].as_str() {
                let old = activity.entry(pane.to_owned()).or_default();
                merge_activity(old, &observation);
            }
            if activity.len() > JOURNAL_RECORDS {
                activity.clear();
                observation_gap.store(true, Ordering::Relaxed);
            }
        }
        for (pane, row) in &mut memory.roster {
            if let Some(observation) = activity.get(pane) {
                enriched |= enrich_row(row, observation);
            }
        }
        if observation_gap.swap(false, Ordering::Relaxed) {
            memory.gap = true;
            memory.cursor = None;
            memory.batch(
                "Private activity observation gap: reconcile live agents and archives".into(),
                true,
            );
            stream = None;
            ready = false;
            enriched = true;
        }
        if enriched {
            memory.save(&path)?;
        }
        if stream.is_none() && Instant::now() >= retry {
            match Stream::connect(&session, &memory) {
                Ok(connected) => {
                    stream = Some(connected);
                    ready = false;
                },
                Err(error) => {
                    log::error!("Mother stream: {error:#}");
                    retry = Instant::now() + Duration::from_secs(2);
                },
            }
        }
        if let Some(connected) = stream.as_mut() {
            match connected.receiver.recv_server_msg() {
                IpcRecv::Msg(ServerToClientMsg::EventUpdate { event_json, .. }, _) => {
                    let event: Value = serde_json::from_str(&event_json)
                        .context("decode Mother lifecycle fact")?;
                    let was_ready = event["event_type"] == "ready";
                    let urgent = event["pane_id"] != pane.to_string()
                        && memory.selects_wake(event["event_type"].as_str().unwrap_or_default())
                        && matches!(
                            event["event_type"].as_str(),
                            Some("needs_input" | "turn_failed")
                        );
                    let alert_id = format!(
                        "{}:{}:{}:{}:{}:{}:{}",
                        event["server_epoch"],
                        event["sequence"],
                        event["pane_generation"],
                        event["conversation_id"],
                        event["event_type"],
                        event["submission_id"],
                        event["turn_id"]
                    );
                    let alert = urgent && !memory.alerts.contains(&alert_id);
                    let alert_text = format!(
                        "Mother: {} {}",
                        event["pane_id"].as_str().unwrap_or("agent"),
                        event["event_type"].as_str().unwrap_or("needs attention")
                    );
                    let had_queued = memory.pending.iter().any(|b| !b.attempted);
                    let mut next = memory.clone();
                    if next.event(event, pane) {
                        if alert {
                            next.alerts.push_back(alert_id);
                        }
                        while next.alerts.len() > 256 {
                            next.alerts.pop_front();
                        }
                        match next.save(&path) {
                            Ok(()) => {
                                if !had_queued && next.pending.iter().any(|b| !b.attempted) {
                                    due = Instant::now() + Duration::from_secs(2);
                                }
                                if urgent {
                                    due = Instant::now();
                                }
                                let overflow = next.gap && !memory.gap;
                                memory = next;
                                failed = false;
                                if alert {
                                    senders
                                        .send_to_screen(ScreenInstruction::Notify(
                                            alert_text,
                                            Some(pane),
                                            None,
                                            None,
                                        ))
                                        .context("notify Mother blocker")?;
                                }
                                if overflow && !was_ready {
                                    memory.cursor = None;
                                    memory.save(&path)?;
                                    stream = None;
                                    ready = false;
                                }
                            },
                            Err(error) => {
                                log::error!("Mother persistence: {error:#}");
                                failed = true;
                                stream = None;
                                ready = false;
                                retry = Instant::now() + Duration::from_secs(2);
                            },
                        }
                    }
                    if was_ready && !failed {
                        ready = true;
                        reconcile.clear();
                        for (pane, row) in &memory.roster {
                            if let (Ok(pane_id), Some(submission)) =
                                (pane.parse::<PaneId>(), row["submission_id"].as_str())
                            {
                                reconcile.push_back(AgentRequest::WaitTurn {
                                    pane_id,
                                    expected_session_id: row["conversation_id"]
                                        .as_str()
                                        .map(str::to_owned),
                                    submission_id: submission.to_owned(),
                                    timeout_ms: 1,
                                });
                            }
                        }
                        memory.gap = false;
                        memory.save(&path)?;
                    }
                },
                IpcRecv::Idle => {},
                IpcRecv::Msg(ServerToClientMsg::LogError { lines }, _) => {
                    log::error!("Mother subscription: {}", lines.join("; "));
                    stream = None;
                    ready = false;
                    retry = Instant::now() + Duration::from_secs(2);
                },
                IpcRecv::Eof | IpcRecv::Malformed => {
                    stream = None;
                    ready = false;
                    retry = Instant::now() + Duration::from_secs(2);
                },
                _ => {},
            }
        } else {
            std::thread::sleep(TICK);
        }
        if ready && reconcile_reply.is_none() {
            if let Some(request) = reconcile.pop_front() {
                let (tx, rx) = mpsc::channel();
                senders
                    .send_to_screen(ScreenInstruction::AgentControlReply(request.clone(), tx))
                    .context("reconcile roster submission")?;
                reconcile_reply = Some((request, rx));
            }
        }
        if let Some((request, rx)) = &reconcile_reply {
            if let Ok(result) = rx.try_recv() {
                if memory.reconcile(request, result) {
                    memory.save(&path)?;
                }
                reconcile_reply = None;
            }
        }
        if status_reply.is_none() && Instant::now() >= status_due {
            let (tx, rx) = mpsc::channel();
            senders
                .send_to_screen(ScreenInstruction::AgentControlReply(
                    AgentRequest::Status {
                        pane_id: pane,
                        expected_session_id: None,
                    },
                    tx,
                ))
                .context("inspect Mother identity")?;
            status_reply = Some(rx);
        }
        if let Some(rx) = &status_reply {
            if let Ok(result) = rx.try_recv() {
                if let Some(status) = result.agents.first() {
                    input_revision = status.input_revision;
                    mother_agent = Some(status.agent.clone());
                    if status.agent.session_id.is_some()
                        && memory.conversation != status.agent.session_id
                    {
                        let mut next = memory.clone();
                        next.conversation = status.agent.session_id.clone();
                        next.batch(
                            "Bootstrap Mother: load memory and reconcile live agents".into(),
                            false,
                        );
                        next.save(&path)?;
                        memory = next;
                        due = Instant::now() + Duration::from_secs(2);
                    }
                }
                status_reply = None;
                status_due = Instant::now() + Duration::from_secs(2);
            }
        }
        if let Some(rx) = &response {
            let received = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    let mut result = AgentResponse::new(AgentOutcome::StatusUnavailable);
                    result.submission_id = memory.receipts.back().map(|r| r.submission.clone());
                    Some(result)
                },
                Err(mpsc::TryRecvError::Empty) => None,
            };
            if let Some(result) = received {
                let mut next = memory.clone();
                next.receipt_response(result, response_delivery);
                next.save(&path)?;
                memory = next;
                response = None;
                due = Instant::now() + Duration::from_secs(2);
            }
        }
        if ready
            && !failed
            && reconcile.is_empty()
            && reconcile_reply.is_none()
            && response.is_none()
            && Instant::now() >= due
        {
            if let Some(receipt) = memory.receipts.back().filter(|r| !r.settled) {
                let (tx, rx) = mpsc::channel();
                senders
                    .send_to_screen(ScreenInstruction::AgentControlReply(
                        AgentRequest::WaitTurn {
                            pane_id: pane,
                            expected_session_id: Some(receipt.conversation.clone()),
                            submission_id: receipt.submission.clone(),
                            timeout_ms: 1000,
                        },
                        tx,
                    ))
                    .context("reconcile Mother submission")?;
                response = Some(rx);
                response_delivery = false;
            } else if !memory.paused {
                if let (Some(batch), Some(conversation), Some(input_revision)) = (
                    memory.pending.iter().find(|b| !b.attempted).cloned(),
                    memory.conversation.clone(),
                    input_revision,
                ) {
                    let submission = format!("mother-{}", batch.id);
                    let prompt = format!("Mother supervision batch {} ({} changes): {}. Read the local yinz-mother skill, inspect `yinz mother inbox`, reconcile correlated results and archives, update `yinz mother checkpoint`, report meaningful changes here, and ack this batch after saving memory. Continue only authorized assignments. Missing evidence is unknown.", batch.id, batch.events, batch.summary.join("; "));
                    let mut next = memory.clone();
                    next.delivery_blocked = None;
                    if let Some(b) = next.pending.iter_mut().find(|b| b.id == batch.id) {
                        b.attempted = true;
                        b.frozen = true;
                    }
                    next.receipts.push_back(Receipt {
                        batch: batch.id,
                        conversation: conversation.clone(),
                        submission: submission.clone(),
                        response: None,
                        settled: false,
                    });
                    while next.receipts.len() > 128 {
                        next.receipts.pop_front();
                    }
                    next.save(&path)?;
                    memory = next;
                    let (tx, rx) = mpsc::channel();
                    senders
                        .send_to_screen(ScreenInstruction::MotherWake {
                            generation: generation.clone(),
                            conversation,
                            input_revision,
                            submission,
                            prompt,
                            reply: tx,
                        })
                        .context("deliver Mother batch")?;
                    response = Some(rx);
                    response_delivery = true;
                }
            }
        }
        let status = memory.status(ready, failed);
        if last_status.as_ref() != Some(&status) {
            senders
                .send_to_screen(ScreenInstruction::MotherStatus(
                    generation.clone(),
                    status.clone(),
                ))
                .context("publish Mother status")?;
            last_status = Some(status);
        }
    }
}

impl Screen {
    pub(super) fn sync_mother_supervisor(&mut self) {
        let pane = self.live_mother();
        if let Some(supervisor) = self
            .mother_supervisor
            .as_ref()
            .filter(|s| Some(s.pane) == pane && s.session == self.session_name.as_str())
        {
            if !supervisor.running() {
                self.mother_status = Some(MotherStatus {
                    state: "degraded".into(),
                    ..Default::default()
                });
            }
            return;
        }
        self.mother_supervisor = None;
        self.mother_status = None;
        let Some(pane) = pane else {
            return;
        };
        let generation = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = mpsc::channel();
        let (observations, observed) = mpsc::sync_channel(256);
        let observation_gap = Arc::new(AtomicBool::new(false));
        let worker_gap = observation_gap.clone();
        let session = self.session_name.to_string();
        let worker_session = session.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = stopped.clone();
        let senders = self.bus.senders.clone();
        let id = generation.clone();
        let thread = match std::thread::Builder::new()
            .name("mother-supervisor".into())
            .spawn(move || {
                if let Err(error) = run(
                    worker_session,
                    pane,
                    id.clone(),
                    senders.clone(),
                    &rx,
                    observed,
                    worker_gap,
                ) {
                    worker_stopped.store(true, Ordering::Release);
                    log::error!("Mother supervisor stopped: {error:#}");
                    senders
                        .send_to_screen(ScreenInstruction::MotherStatus(
                            id,
                            MotherStatus {
                                state: "degraded".into(),
                                ..Default::default()
                            },
                        ))
                        .context("report Mother failure")
                        .non_fatal();
                    // Keep rejecting queued and racing commands until resume replaces this worker.
                    for (_, target, _) in rx {
                        let mut response = AgentResponse::new(AgentOutcome::Rejected);
                        response.message = Some(format!(
                            "Mother supervisor stopped: {error:#}; use `yinz mother resume` to retry"
                        ));
                        reply(&senders, target, response);
                    }
                }
            }) {
            Ok(thread) => Some(thread),
            Err(error) => {
                stopped.store(true, Ordering::Release);
                log::error!("Cannot start Mother supervisor: {error}");
                None
            },
        };
        self.mother_supervisor = Some(Supervisor {
            generation,
            pane,
            session,
            commands: Some(tx),
            thread,
            stopped,
            observations,
            observation_gap,
        });
    }

    pub(super) fn observe_mother_activity(
        &self,
        event: &yinz_types::agent::AgentEvent,
        worker: bool,
    ) {
        let pane = PaneId::Terminal(event.pane_id);
        let Some(supervisor) = self.mother_supervisor.as_ref().filter(|s| s.pane != pane) else {
            return;
        };
        if worker {
            return;
        }
        let observation = json!({
            "pane_id":pane.to_string(), "conversation_id":event.session_id,
            "assignment":event.prompt.as_ref().map(|s| s.chars().take(2048).collect::<String>()),
            "current_activity":event.detail.as_ref().map(|s| s.chars().take(2048).collect::<String>()),
            "last_message":event.last_message.as_ref().map(|s| s.chars().take(4096).collect::<String>()),
            "waiting_on":self.get_pane_info(pane).and_then(|p| p.agent).filter(|a| a.session_id == event.session_id).and_then(|a| a.waiting_on).map(|s| s.chars().take(2048).collect::<String>()),
        });
        if let Err(mpsc::TrySendError::Full(_)) = supervisor.observations.try_send(observation) {
            supervisor.observation_gap.store(true, Ordering::Relaxed);
        }
    }

    pub(super) fn mother_command(&mut self, request: MotherRequest, target: Reply) {
        if let MotherRequest::Resolve { submission_id, .. } = &request {
            if self.mother_submission_in_flight(submission_id) {
                let mut response = AgentResponse::new(AgentOutcome::Rejected);
                response.message = Some("Original operation is still in flight. Wait for it to settle before resolving.".into());
                reply(&self.bus.senders, target, response);
                return;
            }
        }
        if matches!(request, MotherRequest::Resume)
            && self.mother_supervisor.as_ref().is_none_or(|s| !s.running())
        {
            self.mother_supervisor = None;
            self.sync_mother_supervisor();
        }
        let delivery_blocked = self
            .live_mother()
            .and_then(|pane| self.mother_delivery_blocker(pane));
        let result = self
            .mother_supervisor
            .as_ref()
            .context("Mother is not running")
            .and_then(|s| {
                if !s.running() {
                    bail!("Mother supervisor stopped; use `yinz mother resume` to retry");
                }
                s.commands
                    .as_ref()
                    .context("Mother supervisor stopped")?
                    .send((request, target.clone(), delivery_blocked))
                    .context("Mother supervisor is unavailable; use `yinz mother resume` to retry")
            });
        if let Err(error) = result {
            let mut response = AgentResponse::new(AgentOutcome::Rejected);
            response.message = Some(format!("{error:#}"));
            self.agent_reply(&target, response);
        }
    }
}

#[cfg(test)]
#[path = "../unit/mother_supervisor.rs"]
mod tests;

#[cfg(test)]
impl Supervisor {
    pub(super) fn for_test(pane: PaneId) -> Self {
        let (tx, _) = mpsc::channel();
        Self {
            generation: "test-generation".into(),
            pane,
            session: String::new(),
            commands: Some(tx),
            thread: None,
            stopped: Arc::new(AtomicBool::new(false)),
            observations: mpsc::sync_channel(1).0,
            observation_gap: Arc::new(AtomicBool::new(false)),
        }
    }
}
