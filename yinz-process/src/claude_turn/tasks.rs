//! Claude 2.1.318's persisted launch results and task notifications. See the
//! sanitized provider fixture: raw worker hooks cannot identify an execution.
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::{ClaudeTaskExecution, HookEvent};

pub(super) struct Tasks {
    session: String,
    calls: BTreeMap<String, String>,
    executions: BTreeMap<String, (String, bool)>,
}

impl Tasks {
    pub(super) fn new(session: &str) -> Self {
        Self {
            session: session.into(),
            calls: BTreeMap::new(),
            executions: BTreeMap::new(),
        }
    }

    #[cfg(test)]
    pub(super) fn pending(&self) -> bool {
        self.executions.values().any(|(_, complete)| !complete)
    }

    pub(super) fn pending_for(&self, tools: &HashSet<String>) -> bool {
        // Historical launches with no supported completion must not keep every
        // subsequent prompt observer alive until its deadline.
        tools
            .iter()
            .any(|tool| self.executions.get(tool).is_some_and(|(_, done)| !done))
    }

    pub(super) fn record(&mut self, record: &Value) -> Result<Vec<HookEvent>> {
        if record["isSidechain"] == true || record["sessionId"].as_str() != Some(&self.session) {
            return Ok(vec![]);
        }
        // Fail closed instead of evicting tombstones while old records remain admissible.
        if self.calls.len() >= 4096 || self.executions.len() >= 4096 {
            return Err(anyhow!("Claude task observation exceeds 4096 executions"));
        }
        if record["type"] == "assistant"
            && let Some(parts) = record["message"]["content"].as_array()
        {
            for part in parts {
                if part["type"] == "tool_use"
                    && let Some(name @ ("Agent" | "Task" | "SendMessage")) = part["name"].as_str()
                    && let Some(id) = identity(&part["id"])
                {
                    self.calls.entry(id.into()).or_insert_with(|| name.into());
                }
            }
        }
        if record["type"] == "user"
            && let Some(parts) = record["message"]["content"].as_array()
        {
            // toolUseResult describes one tool, so never guess among multiple results.
            let mut results = parts.iter().filter(|part| part["type"] == "tool_result");
            if let Some(result) = results.next()
                && results.next().is_none()
                && result["is_error"] != true
                && let Some(tool) = identity(&result["tool_use_id"])
                && let Some(name) = self.calls.get(tool)
            {
                let data = &record["toolUseResult"];
                let task = if matches!(name.as_str(), "Agent" | "Task")
                    && data["isAsync"] == true
                    && data["status"] == "async_launched"
                {
                    identity(&data["agentId"])
                } else if name == "SendMessage" && data["success"] == true {
                    identity(&data["resumedAgentId"])
                } else {
                    None
                };
                if let Some(task) = task
                    && !self.executions.contains_key(tool)
                {
                    self.executions.insert(tool.into(), (task.into(), false));
                    return Ok(vec![self.event(task, tool, None)]);
                }
            }
        }
        let attachment = &record["attachment"];
        let (origin, run, text) = if record["type"] == "attachment"
            && attachment["type"] == "queued_command"
            && attachment["commandMode"] == "task-notification"
        {
            (
                &attachment["origin"],
                &attachment["runId"],
                &attachment["prompt"],
            )
        } else if record["type"] == "user" {
            (
                &record["origin"],
                &record["origin"]["runId"],
                &record["message"]["content"],
            )
        } else {
            return Ok(vec![]);
        };
        if origin["kind"] != "task-notification" || origin["producer"] != "session-task" {
            return Ok(vec![]);
        }
        let Some(run) = identity(run) else {
            return Ok(vec![]);
        };
        let Some(text) = text.as_str() else {
            return Ok(vec![]);
        };
        let Some((task, tool)) = notification(text) else {
            return Ok(vec![]);
        };
        if let Some((known, complete)) = self.executions.get_mut(tool)
            && known == task
            && !*complete
        {
            *complete = true;
            return Ok(vec![self.event(task, tool, Some(run))]);
        }
        Ok(vec![])
    }

    fn event(&self, task: &str, tool: &str, run: Option<&str>) -> HookEvent {
        HookEvent {
            session_id: self.session.clone(),
            agent_id: Some(task.into()),
            hook_event_name: if run.is_some() {
                "SubagentStop"
            } else {
                "SubagentStart"
            }
            .into(),
            claude_task_execution: Some(ClaudeTaskExecution {
                tool_use_id: tool.into(),
                run_id: run.map(str::to_owned),
            }),
            ..Default::default()
        }
    }
}

fn identity(value: &Value) -> Option<&str> {
    value.as_str().filter(|id| valid_id(id))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn notification(text: &str) -> Option<(&str, &str)> {
    let body = text
        .trim()
        .strip_prefix("<task-notification>")?
        .strip_suffix("</task-notification>")?;
    if body.contains("<task-notification>") {
        return None;
    }
    let field = |name| {
        let open = format!("<{name}>");
        let close = format!("</{name}>");
        let (_, rest) = body.split_once(&open)?;
        if rest.contains(&open) {
            return None;
        }
        rest.split_once(&close).map(|(value, _)| value)
    };
    if field("status")? != "completed" {
        return None;
    }
    let task = field("task-id")?;
    let tool = field("tool-use-id")?;
    (valid_id(task) && valid_id(tool)).then_some((task, tool))
}
