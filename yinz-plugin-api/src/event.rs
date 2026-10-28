pub use super::generated_api::api::event::CellPixelSize as ProtobufCellPixelSize;
pub use super::generated_api::api::{
    action::{Action as ProtobufAction, Position as ProtobufPosition},
    event::{
        event::Payload as ProtobufEventPayload, hook_payload,
        layout_parsing_error::ErrorType as ProtobufLayoutParsingErrorType,
        pane_scrollback_response, ActionCompletePayload as ProtobufActionCompletePayload,
        ActivePaneScrollPayload as ProtobufActivePaneScrollPayload,
        AgentEventPayload as ProtobufAgentEventPayload,
        AgentExitHookPayload as ProtobufAgentExitHookPayload,
        AgentHookPayload as ProtobufAgentHookPayload, AgentInfo as ProtobufAgentInfo,
        AgentSource as ProtobufAgentSource, AgentState as ProtobufAgentState,
        AvailableLayoutInfoPayload as ProtobufAvailableLayoutInfoPayload,
        ClientInfo as ProtobufClientInfo, ClientPaneHistory as ProtobufClientPaneHistory,
        ClientTabHistory as ProtobufClientTabHistory,
        CommandChangedPayload as ProtobufCommandChangedPayload, ContextItem as ProtobufContextItem,
        ContextMenuContext as ProtobufContextMenuContext,
        ContextMenuItem as ProtobufContextMenuItem,
        ContextMenuItemSelectedPayload as ProtobufContextMenuItemSelectedPayload,
        ContextMenuLink as ProtobufContextMenuLink,
        ContextMenuRequestedPayload as ProtobufContextMenuRequestedPayload,
        CopyDestination as ProtobufCopyDestination, CwdChangedPayload as ProtobufCwdChangedPayload,
        Event as ProtobufEvent, EventNameList as ProtobufEventNameList,
        EventType as ProtobufEventType, FileMetadata as ProtobufFileMetadata,
        HintTextPayload as ProtobufHintTextPayload, HookPayload as ProtobufHookPayload,
        HostTerminalThemeChangedPayload as ProtobufHostTerminalThemeChangedPayload,
        HostTerminalThemeIndication as ProtobufHostTerminalThemeIndication,
        InputModeKeybinds as ProtobufInputModeKeybinds, KdlError as ProtobufKdlError,
        KdlErrorVariant as ProtobufKdlErrorVariant, KeyBind as ProtobufKeyBind,
        LayoutInfo as ProtobufLayoutInfo, LayoutMetadata as ProtobufLayoutMetadata,
        LayoutParsingError as ProtobufLayoutParsingError,
        LayoutWithError as ProtobufLayoutWithError, ModeUpdatePayload as ProtobufModeUpdatePayload,
        PaneContents as ProtobufPaneContents, PaneContentsEntry as ProtobufPaneContentsEntry,
        PaneExitHookPayload as ProtobufPaneExitHookPayload,
        PaneFrameStyle as ProtobufPaneFrameStyle, PaneId as ProtobufPaneId,
        PaneInfo as ProtobufPaneInfo, PaneManifest as ProtobufPaneManifest,
        PaneMetadata as ProtobufPaneMetadata,
        PaneRenderReportPayload as ProtobufPaneRenderReportPayload,
        PaneScrollbackResponse as ProtobufPaneScrollbackResponse, PaneType as ProtobufPaneType,
        PluginConfigurationChangedPayload as ProtobufPluginConfigurationChangedPayload,
        PluginDataChangedPayload as ProtobufPluginDataChangedPayload,
        PluginInfo as ProtobufPluginInfo, ResurrectableSession as ProtobufResurrectableSession,
        SelectedText as ProtobufSelectedText, SessionManifest as ProtobufSessionManifest,
        SessionStartHookPayload as ProtobufSessionStartHookPayload,
        StyledText as ProtobufStyledText, StyledTextIndices as ProtobufStyledTextIndices,
        SyntaxError as ProtobufSyntaxError, TabInfo as ProtobufTabInfo,
        TabMetadata as ProtobufTabMetadata, UserActionPayload as ProtobufUserActionPayload, *,
    },
    input_mode::InputMode as ProtobufInputMode,
    key::{key::KeyModifier as ProtobufKeyModifier, Key as ProtobufKey},
    style::Style as ProtobufStyle,
};
use crate::plugin_data::ProtobufRows as ProtobufDataRows;
use yinz_types::internal_ids::ContextMenuItemId;
use yinz_types::internal_ids::ContextMenuRequestId;
use yinz_types::internal_ids::DataWatchId;
use yinz_types::internal_ids::SessionName;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabPosition;
use yinz_types::keys::{KeyModifier, KeyWithModifier};
use yinz_types::multiplayer::ColorSlot;
use yinz_types::position::{CellCoordinates, Column, ColumnCount, Row, RowCount};
use yinz_types::style::{HostTerminalThemeMode, Style};
#[expect(
    hidden_glob_reexports,
    reason = "native event types intentionally shadow protobuf types from the public glob"
)]
use {
    yinz_types::agent::AgentInfo, yinz_types::agent::AgentSource, yinz_types::agent::AgentState,
    yinz_types::command::PaneTarget, yinz_types::data::ClientId, yinz_types::data::ClientInfo,
    yinz_types::data::ContextMenuContext, yinz_types::data::ContextMenuItem,
    yinz_types::data::ContextMenuLink, yinz_types::data::CopyDestination,
    yinz_types::data::DataRows, yinz_types::data::Event, yinz_types::data::EventType,
    yinz_types::data::FileMetadata, yinz_types::data::InputMode, yinz_types::data::KeybindsVec,
    yinz_types::data::LayoutInfo, yinz_types::data::LayoutMetadata, yinz_types::data::ModeInfo,
    yinz_types::data::Mouse, yinz_types::data::PaneContents, yinz_types::data::PaneId,
    yinz_types::data::PaneInfo, yinz_types::data::PaneManifest, yinz_types::data::PaneMetadata,
    yinz_types::data::PaneScrollbackResponse, yinz_types::data::PermissionStatus,
    yinz_types::data::PluginCapabilities, yinz_types::data::PluginInfo,
    yinz_types::data::SelectedText, yinz_types::data::SessionInfo, yinz_types::data::StyledText,
    yinz_types::data::TabInfo, yinz_types::data::TabMetadata,
};

use yinz_types::input::actions::Action;
use yinz_types::input::hooks::HookPoint;

use crate::command::ProtobufPaneTarget;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::convert::TryFrom;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

