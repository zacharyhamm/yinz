//! The buffered Decisions tool; request validation and decoding live in hocus-proto.

use crate::{approval, State, Status, ToolCall, FRAME_SECONDS};
use hocus_proto::decisions::{self, Endpoint, Request};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use zellij_tile::prelude::{web_request, HttpVerb};

pub(crate) const REVIEW_CONTEXT: &str = "tool_review_id";
const REVIEW_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) type ExecutionConfig = (String, BTreeMap<String, String>);

pub(crate) fn parameters() -> Value {
    let input = json!({"type": ["string", "object", "array"], "items": {}});
    json!({
        "type": "object",
        "properties": {
            "state": input,
            "questions": {
                "type": "object", "minProperties": 1,
                "description": "Named questions evaluated against the same state.",
                "additionalProperties": {"anyOf": [
                    {
                        "type": "object",
                        "properties": {
                            "type": {"type": "string", "enum": ["choice"]},
                            "instructions": input,
                            "criteria": {
                                "type": "object", "minProperties": 1,
                                "description": "Option names mapped to guidance; null uses the option name.",
                                "additionalProperties": {"anyOf": [input, {"type": "null"}]}
                            }
                        },
                        "required": ["type", "instructions", "criteria"]
                    },
                    {
                        "type": "object",
                        "properties": {
                            "type": {"type": "string", "enum": ["score"]},
                            "instructions": input,
                            "criteria": {
                                "type": "array", "minItems": 1, "items": input,
                                "description": "Ordered from lowest to highest; score is the weighted mean of zero-based indices."
                            }
                        },
                        "required": ["type", "instructions", "criteria"]
                    },
                    {
                        "type": "object",
                        "properties": {
                            "type": {"type": "string", "enum": ["noul"]},
                            "instructions": input,
                            "criteria": {
                                "type": "object",
                                "properties": {"true": input, "false": input},
                                "required": ["true", "false"]
                            }
                        },
                        "required": ["type", "instructions"]
                    }
                ]}
            },
            "model": {"type": "string", "description": "Override the configured Decisions model for this call."},
            "user": {"type": "string", "maxLength": 256},
            "session_id": {"type": "string", "maxLength": 256}
        },
        "required": ["state", "questions"],
        "additionalProperties": false
    })
}

pub(crate) fn prepare(
    arguments: &str,
    kdl: &BTreeMap<String, String>,
    env: &BTreeMap<String, String>,
) -> Result<(Endpoint, Vec<u8>), String> {
    let mut args: BTreeMap<String, Value> =
        serde_json::from_str(arguments).map_err(|e| format!("invalid decisions arguments: {e}"))?;
    let model: Option<String> = serde_json::from_value(args.remove("model").unwrap_or(Value::Null))
        .map_err(|e| format!("invalid decisions model: {e}"))?;
    for key in args.keys() {
        if !["state", "questions", "user", "session_id"].contains(&key.as_str()) {
            return Err(format!("unknown decisions argument: {key}"));
        }
    }
    let request: Request = serde_json::from_value(Value::Object(args.into_iter().collect()))
        .map_err(|e| format!("invalid decisions arguments: {e}"))?;
    let endpoint = Endpoint::resolve(kdl, env)?.ok_or(
        "Decisions needs credentials: set OPENROUTER_API_KEY, or the variable named by decisions_api_key_env",
    )?;
    let body = request.body(model.as_deref().unwrap_or(&endpoint.model))?;
    Ok((endpoint, body))
}

impl State {
    fn review_execution_config(&self, call: &ToolCall) -> Result<Option<ExecutionConfig>, ()> {
        if call.name == "slack_post" {
            let cfg = self.cfg.as_ref().map_err(|_| ())?;
            Ok(Some((cfg.dobbs.clone(), cfg.dobbs_env())))
        } else if crate::tools::VM_TOOLS.contains(&call.name.as_str()) {
            let cfg = self.cfg.as_ref().map_err(|_| ())?;
            let env = self
                .env
                .iter()
                .filter(|(key, _)| {
                    matches!(key.as_str(), "ZELLIJ_CONFIG_FILE" | "ZELLIJ_CONFIG_DIR")
                })
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            Ok(Some((cfg.zellij.clone(), env)))
        } else {
            Ok(None)
        }
    }

    fn tool_review_enabled(&self) -> bool {
        self.kdl
            .get("decisions_auto_approve")
            .is_none_or(|v| v == "true")
    }

