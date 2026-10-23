//! Bounded archive previews. Redaction is heuristic, not a secret-free guarantee.
use super::HookEvent;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const ARGUMENT_BYTES: usize = 16 * 1024;
const OUTPUT_BYTES: usize = 64 * 1024;
const ERROR_BYTES: usize = 8 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payload {
    pub text: String,
    pub format: String,
    pub original_bytes: usize,
    pub redacted: bool,
    pub truncated: bool,
    pub attachments_omitted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCapture {
    pub name: String,
    pub call_id: Option<String>,
    pub turn_id: Option<String>,
    pub worker_id: Option<String>,
    pub parent_call_id: Option<String>,
    pub source_event: String,
    pub status: String,
    pub arguments: Option<Payload>,
    pub output: Option<Payload>,
    pub error: Option<Payload>,
    pub duration_ms: Option<u64>,
    pub output_unavailable: bool,
    pub metadata_truncated: bool,
    pub metadata_redacted: bool,
    /// Hash of the original worker/call/event identity, before bounded display previews.
    pub identity: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub payload_omitted: bool,
}

pub use crate::secrets::redact_secrets;

fn credential(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace(['-', '_'], "");
    [
        "password",
        "passwd",
        "secret",
        "apikey",
        "accesstoken",
        "refreshtoken",
        "authorization",
        "cookie",
        "privatekey",
        "accesskey",
    ]
    .iter()
    .any(|part| key.contains(part))
        || key.ends_with("token")
        || key.ends_with("tokens")
}

fn clean(value: &mut Value, redacted: &mut bool, omitted: &mut bool) {
    match value {
        Value::Object(object) => {
            let attachment = object
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| {
                    matches!(
                        kind,
                        "image"
                            | "audio"
                            | "image_url"
                            | "input_audio"
                            | "base64"
                            | "file"
                            | "document"
                    )
                });
            for (key, value) in object {
                if credential(key) {
                    *value = Value::String("[REDACTED]".into());
                    *redacted = true;
                } else if matches!(
                    key.as_str(),
                    "base64" | "image_data" | "audio_data" | "blob"
                ) || (attachment
                    && matches!(key.as_str(), "data" | "body" | "bytes" | "image" | "audio"))
                {
                    *value = Value::String("[attachment omitted]".into());
                    *omitted = true;
                } else {
                    clean(value, redacted, omitted);
                }
            }
        },
        Value::Array(values) => values.iter_mut().for_each(|v| clean(v, redacted, omitted)),
        Value::String(text) => {
            *text = literal(text);
            if text.starts_with("data:") && text.contains(";base64,") {
                *text = "[attachment omitted]".into();
                *omitted = true;
            } else {
                let safe = redact_secrets(text);
                *redacted |= safe != *text;
                *text = safe;
            }
        },
        _ => {},
    }
}

fn literal(text: &str) -> String {
    let mut text =
        String::from_utf8_lossy(&strip_ansi_escapes::strip(text.as_bytes())).into_owned();
    text.retain(|c| !c.is_control() || matches!(c, '\n' | '\t'));
    text
}

