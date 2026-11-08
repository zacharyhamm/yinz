//! Conversation paging is chronological and independent of raw revision delivery.
use crate::{
    agents::{rows, RunEntry, RunEntryKind},
    store::Backend,
};
use yinz_tile::prelude::{FetchRequest, Pred, SortDir};

pub use yinz_types::agent::history::{Cursor, Range};
fn cursor(entry: &RunEntry) -> Cursor {
    Cursor {
        created_at_ms: entry.created_at_ms,
        revision: entry.revision,
    }
}
pub struct Page {
    pub entries: Vec<RunEntry>,
    pub shortened: Vec<u64>,
    pub earlier: Option<Cursor>,
    pub latest_received_revision: u64,
}
fn read(backend: &impl Backend, id: &str, request: &FetchRequest) -> Result<Vec<RunEntry>, String> {
    Ok(rows::entries_from_rows(&backend.fetch(request)?)
        .remove(id)
        .unwrap_or_default())
}
/// Select prose in storage before applying either browser budget. Legacy worker
/// attribution is resolved before limiting the visible conversation.
pub fn page(
    backend: &impl Backend,
    id: &str,
    worker: Option<&str>,
    before: Option<Cursor>,
    after_revision: Option<u64>,
) -> Result<Page, String> {
    let latest = read(
        backend,
        id,
        &FetchRequest::entity(rows::ENTRY_ENTITY)
            .filter(Pred::eq("run_id", id))
            .select(["run_id", "revision", "kind"])
            .sort_by("revision", SortDir::Desc)
            .limit(1),
    )?
    .first()
    .map_or(0, |e| e.revision);
    if let Some(after) = after_revision {
        return delivery(backend, id, worker, after, latest);
    }
    let base = FetchRequest::entity(rows::ENTRY_ENTITY)
        .filter(Pred::eq("run_id", id))
        .filter(Pred::Not(Box::new(Pred::is_in(
            "kind",
            ["tool_call", "tool_result"],
        ))))
        .sort_by("created_at_ms", SortDir::Desc)
        .sort_by("revision", SortDir::Desc)
        .limit(100);
    let mut request = base.clone();
    if let Some(after) = after_revision {
        request = request.filter(Pred::gte("revision", after as i64));
    }
    let mut position = before;
    let mut entries = Vec::new();
    loop {
        let current = match position {
            Some(c) => request.clone().filter(c.before()),
            None => request.clone(),
        };
        let mut batch = read(backend, id, &current)?;
        if batch.is_empty() {
            break;
        }
        batch.sort_by_key(cursor);
        position = batch.first().map(cursor);
        entries.extend(
            batch
                .into_iter()
                .rev()
                .filter(|entry| entry.worker().as_deref() == worker),
        );
        suppress_completions(&mut entries);
        if entries.len() > 30 {
            break;
        }
    }
    let mut more = entries.len() > 30;
    entries.sort_by_key(cursor);
    if entries.len() > 30 {
        entries.drain(..entries.len() - 30);
    }
    let shortened = entries
        .iter()
        .filter(|e| e.text.chars().count() > crate::run_history::ENTRY_TEXT_CHARS)
        .map(|e| e.revision)
        .collect();
    more |= crate::run_history::budget(&mut entries, true, false);
    let earlier = more.then(|| entries.first().map(cursor)).flatten();
    Ok(Page {
        entries,
        shortened,
        earlier,
        latest_received_revision: latest,
    })
}
fn delivery(
    backend: &impl Backend,
    id: &str,
    worker: Option<&str>,
    after: u64,
    latest: u64,
) -> Result<Page, String> {
    let base = FetchRequest::entity(rows::ENTRY_ENTITY)
        .filter(Pred::eq("run_id", id))
        .filter(Pred::Not(Box::new(Pred::is_in(
            "kind",
            ["tool_call", "tool_result"],
        ))))
        .sort_by("revision", SortDir::Asc)
        .limit(100);
    let mut from = after;
    let mut entries = Vec::new();
    loop {
        let batch = read(
            backend,
            id,
            &base.clone().filter(Pred::gte("revision", from as i64)),
        )?;
        if batch.is_empty() {
            break;
        }
        from = batch.last().map_or(from, |e| e.revision.saturating_add(1));
        entries.extend(
            batch
                .into_iter()
                .filter(|e| e.worker().as_deref() == worker),
        );
        suppress_completions(&mut entries);
        if entries.len() > 30 {
            break;
        }
    }
    let mut more = entries.len() > 30;
    entries.truncate(30);
    let shortened = entries
        .iter()
        .filter(|e| e.text.chars().count() > crate::run_history::ENTRY_TEXT_CHARS)
        .map(|e| e.revision)
        .collect();
    more |= crate::run_history::budget(&mut entries, false, false);
    let received = if more {
        entries
            .last()
            .map_or(after.saturating_sub(1), |e| e.revision)
    } else {
        latest
    };
    entries.sort_by_key(cursor);
    Ok(Page {
        entries,
        shortened,
        earlier: None,
        latest_received_revision: received,
    })
}
/// Alternative completion reports do not erase the retained raw record. Compare
/// within the same worker and turn; never suppress identical text in distinct turns.
pub fn suppress_completions(entries: &mut Vec<RunEntry>) {
    let messages: Vec<_> = entries
        .iter()
        .filter(|e| e.kind == RunEntryKind::Message)
        .cloned()
        .collect();
    let prompts: Vec<_> = entries
        .iter()
        .filter(|e| e.kind == RunEntryKind::Prompt)
        .map(|e| (e.revision, e.worker()))
        .collect();
    entries.retain(|e| {
        e.kind != RunEntryKind::Result
            || !messages.iter().any(|m| {
                m.worker() == e.worker()
                    && m.text.trim() == e.text.trim()
                    && match (&m.conversation_turn_id, &e.conversation_turn_id) {
                        (Some(a), Some(b)) => a == b,
                        _ => {
                            // Legacy completion reports: a prompt boundary separates turns.
                            let lo = m.revision.min(e.revision);
                            let hi = m.revision.max(e.revision);
                            !prompts
                                .iter()
                                .any(|(r, w)| *r > lo && *r < hi && *w == e.worker())
                        },
                    }
            })
    });
}

