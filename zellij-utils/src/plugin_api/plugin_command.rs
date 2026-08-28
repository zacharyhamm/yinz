#![allow(clippy::unnecessary_fallible_conversions)]
pub use super::generated_api::api::{
    action::{Action as ProtobufAction, PaneIdAndShouldFloat, SwitchToModePayload},
    event::{
        EventNameList as ProtobufEventNameList, Header,
        ResurrectableSession as ProtobufResurrectableSession,
        SessionManifest as ProtobufSessionManifest,
    },
    input_mode::InputMode as ProtobufInputMode,
    pane_frame_style::PaneFrameStyle as ProtobufPaneFrameStyle,
    plugin_command::{
        break_panes_to_new_tab_response, break_panes_to_tab_with_id_response,
        break_panes_to_tab_with_index_response, data_apply_response, data_count_response,
        data_define_response, data_fetch_response, data_unwatch_response, data_watch_response,
        delete_layout_response, dump_layout_response, dump_session_layout_response,
        edit_layout_response, enqueue_queue_job_response, focus_or_create_tab_response,
        fs_list_response, fs_metadata_response, fs_mutate_response, fs_read_response,
        get_focused_pane_info_response, get_pane_cwd_response, get_pane_git_probe_response,
        get_pane_pid_response, get_pane_running_command_response, get_session_list_response,
        hide_floating_panes_response, highlight_style::Style as ProtobufHighlightStyleVariant,
        list_orphaned_panes_response, new_tab_response, new_tab_unfocused_response,
        open_command_pane_in_git_checkout_payload, open_command_pane_in_git_checkout_response,
        parse_layout_response, plugin_command::Payload, rename_layout_response,
        save_layout_response, save_session_response, show_floating_panes_response,
        warp_pane_response, AdoptOrphanedPanePayload, BreakPanesToNewTabPayload,
        BreakPanesToNewTabResponse as ProtobufBreakPanesToNewTabResponse,
        BreakPanesToTabWithIdPayload,
        BreakPanesToTabWithIdResponse as ProtobufBreakPanesToTabWithIdResponse,
        BreakPanesToTabWithIndexPayload,
        BreakPanesToTabWithIndexResponse as ProtobufBreakPanesToTabWithIndexResponse,
        CancelQueueJobPayload, CancelWebRequestPayload, ChangeFloatingPanesCoordinatesPayload,
        ChangeHostFolderPayload, ChangeOverlayCoordinatesPayload, ClearPaneHighlightsPayload,
        ClearScreenForPaneIdPayload, CliPipeOutputPayload, CloseMultiplePanesPayload,
        CloseTabWithIdPayload, CloseTabWithIndexPayload, CommandName, ConfigureQueuePayload,
        ContextItem, CopyToClipboardPayload, CreateTokenResponse as ProtobufCreateTokenResponse,
        CreateTokenResponse, CurrentSessionLastSavedTimePayload,
        CurrentSessionLastSavedTimeResponse as ProtobufCurrentSessionLastSavedTimeResponse,
        CursorPosition, CustomIndexHighlight as ProtobufCustomIndexHighlight,
        CustomRgbHighlight as ProtobufCustomRgbHighlight, DataApplyPayload,
        DataApplyResponse as ProtobufDataApplyResponse,
        DataCountResponse as ProtobufDataCountResponse, DataDefineEntityPayload,
        DataDefineResponse as ProtobufDataDefineResponse, DataFetchPayload,
        DataFetchResponse as ProtobufDataFetchResponse, DataUnwatchPayload,
        DataUnwatchResponse as ProtobufDataUnwatchResponse, DataWatchPayload,
        DataWatchResponse as ProtobufDataWatchResponse,
        DeleteAllDeadSessionsResponse as ProtobufDeleteAllDeadSessionsResponse,
        DeleteDeadSessionResponse as ProtobufDeleteDeadSessionResponse, DeleteLayoutPayload,
        DeleteLayoutResponse as ProtobufDeleteLayoutResponse, DumpLayoutPayload,
        DumpLayoutResponse as ProtobufDumpLayoutResponse, DumpSessionLayoutPayload,
        DumpSessionLayoutResponse as ProtobufDumpSessionLayoutResponse, EditLayoutPayload,
        EditLayoutResponse as ProtobufEditLayoutResponse, EditScrollbackForPaneWithIdPayload,
        EmbedMultiplePanesPayload, EnqueueQueueJobPayload,
        EnqueueQueueJobResponse as ProtobufEnqueueQueueJobResponse, EnvVariable, ExecCmdPayload,
        FinishQueueJobPayload, FixedOrPercent as ProtobufFixedOrPercent,
        FixedOrPercentValue as ProtobufFixedOrPercentValue, FloatMultiplePanesPayload,
        FloatingPaneCoordinates as ProtobufFloatingPaneCoordinates,
        FocusOrCreateTabResponse as ProtobufFocusOrCreateTabResponse, FocusedPaneInfo,
        FsCreateDirPayload, FsEntry as ProtobufFsEntry, FsEntryList as ProtobufFsEntryList,
        FsListPayload, FsListResponse as ProtobufFsListResponse, FsMetadataPayload,
        FsMetadataResponse as ProtobufFsMetadataResponse,
        FsMutateResponse as ProtobufFsMutateResponse, FsReadPayload,
        FsReadResponse as ProtobufFsReadResponse, FsRemovePayload, FsRenamePayload, FsWritePayload,
        GenerateRandomNamePayload,
        GenerateRandomNameResponse as ProtobufGenerateRandomNameResponse,
        GenerateWebLoginTokenPayload, GetFocusedPaneInfoPayload,
        GetFocusedPaneInfoResponse as ProtobufGetFocusedPaneInfoResponse, GetLayoutDirPayload,
        GetLayoutDirResponse as ProtobufGetLayoutDirResponse,
        GetPaneCwdPayload as ProtobufGetPaneCwdPayload,
        GetPaneCwdResponse as ProtobufGetPaneCwdResponse,
        GetPaneGitProbePayload as ProtobufGetPaneGitProbePayload,
        GetPaneGitProbeResponse as ProtobufGetPaneGitProbeResponse, GetPaneInfoPayload,
        GetPaneInfoResponse as ProtobufGetPaneInfoResponse, GetPanePidPayload,
        GetPanePidResponse as ProtobufGetPanePidResponse,
        GetPaneRunningCommandPayload as ProtobufGetPaneRunningCommandPayload,
        GetPaneRunningCommandResponse as ProtobufGetPaneRunningCommandResponse,
        GetPaneScrollbackPayload,
        GetPluginAliasConfigurationPayload as ProtobufGetPluginAliasConfigurationPayload,
        GetPluginAliasConfigurationResponse as ProtobufGetPluginAliasConfigurationResponse,
        GetSessionEnvironmentVariablesPayload as ProtobufGetSessionEnvironmentVariablesPayload,
        GetSessionEnvironmentVariablesResponse as ProtobufGetSessionEnvironmentVariablesResponse,
        GetSessionListPayload as ProtobufGetSessionListPayload,
        GetSessionListResponse as ProtobufGetSessionListResponse, GetTabInfoPayload,
        GetTabInfoResponse as ProtobufGetTabInfoResponse, GetUserPluginDirPayload,
        GetUserPluginDirResponse as ProtobufGetUserPluginDirResponse, GoToTabWithIdPayload,
        GrantPluginPermissionsPayload, GroupAndUngroupPanesPayload,
        HideFloatingPanesPayload as ProtobufHideFloatingPanesPayload,
        HideFloatingPanesResponse as ProtobufHideFloatingPanesResponse, HidePaneWithIdPayload,
        HighlightAndUnhighlightPanesPayload, HighlightLayer as ProtobufHighlightLayer,
        HighlightStyle as ProtobufHighlightStyle, HttpVerb as ProtobufHttpVerb, IdAndNewName,
        KeyToRebind, KeyToUnbind, KillSessionsPayload,
        KillSessionsResponse as ProtobufKillSessionsResponse,
        ListNotificationsResponse as ProtobufListNotificationsResponse,
        ListOrphanedPanesResponse as ProtobufListOrphanedPanesResponse,
        ListRecordingsResponse as ProtobufListRecordingsResponse, ListTokensResponse,
        LoadNewPluginPayload, MessageToPluginPayload, MovePaneWithPaneIdInDirectionPayload,
        MovePaneWithPaneIdPayload, MovePayload, NewBranchGitCheckout,
        NewPluginArgs as ProtobufNewPluginArgs, NewTabPayload,
        NewTabResponse as ProtobufNewTabResponse, NewTabUnfocusedPayload,
        NewTabUnfocusedResponse as ProtobufNewTabUnfocusedResponse,
        NewTabsResponse as ProtobufNewTabsResponse, NewTabsWithLayoutInfoPayload,
        NewTiledPaneInTabPayload, NewTiledPaneInTabResponse as ProtobufNewTiledPaneInTabResponse,
        NewWorktreeGitCheckout, NotificationInfo as ProtobufNotificationInfo,
        NotifyWithPanePayload,
        OpenCommandPaneBackgroundResponse as ProtobufOpenCommandPaneBackgroundResponse,
        OpenCommandPaneFloatingNearPluginPayload,
        OpenCommandPaneFloatingNearPluginResponse as ProtobufOpenCommandPaneFloatingNearPluginResponse,
        OpenCommandPaneFloatingResponse as ProtobufOpenCommandPaneFloatingResponse,
        OpenCommandPaneInGitCheckoutPayload,
        OpenCommandPaneInGitCheckoutResponse as ProtobufOpenCommandPaneInGitCheckoutResponse,
        OpenCommandPaneInPlaceOfPaneIdPayload,
        OpenCommandPaneInPlaceOfPaneIdResponse as ProtobufOpenCommandPaneInPlaceOfPaneIdResponse,
        OpenCommandPaneInPlaceOfPluginPayload,
        OpenCommandPaneInPlaceOfPluginResponse as ProtobufOpenCommandPaneInPlaceOfPluginResponse,
        OpenCommandPaneInPlaceResponse as ProtobufOpenCommandPaneInPlaceResponse,
        OpenCommandPaneNearPluginPayload,
        OpenCommandPaneNearPluginResponse as ProtobufOpenCommandPaneNearPluginResponse,
        OpenCommandPaneOverlayPayload,
        OpenCommandPaneOverlayResponse as ProtobufOpenCommandPaneOverlayResponse,
        OpenCommandPanePayload, OpenCommandPaneResponse as ProtobufOpenCommandPaneResponse,
        OpenEditPaneInPlaceOfPaneIdPayload,
        OpenEditPaneInPlaceOfPaneIdResponse as ProtobufOpenEditPaneInPlaceOfPaneIdResponse,
        OpenFileFloatingNearPluginPayload,
        OpenFileFloatingNearPluginResponse as ProtobufOpenFileFloatingNearPluginResponse,
        OpenFileFloatingResponse as ProtobufOpenFileFloatingResponse,
        OpenFileInPlaceOfPluginPayload,
        OpenFileInPlaceOfPluginResponse as ProtobufOpenFileInPlaceOfPluginResponse,
        OpenFileInPlaceResponse as ProtobufOpenFileInPlaceResponse, OpenFileNearPluginPayload,
        OpenFileNearPluginResponse as ProtobufOpenFileNearPluginResponse, OpenFilePayload,
        OpenFileResponse as ProtobufOpenFileResponse,
        OpenPaneInNewTabResponse as ProtobufOpenPaneInNewTabResponse,
        OpenPluginPaneFloatingPayload,
        OpenPluginPaneFloatingResponse as ProtobufOpenPluginPaneFloatingResponse,
        OpenPluginPaneInNewTabPayload as ProtobufOpenPluginPaneInNewTabPayload,
        OpenTerminalFloatingNearPluginPayload,
        OpenTerminalFloatingNearPluginResponse as ProtobufOpenTerminalFloatingNearPluginResponse,
        OpenTerminalFloatingResponse as ProtobufOpenTerminalFloatingResponse,
        OpenTerminalInPlaceOfPluginPayload,
        OpenTerminalInPlaceOfPluginResponse as ProtobufOpenTerminalInPlaceOfPluginResponse,
        OpenTerminalInPlaceResponse as ProtobufOpenTerminalInPlaceResponse,
        OpenTerminalNearPluginPayload,
        OpenTerminalNearPluginResponse as ProtobufOpenTerminalNearPluginResponse,
        OpenTerminalPaneInPlaceOfPaneIdPayload,
        OpenTerminalPaneInPlaceOfPaneIdResponse as ProtobufOpenTerminalPaneInPlaceOfPaneIdResponse,
        OpenTerminalResponse as ProtobufOpenTerminalResponse, OrphanedPane as ProtobufOrphanedPane,
        OrphanedPaneList as ProtobufOrphanedPaneList, OverrideLayoutPayload,
        PageScrollDownInPaneIdPayload, PageScrollUpInPaneIdPayload,
        PaneGitProbe as ProtobufPaneGitProbe, PaneId as ProtobufPaneId,
        PaneIdAndFloatingPaneCoordinates, PaneType as ProtobufPaneType, ParseLayoutPayload,
        ParseLayoutResponse as ProtobufParseLayoutResponse, PluginCommand as ProtobufPluginCommand,
        PluginMessagePayload, QueueStatusPayload,
        QueueStatusResponse as ProtobufQueueStatusResponse, RebindKeysPayload, ReconfigurePayload,
        RecordingSummary as ProtobufRecordingSummary, RegexHighlight as ProtobufRegexHighlight,
        ReloadPluginPayload, RenameLayoutPayload,
        RenameLayoutResponse as ProtobufRenameLayoutResponse, RenameTabWithIdPayload,
        RenameWebLoginTokenPayload, RenameWebTokenResponse, ReplacePaneWithExistingPanePayload,
        RequestPluginPermissionPayload, RerunCommandPanePayload, ResizePaneIdWithDirectionPayload,
        ResizePayload, RevealPaneWithIdPayload, RevokeAllWebTokensResponse, RevokeTokenResponse,
        RevokeWebLoginTokenPayload, RunActionPayload, RunCommandPayload,
        RunningCommand as ProtobufRunningCommand, SaveLayoutPayload,
        SaveLayoutResponse as ProtobufSaveLayoutResponse, SaveSessionPayload,
        SaveSessionResponse as ProtobufSaveSessionResponse, ScrollDownInPaneIdPayload,
        ScrollToBottomInPaneIdPayload, ScrollToTopInPaneIdPayload, ScrollUpInPaneIdPayload,
        SearchHit as ProtobufSearchHit, SearchRecordingsPayload as ProtobufSearchRecordingsPayload,
        SearchRecordingsResponse as ProtobufSearchRecordingsResponse,
        SessionListSnapshot as ProtobufSessionListSnapshot, SetFloatingPanePinnedPayload,
        SetPaneBorderlessPayload, SetPaneColorPayload,
        SetPaneFrameStylePayload as ProtobufSetPaneFrameStylePayload,
        SetPaneRegexHighlightsPayload, SetPaneTitlePrefixPayload,
        SetSelfMouseSelectionSupportPayload,
        SetSoftKeyboardPayload as ProtobufSetSoftKeyboardPayload, SetTabProjectsPayload,
        SetTimeoutPayload, ShowCursorPayload,
        ShowFloatingPanesPayload as ProtobufShowFloatingPanesPayload,
        ShowFloatingPanesResponse as ProtobufShowFloatingPanesResponse, ShowPaneWithIdPayload,
        StackPanesPayload, SubscribePayload, SwitchSessionPayload, SwitchTabToIdPayload,
        SwitchTabToPayload, TabProject, ToggleFloatingPanesPayload, TogglePaneBorderlessPayload,
        TogglePaneEmbedOrEjectForPaneIdPayload, TogglePaneIdFullscreenPayload, UnsubscribePayload,
        WarpPaneFromSessionPayload, WarpPaneResponse as ProtobufWarpPaneResponse,
        WarpPaneToSessionPayload, WarpPlacement as ProtobufWarpPlacement,
        WarpTabFromSessionPayload, WarpTabToSessionPayload, WebRequestPayload,
        WriteCharsToPaneIdPayload, WriteToPaneIdPayload,
    },
    plugin_permission::PermissionType as ProtobufPermissionType,
    resize::ResizeAction as ProtobufResizeAction,
};

use crate::plugin_api::plugin_data::{watch_mode_from_i32, ProtobufMutation, ProtobufWatchMode};

use crate::data::{
    ConnectToSession, DataApplyResponse, DataCountResponse, DataDefineResponse, DataFetchResponse,
    DataUnwatchResponse, DataWatchResponse, DeleteAllDeadSessionsResponse,
    DeleteDeadSessionResponse, DeleteLayoutResponse, EditLayoutResponse, EnqueueQueueJobResponse,
    FloatingPaneCoordinates, GetFocusedPaneInfoResponse, GetPaneCwdResponse,
    GetPaneGitProbeResponse, GetPanePidResponse, GetPaneRunningCommandResponse,
    GetSessionListResponse, GitCheckout, HighlightLayer, HighlightStyle, HttpVerb, InputMode,
    KeyWithModifier, KillSessionsResponse, ListOrphanedPanesResponse, MessageToPlugin, Mutation,
    NewPluginArgs, NotificationInfo, OpenCommandPaneInGitCheckoutResponse,
    OpenCommandPaneOverlayResponse, OrphanedPane, OverlayPlacement, PaneGitProbe, PaneId,
    PermissionType, PluginCommand, QueueSnapshot, QueueStatusResponse, RecordingSummary,
    RegexHighlight, RenameLayoutResponse, SaveLayoutResponse, SearchHit, SessionInfo,
    SessionListSnapshot, WarpPaneResponse, WarpPlacement,
};
use crate::input::actions::Action;
use crate::input::layout::PercentOrFixed;

use std::collections::BTreeMap;
use std::convert::TryFrom;
use std::path::PathBuf;

#[allow(clippy::from_over_into)]
impl Into<FloatingPaneCoordinates> for ProtobufFloatingPaneCoordinates {
    fn into(self) -> FloatingPaneCoordinates {
        FloatingPaneCoordinates {
            x: self
                .x
                .and_then(|x| match ProtobufFixedOrPercent::try_from(x.r#type).ok() {
                    Some(ProtobufFixedOrPercent::Percent) => {
                        Some(PercentOrFixed::Percent(x.value as usize))
                    },
                    Some(ProtobufFixedOrPercent::Fixed) => {
                        Some(PercentOrFixed::Fixed(x.value as usize))
                    },
                    None => None,
                }),
            y: self
                .y
                .and_then(|y| match ProtobufFixedOrPercent::try_from(y.r#type).ok() {
                    Some(ProtobufFixedOrPercent::Percent) => {
                        Some(PercentOrFixed::Percent(y.value as usize))
                    },
                    Some(ProtobufFixedOrPercent::Fixed) => {
                        Some(PercentOrFixed::Fixed(y.value as usize))
                    },
                    None => None,
                }),
            width: self.width.and_then(|width| {
                match ProtobufFixedOrPercent::try_from(width.r#type).ok() {
                    Some(ProtobufFixedOrPercent::Percent) => {
                        Some(PercentOrFixed::Percent(width.value as usize))
                    },
                    Some(ProtobufFixedOrPercent::Fixed) => {
                        Some(PercentOrFixed::Fixed(width.value as usize))
                    },
                    None => None,
                }
            }),
            height: self.height.and_then(|height| {
                match ProtobufFixedOrPercent::try_from(height.r#type).ok() {
                    Some(ProtobufFixedOrPercent::Percent) => {
                        Some(PercentOrFixed::Percent(height.value as usize))
                    },
                    Some(ProtobufFixedOrPercent::Fixed) => {
                        Some(PercentOrFixed::Fixed(height.value as usize))
                    },
                    None => None,
                }
            }),
            pinned: self.pinned,
            borderless: self.borderless,
        }
    }
}

#[allow(clippy::from_over_into)]
impl Into<ProtobufFloatingPaneCoordinates> for FloatingPaneCoordinates {
    fn into(self) -> ProtobufFloatingPaneCoordinates {
        ProtobufFloatingPaneCoordinates {
            x: match self.x {
                Some(PercentOrFixed::Percent(percent)) => Some(ProtobufFixedOrPercentValue {
                    r#type: ProtobufFixedOrPercent::Percent as i32,
                    value: percent as u32,
                }),
                Some(PercentOrFixed::Fixed(fixed)) => Some(ProtobufFixedOrPercentValue {
                    r#type: ProtobufFixedOrPercent::Fixed as i32,
                    value: fixed as u32,
                }),
                None => None,
            },
            y: match self.y {
                Some(PercentOrFixed::Percent(percent)) => Some(ProtobufFixedOrPercentValue {
                    r#type: ProtobufFixedOrPercent::Percent as i32,
                    value: percent as u32,
                }),
                Some(PercentOrFixed::Fixed(fixed)) => Some(ProtobufFixedOrPercentValue {
                    r#type: ProtobufFixedOrPercent::Fixed as i32,
                    value: fixed as u32,
                }),
                None => None,
            },
            width: match self.width {
                Some(PercentOrFixed::Percent(percent)) => Some(ProtobufFixedOrPercentValue {
                    r#type: ProtobufFixedOrPercent::Percent as i32,
                    value: percent as u32,
                }),
                Some(PercentOrFixed::Fixed(fixed)) => Some(ProtobufFixedOrPercentValue {
                    r#type: ProtobufFixedOrPercent::Fixed as i32,
                    value: fixed as u32,
                }),
                None => None,
            },
            height: match self.height {
                Some(PercentOrFixed::Percent(percent)) => Some(ProtobufFixedOrPercentValue {
                    r#type: ProtobufFixedOrPercent::Percent as i32,
                    value: percent as u32,
                }),
                Some(PercentOrFixed::Fixed(fixed)) => Some(ProtobufFixedOrPercentValue {
                    r#type: ProtobufFixedOrPercent::Fixed as i32,
                    value: fixed as u32,
                }),
                None => None,
            },
            pinned: self.pinned,
            borderless: self.borderless,
        }
    }
}

#[allow(clippy::from_over_into)]
impl Into<HttpVerb> for ProtobufHttpVerb {
    fn into(self) -> HttpVerb {
        match self {
            ProtobufHttpVerb::Get => HttpVerb::Get,
            ProtobufHttpVerb::Post => HttpVerb::Post,
            ProtobufHttpVerb::Put => HttpVerb::Put,
            ProtobufHttpVerb::Delete => HttpVerb::Delete,
        }
    }
}

#[allow(clippy::from_over_into)]
impl Into<ProtobufHttpVerb> for HttpVerb {
    fn into(self) -> ProtobufHttpVerb {
        match self {
            HttpVerb::Get => ProtobufHttpVerb::Get,
            HttpVerb::Post => ProtobufHttpVerb::Post,
            HttpVerb::Put => ProtobufHttpVerb::Put,
            HttpVerb::Delete => ProtobufHttpVerb::Delete,
        }
    }
}

impl TryFrom<ProtobufPaneId> for PaneId {
    type Error = &'static str;
    fn try_from(protobuf_pane_id: ProtobufPaneId) -> Result<Self, &'static str> {
        match ProtobufPaneType::try_from(protobuf_pane_id.pane_type).ok() {
            Some(ProtobufPaneType::Terminal) => Ok(PaneId::Terminal(protobuf_pane_id.id)),
            Some(ProtobufPaneType::Plugin) => Ok(PaneId::Plugin(protobuf_pane_id.id)),
            None => Err("Failed to convert PaneId"),
        }
    }
}

impl TryFrom<PaneId> for ProtobufPaneId {
    type Error = &'static str;
    fn try_from(pane_id: PaneId) -> Result<Self, &'static str> {
        match pane_id {
            PaneId::Terminal(id) => Ok(ProtobufPaneId {
                pane_type: ProtobufPaneType::Terminal as i32,
                id,
            }),
            PaneId::Plugin(id) => Ok(ProtobufPaneId {
                pane_type: ProtobufPaneType::Plugin as i32,
                id,
            }),
        }
    }
}

impl TryFrom<ProtobufGetPanePidResponse> for GetPanePidResponse {
    type Error = &'static str;
    fn try_from(protobuf_response: ProtobufGetPanePidResponse) -> Result<Self, &'static str> {
        match protobuf_response.result {
            Some(get_pane_pid_response::Result::Pid(pid)) => Ok(GetPanePidResponse::Ok(pid)),
            Some(get_pane_pid_response::Result::Error(error)) => Ok(GetPanePidResponse::Err(error)),
            None => Err("Empty GetPanePidResponse"),
        }
    }
}

impl From<GetPanePidResponse> for ProtobufGetPanePidResponse {
    fn from(response: GetPanePidResponse) -> Self {
        match response {
            GetPanePidResponse::Ok(pid) => ProtobufGetPanePidResponse {
                result: Some(get_pane_pid_response::Result::Pid(pid)),
            },
            GetPanePidResponse::Err(error) => ProtobufGetPanePidResponse {
                result: Some(get_pane_pid_response::Result::Error(error)),
            },
        }
    }
}

impl From<GetPaneRunningCommandResponse> for ProtobufGetPaneRunningCommandResponse {
    fn from(response: GetPaneRunningCommandResponse) -> Self {
        match response {
            GetPaneRunningCommandResponse::Ok(args) => ProtobufGetPaneRunningCommandResponse {
                result: Some(get_pane_running_command_response::Result::Command(
                    ProtobufRunningCommand { args },
                )),
            },
            GetPaneRunningCommandResponse::Err(err) => ProtobufGetPaneRunningCommandResponse {
                result: Some(get_pane_running_command_response::Result::Error(err)),
            },
        }
    }
}

impl From<GetSessionListResponse> for ProtobufGetSessionListResponse {
    fn from(response: GetSessionListResponse) -> Self {
        match response {
            GetSessionListResponse::Ok(snapshot) => {
                let live_sessions = snapshot
                    .live_sessions
                    .into_iter()
                    .filter_map(|si| ProtobufSessionManifest::try_from(si).ok())
                    .collect();
                let resurrectable_sessions = snapshot
                    .resurrectable_sessions
                    .into_iter()
                    .map(ProtobufResurrectableSession::from)
                    .collect();
                ProtobufGetSessionListResponse {
                    result: Some(get_session_list_response::Result::Snapshot(
                        ProtobufSessionListSnapshot {
                            live_sessions,
                            resurrectable_sessions,
                        },
                    )),
                }
            },
            GetSessionListResponse::Err(err) => ProtobufGetSessionListResponse {
                result: Some(get_session_list_response::Result::Error(err)),
            },
        }
    }
}

impl TryFrom<ProtobufGetSessionListResponse> for GetSessionListResponse {
    type Error = &'static str;
    fn try_from(protobuf_response: ProtobufGetSessionListResponse) -> Result<Self, &'static str> {
        match protobuf_response.result {
            Some(get_session_list_response::Result::Snapshot(snapshot)) => {
                let mut live_sessions: Vec<SessionInfo> = Vec::new();
                for manifest in snapshot.live_sessions {
                    live_sessions.push(SessionInfo::try_from(manifest)?);
                }
                let resurrectable_sessions = snapshot
                    .resurrectable_sessions
                    .into_iter()
                    .map(<(String, std::time::Duration)>::from)
                    .collect();
                Ok(GetSessionListResponse::Ok(SessionListSnapshot {
                    live_sessions,
                    resurrectable_sessions,
                }))
            },
            Some(get_session_list_response::Result::Error(err)) => {
                Ok(GetSessionListResponse::Err(err))
            },
            None => Err("Empty GetSessionListResponse"),
        }
    }
}

impl From<KillSessionsResponse> for ProtobufKillSessionsResponse {
    fn from(response: KillSessionsResponse) -> Self {
        match response {
            KillSessionsResponse::Ok => ProtobufKillSessionsResponse { error: None },
            KillSessionsResponse::Err(err) => ProtobufKillSessionsResponse { error: Some(err) },
        }
    }
}

impl From<ProtobufKillSessionsResponse> for KillSessionsResponse {
    fn from(protobuf_response: ProtobufKillSessionsResponse) -> Self {
        match protobuf_response.error {
            Some(err) => KillSessionsResponse::Err(err),
            None => KillSessionsResponse::Ok,
        }
    }
}

impl From<DeleteDeadSessionResponse> for ProtobufDeleteDeadSessionResponse {
    fn from(response: DeleteDeadSessionResponse) -> Self {
        match response {
            DeleteDeadSessionResponse::Ok => ProtobufDeleteDeadSessionResponse { error: None },
            DeleteDeadSessionResponse::Err(err) => {
                ProtobufDeleteDeadSessionResponse { error: Some(err) }
            },
        }
    }
}

impl From<ProtobufDeleteDeadSessionResponse> for DeleteDeadSessionResponse {
    fn from(protobuf_response: ProtobufDeleteDeadSessionResponse) -> Self {
        match protobuf_response.error {
            Some(err) => DeleteDeadSessionResponse::Err(err),
            None => DeleteDeadSessionResponse::Ok,
        }
    }
}

impl From<DeleteAllDeadSessionsResponse> for ProtobufDeleteAllDeadSessionsResponse {
    fn from(response: DeleteAllDeadSessionsResponse) -> Self {
        match response {
            DeleteAllDeadSessionsResponse::Ok => {
                ProtobufDeleteAllDeadSessionsResponse { error: None }
            },
            DeleteAllDeadSessionsResponse::Err(err) => {
                ProtobufDeleteAllDeadSessionsResponse { error: Some(err) }
            },
        }
    }
}

impl From<ProtobufDeleteAllDeadSessionsResponse> for DeleteAllDeadSessionsResponse {
    fn from(protobuf_response: ProtobufDeleteAllDeadSessionsResponse) -> Self {
        match protobuf_response.error {
            Some(err) => DeleteAllDeadSessionsResponse::Err(err),
            None => DeleteAllDeadSessionsResponse::Ok,
        }
    }
}

impl From<GetPaneCwdResponse> for ProtobufGetPaneCwdResponse {
    fn from(response: GetPaneCwdResponse) -> Self {
        match response {
            GetPaneCwdResponse::Ok(path) => {
                let cwd_string = path
                    .to_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| String::from(""));
                ProtobufGetPaneCwdResponse {
                    result: Some(get_pane_cwd_response::Result::Cwd(cwd_string)),
                }
            },
            GetPaneCwdResponse::Err(err) => ProtobufGetPaneCwdResponse {
                result: Some(get_pane_cwd_response::Result::Error(err)),
            },
        }
    }
}

impl From<PaneGitProbe> for ProtobufPaneGitProbe {
    fn from(probe: PaneGitProbe) -> Self {
        ProtobufPaneGitProbe {
            host: probe.host,
            cwd: probe.cwd,
            exit_code: probe.exit_code,
            metadata: probe.metadata,
            config: probe.config,
        }
    }
}

impl From<ProtobufPaneGitProbe> for PaneGitProbe {
    fn from(probe: ProtobufPaneGitProbe) -> Self {
        PaneGitProbe {
            host: probe.host,
            cwd: probe.cwd,
            exit_code: probe.exit_code,
            metadata: probe.metadata,
            config: probe.config,
        }
    }
}

impl From<GetPaneGitProbeResponse> for ProtobufGetPaneGitProbeResponse {
    fn from(response: GetPaneGitProbeResponse) -> Self {
        match response {
            GetPaneGitProbeResponse::Ok(probe) => ProtobufGetPaneGitProbeResponse {
                result: Some(get_pane_git_probe_response::Result::Probe(probe.into())),
            },
            GetPaneGitProbeResponse::Err(err) => ProtobufGetPaneGitProbeResponse {
                result: Some(get_pane_git_probe_response::Result::Error(err)),
            },
        }
    }
}

impl From<GetFocusedPaneInfoResponse> for ProtobufGetFocusedPaneInfoResponse {
    fn from(response: GetFocusedPaneInfoResponse) -> Self {
        match response {
            GetFocusedPaneInfoResponse::Ok { tab_index, pane_id } => {
                let protobuf_pane_id = ProtobufPaneId {
                    pane_type: match pane_id {
                        PaneId::Terminal(_) => ProtobufPaneType::Terminal as i32,
                        PaneId::Plugin(_) => ProtobufPaneType::Plugin as i32,
                    },
                    id: match pane_id {
                        PaneId::Terminal(id) | PaneId::Plugin(id) => id,
                    },
                };
                ProtobufGetFocusedPaneInfoResponse {
                    result: Some(get_focused_pane_info_response::Result::FocusedPaneInfo(
                        FocusedPaneInfo {
                            focused_tab_index: tab_index as u32,
                            focused_pane_id: Some(protobuf_pane_id),
                        },
                    )),
                }
            },
            GetFocusedPaneInfoResponse::Err(err) => ProtobufGetFocusedPaneInfoResponse {
                result: Some(get_focused_pane_info_response::Result::Error(err)),
            },
        }
    }
}

impl TryFrom<ProtobufSaveLayoutResponse> for SaveLayoutResponse {
    type Error = &'static str;
    fn try_from(protobuf_response: ProtobufSaveLayoutResponse) -> Result<Self, &'static str> {
        match protobuf_response.result {
            Some(save_layout_response::Result::Success(_)) => Ok(SaveLayoutResponse::Ok(())),
            Some(save_layout_response::Result::Error(error)) => Ok(SaveLayoutResponse::Err(error)),
            None => Err("Empty SaveLayoutResponse"),
        }
    }
}

impl From<SaveLayoutResponse> for ProtobufSaveLayoutResponse {
    fn from(response: SaveLayoutResponse) -> Self {
        match response {
            SaveLayoutResponse::Ok(_) => ProtobufSaveLayoutResponse {
                result: Some(save_layout_response::Result::Success(true)),
            },
            SaveLayoutResponse::Err(error) => ProtobufSaveLayoutResponse {
                result: Some(save_layout_response::Result::Error(error)),
            },
        }
    }
}

impl TryFrom<ProtobufDeleteLayoutResponse> for DeleteLayoutResponse {
    type Error = &'static str;
    fn try_from(protobuf_response: ProtobufDeleteLayoutResponse) -> Result<Self, &'static str> {
        match protobuf_response.result {
            Some(delete_layout_response::Result::Success(_)) => Ok(DeleteLayoutResponse::Ok(())),
            Some(delete_layout_response::Result::Error(error)) => {
                Ok(DeleteLayoutResponse::Err(error))
            },
            None => Err("Empty DeleteLayoutResponse"),
        }
    }
}

impl From<DeleteLayoutResponse> for ProtobufDeleteLayoutResponse {
    fn from(response: DeleteLayoutResponse) -> Self {
        match response {
            DeleteLayoutResponse::Ok(_) => ProtobufDeleteLayoutResponse {
                result: Some(delete_layout_response::Result::Success(true)),
            },
            DeleteLayoutResponse::Err(error) => ProtobufDeleteLayoutResponse {
                result: Some(delete_layout_response::Result::Error(error)),
            },
        }
    }
}

impl TryFrom<ProtobufRenameLayoutResponse> for RenameLayoutResponse {
    type Error = &'static str;
    fn try_from(protobuf_response: ProtobufRenameLayoutResponse) -> Result<Self, &'static str> {
        match protobuf_response.result {
            Some(rename_layout_response::Result::Success(_)) => Ok(RenameLayoutResponse::Ok(())),
            Some(rename_layout_response::Result::Error(error)) => {
                Ok(RenameLayoutResponse::Err(error))
            },
            None => Err("Empty RenameLayoutResponse"),
        }
    }
}

impl From<RenameLayoutResponse> for ProtobufRenameLayoutResponse {
    fn from(response: RenameLayoutResponse) -> Self {
        match response {
            RenameLayoutResponse::Ok(_) => ProtobufRenameLayoutResponse {
                result: Some(rename_layout_response::Result::Success(true)),
            },
            RenameLayoutResponse::Err(error) => ProtobufRenameLayoutResponse {
                result: Some(rename_layout_response::Result::Error(error)),
            },
        }
    }
}

impl TryFrom<ProtobufEditLayoutResponse> for EditLayoutResponse {
    type Error = &'static str;
    fn try_from(protobuf_response: ProtobufEditLayoutResponse) -> Result<Self, &'static str> {
        match protobuf_response.result {
            Some(edit_layout_response::Result::Success(_)) => Ok(EditLayoutResponse::Ok(())),
            Some(edit_layout_response::Result::Error(error)) => Ok(EditLayoutResponse::Err(error)),
            None => Err("Empty EditLayoutResponse"),
        }
    }
}

