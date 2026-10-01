//! Message capture shares the run's persistent entry identities, including after adoption.
use hocus_proto::{
    agents::{AgentRun, RunEntry, RunEntryKind},
    wire::HookEvent,
};
use std::collections::BTreeMap;

const MESSAGE_BYTES: usize = 1024 * 1024;

#[derive(Default)]
pub struct Pending {
    messages: BTreeMap<(String, String, String), Assembly>,
}

#[derive(Default)]
struct Assembly {
    batches: BTreeMap<u64, String>,
    final_index: Option<u64>,
    bytes: usize,
    incomplete: bool,
}

fn identity(turn: &str, source: &str) -> String {
    format!("message:{}", serde_json::json!([turn, source]))
}

fn fallback_identity(turn: &str, text: &str) -> Option<String> {
    (!turn.is_empty()).then(|| format!("stop:{}", serde_json::json!([turn, text.trim()])))
}

pub fn stop_event_id(event: &HookEvent, text: &str) -> Option<String> {
    if event.hook_event_name != "Stop" || event.agent_id.is_some() {
        return None;
    }
    fallback_identity(event.turn_id.as_deref()?, text)
}

fn push(
    run: &mut AgentRun,
    id: String,
    text: String,
    turn: Option<&str>,
    incomplete: bool,
    now: i64,
) -> bool {
    if run
        .entries
        .iter()
        .any(|entry| entry.event_id.as_ref() == Some(&id))
    {
        return false;
    }
    let mut text = hocus_proto::text::sanitize(&text);
    // Keep the persisted Result and its revision when capture arrives after Stop.
    if !incomplete {
        if let Some(fallback) = turn.and_then(|turn| fallback_identity(turn, &text)) {
            if run.entries.iter().any(|entry| {
                entry.kind == RunEntryKind::Result && entry.event_id.as_ref() == Some(&fallback)
            }) {
                return false;
            }
        }
    }
    if incomplete {
        text.push_str("\n[incomplete message: missing or truncated content]");
    }
    if text.trim().is_empty() {
        return false;
    }
    run.revision += 1;
    run.entries.push(RunEntry {
        revision: run.revision,
        event_id: Some(id),
        kind: RunEntryKind::Message,
        text,
        created_at_ms: Some(now),
    });
    true
}

impl Pending {
    pub fn capture(&mut self, run: &mut AgentRun, event: &HookEvent, now: i64) -> bool {
        let mut changed = false;
        if event.messages_omitted {
            changed |= push(
                run,
                format!(
                    "omitted:{}",
                    serde_json::json!([event.session_id, event.turn_id])
                ),
                "[earlier assistant content omitted from bounded hook payload]".into(),
                None,
                false,
                now,
            );
        }
        for message in &event.assistant_messages {
            changed |= push(
                run,
                identity(&message.turn_id, &message.source_id),
                message.text.clone(),
                event.agent_id.is_none().then_some(message.turn_id.as_str()),
                message.incomplete,
                now,
            );
        }
        // Claude's hidden helpers, such as the prompt-suggestion subagent, stop with an empty
        // type. Nobody asked for their output, and archiving it misleads run summaries.
        let hidden_helper = event.agent_type.as_deref() == Some("");
        if event.hook_event_name == "SubagentStop" && !hidden_helper {
            if let Some(text) = event.last_assistant_message.as_ref() {
                let agent = event.agent_id.as_deref().unwrap_or("unknown");
                let turn = event.turn_id.as_deref().unwrap_or("");
                changed |= push(
                    run,
                    format!(
                        "subagent:{}",
                        serde_json::json!([event.session_id, turn, agent])
                    ),
                    format!("Subagent {agent}:\n{text}"),
                    None,
                    false,
                    now,
                );
            }
        }
        if event.hook_event_name != "MessageDisplay" {
            return changed;
        }
        let (Some(turn), Some(message), Some(index), Some(final_batch), Some(delta)) = (
            &event.turn_id,
            &event.message_id,
            event.index,
            event.final_batch,
            &event.delta,
        ) else {
            return changed;
        };
        if turn.is_empty() || message.is_empty() || event.session_id.is_empty() {
            return changed;
        }
        let source = format!("{}:{message}", event.session_id);
        let id = identity(turn, &source);
        if run
            .entries
            .iter()
            .any(|entry| entry.event_id.as_ref() == Some(&id))
        {
            return changed;
        }
        let key = (event.session_id.clone(), turn.clone(), source);
        let assembly = self.messages.entry(key.clone()).or_default();
        if final_batch {
            if assembly.final_index.is_some_and(|last| last != index) {
                assembly.incomplete = true;
            }
            assembly.final_index = Some(index);
        }
        assembly.incomplete |= event.message_incomplete;
        let mut overflow = false;
        if !assembly.batches.contains_key(&index) {
            let remaining = MESSAGE_BYTES.saturating_sub(assembly.bytes);
            let mut end = delta.len().min(remaining);
            while !delta.is_char_boundary(end) {
                end -= 1;
            }
            overflow = end < delta.len() || assembly.batches.len() >= 16384;
            assembly.incomplete |= overflow;
            if assembly.batches.len() < 16384 {
                assembly.batches.insert(index, delta[..end].to_owned());
                assembly.bytes += end;
            }
        }
        let complete = assembly.final_index.is_some_and(|last| {
            last.checked_add(1) == Some(assembly.batches.len() as u64)
                && assembly.batches.keys().copied().eq(0..=last)
        });
        if complete || overflow {
            if let Some(assembly) = self.messages.remove(&key) {
                changed |= push(
                    run,
                    id,
                    assembly.batches.into_values().collect(),
                    event.agent_id.is_none().then_some(turn.as_str()),
                    assembly.incomplete || !complete,
                    now,
                );
            }
        }
        changed
    }

    pub fn flush(self, run: &mut AgentRun, now: i64) {
        for ((_, turn, source), assembly) in self.messages {
            push(
                run,
                identity(&turn, &source),
                assembly.batches.into_values().collect(),
                None,
                true,
                now,
            );
        }
    }
}

/// Stop is a fallback only for the final captured message of this turn.
pub fn stop_duplicates(run: &AgentRun, event: &HookEvent, text: &str) -> bool {
    if event.hook_event_name != "Stop" || event.agent_id.is_some() {
        return false;
    }
    for entry in run.entries.iter().rev() {
        if matches!(entry.kind, RunEntryKind::Prompt | RunEntryKind::Result) {
            return false;
        }
        let Some(id) = entry
            .event_id
            .as_deref()
            .and_then(|id| id.strip_prefix("message:"))
        else {
            continue;
        };
        let Ok((turn, _)) = serde_json::from_str::<(String, String)>(id) else {
            continue;
        };
        return event
            .turn_id
            .as_deref()
            .is_some_and(|id| !id.is_empty() && id == turn)
            && entry.text.trim() == text;
    }
    false
}