/// Reconstruct both intervals around delivered prose, even when its neighbors
/// are outside the newest conversation page (or have newer archive revisions).
pub fn activity_ranges(
    backend: &impl Backend,
    id: &str,
    worker: Option<&str>,
    entries: &[RunEntry],
) -> Result<Vec<Range>, String> {
    let mut ranges = Vec::new();
    for entry in entries {
        let boundary = cursor(entry);
        for previous in [true, false] {
            let mut position = boundary;
            let neighbor = loop {
                let range = Range {
                    start: (!previous).then_some(position),
                    end: previous.then_some(position),
                    worker: worker.map(str::to_owned),
                };
                let dir = if previous {
                    SortDir::Desc
                } else {
                    SortDir::Asc
                };
                let mut batch = read(
                    backend,
                    id,
                    &range
                        .request(id)
                        .filter(Pred::Not(Box::new(Pred::is_in(
                            "kind",
                            ["tool_call", "tool_result"],
                        ))))
                        .sort_by("created_at_ms", dir)
                        .sort_by("revision", dir)
                        .limit(100),
                )?;
                if batch.is_empty() {
                    break None;
                }
                batch.sort_by_key(cursor);
                let edge = if previous {
                    batch.first()
                } else {
                    batch.last()
                };
                let Some(edge) = edge else { break None };
                position = cursor(edge);
                // A neighboring completion may duplicate the delivered message
                // itself, which is outside this chronological query.
                batch.push(entry.clone());
                suppress_completions(&mut batch);
                let mut matching = batch
                    .iter()
                    .filter(|e| e.revision != boundary.revision && e.worker().as_deref() == worker);
                if let Some(entry) = if previous {
                    matching.next_back()
                } else {
                    matching.next()
                } {
                    break Some(cursor(entry));
                }
            };
            let range = Range {
                start: if previous { neighbor } else { Some(boundary) },
                end: if previous { Some(boundary) } else { neighbor },
                worker: worker.map(str::to_owned),
            };
            if !ranges.contains(&range) {
                ranges.push(range);
            }
        }
    }
    Ok(ranges)
}

fn resolved_worker(worker: Option<&str>) -> Pred {
    Pred::eq("attribution_resolved", true).and(match worker {
        Some(worker) => Pred::eq("worker_id", worker),
        None => Pred::is_null("worker_id"),
    })
}

pub struct Activity {
    pub first: Cursor,
    pub tool_calls: usize,
}