impl From<EditLayoutResponse> for ProtobufEditLayoutResponse {
    fn from(response: EditLayoutResponse) -> Self {
        match response {
            EditLayoutResponse::Ok(_) => ProtobufEditLayoutResponse {
                result: Some(edit_layout_response::Result::Success(true)),
            },
            EditLayoutResponse::Err(error) => ProtobufEditLayoutResponse {
                result: Some(edit_layout_response::Result::Error(error)),
            },
        }
    }
}

impl TryFrom<(InputMode, KeyWithModifier, Vec<Action>)> for KeyToRebind {
    type Error = &'static str;
    fn try_from(
        key_to_rebind: (InputMode, KeyWithModifier, Vec<Action>),
    ) -> Result<Self, &'static str> {
        Ok(KeyToRebind {
            input_mode: key_to_rebind.0 as i32,
            key: Some(key_to_rebind.1.try_into()?),
            actions: key_to_rebind
                .2
                .into_iter()
                .filter_map(|a| a.try_into().ok())
                .collect(),
        })
    }
}

impl TryFrom<(InputMode, KeyWithModifier)> for KeyToUnbind {
    type Error = &'static str;
    fn try_from(key_to_unbind: (InputMode, KeyWithModifier)) -> Result<Self, &'static str> {
        Ok(KeyToUnbind {
            input_mode: key_to_unbind.0 as i32,
            key: Some(key_to_unbind.1.try_into()?),
        })
    }
}

fn key_to_rebind_to_plugin_command_assets(
    key_to_rebind: KeyToRebind,
) -> Option<(InputMode, KeyWithModifier, Vec<Action>)> {
    Some((
        ProtobufInputMode::try_from(key_to_rebind.input_mode)
            .ok()?
            .try_into()
            .ok()?,
        key_to_rebind.key?.try_into().ok()?,
        key_to_rebind
            .actions
            .into_iter()
            .filter_map(|a| a.try_into().ok())
            .collect(),
    ))
}

fn key_to_unbind_to_plugin_command_assets(
    key_to_unbind: KeyToUnbind,
) -> Option<(InputMode, KeyWithModifier)> {
    Some((
        ProtobufInputMode::try_from(key_to_unbind.input_mode)
            .ok()?
            .try_into()
            .ok()?,
        key_to_unbind.key?.try_into().ok()?,
    ))
}

