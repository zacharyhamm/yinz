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

#[test]
fn binary_events_use_compact_fields_without_changing_metadata() {
    let bytes = vec![0, 128, 255, b'x'];
    let context = BTreeMap::from([("request".into(), "original".into())]);
    let cases = [
        (
            Event::RunCommandResult(Some(7), bytes.clone(), bytes.clone(), context.clone()),
            vec!["/RunCommandResult/1", "/RunCommandResult/2"],
        ),
        (
            Event::WebRequestResult(
                200,
                BTreeMap::from([("header".into(), "value".into())]),
                bytes.clone(),
                context.clone(),
            ),
            vec!["/WebRequestResult/2"],
        ),
        (
            Event::PaneOutput {
                pane_id: 12,
                data: bytes.clone(),
                context: context.clone(),
            },
            vec!["/PaneOutput/data"],
        ),
        (
            Event::RunCommandOutput {
                data: bytes.clone(),
                stderr: true,
                context,
            },
            vec!["/RunCommandOutput/data"],
        ),
        (
            Event::WebRequestStreamChunk {
                stream_id: "stream".into(),
                chunk: bytes.clone(),
                done: true,
                error: Some("cancelled".into()),
            },
            vec!["/WebRequestStreamChunk/chunk"],
        ),
    ];
    for (event, positions) in cases {
        let mut encoded = event_to_json(&event).unwrap();
        for position in positions {
            let field = encoded.pointer_mut(position).unwrap();
            assert!(
                field.is_object(),
                "{position} was expanded into a JSON array"
            );
            assert_eq!(yinz_steel::binary::decode(field).unwrap(), bytes);
            *field = json!(bytes);
        }
        assert_eq!(encoded, serde_json::to_value(event).unwrap());
    }
}