/// Current archive rows have scalar attribution. Only unmarked legacy rows
/// require payload/text reads, bounded to one batch at a time.
pub fn activity(
    backend: &impl Backend,
    id: &str,
    range: &Range,
) -> Result<Option<Activity>, String> {
    let base = range
        .request(id)
        .filter(Pred::is_in("kind", ["tool_call", "tool_result"]));
    let current = base
        .clone()
        .filter(resolved_worker(range.worker.as_deref()));
    let mut count = backend.count(&current.clone().filter(Pred::eq("kind", "tool_call")))?;
    let mut first = read(
        backend,
        id,
        &current
            .select(["run_id", "revision", "kind", "created_at_ms"])
            .sort_by("created_at_ms", SortDir::Asc)
            .sort_by("revision", SortDir::Asc)
            .limit(1),
    )?
    .first()
    .map(cursor);
    let legacy = base
        .filter(Pred::is_null("attribution_resolved"))
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
    loop {
        let request = match position {
            Some(c) => legacy.clone().filter(c.before()),
            None => legacy.clone(),
        };
        let mut batch = read(backend, id, &request)?;
        if batch.is_empty() {
            break;
        }
        batch.sort_by_key(cursor);
        position = batch.first().map(cursor);
        for entry in batch.into_iter().filter(|e| e.worker() == range.worker) {
            count += u64::from(entry.kind == RunEntryKind::ToolCall);
            let at = cursor(&entry);
            first = Some(first.map_or(at, |first| first.min(at)));
        }
    }
    let tool_calls = usize::try_from(count).map_err(|e| e.to_string())?;
    Ok(first.map(|first| Activity { first, tool_calls }))
}

/// Expanded activity reads at most 30 records, with full retained payloads under
/// the normal byte budget. A cursor advances independently of conversation pages.
pub fn tools(
    backend: &impl Backend,
    id: &str,
    range: &Range,
    before: Option<Cursor>,
) -> Result<Page, String> {
    let request = range
        .request(id)
        .filter(Pred::is_in("kind", ["tool_call", "tool_result"]))
        .filter(resolved_worker(range.worker.as_deref()).or(Pred::is_null("attribution_resolved")))
        .sort_by("created_at_ms", SortDir::Desc)
        .sort_by("revision", SortDir::Desc)
        .limit(30);
    let mut position = before;
    let mut selected = Vec::new();
    loop {
        let current = match position {
            Some(c) => request.clone().filter(c.before()),
            None => request.clone(),
        };
        let mut batch = read(backend, id, &current)?;
        if batch.is_empty() {
            break;
        }
        batch.sort_by_key(cursor);
        position = batch.first().map(cursor);
        selected.extend(
            batch
                .into_iter()
                .rev()
                .filter(|e| e.worker() == range.worker),
        );
        if selected.len() > 30 {
            break;
        }
    }
    let mut more = selected.len() > 30;
    selected.truncate(30);
    let mut entries = Vec::new();
    let mut shortened = Vec::new();
    let mut spent = 0;
    for mut entry in selected {
        let mut cost = serde_json::to_vec(&entry).map_err(|e| e.to_string())?.len();
        if entries.is_empty() && cost > crate::run_history::PAGE_TEXT_BUDGET {
            entry.text = entry.text.chars().take(2000).collect();
            if let Some(tool) = &mut entry.tool {
                tool.bound_payloads(4000);
            }
            shortened.push(entry.revision);
            cost = serde_json::to_vec(&entry).map_err(|e| e.to_string())?.len();
        }
        if spent + cost > crate::run_history::PAGE_TEXT_BUDGET {
            more = true;
            break;
        }
        spent += cost;
        entries.push(entry);
    }
    entries.reverse();
    let earlier = more.then(|| entries.first().map(cursor)).flatten();
    Ok(Page {
        entries,
        shortened,
        earlier,
        latest_received_revision: 0,
    })
}

