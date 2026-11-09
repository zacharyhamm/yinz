//! Claude Code 2.1.319 native result mapping (issue-69 S1, A1–A15).
use serde_json::Value;
use yinz_types::agent::{claude_notifications::*, HookEvent};

pub(super) fn normalize(event: &HookEvent, raw: &Value) -> Option<Evidence> {
    // Internal observer facts are already normalized; never reconstruct them from prose.
    if let Some(evidence) = &event.claude_notifications {
        return Some(evidence.bounded());
    }
    let mut evidence = Evidence::default();
    if event.session_id.starts_with("served:")
        || event.is_worker()
        || raw
            .get("agent_id")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
    {
        return None;
    }
    if event.hook_event_name == "Stop"
        && let Some(tasks) = raw.get("background_tasks").and_then(Value::as_array)
    {
        if tasks.len() > MAX_FACTS {
            evidence.unavailable = true;
        } else {
            for task in tasks {
                if let (Some(id), Some(kind), Some(status)) = (
                    task["id"].as_str(),
                    task["type"].as_str(),
                    task["status"].as_str(),
                ) {
                    evidence.tasks.push(TaskSnapshot {
                        id: id.into(),
                        kind: kind.into(),
                        status: status.into(),
                    });
                } else {
                    evidence.unavailable = true;
                }
            }
        }
    }
    if event.hook_event_name == "PostToolUse"
        && event.is_error != Some(true)
        && event.error.is_none()
    {
        let response = event.tool_response.as_ref().unwrap_or(&Value::Null);
        let prompt = event.prompt_id.as_deref().unwrap_or_default();
        let tool = event.tool_use_id.as_deref().unwrap_or_default();
        let task = match event.tool_name.as_deref() {
            Some("Bash") => response["backgroundTaskId"]
                .as_str()
                .map(|id| (id, TaskKind::Bash)),
            Some("Agent")
                if response["isAsync"] == true && response["status"] == "async_launched" =>
            {
                response["agentId"].as_str().map(|id| (id, TaskKind::Agent))
            },
            _ => None,
        };
        if let Some((task, kind)) = task {
            evidence.launches.push(Launch {
                task: task.into(),
                tool: tool.into(),
                prompt: prompt.into(),
                kind,
            });
        }
        // The local result includes command; the remote branch has only message/task/type.
        if event.tool_name.as_deref() == Some("TaskStop")
            && let Some(task) = response["task_id"].as_str()
        {
            if response["task_type"] == "local_bash"
                && response["command"].is_string()
                && event
                    .tool_input
                    .as_ref()
                    .and_then(|v| v["task_id"].as_str())
                    == Some(task)
            {
                evidence.cancellation = Some(Cancellation {
                    task: task.into(),
                    prompt: prompt.into(),
                    tool: tool.into(),
                });
            } else {
                // Unsupported routing is observation loss for this task, not a no-notice
                // disposition. In particular remote apparent success proves no cancellation.
                evidence.unsupported_tasks.push(task.into());
            }
        }
        if event.tool_name.as_deref() == Some("SendMessage") && response["success"] == true {
            for key in ["resumedAgentId", "revivedAgentId"] {
                if let Some(task) = response[key].as_str() {
                    evidence.unsupported_tasks.push(task.into());
                }
            }
        }
    }
    if (!evidence.launches.is_empty() || evidence.cancellation.is_some())
        && !valid_id(&event.session_id)
    {
        evidence.launches.clear();
        evidence.cancellation = None;
        evidence.unavailable = true;
    }
    (!evidence.launches.is_empty()
        || evidence.cancellation.is_some()
        || !evidence.tasks.is_empty()
        || !evidence.unsupported_tasks.is_empty()
        || evidence.unavailable)
        .then(|| evidence.bounded())
}
