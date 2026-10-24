//! Read-only archive access on the native palette worker.
use super::super::{agent_control, agent_launch::PreparedAgent};
use super::launch;
use agent_proto::{
    agents::{AgentRunHeader, RunOutcome},
    run_history as history, store,
};
use std::path::Path;
use std::sync::atomic;
use yinz_bus::command_palette::{AgentReply, AgentRequest};
use yinz_bus::command_palette::{
    HistoryAction, HistoryEvent, HistoryHeader, HistoryReply, HistoryRow,
};
use yinz_bus::thread_bus::ThreadSenders;
use yinz_storage::data_access::DataAccess;
use yinz_tile::prelude::*;
use yinz_types::agent::AgentSpawnSpec;

struct Reader<'a>(&'a DataAccess);
impl store::Backend for Reader<'_> {
    fn fetch(&self, request: &FetchRequest) -> Result<DataRows, String> {
        self.0
            .plugin_data()
            .fetch(store::SHARED, request)
            .or_else(|e| match e {
                DataError::UnknownEntity(_) => Ok(Default::default()),
                e => Err(e),
            })
            .map_err(|e| e.to_string())
    }
    fn count(&self, request: &FetchRequest) -> Result<u64, String> {
        self.0
            .plugin_data()
            .count(store::SHARED, request)
            .or_else(|e| match e {
                DataError::UnknownEntity(_) => Ok(0),
                e => Err(e),
            })
            .map_err(|e| e.to_string())
    }
    fn define(&self, _: &EntityDef) -> Result<DataSchemaInfo, String> {
        Err("Read-only archive".into())
    }
    fn apply(&self, _: Vec<Mutation>) -> Result<ApplyReport, WriteError> {
        Err(WriteError::Failed("Read-only archive".into()))
    }
    fn watch(&self, _: &FetchRequest, _: WatchMode) -> Result<DataWatchHandle, String> {
        Err("Read-only archive".into())
    }
}

fn row(
    id: String,
    label: impl Into<String>,
    description: String,
    action: HistoryAction,
) -> HistoryRow {
    let label = label.into();
    HistoryRow {
        label: if label.is_empty() { id.clone() } else { label },
        id,
        description,
        action,
    }
}
fn timestamp(ms: Option<i64>) -> String {
    ms.and_then(chrono::DateTime::from_timestamp_millis)
        .map(|time| time.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| "Time unavailable".into())
}
fn run_action(id: &str, from: Option<u64>) -> HistoryAction {
    HistoryAction::Run {
        id: id.into(),
        from,
        query: String::new(),
    }
}