impl TryFrom<ProtobufEvent> for Event {
    type Error = &'static str;
    fn try_from(protobuf_event: ProtobufEvent) -> Result<Self, &'static str> {
        match ProtobufEventType::try_from(protobuf_event.name) {
            Ok(ProtobufEventType::ModeUpdate) => match protobuf_event.payload {
                Some(ProtobufEventPayload::ModeUpdatePayload(protobuf_mode_update_payload)) => {
                    let mode_info: ModeInfo = protobuf_mode_update_payload.try_into()?;
                    Ok(Event::ModeUpdate(mode_info))
                },
                _ => Err("Malformed payload for the ModeUpdate Event"),
            },
            Ok(ProtobufEventType::TabUpdate) => match protobuf_event.payload {
                Some(ProtobufEventPayload::TabUpdatePayload(protobuf_tab_info_payload)) => {
                    let mut tab_infos: Vec<TabInfo> = vec![];
                    for protobuf_tab_info in protobuf_tab_info_payload.tab_info {
                        tab_infos.push(TabInfo::try_from(protobuf_tab_info)?);
                    }
                    Ok(Event::TabUpdate(tab_infos))
                },
                _ => Err("Malformed payload for the TabUpdate Event"),
            },
            Ok(ProtobufEventType::PaneUpdate) => match protobuf_event.payload {
                Some(ProtobufEventPayload::PaneUpdatePayload(protobuf_pane_update_payload)) => {
                    let mut pane_manifest: HashMap<TabPosition, Vec<PaneInfo>> = HashMap::new();
                    for protobuf_pane_manifest in protobuf_pane_update_payload.pane_manifest {
                        let tab_index = TabPosition::new(protobuf_pane_manifest.tab_index as usize);
                        let mut panes = vec![];
                        for protobuf_pane_info in protobuf_pane_manifest.panes {
                            panes.push(protobuf_pane_info.try_into()?);
                        }
                        if pane_manifest.contains_key(&tab_index) {
                            return Err("Duplicate tab definition in pane manifest");
                        }
                        pane_manifest.insert(tab_index, panes);
                    }
                    Ok(Event::PaneUpdate(PaneManifest {
                        panes: pane_manifest,
                    }))
                },
                _ => Err("Malformed payload for the PaneUpdate Event"),
            },
            Ok(ProtobufEventType::Key) => match protobuf_event.payload {
                Some(ProtobufEventPayload::KeyPayload(protobuf_key)) => {
                    Ok(Event::Key(protobuf_key.try_into()?))
                },
                _ => Err("Malformed payload for the Key Event"),
            },
            Ok(ProtobufEventType::Mouse) => match protobuf_event.payload {
                Some(ProtobufEventPayload::MouseEventPayload(protobuf_mouse)) => {
                    Ok(Event::Mouse(protobuf_mouse.try_into()?))
                },
                _ => Err("Malformed payload for the Mouse Event"),
            },
            Ok(ProtobufEventType::Timer) => match protobuf_event.payload {
                Some(ProtobufEventPayload::TimerPayload(seconds)) => Ok(Event::Timer(
                    yinz_types::units::FractionalSeconds::new(seconds as f64),
                )),
                _ => Err("Malformed payload for the Timer Event"),
            },
            Ok(ProtobufEventType::CopyToClipboard) => match protobuf_event.payload {
                Some(ProtobufEventPayload::CopyToClipboardPayload(copy_to_clipboard)) => {
                    let protobuf_copy_to_clipboard =
                        ProtobufCopyDestination::try_from(copy_to_clipboard)
                            .map_err(|_| "Malformed copy to clipboard payload")?;
                    Ok(Event::CopyToClipboard(
                        protobuf_copy_to_clipboard.try_into()?,
                    ))
                },
                _ => Err("Malformed payload for the Copy To Clipboard Event"),
            },
            Ok(ProtobufEventType::SystemClipboardFailure) => match protobuf_event.payload {
                None => Ok(Event::SystemClipboardFailure),
                _ => Err("Malformed payload for the system clipboard failure Event"),
            },
            Ok(ProtobufEventType::InputReceived) => match protobuf_event.payload {
                None => Ok(Event::InputReceived),
                _ => Err("Malformed payload for the input received Event"),
            },
            Ok(ProtobufEventType::Visible) => match protobuf_event.payload {
                Some(ProtobufEventPayload::VisiblePayload(is_visible)) => {
                    Ok(Event::Visible(is_visible))
                },
                _ => Err("Malformed payload for the visible Event"),
            },
            Ok(ProtobufEventType::CustomMessage) => match protobuf_event.payload {
                Some(ProtobufEventPayload::CustomMessagePayload(custom_message_payload)) => {
                    Ok(Event::CustomMessage(
                        custom_message_payload.message_name,
                        custom_message_payload.payload,
                    ))
                },
                _ => Err("Malformed payload for the custom message Event"),
            },
            Ok(ProtobufEventType::FileSystemCreate) => match protobuf_event.payload {
                Some(ProtobufEventPayload::FileListPayload(file_list_payload)) => {
                    let file_paths = file_list_payload
                        .paths
                        .iter()
                        .zip(file_list_payload.paths_metadata.iter())
                        .map(|(p, m)| (PathBuf::from(p), m.into()))
                        .collect();
                    Ok(Event::FileSystemCreate(file_paths))
                },
                _ => Err("Malformed payload for the file system create Event"),
            },
            Ok(ProtobufEventType::FileSystemRead) => match protobuf_event.payload {
                Some(ProtobufEventPayload::FileListPayload(file_list_payload)) => {
                    let file_paths = file_list_payload
                        .paths
                        .iter()
                        .zip(file_list_payload.paths_metadata.iter())
                        .map(|(p, m)| (PathBuf::from(p), m.into()))
                        .collect();
                    Ok(Event::FileSystemRead(file_paths))
                },
                _ => Err("Malformed payload for the file system read Event"),
            },
            Ok(ProtobufEventType::FileSystemUpdate) => match protobuf_event.payload {
                Some(ProtobufEventPayload::FileListPayload(file_list_payload)) => {
                    let file_paths = file_list_payload
                        .paths
                        .iter()
                        .zip(file_list_payload.paths_metadata.iter())
                        .map(|(p, m)| (PathBuf::from(p), m.into()))
                        .collect();
                    Ok(Event::FileSystemUpdate(file_paths))
                },
                _ => Err("Malformed payload for the file system update Event"),
            },
            Ok(ProtobufEventType::FileSystemDelete) => match protobuf_event.payload {
                Some(ProtobufEventPayload::FileListPayload(file_list_payload)) => {
                    let file_paths = file_list_payload
                        .paths
                        .iter()
                        .zip(file_list_payload.paths_metadata.iter())
                        .map(|(p, m)| (PathBuf::from(p), m.into()))
                        .collect();
                    Ok(Event::FileSystemDelete(file_paths))
                },
                _ => Err("Malformed payload for the file system delete Event"),
            },
            Ok(ProtobufEventType::PermissionRequestResult) => match protobuf_event.payload {
                Some(ProtobufEventPayload::PermissionRequestResultPayload(payload)) => {
                    if payload.granted {
                        Ok(Event::PermissionRequestResult(PermissionStatus::Granted))
                    } else {
                        Ok(Event::PermissionRequestResult(PermissionStatus::Denied))
                    }
                },
                _ => Err("Malformed payload for the file system delete Event"),
            },
            Ok(ProtobufEventType::SessionUpdate) => match protobuf_event.payload {
                Some(ProtobufEventPayload::SessionUpdatePayload(
                    protobuf_session_update_payload,
                )) => {
                    let mut session_infos: Vec<SessionInfo> = vec![];
                    let mut resurrectable_sessions: Vec<(SessionName, Duration)> = vec![];
                    for protobuf_session_info in protobuf_session_update_payload.session_manifests {
                        session_infos.push(SessionInfo::try_from(protobuf_session_info)?);
                    }
                    for protobuf_resurrectable_session in
                        protobuf_session_update_payload.resurrectable_sessions
                    {
                        resurrectable_sessions.push(protobuf_resurrectable_session.into());
                    }
                    Ok(Event::SessionUpdate(session_infos, resurrectable_sessions))
                },
                _ => Err("Malformed payload for the SessionUpdate Event"),
            },
            Ok(ProtobufEventType::PaneTabsUpdate) => match protobuf_event.payload {
                Some(ProtobufEventPayload::PaneTabsUpdatePayload(json)) => {
                    serde_json::from_str(&json)
                        .map(Event::PaneTabsUpdate)
                        .map_err(|_| "Malformed PaneTabsUpdate payload")
                },
                _ => Err("Malformed PaneTabsUpdate payload"),
            },
            Ok(ProtobufEventType::PaneOutput) => match protobuf_event.payload {
                Some(ProtobufEventPayload::PaneOutputPayload(p)) => Ok(Event::PaneOutput {
                    pane_id: p.pane_id,
                    data: p.data,
                    context: p.context.into_iter().map(|c| (c.name, c.value)).collect(),
                }),
                _ => Err("Malformed PaneOutput payload"),
            },
            Ok(ProtobufEventType::RunCommandOutput) => match protobuf_event.payload {
                Some(ProtobufEventPayload::RunCommandOutputPayload(p)) => {
                    Ok(Event::RunCommandOutput {
                        data: p.data,
                        stderr: p.stderr,
                        context: p.context.into_iter().map(|c| (c.name, c.value)).collect(),
                    })
                },
                _ => Err("Malformed RunCommandOutput payload"),
            },
            Ok(ProtobufEventType::RunCommandResult) => match protobuf_event.payload {
                Some(ProtobufEventPayload::RunCommandResultPayload(run_command_result_payload)) => {
                    Ok(Event::RunCommandResult(
                        run_command_result_payload.exit_code,
                        run_command_result_payload.stdout,
                        run_command_result_payload.stderr,
                        run_command_result_payload
                            .context
                            .into_iter()
                            .map(|c_i| (c_i.name, c_i.value))
                            .collect(),
                    ))
                },
                _ => Err("Malformed payload for the RunCommandResult Event"),
            },
            Ok(ProtobufEventType::WebRequestResult) => match protobuf_event.payload {
                Some(ProtobufEventPayload::WebRequestResultPayload(web_request_result_payload)) => {
                    Ok(Event::WebRequestResult(
                        web_request_result_payload.status as u16,
                        web_request_result_payload
                            .headers
                            .into_iter()
                            .map(|h| (h.name, h.value))
                            .collect(),
                        web_request_result_payload.body,
                        web_request_result_payload
                            .context
                            .into_iter()
                            .map(|c_i| (c_i.name, c_i.value))
                            .collect(),
                    ))
                },
                _ => Err("Malformed payload for the WebRequestResult Event"),
            },
            Ok(ProtobufEventType::WebRequestStreamChunk) => match protobuf_event.payload {
                Some(ProtobufEventPayload::WebRequestStreamChunkPayload(payload)) => {
                    Ok(Event::WebRequestStreamChunk {
                        stream_id: payload.stream_id,
                        chunk: payload.chunk,
                        done: payload.done,
                        error: payload.error,
                    })
                },
                _ => Err("Malformed payload for the WebRequestStreamChunk Event"),
            },
            Ok(ProtobufEventType::CommandPaneOpened) => match protobuf_event.payload {
                Some(ProtobufEventPayload::CommandPaneOpenedPayload(
                    command_pane_opened_payload,
                )) => Ok(Event::CommandPaneOpened(
                    yinz_types::internal_ids::TerminalId::new(
                        command_pane_opened_payload.terminal_pane_id,
                    ),
                    command_pane_opened_payload
                        .context
                        .into_iter()
                        .map(|c_i| (c_i.name, c_i.value))
                        .collect(),
                )),
                _ => Err("Malformed payload for the CommandPaneOpened Event"),
            },
            Ok(ProtobufEventType::CommandPaneExited) => match protobuf_event.payload {
                Some(ProtobufEventPayload::CommandPaneExitedPayload(
                    command_pane_exited_payload,
                )) => Ok(Event::CommandPaneExited(
                    yinz_types::internal_ids::TerminalId::new(
                        command_pane_exited_payload.terminal_pane_id,
                    ),
                    command_pane_exited_payload.exit_code,
                    command_pane_exited_payload
                        .context
                        .into_iter()
                        .map(|c_i| (c_i.name, c_i.value))
                        .collect(),
                )),
                _ => Err("Malformed payload for the CommandPaneExited Event"),
            },
            Ok(ProtobufEventType::PaneClosed) => match protobuf_event.payload {
                Some(ProtobufEventPayload::PaneClosedPayload(pane_closed_payload)) => {
                    let pane_id = pane_closed_payload
                        .pane_id
                        .ok_or("Malformed payload for the PaneClosed Event")?;
                    Ok(Event::PaneClosed(PaneId::try_from(pane_id)?))
                },
                _ => Err("Malformed payload for the PaneClosed Event"),
            },
            Ok(ProtobufEventType::EditPaneOpened) => match protobuf_event.payload {
                Some(ProtobufEventPayload::EditPaneOpenedPayload(command_pane_opened_payload)) => {
                    Ok(Event::EditPaneOpened(
                        yinz_types::internal_ids::TerminalId::new(
                            command_pane_opened_payload.terminal_pane_id,
                        ),
                        command_pane_opened_payload
                            .context
                            .into_iter()
                            .map(|c_i| (c_i.name, c_i.value))
                            .collect(),
                    ))
                },
                _ => Err("Malformed payload for the EditPaneOpened Event"),
            },
            Ok(ProtobufEventType::EditPaneExited) => match protobuf_event.payload {
                Some(ProtobufEventPayload::EditPaneExitedPayload(command_pane_exited_payload)) => {
                    Ok(Event::EditPaneExited(
                        yinz_types::internal_ids::TerminalId::new(
                            command_pane_exited_payload.terminal_pane_id,
                        ),
                        command_pane_exited_payload.exit_code,
                        command_pane_exited_payload
                            .context
                            .into_iter()
                            .map(|c_i| (c_i.name, c_i.value))
                            .collect(),
                    ))
                },
                _ => Err("Malformed payload for the EditPaneExited Event"),
            },
            Ok(ProtobufEventType::CommandPaneReRun) => match protobuf_event.payload {
                Some(ProtobufEventPayload::CommandPaneRerunPayload(command_pane_rerun_payload)) => {
                    Ok(Event::CommandPaneReRun(
                        yinz_types::internal_ids::TerminalId::new(
                            command_pane_rerun_payload.terminal_pane_id,
                        ),
                        command_pane_rerun_payload
                            .context
                            .into_iter()
                            .map(|c_i| (c_i.name, c_i.value))
                            .collect(),
                    ))
                },
                _ => Err("Malformed payload for the CommandPaneReRun Event"),
            },
            Ok(ProtobufEventType::FailedToWriteConfigToDisk) => match protobuf_event.payload {
                Some(ProtobufEventPayload::FailedToWriteConfigToDiskPayload(
                    failed_to_write_configuration_payload,
                )) => Ok(Event::FailedToWriteConfigToDisk {
                    file_path: failed_to_write_configuration_payload.file_path,
                    reason: failed_to_write_configuration_payload.reason,
                }),
                _ => Err("Malformed payload for the FailedToWriteConfigToDisk Event"),
            },
            Ok(ProtobufEventType::ListClients) => match protobuf_event.payload {
                Some(ProtobufEventPayload::ListClientsPayload(mut list_clients_payload)) => {
                    Ok(Event::ListClients(
                        list_clients_payload
                            .client_info
                            .drain(..)
                            .filter_map(|c| c.try_into().ok())
                            .collect(),
                    ))
                },
                _ => Err("Malformed payload for the FailedToWriteConfigToDisk Event"),
            },
            Ok(ProtobufEventType::HostFolderChanged) => match protobuf_event.payload {
                Some(ProtobufEventPayload::HostFolderChangedPayload(
                    host_folder_changed_payload,
                )) => Ok(Event::HostFolderChanged(PathBuf::from(
                    host_folder_changed_payload.new_host_folder_path,
                ))),
                _ => Err("Malformed payload for the HostFolderChanged Event"),
            },
            Ok(ProtobufEventType::FailedToChangeHostFolder) => match protobuf_event.payload {
                Some(ProtobufEventPayload::FailedToChangeHostFolderPayload(
                    failed_to_change_host_folder_payload,
                )) => Ok(Event::FailedToChangeHostFolder(
                    failed_to_change_host_folder_payload.error_message,
                )),
                _ => Err("Malformed payload for the FailedToChangeHostFolder Event"),
            },
            Ok(ProtobufEventType::PastedText) => match protobuf_event.payload {
                Some(ProtobufEventPayload::PastedTextPayload(pasted_text_payload)) => {
                    Ok(Event::PastedText(pasted_text_payload.pasted_text))
                },
                _ => Err("Malformed payload for the PastedText Event"),
            },
            Ok(ProtobufEventType::ConfigWasWrittenToDisk) => match protobuf_event.payload {
                None => Ok(Event::ConfigWasWrittenToDisk),
                _ => Err("Malformed payload for the ConfigWasWrittenToDisk Event"),
            },
            Ok(ProtobufEventType::CellPixelSizeChanged) => match protobuf_event.payload {
                Some(ProtobufEventPayload::CellPixelSizeChangedPayload(payload)) => Ok(
                    Event::CellPixelSizeChanged(payload.size.map(TryInto::try_into).transpose()?),
                ),
                _ => Err("Malformed payload for CellPixelSizeChanged Event"),
            },
            Ok(ProtobufEventType::BeforeClose) => match protobuf_event.payload {
                None => Ok(Event::BeforeClose),
                _ => Err("Malformed payload for the BeforeClose Event"),
            },
            Ok(ProtobufEventType::InterceptedKeyPress) => match protobuf_event.payload {
                Some(ProtobufEventPayload::KeyPayload(protobuf_key)) => {
                    Ok(Event::InterceptedKeyPress(protobuf_key.try_into()?))
                },
                _ => Err("Malformed payload for the InterceptedKeyPress Event"),
            },
            Ok(ProtobufEventType::PaneRenderReport) => match protobuf_event.payload {
                Some(ProtobufEventPayload::PaneRenderReportPayload(protobuf_payload)) => {
                    Ok(Event::PaneRenderReport(protobuf_payload.try_into()?))
                },
                _ => Err("Malformed payload for the PaneRenderReport Event"),
            },
            Ok(ProtobufEventType::UserAction) => match protobuf_event.payload {
                Some(ProtobufEventPayload::UserActionPayload(protobuf_payload)) => {
                    let action: Action = protobuf_payload
                        .action
                        .ok_or("Missing action in UserAction payload")?
                        .try_into()
                        .map_err(|_| "Failed to convert Action in UserAction payload")?;
                    let client_id = ClientId::new(protobuf_payload.client_id);
                    let terminal_id = protobuf_payload
                        .terminal_id
                        .map(yinz_types::internal_ids::TerminalId::new);
                    let cli_client_id = protobuf_payload.cli_client_id.map(ClientId::new);
                    Ok(Event::UserAction(
                        action,
                        client_id,
                        terminal_id,
                        cli_client_id,
                    ))
                },
                _ => Err("Malformed payload for the UserAction Event"),
            },
            Ok(ProtobufEventType::ActionComplete) => match protobuf_event.payload {
                Some(ProtobufEventPayload::ActionCompletePayload(protobuf_payload)) => {
                    let action: Action = protobuf_payload
                        .action
                        .ok_or("Missing action in ActionComplete payload")?
                        .try_into()
                        .map_err(|_| "Failed to convert Action in ActionComplete payload")?;
                    let pane_id = protobuf_payload
                        .pane_id
                        .map(|id| id.try_into())
                        .transpose()
                        .map_err(|_| "Failed to convert PaneId in ActionComplete payload")?;
                    let context: BTreeMap<String, String> = protobuf_payload
                        .context
                        .into_iter()
                        .map(|item| (item.name, item.value))
                        .collect();
                    Ok(Event::ActionComplete(action, pane_id, context))
                },
                _ => Err("Malformed payload for the ActionComplete Event"),
            },
            Ok(ProtobufEventType::CwdChanged) => match protobuf_event.payload {
                Some(ProtobufEventPayload::CwdChangedPayload(protobuf_payload)) => {
                    let pane_id: PaneId = protobuf_payload
                        .pane_id
                        .ok_or("Missing pane_id in CwdChanged payload")?
                        .try_into()
                        .map_err(|_| "Failed to convert PaneId in CwdChanged payload")?;
                    let new_cwd = PathBuf::from(protobuf_payload.new_cwd);
                    let focused_client_ids: Vec<ClientId> = protobuf_payload
                        .focused_client_ids
                        .into_iter()
                        .map(ClientId::new)
                        .collect();
                    Ok(Event::CwdChanged(pane_id, new_cwd, focused_client_ids))
                },
                _ => Err("Malformed payload for the CwdChanged Event"),
            },
            Ok(ProtobufEventType::CommandChanged) => match protobuf_event.payload {
                Some(ProtobufEventPayload::CommandChangedPayload(p)) => {
                    let pane_id: PaneId = p
                        .pane_id
                        .ok_or("Missing pane_id in CommandChanged payload")?
                        .try_into()
                        .map_err(|_| "Failed to convert PaneId in CommandChanged payload")?;
                    let focused_client_ids: Vec<ClientId> = p
                        .focused_client_ids
                        .into_iter()
                        .map(ClientId::new)
                        .collect();
                    Ok(Event::CommandChanged(
                        pane_id,
                        p.command,
                        p.is_foreground,
                        focused_client_ids,
                    ))
                },
                _ => Err("Malformed payload for the CommandChanged Event"),
            },
            Ok(ProtobufEventType::AvailableLayoutInfo) => match protobuf_event.payload {
                Some(ProtobufEventPayload::AvailableLayoutInfoPayload(
                    available_layout_info_payload,
                )) => {
                    let mut available_layouts: Vec<LayoutInfo> = vec![];
                    let mut layouts_with_errors: Vec<yinz_types::data::LayoutWithError> = vec![];

                    for protobuf_layout_info in available_layout_info_payload.available_layouts {
                        available_layouts.push(LayoutInfo::try_from(protobuf_layout_info)?);
                    }

                    for protobuf_error in available_layout_info_payload.layouts_with_errors {
                        layouts_with_errors
                            .push(yinz_types::data::LayoutWithError::try_from(protobuf_error)?);
                    }

                    Ok(Event::AvailableLayoutInfo(
                        available_layouts,
                        layouts_with_errors,
                    ))
                },
                _ => Err("Malformed payload for the AvailableLayoutInfo Event"),
            },
            Ok(ProtobufEventType::PluginConfigurationChanged) => match protobuf_event.payload {
                Some(ProtobufEventPayload::PluginConfigurationChangedPayload(payload)) => {
                    let configuration = payload
                        .configuration
                        .into_iter()
                        .map(|item| (item.name, item.value))
                        .collect();
                    Ok(Event::PluginConfigurationChanged(configuration))
                },
                _ => Err("Malformed payload for PluginConfigurationChanged Event"),
            },
            Ok(ProtobufEventType::HighlightClicked) => match protobuf_event.payload {
                Some(ProtobufEventPayload::HighlightClickedPayload(p)) => {
                    let pane_id = p
                        .pane_id
                        .ok_or("Missing pane_id in HighlightClicked")?
                        .try_into()?;
                    let context = p
                        .context
                        .into_iter()
                        .map(|item| (item.name, item.value))
                        .collect();
                    Ok(Event::HighlightClicked {
                        pane_id,
                        pattern: p.pattern,
                        matched_string: p.matched_string,
                        context,
                    })
                },
                _ => Err("Malformed payload for HighlightClicked Event"),
            },
            Ok(ProtobufEventType::ContextMenuRequested) => match protobuf_event.payload {
                Some(ProtobufEventPayload::ContextMenuRequestedPayload(p)) => {
                    let context = p
                        .context
                        .ok_or("Missing context in ContextMenuRequested")?
                        .try_into()?;
                    Ok(Event::ContextMenuRequested(Box::new(context)))
                },
                _ => Err("Malformed payload for ContextMenuRequested Event"),
            },
            Ok(ProtobufEventType::ContextMenuItemSelected) => match protobuf_event.payload {
                Some(ProtobufEventPayload::ContextMenuItemSelectedPayload(p)) => {
                    let context = p
                        .context
                        .ok_or("Missing context in ContextMenuItemSelected")?
                        .try_into()?;
                    Ok(Event::ContextMenuItemSelected {
                        label: p.label,
                        item_context: p
                            .item_context
                            .into_iter()
                            .map(|item| (item.name, item.value))
                            .collect(),
                        context: Box::new(context),
                    })
                },
                _ => Err("Malformed payload for ContextMenuItemSelected Event"),
            },
            Ok(ProtobufEventType::InitialKeybinds) => match protobuf_event.payload {
                Some(ProtobufEventPayload::InitialKeybindsPayload(p)) => {
                    let keybinds = decode_keybinds(p.keybinds);
                    Ok(Event::InitialKeybinds(keybinds))
                },
                _ => Err("Malformed payload for InitialKeybinds Event"),
            },
            Ok(ProtobufEventType::HostTerminalThemeChanged) => match protobuf_event.payload {
                Some(ProtobufEventPayload::HostTerminalThemeChangedPayload(p)) => {
                    let mode = ProtobufHostTerminalThemeIndication::try_from(p.mode)
                        .map_err(|_| "Unknown HostTerminalThemeIndication")?;
                    Ok(Event::HostTerminalThemeChanged(mode.into()))
                },
                _ => Err("Malformed payload for HostTerminalThemeChanged Event"),
            },
            Ok(ProtobufEventType::HintText) => match protobuf_event.payload {
                Some(ProtobufEventPayload::HintTextPayload(p)) => {
                    let mut hint_text = BTreeMap::new();
                    for (width, styled_text) in p.hint_text {
                        hint_text.insert(width as usize, StyledText::from(styled_text));
                    }
                    Ok(Event::HintText(hint_text))
                },
                _ => Err("Malformed payload for HintText Event"),
            },
            Ok(ProtobufEventType::ActivePaneScroll) => match protobuf_event.payload {
                Some(ProtobufEventPayload::ActivePaneScrollPayload(p)) => {
                    let scroll = match (p.position, p.length) {
                        (Some(position), Some(length)) => {
                            Some((position as usize, length as usize))
                        },
                        _ => None,
                    };
                    Ok(Event::ActivePaneScroll(scroll))
                },
                _ => Err("Malformed payload for ActivePaneScroll Event"),
            },
            Ok(ProtobufEventType::PluginDataChanged) => match protobuf_event.payload {
                Some(ProtobufEventPayload::PluginDataChangedPayload(p)) => {
                    Ok(Event::PluginDataChanged {
                        watch_id: DataWatchId::new(p.watch_id),
                        // An absent `rows` is an empty result set, which is a legitimate thing for a
                        // watch to be told about — its last matching row having been deleted.
                        rows: p.rows.map(DataRows::from).unwrap_or_default(),
                        truncated: p.truncated,
                    })
                },
                _ => Err("Malformed payload for PluginDataChanged Event"),
            },
            Ok(ProtobufEventType::NotificationReceived) => match protobuf_event.payload {
                Some(ProtobufEventPayload::NotificationReceivedPayload(json)) => {
                    serde_json::from_str(&json)
                        .map(Event::NotificationReceived)
                        .map_err(|_| "Malformed NotificationReceived payload")
                },
                _ => Err("Missing NotificationReceived payload"),
            },
            Ok(ProtobufEventType::NotificationRequested) => match protobuf_event.payload {
                Some(ProtobufEventPayload::NotificationRequestedPayload(json)) => {
                    serde_json::from_str(&json)
                        .map(Event::NotificationRequested)
                        .map_err(|_| "Malformed NotificationRequested payload")
                },
                _ => Err("Missing NotificationRequested payload"),
            },
            Ok(ProtobufEventType::NotificationResolved) => match protobuf_event.payload {
                Some(ProtobufEventPayload::NotificationResolvedPayload(json)) => {
                    serde_json::from_str(&json)
                        .map(Event::NotificationResolved)
                        .map_err(|_| "Malformed NotificationResolved payload")
                },
                _ => Err("Missing NotificationResolved payload"),
            },
            Ok(ProtobufEventType::Hook) => match protobuf_event.payload {
                Some(ProtobufEventPayload::HookPayload(p)) => {
                    let point = HookPoint::from_str(&p.point)
                        .map_err(|_| "Malformed hook point for Hook Event")?;
                    let payload = match p.payload {
                        Some(hook_payload::Payload::AgentExit(exit)) => {
                            yinz_types::data::HookPayload::AgentExit(
                                exit.configuration.into_iter().collect(),
                            )
                        },
                        Some(hook_payload::Payload::Agent(agent)) => {
                            yinz_types::data::HookPayload::Agent {
                                pane_id: yinz_types::internal_ids::TerminalId::new(agent.pane_id),
                                agent: agent
                                    .agent
                                    .ok_or("Malformed agent for Hook Event")?
                                    .try_into()?,
                                previous_state: agent
                                    .previous_state
                                    .as_deref()
                                    .map(|state| {
                                        state
                                            .parse::<AgentState>()
                                            .map_err(|_| "Malformed previous state for Hook Event")
                                    })
                                    .transpose()?,
                                command: agent.command,
                            }
                        },
                        Some(hook_payload::Payload::PaneExit(exit)) => {
                            yinz_types::data::HookPayload::PaneExit {
                                pane_id: yinz_types::internal_ids::TerminalId::new(exit.pane_id),
                                exit_code: exit.exit_code,
                                cwd: exit.cwd.map(PathBuf::from),
                                command: exit.command,
                                held: exit.held,
                            }
                        },
                        Some(hook_payload::Payload::SessionStart(start)) => {
                            yinz_types::data::HookPayload::SessionStart {
                                session_name: start.session_name.into(),
                                client_id: ClientId::new(start.client_id),
                            }
                        },
                        None => return Err("Malformed payload for Hook Event"),
                    };
                    Ok(Event::Hook { point, payload })
                },
                _ => Err("Malformed payload for Hook Event"),
            },
            Ok(ProtobufEventType::PickResult) => match protobuf_event.payload {
                Some(ProtobufEventPayload::PickResultPayload(json)) => serde_json::from_str(&json)
                    .map(Event::PickResult)
                    .map_err(|_| "Malformed PickResult"),
                _ => Err("Malformed PickResult payload"),
            },
            Ok(ProtobufEventType::ProjectProvidersChanged) => match protobuf_event.payload {
                Some(ProtobufEventPayload::ProjectProvidersChangedPayload(json)) => {
                    serde_json::from_str(&json)
                        .map(Event::ProjectProvidersChanged)
                        .map_err(|_| "Malformed agent provider snapshot")
                },
                _ => Err("Malformed payload for ProjectProvidersChanged Event"),
            },
            Ok(ProtobufEventType::AgentProvidersChanged) => match protobuf_event.payload {
                Some(ProtobufEventPayload::AgentProvidersChangedPayload(json)) => {
                    serde_json::from_str(&json)
                        .map(Event::AgentProvidersChanged)
                        .map_err(|_| "Malformed agent provider snapshot")
                },
                _ => Err("Malformed payload for AgentProvidersChanged Event"),
            },
            Ok(ProtobufEventType::AgentEvent) => match protobuf_event.payload {
                Some(ProtobufEventPayload::AgentEventPayload(p)) => {
                    Ok(Event::AgentEvent(yinz_types::agent::AgentEvent {
                        pane_id: yinz_types::internal_ids::TerminalId::new(p.pane_id),
                        kind: p.kind,
                        hook: p.hook,
                        session_id: p.session_id,
                        state: p
                            .state
                            .as_deref()
                            .map(|state| {
                                state
                                    .parse::<AgentState>()
                                    .map_err(|_| "Malformed agent state for AgentEvent")
                            })
                            .transpose()?,
                        detail: p.detail,
                        prompt: p.prompt,
                        last_message: p.last_message,
                        tool_name: p.tool_name,
                        cwd: p.cwd,
                        raw: p.raw,
                    }))
                },
                _ => Err("Malformed payload for AgentEvent Event"),
            },
            Ok(ProtobufEventType::ModifiersHeld) => match protobuf_event.payload {
                Some(ProtobufEventPayload::ModifiersHeldPayload(p)) => {
                    let modifiers = p
                        .modifiers
                        .into_iter()
                        .map(|modifier| {
                            ProtobufKeyModifier::try_from(modifier)
                                .map_err(|_| "Malformed modifier for ModifiersHeld")
                                .and_then(KeyModifier::try_from)
                        })
                        .collect::<Result<BTreeSet<_>, _>>()?;
                    Ok(Event::ModifiersHeld {
                        modifiers,
                        is_live: p.is_live,
                    })
                },
                _ => Err("Malformed payload for ModifiersHeld Event"),
            },
            Err(_) => Err("Unknown Protobuf Event"),
        }
    }
}

