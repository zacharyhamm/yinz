//! The buffered Decisions tool; request validation and decoding live in hocus-proto.

use crate::{State, Status, ToolCall, FRAME_SECONDS};
use hocus_proto::decisions::{self, Endpoint, Request};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use zellij_tile::prelude::{web_request, HttpVerb};

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
