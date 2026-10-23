//! Runtime-neutral decoding of host command replies.
//!
//! Host responses use protobuf. Each arm returns JSON or binary file contents; missing
//! optional payloads become JSON null and errors remain errors. Unrecognized commands
//! return `None` so the caller can retain its raw-byte fallback.

use prost::Message;
use serde::Serialize;
use yinz_types::internal_ids::TabId;

use yinz_plugin_api::event::ProtobufPaneScrollbackResponse;
use yinz_runtime::errors::prelude::*;
use {
    yinz_plugin_api::plugin_command::break_panes_to_new_tab_response,
    yinz_plugin_api::plugin_command::break_panes_to_tab_with_id_response,
    yinz_plugin_api::plugin_command::break_panes_to_tab_with_index_response,
    yinz_plugin_api::plugin_command::delete_layout_response,
    yinz_plugin_api::plugin_command::dump_layout_response,
    yinz_plugin_api::plugin_command::dump_session_layout_response,
    yinz_plugin_api::plugin_command::edit_layout_response,
    yinz_plugin_api::plugin_command::focus_or_create_tab_response,
    yinz_plugin_api::plugin_command::fs_list_response,
    yinz_plugin_api::plugin_command::fs_metadata_response,
    yinz_plugin_api::plugin_command::fs_mutate_response,
    yinz_plugin_api::plugin_command::fs_read_response,
    yinz_plugin_api::plugin_command::get_focused_pane_info_response,
    yinz_plugin_api::plugin_command::get_pane_cwd_response,
    yinz_plugin_api::plugin_command::get_pane_git_probe_response,
    yinz_plugin_api::plugin_command::get_pane_pid_response,
    yinz_plugin_api::plugin_command::get_pane_running_command_response,
    yinz_plugin_api::plugin_command::hide_floating_panes_response,
    yinz_plugin_api::plugin_command::new_tab_response,
    yinz_plugin_api::plugin_command::open_command_pane_in_git_checkout_response,
    yinz_plugin_api::plugin_command::parse_layout_response,
    yinz_plugin_api::plugin_command::read_plugin_log_response,
    yinz_plugin_api::plugin_command::rename_layout_response,
    yinz_plugin_api::plugin_command::save_layout_response,
    yinz_plugin_api::plugin_command::save_session_response,
    yinz_plugin_api::plugin_command::show_floating_panes_response,
    yinz_plugin_api::plugin_command::spawn_agent_response,
    yinz_plugin_api::plugin_command::ProtobufBreakPanesToNewTabResponse,
    yinz_plugin_api::plugin_command::ProtobufBreakPanesToTabWithIdResponse,
    yinz_plugin_api::plugin_command::ProtobufBreakPanesToTabWithIndexResponse,
    yinz_plugin_api::plugin_command::ProtobufCurrentSessionLastSavedTimeResponse,
    yinz_plugin_api::plugin_command::ProtobufDataApplyResponse,
    yinz_plugin_api::plugin_command::ProtobufDataCatalogResponse,
    yinz_plugin_api::plugin_command::ProtobufDataCountResponse,
    yinz_plugin_api::plugin_command::ProtobufDataDefineResponse,
    yinz_plugin_api::plugin_command::ProtobufDataFetchResponse,
    yinz_plugin_api::plugin_command::ProtobufDataUnwatchResponse,
    yinz_plugin_api::plugin_command::ProtobufDataWatchResponse,
    yinz_plugin_api::plugin_command::ProtobufDeleteAllDeadSessionsResponse,
    yinz_plugin_api::plugin_command::ProtobufDeleteDeadSessionResponse,
    yinz_plugin_api::plugin_command::ProtobufDeleteLayoutResponse,
    yinz_plugin_api::plugin_command::ProtobufDumpLayoutResponse,
    yinz_plugin_api::plugin_command::ProtobufDumpSessionLayoutResponse,
    yinz_plugin_api::plugin_command::ProtobufEditLayoutResponse,
    yinz_plugin_api::plugin_command::ProtobufFocusOrCreateTabResponse,
    yinz_plugin_api::plugin_command::ProtobufFsListResponse,
    yinz_plugin_api::plugin_command::ProtobufFsMetadataResponse,
    yinz_plugin_api::plugin_command::ProtobufFsMutateResponse,
    yinz_plugin_api::plugin_command::ProtobufFsReadResponse,
    yinz_plugin_api::plugin_command::ProtobufGenerateRandomNameResponse,
    yinz_plugin_api::plugin_command::ProtobufGetFocusedPaneInfoResponse,
    yinz_plugin_api::plugin_command::ProtobufGetLayoutDirResponse,
    yinz_plugin_api::plugin_command::ProtobufGetLocalUtcOffsetResponse,
    yinz_plugin_api::plugin_command::ProtobufGetPaneCwdResponse,
    yinz_plugin_api::plugin_command::ProtobufGetPaneGitProbeResponse,
    yinz_plugin_api::plugin_command::ProtobufGetPaneInfoResponse,
    yinz_plugin_api::plugin_command::ProtobufGetPanePidResponse,
    yinz_plugin_api::plugin_command::ProtobufGetPaneRunningCommandResponse,
    yinz_plugin_api::plugin_command::ProtobufGetPluginAliasConfigurationResponse,
    yinz_plugin_api::plugin_command::ProtobufGetSessionEnvironmentVariablesResponse,
    yinz_plugin_api::plugin_command::ProtobufGetSessionListResponse,
    yinz_plugin_api::plugin_command::ProtobufGetTabInfoResponse,
    yinz_plugin_api::plugin_command::ProtobufGetUserPluginDirResponse,
    yinz_plugin_api::plugin_command::ProtobufHideFloatingPanesResponse,
    yinz_plugin_api::plugin_command::ProtobufKillSessionsResponse,
    yinz_plugin_api::plugin_command::ProtobufListNotificationsResponse,
    yinz_plugin_api::plugin_command::ProtobufListOrphanedPanesResponse,
    yinz_plugin_api::plugin_command::ProtobufNewTabResponse,
    yinz_plugin_api::plugin_command::ProtobufNewTabsResponse,
    yinz_plugin_api::plugin_command::ProtobufNewTiledPaneInTabResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenCommandPaneFloatingNearPluginResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenCommandPaneInGitCheckoutResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenCommandPaneInPlaceOfPaneIdResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenCommandPaneInPlaceOfPluginResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenCommandPaneNearPluginResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenCommandPaneOverlayResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenCommandPaneResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenEditPaneInPlaceOfPaneIdResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenFileFloatingNearPluginResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenFileInPlaceOfPluginResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenFileNearPluginResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenFileResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenPaneInNewTabResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenPluginPaneFloatingResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenPluginPaneInPlaceOfPaneIdResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenTerminalFloatingNearPluginResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenTerminalInPlaceOfPluginResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenTerminalNearPluginResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenTerminalPaneInPlaceOfPaneIdResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenTerminalResponse,
    yinz_plugin_api::plugin_command::ProtobufPaneId,
    yinz_plugin_api::plugin_command::ProtobufParseLayoutResponse,
    yinz_plugin_api::plugin_command::ProtobufReadPluginLogResponse,
    yinz_plugin_api::plugin_command::ProtobufRenameLayoutResponse,
    yinz_plugin_api::plugin_command::ProtobufSaveLayoutResponse,
    yinz_plugin_api::plugin_command::ProtobufSaveSessionResponse,
    yinz_plugin_api::plugin_command::ProtobufShowFloatingPanesResponse,
    yinz_plugin_api::plugin_command::ProtobufSpawnAgentResponse,
    yinz_plugin_api::plugin_command::ProtobufWarpPaneResponse,
};
use {
    yinz_plugin_api::plugin_ids::ProtobufPluginIds,
    yinz_plugin_api::plugin_ids::ProtobufYinzVersion,
};
use {
    yinz_types::data::CommandType, yinz_types::data::DataApplyResponse,
    yinz_types::data::DataCatalogResponse, yinz_types::data::DataCountResponse,
    yinz_types::data::DataDefineResponse, yinz_types::data::DataFetchResponse,
    yinz_types::data::DataUnwatchResponse, yinz_types::data::DataWatchResponse,
    yinz_types::data::FileMetadata, yinz_types::data::GetSessionListResponse,
    yinz_types::data::LayoutMetadata, yinz_types::data::LayoutParsingError,
    yinz_types::data::ListOrphanedPanesResponse, yinz_types::data::NotificationInfo,
    yinz_types::data::PaneGitProbe, yinz_types::data::PaneId,
    yinz_types::data::PaneScrollbackResponse, yinz_types::data::PluginIds,
    yinz_types::data::WarpPaneResponse,
};

