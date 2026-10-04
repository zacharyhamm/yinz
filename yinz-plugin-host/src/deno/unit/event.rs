use super::*;
use yinz_types::data::{PaneContents, PaneId};
use yinz_types::internal_ids::{PluginId, TerminalId};

#[test]
fn client_grid_reset_carries_the_client_id() {
    let event = Event::ClientGridReset(yinz_types::data::ClientId::new(42));
    assert_eq!(
        event_to_json(&event).unwrap(),
        json!({"ClientGridReset": 42})
    );
}

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