impl TryFrom<ProtobufPluginCommand> for PluginCommand {
    type Error = &'static str;
    fn try_from(protobuf_plugin_command: ProtobufPluginCommand) -> Result<Self, &'static str> {
        match CommandName::try_from(protobuf_plugin_command.name).ok() {
            Some(CommandName::Subscribe) => match protobuf_plugin_command.payload {
                Some(Payload::SubscribePayload(subscribe_payload)) => {
                    let protobuf_event_list = subscribe_payload.subscriptions;
                    match protobuf_event_list {
                        Some(protobuf_event_list) => {
                            Ok(PluginCommand::Subscribe(protobuf_event_list.try_into()?))
                        },
                        None => Err("malformed subscription event"),
                    }
                },
                _ => Err("Mismatched payload for Subscribe"),
            },
            Some(CommandName::Unsubscribe) => match protobuf_plugin_command.payload {
                Some(Payload::UnsubscribePayload(unsubscribe_payload)) => {
                    let protobuf_event_list = unsubscribe_payload.subscriptions;
                    match protobuf_event_list {
                        Some(protobuf_event_list) => {
                            Ok(PluginCommand::Unsubscribe(protobuf_event_list.try_into()?))
                        },
                        None => Err("malformed unsubscription event"),
                    }
                },
                _ => Err("Mismatched payload for Unsubscribe"),
            },
            Some(CommandName::SetSelectable) => match protobuf_plugin_command.payload {
                Some(Payload::SetSelectablePayload(should_be_selectable)) => {
                    Ok(PluginCommand::SetSelectable(should_be_selectable))
                },
                _ => Err("Mismatched payload for SetSelectable"),
            },
            Some(CommandName::ShowCursor) => match protobuf_plugin_command.payload {
                Some(Payload::ShowCursorPayload(payload)) => {
                    let cursor_position =
                        payload.position.map(|pos| (pos.x as usize, pos.y as usize));
                    Ok(PluginCommand::ShowCursor(cursor_position))
                },
                _ => Err("Mismatched payload for ShowCursor"),
            },
            Some(CommandName::GetPluginIds) => {
                if protobuf_plugin_command.payload.is_some() {
                    Err("GetPluginIds should not have a payload")
                } else {
                    Ok(PluginCommand::GetPluginIds)
                }
            },
            Some(CommandName::GetZellijVersion) => {
                if protobuf_plugin_command.payload.is_some() {
                    Err("GetZellijVersion should not have a payload")
                } else {
                    Ok(PluginCommand::GetZellijVersion)
                }
            },
            Some(CommandName::OpenFile) => match protobuf_plugin_command.payload {
                Some(Payload::OpenFilePayload(file_to_open_payload)) => {
                    match file_to_open_payload.file_to_open {
                        Some(file_to_open) => {
                            let context: BTreeMap<String, String> = file_to_open_payload
                                .context
                                .into_iter()
                                .map(|e| (e.name, e.value))
                                .collect();
                            Ok(PluginCommand::OpenFile(file_to_open.try_into()?, context))
                        },
                        None => Err("Malformed open file payload"),
                    }
                },
                _ => Err("Mismatched payload for OpenFile"),
            },
            Some(CommandName::OpenFileFloating) => match protobuf_plugin_command.payload {
                Some(Payload::OpenFileFloatingPayload(file_to_open_payload)) => {
                    let floating_pane_coordinates = file_to_open_payload
                        .floating_pane_coordinates
                        .map(|f| f.into());
                    let context: BTreeMap<String, String> = file_to_open_payload
                        .context
                        .into_iter()
                        .map(|e| (e.name, e.value))
                        .collect();
                    match file_to_open_payload.file_to_open {
                        Some(file_to_open) => Ok(PluginCommand::OpenFileFloating(
                            file_to_open.try_into()?,
                            floating_pane_coordinates,
                            context,
                        )),
                        None => Err("Malformed open file payload"),
                    }
                },
                _ => Err("Mismatched payload for OpenFileFloating"),
            },
            Some(CommandName::OpenTerminal) => match protobuf_plugin_command.payload {
                Some(Payload::OpenTerminalPayload(file_to_open_payload)) => {
                    match file_to_open_payload.file_to_open {
                        Some(file_to_open) => {
                            Ok(PluginCommand::OpenTerminal(file_to_open.try_into()?))
                        },
                        None => Err("Malformed open terminal payload"),
                    }
                },
                _ => Err("Mismatched payload for OpenTerminal"),
            },
            Some(CommandName::OpenTerminalFloating) => match protobuf_plugin_command.payload {
                Some(Payload::OpenTerminalFloatingPayload(file_to_open_payload)) => {
                    let floating_pane_coordinates = file_to_open_payload
                        .floating_pane_coordinates
                        .map(|f| f.into());
                    match file_to_open_payload.file_to_open {
                        Some(file_to_open) => Ok(PluginCommand::OpenTerminalFloating(
                            file_to_open.try_into()?,
                            floating_pane_coordinates,
                        )),
                        None => Err("Malformed open terminal floating payload"),
                    }
                },
                _ => Err("Mismatched payload for OpenTerminalFloating"),
            },
            Some(CommandName::OpenCommandPane) => match protobuf_plugin_command.payload {
                Some(Payload::OpenCommandPanePayload(command_to_run_payload)) => {
                    match command_to_run_payload.command_to_run {
                        Some(command_to_run) => {
                            let context: BTreeMap<String, String> = command_to_run_payload
                                .context
                                .into_iter()
                                .map(|e| (e.name, e.value))
                                .collect();
                            Ok(PluginCommand::OpenCommandPane(
                                command_to_run.try_into()?,
                                context,
                            ))
                        },
                        None => Err("Malformed open open command pane payload"),
                    }
                },
                _ => Err("Mismatched payload for OpenCommandPane"),
            },
            Some(CommandName::OpenCommandPaneInGitCheckoutNearPlugin) => {
                match protobuf_plugin_command.payload {
                    Some(Payload::OpenCommandPaneInGitCheckoutPayload(payload)) => {
                        let command = payload
                            .command_to_run
                            .ok_or("Malformed git checkout command pane payload")?
                            .try_into()?;
                        let checkout = match payload.checkout {
                            Some(open_command_pane_in_git_checkout_payload::Checkout::Current(
                                _,
                            )) => GitCheckout::Current,
                            Some(
                                open_command_pane_in_git_checkout_payload::Checkout::NewBranch(
                                    setup,
                                ),
                            ) => GitCheckout::NewBranch {
                                branch: setup.branch,
                                base: setup.base,
                            },
                            Some(
                                open_command_pane_in_git_checkout_payload::Checkout::NewWorktree(
                                    setup,
                                ),
                            ) => GitCheckout::NewWorktree {
                                branch: setup.branch,
                                base: setup.base,
                                path: setup.path.into(),
                            },
                            None => return Err("Missing git checkout mode"),
                        };
                        Ok(PluginCommand::OpenCommandPaneInGitCheckoutNearPlugin(
                            command,
                            payload.source_directory.into(),
                            checkout,
                        ))
                    },
                    _ => Err("Mismatched payload for OpenCommandPaneInGitCheckoutNearPlugin"),
                }
            },
            Some(CommandName::OpenCommandPaneFloating) => match protobuf_plugin_command.payload {
                Some(Payload::OpenCommandPaneFloatingPayload(command_to_run_payload)) => {
                    let floating_pane_coordinates = command_to_run_payload
                        .floating_pane_coordinates
                        .map(|f| f.into());
                    match command_to_run_payload.command_to_run {
                        Some(command_to_run) => {
                            let context: BTreeMap<String, String> = command_to_run_payload
                                .context
                                .into_iter()
                                .map(|e| (e.name, e.value))
                                .collect();
                            Ok(PluginCommand::OpenCommandPaneFloating(
                                command_to_run.try_into()?,
                                floating_pane_coordinates,
                                context,
                            ))
                        },
                        None => Err("Malformed open command pane floating payload"),
                    }
                },
                _ => Err("Mismatched payload for OpenCommandPaneFloating"),
            },
            Some(CommandName::OpenCommandPaneOverlay) => match protobuf_plugin_command.payload {
                Some(Payload::OpenCommandPaneOverlayPayload(payload)) => {
                    let placement: OverlayPlacement = payload
                        .placement
                        .ok_or("Malformed open command pane overlay payload")?
                        .try_into()?;
                    match payload.command_to_run {
                        Some(command_to_run) => {
                            let context: BTreeMap<String, String> = payload
                                .context
                                .into_iter()
                                .map(|e| (e.name, e.value))
                                .collect();
                            Ok(PluginCommand::OpenCommandPaneOverlay(
                                command_to_run.try_into()?,
                                placement,
                                context,
                            ))
                        },
                        None => Err("Malformed open command pane overlay payload"),
                    }
                },
                _ => Err("Mismatched payload for OpenCommandPaneOverlay"),
            },
            Some(CommandName::SwitchTabTo) => match protobuf_plugin_command.payload {
                Some(Payload::SwitchTabToPayload(switch_to_tab_payload)) => {
                    Ok(PluginCommand::SwitchTabTo(switch_to_tab_payload.tab_index))
                },
                _ => Err("Mismatched payload for SwitchToTab"),
            },
            Some(CommandName::SetTimeout) => match protobuf_plugin_command.payload {
                Some(Payload::SetTimeoutPayload(set_timeout_payload)) => {
                    Ok(PluginCommand::SetTimeout(set_timeout_payload.seconds))
                },
                _ => Err("Mismatched payload for SetTimeout"),
            },
            Some(CommandName::ExecCmd) => match protobuf_plugin_command.payload {
                Some(Payload::ExecCmdPayload(exec_cmd_payload)) => {
                    Ok(PluginCommand::ExecCmd(exec_cmd_payload.command_line))
                },
                _ => Err("Mismatched payload for ExecCmd"),
            },
            Some(CommandName::PostMessageTo) => match protobuf_plugin_command.payload {
                Some(Payload::PostMessageToPayload(post_message_to_payload)) => {
                    match post_message_to_payload.message {
                        Some(message) => Ok(PluginCommand::PostMessageTo(message.try_into()?)),
                        None => Err("Malformed post message to payload"),
                    }
                },
                _ => Err("Mismatched payload for PostMessageTo"),
            },
            Some(CommandName::PostMessageToPlugin) => match protobuf_plugin_command.payload {
                Some(Payload::PostMessageToPluginPayload(post_message_to_payload)) => {
                    match post_message_to_payload.message {
                        Some(message) => {
                            Ok(PluginCommand::PostMessageToPlugin(message.try_into()?))
                        },
                        None => Err("Malformed post message to plugin payload"),
                    }
                },
                _ => Err("Mismatched payload for PostMessageToPlugin"),
            },
            Some(CommandName::HideSelf) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("HideSelf should not have a payload");
                }
                Ok(PluginCommand::HideSelf)
            },
            Some(CommandName::ShowSelf) => match protobuf_plugin_command.payload {
                Some(Payload::ShowSelfPayload(should_float_if_hidden)) => {
                    Ok(PluginCommand::ShowSelf(should_float_if_hidden))
                },
                _ => Err("Mismatched payload for ShowSelf"),
            },
            Some(CommandName::SwitchToMode) => match protobuf_plugin_command.payload {
                Some(Payload::SwitchToModePayload(switch_to_mode_payload)) => {
                    match ProtobufInputMode::try_from(switch_to_mode_payload.input_mode).ok() {
                        Some(protobuf_input_mode) => {
                            Ok(PluginCommand::SwitchToMode(protobuf_input_mode.try_into()?))
                        },
                        None => Err("Malformed switch to mode payload"),
                    }
                },
                _ => Err("Mismatched payload for SwitchToMode"),
            },
            Some(CommandName::NewTabsWithLayout) => match protobuf_plugin_command.payload {
                Some(Payload::NewTabsWithLayoutPayload(raw_layout)) => {
                    Ok(PluginCommand::NewTabsWithLayout(raw_layout))
                },
                _ => Err("Mismatched payload for NewTabsWithLayout"),
            },
            Some(CommandName::NewTab) => match protobuf_plugin_command.payload {
                Some(Payload::NewTabPayload(protobuf_new_tab_payload)) => {
                    Ok(PluginCommand::NewTab {
                        name: protobuf_new_tab_payload.name,
                        cwd: protobuf_new_tab_payload.cwd,
                    })
                },
                None => Ok(PluginCommand::NewTab {
                    name: None,
                    cwd: None,
                }),
                _ => Err("Mismatched payload for NewTab"),
            },
            Some(CommandName::NewTabUnfocused) => match protobuf_plugin_command.payload {
                Some(Payload::NewTabUnfocusedPayload(payload)) => {
                    Ok(PluginCommand::NewTabUnfocused {
                        name: payload.name,
                        cwd: payload.cwd,
                    })
                },
                None => Ok(PluginCommand::NewTabUnfocused {
                    name: None,
                    cwd: None,
                }),
                _ => Err("Mismatched payload for NewTabUnfocused"),
            },
            Some(CommandName::NewTiledPaneInTab) => match protobuf_plugin_command.payload {
                Some(Payload::NewTiledPaneInTabPayload(payload)) => {
                    Ok(PluginCommand::NewTiledPaneInTab {
                        tab_position: payload.tab_position as usize,
                    })
                },
                _ => Err("Mismatched payload for NewTiledPaneInTab"),
            },
            Some(CommandName::ToggleFloatingPanes) => match protobuf_plugin_command.payload {
                Some(Payload::ToggleFloatingPanesPayload(payload)) => {
                    Ok(PluginCommand::ToggleFloatingPanes {
                        tab_id: payload.tab_id,
                    })
                },
                _ => Ok(PluginCommand::ToggleFloatingPanes { tab_id: None }),
            },
            Some(CommandName::NewPane) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("NewPane should not have a payload");
                }
                Ok(PluginCommand::NewPane)
            },
            Some(CommandName::GoToNextTab) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("GoToNextTab should not have a payload");
                }
                Ok(PluginCommand::GoToNextTab)
            },
            Some(CommandName::GoToPreviousTab) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("GoToPreviousTab should not have a payload");
                }
                Ok(PluginCommand::GoToPreviousTab)
            },
            Some(CommandName::Resize) => match protobuf_plugin_command.payload {
                Some(Payload::ResizePayload(resize_payload)) => match resize_payload.resize {
                    Some(resize) => Ok(PluginCommand::Resize(resize.try_into()?)),
                    None => Err("Malformed switch resize payload"),
                },
                _ => Err("Mismatched payload for Resize"),
            },
            Some(CommandName::ResizeWithDirection) => match protobuf_plugin_command.payload {
                Some(Payload::ResizeWithDirectionPayload(resize_with_direction_payload)) => {
                    match resize_with_direction_payload.resize {
                        Some(resize) => Ok(PluginCommand::ResizeWithDirection(resize.try_into()?)),
                        None => Err("Malformed switch resize payload"),
                    }
                },
                _ => Err("Mismatched payload for Resize"),
            },
            Some(CommandName::FocusNextPane) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("FocusNextPane should not have a payload");
                }
                Ok(PluginCommand::FocusNextPane)
            },
            Some(CommandName::FocusPreviousPane) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("FocusPreviousPane should not have a payload");
                }
                Ok(PluginCommand::FocusPreviousPane)
            },
            Some(CommandName::FocusLastPane) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("FocusLastPane should not have a payload");
                }
                Ok(PluginCommand::FocusLastPane)
            },
            Some(CommandName::MoveFocus) => match protobuf_plugin_command.payload {
                Some(Payload::MoveFocusPayload(move_payload)) => match move_payload.direction {
                    Some(direction) => Ok(PluginCommand::MoveFocus(direction.try_into()?)),
                    None => Err("Malformed move focus payload"),
                },
                _ => Err("Mismatched payload for MoveFocus"),
            },
            Some(CommandName::MoveFocusOrTab) => match protobuf_plugin_command.payload {
                Some(Payload::MoveFocusOrTabPayload(move_payload)) => {
                    match move_payload.direction {
                        Some(direction) => Ok(PluginCommand::MoveFocusOrTab(direction.try_into()?)),
                        None => Err("Malformed move focus or tab payload"),
                    }
                },
                _ => Err("Mismatched payload for MoveFocusOrTab"),
            },
            Some(CommandName::Detach) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("Detach should not have a payload");
                }
                Ok(PluginCommand::Detach)
            },
            Some(CommandName::EditScrollback) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("EditScrollback should not have a payload");
                }
                Ok(PluginCommand::EditScrollback)
            },
            Some(CommandName::Write) => match protobuf_plugin_command.payload {
                Some(Payload::WritePayload(bytes)) => Ok(PluginCommand::Write(bytes)),
                _ => Err("Mismatched payload for Write"),
            },
            Some(CommandName::WriteChars) => match protobuf_plugin_command.payload {
                Some(Payload::WriteCharsPayload(chars)) => Ok(PluginCommand::WriteChars(chars)),
                _ => Err("Mismatched payload for WriteChars"),
            },
            Some(CommandName::ToggleTab) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("ToggleTab should not have a payload");
                }
                Ok(PluginCommand::ToggleTab)
            },
            Some(CommandName::MovePane) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("MovePane should not have a payload");
                }
                Ok(PluginCommand::MovePane)
            },
            Some(CommandName::MovePaneWithDirection) => match protobuf_plugin_command.payload {
                Some(Payload::MovePaneWithDirectionPayload(move_payload)) => {
                    match move_payload.direction {
                        Some(direction) => {
                            Ok(PluginCommand::MovePaneWithDirection(direction.try_into()?))
                        },
                        None => Err("Malformed MovePaneWithDirection payload"),
                    }
                },
                _ => Err("Mismatched payload for MovePaneWithDirection"),
            },
            Some(CommandName::ClearScreen) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("ClearScreen should not have a payload");
                }
                Ok(PluginCommand::ClearScreen)
            },
            Some(CommandName::ScrollUp) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("ScrollUp should not have a payload");
                }
                Ok(PluginCommand::ScrollUp)
            },
            Some(CommandName::ScrollDown) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("ScrollDown should not have a payload");
                }
                Ok(PluginCommand::ScrollDown)
            },
            Some(CommandName::ScrollToTop) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("ScrollToTop should not have a payload");
                }
                Ok(PluginCommand::ScrollToTop)
            },
            Some(CommandName::ScrollToBottom) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("ScrollToBottom should not have a payload");
                }
                Ok(PluginCommand::ScrollToBottom)
            },
            Some(CommandName::PageScrollUp) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("PageScrollUp should not have a payload");
                }
                Ok(PluginCommand::PageScrollUp)
            },
            Some(CommandName::PageScrollDown) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("PageScrollDown should not have a payload");
                }
                Ok(PluginCommand::PageScrollDown)
            },
            Some(CommandName::ToggleFocusFullscreen) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("ToggleFocusFullscreen should not have a payload");
                }
                Ok(PluginCommand::ToggleFocusFullscreen)
            },
            Some(CommandName::ToggleFocusNoUiFullscreen) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("ToggleFocusNoUiFullscreen should not have a payload");
                }
                Ok(PluginCommand::ToggleFocusNoUiFullscreen)
            },
            Some(CommandName::FocusHostSession) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("FocusHostSession should not have a payload");
                }
                Ok(PluginCommand::FocusHostSession)
            },
            Some(CommandName::ListRecordings) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("ListRecordings should not have a payload");
                }
                Ok(PluginCommand::ListRecordings)
            },
            Some(CommandName::ListNotifications) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("ListNotifications should not have a payload");
                }
                Ok(PluginCommand::ListNotifications)
            },
            Some(CommandName::SearchRecordings) => match protobuf_plugin_command.payload {
                Some(Payload::SearchRecordingsPayload(payload)) => {
                    Ok(PluginCommand::SearchRecordings {
                        query: payload.query,
                        limit: payload.limit as usize,
                    })
                },
                _ => Err("Malformed payload for SearchRecordings"),
            },
            Some(CommandName::TogglePaneFrames) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("TogglePaneFrames should not have a payload");
                }
                Ok(PluginCommand::TogglePaneFrames)
            },
            Some(CommandName::SetPaneFrameStyle) => match protobuf_plugin_command.payload {
                Some(Payload::SetPaneFrameStylePayload(payload)) => {
                    match ProtobufPaneFrameStyle::try_from(payload.pane_frame_style).ok() {
                        Some(protobuf_pane_frame_style) => Ok(PluginCommand::SetPaneFrameStyle(
                            protobuf_pane_frame_style.try_into()?,
                        )),
                        None => Err("Malformed SetPaneFrameStyle payload"),
                    }
                },
                _ => Err("Mismatched payload for SetPaneFrameStyle"),
            },
            Some(CommandName::TogglePaneEmbedOrEject) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("TogglePaneEmbedOrEject should not have a payload");
                }
                Ok(PluginCommand::TogglePaneEmbedOrEject)
            },
            Some(CommandName::UndoRenamePane) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("UndoRenamePane should not have a payload");
                }
                Ok(PluginCommand::UndoRenamePane)
            },
            Some(CommandName::CloseFocus) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("CloseFocus should not have a payload");
                }
                Ok(PluginCommand::CloseFocus)
            },
            Some(CommandName::ToggleActiveTabSync) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("ToggleActiveTabSync should not have a payload");
                }
                Ok(PluginCommand::ToggleActiveTabSync)
            },
            Some(CommandName::CloseFocusedTab) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("CloseFocusedTab should not have a payload");
                }
                Ok(PluginCommand::CloseFocusedTab)
            },
            Some(CommandName::UndoRenameTab) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("UndoRenameTab should not have a payload");
                }
                Ok(PluginCommand::UndoRenameTab)
            },
            Some(CommandName::QuitZellij) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("QuitZellij should not have a payload");
                }
                Ok(PluginCommand::QuitZellij)
            },
            Some(CommandName::PreviousSwapLayout) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("PreviousSwapLayout should not have a payload");
                }
                Ok(PluginCommand::PreviousSwapLayout)
            },
            Some(CommandName::NextSwapLayout) => {
                if protobuf_plugin_command.payload.is_some() {
                    return Err("NextSwapLayout should not have a payload");
                }
                Ok(PluginCommand::NextSwapLayout)
            },
            Some(CommandName::GoToTabName) => match protobuf_plugin_command.payload {
                Some(Payload::GoToTabNamePayload(tab_name)) => {
                    Ok(PluginCommand::GoToTabName(tab_name))
                },
                _ => Err("Mismatched payload for GoToTabName"),
            },
            Some(CommandName::FocusOrCreateTab) => match protobuf_plugin_command.payload {
                Some(Payload::FocusOrCreateTabPayload(tab_name)) => {
                    Ok(PluginCommand::FocusOrCreateTab(tab_name))
                },
                _ => Err("Mismatched payload for FocusOrCreateTab"),
            },
            Some(CommandName::GoToTab) => match protobuf_plugin_command.payload {
                Some(Payload::GoToTabPayload(tab_index)) => Ok(PluginCommand::GoToTab(tab_index)),
                _ => Err("Mismatched payload for GoToTab"),
            },
            Some(CommandName::StartOrReloadPlugin) => match protobuf_plugin_command.payload {
                Some(Payload::StartOrReloadPluginPayload(url)) => {
                    Ok(PluginCommand::StartOrReloadPlugin(url))
                },
                _ => Err("Mismatched payload for StartOrReloadPlugin"),
            },
            Some(CommandName::CloseTerminalPane) => match protobuf_plugin_command.payload {
                Some(Payload::CloseTerminalPanePayload(pane_id)) => {
                    Ok(PluginCommand::CloseTerminalPane(pane_id))
                },
                _ => Err("Mismatched payload for CloseTerminalPane"),
            },
            Some(CommandName::ClosePluginPane) => match protobuf_plugin_command.payload {
                Some(Payload::ClosePluginPanePayload(pane_id)) => {
                    Ok(PluginCommand::ClosePluginPane(pane_id))
                },
                _ => Err("Mismatched payload for ClosePluginPane"),
            },
            Some(CommandName::FocusTerminalPane) => match protobuf_plugin_command.payload {
                Some(Payload::FocusTerminalPanePayload(payload)) => {
                    let pane_id = payload.pane_id;
                    let should_float = payload.should_float;
                    let should_be_in_place = payload.should_be_in_place;
                    Ok(PluginCommand::FocusTerminalPane(
                        pane_id,
                        should_float,
                        should_be_in_place,
                    ))
                },
                _ => Err("Mismatched payload for ClosePluginPane"),
            },
            Some(CommandName::FocusPluginPane) => match protobuf_plugin_command.payload {
                Some(Payload::FocusPluginPanePayload(payload)) => {
                    let pane_id = payload.pane_id;
                    let should_float = payload.should_float;
                    let should_be_in_place = payload.should_be_in_place;
                    Ok(PluginCommand::FocusPluginPane(
                        pane_id,
                        should_float,
                        should_be_in_place,
                    ))
                },
                _ => Err("Mismatched payload for ClosePluginPane"),
            },
            Some(CommandName::RenameTerminalPane) => match protobuf_plugin_command.payload {
                Some(Payload::RenameTerminalPanePayload(payload)) => {
                    let pane_id = payload.id;
                    let new_name = payload.new_name;
                    Ok(PluginCommand::RenameTerminalPane(pane_id, new_name))
                },
                _ => Err("Mismatched payload for RenameTerminalPane"),
            },
            Some(CommandName::RenamePluginPane) => match protobuf_plugin_command.payload {
                Some(Payload::RenamePluginPanePayload(payload)) => {
                    let pane_id = payload.id;
                    let new_name = payload.new_name;
                    Ok(PluginCommand::RenamePluginPane(pane_id, new_name))
                },
                _ => Err("Mismatched payload for RenamePluginPane"),
            },
            Some(CommandName::RenameTab) => match protobuf_plugin_command.payload {
                Some(Payload::RenameTabPayload(payload)) => {
                    let tab_index = payload.id;
                    let name = payload.new_name;
                    Ok(PluginCommand::RenameTab(tab_index, name))
                },
                _ => Err("Mismatched payload for RenameTab"),
            },
            Some(CommandName::ReportCrash) => match protobuf_plugin_command.payload {
                Some(Payload::ReportCrashPayload(payload)) => {
                    Ok(PluginCommand::ReportPanic(payload))
                },
                _ => Err("Mismatched payload for ReportCrash"),
            },
            Some(CommandName::RequestPluginPermissions) => match protobuf_plugin_command.payload {
                Some(Payload::RequestPluginPermissionPayload(payload)) => {
                    Ok(PluginCommand::RequestPluginPermissions(
                        payload
                            .permissions
                            .iter()
                            .filter_map(|p| ProtobufPermissionType::try_from(*p).ok())
                            .filter_map(|p| PermissionType::try_from(p).ok())
                            .collect(),
                    ))
                },
                _ => Err("Mismatched payload for RequestPluginPermission"),
            },
            Some(CommandName::SwitchSession) => match protobuf_plugin_command.payload {
                Some(Payload::SwitchSessionPayload(payload)) => {
                    let pane_id = match (payload.pane_id, payload.pane_id_is_plugin) {
                        (Some(pane_id), Some(is_plugin)) => Some((pane_id, is_plugin)),
                        (None, None) => None,
                        _ => {
                            return Err("Malformed payload for SwitchSession, 'pane_id' and 'is_plugin' must be included together or not at all")
                        }
                    };
                    Ok(PluginCommand::SwitchSession(ConnectToSession {
                        name: payload.name,
                        tab_position: payload.tab_position.map(|p| p as usize),
                        pane_id,
                        layout: payload.layout.and_then(|l| l.try_into().ok()),
                        cwd: payload.cwd.map(PathBuf::from),
                    }))
                },
                _ => Err("Mismatched payload for SwitchSession"),
            },
            Some(CommandName::OpenTerminalInPlace) => match protobuf_plugin_command.payload {
                Some(Payload::OpenTerminalInPlacePayload(file_to_open_payload)) => {
                    match file_to_open_payload.file_to_open {
                        Some(file_to_open) => {
                            Ok(PluginCommand::OpenTerminalInPlace(file_to_open.try_into()?))
                        },
                        None => Err("Malformed open terminal in-place payload"),
                    }
                },
                _ => Err("Mismatched payload for OpenTerminalInPlace"),
            },
            Some(CommandName::OpenFileInPlace) => match protobuf_plugin_command.payload {
                Some(Payload::OpenFileInPlacePayload(file_to_open_payload)) => {
                    match file_to_open_payload.file_to_open {
                        Some(file_to_open) => {
                            let context: BTreeMap<String, String> = file_to_open_payload
                                .context
                                .into_iter()
                                .map(|e| (e.name, e.value))
                                .collect();
                            Ok(PluginCommand::OpenFileInPlace(
                                file_to_open.try_into()?,
                                context,
                            ))
                        },
                        None => Err("Malformed open file in place payload"),
                    }
                },
                _ => Err("Mismatched payload for OpenFileInPlace"),
            },
            Some(CommandName::OpenCommandInPlace) => match protobuf_plugin_command.payload {
                Some(Payload::OpenCommandPaneInPlacePayload(command_to_run_payload)) => {
                    match command_to_run_payload.command_to_run {
                        Some(command_to_run) => {
                            let context: BTreeMap<String, String> = command_to_run_payload
                                .context
                                .into_iter()
                                .map(|e| (e.name, e.value))
                                .collect();
                            Ok(PluginCommand::OpenCommandPaneInPlace(
                                command_to_run.try_into()?,
                                context,
                            ))
                        },
                        None => Err("Malformed open command pane in-place payload"),
                    }
                },
                _ => Err("Mismatched payload for OpenCommandPaneInPlace"),
            },
            Some(CommandName::RunCommand) => match protobuf_plugin_command.payload {
                Some(Payload::RunCommandPayload(run_command_payload)) => {
                    let env_variables: BTreeMap<String, String> = run_command_payload
                        .env_variables
                        .into_iter()
                        .map(|e| (e.name, e.value))
                        .collect();
                    let context: BTreeMap<String, String> = run_command_payload
                        .context
                        .into_iter()
                        .map(|e| (e.name, e.value))
                        .collect();
                    Ok(PluginCommand::RunCommand(
                        run_command_payload.command_line,
                        env_variables,
                        PathBuf::from(run_command_payload.cwd),
                        context,
                        run_command_payload
                            .max_output_bytes
                            .map(usize::try_from)
                            .transpose()
                            .map_err(|_| "Run command output limit is too large")?,
                    ))
                },
                _ => Err("Mismatched payload for RunCommand"),
            },
            Some(CommandName::WebRequest) => match protobuf_plugin_command.payload {
                Some(Payload::WebRequestPayload(web_request_payload)) => {
                    let context: BTreeMap<String, String> = web_request_payload
                        .context
                        .into_iter()
                        .map(|e| (e.name, e.value))
                        .collect();
                    let headers: BTreeMap<String, String> = web_request_payload
                        .headers
                        .into_iter()
                        .map(|e| (e.name, e.value))
                        .collect();
                    let verb = match ProtobufHttpVerb::try_from(web_request_payload.verb).ok() {
                        Some(verb) => verb.into(),
                        None => {
                            return Err("Unrecognized http verb");
                        },
                    };
                    Ok(PluginCommand::WebRequest(
                        web_request_payload.url,
                        verb,
                        headers,
                        web_request_payload.body,
                        context,
                        web_request_payload.stream_id,
                        web_request_payload.proxy,
                    ))
                },
                _ => Err("Mismatched payload for WebRequest"),
            },
            Some(CommandName::CancelWebRequest) => match protobuf_plugin_command.payload {
                Some(Payload::CancelWebRequestPayload(payload)) => {
                    Ok(PluginCommand::CancelWebRequest(payload.stream_id))
                },
                _ => Err("Mismatched payload for CancelWebRequest"),
            },
            Some(CommandName::DeleteDeadSession) => match protobuf_plugin_command.payload {
                Some(Payload::DeleteDeadSessionPayload(dead_session_name)) => {
                    Ok(PluginCommand::DeleteDeadSession(dead_session_name))
                },
                _ => Err("Mismatched payload for DeleteDeadSession"),
            },
            Some(CommandName::DeleteAllDeadSessions) => Ok(PluginCommand::DeleteAllDeadSessions),
            Some(CommandName::RenameSession) => match protobuf_plugin_command.payload {
                Some(Payload::RenameSessionPayload(new_session_name)) => {
                    Ok(PluginCommand::RenameSession(new_session_name))
                },
                _ => Err("Mismatched payload for RenameSession"),
            },
            Some(CommandName::Notify) => match protobuf_plugin_command.payload {
                Some(Payload::NotifyPayload(text)) => Ok(PluginCommand::Notify(text, None)),
                Some(Payload::NotifyWithPanePayload(payload)) => Ok(PluginCommand::Notify(
                    payload.text,
                    Some(
                        payload
                            .pane_id
                            .ok_or("NotifyWithPane must have a pane id")?
                            .try_into()?,
                    ),
                )),
                _ => Err("Mismatched payload for Notify"),
            },
            Some(CommandName::UnblockCliPipeInput) => match protobuf_plugin_command.payload {
                Some(Payload::UnblockCliPipeInputPayload(pipe_name)) => {
                    Ok(PluginCommand::UnblockCliPipeInput(pipe_name))
                },
                _ => Err("Mismatched payload for UnblockPipeInput"),
            },
            Some(CommandName::BlockCliPipeInput) => match protobuf_plugin_command.payload {
                Some(Payload::BlockCliPipeInputPayload(pipe_name)) => {
                    Ok(PluginCommand::BlockCliPipeInput(pipe_name))
                },
                _ => Err("Mismatched payload for BlockPipeInput"),
            },
            Some(CommandName::CliPipeOutput) => match protobuf_plugin_command.payload {
                Some(Payload::CliPipeOutputPayload(CliPipeOutputPayload { pipe_name, output })) => {
                    Ok(PluginCommand::CliPipeOutput(pipe_name, output))
                },
                _ => Err("Mismatched payload for PipeOutput"),
            },
            Some(CommandName::MessageToPlugin) => match protobuf_plugin_command.payload {
                Some(Payload::MessageToPluginPayload(MessageToPluginPayload {
                    plugin_url,
                    plugin_config,
                    message_name,
                    message_payload,
                    message_args,
                    new_plugin_args,
                    destination_plugin_id,
                    floating_pane_coordinates,
                })) => {
                    let plugin_config: BTreeMap<String, String> = plugin_config
                        .into_iter()
                        .map(|e| (e.name, e.value))
                        .collect();
                    let message_args: BTreeMap<String, String> = message_args
                        .into_iter()
                        .map(|e| (e.name, e.value))
                        .collect();
                    Ok(PluginCommand::MessageToPlugin(MessageToPlugin {
                        plugin_url,
                        plugin_config,
                        message_name,
                        message_payload,
                        message_args,
                        new_plugin_args: new_plugin_args.map(|protobuf_new_plugin_args| {
                            NewPluginArgs {
                                should_float: protobuf_new_plugin_args.should_float,
                                pane_id_to_replace: protobuf_new_plugin_args
                                    .pane_id_to_replace
                                    .and_then(|p_id| PaneId::try_from(p_id).ok()),
                                pane_title: protobuf_new_plugin_args.pane_title,
                                cwd: protobuf_new_plugin_args.cwd.map(PathBuf::from),
                                skip_cache: protobuf_new_plugin_args.skip_cache,
                                should_focus: protobuf_new_plugin_args.should_focus,
                            }
                        }),
                        destination_plugin_id,
                        floating_pane_coordinates: floating_pane_coordinates
                            .and_then(|f| f.try_into().ok()),
                    }))
                },
                _ => Err("Mismatched payload for MessageToPlugin"),
            },
            Some(CommandName::DisconnectOtherClients) => match protobuf_plugin_command.payload {
                None => Ok(PluginCommand::DisconnectOtherClients),
                _ => Err("Mismatched payload for DisconnectOtherClients"),
            },
            Some(CommandName::KillSessions) => match protobuf_plugin_command.payload {
                Some(Payload::KillSessionsPayload(KillSessionsPayload { session_names })) => {
                    Ok(PluginCommand::KillSessions(session_names))
                },
                _ => Err("Mismatched payload for KillSessions"),
            },
            Some(CommandName::ScanHostFolder) => match protobuf_plugin_command.payload {
                Some(Payload::ScanHostFolderPayload(folder_to_scan)) => {
                    Ok(PluginCommand::ScanHostFolder(PathBuf::from(folder_to_scan)))
                },
                _ => Err("Mismatched payload for ScanHostFolder"),
            },
            Some(CommandName::WatchFilesystem) => match protobuf_plugin_command.payload {
                Some(_) => Err("WatchFilesystem should have no payload, found a payload"),
                None => Ok(PluginCommand::WatchFilesystem),
            },
            Some(CommandName::FsRead) => match protobuf_plugin_command.payload {
                Some(Payload::FsReadPayload(payload)) => Ok(PluginCommand::FsRead {
                    path: PathBuf::from(payload.path),
                    max_bytes: payload.max_bytes.map(|b| b as usize),
                }),
                _ => Err("Mismatched payload for FsRead"),
            },
            Some(CommandName::FsList) => match protobuf_plugin_command.payload {
                Some(Payload::FsListPayload(payload)) => Ok(PluginCommand::FsList {
                    path: PathBuf::from(payload.path),
                }),
                _ => Err("Mismatched payload for FsList"),
            },
            Some(CommandName::FsMetadata) => match protobuf_plugin_command.payload {
                Some(Payload::FsMetadataPayload(payload)) => Ok(PluginCommand::FsMetadata {
                    path: PathBuf::from(payload.path),
                }),
                _ => Err("Mismatched payload for FsMetadata"),
            },
            Some(CommandName::FsWrite) => match protobuf_plugin_command.payload {
                Some(Payload::FsWritePayload(payload)) => Ok(PluginCommand::FsWrite {
                    path: PathBuf::from(payload.path),
                    contents: payload.contents,
                    append: payload.append,
                    create_dirs: payload.create_dirs,
                }),
                _ => Err("Mismatched payload for FsWrite"),
            },
            Some(CommandName::FsRemove) => match protobuf_plugin_command.payload {
                Some(Payload::FsRemovePayload(payload)) => Ok(PluginCommand::FsRemove {
                    path: PathBuf::from(payload.path),
                    recursive: payload.recursive,
                }),
                _ => Err("Mismatched payload for FsRemove"),
            },
            Some(CommandName::FsCreateDir) => match protobuf_plugin_command.payload {
                Some(Payload::FsCreateDirPayload(payload)) => Ok(PluginCommand::FsCreateDir {
                    path: PathBuf::from(payload.path),
                    all: payload.all,
                }),
                _ => Err("Mismatched payload for FsCreateDir"),
            },
            Some(CommandName::GrantPluginPermissions) => match protobuf_plugin_command.payload {
                Some(Payload::GrantPluginPermissionsPayload(payload)) => {
                    Ok(PluginCommand::GrantPluginPermissions {
                        plugin_url: payload.plugin_url,
                        permissions: payload
                            .permissions
                            .iter()
                            .filter_map(|p| ProtobufPermissionType::try_from(*p).ok())
                            .filter_map(|p| PermissionType::try_from(p).ok())
                            .collect(),
                    })
                },
                _ => Err("Mismatched payload for GrantPluginPermissions"),
            },
            Some(CommandName::FsRename) => match protobuf_plugin_command.payload {
                Some(Payload::FsRenamePayload(payload)) => Ok(PluginCommand::FsRename {
                    from: PathBuf::from(payload.from),
                    to: PathBuf::from(payload.to),
                }),
                _ => Err("Mismatched payload for FsRename"),
            },
            Some(CommandName::GetSessionList) => match protobuf_plugin_command.payload {
                Some(Payload::GetSessionListPayload(_)) => Ok(PluginCommand::GetSessionList),
                None => Ok(PluginCommand::GetSessionList),
                _ => Err("Mismatched payload for GetSessionList"),
            },
            Some(CommandName::KillSessionsAndReply) => match protobuf_plugin_command.payload {
                Some(Payload::KillSessionsAndReplyPayload(KillSessionsPayload {
                    session_names,
                })) => Ok(PluginCommand::KillSessionsAndReply(session_names)),
                _ => Err("Mismatched payload for KillSessionsAndReply"),
            },
            Some(CommandName::DeleteDeadSessionAndReply) => match protobuf_plugin_command.payload {
                Some(Payload::DeleteDeadSessionAndReplyPayload(dead_session_name)) => {
                    Ok(PluginCommand::DeleteDeadSessionAndReply(dead_session_name))
                },
                _ => Err("Mismatched payload for DeleteDeadSessionAndReply"),
            },
            Some(CommandName::DeleteAllDeadSessionsAndReply) => {
                Ok(PluginCommand::DeleteAllDeadSessionsAndReply)
            },
            Some(CommandName::SetSoftKeyboard) => match protobuf_plugin_command.payload {
                Some(Payload::SetSoftKeyboardPayload(payload)) => {
                    Ok(PluginCommand::SetSoftKeyboard(payload.on))
                },
                _ => Err("Mismatched payload for SetSoftKeyboard"),
            },
            Some(CommandName::DumpSessionLayout) => match protobuf_plugin_command.payload {
                Some(Payload::DumpSessionLayoutPayload(payload)) => {
                    Ok(PluginCommand::DumpSessionLayout {
                        tab_index: payload.tab_index.map(|i| i as usize),
                    })
                },
                None => Ok(PluginCommand::DumpSessionLayout { tab_index: None }),
                _ => Err("Mismatched payload for DumpSessionLayout"),
            },
            Some(CommandName::CloseSelf) => match protobuf_plugin_command.payload {
                Some(_) => Err("CloseSelf should have no payload, found a payload"),
                None => Ok(PluginCommand::CloseSelf),
            },
            Some(CommandName::NewTabsWithLayoutInfo) => match protobuf_plugin_command.payload {
                Some(Payload::NewTabsWithLayoutInfoPayload(new_tabs_with_layout_info_payload)) => {
                    new_tabs_with_layout_info_payload
                        .layout_info
                        .and_then(|layout_info| {
                            Some(PluginCommand::NewTabsWithLayoutInfo(
                                layout_info.try_into().ok()?,
                            ))
                        })
                        .ok_or("Failed to parse NewTabsWithLayoutInfo command")
                },
                _ => Err("Mismatched payload for NewTabsWithLayoutInfo"),
            },
            Some(CommandName::Reconfigure) => match protobuf_plugin_command.payload {
                Some(Payload::ReconfigurePayload(reconfigure_payload)) => {
                    Ok(PluginCommand::Reconfigure(
                        reconfigure_payload.config,
                        reconfigure_payload.write_to_disk,
                    ))
                },
                _ => Err("Mismatched payload for Reconfigure"),
            },
            Some(CommandName::HidePaneWithId) => match protobuf_plugin_command.payload {
                Some(Payload::HidePaneWithIdPayload(hide_pane_with_id_payload)) => {
                    let pane_id = hide_pane_with_id_payload
                        .pane_id
                        .and_then(|p_id| PaneId::try_from(p_id).ok())
                        .ok_or("Failed to parse HidePaneWithId command")?;
                    Ok(PluginCommand::HidePaneWithId(pane_id))
                },
                _ => Err("Mismatched payload for HidePaneWithId"),
            },
            Some(CommandName::ShowPaneWithId) => match protobuf_plugin_command.payload {
                Some(Payload::ShowPaneWithIdPayload(show_pane_with_id_payload)) => {
                    let pane_id = show_pane_with_id_payload
                        .pane_id
                        .and_then(|p_id| PaneId::try_from(p_id).ok())
                        .ok_or("Failed to parse ShowPaneWithId command")?;
                    let should_float_if_hidden = show_pane_with_id_payload.should_float_if_hidden;
                    let should_focus_pane = show_pane_with_id_payload.should_focus_pane;
                    Ok(PluginCommand::ShowPaneWithId(
                        pane_id,
                        should_float_if_hidden,
                        should_focus_pane,
                    ))
                },
                _ => Err("Mismatched payload for ShowPaneWithId"),
            },
            Some(CommandName::OpenCommandPaneBackground) => match protobuf_plugin_command.payload {
                Some(Payload::OpenCommandPaneBackgroundPayload(command_to_run_payload)) => {
                    match command_to_run_payload.command_to_run {
                        Some(command_to_run) => {
                            let context: BTreeMap<String, String> = command_to_run_payload
                                .context
                                .into_iter()
                                .map(|e| (e.name, e.value))
                                .collect();
                            Ok(PluginCommand::OpenCommandPaneBackground(
                                command_to_run.try_into()?,
                                context,
                            ))
                        },
                        None => Err("Malformed open command pane background payload"),
                    }
                },
                _ => Err("Mismatched payload for OpenCommandPaneBackground"),
            },
            Some(CommandName::RerunCommandPane) => match protobuf_plugin_command.payload {
                Some(Payload::RerunCommandPanePayload(rerun_command_pane_payload)) => Ok(
                    PluginCommand::RerunCommandPane(rerun_command_pane_payload.terminal_pane_id),
                ),
                _ => Err("Mismatched payload for RerunCommandPane"),
            },
            Some(CommandName::ResizePaneIdWithDirection) => match protobuf_plugin_command.payload {
                Some(Payload::ResizePaneIdWithDirectionPayload(resize_with_direction_payload)) => {
                    match (
                        resize_with_direction_payload.resize,
                        resize_with_direction_payload.pane_id,
                    ) {
                        (Some(resize), Some(pane_id)) => {
                            Ok(PluginCommand::ResizePaneIdWithDirection(
                                resize.try_into()?,
                                pane_id.try_into()?,
                            ))
                        },
                        _ => Err("Malformed resize_pane_with_id payload"),
                    }
                },
                _ => Err("Mismatched payload for Resize"),
            },
            Some(CommandName::EditScrollbackForPaneWithId) => match protobuf_plugin_command.payload
            {
                Some(Payload::EditScrollbackForPaneWithIdPayload(
                    edit_scrollback_for_pane_with_id_payload,
                )) => match edit_scrollback_for_pane_with_id_payload.pane_id {
                    Some(pane_id) => Ok(PluginCommand::EditScrollbackForPaneWithId(
                        pane_id.try_into()?,
                    )),
                    _ => Err("Malformed edit_scrollback_for_pane_with_id payload"),
                },
                _ => Err("Mismatched payload for EditScrollback"),
            },
            Some(CommandName::GetPaneScrollback) => match protobuf_plugin_command.payload {
                Some(Payload::GetPaneScrollbackPayload(get_pane_scrollback_payload)) => {
                    match get_pane_scrollback_payload.pane_id {
                        Some(pane_id) => Ok(PluginCommand::GetPaneScrollback {
                            pane_id: pane_id.try_into()?,
                            get_full_scrollback: get_pane_scrollback_payload.get_full_scrollback,
                        }),
                        _ => Err("Malformed get_pane_scrollback_payload"),
                    }
                },
                _ => Err("Mismatched payload for GetPaneScrollback"),
            },
            Some(CommandName::WriteToPaneId) => match protobuf_plugin_command.payload {
                Some(Payload::WriteToPaneIdPayload(write_to_pane_id_payload)) => {
                    match write_to_pane_id_payload.pane_id {
                        Some(pane_id) => Ok(PluginCommand::WriteToPaneId(
                            write_to_pane_id_payload.bytes_to_write,
                            pane_id.try_into()?,
                        )),
                        _ => Err("Malformed write_to_pane_id payload"),
                    }
                },
                _ => Err("Mismatched payload for WriteToPaneId"),
            },
            Some(CommandName::WriteCharsToPaneId) => match protobuf_plugin_command.payload {
                Some(Payload::WriteCharsToPaneIdPayload(write_chars_to_pane_id_payload)) => {
                    match write_chars_to_pane_id_payload.pane_id {
                        Some(pane_id) => Ok(PluginCommand::WriteCharsToPaneId(
                            write_chars_to_pane_id_payload.chars_to_write,
                            pane_id.try_into()?,
                        )),
                        _ => Err("Malformed write_chars_to_pane_id payload"),
                    }
                },
                _ => Err("Mismatched payload for WriteCharsCharsToPaneId"),
            },
            Some(CommandName::SendSigintToPaneId) => match protobuf_plugin_command.payload {
                Some(Payload::SendSigintToPaneIdPayload(pane_id)) => {
                    Ok(PluginCommand::SendSigintToPaneId(pane_id.try_into()?))
                },
                _ => Err("Mismatched payload for SendSigintToPaneId"),
            },
            Some(CommandName::SendSigkillToPaneId) => match protobuf_plugin_command.payload {
                Some(Payload::SendSigkillToPaneIdPayload(pane_id)) => {
                    Ok(PluginCommand::SendSigkillToPaneId(pane_id.try_into()?))
                },
                _ => Err("Mismatched payload for SendSigkillToPaneId"),
            },
            Some(CommandName::GetPanePid) => match protobuf_plugin_command.payload {
                Some(Payload::GetPanePidPayload(get_pane_pid_payload)) => {
                    match get_pane_pid_payload.pane_id {
                        Some(pane_id) => Ok(PluginCommand::GetPanePid {
                            pane_id: pane_id.try_into()?,
                        }),
                        _ => Err("Malformed get_pane_pid payload"),
                    }
                },
                _ => Err("Mismatched payload for GetPanePid"),
            },
            Some(CommandName::GetPaneRunningCommand) => match protobuf_plugin_command.payload {
                Some(Payload::GetPaneRunningCommandPayload(payload)) => {
                    Ok(PluginCommand::GetPaneRunningCommand {
                        pane_id: payload
                            .pane_id
                            .ok_or("Malformed GetPaneRunningCommand: missing pane_id")?
                            .try_into()?,
                    })
                },
                _ => Err("Malformed GetPaneRunningCommand payload"),
            },
            Some(CommandName::GetPaneCwd) => match protobuf_plugin_command.payload {
                Some(Payload::GetPaneCwdPayload(payload)) => Ok(PluginCommand::GetPaneCwd {
                    pane_id: payload
                        .pane_id
                        .ok_or("Malformed GetPaneCwd: missing pane_id")?
                        .try_into()?,
                }),
                _ => Err("Malformed GetPaneCwd payload"),
            },
            Some(CommandName::GetPaneGitProbe) => match protobuf_plugin_command.payload {
                Some(Payload::GetPaneGitProbePayload(payload)) => {
                    Ok(PluginCommand::GetPaneGitProbe {
                        pane_id: payload
                            .pane_id
                            .ok_or("Malformed GetPaneGitProbe: missing pane_id")?
                            .try_into()?,
                    })
                },
                _ => Err("Malformed GetPaneGitProbe payload"),
            },
            Some(CommandName::OverrideLayout) => match protobuf_plugin_command.payload {
                Some(Payload::OverrideLayoutPayload(override_layout_payload)) => {
                    let layout_info = override_layout_payload
                        .layout_info
                        .ok_or("OverrideLayout missing layout_info")?
                        .try_into()
                        .map_err(|_| "Failed to parse LayoutInfo")?;
                    let context = override_layout_payload
                        .context
                        .into_iter()
                        .map(|c| (c.name, c.value))
                        .collect();
                    Ok(PluginCommand::OverrideLayout(
                        layout_info,
                        override_layout_payload.retain_existing_terminal_panes,
                        override_layout_payload.retain_existing_plugin_panes,
                        override_layout_payload.apply_only_to_active_tab,
                        context,
                    ))
                },
                _ => Err("Mismatched payload for OverrideLayout"),
            },
            Some(CommandName::SaveLayout) => match protobuf_plugin_command.payload {
                Some(Payload::SaveLayoutPayload(save_layout_payload)) => {
                    Ok(PluginCommand::SaveLayout {
                        layout_name: save_layout_payload.layout_name,
                        layout_kdl: save_layout_payload.layout_kdl,
                        overwrite: save_layout_payload.overwrite,
                    })
                },
                _ => Err("Mismatched payload for SaveLayout"),
            },
            Some(CommandName::DeleteLayout) => match protobuf_plugin_command.payload {
                Some(Payload::DeleteLayoutPayload(delete_layout_payload)) => {
                    Ok(PluginCommand::DeleteLayout {
                        layout_name: delete_layout_payload.layout_name,
                    })
                },
                _ => Err("Mismatched payload for DeleteLayout"),
            },
            Some(CommandName::RenameLayout) => match protobuf_plugin_command.payload {
                Some(Payload::RenameLayoutPayload(rename_layout_payload)) => {
                    Ok(PluginCommand::RenameLayout {
                        old_layout_name: rename_layout_payload.old_layout_name,
                        new_layout_name: rename_layout_payload.new_layout_name,
                    })
                },
                _ => Err("Mismatched payload for RenameLayout"),
            },
            Some(CommandName::EditLayout) => match protobuf_plugin_command.payload {
                Some(Payload::EditLayoutPayload(edit_layout_payload)) => {
                    let layout_name = edit_layout_payload.layout_name;
                    let context: BTreeMap<String, String> = edit_layout_payload
                        .context
                        .into_iter()
                        .map(|e| (e.name, e.value))
                        .collect();
                    Ok(PluginCommand::EditLayout {
                        layout_name,
                        context,
                    })
                },
                _ => Err("Mismatched payload for EditLayout"),
            },
            Some(CommandName::MovePaneWithPaneId) => match protobuf_plugin_command.payload {
                Some(Payload::MovePaneWithPaneIdPayload(move_pane_with_pane_id_payload)) => {
                    match move_pane_with_pane_id_payload.pane_id {
                        Some(pane_id) => Ok(PluginCommand::MovePaneWithPaneId(pane_id.try_into()?)),
                        _ => Err("Malformed move_pane_with_pane_id payload"),
                    }
                },
                _ => Err("Mismatched payload for MovePaneWithPaneId"),
            },
            Some(CommandName::MovePaneWithPaneIdInDirection) => {
                match protobuf_plugin_command.payload {
                    Some(Payload::MovePaneWithPaneIdInDirectionPayload(move_payload)) => {
                        match (move_payload.direction, move_payload.pane_id) {
                            (Some(direction), Some(pane_id)) => {
                                Ok(PluginCommand::MovePaneWithPaneIdInDirection(
                                    pane_id.try_into()?,
                                    direction.try_into()?,
                                ))
                            },
                            _ => Err("Malformed MovePaneWithPaneIdInDirection payload"),
                        }
                    },
                    _ => Err("Mismatched payload for MovePaneWithDirection"),
                }
            },
            Some(CommandName::ClearScreenForPaneId) => match protobuf_plugin_command.payload {
                Some(Payload::ClearScreenForPaneIdPayload(clear_screen_for_pane_id_payload)) => {
                    match clear_screen_for_pane_id_payload.pane_id {
                        Some(pane_id) => {
                            Ok(PluginCommand::ClearScreenForPaneId(pane_id.try_into()?))
                        },
                        _ => Err("Malformed clear_screen_for_pane_id_payload payload"),
                    }
                },
                _ => Err("Mismatched payload for ClearScreenForPaneId"),
            },
            Some(CommandName::ScrollUpInPaneId) => match protobuf_plugin_command.payload {
                Some(Payload::ScrollUpInPaneIdPayload(scroll_up_in_pane_id_payload)) => {
                    match scroll_up_in_pane_id_payload.pane_id {
                        Some(pane_id) => Ok(PluginCommand::ScrollUpInPaneId(pane_id.try_into()?)),
                        _ => Err("Malformed scroll_up_in_pane_id_payload payload"),
                    }
                },
                _ => Err("Mismatched payload for ScrollUpInPaneId"),
            },
            Some(CommandName::ScrollDownInPaneId) => match protobuf_plugin_command.payload {
                Some(Payload::ScrollDownInPaneIdPayload(scroll_down_in_pane_id_payload)) => {
                    match scroll_down_in_pane_id_payload.pane_id {
                        Some(pane_id) => Ok(PluginCommand::ScrollDownInPaneId(pane_id.try_into()?)),
                        _ => Err("Malformed scroll_down_in_pane_id_payload payload"),
                    }
                },
                _ => Err("Mismatched payload for ScrollDownInPaneId"),
            },
            Some(CommandName::ScrollToTopInPaneId) => match protobuf_plugin_command.payload {
                Some(Payload::ScrollToTopInPaneIdPayload(scroll_to_top_in_pane_id_payload)) => {
                    match scroll_to_top_in_pane_id_payload.pane_id {
                        Some(pane_id) => {
                            Ok(PluginCommand::ScrollToTopInPaneId(pane_id.try_into()?))
                        },
                        _ => Err("Malformed scroll_to_top_in_pane_id_payload payload"),
                    }
                },
                _ => Err("Mismatched payload for ScrollToTopInPaneId"),
            },
            Some(CommandName::ScrollToBottomInPaneId) => match protobuf_plugin_command.payload {
                Some(Payload::ScrollToBottomInPaneIdPayload(
                    scroll_to_bottom_in_pane_id_payload,
                )) => match scroll_to_bottom_in_pane_id_payload.pane_id {
                    Some(pane_id) => Ok(PluginCommand::ScrollToBottomInPaneId(pane_id.try_into()?)),
                    _ => Err("Malformed scroll_to_bottom_in_pane_id_payload payload"),
                },
                _ => Err("Mismatched payload for ScrollToBottomInPaneId"),
            },
            Some(CommandName::PageScrollUpInPaneId) => match protobuf_plugin_command.payload {
                Some(Payload::PageScrollUpInPaneIdPayload(page_scroll_up_in_pane_id_payload)) => {
                    match page_scroll_up_in_pane_id_payload.pane_id {
                        Some(pane_id) => {
                            Ok(PluginCommand::PageScrollUpInPaneId(pane_id.try_into()?))
                        },
                        _ => Err("Malformed page_scroll_up_in_pane_id_payload payload"),
                    }
                },
                _ => Err("Mismatched payload for PageScrollUpInPaneId"),
            },
            Some(CommandName::PageScrollDownInPaneId) => match protobuf_plugin_command.payload {
                Some(Payload::PageScrollDownInPaneIdPayload(
                    page_scroll_down_in_pane_id_payload,
                )) => match page_scroll_down_in_pane_id_payload.pane_id {
                    Some(pane_id) => Ok(PluginCommand::PageScrollDownInPaneId(pane_id.try_into()?)),
                    _ => Err("Malformed page_scroll_down_in_pane_id_payload payload"),
                },
                _ => Err("Mismatched payload for PageScrollDownInPaneId"),
            },
            Some(CommandName::TogglePaneIdFullscreen) => match protobuf_plugin_command.payload {
                Some(Payload::TogglePaneIdFullscreenPayload(toggle_pane_id_fullscreen_payload)) => {
                    match toggle_pane_id_fullscreen_payload.pane_id {
                        Some(pane_id) => {
                            Ok(PluginCommand::TogglePaneIdFullscreen(pane_id.try_into()?))
                        },
                        _ => Err("Malformed toggle_pane_id_fullscreen_payload payload"),
                    }
                },
                _ => Err("Mismatched payload for TogglePaneIdFullscreen"),
            },
            Some(CommandName::TogglePaneEmbedOrEjectForPaneId) => {
                match protobuf_plugin_command.payload {
                    Some(Payload::TogglePaneEmbedOrEjectForPaneIdPayload(
                        toggle_pane_embed_or_eject_payload,
                    )) => match toggle_pane_embed_or_eject_payload.pane_id {
                        Some(pane_id) => Ok(PluginCommand::TogglePaneEmbedOrEjectForPaneId(
                            pane_id.try_into()?,
                        )),
                        _ => Err("Malformed toggle_pane_embed_or_eject_payload payload"),
                    },
                    _ => Err("Mismatched payload for TogglePaneEmbedOrEjectForPaneId"),
                }
            },
            Some(CommandName::CloseTabWithIndex) => match protobuf_plugin_command.payload {
                Some(Payload::CloseTabWithIndexPayload(close_tab_index_payload)) => Ok(
                    PluginCommand::CloseTabWithIndex(close_tab_index_payload.tab_index as usize),
                ),
                _ => Err("Mismatched payload for CloseTabWithIndex"),
            },
            Some(CommandName::BreakPanesToNewTab) => match protobuf_plugin_command.payload {
                Some(Payload::BreakPanesToNewTabPayload(break_panes_to_new_tab_payload)) => {
                    Ok(PluginCommand::BreakPanesToNewTab(
                        break_panes_to_new_tab_payload
                            .pane_ids
                            .into_iter()
                            .filter_map(|p_id| p_id.try_into().ok())
                            .collect(),
                        break_panes_to_new_tab_payload.new_tab_name,
                        break_panes_to_new_tab_payload.should_change_focus_to_new_tab,
                    ))
                },
                _ => Err("Mismatched payload for BreakPanesToNewTab"),
            },
            Some(CommandName::BreakPanesToTabWithIndex) => match protobuf_plugin_command.payload {
                Some(Payload::BreakPanesToTabWithIndexPayload(
                    break_panes_to_tab_with_index_payload,
                )) => Ok(PluginCommand::BreakPanesToTabWithIndex(
                    break_panes_to_tab_with_index_payload
                        .pane_ids
                        .into_iter()
                        .filter_map(|p_id| p_id.try_into().ok())
                        .collect(),
                    break_panes_to_tab_with_index_payload.tab_index as usize,
                    break_panes_to_tab_with_index_payload.should_change_focus_to_target_tab,
                )),
                _ => Err("Mismatched payload for BreakPanesToTabWithIndex"),
            },
            Some(CommandName::SwitchTabToId) => match protobuf_plugin_command.payload {
                Some(Payload::SwitchTabToIdPayload(payload)) => {
                    Ok(PluginCommand::SwitchTabToId(payload.tab_id))
                },
                _ => Err("Mismatched payload for SwitchTabToId"),
            },
            Some(CommandName::GoToTabWithId) => match protobuf_plugin_command.payload {
                Some(Payload::GoToTabWithIdPayload(payload)) => {
                    Ok(PluginCommand::GoToTabWithId(payload.tab_id))
                },
                _ => Err("Mismatched payload for GoToTabWithId"),
            },
            Some(CommandName::CloseTabWithId) => match protobuf_plugin_command.payload {
                Some(Payload::CloseTabWithIdPayload(payload)) => {
                    Ok(PluginCommand::CloseTabWithId(payload.tab_id))
                },
                _ => Err("Mismatched payload for CloseTabWithId"),
            },
            Some(CommandName::RenameTabWithId) => match protobuf_plugin_command.payload {
                Some(Payload::RenameTabWithIdPayload(payload)) => Ok(
                    PluginCommand::RenameTabWithId(payload.tab_id, payload.new_name),
                ),
                _ => Err("Mismatched payload for RenameTabWithId"),
            },
            Some(CommandName::SetTabProjects) => match protobuf_plugin_command.payload {
                Some(Payload::SetTabProjectsPayload(payload)) => Ok(PluginCommand::SetTabProjects(
                    payload
                        .tab_projects
                        .into_iter()
                        .map(|tab_project| (tab_project.tab_id, tab_project.project))
                        .collect(),
                )),
                _ => Err("Mismatched payload for SetTabProjects"),
            },
            Some(CommandName::BreakPanesToTabWithId) => match protobuf_plugin_command.payload {
                Some(Payload::BreakPanesToTabWithIdPayload(payload)) => {
                    Ok(PluginCommand::BreakPanesToTabWithId(
                        payload
                            .pane_ids
                            .into_iter()
                            .filter_map(|p_id| p_id.try_into().ok())
                            .collect(),
                        payload.tab_id,
                        payload.should_change_focus_to_target_tab,
                    ))
                },
                _ => Err("Mismatched payload for BreakPanesToTabWithId"),
            },
            Some(CommandName::ReloadPlugin) => match protobuf_plugin_command.payload {
                Some(Payload::ReloadPluginPayload(reload_plugin_payload)) => {
                    Ok(PluginCommand::ReloadPlugin(reload_plugin_payload.plugin_id))
                },
                _ => Err("Mismatched payload for ReloadPlugin"),
            },
            Some(CommandName::LoadNewPlugin) => match protobuf_plugin_command.payload {
                Some(Payload::LoadNewPluginPayload(load_new_plugin_payload)) => {
                    Ok(PluginCommand::LoadNewPlugin {
                        url: load_new_plugin_payload.plugin_url,
                        config: load_new_plugin_payload
                            .plugin_config
                            .into_iter()
                            .map(|e| (e.name, e.value))
                            .collect(),
                        load_in_background: load_new_plugin_payload
                            .should_load_plugin_in_background,
                        skip_plugin_cache: load_new_plugin_payload.should_skip_plugin_cache,
                    })
                },
                _ => Err("Mismatched payload for LoadNewPlugin"),
            },
            Some(CommandName::RebindKeys) => match protobuf_plugin_command.payload {
                Some(Payload::RebindKeysPayload(rebind_keys_payload)) => {
                    Ok(PluginCommand::RebindKeys {
                        keys_to_rebind: rebind_keys_payload
                            .keys_to_rebind
                            .into_iter()
                            .filter_map(key_to_rebind_to_plugin_command_assets)
                            .collect(),
                        keys_to_unbind: rebind_keys_payload
                            .keys_to_unbind
                            .into_iter()
                            .filter_map(key_to_unbind_to_plugin_command_assets)
                            .collect(),
                        write_config_to_disk: rebind_keys_payload.write_config_to_disk,
                    })
                },
                _ => Err("Mismatched payload for RebindKeys"),
            },
            Some(CommandName::ListClients) => match protobuf_plugin_command.payload {
                Some(_) => Err("ListClients should have no payload, found a payload"),
                None => Ok(PluginCommand::ListClients),
            },
            Some(CommandName::ChangeHostFolder) => match protobuf_plugin_command.payload {
                Some(Payload::ChangeHostFolderPayload(change_host_folder_payload)) => {
                    Ok(PluginCommand::ChangeHostFolder(PathBuf::from(
                        change_host_folder_payload.new_host_folder,
                    )))
                },
                _ => Err("Mismatched payload for ChangeHostFolder"),
            },
            Some(CommandName::SetFloatingPanePinned) => match protobuf_plugin_command.payload {
                Some(Payload::SetFloatingPanePinnedPayload(set_floating_pane_pinned_payload)) => {
                    match set_floating_pane_pinned_payload
                        .pane_id
                        .and_then(|p| p.try_into().ok())
                    {
                        Some(pane_id) => Ok(PluginCommand::SetFloatingPanePinned(
                            pane_id,
                            set_floating_pane_pinned_payload.should_be_pinned,
                        )),
                        None => Err("PaneId not found!"),
                    }
                },
                _ => Err("Mismatched payload for SetFloatingPanePinned"),
            },
            Some(CommandName::StackPanes) => match protobuf_plugin_command.payload {
                Some(Payload::StackPanesPayload(stack_panes_payload)) => {
                    Ok(PluginCommand::StackPanes(
                        stack_panes_payload
                            .pane_ids
                            .into_iter()
                            .filter_map(|p_id| p_id.try_into().ok())
                            .collect(),
                    ))
                },
                _ => Err("Mismatched payload for StackPanes"),
            },
            Some(CommandName::ChangeFloatingPanesCoordinates) => {
                match protobuf_plugin_command.payload {
                    Some(Payload::ChangeFloatingPanesCoordinatesPayload(
                        change_floating_panes_coordinates_payload,
                    )) => Ok(PluginCommand::ChangeFloatingPanesCoordinates(
                        change_floating_panes_coordinates_payload
                            .pane_ids_and_floating_panes_coordinates
                            .into_iter()
                            .filter_map(|p_id_a_fp| {
                                let pane_id: PaneId = p_id_a_fp.pane_id?.try_into().ok()?;
                                let floating_pane_coordinates: FloatingPaneCoordinates =
                                    p_id_a_fp.floating_pane_coordinates?.try_into().ok()?;
                                Some((pane_id, floating_pane_coordinates))
                            })
                            .collect(),
                    )),
                    _ => Err("Mismatched payload for ChangeFloatingPanesCoordinates"),
                }
            },
            Some(CommandName::ChangeOverlayCoordinates) => match protobuf_plugin_command.payload {
                Some(Payload::ChangeOverlayCoordinatesPayload(
                    change_overlay_coordinates_payload,
                )) => Ok(PluginCommand::ChangeOverlayCoordinates(
                    change_overlay_coordinates_payload
                        .pane_ids_and_floating_panes_coordinates
                        .into_iter()
                        .filter_map(|p_id_a_fp| {
                            let pane_id: PaneId = p_id_a_fp.pane_id?.try_into().ok()?;
                            let coordinates: FloatingPaneCoordinates =
                                p_id_a_fp.floating_pane_coordinates?.try_into().ok()?;
                            Some((pane_id, coordinates))
                        })
                        .collect(),
                )),
                _ => Err("Mismatched payload for ChangeOverlayCoordinates"),
            },
            Some(CommandName::TogglePaneBorderless) => match protobuf_plugin_command.payload {
                Some(Payload::TogglePaneBorderlessPayload(toggle_payload)) => {
                    match toggle_payload.pane_id {
                        Some(pane_id) => {
                            Ok(PluginCommand::TogglePaneBorderless(pane_id.try_into()?))
                        },
                        None => Err("Malformed TogglePaneBorderless payload"),
                    }
                },
                _ => Err("Mismatched payload for TogglePaneBorderless"),
            },
            Some(CommandName::SetPaneBorderless) => match protobuf_plugin_command.payload {
                Some(Payload::SetPaneBorderlessPayload(payload)) => {
                    match (payload.pane_id, payload.borderless) {
                        (Some(pane_id), borderless) => Ok(PluginCommand::SetPaneBorderless(
                            pane_id.try_into()?,
                            borderless,
                        )),
                        _ => Err("Malformed SetPaneBorderless payload"),
                    }
                },
                _ => Err("Mismatched payload for SetPaneBorderless"),
            },
            Some(CommandName::OpenCommandPaneNearPlugin) => match protobuf_plugin_command.payload {
                Some(Payload::OpenCommandPaneNearPluginPayload(command_to_run_payload)) => {
                    match command_to_run_payload.command_to_run {
                        Some(command_to_run) => {
                            let context: BTreeMap<String, String> = command_to_run_payload
                                .context
                                .into_iter()
                                .map(|e| (e.name, e.value))
                                .collect();
                            Ok(PluginCommand::OpenCommandPaneNearPlugin(
                                command_to_run.try_into()?,
                                context,
                            ))
                        },
                        None => Err("Malformed open command pane near plugin payload"),
                    }
                },
                _ => Err("Mismatched payload for OpenCommandPaneNearPlugin"),
            },
            Some(CommandName::OpenTerminalNearPlugin) => match protobuf_plugin_command.payload {
                Some(Payload::OpenTerminalNearPluginPayload(open_terminal_near_plugin_payload)) => {
                    match open_terminal_near_plugin_payload.file_to_open {
                        Some(file_to_open) => Ok(PluginCommand::OpenTerminalNearPlugin(
                            file_to_open.try_into()?,
                        )),
                        None => Err("Malformed open terminal near plugin payload"),
                    }
                },
                _ => Err("Mismatched payload for OpenTerminalNearPluginPayload"),
            },
            Some(CommandName::OpenTerminalFloatingNearPlugin) => match protobuf_plugin_command
                .payload
            {
                Some(Payload::OpenTerminalFloatingNearPluginPayload(
                    open_terminal_floating_near_plugin_payload,
                )) => {
                    let floating_pane_coordinates = open_terminal_floating_near_plugin_payload
                        .floating_pane_coordinates
                        .map(|f| f.into());
                    match open_terminal_floating_near_plugin_payload.file_to_open {
                        Some(file_to_open) => Ok(PluginCommand::OpenTerminalFloatingNearPlugin(
                            file_to_open.try_into()?,
                            floating_pane_coordinates,
                        )),
                        None => Err("Malformed open terminal floating near plugin payload"),
                    }
                },
                _ => Err("Mismatched payload for OpenTerminalFloatingNearPlugin"),
            },
            Some(CommandName::OpenTerminalInPlaceOfPlugin) => match protobuf_plugin_command.payload
            {
                Some(Payload::OpenTerminalInPlaceOfPluginPayload(
                    open_terminal_in_place_of_plugin_payload,
                )) => match open_terminal_in_place_of_plugin_payload.file_to_open {
                    Some(file_to_open) => Ok(PluginCommand::OpenTerminalInPlaceOfPlugin(
                        file_to_open.try_into()?,
                        open_terminal_in_place_of_plugin_payload.close_plugin_after_replace,
                    )),
                    None => Err("Malformed open terminal in place of plugin payload"),
                },
                _ => Err("Mismatched payload for OpenTerminalInPlaceOfPlugin"),
            },
            Some(CommandName::OpenCommandPaneFloatingNearPlugin) => {
                match protobuf_plugin_command.payload {
                    Some(Payload::OpenCommandPaneFloatingNearPluginPayload(
                        open_command_pane_floating_near_plugin,
                    )) => match open_command_pane_floating_near_plugin.command_to_run {
                        Some(command_to_run) => {
                            let context: BTreeMap<String, String> =
                                open_command_pane_floating_near_plugin
                                    .context
                                    .into_iter()
                                    .map(|e| (e.name, e.value))
                                    .collect();
                            let floating_pane_coordinates = open_command_pane_floating_near_plugin
                                .floating_pane_coordinates
                                .map(|f| f.into());
                            Ok(PluginCommand::OpenCommandPaneFloatingNearPlugin(
                                command_to_run.try_into()?,
                                floating_pane_coordinates,
                                context,
                            ))
                        },
                        None => Err("Malformed open command pane floating near plugin payload"),
                    },
                    _ => Err("Mismatched payload for OpenCommandPaneFloatingNearPlugin"),
                }
            },
            Some(CommandName::OpenCommandPaneInPlaceOfPlugin) => {
                match protobuf_plugin_command.payload {
                    Some(Payload::OpenCommandPaneInPlaceOfPluginPayload(
                        open_command_pane_in_place_of_plugin_payload,
                    )) => match open_command_pane_in_place_of_plugin_payload.command_to_run {
                        Some(command_to_run) => {
                            let context: BTreeMap<String, String> =
                                open_command_pane_in_place_of_plugin_payload
                                    .context
                                    .into_iter()
                                    .map(|e| (e.name, e.value))
                                    .collect();
                            Ok(PluginCommand::OpenCommandPaneInPlaceOfPlugin(
                                command_to_run.try_into()?,
                                open_command_pane_in_place_of_plugin_payload
                                    .close_plugin_after_replace,
                                context,
                            ))
                        },
                        None => Err("Malformed open command pane in place of plugin payload"),
                    },
                    _ => Err("Mismatched payload for OpenCommandPaneInPlaceOfPlugin"),
                }
            },
            Some(CommandName::OpenFileNearPlugin) => match protobuf_plugin_command.payload {
                Some(Payload::OpenFileNearPluginPayload(file_to_open_payload)) => {
                    match file_to_open_payload.file_to_open {
                        Some(file_to_open) => {
                            let context: BTreeMap<String, String> = file_to_open_payload
                                .context
                                .into_iter()
                                .map(|e| (e.name, e.value))
                                .collect();
                            Ok(PluginCommand::OpenFileNearPlugin(
                                file_to_open.try_into()?,
                                context,
                            ))
                        },
                        None => Err("Malformed open file payload"),
                    }
                },
                _ => Err("Mismatched payload for OpenFileNearPlugin"),
            },
            Some(CommandName::OpenFileFloatingNearPlugin) => {
                match protobuf_plugin_command.payload {
                    Some(Payload::OpenFileFloatingNearPluginPayload(file_to_open_payload)) => {
                        let floating_pane_coordinates = file_to_open_payload
                            .floating_pane_coordinates
                            .map(|f| f.into());
                        let context: BTreeMap<String, String> = file_to_open_payload
                            .context
                            .into_iter()
                            .map(|e| (e.name, e.value))
                            .collect();
                        match file_to_open_payload.file_to_open {
                            Some(file_to_open) => Ok(PluginCommand::OpenFileFloatingNearPlugin(
                                file_to_open.try_into()?,
                                floating_pane_coordinates,
                                context,
                            )),
                            None => Err("Malformed open file payload"),
                        }
                    },
                    _ => Err("Mismatched payload for OpenFileFloatingNearPlugin"),
                }
            },
            Some(CommandName::OpenFileInPlaceOfPlugin) => match protobuf_plugin_command.payload {
                Some(Payload::OpenFileInPlaceOfPluginPayload(file_to_open_payload)) => {
                    match file_to_open_payload.file_to_open {
                        Some(file_to_open) => {
                            let context: BTreeMap<String, String> = file_to_open_payload
                                .context
                                .into_iter()
                                .map(|e| (e.name, e.value))
                                .collect();
                            Ok(PluginCommand::OpenFileInPlaceOfPlugin(
                                file_to_open.try_into()?,
                                file_to_open_payload.close_plugin_after_replace,
                                context,
                            ))
                        },
                        None => Err("Malformed open file in place payload"),
                    }
                },
                _ => Err("Mismatched payload for OpenFileInPlaceOfPlugin"),
            },
            Some(CommandName::StartWebServer) => {
                if protobuf_plugin_command.payload.is_some() {
                    Err("StartWebServer should not have a payload")
                } else {
                    Ok(PluginCommand::StartWebServer)
                }
            },
            Some(CommandName::StopWebServer) => {
                if protobuf_plugin_command.payload.is_some() {
                    Err("StopWebServer should not have a payload")
                } else {
                    Ok(PluginCommand::StopWebServer)
                }
            },
            Some(CommandName::QueryWebServerStatus) => {
                if protobuf_plugin_command.payload.is_some() {
                    Err("QueryWebServerStatus should not have a payload")
                } else {
                    Ok(PluginCommand::QueryWebServerStatus)
                }
            },
            Some(CommandName::GroupAndUngroupPanes) => match protobuf_plugin_command.payload {
                Some(Payload::GroupAndUngroupPanesPayload(group_and_ungroup_panes_payload)) => {
                    Ok(PluginCommand::GroupAndUngroupPanes(
                        group_and_ungroup_panes_payload
                            .pane_ids_to_group
                            .into_iter()
                            .filter_map(|p| p.try_into().ok())
                            .collect(),
                        group_and_ungroup_panes_payload
                            .pane_ids_to_ungroup
                            .into_iter()
                            .filter_map(|p| p.try_into().ok())
                            .collect(),
                        group_and_ungroup_panes_payload.for_all_clients,
                    ))
                },
                _ => Err("Mismatched payload for GroupAndUngroupPanes"),
            },
            Some(CommandName::HighlightAndUnhighlightPanes) => {
                match protobuf_plugin_command.payload {
                    Some(Payload::HighlightAndUnhighlightPanesPayload(
                        highlight_and_unhighlight_panes_payload,
                    )) => Ok(PluginCommand::HighlightAndUnhighlightPanes(
                        highlight_and_unhighlight_panes_payload
                            .pane_ids_to_highlight
                            .into_iter()
                            .filter_map(|p| p.try_into().ok())
                            .collect(),
                        highlight_and_unhighlight_panes_payload
                            .pane_ids_to_unhighlight
                            .into_iter()
                            .filter_map(|p| p.try_into().ok())
                            .collect(),
                    )),
                    _ => Err("Mismatched payload for HighlightAndUnhighlightPanes"),
                }
            },
            Some(CommandName::SetPaneTitlePrefix) => match protobuf_plugin_command.payload {
                Some(Payload::SetPaneTitlePrefixPayload(payload)) => {
                    Ok(PluginCommand::SetPaneTitlePrefix {
                        pane_id: payload
                            .pane_id
                            .ok_or("SetPaneTitlePrefix must have a pane id")?
                            .try_into()?,
                        prefix: payload.prefix,
                    })
                },
                _ => Err("Mismatched payload for SetPaneTitlePrefix"),
            },
            Some(CommandName::RevealPaneWithId) => match protobuf_plugin_command.payload {
                Some(Payload::RevealPaneWithIdPayload(reveal_pane_with_id_payload)) => {
                    let pane_id = reveal_pane_with_id_payload
                        .pane_id
                        .ok_or("RevealPaneWithId must have a pane id")?
                        .try_into()?;
                    Ok(PluginCommand::RevealPaneWithId(
                        pane_id,
                        reveal_pane_with_id_payload.label,
                    ))
                },
                _ => Err("Mismatched payload for RevealPaneWithId"),
            },
            Some(CommandName::CloseMultiplePanes) => match protobuf_plugin_command.payload {
                Some(Payload::CloseMultiplePanesPayload(close_multiple_panes_payload)) => {
                    Ok(PluginCommand::CloseMultiplePanes(
                        close_multiple_panes_payload
                            .pane_ids
                            .into_iter()
                            .filter_map(|p| p.try_into().ok())
                            .collect(),
                    ))
                },
                _ => Err("Mismatched payload for CloseMultiplePanes"),
            },
            Some(CommandName::FloatMultiplePanes) => match protobuf_plugin_command.payload {
                Some(Payload::FloatMultiplePanesPayload(float_multiple_panes_payload)) => {
                    Ok(PluginCommand::FloatMultiplePanes(
                        float_multiple_panes_payload
                            .pane_ids
                            .into_iter()
                            .filter_map(|p| p.try_into().ok())
                            .collect(),
                    ))
                },
                _ => Err("Mismatched payload for FloatMultiplePanes"),
            },
            Some(CommandName::EmbedMultiplePanes) => match protobuf_plugin_command.payload {
                Some(Payload::EmbedMultiplePanesPayload(embed_multiple_panes_payload)) => {
                    Ok(PluginCommand::EmbedMultiplePanes(
                        embed_multiple_panes_payload
                            .pane_ids
                            .into_iter()
                            .filter_map(|p| p.try_into().ok())
                            .collect(),
                    ))
                },
                _ => Err("Mismatched payload for EmbedMultiplePanes"),
            },
            Some(CommandName::ShareCurrentSession) => {
                if protobuf_plugin_command.payload.is_some() {
                    Err("ShareCurrentSession should not have a payload")
                } else {
                    Ok(PluginCommand::ShareCurrentSession)
                }
            },
            Some(CommandName::StopSharingCurrentSession) => {
                if protobuf_plugin_command.payload.is_some() {
                    Err("StopSharingCurrentSession should not have a payload")
                } else {
                    Ok(PluginCommand::StopSharingCurrentSession)
                }
            },
            Some(CommandName::SetSelfMouseSelectionSupport) => {
                match protobuf_plugin_command.payload {
                    Some(Payload::SetSelfMouseSelectionSupportPayload(
                        set_self_mouse_selection_support_payload,
                    )) => Ok(PluginCommand::SetSelfMouseSelectionSupport(
                        set_self_mouse_selection_support_payload.support_mouse_selection,
                    )),
                    _ => Err("SetSelfMouseSelectionSupport requires a payload"),
                }
            },
            Some(CommandName::GenerateWebLoginToken) => match protobuf_plugin_command.payload {
                Some(Payload::GenerateWebLoginTokenPayload(generate_web_login_token_payload)) => {
                    Ok(PluginCommand::GenerateWebLoginToken(
                        generate_web_login_token_payload.token_label,
                        generate_web_login_token_payload.read_only.unwrap_or(false),
                    ))
                },
                _ => Err("GenerateWebLoginToken requires a payload"),
            },
            Some(CommandName::RevokeWebLoginToken) => match protobuf_plugin_command.payload {
                Some(Payload::RevokeWebLoginTokenPayload(revoke_web_login_token_payload)) => Ok(
                    PluginCommand::RevokeWebLoginToken(revoke_web_login_token_payload.token_label),
                ),
                _ => Err("RevokeWebLoginToken requires a payload"),
            },
            Some(CommandName::ListWebLoginTokens) => {
                if protobuf_plugin_command.payload.is_some() {
                    Err("ListWebLoginTokens should not have a payload")
                } else {
                    Ok(PluginCommand::ListWebLoginTokens)
                }
            },
            Some(CommandName::RevokeAllWebLoginTokens) => {
                if protobuf_plugin_command.payload.is_some() {
                    Err("RevokeAllWebLoginTokens should not have a payload")
                } else {
                    Ok(PluginCommand::RevokeAllWebLoginTokens)
                }
            },
            Some(CommandName::RenameWebLoginToken) => match protobuf_plugin_command.payload {
                Some(Payload::RenameWebLoginTokenPayload(rename_web_login_token_payload)) => {
                    Ok(PluginCommand::RenameWebLoginToken(
                        rename_web_login_token_payload.old_name,
                        rename_web_login_token_payload.new_name,
                    ))
                },
                _ => Err("RenameWebLoginToken requires a payload"),
            },
            Some(CommandName::InterceptKeyPresses) => match protobuf_plugin_command.payload {
                Some(_) => Err("InterceptKeyPresses should have no payload, found a payload"),
                None => Ok(PluginCommand::InterceptKeyPresses),
            },
            Some(CommandName::ClearKeyPressesIntercepts) => match protobuf_plugin_command.payload {
                Some(_) => Err("ClearKeyPressesIntercepts should have no payload, found a payload"),
                None => Ok(PluginCommand::ClearKeyPressesIntercepts),
            },
            Some(CommandName::ReplacePaneWithExistingPane) => match protobuf_plugin_command.payload
            {
                Some(Payload::ReplacePaneWithExistingPanePayload(
                    replace_pane_with_other_pane_payload,
                )) => Ok(PluginCommand::ReplacePaneWithExistingPane(
                    replace_pane_with_other_pane_payload
                        .pane_id_to_replace
                        .and_then(|p_id| PaneId::try_from(p_id).ok())
                        .ok_or("Failed to parse ReplacePaneWithExistingPanePayload")?,
                    replace_pane_with_other_pane_payload
                        .existing_pane_id
                        .and_then(|p_id| PaneId::try_from(p_id).ok())
                        .ok_or("Failed to parse ReplacePaneWithExistingPanePayload")?,
                    replace_pane_with_other_pane_payload.suppress_replaced_pane,
                )),
                _ => Err("Mismatched payload for ReplacePaneWithExistingPane"),
            },
            Some(CommandName::RunAction) => match protobuf_plugin_command.payload {
                Some(Payload::RunActionPayload(protobuf_payload)) => {
                    let action = Action::try_from(
                        protobuf_payload
                            .action
                            .ok_or("Missing action in RunAction payload")?,
                    )
                    .map_err(|_| "Failed to convert protobuf action")?;

                    let context: BTreeMap<String, String> = protobuf_payload
                        .context
                        .into_iter()
                        .map(|item| (item.name, item.value))
                        .collect();

                    Ok(PluginCommand::RunAction(action, context))
                },
                _ => Err("Mismatched payload for RunAction"),
            },
            Some(CommandName::CopyToClipboard) => match protobuf_plugin_command.payload {
                Some(Payload::CopyToClipboardPayload(payload)) => {
                    Ok(PluginCommand::CopyToClipboard(payload.text))
                },
                _ => Err("Mismatched payload for CopyToClipboard"),
            },
            Some(CommandName::GenerateRandomName) => Ok(PluginCommand::GenerateRandomName),
            Some(CommandName::DumpLayout) => match protobuf_plugin_command.payload {
                Some(Payload::DumpLayoutPayload(payload)) => {
                    Ok(PluginCommand::DumpLayout(payload.layout_name))
                },
                _ => Err("Mismatched payload for DumpLayout"),
            },
            Some(CommandName::ParseLayout) => match protobuf_plugin_command.payload {
                Some(Payload::ParseLayoutPayload(payload)) => {
                    Ok(PluginCommand::ParseLayout(payload.layout_string))
                },
                _ => Err("Mismatched payload for ParseLayout"),
            },
            Some(CommandName::GetLayoutDir) => Ok(PluginCommand::GetLayoutDir),
            Some(CommandName::GetUserPluginDir) => Ok(PluginCommand::GetUserPluginDir),
            Some(CommandName::GetFocusedPaneInfo) => Ok(PluginCommand::GetFocusedPaneInfo),
            Some(CommandName::SaveSession) => Ok(PluginCommand::SaveSession),
            Some(CommandName::CurrentSessionLastSavedTime) => {
                Ok(PluginCommand::CurrentSessionLastSavedTime)
            },
            Some(CommandName::GetPaneInfo) => match protobuf_plugin_command.payload {
                Some(Payload::GetPaneInfoPayload(get_pane_info_payload)) => {
                    let pane_id = get_pane_info_payload
                        .pane_id
                        .ok_or("Malformed pane_id for GetPaneInfo")
                        .and_then(|p| p.try_into())?;
                    Ok(PluginCommand::GetPaneInfo(pane_id))
                },
                _ => Err("Malformed payload for GetPaneInfo"),
            },
            Some(CommandName::GetTabInfo) => match protobuf_plugin_command.payload {
                Some(Payload::GetTabInfoPayload(get_tab_info_payload)) => {
                    let tab_id = get_tab_info_payload.tab_id as usize;
                    Ok(PluginCommand::GetTabInfo(tab_id))
                },
                _ => Err("Malformed payload for GetTabInfo"),
            },
            Some(CommandName::GetSessionEnvironmentVariables) => {
                match protobuf_plugin_command.payload {
                    Some(Payload::GetSessionEnvironmentVariablesPayload(_)) => {
                        Ok(PluginCommand::GetSessionEnvironmentVariables)
                    },
                    _ => Err("Mismatched payload for GetSessionEnvironmentVariables"),
                }
            },
            Some(CommandName::GetPluginAliasConfiguration) => {
                match protobuf_plugin_command.payload {
                    Some(Payload::GetPluginAliasConfigurationPayload(payload)) => {
                        Ok(PluginCommand::GetPluginAliasConfiguration(payload.alias))
                    },
                    _ => Err("Mismatched payload for GetPluginAliasConfiguration"),
                }
            },
            Some(CommandName::OpenCommandPaneInNewTab) => match protobuf_plugin_command.payload {
                Some(Payload::OpenCommandPaneInNewTabPayload(payload)) => {
                    match payload.command_to_run {
                        Some(command_to_run) => {
                            let context: BTreeMap<String, String> = payload
                                .context
                                .into_iter()
                                .map(|e| (e.name, e.value))
                                .collect();
                            Ok(PluginCommand::OpenCommandPaneInNewTab(
                                command_to_run.try_into()?,
                                context,
                            ))
                        },
                        None => Err("Malformed open_command_pane_in_new_tab payload"),
                    }
                },
                _ => Err("Mismatched payload for OpenCommandPaneInNewTab"),
            },
            Some(CommandName::OpenPluginPaneInNewTab) => match protobuf_plugin_command.payload {
                Some(Payload::OpenPluginPaneInNewTabPayload(payload)) => {
                    let configuration: BTreeMap<String, String> =
                        payload.configuration.into_iter().collect();
                    let context: BTreeMap<String, String> = payload
                        .context
                        .into_iter()
                        .map(|e| (e.name, e.value))
                        .collect();
                    Ok(PluginCommand::OpenPluginPaneInNewTab {
                        plugin_url: payload.plugin_url,
                        configuration,
                        context,
                    })
                },
                _ => Err("Mismatched payload for OpenPluginPaneInNewTab"),
            },
            Some(CommandName::OpenEditorPaneInNewTab) => match protobuf_plugin_command.payload {
                Some(Payload::OpenEditorPaneInNewTabPayload(payload)) => {
                    match payload.file_to_open {
                        Some(file) => {
                            let context: BTreeMap<String, String> = payload
                                .context
                                .into_iter()
                                .map(|e| (e.name, e.value))
                                .collect();
                            Ok(PluginCommand::OpenEditorPaneInNewTab(
                                file.try_into()?,
                                context,
                            ))
                        },
                        None => Err("Malformed open_editor_pane_in_new_tab payload"),
                    }
                },
                _ => Err("Mismatched payload for OpenEditorPaneInNewTab"),
            },
            Some(CommandName::OpenCommandPaneInPlaceOfPaneId) => {
                match protobuf_plugin_command.payload {
                    Some(Payload::OpenCommandPaneInPlaceOfPaneIdPayload(payload)) => {
                        match (payload.pane_id_to_replace, payload.command_to_run) {
                            (Some(pane_id), Some(command_to_run)) => {
                                let context: BTreeMap<String, String> = payload
                                    .context
                                    .into_iter()
                                    .map(|e| (e.name, e.value))
                                    .collect();
                                Ok(PluginCommand::OpenCommandPaneInPlaceOfPaneId(
                                    pane_id.try_into()?,
                                    command_to_run.try_into()?,
                                    payload.close_replaced_pane,
                                    context,
                                ))
                            },
                            _ => Err("Malformed open_command_pane_in_place_of_pane_id payload"),
                        }
                    },
                    _ => Err("Mismatched payload for OpenCommandPaneInPlaceOfPaneId"),
                }
            },
            Some(CommandName::OpenTerminalPaneInPlaceOfPaneId) => {
                match protobuf_plugin_command.payload {
                    Some(Payload::OpenTerminalPaneInPlaceOfPaneIdPayload(payload)) => {
                        match payload.pane_id_to_replace {
                            Some(pane_id) => {
                                let cwd = payload.cwd.map(|f| f.try_into()).transpose()?;
                                let cwd = cwd.unwrap_or_else(|| crate::data::FileToOpen {
                                    path: std::path::PathBuf::from("."),
                                    ..Default::default()
                                });
                                Ok(PluginCommand::OpenTerminalPaneInPlaceOfPaneId(
                                    pane_id.try_into()?,
                                    cwd,
                                    payload.close_replaced_pane,
                                ))
                            },
                            None => Err("Malformed open_terminal_pane_in_place_of_pane_id payload"),
                        }
                    },
                    _ => Err("Mismatched payload for OpenTerminalPaneInPlaceOfPaneId"),
                }
            },
            Some(CommandName::OpenEditPaneInPlaceOfPaneId) => {
                match protobuf_plugin_command.payload {
                    Some(Payload::OpenEditPaneInPlaceOfPaneIdPayload(payload)) => {
                        match (payload.pane_id_to_replace, payload.file_to_open) {
                            (Some(pane_id), Some(file_to_open)) => {
                                let context: BTreeMap<String, String> = payload
                                    .context
                                    .into_iter()
                                    .map(|e| (e.name, e.value))
                                    .collect();
                                Ok(PluginCommand::OpenEditPaneInPlaceOfPaneId(
                                    pane_id.try_into()?,
                                    file_to_open.try_into()?,
                                    payload.close_replaced_pane,
                                    context,
                                ))
                            },
                            _ => Err("Malformed open_edit_pane_in_place_of_pane_id payload"),
                        }
                    },
                    _ => Err("Mismatched payload for OpenEditPaneInPlaceOfPaneId"),
                }
            },
            Some(CommandName::HideFloatingPanes) => match protobuf_plugin_command.payload {
                Some(Payload::HideFloatingPanesPayload(payload)) => {
                    Ok(PluginCommand::HideFloatingPanes {
                        tab_id: payload.tab_id.map(|id| id as usize),
                    })
                },
                None => Ok(PluginCommand::HideFloatingPanes { tab_id: None }),
                _ => Err("Mismatched payload for HideFloatingPanes"),
            },
            Some(CommandName::ShowFloatingPanes) => match protobuf_plugin_command.payload {
                Some(Payload::ShowFloatingPanesPayload(payload)) => {
                    Ok(PluginCommand::ShowFloatingPanes {
                        tab_id: payload.tab_id.map(|id| id as usize),
                    })
                },
                None => Ok(PluginCommand::ShowFloatingPanes { tab_id: None }),
                _ => Err("Mismatched payload for ShowFloatingPanes"),
            },
            Some(CommandName::SetPaneColor) => match protobuf_plugin_command.payload {
                Some(Payload::SetPaneColorPayload(payload)) => match payload.pane_id {
                    Some(pane_id) => Ok(PluginCommand::SetPaneColor(
                        pane_id.try_into()?,
                        payload.fg,
                        payload.bg,
                    )),
                    None => Err("Malformed SetPaneColor payload"),
                },
                _ => Err("Mismatched payload for SetPaneColor"),
            },
            Some(CommandName::SetPaneRegexHighlights) => {
                match protobuf_plugin_command.payload {
                    Some(Payload::SetPaneRegexHighlightsPayload(p)) => {
                        let pane_id: PaneId = p
                            .pane_id
                            .ok_or("Missing pane_id in SetPaneRegexHighlights")?
                            .try_into()?;
                        let highlights = p
                            .highlights
                            .into_iter()
                            .map(|h| {
                                let style = match h.style.and_then(|s| s.style) {
                                    Some(ProtobufHighlightStyleVariant::Emphasis0(_)) => {
                                        HighlightStyle::Emphasis0
                                    },
                                    Some(ProtobufHighlightStyleVariant::Emphasis1(_)) => {
                                        HighlightStyle::Emphasis1
                                    },
                                    Some(ProtobufHighlightStyleVariant::Emphasis2(_)) => {
                                        HighlightStyle::Emphasis2
                                    },
                                    Some(ProtobufHighlightStyleVariant::Emphasis3(_)) => {
                                        HighlightStyle::Emphasis3
                                    },
                                    Some(ProtobufHighlightStyleVariant::CustomRgb(c)) => {
                                        HighlightStyle::CustomRgb {
                                            fg: match (c.fg_r, c.fg_g, c.fg_b) {
                                                (Some(r), Some(g), Some(b)) => {
                                                    Some((r as u8, g as u8, b as u8))
                                                },
                                                _ => None,
                                            },
                                            bg: match (c.bg_r, c.bg_g, c.bg_b) {
                                                (Some(r), Some(g), Some(b)) => {
                                                    Some((r as u8, g as u8, b as u8))
                                                },
                                                _ => None,
                                            },
                                        }
                                    },
                                    Some(ProtobufHighlightStyleVariant::CustomIndex(c)) => {
                                        HighlightStyle::CustomIndex {
                                            fg: c.fg.map(|v| v as u8),
                                            bg: c.bg.map(|v| v as u8),
                                        }
                                    },
                                    Some(ProtobufHighlightStyleVariant::BackgroundEmphasis0(_)) => {
                                        HighlightStyle::BackgroundEmphasis0
                                    },
                                    Some(ProtobufHighlightStyleVariant::BackgroundEmphasis1(_)) => {
                                        HighlightStyle::BackgroundEmphasis1
                                    },
                                    Some(ProtobufHighlightStyleVariant::BackgroundEmphasis2(_)) => {
                                        HighlightStyle::BackgroundEmphasis2
                                    },
                                    Some(ProtobufHighlightStyleVariant::BackgroundEmphasis3(_)) => {
                                        HighlightStyle::BackgroundEmphasis3
                                    },
                                    Some(ProtobufHighlightStyleVariant::None(_)) => {
                                        HighlightStyle::None
                                    },
                                    None => HighlightStyle::Emphasis0, // fallback
                                };
                                let context = h
                                    .context
                                    .into_iter()
                                    .map(|item| (item.name, item.value))
                                    .collect();
                                RegexHighlight {
                                    pattern: h.pattern,
                                    style,
                                    layer: match h.layer {
                                        x if x == ProtobufHighlightLayer::Tool as i32 => {
                                            HighlightLayer::Tool
                                        },
                                        x if x == ProtobufHighlightLayer::ActionFeedback as i32 => {
                                            HighlightLayer::ActionFeedback
                                        },
                                        _ => HighlightLayer::Hint, // 0 or unknown => Hint
                                    },
                                    context,
                                    on_hover: h.on_hover,
                                    bold: h.bold,
                                    italic: h.italic,
                                    underline: h.underline,
                                    tooltip_text: h.tooltip_text,
                                }
                            })
                            .collect();
                        Ok(PluginCommand::SetPaneRegexHighlights(pane_id, highlights))
                    },
                    _ => Err("Mismatched payload for SetPaneRegexHighlights"),
                }
            },
            Some(CommandName::ClearPaneHighlights) => match protobuf_plugin_command.payload {
                Some(Payload::ClearPaneHighlightsPayload(p)) => {
                    let pane_id: PaneId = p
                        .pane_id
                        .ok_or("Missing pane_id in ClearPaneHighlights")?
                        .try_into()?;
                    Ok(PluginCommand::ClearPaneHighlights(pane_id))
                },
                _ => Err("Mismatched payload for ClearPaneHighlights"),
            },
            Some(CommandName::OpenPluginPaneFloating) => match protobuf_plugin_command.payload {
                Some(Payload::OpenPluginPaneFloatingPayload(payload)) => {
                    let configuration: BTreeMap<String, String> =
                        payload.configuration.into_iter().collect();
                    let floating_pane_coordinates =
                        payload.floating_pane_coordinates.map(|f| f.into());
                    let context: BTreeMap<String, String> = payload
                        .context
                        .into_iter()
                        .map(|e| (e.name, e.value))
                        .collect();
                    Ok(PluginCommand::OpenPluginPaneFloating {
                        plugin_url: payload.plugin_url,
                        configuration,
                        floating_pane_coordinates,
                        context,
                    })
                },
                _ => Err("Mismatched payload for OpenPluginPaneFloating"),
            },
            Some(CommandName::WarpPaneToSession) => match protobuf_plugin_command.payload {
                Some(Payload::WarpPaneToSessionPayload(payload)) => {
                    Ok(PluginCommand::WarpPaneToSession {
                        pane_id: payload
                            .pane_id
                            .ok_or("Missing pane_id for WarpPaneToSession")?
                            .try_into()?,
                        to_session: payload.to_session,
                        placement: warp_placement_from_i32(payload.placement)?,
                    })
                },
                _ => Err("Mismatched payload for WarpPaneToSession"),
            },
            Some(CommandName::WarpPaneFromSession) => match protobuf_plugin_command.payload {
                Some(Payload::WarpPaneFromSessionPayload(payload)) => {
                    Ok(PluginCommand::WarpPaneFromSession {
                        pane_id: payload
                            .pane_id
                            .ok_or("Missing pane_id for WarpPaneFromSession")?
                            .try_into()?,
                        from_session: payload.from_session,
                        placement: warp_placement_from_i32(payload.placement)?,
                    })
                },
                _ => Err("Mismatched payload for WarpPaneFromSession"),
            },
            Some(CommandName::WarpTabToSession) => match protobuf_plugin_command.payload {
                Some(Payload::WarpTabToSessionPayload(payload)) => {
                    Ok(PluginCommand::WarpTabToSession {
                        tab_index: payload.tab_index as usize,
                        to_session: payload.to_session,
                    })
                },
                _ => Err("Mismatched payload for WarpTabToSession"),
            },
            Some(CommandName::WarpTabFromSession) => match protobuf_plugin_command.payload {
                Some(Payload::WarpTabFromSessionPayload(payload)) => {
                    Ok(PluginCommand::WarpTabFromSession {
                        tab_index: payload.tab_index as usize,
                        from_session: payload.from_session,
                    })
                },
                _ => Err("Mismatched payload for WarpTabFromSession"),
            },
            Some(CommandName::ListOrphanedPanes) => match protobuf_plugin_command.payload {
                Some(_) => Err("ListOrphanedPanes should have no payload"),
                None => Ok(PluginCommand::ListOrphanedPanes),
            },
            Some(CommandName::AdoptOrphanedPane) => match protobuf_plugin_command.payload {
                Some(Payload::AdoptOrphanedPanePayload(payload)) => {
                    Ok(PluginCommand::AdoptOrphanedPane {
                        host_id: payload.host_id,
                        placement: warp_placement_from_i32(payload.placement)?,
                    })
                },
                _ => Err("Mismatched payload for AdoptOrphanedPane"),
            },
            Some(CommandName::DataDefineEntity) => match protobuf_plugin_command.payload {
                Some(Payload::DataDefineEntityPayload(payload)) => {
                    Ok(PluginCommand::DataDefineEntity {
                        namespace: payload.namespace,
                        def: payload
                            .def
                            .ok_or("Missing def for DataDefineEntity")?
                            .try_into()?,
                        allow_destructive: payload.allow_destructive,
                    })
                },
                _ => Err("Mismatched payload for DataDefineEntity"),
            },
            Some(CommandName::DataApply) => match protobuf_plugin_command.payload {
                Some(Payload::DataApplyPayload(payload)) => Ok(PluginCommand::DataApply {
                    namespace: payload.namespace,
                    mutations: payload
                        .mutations
                        .into_iter()
                        .map(Mutation::try_from)
                        .collect::<Result<Vec<_>, _>>()?,
                }),
                _ => Err("Mismatched payload for DataApply"),
            },
            Some(CommandName::DataFetch) => match protobuf_plugin_command.payload {
                Some(Payload::DataFetchPayload(payload)) => Ok(PluginCommand::DataFetch {
                    request: payload
                        .request
                        .ok_or("Missing request for DataFetch")?
                        .try_into()?,
                }),
                _ => Err("Mismatched payload for DataFetch"),
            },
            Some(CommandName::DataCount) => match protobuf_plugin_command.payload {
                Some(Payload::DataCountPayload(payload)) => Ok(PluginCommand::DataCount {
                    request: payload
                        .request
                        .ok_or("Missing request for DataCount")?
                        .try_into()?,
                }),
                _ => Err("Mismatched payload for DataCount"),
            },
            Some(CommandName::DataWatch) => match protobuf_plugin_command.payload {
                Some(Payload::DataWatchPayload(payload)) => Ok(PluginCommand::DataWatch {
                    request: payload
                        .request
                        .ok_or("Missing request for DataWatch")?
                        .try_into()?,
                    mode: watch_mode_from_i32(payload.mode)?,
                }),
                _ => Err("Mismatched payload for DataWatch"),
            },
            Some(CommandName::DataUnwatch) => match protobuf_plugin_command.payload {
                Some(Payload::DataUnwatchPayload(payload)) => Ok(PluginCommand::DataUnwatch {
                    watch_id: payload.watch_id,
                }),
                _ => Err("Mismatched payload for DataUnwatch"),
            },
            Some(CommandName::EnqueueQueueJob) => match protobuf_plugin_command.payload {
                Some(Payload::EnqueueQueueJobPayload(payload)) => {
                    Ok(PluginCommand::EnqueueQueueJob {
                        queue: payload.queue,
                        plugin_url: payload.plugin_url,
                        config: payload
                            .config
                            .into_iter()
                            .map(|e| (e.name, e.value))
                            .collect(),
                        context: payload
                            .context
                            .into_iter()
                            .map(|e| (e.name, e.value))
                            .collect(),
                    })
                },
                _ => Err("Mismatched payload for EnqueueQueueJob"),
            },
            Some(CommandName::ConfigureQueue) => match protobuf_plugin_command.payload {
                Some(Payload::ConfigureQueuePayload(payload)) => {
                    Ok(PluginCommand::ConfigureQueue {
                        queue: payload.queue,
                        concurrency: payload.concurrency as usize,
                    })
                },
                _ => Err("Mismatched payload for ConfigureQueue"),
            },
            Some(CommandName::CancelQueueJob) => match protobuf_plugin_command.payload {
                Some(Payload::CancelQueueJobPayload(payload)) => {
                    Ok(PluginCommand::CancelQueueJob {
                        job_id: payload.job_id,
                    })
                },
                _ => Err("Mismatched payload for CancelQueueJob"),
            },
            Some(CommandName::QueueStatus) => match protobuf_plugin_command.payload {
                Some(Payload::QueueStatusPayload(payload)) => Ok(PluginCommand::QueueStatus {
                    queue: payload.queue,
                }),
                _ => Err("Mismatched payload for QueueStatus"),
            },
            Some(CommandName::FinishQueueJob) => match protobuf_plugin_command.payload {
                Some(Payload::FinishQueueJobPayload(payload)) => {
                    Ok(PluginCommand::FinishQueueJob {
                        success: payload.success,
                        payload: payload.payload,
                    })
                },
                _ => Err("Mismatched payload for FinishQueueJob"),
            },
            Some(CommandName::RegisterDefaultKeybindings) => {
                match protobuf_plugin_command.payload {
                    Some(Payload::RegisterDefaultKeybindingsPayload(keybindings)) => {
                        Ok(PluginCommand::RegisterDefaultKeybindings(keybindings))
                    },
                    _ => Err("Mismatched payload for RegisterDefaultKeybindings"),
                }
            },
            None => Err("Unrecognized plugin command"),
        }
    }
}

