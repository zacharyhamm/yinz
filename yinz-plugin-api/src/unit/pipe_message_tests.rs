use super::*;
use std::collections::BTreeMap;

/// Everything a pipe message carries survives the trip to a plugin and back, the pane it
/// was sent from included.
#[test]
fn a_pipe_message_round_trips_with_its_pane() {
    for pane_id in [
        None,
        Some(PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(
            7,
        ))),
    ] {
        let mut message = PipeMessage::new(
            PipeSource::Cli("pipe-1".to_owned()),
            "agent_hook",
            &Some("{}".to_owned()),
            &Some(BTreeMap::from([("k".to_owned(), "v".to_owned())])),
            false,
        );
        message.pane_id = pane_id;
        let protobuf = ProtobufPipeMessage::try_from(message.clone()).unwrap();
        assert_eq!(PipeMessage::try_from(protobuf).unwrap(), message);
    }
    // The wire has a slot for a terminal only: a plugin pane cannot run `yinz pipe`.
    let mut from_plugin_pane = PipeMessage::new(PipeSource::Keybind, "x", &None, &None, true);
    from_plugin_pane.pane_id = Some(PaneId::Plugin(yinz_types::internal_ids::PluginId::new(2)));
    let protobuf = ProtobufPipeMessage::try_from(from_plugin_pane).unwrap();
    assert_eq!(PipeMessage::try_from(protobuf).unwrap().pane_id, None);
}
