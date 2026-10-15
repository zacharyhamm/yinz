//! JSON event encoding without runtime-specific values.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use yinz_runtime::errors::prelude::*;
use yinz_types::data::Event;

pub(crate) fn event_to_json(event: &Event) -> Result<Value> {
    use yinz_steel::binary::encode;

    match event {
        Event::RunCommandResult(code, stdout, stderr, context) => {
            anyhow::ensure!(
                stdout.len().saturating_add(stderr.len()) <= yinz_steel::binary::MAX_BYTES,
                "event binary payload exceeds 64 MiB"
            );
            Ok(json!({
                "RunCommandResult": [code, encode(stdout)?, encode(stderr)?, context]
            }))
        },
        Event::WebRequestResult(status, headers, body, context) => Ok(json!({
            "WebRequestResult": [status, headers, encode(body)?, context]
        })),
        Event::PaneOutput {
            pane_id,
            data,
            context,
        } => Ok(json!({
            "PaneOutput": { "pane_id": pane_id, "data": encode(data)?, "context": context }
        })),
        Event::RunCommandOutput {
            data,
            stderr,
            context,
        } => Ok(json!({
            "RunCommandOutput": { "data": encode(data)?, "stderr": stderr, "context": context }
        })),
        Event::WebRequestStreamChunk {
            stream_id,
            chunk,
            done,
            error,
        } => Ok(json!({
            "WebRequestStreamChunk": {
                "stream_id": stream_id, "chunk": encode(chunk)?, "done": done, "error": error
            }
        })),
        Event::PluginDataChanged { watch_id, rows, truncated } => Ok(json!({
            "PluginDataChanged": { "watch_id": watch_id, "rows": crate::binary_data::rows(rows)?, "truncated": truncated }
        })),
        Event::PaneRenderReport(panes) => {
            // JSON object keys cannot be externally tagged enums. Use the same readable
            // pane identity as the CLI, preserving terminal/plugin IDs with the same number.
            let panes: BTreeMap<_, _> = panes
                .iter()
                .map(|(pane_id, contents)| (pane_id.to_string(), contents))
                .collect();
            serde_json::to_value(panes).map(|panes| json!({ "PaneRenderReport": panes }))
        },
        _ => serde_json::to_value(event),
    }
    .context("serializing plugin event")
}

#[cfg(test)]
#[path = "unit/event.rs"]
mod tests;
