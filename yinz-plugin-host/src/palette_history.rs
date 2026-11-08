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
    HistoryAction, HistoryEvent, HistoryHeader, HistoryReply, HistoryRow, PromptRecord,
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

/// Read only assistant prose from the current recorded run. A run id alone is
/// insufficient on the shared archive: its header must belong to this host/session.
pub(super) fn last_reply(
    data: &DataAccess,
    source: &yinz_bus::command_palette::AgentRunSource,
) -> Result<Option<yinz_bus::command_palette::AgentReplyPreview>, String> {
    use agent_proto::agents::rows;
    use yinz_bus::command_palette::AgentReplyPreview;

    let backend = Reader(data);
    let owned = FetchRequest::entity(rows::RUN_ENTITY)
        .filter(Pred::eq("uid", source.id.clone()))
        .filter(store::host_or_legacy(&source.host_id))
        .filter(Pred::eq("yinz_session_name", source.session.clone()))
        .select(["uid"])
        .limit(1);
    if store::Backend::fetch(&backend, &owned)?.is_empty() {
        return Ok(None);
    }
    let request = FetchRequest::entity(rows::ENTRY_ENTITY)
        .filter(Pred::eq("run_id", source.id.clone()))
        .filter(store::host_or_legacy(&source.host_id))
        .filter(Pred::is_in("kind", ["message", "result"]))
        .select(["revision", "text"])
        .sort_by("revision", SortDir::Desc)
        .limit(1);
    let mut before = None;
    loop {
        let page = store::Backend::fetch(
            &backend,
            &match before {
                Some(revision) => request.clone().filter(Pred::lt("revision", revision)),
                None => request.clone(),
            },
        )?;
        if page.is_empty() {
            return Ok(None);
        }
        for row in &page.rows {
            if let Some(DataValue::Text(text)) = page.get(row, "text") {
                // Bounded, literal single-line text crosses back to screen. Keep
                // the source truncation flag even if it fits the terminal width.
                let normalized = text
                    .chars()
                    .map(|ch| if ch.is_control() { ' ' } else { ch })
                    .collect::<String>()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                if !normalized.is_empty() {
                    let mut chars = normalized.chars();
                    let text = chars.by_ref().take(512).collect();
                    let truncated = chars.next().is_some();
                    return Ok(Some(AgentReplyPreview { text, truncated }));
                }
            }
        }
        before = page
            .rows
            .last()
            .and_then(|row| match page.get(row, "revision") {
                Some(DataValue::Int(revision)) => Some(*revision),
                _ => None,
            });
        if before.is_none() || !page.truncated {
            return Ok(None);
        }
    }
}

