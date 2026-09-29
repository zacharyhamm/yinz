//! Hocus is a client of the same background operations as the CLI.
use crate::*;
use serde_json::{json, Value};

pub(crate) const TOOLS: &[&str] = &[
    "agent_register",
    "project_assign",
    "project_open",
    "project_forget",
    "project_rename",
    "project_merge",
    "agent_runs",
    "agent_run",
    "agent_history_search",
];

impl State {
    pub(crate) fn start_background_tool(
        &mut self,
        call: &ToolCall,
        assigned_project: bool,
    ) -> bool {
        let request = (|| -> Result<_, String> {
            let parsed = tools::Args::parse(&call.arguments)?;
            let arguments = if call.arguments.trim().is_empty() {
                "{}"
            } else {
                &call.arguments
            };
            let mut args: Value =
                serde_json::from_str(arguments).map_err(|error| error.to_string())?;
            let mut op = call.name.as_str();
            let (service, pipe) = match op {
                "agent_register" => ("hocus-agentd", "agent_action"),
                "project_assign" | "project_open" if assigned_project => {
                    args["project"] =
                        json!(parsed.name.as_deref().ok_or("name is required")?.trim());
                    if op == "project_assign" {
                        args["tab_id"] = json!(parsed
                            .tab_id
                            .or_else(|| self.focused_tab())
                            .or_else(|| self.tabs.iter().find(|t| t.active).map(|t| t.tab_id))
                            .ok_or("no active tab is available")?);
                    }
                    ("hocus-agentd", "agent_action")
                },
                "project_assign" | "project_open" => {
                    op = "project_remember";
                    ("hocus-store", wire::STORE_MUTATE)
                },
                "agent_runs" | "agent_run" | "agent_history_search" => {
                    args["since_ms"] = json!(parsed.since_ms(agents::now_ms())?);
                    if parsed.this_session {
                        if self.yinz_session_name.as_str().is_empty() {
                            return Err("this session is not named yet".into());
                        }
                        args["run_session"] = json!(self.yinz_session_name);
                        args["host_id"] = json!(self.host_id);
                    }
                    if op == "agent_run" {
                        args["run_id"] = json!(self.named_run(&parsed)?);
                    }
                    ("hocus-store", wire::STORE_QUERY)
                },
                _ => ("hocus-store", wire::STORE_MUTATE),
            };
            let location = format!("yinz:{service}");
            let mut owners = self
                .plugin_instances
                .iter()
                .filter(|(_, p)| p.location == location);
            let owner = owners
                .next()
                .map(|(id, _)| *id)
                .ok_or_else(|| format!("No running {service}; load it as a background plugin"))?;
            if owners.next().is_some() {
                return Err(format!(
                    "multiple {service} instances; keep one background service"
                ));
            }
            Ok((owner, pipe, op.to_owned(), args))
        })();
        let (owner, pipe, op, args) = match request {
            Ok(request) => request,
            Err(error) => {
                self.finish_head(error);
                return true;
            },
        };
        self.workflow_seq += 1;
        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let rpc_id = format!(
            "operation-{}-{epoch}-{}",
            self.own_plugin_id, self.workflow_seq
        );
        pipe_message_to_plugin(
            MessageToPlugin::new(pipe)
                .with_destination_plugin_id(owner)
                .with_payload(
                    json!({"request_id":rpc_id,"structured_reply":true,"op":op,"args":args})
                        .to_string(),
                ),
        );
        self.workflow_call = Some(workflows::Pending {
            owner: Some(owner),
            rpc_id,
        });
        self.status = Status::AwaitingExtension;
        self.extension_call_started_ms = Some(self.mono_ms);
        self.arm_timer(FRAME_SECONDS);
        false
    }

    /// The project must be persisted before its tab is assigned or opened.
    pub(crate) fn continue_project_assignment(&mut self, owner: PluginId, result: &Value) -> bool {
        if result["ok"] != true
            || !self
                .plugin_instances
                .get(&owner)
                .is_some_and(|p| p.location == "yinz:hocus-store")
        {
            return false;
        }
        let Some(call) = self
            .pending
            .front()
            .filter(|call| matches!(call.name.as_str(), "project_assign" | "project_open"))
            .cloned()
        else {
            return false;
        };
        if self.start_background_tool(&call, true) {
            self.workflow_call = None;
            self.extension_call_started_ms = None;
            self.status = Status::Idle;
            self.pump_tools();
        }
        true
    }
}
