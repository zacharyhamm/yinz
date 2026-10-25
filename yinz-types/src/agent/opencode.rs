//! Compatibility reader for OpenCode hook files installed before agent plugins.
//!
//! The current provider hook owns event translation and emits [`HookEvent`] directly.
//! This reader accepts the native payloads from older installed hook files until users run
//! `yinz hooks install --agent opencode` again. New providers need no Rust adapter.

use super::HookEvent;
use serde_json::Value;

/// The opencode events and interception hooks the generated plugin forwards.
///
/// `status` counts against this list the way it counts Claude Code's wired events, and
/// the installer's tests assert the plugin carries them.
pub const OPENCODE_HOOK_EVENTS: &[&str] = &[
    "session.created",
    "session.deleted",
    "session.idle",
    "session.error",
    "session.compacted",
    "permission.asked",
    "permission.replied",
    "chat.message",
    "tool.execute.before",
    "tool.execute.after",
    "assistant.message",
];

/// OpenCode callbacks queue reports without waiting; the queue waits for each child
/// to close before spawning the next. Kept beside the others so `status` needs no
/// special case of its own.
pub const OPENCODE_HOOK_TIMEOUT_SECONDS: u64 = 5;

/// Translate one forwarded opencode event into the hook shape the session reads.
///
/// Every forwarded type maps to an event [`super::hook_state`] already understands, or
/// to one it deliberately ignores (`MessageDisplay`): unknown types become a
/// [`HookEvent`] named for themselves, which reports no state but still reaches the
/// plugins subscribed to the broadcast. A type the plugin never forwards is a shape
/// this module has not been taught, and answering with "nothing to say" rather than a
/// guess is the same contract Claude Code's reader keeps.
pub fn to_hook_event(event: &Value) -> HookEvent {
    let event_type = event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let properties = event.get("properties");
    let prop = |key: &str| properties.and_then(|p| p.get(key));

    let session_id = prop("sessionID")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            prop("info")
                .and_then(|info| info.get("id"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default();
    // The plugin stamps every forwarded event with the folder opencode is working in,
    // because an opencode record is complete every time it is set and an event that
    // omitted the folder would clear one an earlier event had set. Session events carry
    // their own directory as well; it is the fallback for a payload written by hand.
    let cwd = prop("cwd")
        .and_then(Value::as_str)
        .filter(|cwd| !cwd.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            prop("info")
                .and_then(|info| info.get("directory"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        });

    let mut hook = HookEvent {
        hook_event_name: event_type.to_owned(),
        session_id,
        cwd,
        ..Default::default()
    };

    match event_type {
        // A session beginning is the feed beginning: the pane is proved to run opencode.
        "session.created" => {
            hook.hook_event_name = "SessionStart".to_owned();
            hook.source = Some("startup".to_owned());
        },
        // opencode has no `SessionEnd` of its own; a deleted session is the feed ending.
        "session.deleted" => {
            hook.hook_event_name = "SessionEnd".to_owned();
        },
        // The turn ended. `last_assistant_message` and `turn_id` are tracked by the
        // plugin across the turn's message events, because opencode's idle event carries
        // neither.
        "session.idle" => {
            hook.hook_event_name = "Stop".to_owned();
            hook.last_assistant_message = string(prop("lastAssistantMessage"));
            hook.turn_id = string(prop("turnID"));
        },
        "session.error" => {
            hook.hook_event_name = "StopFailure".to_owned();
            hook.last_assistant_message = string(prop("lastAssistantMessage"));
            hook.turn_id = string(prop("turnID"));
        },
        "session.compacted" => {
            hook.hook_event_name = "PostCompact".to_owned();
        },
        // The user asked for something. opencode's `chat.message` hook hands the parts
        // over whole; the plugin has already joined their text.
        "chat.message" => {
            hook.hook_event_name = "UserPromptSubmit".to_owned();
            hook.prompt = string(prop("prompt"));
        },
        "tool.execute.before" => {
            hook.hook_event_name = "PreToolUse".to_owned();
            tool_call(&mut hook, properties);
        },
        "tool.execute.after" => {
            hook.hook_event_name = "PostToolUse".to_owned();
            tool_call(&mut hook, properties);
        },
        // opencode's permission prompt. Automatic approval under `--auto` emits no
        // prompt of its own, so anything that reaches here is a person being waited on;
        // the record's `waiting_needs_a_person` therefore comes out true, which is the
        // right reading for the only events that arrive.
        "permission.asked" => {
            hook.hook_event_name = "PermissionRequest".to_owned();
            hook.tool_name = prop("permission")
                .and_then(Value::as_str)
                .map(normalize_tool);
            hook.tool_use_id = string(prop("tool").and_then(|tool| tool.get("callID")));
            let patterns = prop("patterns")
                .and_then(Value::as_array)
                .map(|patterns| {
                    patterns
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .map(Value::String);
            let metadata = prop("metadata").map(normalize_args);
            hook.tool_input =
                permission_input(prop("permission"), patterns.as_ref(), metadata.as_ref());
        },
        // Older OpenCode versions use a type/title and a top-level call ID.
        "permission.updated" => {
            hook.hook_event_name = "PermissionRequest".to_owned();
            hook.tool_name = prop("type").and_then(Value::as_str).map(normalize_tool);
            hook.tool_use_id = string(prop("callID"));
            hook.tool_input = permission_input(prop("type"), prop("title"), prop("metadata"));
        },
        // The answer came back: the agent is working again, and the wait clears.
        "permission.replied" => {
            hook.hook_event_name = "PostToolUse".to_owned();
        },
        // One completed assistant message, handed over as the batch `MessageDisplay`
        // already means to the run history. The plugin only sends it once per message,
        // with the whole text, so one batch at index zero is the complete message.
        "assistant.message" => {
            hook.hook_event_name = "MessageDisplay".to_owned();
            hook.message_id = string(prop("messageID"));
            hook.turn_id = string(prop("messageID"));
            hook.index = Some(0);
            hook.final_batch = Some(true);
            hook.delta = string(prop("text"));
        },
        _ => {},
    }
    hook
}

/// A tool about to run or just finished, in opencode's lowercase spelling.
fn tool_call(hook: &mut HookEvent, properties: Option<&Value>) {
    let prop = |key: &str| properties.and_then(|p| p.get(key));
    hook.tool_name = prop("tool").and_then(Value::as_str).map(normalize_tool);
    hook.tool_use_id = string(prop("callID"));
    hook.tool_input = prop("args").map(normalize_args);
    hook.tool_response = prop("output").cloned();
}

/// opencode's tool name as the equivalent the shared readers already know.
///
/// `hook_detail`, `waiting_detail` and `run_entry` are keyed on the names Claude Code
/// and Codex use, so the translation is what lets one reader serve a third agent.
/// A tool nothing here knows keeps its own name, which still shows it in a
/// `busy · <tool>` row.
fn normalize_tool(tool: &str) -> String {
    match tool.to_ascii_lowercase().as_str() {
        "bash" => "Bash",
        "read" => "Read",
        "write" => "Write",
        "edit" => "Edit",
        "multiedit" => "MultiEdit",
        "notebookedit" => "NotebookEdit",
        // opencode's question tool is the pane asking a person, which is the wait
        // `AskUserQuestion` already names.
        "question" => "AskUserQuestion",
        "exitplanmode" => "ExitPlanMode",
        _ => tool,
    }
    .to_owned()
}

/// A tool's arguments with the one key the shared readers spell differently.
///
/// opencode's file tools take `filePath`; `tool_summary` reads `file_path`, because
/// that is what Claude Code calls it. Renaming here keeps one reading of the file tools
/// rather than a second, opencode-shaped branch in `tool_summary`.
fn normalize_args(args: &Value) -> Value {
    let Value::Object(fields) = args else {
        return args.clone();
    };
    let mut normalized = serde_json::Map::with_capacity(fields.len());
    for (key, value) in fields {
        let key = match key.as_str() {
            "filePath" | "filepath" => "file_path",
            other => other,
        };
        normalized.insert(key.to_owned(), value.clone());
    }
    Value::Object(normalized)
}

/// What a permission prompt is about, in the shape `waiting_detail` reads for the tool.
///
/// A `Bash` prompt reads `tool_input.command` and a file tool reads `file_path`, so the
/// prompt's legacy title or current patterns are filed under whichever key names it.
/// A tool with no obvious argument keeps its metadata, or nothing, and the toast
/// falls back to the tool name.
fn permission_input(
    tool: Option<&Value>,
    title: Option<&Value>,
    metadata: Option<&Value>,
) -> Option<Value> {
    let key = match tool.and_then(Value::as_str) {
        Some("bash") => Some("command"),
        Some("read" | "edit" | "write") => Some("file_path"),
        _ => None,
    };
    // The prompt's own metadata is kept when it already names what the tool summary
    // reads; an empty or unrelated object is not allowed to hide the title behind it.
    if let Some(Value::Object(fields)) = metadata
        && key.map_or(!fields.is_empty(), |key| fields.contains_key(key))
    {
        return metadata.cloned();
    }
    let title = title.and_then(Value::as_str).filter(|t| !t.is_empty())?;
    match key {
        Some(key) => Some(serde_json::json!({ key: title })),
        None => Some(Value::String(title.to_owned())),
    }
}

fn string(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

#[cfg(test)]
#[path = "unit/opencode_tests.rs"]
mod tests;