impl TryFrom<ProtobufClientInfo> for ClientInfo {
    type Error = &'static str;
    fn try_from(protobuf_client_info: ProtobufClientInfo) -> Result<Self, &'static str> {
        Ok(ClientInfo::new(
            ClientId::new(protobuf_client_info.client_id),
            protobuf_client_info
                .pane_id
                .ok_or("No pane id found")?
                .try_into()?,
            protobuf_client_info.running_command,
            protobuf_client_info.is_current_client,
        ))
    }
}

impl TryFrom<ClientInfo> for ProtobufClientInfo {
    type Error = &'static str;
    fn try_from(client_info: ClientInfo) -> Result<Self, &'static str> {
        Ok(ProtobufClientInfo {
            client_id: client_info.client_id.get(),
            pane_id: Some(client_info.pane_id.try_into()?),
            running_command: client_info.running_command,
            is_current_client: client_info.is_current_client,
        })
    }
}

impl TryFrom<Event> for ProtobufEvent {
    type Error = &'static str;
    fn try_from(event: Event) -> Result<Self, &'static str> {
        match event {
            Event::ModeUpdate(mode_info) => {
                let protobuf_mode_update_payload = mode_info.try_into()?;
                Ok(ProtobufEvent {
                    name: ProtobufEventType::ModeUpdate as i32,
                    payload: Some(event::Payload::ModeUpdatePayload(
                        protobuf_mode_update_payload,
                    )),
                })
            },
            Event::TabUpdate(tab_infos) => {
                let mut protobuf_tab_infos = vec![];
                for tab_info in tab_infos {
                    protobuf_tab_infos.push(tab_info.try_into()?);
                }
                let tab_update_payload = TabUpdatePayload {
                    tab_info: protobuf_tab_infos,
                };
                Ok(ProtobufEvent {
                    name: ProtobufEventType::TabUpdate as i32,
                    payload: Some(event::Payload::TabUpdatePayload(tab_update_payload)),
                })
            },
            Event::PaneUpdate(pane_manifest) => {
                let mut protobuf_pane_manifests = vec![];
                for (tab_index, pane_infos) in pane_manifest.panes {
                    let mut protobuf_pane_infos = vec![];
                    for pane_info in pane_infos {
                        protobuf_pane_infos.push(pane_info.try_into()?);
                    }
                    protobuf_pane_manifests.push(ProtobufPaneManifest {
                        tab_index: u32::try_from(tab_index.get())
                            .map_err(|_| "Tab position is outside u32 range")?,
                        panes: protobuf_pane_infos,
                    });
                }
                Ok(ProtobufEvent {
                    name: ProtobufEventType::PaneUpdate as i32,
                    payload: Some(event::Payload::PaneUpdatePayload(PaneUpdatePayload {
                        pane_manifest: protobuf_pane_manifests,
                    })),
                })
            },
            Event::Key(key) => Ok(ProtobufEvent {
                name: ProtobufEventType::Key as i32,
                payload: Some(event::Payload::KeyPayload(key.try_into()?)),
            }),
            Event::Mouse(mouse_event) => {
                let protobuf_mouse_payload = mouse_event.try_into()?;
                Ok(ProtobufEvent {
                    name: ProtobufEventType::Mouse as i32,
                    payload: Some(event::Payload::MouseEventPayload(protobuf_mouse_payload)),
                })
            },
            Event::Timer(seconds) => Ok(ProtobufEvent {
                name: ProtobufEventType::Timer as i32,
                payload: Some(event::Payload::TimerPayload(seconds.get() as f32)),
            }),
            Event::CopyToClipboard(clipboard_destination) => {
                let protobuf_copy_destination: ProtobufCopyDestination =
                    clipboard_destination.try_into()?;
                Ok(ProtobufEvent {
                    name: ProtobufEventType::CopyToClipboard as i32,
                    payload: Some(event::Payload::CopyToClipboardPayload(
                        protobuf_copy_destination as i32,
                    )),
                })
            },
            Event::SystemClipboardFailure => Ok(ProtobufEvent {
                name: ProtobufEventType::SystemClipboardFailure as i32,
                payload: None,
            }),
            Event::InputReceived => Ok(ProtobufEvent {
                name: ProtobufEventType::InputReceived as i32,
                payload: None,
            }),
            Event::Visible(is_visible) => Ok(ProtobufEvent {
                name: ProtobufEventType::Visible as i32,
                payload: Some(event::Payload::VisiblePayload(is_visible)),
            }),
            Event::CustomMessage(message, payload) => Ok(ProtobufEvent {
                name: ProtobufEventType::CustomMessage as i32,
                payload: Some(event::Payload::CustomMessagePayload(CustomMessagePayload {
                    message_name: message,
                    payload,
                })),
            }),
            Event::FileSystemCreate(event_paths) => {
                let mut paths = vec![];
                let mut paths_metadata = vec![];
                for (path, path_metadata) in event_paths {
                    paths.push(path.display().to_string());
                    paths_metadata.push(path_metadata.into());
                }
                let file_list_payload = FileListPayload {
                    paths,
                    paths_metadata,
                };
                Ok(ProtobufEvent {
                    name: ProtobufEventType::FileSystemCreate as i32,
                    payload: Some(event::Payload::FileListPayload(file_list_payload)),
                })
            },
            Event::FileSystemRead(event_paths) => {
                let mut paths = vec![];
                let mut paths_metadata = vec![];
                for (path, path_metadata) in event_paths {
                    paths.push(path.display().to_string());
                    paths_metadata.push(path_metadata.into());
                }
                let file_list_payload = FileListPayload {
                    paths,
                    paths_metadata,
                };
                Ok(ProtobufEvent {
                    name: ProtobufEventType::FileSystemRead as i32,
                    payload: Some(event::Payload::FileListPayload(file_list_payload)),
                })
            },
            Event::FileSystemUpdate(event_paths) => {
                let mut paths = vec![];
                let mut paths_metadata = vec![];
                for (path, path_metadata) in event_paths {
                    paths.push(path.display().to_string());
                    paths_metadata.push(path_metadata.into());
                }
                let file_list_payload = FileListPayload {
                    paths,
                    paths_metadata,
                };
                Ok(ProtobufEvent {
                    name: ProtobufEventType::FileSystemUpdate as i32,
                    payload: Some(event::Payload::FileListPayload(file_list_payload)),
                })
            },
            Event::FileSystemDelete(event_paths) => {
                let mut paths = vec![];
                let mut paths_metadata = vec![];
                for (path, path_metadata) in event_paths {
                    paths.push(path.display().to_string());
                    paths_metadata.push(path_metadata.into());
                }
                let file_list_payload = FileListPayload {
                    paths,
                    paths_metadata,
                };
                Ok(ProtobufEvent {
                    name: ProtobufEventType::FileSystemDelete as i32,
                    payload: Some(event::Payload::FileListPayload(file_list_payload)),
                })
            },
            Event::PermissionRequestResult(permission_status) => {
                let granted = match permission_status {
                    PermissionStatus::Granted => true,
                    PermissionStatus::Denied => false,
                };
                Ok(ProtobufEvent {
                    name: ProtobufEventType::PermissionRequestResult as i32,
                    payload: Some(event::Payload::PermissionRequestResultPayload(
                        PermissionRequestResultPayload { granted },
                    )),
                })
            },
            Event::SessionUpdate(session_infos, resurrectable_sessions) => {
                let mut protobuf_session_manifests = vec![];
                for session_info in session_infos {
                    protobuf_session_manifests.push(session_info.try_into()?);
                }
                let mut protobuf_resurrectable_sessions = vec![];
                for resurrectable_session in resurrectable_sessions {
                    protobuf_resurrectable_sessions.push(resurrectable_session.into());
                }
                let session_update_payload = SessionUpdatePayload {
                    session_manifests: protobuf_session_manifests,
                    resurrectable_sessions: protobuf_resurrectable_sessions,
                };
                Ok(ProtobufEvent {
                    name: ProtobufEventType::SessionUpdate as i32,
                    payload: Some(event::Payload::SessionUpdatePayload(session_update_payload)),
                })
            },
            Event::PaneTabsUpdate(tabs) => Ok(ProtobufEvent {
                name: ProtobufEventType::PaneTabsUpdate as i32,
                payload: Some(ProtobufEventPayload::PaneTabsUpdatePayload(
                    serde_json::to_string(&tabs).map_err(|_| "Failed to encode PaneTabsUpdate")?,
                )),
            }),
            Event::PaneOutput {
                pane_id,
                data,
                context,
            } => Ok(ProtobufEvent {
                name: ProtobufEventType::PaneOutput as i32,
                payload: Some(ProtobufEventPayload::PaneOutputPayload(PaneOutputPayload {
                    pane_id,
                    data,
                    context: context
                        .into_iter()
                        .map(|(name, value)| ProtobufContextItem { name, value })
                        .collect(),
                })),
            }),
            Event::RunCommandOutput {
                data,
                stderr,
                context,
            } => Ok(ProtobufEvent {
                name: ProtobufEventType::RunCommandOutput as i32,
                payload: Some(ProtobufEventPayload::RunCommandOutputPayload(
                    RunCommandOutputPayload {
                        data,
                        stderr,
                        context: context
                            .into_iter()
                            .map(|(name, value)| ProtobufContextItem { name, value })
                            .collect(),
                    },
                )),
            }),
            Event::RunCommandResult(exit_code, stdout, stderr, context) => {
                let run_command_result_payload = RunCommandResultPayload {
                    exit_code,
                    stdout,
                    stderr,
                    context: context
                        .into_iter()
                        .map(|(name, value)| ContextItem { name, value })
                        .collect(),
                };
                Ok(ProtobufEvent {
                    name: ProtobufEventType::RunCommandResult as i32,
                    payload: Some(event::Payload::RunCommandResultPayload(
                        run_command_result_payload,
                    )),
                })
            },
            Event::WebRequestResult(status, headers, body, context) => {
                let web_request_result_payload = WebRequestResultPayload {
                    status: status as i32,
                    headers: headers
                        .into_iter()
                        .map(|(name, value)| Header { name, value })
                        .collect(),
                    body,
                    context: context
                        .into_iter()
                        .map(|(name, value)| ContextItem { name, value })
                        .collect(),
                };
                Ok(ProtobufEvent {
                    name: ProtobufEventType::WebRequestResult as i32,
                    payload: Some(event::Payload::WebRequestResultPayload(
                        web_request_result_payload,
                    )),
                })
            },
            Event::WebRequestStreamChunk {
                stream_id,
                chunk,
                done,
                error,
            } => Ok(ProtobufEvent {
                name: ProtobufEventType::WebRequestStreamChunk as i32,
                payload: Some(event::Payload::WebRequestStreamChunkPayload(
                    WebRequestStreamChunkPayload {
                        stream_id,
                        chunk,
                        done,
                        error,
                    },
                )),
            }),
            Event::CommandPaneOpened(terminal_pane_id, context) => {
                let command_pane_opened_payload = CommandPaneOpenedPayload {
                    terminal_pane_id: terminal_pane_id.get(),
                    context: context
                        .into_iter()
                        .map(|(name, value)| ContextItem { name, value })
                        .collect(),
                };
                Ok(ProtobufEvent {
                    name: ProtobufEventType::CommandPaneOpened as i32,
                    payload: Some(event::Payload::CommandPaneOpenedPayload(
                        command_pane_opened_payload,
                    )),
                })
            },
            Event::CommandPaneExited(terminal_pane_id, exit_code, context) => {
                let command_pane_exited_payload = CommandPaneExitedPayload {
                    terminal_pane_id: terminal_pane_id.get(),
                    exit_code,
                    context: context
                        .into_iter()
                        .map(|(name, value)| ContextItem { name, value })
                        .collect(),
                };
                Ok(ProtobufEvent {
                    name: ProtobufEventType::CommandPaneExited as i32,
                    payload: Some(event::Payload::CommandPaneExitedPayload(
                        command_pane_exited_payload,
                    )),
                })
            },
            Event::PaneClosed(pane_id) => Ok(ProtobufEvent {
                name: ProtobufEventType::PaneClosed as i32,
                payload: Some(event::Payload::PaneClosedPayload(PaneClosedPayload {
                    pane_id: Some(pane_id.try_into()?),
                })),
            }),
            Event::EditPaneOpened(terminal_pane_id, context) => {
                let command_pane_opened_payload = EditPaneOpenedPayload {
                    terminal_pane_id: terminal_pane_id.get(),
                    context: context
                        .into_iter()
                        .map(|(name, value)| ContextItem { name, value })
                        .collect(),
                };
                Ok(ProtobufEvent {
                    name: ProtobufEventType::EditPaneOpened as i32,
                    payload: Some(event::Payload::EditPaneOpenedPayload(
                        command_pane_opened_payload,
                    )),
                })
            },
            Event::EditPaneExited(terminal_pane_id, exit_code, context) => {
                let command_pane_exited_payload = EditPaneExitedPayload {
                    terminal_pane_id: terminal_pane_id.get(),
                    exit_code,
                    context: context
                        .into_iter()
                        .map(|(name, value)| ContextItem { name, value })
                        .collect(),
                };
                Ok(ProtobufEvent {
                    name: ProtobufEventType::EditPaneExited as i32,
                    payload: Some(event::Payload::EditPaneExitedPayload(
                        command_pane_exited_payload,
                    )),
                })
            },
            Event::CommandPaneReRun(terminal_pane_id, context) => {
                let command_pane_rerun_payload = CommandPaneReRunPayload {
                    terminal_pane_id: terminal_pane_id.get(),
                    context: context
                        .into_iter()
                        .map(|(name, value)| ContextItem { name, value })
                        .collect(),
                };
                Ok(ProtobufEvent {
                    name: ProtobufEventType::CommandPaneReRun as i32,
                    payload: Some(event::Payload::CommandPaneRerunPayload(
                        command_pane_rerun_payload,
                    )),
                })
            },
            Event::FailedToWriteConfigToDisk { file_path, reason } => Ok(ProtobufEvent {
                name: ProtobufEventType::FailedToWriteConfigToDisk as i32,
                payload: Some(event::Payload::FailedToWriteConfigToDiskPayload(
                    FailedToWriteConfigToDiskPayload { file_path, reason },
                )),
            }),
            Event::ListClients(mut client_info_list) => Ok(ProtobufEvent {
                name: ProtobufEventType::ListClients as i32,
                payload: Some(event::Payload::ListClientsPayload(ListClientsPayload {
                    client_info: client_info_list
                        .drain(..)
                        .filter_map(|c| c.try_into().ok())
                        .collect(),
                })),
            }),
            Event::HostFolderChanged(new_host_folder_path) => Ok(ProtobufEvent {
                name: ProtobufEventType::HostFolderChanged as i32,
                payload: Some(event::Payload::HostFolderChangedPayload(
                    HostFolderChangedPayload {
                        new_host_folder_path: new_host_folder_path.display().to_string(),
                    },
                )),
            }),
            Event::FailedToChangeHostFolder(error_message) => Ok(ProtobufEvent {
                name: ProtobufEventType::FailedToChangeHostFolder as i32,
                payload: Some(event::Payload::FailedToChangeHostFolderPayload(
                    FailedToChangeHostFolderPayload { error_message },
                )),
            }),
            Event::PastedText(pasted_text) => Ok(ProtobufEvent {
                name: ProtobufEventType::PastedText as i32,
                payload: Some(event::Payload::PastedTextPayload(PastedTextPayload {
                    pasted_text,
                })),
            }),
            Event::ConfigWasWrittenToDisk => Ok(ProtobufEvent {
                name: ProtobufEventType::ConfigWasWrittenToDisk as i32,
                payload: None,
            }),
            Event::CellPixelSizeChanged(size) => Ok(ProtobufEvent {
                name: ProtobufEventType::CellPixelSizeChanged as i32,
                payload: Some(ProtobufEventPayload::CellPixelSizeChangedPayload(
                    super::generated_api::api::event::CellPixelSizeChangedPayload {
                        size: size.map(TryInto::try_into).transpose()?,
                    },
                )),
            }),
            Event::BeforeClose => Ok(ProtobufEvent {
                name: ProtobufEventType::BeforeClose as i32,
                payload: None,
            }),
            Event::InterceptedKeyPress(key) => Ok(ProtobufEvent {
                name: ProtobufEventType::InterceptedKeyPress as i32,
                payload: Some(event::Payload::KeyPayload(key.try_into()?)),
            }),
            Event::PaneRenderReport(pane_contents_map) => Ok(ProtobufEvent {
                name: ProtobufEventType::PaneRenderReport as i32,
                payload: Some(event::Payload::PaneRenderReportPayload(
                    pane_contents_map.try_into()?,
                )),
            }),
            Event::UserAction(action, client_id, terminal_id, cli_client_id) => {
                let protobuf_action: ProtobufAction = action
                    .try_into()
                    .map_err(|_| "Failed to convert Action to protobuf")?;
                let protobuf_payload = ProtobufUserActionPayload {
                    action: Some(protobuf_action),
                    client_id: client_id.get(),
                    terminal_id: terminal_id.map(|id| id.get()),
                    cli_client_id: cli_client_id.map(|id| id.get()),
                };
                Ok(ProtobufEvent {
                    name: ProtobufEventType::UserAction as i32,
                    payload: Some(event::Payload::UserActionPayload(protobuf_payload)),
                })
            },
            Event::ActionComplete(action, pane_id, context) => {
                let protobuf_action = action.try_into()?;
                let protobuf_pane_id = pane_id.map(|id| id.try_into()).transpose()?;
                let context_items: Vec<ProtobufContextItem> = context
                    .into_iter()
                    .map(|(name, value)| ProtobufContextItem { name, value })
                    .collect();
                let action_complete_payload = ProtobufActionCompletePayload {
                    action: Some(protobuf_action),
                    pane_id: protobuf_pane_id,
                    context: context_items,
                };
                Ok(ProtobufEvent {
                    name: ProtobufEventType::ActionComplete as i32,
                    payload: Some(event::Payload::ActionCompletePayload(
                        action_complete_payload,
                    )),
                })
            },
            Event::CwdChanged(pane_id, new_cwd, focused_client_ids) => {
                let protobuf_pane_id: ProtobufPaneId = pane_id.try_into()?;
                let new_cwd_string = new_cwd
                    .to_str()
                    .ok_or("Failed to convert PathBuf to string")?
                    .to_string();
                let focused_client_ids_u32: Vec<u32> =
                    focused_client_ids.into_iter().map(|id| id.get()).collect();
                let cwd_changed_payload = ProtobufCwdChangedPayload {
                    pane_id: Some(protobuf_pane_id),
                    new_cwd: new_cwd_string,
                    focused_client_ids: focused_client_ids_u32,
                };
                Ok(ProtobufEvent {
                    name: ProtobufEventType::CwdChanged as i32,
                    payload: Some(event::Payload::CwdChangedPayload(cwd_changed_payload)),
                })
            },
            Event::CommandChanged(pane_id, command, is_foreground, focused_client_ids) => {
                let protobuf_pane_id: ProtobufPaneId = pane_id.try_into()?;
                let focused_client_ids_u32: Vec<u32> =
                    focused_client_ids.into_iter().map(|id| id.get()).collect();
                let payload = ProtobufCommandChangedPayload {
                    pane_id: Some(protobuf_pane_id),
                    command,
                    is_foreground,
                    focused_client_ids: focused_client_ids_u32,
                };
                Ok(ProtobufEvent {
                    name: ProtobufEventType::CommandChanged as i32,
                    payload: Some(event::Payload::CommandChangedPayload(payload)),
                })
            },
            Event::AvailableLayoutInfo(available_layouts, layouts_with_errors) => {
                let mut protobuf_available_layouts = vec![];
                let mut protobuf_layouts_with_errors = vec![];

                for layout_info in available_layouts {
                    protobuf_available_layouts.push(layout_info.try_into()?);
                }

                for layout_error in layouts_with_errors {
                    protobuf_layouts_with_errors.push(layout_error.try_into()?);
                }

                let available_layout_info_payload = ProtobufAvailableLayoutInfoPayload {
                    available_layouts: protobuf_available_layouts,
                    layouts_with_errors: protobuf_layouts_with_errors,
                };

                Ok(ProtobufEvent {
                    name: ProtobufEventType::AvailableLayoutInfo as i32,
                    payload: Some(event::Payload::AvailableLayoutInfoPayload(
                        available_layout_info_payload,
                    )),
                })
            },
            Event::PluginConfigurationChanged(configuration) => {
                let configuration_items: Vec<ProtobufContextItem> = configuration
                    .into_iter()
                    .map(|(name, value)| ProtobufContextItem { name, value })
                    .collect();

                let payload = ProtobufPluginConfigurationChangedPayload {
                    configuration: configuration_items,
                };

                Ok(ProtobufEvent {
                    name: ProtobufEventType::PluginConfigurationChanged as i32,
                    payload: Some(event::Payload::PluginConfigurationChangedPayload(payload)),
                })
            },
            Event::HighlightClicked {
                pane_id,
                pattern,
                matched_string,
                context,
            } => Ok(ProtobufEvent {
                name: ProtobufEventType::HighlightClicked as i32,
                payload: Some(event::Payload::HighlightClickedPayload(
                    HighlightClickedPayload {
                        pane_id: pane_id.try_into().ok(),
                        pattern,
                        matched_string,
                        context: context
                            .into_iter()
                            .map(|(name, value)| ProtobufContextItem { name, value })
                            .collect(),
                    },
                )),
            }),
            Event::ContextMenuRequested(context) => Ok(ProtobufEvent {
                name: ProtobufEventType::ContextMenuRequested as i32,
                payload: Some(event::Payload::ContextMenuRequestedPayload(
                    ProtobufContextMenuRequestedPayload {
                        context: Some((*context).try_into()?),
                    },
                )),
            }),
            Event::ContextMenuItemSelected {
                label,
                item_context,
                context,
            } => Ok(ProtobufEvent {
                name: ProtobufEventType::ContextMenuItemSelected as i32,
                payload: Some(event::Payload::ContextMenuItemSelectedPayload(
                    ProtobufContextMenuItemSelectedPayload {
                        label,
                        item_context: item_context
                            .into_iter()
                            .map(|(name, value)| ProtobufContextItem { name, value })
                            .collect(),
                        context: Some((*context).try_into()?),
                    },
                )),
            }),
            Event::HostTerminalThemeChanged(mode) => {
                let proto_mode: ProtobufHostTerminalThemeIndication = mode.into();
                let payload = ProtobufHostTerminalThemeChangedPayload {
                    mode: proto_mode as i32,
                };
                Ok(ProtobufEvent {
                    name: ProtobufEventType::HostTerminalThemeChanged as i32,
                    payload: Some(event::Payload::HostTerminalThemeChangedPayload(payload)),
                })
            },
            Event::HintText(hint_text) => {
                let mut proto_hint_text = HashMap::new();
                for (width, styled_text) in hint_text {
                    proto_hint_text.insert(width as u32, ProtobufStyledText::from(styled_text));
                }
                let payload = ProtobufHintTextPayload {
                    hint_text: proto_hint_text,
                };
                Ok(ProtobufEvent {
                    name: ProtobufEventType::HintText as i32,
                    payload: Some(event::Payload::HintTextPayload(payload)),
                })
            },
            Event::ActivePaneScroll(scroll) => {
                let payload = match scroll {
                    Some((position, length)) => ProtobufActivePaneScrollPayload {
                        position: Some(position as u32),
                        length: Some(length as u32),
                    },
                    None => ProtobufActivePaneScrollPayload {
                        position: None,
                        length: None,
                    },
                };
                Ok(ProtobufEvent {
                    name: ProtobufEventType::ActivePaneScroll as i32,
                    payload: Some(event::Payload::ActivePaneScrollPayload(payload)),
                })
            },
            Event::PluginDataChanged {
                watch_id,
                rows,
                truncated,
            } => Ok(ProtobufEvent {
                name: ProtobufEventType::PluginDataChanged as i32,
                payload: Some(event::Payload::PluginDataChangedPayload(
                    ProtobufPluginDataChangedPayload {
                        watch_id: watch_id.get(),
                        rows: Some(ProtobufDataRows::from(rows)),
                        truncated,
                    },
                )),
            }),
            Event::NotificationReceived(notification) => Ok(ProtobufEvent {
                name: ProtobufEventType::NotificationReceived as i32,
                payload: Some(event::Payload::NotificationReceivedPayload(
                    serde_json::to_string(&notification)
                        .map_err(|_| "Cannot encode NotificationReceived")?,
                )),
            }),
            Event::NotificationRequested(notification) => Ok(ProtobufEvent {
                name: ProtobufEventType::NotificationRequested as i32,
                payload: Some(event::Payload::NotificationRequestedPayload(
                    serde_json::to_string(&notification)
                        .map_err(|_| "Cannot encode NotificationRequested")?,
                )),
            }),
            Event::NotificationResolved(notification) => Ok(ProtobufEvent {
                name: ProtobufEventType::NotificationResolved as i32,
                payload: Some(event::Payload::NotificationResolvedPayload(
                    serde_json::to_string(&notification)
                        .map_err(|_| "Cannot encode NotificationResolved")?,
                )),
            }),
            Event::Hook { point, payload } => {
                let payload = match payload {
                    yinz_types::data::HookPayload::AgentExit(configuration) => {
                        hook_payload::Payload::AgentExit(ProtobufAgentExitHookPayload {
                            configuration: configuration.into_iter().collect(),
                        })
                    },
                    yinz_types::data::HookPayload::Agent {
                        pane_id,
                        agent,
                        previous_state,
                        command,
                    } => hook_payload::Payload::Agent(ProtobufAgentHookPayload {
                        pane_id: pane_id.get(),
                        agent: Some(agent.try_into()?),
                        previous_state: previous_state.map(|state| state.to_string()),
                        command,
                    }),
                    yinz_types::data::HookPayload::PaneExit {
                        pane_id,
                        exit_code,
                        cwd,
                        command,
                        held,
                    } => hook_payload::Payload::PaneExit(ProtobufPaneExitHookPayload {
                        pane_id: pane_id.get(),
                        exit_code,
                        cwd: cwd.map(|cwd| cwd.display().to_string()),
                        command,
                        held,
                    }),
                    yinz_types::data::HookPayload::SessionStart {
                        session_name,
                        client_id,
                    } => hook_payload::Payload::SessionStart(ProtobufSessionStartHookPayload {
                        session_name: session_name.to_string(),
                        client_id: client_id.get(),
                    }),
                };
                Ok(ProtobufEvent {
                    name: ProtobufEventType::Hook as i32,
                    payload: Some(event::Payload::HookPayload(ProtobufHookPayload {
                        point: point.as_str().to_owned(),
                        payload: Some(payload),
                    })),
                })
            },
            Event::ModifiersHeld { modifiers, is_live } => {
                let modifiers = modifiers
                    .into_iter()
                    .map(|modifier| {
                        ProtobufKeyModifier::try_from(modifier).map(|modifier| modifier as i32)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(ProtobufEvent {
                    name: ProtobufEventType::ModifiersHeld as i32,
                    payload: Some(event::Payload::ModifiersHeldPayload(ModifiersHeldPayload {
                        modifiers,
                        is_live,
                    })),
                })
            },
            Event::PickResult(result) => Ok(ProtobufEvent {
                name: ProtobufEventType::PickResult as i32,
                payload: Some(event::Payload::PickResultPayload(
                    serde_json::to_string(&result).map_err(|_| "Invalid PickResult")?,
                )),
            }),
            Event::ProjectProvidersChanged(providers) => Ok(ProtobufEvent {
                name: ProtobufEventType::ProjectProvidersChanged as i32,
                payload: Some(event::Payload::ProjectProvidersChangedPayload(
                    serde_json::to_string(&providers)
                        .map_err(|_| "Cannot serialize agent providers")?,
                )),
            }),
            Event::AgentProvidersChanged(providers) => Ok(ProtobufEvent {
                name: ProtobufEventType::AgentProvidersChanged as i32,
                payload: Some(event::Payload::AgentProvidersChangedPayload(
                    serde_json::to_string(&providers)
                        .map_err(|_| "Cannot serialize agent providers")?,
                )),
            }),
            Event::AgentEvent(agent_event) => Ok(ProtobufEvent {
                name: ProtobufEventType::AgentEvent as i32,
                payload: Some(event::Payload::AgentEventPayload(
                    ProtobufAgentEventPayload {
                        pane_id: agent_event.pane_id.get(),
                        kind: agent_event.kind,
                        hook: agent_event.hook,
                        session_id: agent_event.session_id,
                        state: agent_event.state.map(|state| state.to_string()),
                        detail: agent_event.detail,
                        prompt: agent_event.prompt,
                        last_message: agent_event.last_message,
                        tool_name: agent_event.tool_name,
                        cwd: agent_event.cwd,
                        raw: agent_event.raw,
                    },
                )),
            }),
            Event::InitialKeybinds(keybinds) => {
                let mut protobuf_keybinds: Vec<ProtobufInputModeKeybinds> = vec![];
                for (input_mode, input_mode_keybinds) in keybinds {
                    let mode: ProtobufInputMode = input_mode.try_into()?;
                    let mut key_binds: Vec<ProtobufKeyBind> = vec![];
                    for (key, actions) in input_mode_keybinds {
                        let protobuf_key: ProtobufKey = key.try_into()?;
                        let mut protobuf_actions: Vec<ProtobufAction> = vec![];
                        for action in actions {
                            if let Ok(protobuf_action) = action.try_into() {
                                protobuf_actions.push(protobuf_action);
                            }
                        }
                        key_binds.push(ProtobufKeyBind {
                            key: Some(protobuf_key),
                            action: protobuf_actions,
                        });
                    }
                    protobuf_keybinds.push(ProtobufInputModeKeybinds {
                        mode: mode as i32,
                        key_bind: key_binds,
                    });
                }
                Ok(ProtobufEvent {
                    name: ProtobufEventType::InitialKeybinds as i32,
                    payload: Some(event::Payload::InitialKeybindsPayload(
                        InitialKeybindsPayload {
                            keybinds: protobuf_keybinds,
                        },
                    )),
                })
            },
            _ => Err("Unsupported event"),
        }
    }
}

impl TryFrom<SessionInfo> for ProtobufSessionManifest {
    type Error = &'static str;
    fn try_from(session_info: SessionInfo) -> Result<Self, &'static str> {
        let mut protobuf_pane_manifests = vec![];
        for (tab_index, pane_infos) in session_info.panes.panes {
            let mut protobuf_pane_infos = vec![];
            for pane_info in pane_infos {
                protobuf_pane_infos.push(pane_info.try_into()?);
            }
            protobuf_pane_manifests.push(ProtobufPaneManifest {
                tab_index: u32::try_from(tab_index.get())
                    .map_err(|_| "Tab position is outside u32 range")?,
                panes: protobuf_pane_infos,
            });
        }
        Ok(ProtobufSessionManifest {
            name: session_info.name.to_string(),
            panes: protobuf_pane_manifests,
            tabs: session_info
                .tabs
                .iter()
                .map(|t| t.clone().try_into())
                .collect::<Result<_, _>>()?,
            connected_clients: session_info.connected_clients as u32,
            is_current_session: session_info.is_current_session,
            available_layouts: session_info
                .available_layouts
                .into_iter()
                .filter_map(|l| ProtobufLayoutInfo::try_from(l).ok())
                .collect(),
            plugins: session_info
                .plugins
                .into_iter()
                .map(ProtobufPluginInfo::from)
                .collect(),
            tab_history: session_info
                .tab_history
                .into_iter()
                .map(ProtobufClientTabHistory::try_from)
                .collect::<Result<_, _>>()?,
            pane_history: session_info
                .pane_history
                .into_iter()
                .map(ProtobufClientPaneHistory::from)
                .collect(),
            creation_time: session_info.creation_time.as_secs(),
        })
    }
}

