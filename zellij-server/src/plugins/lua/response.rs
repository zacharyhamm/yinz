//! Turning a command's reply into a Lua value.
//!
//! A host command that answers writes its reply with `wasi_write_object`, which serializes the
//! *protobuf* encoding of the response - the wasm guest decodes it on the other side of its stdin
//! pipe, in `zellij-tile/src/shim.rs`. A Lua plugin has no guest to do that, so the decoding
//! happens here instead, and the shim is the reference for what each response means.
//!
//! Unlike the command direction, this cannot be one `from_value` call: the protobuf response types
//! are prost-generated and have no serde representation, and each one names its payload
//! differently. So this is a table, and it is complete: every command whose reply the shim reads
//! has an arm here. The shape of each arm follows the shim's - a `oneof { ok | error }` becomes
//! "a Lua value, or raise", an `Option` that means "nothing to report" becomes `nil`, and a
//! reply that has a `zellij_utils::data` type goes through the same conversion the shim uses, so
//! that a plugin sees the same fields in a reply as it does in an event.
//!
//! The `_ => return None` at the end is for commands that do not answer at all; a reply arriving
//! for one of those is handed back as the raw bytes, which is the honest answer for something
//! this does not know how to read. `every_reply_the_shim_reads_is_decoded` in `unit_tests.rs`
//! pins the list of commands that must not fall through, so a new answering command added to
//! the shim without an arm here fails a test rather than silently regressing to bytes.

use mlua::{Lua, Value as LuaValue};
use prost::Message;
use serde::Serialize;

