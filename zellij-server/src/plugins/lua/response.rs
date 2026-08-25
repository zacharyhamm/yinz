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

use mlua::{Lua, Value as LuaValue};
use prost::Message;
use serde::Serialize;

use zellij_utils::data::{CommandType, FileMetadata, PaneId, PluginIds, QueueStatusResponse};
use zellij_utils::errors::prelude::*;
use zellij_utils::plugin_api::plugin_command::{
    dump_layout_response, enqueue_queue_job_response, fs_list_response, fs_metadata_response,
    fs_mutate_response, fs_read_response, get_focused_pane_info_response, get_pane_cwd_response,
    get_pane_pid_response, get_pane_running_command_response, new_tab_response,
    ProtobufCurrentSessionLastSavedTimeResponse, ProtobufDumpLayoutResponse,
    ProtobufEnqueueQueueJobResponse, ProtobufFsListResponse, ProtobufFsMetadataResponse,
    ProtobufFsMutateResponse, ProtobufFsReadResponse, ProtobufGenerateRandomNameResponse,
    ProtobufGetFocusedPaneInfoResponse, ProtobufGetLayoutDirResponse, ProtobufGetPaneCwdResponse,
    ProtobufGetPaneInfoResponse, ProtobufGetPanePidResponse, ProtobufGetPaneRunningCommandResponse,
    ProtobufGetPluginAliasConfigurationResponse, ProtobufGetSessionEnvironmentVariablesResponse,
    ProtobufGetTabInfoResponse, ProtobufGetUserPluginDirResponse, ProtobufNewTabResponse,
    ProtobufOpenCommandPaneResponse, ProtobufOpenFileResponse, ProtobufOpenTerminalResponse,
    ProtobufPaneId, ProtobufQueueStatusResponse,
};
use zellij_utils::plugin_api::plugin_ids::{ProtobufPluginIds, ProtobufZellijVersion};

/// Decode the reply `bytes` a `command` produced. `None` means "no table for this one" - the
/// caller hands the bytes back untouched.
pub fn response_to_lua(lua: &Lua, command: CommandType, bytes: &[u8]) -> Option<Result<LuaValue>> {
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
        CommandType::GetUserPluginDir => decode(bytes)
            .and_then(|r: ProtobufGetUserPluginDirResponse| value(lua, r.user_plugin_dir)),
        CommandType::GetLayoutDir => {
            decode(bytes).and_then(|r: ProtobufGetLayoutDirResponse| value(lua, r.layout_dir))
        },
        CommandType::GetSessionEnvironmentVariables => {
            decode(bytes).and_then(|r: ProtobufGetSessionEnvironmentVariablesResponse| {
                let vars: std::collections::BTreeMap<String, String> = r
                    .env_vars
                    .into_iter()
                    .map(|env_var| (env_var.name, env_var.value))
                    .collect();
                value(lua, vars)
            })
        },
        CommandType::GetPluginAliasConfiguration => {
            decode(bytes).and_then(|r: ProtobufGetPluginAliasConfigurationResponse| {
                let configuration: std::collections::BTreeMap<String, String> =
                    r.configuration.into_iter().collect();
                value(lua, configuration)
            })
        },
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
        CommandType::GetPaneInfo => {
            decode(bytes).and_then(|r: ProtobufGetPaneInfoResponse| match r.pane_info {
                Some(info) => value(
                    lua,
                    zellij_utils::data::PaneInfo::try_from(info).map_err(|e| anyhow!("{}", e))?,
                ),
                None => Ok(LuaValue::Nil),
            })
        },
        CommandType::GetTabInfo => {
            decode(bytes).and_then(|r: ProtobufGetTabInfoResponse| match r.tab_info {
                Some(info) => value(
                    lua,
                    zellij_utils::data::TabInfo::try_from(info).map_err(|e| anyhow!("{}", e))?,
                ),
                None => Ok(LuaValue::Nil),
            })
        },
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
        CommandType::CurrentSessionLastSavedTime => {
            decode(bytes).and_then(|r: ProtobufCurrentSessionLastSavedTimeResponse| {
                value(lua, r.timestamp_millis)
            })
        },
        CommandType::DumpLayout => {
            decode(bytes).and_then(|r: ProtobufDumpLayoutResponse| match r.result {
                Some(dump_layout_response::Result::LayoutContent(layout)) => value(lua, layout),
                Some(dump_layout_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("zellij answered with nothing")),
            })
        },
        CommandType::EnqueueQueueJob => {
            decode(bytes).and_then(|r: ProtobufEnqueueQueueJobResponse| match r.result {
                Some(enqueue_queue_job_response::Result::JobId(job_id)) => value(lua, job_id),
                Some(enqueue_queue_job_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("zellij answered with nothing")),
            })
        },
        CommandType::QueueStatus => decode(bytes).and_then(|r: ProtobufQueueStatusResponse| {
            let queues = QueueStatusResponse::try_from(r).map_err(|e| anyhow!("{}", e))?;
            value(lua, queues)
        }),
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
        CommandType::FsRead => decode(bytes).and_then(|r: ProtobufFsReadResponse| match r.result {
            Some(fs_read_response::Result::Contents(contents)) => lua
                .create_string(&contents)
                .map(LuaValue::String)
                .map_err(|e| anyhow!("{}", e)),
            Some(fs_read_response::Result::Error(e)) => Err(anyhow!("{}", e)),
            None => Err(anyhow!("zellij answered with nothing")),
        }),
        CommandType::FsList => decode(bytes).and_then(|r: ProtobufFsListResponse| match r.result {
            Some(fs_list_response::Result::Entries(list)) => {
                let entries: Vec<FsEntry> = list
                    .entries
                    .into_iter()
                    .map(|entry| FsEntry {
                        path: entry.path,
                        // `metadata_is_set` is false when the entry could not be stat'ed, which
                        // is a real answer ("this exists, I could not read it") and not an error.
                        metadata: entry
                            .metadata
                            .as_ref()
                            .and_then(Option::<FileMetadata>::from),
                    })
                    .collect();
                value(lua, entries)
            },
            Some(fs_list_response::Result::Error(e)) => Err(anyhow!("{}", e)),
            None => Err(anyhow!("zellij answered with nothing")),
        }),
        CommandType::FsMetadata => {
            decode(bytes).and_then(|r: ProtobufFsMetadataResponse| match r.result {
                Some(fs_metadata_response::Result::Metadata(metadata)) => {
                    match Option::<FileMetadata>::from(&metadata) {
                        Some(metadata) => value(lua, metadata),
                        None => Ok(LuaValue::Nil),
                    }
                },
                Some(fs_metadata_response::Result::NotFound(_)) => Ok(LuaValue::Nil),
                Some(fs_metadata_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("zellij answered with nothing")),
            })
        },
        CommandType::FsWrite
        | CommandType::FsRemove
        | CommandType::FsCreateDir
        | CommandType::FsRename => {
            decode(bytes).and_then(|r: ProtobufFsMutateResponse| match r.result {
                Some(fs_mutate_response::Result::Success(_)) => Ok(LuaValue::Boolean(true)),
                Some(fs_mutate_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("zellij answered with nothing")),
            })
        },
        _ => return None,
    })
}

/// One entry of an `FsList` answer. Named fields rather than a pair, so that a plugin writes
/// `entry.path` instead of `entry[1]`.
#[derive(Serialize)]
struct FsEntry {
    path: String,
    metadata: Option<FileMetadata>,
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
    super::to_lua_value(lua, &from)
        .map_err(|e| anyhow!("zellij's answer could not be given to Lua: {}", e))
}