impl TryFrom<(ClientId, Vec<TabId>)> for ProtobufClientTabHistory {
    type Error = &'static str;
    fn try_from((client_id, tab_history): (ClientId, Vec<TabId>)) -> Result<Self, Self::Error> {
        Ok(ProtobufClientTabHistory {
            client_id: client_id.get(),
            tab_history: tab_history
                .into_iter()
                .map(|t| u32::try_from(t.get()))
                .collect::<Result<_, _>>()
                .map_err(|_| "tab_id is outside u32 range")?,
        })
    }
}

impl From<(ClientId, Vec<PaneId>)> for ProtobufClientPaneHistory {
    fn from((client_id, pane_history): (ClientId, Vec<PaneId>)) -> ProtobufClientPaneHistory {
        ProtobufClientPaneHistory {
            client_id: client_id.get(),
            pane_history: pane_history
                .into_iter()
                .filter_map(|p| p.try_into().ok())
                .collect(),
        }
    }
}
impl From<(yinz_types::internal_ids::PluginId, PluginInfo)> for ProtobufPluginInfo {
    fn from(
        (plugin_id, plugin_info): (yinz_types::internal_ids::PluginId, PluginInfo),
    ) -> ProtobufPluginInfo {
        ProtobufPluginInfo {
            plugin_id: plugin_id.get(),
            plugin_url: plugin_info.location,
            plugin_config: plugin_info
                .configuration
                .into_iter()
                .map(|(name, value)| ContextItem { name, value })
                .collect(),
        }
    }
}