use zellij_utils::data::{
    CommandType, DataApplyResponse, DataCatalogResponse, DataCountResponse, DataDefineResponse,
    DataFetchResponse, DataUnwatchResponse, DataWatchResponse, FileMetadata,
    GetSessionListResponse, LayoutMetadata, LayoutParsingError, ListOrphanedPanesResponse,
    NotificationInfo, PaneGitProbe, PaneId, PaneScrollbackResponse, PluginIds, QueueStatusResponse,
    RecordingSummary, SearchHit, WarpPaneResponse,
};
use zellij_utils::errors::prelude::*;
use zellij_utils::plugin_api::event::ProtobufPaneScrollbackResponse;
use zellij_utils::plugin_api::plugin_command::{
    break_panes_to_new_tab_response, break_panes_to_tab_with_id_response,
    break_panes_to_tab_with_index_response, delete_layout_response, dump_layout_response,
    dump_session_layout_response, edit_layout_response, enqueue_queue_job_response,
    focus_or_create_tab_response, fs_list_response, fs_metadata_response, fs_mutate_response,
    fs_read_response, get_focused_pane_info_response, get_pane_cwd_response,
    get_pane_git_probe_response, get_pane_pid_response, get_pane_running_command_response,
    hide_floating_panes_response, new_tab_response, open_command_pane_in_git_checkout_response,
    parse_layout_response, read_plugin_log_response, rename_layout_response, save_layout_response,
    save_session_response, show_floating_panes_response, spawn_agent_response,
    ProtobufBreakPanesToNewTabResponse, ProtobufBreakPanesToTabWithIdResponse,
    ProtobufBreakPanesToTabWithIndexResponse, ProtobufCheckLuaPluginResponse,
    ProtobufCurrentSessionLastSavedTimeResponse, ProtobufDataApplyResponse,
    ProtobufDataCatalogResponse, ProtobufDataCountResponse, ProtobufDataDefineResponse,
    ProtobufDataFetchResponse, ProtobufDataUnwatchResponse, ProtobufDataWatchResponse,
    ProtobufDeleteAllDeadSessionsResponse, ProtobufDeleteDeadSessionResponse,
    ProtobufDeleteLayoutResponse, ProtobufDumpLayoutResponse, ProtobufDumpSessionLayoutResponse,
    ProtobufEditLayoutResponse, ProtobufEnqueueQueueJobResponse, ProtobufFocusOrCreateTabResponse,
    ProtobufFsListResponse, ProtobufFsMetadataResponse, ProtobufFsMutateResponse,
    ProtobufFsReadResponse, ProtobufGenerateRandomNameResponse, ProtobufGetFocusedPaneInfoResponse,
    ProtobufGetLayoutDirResponse, ProtobufGetLocalUtcOffsetResponse, ProtobufGetPaneCwdResponse,
    ProtobufGetPaneGitProbeResponse, ProtobufGetPaneInfoResponse, ProtobufGetPanePidResponse,
    ProtobufGetPaneRunningCommandResponse, ProtobufGetPluginAliasConfigurationResponse,
    ProtobufGetSessionEnvironmentVariablesResponse, ProtobufGetSessionListResponse,
    ProtobufGetTabInfoResponse, ProtobufGetUserPluginDirResponse,
    ProtobufHideFloatingPanesResponse, ProtobufKillSessionsResponse,
    ProtobufListNotificationsResponse, ProtobufListOrphanedPanesResponse,
    ProtobufListRecordingsResponse, ProtobufNewTabResponse, ProtobufNewTabsResponse,
    ProtobufNewTiledPaneInTabResponse, ProtobufOpenCommandPaneFloatingNearPluginResponse,
    ProtobufOpenCommandPaneInGitCheckoutResponse, ProtobufOpenCommandPaneInPlaceOfPaneIdResponse,
    ProtobufOpenCommandPaneInPlaceOfPluginResponse, ProtobufOpenCommandPaneNearPluginResponse,
    ProtobufOpenCommandPaneOverlayResponse, ProtobufOpenCommandPaneResponse,
    ProtobufOpenEditPaneInPlaceOfPaneIdResponse, ProtobufOpenFileFloatingNearPluginResponse,
    ProtobufOpenFileInPlaceOfPluginResponse, ProtobufOpenFileNearPluginResponse,
    ProtobufOpenFileResponse, ProtobufOpenPaneInNewTabResponse,
    ProtobufOpenPluginPaneFloatingResponse, ProtobufOpenTerminalFloatingNearPluginResponse,
    ProtobufOpenTerminalInPlaceOfPluginResponse, ProtobufOpenTerminalNearPluginResponse,
    ProtobufOpenTerminalPaneInPlaceOfPaneIdResponse, ProtobufOpenTerminalResponse, ProtobufPaneId,
    ProtobufParseLayoutResponse, ProtobufQueueStatusResponse, ProtobufReadPluginLogResponse,
    ProtobufRenameLayoutResponse, ProtobufSaveLayoutResponse, ProtobufSaveSessionResponse,
    ProtobufSearchRecordingsResponse, ProtobufShowFloatingPanesResponse,
    ProtobufSpawnAgentResponse, ProtobufWarpPaneResponse,
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
        // Seconds, as a number. Lua has `os.date` and does not need this to draw a
        // calendar - the command is for wasm, which has no timezone at all - but a
        // reply the shim reads is a reply every runtime should get decoded.
        CommandType::GetLocalUtcOffset => decode(bytes)
            .and_then(|r: ProtobufGetLocalUtcOffsetResponse| value(lua, r.offset_seconds)),
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
        CommandType::GetPaneGitProbe => {
            decode(bytes).and_then(|r: ProtobufGetPaneGitProbeResponse| match r.result {
                Some(get_pane_git_probe_response::Result::Probe(probe)) => {
                    value(lua, PaneGitProbe::from(probe))
                },
                Some(get_pane_git_probe_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("zellij answered with nothing")),
            })
        },
        CommandType::GetPaneScrollback => {
            decode(bytes).and_then(|r: ProtobufPaneScrollbackResponse| {
                match PaneScrollbackResponse::try_from(r).map_err(|e| anyhow!("{}", e))? {
                    PaneScrollbackResponse::Ok(contents) => value(lua, contents),
                    PaneScrollbackResponse::Err(e) => Err(anyhow!("{}", e)),
                }
            })
        },
        CommandType::GetSessionList => {
            decode(bytes).and_then(|r: ProtobufGetSessionListResponse| {
                match GetSessionListResponse::try_from(r).map_err(|e| anyhow!("{}", e))? {
                    GetSessionListResponse::Ok(snapshot) => value(lua, snapshot),
                    GetSessionListResponse::Err(e) => Err(anyhow!("{}", e)),
                }
            })
        },
        CommandType::CurrentSessionLastSavedTime => {
            decode(bytes).and_then(|r: ProtobufCurrentSessionLastSavedTimeResponse| {
                value(lua, r.timestamp_millis)
            })
        },
        CommandType::SaveSession => {
            decode(bytes).and_then(|r: ProtobufSaveSessionResponse| match r.result {
                Some(save_session_response::Result::Success(_)) => Ok(LuaValue::Boolean(true)),
                Some(save_session_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("zellij answered with nothing")),
            })
        },
        CommandType::DumpLayout => {
            decode(bytes).and_then(|r: ProtobufDumpLayoutResponse| match r.result {
                Some(dump_layout_response::Result::LayoutContent(layout)) => value(lua, layout),
                Some(dump_layout_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("zellij answered with nothing")),
            })
        },
        CommandType::DumpSessionLayout => {
            decode(bytes).and_then(|r: ProtobufDumpSessionLayoutResponse| {
                // The shim drops metadata that fails to convert rather than failing the whole
                // dump, since the layout text is the answer and the metadata is a bonus.
                let metadata = r
                    .metadata
                    .and_then(|metadata| LayoutMetadata::try_from(metadata).ok());
                match r.result {
                    Some(dump_session_layout_response::Result::LayoutContent(layout)) => {
                        value(lua, SessionLayout { layout, metadata })
                    },
                    Some(dump_session_layout_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                    None => Err(anyhow!("zellij answered with nothing")),
                }
            })
        },
        // `nil` for a plugin that compiles, the error text otherwise: a check is a question,
        // so a failing one is an answer rather than a raised error.
        CommandType::CheckLuaPlugin => {
            decode(bytes).and_then(|r: ProtobufCheckLuaPluginResponse| value(lua, r.error))
        },
        CommandType::ParseLayout => {
            decode(bytes).and_then(|r: ProtobufParseLayoutResponse| match r.result {
                Some(parse_layout_response::Result::Metadata(metadata)) => value(
                    lua,
                    LayoutMetadata::try_from(metadata).map_err(|e| anyhow!("{}", e))?,
                ),
                Some(parse_layout_response::Result::Error(e)) => Err(layout_parsing_error(
                    LayoutParsingError::try_from(e).map_err(|e| anyhow!("{}", e))?,
                )),
                None => Err(anyhow!("zellij answered with nothing")),
            })
        },
        CommandType::SaveLayout => {
            decode(bytes).and_then(|r: ProtobufSaveLayoutResponse| match r.result {
                Some(save_layout_response::Result::Success(_)) => Ok(LuaValue::Boolean(true)),
                Some(save_layout_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("zellij answered with nothing")),
            })
        },
        CommandType::DeleteLayout => {
            decode(bytes).and_then(|r: ProtobufDeleteLayoutResponse| match r.result {
                Some(delete_layout_response::Result::Success(_)) => Ok(LuaValue::Boolean(true)),
                Some(delete_layout_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("zellij answered with nothing")),
            })
        },
        CommandType::RenameLayout => {
            decode(bytes).and_then(|r: ProtobufRenameLayoutResponse| match r.result {
                Some(rename_layout_response::Result::Success(_)) => Ok(LuaValue::Boolean(true)),
                Some(rename_layout_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("zellij answered with nothing")),
            })
        },
        CommandType::EditLayout => {
            decode(bytes).and_then(|r: ProtobufEditLayoutResponse| match r.result {
                Some(edit_layout_response::Result::Success(_)) => Ok(LuaValue::Boolean(true)),
                Some(edit_layout_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("zellij answered with nothing")),
            })
        },
        // These two answer with whether anything changed, not just that nothing went wrong, so
        // the flag is kept rather than collapsed to `true`.
        CommandType::ShowFloatingPanes => {
            decode(bytes).and_then(|r: ProtobufShowFloatingPanesResponse| match r.result {
                Some(show_floating_panes_response::Result::Success(changed)) => {
                    Ok(LuaValue::Boolean(changed))
                },
                Some(show_floating_panes_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("zellij answered with nothing")),
            })
        },
        CommandType::HideFloatingPanes => {
            decode(bytes).and_then(|r: ProtobufHideFloatingPanesResponse| match r.result {
                Some(hide_floating_panes_response::Result::Success(changed)) => {
                    Ok(LuaValue::Boolean(changed))
                },
                Some(hide_floating_panes_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("zellij answered with nothing")),
            })
        },
        CommandType::DeleteDeadSessionAndReply => {
            decode(bytes).and_then(|r: ProtobufDeleteDeadSessionResponse| error_or_true(r.error))
        },
        CommandType::DeleteAllDeadSessionsAndReply => decode(bytes)
            .and_then(|r: ProtobufDeleteAllDeadSessionsResponse| error_or_true(r.error)),
        CommandType::KillSessionsAndReply => {
            decode(bytes).and_then(|r: ProtobufKillSessionsResponse| error_or_true(r.error))
        },
        CommandType::WarpPaneToSession
        | CommandType::WarpPaneFromSession
        | CommandType::WarpTabToSession
        | CommandType::WarpTabFromSession
        | CommandType::AdoptOrphanedPane => decode(bytes).and_then(
            |r: ProtobufWarpPaneResponse| match WarpPaneResponse::try_from(r)
                .map_err(|e| anyhow!("{}", e))?
            {
                WarpPaneResponse::Ok(message) => value(lua, message),
                WarpPaneResponse::Err(e) => Err(anyhow!("{}", e)),
            },
        ),
        CommandType::ListOrphanedPanes => {
            decode(bytes).and_then(|r: ProtobufListOrphanedPanesResponse| {
                match ListOrphanedPanesResponse::try_from(r).map_err(|e| anyhow!("{}", e))? {
                    ListOrphanedPanesResponse::Ok(panes) => value(lua, panes),
                    ListOrphanedPanesResponse::Err(e) => Err(anyhow!("{}", e)),
                }
            })
        },
        CommandType::DataDefineEntity => decode(bytes).and_then(|r: ProtobufDataDefineResponse| {
            match DataDefineResponse::try_from(r).map_err(|e| anyhow!("{}", e))? {
                DataDefineResponse::Ok(info) => value(lua, info),
                DataDefineResponse::Err(e) => Err(anyhow!("{}", e)),
            }
        }),
        CommandType::DataApply => {
            decode(bytes).and_then(
                |r: ProtobufDataApplyResponse| match DataApplyResponse::try_from(r)
                    .map_err(|e| anyhow!("{}", e))?
                {
                    DataApplyResponse::Ok(report) => value(lua, report),
                    DataApplyResponse::Err(e) => Err(anyhow!("{}", e)),
                },
            )
        },
        CommandType::DataFetch => {
            decode(bytes).and_then(
                |r: ProtobufDataFetchResponse| match DataFetchResponse::try_from(r)
                    .map_err(|e| anyhow!("{}", e))?
                {
                    DataFetchResponse::Ok(rows) => value(lua, rows),
                    DataFetchResponse::Err(e) => Err(anyhow!("{}", e)),
                },
            )
        },
        CommandType::DataCount => {
            decode(bytes).and_then(
                |r: ProtobufDataCountResponse| match DataCountResponse::try_from(r)
                    .map_err(|e| anyhow!("{}", e))?
                {
                    DataCountResponse::Ok(count) => value(lua, count),
                    DataCountResponse::Err(e) => Err(anyhow!("{}", e)),
                },
            )
        },
        CommandType::DataWatch => {
            decode(bytes).and_then(
                |r: ProtobufDataWatchResponse| match DataWatchResponse::try_from(r)
                    .map_err(|e| anyhow!("{}", e))?
                {
                    DataWatchResponse::Ok(handle) => value(lua, handle),
                    DataWatchResponse::Err(e) => Err(anyhow!("{}", e)),
                },
            )
        },
        CommandType::DataCatalog => decode(bytes).and_then(|r: ProtobufDataCatalogResponse| {
            match DataCatalogResponse::try_from(r).map_err(|e| anyhow!("{}", e))? {
                DataCatalogResponse::Ok(entities) => value(lua, entities),
                DataCatalogResponse::Err(e) => Err(anyhow!("{}", e)),
            }
        }),
        CommandType::DataUnwatch => decode(bytes).and_then(|r: ProtobufDataUnwatchResponse| {
            match DataUnwatchResponse::try_from(r).map_err(|e| anyhow!("{}", e))? {
                DataUnwatchResponse::Ok => Ok(LuaValue::Boolean(true)),
                DataUnwatchResponse::Err(e) => Err(anyhow!("{}", e)),
            }
        }),
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
        CommandType::ListRecordings => {
            decode(bytes).and_then(|r: ProtobufListRecordingsResponse| {
                let recordings: Vec<RecordingSummary> =
                    r.recordings.into_iter().map(Into::into).collect();
                value(lua, recordings)
            })
        },
        CommandType::SearchRecordings => {
            decode(bytes).and_then(|r: ProtobufSearchRecordingsResponse| {
                let hits: Vec<SearchHit> = r.hits.into_iter().map(Into::into).collect();
                value(lua, hits)
            })
        },
        CommandType::ListNotifications => {
            decode(bytes).and_then(|r: ProtobufListNotificationsResponse| match r.error {
                Some(e) => Err(anyhow!("{}", e)),
                None => {
                    let notifications: Vec<NotificationInfo> =
                        r.notifications.into_iter().map(Into::into).collect();
                    value(lua, notifications)
                },
            })
        },
        CommandType::NewTab | CommandType::NewTabUnfocused => {
            decode(bytes).and_then(|r: ProtobufNewTabResponse| match r.result {
                Some(new_tab_response::Result::TabId(tab_id)) => value(lua, tab_id),
                _ => Ok(LuaValue::Nil),
            })
        },
        CommandType::NewTabsWithLayout | CommandType::NewTabsWithLayoutInfo => {
            decode(bytes).and_then(|r: ProtobufNewTabsResponse| value(lua, r.tab_ids))
        },
        CommandType::FocusOrCreateTab => {
            decode(bytes).and_then(|r: ProtobufFocusOrCreateTabResponse| match r.result {
                Some(focus_or_create_tab_response::Result::TabId(tab_id)) => value(lua, tab_id),
                _ => Ok(LuaValue::Nil),
            })
        },
        CommandType::BreakPanesToNewTab => {
            decode(bytes).and_then(|r: ProtobufBreakPanesToNewTabResponse| match r.result {
                Some(break_panes_to_new_tab_response::Result::TabId(tab_id)) => value(lua, tab_id),
                _ => Ok(LuaValue::Nil),
            })
        },
        CommandType::BreakPanesToTabWithIndex => decode(bytes).and_then(
            |r: ProtobufBreakPanesToTabWithIndexResponse| match r.result {
                Some(break_panes_to_tab_with_index_response::Result::TabId(tab_id)) => {
                    value(lua, tab_id)
                },
                _ => Ok(LuaValue::Nil),
            },
        ),
        CommandType::BreakPanesToTabWithId => {
            decode(bytes).and_then(|r: ProtobufBreakPanesToTabWithIdResponse| match r.result {
                Some(break_panes_to_tab_with_id_response::Result::TabId(tab_id)) => {
                    value(lua, tab_id)
                },
                _ => Ok(LuaValue::Nil),
            })
        },
        CommandType::NewTiledPaneInTab => {
            decode(bytes).and_then(|r: ProtobufNewTiledPaneInTabResponse| pane_id(lua, r.pane_id))
        },
        CommandType::OpenFile | CommandType::OpenFileFloating | CommandType::OpenFileInPlace => {
            decode(bytes).and_then(|r: ProtobufOpenFileResponse| pane_id(lua, r.pane_id))
        },
        CommandType::OpenFileNearPlugin => {
            decode(bytes).and_then(|r: ProtobufOpenFileNearPluginResponse| pane_id(lua, r.pane_id))
        },
        CommandType::OpenFileFloatingNearPlugin => decode(bytes)
            .and_then(|r: ProtobufOpenFileFloatingNearPluginResponse| pane_id(lua, r.pane_id)),
        CommandType::OpenFileInPlaceOfPlugin => decode(bytes)
            .and_then(|r: ProtobufOpenFileInPlaceOfPluginResponse| pane_id(lua, r.pane_id)),
        CommandType::OpenTerminal
        | CommandType::OpenTerminalFloating
        | CommandType::OpenTerminalInPlace => {
            decode(bytes).and_then(|r: ProtobufOpenTerminalResponse| pane_id(lua, r.pane_id))
        },
        CommandType::OpenTerminalNearPlugin => decode(bytes)
            .and_then(|r: ProtobufOpenTerminalNearPluginResponse| pane_id(lua, r.pane_id)),
        CommandType::OpenTerminalFloatingNearPlugin => decode(bytes)
            .and_then(|r: ProtobufOpenTerminalFloatingNearPluginResponse| pane_id(lua, r.pane_id)),
        CommandType::OpenTerminalInPlaceOfPlugin => decode(bytes)
            .and_then(|r: ProtobufOpenTerminalInPlaceOfPluginResponse| pane_id(lua, r.pane_id)),
        CommandType::OpenTerminalPaneInPlaceOfPaneId => decode(bytes)
            .and_then(|r: ProtobufOpenTerminalPaneInPlaceOfPaneIdResponse| pane_id(lua, r.pane_id)),
        CommandType::OpenEditPaneInPlaceOfPaneId => decode(bytes)
            .and_then(|r: ProtobufOpenEditPaneInPlaceOfPaneIdResponse| pane_id(lua, r.pane_id)),
        CommandType::OpenCommandPane
        | CommandType::OpenCommandPaneFloating
        | CommandType::OpenCommandPaneInPlace
        | CommandType::OpenCommandPaneBackground => {
            decode(bytes).and_then(|r: ProtobufOpenCommandPaneResponse| pane_id(lua, r.pane_id))
        },
        CommandType::OpenCommandPaneNearPlugin => decode(bytes)
            .and_then(|r: ProtobufOpenCommandPaneNearPluginResponse| pane_id(lua, r.pane_id)),
        CommandType::OpenCommandPaneOverlay => decode(bytes)
            .and_then(|r: ProtobufOpenCommandPaneOverlayResponse| pane_id(lua, r.pane_id)),
        CommandType::OpenCommandPaneFloatingNearPlugin => {
            decode(bytes).and_then(|r: ProtobufOpenCommandPaneFloatingNearPluginResponse| {
                pane_id(lua, r.pane_id)
            })
        },
        CommandType::OpenCommandPaneInPlaceOfPlugin => decode(bytes)
            .and_then(|r: ProtobufOpenCommandPaneInPlaceOfPluginResponse| pane_id(lua, r.pane_id)),
        CommandType::OpenCommandPaneInPlaceOfPaneId => decode(bytes)
            .and_then(|r: ProtobufOpenCommandPaneInPlaceOfPaneIdResponse| pane_id(lua, r.pane_id)),
        CommandType::OpenPluginPaneFloating => decode(bytes)
            .and_then(|r: ProtobufOpenPluginPaneFloatingResponse| pane_id(lua, r.pane_id)),
        CommandType::OpenCommandPaneInGitCheckoutNearPlugin => {
            decode(bytes).and_then(|r: ProtobufOpenCommandPaneInGitCheckoutResponse| {
                match r.result {
                    Some(open_command_pane_in_git_checkout_response::Result::PaneId(id)) => {
                        pane_id(lua, Some(id))
                    },
                    Some(open_command_pane_in_git_checkout_response::Result::Error(e)) => {
                        Err(anyhow!("{}", e))
                    },
                    None => Err(anyhow!("zellij answered with nothing")),
                }
            })
        },
        CommandType::SpawnAgent => {
            decode(bytes).and_then(|r: ProtobufSpawnAgentResponse| match r.result {
                Some(spawn_agent_response::Result::PaneId(id)) => pane_id(lua, Some(id)),
                Some(spawn_agent_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("zellij answered with nothing")),
            })
        },
        CommandType::OpenCommandPaneInNewTab
        | CommandType::OpenPluginPaneInNewTab
        | CommandType::OpenEditorPaneInNewTab => {
            decode(bytes).and_then(|r: ProtobufOpenPaneInNewTabResponse| {
                let pane_id = match r.pane_id.map(PaneId::try_from) {
                    Some(Ok(pane_id)) => Some(pane_id),
                    Some(Err(e)) => return Err(anyhow!("{}", e)),
                    None => None,
                };
                value(
                    lua,
                    PaneInNewTab {
                        tab_id: r.tab_id.map(|id| id as usize),
                        pane_id,
                    },
                )
            })
        },
        CommandType::ReadPluginLog => {
            decode(bytes).and_then(|r: ProtobufReadPluginLogResponse| match r.result {
                Some(read_plugin_log_response::Result::Lines(lines)) => value(lua, lines),
                Some(read_plugin_log_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("zellij answered with nothing")),
            })
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

/// A `DumpSessionLayout` answer: the KDL text, and the metadata the host sent alongside it.
#[derive(Serialize)]
struct SessionLayout {
    layout: String,
    metadata: Option<LayoutMetadata>,
}

/// The `(tab_id, pane_id)` pair the shim returns for the `*InNewTab` family, with names.
#[derive(Serialize)]
struct PaneInNewTab {
    tab_id: Option<usize>,
    pane_id: Option<PaneId>,
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

/// The replies whose only field is an optional error: absent means it worked.
fn error_or_true(error: Option<String>) -> Result<LuaValue> {
    match error {
        Some(e) => Err(anyhow!("{}", e)),
        None => Ok(LuaValue::Boolean(true)),
    }
}

/// The shim hands `LayoutParsingError` back as a value; Lua gets it as a raised message, so the
/// parts a plugin would show - where, what, and the parser's hint - are folded into one line.
fn layout_parsing_error(error: LayoutParsingError) -> anyhow::Error {
    match error {
        LayoutParsingError::KdlError {
            kdl_error,
            file_name,
            ..
        } => match kdl_error.help_message {
            Some(help) => anyhow!("{}: {} ({})", file_name, kdl_error.error_message, help),
            None => anyhow!("{}: {}", file_name, kdl_error.error_message),
        },
        LayoutParsingError::SyntaxError => anyhow!("the layout could not be parsed"),
    }
}

fn value<T: Serialize>(lua: &Lua, from: T) -> Result<LuaValue> {
    super::to_lua_value(lua, &from)
        .map_err(|e| anyhow!("zellij's answer could not be given to Lua: {}", e))
}