    pub(crate) fn start_tool_review(&mut self, call: &ToolCall) -> bool {
        if !self.tool_review_enabled() || !self.asks.is_empty() || self.manual_head {
            return false;
        }
        let Ok(Some(endpoint)) = Endpoint::resolve(&self.kdl, &self.env) else {
            return false;
        };
        let Ok(execution_config) = self.review_execution_config(call) else {
            return false;
        };
        let body = match approval::prepare(self, call, &endpoint) {
            Ok(body) => body,
            Err(error) => {
                self.log("tool_review_unavailable", json!({"error": error}));
                return false;
            },
        };
        // Buffered replies can survive plugin reload, which resets the counter.
        let Ok(epoch) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) else {
            return false;
        };
        // This counter belongs to the plugin, not a chat or model-supplied call id.
        self.stream_seq += 1;
        let request_id = format!(
            "hocus-review-{}-{}-{}",
            self.own_plugin_id,
            epoch.as_nanos(),
            self.stream_seq
        );
        let deadline = Instant::now().checked_add(REVIEW_TIMEOUT);
        web_request(
            &endpoint.url,
            HttpVerb::Post,
            endpoint.headers(),
            body.clone(),
            BTreeMap::from([(REVIEW_CONTEXT.into(), request_id.clone())]),
            None,
        );
        self.status = Status::ReviewingTool {
            request_id,
            url: endpoint.url,
            deadline,
            execution_config,
            call: call.clone(),
            body,
        };
        self.pin_to_bottom();
        self.arm_timer(FRAME_SECONDS);
        true
    }

    pub(crate) fn finish_tool_review(
        &mut self,
        status: u16,
        body: &[u8],
        context: &BTreeMap<String, String>,
    ) -> bool {
        let Status::ReviewingTool {
            request_id,
            url,
            deadline,
            execution_config,
            call,
            body: sent,
        } = &self.status
        else {
            return false;
        };
        if context.get(REVIEW_CONTEXT) != Some(request_id) {
            return false;
        }
        // An approval applies only to the exact call and context Jev saw. New
        // steering, changed project defaults/targets, or a late answer needs a human.
        let unchanged = self.tool_review_enabled()
            && deadline.is_some_and(|deadline| Instant::now() < deadline)
            && self.pending.front() == Some(call)
            && self.asks.is_empty()
            && !self.plan_mode_on()
            && self
                .review_execution_config(call)
                .is_ok_and(|current| current == *execution_config)
            && Endpoint::resolve(&self.kdl, &self.env)
                .ok()
                .flatten()
                .filter(|endpoint| endpoint.url == *url)
                .and_then(|endpoint| approval::prepare(self, call, &endpoint).ok())
                .is_some_and(|current| current == *sent);
        let response = decisions::parse_response(status, body);
        let approved = unchanged && response.as_ref().is_ok_and(approval::approved);
        self.log(
            "tool_review",
            json!({
                "request_id": request_id,
                "tool_call_id": call.id,
                "tool_name": call.name,
                "decision": if approved { "automatic_jev" } else { "confirmation_required" },
                "context_unchanged": unchanged,
                "response": response.as_ref().ok(),
                "error": response.as_ref().err(),
            }),
        );
        self.status = Status::Idle;
        if let Some(call) = self.pending.front().cloned() {
            if approved {
                // Use the normal dispatcher once; never turn this into standing authority.
                if self.run_head(&call) {
                    self.pump_tools();
                }
            } else {
                self.show_tool_confirmation(&call);
            }
        }
        true
    }

    pub(crate) fn tool_review_pending(&self) -> bool {
        matches!(self.status, Status::ReviewingTool { .. })
            || self
                .parked
                .iter()
                .any(|chat| matches!(chat.status, Status::ReviewingTool { .. }))
    }

    fn poll_tool_review(&mut self) -> bool {
        let Status::ReviewingTool { deadline, .. } = &self.status else {
            return false;
        };
        if self.tool_review_enabled()
            && deadline.is_some_and(|deadline| Instant::now() < deadline)
            && self.asks.is_empty()
        {
            return false;
        }
        self.log(
            "tool_review_unavailable",
            json!({"error": "review timed out, was disabled, or new user input arrived"}),
        );
        self.status = Status::Idle;
        if let Some(call) = self.pending.front().cloned() {
            self.show_tool_confirmation(&call);
        }
        true
    }

    pub(crate) fn poll_tool_reviews(&mut self) -> bool {
        let mut changed = self.poll_tool_review();
        for index in 0..self.parked.len() {
            if matches!(self.parked[index].status, Status::ReviewingTool { .. }) {
                changed |= self.with_parked(index, Self::poll_tool_review);
            }
        }
        changed
    }

    pub(crate) fn start_decisions(&mut self, call: &ToolCall) -> bool {
        let (endpoint, body) = match prepare(&call.arguments, &self.kdl, &self.env) {
            Ok(request) => request,
            Err(error) => {
                self.finish_head(error);
                return true;
            },
        };
        // ponytail: buffered requests cannot be cancelled; interruption drops the late
        // reply. Use a cancellable stream if hung requests become a problem.
        web_request(
            &endpoint.url,
            HttpVerb::Post,
            endpoint.headers(),
            body,
            BTreeMap::from([("tool_call_id".into(), call.id.clone())]),
            None,
        );
        self.status = Status::AwaitingDecisions;
        self.pin_to_bottom();
        self.arm_timer(FRAME_SECONDS);
        false
    }

    pub(crate) fn finish_decisions(
        &mut self,
        status: u16,
        body: &[u8],
        context: &BTreeMap<String, String>,
    ) -> bool {
        if self.status != Status::AwaitingDecisions
            || !self.pending.front().is_some_and(|head| {
                head.name == "openrouter_decisions" && context.get("tool_call_id") == Some(&head.id)
            })
        {
            return false;
        }
        let result = decisions::parse_response(status, body).and_then(|response| {
            serde_json::to_string(&response)
                .map_err(|e| format!("could not serialize decisions response: {e}"))
        });
        self.status = Status::Idle;
        self.finish_head(result.unwrap_or_else(|error| error));
        self.pump_tools();
        true
    }
}