impl TryFrom<ProtobufSessionManifest> for SessionInfo {
    type Error = &'static str;
    fn try_from(protobuf_session_manifest: ProtobufSessionManifest) -> Result<Self, &'static str> {
        let mut pane_manifest: HashMap<TabPosition, Vec<PaneInfo>> = HashMap::new();
        for protobuf_pane_manifest in protobuf_session_manifest.panes {
            let tab_index = TabPosition::new(protobuf_pane_manifest.tab_index as usize);
            let mut panes = vec![];
            for protobuf_pane_info in protobuf_pane_manifest.panes {
                panes.push(protobuf_pane_info.try_into()?);
            }
            if pane_manifest.contains_key(&tab_index) {
                return Err("Duplicate tab definition in pane manifest");
            }
            pane_manifest.insert(tab_index, panes);
        }
        let panes = PaneManifest {
            panes: pane_manifest,
        };
        let mut plugins = BTreeMap::new();
        for plugin_info in protobuf_session_manifest.plugins.into_iter() {
            let mut configuration = BTreeMap::new();
            for context_item in plugin_info.plugin_config.into_iter() {
                configuration.insert(context_item.name, context_item.value);
            }
            plugins.insert(
                yinz_types::internal_ids::PluginId::new(plugin_info.plugin_id),
                PluginInfo {
                    location: plugin_info.plugin_url,
                    configuration,
                },
            );
        }
        let mut tab_history = BTreeMap::new();
        for client_tab_history in protobuf_session_manifest.tab_history.into_iter() {
            let client_id = client_tab_history.client_id;
            let tab_history_for_client = client_tab_history
                .tab_history
                .iter()
                .map(|t| TabId::new(*t as usize))
                .collect();
            tab_history.insert(ClientId::new(client_id), tab_history_for_client);
        }
        let mut pane_history = BTreeMap::new();
        for client_pane_history in protobuf_session_manifest.pane_history.into_iter() {
            let client_id = client_pane_history.client_id;
            let pane_history_for_client = client_pane_history
                .pane_history
                .into_iter()
                .filter_map(|p| p.try_into().ok())
                .collect();
            pane_history.insert(ClientId::new(client_id), pane_history_for_client);
        }
        Ok(SessionInfo {
            name: protobuf_session_manifest.name.into(),
            tabs: protobuf_session_manifest
                .tabs
                .iter()
                .map(|t| t.clone().try_into())
                .collect::<Result<_, _>>()?,
            panes,
            connected_clients: protobuf_session_manifest.connected_clients as usize,
            is_current_session: protobuf_session_manifest.is_current_session,
            available_layouts: protobuf_session_manifest
                .available_layouts
                .into_iter()
                .filter_map(|l| LayoutInfo::try_from(l).ok())
                .collect(),
            plugins,
            tab_history,
            pane_history,
            creation_time: Duration::from_secs(protobuf_session_manifest.creation_time),
        })
    }
}

impl TryFrom<LayoutInfo> for ProtobufLayoutInfo {
    type Error = &'static str;
    fn try_from(layout_info: LayoutInfo) -> Result<Self, &'static str> {
        match layout_info {
            LayoutInfo::File(name, layout_metadata) => Ok(ProtobufLayoutInfo {
                source: "file".to_owned(),
                name,
                layout_metadata: Some(layout_metadata.try_into()?),
            }),
            LayoutInfo::BuiltIn(name) => Ok(ProtobufLayoutInfo {
                source: "built-in".to_owned(),
                name,
                layout_metadata: None,
            }),
            LayoutInfo::Stringified(stringified_layout) => Ok(ProtobufLayoutInfo {
                source: "stringified".to_owned(),
                name: stringified_layout.clone(),
                layout_metadata: None,
            }),
        }
    }
}

impl TryFrom<ProtobufLayoutInfo> for LayoutInfo {
    type Error = &'static str;
    fn try_from(protobuf_layout_info: ProtobufLayoutInfo) -> Result<Self, &'static str> {
        match protobuf_layout_info.source.as_str() {
            "file" => {
                let layout_metadata = protobuf_layout_info
                    .layout_metadata
                    .map(|m| m.try_into())
                    .transpose()?
                    .unwrap_or_default();
                Ok(LayoutInfo::File(protobuf_layout_info.name, layout_metadata))
            },
            "built-in" => Ok(LayoutInfo::BuiltIn(protobuf_layout_info.name)),
            "stringified" => Ok(LayoutInfo::Stringified(protobuf_layout_info.name)),
            _ => Err("Unknown source for layout"),
        }
    }
}

impl TryFrom<ProtobufLayoutMetadata> for LayoutMetadata {
    type Error = &'static str;
    fn try_from(protobuf_metadata: ProtobufLayoutMetadata) -> Result<Self, &'static str> {
        let tabs = protobuf_metadata
            .tabs
            .into_iter()
            .map(|t| t.try_into())
            .collect::<Result<Vec<_>, _>>()?;
        Ok(LayoutMetadata {
            tabs,
            creation_time: protobuf_metadata.creation_time,
            update_time: protobuf_metadata.update_time,
        })
    }
}

impl TryFrom<LayoutMetadata> for ProtobufLayoutMetadata {
    type Error = &'static str;
    fn try_from(metadata: LayoutMetadata) -> Result<Self, &'static str> {
        let tabs = metadata
            .tabs
            .into_iter()
            .map(|t| t.try_into())
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ProtobufLayoutMetadata {
            tabs,
            creation_time: metadata.creation_time,
            update_time: metadata.update_time,
        })
    }
}

impl TryFrom<ProtobufTabMetadata> for TabMetadata {
    type Error = &'static str;
    fn try_from(protobuf_metadata: ProtobufTabMetadata) -> Result<Self, &'static str> {
        let panes = protobuf_metadata
            .pane_metadata
            .into_iter()
            .map(|p| p.try_into())
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TabMetadata {
            panes,
            name: protobuf_metadata.name,
        })
    }
}

impl TryFrom<TabMetadata> for ProtobufTabMetadata {
    type Error = &'static str;
    fn try_from(metadata: TabMetadata) -> Result<Self, &'static str> {
        let pane_metadata = metadata
            .panes
            .into_iter()
            .map(|p| p.try_into())
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ProtobufTabMetadata {
            pane_metadata,
            name: metadata.name,
        })
    }
}

impl TryFrom<ProtobufPaneMetadata> for PaneMetadata {
    type Error = &'static str;
    fn try_from(protobuf_metadata: ProtobufPaneMetadata) -> Result<Self, &'static str> {
        Ok(PaneMetadata {
            name: protobuf_metadata.name,
            is_plugin: protobuf_metadata.is_plugin,
            is_builtin_plugin: protobuf_metadata.is_builtin_plugin,
        })
    }
}

impl TryFrom<PaneMetadata> for ProtobufPaneMetadata {
    type Error = &'static str;
    fn try_from(metadata: PaneMetadata) -> Result<Self, &'static str> {
        Ok(ProtobufPaneMetadata {
            name: metadata.name,
            is_plugin: metadata.is_plugin,
            is_builtin_plugin: metadata.is_builtin_plugin,
        })
    }
}

// LayoutWithError conversions
impl TryFrom<ProtobufLayoutWithError> for yinz_types::data::LayoutWithError {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufLayoutWithError) -> Result<Self, Self::Error> {
        Ok(yinz_types::data::LayoutWithError {
            layout_name: protobuf.layout_name,
            error: protobuf.error.ok_or("Missing error field")?.try_into()?,
        })
    }
}

impl TryFrom<yinz_types::data::LayoutWithError> for ProtobufLayoutWithError {
    type Error = &'static str;
    fn try_from(layout_error: yinz_types::data::LayoutWithError) -> Result<Self, Self::Error> {
        Ok(ProtobufLayoutWithError {
            layout_name: layout_error.layout_name,
            error: Some(layout_error.error.try_into()?),
        })
    }
}

// LayoutParsingError conversions
impl TryFrom<ProtobufLayoutParsingError> for yinz_types::data::LayoutParsingError {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufLayoutParsingError) -> Result<Self, Self::Error> {
        match protobuf.error_type.ok_or("Missing error_type")? {
            ProtobufLayoutParsingErrorType::KdlError(kdl_variant) => {
                Ok(yinz_types::data::LayoutParsingError::KdlError {
                    kdl_error: Box::new(
                        kdl_variant
                            .kdl_error
                            .ok_or("Missing kdl_error")?
                            .try_into()?,
                    ),
                    file_name: kdl_variant.file_name,
                    source_code: kdl_variant.source_code,
                })
            },
            ProtobufLayoutParsingErrorType::SyntaxError(_) => {
                Ok(yinz_types::data::LayoutParsingError::SyntaxError)
            },
        }
    }
}

impl TryFrom<yinz_types::data::LayoutParsingError> for ProtobufLayoutParsingError {
    type Error = &'static str;
    fn try_from(error: yinz_types::data::LayoutParsingError) -> Result<Self, Self::Error> {
        let error_type = match error {
            yinz_types::data::LayoutParsingError::KdlError {
                kdl_error,
                file_name,
                source_code,
            } => ProtobufLayoutParsingErrorType::KdlError(ProtobufKdlErrorVariant {
                kdl_error: Some((*kdl_error).try_into()?),
                file_name,
                source_code,
            }),
            yinz_types::data::LayoutParsingError::SyntaxError => {
                ProtobufLayoutParsingErrorType::SyntaxError(ProtobufSyntaxError {})
            },
        };
        Ok(ProtobufLayoutParsingError {
            error_type: Some(error_type),
        })
    }
}

// KdlError conversions
impl TryFrom<ProtobufKdlError> for yinz_types::data::LayoutDiagnostic {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufKdlError) -> Result<Self, Self::Error> {
        Ok(yinz_types::data::LayoutDiagnostic {
            error_message: protobuf.error_message,
            offset: protobuf.offset.map(|o| o as usize),
            len: protobuf.len.map(|l| l as usize),
            help_message: protobuf.help_message,
        })
    }
}

impl TryFrom<yinz_types::data::LayoutDiagnostic> for ProtobufKdlError {
    type Error = &'static str;
    fn try_from(kdl: yinz_types::data::LayoutDiagnostic) -> Result<Self, Self::Error> {
        Ok(ProtobufKdlError {
            error_message: kdl.error_message,
            // src is not serialized
            offset: kdl.offset.map(|o| o as u64),
            len: kdl.len.map(|l| l as u64),
            help_message: kdl.help_message,
        })
    }
}

impl TryFrom<CopyDestination> for ProtobufCopyDestination {
    type Error = &'static str;
    fn try_from(copy_destination: CopyDestination) -> Result<Self, &'static str> {
        match copy_destination {
            CopyDestination::Command => Ok(ProtobufCopyDestination::Command),
            CopyDestination::Primary => Ok(ProtobufCopyDestination::Primary),
            CopyDestination::System => Ok(ProtobufCopyDestination::System),
        }
    }
}

impl TryFrom<ProtobufCopyDestination> for CopyDestination {
    type Error = &'static str;
    fn try_from(protobuf_copy_destination: ProtobufCopyDestination) -> Result<Self, &'static str> {
        match protobuf_copy_destination {
            ProtobufCopyDestination::Command => Ok(CopyDestination::Command),
            ProtobufCopyDestination::Primary => Ok(CopyDestination::Primary),
            ProtobufCopyDestination::System => Ok(CopyDestination::System),
        }
    }
}

impl TryFrom<MouseEventPayload> for Mouse {
    type Error = &'static str;
    fn try_from(mouse_event_payload: MouseEventPayload) -> Result<Self, &'static str> {
        match MouseEventName::try_from(mouse_event_payload.mouse_event_name) {
            Ok(MouseEventName::MouseScrollUp) => match mouse_event_payload.mouse_event_payload {
                Some(mouse_event_payload::MouseEventPayload::LineCount(line_count)) => {
                    Ok(Mouse::ScrollUp(line_count as usize))
                },
                _ => Err("Malformed payload for mouse scroll up"),
            },
            Ok(MouseEventName::MouseScrollDown) => match mouse_event_payload.mouse_event_payload {
                Some(mouse_event_payload::MouseEventPayload::LineCount(line_count)) => {
                    Ok(Mouse::ScrollDown(line_count as usize))
                },
                _ => Err("Malformed payload for mouse scroll down"),
            },
            Ok(MouseEventName::MouseLeftClick) => match mouse_event_payload.mouse_event_payload {
                Some(mouse_event_payload::MouseEventPayload::Position(position)) => Ok(
                    Mouse::LeftClick(position.line as isize, position.column as usize),
                ),
                _ => Err("Malformed payload for mouse left click"),
            },
            Ok(MouseEventName::MouseRightClick) => match mouse_event_payload.mouse_event_payload {
                Some(mouse_event_payload::MouseEventPayload::Position(position)) => Ok(
                    Mouse::RightClick(position.line as isize, position.column as usize),
                ),
                _ => Err("Malformed payload for mouse right click"),
            },
            Ok(MouseEventName::MouseHold) => match mouse_event_payload.mouse_event_payload {
                Some(mouse_event_payload::MouseEventPayload::Position(position)) => Ok(
                    Mouse::Hold(position.line as isize, position.column as usize),
                ),
                _ => Err("Malformed payload for mouse hold"),
            },
            Ok(MouseEventName::MouseRelease) => match mouse_event_payload.mouse_event_payload {
                Some(mouse_event_payload::MouseEventPayload::Position(position)) => Ok(
                    Mouse::Release(position.line as isize, position.column as usize),
                ),
                _ => Err("Malformed payload for mouse release"),
            },
            Ok(MouseEventName::MouseHover) => match mouse_event_payload.mouse_event_payload {
                Some(mouse_event_payload::MouseEventPayload::Position(position)) => Ok(
                    Mouse::Hover(position.line as isize, position.column as usize),
                ),
                _ => Err("Malformed payload for mouse hover"),
            },
            Ok(MouseEventName::MouseScrollLeft) => match mouse_event_payload.mouse_event_payload {
                Some(mouse_event_payload::MouseEventPayload::LineCount(line_count)) => {
                    Ok(Mouse::ScrollLeft(line_count as usize))
                },
                _ => Err("Malformed payload for mouse scroll left"),
            },
            Ok(MouseEventName::MouseScrollRight) => match mouse_event_payload.mouse_event_payload {
                Some(mouse_event_payload::MouseEventPayload::LineCount(line_count)) => {
                    Ok(Mouse::ScrollRight(line_count as usize))
                },
                _ => Err("Malformed payload for mouse scroll right"),
            },
            Err(_) => Err("Malformed payload for MouseEventName"),
        }
    }
}

impl TryFrom<Mouse> for MouseEventPayload {
    type Error = &'static str;
    fn try_from(mouse: Mouse) -> Result<Self, &'static str> {
        match mouse {
            Mouse::ScrollUp(number_of_lines) => Ok(MouseEventPayload {
                mouse_event_name: MouseEventName::MouseScrollUp as i32,
                mouse_event_payload: Some(mouse_event_payload::MouseEventPayload::LineCount(
                    number_of_lines as u32,
                )),
            }),
            Mouse::ScrollDown(number_of_lines) => Ok(MouseEventPayload {
                mouse_event_name: MouseEventName::MouseScrollDown as i32,
                mouse_event_payload: Some(mouse_event_payload::MouseEventPayload::LineCount(
                    number_of_lines as u32,
                )),
            }),
            Mouse::LeftClick(line, column) => Ok(MouseEventPayload {
                mouse_event_name: MouseEventName::MouseLeftClick as i32,
                mouse_event_payload: Some(mouse_event_payload::MouseEventPayload::Position(
                    ProtobufPosition {
                        line: line as i64,
                        column: column as i64,
                    },
                )),
            }),
            Mouse::RightClick(line, column) => Ok(MouseEventPayload {
                mouse_event_name: MouseEventName::MouseRightClick as i32,
                mouse_event_payload: Some(mouse_event_payload::MouseEventPayload::Position(
                    ProtobufPosition {
                        line: line as i64,
                        column: column as i64,
                    },
                )),
            }),
            Mouse::Hold(line, column) => Ok(MouseEventPayload {
                mouse_event_name: MouseEventName::MouseHold as i32,
                mouse_event_payload: Some(mouse_event_payload::MouseEventPayload::Position(
                    ProtobufPosition {
                        line: line as i64,
                        column: column as i64,
                    },
                )),
            }),
            Mouse::Release(line, column) => Ok(MouseEventPayload {
                mouse_event_name: MouseEventName::MouseRelease as i32,
                mouse_event_payload: Some(mouse_event_payload::MouseEventPayload::Position(
                    ProtobufPosition {
                        line: line as i64,
                        column: column as i64,
                    },
                )),
            }),
            Mouse::Hover(line, column) => Ok(MouseEventPayload {
                mouse_event_name: MouseEventName::MouseHover as i32,
                mouse_event_payload: Some(mouse_event_payload::MouseEventPayload::Position(
                    ProtobufPosition {
                        line: line as i64,
                        column: column as i64,
                    },
                )),
            }),
            Mouse::ScrollLeft(cols) => Ok(MouseEventPayload {
                mouse_event_name: MouseEventName::MouseScrollLeft as i32,
                mouse_event_payload: Some(mouse_event_payload::MouseEventPayload::LineCount(
                    cols as u32,
                )),
            }),
            Mouse::ScrollRight(cols) => Ok(MouseEventPayload {
                mouse_event_name: MouseEventName::MouseScrollRight as i32,
                mouse_event_payload: Some(mouse_event_payload::MouseEventPayload::LineCount(
                    cols as u32,
                )),
            }),
        }
    }
}

