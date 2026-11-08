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
        .filter(Pred::not_null("worker_id"))
        .select(["run_id", "revision", "kind", "worker_id"])
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

/// Reconcile source reports without changing an existing archive row. Legacy
/// SubagentStop text can account for only this worker's final transcript reply;
/// identical earlier replies and replies in different turns remain distinct.
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
                let identity = format!("message:{}", serde_json::json!([m.turn_id, m.source_id]));
                if e.event_id.as_deref() == Some(identity.as_str()) {
                    return true;
                }
                if e.worker() != m.worker_id {
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
                    || e.event_id
                        .as_ref()
                        .is_some_and(|id| id.starts_with("subagent:"))
                        && m.worker_id.as_ref().and_then(|w| final_source.get(w))
                            == Some(&m.source_id)
                        && e.created_at_ms
                            .zip(m.created_at_ms)
                            .is_some_and(|(recorded, source)| recorded >= source)
            })
        })
        .collect()
}
