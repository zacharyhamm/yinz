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
/// Codex's reviewer; unreadable or unrecognised transcript context leaves the hook unchanged.
pub fn parse_hook_event(kind: &str, json: String) -> serde_json::Result<(HookEvent, String)> {
    let mut event: HookEvent = serde_json::from_str(&json)?;
    if kind != "codex"
        || event.hook_event_name != "PermissionRequest"
        || event.approvals_reviewer.is_some()
    {
        return Ok((event, json));
    }
    let mut raw: Value = serde_json::from_str(&json)?;
    let Some(reviewer) = transcript_reviewer(&raw) else {
        return Ok((event, json));
    };
    raw["approvals_reviewer"] = Value::String(reviewer.clone());
    event.approvals_reviewer = Some(reviewer);
    Ok((event, serde_json::to_string(&raw)?))
}

fn transcript_reviewer(hook: &Value) -> Option<String> {
    let session = hook["session_id"].as_str().filter(|id| !id.is_empty())?;
    let turn = hook["turn_id"].as_str().filter(|id| !id.is_empty())?;
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
    if tail.len() as u64 != metadata.len() - start || tail.last() != Some(&b'\n') {
        return None;
    }
    let mut lines = tail.split_inclusive(|byte| *byte == b'\n');
    if start > 0 {
        lines.next(); // The window may start in the middle of a record.
    }
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

#[cfg(test)]
#[path = "../unit/agent_hooks_tests.rs"]
mod tests;