/// Unknown values are rejected rather than defaulted.
///
/// A placement this build does not know about came from a plugin built against a
/// newer API, and guessing "new tab" for it would put the pane somewhere the
/// plugin did not ask for — with no way for it to tell.
fn warp_placement_from_i32(placement: i32) -> Result<WarpPlacement, &'static str> {
    match ProtobufWarpPlacement::try_from(placement) {
        Ok(ProtobufWarpPlacement::NewTab) => Ok(WarpPlacement::NewTab),
        Ok(ProtobufWarpPlacement::FocusedTab) => Ok(WarpPlacement::FocusedTab),
        Ok(ProtobufWarpPlacement::Floating) => Ok(WarpPlacement::Floating),
        Err(_) => Err("Unrecognized warp placement"),
    }
}

impl From<WarpPlacement> for ProtobufWarpPlacement {
    fn from(placement: WarpPlacement) -> Self {
        match placement {
            WarpPlacement::NewTab => ProtobufWarpPlacement::NewTab,
            WarpPlacement::FocusedTab => ProtobufWarpPlacement::FocusedTab,
            WarpPlacement::Floating => ProtobufWarpPlacement::Floating,
        }
    }
}

impl TryFrom<PluginCommand> for ProtobufPluginCommand {
    type Error = &'static str;
    fn try_from(plugin_command: PluginCommand) -> Result<Self, &'static str> {
        match plugin_command {
            PluginCommand::Subscribe(subscriptions) => {
                let subscriptions: ProtobufEventNameList = subscriptions.try_into()?;
                Ok(ProtobufPluginCommand {
                    name: CommandName::Subscribe as i32,
                    payload: Some(Payload::SubscribePayload(SubscribePayload {
                        subscriptions: Some(subscriptions),
                    })),
                })
            },
            PluginCommand::Unsubscribe(subscriptions) => {
                let subscriptions: ProtobufEventNameList = subscriptions.try_into()?;
                Ok(ProtobufPluginCommand {
                    name: CommandName::Unsubscribe as i32,
                    payload: Some(Payload::UnsubscribePayload(UnsubscribePayload {
                        subscriptions: Some(subscriptions),
                    })),
                })
            },
            PluginCommand::SetSelectable(should_be_selectable) => Ok(ProtobufPluginCommand {
                name: CommandName::SetSelectable as i32,
                payload: Some(Payload::SetSelectablePayload(should_be_selectable)),
            }),
            PluginCommand::ShowCursor(cursor_position) => {
                let position = cursor_position.map(|(x, y)| CursorPosition {
                    x: x as u32,
                    y: y as u32,
                });
                Ok(ProtobufPluginCommand {
                    name: CommandName::ShowCursor as i32,
                    payload: Some(Payload::ShowCursorPayload(ShowCursorPayload { position })),
                })
            },
            PluginCommand::GetPluginIds => Ok(ProtobufPluginCommand {
                name: CommandName::GetPluginIds as i32,
                payload: None,
            }),
            PluginCommand::GetZellijVersion => Ok(ProtobufPluginCommand {
                name: CommandName::GetZellijVersion as i32,
                payload: None,
            }),
            PluginCommand::OpenFile(file_to_open, context) => Ok(ProtobufPluginCommand {
                name: CommandName::OpenFile as i32,
                payload: Some(Payload::OpenFilePayload(OpenFilePayload {
                    file_to_open: Some(file_to_open.try_into()?),
                    floating_pane_coordinates: None,
                    context: context
                        .into_iter()
                        .map(|(name, value)| ContextItem { name, value })
                        .collect(),
                })),
            }),
            PluginCommand::OpenFileFloating(file_to_open, floating_pane_coordinates, context) => {
                Ok(ProtobufPluginCommand {
                    name: CommandName::OpenFileFloating as i32,
                    payload: Some(Payload::OpenFileFloatingPayload(OpenFilePayload {
                        file_to_open: Some(file_to_open.try_into()?),
                        floating_pane_coordinates: floating_pane_coordinates.map(|f| f.into()),
                        context: context
                            .into_iter()
                            .map(|(name, value)| ContextItem { name, value })
                            .collect(),
                    })),
                })
            },
            PluginCommand::OpenTerminal(cwd) => Ok(ProtobufPluginCommand {
                name: CommandName::OpenTerminal as i32,
                payload: Some(Payload::OpenTerminalPayload(OpenFilePayload {
                    file_to_open: Some(cwd.try_into()?),
                    floating_pane_coordinates: None,
                    context: vec![], // will be added in the future
                })),
            }),
            PluginCommand::OpenTerminalFloating(cwd, floating_pane_coordinates) => {
                Ok(ProtobufPluginCommand {
                    name: CommandName::OpenTerminalFloating as i32,
                    payload: Some(Payload::OpenTerminalFloatingPayload(OpenFilePayload {
                        file_to_open: Some(cwd.try_into()?),
                        floating_pane_coordinates: floating_pane_coordinates.map(|f| f.into()),
                        context: vec![], // will be added in the future
                    })),
                })
            },
            PluginCommand::OpenCommandPane(command_to_run, context) => {
                let context: Vec<_> = context
                    .into_iter()
                    .map(|(name, value)| ContextItem { name, value })
                    .collect();
                Ok(ProtobufPluginCommand {
                    name: CommandName::OpenCommandPane as i32,
                    payload: Some(Payload::OpenCommandPanePayload(OpenCommandPanePayload {
                        command_to_run: Some(command_to_run.try_into()?),
                        floating_pane_coordinates: None,
                        context,
                    })),
                })
            },
            PluginCommand::OpenCommandPaneInGitCheckoutNearPlugin(
                command_to_run,
                source_directory,
                checkout,
            ) => {
                let checkout = match checkout {
                    GitCheckout::Current => {
                        open_command_pane_in_git_checkout_payload::Checkout::Current(
                            Default::default(),
                        )
                    },
                    GitCheckout::NewBranch { branch, base } => {
                        open_command_pane_in_git_checkout_payload::Checkout::NewBranch(
                            NewBranchGitCheckout { branch, base },
                        )
                    },
                    GitCheckout::NewWorktree { branch, base, path } => {
                        open_command_pane_in_git_checkout_payload::Checkout::NewWorktree(
                            NewWorktreeGitCheckout {
                                branch,
                                base,
                                path: path.display().to_string(),
                            },
                        )
                    },
                };
                Ok(ProtobufPluginCommand {
                    name: CommandName::OpenCommandPaneInGitCheckoutNearPlugin as i32,
                    payload: Some(Payload::OpenCommandPaneInGitCheckoutPayload(
                        OpenCommandPaneInGitCheckoutPayload {
                            command_to_run: Some(command_to_run.try_into()?),
                            source_directory: source_directory.display().to_string(),
                            checkout: Some(checkout),
                        },
                    )),
                })
            },
            PluginCommand::OpenCommandPaneFloating(
                command_to_run,
                floating_pane_coordinates,
                context,
            ) => {
                let context: Vec<_> = context
                    .into_iter()
                    .map(|(name, value)| ContextItem { name, value })
                    .collect();
                Ok(ProtobufPluginCommand {
                    name: CommandName::OpenCommandPaneFloating as i32,
                    payload: Some(Payload::OpenCommandPaneFloatingPayload(
                        OpenCommandPanePayload {
                            command_to_run: Some(command_to_run.try_into()?),
                            floating_pane_coordinates: floating_pane_coordinates.map(|f| f.into()),
                            context,
                        },
                    )),
                })
            },
            PluginCommand::OpenCommandPaneOverlay(command_to_run, placement, context) => {
                let context: Vec<_> = context
                    .into_iter()
                    .map(|(name, value)| ContextItem { name, value })
                    .collect();
                Ok(ProtobufPluginCommand {
                    name: CommandName::OpenCommandPaneOverlay as i32,
                    payload: Some(Payload::OpenCommandPaneOverlayPayload(
                        OpenCommandPaneOverlayPayload {
                            command_to_run: Some(command_to_run.try_into()?),
                            placement: Some(placement.try_into()?),
                            context,
                        },
                    )),
                })
            },
            PluginCommand::SwitchTabTo(tab_index) => Ok(ProtobufPluginCommand {
                name: CommandName::SwitchTabTo as i32,
                payload: Some(Payload::SwitchTabToPayload(SwitchTabToPayload {
                    tab_index,
                })),
            }),
            PluginCommand::SetTimeout(seconds) => Ok(ProtobufPluginCommand {
                name: CommandName::SetTimeout as i32,
                payload: Some(Payload::SetTimeoutPayload(SetTimeoutPayload { seconds })),
            }),
            PluginCommand::ExecCmd(command_line) => Ok(ProtobufPluginCommand {
                name: CommandName::ExecCmd as i32,
                payload: Some(Payload::ExecCmdPayload(ExecCmdPayload { command_line })),
            }),
            PluginCommand::PostMessageTo(plugin_message) => Ok(ProtobufPluginCommand {
                name: CommandName::PostMessageTo as i32,
                payload: Some(Payload::PostMessageToPayload(PluginMessagePayload {
                    message: Some(plugin_message.try_into()?),
                })),
            }),
            PluginCommand::PostMessageToPlugin(plugin_message) => Ok(ProtobufPluginCommand {
                name: CommandName::PostMessageToPlugin as i32,
                payload: Some(Payload::PostMessageToPluginPayload(PluginMessagePayload {
                    message: Some(plugin_message.try_into()?),
                })),
            }),
            PluginCommand::HideSelf => Ok(ProtobufPluginCommand {
                name: CommandName::HideSelf as i32,
                payload: None,
            }),
            PluginCommand::ShowSelf(should_float_if_hidden) => Ok(ProtobufPluginCommand {
                name: CommandName::ShowSelf as i32,
                payload: Some(Payload::ShowSelfPayload(should_float_if_hidden)),
            }),
            PluginCommand::SwitchToMode(input_mode) => Ok(ProtobufPluginCommand {
                name: CommandName::SwitchToMode as i32,
                payload: Some(Payload::SwitchToModePayload(SwitchToModePayload {
                    input_mode: ProtobufInputMode::try_from(input_mode)? as i32,
                })),
            }),
            PluginCommand::NewTabsWithLayout(raw_layout) => Ok(ProtobufPluginCommand {
                name: CommandName::NewTabsWithLayout as i32,
                payload: Some(Payload::NewTabsWithLayoutPayload(raw_layout)),
            }),
            PluginCommand::NewTab { name, cwd } => Ok(ProtobufPluginCommand {
                name: CommandName::NewTab as i32,
                payload: Some(Payload::NewTabPayload(NewTabPayload { name, cwd })),
            }),
            PluginCommand::NewTabUnfocused { name, cwd } => Ok(ProtobufPluginCommand {
                name: CommandName::NewTabUnfocused as i32,
                payload: Some(Payload::NewTabUnfocusedPayload(NewTabUnfocusedPayload {
                    name,
                    cwd,
                })),
            }),
            PluginCommand::NewTiledPaneInTab { tab_position } => Ok(ProtobufPluginCommand {
                name: CommandName::NewTiledPaneInTab as i32,
                payload: Some(Payload::NewTiledPaneInTabPayload(
                    NewTiledPaneInTabPayload {
                        tab_position: tab_position as u32,
                    },
                )),
            }),
            PluginCommand::ToggleFloatingPanes { tab_id } => Ok(ProtobufPluginCommand {
                name: CommandName::ToggleFloatingPanes as i32,
                payload: Some(Payload::ToggleFloatingPanesPayload(
                    ToggleFloatingPanesPayload { tab_id },
                )),
            }),
            PluginCommand::NewPane => Ok(ProtobufPluginCommand {
                name: CommandName::NewPane as i32,
                payload: None,
            }),
            PluginCommand::GoToNextTab => Ok(ProtobufPluginCommand {
                name: CommandName::GoToNextTab as i32,
                payload: None,
            }),
            PluginCommand::GoToPreviousTab => Ok(ProtobufPluginCommand {
                name: CommandName::GoToPreviousTab as i32,
                payload: None,
            }),
            PluginCommand::Resize(resize) => Ok(ProtobufPluginCommand {
                name: CommandName::Resize as i32,
                payload: Some(Payload::ResizePayload(ResizePayload {
                    resize: Some(resize.try_into()?),
                })),
            }),
            PluginCommand::ResizeWithDirection(resize) => Ok(ProtobufPluginCommand {
                name: CommandName::ResizeWithDirection as i32,
                payload: Some(Payload::ResizeWithDirectionPayload(ResizePayload {
                    resize: Some(resize.try_into()?),
                })),
            }),
            PluginCommand::FocusNextPane => Ok(ProtobufPluginCommand {
                name: CommandName::FocusNextPane as i32,
                payload: None,
            }),
            PluginCommand::FocusPreviousPane => Ok(ProtobufPluginCommand {
                name: CommandName::FocusPreviousPane as i32,
                payload: None,
            }),
            PluginCommand::FocusLastPane => Ok(ProtobufPluginCommand {
                name: CommandName::FocusLastPane as i32,
                payload: None,
            }),
            PluginCommand::MoveFocus(direction) => Ok(ProtobufPluginCommand {
                name: CommandName::MoveFocus as i32,
                payload: Some(Payload::MoveFocusPayload(MovePayload {
                    direction: Some(direction.try_into()?),
                })),
            }),
            PluginCommand::MoveFocusOrTab(direction) => Ok(ProtobufPluginCommand {
                name: CommandName::MoveFocusOrTab as i32,
                payload: Some(Payload::MoveFocusOrTabPayload(MovePayload {
                    direction: Some(direction.try_into()?),
                })),
            }),
            PluginCommand::Detach => Ok(ProtobufPluginCommand {
                name: CommandName::Detach as i32,
                payload: None,
            }),
            PluginCommand::EditScrollback => Ok(ProtobufPluginCommand {
                name: CommandName::EditScrollback as i32,
                payload: None,
            }),
            PluginCommand::Write(bytes) => Ok(ProtobufPluginCommand {
                name: CommandName::Write as i32,
                payload: Some(Payload::WritePayload(bytes)),
            }),
            PluginCommand::WriteChars(chars) => Ok(ProtobufPluginCommand {
                name: CommandName::WriteChars as i32,
                payload: Some(Payload::WriteCharsPayload(chars)),
            }),
            PluginCommand::ToggleTab => Ok(ProtobufPluginCommand {
                name: CommandName::ToggleTab as i32,
                payload: None,
            }),
            PluginCommand::MovePane => Ok(ProtobufPluginCommand {
                name: CommandName::MovePane as i32,
                payload: None,
            }),
            PluginCommand::MovePaneWithDirection(direction) => Ok(ProtobufPluginCommand {
                name: CommandName::MovePaneWithDirection as i32,
                payload: Some(Payload::MovePaneWithDirectionPayload(MovePayload {
                    direction: Some(direction.try_into()?),
                })),
            }),
            PluginCommand::ClearScreen => Ok(ProtobufPluginCommand {
                name: CommandName::ClearScreen as i32,
                payload: None,
            }),
            PluginCommand::ScrollUp => Ok(ProtobufPluginCommand {
                name: CommandName::ScrollUp as i32,
                payload: None,
            }),
            PluginCommand::ScrollDown => Ok(ProtobufPluginCommand {
                name: CommandName::ScrollDown as i32,
                payload: None,
            }),
            PluginCommand::ScrollToTop => Ok(ProtobufPluginCommand {
                name: CommandName::ScrollToTop as i32,
                payload: None,
            }),
            PluginCommand::ScrollToBottom => Ok(ProtobufPluginCommand {
                name: CommandName::ScrollToBottom as i32,
                payload: None,
            }),
            PluginCommand::PageScrollUp => Ok(ProtobufPluginCommand {
                name: CommandName::PageScrollUp as i32,
                payload: None,
            }),
            PluginCommand::PageScrollDown => Ok(ProtobufPluginCommand {
                name: CommandName::PageScrollDown as i32,
                payload: None,
            }),
            PluginCommand::ToggleFocusFullscreen => Ok(ProtobufPluginCommand {
                name: CommandName::ToggleFocusFullscreen as i32,
                payload: None,
            }),
            PluginCommand::ToggleFocusNoUiFullscreen => Ok(ProtobufPluginCommand {
                name: CommandName::ToggleFocusNoUiFullscreen as i32,
                payload: None,
            }),
            PluginCommand::FocusHostSession => Ok(ProtobufPluginCommand {
                name: CommandName::FocusHostSession as i32,
                payload: None,
            }),
            PluginCommand::ListRecordings => Ok(ProtobufPluginCommand {
                name: CommandName::ListRecordings as i32,
                payload: None,
            }),
            PluginCommand::ListNotifications => Ok(ProtobufPluginCommand {
                name: CommandName::ListNotifications as i32,
                payload: None,
            }),
            PluginCommand::SearchRecordings { query, limit } => Ok(ProtobufPluginCommand {
                name: CommandName::SearchRecordings as i32,
                payload: Some(Payload::SearchRecordingsPayload(
                    ProtobufSearchRecordingsPayload {
                        query,
                        limit: limit as u32,
                    },
                )),
            }),
            PluginCommand::TogglePaneFrames => Ok(ProtobufPluginCommand {
                name: CommandName::TogglePaneFrames as i32,
                payload: None,
            }),
            PluginCommand::SetPaneFrameStyle(pane_frame_style) => Ok(ProtobufPluginCommand {
                name: CommandName::SetPaneFrameStyle as i32,
                payload: Some(Payload::SetPaneFrameStylePayload(
                    ProtobufSetPaneFrameStylePayload {
                        pane_frame_style: ProtobufPaneFrameStyle::try_from(pane_frame_style)?
                            as i32,
                    },
                )),
            }),
            PluginCommand::TogglePaneEmbedOrEject => Ok(ProtobufPluginCommand {
                name: CommandName::TogglePaneEmbedOrEject as i32,
                payload: None,
            }),
            PluginCommand::UndoRenamePane => Ok(ProtobufPluginCommand {
                name: CommandName::UndoRenamePane as i32,
                payload: None,
            }),
            PluginCommand::CloseFocus => Ok(ProtobufPluginCommand {
                name: CommandName::CloseFocus as i32,
                payload: None,
            }),
            PluginCommand::ToggleActiveTabSync => Ok(ProtobufPluginCommand {
                name: CommandName::ToggleActiveTabSync as i32,
                payload: None,
            }),
            PluginCommand::CloseFocusedTab => Ok(ProtobufPluginCommand {
                name: CommandName::CloseFocusedTab as i32,
                payload: None,
            }),
            PluginCommand::UndoRenameTab => Ok(ProtobufPluginCommand {
                name: CommandName::UndoRenameTab as i32,
                payload: None,
            }),
            PluginCommand::QuitZellij => Ok(ProtobufPluginCommand {
                name: CommandName::QuitZellij as i32,
                payload: None,
            }),
            PluginCommand::PreviousSwapLayout => Ok(ProtobufPluginCommand {
                name: CommandName::PreviousSwapLayout as i32,
                payload: None,
            }),
            PluginCommand::NextSwapLayout => Ok(ProtobufPluginCommand {
                name: CommandName::NextSwapLayout as i32,
                payload: None,
            }),
            PluginCommand::GoToTabName(tab_name) => Ok(ProtobufPluginCommand {
                name: CommandName::GoToTabName as i32,
                payload: Some(Payload::GoToTabNamePayload(tab_name)),
            }),
            PluginCommand::FocusOrCreateTab(tab_name) => Ok(ProtobufPluginCommand {
                name: CommandName::FocusOrCreateTab as i32,
                payload: Some(Payload::FocusOrCreateTabPayload(tab_name)),
            }),
            PluginCommand::GoToTab(tab_index) => Ok(ProtobufPluginCommand {
                name: CommandName::GoToTab as i32,
                payload: Some(Payload::GoToTabPayload(tab_index)),
            }),
            PluginCommand::StartOrReloadPlugin(url) => Ok(ProtobufPluginCommand {
                name: CommandName::StartOrReloadPlugin as i32,
                payload: Some(Payload::StartOrReloadPluginPayload(url)),
            }),
            PluginCommand::CloseTerminalPane(pane_id) => Ok(ProtobufPluginCommand {
                name: CommandName::CloseTerminalPane as i32,
                payload: Some(Payload::CloseTerminalPanePayload(pane_id)),
            }),
            PluginCommand::ClosePluginPane(pane_id) => Ok(ProtobufPluginCommand {
                name: CommandName::ClosePluginPane as i32,
                payload: Some(Payload::ClosePluginPanePayload(pane_id)),
            }),
            PluginCommand::FocusTerminalPane(
                pane_id,
                should_float_if_hidden,
                should_be_in_place_if_hidden,
            ) => Ok(ProtobufPluginCommand {
                name: CommandName::FocusTerminalPane as i32,
                payload: Some(Payload::FocusTerminalPanePayload(PaneIdAndShouldFloat {
                    pane_id,
                    should_float: should_float_if_hidden,
                    should_be_in_place: should_be_in_place_if_hidden,
                })),
            }),
            PluginCommand::FocusPluginPane(
                pane_id,
                should_float_if_hidden,
                should_be_in_place_if_hidden,
            ) => Ok(ProtobufPluginCommand {
                name: CommandName::FocusPluginPane as i32,
                payload: Some(Payload::FocusPluginPanePayload(PaneIdAndShouldFloat {
                    pane_id,
                    should_float: should_float_if_hidden,
                    should_be_in_place: should_be_in_place_if_hidden,
                })),
            }),
            PluginCommand::RenameTerminalPane(pane_id, new_name) => Ok(ProtobufPluginCommand {
                name: CommandName::RenameTerminalPane as i32,
                payload: Some(Payload::RenameTerminalPanePayload(IdAndNewName {
                    id: pane_id,
                    new_name,
                })),
            }),
            PluginCommand::RenamePluginPane(pane_id, new_name) => Ok(ProtobufPluginCommand {
                name: CommandName::RenamePluginPane as i32,
                payload: Some(Payload::RenamePluginPanePayload(IdAndNewName {
                    id: pane_id,
                    new_name,
                })),
            }),
            PluginCommand::RenameTab(tab_index, new_name) => Ok(ProtobufPluginCommand {
                name: CommandName::RenameTab as i32,
                payload: Some(Payload::RenameTabPayload(IdAndNewName {
                    id: tab_index,
                    new_name,
                })),
            }),
            PluginCommand::ReportPanic(payload) => Ok(ProtobufPluginCommand {
                name: CommandName::ReportCrash as i32,
                payload: Some(Payload::ReportCrashPayload(payload)),
            }),
            PluginCommand::RequestPluginPermissions(permissions) => Ok(ProtobufPluginCommand {
                name: CommandName::RequestPluginPermissions as i32,
                payload: Some(Payload::RequestPluginPermissionPayload(
                    RequestPluginPermissionPayload {
                        permissions: permissions
                            .iter()
                            .filter_map(|p| ProtobufPermissionType::try_from(*p).ok())
                            .map(|p| p as i32)
                            .collect(),
                    },
                )),
            }),
            PluginCommand::SwitchSession(switch_to_session) => Ok(ProtobufPluginCommand {
                name: CommandName::SwitchSession as i32,
                payload: Some(Payload::SwitchSessionPayload(SwitchSessionPayload {
                    name: switch_to_session.name,
                    tab_position: switch_to_session.tab_position.map(|t| t as u32),
                    pane_id: switch_to_session.pane_id.map(|p| p.0),
                    pane_id_is_plugin: switch_to_session.pane_id.map(|p| p.1),
                    layout: switch_to_session.layout.and_then(|l| l.try_into().ok()),
                    cwd: switch_to_session.cwd.map(|c| c.display().to_string()),
                })),
            }),
            PluginCommand::OpenTerminalInPlace(cwd) => Ok(ProtobufPluginCommand {
                name: CommandName::OpenTerminalInPlace as i32,
                payload: Some(Payload::OpenTerminalInPlacePayload(OpenFilePayload {
                    file_to_open: Some(cwd.try_into()?),
                    floating_pane_coordinates: None,
                    context: vec![], // will be added in the future
                })),
            }),
            PluginCommand::OpenFileInPlace(file_to_open, context) => Ok(ProtobufPluginCommand {
                name: CommandName::OpenFileInPlace as i32,
                payload: Some(Payload::OpenFileInPlacePayload(OpenFilePayload {
                    file_to_open: Some(file_to_open.try_into()?),
                    floating_pane_coordinates: None,
                    context: context
                        .into_iter()
                        .map(|(name, value)| ContextItem { name, value })
                        .collect(),
                })),
            }),
            PluginCommand::OpenCommandPaneInPlace(command_to_run, context) => {
                let context: Vec<_> = context
                    .into_iter()
                    .map(|(name, value)| ContextItem { name, value })
                    .collect();
                Ok(ProtobufPluginCommand {
                    name: CommandName::OpenCommandInPlace as i32,
                    payload: Some(Payload::OpenCommandPaneInPlacePayload(
                        OpenCommandPanePayload {
                            command_to_run: Some(command_to_run.try_into()?),
                            floating_pane_coordinates: None,
                            context,
                        },
                    )),
                })
            },
            PluginCommand::RunCommand(
                command_line,
                env_variables,
                cwd,
                context,
                max_output_bytes,
            ) => {
                let env_variables: Vec<_> = env_variables
                    .into_iter()
                    .map(|(name, value)| EnvVariable { name, value })
                    .collect();
                let context: Vec<_> = context
                    .into_iter()
                    .map(|(name, value)| ContextItem { name, value })
                    .collect();
                let cwd = cwd.display().to_string();
                Ok(ProtobufPluginCommand {
                    name: CommandName::RunCommand as i32,
                    payload: Some(Payload::RunCommandPayload(RunCommandPayload {
                        command_line,
                        env_variables,
                        cwd,
                        context,
                        max_output_bytes: max_output_bytes.map(|limit| limit as u64),
                    })),
                })
            },
            PluginCommand::WebRequest(url, verb, headers, body, context, stream_id, proxy) => {
                let context: Vec<_> = context
                    .into_iter()
                    .map(|(name, value)| ContextItem { name, value })
                    .collect();
                let headers: Vec<_> = headers
                    .into_iter()
                    .map(|(name, value)| Header { name, value })
                    .collect();
                let verb: ProtobufHttpVerb = verb.into();
                Ok(ProtobufPluginCommand {
                    name: CommandName::WebRequest as i32,
                    payload: Some(Payload::WebRequestPayload(WebRequestPayload {
                        url,
                        verb: verb as i32,
                        body,
                        headers,
                        context,
                        stream_id,
                        proxy,
                    })),
                })
            },
            PluginCommand::CancelWebRequest(stream_id) => Ok(ProtobufPluginCommand {
                name: CommandName::CancelWebRequest as i32,
                payload: Some(Payload::CancelWebRequestPayload(CancelWebRequestPayload {
                    stream_id,
                })),
            }),
            PluginCommand::DeleteDeadSession(dead_session_name) => Ok(ProtobufPluginCommand {
                name: CommandName::DeleteDeadSession as i32,
                payload: Some(Payload::DeleteDeadSessionPayload(dead_session_name)),
            }),
            PluginCommand::DeleteAllDeadSessions => Ok(ProtobufPluginCommand {
                name: CommandName::DeleteAllDeadSessions as i32,
                payload: None,
            }),
            PluginCommand::RenameSession(new_session_name) => Ok(ProtobufPluginCommand {
                name: CommandName::RenameSession as i32,
                payload: Some(Payload::RenameSessionPayload(new_session_name)),
            }),
            PluginCommand::Notify(text, pane_id) => Ok(ProtobufPluginCommand {
                name: CommandName::Notify as i32,
                payload: Some(match pane_id {
                    Some(pane_id) => Payload::NotifyWithPanePayload(NotifyWithPanePayload {
                        text,
                        pane_id: Some(pane_id.try_into()?),
                    }),
                    None => Payload::NotifyPayload(text),
                }),
            }),
            PluginCommand::UnblockCliPipeInput(pipe_name) => Ok(ProtobufPluginCommand {
                name: CommandName::UnblockCliPipeInput as i32,
                payload: Some(Payload::UnblockCliPipeInputPayload(pipe_name)),
            }),
            PluginCommand::BlockCliPipeInput(pipe_name) => Ok(ProtobufPluginCommand {
                name: CommandName::BlockCliPipeInput as i32,
                payload: Some(Payload::BlockCliPipeInputPayload(pipe_name)),
            }),
            PluginCommand::CliPipeOutput(pipe_name, output) => Ok(ProtobufPluginCommand {
                name: CommandName::CliPipeOutput as i32,
                payload: Some(Payload::CliPipeOutputPayload(CliPipeOutputPayload {
                    pipe_name,
                    output,
                })),
            }),
            PluginCommand::MessageToPlugin(message_to_plugin) => {
                let plugin_config: Vec<_> = message_to_plugin
                    .plugin_config
                    .into_iter()
                    .map(|(name, value)| ContextItem { name, value })
                    .collect();
                let message_args: Vec<_> = message_to_plugin
                    .message_args
                    .into_iter()
                    .map(|(name, value)| ContextItem { name, value })
                    .collect();
                Ok(ProtobufPluginCommand {
                    name: CommandName::MessageToPlugin as i32,
                    payload: Some(Payload::MessageToPluginPayload(MessageToPluginPayload {
                        plugin_url: message_to_plugin.plugin_url,
                        plugin_config,
                        message_name: message_to_plugin.message_name,
                        message_payload: message_to_plugin.message_payload,
                        message_args,
                        new_plugin_args: message_to_plugin.new_plugin_args.map(|m_t_p| {
                            ProtobufNewPluginArgs {
                                should_float: m_t_p.should_float,
                                pane_id_to_replace: m_t_p
                                    .pane_id_to_replace
                                    .and_then(|p_id| ProtobufPaneId::try_from(p_id).ok()),
                                pane_title: m_t_p.pane_title,
                                cwd: m_t_p.cwd.map(|cwd| cwd.display().to_string()),
                                skip_cache: m_t_p.skip_cache,
                                should_focus: m_t_p.should_focus,
                            }
                        }),
                        destination_plugin_id: message_to_plugin.destination_plugin_id,
                        floating_pane_coordinates: message_to_plugin
                            .floating_pane_coordinates
                            .and_then(|f| f.try_into().ok()),
                    })),
                })
            },
            PluginCommand::DisconnectOtherClients => Ok(ProtobufPluginCommand {
                name: CommandName::DisconnectOtherClients as i32,
                payload: None,
            }),
            PluginCommand::KillSessions(session_names) => Ok(ProtobufPluginCommand {
                name: CommandName::KillSessions as i32,
                payload: Some(Payload::KillSessionsPayload(KillSessionsPayload {
                    session_names,
                })),
            }),
            PluginCommand::ScanHostFolder(folder_to_scan) => Ok(ProtobufPluginCommand {
                name: CommandName::ScanHostFolder as i32,
                payload: Some(Payload::ScanHostFolderPayload(
                    folder_to_scan.display().to_string(),
                )),
            }),
            PluginCommand::WatchFilesystem => Ok(ProtobufPluginCommand {
                name: CommandName::WatchFilesystem as i32,
                payload: None,
            }),
            PluginCommand::FsRead { path, max_bytes } => Ok(ProtobufPluginCommand {
                name: CommandName::FsRead as i32,
                payload: Some(Payload::FsReadPayload(FsReadPayload {
                    path: path.display().to_string(),
                    max_bytes: max_bytes.map(|b| b as u64),
                })),
            }),
            PluginCommand::FsList { path } => Ok(ProtobufPluginCommand {
                name: CommandName::FsList as i32,
                payload: Some(Payload::FsListPayload(FsListPayload {
                    path: path.display().to_string(),
                })),
            }),
            PluginCommand::FsMetadata { path } => Ok(ProtobufPluginCommand {
                name: CommandName::FsMetadata as i32,
                payload: Some(Payload::FsMetadataPayload(FsMetadataPayload {
                    path: path.display().to_string(),
                })),
            }),
            PluginCommand::FsWrite {
                path,
                contents,
                append,
                create_dirs,
            } => Ok(ProtobufPluginCommand {
                name: CommandName::FsWrite as i32,
                payload: Some(Payload::FsWritePayload(FsWritePayload {
                    path: path.display().to_string(),
                    contents,
                    append,
                    create_dirs,
                })),
            }),
            PluginCommand::FsRemove { path, recursive } => Ok(ProtobufPluginCommand {
                name: CommandName::FsRemove as i32,
                payload: Some(Payload::FsRemovePayload(FsRemovePayload {
                    path: path.display().to_string(),
                    recursive,
                })),
            }),
            PluginCommand::FsCreateDir { path, all } => Ok(ProtobufPluginCommand {
                name: CommandName::FsCreateDir as i32,
                payload: Some(Payload::FsCreateDirPayload(FsCreateDirPayload {
                    path: path.display().to_string(),
                    all,
                })),
            }),
            PluginCommand::GrantPluginPermissions {
                plugin_url,
                permissions,
            } => Ok(ProtobufPluginCommand {
                name: CommandName::GrantPluginPermissions as i32,
                payload: Some(Payload::GrantPluginPermissionsPayload(
                    GrantPluginPermissionsPayload {
                        plugin_url,
                        permissions: permissions
                            .iter()
                            .filter_map(|p| ProtobufPermissionType::try_from(*p).ok())
                            .map(|p| p as i32)
                            .collect(),
                    },
                )),
            }),
            PluginCommand::FsRename { from, to } => Ok(ProtobufPluginCommand {
                name: CommandName::FsRename as i32,
                payload: Some(Payload::FsRenamePayload(FsRenamePayload {
                    from: from.display().to_string(),
                    to: to.display().to_string(),
                })),
            }),
            PluginCommand::GetSessionList => Ok(ProtobufPluginCommand {
                name: CommandName::GetSessionList as i32,
                payload: Some(Payload::GetSessionListPayload(
                    ProtobufGetSessionListPayload {},
                )),
            }),
            PluginCommand::KillSessionsAndReply(session_names) => Ok(ProtobufPluginCommand {
                name: CommandName::KillSessionsAndReply as i32,
                payload: Some(Payload::KillSessionsAndReplyPayload(KillSessionsPayload {
                    session_names,
                })),
            }),
            PluginCommand::DeleteDeadSessionAndReply(dead_session_name) => {
                Ok(ProtobufPluginCommand {
                    name: CommandName::DeleteDeadSessionAndReply as i32,
                    payload: Some(Payload::DeleteDeadSessionAndReplyPayload(dead_session_name)),
                })
            },
            PluginCommand::DeleteAllDeadSessionsAndReply => Ok(ProtobufPluginCommand {
                name: CommandName::DeleteAllDeadSessionsAndReply as i32,
                payload: None,
            }),
            PluginCommand::SetSoftKeyboard(on) => Ok(ProtobufPluginCommand {
                name: CommandName::SetSoftKeyboard as i32,
                payload: Some(Payload::SetSoftKeyboardPayload(
                    ProtobufSetSoftKeyboardPayload { on },
                )),
            }),
            PluginCommand::DumpSessionLayout { tab_index } => Ok(ProtobufPluginCommand {
                name: CommandName::DumpSessionLayout as i32,
                payload: tab_index.map(|idx| {
                    Payload::DumpSessionLayoutPayload(DumpSessionLayoutPayload {
                        tab_index: Some(idx as u32),
                    })
                }),
            }),
            PluginCommand::CloseSelf => Ok(ProtobufPluginCommand {
                name: CommandName::CloseSelf as i32,
                payload: None,
            }),
            PluginCommand::NewTabsWithLayoutInfo(new_tabs_with_layout_info_payload) => {
                Ok(ProtobufPluginCommand {
                    name: CommandName::NewTabsWithLayoutInfo as i32,
                    payload: Some(Payload::NewTabsWithLayoutInfoPayload(
                        NewTabsWithLayoutInfoPayload {
                            layout_info: new_tabs_with_layout_info_payload.try_into().ok(),
                        },
                    )),
                })
            },
            PluginCommand::Reconfigure(config, write_to_disk) => Ok(ProtobufPluginCommand {
                name: CommandName::Reconfigure as i32,
                payload: Some(Payload::ReconfigurePayload(ReconfigurePayload {
                    config,
                    write_to_disk,
                })),
            }),
            PluginCommand::HidePaneWithId(pane_id_to_hide) => Ok(ProtobufPluginCommand {
                name: CommandName::HidePaneWithId as i32,
                payload: Some(Payload::HidePaneWithIdPayload(HidePaneWithIdPayload {
                    pane_id: ProtobufPaneId::try_from(pane_id_to_hide).ok(),
                })),
            }),
            PluginCommand::ShowPaneWithId(
                pane_id_to_show,
                should_float_if_hidden,
                should_focus_pane,
            ) => Ok(ProtobufPluginCommand {
                name: CommandName::ShowPaneWithId as i32,
                payload: Some(Payload::ShowPaneWithIdPayload(ShowPaneWithIdPayload {
                    pane_id: ProtobufPaneId::try_from(pane_id_to_show).ok(),
                    should_float_if_hidden,
                    should_focus_pane,
                })),
            }),
            PluginCommand::OpenCommandPaneBackground(command_to_run, context) => {
                let context: Vec<_> = context
                    .into_iter()
                    .map(|(name, value)| ContextItem { name, value })
                    .collect();
                Ok(ProtobufPluginCommand {
                    name: CommandName::OpenCommandPaneBackground as i32,
                    payload: Some(Payload::OpenCommandPaneBackgroundPayload(
                        OpenCommandPanePayload {
                            command_to_run: Some(command_to_run.try_into()?),
                            floating_pane_coordinates: None,
                            context,
                        },
                    )),
                })
            },
            PluginCommand::RerunCommandPane(terminal_pane_id) => Ok(ProtobufPluginCommand {
                name: CommandName::RerunCommandPane as i32,
                payload: Some(Payload::RerunCommandPanePayload(RerunCommandPanePayload {
                    terminal_pane_id,
                })),
            }),
            PluginCommand::ResizePaneIdWithDirection(resize, pane_id) => {
                Ok(ProtobufPluginCommand {
                    name: CommandName::ResizePaneIdWithDirection as i32,
                    payload: Some(Payload::ResizePaneIdWithDirectionPayload(
                        ResizePaneIdWithDirectionPayload {
                            resize: Some(resize.try_into()?),
                            pane_id: Some(pane_id.try_into()?),
                        },
                    )),
                })
            },
            PluginCommand::EditScrollbackForPaneWithId(pane_id) => Ok(ProtobufPluginCommand {
                name: CommandName::EditScrollbackForPaneWithId as i32,
                payload: Some(Payload::EditScrollbackForPaneWithIdPayload(
                    EditScrollbackForPaneWithIdPayload {
                        pane_id: Some(pane_id.try_into()?),
                    },
                )),
            }),
            PluginCommand::GetPaneScrollback {
                pane_id,
                get_full_scrollback,
            } => Ok(ProtobufPluginCommand {
                name: CommandName::GetPaneScrollback as i32,
                payload: Some(Payload::GetPaneScrollbackPayload(
                    GetPaneScrollbackPayload {
                        pane_id: Some(pane_id.try_into()?),
                        get_full_scrollback,
                    },
                )),
            }),
            PluginCommand::WriteToPaneId(bytes_to_write, pane_id) => Ok(ProtobufPluginCommand {
                name: CommandName::WriteToPaneId as i32,
                payload: Some(Payload::WriteToPaneIdPayload(WriteToPaneIdPayload {
                    bytes_to_write,
                    pane_id: Some(pane_id.try_into()?),
                })),
            }),
            PluginCommand::WriteCharsToPaneId(chars_to_write, pane_id) => {
                Ok(ProtobufPluginCommand {
                    name: CommandName::WriteCharsToPaneId as i32,
                    payload: Some(Payload::WriteCharsToPaneIdPayload(
                        WriteCharsToPaneIdPayload {
                            chars_to_write,
                            pane_id: Some(pane_id.try_into()?),
                        },
                    )),
                })
            },
            PluginCommand::SendSigintToPaneId(pane_id) => Ok(ProtobufPluginCommand {
                name: CommandName::SendSigintToPaneId as i32,
                payload: Some(Payload::SendSigintToPaneIdPayload(pane_id.try_into()?)),
            }),
            PluginCommand::SendSigkillToPaneId(pane_id) => Ok(ProtobufPluginCommand {
                name: CommandName::SendSigkillToPaneId as i32,
                payload: Some(Payload::SendSigkillToPaneIdPayload(pane_id.try_into()?)),
            }),
            PluginCommand::GetPanePid { pane_id } => Ok(ProtobufPluginCommand {
                name: CommandName::GetPanePid as i32,
                payload: Some(Payload::GetPanePidPayload(GetPanePidPayload {
                    pane_id: Some(pane_id.try_into()?),
                })),
            }),
            PluginCommand::GetPaneRunningCommand { pane_id } => Ok(ProtobufPluginCommand {
                name: CommandName::GetPaneRunningCommand as i32,
                payload: Some(Payload::GetPaneRunningCommandPayload(
                    ProtobufGetPaneRunningCommandPayload {
                        pane_id: Some(pane_id.try_into()?),
                    },
                )),
            }),
            PluginCommand::GetPaneCwd { pane_id } => Ok(ProtobufPluginCommand {
                name: CommandName::GetPaneCwd as i32,
                payload: Some(Payload::GetPaneCwdPayload(ProtobufGetPaneCwdPayload {
                    pane_id: Some(pane_id.try_into()?),
                })),
            }),
            PluginCommand::GetPaneGitProbe { pane_id } => Ok(ProtobufPluginCommand {
                name: CommandName::GetPaneGitProbe as i32,
                payload: Some(Payload::GetPaneGitProbePayload(
                    ProtobufGetPaneGitProbePayload {
                        pane_id: Some(pane_id.try_into()?),
                    },
                )),
            }),
            PluginCommand::OverrideLayout(
                layout_info,
                retain_existing_terminal_panes,
                retain_existing_plugin_panes,
                apply_only_to_active_tab,
                context,
            ) => Ok(ProtobufPluginCommand {
                name: CommandName::OverrideLayout as i32,
                payload: Some(Payload::OverrideLayoutPayload(OverrideLayoutPayload {
                    layout_info: layout_info.try_into().ok(),
                    context: context
                        .into_iter()
                        .map(|(name, value)| ContextItem { name, value })
                        .collect(),
                    retain_existing_terminal_panes,
                    retain_existing_plugin_panes,
                    apply_only_to_active_tab,
                })),
            }),
            PluginCommand::SaveLayout {
                layout_name,
                layout_kdl,
                overwrite,
            } => Ok(ProtobufPluginCommand {
                name: CommandName::SaveLayout as i32,
                payload: Some(Payload::SaveLayoutPayload(SaveLayoutPayload {
                    layout_name,
                    layout_kdl,
                    overwrite,
                })),
            }),
            PluginCommand::DeleteLayout { layout_name } => Ok(ProtobufPluginCommand {
                name: CommandName::DeleteLayout as i32,
                payload: Some(Payload::DeleteLayoutPayload(DeleteLayoutPayload {
                    layout_name,
                })),
            }),
            PluginCommand::RenameLayout {
                old_layout_name,
                new_layout_name,
            } => Ok(ProtobufPluginCommand {
                name: CommandName::RenameLayout as i32,
                payload: Some(Payload::RenameLayoutPayload(RenameLayoutPayload {
                    old_layout_name,
                    new_layout_name,
                })),
            }),
            PluginCommand::EditLayout {
                layout_name,
                context,
            } => {
                let context: Vec<_> = context
                    .into_iter()
                    .map(|(name, value)| ContextItem { name, value })
                    .collect();
                Ok(ProtobufPluginCommand {
                    name: CommandName::EditLayout as i32,
                    payload: Some(Payload::EditLayoutPayload(EditLayoutPayload {
                        layout_name,
                        context,
                    })),
                })
            },
            PluginCommand::MovePaneWithPaneId(pane_id) => Ok(ProtobufPluginCommand {
                name: CommandName::MovePaneWithPaneId as i32,
                payload: Some(Payload::MovePaneWithPaneIdPayload(
                    MovePaneWithPaneIdPayload {
                        pane_id: Some(pane_id.try_into()?),
                    },
                )),
            }),
            PluginCommand::MovePaneWithPaneIdInDirection(pane_id, direction) => {
                Ok(ProtobufPluginCommand {
                    name: CommandName::MovePaneWithPaneIdInDirection as i32,
                    payload: Some(Payload::MovePaneWithPaneIdInDirectionPayload(
                        MovePaneWithPaneIdInDirectionPayload {
                            pane_id: Some(pane_id.try_into()?),
                            direction: Some(direction.try_into()?),
                        },
                    )),
                })
            },
            PluginCommand::ClearScreenForPaneId(pane_id) => Ok(ProtobufPluginCommand {
                name: CommandName::ClearScreenForPaneId as i32,
                payload: Some(Payload::ClearScreenForPaneIdPayload(
                    ClearScreenForPaneIdPayload {
                        pane_id: Some(pane_id.try_into()?),
                    },
                )),
            }),
            PluginCommand::ScrollUpInPaneId(pane_id) => Ok(ProtobufPluginCommand {
                name: CommandName::ScrollUpInPaneId as i32,
                payload: Some(Payload::ScrollUpInPaneIdPayload(ScrollUpInPaneIdPayload {
                    pane_id: Some(pane_id.try_into()?),
                })),
            }),
            PluginCommand::ScrollDownInPaneId(pane_id) => Ok(ProtobufPluginCommand {
                name: CommandName::ScrollDownInPaneId as i32,
                payload: Some(Payload::ScrollDownInPaneIdPayload(
                    ScrollDownInPaneIdPayload {
                        pane_id: Some(pane_id.try_into()?),
                    },
                )),
            }),
            PluginCommand::ScrollToTopInPaneId(pane_id) => Ok(ProtobufPluginCommand {
                name: CommandName::ScrollToTopInPaneId as i32,
                payload: Some(Payload::ScrollToTopInPaneIdPayload(
                    ScrollToTopInPaneIdPayload {
                        pane_id: Some(pane_id.try_into()?),
                    },
                )),
            }),
            PluginCommand::ScrollToBottomInPaneId(pane_id) => Ok(ProtobufPluginCommand {
                name: CommandName::ScrollToBottomInPaneId as i32,
                payload: Some(Payload::ScrollToBottomInPaneIdPayload(
                    ScrollToBottomInPaneIdPayload {
                        pane_id: Some(pane_id.try_into()?),
                    },
                )),
            }),
            PluginCommand::PageScrollUpInPaneId(pane_id) => Ok(ProtobufPluginCommand {
                name: CommandName::PageScrollUpInPaneId as i32,
                payload: Some(Payload::PageScrollUpInPaneIdPayload(
                    PageScrollUpInPaneIdPayload {
                        pane_id: Some(pane_id.try_into()?),
                    },
                )),
            }),
            PluginCommand::PageScrollDownInPaneId(pane_id) => Ok(ProtobufPluginCommand {
                name: CommandName::PageScrollDownInPaneId as i32,
                payload: Some(Payload::PageScrollDownInPaneIdPayload(
                    PageScrollDownInPaneIdPayload {
                        pane_id: Some(pane_id.try_into()?),
                    },
                )),
            }),
            PluginCommand::TogglePaneIdFullscreen(pane_id) => Ok(ProtobufPluginCommand {
                name: CommandName::TogglePaneIdFullscreen as i32,
                payload: Some(Payload::TogglePaneIdFullscreenPayload(
                    TogglePaneIdFullscreenPayload {
                        pane_id: Some(pane_id.try_into()?),
                    },
                )),
            }),
            PluginCommand::TogglePaneEmbedOrEjectForPaneId(pane_id) => Ok(ProtobufPluginCommand {
                name: CommandName::TogglePaneEmbedOrEjectForPaneId as i32,
                payload: Some(Payload::TogglePaneEmbedOrEjectForPaneIdPayload(
                    TogglePaneEmbedOrEjectForPaneIdPayload {
                        pane_id: Some(pane_id.try_into()?),
                    },
                )),
            }),
            PluginCommand::CloseTabWithIndex(tab_index) => Ok(ProtobufPluginCommand {
                name: CommandName::CloseTabWithIndex as i32,
                payload: Some(Payload::CloseTabWithIndexPayload(
                    CloseTabWithIndexPayload {
                        tab_index: tab_index as u32,
                    },
                )),
            }),
            PluginCommand::BreakPanesToNewTab(
                pane_ids,
                new_tab_name,
                should_change_focus_to_new_tab,
            ) => Ok(ProtobufPluginCommand {
                name: CommandName::BreakPanesToNewTab as i32,
                payload: Some(Payload::BreakPanesToNewTabPayload(
                    BreakPanesToNewTabPayload {
                        pane_ids: pane_ids
                            .into_iter()
                            .filter_map(|p_id| p_id.try_into().ok())
                            .collect(),
                        should_change_focus_to_new_tab,
                        new_tab_name,
                    },
                )),
            }),
            PluginCommand::BreakPanesToTabWithIndex(
                pane_ids,
                tab_index,
                should_change_focus_to_target_tab,
            ) => Ok(ProtobufPluginCommand {
                name: CommandName::BreakPanesToTabWithIndex as i32,
                payload: Some(Payload::BreakPanesToTabWithIndexPayload(
                    BreakPanesToTabWithIndexPayload {
                        pane_ids: pane_ids
                            .into_iter()
                            .filter_map(|p_id| p_id.try_into().ok())
                            .collect(),
                        tab_index: tab_index as u32,
                        should_change_focus_to_target_tab,
                    },
                )),
            }),
            PluginCommand::SwitchTabToId(tab_id) => Ok(ProtobufPluginCommand {
                name: CommandName::SwitchTabToId as i32,
                payload: Some(Payload::SwitchTabToIdPayload(SwitchTabToIdPayload {
                    tab_id,
                })),
            }),
            PluginCommand::GoToTabWithId(tab_id) => Ok(ProtobufPluginCommand {
                name: CommandName::GoToTabWithId as i32,
                payload: Some(Payload::GoToTabWithIdPayload(GoToTabWithIdPayload {
                    tab_id,
                })),
            }),
            PluginCommand::CloseTabWithId(tab_id) => Ok(ProtobufPluginCommand {
                name: CommandName::CloseTabWithId as i32,
                payload: Some(Payload::CloseTabWithIdPayload(CloseTabWithIdPayload {
                    tab_id,
                })),
            }),
            PluginCommand::RenameTabWithId(tab_id, new_name) => Ok(ProtobufPluginCommand {
                name: CommandName::RenameTabWithId as i32,
                payload: Some(Payload::RenameTabWithIdPayload(RenameTabWithIdPayload {
                    tab_id,
                    new_name,
                })),
            }),
            PluginCommand::SetTabProjects(tab_projects) => Ok(ProtobufPluginCommand {
                name: CommandName::SetTabProjects as i32,
                payload: Some(Payload::SetTabProjectsPayload(SetTabProjectsPayload {
                    tab_projects: tab_projects
                        .into_iter()
                        .map(|(tab_id, project)| TabProject { tab_id, project })
                        .collect(),
                })),
            }),
            PluginCommand::BreakPanesToTabWithId(
                pane_ids,
                tab_id,
                should_change_focus_to_target_tab,
            ) => Ok(ProtobufPluginCommand {
                name: CommandName::BreakPanesToTabWithId as i32,
                payload: Some(Payload::BreakPanesToTabWithIdPayload(
                    BreakPanesToTabWithIdPayload {
                        pane_ids: pane_ids
                            .into_iter()
                            .filter_map(|p_id| p_id.try_into().ok())
                            .collect(),
                        tab_id,
                        should_change_focus_to_target_tab,
                    },
                )),
            }),
            PluginCommand::ReloadPlugin(plugin_id) => Ok(ProtobufPluginCommand {
                name: CommandName::ReloadPlugin as i32,
                payload: Some(Payload::ReloadPluginPayload(ReloadPluginPayload {
                    plugin_id,
                })),
            }),
            PluginCommand::LoadNewPlugin {
                url,
                config,
                load_in_background,
                skip_plugin_cache,
            } => Ok(ProtobufPluginCommand {
                name: CommandName::LoadNewPlugin as i32,
                payload: Some(Payload::LoadNewPluginPayload(LoadNewPluginPayload {
                    plugin_url: url,
                    plugin_config: config
                        .into_iter()
                        .map(|(name, value)| ContextItem { name, value })
                        .collect(),
                    should_skip_plugin_cache: skip_plugin_cache,
                    should_load_plugin_in_background: load_in_background,
                })),
            }),
            PluginCommand::RebindKeys {
                keys_to_rebind,
                keys_to_unbind,
                write_config_to_disk,
            } => Ok(ProtobufPluginCommand {
                name: CommandName::RebindKeys as i32,
                payload: Some(Payload::RebindKeysPayload(RebindKeysPayload {
                    keys_to_rebind: keys_to_rebind
                        .into_iter()
                        .filter_map(|k| k.try_into().ok())
                        .collect(),
                    keys_to_unbind: keys_to_unbind
                        .into_iter()
                        .filter_map(|k| k.try_into().ok())
                        .collect(),
                    write_config_to_disk,
                })),
            }),
            PluginCommand::ListClients => Ok(ProtobufPluginCommand {
                name: CommandName::ListClients as i32,
                payload: None,
            }),
            PluginCommand::ChangeHostFolder(new_host_folder) => Ok(ProtobufPluginCommand {
                name: CommandName::ChangeHostFolder as i32,
                payload: Some(Payload::ChangeHostFolderPayload(ChangeHostFolderPayload {
                    new_host_folder: new_host_folder.display().to_string(), // TODO: not accurate?
                })),
            }),
            PluginCommand::SetFloatingPanePinned(pane_id, should_be_pinned) => {
                Ok(ProtobufPluginCommand {
                    name: CommandName::SetFloatingPanePinned as i32,
                    payload: Some(Payload::SetFloatingPanePinnedPayload(
                        SetFloatingPanePinnedPayload {
                            pane_id: pane_id.try_into().ok(),
                            should_be_pinned,
                        },
                    )),
                })
            },
            PluginCommand::StackPanes(pane_ids) => Ok(ProtobufPluginCommand {
                name: CommandName::StackPanes as i32,
                payload: Some(Payload::StackPanesPayload(StackPanesPayload {
                    pane_ids: pane_ids
                        .into_iter()
                        .filter_map(|p_id| p_id.try_into().ok())
                        .collect(),
                })),
            }),
            PluginCommand::ChangeFloatingPanesCoordinates(
                pane_ids_and_floating_panes_coordinates,
            ) => Ok(ProtobufPluginCommand {
                name: CommandName::ChangeFloatingPanesCoordinates as i32,
                payload: Some(Payload::ChangeFloatingPanesCoordinatesPayload(
                    ChangeFloatingPanesCoordinatesPayload {
                        pane_ids_and_floating_panes_coordinates:
                            pane_ids_and_floating_panes_coordinates
                                .into_iter()
                                .filter_map(|(p_id, floating_pane_coordinates)| {
                                    Some(PaneIdAndFloatingPaneCoordinates {
                                        pane_id: Some(p_id.try_into().ok()?),
                                        floating_pane_coordinates: Some(
                                            floating_pane_coordinates.try_into().ok()?,
                                        ),
                                    })
                                })
                                .collect(),
                    },
                )),
            }),
            PluginCommand::ChangeOverlayCoordinates(pane_ids_and_coordinates) => {
                Ok(ProtobufPluginCommand {
                    name: CommandName::ChangeOverlayCoordinates as i32,
                    payload: Some(Payload::ChangeOverlayCoordinatesPayload(
                        ChangeOverlayCoordinatesPayload {
                            pane_ids_and_floating_panes_coordinates: pane_ids_and_coordinates
                                .into_iter()
                                .filter_map(|(p_id, coordinates)| {
                                    Some(PaneIdAndFloatingPaneCoordinates {
                                        pane_id: Some(p_id.try_into().ok()?),
                                        floating_pane_coordinates: Some(
                                            coordinates.try_into().ok()?,
                                        ),
                                    })
                                })
                                .collect(),
                        },
                    )),
                })
            },
            PluginCommand::TogglePaneBorderless(pane_id) => Ok(ProtobufPluginCommand {
                name: CommandName::TogglePaneBorderless as i32,
                payload: Some(Payload::TogglePaneBorderlessPayload(
                    TogglePaneBorderlessPayload {
                        pane_id: Some(pane_id.try_into()?),
                    },
                )),
            }),
            PluginCommand::SetPaneBorderless(pane_id, borderless) => Ok(ProtobufPluginCommand {
                name: CommandName::SetPaneBorderless as i32,
                payload: Some(Payload::SetPaneBorderlessPayload(
                    SetPaneBorderlessPayload {
                        pane_id: Some(pane_id.try_into()?),
                        borderless,
                    },
                )),
            }),
            PluginCommand::OpenCommandPaneNearPlugin(command_to_run, context) => {
                let context: Vec<_> = context
                    .into_iter()
                    .map(|(name, value)| ContextItem { name, value })
                    .collect();
                Ok(ProtobufPluginCommand {
                    name: CommandName::OpenCommandPaneNearPlugin as i32,
                    payload: Some(Payload::OpenCommandPaneNearPluginPayload(
                        OpenCommandPaneNearPluginPayload {
                            command_to_run: Some(command_to_run.try_into()?),
                            floating_pane_coordinates: None,
                            context,
                        },
                    )),
                })
            },
            PluginCommand::OpenCommandPaneFloatingNearPlugin(
                command_to_run,
                floating_pane_coordinates,
                context,
            ) => {
                let context: Vec<_> = context
                    .into_iter()
                    .map(|(name, value)| ContextItem { name, value })
                    .collect();
                Ok(ProtobufPluginCommand {
                    name: CommandName::OpenCommandPaneFloatingNearPlugin as i32,
                    payload: Some(Payload::OpenCommandPaneFloatingNearPluginPayload(
                        OpenCommandPaneFloatingNearPluginPayload {
                            command_to_run: Some(command_to_run.try_into()?),
                            floating_pane_coordinates: floating_pane_coordinates.map(|f| f.into()),
                            context,
                        },
                    )),
                })
            },
            PluginCommand::OpenTerminalNearPlugin(cwd) => Ok(ProtobufPluginCommand {
                name: CommandName::OpenTerminalNearPlugin as i32,
                payload: Some(Payload::OpenTerminalNearPluginPayload(
                    OpenTerminalNearPluginPayload {
                        file_to_open: Some(cwd.try_into()?),
                        context: vec![], // will be added in the future
                    },
                )),
            }),
            PluginCommand::OpenTerminalFloatingNearPlugin(cwd, floating_pane_coordinates) => {
                Ok(ProtobufPluginCommand {
                    name: CommandName::OpenTerminalFloatingNearPlugin as i32,
                    payload: Some(Payload::OpenTerminalFloatingNearPluginPayload(
                        OpenTerminalFloatingNearPluginPayload {
                            file_to_open: Some(cwd.try_into()?),
                            floating_pane_coordinates: floating_pane_coordinates.map(|f| f.into()),
                            context: vec![], // will be added in the future
                        },
                    )),
                })
            },
            PluginCommand::OpenTerminalInPlaceOfPlugin(cwd, close_plugin_after_replace) => {
                Ok(ProtobufPluginCommand {
                    name: CommandName::OpenTerminalInPlaceOfPlugin as i32,
                    payload: Some(Payload::OpenTerminalInPlaceOfPluginPayload(
                        OpenTerminalInPlaceOfPluginPayload {
                            file_to_open: Some(cwd.try_into()?),
                            close_plugin_after_replace,
                            context: vec![], // will be added in the future
                        },
                    )),
                })
            },
            PluginCommand::OpenCommandPaneInPlaceOfPlugin(
                command_to_run,
                close_plugin_after_replace,
                context,
            ) => {
                let context: Vec<_> = context
                    .into_iter()
                    .map(|(name, value)| ContextItem { name, value })
                    .collect();
                Ok(ProtobufPluginCommand {
                    name: CommandName::OpenCommandPaneInPlaceOfPlugin as i32,
                    payload: Some(Payload::OpenCommandPaneInPlaceOfPluginPayload(
                        OpenCommandPaneInPlaceOfPluginPayload {
                            command_to_run: Some(command_to_run.try_into()?),
                            close_plugin_after_replace,
                            context,
                        },
                    )),
                })
            },
            PluginCommand::OpenFileNearPlugin(file_to_open, context) => Ok(ProtobufPluginCommand {
                name: CommandName::OpenFileNearPlugin as i32,
                payload: Some(Payload::OpenFileNearPluginPayload(
                    OpenFileNearPluginPayload {
                        file_to_open: Some(file_to_open.try_into()?),
                        floating_pane_coordinates: None,
                        context: context
                            .into_iter()
                            .map(|(name, value)| ContextItem { name, value })
                            .collect(),
                    },
                )),
            }),
            PluginCommand::OpenFileFloatingNearPlugin(
                file_to_open,
                floating_pane_coordinates,
                context,
            ) => Ok(ProtobufPluginCommand {
                name: CommandName::OpenFileFloatingNearPlugin as i32,
                payload: Some(Payload::OpenFileFloatingNearPluginPayload(
                    OpenFileFloatingNearPluginPayload {
                        file_to_open: Some(file_to_open.try_into()?),
                        floating_pane_coordinates: floating_pane_coordinates.map(|f| f.into()),
                        context: context
                            .into_iter()
                            .map(|(name, value)| ContextItem { name, value })
                            .collect(),
                    },
                )),
            }),
            PluginCommand::OpenFileInPlaceOfPlugin(
                file_to_open,
                close_plugin_after_replace,
                context,
            ) => Ok(ProtobufPluginCommand {
                name: CommandName::OpenFileInPlaceOfPlugin as i32,
                payload: Some(Payload::OpenFileInPlaceOfPluginPayload(
                    OpenFileInPlaceOfPluginPayload {
                        file_to_open: Some(file_to_open.try_into()?),
                        floating_pane_coordinates: None,
                        close_plugin_after_replace,
                        context: context
                            .into_iter()
                            .map(|(name, value)| ContextItem { name, value })
                            .collect(),
                    },
                )),
            }),
            PluginCommand::GroupAndUngroupPanes(
                panes_to_group,
                panes_to_ungroup,
                for_all_clients,
            ) => Ok(ProtobufPluginCommand {
                name: CommandName::GroupAndUngroupPanes as i32,
                payload: Some(Payload::GroupAndUngroupPanesPayload(
                    GroupAndUngroupPanesPayload {
                        pane_ids_to_group: panes_to_group
                            .iter()
                            .filter_map(|&p| p.try_into().ok())
                            .collect(),
                        pane_ids_to_ungroup: panes_to_ungroup
                            .iter()
                            .filter_map(|&p| p.try_into().ok())
                            .collect(),
                        for_all_clients,
                    },
                )),
            }),
            PluginCommand::StartWebServer => Ok(ProtobufPluginCommand {
                name: CommandName::StartWebServer as i32,
                payload: None,
            }),
            PluginCommand::StopWebServer => Ok(ProtobufPluginCommand {
                name: CommandName::StopWebServer as i32,
                payload: None,
            }),
            PluginCommand::QueryWebServerStatus => Ok(ProtobufPluginCommand {
                name: CommandName::QueryWebServerStatus as i32,
                payload: None,
            }),
            PluginCommand::HighlightAndUnhighlightPanes(
                panes_to_highlight,
                panes_to_unhighlight,
            ) => Ok(ProtobufPluginCommand {
                name: CommandName::HighlightAndUnhighlightPanes as i32,
                payload: Some(Payload::HighlightAndUnhighlightPanesPayload(
                    HighlightAndUnhighlightPanesPayload {
                        pane_ids_to_highlight: panes_to_highlight
                            .iter()
                            .filter_map(|&p| p.try_into().ok())
                            .collect(),
                        pane_ids_to_unhighlight: panes_to_unhighlight
                            .iter()
                            .filter_map(|&p| p.try_into().ok())
                            .collect(),
                    },
                )),
            }),
            PluginCommand::SetPaneTitlePrefix { pane_id, prefix } => Ok(ProtobufPluginCommand {
                name: CommandName::SetPaneTitlePrefix as i32,
                payload: Some(Payload::SetPaneTitlePrefixPayload(
                    SetPaneTitlePrefixPayload {
                        pane_id: pane_id.try_into().ok(),
                        prefix,
                    },
                )),
            }),
            PluginCommand::RevealPaneWithId(pane_id, label) => Ok(ProtobufPluginCommand {
                name: CommandName::RevealPaneWithId as i32,
                payload: Some(Payload::RevealPaneWithIdPayload(RevealPaneWithIdPayload {
                    pane_id: pane_id.try_into().ok(),
                    label,
                })),
            }),
            PluginCommand::CloseMultiplePanes(pane_ids) => Ok(ProtobufPluginCommand {
                name: CommandName::CloseMultiplePanes as i32,
                payload: Some(Payload::CloseMultiplePanesPayload(
                    CloseMultiplePanesPayload {
                        pane_ids: pane_ids.iter().filter_map(|&p| p.try_into().ok()).collect(),
                    },
                )),
            }),
            PluginCommand::FloatMultiplePanes(pane_ids) => Ok(ProtobufPluginCommand {
                name: CommandName::FloatMultiplePanes as i32,
                payload: Some(Payload::FloatMultiplePanesPayload(
                    FloatMultiplePanesPayload {
                        pane_ids: pane_ids.iter().filter_map(|&p| p.try_into().ok()).collect(),
                    },
                )),
            }),
            PluginCommand::EmbedMultiplePanes(pane_ids) => Ok(ProtobufPluginCommand {
                name: CommandName::EmbedMultiplePanes as i32,
                payload: Some(Payload::EmbedMultiplePanesPayload(
                    EmbedMultiplePanesPayload {
                        pane_ids: pane_ids.iter().filter_map(|&p| p.try_into().ok()).collect(),
                    },
                )),
            }),
            PluginCommand::ShareCurrentSession => Ok(ProtobufPluginCommand {
                name: CommandName::ShareCurrentSession as i32,
                payload: None,
            }),
            PluginCommand::StopSharingCurrentSession => Ok(ProtobufPluginCommand {
                name: CommandName::StopSharingCurrentSession as i32,
                payload: None,
            }),
            PluginCommand::SetSelfMouseSelectionSupport(support_mouse_selection) => {
                Ok(ProtobufPluginCommand {
                    name: CommandName::SetSelfMouseSelectionSupport as i32,
                    payload: Some(Payload::SetSelfMouseSelectionSupportPayload(
                        SetSelfMouseSelectionSupportPayload {
                            support_mouse_selection,
                        },
                    )),
                })
            },
            PluginCommand::GenerateWebLoginToken(token_label, read_only) => {
                Ok(ProtobufPluginCommand {
                    name: CommandName::GenerateWebLoginToken as i32,
                    payload: Some(Payload::GenerateWebLoginTokenPayload(
                        GenerateWebLoginTokenPayload {
                            token_label,
                            read_only: Some(read_only),
                        },
                    )),
                })
            },
            PluginCommand::RevokeWebLoginToken(token_label) => Ok(ProtobufPluginCommand {
                name: CommandName::RevokeWebLoginToken as i32,
                payload: Some(Payload::RevokeWebLoginTokenPayload(
                    RevokeWebLoginTokenPayload { token_label },
                )),
            }),
            PluginCommand::ListWebLoginTokens => Ok(ProtobufPluginCommand {
                name: CommandName::ListWebLoginTokens as i32,
                payload: None,
            }),
            PluginCommand::RevokeAllWebLoginTokens => Ok(ProtobufPluginCommand {
                name: CommandName::RevokeAllWebLoginTokens as i32,
                payload: None,
            }),
            PluginCommand::RenameWebLoginToken(old_name, new_name) => Ok(ProtobufPluginCommand {
                name: CommandName::RenameWebLoginToken as i32,
                payload: Some(Payload::RenameWebLoginTokenPayload(
                    RenameWebLoginTokenPayload { old_name, new_name },
                )),
            }),
            PluginCommand::InterceptKeyPresses => Ok(ProtobufPluginCommand {
                name: CommandName::InterceptKeyPresses as i32,
                payload: None,
            }),
            PluginCommand::ClearKeyPressesIntercepts => Ok(ProtobufPluginCommand {
                name: CommandName::ClearKeyPressesIntercepts as i32,
                payload: None,
            }),
            PluginCommand::ReplacePaneWithExistingPane(
                pane_id_to_replace,
                existing_pane_id,
                suppress_replaced_pane,
            ) => Ok(ProtobufPluginCommand {
                name: CommandName::ReplacePaneWithExistingPane as i32,
                payload: Some(Payload::ReplacePaneWithExistingPanePayload(
                    ReplacePaneWithExistingPanePayload {
                        pane_id_to_replace: ProtobufPaneId::try_from(pane_id_to_replace).ok(),
                        existing_pane_id: ProtobufPaneId::try_from(existing_pane_id).ok(),
                        suppress_replaced_pane,
                    },
                )),
            }),
            PluginCommand::RunAction(action, context) => {
                let protobuf_action = ProtobufAction::try_from(action)
                    .map_err(|_| "Failed to convert action to protobuf")?;

                let context_items: Vec<ContextItem> = context
                    .into_iter()
                    .map(|(name, value)| ContextItem { name, value })
                    .collect();

                Ok(ProtobufPluginCommand {
                    name: CommandName::RunAction as i32,
                    payload: Some(Payload::RunActionPayload(RunActionPayload {
                        action: Some(protobuf_action),
                        context: context_items,
                    })),
                })
            },
            PluginCommand::CopyToClipboard(text) => Ok(ProtobufPluginCommand {
                name: CommandName::CopyToClipboard as i32,
                payload: Some(Payload::CopyToClipboardPayload(CopyToClipboardPayload {
                    text,
                })),
            }),
            PluginCommand::GenerateRandomName => Ok(ProtobufPluginCommand {
                name: CommandName::GenerateRandomName as i32,
                payload: Some(Payload::GenerateRandomNamePayload(
                    GenerateRandomNamePayload {},
                )),
            }),
            PluginCommand::DumpLayout(layout_name) => Ok(ProtobufPluginCommand {
                name: CommandName::DumpLayout as i32,
                payload: Some(Payload::DumpLayoutPayload(DumpLayoutPayload {
                    layout_name,
                })),
            }),
            PluginCommand::ParseLayout(layout_string) => Ok(ProtobufPluginCommand {
                name: CommandName::ParseLayout as i32,
                payload: Some(Payload::ParseLayoutPayload(ParseLayoutPayload {
                    layout_string,
                })),
            }),
            PluginCommand::GetLayoutDir => Ok(ProtobufPluginCommand {
                name: CommandName::GetLayoutDir as i32,
                payload: Some(Payload::GetLayoutDirPayload(GetLayoutDirPayload {})),
            }),
            PluginCommand::GetUserPluginDir => Ok(ProtobufPluginCommand {
                name: CommandName::GetUserPluginDir as i32,
                payload: Some(Payload::GetUserPluginDirPayload(GetUserPluginDirPayload {})),
            }),
            PluginCommand::GetFocusedPaneInfo => Ok(ProtobufPluginCommand {
                name: CommandName::GetFocusedPaneInfo as i32,
                payload: Some(Payload::GetFocusedPaneInfoPayload(
                    GetFocusedPaneInfoPayload {},
                )),
            }),
            PluginCommand::SaveSession => Ok(ProtobufPluginCommand {
                name: CommandName::SaveSession as i32,
                payload: Some(Payload::SaveSessionPayload(SaveSessionPayload {})),
            }),
            PluginCommand::CurrentSessionLastSavedTime => Ok(ProtobufPluginCommand {
                name: CommandName::CurrentSessionLastSavedTime as i32,
                payload: Some(Payload::CurrentSessionLastSavedTimePayload(
                    CurrentSessionLastSavedTimePayload {},
                )),
            }),
            PluginCommand::GetPaneInfo(pane_id) => {
                let protobuf_pane_id: ProtobufPaneId = pane_id.try_into()?;
                Ok(ProtobufPluginCommand {
                    name: CommandName::GetPaneInfo as i32,
                    payload: Some(Payload::GetPaneInfoPayload(GetPaneInfoPayload {
                        pane_id: Some(protobuf_pane_id),
                    })),
                })
            },
            PluginCommand::GetTabInfo(tab_id) => Ok(ProtobufPluginCommand {
                name: CommandName::GetTabInfo as i32,
                payload: Some(Payload::GetTabInfoPayload(GetTabInfoPayload {
                    tab_id: tab_id as u64,
                })),
            }),
            PluginCommand::GetSessionEnvironmentVariables => {
                let payload = ProtobufGetSessionEnvironmentVariablesPayload {};
                Ok(ProtobufPluginCommand {
                    name: CommandName::GetSessionEnvironmentVariables as i32,
                    payload: Some(Payload::GetSessionEnvironmentVariablesPayload(payload)),
                })
            },
            PluginCommand::GetPluginAliasConfiguration(alias) => {
                let payload = ProtobufGetPluginAliasConfigurationPayload { alias };
                Ok(ProtobufPluginCommand {
                    name: CommandName::GetPluginAliasConfiguration as i32,
                    payload: Some(Payload::GetPluginAliasConfigurationPayload(payload)),
                })
            },
            PluginCommand::OpenCommandPaneInNewTab(command_to_run, context) => {
                let context: Vec<_> = context
                    .into_iter()
                    .map(|(name, value)| ContextItem { name, value })
                    .collect();
                Ok(ProtobufPluginCommand {
                    name: CommandName::OpenCommandPaneInNewTab as i32,
                    payload: Some(Payload::OpenCommandPaneInNewTabPayload(
                        OpenCommandPanePayload {
                            command_to_run: Some(command_to_run.try_into()?),
                            floating_pane_coordinates: None,
                            context,
                        },
                    )),
                })
            },
            PluginCommand::OpenPluginPaneInNewTab {
                plugin_url,
                configuration,
                context,
            } => {
                let context: Vec<_> = context
                    .into_iter()
                    .map(|(name, value)| ContextItem { name, value })
                    .collect();
                let configuration: std::collections::HashMap<String, String> =
                    configuration.into_iter().collect();
                Ok(ProtobufPluginCommand {
                    name: CommandName::OpenPluginPaneInNewTab as i32,
                    payload: Some(Payload::OpenPluginPaneInNewTabPayload(
                        ProtobufOpenPluginPaneInNewTabPayload {
                            plugin_url,
                            configuration,
                            context,
                        },
                    )),
                })
            },
            PluginCommand::OpenEditorPaneInNewTab(file_to_open, context) => {
                let context: Vec<_> = context
                    .into_iter()
                    .map(|(name, value)| ContextItem { name, value })
                    .collect();
                Ok(ProtobufPluginCommand {
                    name: CommandName::OpenEditorPaneInNewTab as i32,
                    payload: Some(Payload::OpenEditorPaneInNewTabPayload(OpenFilePayload {
                        file_to_open: Some(file_to_open.try_into()?),
                        floating_pane_coordinates: None,
                        context,
                    })),
                })
            },
            PluginCommand::OpenCommandPaneInPlaceOfPaneId(
                pane_id,
                command_to_run,
                close_replaced_pane,
                context,
            ) => {
                let context: Vec<_> = context
                    .into_iter()
                    .map(|(name, value)| ContextItem { name, value })
                    .collect();
                Ok(ProtobufPluginCommand {
                    name: CommandName::OpenCommandPaneInPlaceOfPaneId as i32,
                    payload: Some(Payload::OpenCommandPaneInPlaceOfPaneIdPayload(
                        OpenCommandPaneInPlaceOfPaneIdPayload {
                            pane_id_to_replace: Some(pane_id.try_into()?),
                            command_to_run: Some(command_to_run.try_into()?),
                            close_replaced_pane,
                            context,
                        },
                    )),
                })
            },
            PluginCommand::OpenTerminalPaneInPlaceOfPaneId(pane_id, cwd, close_replaced_pane) => {
                Ok(ProtobufPluginCommand {
                    name: CommandName::OpenTerminalPaneInPlaceOfPaneId as i32,
                    payload: Some(Payload::OpenTerminalPaneInPlaceOfPaneIdPayload(
                        OpenTerminalPaneInPlaceOfPaneIdPayload {
                            pane_id_to_replace: Some(pane_id.try_into()?),
                            cwd: Some(cwd.try_into()?),
                            close_replaced_pane,
                        },
                    )),
                })
            },
            PluginCommand::OpenEditPaneInPlaceOfPaneId(
                pane_id,
                file_to_open,
                close_replaced_pane,
                context,
            ) => {
                let context: Vec<_> = context
                    .into_iter()
                    .map(|(name, value)| ContextItem { name, value })
                    .collect();
                Ok(ProtobufPluginCommand {
                    name: CommandName::OpenEditPaneInPlaceOfPaneId as i32,
                    payload: Some(Payload::OpenEditPaneInPlaceOfPaneIdPayload(
                        OpenEditPaneInPlaceOfPaneIdPayload {
                            pane_id_to_replace: Some(pane_id.try_into()?),
                            file_to_open: Some(file_to_open.try_into()?),
                            close_replaced_pane,
                            context,
                        },
                    )),
                })
            },
            PluginCommand::HideFloatingPanes { tab_id } => Ok(ProtobufPluginCommand {
                name: CommandName::HideFloatingPanes as i32,
                payload: Some(Payload::HideFloatingPanesPayload(
                    ProtobufHideFloatingPanesPayload {
                        tab_id: tab_id.map(|id| id as u32),
                    },
                )),
            }),
            PluginCommand::ShowFloatingPanes { tab_id } => Ok(ProtobufPluginCommand {
                name: CommandName::ShowFloatingPanes as i32,
                payload: Some(Payload::ShowFloatingPanesPayload(
                    ProtobufShowFloatingPanesPayload {
                        tab_id: tab_id.map(|id| id as u32),
                    },
                )),
            }),
            PluginCommand::SetPaneColor(pane_id, fg, bg) => Ok(ProtobufPluginCommand {
                name: CommandName::SetPaneColor as i32,
                payload: Some(Payload::SetPaneColorPayload(SetPaneColorPayload {
                    pane_id: Some(pane_id.try_into()?),
                    fg,
                    bg,
                })),
            }),
            PluginCommand::SetPaneRegexHighlights(pane_id, highlights) => {
                Ok(ProtobufPluginCommand {
                    name: CommandName::SetPaneRegexHighlights as i32,
                    payload: Some(Payload::SetPaneRegexHighlightsPayload(
                        SetPaneRegexHighlightsPayload {
                            pane_id: pane_id.try_into().ok(),
                            highlights: highlights
                                .into_iter()
                                .map(|h| {
                                    let style_variant = match h.style {
                                        HighlightStyle::Emphasis0 => {
                                            ProtobufHighlightStyleVariant::Emphasis0(true)
                                        },
                                        HighlightStyle::Emphasis1 => {
                                            ProtobufHighlightStyleVariant::Emphasis1(true)
                                        },
                                        HighlightStyle::Emphasis2 => {
                                            ProtobufHighlightStyleVariant::Emphasis2(true)
                                        },
                                        HighlightStyle::Emphasis3 => {
                                            ProtobufHighlightStyleVariant::Emphasis3(true)
                                        },
                                        HighlightStyle::CustomRgb { fg, bg } => {
                                            ProtobufHighlightStyleVariant::CustomRgb(
                                                ProtobufCustomRgbHighlight {
                                                    fg_r: fg.map(|(r, _, _)| r as u32),
                                                    fg_g: fg.map(|(_, g, _)| g as u32),
                                                    fg_b: fg.map(|(_, _, b)| b as u32),
                                                    bg_r: bg.map(|(r, _, _)| r as u32),
                                                    bg_g: bg.map(|(_, g, _)| g as u32),
                                                    bg_b: bg.map(|(_, _, b)| b as u32),
                                                },
                                            )
                                        },
                                        HighlightStyle::CustomIndex { fg, bg } => {
                                            ProtobufHighlightStyleVariant::CustomIndex(
                                                ProtobufCustomIndexHighlight {
                                                    fg: fg.map(|v| v as u32),
                                                    bg: bg.map(|v| v as u32),
                                                },
                                            )
                                        },
                                        HighlightStyle::BackgroundEmphasis0 => {
                                            ProtobufHighlightStyleVariant::BackgroundEmphasis0(true)
                                        },
                                        HighlightStyle::BackgroundEmphasis1 => {
                                            ProtobufHighlightStyleVariant::BackgroundEmphasis1(true)
                                        },
                                        HighlightStyle::BackgroundEmphasis2 => {
                                            ProtobufHighlightStyleVariant::BackgroundEmphasis2(true)
                                        },
                                        HighlightStyle::BackgroundEmphasis3 => {
                                            ProtobufHighlightStyleVariant::BackgroundEmphasis3(true)
                                        },
                                        HighlightStyle::None => {
                                            ProtobufHighlightStyleVariant::None(true)
                                        },
                                    };
                                    ProtobufRegexHighlight {
                                        pattern: h.pattern,
                                        style: Some(ProtobufHighlightStyle {
                                            style: Some(style_variant),
                                        }),
                                        context: h
                                            .context
                                            .into_iter()
                                            .map(|(name, value)| ContextItem { name, value })
                                            .collect(),
                                        on_hover: h.on_hover,
                                        bold: h.bold,
                                        italic: h.italic,
                                        underline: h.underline,
                                        tooltip_text: h.tooltip_text,
                                        layer: match h.layer {
                                            HighlightLayer::Hint => {
                                                ProtobufHighlightLayer::Hint as i32
                                            },
                                            HighlightLayer::Tool => {
                                                ProtobufHighlightLayer::Tool as i32
                                            },
                                            HighlightLayer::ActionFeedback => {
                                                ProtobufHighlightLayer::ActionFeedback as i32
                                            },
                                        },
                                    }
                                })
                                .collect(),
                        },
                    )),
                })
            },
            PluginCommand::ClearPaneHighlights(pane_id) => Ok(ProtobufPluginCommand {
                name: CommandName::ClearPaneHighlights as i32,
                payload: Some(Payload::ClearPaneHighlightsPayload(
                    ClearPaneHighlightsPayload {
                        pane_id: pane_id.try_into().ok(),
                    },
                )),
            }),
            PluginCommand::OpenPluginPaneFloating {
                plugin_url,
                configuration,
                floating_pane_coordinates,
                context,
            } => {
                let context: Vec<_> = context
                    .into_iter()
                    .map(|(name, value)| ContextItem { name, value })
                    .collect();
                let configuration: std::collections::HashMap<String, String> =
                    configuration.into_iter().collect();
                Ok(ProtobufPluginCommand {
                    name: CommandName::OpenPluginPaneFloating as i32,
                    payload: Some(Payload::OpenPluginPaneFloatingPayload(
                        OpenPluginPaneFloatingPayload {
                            plugin_url,
                            configuration,
                            floating_pane_coordinates: floating_pane_coordinates.map(|f| f.into()),
                            context,
                        },
                    )),
                })
            },
            PluginCommand::WarpPaneToSession {
                pane_id,
                to_session,
                placement,
            } => Ok(ProtobufPluginCommand {
                name: CommandName::WarpPaneToSession as i32,
                payload: Some(Payload::WarpPaneToSessionPayload(
                    WarpPaneToSessionPayload {
                        pane_id: Some(pane_id.try_into()?),
                        to_session,
                        placement: ProtobufWarpPlacement::from(placement) as i32,
                    },
                )),
            }),
            PluginCommand::WarpPaneFromSession {
                pane_id,
                from_session,
                placement,
            } => Ok(ProtobufPluginCommand {
                name: CommandName::WarpPaneFromSession as i32,
                payload: Some(Payload::WarpPaneFromSessionPayload(
                    WarpPaneFromSessionPayload {
                        pane_id: Some(pane_id.try_into()?),
                        from_session,
                        placement: ProtobufWarpPlacement::from(placement) as i32,
                    },
                )),
            }),
            PluginCommand::WarpTabToSession {
                tab_index,
                to_session,
            } => Ok(ProtobufPluginCommand {
                name: CommandName::WarpTabToSession as i32,
                payload: Some(Payload::WarpTabToSessionPayload(WarpTabToSessionPayload {
                    tab_index: tab_index as u32,
                    to_session,
                })),
            }),
            PluginCommand::WarpTabFromSession {
                tab_index,
                from_session,
            } => Ok(ProtobufPluginCommand {
                name: CommandName::WarpTabFromSession as i32,
                payload: Some(Payload::WarpTabFromSessionPayload(
                    WarpTabFromSessionPayload {
                        tab_index: tab_index as u32,
                        from_session,
                    },
                )),
            }),
            PluginCommand::ListOrphanedPanes => Ok(ProtobufPluginCommand {
                name: CommandName::ListOrphanedPanes as i32,
                payload: None,
            }),
            PluginCommand::AdoptOrphanedPane { host_id, placement } => Ok(ProtobufPluginCommand {
                name: CommandName::AdoptOrphanedPane as i32,
                payload: Some(Payload::AdoptOrphanedPanePayload(
                    AdoptOrphanedPanePayload {
                        host_id,
                        placement: ProtobufWarpPlacement::from(placement) as i32,
                    },
                )),
            }),
            PluginCommand::DataDefineEntity {
                namespace,
                def,
                allow_destructive,
            } => Ok(ProtobufPluginCommand {
                name: CommandName::DataDefineEntity as i32,
                payload: Some(Payload::DataDefineEntityPayload(DataDefineEntityPayload {
                    namespace,
                    def: Some(def.into()),
                    allow_destructive,
                })),
            }),
            PluginCommand::DataApply {
                namespace,
                mutations,
            } => Ok(ProtobufPluginCommand {
                name: CommandName::DataApply as i32,
                payload: Some(Payload::DataApplyPayload(DataApplyPayload {
                    namespace,
                    mutations: mutations
                        .into_iter()
                        .map(ProtobufMutation::try_from)
                        .collect::<Result<Vec<_>, _>>()?,
                })),
            }),
            PluginCommand::DataFetch { request } => Ok(ProtobufPluginCommand {
                name: CommandName::DataFetch as i32,
                payload: Some(Payload::DataFetchPayload(DataFetchPayload {
                    request: Some(request.try_into()?),
                })),
            }),
            // `DataCount` reuses `DataFetchPayload`, since a count is a fetch whose rows are
            // discarded; the command name is what distinguishes them.
            PluginCommand::DataCount { request } => Ok(ProtobufPluginCommand {
                name: CommandName::DataCount as i32,
                payload: Some(Payload::DataCountPayload(DataFetchPayload {
                    request: Some(request.try_into()?),
                })),
            }),
            PluginCommand::DataWatch { request, mode } => Ok(ProtobufPluginCommand {
                name: CommandName::DataWatch as i32,
                payload: Some(Payload::DataWatchPayload(DataWatchPayload {
                    request: Some(request.try_into()?),
                    mode: ProtobufWatchMode::from(mode) as i32,
                })),
            }),
            PluginCommand::DataUnwatch { watch_id } => Ok(ProtobufPluginCommand {
                name: CommandName::DataUnwatch as i32,
                payload: Some(Payload::DataUnwatchPayload(DataUnwatchPayload { watch_id })),
            }),
            PluginCommand::EnqueueQueueJob {
                queue,
                plugin_url,
                config,
                context,
            } => Ok(ProtobufPluginCommand {
                name: CommandName::EnqueueQueueJob as i32,
                payload: Some(Payload::EnqueueQueueJobPayload(EnqueueQueueJobPayload {
                    queue,
                    plugin_url,
                    config: config
                        .into_iter()
                        .map(|(name, value)| ContextItem { name, value })
                        .collect(),
                    context: context
                        .into_iter()
                        .map(|(name, value)| ContextItem { name, value })
                        .collect(),
                })),
            }),
            PluginCommand::ConfigureQueue { queue, concurrency } => Ok(ProtobufPluginCommand {
                name: CommandName::ConfigureQueue as i32,
                payload: Some(Payload::ConfigureQueuePayload(ConfigureQueuePayload {
                    queue,
                    concurrency: concurrency.min(u32::MAX as usize) as u32,
                })),
            }),
            PluginCommand::CancelQueueJob { job_id } => Ok(ProtobufPluginCommand {
                name: CommandName::CancelQueueJob as i32,
                payload: Some(Payload::CancelQueueJobPayload(CancelQueueJobPayload {
                    job_id,
                })),
            }),
            PluginCommand::QueueStatus { queue } => Ok(ProtobufPluginCommand {
                name: CommandName::QueueStatus as i32,
                payload: Some(Payload::QueueStatusPayload(QueueStatusPayload { queue })),
            }),
            PluginCommand::FinishQueueJob { success, payload } => Ok(ProtobufPluginCommand {
                name: CommandName::FinishQueueJob as i32,
                payload: Some(Payload::FinishQueueJobPayload(FinishQueueJobPayload {
                    success,
                    payload,
                })),
            }),
            PluginCommand::RegisterDefaultKeybindings(keybindings) => Ok(ProtobufPluginCommand {
                name: CommandName::RegisterDefaultKeybindings as i32,
                payload: Some(Payload::RegisterDefaultKeybindingsPayload(keybindings)),
            }),
        }
    }
}