impl TryFrom<ProtobufAgentInfo> for AgentInfo {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufAgentInfo) -> Result<Self, &'static str> {
        let state = match ProtobufAgentState::try_from(protobuf.state) {
            Ok(ProtobufAgentState::Busy) => AgentState::Busy,
            Ok(ProtobufAgentState::Waiting) => AgentState::Waiting,
            Ok(ProtobufAgentState::Idle) => AgentState::Idle,
            Ok(ProtobufAgentState::Exited) => AgentState::Exited,
            Err(_) => return Err("unknown agent state"),
        };
        let source = match ProtobufAgentSource::try_from(protobuf.source) {
            Ok(ProtobufAgentSource::Spawned) => AgentSource::Spawned,
            Ok(ProtobufAgentSource::Reported) => AgentSource::Reported,
            Ok(ProtobufAgentSource::Registered) => AgentSource::Registered,
            Ok(ProtobufAgentSource::Detected) => AgentSource::Detected,
            Err(_) => return Err("unknown agent source"),
        };
        Ok(AgentInfo {
            kind: protobuf.kind,
            state,
            source,
            detail: protobuf.detail,
            waiting_on: protobuf.waiting_on,
            wait_needs_a_person: protobuf.wait_needs_a_person,
            cwd: protobuf.cwd,
            session_id: protobuf.session_id,
            run_id: protobuf.run_id,
        })
    }
}

impl TryFrom<AgentInfo> for ProtobufAgentInfo {
    type Error = &'static str;
    fn try_from(agent: AgentInfo) -> Result<Self, &'static str> {
        let state = match agent.state {
            AgentState::Busy => ProtobufAgentState::Busy,
            AgentState::Waiting => ProtobufAgentState::Waiting,
            AgentState::Idle => ProtobufAgentState::Idle,
            AgentState::Exited => ProtobufAgentState::Exited,
        };
        let source = match agent.source {
            AgentSource::Spawned => ProtobufAgentSource::Spawned,
            AgentSource::Reported => ProtobufAgentSource::Reported,
            AgentSource::Registered => ProtobufAgentSource::Registered,
            AgentSource::Detected => ProtobufAgentSource::Detected,
        };
        Ok(ProtobufAgentInfo {
            kind: agent.kind,
            state: state as i32,
            source: source as i32,
            detail: agent.detail,
            waiting_on: agent.waiting_on,
            wait_needs_a_person: agent.wait_needs_a_person,
            cwd: agent.cwd,
            session_id: agent.session_id,
            run_id: agent.run_id,
        })
    }
}

impl TryFrom<ProtobufPaneInfo> for PaneInfo {
    type Error = &'static str;
    fn try_from(protobuf_pane_info: ProtobufPaneInfo) -> Result<Self, &'static str> {
        Ok(PaneInfo {
            pane_tabs: protobuf_pane_info
                .pane_tabs_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .map_err(|_| "Invalid pane tabs metadata")?,
            multipane: protobuf_pane_info
                .multipane
                .map(TryInto::try_into)
                .transpose()?,
            id: yinz_types::pane_id::PaneId::from_legacy(
                protobuf_pane_info.id,
                protobuf_pane_info.is_plugin,
            ),
            is_focused: protobuf_pane_info.is_focused,
            is_fullscreen: protobuf_pane_info.is_fullscreen,
            is_floating: protobuf_pane_info.is_floating,
            is_suppressed: protobuf_pane_info.is_suppressed,
            is_stack_list_member: protobuf_pane_info.is_stack_list_member,
            title: protobuf_pane_info.title,
            display_title: protobuf_pane_info.display_title,
            exited: protobuf_pane_info.exited,
            exit_status: protobuf_pane_info.exit_status,
            is_held: protobuf_pane_info.is_held,
            restart_error: protobuf_pane_info.restart_error,
            last_agent_session_id: protobuf_pane_info.last_agent_session_id,
            last_hook_at_ms: protobuf_pane_info.last_hook_at_ms,
            pane_x: Column::try_from(protobuf_pane_info.pane_x)
                .map_err(|_| "pane_x exceeds usize")?,
            pane_content_x: Column::try_from(protobuf_pane_info.pane_content_x)
                .map_err(|_| "pane_content_x exceeds usize")?,
            pane_y: Row::try_from(protobuf_pane_info.pane_y).map_err(|_| "pane_y exceeds usize")?,
            pane_content_y: Row::try_from(protobuf_pane_info.pane_content_y)
                .map_err(|_| "pane_content_y exceeds usize")?,
            pane_rows: RowCount::try_from(protobuf_pane_info.pane_rows)
                .map_err(|_| "pane_rows exceeds usize")?,
            pane_content_rows: RowCount::try_from(protobuf_pane_info.pane_content_rows)
                .map_err(|_| "pane_content_rows exceeds usize")?,
            pane_columns: ColumnCount::try_from(protobuf_pane_info.pane_columns)
                .map_err(|_| "pane_columns exceeds usize")?,
            pane_content_columns: ColumnCount::try_from(protobuf_pane_info.pane_content_columns)
                .map_err(|_| "pane_content_columns exceeds usize")?,
            cursor_coordinates_in_pane: protobuf_pane_info
                .cursor_coordinates_in_pane
                .map(TryInto::try_into)
                .transpose()?,
            terminal_command: protobuf_pane_info.terminal_command,
            plugin_url: protobuf_pane_info.plugin_url,
            is_selectable: protobuf_pane_info.is_selectable,
            index_in_pane_group: protobuf_pane_info
                .index_in_pane_group
                .iter()
                .map(|index_in_pane_group| {
                    Ok((
                        ClientId::new(index_in_pane_group.client_id),
                        index_in_pane_group.index as usize,
                    ))
                })
                .collect::<Result<_, &'static str>>()?,
            default_fg: protobuf_pane_info.default_fg,
            default_bg: protobuf_pane_info.default_bg,
            cwd: protobuf_pane_info.cwd,
            target: match protobuf_pane_info.target {
                Some(target) => PaneTarget::try_from(target)?,
                None => PaneTarget::parse_display(protobuf_pane_info.host.as_deref()),
            },
            host: protobuf_pane_info.host,
            agent: protobuf_pane_info
                .agent
                .map(AgentInfo::try_from)
                .transpose()?,
        })
    }
}

impl TryFrom<PaneInfo> for ProtobufPaneInfo {
    type Error = &'static str;
    fn try_from(pane_info: PaneInfo) -> Result<Self, &'static str> {
        Ok(ProtobufPaneInfo {
            pane_tabs_json: pane_info
                .pane_tabs
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|_| "Failed to encode pane tabs metadata")?,
            multipane: pane_info.multipane.clone().map(Into::into),
            id: pane_info.id.legacy_parts().0,
            is_plugin: pane_info.is_plugin(),
            is_focused: pane_info.is_focused,
            is_fullscreen: pane_info.is_fullscreen,
            is_floating: pane_info.is_floating,
            is_suppressed: pane_info.is_suppressed,
            is_stack_list_member: pane_info.is_stack_list_member,
            title: pane_info.title,
            display_title: pane_info.display_title,
            exited: pane_info.exited,
            exit_status: pane_info.exit_status,
            is_held: pane_info.is_held,
            restart_error: pane_info.restart_error,
            last_agent_session_id: pane_info.last_agent_session_id,
            last_hook_at_ms: pane_info.last_hook_at_ms,
            pane_x: u32::try_from(pane_info.pane_x).map_err(|_| "pane_x exceeds u32")?,
            pane_content_x: u32::try_from(pane_info.pane_content_x)
                .map_err(|_| "pane_content_x exceeds u32")?,
            pane_y: u32::try_from(pane_info.pane_y).map_err(|_| "pane_y exceeds u32")?,
            pane_content_y: u32::try_from(pane_info.pane_content_y)
                .map_err(|_| "pane_content_y exceeds u32")?,
            pane_rows: u32::try_from(pane_info.pane_rows).map_err(|_| "pane_rows exceeds u32")?,
            pane_content_rows: u32::try_from(pane_info.pane_content_rows)
                .map_err(|_| "pane_content_rows exceeds u32")?,
            pane_columns: u32::try_from(pane_info.pane_columns)
                .map_err(|_| "pane_columns exceeds u32")?,
            pane_content_columns: u32::try_from(pane_info.pane_content_columns)
                .map_err(|_| "pane_content_columns exceeds u32")?,
            cursor_coordinates_in_pane: pane_info
                .cursor_coordinates_in_pane
                .map(TryInto::try_into)
                .transpose()?,
            terminal_command: pane_info.terminal_command,
            plugin_url: pane_info.plugin_url,
            is_selectable: pane_info.is_selectable,
            index_in_pane_group: pane_info
                .index_in_pane_group
                .iter()
                .map(|(&client_id, &index)| IndexInPaneGroup {
                    client_id: client_id.get(),
                    index: index as u32,
                })
                .collect(),
            default_fg: pane_info.default_fg,
            default_bg: pane_info.default_bg,
            cwd: pane_info.cwd,
            host: pane_info.host,
            target: Some(ProtobufPaneTarget::from(&pane_info.target)),
            agent: pane_info
                .agent
                .map(ProtobufAgentInfo::try_from)
                .transpose()?,
        })
    }
}

impl TryFrom<ProtobufTabInfo> for TabInfo {
    type Error = &'static str;
    fn try_from(protobuf_tab_info: ProtobufTabInfo) -> Result<Self, &'static str> {
        Ok(TabInfo {
            group: protobuf_tab_info
                .group
                .map(|g| yinz_types::tab_bar::TabGroupInfo {
                    id: g.id,
                    name: g.name,
                }),
            position: TabPosition::new(protobuf_tab_info.position as usize),
            name: protobuf_tab_info.name,
            active: protobuf_tab_info.active,
            panes_to_hide: protobuf_tab_info.panes_to_hide as usize,
            is_fullscreen_active: protobuf_tab_info.is_fullscreen_active,
            is_sync_panes_active: protobuf_tab_info.is_sync_panes_active,
            are_floating_panes_visible: protobuf_tab_info.are_floating_panes_visible,
            other_focused_clients: protobuf_tab_info
                .other_focused_clients
                .iter()
                .map(|c| ClientId::new(*c))
                .collect(),
            other_focused_client_slots: protobuf_tab_info
                .other_focused_client_slots
                .iter()
                .map(|slot| u8::try_from(*slot).ok().and_then(ColorSlot::new))
                .collect(),
            active_swap_layout_name: protobuf_tab_info.active_swap_layout_name,
            is_swap_layout_dirty: protobuf_tab_info.is_swap_layout_dirty,
            viewport_rows: RowCount::try_from(protobuf_tab_info.viewport_rows)
                .map_err(|_| "viewport_rows exceeds usize")?,
            viewport_columns: ColumnCount::try_from(protobuf_tab_info.viewport_columns)
                .map_err(|_| "viewport_columns exceeds usize")?,
            display_area_rows: RowCount::try_from(protobuf_tab_info.display_area_rows)
                .map_err(|_| "display_area_rows exceeds usize")?,
            display_area_columns: ColumnCount::try_from(protobuf_tab_info.display_area_columns)
                .map_err(|_| "display_area_columns exceeds usize")?,
            selectable_tiled_panes_count: protobuf_tab_info.selectable_tiled_panes_count as usize,
            selectable_floating_panes_count: protobuf_tab_info.selectable_floating_panes_count
                as usize,
            tab_id: TabId::try_from(u64::from(protobuf_tab_info.tab_id))
                .map_err(|_| "tab_id is outside usize range")?,
            has_bell_notification: protobuf_tab_info.has_bell_notification,
            is_flashing_bell: protobuf_tab_info.is_flashing_bell,
        })
    }
}

impl TryFrom<TabInfo> for ProtobufTabInfo {
    type Error = &'static str;
    fn try_from(tab_info: TabInfo) -> Result<Self, &'static str> {
        Ok(ProtobufTabInfo {
            group: tab_info
                .group
                .map(|g| crate::generated_api::api::action::TabGroup {
                    id: g.id,
                    name: g.name,
                }),
            position: u32::try_from(tab_info.position.get())
                .map_err(|_| "Tab position is outside u32 range")?,
            name: tab_info.name,
            active: tab_info.active,
            panes_to_hide: tab_info.panes_to_hide as u32,
            is_fullscreen_active: tab_info.is_fullscreen_active,
            is_sync_panes_active: tab_info.is_sync_panes_active,
            are_floating_panes_visible: tab_info.are_floating_panes_visible,
            other_focused_clients: tab_info
                .other_focused_clients
                .iter()
                .map(|c| c.get())
                .collect(),
            other_focused_client_slots: tab_info
                .other_focused_client_slots
                .iter()
                .map(|slot| slot.map_or(0, |slot| u32::from(slot.get())))
                .collect(),
            active_swap_layout_name: tab_info.active_swap_layout_name,
            is_swap_layout_dirty: tab_info.is_swap_layout_dirty,
            viewport_rows: u32::try_from(tab_info.viewport_rows)
                .map_err(|_| "viewport_rows exceeds u32")?,
            viewport_columns: u32::try_from(tab_info.viewport_columns)
                .map_err(|_| "viewport_columns exceeds u32")?,
            display_area_rows: u32::try_from(tab_info.display_area_rows)
                .map_err(|_| "display_area_rows exceeds u32")?,
            display_area_columns: u32::try_from(tab_info.display_area_columns)
                .map_err(|_| "display_area_columns exceeds u32")?,
            selectable_tiled_panes_count: tab_info.selectable_tiled_panes_count as u32,
            selectable_floating_panes_count: tab_info.selectable_floating_panes_count as u32,
            tab_id: u32::try_from(tab_info.tab_id.get())
                .map_err(|_| "tab_id is outside u32 range")?,
            has_bell_notification: tab_info.has_bell_notification,
            is_flashing_bell: tab_info.is_flashing_bell,
        })
    }
}

fn decode_keybinds(keybinds: Vec<ProtobufInputModeKeybinds>) -> KeybindsVec {
    keybinds
        .into_iter()
        .filter_map(|imk| {
            let mode: InputMode = ProtobufInputMode::try_from(imk.mode)
                .ok()?
                .try_into()
                .ok()?;
            let key_binds: Vec<(KeyWithModifier, Vec<Action>)> = imk
                .key_bind
                .into_iter()
                .filter_map(|kb| {
                    let key: KeyWithModifier = kb.key?.try_into().ok()?;
                    let actions: Vec<Action> = kb
                        .action
                        .into_iter()
                        .filter_map(|a| a.try_into().ok())
                        .collect();
                    Some((key, actions))
                })
                .collect();
            Some((mode, key_binds))
        })
        .collect()
}

impl TryFrom<ProtobufModeUpdatePayload> for ModeInfo {
    type Error = &'static str;
    fn try_from(
        protobuf_mode_update_payload: ProtobufModeUpdatePayload,
    ) -> Result<Self, &'static str> {
        let current_mode: InputMode =
            ProtobufInputMode::try_from(protobuf_mode_update_payload.current_mode)
                .map_err(|_| "Malformed InputMode in the ModeUpdate Event")?
                .try_into()?;
        let base_mode: Option<InputMode> = protobuf_mode_update_payload
            .base_mode
            .and_then(|b_m| ProtobufInputMode::try_from(b_m).ok()?.try_into().ok());
        let keybinds = decode_keybinds(protobuf_mode_update_payload.keybinds);
        let style: Style = protobuf_mode_update_payload
            .style
            .and_then(|m| m.try_into().ok())
            .ok_or("malformed payload for mode_info")?;
        let session_name = protobuf_mode_update_payload.session_name;
        let editor = protobuf_mode_update_payload.editor.map(PathBuf::from);
        let shell = protobuf_mode_update_payload.shell.map(PathBuf::from);
        let capabilities = PluginCapabilities {
            arrow_fonts: protobuf_mode_update_payload.arrow_fonts_support,
            kitty_graphics: protobuf_mode_update_payload.kitty_graphics_support,
        };
        let currently_marking_pane_group =
            protobuf_mode_update_payload.currently_marking_pane_group;
        let pane_frame_style = protobuf_mode_update_payload
            .pane_frame_style
            .and_then(|p| ProtobufPaneFrameStyle::try_from(p).ok())
            .map(|p| p.into());

        let session_dimmed = protobuf_mode_update_payload.session_dimmed;

        let session_ancestry = protobuf_mode_update_payload.ancestry;

        let host_fullscreen = protobuf_mode_update_payload.host_fullscreen;

        let nested_ascend_keys = protobuf_mode_update_payload
            .nested_ascend_keys
            .iter()
            .filter_map(|key| KeyWithModifier::from_str(key).ok())
            .collect();

        let session_ascended = protobuf_mode_update_payload.session_ascended;

        let nested_descend_keys = protobuf_mode_update_payload
            .nested_descend_keys
            .iter()
            .filter_map(|key| KeyWithModifier::from_str(key).ok())
            .collect();

