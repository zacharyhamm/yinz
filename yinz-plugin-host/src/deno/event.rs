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
mod tests {
    use super::*;
    use yinz_types::data::{PaneContents, PaneId};
    use yinz_types::internal_ids::{PluginId, TerminalId};

    #[test]
    fn render_report_uses_cli_pane_keys_and_preserves_contents_and_event_tag() {
        let terminal = PaneContents {
            viewport: vec!["terminal output".into()],
            ..Default::default()
        };
        let plugin = PaneContents {
            viewport: vec!["plugin output".into()],
            ..Default::default()
        };
        let event = Event::PaneRenderReport(
            [
                (PaneId::Terminal(TerminalId::new(7)), terminal.clone()),
                (PaneId::Plugin(PluginId::new(7)), plugin.clone()),
            ]
            .into_iter()
            .collect(),
        );
        assert_eq!(
            event_to_json(&event).unwrap(),
            json!({ "PaneRenderReport": {
                "terminal_7": terminal,
                "plugin_7": plugin,
            } })
        );
        assert_eq!(
            event_to_json(&Event::PaneRenderReport(Default::default())).unwrap(),
            json!({ "PaneRenderReport": {} })
        );
        for event in [
            Event::BeforeClose,
            Event::CommandPaneExited(TerminalId::new(7), None, Default::default()),
        ] {
            assert_eq!(
                event_to_json(&event).unwrap(),
                serde_json::to_value(event).unwrap()
            );
        }
    }
}
