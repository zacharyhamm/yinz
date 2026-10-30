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
        let mut reply = execute(
            request,
            &HistoryAction::Run {
                id: id.clone(),
                from: Some(history::anchored_from(*revision)),
                query: String::new(),
            },
            senders,
            data,
        )?;
        if let AgentReply::History(HistoryReply::Run { rows, .. }) = &mut reply {
            // Show the hit immediately, then backfill the whole run. Only the
            // initial Hit restarts at 1; Run continuations advance normally.
            rows.retain(|row| row.id != "continue");
            rows.push(row(
                "continue".into(),
                "Loading history…",
                String::new(),
                run_action(id, Some(1)),
            ));
        }
        return Ok(reply);
    }
    let timestamp = |ms| request.timezone.timestamp(ms);
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
            let metadata = format!("Run: {}\nProvider: {}\nTitle: {}\nAgent session: {}\nSession: {}\nHost: {}\nPane: {}\nDirectory: {}\nExecution target: {}\nStarted: {}\nEnded: {}\nOutcome: {:?}\nRevision: {}\nSummary: {}\nPrevious run: {}\nOriginal run: {}",
                h.id, h.kind, h.title, h.session_id, h.yinz_session_name, h.host_id, h.pane_id,
                h.cwd.as_deref().unwrap_or("Unavailable"), h.host.as_deref().unwrap_or("local"), timestamp(Some(h.started_at_ms)),
                timestamp(h.ended_at_ms), h.outcome, h.revision,
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
                // Opening a run reads only its tail. Backfill is an explicit
                // action so a long run does not keep loading on every open.
                if from.is_none() && page.earlier {
                    rows.push(row(
                        "earlier".into(),
                        "Load earlier messages",
                        String::new(),
                        run_action(id, Some(1)),
                    ));
                }
                if let Some(next) = page.next_revision {
                    rows.push(row(
                        "continue".into(),
                        "Loading history…",
                        String::new(),
                        run_action(id, Some(next)),
                    ));
                }
                status = format!("{} retained events · Search filters this run · previews may be shortened; Enter expands", page.total);
                for e in page.entries {
                    events.push(HistoryEvent {
                        revision: e.revision,
                        revisions: vec![e.revision],
                        tool_calls: usize::from(
                            e.kind == agent_proto::agents::RunEntryKind::ToolCall,
                        ),
                        created_at_ms: e.created_at_ms,
                        kind: e.kind.label().into(),
                        time: timestamp(e.created_at_ms),
                        truncated: page.truncated_revisions.contains(&e.revision),
                        markdown: e.text.clone(),
                    });
                }

                for event in &events {
                    let action = if event.kind == "tools" {
                        HistoryAction::Tools {
                            id: id.clone(),
                            revisions: event.revisions.clone(),
                        }
                    } else {
                        HistoryAction::Event {
                            id: id.clone(),
                            revision: event.revision,
                        }
                    };
                    rows.push(row(
                        format!("event:{}", event.revision),
                        format!("{} · {}", event.revision, event.kind),
                        String::new(),
                        action,
                    ));
                }
            } else {
                let (hits, next) =
                    history::search_run_page(&backend, id, query, from.unwrap_or(1))?;
                if let Some(next) = next {
                    rows.push(row(
                        "continue".into(),
                        "Searching history…",
                        String::new(),
                        HistoryAction::Run {
                            id: id.clone(),
                            from: Some(next),
                            query: query.clone(),
                        },
                    ));
                }
                status = "Matching retained events".into();
                for hit in hits {
                    events.push(HistoryEvent {
                        revision: hit.revision,
                        revisions: vec![hit.revision],
                        tool_calls: 0,
                        created_at_ms: hit.created_at_ms,
                        kind: hit.kind.label().into(),
                        time: timestamp(hit.created_at_ms),
                        markdown: if matches!(
                            hit.kind,
                            agent_proto::agents::RunEntryKind::ToolCall
                                | agent_proto::agents::RunEntryKind::ToolResult
                        ) {
                            hit.snippet.clone()
                        } else {
                            yinz_tile_markdown::escape_inline(&hit.snippet)
                        },
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
                    task: h.title.clone(),
                    session_id: if h.session_id.is_empty() {
                        format!("run {}", h.id)
                    } else {
                        format!("session {}", h.session_id)
                    },
                    project: agent_proto::agents::checkout_location(
                        h.host.as_deref(),
                        h.cwd.as_deref(),
                    )
                    .and_then(|location| project_data::projects::for_cwd(&projects, &location))
                    .map(|project| project.name.clone())
                    .unwrap_or_default(),
                    usage: None,
                    ..Default::default()
                },
                events,
                rows,
                status,
            }
        },
        HistoryAction::Tools { id, revisions } => {
            // Merged groups can exceed one fetch's byte budget. Paginate in row-ID
            // order; entries_from_rows restores revision order for the reader.
            let fetched = store::fetch_all(
                &backend,
                FetchRequest::entity(agent_proto::agents::rows::ENTRY_ENTITY)
                    .filter(Pred::eq("run_id", id.as_str()))
                    .filter(Pred::is_in(
                        "revision",
                        revisions
                            .iter()
                            .map(|r| DataValue::Int(*r as i64))
                            .collect::<Vec<_>>(),
                    )),
            )?;
            let entries = agent_proto::agents::rows::entries_from_rows(&fetched)
                .remove(id)
                .unwrap_or_default();
            if entries.len() != revisions.len() {
                return Err("Tool event no longer retained".into());
            }
            HistoryReply::Reader {
                header: "tool_call".into(),
                markdown: entries
                    .iter()
                    .map(|e| {
                        let tool = e
                            .tool
                            .as_ref()
                            .map(serde_json::to_string_pretty)
                            .transpose()
                            .map_err(|e| format!("Cannot format tool details: {e}"))?;
                        let mut text =
                            format!("{} · revision {}:\n{}", e.kind.label(), e.revision, e.text);
                        if let Some(tool) = tool {
                            text.push_str("\n\n");
                            text.push_str(&tool);
                        }
                        Ok(text)
                    })
                    .collect::<Result<Vec<_>, String>>()?
                    .join("\n\n"),
                tool: String::new(),
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
                header: entry.kind.label().into(),
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
