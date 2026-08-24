//! Turning a command's reply into a Lua value.
//!
//! A host command that answers writes its reply with `wasi_write_object`, which serializes the
//! *protobuf* encoding of the response - the wasm guest decodes it on the other side of its stdin
//! pipe, in `zellij-tile/src/shim.rs`. A Lua plugin has no guest to do that, so the decoding
//! happens here instead, and the shim is the reference for what each response means.
//!
//! Unlike the command direction, this cannot be one `from_value` call: the protobuf response types
//! are prost-generated and have no serde representation, and each one names its payload
//! differently. So this is a table, and it covers the commands whose *purpose* is to answer -
//! the `get_`/`list_`/`dump_` family, and the pane-opening commands, whose reply is the id of the
//! pane they just made and so is what lets one command feed the next.
//!
//! Anything not listed still returns its reply, as the raw bytes it always did. That is the honest
//! answer for a response this does not yet know how to read, and it is what
//! `docs/LUA_PLUGINS.md` says.

use mlua::{Lua, LuaSerdeExt, Value as LuaValue};
use prost::Message;
use serde::Serialize;

use zellij_utils::data::{CommandType, PaneId, PluginIds};
use zellij_utils::errors::prelude::*;
use zellij_utils::plugin_api::plugin_command::{
    dump_layout_response, get_focused_pane_info_response, get_pane_cwd_response,
    get_pane_pid_response, get_pane_running_command_response, new_tab_response,
    ProtobufCurrentSessionLastSavedTimeResponse, ProtobufDumpLayoutResponse,
    ProtobufGenerateRandomNameResponse, ProtobufGetFocusedPaneInfoResponse,
    ProtobufGetLayoutDirResponse, ProtobufGetPaneCwdResponse, ProtobufGetPaneInfoResponse,
    ProtobufGetPanePidResponse, ProtobufGetPaneRunningCommandResponse,
    ProtobufGetPluginAliasConfigurationResponse, ProtobufGetSessionEnvironmentVariablesResponse,
    ProtobufGetTabInfoResponse, ProtobufNewTabResponse, ProtobufOpenCommandPaneResponse,
    ProtobufOpenFileResponse, ProtobufOpenTerminalResponse, ProtobufPaneId,
};
use zellij_utils::plugin_api::plugin_ids::{ProtobufPluginIds, ProtobufZellijVersion};

