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
    let notice = Notice {
        task: field("task-id")?,
        tool: field("tool-use-id")?,
        run: row["origin"]["runId"].as_str()?.into(),
        prompt: prompt.into(),
        row: row["uuid"].as_str()?.into(),
    };
    let valid = [&notice.run, &notice.prompt, &notice.row]
        .into_iter()
        .all(|s| valid_id(s));
    valid.then_some(notice)
}
