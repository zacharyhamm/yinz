//! Host-side hook enrichment, before state mapping and broadcast pruning.
use serde_json::Value;
use std::{
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
};
use yinz_types::agent::HookEvent;

const METADATA_BYTES: u64 = 64 * 1024;
const TAIL_BYTES: u64 = 4 * 1024 * 1024;
const REVIEWER_BYTES: u64 = 64 * 1024 * 1024;

/// Preserve small hooks verbatim; bound larger hooks without losing turn or prompt identity.
pub fn prune_for_forward(kind: &str, raw: String) -> serde_json::Result<String> {
    if raw.len() <= yinz_types::agent::HOOK_BROADCAST_MAX_BYTES {
        return Ok(raw);
    }
    let (event, json) = parse_hook_event(kind, raw)?;
    Ok(event.broadcast_json(json))
}

/// Parse a hook on the machine running the agent. Preserve unknown JSON fields when adding
/// Codex's reviewer and assistant messages. Unreadable transcripts leave reporting functional.
pub fn parse_hook_event(kind: &str, json: String) -> serde_json::Result<(HookEvent, String)> {
    let bytes = json.len();
    let (mut event, mut json) = parse_provider_hook_event(kind, json).map_err(|error| {
        // Serde's type errors can quote the rejected value. Report only size and location.
        <serde_json::Error as serde::de::Error>::custom(format!(
            "invalid hook JSON ({bytes} bytes, line {}, column {})",
            error.line(),
            error.column()
        ))
    })?;
    if let Some(capture) = &mut event.tool_capture {
        capture.normalize()?;
    } else {
        event.tool_capture = yinz_types::agent::tool_capture::prepare(kind, &event);
    }
    let mut raw: Value = serde_json::from_str(&json)?;
    if event.tool_capture.is_some() || raw.get("tool_capture").is_some() {
        raw["tool_capture"] = serde_json::to_value(&event.tool_capture)?;
        json = serde_json::to_string(&raw)?;
    }
    if kind == "codex" {
        let (event, json) = event.broadcast(json);
        return Ok((event.into_owned(), json));
    }
    Ok((event, json))
}

