//! Structural accepted-root mapping; never recognize an envelope embedded in ordinary prose.
use serde_json::Value;
use yinz_types::agent::claude_notifications::{valid_id, Notice};

pub(super) fn accepted(row: &Value, session: &str, prompt: &str) -> Option<Notice> {
    if row["type"] != "user"
        || row["isSidechain"] != false
        || row["sessionId"].as_str().is_some_and(|s| s != session)
        || row["promptId"].as_str() != Some(prompt)
        || row["origin"]["kind"] != "task-notification"
        || row["origin"]["producer"] != "session-task"
    {
        return None;
    }
    let content = &row["message"]["content"];
    let text = if let Some(text) = content.as_str() {
        text
    } else {
        let parts = content.as_array()?;
        if parts.len() != 1 || parts[0]["type"] != "text" {
            return None;
        }
        parts[0]["text"].as_str()?
    };
    let (task, tool) = envelope(text)?;
    notice(
        task,
        tool,
        row["origin"]["runId"].as_str()?,
        prompt,
        row,
        false,
    )
}

/// A root task notification Claude delivered while `prompt`'s turn was running. Claude
/// 2.1.325 folds it into that turn as a `queued_command` attachment row, without a prompt
/// ID of its own, instead of starting a notification turn. The caller must establish that
/// the row descends from the observed turn; this checks only the row's own structure.
pub(super) fn queued(row: &Value, session: &str, prompt: &str) -> Option<Notice> {
    let attachment = &row["attachment"];
    if row["type"] != "attachment"
        || row["isSidechain"] != false
        || row["sessionId"].as_str().is_some_and(|s| s != session)
        || row
            .get("promptId")
            .is_some_and(|id| id.as_str() != Some(prompt))
        || attachment["type"] != "queued_command"
        || attachment["commandMode"] != "task-notification"
        || attachment["origin"]["kind"] != "task-notification"
        || attachment["origin"]["producer"] != "session-task"
    {
        return None;
    }
    let (task, tool) = envelope(attachment["prompt"].as_str()?)?;
    notice(task, tool, attachment["runId"].as_str()?, prompt, row, true)
}

/// The task and tool IDs of exactly one `<task-notification>` envelope spanning `text`.
/// Monitor events carry no tool-use ID and are therefore never matched.
fn envelope(text: &str) -> Option<(String, String)> {
    let body = text
        .trim()
        .strip_prefix("<task-notification>")?
        .strip_suffix("</task-notification>")?;
    if body.contains("<task-notification>") {
        return None;
    }
    let field = |tag: &str| -> Option<String> {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        let mut parts = body.split(&open);
        parts.next()?;
        let value = parts.next()?.split_once(&close)?.0;
        if parts.next().is_some() || !valid_id(value) {
            return None;
        }
        Some(value.into())
    };
    Some((field("task-id")?, field("tool-use-id")?))
}

fn notice(
    task: String,
    tool: String,
    run: &str,
    prompt: &str,
    row: &Value,
    queued: bool,
) -> Option<Notice> {
    let notice = Notice {
        task,
        tool,
        run: run.into(),
        prompt: prompt.into(),
        row: row["uuid"].as_str()?.into(),
        queued,
    };
    let valid = [&notice.run, &notice.prompt, &notice.row]
        .into_iter()
        .all(|s| valid_id(s));
    valid.then_some(notice)
}