/// Worker picker: prose is cheap to scan and also recognizes prefixed legacy replies.
pub fn workers(backend: &impl Backend, id: &str) -> Result<Vec<String>, String> {
    let mut workers = std::collections::BTreeSet::new();
    let request = FetchRequest::entity(rows::ENTRY_ENTITY)
        .filter(Pred::eq("run_id", id))
        .filter(Pred::Not(Box::new(Pred::is_in(
            "kind",
            ["tool_call", "tool_result"],
        ))))
        .limit(100);
    let mut offset = 0;
    loop {
        let fetched = backend.fetch(&request.clone().offset(offset))?;
        for entry in rows::entries_from_rows(&fetched)
            .remove(id)
            .unwrap_or_default()
        {
            workers.extend(entry.worker());
        }
        if !fetched.truncated || fetched.rows.is_empty() {
            break;
        }
        offset += fetched.rows.len() as u32;
    }
    let request = FetchRequest::entity(rows::ENTRY_ENTITY)
        .filter(Pred::eq("run_id", id))
        .filter(Pred::not_null("worker_id").or(Pred::not_null("tool")))
        .select(["run_id", "revision", "kind", "worker_id", "tool"])
        .limit(100);
    let mut offset = 0;
    loop {
        let fetched = backend.fetch(&request.clone().offset(offset))?;
        for entry in rows::entries_from_rows(&fetched)
            .remove(id)
            .unwrap_or_default()
        {
            workers.extend(entry.worker());
        }
        if !fetched.truncated || fetched.rows.is_empty() {
            break;
        }
        offset += fetched.rows.len() as u32;
    }
    Ok(workers.into_iter().collect())
}

#[cfg(test)]
#[path = "unit/conversation.rs"]
mod tests;

// Only Claude worker sources have a legacy spelling without the provider prefix.
fn worker_source<'a>(source: &'a str, worker: Option<&str>) -> &'a str {
    let legacy = source.strip_prefix("claude:").filter(|source| {
        let mut parts = source.split(':');
        parts.next().is_some_and(|session| !session.is_empty())
            && parts
                .next()
                .is_some_and(|id| Some(id) == worker && !id.is_empty())
            && parts.next().is_some_and(|id| !id.is_empty())
            && parts.next().is_none()
    });
    legacy.unwrap_or(source)
}

fn entry_has_message_source(entry: &RunEntry, source: &str, worker: Option<&str>) -> bool {
    entry.worker().as_deref() == worker
        && entry
            .event_id
            .as_deref()
            .and_then(|id| id.strip_prefix("message:"))
            .and_then(|id| serde_json::from_str::<[String; 2]>(id).ok())
            .is_some_and(|[_, archived]| {
                worker_source(&archived, worker) == worker_source(source, worker)
            })
}

/// Recognize an archived provider source across inferred turns and legacy spellings.
pub fn message_source_exists(source: &str, worker: Option<&str>, entries: &[RunEntry]) -> bool {
    entries
        .iter()
        .any(|entry| entry_has_message_source(entry, source, worker))
}

/// Reconcile source reports without changing an existing archive row. Source
/// identity is scoped to the worker, independently of the inferred turn. Only
/// completion reports may stand in for a distinct source by text. SubagentStop
/// text can account for only this worker's final transcript reply; identical
/// earlier replies and replies in different turns remain distinct.
pub fn missing_messages(
    messages: Vec<yinz_types::agent::CapturedMessage>,
    entries: &[RunEntry],
) -> Vec<yinz_types::agent::CapturedMessage> {
    let mut final_source = std::collections::BTreeMap::new();
    for m in &messages {
        if let Some(worker) = &m.worker_id {
            final_source.insert(worker.clone(), m.source_id.clone());
        }
    }
    messages
        .into_iter()
        .filter(|m| {
            !entries.iter().any(|e| {
                if e.worker() != m.worker_id {
                    return false;
                }
                // A bounded transcript can lose the prompt that supplied the turn ID.
                if entry_has_message_source(e, &m.source_id, m.worker_id.as_deref()) {
                    return true;
                }
                let worker_fallback = e
                    .event_id
                    .as_ref()
                    .is_some_and(|id| id.starts_with("subagent:"));
                if e.kind != RunEntryKind::Result && !worker_fallback {
                    return false;
                }
                if worker_fallback
                    && m.worker_id.as_ref().and_then(|w| final_source.get(w)) != Some(&m.source_id)
                {
                    return false;
                }
                let text = e
                    .worker()
                    .and_then(|w| {
                        e.text
                            .strip_prefix(&format!("Subagent {w}:\n"))
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| e.text.clone());
                if text.trim() != m.text.trim() {
                    return false;
                }
                e.conversation_turn_id.as_deref() == Some(m.turn_id.as_str())
                    || worker_fallback
                        && e.created_at_ms
                            .zip(m.created_at_ms)
                            .is_some_and(|(recorded, source)| recorded >= source)
            })
        })
        .collect()
}