impl From<EnqueueQueueJobResponse> for ProtobufEnqueueQueueJobResponse {
    fn from(response: EnqueueQueueJobResponse) -> Self {
        ProtobufEnqueueQueueJobResponse {
            result: Some(match response {
                EnqueueQueueJobResponse::Ok(job_id) => {
                    enqueue_queue_job_response::Result::JobId(job_id)
                },
                EnqueueQueueJobResponse::Err(error) => {
                    enqueue_queue_job_response::Result::Error(error)
                },
            }),
        }
    }
}

impl TryFrom<ProtobufEnqueueQueueJobResponse> for EnqueueQueueJobResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufEnqueueQueueJobResponse) -> Result<Self, &'static str> {
        match protobuf.result {
            Some(enqueue_queue_job_response::Result::JobId(job_id)) => {
                Ok(EnqueueQueueJobResponse::Ok(job_id))
            },
            Some(enqueue_queue_job_response::Result::Error(error)) => {
                Ok(EnqueueQueueJobResponse::Err(error))
            },
            None => Err("EnqueueQueueJobResponse without a result"),
        }
    }
}

impl From<QueueStatusResponse> for ProtobufQueueStatusResponse {
    fn from(queues: QueueStatusResponse) -> Self {
        ProtobufQueueStatusResponse {
            queues: queues.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<ProtobufQueueStatusResponse> for QueueStatusResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufQueueStatusResponse) -> Result<Self, &'static str> {
        protobuf
            .queues
            .into_iter()
            .map(QueueSnapshot::try_from)
            .collect()
    }
}

