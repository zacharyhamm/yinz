//! JSON event encoding without runtime-specific values.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use yinz_runtime::errors::prelude::*;
use yinz_types::data::Event;

pub(super) fn event_to_json(event: &Event) -> Result<Value> {
    match event {
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
    .context("serializing Deno event")
}

#[cfg(test)]
#[path = "unit/event.rs"]
mod tests;