fn cut(text: &mut String, limit: usize) -> bool {
    if text.len() <= limit {
        return false;
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    true
}

fn metadata(text: &str, truncated: &mut bool, redacted: &mut bool) -> String {
    let mut text = literal(text);
    text.retain(|c| !c.is_control());
    let mut safe = redact_secrets(&text);
    *redacted |= safe != text;
    *truncated |= cut(&mut safe, 512);
    while serde_json::to_string(&safe).map_or(usize::MAX, |v| v.len()) > 512 {
        let next = safe.len() * 3 / 4;
        *truncated |= cut(&mut safe, next);
    }
    safe
}

fn payload(value: &Value, limit: usize, retained: Option<&Payload>) -> Payload {
    let original_bytes = match value {
        Value::String(s) => s.len(),
        _ => value.to_string().len(),
    };
    let mut value = value.clone();
    let mut redacted = false;
    let mut attachments_omitted = false;
    clean(&mut value, &mut redacted, &mut attachments_omitted);
    let (text, format) = match value {
        Value::String(text) => (text, "text"),
        value => (value.to_string(), "json"),
    };
    let mut result = Payload {
        truncated: false,
        text,
        format: format.into(),
        original_bytes,
        redacted,
        attachments_omitted,
    };
    if let Some(retained) = retained {
        result.original_bytes = retained.original_bytes;
        result.redacted |= retained.redacted;
        result.truncated |= retained.truncated;
        result.attachments_omitted |= retained.attachments_omitted;
    }
    result.bound(limit);
    result
}

impl Payload {
    fn bound(&mut self, limit: usize) {
        self.truncated |= cut(&mut self.text, limit);
        // JSON escaping and the retained provenance count against the byte budget too.
        while serde_json::to_vec(&self).map_or(usize::MAX, |v| v.len()) > limit {
            let next = self.text.len() * 3 / 4;
            cut(&mut self.text, next);
            self.truncated = true;
        }
        if self.format == "json"
            && self.truncated
            && serde_json::from_str::<Value>(&self.text).is_err()
        {
            // A JSON fragment must already satisfy the text fallback on its next hop.
            self.format = "text".into();
            let safe = redact_secrets(&self.text);
            self.redacted |= safe != self.text;
            self.text = safe;
            self.bound(limit);
        }
    }
}

pub fn prepare(provider: &str, event: &HookEvent) -> Option<ToolCapture> {
    let status = match event.hook_event_name.as_str() {
        "PreToolUse" => "requested",
        "PostToolUseFailure" if event.is_interrupt == Some(true) => "cancelled",
        "PostToolUseFailure" => "failed",
        "ToolUseDenied" => "denied",
        "PostToolUse" => {
            let response = event.tool_response.as_ref();
            let reported_error = event.is_error.or_else(|| {
                response.and_then(|v| {
                    v.get("isError")
                        .or_else(|| v.get("is_error"))
                        .and_then(Value::as_bool)
                })
            });
            let exit_code = response
                .and_then(|v| v.get("exit_code"))
                .and_then(Value::as_i64);
            let has_error = |value: &Value| {
                !value.is_null() && value != &Value::Bool(false) && value.as_str() != Some("")
            };
            let failed = reported_error == Some(true)
                || exit_code.is_some_and(|code| code != 0)
                || event.error.as_ref().is_some_and(has_error)
                || response.and_then(|v| v.get("error")).is_some_and(has_error);
            if event.is_interrupt == Some(true) {
                "cancelled"
            } else if failed {
                "failed"
            } else if reported_error == Some(false) || exit_code == Some(0) || provider == "pi" {
                "succeeded"
            } else {
                "completed"
            }
        },
        _ => return None,
    };
    // Identity-free permission reminders are not tool completions.
    if event.tool_name.is_none() && event.tool_use_id.is_none() {
        return None;
    }
    let mut metadata_truncated = false;
    let mut metadata_redacted = false;
    let mut metadata = |text: &str| metadata(text, &mut metadata_truncated, &mut metadata_redacted);
    let capture = ToolCapture {
        name: metadata(event.tool_name.as_deref().unwrap_or("unknown")),
        call_id: event
            .tool_use_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .map(&mut metadata),
        turn_id: event.turn_id.as_deref().map(&mut metadata),
        worker_id: event.agent_id.as_deref().map(&mut metadata),
        parent_call_id: event.parent_tool_use_id.as_deref().map(&mut metadata),
        source_event: metadata(&event.hook_event_name),
        status: status.into(),
        arguments: event
            .tool_input
            .as_ref()
            .map(|v| payload(v, ARGUMENT_BYTES, None)),
        output: event
            .tool_response
            .as_ref()
            .map(|v| payload(v, OUTPUT_BYTES, None)),
        error: event.error.as_ref().map(|v| payload(v, ERROR_BYTES, None)),
        duration_ms: event.duration_ms,
        output_unavailable: status != "requested" && event.tool_response.is_none(),
        metadata_truncated,
        metadata_redacted,
        identity: event
            .tool_use_id
            .as_ref()
            .filter(|id| !id.is_empty())
            .map(|id| {
                format!(
                    "tool:{}",
                    crate::agent::control::prompt_sha256(
                        &serde_json::json!([event.agent_id, id, event.hook_event_name]).to_string()
                    )
                )
            }),
        payload_omitted: false,
    };
    Some(capture)
}

impl ToolCapture {
    /// Validate a forwarded capture without deriving its identity or outcome from pruned fields.
    pub fn normalize(&mut self) -> serde_json::Result<()> {
        if self.identity.as_ref().is_some_and(|identity| {
            !identity.strip_prefix("tool:").is_some_and(|hash| {
                hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        }) || !matches!(
            self.status.as_str(),
            "requested" | "cancelled" | "failed" | "denied" | "succeeded" | "completed"
        ) {
            return Err(<serde_json::Error as serde::de::Error>::custom(
                "invalid tool capture identity or status",
            ));
        }
        for text in [
            Some(&mut self.name),
            self.call_id.as_mut(),
            self.turn_id.as_mut(),
            self.worker_id.as_mut(),
            self.parent_call_id.as_mut(),
            Some(&mut self.source_event),
        ]
        .into_iter()
        .flatten()
        {
            *text = metadata(
                text,
                &mut self.metadata_truncated,
                &mut self.metadata_redacted,
            );
        }
        for (preview, limit) in [
            (&mut self.arguments, ARGUMENT_BYTES),
            (&mut self.output, OUTPUT_BYTES),
            (&mut self.error, ERROR_BYTES),
        ] {
            if let Some(preview) = preview {
                if preview.format != "json" {
                    preview.format = "text".into();
                }
                if self.payload_omitted {
                    preview.text.clear();
                    continue;
                }
                let value = (preview.format == "json")
                    .then(|| serde_json::from_str::<Value>(&preview.text).ok())
                    .flatten()
                    .unwrap_or_else(|| Value::String(preview.text.clone()));
                *preview = payload(&value, limit, Some(preview));
            }
        }
        Ok(())
    }

    pub fn event_id(&self) -> Option<String> {
        self.identity.clone()
    }

    pub fn text(&self) -> String {
        let mut text = format!("{} · {}", self.name, self.status);
        if let Some(worker) = &self.worker_id {
            text.push_str(&format!("\nworker: {worker}"));
        }
        for (label, value) in [
            ("call", &self.call_id),
            ("turn", &self.turn_id),
            ("parent call", &self.parent_call_id),
        ] {
            if let Some(value) = value {
                text.push_str(&format!("\n{label}: {value}"));
            }
        }
        for (label, value) in [
            ("arguments", &self.arguments),
            ("output", &self.output),
            ("error", &self.error),
        ] {
            if let Some(value) = value {
                text.push_str(&format!("\n{label}: {}", value.text));
            }
        }
        text
    }

    pub fn omit_payload(&mut self) {
        for payload in [&mut self.arguments, &mut self.output, &mut self.error]
            .into_iter()
            .flatten()
        {
            payload.text.clear();
        }
        self.payload_omitted = true;
    }
}

#[cfg(test)]
#[path = "unit/tool_capture.rs"]
mod tests;