pub(super) fn execute(
    request: &AgentRequest,
    action: &HistoryAction,
    senders: &ThreadSenders,
    data: &DataAccess,
) -> Result<AgentReply, String> {
    if !request.active.load(atomic::Ordering::Acquire) {
        return Err("Archive request cancelled".into());
    }
    if let HistoryAction::Hit { id, revision } = action {
        return execute(
            request,
            &HistoryAction::Run {
                id: id.clone(),
                from: Some(history::anchored_from(*revision)),
                query: String::new(),
            },
            senders,
            data,
        );
    }
    let backend = Reader(data);
    let projects = project_data::projects::from_rows(&agent_control::projects(data)?);
    let reply = match action {
        HistoryAction::Hit { .. } => unreachable!("handled above"),
        HistoryAction::Search(query) => {
            let keeps = |h: &AgentRunHeader| {
                if h.outcome == RunOutcome::Active {
                    return false;
                }
                let mut metadata: std::collections::BTreeMap<String, String> =
                    std::collections::BTreeMap::from([
                        ("provider".into(), h.kind.clone()),
                        ("session".into(), h.yinz_session_name.to_string()),
                        ("host".into(), h.host_id.to_string()),
                        ("run".into(), h.id.clone()),
                    ]);
                if let Some(location) =
                    agent_proto::agents::checkout_location(h.host.as_deref(), h.cwd.as_deref())
                    && let Some(p) = project_data::projects::for_cwd(&projects, &location)
                {
                    metadata.insert("project".into(), p.name.clone());
                }
                query.filters.iter().all(|(k, v)| {
                    if k == "since" {
                        v.parse::<i64>().is_ok_and(|since| h.started_at_ms >= since)
                    } else if k == "host"
                        && h.host_id.is_empty()
                        && query.filters.contains_key("session")
                    {
                        true
                    } else {
                        metadata
                            .get(k)
                            .is_some_and(|actual| actual.eq_ignore_ascii_case(v))
                    }
                })
            };
            let mut rows = Vec::new();
            let partial;
            if query.text.is_empty() {
                let mut offset = 0;
                loop {
                    // Text filters use `keeps` in both paths: database equality is case-sensitive.
                    let page = history::list_runs(
                        &backend,
                        &history::Filter {
                            active: Some(false),
                            since_ms: query.filters.get("since").and_then(|v| v.parse().ok()),
                            ..Default::default()
                        },
                        50,
                        offset,
                    )?;
                    offset += page.scanned;
                    for h in page.headers.iter().filter(|h| keeps(h)) {
                        rows.push(row(
                            h.id.clone(),
                            &h.title,
                            format!(
                                "{} · {} · {:?} · {}",
                                h.kind,
                                h.yinz_session_name,
                                h.outcome,
                                h.cwd.as_deref().unwrap_or("Directory unavailable")
                            ),
                            run_action(&h.id, None),
                        ));
                    }
                    if rows.len() > 50 || !page.more || offset >= 4000 || page.scanned == 0 {
                        partial = page.more || rows.len() > 50;
                        rows.truncate(50);
                        break;
                    }
                }
            } else {
                let found = history::search_runs_filtered(&backend, &query.text, keeps)?;
                partial = found.truncated;
                for hit in found.hits {
                    if let Some(h) = found.runs.get(&hit.run_id) {
                        rows.push(row(
                            h.id.clone(),
                            &h.title,
                            format!(
                                "{} · {} · {:?} · {}",
                                h.kind, h.yinz_session_name, h.outcome, hit.snippet
                            ),
                            HistoryAction::Hit {
                                id: h.id.clone(),
                                revision: hit.revision,
                            },
                        ));
                    }
                }
            }
            HistoryReply::Rows {
                rows,
                status: if partial {
                    "Partial results: 50-run or 4,000-row scan limit; narrow filters"
                } else {
                    "Ended runs · all stored hosts and sessions"
                }
                .into(),
            }
        },
        HistoryAction::Run { id, from, query } => {
            let Some(h) = history::run_header(&backend, id)? else {
                return Ok(AgentReply::History(HistoryReply::Run {
                    header: HistoryHeader {
                        name: "Recorded history unavailable".into(),
                        details: "No recorded history".into(),
                        ..Default::default()
                    },
                    events: vec![],
                    rows: vec![],
                    status: "No recorded history".into(),
                }));
            };
            let metadata = format!("Run: {}\nProvider: {}\nTitle: {}\nAgent session: {}\nSession: {}\nHost: {}\nPane: {}\nDirectory: {}\nExecution target: {}\nStarted (epoch ms): {}\nEnded (epoch ms): {}\nOutcome: {:?}\nRevision: {}\nSummary: {}\nPrevious run: {}\nOriginal run: {}",
                h.id, h.kind, h.title, h.session_id, h.yinz_session_name, h.host_id, h.pane_id,
                h.cwd.as_deref().unwrap_or("Unavailable"), h.host.as_deref().unwrap_or("local"), h.started_at_ms,
                h.ended_at_ms.map(|v| v.to_string()).unwrap_or_else(|| "Unavailable".into()), h.outcome, h.revision,
                h.summary.as_ref().map(|s| s.text.as_str()).unwrap_or("Unavailable"), h.previous_run_id.as_deref().unwrap_or("Unavailable"), h.origin_run_id.as_deref().unwrap_or("Unavailable"));
            let mut rows = Vec::new();
            if h.outcome != RunOutcome::Active {
                rows.push(row(
                    "resume".into(),
                    "Resume",
                    "Launch and focus a new pane in the recorded directory".into(),
                    HistoryAction::Resume { id: id.clone() },
                ));
            }
            for (key, label, target) in [
                ("previous", "Previous run", &h.previous_run_id),
                ("origin", "Original run", &h.origin_run_id),
            ] {
                if let Some(target) = target {
                    rows.push(row(
                        key.into(),
                        label,
                        target.clone(),
                        run_action(target, None),
                    ));
                }
            }
            rows.push(row(
                "latest".into(),
                "Latest / Refresh",
                String::new(),
                run_action(id, None),
            ));
            let mut events = Vec::new();
            let status;
            if query.is_empty() {
                let page = history::entry_page(&backend, id, *from, 30)?;
                if page.earlier {
                    let start = page
                        .entries
                        .first()
                        .map(|e| e.revision)
                        .or(*from)
                        .unwrap_or(1);
                    let earlier = store::Backend::fetch(
                        &backend,
                        &FetchRequest::entity(agent_proto::agents::rows::ENTRY_ENTITY)
                            .select(["revision"])
                            .filter(Pred::eq("run_id", id.as_str()))
                            .filter(Pred::lt("revision", start as i64))
                            .sort_by("revision", SortDir::Desc)
                            .limit(30),
                    )?;
                    if let Some(DataValue::Int(revision)) = earlier
                        .rows
                        .last()
                        .and_then(|row| earlier.get(row, "revision"))
                    {
                        rows.push(row(
                            "older".into(),
                            "Older",
                            String::new(),
                            run_action(id, Some(*revision as u64)),
                        ));
                    }
                }
                if let Some(next) = page.next_revision {
                    rows.push(row(
                        "newer".into(),
                        "Newer",
                        String::new(),
                        run_action(id, Some(next)),
                    ));
                }
                status = format!("{} retained events · type to search this run · previews may be shortened; Enter expands", page.total);
                for e in page.entries {
                    events.push(HistoryEvent {
                        revision: e.revision,
                        kind: e.kind.label().into(),
                        time: timestamp(e.created_at_ms),
                        truncated: page.truncated_revisions.contains(&e.revision),
                        markdown: e.text.clone(),
                    });
                    rows.push(row(
                        format!("event:{}", e.revision),
                        format!("{} · {}", e.revision, e.kind.label()),
                        String::new(),
                        HistoryAction::Event {
                            id: id.clone(),
                            revision: e.revision,
                        },
                    ));
                }
            } else {
                let found =
                    history::search(&backend, &Default::default(), Some(id), query, false, 50)?;
                status = if found.truncated {
                    "Partial results: search limit reached"
                } else {
                    "Matching retained events"
                }
                .into();
                for hit in found.hits {
                    events.push(HistoryEvent {
                        revision: hit.revision,
                        kind: hit.kind.label().into(),
                        time: timestamp(hit.created_at_ms),
                        markdown: yinz_tile_markdown::escape_inline(&hit.snippet),
                        truncated: true,
                    });
                    rows.push(row(
                        format!("event:{}", hit.revision),
                        format!("{} · {}", hit.revision, hit.kind.label()),
                        hit.snippet,
                        HistoryAction::Hit {
                            id: id.clone(),
                            revision: hit.revision,
                        },
                    ));
                }
            }
            events.sort_by_key(|event| event.revision);
            HistoryReply::Run {
                header: HistoryHeader {
                    name: if h.title.is_empty() {
                        h.id.clone()
                    } else {
                        h.title.clone()
                    },
                    status: format!("{} · {:?}", h.kind, h.outcome),
                    location: format!(
                        "{} · {}",
                        h.yinz_session_name,
                        h.cwd.as_deref().unwrap_or("Directory unavailable")
                    ),
                    started: timestamp(Some(h.started_at_ms)),
                    details: metadata,
                },
                events,
                rows,
                status,
            }
        },
        HistoryAction::Event { id, revision } => {
            // Read the exact retained event without the multi-entry preview budget.
            let fetched = store::Backend::fetch(
                &backend,
                &FetchRequest::entity(agent_proto::agents::rows::ENTRY_ENTITY)
                    .filter(Pred::eq("run_id", id.as_str()))
                    .filter(Pred::eq("revision", *revision as i64))
                    .limit(1),
            )?;
            let entry = agent_proto::agents::rows::entries_from_rows(&fetched)
                .remove(id)
                .and_then(|mut entries| entries.pop())
                .ok_or("Event no longer retained")?;
            if fetched.truncated {
                return Err("Retained event exceeds the store read limit".into());
            }
            let tool = entry
                .tool
                .as_ref()
                .map(serde_json::to_string_pretty)
                .transpose()
                .map_err(|e| format!("Cannot format tool details: {e}"))?
                .unwrap_or_default();
            HistoryReply::Reader {
                header: format!(
                    "Revision: {} · {} · {}",
                    entry.revision,
                    entry.kind.label(),
                    timestamp(entry.created_at_ms)
                ),
                markdown: entry.text,
                tool,
            }
        },
        HistoryAction::Resume { id } => {
            let h = history::run_header(&backend, id)?.ok_or("Run no longer retained")?;
            if h.outcome == RunOutcome::Active {
                return Err("This run is active; focus its live agent".into());
            }
            if !h.host_id.is_empty() && h.host_id != yinz_runtime::host_identity::host_id() {
                return Err(
                    "Run belongs to an unmapped foreign host; resume on its owning host".into(),
                );
            }
            let cwd = h
                .cwd
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .ok_or("Run has no recorded directory")?;
            let mut command = yinz_runtime::agent::launch::resume_command(
                &h.kind,
                &h.session_id,
                None,
                h.host.as_deref(),
                h.bubblewrap.as_deref(),
            )?;
            // Native launches do not translate Wasm mount paths.
            command.cwd = Some(cwd.into());
            if !command.target.is_remote() {
                if !Path::new(cwd).is_dir() {
                    return Err("Recorded directory no longer exists".into());
                }
                yinz_runtime::agent::pi::prepare_command(&mut command)?;
            } else {
                yinz_runtime::agent::pi::strip_extension(&mut command.args);
            }
            let spec = AgentSpawnSpec {
                kind: Some(h.kind),
                ..Default::default()
            };
            return launch(request, senders, spec, |_| {
                Ok(PreparedAgent {
                    command,
                    cwd: cwd.into(),
                    branch: None,
                    requested_branch: None,
                    reused: false,
                })
            });
        },
    };
    Ok(AgentReply::History(reply))
}