        let mode_info = ModeInfo {
            max_agent_width: protobuf_mode_update_payload
                .max_agent_width
                .map(usize::try_from)
                .transpose()
                .map_err(|_| "agent width exceeds platform size")?,
            tab_groups: protobuf_mode_update_payload.tab_groups,
            mode: current_mode,
            keybinds,
            style,
            capabilities,
            session_name: session_name.map(Into::into),
            base_mode,
            editor,
            shell,
            currently_marking_pane_group,
            pane_frame_style,
            session_dimmed,
            session_ancestry: session_ancestry.into_iter().map(Into::into).collect(),
            host_fullscreen,
            nested_ascend_keys,
            session_ascended,
            nested_descend_keys,
        };
        Ok(mode_info)
    }
}

impl TryFrom<ModeInfo> for ProtobufModeUpdatePayload {
    type Error = &'static str;
    fn try_from(mode_info: ModeInfo) -> Result<Self, &'static str> {
        let current_mode: ProtobufInputMode = mode_info.mode.try_into()?;
        let base_mode: Option<ProtobufInputMode> = mode_info
            .base_mode
            .and_then(|mode| ProtobufInputMode::try_from(mode).ok());
        let style: ProtobufStyle = mode_info.style.try_into()?;
        let arrow_fonts_support: bool = mode_info.capabilities.arrow_fonts;
        let kitty_graphics_support: bool = mode_info.capabilities.kitty_graphics;
        let session_name = mode_info.session_name;
        let editor = mode_info.editor.map(|e| e.display().to_string());
        let shell = mode_info.shell.map(|s| s.display().to_string());
        let currently_marking_pane_group = mode_info.currently_marking_pane_group;
        let pane_frame_style = mode_info.pane_frame_style.map(|p| {
            let protobuf_pane_frame_style: ProtobufPaneFrameStyle = p.into();
            protobuf_pane_frame_style as i32
        });
        let session_dimmed = mode_info.session_dimmed;
        let session_ancestry = mode_info.session_ancestry;
        let host_fullscreen = mode_info.host_fullscreen;
        let nested_ascend_keys = mode_info
            .nested_ascend_keys
            .iter()
            .map(|key| key.to_keybind_string())
            .collect();
        let session_ascended = mode_info.session_ascended;
        let nested_descend_keys = mode_info
            .nested_descend_keys
            .iter()
            .map(|key| key.to_keybind_string())
            .collect();
        let mut protobuf_input_mode_keybinds: Vec<ProtobufInputModeKeybinds> = vec![];
        for (input_mode, input_mode_keybinds) in mode_info.keybinds {
            let mode: ProtobufInputMode = input_mode.try_into()?;
            let mut keybinds: Vec<ProtobufKeyBind> = vec![];
            for (key, actions) in input_mode_keybinds {
                let protobuf_key: ProtobufKey = key.try_into()?;
                let mut protobuf_actions: Vec<ProtobufAction> = vec![];
                for action in actions {
                    if let Ok(protobuf_action) = action.try_into() {
                        protobuf_actions.push(protobuf_action);
                    }
                }
                let key_bind = ProtobufKeyBind {
                    key: Some(protobuf_key),
                    action: protobuf_actions,
                };
                keybinds.push(key_bind);
            }
            let input_mode_keybind = ProtobufInputModeKeybinds {
                mode: mode as i32,
                key_bind: keybinds,
            };
            protobuf_input_mode_keybinds.push(input_mode_keybind);
        }
        Ok(ProtobufModeUpdatePayload {
            max_agent_width: mode_info.max_agent_width.map(|width| width as u64),
            tab_groups: mode_info.tab_groups,
            current_mode: current_mode as i32,
            style: Some(style),
            keybinds: protobuf_input_mode_keybinds,
            arrow_fonts_support,
            kitty_graphics_support,
            session_name: session_name.map(|name| name.into_string()),
            base_mode: base_mode.map(|b_m| b_m as i32),
            editor,
            shell,
            currently_marking_pane_group,
            pane_frame_style,
            session_dimmed,
            ancestry: session_ancestry
                .into_iter()
                .map(|name| name.into_string())
                .collect(),
            host_fullscreen,
            nested_ascend_keys,
            session_ascended,
            nested_descend_keys,
        })
    }
}

impl TryFrom<ProtobufEventNameList> for HashSet<EventType> {
    type Error = &'static str;
    fn try_from(protobuf_event_name_list: ProtobufEventNameList) -> Result<Self, &'static str> {
        let event_types: Vec<ProtobufEventType> = protobuf_event_name_list
            .event_types
            .iter()
            .filter_map(|i| ProtobufEventType::try_from(*i).ok())
            .collect();
        let event_types: Vec<EventType> = event_types
            .iter()
            .filter_map(|e| EventType::try_from(*e).ok())
            .collect();
        Ok(event_types.into_iter().collect())
    }
}

impl TryFrom<HashSet<EventType>> for ProtobufEventNameList {
    type Error = &'static str;
    fn try_from(event_types: HashSet<EventType>) -> Result<Self, &'static str> {
        let protobuf_event_name_list = ProtobufEventNameList {
            event_types: event_types
                .iter()
                .filter_map(|e| ProtobufEventType::try_from(*e).ok())
                .map(|e| e as i32)
                .collect(),
        };
        Ok(protobuf_event_name_list)
    }
}

impl TryFrom<ProtobufEventType> for EventType {
    type Error = &'static str;
    fn try_from(protobuf_event_type: ProtobufEventType) -> Result<Self, &'static str> {
        Ok(match protobuf_event_type {
            ProtobufEventType::ModeUpdate => EventType::ModeUpdate,
            ProtobufEventType::TabUpdate => EventType::TabUpdate,
            ProtobufEventType::PaneUpdate => EventType::PaneUpdate,
            ProtobufEventType::Key => EventType::Key,
            ProtobufEventType::Mouse => EventType::Mouse,
            ProtobufEventType::Timer => EventType::Timer,
            ProtobufEventType::CopyToClipboard => EventType::CopyToClipboard,
            ProtobufEventType::SystemClipboardFailure => EventType::SystemClipboardFailure,
            ProtobufEventType::InputReceived => EventType::InputReceived,
            ProtobufEventType::Visible => EventType::Visible,
            ProtobufEventType::CustomMessage => EventType::CustomMessage,
            ProtobufEventType::FileSystemCreate => EventType::FileSystemCreate,
            ProtobufEventType::FileSystemRead => EventType::FileSystemRead,
            ProtobufEventType::FileSystemUpdate => EventType::FileSystemUpdate,
            ProtobufEventType::FileSystemDelete => EventType::FileSystemDelete,
            ProtobufEventType::PermissionRequestResult => EventType::PermissionRequestResult,
            ProtobufEventType::SessionUpdate => EventType::SessionUpdate,
            ProtobufEventType::PaneTabsUpdate => EventType::PaneTabsUpdate,
            ProtobufEventType::PaneOutput => EventType::PaneOutput,
            ProtobufEventType::RunCommandOutput => EventType::RunCommandOutput,
            ProtobufEventType::RunCommandResult => EventType::RunCommandResult,
            ProtobufEventType::WebRequestResult => EventType::WebRequestResult,
            ProtobufEventType::CommandPaneOpened => EventType::CommandPaneOpened,
            ProtobufEventType::CommandPaneExited => EventType::CommandPaneExited,
            ProtobufEventType::PaneClosed => EventType::PaneClosed,
            ProtobufEventType::EditPaneOpened => EventType::EditPaneOpened,
            ProtobufEventType::EditPaneExited => EventType::EditPaneExited,
            ProtobufEventType::CommandPaneReRun => EventType::CommandPaneReRun,
            ProtobufEventType::FailedToWriteConfigToDisk => EventType::FailedToWriteConfigToDisk,
            ProtobufEventType::ListClients => EventType::ListClients,
            ProtobufEventType::HostFolderChanged => EventType::HostFolderChanged,
            ProtobufEventType::FailedToChangeHostFolder => EventType::FailedToChangeHostFolder,
            ProtobufEventType::PastedText => EventType::PastedText,
            ProtobufEventType::ConfigWasWrittenToDisk => EventType::ConfigWasWrittenToDisk,
            ProtobufEventType::CellPixelSizeChanged => EventType::CellPixelSizeChanged,
            ProtobufEventType::BeforeClose => EventType::BeforeClose,
            ProtobufEventType::InterceptedKeyPress => EventType::InterceptedKeyPress,
            ProtobufEventType::PaneRenderReport => EventType::PaneRenderReport,
            ProtobufEventType::UserAction => EventType::UserAction,
            ProtobufEventType::ActionComplete => EventType::ActionComplete,
            ProtobufEventType::CwdChanged => EventType::CwdChanged,
            ProtobufEventType::CommandChanged => EventType::CommandChanged,
            ProtobufEventType::AvailableLayoutInfo => EventType::AvailableLayoutInfo,
            ProtobufEventType::PluginConfigurationChanged => EventType::PluginConfigurationChanged,
            ProtobufEventType::HighlightClicked => EventType::HighlightClicked,
            ProtobufEventType::ContextMenuRequested => EventType::ContextMenuRequested,
            ProtobufEventType::ContextMenuItemSelected => EventType::ContextMenuItemSelected,
            ProtobufEventType::InitialKeybinds => EventType::InitialKeybinds,
            ProtobufEventType::HostTerminalThemeChanged => EventType::HostTerminalThemeChanged,
            ProtobufEventType::HintText => EventType::HintText,
            ProtobufEventType::ActivePaneScroll => EventType::ActivePaneScroll,
            ProtobufEventType::PluginDataChanged => EventType::PluginDataChanged,
            ProtobufEventType::WebRequestStreamChunk => EventType::WebRequestStreamChunk,
            ProtobufEventType::NotificationReceived => EventType::NotificationReceived,
            ProtobufEventType::NotificationRequested => EventType::NotificationRequested,
            ProtobufEventType::NotificationResolved => EventType::NotificationResolved,
            ProtobufEventType::Hook => EventType::Hook,
            ProtobufEventType::AgentEvent => EventType::AgentEvent,
            ProtobufEventType::PickResult => EventType::PickResult,
            ProtobufEventType::AgentProvidersChanged => EventType::AgentProvidersChanged,
            ProtobufEventType::ProjectProvidersChanged => EventType::ProjectProvidersChanged,
            ProtobufEventType::ModifiersHeld => EventType::ModifiersHeld,
        })
    }
}

impl TryFrom<EventType> for ProtobufEventType {
    type Error = &'static str;
    fn try_from(event_type: EventType) -> Result<Self, &'static str> {
        Ok(match event_type {
            EventType::ModeUpdate => ProtobufEventType::ModeUpdate,
            EventType::TabUpdate => ProtobufEventType::TabUpdate,
            EventType::PaneUpdate => ProtobufEventType::PaneUpdate,
            EventType::Key => ProtobufEventType::Key,
            EventType::Mouse => ProtobufEventType::Mouse,
            EventType::Timer => ProtobufEventType::Timer,
            EventType::CopyToClipboard => ProtobufEventType::CopyToClipboard,
            EventType::SystemClipboardFailure => ProtobufEventType::SystemClipboardFailure,
            EventType::InputReceived => ProtobufEventType::InputReceived,
            EventType::Visible => ProtobufEventType::Visible,
            EventType::CustomMessage => ProtobufEventType::CustomMessage,
            EventType::FileSystemCreate => ProtobufEventType::FileSystemCreate,
            EventType::FileSystemRead => ProtobufEventType::FileSystemRead,
            EventType::FileSystemUpdate => ProtobufEventType::FileSystemUpdate,
            EventType::FileSystemDelete => ProtobufEventType::FileSystemDelete,
            EventType::PermissionRequestResult => ProtobufEventType::PermissionRequestResult,
            EventType::SessionUpdate => ProtobufEventType::SessionUpdate,
            EventType::PaneTabsUpdate => ProtobufEventType::PaneTabsUpdate,
            EventType::PaneOutput => ProtobufEventType::PaneOutput,
            EventType::RunCommandOutput => ProtobufEventType::RunCommandOutput,
            EventType::RunCommandResult => ProtobufEventType::RunCommandResult,
            EventType::WebRequestResult => ProtobufEventType::WebRequestResult,
            EventType::CommandPaneOpened => ProtobufEventType::CommandPaneOpened,
            EventType::CommandPaneExited => ProtobufEventType::CommandPaneExited,
            EventType::PaneClosed => ProtobufEventType::PaneClosed,
            EventType::EditPaneOpened => ProtobufEventType::EditPaneOpened,
            EventType::EditPaneExited => ProtobufEventType::EditPaneExited,
            EventType::CommandPaneReRun => ProtobufEventType::CommandPaneReRun,
            EventType::FailedToWriteConfigToDisk => ProtobufEventType::FailedToWriteConfigToDisk,
            EventType::ListClients => ProtobufEventType::ListClients,
            EventType::HostFolderChanged => ProtobufEventType::HostFolderChanged,
            EventType::FailedToChangeHostFolder => ProtobufEventType::FailedToChangeHostFolder,
            EventType::PastedText => ProtobufEventType::PastedText,
            EventType::ConfigWasWrittenToDisk => ProtobufEventType::ConfigWasWrittenToDisk,
            EventType::CellPixelSizeChanged => ProtobufEventType::CellPixelSizeChanged,
            EventType::BeforeClose => ProtobufEventType::BeforeClose,
            EventType::InterceptedKeyPress => ProtobufEventType::InterceptedKeyPress,
            EventType::PaneRenderReport => ProtobufEventType::PaneRenderReport,
            EventType::UserAction => ProtobufEventType::UserAction,
            EventType::ActionComplete => ProtobufEventType::ActionComplete,
            EventType::CwdChanged => ProtobufEventType::CwdChanged,
            EventType::CommandChanged => ProtobufEventType::CommandChanged,
            EventType::AvailableLayoutInfo => ProtobufEventType::AvailableLayoutInfo,
            EventType::PluginConfigurationChanged => ProtobufEventType::PluginConfigurationChanged,
            EventType::HighlightClicked => ProtobufEventType::HighlightClicked,
            EventType::ContextMenuRequested => ProtobufEventType::ContextMenuRequested,
            EventType::ContextMenuItemSelected => ProtobufEventType::ContextMenuItemSelected,
            EventType::InitialKeybinds => ProtobufEventType::InitialKeybinds,
            EventType::HostTerminalThemeChanged => ProtobufEventType::HostTerminalThemeChanged,
            EventType::HintText => ProtobufEventType::HintText,
            EventType::ActivePaneScroll => ProtobufEventType::ActivePaneScroll,
            EventType::PluginDataChanged => ProtobufEventType::PluginDataChanged,
            EventType::WebRequestStreamChunk => ProtobufEventType::WebRequestStreamChunk,
            EventType::NotificationReceived => ProtobufEventType::NotificationReceived,
            EventType::NotificationRequested => ProtobufEventType::NotificationRequested,
            EventType::NotificationResolved => ProtobufEventType::NotificationResolved,
            EventType::Hook => ProtobufEventType::Hook,
            EventType::AgentEvent => ProtobufEventType::AgentEvent,
            EventType::PickResult => ProtobufEventType::PickResult,
            EventType::AgentProvidersChanged => ProtobufEventType::AgentProvidersChanged,
            EventType::ProjectProvidersChanged => ProtobufEventType::ProjectProvidersChanged,
            EventType::ModifiersHeld => ProtobufEventType::ModifiersHeld,
        })
    }
}

impl From<StyledText> for ProtobufStyledText {
    fn from(styled_text: StyledText) -> Self {
        ProtobufStyledText {
            text: styled_text.text,
            indices: styled_text
                .indices
                .into_iter()
                .map(|inner| ProtobufStyledTextIndices {
                    indices: inner.into_iter().map(|i| i as u32).collect(),
                })
                .collect(),
        }
    }
}

impl From<ProtobufStyledText> for StyledText {
    fn from(styled_text: ProtobufStyledText) -> Self {
        StyledText {
            text: styled_text.text,
            indices: styled_text
                .indices
                .into_iter()
                .map(|inner| inner.indices.into_iter().map(|i| i as usize).collect())
                .collect(),
        }
    }
}

impl From<HostTerminalThemeMode> for ProtobufHostTerminalThemeIndication {
    fn from(mode: HostTerminalThemeMode) -> Self {
        match mode {
            HostTerminalThemeMode::Dark => ProtobufHostTerminalThemeIndication::Dark,
            HostTerminalThemeMode::Light => ProtobufHostTerminalThemeIndication::Light,
        }
    }
}

impl From<ProtobufHostTerminalThemeIndication> for HostTerminalThemeMode {
    fn from(mode: ProtobufHostTerminalThemeIndication) -> Self {
        match mode {
            ProtobufHostTerminalThemeIndication::Dark => HostTerminalThemeMode::Dark,
            ProtobufHostTerminalThemeIndication::Light => HostTerminalThemeMode::Light,
        }
    }
}

impl From<ProtobufResurrectableSession> for (SessionName, Duration) {
    fn from(
        protobuf_resurrectable_session: ProtobufResurrectableSession,
    ) -> (SessionName, Duration) {
        (
            protobuf_resurrectable_session.name.into(),
            Duration::from_secs(protobuf_resurrectable_session.creation_time),
        )
    }
}

impl From<(SessionName, Duration)> for ProtobufResurrectableSession {
    fn from(
        session_name_and_creation_time: (SessionName, Duration),
    ) -> ProtobufResurrectableSession {
        ProtobufResurrectableSession {
            name: session_name_and_creation_time.0.to_string(),
            creation_time: session_name_and_creation_time.1.as_secs(),
        }
    }
}

impl From<&ProtobufFileMetadata> for Option<FileMetadata> {
    fn from(protobuf_file_metadata: &ProtobufFileMetadata) -> Option<FileMetadata> {
        if protobuf_file_metadata.metadata_is_set {
            Some(FileMetadata {
                is_file: protobuf_file_metadata.is_file,
                is_dir: protobuf_file_metadata.is_dir,
                is_symlink: protobuf_file_metadata.is_symlink,
                len: protobuf_file_metadata.len,
            })
        } else {
            None
        }
    }
}

