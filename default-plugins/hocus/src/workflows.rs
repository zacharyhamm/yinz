//! Native tools backed by the authenticated, managed Swamp bridge.
use crate::{extensions, tools::ToolCall, State, Status, FRAME_SECONDS};
use serde_json::{json, Value};
use yinz_tile::prelude::PluginId;

pub const TOOLS: &[&str] = &[
    "workflow_search",
    "workflow_get",
    "workflow_schema",
    "workflow_create",
    "workflow_edit",
    "workflow_delete",
    "workflow_validate",
    "workflow_evaluate",
    "workflow_active",
    "workflow_history",
    "workflow_run_get",
    "workflow_logs",
    "workflow_reports",
    "workflow_report",
    "workflow_data",
    "workflow_models",
    "workflow_model",
    "workflow_model_types",
    "workflow_model_type",
    "workflow_model_method",
    "workflow_launch",
    "workflow_resume",
    "workflow_approvals",
    "workflow_approve",
    "workflow_reject",
    "workflow_answer",
    "workflow_cancel",
    "workflow_cancel_step",
];
pub const READ_ONLY: &[&str] = &[
    "workflow_search",
    "workflow_get",
    "workflow_schema",
    "workflow_validate",
    "workflow_active",
    "workflow_history",
    "workflow_run_get",
    "workflow_logs",
    "workflow_reports",
    "workflow_report",
    "workflow_data",
    "workflow_models",
    "workflow_model",
    "workflow_model_types",
    "workflow_model_type",
    "workflow_model_method",
    "workflow_approvals",
];
pub fn mutates(name: &str) -> bool {
    TOOLS.contains(&name) && !READ_ONLY.contains(&name)
}

pub fn specs() -> Vec<Value> {
    let string = json!({"type":"string", "minLength":1});
    let object = json!({"type":"object"});
    let run = json!({"type":"string", "format":"uuid", "description":"Exact inspected run UUID. Never a workflow name or latest."});
    let workflow = json!({"type":"string", "minLength":1, "description":"Workflow name or ID from workflow_search/get."});
    let inputs = json!({"type":"object", "description":"Workflow input values."});
    let version = json!({"type":"integer", "minimum":1});
    let spec = |name, description, properties: Value, required: &[&str]| {
        json!({"type":"function", "function":{
            "name":name,"description":description,"parameters":{"type":"object","properties":properties,"required":required,"additionalProperties":false}
        }})
    };
    vec![
        spec("workflow_search", "Search Swamp definitions in the active project's control checkout.", json!({"query":string}), &[]),
        spec("workflow_get", "Inspect a definition and its complete source.content. Prepare replacements from that source text. Keep the complete result as expected before editing or deleting.", json!({"workflow":workflow}), &["workflow"]),
        spec("workflow_schema", "Fetch Swamp's authoring schema before creating or editing. With workflow, inspect its input schema.", json!({"workflow":workflow}), &[]),
        spec("workflow_create", "Create a scaffold through Swamp, which assigns its ID. Fetch schema first, then get/edit the scaffold.", json!({"name":string}), &["name"]),
        spec("workflow_edit", "Replace a previously inspected definition through Swamp. expected is the complete workflow_get result; definition is a JSON object prepared from expected.source.content and matching the schema; retain fields outside the intended change. Preserve its ID. Stale or third-party definitions are refused.", json!({"workflow":workflow,"expected":object,"definition":object}), &["workflow","expected","definition"]),
        spec("workflow_delete", "Delete the inspected project definition and its run history. Requires the complete prior workflow_get result as expected.", json!({"workflow":workflow,"expected":object}), &["workflow","expected"]),
        spec("workflow_validate", "Validate a saved workflow DAG before execution. Returns diagnostics without executing steps.", json!({"workflow":workflow}), &["workflow"]),
        spec("workflow_evaluate", "Evaluate expressions with inputs. This can persist evaluated state and requires confirmation.", json!({"workflow":workflow,"inputs":inputs}), &["workflow"]),
        spec("workflow_active", "List active executions in the control checkout.", json!({}), &[]),
        spec("workflow_history", "Search persisted workflow runs. filter is CEL over run metadata, including status, inputs and tags.", json!({"workflow":workflow,"query":string,"filter":string}), &[]),
        spec("workflow_run_get", "Inspect an exact run, its child runs, questions, outputs and owned active operation. Read failure reports before repair or retry.", json!({"run":run}), &["run"]),
        spec("workflow_logs", "Read run logs. tail bounds the server read; contains focuses the returned lines on a step or diagnostic.", json!({"run":run,"tail":{"type":"integer","minimum":1,"maximum":10000},"contains":string}), &["run"]),
        spec("workflow_reports", "Search stored reports, optionally scoped to a workflow or model. Inspect these after failures.", json!({"workflow":workflow,"model":string,"query":string}), &[]),
        spec("workflow_report", "Read a stored report, optionally selecting its exact version and variant. Use tool_result_read for retained large results.", json!({"name":string,"workflow":workflow,"model":string,"version":version,"variant":string}), &["name"]),
        spec("workflow_data", "Query versioned Swamp output. Use predicate to select exact modelName, name, version or tags.workflowRunId; select is a CEL projection for focused or sliced content.", json!({"predicate":string,"select":string,"limit":{"type":"integer","minimum":1,"maximum":1000}}), &["predicate"]),
        spec("workflow_models", "Find models used by workflow steps.", json!({"query":string}), &[]),
        spec("workflow_model", "Inspect a model definition and state without invoking methods.", json!({"model":string}), &["model"]),
        spec("workflow_model_types", "Discover installed model types.", json!({"query":string}), &[]),
        spec("workflow_model_type", "Describe a model type and its available methods and schemas.", json!({"type":string}), &["type"]),
        spec("workflow_model_method", "Get an existing model method's input and output schemas.", json!({"model":string,"method":string}), &["model","method"]),
        spec("workflow_launch", "Validate then launch through the owned bridge. Returns a launch token and run ID without waiting for completion. Never repeat an uncertain launch; inspect it.", json!({"workflow":workflow,"inputs":inputs}), &["workflow"]),
        spec("workflow_resume", "Validate then resume the exact inspected run, optionally overriding inputs or starting at from. Closed lifecycles are refused. Never repeat an uncertain resume.", json!({"run":run,"inputs":inputs,"from":{"type":"string","minLength":1,"description":"Template step name, without a job prefix."}}), &["run"]),
        spec("workflow_approvals", "List steps waiting for approval.", json!({}), &[]),
        spec("workflow_approve", "Approve a step on the exact inspected run. Does not automatically resume it.", json!({"run":run,"step":string,"reason":string}), &["run","step"]),
        spec("workflow_reject", "Reject a step on the exact inspected run.", json!({"run":run,"step":string,"reason":string}), &["run","step"]),
        spec("workflow_answer", "Submit an answer to an inspected workflow question through its owning model. The question model records the answer, approves its step and resumes the workflow.", json!({"run":run,"model":string,"question":string,"answer":string}), &["run","model","question","answer"]),
        spec("workflow_cancel", "Cancel only the active run identified by the supplied UUID through the bridge. Suspended runs are not active executions.", json!({"run":run}), &["run"]),
        spec("workflow_cancel_step", "Cancel an owned active process from workflow_run_get. Requires its exact run, work and key; preserves supervisor ownership checks.", json!({"run":run,"work":string,"key":string}), &["run","work","key"]),
    ]
}