/// Decode the reply `bytes` a `command` produced. `None` means "no table for this one" - the
/// caller hands the bytes back untouched.
pub fn response_to_lua(
    lua: &Lua,
    command: CommandType,
    bytes: &[u8],
) -> Option<Result<LuaValue>> {
    Some(match command {
        CommandType::GetPluginIds => decode(bytes).and_then(|response: ProtobufPluginIds| {
            let ids = PluginIds::try_from(response).map_err(|e| anyhow!("{}", e))?;
            value(lua, ids)
        }),
        CommandType::GetZellijVersion => {
            decode(bytes).and_then(|r: ProtobufZellijVersion| value(lua, r.version))
        },
        CommandType::GenerateRandomName => {
            decode(bytes).and_then(|r: ProtobufGenerateRandomNameResponse| value(lua, r.name))
        },
        CommandType::GetLayoutDir => {
            decode(bytes).and_then(|r: ProtobufGetLayoutDirResponse| value(lua, r.layout_dir))
        },
        CommandType::GetSessionEnvironmentVariables => decode(bytes).and_then(
            |r: ProtobufGetSessionEnvironmentVariablesResponse| {
                let vars: std::collections::BTreeMap<String, String> = r
                    .env_vars
                    .into_iter()
                    .map(|env_var| (env_var.name, env_var.value))
                    .collect();
                value(lua, vars)
            },
        ),
        CommandType::GetPluginAliasConfiguration => decode(bytes).and_then(
            |r: ProtobufGetPluginAliasConfigurationResponse| {
                let configuration: std::collections::BTreeMap<String, String> =
                    r.configuration.into_iter().collect();
                value(lua, configuration)
            },
        ),
        CommandType::GetFocusedPaneInfo => {
            decode(bytes).and_then(|r: ProtobufGetFocusedPaneInfoResponse| match r.result {
                Some(get_focused_pane_info_response::Result::FocusedPaneInfo(info)) => {
                    let pane_id = info
                        .focused_pane_id
                        .and_then(|id| PaneId::try_from(id).ok())
                        .ok_or_else(|| anyhow!("the focused pane has no id"))?;
                    value(
                        lua,
                        FocusedPane {
                            tab_index: info.focused_tab_index as usize,
                            pane_id,
                        },
                    )
                },
                Some(get_focused_pane_info_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("zellij answered with nothing")),
            })
        },
        CommandType::GetPaneInfo => decode(bytes).and_then(|r: ProtobufGetPaneInfoResponse| {
            match r.pane_info {
                Some(info) => value(
                    lua,
                    zellij_utils::data::PaneInfo::try_from(info).map_err(|e| anyhow!("{}", e))?,
                ),
                None => Ok(LuaValue::Nil),
            }
        }),
        CommandType::GetTabInfo => decode(bytes).and_then(|r: ProtobufGetTabInfoResponse| {
            match r.tab_info {
                Some(info) => value(
                    lua,
                    zellij_utils::data::TabInfo::try_from(info).map_err(|e| anyhow!("{}", e))?,
                ),
                None => Ok(LuaValue::Nil),
            }
        }),
        CommandType::GetPaneCwd => {
            decode(bytes).and_then(|r: ProtobufGetPaneCwdResponse| match r.result {
                Some(get_pane_cwd_response::Result::Cwd(cwd)) => value(lua, cwd),
                Some(get_pane_cwd_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Ok(LuaValue::Nil),
            })
        },
        CommandType::GetPanePid => {
            decode(bytes).and_then(|r: ProtobufGetPanePidResponse| match r.result {
                Some(get_pane_pid_response::Result::Pid(pid)) => value(lua, pid),
                Some(get_pane_pid_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Ok(LuaValue::Nil),
            })
        },
        CommandType::GetPaneRunningCommand => {
            decode(bytes).and_then(|r: ProtobufGetPaneRunningCommandResponse| match r.result {
                Some(get_pane_running_command_response::Result::Command(command)) => {
                    value(lua, command.args)
                },
                Some(get_pane_running_command_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Ok(LuaValue::Nil),
            })
        },
        CommandType::CurrentSessionLastSavedTime => decode(bytes)
            .and_then(|r: ProtobufCurrentSessionLastSavedTimeResponse| {
                value(lua, r.timestamp_millis)
            }),
        CommandType::DumpLayout => {
            decode(bytes).and_then(|r: ProtobufDumpLayoutResponse| match r.result {
                Some(dump_layout_response::Result::LayoutContent(layout)) => value(lua, layout),
                Some(dump_layout_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("zellij answered with nothing")),
            })
        },
        CommandType::NewTab | CommandType::NewTabUnfocused => {
            decode(bytes).and_then(|r: ProtobufNewTabResponse| match r.result {
                Some(new_tab_response::Result::TabId(tab_id)) => value(lua, tab_id),
                _ => Ok(LuaValue::Nil),
            })
        },
        CommandType::OpenFile | CommandType::OpenFileFloating | CommandType::OpenFileInPlace => {
            decode(bytes).and_then(|r: ProtobufOpenFileResponse| pane_id(lua, r.pane_id))
        },
        CommandType::OpenTerminal
        | CommandType::OpenTerminalFloating
        | CommandType::OpenTerminalInPlace => {
            decode(bytes).and_then(|r: ProtobufOpenTerminalResponse| pane_id(lua, r.pane_id))
        },
        CommandType::OpenCommandPane
        | CommandType::OpenCommandPaneFloating
        | CommandType::OpenCommandPaneInPlace
        | CommandType::OpenCommandPaneBackground => {
            decode(bytes).and_then(|r: ProtobufOpenCommandPaneResponse| pane_id(lua, r.pane_id))
        },
        _ => return None,
    })
}

/// The two halves of `GetFocusedPaneInfo`, which protobuf keeps apart and a plugin wants together.
#[derive(Serialize)]
struct FocusedPane {
    tab_index: usize,
    pane_id: PaneId,
}

fn decode<T: Message + Default>(bytes: &[u8]) -> Result<T> {
    T::decode(bytes).map_err(|e| anyhow!("zellij's answer could not be read: {}", e))
}

fn pane_id(lua: &Lua, id: Option<ProtobufPaneId>) -> Result<LuaValue> {
    match id.map(PaneId::try_from) {
        Some(Ok(pane_id)) => value(lua, pane_id),
        Some(Err(e)) => Err(anyhow!("{}", e)),
        None => Ok(LuaValue::Nil),
    }
}

fn value<T: Serialize>(lua: &Lua, from: T) -> Result<LuaValue> {
    lua.to_value(&from)
        .map_err(|e| anyhow!("zellij's answer could not be given to Lua: {}", e))
}