impl From<WarpPaneResponse> for ProtobufWarpPaneResponse {
    fn from(response: WarpPaneResponse) -> Self {
        match response {
            WarpPaneResponse::Ok(message) => ProtobufWarpPaneResponse {
                result: Some(warp_pane_response::Result::Message(message)),
            },
            WarpPaneResponse::Err(error) => ProtobufWarpPaneResponse {
                result: Some(warp_pane_response::Result::Error(error)),
            },
        }
    }
}

impl TryFrom<ProtobufWarpPaneResponse> for WarpPaneResponse {
    type Error = &'static str;
    fn try_from(protobuf_response: ProtobufWarpPaneResponse) -> Result<Self, &'static str> {
        match protobuf_response.result {
            Some(warp_pane_response::Result::Message(message)) => Ok(WarpPaneResponse::Ok(message)),
            Some(warp_pane_response::Result::Error(error)) => Ok(WarpPaneResponse::Err(error)),
            None => Err("WarpPaneResponse without a result"),
        }
    }
}

impl From<OrphanedPane> for ProtobufOrphanedPane {
    fn from(pane: OrphanedPane) -> Self {
        // proto3 has no nested optional, so `Option<Option<i32>>` travels as a
        // flag plus a code: not exited, exited on a signal, exited with a status.
        let (exited, exit_code) = match pane.exit_status {
            None => (false, None),
            Some(code) => (true, code),
        };
        ProtobufOrphanedPane {
            host_id: pane.host_id,
            session_name: pane.session_name,
            command: pane.command,
            cwd: pane.cwd.map(|cwd| cwd.display().to_string()),
            child_pid: pane.child_pid,
            started_at_unix: pane.started_at_unix,
            exited,
            exit_code,
            buffered_bytes: pane.buffered_bytes,
        }
    }
}

