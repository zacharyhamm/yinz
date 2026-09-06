pub use super::generated_api::api::pipe_message::{
    Arg as ProtobufArg, PipeMessage as ProtobufPipeMessage, PipeSource as ProtobufPipeSource,
};
use {zellij_types::data::PaneId, zellij_types::data::PipeMessage, zellij_types::data::PipeSource};

use std::convert::TryFrom;

impl TryFrom<ProtobufPipeMessage> for PipeMessage {
    type Error = &'static str;
    fn try_from(protobuf_pipe_message: ProtobufPipeMessage) -> Result<Self, &'static str> {
        let source = match (
            ProtobufPipeSource::try_from(protobuf_pipe_message.source).ok(),
            protobuf_pipe_message.cli_source_id,
            protobuf_pipe_message.plugin_source_id,
        ) {
            (Some(ProtobufPipeSource::Cli), Some(cli_source_id), _) => {
                PipeSource::Cli(cli_source_id)
            },
            (Some(ProtobufPipeSource::Plugin), _, Some(plugin_source_id)) => {
                PipeSource::Plugin(zellij_types::internal_ids::PluginId::new(plugin_source_id))
            },
            (Some(ProtobufPipeSource::Keybind), _, _) => PipeSource::Keybind,
            _ => return Err("Invalid PipeSource or payload"),
        };
        let name = protobuf_pipe_message.name;
        let payload = protobuf_pipe_message.payload;
        let args = protobuf_pipe_message
            .args
            .into_iter()
            .map(|arg| (arg.key, arg.value))
            .collect();
        let is_private = protobuf_pipe_message.is_private;
        Ok(PipeMessage {
            source,
            name,
            payload,
            args,
            is_private,
            pane_id: protobuf_pipe_message
                .source_terminal_pane_id
                .map(|id| PaneId::Terminal(zellij_types::internal_ids::TerminalId::new(id))),
        })
    }
}

impl TryFrom<PipeMessage> for ProtobufPipeMessage {
    type Error = &'static str;
    fn try_from(pipe_message: PipeMessage) -> Result<Self, &'static str> {
        let (source, cli_source_id, plugin_source_id) = match pipe_message.source {
            PipeSource::Cli(input_pipe_id) => {
                (ProtobufPipeSource::Cli as i32, Some(input_pipe_id), None)
            },
            PipeSource::Plugin(plugin_id) => {
                (ProtobufPipeSource::Plugin as i32, None, Some(plugin_id))
            },
            PipeSource::Keybind => (ProtobufPipeSource::Keybind as i32, None, None),
        };
        let name = pipe_message.name;
        let payload = pipe_message.payload;
        let args: Vec<_> = pipe_message
            .args
            .into_iter()
            .map(|(key, value)| ProtobufArg { key, value })
            .collect();
        let is_private = pipe_message.is_private;
        // A CLI pipe runs in a terminal; a plugin pane has no shell for it to run in.
        let source_terminal_pane_id = match pipe_message.pane_id {
            Some(PaneId::Terminal(id)) => Some(id),
            _ => None,
        };
        Ok(ProtobufPipeMessage {
            source,
            cli_source_id,
            plugin_source_id: plugin_source_id.map(|id| id.get()),
            name,
            payload,
            args,
            is_private,
            source_terminal_pane_id: source_terminal_pane_id.map(|id| id.get()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Everything a pipe message carries survives the trip to a plugin and back, the pane it
    /// was sent from included.
    #[test]
    fn a_pipe_message_round_trips_with_its_pane() {
        for pane_id in [
            None,
            Some(PaneId::Terminal(
                zellij_types::internal_ids::TerminalId::new(7),
            )),
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
        // The wire has a slot for a terminal only: a plugin pane cannot run `zellij pipe`.
        let mut from_plugin_pane = PipeMessage::new(PipeSource::Keybind, "x", &None, &None, true);
        from_plugin_pane.pane_id =
            Some(PaneId::Plugin(zellij_types::internal_ids::PluginId::new(2)));
        let protobuf = ProtobufPipeMessage::try_from(from_plugin_pane).unwrap();
        assert_eq!(PipeMessage::try_from(protobuf).unwrap().pane_id, None);
    }
}