/// Reject malformed arguments before asking for confirmation or invoking the bridge.
pub fn arguments(call: &ToolCall) -> Result<Value, String> {
    let args: Value = serde_json::from_str(&call.arguments).map_err(|e| e.to_string())?;
    let fields = args.as_object().ok_or("expected an argument object")?;
    let spec = specs()
        .into_iter()
        .find(|s| s["function"]["name"] == call.name)
        .ok_or("unknown workflow tool")?;
    let schema = &spec["function"]["parameters"];
    for required in schema["required"].as_array().into_iter().flatten() {
        let name = required.as_str().ok_or("invalid tool schema")?;
        if !fields.contains_key(name) {
            return Err(format!("{name} is required"));
        }
    }
    for (name, value) in fields {
        let property = &schema["properties"][name];
        let valid = match property["type"].as_str() {
            Some("string") => value
                .as_str()
                .is_some_and(|s| !s.trim().is_empty() && !s.contains('\0')),
            Some("object") => value.is_object(),
            Some("integer") => value.as_u64().is_some_and(|n| {
                n >= property["minimum"].as_u64().unwrap_or(0)
                    && n <= property["maximum"].as_u64().unwrap_or(u64::MAX)
            }),
            _ => false,
        };
        if !valid {
            return Err(format!("invalid or unsupported argument: {name}"));
        }
        if property["format"] == "uuid" && !is_run_id(value.as_str().unwrap_or("")) {
            return Err("run must be an exact UUID".into());
        }
    }
    if matches!(call.name.as_str(), "workflow_edit" | "workflow_delete") {
        if args["expected"]["source"]["editable"] == false {
            return Err("downloaded or external workflow definitions are inspectable only".into());
        }
        if call.name == "workflow_edit"
            && (args["expected"]["id"].as_str().is_none()
                || args["definition"]["id"] != args["expected"]["id"])
        {
            return Err("preserve the ID from the inspected workflow definition".into());
        }
    }
    Ok(args)
}

fn is_run_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

pub fn uncertain_result(call: Option<&ToolCall>, rpc_id: &str, message: &str) -> String {
    let args: Value = call
        .and_then(|call| serde_json::from_str(&call.arguments).ok())
        .unwrap_or_default();
    let mut result = json!({"error":{"code":"transport_unknown","message":message},
        "request_id":rpc_id,"workflow":args["workflow"],"run":args["run"],
        "recovery":"Inspect persisted definitions, run history, approvals or question submissions before retrying. The mutation may already have occurred."});
    if call.is_some_and(|call| call.name == "workflow_launch") {
        let mut filter = format!(
            "has(tags.yinzOperation) && tags.yinzOperation.contains({})",
            json!(rpc_id)
        );
        if args["inputs"]["launch"].is_string() && args["inputs"]["session"].is_string() {
            filter = format!(
                "({filter}) || (has(inputs.launch) && has(inputs.session) && inputs.launch == {} && inputs.session == {})",
                args["inputs"]["launch"], args["inputs"]["session"]
            );
        }
        result["history_filter"] = json!(filter);
    }
    result.to_string()
}