impl From<Option<FileMetadata>> for ProtobufFileMetadata {
    fn from(file_metadata: Option<FileMetadata>) -> ProtobufFileMetadata {
        match file_metadata {
            Some(file_metadata) => ProtobufFileMetadata {
                metadata_is_set: true,
                is_file: file_metadata.is_file,
                is_dir: file_metadata.is_dir,
                is_symlink: file_metadata.is_symlink,
                len: file_metadata.len,
            },
            None => ProtobufFileMetadata {
                metadata_is_set: false,
                ..Default::default()
            },
        }
    }
}

// note: ProtobufPaneId and ProtobufPaneType are not the same as the ones defined in plugin_command.rs
// this is a duplicate type - we are forced to do this because protobuffs do not support recursive
// imports
impl TryFrom<ProtobufPaneId> for PaneId {
    type Error = &'static str;
    fn try_from(protobuf_pane_id: ProtobufPaneId) -> Result<Self, &'static str> {
        match ProtobufPaneType::try_from(protobuf_pane_id.pane_type) {
            Ok(ProtobufPaneType::Terminal) => Ok(PaneId::Terminal(
                yinz_types::internal_ids::TerminalId::new(protobuf_pane_id.id),
            )),
            Ok(ProtobufPaneType::Plugin) => Ok(PaneId::Plugin(
                yinz_types::internal_ids::PluginId::new(protobuf_pane_id.id),
            )),
            Err(_) => Err("Failed to convert PaneId"),
        }
    }
}

// note: ProtobufPaneId and ProtobufPaneType are not the same as the ones defined in plugin_command.rs
// this is a duplicate type - we are forced to do this because protobuffs do not support recursive
// imports
impl TryFrom<PaneId> for ProtobufPaneId {
    type Error = &'static str;
    fn try_from(pane_id: PaneId) -> Result<Self, &'static str> {
        match pane_id {
            PaneId::Terminal(id) => Ok(ProtobufPaneId {
                pane_type: ProtobufPaneType::Terminal as i32,
                id: id.get(),
            }),
            PaneId::Plugin(id) => Ok(ProtobufPaneId {
                pane_type: ProtobufPaneType::Plugin as i32,
                id: id.get(),
            }),
        }
    }
}

impl From<yinz_types::input::options::PaneFrameStyle> for ProtobufPaneFrameStyle {
    fn from(val: yinz_types::input::options::PaneFrameStyle) -> Self {
        match val {
            yinz_types::input::options::PaneFrameStyle::Full => ProtobufPaneFrameStyle::Full,
            yinz_types::input::options::PaneFrameStyle::Titles => ProtobufPaneFrameStyle::Titles,
            yinz_types::input::options::PaneFrameStyle::None => ProtobufPaneFrameStyle::None,
        }
    }
}

impl From<ProtobufPaneFrameStyle> for yinz_types::input::options::PaneFrameStyle {
    fn from(val: ProtobufPaneFrameStyle) -> Self {
        match val {
            ProtobufPaneFrameStyle::Full => yinz_types::input::options::PaneFrameStyle::Full,
            ProtobufPaneFrameStyle::Titles => yinz_types::input::options::PaneFrameStyle::Titles,
            ProtobufPaneFrameStyle::None => yinz_types::input::options::PaneFrameStyle::None,
        }
    }
}

impl TryFrom<ProtobufPaneRenderReportPayload> for HashMap<PaneId, PaneContents> {
    type Error = &'static str;
    fn try_from(protobuf_payload: ProtobufPaneRenderReportPayload) -> Result<Self, &'static str> {
        let mut pane_contents_map = HashMap::new();

        for entry in protobuf_payload.pane_contents {
            let pane_id = entry
                .pane_id
                .ok_or("Missing pane_id in PaneContentsEntry")?
                .try_into()?;
            let pane_contents = entry
                .pane_contents
                .ok_or("Missing pane_contents in PaneContentsEntry")?
                .try_into()?;
            pane_contents_map.insert(pane_id, pane_contents);
        }

        Ok(pane_contents_map)
    }
}

impl TryFrom<HashMap<PaneId, PaneContents>> for ProtobufPaneRenderReportPayload {
    type Error = &'static str;
    fn try_from(pane_contents_map: HashMap<PaneId, PaneContents>) -> Result<Self, &'static str> {
        let mut pane_contents_vec = vec![];

        for (pane_id, pane_contents) in pane_contents_map {
            pane_contents_vec.push(ProtobufPaneContentsEntry {
                pane_id: Some(pane_id.try_into()?),
                pane_contents: Some(pane_contents.try_into()?),
            });
        }

        Ok(ProtobufPaneRenderReportPayload {
            pane_contents: pane_contents_vec,
        })
    }
}

impl TryFrom<ProtobufPaneContents> for PaneContents {
    type Error = &'static str;
    fn try_from(protobuf_contents: ProtobufPaneContents) -> Result<Self, &'static str> {
        let selected_text = protobuf_contents
            .selected_text
            .map(|st| st.try_into())
            .transpose()?;
        let cursor = protobuf_contents
            .cursor
            .map(TryInto::try_into)
            .transpose()?;

        Ok(PaneContents {
            viewport: protobuf_contents.viewport,
            selected_text,
            lines_above_viewport: protobuf_contents.lines_above_viewport,
            lines_below_viewport: protobuf_contents.lines_below_viewport,
            cursor,
            last_output_ms_ago: protobuf_contents
                .last_output_ms_ago
                .map(yinz_types::units::ElapsedMillis::new),
            is_scrolled: protobuf_contents.is_scrolled,
        })
    }
}

impl TryFrom<PaneContents> for ProtobufPaneContents {
    type Error = &'static str;
    fn try_from(pane_contents: PaneContents) -> Result<Self, &'static str> {
        let selected_text = pane_contents
            .selected_text
            .map(|st| st.try_into())
            .transpose()?;
        let cursor = pane_contents.cursor.map(TryInto::try_into).transpose()?;

        Ok(ProtobufPaneContents {
            viewport: pane_contents.viewport,
            selected_text,
            lines_above_viewport: pane_contents.lines_above_viewport,
            lines_below_viewport: pane_contents.lines_below_viewport,
            cursor,
            last_output_ms_ago: pane_contents
                .last_output_ms_ago
                .map(yinz_types::units::ElapsedMillis::get),
            is_scrolled: pane_contents.is_scrolled,
        })
    }
}

impl TryFrom<ProtobufPaneScrollbackResponse> for PaneScrollbackResponse {
    type Error = &'static str;
    fn try_from(protobuf_response: ProtobufPaneScrollbackResponse) -> Result<Self, &'static str> {
        match protobuf_response.response {
            Some(pane_scrollback_response::Response::Ok(pane_contents)) => {
                Ok(PaneScrollbackResponse::Ok(pane_contents.try_into()?))
            },
            Some(pane_scrollback_response::Response::Err(error_msg)) => {
                Ok(PaneScrollbackResponse::Err(error_msg))
            },
            None => Err("PaneScrollbackResponse missing response field"),
        }
    }
}

impl TryFrom<PaneScrollbackResponse> for ProtobufPaneScrollbackResponse {
    type Error = &'static str;
    fn try_from(response: PaneScrollbackResponse) -> Result<Self, &'static str> {
        let response_field = match response {
            PaneScrollbackResponse::Ok(pane_contents) => {
                pane_scrollback_response::Response::Ok(pane_contents.try_into()?)
            },
            PaneScrollbackResponse::Err(error_msg) => {
                pane_scrollback_response::Response::Err(error_msg)
            },
        };
        Ok(ProtobufPaneScrollbackResponse {
            response: Some(response_field),
        })
    }
}

impl TryFrom<ProtobufSelectedText> for SelectedText {
    type Error = &'static str;
    fn try_from(protobuf_selected_text: ProtobufSelectedText) -> Result<Self, &'static str> {
        Ok(SelectedText {
            start: protobuf_selected_text
                .start
                .ok_or("Missing start in SelectedText")?
                .try_into()?,
            end: protobuf_selected_text
                .end
                .ok_or("Missing end in SelectedText")?
                .try_into()?,
        })
    }
}

impl TryFrom<SelectedText> for ProtobufSelectedText {
    type Error = &'static str;
    fn try_from(selected_text: SelectedText) -> Result<Self, &'static str> {
        Ok(ProtobufSelectedText {
            start: Some(selected_text.start.try_into()?),
            end: Some(selected_text.end.try_into()?),
        })
    }
}

// serialize_{copy_to_clipboard,clipboard_failure,input_received,visible,
// custom_message,file_system_create,file_system_read,file_system_update,
// file_system_delete}_event were removed here: each was the same eight-line
// encode/decode body over one literal, and every one of those variants is in
// the sweep of tests::props::every_event_survives_a_protobuf_round_trip.
// serialize_timer_event stays as the exemplar of the shape, and because
// Timer's payload narrows to f32 - see the note on the generator.

/// `ContextMenuItem` and friends cross the plugin boundary the long way round, by hand, like
/// everything else in this file. Two shapes are worth noting:
///
/// - **Actions do not travel at all.** Several `Action` variants have no protobuf form, so the
///   wire carries a configured item's `id` and the server re-attaches its actions on the way back.
///   See `data::ContextMenuItem`.
/// - **`click` and `click_in_pane` are pairs of `uint32`, not a `Position`.** A menu's anchor is a
///   cell on the surface and is never negative; `Position`'s signed line would have to be validated
///   on the way back in for no gain.
impl From<ProtobufContextMenuItem> for ContextMenuItem {
    fn from(item: ProtobufContextMenuItem) -> Self {
        ContextMenuItem {
            id: item.id.map(ContextMenuItemId::new),
            label: item.label,
            shortcut: item.shortcut,
            // Empty by construction: the wire carries no actions, and the server re-attaches them
            // by `id` when it takes the plugin's answer.
            actions: vec![],
            context: item
                .context
                .into_iter()
                .map(|entry| (entry.name, entry.value))
                .collect(),
            disabled: item.disabled,
            separator: item.separator,
        }
    }
}

impl From<ContextMenuItem> for ProtobufContextMenuItem {
    fn from(item: ContextMenuItem) -> Self {
        ProtobufContextMenuItem {
            id: item.id.map(|id| id.get()),
            label: item.label,
            shortcut: item.shortcut,
            context: item
                .context
                .into_iter()
                .map(|(name, value)| ProtobufContextItem { name, value })
                .collect(),
            disabled: item.disabled,
            separator: item.separator,
        }
    }
}

impl From<ProtobufContextMenuLink> for ContextMenuLink {
    fn from(link: ProtobufContextMenuLink) -> Self {
        ContextMenuLink {
            plugin_id: yinz_types::internal_ids::PluginId::new(link.plugin_id),
            pattern: link.pattern,
            matched: link.matched,
            context: link
                .context
                .into_iter()
                .map(|entry| (entry.name, entry.value))
                .collect(),
        }
    }
}

impl From<ContextMenuLink> for ProtobufContextMenuLink {
    fn from(link: ContextMenuLink) -> Self {
        ProtobufContextMenuLink {
            plugin_id: link.plugin_id.get(),
            pattern: link.pattern,
            matched: link.matched,
            context: link
                .context
                .into_iter()
                .map(|(name, value)| ProtobufContextItem { name, value })
                .collect(),
        }
    }
}

impl TryFrom<ProtobufContextMenuContext> for ContextMenuContext {
    type Error = &'static str;
    fn try_from(context: ProtobufContextMenuContext) -> Result<Self, &'static str> {
        let pane_id = context
            .pane_id
            .ok_or("Missing pane_id in ContextMenuContext")?
            .try_into()?;
        let items = context.items.into_iter().map(Into::into).collect();
        Ok(ContextMenuContext {
            request_id: ContextMenuRequestId::new(context.request_id),
            client_id: ClientId::new(context.client_id),
            pane_id,
            tab_position: TabPosition::new(context.tab_position as usize),
            click: (context.click_x as usize, context.click_y as usize),
            click_in_pane: (
                context.click_in_pane_x as usize,
                context.click_in_pane_y as usize,
            ),
            on_frame: context.on_frame,
            pane_title: context.pane_title,
            pane_command: context.pane_command,
            pane_exited: context.pane_exited,
            pane_exit_status: context.pane_exit_status,
            pane_is_held: context.pane_is_held,
            pane_is_floating: context.pane_is_floating,
            selected_text: context.selected_text,
            word_under_cursor: context.word_under_cursor,
            line_under_cursor: context.line_under_cursor,
            link: context.link.map(Into::into),
            items,
        })
    }
}

impl TryFrom<ContextMenuContext> for ProtobufContextMenuContext {
    type Error = &'static str;
    fn try_from(context: ContextMenuContext) -> Result<Self, &'static str> {
        // `without_actions` rather than a plain conversion so the stripping is stated at the one
        // place the boundary is crossed, not buried in a `From` impl.
        let items = context
            .items
            .iter()
            .map(|item| item.without_actions().into())
            .collect();
        Ok(ProtobufContextMenuContext {
            request_id: context.request_id.get(),
            client_id: context.client_id.get(),
            pane_id: Some(context.pane_id.try_into()?),
            tab_position: u32::try_from(context.tab_position.get())
                .map_err(|_| "Tab position is outside u32 range")?,
            click_x: context.click.0 as u32,
            click_y: context.click.1 as u32,
            click_in_pane_x: context.click_in_pane.0 as u32,
            click_in_pane_y: context.click_in_pane.1 as u32,
            on_frame: context.on_frame,
            pane_title: context.pane_title,
            pane_command: context.pane_command,
            pane_exited: context.pane_exited,
            pane_exit_status: context.pane_exit_status,
            pane_is_held: context.pane_is_held,
            pane_is_floating: context.pane_is_floating,
            selected_text: context.selected_text,
            word_under_cursor: context.word_under_cursor,
            line_under_cursor: context.line_under_cursor,
            link: context.link.map(Into::into),
            items,
        })
    }
}

#[cfg(test)]
#[path = "unit/event_tests.rs"]
mod tests;

impl TryFrom<ProtobufPosition> for CellCoordinates {
    type Error = &'static str;
    fn try_from(value: ProtobufPosition) -> Result<Self, Self::Error> {
        Ok(Self::new(
            Column::try_from(value.column).map_err(|_| "cursor column outside usize range")?,
            Row::try_from(value.line).map_err(|_| "cursor row outside usize range")?,
        ))
    }
}
impl TryFrom<CellCoordinates> for ProtobufPosition {
    type Error = &'static str;
    fn try_from(value: CellCoordinates) -> Result<Self, Self::Error> {
        Ok(Self {
            column: i64::try_from(value.x).map_err(|_| "cursor column exceeds i64")?,
            line: i64::try_from(value.y).map_err(|_| "cursor row exceeds i64")?,
        })
    }
}

impl TryFrom<MultipaneInfo> for yinz_types::multipane::MultipaneInfo {
    type Error = &'static str;
    fn try_from(info: MultipaneInfo) -> Result<Self, Self::Error> {
        use yinz_types::multipane::{MultipaneDirection, MultipaneId, MultipaneSplit};
        let children = info
            .children
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()?
            .try_into()
            .map_err(|_| "multipane must have exactly two children")?;
        let split = MultipaneSplit {
            direction: if info.top_bottom {
                MultipaneDirection::TopBottom
            } else {
                MultipaneDirection::LeftRight
            },
            ratio: u16::try_from(info.ratio).map_err(|_| "multipane ratio exceeds u16")?,
        };
        split.validate()?;
        Ok(Self {
            id: MultipaneId(info.id),
            children,
            x: usize::try_from(info.x).map_err(|_| "multipane x exceeds usize")?,
            y: usize::try_from(info.y).map_err(|_| "multipane y exceeds usize")?,
            rows: usize::try_from(info.rows).map_err(|_| "multipane rows exceeds usize")?,
            columns: usize::try_from(info.columns)
                .map_err(|_| "multipane columns exceeds usize")?,
            split,
        })
    }
}
impl From<yinz_types::multipane::MultipaneInfo> for MultipaneInfo {
    fn from(info: yinz_types::multipane::MultipaneInfo) -> Self {
        Self {
            id: info.id.0,
            children: info
                .children
                .into_iter()
                .map(|id| ProtobufPaneId {
                    id: id.legacy_parts().0,
                    pane_type: if matches!(id, PaneId::Plugin(_)) {
                        ProtobufPaneType::Plugin as i32
                    } else {
                        ProtobufPaneType::Terminal as i32
                    },
                })
                .collect(),
            x: info.x as u64,
            y: info.y as u64,
            rows: info.rows as u64,
            columns: info.columns as u64,
            top_bottom: info.split.direction
                == yinz_types::multipane::MultipaneDirection::TopBottom,
            ratio: u32::from(info.split.ratio),
        }
    }
}

impl TryFrom<ProtobufCellPixelSize> for yinz_types::pane_size::SizeInPixels {
    type Error = &'static str;

    fn try_from(size: ProtobufCellPixelSize) -> Result<Self, Self::Error> {
        if size.width == 0 || size.height == 0 {
            return Err("Cell pixel dimensions must be nonzero");
        }
        Ok(Self {
            width: size
                .width
                .try_into()
                .map_err(|_| "Cell width is too large")?,
            height: size
                .height
                .try_into()
                .map_err(|_| "Cell height is too large")?,
        })
    }
}

impl TryFrom<yinz_types::pane_size::SizeInPixels> for ProtobufCellPixelSize {
    type Error = &'static str;

    fn try_from(size: yinz_types::pane_size::SizeInPixels) -> Result<Self, Self::Error> {
        if size.width == 0 || size.height == 0 {
            return Err("Cell pixel dimensions must be nonzero");
        }
        Ok(Self {
            width: size
                .width
                .try_into()
                .map_err(|_| "Cell width is too large")?,
            height: size
                .height
                .try_into()
                .map_err(|_| "Cell height is too large")?,
        })
    }
}
