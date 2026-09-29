//! Live automation uses the host's lifecycle controller, including delayed replies.
use crate::*;
use yinz_tile::agent::control::AgentRequest;

pub(crate) const TOOLS: &[&str] = &[
    "agent_list",
    "agent_spawn",
    "agent_status",
    "agent_prompt",
    "agent_wait",
    "agent_read",
    "agent_recover",
    "agent_send_keys",
];

impl State {
    pub(crate) fn agent_request(&self, call: &ToolCall) -> Result<AgentRequest, String> {
        self.agent_request_with_focus(call, tools::focused_pane_cwd)
    }

    pub(crate) fn agent_request_with_focus(
        &self,
        call: &ToolCall,
        focused_cwd: impl FnOnce() -> Option<std::path::PathBuf>,
    ) -> Result<AgentRequest, String> {
        let args = tools::Args::parse(&call.arguments)?;
        let raw: serde_json::Value = serde_json::from_str(if call.arguments.trim().is_empty() {
            "{}"
        } else {
            &call.arguments
        })
        .map_err(|e| e.to_string())?;
        if call.name == "agent_list" {
            return Ok(AgentRequest::List {
                project: args.project.or_else(|| self.active_project.clone()),
            });
        }
        if call.name == "agent_spawn" {
            if raw.get("prompt").is_some() || raw.get("reuse_prompt").is_some() {
                return Err(
                    "spawn does not accept prompt or reuse_prompt; spawn, wait, then prompt".into(),
                );
            }
            let mut spec = args.launch_spec();
            if args.firecracker {
                spec.firecracker = Some(launch::unique_firecracker_vm());
            }
            let project = args.project.or_else(|| self.active_project.clone());
            let cwd = args.cwd.map(Into::into).or_else(|| {
                (project.is_none()
                    && args.remote_host.is_none()
                    && (spec.firecracker.is_none() || spec.branch.is_some()))
                .then(focused_cwd)
                .flatten()
            });
            return Ok(AgentRequest::Spawn {
                spec,
                cwd,
                project,
                automatic_firecracker_vm: args.firecracker,
                remote_host: args.remote_host,
            });
        }
        let pane_id = PaneId::Terminal(TerminalId::new(args.pane_id.ok_or("pane_id is required")?));
        let expected_session_id = args.conversation;
        Ok(match call.name.as_str() {
            "agent_status" => AgentRequest::Status {
                pane_id,
                expected_session_id,
            },
            "agent_prompt" => AgentRequest::Prompt {
                pane_id,
                expected_session_id,
                prompt: args.text.ok_or("text is required")?,
                wait: args.wait,
                timeout_ms: args
                    .timeout_ms
                    .unwrap_or(if args.wait { 600_000 } else { 30_000 }),
            },
            "agent_wait" => AgentRequest::Wait {
                pane_id,
                expected_session_id,
                until: yinz_tile::agent::AgentState::parse_lenient(
                    args.until.as_deref().unwrap_or("idle"),
                )
                .ok_or("until must be busy, waiting, idle or exited")?,
                timeout_ms: args.timeout_ms.unwrap_or(30_000),
            },
            "agent_read" => AgentRequest::Read {
                pane_id,
                expected_session_id,
                lines: args.line_count.unwrap_or(80),
            },
            "agent_recover" => AgentRequest::Recover {
                pane_id,
                expected_session_id: expected_session_id.ok_or("conversation is required")?,
                confirm_empty: args.confirm_empty,
            },
            "agent_send_keys" => AgentRequest::SendKeys {
                pane_id,
                expected_session_id,
                keys: args
                    .keys
                    .iter()
                    .map(|key| key.parse().map_err(|e| format!("invalid key {key:?}: {e}")))
                    .collect::<Result<_, _>>()?,
            },
            _ => return Err("unknown agent operation".into()),
        })
    }

    pub(crate) fn start_agent_tool(&mut self, call: &ToolCall) -> bool {
        let result = self.agent_request(call).and_then(|request| {
            self.workflow_seq += 1;
            let epoch = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let id = format!(
                "agent-{}-{epoch}-{}-{}",
                self.own_plugin_id, self.workflow_seq, call.id
            );
            yinz_tile::shim::agent_control(id.clone(), &request)?;
            self.workflow_call = Some(workflows::Pending {
                owner: None,
                rpc_id: id,
            });
            Ok(())
        });
        if let Err(error) = result {
            self.finish_head(error);
            return true;
        }
        self.status = Status::AwaitingExtension;
        self.extension_call_started_ms = Some(self.mono_ms);
        self.arm_timer(FRAME_SECONDS);
        false
    }

    pub(crate) fn on_agent_response(&mut self, payload: &str) -> bool {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
            return false;
        };
        let matches = |pending: &Option<workflows::Pending>| {
            pending.as_ref().is_some_and(|p| value["id"] == p.rpc_id)
        };
        if let Some(index) = self
            .parked
            .iter()
            .position(|chat| matches(&chat.workflow_call))
        {
            return self.with_parked(index, |state| state.on_agent_response(payload));
        }
        if !matches(&self.workflow_call) || !matches!(self.status, Status::AwaitingExtension) {
            return false;
        }
        self.workflow_call = None;
        self.extension_call_started_ms = None;
        self.status = Status::Idle;
        self.finish_head(value["response"].to_string());
        self.pump_tools();
        true
    }
}
