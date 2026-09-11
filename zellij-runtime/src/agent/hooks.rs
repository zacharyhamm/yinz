//! Host-side hook enrichment, before state mapping and broadcast pruning.
use serde_json::Value;
use std::{
    fs::OpenOptions,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
};
use zellij_types::agent::HookEvent;

const METADATA_BYTES: u64 = 64 * 1024;
const TAIL_BYTES: u64 = 4 * 1024 * 1024;

/// Parse a hook on the machine running the agent. Preserve unknown JSON fields when adding
/// Codex's reviewer and assistant messages. Unreadable transcripts leave reporting functional.
pub fn parse_hook_event(kind: &str, json: String) -> serde_json::Result<(HookEvent, String)> {
    // opencode's plugin forwards its own event stream, so the one reading of it is here,
    // on the way in: the translated event is what the session keeps as `raw`, because the
    // broker validates `raw` as a `HookEvent` and `hocus-agentd` re-parses it.
    if kind == "opencode" {
        let value: serde_json::Value = serde_json::from_str(&json)?;
        let event = zellij_types::agent::opencode::to_hook_event(&value);
        let translated = serde_json::to_string(&event)?;
        return Ok((event, translated));
    }
    let mut event: HookEvent = serde_json::from_str(&json)?;
    if kind != "codex" {
        return Ok((event, json));
    }
    let mut raw: Value = serde_json::from_str(&json)?;
    let Some((start, tail)) = transcript_tail(&raw) else {
        return Ok((event, json));
    };
    let mut changed = start > 0;
    if start > 0 {
        event.messages_omitted = true;
        raw["messages_omitted"] = true.into();
    }
    if event.hook_event_name == "PermissionRequest" && event.approvals_reviewer.is_none() {
        if let Some(reviewer) = transcript_reviewer(&raw, &tail) {
            raw["approvals_reviewer"] = reviewer.clone().into();
            event.approvals_reviewer = Some(reviewer);
            changed = true;
        }
    }
    let messages = transcript_messages(&event.session_id, start, &tail);
    if !messages.is_empty() {
        event.assistant_messages = messages;
        raw["assistant_messages"] = serde_json::to_value(&event.assistant_messages)?;
        changed = true;
    }
    if !changed {
        return Ok((event, json));
    }
    let json = event.broadcast_json(serde_json::to_string(&raw)?);
    Ok((serde_json::from_str(&json)?, json))
}

fn transcript_tail(hook: &Value) -> Option<(u64, Vec<u8>)> {
    let session = hook["session_id"].as_str().filter(|id| !id.is_empty())?;
    let path = hook["transcript_path"].as_str()?;
    let mut options = OpenOptions::new();
    options.read(true);
    // A substituted FIFO must not block the reporting process before the file-type check.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let mut file = options.open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    let mut first = Vec::new();
    BufReader::new((&mut file).take(METADATA_BYTES))
        .read_until(b'\n', &mut first)
        .ok()?;
    if first.last() != Some(&b'\n') {
        return None;
    }
    let first: Value = serde_json::from_slice(&first).ok()?;
    if first["type"] != "session_meta" || first["payload"]["id"].as_str() != Some(session) {
        return None;
    }

    // ponytail: only the final 4 MiB; use a stable hook field if Codex adds one, not a full scan.
    let start = metadata.len().saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = Vec::new();
    file.take(metadata.len() - start)
        .read_to_end(&mut tail)
        .ok()?;
    if tail.len() as u64 != metadata.len() - start {
        return None;
    }
    if start > 0 {
        let first_end = tail.iter().position(|byte| *byte == b'\n')? + 1;
        return Some((start + first_end as u64, tail[first_end..].to_vec()));
    }
    Some((start, tail))
}

fn transcript_reviewer(hook: &Value, tail: &[u8]) -> Option<String> {
    let turn = hook["turn_id"].as_str().filter(|id| !id.is_empty())?;
    if tail.last() != Some(&b'\n') {
        return None;
    }
    let lines = tail.split_inclusive(|byte| *byte == b'\n');
    for line in lines.rev() {
        let record: Value = serde_json::from_slice(line).ok()?;
        if record["type"] == "turn_context" && record["payload"]["turn_id"].as_str() == Some(turn) {
            // Do not fall back to an older context if the newest matching one lacks a reviewer.
            return record["payload"]["approvals_reviewer"]
                .as_str()
                .filter(|reviewer| !reviewer.is_empty())
                .map(str::to_owned);
        }
    }
    None
}

/// The JSONL format is best-effort: only explicit turn boundaries and assistant output_text.
fn transcript_messages(
    session: &str,
    start: u64,
    tail: &[u8],
) -> Vec<zellij_types::agent::CapturedMessage> {
    let mut messages = Vec::new();
    let mut turn = None;
    let mut offset = start;
    for line in tail.split_inclusive(|byte| *byte == b'\n') {
        let record_offset = offset;
        offset += line.len() as u64;
        if line.last() != Some(&b'\n') {
            break;
        }
        let Ok(record) = serde_json::from_slice::<Value>(line) else {
            turn = None;
            continue;
        };
        let payload = &record["payload"];
        match record["type"].as_str() {
            Some("turn_context") => {
                turn = payload["turn_id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned);
            },
            Some("event_msg") => match payload["type"].as_str() {
                Some("task_started") => {
                    turn = payload["turn_id"]
                        .as_str()
                        .filter(|id| !id.is_empty())
                        .map(str::to_owned);
                },
                Some("task_complete" | "task_interrupted") => turn = None,
                _ => {},
            },
            Some("response_item")
                if payload["type"] == "message" && payload["role"] == "assistant" =>
            {
                let Some(turn_id) = turn.as_ref() else {
                    continue;
                };
                // Tool-directed and reasoning messages are not user-facing prose.
                if payload["phase"]
                    .as_str()
                    .is_some_and(|phase| !matches!(phase, "commentary" | "final_answer"))
                    || payload["recipient"]
                        .as_str()
                        .is_some_and(|recipient| recipient != "all")
                    || payload["channel"].as_str().is_some_and(|channel| {
                        !matches!(channel, "commentary" | "final" | "final_answer")
                    })
                {
                    continue;
                }
                let Some(content) = payload["content"].as_array() else {
                    continue;
                };
                let text: String = content
                    .iter()
                    .filter(|part| part["type"] == "output_text")
                    .filter_map(|part| part["text"].as_str())
                    .collect();
                if text.is_empty() {
                    continue;
                }
                let id = payload["id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("offset:{record_offset}"));
                messages.push(zellij_types::agent::CapturedMessage {
                    source_id: format!("{session}:{id}"),
                    turn_id: turn_id.clone(),
                    text,
                    incomplete: false,
                });
            },
            _ => {},
        }
    }
    messages
}

#[cfg(test)]
#[path = "../unit/agent_hooks_tests.rs"]
mod tests;