impl From<ProtobufOrphanedPane> for OrphanedPane {
    fn from(pane: ProtobufOrphanedPane) -> Self {
        OrphanedPane {
            host_id: pane.host_id,
            session_name: pane.session_name,
            command: pane.command,
            cwd: pane.cwd.map(PathBuf::from),
            child_pid: pane.child_pid,
            started_at_unix: pane.started_at_unix,
            exit_status: if pane.exited {
                Some(pane.exit_code)
            } else {
                None
            },
            buffered_bytes: pane.buffered_bytes,
        }
    }
}

impl From<ListOrphanedPanesResponse> for ProtobufListOrphanedPanesResponse {
    fn from(response: ListOrphanedPanesResponse) -> Self {
        match response {
            ListOrphanedPanesResponse::Ok(panes) => ProtobufListOrphanedPanesResponse {
                result: Some(list_orphaned_panes_response::Result::Panes(
                    ProtobufOrphanedPaneList {
                        panes: panes.into_iter().map(Into::into).collect(),
                    },
                )),
            },
            ListOrphanedPanesResponse::Err(error) => ProtobufListOrphanedPanesResponse {
                result: Some(list_orphaned_panes_response::Result::Error(error)),
            },
        }
    }
}

impl TryFrom<ProtobufListOrphanedPanesResponse> for ListOrphanedPanesResponse {
    type Error = &'static str;
    fn try_from(
        protobuf_response: ProtobufListOrphanedPanesResponse,
    ) -> Result<Self, &'static str> {
        match protobuf_response.result {
            Some(list_orphaned_panes_response::Result::Panes(list)) => Ok(
                ListOrphanedPanesResponse::Ok(list.panes.into_iter().map(Into::into).collect()),
            ),
            Some(list_orphaned_panes_response::Result::Error(error)) => {
                Ok(ListOrphanedPanesResponse::Err(error))
            },
            None => Err("ListOrphanedPanesResponse without a result"),
        }
    }
}

// -------------------------------------------------------- data store responses

impl From<DataDefineResponse> for ProtobufDataDefineResponse {
    fn from(response: DataDefineResponse) -> Self {
        ProtobufDataDefineResponse {
            result: Some(match response {
                DataDefineResponse::Ok(info) => data_define_response::Result::Info(info.into()),
                DataDefineResponse::Err(error) => data_define_response::Result::Error(error),
            }),
        }
    }
}

impl TryFrom<ProtobufDataDefineResponse> for DataDefineResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufDataDefineResponse) -> Result<Self, &'static str> {
        match protobuf.result {
            Some(data_define_response::Result::Info(info)) => {
                Ok(DataDefineResponse::Ok(info.into()))
            },
            Some(data_define_response::Result::Error(error)) => Ok(DataDefineResponse::Err(error)),
            None => Err("DataDefineResponse without a result"),
        }
    }
}

impl From<DataApplyResponse> for ProtobufDataApplyResponse {
    fn from(response: DataApplyResponse) -> Self {
        ProtobufDataApplyResponse {
            result: Some(match response {
                DataApplyResponse::Ok(report) => data_apply_response::Result::Report(report.into()),
                DataApplyResponse::Err(error) => data_apply_response::Result::Error(error),
            }),
        }
    }
}

impl TryFrom<ProtobufDataApplyResponse> for DataApplyResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufDataApplyResponse) -> Result<Self, &'static str> {
        match protobuf.result {
            Some(data_apply_response::Result::Report(report)) => {
                Ok(DataApplyResponse::Ok(report.into()))
            },
            Some(data_apply_response::Result::Error(error)) => Ok(DataApplyResponse::Err(error)),
            None => Err("DataApplyResponse without a result"),
        }
    }
}

impl From<DataFetchResponse> for ProtobufDataFetchResponse {
    fn from(response: DataFetchResponse) -> Self {
        ProtobufDataFetchResponse {
            result: Some(match response {
                DataFetchResponse::Ok(rows) => data_fetch_response::Result::Rows(rows.into()),
                DataFetchResponse::Err(error) => data_fetch_response::Result::Error(error),
            }),
        }
    }
}

impl TryFrom<ProtobufDataFetchResponse> for DataFetchResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufDataFetchResponse) -> Result<Self, &'static str> {
        match protobuf.result {
            Some(data_fetch_response::Result::Rows(rows)) => Ok(DataFetchResponse::Ok(rows.into())),
            Some(data_fetch_response::Result::Error(error)) => Ok(DataFetchResponse::Err(error)),
            None => Err("DataFetchResponse without a result"),
        }
    }
}

impl From<DataCountResponse> for ProtobufDataCountResponse {
    fn from(response: DataCountResponse) -> Self {
        ProtobufDataCountResponse {
            result: Some(match response {
                DataCountResponse::Ok(count) => data_count_response::Result::Count(count),
                DataCountResponse::Err(error) => data_count_response::Result::Error(error),
            }),
        }
    }
}

impl TryFrom<ProtobufDataCountResponse> for DataCountResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufDataCountResponse) -> Result<Self, &'static str> {
        match protobuf.result {
            Some(data_count_response::Result::Count(count)) => Ok(DataCountResponse::Ok(count)),
            Some(data_count_response::Result::Error(error)) => Ok(DataCountResponse::Err(error)),
            None => Err("DataCountResponse without a result"),
        }
    }
}

impl From<DataWatchResponse> for ProtobufDataWatchResponse {
    fn from(response: DataWatchResponse) -> Self {
        ProtobufDataWatchResponse {
            result: Some(match response {
                DataWatchResponse::Ok(handle) => data_watch_response::Result::Handle(handle.into()),
                DataWatchResponse::Err(error) => data_watch_response::Result::Error(error),
            }),
        }
    }
}

impl TryFrom<ProtobufDataWatchResponse> for DataWatchResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufDataWatchResponse) -> Result<Self, &'static str> {
        match protobuf.result {
            Some(data_watch_response::Result::Handle(handle)) => {
                Ok(DataWatchResponse::Ok(handle.into()))
            },
            Some(data_watch_response::Result::Error(error)) => Ok(DataWatchResponse::Err(error)),
            None => Err("DataWatchResponse without a result"),
        }
    }
}

impl From<DataUnwatchResponse> for ProtobufDataUnwatchResponse {
    fn from(response: DataUnwatchResponse) -> Self {
        ProtobufDataUnwatchResponse {
            result: Some(match response {
                DataUnwatchResponse::Ok => data_unwatch_response::Result::Ok(true),
                DataUnwatchResponse::Err(error) => data_unwatch_response::Result::Error(error),
            }),
        }
    }
}

impl TryFrom<ProtobufDataUnwatchResponse> for DataUnwatchResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufDataUnwatchResponse) -> Result<Self, &'static str> {
        match protobuf.result {
            Some(data_unwatch_response::Result::Ok(_)) => Ok(DataUnwatchResponse::Ok),
            Some(data_unwatch_response::Result::Error(error)) => {
                Ok(DataUnwatchResponse::Err(error))
            },
            None => Err("DataUnwatchResponse without a result"),
        }
    }
}

// Conversion implementations for tab creation response types
use crate::data::{
    BreakPanesToNewTabResponse, BreakPanesToTabWithIdResponse, BreakPanesToTabWithIndexResponse,
    FocusOrCreateTabResponse, NewTabResponse, NewTabUnfocusedResponse, NewTabsResponse,
    NewTiledPaneInTabResponse, OpenCommandPaneBackgroundResponse,
    OpenCommandPaneFloatingNearPluginResponse, OpenCommandPaneFloatingResponse,
    OpenCommandPaneInPlaceOfPaneIdResponse, OpenCommandPaneInPlaceOfPluginResponse,
    OpenCommandPaneInPlaceResponse, OpenCommandPaneNearPluginResponse, OpenCommandPaneResponse,
    OpenEditPaneInPlaceOfPaneIdResponse, OpenFileFloatingNearPluginResponse,
    OpenFileFloatingResponse, OpenFileInPlaceOfPluginResponse, OpenFileInPlaceResponse,
    OpenFileNearPluginResponse, OpenFileResponse, OpenPaneInNewTabResponse,
    OpenPluginPaneFloatingResponse, OpenTerminalFloatingNearPluginResponse,
    OpenTerminalFloatingResponse, OpenTerminalInPlaceOfPluginResponse, OpenTerminalInPlaceResponse,
    OpenTerminalNearPluginResponse, OpenTerminalPaneInPlaceOfPaneIdResponse, OpenTerminalResponse,
};

impl TryFrom<ProtobufNewTabResponse> for NewTabResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufNewTabResponse) -> Result<Self, Self::Error> {
        match protobuf.result {
            Some(new_tab_response::Result::TabId(id)) => Ok(Some(id as usize)),
            Some(new_tab_response::Result::None(_)) | None => Ok(None),
        }
    }
}

impl From<NewTabResponse> for ProtobufNewTabResponse {
    fn from(response: NewTabResponse) -> Self {
        match response {
            Some(tab_id) => ProtobufNewTabResponse {
                result: Some(new_tab_response::Result::TabId(tab_id as u64)),
            },
            None => ProtobufNewTabResponse {
                result: Some(new_tab_response::Result::None(true)),
            },
        }
    }
}

impl TryFrom<ProtobufNewTabUnfocusedResponse> for NewTabUnfocusedResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufNewTabUnfocusedResponse) -> Result<Self, Self::Error> {
        match protobuf.result {
            Some(new_tab_unfocused_response::Result::TabId(id)) => Ok(Some(id as usize)),
            Some(new_tab_unfocused_response::Result::None(_)) | None => Ok(None),
        }
    }
}

impl From<NewTabUnfocusedResponse> for ProtobufNewTabUnfocusedResponse {
    fn from(response: NewTabUnfocusedResponse) -> Self {
        match response {
            Some(tab_id) => ProtobufNewTabUnfocusedResponse {
                result: Some(new_tab_unfocused_response::Result::TabId(tab_id as u64)),
            },
            None => ProtobufNewTabUnfocusedResponse {
                result: Some(new_tab_unfocused_response::Result::None(true)),
            },
        }
    }
}

impl TryFrom<ProtobufNewTiledPaneInTabResponse> for NewTiledPaneInTabResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufNewTiledPaneInTabResponse) -> Result<Self, Self::Error> {
        match protobuf.pane_id {
            Some(pane_id) => Ok(Some(pane_id.try_into()?)),
            None => Ok(None),
        }
    }
}

impl From<NewTiledPaneInTabResponse> for ProtobufNewTiledPaneInTabResponse {
    fn from(response: NewTiledPaneInTabResponse) -> Self {
        ProtobufNewTiledPaneInTabResponse {
            pane_id: response.map(|p| p.try_into().unwrap()),
        }
    }
}

impl TryFrom<ProtobufNewTabsResponse> for NewTabsResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufNewTabsResponse) -> Result<Self, Self::Error> {
        Ok(protobuf.tab_ids.into_iter().map(|id| id as usize).collect())
    }
}

impl From<NewTabsResponse> for ProtobufNewTabsResponse {
    fn from(response: NewTabsResponse) -> Self {
        ProtobufNewTabsResponse {
            tab_ids: response.into_iter().map(|id| id as u64).collect(),
        }
    }
}

impl TryFrom<ProtobufFocusOrCreateTabResponse> for FocusOrCreateTabResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufFocusOrCreateTabResponse) -> Result<Self, Self::Error> {
        match protobuf.result {
            Some(focus_or_create_tab_response::Result::TabId(id)) => Ok(Some(id as usize)),
            Some(focus_or_create_tab_response::Result::None(_)) | None => Ok(None),
        }
    }
}

impl From<FocusOrCreateTabResponse> for ProtobufFocusOrCreateTabResponse {
    fn from(response: FocusOrCreateTabResponse) -> Self {
        match response {
            Some(tab_id) => ProtobufFocusOrCreateTabResponse {
                result: Some(focus_or_create_tab_response::Result::TabId(tab_id as u64)),
            },
            None => ProtobufFocusOrCreateTabResponse {
                result: Some(focus_or_create_tab_response::Result::None(true)),
            },
        }
    }
}

impl TryFrom<ProtobufBreakPanesToNewTabResponse> for BreakPanesToNewTabResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufBreakPanesToNewTabResponse) -> Result<Self, Self::Error> {
        match protobuf.result {
            Some(break_panes_to_new_tab_response::Result::TabId(id)) => Ok(Some(id as usize)),
            Some(break_panes_to_new_tab_response::Result::None(_)) | None => Ok(None),
        }
    }
}

impl From<BreakPanesToNewTabResponse> for ProtobufBreakPanesToNewTabResponse {
    fn from(response: BreakPanesToNewTabResponse) -> Self {
        match response {
            Some(tab_id) => ProtobufBreakPanesToNewTabResponse {
                result: Some(break_panes_to_new_tab_response::Result::TabId(
                    tab_id as u64,
                )),
            },
            None => ProtobufBreakPanesToNewTabResponse {
                result: Some(break_panes_to_new_tab_response::Result::None(true)),
            },
        }
    }
}

impl TryFrom<ProtobufBreakPanesToTabWithIndexResponse> for BreakPanesToTabWithIndexResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufBreakPanesToTabWithIndexResponse) -> Result<Self, Self::Error> {
        match protobuf.result {
            Some(break_panes_to_tab_with_index_response::Result::TabId(id)) => {
                Ok(Some(id as usize))
            },
            Some(break_panes_to_tab_with_index_response::Result::None(_)) | None => Ok(None),
        }
    }
}

impl From<BreakPanesToTabWithIndexResponse> for ProtobufBreakPanesToTabWithIndexResponse {
    fn from(response: BreakPanesToTabWithIndexResponse) -> Self {
        match response {
            Some(tab_id) => ProtobufBreakPanesToTabWithIndexResponse {
                result: Some(break_panes_to_tab_with_index_response::Result::TabId(
                    tab_id as u64,
                )),
            },
            None => ProtobufBreakPanesToTabWithIndexResponse {
                result: Some(break_panes_to_tab_with_index_response::Result::None(true)),
            },
        }
    }
}

impl TryFrom<ProtobufBreakPanesToTabWithIdResponse> for BreakPanesToTabWithIdResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufBreakPanesToTabWithIdResponse) -> Result<Self, Self::Error> {
        match protobuf.result {
            Some(break_panes_to_tab_with_id_response::Result::TabId(id)) => Ok(Some(id as usize)),
            Some(break_panes_to_tab_with_id_response::Result::None(_)) | None => Ok(None),
        }
    }
}

impl From<BreakPanesToTabWithIdResponse> for ProtobufBreakPanesToTabWithIdResponse {
    fn from(response: BreakPanesToTabWithIdResponse) -> Self {
        match response {
            Some(tab_id) => ProtobufBreakPanesToTabWithIdResponse {
                result: Some(break_panes_to_tab_with_id_response::Result::TabId(
                    tab_id as u64,
                )),
            },
            None => ProtobufBreakPanesToTabWithIdResponse {
                result: Some(break_panes_to_tab_with_id_response::Result::None(true)),
            },
        }
    }
}

// Pane-creating command response conversions

impl TryFrom<ProtobufOpenFileResponse> for OpenFileResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufOpenFileResponse) -> Result<Self, Self::Error> {
        match protobuf.pane_id {
            Some(pane_id) => Ok(Some(pane_id.try_into()?)),
            None => Ok(None),
        }
    }
}

impl From<OpenFileResponse> for ProtobufOpenFileResponse {
    fn from(response: OpenFileResponse) -> Self {
        ProtobufOpenFileResponse {
            pane_id: response.map(|p| p.try_into().unwrap()),
        }
    }
}

impl TryFrom<ProtobufOpenFileFloatingResponse> for OpenFileFloatingResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufOpenFileFloatingResponse) -> Result<Self, Self::Error> {
        match protobuf.pane_id {
            Some(pane_id) => Ok(Some(pane_id.try_into()?)),
            None => Ok(None),
        }
    }
}

impl From<OpenFileFloatingResponse> for ProtobufOpenFileFloatingResponse {
    fn from(response: OpenFileFloatingResponse) -> Self {
        ProtobufOpenFileFloatingResponse {
            pane_id: response.map(|p| p.try_into().unwrap()),
        }
    }
}