/// Every distinct prompt in the archive, described for the palette's picker.
pub(super) fn prompt_library(data: &DataAccess) -> Result<Vec<PromptRecord>, String> {
    let now_ms = agent_proto::agents::now_ms();
    Ok(history::prompt_library(&Reader(data))?
        .into_iter()
        .map(|prompt| {
            let mut parts = Vec::with_capacity(3);
            parts.extend(prompt.agent);
            parts.extend(
                prompt
                    .last_used_ms
                    .map(|at| agent_proto::agents::short_age(now_ms, at)),
            );
            if prompt.uses > 1 {
                parts.push(format!("{}×", prompt.uses));
            }
            PromptRecord {
                text: prompt.text,
                description: parts.join(" · "),
            }
        })
        .collect())
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
    match action {
        HistoryAction::Run { id, from, query } if query.is_empty() => {
            let mut reply = conversation_view(
                request,
                &Reader(data),
                id,
                ConversationRead {
                    after: *from,
                    ..Default::default()
                },
            )?;
            if from.is_some() {
                // Refresh activity counts at the tail while delivering newly stored
                // revisions, including repaired messages with old source timestamps.
                if let (
                    AgentReply::History(HistoryReply::Run { events, rows, .. }),
                    AgentReply::History(HistoryReply::Run {
                        events: tail,
                        rows: tail_rows,
                        ..
                    }),
                ) = (
                    &mut reply,
                    conversation_view(request, &Reader(data), id, ConversationRead::default())?,
                ) {
                    events.extend(tail);
                    rows.extend(tail_rows.into_iter().filter(|r| r.id != "received"));
                }
            }
            return Ok(reply);
        },
        HistoryAction::Conversation { id, worker, before } => {
            return conversation_view(
                request,
                &Reader(data),
                id,
                ConversationRead {
                    worker: worker.as_deref(),
                    before: *before,
                    ..Default::default()
                },
            )
        },
        HistoryAction::Tools { id, range, before } => {
            return conversation_view(
                request,
                &Reader(data),
                id,
                ConversationRead {
                    worker: range.worker.as_deref(),
                    before: *before,
                    tools: Some(range),
                    ..Default::default()
                },
            )
        },
        HistoryAction::Workers { id } => {
            let workers = agent_proto::conversation::workers(&Reader(data), id)?;
            return Ok(AgentReply::History(HistoryReply::Rows {
                rows: workers
                    .into_iter()
                    .map(|worker| {
                        row(
                            worker.clone(),
                            format!("Worker {worker}"),
                            String::new(),
                            HistoryAction::Conversation {
                                id: id.clone(),
                                worker: Some(worker),
                                before: None,
                            },
                        )
                    })
                    .collect(),
                status: "Workers · Enter opens attributed conversation and activity".into(),
            }));
        },
        HistoryAction::Hit { id, revision } => {
            let fetched = store::Backend::fetch(
                &Reader(data),
                &FetchRequest::entity("agent_run_entry")
                    .filter(Pred::eq("run_id", id.as_str()))
                    .filter(Pred::eq("revision", *revision as i64))
                    .limit(1),
            )?;
            let entry = agent_proto::agents::rows::entries_from_rows(&fetched)
                .remove(id)
                .and_then(|mut es| es.pop())
                .ok_or("Event no longer retained")?;
            let before = Some(agent_proto::conversation::Cursor {
                created_at_ms: entry.created_at_ms,
                revision: entry.revision.saturating_add(1),
            });
            let range = matches!(
                entry.kind,
                agent_proto::agents::RunEntryKind::ToolCall
                    | agent_proto::agents::RunEntryKind::ToolResult
            )
            .then(|| agent_proto::conversation::Range {
                start: None,
                end: None,
                worker: entry.worker(),
            });
            let mut reply = conversation_view(
                request,
                &Reader(data),
                id,
                ConversationRead {
                    worker: entry.worker().as_deref(),
                    before,
                    tools: range.as_ref(),
                    after: None,
                },
            )?;
            if let AgentReply::History(HistoryReply::Run { rows, .. }) = &mut reply {
                rows.push(row(
                    "context".into(),
                    "",
                    String::new(),
                    match range {
                        Some(range) => HistoryAction::Tools {
                            id: id.clone(),
                            range,
                            before,
                        },
                        None => HistoryAction::Conversation {
                            id: id.clone(),
                            worker: entry.worker(),
                            before,
                        },
                    },
                ));
            }
            return Ok(reply);
        },
        _ => {},
    }
    let timestamp = |ms| request.timezone.timestamp(ms);
    let backend = Reader(data);
    let projects = project_data::projects::from_rows(&agent_control::projects(data)?);
    let reply = match action {
        HistoryAction::Hit { .. }
        | HistoryAction::Conversation { .. }
        | HistoryAction::Workers { .. }
        | HistoryAction::Tools { .. } => unreachable!("handled above"),
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
            let metadata = run_details(&h, &timestamp);
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
            let (hits, next) = history::search_run_page(&backend, id, query, from.unwrap_or(1))?;
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
            let status = "Matching retained events".into();
            for hit in hits {
                events.push(HistoryEvent {
                    range: None,
                    revision: hit.revision,
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
        HistoryAction::Event { id, revision } => {
            // Read the exact retained event without the multi-entry preview budget.
            let fetched = store::Backend::fetch(
                &backend,
                &FetchRequest::entity(agent_proto::agents::rows::ENTRY_ENTITY)
                    .filter(Pred::eq("run_id", id.as_str()))
                    .filter(Pred::eq("revision", *revision as i64))
                    .limit(1),
            )?;
            let mut entry = agent_proto::agents::rows::entries_from_rows(&fetched)
                .remove(id)
                .and_then(|mut entries| entries.pop())
                .ok_or("Event no longer retained")?;
            if fetched.truncated {
                return Err("Retained event exceeds the store read limit".into());
            }
            if let Some(tool) = &mut entry.tool {
                tool.bound_payloads(4000);
                if entry.text.len() > 8000 {
                    entry.text = entry.text.chars().take(2000).collect::<String>()
                        + "\n[incomplete tool preview: use raw archive for retained payload]";
                }
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
            let cwd =
                agent_proto::launch::resume_directory(&h, &yinz_runtime::host_identity::host_id())?;
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
                kind: Some(h.kind.clone()),
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

#[derive(Default)]
struct ConversationRead<'a> {
    worker: Option<&'a str>,
    before: Option<agent_proto::conversation::Cursor>,
    after: Option<u64>,
    tools: Option<&'a agent_proto::conversation::Range>,
}
fn conversation_view(
    request: &AgentRequest,
    backend: &impl store::Backend,
    id: &str,
    read: ConversationRead<'_>,
) -> Result<AgentReply, String> {
    let ConversationRead {
        worker,
        before,
        after,
        tools,
    } = read;
    use agent_proto::conversation::{self, Cursor, Range};
    let h = history::run_header(backend, id)?.ok_or("Recorded history unavailable")?;
    let page = match tools {
        Some(range) => conversation::tools(backend, id, range, before)?,
        None => conversation::page(backend, id, worker, before, after)?,
    };
    let mut rows = Vec::new();
    if tools.is_none() {
        if h.outcome != RunOutcome::Active {
            rows.push(row(
                "resume".into(),
                "Resume",
                "Launch and focus a new pane in the recorded directory".into(),
                HistoryAction::Resume { id: id.into() },
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
    }
    if let Some(before) = page.earlier {
        let action = match tools {
            Some(range) => HistoryAction::Tools {
                id: id.into(),
                range: range.clone(),
                before: Some(before),
            },
            None => HistoryAction::Conversation {
                id: id.into(),
                worker: worker.map(str::to_owned),
                before: Some(before),
            },
        };
        rows.push(row(
            "earlier".into(),
            "Load earlier messages",
            String::new(),
            action,
        ));
    }
    if worker.is_some() && tools.is_none() {
        rows.push(row(
            "tools".into(),
            "Tools…",
            String::new(),
            HistoryAction::Tools {
                id: id.into(),
                range: Range {
                    start: None,
                    end: None,
                    worker: worker.map(str::to_owned),
                },
                before: None,
            },
        ));
    }
    if tools.is_none() && before.is_none() {
        // Delivery watermark is independent of the chronological conversation cursor.
        rows.push(row(
            "received".into(),
            "",
            String::new(),
            run_action(id, Some(page.latest_received_revision.saturating_add(1))),
        ));
    }
    let mut events = Vec::new();
    let timestamp = |at| request.timezone.timestamp(at);
    let mut start = None;
    for entry in page.entries {
        let end = Cursor {
            created_at_ms: entry.created_at_ms,
            revision: entry.revision,
        };
        if tools.is_none() && after.is_none() && (start.is_some() || page.earlier.is_none()) {
            activity(
                backend,
                id,
                Range {
                    start,
                    end: Some(end),
                    worker: worker.map(str::to_owned),
                },
                &timestamp,
                &mut events,
            )?;
        }
        start = Some(end);
        rows.push(row(
            format!("event:{}", entry.revision),
            "",
            String::new(),
            HistoryAction::Event {
                id: id.into(),
                revision: entry.revision,
            },
        ));
        events.push(HistoryEvent {
            range: None,
            revision: entry.revision,
            tool_calls: 0,
            created_at_ms: entry.created_at_ms,
            kind: entry.kind.label().into(),
            time: timestamp(entry.created_at_ms),
            markdown: if tools.is_some() {
                format!(
                    "{}\n{}",
                    entry.text,
                    entry
                        .tool
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()
                        .map_err(|e| e.to_string())?
                        .unwrap_or_default()
                )
            } else {
                entry.text
            },
            truncated: page.shortened.contains(&entry.revision),
        });
    }
    if tools.is_none() && after.is_none() {
        // An earlier page owns the gap up to the next page's first message.
        activity(
            backend,
            id,
            Range {
                start,
                end: before,
                worker: worker.map(str::to_owned),
            },
            &timestamp,
            &mut events,
        )?;
    }
    if tools.is_none() && after.is_none() {
        if worker.is_none() {
            rows.push(row(
                "workers".into(),
                "Workers…",
                "Expandable worker conversations and tools".into(),
                HistoryAction::Workers { id: id.into() },
            ));
        }
        rows.push(row(
            "latest".into(),
            "Latest / Refresh",
            String::new(),
            HistoryAction::Conversation {
                id: id.into(),
                worker: worker.map(str::to_owned),
                before: None,
            },
        ));
    }
    for event in events.iter().filter(|e| e.kind == "tools") {
        if let Some(range) = &event.range {
            rows.push(row(
                format!("event:{}", event.revision),
                "",
                String::new(),
                HistoryAction::Tools {
                    id: id.into(),
                    range: range.clone(),
                    before: None,
                },
            ));
        }
    }
    Ok(AgentReply::History(HistoryReply::Run {
        header: HistoryHeader {
            name: worker
                .map(|w| format!("Worker {w}"))
                .unwrap_or_else(|| h.title.clone()),
            status: format!("{} · {:?}", h.kind, h.outcome),
            location: format!(
                "{} · {}",
                h.yinz_session_name,
                h.cwd.as_deref().unwrap_or("Directory unavailable")
            ),
            started: timestamp(Some(h.started_at_ms)),
            session_id: format!("session {}", h.session_id),
            task: h.title.clone(),
            details: run_details(&h, &timestamp),
            project: agent_proto::agents::checkout_location(h.host.as_deref(), h.cwd.as_deref())
                .and_then(|location| {
                    project_data::projects::for_cwd(
                        &project_data::projects::from_rows(
                            &backend
                                .fetch(&FetchRequest::entity("project").limit(1000))
                                .ok()?,
                        ),
                        &location,
                    )
                    .map(|p| p.name.clone())
                })
                .unwrap_or_default(),
            ..Default::default()
        },
        events,
        rows,
        status: if tools.is_some() {
            "Retained tool details · 30 records per page"
        } else {
            "Conversation · Search includes retained tools and workers"
        }
        .into(),
    }))
}
fn activity(
    backend: &impl store::Backend,
    id: &str,
    range: agent_proto::conversation::Range,
    timestamp: &impl Fn(Option<i64>) -> String,
    events: &mut Vec<HistoryEvent>,
) -> Result<(), String> {
    use agent_proto::{agents::RunEntryKind, conversation::Cursor};

    let request = range
        .request(id)
        .filter(Pred::is_in("kind", ["tool_call", "tool_result"]))
        .select([
            "run_id",
            "revision",
            "kind",
            "created_at_ms",
            "worker_id",
            "tool",
            "text",
        ])
        .sort_by("created_at_ms", SortDir::Desc)
        .sort_by("revision", SortDir::Desc)
        .limit(100);
    let mut position: Option<Cursor> = None;
    let mut first = None;
    let mut count = 0;
    loop {
        let current = match position {
            Some(cursor) => request.clone().filter(cursor.before()),
            None => request.clone(),
        };
        let mut batch = agent_proto::agents::rows::entries_from_rows(&backend.fetch(&current)?)
            .remove(id)
            .unwrap_or_default();
        if batch.is_empty() {
            break;
        }
        let cursor = |entry: &agent_proto::agents::RunEntry| Cursor {
            created_at_ms: entry.created_at_ms,
            revision: entry.revision,
        };
        batch.sort_by_key(cursor);
        position = batch.first().map(cursor);
        // Resolve legacy attribution just as expanded tool reads do.
        for entry in batch
            .into_iter()
            .rev()
            .filter(|e| e.worker() == range.worker)
        {
            count += usize::from(entry.kind == RunEntryKind::ToolCall);
            first = Some(cursor(&entry));
        }
    }
    if let Some(entry) = first {
        events.push(HistoryEvent {
            revision: entry.revision,
            tool_calls: count,
            kind: "tools".into(),
            time: timestamp(entry.created_at_ms),
            created_at_ms: entry.created_at_ms,
            markdown: String::new(),
            truncated: false,
            range: Some(range),
        });
    }
    Ok(())
}

fn run_details(h: &AgentRunHeader, timestamp: &impl Fn(Option<i64>) -> String) -> String {
    format!("Run: {}\nProvider: {}\nTitle: {}\nAgent session: {}\nSession: {}\nHost: {}\nPane: {}\nDirectory: {}\nExecution target: {}\nStarted: {}\nEnded: {}\nOutcome: {:?}\nRevision: {}\nSummary: {}\nPrevious run: {}\nOriginal run: {}",
                h.id, h.kind, h.title, h.session_id, h.yinz_session_name, h.host_id, h.pane_id,
                h.cwd.as_deref().unwrap_or("Unavailable"), h.host.as_deref().unwrap_or("local"), timestamp(Some(h.started_at_ms)),
                timestamp(h.ended_at_ms), h.outcome, h.revision,
                h.summary.as_ref().map(|s| s.text.as_str()).unwrap_or("Unavailable"), h.previous_run_id.as_deref().unwrap_or("Unavailable"), h.origin_run_id.as_deref().unwrap_or("Unavailable"))
}
