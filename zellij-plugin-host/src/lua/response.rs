//! Convert shared host replies to Lua values.

use mlua::{serde::SerializeOptions, Lua, LuaSerdeExt, Value as LuaValue};
use zellij_runtime::errors::prelude::*;
use zellij_types::data::{CommandType, PaneInfo, SessionListSnapshot};

#[allow(unused_imports)]
pub(crate) use crate::response::DATA_CONFLICT_MARKER;
use crate::response::{decode_response, DecodedResponse};

/// Decode a reply, retaining `None` for commands with no known response format.
pub fn response_to_lua(lua: &Lua, command: CommandType, bytes: &[u8]) -> Option<Result<LuaValue>> {
    decode_response(command, bytes).map(|response| {
        match response? {
            DecodedResponse::Json(value) => {
                // JSON object keys are strings. Restore typed numeric map keys for Lua,
                // including the pane manifests and histories nested in a session snapshot.
                match command {
                    CommandType::GetSessionList => super::to_lua_value(
                        lua,
                        &serde_json::from_value::<SessionListSnapshot>(value)?,
                    ),
                    CommandType::GetPaneInfo => super::to_lua_value(
                        lua,
                        &serde_json::from_value::<Option<PaneInfo>>(value)?,
                    ),
                    _ => lua
                        .to_value_with(&value, SerializeOptions::new().serialize_unit_to_null(true))
                        .and_then(|value| super::nulls_to_nil(lua, value))
                        .map_err(super::lua_err),
                }
                .map_err(|e| anyhow!("zellij's answer could not be given to Lua: {}", e))
            },
            DecodedResponse::Bytes(bytes) => lua
                .create_string(&bytes)
                .map(LuaValue::String)
                .map_err(|e| anyhow!("{}", e)),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;
    use zellij_plugin_api::plugin_command::{fs_read_response, ProtobufFsReadResponse};

    #[test]
    fn pane_reply_keeps_numeric_client_keys() {
        use zellij_plugin_api::plugin_command::ProtobufGetPaneInfoResponse;
        use zellij_types::internal_ids::ClientId;

        let pane = PaneInfo {
            index_in_pane_group: [(ClientId::new(7), 2)].into_iter().collect(),
            ..Default::default()
        };
        let reply = ProtobufGetPaneInfoResponse {
            pane_info: Some(pane.try_into().unwrap()),
        }
        .encode_to_vec();
        let lua = Lua::new();
        let value = response_to_lua(&lua, CommandType::GetPaneInfo, &reply)
            .unwrap()
            .unwrap();
        lua.globals().set("reply", value).unwrap();
        lua.load("assert(reply.index_in_pane_group[7] == 2 and reply.exit_status == nil)")
            .exec()
            .unwrap();
    }

    #[test]
    fn session_reply_preserves_all_numeric_maps_and_string_configuration_keys() {
        use zellij_plugin_api::plugin_command::ProtobufGetSessionListResponse;
        use zellij_types::data::{
            GetSessionListResponse, PaneId, PaneManifest, PluginInfo, SessionInfo,
        };
        use zellij_types::internal_ids::{ClientId, PluginId, TabId, TabPosition, TerminalId};

        let client = ClientId::new(7);
        let pane_id = PaneId::Terminal(TerminalId::new(42));
        let snapshot = SessionListSnapshot {
            live_sessions: vec![SessionInfo {
                panes: PaneManifest {
                    panes: [(
                        TabPosition::new(0),
                        vec![PaneInfo {
                            id: pane_id,
                            index_in_pane_group: [(client, 2)].into_iter().collect(),
                            ..Default::default()
                        }],
                    )]
                    .into_iter()
                    .collect(),
                },
                plugins: [(
                    PluginId::new(3),
                    PluginInfo {
                        location: "zellij:hocus".into(),
                        configuration: [("7".into(), "string key".into())].into_iter().collect(),
                    },
                )]
                .into_iter()
                .collect(),
                tab_history: [(client, vec![TabId::new(9)])].into_iter().collect(),
                pane_history: [(client, vec![pane_id])].into_iter().collect(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let reply = ProtobufGetSessionListResponse::from(GetSessionListResponse::Ok(snapshot))
            .encode_to_vec();
        let lua = Lua::new();
        let value = response_to_lua(&lua, CommandType::GetSessionList, &reply)
            .unwrap()
            .unwrap();
        lua.globals().set("reply", value).unwrap();
        lua.load(
            r#"
            local s = reply.live_sessions[1]
            assert(s.plugins[3].location == "zellij:hocus" and s.plugins["3"] == nil)
            assert(s.plugins[3].configuration["7"] == "string key")
            assert(s.plugins[3].configuration[7] == nil)
            assert(s.tab_history[7][1] == 9 and s.tab_history["7"] == nil)
            assert(s.pane_history[7][1].Terminal == 42 and s.pane_history["7"] == nil)
            assert(s.panes.panes["0"] == nil)
            local pane = s.panes.panes[0][1]
            assert(pane.id == 42 and pane.is_plugin == false)
            assert(pane.index_in_pane_group[7] == 2 and pane.index_in_pane_group["7"] == nil)
            assert(pane.exit_status == nil)
        "#,
        )
        .exec()
        .unwrap();
    }

    #[test]
    fn tab_reply_preserves_numeric_values_array_indices_and_missing_fields() {
        use zellij_plugin_api::plugin_command::ProtobufGetTabInfoResponse;
        use zellij_types::data::TabInfo;
        use zellij_types::internal_ids::{ClientId, TabId, TabPosition};

        let reply = ProtobufGetTabInfoResponse {
            tab_info: Some(
                TabInfo {
                    tab_id: TabId::new(9),
                    position: TabPosition::new(0),
                    other_focused_clients: vec![ClientId::new(7), ClientId::new(11)],
                    ..Default::default()
                }
                .try_into()
                .unwrap(),
            ),
        }
        .encode_to_vec();
        let lua = Lua::new();
        let value = response_to_lua(&lua, CommandType::GetTabInfo, &reply)
            .unwrap()
            .unwrap();
        lua.globals().set("reply", value).unwrap();
        lua.load(
            r#"
            assert(reply.tab_id == 9 and reply.position == 0)
            assert(reply.other_focused_clients[1] == 7 and reply.other_focused_clients[2] == 11)
            assert(reply.other_focused_clients["1"] == nil)
            assert(reply.active_swap_layout_name == nil)
        "#,
        )
        .exec()
        .unwrap();
    }

    #[test]
    fn data_reply_preserves_large_signed_integers_and_tagged_values() {
        use zellij_plugin_api::plugin_command::ProtobufDataFetchResponse;
        use zellij_types::data::{DataFetchResponse, DataRows, DataValue};

        let reply = ProtobufDataFetchResponse::from(DataFetchResponse::Ok(DataRows {
            columns: vec![
                "min".into(),
                "max".into(),
                "real".into(),
                "null".into(),
                "blob".into(),
            ],
            rows: vec![vec![
                DataValue::Int(i64::MIN),
                DataValue::Int(i64::MAX),
                DataValue::Real(1.5),
                DataValue::Null,
                DataValue::Blob(vec![0, 255]),
            ]],
            ..Default::default()
        }))
        .encode_to_vec();
        let lua = Lua::new();
        let value = response_to_lua(&lua, CommandType::DataFetch, &reply)
            .unwrap()
            .unwrap();
        lua.globals().set("reply", value).unwrap();
        lua.load(
            r#"
            local row = reply.rows[1]
            assert(row[1].Int == math.mininteger and row[2].Int == math.maxinteger)
            assert(math.type(row[1].Int) == "integer" and math.type(row[2].Int) == "integer")
            assert(row[3].Real == 1.5 and row[4] == "Null")
            assert(row[5].Blob[1] == 0 and row[5].Blob[2] == 255)
            assert(reply.next_cursor == nil)
        "#,
        )
        .exec()
        .unwrap();
    }

    #[test]
    fn file_bytes_and_json_nulls_keep_their_lua_representation() {
        let lua = Lua::new();
        let contents = vec![0, 255, 128, b'a'];
        let reply = ProtobufFsReadResponse {
            result: Some(fs_read_response::Result::Contents(contents.clone())),
        }
        .encode_to_vec();
        assert_eq!(
            decode_response(CommandType::FsRead, &reply)
                .unwrap()
                .unwrap(),
            DecodedResponse::Bytes(contents.clone())
        );
        let LuaValue::String(value) = response_to_lua(&lua, CommandType::FsRead, &reply)
            .unwrap()
            .unwrap()
        else {
            panic!("file contents must be a binary Lua string");
        };
        assert_eq!(value.as_bytes().as_ref(), contents.as_slice());

        let snapshot = serde_json::json!({"missing": null, "items": [1, null, 3]});
        let reply = ProtobufFsReadResponse {
            result: Some(fs_read_response::Result::Contents(
                serde_json::to_vec(&snapshot).unwrap(),
            )),
        }
        .encode_to_vec();
        assert_eq!(
            decode_response(CommandType::ProjectConfigRead, &reply)
                .unwrap()
                .unwrap(),
            DecodedResponse::Json(snapshot)
        );
        let value = response_to_lua(&lua, CommandType::ProjectConfigRead, &reply)
            .unwrap()
            .unwrap();
        lua.globals().set("reply", value).unwrap();
        lua.load("assert(reply.missing == nil and reply.items[1] == 1 and reply.items[2] == nil and reply.items[3] == 3)")
            .exec()
            .unwrap();
        assert!(matches!(
            response_to_lua(&lua, CommandType::GetPaneCwd, &[])
                .unwrap()
                .unwrap(),
            LuaValue::Nil
        ));
    }
}