impl TryFrom<ProtobufOpenFileInPlaceResponse> for OpenFileInPlaceResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufOpenFileInPlaceResponse) -> Result<Self, Self::Error> {
        match protobuf.pane_id {
            Some(pane_id) => Ok(Some(pane_id.try_into()?)),
            None => Ok(None),
        }
    }
}

impl From<OpenFileInPlaceResponse> for ProtobufOpenFileInPlaceResponse {
    fn from(response: OpenFileInPlaceResponse) -> Self {
        ProtobufOpenFileInPlaceResponse {
            pane_id: response.map(|p| p.try_into().unwrap()),
        }
    }
}

impl TryFrom<ProtobufOpenFileNearPluginResponse> for OpenFileNearPluginResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufOpenFileNearPluginResponse) -> Result<Self, Self::Error> {
        match protobuf.pane_id {
            Some(pane_id) => Ok(Some(pane_id.try_into()?)),
            None => Ok(None),
        }
    }
}

impl From<OpenFileNearPluginResponse> for ProtobufOpenFileNearPluginResponse {
    fn from(response: OpenFileNearPluginResponse) -> Self {
        ProtobufOpenFileNearPluginResponse {
            pane_id: response.map(|p| p.try_into().unwrap()),
        }
    }
}

impl TryFrom<ProtobufOpenFileFloatingNearPluginResponse> for OpenFileFloatingNearPluginResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufOpenFileFloatingNearPluginResponse) -> Result<Self, Self::Error> {
        match protobuf.pane_id {
            Some(pane_id) => Ok(Some(pane_id.try_into()?)),
            None => Ok(None),
        }
    }
}

impl From<OpenFileFloatingNearPluginResponse> for ProtobufOpenFileFloatingNearPluginResponse {
    fn from(response: OpenFileFloatingNearPluginResponse) -> Self {
        ProtobufOpenFileFloatingNearPluginResponse {
            pane_id: response.map(|p| p.try_into().unwrap()),
        }
    }
}

impl TryFrom<ProtobufOpenFileInPlaceOfPluginResponse> for OpenFileInPlaceOfPluginResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufOpenFileInPlaceOfPluginResponse) -> Result<Self, Self::Error> {
        match protobuf.pane_id {
            Some(pane_id) => Ok(Some(pane_id.try_into()?)),
            None => Ok(None),
        }
    }
}

impl From<OpenFileInPlaceOfPluginResponse> for ProtobufOpenFileInPlaceOfPluginResponse {
    fn from(response: OpenFileInPlaceOfPluginResponse) -> Self {
        ProtobufOpenFileInPlaceOfPluginResponse {
            pane_id: response.map(|p| p.try_into().unwrap()),
        }
    }
}

impl TryFrom<ProtobufOpenTerminalResponse> for OpenTerminalResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufOpenTerminalResponse) -> Result<Self, Self::Error> {
        match protobuf.pane_id {
            Some(pane_id) => Ok(Some(pane_id.try_into()?)),
            None => Ok(None),
        }
    }
}

impl From<OpenTerminalResponse> for ProtobufOpenTerminalResponse {
    fn from(response: OpenTerminalResponse) -> Self {
        ProtobufOpenTerminalResponse {
            pane_id: response.map(|p| p.try_into().unwrap()),
        }
    }
}

impl TryFrom<ProtobufOpenTerminalFloatingResponse> for OpenTerminalFloatingResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufOpenTerminalFloatingResponse) -> Result<Self, Self::Error> {
        match protobuf.pane_id {
            Some(pane_id) => Ok(Some(pane_id.try_into()?)),
            None => Ok(None),
        }
    }
}

impl From<OpenTerminalFloatingResponse> for ProtobufOpenTerminalFloatingResponse {
    fn from(response: OpenTerminalFloatingResponse) -> Self {
        ProtobufOpenTerminalFloatingResponse {
            pane_id: response.map(|p| p.try_into().unwrap()),
        }
    }
}

impl TryFrom<ProtobufOpenTerminalInPlaceResponse> for OpenTerminalInPlaceResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufOpenTerminalInPlaceResponse) -> Result<Self, Self::Error> {
        match protobuf.pane_id {
            Some(pane_id) => Ok(Some(pane_id.try_into()?)),
            None => Ok(None),
        }
    }
}

impl From<OpenTerminalInPlaceResponse> for ProtobufOpenTerminalInPlaceResponse {
    fn from(response: OpenTerminalInPlaceResponse) -> Self {
        ProtobufOpenTerminalInPlaceResponse {
            pane_id: response.map(|p| p.try_into().unwrap()),
        }
    }
}

impl TryFrom<ProtobufOpenTerminalNearPluginResponse> for OpenTerminalNearPluginResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufOpenTerminalNearPluginResponse) -> Result<Self, Self::Error> {
        match protobuf.pane_id {
            Some(pane_id) => Ok(Some(pane_id.try_into()?)),
            None => Ok(None),
        }
    }
}

impl From<OpenTerminalNearPluginResponse> for ProtobufOpenTerminalNearPluginResponse {
    fn from(response: OpenTerminalNearPluginResponse) -> Self {
        ProtobufOpenTerminalNearPluginResponse {
            pane_id: response.map(|p| p.try_into().unwrap()),
        }
    }
}

impl TryFrom<ProtobufOpenTerminalFloatingNearPluginResponse>
    for OpenTerminalFloatingNearPluginResponse
{
    type Error = &'static str;
    fn try_from(
        protobuf: ProtobufOpenTerminalFloatingNearPluginResponse,
    ) -> Result<Self, Self::Error> {
        match protobuf.pane_id {
            Some(pane_id) => Ok(Some(pane_id.try_into()?)),
            None => Ok(None),
        }
    }
}

impl From<OpenTerminalFloatingNearPluginResponse>
    for ProtobufOpenTerminalFloatingNearPluginResponse
{
    fn from(response: OpenTerminalFloatingNearPluginResponse) -> Self {
        ProtobufOpenTerminalFloatingNearPluginResponse {
            pane_id: response.map(|p| p.try_into().unwrap()),
        }
    }
}

impl TryFrom<ProtobufOpenTerminalInPlaceOfPluginResponse> for OpenTerminalInPlaceOfPluginResponse {
    type Error = &'static str;
    fn try_from(
        protobuf: ProtobufOpenTerminalInPlaceOfPluginResponse,
    ) -> Result<Self, Self::Error> {
        match protobuf.pane_id {
            Some(pane_id) => Ok(Some(pane_id.try_into()?)),
            None => Ok(None),
        }
    }
}

impl From<OpenTerminalInPlaceOfPluginResponse> for ProtobufOpenTerminalInPlaceOfPluginResponse {
    fn from(response: OpenTerminalInPlaceOfPluginResponse) -> Self {
        ProtobufOpenTerminalInPlaceOfPluginResponse {
            pane_id: response.map(|p| p.try_into().unwrap()),
        }
    }
}

impl TryFrom<ProtobufOpenCommandPaneResponse> for OpenCommandPaneResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufOpenCommandPaneResponse) -> Result<Self, Self::Error> {
        match protobuf.pane_id {
            Some(pane_id) => Ok(Some(pane_id.try_into()?)),
            None => Ok(None),
        }
    }
}

impl From<OpenCommandPaneResponse> for ProtobufOpenCommandPaneResponse {
    fn from(response: OpenCommandPaneResponse) -> Self {
        ProtobufOpenCommandPaneResponse {
            pane_id: response.map(|p| p.try_into().unwrap()),
        }
    }
}

impl From<OpenCommandPaneInGitCheckoutResponse> for ProtobufOpenCommandPaneInGitCheckoutResponse {
    fn from(response: OpenCommandPaneInGitCheckoutResponse) -> Self {
        let result = match response {
            Ok(pane_id) => open_command_pane_in_git_checkout_response::Result::PaneId(
                pane_id.try_into().unwrap(),
            ),
            Err(error) => open_command_pane_in_git_checkout_response::Result::Error(error),
        };
        Self {
            result: Some(result),
        }
    }
}

impl TryFrom<ProtobufOpenCommandPaneInGitCheckoutResponse>
    for OpenCommandPaneInGitCheckoutResponse
{
    type Error = &'static str;

    fn try_from(
        response: ProtobufOpenCommandPaneInGitCheckoutResponse,
    ) -> Result<Self, Self::Error> {
        match response.result {
            Some(open_command_pane_in_git_checkout_response::Result::PaneId(pane_id)) => {
                Ok(Ok(pane_id.try_into()?))
            },
            Some(open_command_pane_in_git_checkout_response::Result::Error(error)) => {
                Ok(Err(error))
            },
            None => Err("OpenCommandPaneInGitCheckoutResponse without a result"),
        }
    }
}

impl TryFrom<ProtobufOpenCommandPaneFloatingResponse> for OpenCommandPaneFloatingResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufOpenCommandPaneFloatingResponse) -> Result<Self, Self::Error> {
        match protobuf.pane_id {
            Some(pane_id) => Ok(Some(pane_id.try_into()?)),
            None => Ok(None),
        }
    }
}

impl From<OpenCommandPaneFloatingResponse> for ProtobufOpenCommandPaneFloatingResponse {
    fn from(response: OpenCommandPaneFloatingResponse) -> Self {
        ProtobufOpenCommandPaneFloatingResponse {
            pane_id: response.map(|p| p.try_into().unwrap()),
        }
    }
}

impl TryFrom<ProtobufOpenCommandPaneOverlayResponse> for OpenCommandPaneOverlayResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufOpenCommandPaneOverlayResponse) -> Result<Self, Self::Error> {
        match protobuf.pane_id {
            Some(pane_id) => Ok(Some(pane_id.try_into()?)),
            None => Ok(None),
        }
    }
}

impl From<OpenCommandPaneOverlayResponse> for ProtobufOpenCommandPaneOverlayResponse {
    fn from(response: OpenCommandPaneOverlayResponse) -> Self {
        ProtobufOpenCommandPaneOverlayResponse {
            pane_id: response.map(|p| p.try_into().unwrap()),
        }
    }
}

impl TryFrom<ProtobufOpenCommandPaneInPlaceResponse> for OpenCommandPaneInPlaceResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufOpenCommandPaneInPlaceResponse) -> Result<Self, Self::Error> {
        match protobuf.pane_id {
            Some(pane_id) => Ok(Some(pane_id.try_into()?)),
            None => Ok(None),
        }
    }
}

impl From<OpenCommandPaneInPlaceResponse> for ProtobufOpenCommandPaneInPlaceResponse {
    fn from(response: OpenCommandPaneInPlaceResponse) -> Self {
        ProtobufOpenCommandPaneInPlaceResponse {
            pane_id: response.map(|p| p.try_into().unwrap()),
        }
    }
}

impl TryFrom<ProtobufOpenCommandPaneNearPluginResponse> for OpenCommandPaneNearPluginResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufOpenCommandPaneNearPluginResponse) -> Result<Self, Self::Error> {
        match protobuf.pane_id {
            Some(pane_id) => Ok(Some(pane_id.try_into()?)),
            None => Ok(None),
        }
    }
}

impl From<OpenCommandPaneNearPluginResponse> for ProtobufOpenCommandPaneNearPluginResponse {
    fn from(response: OpenCommandPaneNearPluginResponse) -> Self {
        ProtobufOpenCommandPaneNearPluginResponse {
            pane_id: response.map(|p| p.try_into().unwrap()),
        }
    }
}

impl TryFrom<ProtobufOpenCommandPaneFloatingNearPluginResponse>
    for OpenCommandPaneFloatingNearPluginResponse
{
    type Error = &'static str;
    fn try_from(
        protobuf: ProtobufOpenCommandPaneFloatingNearPluginResponse,
    ) -> Result<Self, Self::Error> {
        match protobuf.pane_id {
            Some(pane_id) => Ok(Some(pane_id.try_into()?)),
            None => Ok(None),
        }
    }
}

impl From<OpenCommandPaneFloatingNearPluginResponse>
    for ProtobufOpenCommandPaneFloatingNearPluginResponse
{
    fn from(response: OpenCommandPaneFloatingNearPluginResponse) -> Self {
        ProtobufOpenCommandPaneFloatingNearPluginResponse {
            pane_id: response.map(|p| p.try_into().unwrap()),
        }
    }
}

impl TryFrom<ProtobufOpenCommandPaneInPlaceOfPluginResponse>
    for OpenCommandPaneInPlaceOfPluginResponse
{
    type Error = &'static str;
    fn try_from(
        protobuf: ProtobufOpenCommandPaneInPlaceOfPluginResponse,
    ) -> Result<Self, Self::Error> {
        match protobuf.pane_id {
            Some(pane_id) => Ok(Some(pane_id.try_into()?)),
            None => Ok(None),
        }
    }
}

impl From<OpenCommandPaneInPlaceOfPluginResponse>
    for ProtobufOpenCommandPaneInPlaceOfPluginResponse
{
    fn from(response: OpenCommandPaneInPlaceOfPluginResponse) -> Self {
        ProtobufOpenCommandPaneInPlaceOfPluginResponse {
            pane_id: response.map(|p| p.try_into().unwrap()),
        }
    }
}

impl TryFrom<ProtobufOpenCommandPaneBackgroundResponse> for OpenCommandPaneBackgroundResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufOpenCommandPaneBackgroundResponse) -> Result<Self, Self::Error> {
        match protobuf.pane_id {
            Some(pane_id) => Ok(Some(pane_id.try_into()?)),
            None => Ok(None),
        }
    }
}

impl From<OpenCommandPaneBackgroundResponse> for ProtobufOpenCommandPaneBackgroundResponse {
    fn from(response: OpenCommandPaneBackgroundResponse) -> Self {
        ProtobufOpenCommandPaneBackgroundResponse {
            pane_id: response.map(|p| p.try_into().unwrap()),
        }
    }
}

impl TryFrom<ProtobufOpenPaneInNewTabResponse> for OpenPaneInNewTabResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufOpenPaneInNewTabResponse) -> Result<Self, Self::Error> {
        let tab_id = protobuf.tab_id.map(|id| id as usize);
        let pane_id = match protobuf.pane_id {
            Some(pane_id) => Some(pane_id.try_into()?),
            None => None,
        };
        Ok(OpenPaneInNewTabResponse { tab_id, pane_id })
    }
}

impl From<OpenPaneInNewTabResponse> for ProtobufOpenPaneInNewTabResponse {
    fn from(response: OpenPaneInNewTabResponse) -> Self {
        ProtobufOpenPaneInNewTabResponse {
            tab_id: response.tab_id.map(|id| id as u64),
            pane_id: response.pane_id.map(|p| p.try_into().unwrap()),
        }
    }
}

impl TryFrom<ProtobufOpenCommandPaneInPlaceOfPaneIdResponse>
    for OpenCommandPaneInPlaceOfPaneIdResponse
{
    type Error = &'static str;
    fn try_from(
        protobuf: ProtobufOpenCommandPaneInPlaceOfPaneIdResponse,
    ) -> Result<Self, Self::Error> {
        match protobuf.pane_id {
            Some(pane_id) => Ok(Some(pane_id.try_into()?)),
            None => Ok(None),
        }
    }
}

impl From<OpenCommandPaneInPlaceOfPaneIdResponse>
    for ProtobufOpenCommandPaneInPlaceOfPaneIdResponse
{
    fn from(response: OpenCommandPaneInPlaceOfPaneIdResponse) -> Self {
        ProtobufOpenCommandPaneInPlaceOfPaneIdResponse {
            pane_id: response.map(|p| p.try_into().unwrap()),
        }
    }
}

impl TryFrom<ProtobufOpenTerminalPaneInPlaceOfPaneIdResponse>
    for OpenTerminalPaneInPlaceOfPaneIdResponse
{
    type Error = &'static str;
    fn try_from(
        protobuf: ProtobufOpenTerminalPaneInPlaceOfPaneIdResponse,
    ) -> Result<Self, Self::Error> {
        match protobuf.pane_id {
            Some(pane_id) => Ok(Some(pane_id.try_into()?)),
            None => Ok(None),
        }
    }
}

impl From<OpenTerminalPaneInPlaceOfPaneIdResponse>
    for ProtobufOpenTerminalPaneInPlaceOfPaneIdResponse
{
    fn from(response: OpenTerminalPaneInPlaceOfPaneIdResponse) -> Self {
        ProtobufOpenTerminalPaneInPlaceOfPaneIdResponse {
            pane_id: response.map(|p| p.try_into().unwrap()),
        }
    }
}

impl TryFrom<ProtobufOpenEditPaneInPlaceOfPaneIdResponse> for OpenEditPaneInPlaceOfPaneIdResponse {
    type Error = &'static str;
    fn try_from(
        protobuf: ProtobufOpenEditPaneInPlaceOfPaneIdResponse,
    ) -> Result<Self, Self::Error> {
        match protobuf.pane_id {
            Some(pane_id) => Ok(Some(pane_id.try_into()?)),
            None => Ok(None),
        }
    }
}

impl From<OpenEditPaneInPlaceOfPaneIdResponse> for ProtobufOpenEditPaneInPlaceOfPaneIdResponse {
    fn from(response: OpenEditPaneInPlaceOfPaneIdResponse) -> Self {
        ProtobufOpenEditPaneInPlaceOfPaneIdResponse {
            pane_id: response.map(|p| p.try_into().unwrap()),
        }
    }
}

impl TryFrom<ProtobufOpenPluginPaneFloatingResponse> for OpenPluginPaneFloatingResponse {
    type Error = &'static str;
    fn try_from(protobuf: ProtobufOpenPluginPaneFloatingResponse) -> Result<Self, Self::Error> {
        match protobuf.pane_id {
            Some(pane_id) => Ok(Some(pane_id.try_into()?)),
            None => Ok(None),
        }
    }
}

impl From<OpenPluginPaneFloatingResponse> for ProtobufOpenPluginPaneFloatingResponse {
    fn from(response: OpenPluginPaneFloatingResponse) -> Self {
        ProtobufOpenPluginPaneFloatingResponse {
            pane_id: response.map(|p| p.try_into().unwrap()),
        }
    }
}

impl From<ProtobufRecordingSummary> for RecordingSummary {
    fn from(protobuf: ProtobufRecordingSummary) -> Self {
        RecordingSummary {
            id: protobuf.id,
            session_name: protobuf.session_name,
            tab_name: protobuf.tab_name,
            terminal_id: protobuf.terminal_id,
            pane_title: protobuf.pane_title,
            started_at_us: protobuf.started_at_us,
            duration_us: protobuf.duration_us,
            is_running: protobuf.is_running,
            capture_input: protobuf.capture_input,
            error: protobuf.error,
        }
    }
}

impl From<ProtobufNotificationInfo> for NotificationInfo {
    fn from(notification: ProtobufNotificationInfo) -> Self {
        NotificationInfo {
            id: notification.id,
            message: notification.message,
            received_at_unix_ms: notification.received_at_unix_ms,
        }
    }
}

impl From<NotificationInfo> for ProtobufNotificationInfo {
    fn from(notification: NotificationInfo) -> Self {
        ProtobufNotificationInfo {
            id: notification.id,
            message: notification.message,
            received_at_unix_ms: notification.received_at_unix_ms,
        }
    }
}

impl From<RecordingSummary> for ProtobufRecordingSummary {
    fn from(summary: RecordingSummary) -> Self {
        ProtobufRecordingSummary {
            id: summary.id,
            session_name: summary.session_name,
            tab_name: summary.tab_name,
            terminal_id: summary.terminal_id,
            pane_title: summary.pane_title,
            started_at_us: summary.started_at_us,
            duration_us: summary.duration_us,
            is_running: summary.is_running,
            capture_input: summary.capture_input,
            error: summary.error,
        }
    }
}

impl From<ProtobufSearchHit> for SearchHit {
    fn from(protobuf: ProtobufSearchHit) -> Self {
        SearchHit {
            recording_id: protobuf.recording_id,
            session_name: protobuf.session_name,
            tab_name: protobuf.tab_name,
            pane_title: protobuf.pane_title,
            elapsed_us: protobuf.elapsed_us,
            started_at_us: protobuf.started_at_us,
            text: protobuf.text,
        }
    }
}

impl From<SearchHit> for ProtobufSearchHit {
    fn from(hit: SearchHit) -> Self {
        ProtobufSearchHit {
            recording_id: hit.recording_id,
            session_name: hit.session_name,
            tab_name: hit.tab_name,
            pane_title: hit.pane_title,
            elapsed_us: hit.elapsed_us,
            started_at_us: hit.started_at_us,
            text: hit.text,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{CommandToRun, GitCheckout, PluginCommand};

    #[test]
    fn default_keybindings_survive_the_wire() {
        let protobuf: ProtobufPluginCommand = PluginCommand::RegisterDefaultKeybindings(
            "keybinds { normal { bind \"x\" { Quit; }; }; }".to_owned(),
        )
        .try_into()
        .unwrap();
        let decoded: PluginCommand = protobuf.try_into().unwrap();
        assert!(
            matches!(decoded, PluginCommand::RegisterDefaultKeybindings(keybindings) if keybindings.contains("Quit"))
        );
    }

    #[test]
    fn git_checkout_command_survives_the_wire() {
        let command = PluginCommand::OpenCommandPaneInGitCheckoutNearPlugin(
            CommandToRun::new("codex"),
            PathBuf::from("/work/project"),
            GitCheckout::NewWorktree {
                branch: "topic".to_owned(),
                base: "main".to_owned(),
                path: PathBuf::from(".worktrees/topic"),
            },
        );
        let protobuf: ProtobufPluginCommand = command.try_into().unwrap();
        let decoded: PluginCommand = protobuf.try_into().unwrap();
        assert!(matches!(decoded,
            PluginCommand::OpenCommandPaneInGitCheckoutNearPlugin(_, source, GitCheckout::NewWorktree { branch, base, path })
            if source == *"/work/project" && branch == "topic" && base == "main" && path == *".worktrees/topic"));
    }

    #[test]
    fn notifications_with_and_without_panes_survive_the_wire() {
        for pane_id in [None, Some(PaneId::Terminal(7)), Some(PaneId::Plugin(3))] {
            let protobuf: ProtobufPluginCommand = PluginCommand::Notify("done".to_owned(), pane_id)
                .try_into()
                .expect("encode");
            assert_eq!(
                matches!(&protobuf.payload, Some(Payload::NotifyPayload(_))),
                pane_id.is_none(),
                "plain notifications keep the legacy string payload",
            );
            let decoded: PluginCommand = protobuf.try_into().expect("decode");
            match decoded {
                PluginCommand::Notify(text, decoded_pane_id) => {
                    assert_eq!(text, "done");
                    assert_eq!(decoded_pane_id, pane_id);
                },
                other => panic!("expected Notify, got {:?}", other),
            }
        }
    }

    #[test]
    fn set_pane_frame_style_protobuf_round_trip() {
        use crate::input::options::PaneFrameStyle;
        for style in [
            PaneFrameStyle::Full,
            PaneFrameStyle::Titles,
            PaneFrameStyle::None,
        ] {
            let original = PluginCommand::SetPaneFrameStyle(style);
            let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
            let decoded: PluginCommand = protobuf.try_into().expect("decode");
            match decoded {
                PluginCommand::SetPaneFrameStyle(decoded_style) => {
                    assert_eq!(decoded_style, style)
                },
                other => panic!("expected SetPaneFrameStyle, got {:?}", other),
            }
        }
    }

    /// Both halves have to survive: a reveal that loses its pane id points at
    /// nothing, and one that loses its label marks a pane without saying what
    /// for.
    #[test]
    fn reveal_pane_with_id_protobuf_round_trip() {
        for (pane_id, label) in [
            (PaneId::Terminal(7), Some("hocus: send_input?".to_owned())),
            (PaneId::Plugin(3), None),
        ] {
            let original = PluginCommand::RevealPaneWithId(pane_id, label.clone());
            let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
            let decoded: PluginCommand = protobuf.try_into().expect("decode");
            match decoded {
                PluginCommand::RevealPaneWithId(decoded_pane_id, decoded_label) => {
                    assert_eq!(decoded_pane_id, pane_id);
                    assert_eq!(decoded_label, label);
                },
                other => panic!("expected RevealPaneWithId, got {:?}", other),
            }
        }
    }

    /// `None` is a value here, not an absence: it is how the prefix is *dropped*,
    /// so an encoding that lost the difference between "no prefix" and "clear the
    /// prefix" would leave a stale checkout on a pane that is no longer an agent.
    #[test]
    fn set_pane_title_prefix_protobuf_round_trip() {
        for (pane_id, prefix) in [
            (PaneId::Terminal(7), Some("zellij:topic".to_owned())),
            (PaneId::Terminal(7), Some(String::new())),
            (PaneId::Plugin(3), None),
        ] {
            let original = PluginCommand::SetPaneTitlePrefix {
                pane_id,
                prefix: prefix.clone(),
            };
            let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
            let decoded: PluginCommand = protobuf.try_into().expect("decode");
            match decoded {
                PluginCommand::SetPaneTitlePrefix {
                    pane_id: decoded_pane_id,
                    prefix: decoded_prefix,
                } => {
                    assert_eq!(decoded_pane_id, pane_id);
                    assert_eq!(decoded_prefix, prefix);
                },
                other => panic!("expected SetPaneTitlePrefix, got {:?}", other),
            }
        }
    }

    #[test]
    fn new_pane_protobuf_round_trip() {
        let original = PluginCommand::NewPane;
        let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
        let decoded: PluginCommand = protobuf.try_into().expect("decode");
        match decoded {
            PluginCommand::NewPane => {},
            other => panic!("expected NewPane, got {:?}", other),
        }
    }

    #[test]
    fn toggle_floating_panes_focused_tab_protobuf_round_trip() {
        let original = PluginCommand::ToggleFloatingPanes { tab_id: None };
        let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
        let decoded: PluginCommand = protobuf.try_into().expect("decode");
        match decoded {
            PluginCommand::ToggleFloatingPanes { tab_id } => assert_eq!(tab_id, None),
            other => panic!("expected ToggleFloatingPanes, got {:?}", other),
        }
    }

    #[test]
    fn toggle_floating_panes_with_tab_id_protobuf_round_trip() {
        let original = PluginCommand::ToggleFloatingPanes { tab_id: Some(2) };
        let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
        let decoded: PluginCommand = protobuf.try_into().expect("decode");
        match decoded {
            PluginCommand::ToggleFloatingPanes { tab_id } => assert_eq!(tab_id, Some(2)),
            other => panic!("expected ToggleFloatingPanes, got {:?}", other),
        }
    }

    #[test]
    fn web_request_protobuf_round_trip() {
        use crate::data::HttpVerb;
        // None must survive as None: it is what keeps a plain web_request buffered,
        // and what keeps a request unproxied.
        for stream_id in [None, Some("stream-1".to_owned())] {
            for proxy in [None, Some("socks5://127.0.0.1:1080".to_owned())] {
                let original = PluginCommand::WebRequest(
                    "https://example.com/v1/chat/completions".to_owned(),
                    HttpVerb::Post,
                    BTreeMap::from([("authorization".to_owned(), "Bearer sk-test".to_owned())]),
                    b"{\"stream\":true}".to_vec(),
                    BTreeMap::from([("request_id".to_owned(), "7".to_owned())]),
                    stream_id.clone(),
                    proxy.clone(),
                );
                let protobuf: ProtobufPluginCommand = original.clone().try_into().expect("encode");
                let decoded: PluginCommand = protobuf.try_into().expect("decode");
                match (decoded, original) {
                    (
                        PluginCommand::WebRequest(
                            url,
                            _,
                            headers,
                            body,
                            context,
                            decoded_stream_id,
                            decoded_proxy,
                        ),
                        PluginCommand::WebRequest(
                            expected_url,
                            _,
                            expected_headers,
                            expected_body,
                            expected_context,
                            _,
                            _,
                        ),
                    ) => {
                        assert_eq!(url, expected_url);
                        assert_eq!(headers, expected_headers);
                        assert_eq!(body, expected_body);
                        assert_eq!(context, expected_context);
                        assert_eq!(decoded_stream_id, stream_id);
                        assert_eq!(decoded_proxy, proxy);
                    },
                    (other, _) => panic!("expected WebRequest, got {:?}", other),
                }
            }
        }
    }

    #[test]
    fn recording_commands_protobuf_round_trip() {
        let original = PluginCommand::SearchRecordings {
            query: "needle".to_owned(),
            limit: 7,
        };
        let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
        let decoded: PluginCommand = protobuf.try_into().expect("decode");
        match decoded {
            PluginCommand::SearchRecordings { query, limit } => {
                assert_eq!(query, "needle");
                assert_eq!(limit, 7);
            },
            other => panic!("expected SearchRecordings, got {:?}", other),
        }

        let protobuf: ProtobufPluginCommand =
            PluginCommand::ListRecordings.try_into().expect("encode");
        let decoded: PluginCommand = protobuf.try_into().expect("decode");
        assert!(matches!(decoded, PluginCommand::ListRecordings));
    }

    #[test]
    fn recording_results_protobuf_round_trip() {
        let summary = RecordingSummary {
            id: 3,
            session_name: "session".to_owned(),
            tab_name: "tab".to_owned(),
            terminal_id: 5,
            pane_title: "title".to_owned(),
            started_at_us: 1_000,
            duration_us: 2_000,
            is_running: true,
            capture_input: false,
            error: Some("boom".to_owned()),
        };
        let protobuf: ProtobufRecordingSummary = summary.clone().into();
        assert_eq!(RecordingSummary::from(protobuf), summary);

        let hit = SearchHit {
            recording_id: 3,
            session_name: "session".to_owned(),
            tab_name: "tab".to_owned(),
            pane_title: "title".to_owned(),
            elapsed_us: 42,
            started_at_us: 1_000,
            text: "needle in a haystack".to_owned(),
        };
        let protobuf: ProtobufSearchHit = hit.clone().into();
        assert_eq!(SearchHit::from(protobuf), hit);
    }

    #[test]
    fn list_notifications_protobuf_round_trip() {
        let protobuf: ProtobufPluginCommand =
            PluginCommand::ListNotifications.try_into().expect("encode");
        let decoded: PluginCommand = protobuf.try_into().expect("decode");
        assert!(matches!(decoded, PluginCommand::ListNotifications));

        let notification = NotificationInfo {
            id: 7,
            message: "build finished".to_owned(),
            received_at_unix_ms: 42,
        };
        let protobuf: ProtobufNotificationInfo = notification.clone().into();
        assert_eq!(NotificationInfo::from(protobuf), notification);
    }

    #[test]
    fn cancel_web_request_protobuf_round_trip() {
        let original = PluginCommand::CancelWebRequest("stream-1".to_owned());
        let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
        let decoded: PluginCommand = protobuf.try_into().expect("decode");
        match decoded {
            PluginCommand::CancelWebRequest(stream_id) => assert_eq!(stream_id, "stream-1"),
            other => panic!("expected CancelWebRequest, got {:?}", other),
        }
    }

    #[test]
    fn run_command_output_limit_protobuf_round_trip() {
        for max_output_bytes in [None, Some(40_000)] {
            let original = PluginCommand::RunCommand(
                vec!["git".to_owned(), "show".to_owned()],
                BTreeMap::new(),
                PathBuf::from("."),
                BTreeMap::new(),
                max_output_bytes,
            );
            let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
            let decoded: PluginCommand = protobuf.try_into().expect("decode");
            match decoded {
                PluginCommand::RunCommand(_, _, _, _, decoded_limit) => {
                    assert_eq!(decoded_limit, max_output_bytes)
                },
                other => panic!("expected RunCommand, got {:?}", other),
            }
        }
    }

    /// Every placement, every time: a mis-numbered proto enum is exactly the kind
    /// of mistake a single-value test walks straight past.
    const EVERY_PLACEMENT: [WarpPlacement; 3] = [
        WarpPlacement::NewTab,
        WarpPlacement::FocusedTab,
        WarpPlacement::Floating,
    ];

    #[test]
    fn warp_pane_to_session_protobuf_round_trip() {
        for placement in EVERY_PLACEMENT {
            let original = PluginCommand::WarpPaneToSession {
                pane_id: PaneId::Terminal(7),
                to_session: "other".to_owned(),
                placement,
            };
            let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
            let decoded: PluginCommand = protobuf.try_into().expect("decode");
            match decoded {
                PluginCommand::WarpPaneToSession {
                    pane_id,
                    to_session,
                    placement: decoded_placement,
                } => {
                    assert_eq!(pane_id, PaneId::Terminal(7));
                    assert_eq!(to_session, "other");
                    assert_eq!(decoded_placement, placement);
                },
                other => panic!("expected WarpPaneToSession, got {:?}", other),
            }
        }
    }

    #[test]
    fn warp_pane_from_session_protobuf_round_trip() {
        for placement in EVERY_PLACEMENT {
            let original = PluginCommand::WarpPaneFromSession {
                pane_id: PaneId::Terminal(11),
                from_session: "other".to_owned(),
                placement,
            };
            let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
            let decoded: PluginCommand = protobuf.try_into().expect("decode");
            match decoded {
                PluginCommand::WarpPaneFromSession {
                    pane_id,
                    from_session,
                    placement: decoded_placement,
                } => {
                    assert_eq!(pane_id, PaneId::Terminal(11));
                    assert_eq!(from_session, "other");
                    assert_eq!(decoded_placement, placement);
                },
                other => panic!("expected WarpPaneFromSession, got {:?}", other),
            }
        }
    }

    #[test]
    fn list_orphaned_panes_protobuf_round_trip() {
        let original = PluginCommand::ListOrphanedPanes;
        let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
        let decoded: PluginCommand = protobuf.try_into().expect("decode");
        match decoded {
            PluginCommand::ListOrphanedPanes => {},
            other => panic!("expected ListOrphanedPanes, got {:?}", other),
        }
    }

    #[test]
    fn adopt_orphaned_pane_protobuf_round_trip() {
        for placement in EVERY_PLACEMENT {
            let original = PluginCommand::AdoptOrphanedPane {
                host_id: "abc123".to_owned(),
                placement,
            };
            let protobuf: ProtobufPluginCommand = original.try_into().expect("encode");
            let decoded: PluginCommand = protobuf.try_into().expect("decode");
            match decoded {
                PluginCommand::AdoptOrphanedPane {
                    host_id,
                    placement: decoded_placement,
                } => {
                    assert_eq!(host_id, "abc123");
                    assert_eq!(decoded_placement, placement);
                },
                other => panic!("expected AdoptOrphanedPane, got {:?}", other),
            }
        }
    }

    #[test]
    fn a_warp_placement_from_a_newer_api_is_rejected_rather_than_defaulted() {
        // Defaulting an unknown placement to NewTab would put the pane somewhere
        // the caller did not ask for, with no way for it to find out.
        assert!(warp_placement_from_i32(99).is_err());
    }

    #[test]
    fn warp_pane_response_protobuf_round_trip() {
        for response in [
            WarpPaneResponse::Ok("Moved pane to session 'other'".to_owned()),
            WarpPaneResponse::Err("this is the session's only pane".to_owned()),
        ] {
            let protobuf: ProtobufWarpPaneResponse = response.clone().into();
            assert_eq!(
                WarpPaneResponse::try_from(protobuf).expect("decode"),
                response
            );
        }
    }

    /// The three exit states an orphan can be in. Flattening `Option<Option<i32>>`
    /// into a flag plus a code is the one place in this conversion where
    /// information could silently go missing.
    #[test]
    fn an_orphaned_panes_exit_state_survives_the_wire() {
        for exit_status in [None, Some(None), Some(Some(1))] {
            let pane = OrphanedPane {
                host_id: "abc123".to_owned(),
                session_name: Some("gone".to_owned()),
                command: "top".to_owned(),
                cwd: Some(PathBuf::from("/tmp")),
                child_pid: Some(4242),
                started_at_unix: 1_700_000_000,
                exit_status,
                buffered_bytes: 4096,
            };
            let protobuf: ProtobufOrphanedPane = pane.clone().into();
            assert_eq!(OrphanedPane::from(protobuf), pane);
        }
    }

    #[test]
    fn list_orphaned_panes_response_protobuf_round_trip() {
        for response in [
            ListOrphanedPanesResponse::Ok(vec![]),
            ListOrphanedPanesResponse::Ok(vec![OrphanedPane {
                host_id: "abc123".to_owned(),
                command: "top".to_owned(),
                ..Default::default()
            }]),
            ListOrphanedPanesResponse::Err("not supported here".to_owned()),
        ] {
            let protobuf: ProtobufListOrphanedPanesResponse = response.clone().into();
            assert_eq!(
                ListOrphanedPanesResponse::try_from(protobuf).expect("decode"),
                response
            );
        }
    }
}