/// A decoded host reply. File contents stay binary, including invalid UTF-8.
#[derive(Debug, PartialEq)]
pub enum DecodedResponse {
    Json(serde_json::Value),
    Bytes(Vec<u8>),
}

/// Decode the reply `bytes` a `command` produced. `None` means "no table for this one" - the
/// caller hands the bytes back untouched.
pub fn decode_response(command: CommandType, bytes: &[u8]) -> Option<Result<DecodedResponse>> {
    Some(match command {
        CommandType::ResolveProject
        | CommandType::RegisterProjectProvider
        | CommandType::GetProjectProviders
        | CommandType::ValidateProjectMatch
        | CommandType::PublishProjectLocations => decode(bytes).and_then(
            |response: yinz_plugin_api::plugin_command::ProtobufAgentProvidersResponse| {
                let result: Result<serde_json::Value, String> =
                    serde_json::from_str(&response.json)
                        .context("decode agent provider response")?;
                value(result.map_err(|error| anyhow!(error))?)
            },
        ),
        CommandType::RegisterAgentProvider | CommandType::GetAgentProviders => decode(bytes)
            .and_then(
                |response: yinz_plugin_api::plugin_command::ProtobufAgentProvidersResponse| {
                    let result: Result<Vec<yinz_types::agent::provider::AgentProvider>, String> =
                        serde_json::from_str(&response.json)
                            .context("decode agent provider response")?;
                    value(result.map_err(|error| anyhow!(error))?)
                },
            ),
        CommandType::RestartAgentPane => decode(bytes).and_then(
            |response: yinz_plugin_api::plugin_command::ProtobufRestartAgentPaneResponse| {
                let result: Result<yinz_types::agent::control::RestartedAgent, String> =
                    serde_json::from_str(&response.json)
                        .context("decode agent restart response")?;
                value(result.map_err(|error| anyhow!(error))?)
            },
        ),
        CommandType::PaneTabs => decode(bytes).and_then(
            |response: yinz_plugin_api::plugin_command::ProtobufPaneTabsResponse| {
                let result: yinz_types::pane_tabs::PaneTabsResponse =
                    serde_json::from_str(&response.json)?;
                value(result.map_err(|error| anyhow!(error))?)
            },
        ),
        CommandType::CreateMultipane
        | CommandType::UpdateMultipane
        | CommandType::ReplaceMultipaneChild => decode(bytes).and_then(
            |response: yinz_plugin_api::plugin_command::ProtobufMultipaneResponse| {
                let result: yinz_types::multipane::MultipaneResponse =
                    serde_json::from_str(&response.json)?;
                value(result.map_err(|error| anyhow!(error))?)
            },
        ),
        CommandType::GetCellPixelSize => decode(bytes).and_then(
            |response: yinz_plugin_api::plugin_command::ProtobufGetCellPixelSizeResponse| {
                let size: Option<yinz_types::pane_size::SizeInPixels> = response
                    .size
                    .map(TryInto::try_into)
                    .transpose()
                    .map_err(|error| anyhow!("{error}"))?;
                value(size)
            },
        ),
        CommandType::GetPluginIds => decode(bytes).and_then(|response: ProtobufPluginIds| {
            let ids = PluginIds::try_from(response).map_err(|e| anyhow!("{}", e))?;
            value(ids)
        }),
        CommandType::GetYinzVersion => {
            decode(bytes).and_then(|r: ProtobufYinzVersion| value(r.version))
        },
        CommandType::GenerateRandomName => {
            decode(bytes).and_then(|r: ProtobufGenerateRandomNameResponse| value(r.name))
        },
        CommandType::GetUserPluginDir => {
            decode(bytes).and_then(|r: ProtobufGetUserPluginDirResponse| value(r.user_plugin_dir))
        },
        // Seconds east of UTC, supplied by the host timezone database.
        CommandType::GetLocalUtcOffset => {
            decode(bytes).and_then(|r: ProtobufGetLocalUtcOffsetResponse| value(r.offset_seconds))
        },
        CommandType::GetLayoutDir => {
            decode(bytes).and_then(|r: ProtobufGetLayoutDirResponse| value(r.layout_dir))
        },
        CommandType::GetSessionEnvironmentVariables => {
            decode(bytes).and_then(|r: ProtobufGetSessionEnvironmentVariablesResponse| {
                let vars: std::collections::BTreeMap<String, String> = r
                    .env_vars
                    .into_iter()
                    .map(|env_var| (env_var.name, env_var.value))
                    .collect();
                value(vars)
            })
        },
        CommandType::Pick => {
            decode(bytes).and_then(|r: yinz_plugin_api::plugin_command::ProtobufPickResponse| {
                serde_json::from_str::<Result<Option<yinz_types::pick::PickRequest>, String>>(
                    &r.json,
                )
                .context("decode picker response")?
                .map_err(|error| anyhow!(error))
                .and_then(value)
            })
        },
        CommandType::GetPluginInfo => decode(bytes).and_then(
            |r: yinz_plugin_api::plugin_command::ProtobufGetPluginInfoResponse| {
                serde_json::from_str::<serde_json::Value>(&r.json)
                    .context("decode plugin info")
                    .and_then(value)
            },
        ),
        CommandType::GetPluginIdsForUrl | CommandType::GetPluginIdsForUrlWithConfiguration => {
            decode(bytes).and_then(
                |response: yinz_plugin_api::plugin_command::ProtobufGetPluginIdsForUrlResponse| {
                    match response.error {
                        Some(error) => Err(anyhow!(error)),
                        None => value(response.plugin_ids),
                    }
                },
            )
        },
        CommandType::GetPluginAliasConfiguration => {
            decode(bytes).and_then(|r: ProtobufGetPluginAliasConfigurationResponse| {
                let configuration: std::collections::BTreeMap<String, String> =
                    r.configuration.into_iter().collect();
                value(configuration)
            })
        },
        CommandType::GetFocusedPaneInfo => {
            decode(bytes).and_then(|r: ProtobufGetFocusedPaneInfoResponse| match r.result {
                Some(get_focused_pane_info_response::Result::FocusedPaneInfo(info)) => {
                    let pane_id = info
                        .focused_pane_id
                        .and_then(|id| PaneId::try_from(id).ok())
                        .ok_or_else(|| anyhow!("the focused pane has no id"))?;
                    value(FocusedPane {
                        tab_id: TabId::new(info.focused_tab_id as usize),
                        pane_id,
                    })
                },
                Some(get_focused_pane_info_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("yinz answered with nothing")),
            })
        },
        CommandType::GetPaneInfo => {
            decode(bytes).and_then(|r: ProtobufGetPaneInfoResponse| match r.pane_info {
                Some(info) => {
                    value(yinz_types::data::PaneInfo::try_from(info).map_err(|e| anyhow!("{}", e))?)
                },
                None => value(serde_json::Value::Null),
            })
        },
        CommandType::GetTabInfo => {
            decode(bytes).and_then(|r: ProtobufGetTabInfoResponse| match r.tab_info {
                Some(info) => {
                    value(yinz_types::data::TabInfo::try_from(info).map_err(|e| anyhow!("{}", e))?)
                },
                None => value(serde_json::Value::Null),
            })
        },
        CommandType::GetPaneCwd => {
            decode(bytes).and_then(|r: ProtobufGetPaneCwdResponse| match r.result {
                Some(get_pane_cwd_response::Result::Cwd(cwd)) => value(cwd),
                Some(get_pane_cwd_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => value(serde_json::Value::Null),
            })
        },
        CommandType::GetPanePid => {
            decode(bytes).and_then(|r: ProtobufGetPanePidResponse| match r.result {
                Some(get_pane_pid_response::Result::Pid(pid)) => value(pid),
                Some(get_pane_pid_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => value(serde_json::Value::Null),
            })
        },
        CommandType::GetPaneRunningCommand => {
            decode(bytes).and_then(|r: ProtobufGetPaneRunningCommandResponse| match r.result {
                Some(get_pane_running_command_response::Result::Command(command)) => {
                    value(command.args)
                },
                Some(get_pane_running_command_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => value(serde_json::Value::Null),
            })
        },
        CommandType::GetPaneGitProbe => {
            decode(bytes).and_then(|r: ProtobufGetPaneGitProbeResponse| match r.result {
                Some(get_pane_git_probe_response::Result::Probe(probe)) => {
                    let probe = PaneGitProbe::from(probe);
                    Ok(DecodedResponse::Json(serde_json::json!({
                        "host": probe.host,
                        "cwd": probe.cwd,
                        "exit_code": probe.exit_code,
                        "metadata": yinz_steel::binary::encode(&probe.metadata)?,
                        "config": yinz_steel::binary::encode(&probe.config)?,
                    })))
                },
                Some(get_pane_git_probe_response::Result::Pending(_)) => {
                    value(serde_json::Value::Null)
                },
                Some(get_pane_git_probe_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("yinz answered with nothing")),
            })
        },
        CommandType::GetPaneScrollback => {
            decode(bytes).and_then(|r: ProtobufPaneScrollbackResponse| {
                match PaneScrollbackResponse::try_from(r).map_err(|e| anyhow!("{}", e))? {
                    PaneScrollbackResponse::Ok(contents) => value(contents),
                    PaneScrollbackResponse::Err(e) => Err(anyhow!("{}", e)),
                }
            })
        },
        CommandType::GetDisconnectTargets => decode(bytes).and_then(
            |r: yinz_plugin_api::plugin_command::ProtobufGetDisconnectTargetsResponse| match r.error
            {
                Some(error) => Err(anyhow!(error)),
                None => value(r.client_ids),
            },
        ),
        CommandType::GetSessionInventory | CommandType::SwitchSessionDestination => decode(bytes)
            .and_then(
            |r: yinz_plugin_api::generated_api::api::plugin_command::SessionDestinationResponse| {
                let reply: Result<serde_json::Value, String> =
                    serde_json::from_str(&r.json).context("decode session destination response")?;
                value(reply.map_err(anyhow::Error::msg)?)
            },
        ),
        CommandType::GetSessionList => {
            decode(bytes).and_then(|r: ProtobufGetSessionListResponse| {
                match GetSessionListResponse::try_from(r).map_err(|e| anyhow!("{}", e))? {
                    GetSessionListResponse::Ok(snapshot) => value(snapshot),
                    GetSessionListResponse::Err(e) => Err(anyhow!("{}", e)),
                }
            })
        },
        CommandType::CurrentSessionLastSavedTime => decode(bytes)
            .and_then(|r: ProtobufCurrentSessionLastSavedTimeResponse| value(r.timestamp_millis)),
        CommandType::SaveSession => {
            decode(bytes).and_then(|r: ProtobufSaveSessionResponse| match r.result {
                Some(save_session_response::Result::Success(_)) => value(true),
                Some(save_session_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("yinz answered with nothing")),
            })
        },
        CommandType::DumpLayout => {
            decode(bytes).and_then(|r: ProtobufDumpLayoutResponse| match r.result {
                Some(dump_layout_response::Result::LayoutContent(layout)) => value(layout),
                Some(dump_layout_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("yinz answered with nothing")),
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
                        value(SessionLayout { layout, metadata })
                    },
                    Some(dump_session_layout_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                    None => Err(anyhow!("yinz answered with nothing")),
                }
            })
        },
        // `nil` for a plugin that compiles, the error text otherwise: a check is a question,
        // so a failing one is an answer rather than a raised error.
        CommandType::CheckPlugin => {
            use yinz_plugin_api::plugin_command::{
                check_plugin_response, ProtobufCheckPluginResponse,
            };
            decode(bytes).and_then(|r: ProtobufCheckPluginResponse| match r.result {
                Some(check_plugin_response::Result::ReportJson(json)) => {
                    value(serde_json::from_str::<
                        yinz_types::authoring::PluginCheckReport,
                    >(&json)?)
                },
                Some(check_plugin_response::Result::Error(error)) => Err(anyhow!(error)),
                None => Err(anyhow!("yinz answered with nothing")),
            })
        },
        CommandType::ParseLayout => {
            decode(bytes).and_then(|r: ProtobufParseLayoutResponse| match r.result {
                Some(parse_layout_response::Result::Metadata(metadata)) => {
                    value(LayoutMetadata::try_from(metadata).map_err(|e| anyhow!("{}", e))?)
                },
                Some(parse_layout_response::Result::Error(e)) => Err(layout_parsing_error(
                    LayoutParsingError::try_from(e).map_err(|e| anyhow!("{}", e))?,
                )),
                None => Err(anyhow!("yinz answered with nothing")),
            })
        },
        CommandType::SaveLayout => {
            decode(bytes).and_then(|r: ProtobufSaveLayoutResponse| match r.result {
                Some(save_layout_response::Result::Success(_)) => value(true),
                Some(save_layout_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("yinz answered with nothing")),
            })
        },
        CommandType::DeleteLayout => {
            decode(bytes).and_then(|r: ProtobufDeleteLayoutResponse| match r.result {
                Some(delete_layout_response::Result::Success(_)) => value(true),
                Some(delete_layout_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("yinz answered with nothing")),
            })
        },
        CommandType::RenameLayout => {
            decode(bytes).and_then(|r: ProtobufRenameLayoutResponse| match r.result {
                Some(rename_layout_response::Result::Success(_)) => value(true),
                Some(rename_layout_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("yinz answered with nothing")),
            })
        },
        CommandType::EditLayout => {
            decode(bytes).and_then(|r: ProtobufEditLayoutResponse| match r.result {
                Some(edit_layout_response::Result::Success(_)) => value(true),
                Some(edit_layout_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("yinz answered with nothing")),
            })
        },
        // These two answer with whether anything changed, not just that nothing went wrong, so
        // the flag is kept rather than collapsed to `true`.
        CommandType::ShowFloatingPanes => {
            decode(bytes).and_then(|r: ProtobufShowFloatingPanesResponse| match r.result {
                Some(show_floating_panes_response::Result::Success(changed)) => value(changed),
                Some(show_floating_panes_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("yinz answered with nothing")),
            })
        },
        CommandType::HideFloatingPanes => {
            decode(bytes).and_then(|r: ProtobufHideFloatingPanesResponse| match r.result {
                Some(hide_floating_panes_response::Result::Success(changed)) => value(changed),
                Some(hide_floating_panes_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("yinz answered with nothing")),
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
                WarpPaneResponse::Ok(message) => value(message),
                WarpPaneResponse::Err(e) => Err(anyhow!("{}", e)),
            },
        ),
        CommandType::ListOrphanedPanes => {
            decode(bytes).and_then(|r: ProtobufListOrphanedPanesResponse| {
                match ListOrphanedPanesResponse::try_from(r).map_err(|e| anyhow!("{}", e))? {
                    ListOrphanedPanesResponse::Ok(panes) => value(panes),
                    ListOrphanedPanesResponse::Err(e) => Err(anyhow!("{}", e)),
                }
            })
        },
        CommandType::DataDefineEntity => decode(bytes).and_then(|r: ProtobufDataDefineResponse| {
            match DataDefineResponse::try_from(r).map_err(|e| anyhow!("{}", e))? {
                DataDefineResponse::Ok(info) => value(info),
                DataDefineResponse::Err(e) => Err(anyhow!("{}", e)),
            }
        }),
        CommandType::DataApply => {
            decode(bytes).and_then(
                |r: ProtobufDataApplyResponse| match DataApplyResponse::try_from(r)
                    .map_err(|e| anyhow!("{}", e))?
                {
                    DataApplyResponse::Ok(report) => value(report),
                    // The SDK recognizes revision conflicts by their message prefix.
                    DataApplyResponse::Conflict(e) => Err(anyhow!("{}{}", DATA_CONFLICT_MARKER, e)),
                    DataApplyResponse::Err(e) => Err(anyhow!("{}", e)),
                },
            )
        },
        CommandType::DataFetch => {
            decode(bytes).and_then(
                |r: ProtobufDataFetchResponse| match DataFetchResponse::try_from(r)
                    .map_err(|e| anyhow!("{}", e))?
                {
                    DataFetchResponse::Ok(rows) => {
                        crate::binary_data::rows(&rows).map(DecodedResponse::Json)
                    },
                    DataFetchResponse::Err(e) => Err(anyhow!("{}", e)),
                },
            )
        },
        CommandType::DataCount => {
            decode(bytes).and_then(
                |r: ProtobufDataCountResponse| match DataCountResponse::try_from(r)
                    .map_err(|e| anyhow!("{}", e))?
                {
                    DataCountResponse::Ok(count) => value(count),
                    DataCountResponse::Err(e) => Err(anyhow!("{}", e)),
                },
            )
        },
        CommandType::DataWatch => {
            decode(bytes).and_then(
                |r: ProtobufDataWatchResponse| match DataWatchResponse::try_from(r)
                    .map_err(|e| anyhow!("{}", e))?
                {
                    DataWatchResponse::Ok(handle) => Ok(DecodedResponse::Json(serde_json::json!({
                        "watch_id": handle.watch_id,
                        "rows": crate::binary_data::rows(&handle.rows)?,
                    }))),
                    DataWatchResponse::Err(e) => Err(anyhow!("{}", e)),
                },
            )
        },
        CommandType::DataCatalog => decode(bytes).and_then(|r: ProtobufDataCatalogResponse| {
            match DataCatalogResponse::try_from(r).map_err(|e| anyhow!("{}", e))? {
                DataCatalogResponse::Ok(entities) => {
                    crate::binary_data::catalog(entities).map(DecodedResponse::Json)
                },
                DataCatalogResponse::Err(e) => Err(anyhow!("{}", e)),
            }
        }),
        CommandType::DataUnwatch => decode(bytes).and_then(|r: ProtobufDataUnwatchResponse| {
            match DataUnwatchResponse::try_from(r).map_err(|e| anyhow!("{}", e))? {
                DataUnwatchResponse::Ok => value(true),
                DataUnwatchResponse::Err(e) => Err(anyhow!("{}", e)),
            }
        }),
        CommandType::ListNotifications => {
            decode(bytes).and_then(|r: ProtobufListNotificationsResponse| match r.error {
                Some(e) => Err(anyhow!("{}", e)),
                None => {
                    let notifications: Vec<NotificationInfo> =
                        r.notifications.into_iter().map(Into::into).collect();
                    value(notifications)
                },
            })
        },
        CommandType::NewTab | CommandType::NewTabUnfocused => {
            decode(bytes).and_then(|r: ProtobufNewTabResponse| match r.result {
                Some(new_tab_response::Result::TabId(tab_id)) => value(tab_id),
                _ => value(serde_json::Value::Null),
            })
        },
        CommandType::NewTabsWithLayout | CommandType::NewTabsWithLayoutInfo => {
            decode(bytes).and_then(|r: ProtobufNewTabsResponse| value(r.tab_ids))
        },
        CommandType::FocusOrCreateTab => {
            decode(bytes).and_then(|r: ProtobufFocusOrCreateTabResponse| match r.result {
                Some(focus_or_create_tab_response::Result::TabId(tab_id)) => value(tab_id),
                _ => value(serde_json::Value::Null),
            })
        },
        CommandType::BreakPanesToNewTab => {
            decode(bytes).and_then(|r: ProtobufBreakPanesToNewTabResponse| match r.result {
                Some(break_panes_to_new_tab_response::Result::TabId(tab_id)) => value(tab_id),
                _ => value(serde_json::Value::Null),
            })
        },
        CommandType::BreakPanesToTabWithIndex => decode(bytes).and_then(
            |r: ProtobufBreakPanesToTabWithIndexResponse| match r.result {
                Some(break_panes_to_tab_with_index_response::Result::TabId(tab_id)) => {
                    value(tab_id)
                },
                _ => value(serde_json::Value::Null),
            },
        ),
        CommandType::BreakPanesToTabWithId => {
            decode(bytes).and_then(|r: ProtobufBreakPanesToTabWithIdResponse| match r.result {
                Some(break_panes_to_tab_with_id_response::Result::TabId(tab_id)) => value(tab_id),
                _ => value(serde_json::Value::Null),
            })
        },
        CommandType::NewTiledPaneInTab => {
            decode(bytes).and_then(|r: ProtobufNewTiledPaneInTabResponse| pane_id(r.pane_id))
        },
        CommandType::OpenFile | CommandType::OpenFileFloating | CommandType::OpenFileInPlace => {
            decode(bytes).and_then(|r: ProtobufOpenFileResponse| pane_id(r.pane_id))
        },
        CommandType::OpenFileNearPlugin => {
            decode(bytes).and_then(|r: ProtobufOpenFileNearPluginResponse| pane_id(r.pane_id))
        },
        CommandType::OpenFileFloatingNearPlugin => decode(bytes)
            .and_then(|r: ProtobufOpenFileFloatingNearPluginResponse| pane_id(r.pane_id)),
        CommandType::OpenFileInPlaceOfPlugin => {
            decode(bytes).and_then(|r: ProtobufOpenFileInPlaceOfPluginResponse| pane_id(r.pane_id))
        },
        CommandType::OpenTerminal
        | CommandType::OpenTerminalFloating
        | CommandType::OpenTerminalInPlace => {
            decode(bytes).and_then(|r: ProtobufOpenTerminalResponse| pane_id(r.pane_id))
        },
        CommandType::OpenTerminalNearPlugin => {
            decode(bytes).and_then(|r: ProtobufOpenTerminalNearPluginResponse| pane_id(r.pane_id))
        },
        CommandType::OpenTerminalFloatingNearPlugin => decode(bytes)
            .and_then(|r: ProtobufOpenTerminalFloatingNearPluginResponse| pane_id(r.pane_id)),
        CommandType::OpenTerminalInPlaceOfPlugin => decode(bytes)
            .and_then(|r: ProtobufOpenTerminalInPlaceOfPluginResponse| pane_id(r.pane_id)),
        CommandType::OpenTerminalPaneInPlaceOfPaneId => decode(bytes)
            .and_then(|r: ProtobufOpenTerminalPaneInPlaceOfPaneIdResponse| pane_id(r.pane_id)),
        CommandType::OpenEditPaneInPlaceOfPaneId => decode(bytes)
            .and_then(|r: ProtobufOpenEditPaneInPlaceOfPaneIdResponse| pane_id(r.pane_id)),
        CommandType::OpenCommandPane
        | CommandType::OpenCommandPaneFloating
        | CommandType::OpenCommandPaneInPlace
        | CommandType::OpenCommandPaneBackground => {
            decode(bytes).and_then(|r: ProtobufOpenCommandPaneResponse| pane_id(r.pane_id))
        },
        CommandType::OpenCommandPaneNearPlugin => decode(bytes)
            .and_then(|r: ProtobufOpenCommandPaneNearPluginResponse| pane_id(r.pane_id)),
        CommandType::OpenCommandPaneOverlay => {
            decode(bytes).and_then(|r: ProtobufOpenCommandPaneOverlayResponse| pane_id(r.pane_id))
        },
        CommandType::OpenCommandPaneFloatingNearPlugin => decode(bytes)
            .and_then(|r: ProtobufOpenCommandPaneFloatingNearPluginResponse| pane_id(r.pane_id)),
        CommandType::OpenCommandPaneInPlaceOfPlugin => decode(bytes)
            .and_then(|r: ProtobufOpenCommandPaneInPlaceOfPluginResponse| pane_id(r.pane_id)),
        CommandType::OpenCommandPaneInPlaceOfPaneId => decode(bytes)
            .and_then(|r: ProtobufOpenCommandPaneInPlaceOfPaneIdResponse| pane_id(r.pane_id)),
        CommandType::OpenPluginPaneFloating => {
            decode(bytes).and_then(|r: ProtobufOpenPluginPaneFloatingResponse| pane_id(r.pane_id))
        },
        CommandType::OpenPluginPaneInPlaceOfPaneId => decode(bytes)
            .and_then(|r: ProtobufOpenPluginPaneInPlaceOfPaneIdResponse| pane_id(r.pane_id)),
        CommandType::OpenCommandPaneInGitCheckoutNearPlugin => {
            decode(bytes).and_then(|r: ProtobufOpenCommandPaneInGitCheckoutResponse| {
                match r.result {
                    Some(open_command_pane_in_git_checkout_response::Result::PaneId(id)) => {
                        pane_id(Some(id))
                    },
                    Some(open_command_pane_in_git_checkout_response::Result::Error(e)) => {
                        Err(anyhow!("{}", e))
                    },
                    None => Err(anyhow!("yinz answered with nothing")),
                }
            })
        },
        CommandType::SpawnAgent => {
            decode(bytes).and_then(|r: ProtobufSpawnAgentResponse| match r.result {
                Some(spawn_agent_response::Result::PaneId(id)) => {
                    value(yinz_types::agent::SpawnedAgent {
                        pane_id: PaneId::try_from(id).map_err(|e| anyhow!(e))?,
                        branch: r.branch,
                        cwd: r.cwd.map(Into::into),
                        ..Default::default()
                    })
                },
                Some(spawn_agent_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("yinz answered with nothing")),
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
                value(PaneInNewTab {
                    tab_id: r.tab_id.map(TabId::try_from).transpose()?,
                    pane_id,
                })
            })
        },
        CommandType::ReadPluginLog => {
            decode(bytes).and_then(|r: ProtobufReadPluginLogResponse| match r.result {
                Some(read_plugin_log_response::Result::Lines(lines)) => value(lines),
                Some(read_plugin_log_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("yinz answered with nothing")),
            })
        },
        CommandType::InspectConfiguration
        | CommandType::EditConfiguration
        | CommandType::ProjectConfigRead
        | CommandType::ProjectConfigWrite => {
            decode(bytes).and_then(|r: ProtobufFsReadResponse| match r.result {
                Some(fs_read_response::Result::Contents(contents)) => {
                    let snapshot: serde_json::Value = serde_json::from_slice(&contents)
                        .context("invalid project configuration reply")?;
                    value(snapshot)
                },
                Some(fs_read_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("yinz answered with nothing")),
            })
        },
        CommandType::FsRead => decode(bytes).and_then(|r: ProtobufFsReadResponse| match r.result {
            Some(fs_read_response::Result::Contents(contents)) => {
                Ok(DecodedResponse::Bytes(contents))
            },
            Some(fs_read_response::Result::Error(e)) => Err(anyhow!("{}", e)),
            None => Err(anyhow!("yinz answered with nothing")),
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
                value(entries)
            },
            Some(fs_list_response::Result::Error(e)) => Err(anyhow!("{}", e)),
            None => Err(anyhow!("yinz answered with nothing")),
        }),
        CommandType::FsMetadata => {
            decode(bytes).and_then(|r: ProtobufFsMetadataResponse| match r.result {
                Some(fs_metadata_response::Result::Metadata(metadata)) => {
                    match Option::<FileMetadata>::from(&metadata) {
                        Some(metadata) => value(metadata),
                        None => value(serde_json::Value::Null),
                    }
                },
                Some(fs_metadata_response::Result::NotFound(_)) => value(serde_json::Value::Null),
                Some(fs_metadata_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("yinz answered with nothing")),
            })
        },
        CommandType::FsWrite
        | CommandType::FsRemove
        | CommandType::FsCreateDir
        | CommandType::FsRename => {
            decode(bytes).and_then(|r: ProtobufFsMutateResponse| match r.result {
                Some(fs_mutate_response::Result::Success(_)) => value(true),
                Some(fs_mutate_response::Result::Error(e)) => Err(anyhow!("{}", e)),
                None => Err(anyhow!("yinz answered with nothing")),
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
    tab_id: TabId,
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
    tab_id: Option<TabId>,
    pane_id: Option<PaneId>,
}

fn decode<T: Message + Default>(bytes: &[u8]) -> Result<T> {
    T::decode(bytes).map_err(|e| anyhow!("yinz's answer could not be read: {}", e))
}

fn pane_id(id: Option<ProtobufPaneId>) -> Result<DecodedResponse> {
    match id.map(PaneId::try_from) {
        Some(Ok(pane_id)) => value(pane_id),
        Some(Err(e)) => Err(anyhow!("{}", e)),
        None => value(serde_json::Value::Null),
    }
}

/// Prefix recognized by the SDK to distinguish a revision conflict from other data errors.
pub(crate) const DATA_CONFLICT_MARKER: &str = "data conflict: ";

/// The replies whose only field is an optional error: absent means it worked.
fn error_or_true(error: Option<String>) -> Result<DecodedResponse> {
    match error {
        Some(e) => Err(anyhow!("{}", e)),
        None => value(true),
    }
}

/// Preserve the source location, parser message and hint in the raised guest error.
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

fn value<T: Serialize>(from: T) -> Result<DecodedResponse> {
    serde_json::to_value(from)
        .map(DecodedResponse::Json)
        .context("yinz's answer could not be serialized")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_decoder_distinguishes_missing_unknown_invalid_and_failed_replies() {
        assert_eq!(
            decode_response(CommandType::GetPaneCwd, &[])
                .unwrap()
                .unwrap(),
            DecodedResponse::Json(serde_json::Value::Null)
        );
        assert!(decode_response(CommandType::Subscribe, &[]).is_none());
        assert!(decode_response(CommandType::GetPaneCwd, &[255])
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("could not be read"));
        let failed = ProtobufFsReadResponse {
            result: Some(fs_read_response::Result::Error("denied".into())),
        }
        .encode_to_vec();
        assert_eq!(
            decode_response(CommandType::FsRead, &failed)
                .unwrap()
                .unwrap_err()
                .to_string(),
            "denied"
        );
    }

    #[test]
    fn remote_git_probe_keeps_large_metadata_and_config_compact() {
        let probe = PaneGitProbe {
            host: "remote".into(),
            cwd: "/project".into(),
            exit_code: Some(128),
            metadata: vec![255; 200_001],
            config: vec![0; 200_001],
        };
        let response = ProtobufGetPaneGitProbeResponse::from(
            yinz_types::data::GetPaneGitProbeResponse::Ok(probe.clone()),
        )
        .encode_to_vec();
        let DecodedResponse::Json(value) = decode_response(CommandType::GetPaneGitProbe, &response)
            .unwrap()
            .unwrap()
        else {
            panic!("expected probe object")
        };
        assert_eq!(value["host"], "remote");
        assert_eq!(value["cwd"], "/project");
        assert_eq!(value["exit_code"], 128);
        for (field, bytes) in [("metadata", probe.metadata), ("config", probe.config)] {
            assert!(
                value[field].is_object(),
                "probe {field} expanded into a JSON array"
            );
            assert_eq!(yinz_steel::binary::decode(&value[field]).unwrap(), bytes);
        }
    }
}
