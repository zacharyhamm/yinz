//! Bounded host-side Claude prose recovery. No reasoning or tool payload is imported.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};
use yinz_types::agent::{CapturedMessage, HookEvent};

const FILE_BYTES: u64 = 8 * 1024 * 1024;
const TOTAL_BYTES: u64 = 128 * 1024 * 1024;
const TEXT_BYTES: usize = 2 * 1024 * 1024;
const MAX_FILES: usize = 512;
#[derive(Default, Serialize, Deserialize)]
pub struct Recovery {
    pub messages: Vec<CapturedMessage>,
    pub diagnostics: Vec<String>,
    pub files: usize,
}
fn identifier(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}
fn open(path: &Path) -> Option<File> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = opts.open(path).ok()?;
    file.metadata().ok()?.is_file().then_some(file)
}
fn tail(path: &Path, budget: &mut u64) -> Option<(Vec<u8>, bool)> {
    let mut file = open(path)?;
    let len = file.metadata().ok()?.len();
    let bytes = len.min(FILE_BYTES).min(*budget);
    if bytes == 0 {
        return None;
    }
    *budget -= bytes;
    let start = len - bytes;
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut data = Vec::new();
    file.take(bytes).read_to_end(&mut data).ok()?;
    if start > 0 {
        let end = data.iter().position(|c| *c == b'\n')?;
        data.drain(..=end);
    }
    Some((data, start > 0))
}
/// Scan only files underneath this exact conversation's subagent directory.
/// Every imported record must independently name the session and the filename's worker.
pub fn recover(path: &Path, session: &str, only_worker: Option<&str>) -> Recovery {
    let mut result = Recovery::default();
    if !identifier(session) || path.file_stem().and_then(|s| s.to_str()) != Some(session) {
        result
            .diagnostics
            .push("unverified conversation path".into());
        return result;
    }
    let root = path.with_extension("").join("subagents");
    let mut dirs = vec![root.clone()];
    let mut files = Vec::new();
    let mut visited = 0;
    while let Some(dir) = dirs.pop() {
        visited += 1;
        if visited > MAX_FILES {
            result
                .diagnostics
                .push("directory scan limit reached".into());
            break;
        }
        let Ok(items) = std::fs::read_dir(&dir) else {
            continue;
        };
        for item in items.take(MAX_FILES * 2).flatten() {
            let Ok(kind) = item.file_type() else {
                continue;
            };
            let p = item.path();
            if kind.is_dir() && dirs.len() < MAX_FILES {
                dirs.push(p);
            } else if kind.is_file()
                && p.extension().is_some_and(|x| x == "jsonl")
                && p.file_stem()
                    .and_then(|x| x.to_str())
                    .is_some_and(|x| x.starts_with("agent-"))
            {
                files.push(p);
                if files.len() >= MAX_FILES {
                    break;
                }
            }
        }
        if files.len() >= MAX_FILES {
            result
                .diagnostics
                .push("transcript file limit reached".into());
            break;
        }
    }
    files.sort();
    let mut budget = TOTAL_BYTES;
    let mut text_bytes = 0usize;
    let mut seen = BTreeSet::new();
    for file in files {
        let worker = file
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_prefix("agent-"))
            .unwrap_or_default();
        if !identifier(worker) || only_worker.is_some_and(|w| w != worker) {
            continue;
        }
        let Some((data, cut)) = tail(&file, &mut budget) else {
            result
                .diagnostics
                .push(format!("unavailable transcript: {worker}"));
            continue;
        };
        result.files += 1;
        let mut turn = String::new();
        let mut first = true;
        for line in data.split(|c| *c == b'\n') {
            let Ok(row) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            if row["sessionId"].as_str() != Some(session)
                || row["agentId"].as_str() != Some(worker)
                || row["isSidechain"] != true
            {
                continue;
            }
            if row["type"] == "user"
                && row["message"]["content"]
                    .as_array()
                    .is_none_or(|parts| !parts.iter().any(|p| p["type"] == "tool_result"))
            {
                turn = row["promptId"]
                    .as_str()
                    .or(row["uuid"].as_str())
                    .unwrap_or_default()
                    .into();
            }
            if row["type"] != "assistant" || row["message"]["role"] != "assistant" {
                continue;
            }
            let Some(source) = row["uuid"].as_str().filter(|id| identifier(id)) else {
                continue;
            };
            let Some(at) = row["timestamp"]
                .as_str()
                .and_then(|s| s.parse::<jiff::Timestamp>().ok())
                .map(|t| t.as_millisecond())
            else {
                continue;
            };
            let text = row["message"]["content"]
                .as_array()
                .map(|parts| {
                    parts
                        .iter()
                        .filter(|p| p["type"] == "text")
                        .filter_map(|p| p["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            if text.trim().is_empty() || !seen.insert((worker.to_string(), source.to_string())) {
                continue;
            }
            if text_bytes.saturating_add(text.len()) > TEXT_BYTES {
                result
                    .diagnostics
                    .push("assistant text budget reached; recovery incomplete".into());
                return result;
            }
            text_bytes += text.len();
            result.messages.push(CapturedMessage {
                source_id: format!("claude:{session}:{worker}:{source}"),
                turn_id: if turn.is_empty() {
                    format!("source:{source}")
                } else {
                    turn.clone()
                },
                text,
                worker_id: Some(worker.into()),
                created_at_ms: Some(at),
                incomplete: cut && first,
            });
            first = false;
        }
        if cut {
            result
                .diagnostics
                .push(format!("earlier transcript content omitted: {worker}"));
        }
    }
    result.messages.sort_by_key(|m| m.created_at_ms);
    result
}
/// Locate a local transcript from the provider's configured root and recorded cwd.
pub fn transcript(cwd: &str, session: &str) -> Option<PathBuf> {
    if !identifier(session) {
        return None;
    }
    let root = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude")))?;
    let project: String = cwd
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let path = root
        .join("projects")
        .join(project)
        .join(format!("{session}.jsonl"));
    open(&path).map(|_| path)
}
fn prompt_for_report(event: &HookEvent, data: &[u8]) -> Option<String> {
    let rows: Vec<Value> = data
        .split(|c| *c == b'\n')
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .filter(|row| {
            row["sessionId"] == event.session_id
                && match event.agent_id.as_deref() {
                    Some(worker) => {
                        row["agentId"].as_str() == Some(worker) && row["isSidechain"] == true
                    },
                    None => row["isSidechain"] != true,
                }
        })
        .collect();
    let user = |row: &Value| {
        row["type"] == "user"
            && row["message"]["content"]
                .as_array()
                .is_none_or(|ps| !ps.iter().any(|p| p["type"] == "tool_result"))
    };
    let target = rows.iter().rfind(|row| {
        row["type"] == "assistant"
            && (event.message_id.as_deref().is_some_and(|id| {
                row["uuid"].as_str() == Some(id) || row["message"]["id"].as_str() == Some(id)
            }) || event.last_assistant_message.as_deref().is_some_and(|text| {
                row["message"]["content"].as_array().is_some_and(|ps| {
                    ps.iter()
                        .filter(|p| p["type"] == "text")
                        .filter_map(|p| p["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                        .trim()
                        == text.trim()
                })
            }))
    });
    if let Some(mut row) = target {
        let by_id: std::collections::BTreeMap<_, _> = rows
            .iter()
            .filter_map(|r| r["uuid"].as_str().map(|id| (id, r)))
            .collect();
        for _ in 0..4096 {
            if let Some(prompt) = row["promptId"].as_str() {
                return Some(prompt.into());
            }
            if user(row) {
                return row["uuid"].as_str().map(str::to_owned);
            }
            row = by_id.get(row["parentUuid"].as_str()?).copied()?;
        }
        return None;
    }
    // Without a text-bearing report, the most recent verified prompt remains
    // useful. A matching reply above follows ancestry past any queued prompt.
    rows.iter()
        .rev()
        .find(|row| user(row))
        .and_then(|row| row["promptId"].as_str().or(row["uuid"].as_str()))
        .map(str::to_owned)
}

/// Native Stop and observer confirmation share Claude's prompt identity. Worker
/// completion and session restoration also recover message-hook gaps.
pub fn enrich(event: &mut HookEvent) {
    if let Some(prompt) = event.prompt_id.as_ref().filter(|p| !p.is_empty()) {
        event.turn_id = Some(prompt.clone());
    }
    let Some(path) = event.transcript_path.as_deref().map(Path::new) else {
        return;
    };
    if event.turn_id.is_none()
        && matches!(
            event.hook_event_name.as_str(),
            "Stop" | "StopFailure" | "MessageDisplay"
        )
        && let Some((data, _)) = tail(path, &mut { FILE_BYTES })
    {
        event.turn_id = prompt_for_report(event, &data);
        event.prompt_id = event.turn_id.clone();
    }
    if matches!(
        event.hook_event_name.as_str(),
        "SubagentStop" | "SessionStart" | "SessionEnd"
    ) || event.claude_recovered_after.is_some()
    {
        event
            .assistant_messages
            .extend(recover(path, &event.session_id, event.agent_id.as_deref()).messages);
    }
}

#[cfg(test)]
#[path = "../unit/agent_history.rs"]
mod tests;