fn parse_provider_hook_event(kind: &str, json: String) -> serde_json::Result<(HookEvent, String)> {
    // Compatibility with OpenCode hook files installed before TypeScript agent providers.
    // Current adapters already emit the shared HookEvent wire format.
    if kind == "opencode" {
        let value: serde_json::Value = serde_json::from_str(&json)?;
        // Current provider adapters emit HookEvent directly. Keep accepting v4 installed
        // hook files until users reinstall them with `yinz hooks install`.
        if value.get("hook_event_name").is_some() {
            return Ok((serde_json::from_value(value)?, json));
        }
        let event = yinz_types::agent::opencode::to_hook_event(&value);
        let translated = serde_json::to_string(&event)?;
        return Ok((event, translated));
    }
    if kind == "muse" {
        return parse_muse_hook_event(json);
    }
    if kind == "agy" {
        let value: serde_json::Value = serde_json::from_str(&json)?;
        // Detached confirmations and broker deliveries already use HookEvent's shape.
        // Re-translating them as native hooks drops conversation and prompt identities.
        if value.get("session_id").is_some() {
            return Ok((serde_json::from_value(value)?, json));
        }
        let mut event = yinz_types::agent::agy::to_hook_event(&value);
        if let Some(path) = event.transcript_path.clone() {
            enrich_agy_transcript(&mut event, &path);
        }
        let translated = serde_json::to_string(&event)?;
        return Ok((event, translated));
    }
    let mut event: HookEvent = serde_json::from_str(&json)?;
    if kind != "codex" {
        return Ok((event, json));
    }
    let mut raw: Value = serde_json::from_str(&json)?;
    let Some((start, tail, mut transcript)) = transcript_tail(&raw) else {
        return Ok((event, json));
    };
    let mut changed = start > 0;
    if start > 0 {
        event.messages_omitted = true;
        raw["messages_omitted"] = true.into();
    }
    if event.hook_event_name == "PermissionRequest"
        && event.approvals_reviewer.is_none()
        && let Some(reviewer) = transcript_reviewer(&raw, start, &tail, &mut transcript)
    {
        raw["approvals_reviewer"] = reviewer.clone().into();
        event.approvals_reviewer = Some(reviewer);
        changed = true;
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
    let json = serde_json::to_string(&raw)?;
    Ok((serde_json::from_str(&json)?, json))
}

/// Parse a Muse hook, translating its `Notification` to the wait it names.
///
/// The payload is otherwise the Claude Code shape, so it parses as one. But a Muse
/// `Notification` fires when the agent needs approval, where Claude Code's
/// notification kinds are mostly not waits - and neither `hook_state`, the daemon's
/// feed, nor prompt observation can tell one agent's notification from another's.
/// Renaming it to `Elicitation`, the unconditional wait, keeps one reading everywhere:
/// the record, the feed and a waited prompt all see the wait. Translated on the way
/// in like opencode's native events, for the same reason, rather than branched on
/// in every reader.
fn parse_muse_hook_event(json: String) -> serde_json::Result<(HookEvent, String)> {
    let mut event: HookEvent = serde_json::from_str(&json)?;
    if event.hook_event_name != "Notification" {
        return Ok((event, json));
    }
    event.hook_event_name = "Elicitation".to_owned();
    let mut raw: Value = serde_json::from_str(&json)?;
    raw["hook_event_name"] = "Elicitation".into();
    Ok((event, serde_json::to_string(&raw)?))
}

fn transcript_tail(hook: &Value) -> Option<(u64, Vec<u8>, File)> {
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
    (&mut file)
        .take(metadata.len() - start)
        .read_to_end(&mut tail)
        .ok()?;
    if tail.len() as u64 != metadata.len() - start {
        return None;
    }
    if start > 0 {
        let first_end = tail.iter().position(|byte| *byte == b'\n')? + 1;
        return Some((start + first_end as u64, tail[first_end..].to_vec(), file));
    }
    Some((start, tail, file))
}

fn transcript_reviewer(hook: &Value, start: u64, tail: &[u8], file: &mut File) -> Option<String> {
    let turn = hook["turn_id"].as_str().filter(|id| !id.is_empty())?;
    if tail.last() != Some(&b'\n') {
        return None;
    }
    let mut end = start;
    let mut remaining = REVIEWER_BYTES.saturating_sub(tail.len() as u64);
    let mut records = std::borrow::Cow::Borrowed(tail);
    loop {
        for line in records.split_inclusive(|byte| *byte == b'\n').rev() {
            let record: Value = serde_json::from_slice(line).ok()?;
            if record["type"] == "turn_context"
                && record["payload"]["turn_id"].as_str() == Some(turn)
            {
                // The newest matching context is authoritative even when its reviewer is absent.
                return record["payload"]["approvals_reviewer"]
                    .as_str()
                    .filter(|reviewer| !reviewer.is_empty())
                    .map(str::to_owned);
            }
        }
        if end == 0 || remaining == 0 {
            return None;
        }
        // Long turns can outlive the message tail. Scan older complete records on the
        // same identity-verified file, without expanding the assistant-message tail.
        // ponytail: scan at most 64 MiB in 4 MiB windows; prefer a native reviewer hook
        // field if larger transcripts need support. Oversized records fail closed.
        let bytes = end.min(TAIL_BYTES).min(remaining);
        let begin = end - bytes;
        file.seek(SeekFrom::Start(begin)).ok()?;
        let mut older = Vec::new();
        (&mut *file).take(bytes).read_to_end(&mut older).ok()?;
        if older.len() as u64 != bytes || older.last() != Some(&b'\n') {
            return None;
        }
        remaining -= bytes;
        let first = if begin == 0 {
            0
        } else {
            older.iter().position(|byte| *byte == b'\n')? + 1
        };
        if first == older.len() {
            return None;
        }
        end = begin + first as u64;
        older.drain(..first);
        records = std::borrow::Cow::Owned(older);
    }
}

/// The JSONL format is best-effort: only explicit turn boundaries and assistant output_text.
fn transcript_messages(
    session: &str,
    start: u64,
    tail: &[u8],
) -> Vec<yinz_types::agent::CapturedMessage> {
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
                messages.push(yinz_types::agent::CapturedMessage {
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

fn enrich_agy_transcript(event: &mut HookEvent, path: &str) {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let Ok(mut file) = options.open(path) else {
        return;
    };
    let Ok(metadata) = file.metadata() else {
        return;
    };
    if !metadata.is_file() {
        return;
    }

    let start = metadata.len().saturating_sub(TAIL_BYTES);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return;
    }
    let mut tail = Vec::new();
    if file.read_to_end(&mut tail).is_err() {
        return;
    }

    if event.hook_event_name == "UserPromptSubmit"
        && event.prompt.is_none()
        && let Some(prompt) = extract_agy_prompt(&tail, event.turn_id.as_deref())
    {
        event.prompt_sha256 = Some(yinz_types::agent::control::prompt_sha256(&prompt));
        event.prompt = Some(prompt);
    }

    if matches!(event.hook_event_name.as_str(), "Stop" | "StopFailure")
        && event.last_assistant_message.is_none()
        && let Some(last_msg) = extract_agy_last_assistant_message(&tail)
    {
        event.last_assistant_message = Some(last_msg);
    }
}

fn extract_agy_prompt(tail: &[u8], initial_steps: Option<&str>) -> Option<String> {
    let expected = initial_steps?.parse::<u64>().ok()?.checked_sub(1)?;
    for line in tail.split_inclusive(|byte| *byte == b'\n').rev() {
        let Ok(record) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        if record.get("type").and_then(Value::as_str) == Some("USER_INPUT") {
            if record["step_index"].as_u64() != Some(expected) {
                return None;
            }
            let content = record.get("content").and_then(Value::as_str)?;
            let text = if let Some(start) = content.find("<USER_REQUEST>") {
                let rest = &content[start + "<USER_REQUEST>".len()..];
                // Find the outer wrapper's closing delimiter from the end so that literal
                // `</USER_REQUEST>` delimiters within user prompt content are preserved.
                if let Some(end) = rest.rfind("</USER_REQUEST>") {
                    &rest[..end]
                } else {
                    rest
                }
            } else {
                content
            };
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
    }
    None
}

fn extract_agy_last_assistant_message(tail: &[u8]) -> Option<String> {
    for line in tail.split_inclusive(|byte| *byte == b'\n').rev() {
        let Ok(record) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        match record.get("type").and_then(Value::as_str) {
            Some("USER_INPUT") => break,
            Some("PLANNER_RESPONSE") => {
                if let Some(content) = record.get("content").and_then(Value::as_str) {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        return Some(trimmed.to_owned());
                    }
                }
            },
            _ => {},
        }
    }
    None
}

#[cfg(test)]
#[path = "../unit/agent_hooks_tests.rs"]
mod tests;