#[derive(Clone, Debug)]
pub struct Pending {
    pub owner: Option<PluginId>,
    pub rpc_id: String,
}

#[cfg(target_arch = "wasm32")]
fn launch_bridge(payload: String) {
    yinz_tile::shim::pipe_message_to_plugin(
        yinz_tile::prelude::MessageToPlugin::new(extensions::CALL)
            .with_plugin_url("yinz:swamp-bridge")
            .with_payload(payload),
    );
}
#[cfg(not(target_arch = "wasm32"))]
fn launch_bridge(_payload: String) {}

impl State {
    fn verified_workflow_bridge(&self, owner: PluginId) -> bool {
        #[cfg(target_arch = "wasm32")]
        return yinz_tile::shim::get_session_list().is_ok_and(|snapshot| {
            snapshot
                .live_sessions
                .iter()
                .filter(|session| session.is_current_session)
                .any(|session| {
                    session
                        .plugins
                        .get(&owner)
                        .is_some_and(|p| p.location == "yinz:swamp-bridge")
                })
        });
        #[cfg(not(target_arch = "wasm32"))]
        self.plugin_instances
            .get(&owner)
            .is_some_and(|p| p.location == "yinz:swamp-bridge")
    }
    pub(crate) fn start_workflow_tool(&mut self, call: &ToolCall) -> bool {
        let request = (|| {
            let args = arguments(call)?;
            let directory = self.project_target()?.root.clone();
            let mut owners = self
                .plugin_instances
                .iter()
                .filter(|(_, p)| p.location == "yinz:swamp-bridge");
            let owner = owners.next().map(|(id, _)| *id);
            if owners.next().is_some() {
                return Err("multiple Swamp bridges; select one session bridge".to_owned());
            }
            Ok((
                owner,
                json!({"tool":call.name,"args":args,"directory":directory}),
            ))
        })();
        let (owner, payload) = match request {
            Ok(value) => value,
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
        let rpc_id = format!("hocus-{}-{epoch}-{}", self.own_plugin_id, self.workflow_seq);
        let encoded =
            json!({"version":1,"call_id":rpc_id,"name":"hocus_workflow","payload":payload})
                .to_string();
        if let Some(owner) = owner {
            extensions::call(owner, encoded);
        } else {
            launch_bridge(encoded);
        }
        self.workflow_call = Some(Pending { owner, rpc_id });
        self.status = Status::AwaitingExtension;
        self.extension_call_started_ms = Some(self.mono_ms);
        self.arm_timer(FRAME_SECONDS);
        false
    }

    pub(crate) fn on_workflow_reply(
        &mut self,
        owner: PluginId,
        payload: &str,
        progress: bool,
    ) -> bool {
        if payload.len() > 1_048_576 {
            return false;
        }
        let Ok(value) = serde_json::from_str::<Value>(payload) else {
            return false;
        };
        if value["version"] != 1 {
            return false;
        }
        let matches = |pending: &Option<Pending>| {
            pending.as_ref().is_some_and(|p| {
                p.owner.is_none_or(|id| id == owner) && value["call_id"] == p.rpc_id
            })
        };
        if let Some(index) = self
            .parked
            .iter()
            .position(|chat| matches(&chat.workflow_call))
        {
            return self.with_parked(index, |state| {
                state.on_workflow_reply(owner, payload, progress)
            });
        }
        if !matches(&self.workflow_call) || !matches!(self.status, Status::AwaitingExtension) {
            return false;
        }
        if self
            .workflow_call
            .as_ref()
            .is_some_and(|p| p.owner.is_none())
        {
            if !self.verified_workflow_bridge(owner) {
                return false;
            }
            if let Some(pending) = &mut self.workflow_call {
                pending.owner = Some(owner);
            }
        }
        if progress {
            if value.get("payload").is_none()
                || value.get("error").is_some()
                || value.get("result").is_some()
            {
                return false;
            }
            self.extension_call_started_ms = Some(self.mono_ms);
            return true;
        }
        if value.get("result").is_none() && value.get("error").and_then(Value::as_str).is_none() {
            return false;
        }
        let result = if let Some(error) = value["error"].as_str() {
            uncertain_result(
                self.pending.front(),
                value["call_id"].as_str().unwrap_or(""),
                error,
            )
        } else {
            value["result"].to_string()
        };
        self.workflow_call = None;
        self.extension_call_started_ms = None;
        self.status = Status::Idle;
        // finish_head retains the full result; request assembly chooses the preview.
        self.finish_head(result);
        self.pump_tools();
        true
    }
}
