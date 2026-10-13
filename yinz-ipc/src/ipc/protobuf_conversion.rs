use std::str::FromStr;
use yinz_types::internal_ids::ProcessId;
use yinz_types::internal_ids::SupervisorId;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabNumber;
use yinz_types::internal_ids::TabPosition;

use std::collections::BTreeMap;
use std::path::PathBuf;
use {
    crate::client_server_contract::client_server_contract::client_to_server_msg,
    crate::client_server_contract::client_server_contract::server_to_client_msg,
    crate::client_server_contract::client_server_contract::ActionMsg,
    crate::client_server_contract::client_server_contract::AgentReportAcceptedMsg,
    crate::client_server_contract::client_server_contract::AgentReportMsg,
    crate::client_server_contract::client_server_contract::AttachClientMsg,
    crate::client_server_contract::client_server_contract::AttachWatcherClientMsg,
    crate::client_server_contract::client_server_contract::BackgroundColorMsg,
    crate::client_server_contract::client_server_contract::CliPipeOutputMsg,
    crate::client_server_contract::client_server_contract::ClientExitedMsg,
    crate::client_server_contract::client_server_contract::ClientToServerMsg as ProtoClientToServerMsg,
    crate::client_server_contract::client_server_contract::ColorRegistersMsg,
    crate::client_server_contract::client_server_contract::ConfigFileUpdatedMsg,
    crate::client_server_contract::client_server_contract::ConnStatusMsg,
    crate::client_server_contract::client_server_contract::ConnectedMsg,
    crate::client_server_contract::client_server_contract::DesktopNotificationResponseMsg,
    crate::client_server_contract::client_server_contract::DetachSessionMsg,
    crate::client_server_contract::client_server_contract::EmitNestedSessionFrameMsg,
    crate::client_server_contract::client_server_contract::EventUpdateMsg,
    crate::client_server_contract::client_server_contract::ExitMsg,
    crate::client_server_contract::client_server_contract::ExitReason as ProtoExitReason,
    crate::client_server_contract::client_server_contract::FirstClientConnectedMsg,
    crate::client_server_contract::client_server_contract::ForegroundColorMsg,
    crate::client_server_contract::client_server_contract::ForwardQueryToHostMsg,
    crate::client_server_contract::client_server_contract::ForwardedReplyFromHostMsg,
    crate::client_server_contract::client_server_contract::HostTerminalFocusChangedMsg,
    crate::client_server_contract::client_server_contract::HostTerminalThemeChangedMsg,
    crate::client_server_contract::client_server_contract::HostTerminalThemeIndication as ProtoHostTerminalThemeIndication,
    crate::client_server_contract::client_server_contract::InputMode as ProtoInputMode,
    crate::client_server_contract::client_server_contract::KeyMsg,
    crate::client_server_contract::client_server_contract::KillSessionCompleteMsg,
    crate::client_server_contract::client_server_contract::KillSessionMsg,
    crate::client_server_contract::client_server_contract::KittyGraphicsSupportMsg,
    crate::client_server_contract::client_server_contract::LayoutMetadata as ProtoLayoutMetadata,
    crate::client_server_contract::client_server_contract::LogErrorMsg,
    crate::client_server_contract::client_server_contract::LogMsg,
    crate::client_server_contract::client_server_contract::ModifiersHeldMsg,
    crate::client_server_contract::client_server_contract::NestedSessionFrameFromHostMsg,
    crate::client_server_contract::client_server_contract::PaneMetadata as ProtoPaneMetadata,
    crate::client_server_contract::client_server_contract::PaneRenderUpdateMsg,
    crate::client_server_contract::client_server_contract::QueryTerminalSizeMsg,
    crate::client_server_contract::client_server_contract::RenamedSessionMsg,
    crate::client_server_contract::client_server_contract::RenderMsg,
    crate::client_server_contract::client_server_contract::RequestSessionListMsg,
    crate::client_server_contract::client_server_contract::ServerToClientMsg as ProtoServerToClientMsg,
    crate::client_server_contract::client_server_contract::SixelSupportMsg,
    crate::client_server_contract::client_server_contract::SubscribeToEventsMsg,
    crate::client_server_contract::client_server_contract::SubscribeToPaneRendersMsg,
    crate::client_server_contract::client_server_contract::SubscribedPaneClosedMsg,
    crate::client_server_contract::client_server_contract::SwitchSessionMsg,
    crate::client_server_contract::client_server_contract::SynchronizedOutputSupportMsg,
    crate::client_server_contract::client_server_contract::TabMetadata as ProtoTabMetadata,
    crate::client_server_contract::client_server_contract::TerminalPixelDimensionsMsg,
    crate::client_server_contract::client_server_contract::TerminalResizeMsg,
    crate::client_server_contract::client_server_contract::UnblockCliPipeInputMsg,
    crate::client_server_contract::client_server_contract::UnblockInputThreadMsg,
    crate::client_server_contract::client_server_contract::WarpPlacement as ProtobufWarpPlacement,
    crate::ipc::ClientToServerMsg, crate::ipc::ColorRegister, crate::ipc::ExitReason,
    crate::ipc::PixelDimensions, crate::ipc::ServerToClientMsg, yinz_runtime::errors::prelude::*,
    yinz_types::data::ClientId, yinz_types::data::HostTerminalThemeMode,
    yinz_types::data::InputMode, yinz_types::data::PaneId,
};

/// Converts `ClientToServerMsg` to its protobuf representation.
/// # Errors
/// Returns `anyhow::Error` if the contained `Action` fails to convert.
/// This changed from `From` (infallible) at base `5254e4fc` to `TryFrom` in this branch; external callers must now use `try_into()?` / `TryFrom::try_from` and handle the error (see `yinz-ipc/src/ipc.rs` for internal handling).
// Convert Rust ClientToServerMsg to protobuf
impl TryFrom<ClientToServerMsg> for ProtoClientToServerMsg {
    type Error = anyhow::Error;

    fn try_from(msg: ClientToServerMsg) -> Result<Self> {
        let message = match msg {
            ClientToServerMsg::AgentControl(request) => {
                client_to_server_msg::Message::AgentControlJson(
                    serde_json::to_string(&request).context("encode agent request")?,
                )
            },
            ClientToServerMsg::PluginRequest {
                request_id,
                request,
            } => client_to_server_msg::Message::PluginRequest(
                crate::client_server_contract::client_server_contract::DataRequestMsg {
                    request_id,
                    request_json: serde_json::to_string(&request).context("encode data request")?,
                },
            ),
            ClientToServerMsg::DataRequest {
                request_id,
                request,
            } => client_to_server_msg::Message::DataRequest(
                crate::client_server_contract::client_server_contract::DataRequestMsg {
                    request_id,
                    request_json: serde_json::to_string(&request).context("encode data request")?,
                },
            ),
            ClientToServerMsg::AgentReport(report) => {
                use yinz_types::agent::report::AgentReportState;
                let (state, agent) = match report.state {
                    AgentReportState::Unchanged => (0, None),
                    AgentReportState::Clear => (1, None),
                    AgentReportState::Set(agent) => (2, Some(agent_info_to_proto(agent))),
                };
                client_to_server_msg::Message::AgentReport(AgentReportMsg {
                    report_id: report.report_id,
                    pane_id: Some(report.pane_id.into()),
                    state,
                    agent,
                    event: report.event.map(agent_event_to_proto),
                    trace: report.trace,
                    process_id: report.process_id.get(),
                    kind: report.kind,
                })
            },
            ClientToServerMsg::DetachSession { client_ids } => {
                client_to_server_msg::Message::DetachSession(DetachSessionMsg {
                    client_ids: client_ids
                        .into_iter()
                        .map(|id| u32::from(id.get()))
                        .collect(),
                })
            },
            ClientToServerMsg::TerminalPixelDimensions { pixel_dimensions } => {
                client_to_server_msg::Message::TerminalPixelDimensions(TerminalPixelDimensionsMsg {
                    pixel_dimensions: Some(pixel_dimensions.into()),
                })
            },
            ClientToServerMsg::BackgroundColor { color } => {
                client_to_server_msg::Message::BackgroundColor(BackgroundColorMsg { color })
            },
            ClientToServerMsg::ForegroundColor { color } => {
                client_to_server_msg::Message::ForegroundColor(ForegroundColorMsg { color })
            },
            ClientToServerMsg::ColorRegisters { color_registers } => {
                client_to_server_msg::Message::ColorRegisters(ColorRegistersMsg {
                    color_registers: color_registers.into_iter().map(|cr| cr.into()).collect(),
                })
            },
            ClientToServerMsg::TerminalResize { new_size } => {
                client_to_server_msg::Message::TerminalResize(TerminalResizeMsg {
                    new_size: Some(new_size.into()),
                })
            },
            ClientToServerMsg::FirstClientConnected { cli_assets } => {
                client_to_server_msg::Message::FirstClientConnected(FirstClientConnectedMsg {
                    cli_assets: Some(cli_assets.into()),
                })
            },
            ClientToServerMsg::AttachClient {
                cli_assets,
                tab_number_to_focus,
                pane_to_focus,
            } => client_to_server_msg::Message::AttachClient(AttachClientMsg {
                cli_assets: Some(cli_assets.into()),
                tab_number_to_focus: tab_number_to_focus.map(|pos| pos.get()),
                pane_to_focus: pane_to_focus.map(|p| p.into()),
            }),
            ClientToServerMsg::AttachWatcherClient { terminal_size } => {
                client_to_server_msg::Message::AttachWatcherClient(AttachWatcherClientMsg {
                    terminal_size: Some(terminal_size.into()),
                })
            },
            ClientToServerMsg::Action {
                action,
                terminal_id,
                client_id,
                is_cli_client,
            } => client_to_server_msg::Message::Action(ActionMsg {
                action: Some(action.try_into()?),
                terminal_id: terminal_id.map(|id| id.get()),
                client_id: client_id.map(|id| u32::from(id.get())),
                is_cli_client,
            }),
            ClientToServerMsg::Paste(text) => client_to_server_msg::Message::Paste(text),
            ClientToServerMsg::Key {
                key,
                raw_bytes,
                is_kitty_keyboard_protocol,
            } => client_to_server_msg::Message::Key(KeyMsg {
                key: Some(key.into()),
                raw_bytes: raw_bytes.into_iter().map(|b| b as u32).collect(),
                is_kitty_keyboard_protocol,
            }),
            ClientToServerMsg::ClientExited => {
                client_to_server_msg::Message::ClientExited(ClientExitedMsg {})
            },
            ClientToServerMsg::KillSessionWithCompletion => {
                client_to_server_msg::Message::KillSessionWithCompletion(KillSessionMsg {})
            },
            ClientToServerMsg::KillSession => {
                client_to_server_msg::Message::KillSession(KillSessionMsg {})
            },
            ClientToServerMsg::ConnStatus => {
                client_to_server_msg::Message::ConnStatus(ConnStatusMsg {})
            },
            ClientToServerMsg::SubscribeToPaneRenders {
                pane_ids,
                scrollback,
                ansi,
            } => client_to_server_msg::Message::SubscribeToPaneRenders(SubscribeToPaneRendersMsg {
                pane_ids: pane_ids.into_iter().map(|id| id.into()).collect(),
                scrollback: scrollback.map(|s| s as u32),
                ansi,
            }),
            ClientToServerMsg::SubscribeToEvents {
                event_types,
                options,
            } => client_to_server_msg::Message::SubscribeToEvents(SubscribeToEventsMsg {
                event_types,
                panes: options.panes,
                projects: options.projects,
                kinds: options.kinds,
                lifecycle_types: options.lifecycle_types,
                cursor: options.cursor,
            }),
            ClientToServerMsg::DesktopNotificationResponse { raw_bytes } => {
                client_to_server_msg::Message::DesktopNotificationResponse(
                    DesktopNotificationResponseMsg { raw_bytes },
                )
            },
            ClientToServerMsg::ForwardedReplyFromHost { token, reply_bytes } => {
                client_to_server_msg::Message::ForwardedReplyFromHost(ForwardedReplyFromHostMsg {
                    token,
                    reply_bytes,
                })
            },
            ClientToServerMsg::HostTerminalThemeChanged { mode } => {
                let proto_mode: ProtoHostTerminalThemeIndication = mode.into();
                client_to_server_msg::Message::HostTerminalThemeChanged(
                    HostTerminalThemeChangedMsg {
                        mode: proto_mode as i32,
                    },
                )
            },
            ClientToServerMsg::NestedSessionFrameFromHost { payload_bytes } => {
                client_to_server_msg::Message::NestedSessionFrameFromHost(
                    NestedSessionFrameFromHostMsg { payload_bytes },
                )
            },
            ClientToServerMsg::KittyGraphicsSupport { supported } => {
                client_to_server_msg::Message::KittyGraphicsSupport(KittyGraphicsSupportMsg {
                    supported,
                })
            },
            ClientToServerMsg::SixelSupport { supported } => {
                client_to_server_msg::Message::SixelSupport(SixelSupportMsg { supported })
            },
            ClientToServerMsg::SynchronizedOutputSupport { supported } => {
                client_to_server_msg::Message::SynchronizedOutputSupport(
                    SynchronizedOutputSupportMsg { supported },
                )
            },
            ClientToServerMsg::RequestSessionList => {
                client_to_server_msg::Message::RequestSessionList(RequestSessionListMsg {})
            },
            ClientToServerMsg::HostTerminalFocusChanged { focused } => {
                client_to_server_msg::Message::HostTerminalFocusChanged(
                    HostTerminalFocusChangedMsg { focused },
                )
            },
            ClientToServerMsg::ModifiersHeld { modifiers } => {
                client_to_server_msg::Message::ModifiersHeld(ModifiersHeldMsg {
                    modifiers: modifiers
                        .into_iter()
                        .map(crate::ipc::enum_conversions::key_modifier_to_proto_i32)
                        .collect(),
                })
            },
        };

        Ok(ProtoClientToServerMsg {
            message: Some(message),
        })
    }
}

// Convert protobuf ClientToServerMsg to Rust
impl TryFrom<ProtoClientToServerMsg> for ClientToServerMsg {
    type Error = anyhow::Error;

    fn try_from(msg: ProtoClientToServerMsg) -> Result<Self> {
        match msg.message {
            Some(client_to_server_msg::Message::AgentControlJson(json)) => {
                Ok(ClientToServerMsg::AgentControl(
                    serde_json::from_str(&json).context("decode agent request")?,
                ))
            },
            Some(client_to_server_msg::Message::PluginRequest(msg)) => {
                Ok(ClientToServerMsg::PluginRequest {
                    request_id: msg.request_id,
                    request: serde_json::from_str(&msg.request_json)
                        .context("decode data request")?,
                })
            },
            Some(client_to_server_msg::Message::DataRequest(msg)) => {
                Ok(ClientToServerMsg::DataRequest {
                    request_id: msg.request_id,
                    request: serde_json::from_str(&msg.request_json)
                        .context("decode data request")?,
                })
            },
            Some(client_to_server_msg::Message::AgentReport(msg)) => {
                use {
                    yinz_types::agent::report::AgentReport,
                    yinz_types::agent::report::AgentReportState,
                };
                let state = match (msg.state, msg.agent) {
                    (0, None) => AgentReportState::Unchanged,
                    (1, None) => AgentReportState::Clear,
                    (2, Some(agent)) => AgentReportState::Set(agent_info_from_proto(agent)?),
                    _ => return Err(anyhow!("invalid agent report state")),
                };
                let report = AgentReport {
                    report_id: msg.report_id,
                    pane_id: msg
                        .pane_id
                        .context("agent report missing pane")?
                        .try_into()?,
                    state,
                    event: msg.event.map(agent_event_from_proto).transpose()?,
                    trace: msg.trace,
                    process_id: ProcessId::new(msg.process_id),
                    kind: msg.kind,
                };
                yinz_runtime::agent::report::validate_report(&report)?;
                Ok(ClientToServerMsg::AgentReport(report))
            },
            Some(client_to_server_msg::Message::DetachSession(detach)) => {
                Ok(ClientToServerMsg::DetachSession {
                    client_ids: detach
                        .client_ids
                        .into_iter()
                        .map(ClientId::try_from)
                        .collect::<std::result::Result<_, _>>()?,
                })
            },
            Some(client_to_server_msg::Message::TerminalPixelDimensions(pixel_dims)) => {
                Ok(ClientToServerMsg::TerminalPixelDimensions {
                    pixel_dimensions: pixel_dims
                        .pixel_dimensions
                        .ok_or_else(|| anyhow!("Missing pixel_dimensions"))?
                        .try_into()?,
                })
            },
            Some(client_to_server_msg::Message::BackgroundColor(bg_color)) => {
                Ok(ClientToServerMsg::BackgroundColor {
                    color: bg_color.color,
                })
            },
            Some(client_to_server_msg::Message::ForegroundColor(fg_color)) => {
                Ok(ClientToServerMsg::ForegroundColor {
                    color: fg_color.color,
                })
            },
            Some(client_to_server_msg::Message::ColorRegisters(color_regs)) => {
                Ok(ClientToServerMsg::ColorRegisters {
                    color_registers: color_regs
                        .color_registers
                        .into_iter()
                        .map(|cr| cr.try_into())
                        .collect::<Result<Vec<_>>>()?,
                })
            },
            Some(client_to_server_msg::Message::TerminalResize(resize)) => {
                Ok(ClientToServerMsg::TerminalResize {
                    new_size: resize
                        .new_size
                        .ok_or_else(|| anyhow!("Missing new_size"))?
                        .try_into()?,
                })
            },
            Some(client_to_server_msg::Message::FirstClientConnected(first_client)) => {
                Ok(ClientToServerMsg::FirstClientConnected {
                    cli_assets: first_client
                        .cli_assets
                        .ok_or_else(|| anyhow!("Missing cli_assets"))?
                        .try_into()?,
                })
            },
            Some(client_to_server_msg::Message::AttachClient(attach)) => {
                Ok(ClientToServerMsg::AttachClient {
                    cli_assets: attach
                        .cli_assets
                        .ok_or_else(|| anyhow!("Missing cli_assets"))?
                        .try_into()?,
                    tab_number_to_focus: attach.tab_number_to_focus.map(TabNumber::new),
                    pane_to_focus: attach.pane_to_focus.map(Into::into),
                })
            },
            Some(client_to_server_msg::Message::AttachWatcherClient(attach_watcher)) => {
                Ok(ClientToServerMsg::AttachWatcherClient {
                    terminal_size: attach_watcher
                        .terminal_size
                        .ok_or_else(|| anyhow::anyhow!("Missing terminal_size"))?
                        .try_into()?,
                })
            },
            Some(client_to_server_msg::Message::Action(action)) => Ok(ClientToServerMsg::Action {
                action: action
                    .action
                    .ok_or_else(|| anyhow!("Missing action"))?
                    .try_into()?,
                terminal_id: action
                    .terminal_id
                    .map(yinz_types::internal_ids::TerminalId::new),
                client_id: action.client_id.map(ClientId::try_from).transpose()?,
                is_cli_client: action.is_cli_client,
            }),
            Some(client_to_server_msg::Message::Paste(text)) => Ok(ClientToServerMsg::Paste(text)),
            Some(client_to_server_msg::Message::Key(key)) => Ok(ClientToServerMsg::Key {
                key: key.key.ok_or_else(|| anyhow!("Missing key"))?.try_into()?,
                raw_bytes: key.raw_bytes.into_iter().map(|b| b as u8).collect(),
                is_kitty_keyboard_protocol: key.is_kitty_keyboard_protocol,
            }),
            Some(client_to_server_msg::Message::ClientExited(_)) => {
                Ok(ClientToServerMsg::ClientExited)
            },
            Some(client_to_server_msg::Message::KillSessionWithCompletion(_)) => {
                Ok(ClientToServerMsg::KillSessionWithCompletion)
            },
            Some(client_to_server_msg::Message::KillSession(_)) => {
                Ok(ClientToServerMsg::KillSession)
            },
            Some(client_to_server_msg::Message::ConnStatus(_)) => Ok(ClientToServerMsg::ConnStatus),
            Some(client_to_server_msg::Message::SubscribeToPaneRenders(msg)) => {
                let pane_ids: Result<Vec<PaneId>> =
                    msg.pane_ids.into_iter().map(|p| p.try_into()).collect();
                Ok(ClientToServerMsg::SubscribeToPaneRenders {
                    pane_ids: pane_ids?,
                    scrollback: msg.scrollback.map(|s| s as usize),
                    ansi: msg.ansi,
                })
            },
            Some(client_to_server_msg::Message::SubscribeToEvents(msg)) => {
                Ok(ClientToServerMsg::SubscribeToEvents {
                    event_types: msg.event_types,
                    options: super::EventSubscription {
                        panes: msg.panes,
                        projects: msg.projects,
                        kinds: msg.kinds,
                        lifecycle_types: msg.lifecycle_types,
                        cursor: msg.cursor,
                    },
                })
            },
            Some(client_to_server_msg::Message::DesktopNotificationResponse(msg)) => {
                Ok(ClientToServerMsg::DesktopNotificationResponse {
                    raw_bytes: msg.raw_bytes,
                })
            },
            Some(client_to_server_msg::Message::ForwardedReplyFromHost(msg)) => {
                Ok(ClientToServerMsg::ForwardedReplyFromHost {
                    token: msg.token,
                    reply_bytes: msg.reply_bytes,
                })
            },
            Some(client_to_server_msg::Message::HostTerminalThemeChanged(msg)) => {
                let proto_mode = ProtoHostTerminalThemeIndication::try_from(msg.mode)
                    .map_err(|_| anyhow!("Unknown HostTerminalThemeIndication: {}", msg.mode))?;
                Ok(ClientToServerMsg::HostTerminalThemeChanged {
                    mode: proto_mode.into(),
                })
            },
            Some(client_to_server_msg::Message::NestedSessionFrameFromHost(msg)) => {
                Ok(ClientToServerMsg::NestedSessionFrameFromHost {
                    payload_bytes: msg.payload_bytes,
                })
            },
            Some(client_to_server_msg::Message::KittyGraphicsSupport(msg)) => {
                Ok(ClientToServerMsg::KittyGraphicsSupport {
                    supported: msg.supported,
                })
            },
            Some(client_to_server_msg::Message::SixelSupport(msg)) => {
                Ok(ClientToServerMsg::SixelSupport {
                    supported: msg.supported,
                })
            },
            Some(client_to_server_msg::Message::SynchronizedOutputSupport(msg)) => {
                Ok(ClientToServerMsg::SynchronizedOutputSupport {
                    supported: msg.supported,
                })
            },
            Some(client_to_server_msg::Message::RequestSessionList(_)) => {
                Ok(ClientToServerMsg::RequestSessionList)
            },
            Some(client_to_server_msg::Message::HostTerminalFocusChanged(msg)) => {
                Ok(ClientToServerMsg::HostTerminalFocusChanged {
                    focused: msg.focused,
                })
            },
            Some(client_to_server_msg::Message::ModifiersHeld(msg)) => {
                Ok(ClientToServerMsg::ModifiersHeld {
                    modifiers: msg
                        .modifiers
                        .into_iter()
                        .map(crate::ipc::enum_conversions::key_modifier_from_proto_i32)
                        .collect::<Result<_>>()?,
                })
            },
            None => Err(anyhow!("Empty ClientToServerMsg message")),
        }
    }
}

// Convert Rust ServerToClientMsg to protobuf
impl From<ServerToClientMsg> for ProtoServerToClientMsg {
    fn from(msg: ServerToClientMsg) -> Self {
        let message = match msg {
            ServerToClientMsg::PluginResponse { request_id, result } => {
                let (data_json, error) = match result {
                    Ok(data) => (
                        serde_json::to_string(&data).expect("plugin response is serializable"),
                        None,
                    ),
                    Err(error) => (String::new(), Some(error)),
                };
                server_to_client_msg::Message::PluginResponse(
                    crate::client_server_contract::client_server_contract::DataResponseMsg {
                        request_id,
                        data_json,
                        error,
                    },
                )
            },
            ServerToClientMsg::DataResponse { request_id, result } => {
                let (data_json, error) = match result {
                    Ok(data) => (data, None),
                    Err(error) => (String::new(), Some(error)),
                };
                server_to_client_msg::Message::DataResponse(
                    crate::client_server_contract::client_server_contract::DataResponseMsg {
                        request_id,
                        data_json,
                        error,
                    },
                )
            },
            ServerToClientMsg::KillSessionComplete { error } => {
                server_to_client_msg::Message::KillSessionComplete(KillSessionCompleteMsg { error })
            },
            ServerToClientMsg::AgentControl(response) => match serde_json::to_string(&response) {
                Ok(json) => server_to_client_msg::Message::AgentControlJson(json),
                Err(error) => server_to_client_msg::Message::LogError(LogErrorMsg {
                    lines: vec![format!("encode agent response: {error}")],
                }),
            },
            ServerToClientMsg::AgentReportAccepted { report_id } => {
                server_to_client_msg::Message::AgentReportAccepted(AgentReportAcceptedMsg {
                    report_id,
                })
            },
            ServerToClientMsg::Render { content } => {
                server_to_client_msg::Message::Render(RenderMsg { content })
            },
            ServerToClientMsg::UnblockInputThread => {
                server_to_client_msg::Message::UnblockInputThread(UnblockInputThreadMsg {})
            },
            ServerToClientMsg::Exit { exit_reason } => {
                let (proto_exit_reason, payload) = match exit_reason {
                    ExitReason::Error(ref msg) => (ProtoExitReason::Error, Some(msg.clone())),
                    ExitReason::CustomExitStatus(status) => {
                        (ProtoExitReason::CustomExitStatus, Some(status.to_string()))
                    },
                    other => (ProtoExitReason::from(other), None),
                };
                server_to_client_msg::Message::Exit(ExitMsg {
                    exit_reason: proto_exit_reason as i32,
                    payload,
                })
            },
            ServerToClientMsg::Connected {
                warp_companions,
                warp_held_terminals,
            } => server_to_client_msg::Message::Connected(ConnectedMsg {
                warp_companions,
                warp_held_terminals,
            }),
            ServerToClientMsg::Log { lines } => {
                server_to_client_msg::Message::Log(LogMsg { lines })
            },
            ServerToClientMsg::LogError { lines } => {
                server_to_client_msg::Message::LogError(LogErrorMsg { lines })
            },
            ServerToClientMsg::SwitchSession { connect_to_session } => {
                server_to_client_msg::Message::SwitchSession(SwitchSessionMsg {
                    connect_to_session: Some(connect_to_session.into()),
                })
            },
            ServerToClientMsg::UnblockCliPipeInput { pipe_name } => {
                server_to_client_msg::Message::UnblockCliPipeInput(UnblockCliPipeInputMsg {
                    pipe_name,
                })
            },
            ServerToClientMsg::CliPipeOutput { pipe_name, output } => {
                server_to_client_msg::Message::CliPipeOutput(CliPipeOutputMsg { pipe_name, output })
            },
            ServerToClientMsg::QueryTerminalSize => {
                server_to_client_msg::Message::QueryTerminalSize(QueryTerminalSizeMsg {})
            },
            ServerToClientMsg::RenamedSession { name } => {
                server_to_client_msg::Message::RenamedSession(RenamedSessionMsg {
                    name: name.to_string(),
                })
            },
            ServerToClientMsg::ConfigFileUpdated => {
                server_to_client_msg::Message::ConfigFileUpdated(ConfigFileUpdatedMsg {})
            },
            ServerToClientMsg::PaneRenderUpdate {
                pane_id,
                viewport,
                scrollback,
                is_initial,
            } => server_to_client_msg::Message::PaneRenderUpdate(PaneRenderUpdateMsg {
                pane_id: Some(pane_id.into()),
                viewport,
                scrollback: scrollback.clone().unwrap_or_default(),
                has_scrollback: scrollback.is_some(),
                is_initial,
            }),
            ServerToClientMsg::SubscribedPaneClosed { pane_id } => {
                server_to_client_msg::Message::SubscribedPaneClosed(SubscribedPaneClosedMsg {
                    pane_id: Some(pane_id.into()),
                })
            },
            ServerToClientMsg::EventUpdate {
                event_type,
                event_json,
            } => server_to_client_msg::Message::EventUpdate(EventUpdateMsg {
                event_type,
                event_json,
            }),
            ServerToClientMsg::ForwardQueryToHost {
                token,
                query_bytes,
                resolve_async,
            } => server_to_client_msg::Message::ForwardQueryToHost(ForwardQueryToHostMsg {
                token,
                query_bytes,
                resolve_async,
            }),
            ServerToClientMsg::EmitNestedSessionFrame { payload_bytes } => {
                server_to_client_msg::Message::EmitNestedSessionFrame(EmitNestedSessionFrameMsg {
                    payload_bytes,
                })
            },
        };

        ProtoServerToClientMsg {
            message: Some(message),
        }
    }
}

// Convert protobuf ServerToClientMsg to Rust
impl TryFrom<ProtoServerToClientMsg> for ServerToClientMsg {
    type Error = anyhow::Error;

    fn try_from(msg: ProtoServerToClientMsg) -> Result<Self> {
        match msg.message {
            Some(server_to_client_msg::Message::KillSessionComplete(msg)) => {
                Ok(ServerToClientMsg::KillSessionComplete { error: msg.error })
            },
            Some(server_to_client_msg::Message::AgentControlJson(json)) => {
                Ok(ServerToClientMsg::AgentControl(
                    serde_json::from_str(&json).context("decode agent response")?,
                ))
            },
            Some(server_to_client_msg::Message::PluginResponse(msg)) => {
                Ok(ServerToClientMsg::PluginResponse {
                    request_id: msg.request_id,
                    result: match msg.error {
                        Some(error) => Err(error),
                        None => Ok(serde_json::from_str(&msg.data_json)
                            .context("decode plugin response")?),
                    },
                })
            },
            Some(server_to_client_msg::Message::DataResponse(msg)) => {
                Ok(ServerToClientMsg::DataResponse {
                    request_id: msg.request_id,
                    result: match msg.error {
                        Some(error) => Err(error),
                        None => Ok(msg.data_json),
                    },
                })
            },
            Some(server_to_client_msg::Message::AgentReportAccepted(msg)) => {
                Ok(ServerToClientMsg::AgentReportAccepted {
                    report_id: msg.report_id,
                })
            },
            Some(server_to_client_msg::Message::Render(render)) => Ok(ServerToClientMsg::Render {
                content: render.content,
            }),
            Some(server_to_client_msg::Message::UnblockInputThread(_)) => {
                Ok(ServerToClientMsg::UnblockInputThread)
            },
            Some(server_to_client_msg::Message::Exit(exit)) => {
                let proto_exit_reason = ProtoExitReason::try_from(exit.exit_reason)
                    .map_err(|_| anyhow!("Invalid exit_reason"))?;

                let exit_reason = match proto_exit_reason {
                    ProtoExitReason::Error => {
                        let error_msg =
                            exit.payload.unwrap_or_else(|| "Protobuf error".to_string());
                        ExitReason::Error(error_msg)
                    },
                    ProtoExitReason::CustomExitStatus => {
                        let status_str = exit.payload.unwrap_or_else(|| "0".to_string());
                        let status = status_str
                            .parse::<i32>()
                            .map_err(|_| anyhow!("Invalid custom exit status: {}", status_str))?;
                        ExitReason::CustomExitStatus(status)
                    },
                    other => other.try_into()?,
                };

                Ok(ServerToClientMsg::Exit { exit_reason })
            },
            Some(server_to_client_msg::Message::Connected(msg)) => {
                Ok(ServerToClientMsg::Connected {
                    warp_companions: msg.warp_companions,
                    warp_held_terminals: msg.warp_held_terminals,
                })
            },
            Some(server_to_client_msg::Message::Log(log)) => {
                Ok(ServerToClientMsg::Log { lines: log.lines })
            },
            Some(server_to_client_msg::Message::LogError(log_error)) => {
                Ok(ServerToClientMsg::LogError {
                    lines: log_error.lines,
                })
            },
            Some(server_to_client_msg::Message::SwitchSession(switch)) => {
                Ok(ServerToClientMsg::SwitchSession {
                    connect_to_session: switch
                        .connect_to_session
                        .ok_or_else(|| anyhow!("Missing connect_to_session"))?
                        .try_into()?,
                })
            },
            Some(server_to_client_msg::Message::UnblockCliPipeInput(unblock)) => {
                Ok(ServerToClientMsg::UnblockCliPipeInput {
                    pipe_name: unblock.pipe_name,
                })
            },
            Some(server_to_client_msg::Message::CliPipeOutput(pipe_output)) => {
                Ok(ServerToClientMsg::CliPipeOutput {
                    pipe_name: pipe_output.pipe_name,
                    output: pipe_output.output,
                })
            },
            Some(server_to_client_msg::Message::QueryTerminalSize(_)) => {
                Ok(ServerToClientMsg::QueryTerminalSize)
            },
            Some(server_to_client_msg::Message::RenamedSession(renamed)) => {
                Ok(ServerToClientMsg::RenamedSession {
                    name: renamed.name.into(),
                })
            },
            Some(server_to_client_msg::Message::ConfigFileUpdated(_)) => {
                Ok(ServerToClientMsg::ConfigFileUpdated)
            },
            Some(server_to_client_msg::Message::PaneRenderUpdate(msg)) => {
                let pane_id: PaneId = msg
                    .pane_id
                    .ok_or_else(|| anyhow!("Missing pane_id"))?
                    .try_into()?;
                let scrollback = if msg.has_scrollback {
                    Some(msg.scrollback)
                } else {
                    None
                };
                Ok(ServerToClientMsg::PaneRenderUpdate {
                    pane_id,
                    viewport: msg.viewport,
                    scrollback,
                    is_initial: msg.is_initial,
                })
            },
            Some(server_to_client_msg::Message::SubscribedPaneClosed(msg)) => {
                let pane_id: PaneId = msg
                    .pane_id
                    .ok_or_else(|| anyhow!("Missing pane_id"))?
                    .try_into()?;
                Ok(ServerToClientMsg::SubscribedPaneClosed { pane_id })
            },
            Some(server_to_client_msg::Message::EventUpdate(msg)) => {
                Ok(ServerToClientMsg::EventUpdate {
                    event_type: msg.event_type,
                    event_json: msg.event_json,
                })
            },
            Some(server_to_client_msg::Message::ForwardQueryToHost(msg)) => {
                Ok(ServerToClientMsg::ForwardQueryToHost {
                    token: msg.token,
                    query_bytes: msg.query_bytes,
                    resolve_async: msg.resolve_async,
                })
            },
            Some(server_to_client_msg::Message::EmitNestedSessionFrame(msg)) => {
                Ok(ServerToClientMsg::EmitNestedSessionFrame {
                    payload_bytes: msg.payload_bytes,
                })
            },
            None => Err(anyhow!("Empty ServerToClientMsg message")),
        }
    }
}

// Basic type conversions
impl From<yinz_types::pane_size::Size>
    for crate::client_server_contract::client_server_contract::Size
{
    fn from(size: yinz_types::pane_size::Size) -> Self {
        Self {
            cols: size.cols as u32,
            rows: size.rows as u32,
        }
    }
}

impl TryFrom<crate::client_server_contract::client_server_contract::Size>
    for yinz_types::pane_size::Size
{
    type Error = anyhow::Error;
    fn try_from(size: crate::client_server_contract::client_server_contract::Size) -> Result<Self> {
        Ok(Self {
            rows: size.rows as usize,
            cols: size.cols as usize,
        })
    }
}

impl From<PixelDimensions>
    for crate::client_server_contract::client_server_contract::PixelDimensions
{
    fn from(pixel_dims: PixelDimensions) -> Self {
        Self {
            text_area_size: pixel_dims.text_area_size.map(|size| {
                crate::client_server_contract::client_server_contract::SizeInPixels {
                    width: size.width as u32,
                    height: size.height as u32,
                }
            }),
            character_cell_size: pixel_dims.character_cell_size.map(|size| {
                crate::client_server_contract::client_server_contract::SizeInPixels {
                    width: size.width as u32,
                    height: size.height as u32,
                }
            }),
        }
    }
}

impl TryFrom<crate::client_server_contract::client_server_contract::PixelDimensions>
    for PixelDimensions
{
    type Error = anyhow::Error;
    fn try_from(
        pixel_dims: crate::client_server_contract::client_server_contract::PixelDimensions,
    ) -> Result<Self> {
        Ok(Self {
            text_area_size: pixel_dims.text_area_size.map(|size| {
                yinz_types::pane_size::SizeInPixels {
                    width: size.width as usize,
                    height: size.height as usize,
                }
            }),
            character_cell_size: pixel_dims.character_cell_size.map(|size| {
                yinz_types::pane_size::SizeInPixels {
                    width: size.width as usize,
                    height: size.height as usize,
                }
            }),
        })
    }
}

impl From<PaneId> for crate::client_server_contract::client_server_contract::PaneReference {
    fn from(pane: PaneId) -> Self {
        let (pane_id, is_plugin) = pane.legacy_parts();
        Self { pane_id, is_plugin }
    }
}

impl From<crate::client_server_contract::client_server_contract::PaneReference> for PaneId {
    fn from(pane: crate::client_server_contract::client_server_contract::PaneReference) -> Self {
        Self::from_legacy(pane.pane_id, pane.is_plugin)
    }
}

impl From<ColorRegister> for crate::client_server_contract::client_server_contract::ColorRegister {
    fn from(color_reg: ColorRegister) -> Self {
        Self {
            index: color_reg.index as u32,
            color: color_reg.color,
        }
    }
}

impl TryFrom<crate::client_server_contract::client_server_contract::ColorRegister>
    for ColorRegister
{
    type Error = anyhow::Error;
    fn try_from(
        color_reg: crate::client_server_contract::client_server_contract::ColorRegister,
    ) -> Result<Self> {
        Ok(Self {
            index: color_reg.index as usize,
            color: color_reg.color,
        })
    }
}

impl From<yinz_types::input::cli_assets::CliAssets>
    for crate::client_server_contract::client_server_contract::CliAssets
{
    fn from(cli_assets: yinz_types::input::cli_assets::CliAssets) -> Self {
        Self {
            config_file_path: cli_assets
                .config_file_path
                .map(|p| p.to_string_lossy().to_string()),
            config_dir: cli_assets
                .config_dir
                .map(|p| p.to_string_lossy().to_string()),
            should_ignore_config: cli_assets.should_ignore_config,
            configuration_options: cli_assets.configuration_options.map(|o| o.into()),
            layout: cli_assets.layout.map(|l| l.into()),
            terminal_window_size: Some(cli_assets.terminal_window_size.into()),
            data_dir: cli_assets.data_dir.map(|p| p.to_string_lossy().to_string()),
            is_debug: cli_assets.is_debug,
            max_panes: cli_assets.max_panes.map(|m| m as u32),
            force_run_layout_commands: cli_assets.force_run_layout_commands,
            cwd: cli_assets.cwd.map(|p| p.to_string_lossy().to_string()),
            host_terminal_env: cli_assets.host_terminal_env.into_iter().collect(),
        }
    }
}

impl TryFrom<crate::client_server_contract::client_server_contract::CliAssets>
    for yinz_types::input::cli_assets::CliAssets
{
    type Error = anyhow::Error;
    fn try_from(
        cli_assets: crate::client_server_contract::client_server_contract::CliAssets,
    ) -> Result<Self> {
        Ok(Self {
            config_file_path: cli_assets.config_file_path.map(PathBuf::from),
            config_dir: cli_assets.config_dir.map(PathBuf::from),
            should_ignore_config: cli_assets.should_ignore_config,
            configuration_options: cli_assets
                .configuration_options
                .map(|o| o.try_into())
                .transpose()?,
            layout: cli_assets.layout.map(|l| l.try_into()).transpose()?,
            terminal_window_size: cli_assets
                .terminal_window_size
                .ok_or_else(|| anyhow!("CliAssets missing terminal_window_size"))?
                .try_into()?,
            data_dir: cli_assets.data_dir.map(PathBuf::from),
            is_debug: cli_assets.is_debug,
            max_panes: cli_assets.max_panes.map(|m| m as usize),
            force_run_layout_commands: cli_assets.force_run_layout_commands,
            cwd: cli_assets.cwd.map(PathBuf::from),
            host_terminal_env: cli_assets.host_terminal_env.into_iter().collect(),
        })
    }
}

impl From<yinz_types::input::options::Options>
    for crate::client_server_contract::client_server_contract::Options
{
    fn from(options: yinz_types::input::options::Options) -> Self {
        use crate::client_server_contract::client_server_contract::{
            Clipboard as ProtoClipboard, OnForceClose as ProtoOnForceClose,
        };

        Self {
            simplified_ui: options.simplified_ui,
            theme: options.theme,
            theme_dark: options.theme_dark,
            theme_light: options.theme_light,
            default_mode: options.default_mode.map(input_mode_to_proto_i32),
            default_shell: options
                .default_shell
                .map(|p| p.to_string_lossy().to_string()),
            default_cwd: options.default_cwd.map(|p| p.to_string_lossy().to_string()),
            default_layout: options
                .default_layout
                .map(|p| p.to_string_lossy().to_string()),
            layout_dir: options.layout_dir.map(|p| p.to_string_lossy().to_string()),
            theme_dir: options.theme_dir.map(|p| p.to_string_lossy().to_string()),
            user_plugin_dir: options
                .user_plugin_dir
                .map(|p| p.to_string_lossy().to_string()),
            builtin_plugins_dir: options
                .builtin_plugins_dir
                .map(|p| p.to_string_lossy().to_string()),
            mouse_mode: options.mouse_mode,
            agent_double_escape: options.agent_double_escape,
            pane_frames: options.pane_frames,
            pane_animations: options.pane_animations,
            background_opacity: options.background_opacity.map(u32::from),
            mirror_session: options.mirror_session,
            on_force_close: options.on_force_close.map(|o| match o {
                yinz_types::input::options::OnForceClose::Quit => ProtoOnForceClose::Quit as i32,
                yinz_types::input::options::OnForceClose::Detach => ProtoOnForceClose::Detach as i32,
            }),
            scroll_buffer_size: options.scroll_buffer_size.map(|s| s as u32),
            max_fullscreen_agent_width: options.max_fullscreen_agent_width.map(|width| width as u64),
            copy_command: options.copy_command,
            copy_clipboard: options.copy_clipboard.map(|c| match c {
                yinz_types::input::options::Clipboard::System => ProtoClipboard::System as i32,
                yinz_types::input::options::Clipboard::Primary => ProtoClipboard::Primary as i32,
            }),
            copy_on_select: options.copy_on_select,
            osc8_hyperlinks: options.osc8_hyperlinks,
            scrollback_editor: options
                .scrollback_editor
                .map(|p| p.to_string_lossy().to_string()),
            session_name: options.session_name.map(|name| name.into_string()),
            attach_to_session: options.attach_to_session,
            auto_layout: options.auto_layout,
            session_serialization: options.session_serialization,
            serialize_pane_viewport: options.serialize_pane_viewport,
            scrollback_lines_to_serialize: options.scrollback_lines_to_serialize.map(|s| s as u32),
            styled_underlines: options.styled_underlines,
            serialization_interval: options.serialization_interval.map(yinz_types::units::ElapsedSeconds::get),
            locked_escape_timeout_ms: options.locked_escape_timeout_ms.map(yinz_types::units::ElapsedMillis::get),
            disable_session_metadata: options.disable_session_metadata,
            support_kitty_keyboard_protocol: options.support_kitty_keyboard_protocol,
            report_modifier_keys: options.report_modifier_keys,
            support_kitty_graphics_protocol: options.support_kitty_graphics_protocol,
            stacked_resize: options.stacked_resize,
            agent_pane_min_columns: options.agent_pane_min_columns.map(|c| c as u32),
            agent_pane_min_rows: options.agent_pane_min_rows.map(|r| r as u32),
            stacked_pane_list: options.stacked_pane_list,
            dangerously_enable_paste_buffer_read: options.dangerously_enable_paste_buffer_read,
            advanced_mouse_actions: options.advanced_mouse_actions,
            mouse_scroll_resize: options.mouse_scroll_resize,
            mouse_hover_effects: options.mouse_hover_effects,
            post_command_discovery_hook: options.post_command_discovery_hook,
            visual_bell: options.visual_bell,
            focus_follows_mouse: options.focus_follows_mouse,
            mouse_click_through: options.mouse_click_through,
            context_menu_on_right_click: options.context_menu_on_right_click,
            context_menu_override_agent_mouse: options.context_menu_override_agent_mouse,
            agent_theme_mapping: options.agent_theme_mapping.map(|value| value.to_string()),
            osc133_command_selection: options.osc133_command_selection,
            word_separators: options.word_separators,
            pane_frame_style: options.pane_frame_style.map(|s| match s {
                yinz_types::input::options::PaneFrameStyle::Full => "full".to_owned(),
                yinz_types::input::options::PaneFrameStyle::Titles => "titles".to_owned(),
                yinz_types::input::options::PaneFrameStyle::None => "none".to_owned(),
            }),
            notification_position: options
                .notification_position
                .map(|p| p.as_str().to_owned()),
            notification_timeout: options.notification_timeout.map(yinz_types::units::ElapsedSeconds::get),
            host_notification_protocol: options
                .host_notification_protocol
                .map(|p| p.as_str().to_owned()),
            nested_session_handling: options.nested_session_handling.map(|n| {
                use crate::client_server_contract::client_server_contract::NestedSessionHandling as ProtoNestedSessionHandling;
                use yinz_types::input::options::NestedSessionHandling;
                match n {
                    NestedSessionHandling::Ask => ProtoNestedSessionHandling::Ask as i32,
                    NestedSessionHandling::Fullscreen => {
                        ProtoNestedSessionHandling::Fullscreen as i32
                    },
                    NestedSessionHandling::Descend => ProtoNestedSessionHandling::Descend as i32,
                    NestedSessionHandling::Never => ProtoNestedSessionHandling::Never as i32,
                }
            }),
            explicit_theme_hue: options.explicit_theme_hue.map(|hue| {
                use crate::client_server_contract::client_server_contract::ThemeHue as ProtoThemeHue;
                match hue {
                    yinz_types::data::ThemeHue::Dark => ProtoThemeHue::Dark as i32,
                    yinz_types::data::ThemeHue::Light => ProtoThemeHue::Light as i32,
                }
            }),
        }
    }
}

impl TryFrom<crate::client_server_contract::client_server_contract::Options>
    for yinz_types::input::options::Options
{
    type Error = anyhow::Error;
    fn try_from(
        options: crate::client_server_contract::client_server_contract::Options,
    ) -> Result<Self> {
        use crate::client_server_contract::client_server_contract::{
            Clipboard as ProtoClipboard, NestedSessionHandling as ProtoNestedSessionHandling,
            OnForceClose as ProtoOnForceClose,
        };

        Ok(Self {
            simplified_ui: options.simplified_ui,
            theme: options.theme,
            theme_dark: options.theme_dark,
            theme_light: options.theme_light,
            default_mode: options
                .default_mode
                .map(proto_i32_to_input_mode)
                .transpose()?,
            default_shell: options.default_shell.map(std::path::PathBuf::from),
            default_cwd: options.default_cwd.map(std::path::PathBuf::from),
            default_layout: options.default_layout.map(std::path::PathBuf::from),
            layout_dir: options.layout_dir.map(std::path::PathBuf::from),
            theme_dir: options.theme_dir.map(std::path::PathBuf::from),
            user_plugin_dir: options.user_plugin_dir.map(std::path::PathBuf::from),
            builtin_plugins_dir: options.builtin_plugins_dir.map(std::path::PathBuf::from),
            mouse_mode: options.mouse_mode,
            agent_double_escape: options.agent_double_escape,
            // Not on the wire: the server reads this straight out of the config
            // file it loads itself, and it has no per-invocation CLI form.
            detachable_panes: None,
            // Likewise, and the URL may carry a database password.
            // Likewise. The server resolves its host id once at startup from its own data dir,
            // and a client has no business naming which machine the server it is talking to is.
            host_id: None,
            pane_frames: options.pane_frames,
            pane_animations: options.pane_animations,
            background_opacity: options.background_opacity.map(|value| {
                u8::try_from(value).ok().filter(|v| *v <= 100)
                    .ok_or_else(|| anyhow!("background_opacity must be from 0 to 100"))
            }).transpose()?,
            pane_frame_style: options.pane_frame_style.as_deref().and_then(|s| match s {
                "full" => Some(yinz_types::input::options::PaneFrameStyle::Full),
                "titles" => Some(yinz_types::input::options::PaneFrameStyle::Titles),
                "none" => Some(yinz_types::input::options::PaneFrameStyle::None),
                _ => None,
            }),
            notification_position: options
                .notification_position
                .as_deref()
                .and_then(|p| p.parse().ok()),
            notification_timeout: options.notification_timeout.map(yinz_types::units::ElapsedSeconds::new),
            mirror_session: options.mirror_session,
            on_force_close: options
                .on_force_close
                .map(|o| match ProtoOnForceClose::try_from(o) {
                    Ok(ProtoOnForceClose::Quit) => Ok(yinz_types::input::options::OnForceClose::Quit),
                    Ok(ProtoOnForceClose::Detach) => {
                        Ok(yinz_types::input::options::OnForceClose::Detach)
                    },
                    _ => Err(anyhow!("Invalid OnForceClose value: {}", o)),
                })
                .transpose()?,
            scroll_buffer_size: options.scroll_buffer_size.map(|s| s as usize),
            max_fullscreen_agent_width: options.max_fullscreen_agent_width.map(usize::try_from).transpose().context("agent width exceeds platform size")?,
            copy_command: options.copy_command,
            copy_clipboard: options
                .copy_clipboard
                .map(|c| match ProtoClipboard::try_from(c) {
                    Ok(ProtoClipboard::System) => Ok(yinz_types::input::options::Clipboard::System),
                    Ok(ProtoClipboard::Primary) => Ok(yinz_types::input::options::Clipboard::Primary),
                    _ => Err(anyhow!("Invalid Clipboard value: {}", c)),
                })
                .transpose()?,
            copy_on_select: options.copy_on_select,
            osc8_hyperlinks: options.osc8_hyperlinks,
            scrollback_editor: options.scrollback_editor.map(std::path::PathBuf::from),
            session_name: options.session_name.map(Into::into),
            attach_to_session: options.attach_to_session,
            auto_layout: options.auto_layout,
            session_serialization: options.session_serialization,
            serialize_pane_viewport: options.serialize_pane_viewport,
            scrollback_lines_to_serialize: options
                .scrollback_lines_to_serialize
                .map(|s| s as usize),
            styled_underlines: options.styled_underlines,
            serialization_interval: options.serialization_interval.map(yinz_types::units::ElapsedSeconds::new),
            locked_escape_timeout_ms: options.locked_escape_timeout_ms.map(yinz_types::units::ElapsedMillis::new),
            disable_session_metadata: options.disable_session_metadata,
            support_kitty_keyboard_protocol: options.support_kitty_keyboard_protocol,
            report_modifier_keys: options.report_modifier_keys,
            support_kitty_graphics_protocol: options.support_kitty_graphics_protocol,
            stacked_resize: options.stacked_resize,
            agent_pane_min_columns: options.agent_pane_min_columns.map(|c| c as usize),
            agent_pane_min_rows: options.agent_pane_min_rows.map(|r| r as usize),
            stacked_pane_list: options.stacked_pane_list,
            dangerously_enable_paste_buffer_read: options.dangerously_enable_paste_buffer_read,
            advanced_mouse_actions: options.advanced_mouse_actions,
            mouse_scroll_resize: options.mouse_scroll_resize,
            mouse_hover_effects: options.mouse_hover_effects,
            post_command_discovery_hook: options.post_command_discovery_hook,
            visual_bell: options.visual_bell,
            focus_follows_mouse: options.focus_follows_mouse,
            mouse_click_through: options.mouse_click_through,
            context_menu_on_right_click: options.context_menu_on_right_click,
            context_menu_override_agent_mouse: options.context_menu_override_agent_mouse,
            agent_theme_mapping: options.agent_theme_mapping.map(|value| value.parse().map_err(anyhow::Error::msg)).transpose()?,
            osc133_command_selection: options.osc133_command_selection,
            word_separators: options.word_separators,
            host_notification_protocol: options
                .host_notification_protocol
                .map(|p| {
                    p.parse::<yinz_types::input::options::HostNotificationProtocol>()
                        .map_err(|e| anyhow!(e))
                })
                .transpose()?,
            nested_session_handling: options
                .nested_session_handling
                .map(|n| match ProtoNestedSessionHandling::try_from(n) {
                    Ok(ProtoNestedSessionHandling::Ask) => {
                        Ok(yinz_types::input::options::NestedSessionHandling::Ask)
                    },
                    Ok(ProtoNestedSessionHandling::Fullscreen) => {
                        Ok(yinz_types::input::options::NestedSessionHandling::Fullscreen)
                    },
                    Ok(ProtoNestedSessionHandling::Descend) => {
                        Ok(yinz_types::input::options::NestedSessionHandling::Descend)
                    },
                    Ok(ProtoNestedSessionHandling::Never) => {
                        Ok(yinz_types::input::options::NestedSessionHandling::Never)
                    },
                    _ => Err(anyhow!("Invalid NestedSessionHandling value: {}", n)),
                })
                .transpose()?,
            explicit_theme_hue: options
                .explicit_theme_hue
                .map(|hue| {
                    use crate::client_server_contract::client_server_contract::ThemeHue as ProtoThemeHue;
                    match ProtoThemeHue::try_from(hue).ok() {
                        Some(ProtoThemeHue::Dark) => Ok(yinz_types::data::ThemeHue::Dark),
                        Some(ProtoThemeHue::Light) => Ok(yinz_types::data::ThemeHue::Light),
                        _ => Err(anyhow!("Invalid ThemeHue value: {}", hue)),
                    }
                })
                .transpose()?,
        })
    }
}

impl From<yinz_types::data::WarpPlacement> for ProtobufWarpPlacement {
    fn from(placement: yinz_types::data::WarpPlacement) -> Self {
        match placement {
            yinz_types::data::WarpPlacement::NewTab => ProtobufWarpPlacement::NewTab,
            yinz_types::data::WarpPlacement::FocusedTab => ProtobufWarpPlacement::FocusedTab,
            yinz_types::data::WarpPlacement::Floating => ProtobufWarpPlacement::Floating,
        }
    }
}

/// Protobuf enums arrive as bare `i32`, so an unknown value is a real
/// possibility — a newer peer with a placement this build has never heard of.
/// Refuse rather than silently landing the pane somewhere the user did not ask
/// for.
fn warp_placement_from_i32(value: i32) -> Result<yinz_types::data::WarpPlacement> {
    match ProtobufWarpPlacement::try_from(value) {
        Ok(ProtobufWarpPlacement::NewTab) => Ok(yinz_types::data::WarpPlacement::NewTab),
        Ok(ProtobufWarpPlacement::FocusedTab) => Ok(yinz_types::data::WarpPlacement::FocusedTab),
        Ok(ProtobufWarpPlacement::Floating) => Ok(yinz_types::data::WarpPlacement::Floating),
        Err(_) => Err(anyhow!("unknown warp placement: {value}")),
    }
}

/// The pane-sized half of a tab warp, which the tab message repeats once per
/// pane.
///
/// Written as a pair of free functions rather than `From` impls because the
/// protobuf `WarpedPane` has no placement while `AdoptPaneAction` does — the two
/// are the same five fields but not the same message, and conflating them would
/// mean inventing a placement for a pane whose place the layout already decides.
fn warped_tab_to_protobuf(
    payload: &yinz_types::data::WarpedTab,
) -> crate::client_server_contract::client_server_contract::AdoptTabAction {
    crate::client_server_contract::client_server_contract::AdoptTabAction {
        name: payload.name.clone(),
        user_named: payload.user_named,
        tiled_layout: Some(payload.tiled_layout.clone().into()),
        floating_layout: payload
            .floating_layout
            .iter()
            .map(|l| l.clone().into())
            .collect(),
        tiled_panes: payload
            .tiled_panes
            .iter()
            .map(warped_pane_to_protobuf)
            .collect(),
        floating_panes: payload
            .floating_panes
            .iter()
            .map(warped_pane_to_protobuf)
            .collect(),
        hide_floating_panes: payload.hide_floating_panes,
        relationships: payload
            .relationships
            .as_ref()
            .map(warp_relationships_to_protobuf),
    }
}
fn warped_tab_from_protobuf(
    adopt_tab_action: crate::client_server_contract::client_server_contract::AdoptTabAction,
) -> Result<yinz_types::data::WarpedTab> {
    let payload = yinz_types::data::WarpedTab {
        relationships: adopt_tab_action
            .relationships
            .map(warp_relationships_from_protobuf),
        name: adopt_tab_action.name,
        user_named: adopt_tab_action.user_named,
        tiled_layout: adopt_tab_action
            .tiled_layout
            .ok_or_else(|| anyhow!("AdoptTab missing tiled_layout"))?
            .try_into()?,
        floating_layout: adopt_tab_action
            .floating_layout
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>>>()?,
        tiled_panes: adopt_tab_action
            .tiled_panes
            .into_iter()
            .map(warped_pane_from_protobuf)
            .collect::<Result<Vec<_>>>()?,
        floating_panes: adopt_tab_action
            .floating_panes
            .into_iter()
            .map(warped_pane_from_protobuf)
            .collect::<Result<Vec<_>>>()?,
        hide_floating_panes: adopt_tab_action.hide_floating_panes,
    };
    payload
        .validate_relationships()
        .map_err(|e| anyhow!("{e}"))?;
    Ok(payload)
}
fn pane_group_from_protobuf(
    group: crate::client_server_contract::client_server_contract::AdoptTabAction,
) -> Result<yinz_types::data::WarpedTab> {
    let group = warped_tab_from_protobuf(group)?;
    group.validate_pane_group().map_err(|e| anyhow!("{e}"))?;
    Ok(group)
}

fn warp_relationships_to_protobuf(
    value: &yinz_types::data::WarpRelationships,
) -> crate::client_server_contract::client_server_contract::WarpRelationships {
    use crate::client_server_contract::client_server_contract as proto;
    proto::WarpRelationships {
        hidden: value
            .hidden
            .iter()
            .map(|p| proto::WarpHiddenPane {
                pane: p.pane,
                restored_by: p.restored_by,
                is_scrollback_editor: p.is_scrollback_editor,
            })
            .collect(),
        stacks: value
            .stacks
            .iter()
            .map(|s| proto::WarpStackList {
                members: s.members.clone(),
                selected: s.selected,
            })
            .collect(),
    }
}
fn warp_relationships_from_protobuf(
    value: crate::client_server_contract::client_server_contract::WarpRelationships,
) -> yinz_types::data::WarpRelationships {
    use yinz_types::data::{WarpHiddenPane, WarpRelationships, WarpStackList};
    WarpRelationships {
        hidden: value
            .hidden
            .into_iter()
            .map(|p| WarpHiddenPane {
                pane: p.pane,
                restored_by: p.restored_by,
                is_scrollback_editor: p.is_scrollback_editor,
            })
            .collect(),
        stacks: value
            .stacks
            .into_iter()
            .map(|s| WarpStackList {
                members: s.members,
                selected: s.selected,
            })
            .collect(),
    }
}

fn warped_agent_input_to_protobuf(
    input: &yinz_types::data::WarpedAgentInput,
) -> crate::client_server_contract::client_server_contract::WarpedAgentInput {
    crate::client_server_contract::client_server_contract::WarpedAgentInput {
        kind: input.kind.clone(),
        conversation: input.conversation.clone(),
        consumed: input.consumed,
    }
}
fn warped_agent_input_from_protobuf(
    input: crate::client_server_contract::client_server_contract::WarpedAgentInput,
) -> yinz_types::data::WarpedAgentInput {
    yinz_types::data::WarpedAgentInput {
        kind: input.kind,
        conversation: input.conversation,
        consumed: input.consumed,
    }
}

fn warped_pane_to_protobuf(
    pane: &yinz_types::data::WarpedPane,
) -> crate::client_server_contract::client_server_contract::WarpedPane {
    crate::client_server_contract::client_server_contract::WarpedPane {
        bracketed_paste: pane.bracketed_paste,
        agent_input: pane
            .agent_input
            .as_ref()
            .map(warped_agent_input_to_protobuf),
        held: pane.held.as_ref().map(held_to_protobuf),
        group: pane.group.as_deref().map(warped_tab_to_protobuf),
        host_id: pane.host_id.clone().into_string(),
        control_socket: pane.control_socket.to_string_lossy().to_string(),
        title: pane.title.clone(),
        caption: pane.caption.clone(),
        contents: pane.contents.clone(),
        agent_theme_json: pane
            .agent_theme
            .as_ref()
            .and_then(|m| serde_json::to_string(m).ok()),
        borderless: pane.borderless,
        remote: pane.remote.as_ref().map(remote_handoff_to_protobuf),
        agent: pane.agent.clone().map(agent_info_to_proto),
    }
}

fn warped_pane_from_protobuf(
    pane: crate::client_server_contract::client_server_contract::WarpedPane,
) -> Result<yinz_types::data::WarpedPane> {
    Ok(yinz_types::data::WarpedPane {
        bracketed_paste: pane.bracketed_paste,
        agent_input: pane.agent_input.map(warped_agent_input_from_protobuf),
        held: pane.held.map(held_from_protobuf).transpose()?,
        group: pane
            .group
            .map(pane_group_from_protobuf)
            .transpose()?
            .map(Box::new),
        host_id: SupervisorId::new(pane.host_id),
        control_socket: std::path::PathBuf::from(pane.control_socket),
        title: pane.title,
        caption: pane.caption,
        agent_theme: pane
            .agent_theme_json
            .as_deref()
            .and_then(|m| serde_json::from_str(m).ok()),
        contents: pane.contents,
        borderless: pane.borderless,
        remote: pane.remote.and_then(remote_handoff_from_protobuf),
        agent: pane.agent.map(agent_info_from_proto).transpose()?,
    })
}

fn held_to_protobuf(
    held: &yinz_types::data::HeldTerminal,
) -> crate::client_server_contract::client_server_contract::HeldTerminal {
    crate::client_server_contract::client_server_contract::HeldTerminal {
        command: Some(run_command_to_protobuf(&held.command)),
        first_run: held.first_run,
        exit_status: held.exit_status,
    }
}

fn held_from_protobuf(
    held: crate::client_server_contract::client_server_contract::HeldTerminal,
) -> Result<yinz_types::data::HeldTerminal> {
    Ok(yinz_types::data::HeldTerminal {
        command: held
            .command
            .and_then(run_command_from_protobuf)
            .context("held terminal has no valid command")?,
        first_run: held.first_run,
        exit_status: held.exit_status,
    })
}

/// The address of a pane whose process is on another machine.
///
/// Free functions for the same reason the pane ones above are: this rides in
/// both `WarpedPane` and `AdoptPaneAction`, which are the same fields in two
/// messages.
fn remote_handoff_to_protobuf(
    remote: &yinz_types::data::RemotePaneHandoff,
) -> crate::client_server_contract::client_server_contract::RemotePaneHandoff {
    crate::client_server_contract::client_server_contract::RemotePaneHandoff {
        host: remote.host.clone(),
        pane_token: remote.pane_token.clone(),
        received: remote.received,
        cmd: Some(run_command_to_protobuf(&remote.cmd)),
    }
}

/// `None` if the command did not survive the wire.
///
/// A handoff without one is not a handoff: the adopting session would have a
/// running process it could not name, re-run, or title. Better to arrive as no
/// remote pane at all — the adoption then fails with "no supervisor" and the
/// warp rolls back — than as one that is half there.
fn remote_handoff_from_protobuf(
    remote: crate::client_server_contract::client_server_contract::RemotePaneHandoff,
) -> Option<yinz_types::data::RemotePaneHandoff> {
    Some(yinz_types::data::RemotePaneHandoff {
        host: remote.host,
        pane_token: remote.pane_token,
        received: remote.received,
        cmd: remote.cmd.and_then(run_command_from_protobuf)?,
    })
}

fn pane_target_to_protobuf(
    target: &yinz_types::command::PaneTarget,
) -> crate::client_server_contract::client_server_contract::PaneTarget {
    use crate::client_server_contract::client_server_contract::{pane_target::Kind, PaneTarget};
    let kind = match target {
        yinz_types::command::PaneTarget::Local => Kind::Local(true),
        yinz_types::command::PaneTarget::Ssh { host } => Kind::SshHost(host.clone()),
        yinz_types::command::PaneTarget::Bubblewrap { root } => {
            Kind::BubblewrapRoot(root.to_string_lossy().to_string())
        },
    };
    PaneTarget {
        kind: Some(kind),
        firecracker: None,
    }
}

fn pane_target_from_protobuf(
    target: Option<crate::client_server_contract::client_server_contract::PaneTarget>,
) -> Result<yinz_types::command::PaneTarget> {
    use crate::client_server_contract::client_server_contract::pane_target::Kind;
    if target
        .as_ref()
        .is_some_and(|target| target.firecracker.is_some())
    {
        return Err(anyhow!("Firecracker execution is no longer supported."));
    }
    Ok(match target.and_then(|target| target.kind) {
        None | Some(Kind::Local(_)) => yinz_types::command::PaneTarget::Local,
        Some(Kind::SshHost(host)) => yinz_types::command::PaneTarget::Ssh { host },
        Some(Kind::BubblewrapRoot(root)) => {
            yinz_types::command::PaneTarget::bubblewrap(std::path::Path::new(&root))
                .map_err(|error| anyhow!(error))?
        },
    })
}

fn run_command_to_protobuf(
    cmd: &yinz_types::input::command::RunCommand,
) -> crate::client_server_contract::client_server_contract::RunCommandAction {
    crate::client_server_contract::client_server_contract::RunCommandAction {
        command: cmd.command.to_string_lossy().to_string(),
        args: cmd.args.clone(),
        cwd: cmd.cwd.as_ref().map(|p| p.to_string_lossy().to_string()),
        // `RunCommand` has no direction; the field belongs to the action that
        // creates one, not to the command itself.
        direction: None,
        hold_on_close: cmd.hold_on_close,
        hold_on_start: cmd.hold_on_start,
        originating_plugin: cmd.originating_plugin.clone().map(|op| op.into()),
        use_terminal_title: cmd.use_terminal_title,
        remote_default_shell: cmd.remote_default_shell,
        target: Some(pane_target_to_protobuf(&cmd.target)),
    }
}

fn run_command_from_protobuf(
    cmd: crate::client_server_contract::client_server_contract::RunCommandAction,
) -> Option<yinz_types::input::command::RunCommand> {
    Some(yinz_types::input::command::RunCommand {
        agent_theme: None,
        claude_settings: None,
        workflow_owner: None,
        command: std::path::PathBuf::from(cmd.command),
        args: cmd.args,
        cwd: cmd.cwd.map(std::path::PathBuf::from),
        hold_on_close: cmd.hold_on_close,
        hold_on_start: cmd.hold_on_start,
        originating_plugin: cmd.originating_plugin.and_then(|op| op.try_into().ok()),
        use_terminal_title: cmd.use_terminal_title,
        remote_default_shell: cmd.remote_default_shell,
        target: pane_target_from_protobuf(cmd.target).ok()?,
    })
}

// Complete Action conversion implementation - all 91 variants
impl TryFrom<yinz_types::input::actions::Action>
    for crate::client_server_contract::client_server_contract::Action
{
    type Error = anyhow::Error;

    fn try_from(action: yinz_types::input::actions::Action) -> Result<Self> {
        use crate::client_server_contract::client_server_contract::{
            action::ActionType,
            ActivateNotificationAction,
            AdoptPaneAction,
            AreFloatingPanesVisibleAction,
            ArrangePanesByProjectAction,
            BreakPaneAction,
            BreakPaneLeftAction,
            BreakPaneRightAction,
            ChangeFloatingPaneCoordinatesAction,
            ClearScreenAction,
            ClearScreenByPaneIdAction,
            CliPipeAction,
            CloseFocusAction,
            CloseFocusByPaneIdAction,
            ClosePluginPaneAction,
            CloseTabAction,
            CloseTabByIdAction,
            CloseTerminalPaneAction,
            ConfirmAction,
            CopyAction,
            CopyLastCommandOutputAction,
            CurrentTabInfoAction,
            DenyAction,
            DetachAction,
            DumpLayoutAction,
            DumpScreenAction,
            EditFileAction,
            EditScrollbackAction,
            EditScrollbackByPaneIdAction,
            FocusGuestSessionAction,
            FocusHostSessionAction,
            FocusLastPaneAction,
            FocusNextPaneAction,
            FocusPaneByPaneIdAction,
            FocusPaneTabsAction,
            FocusPluginPaneWithIdAction,
            FocusPreviousPaneAction,
            FocusTerminalPaneWithIdAction,
            GoToNextTabAction,
            GoToPreviousTabAction,
            GoToTabAction,
            GoToTabByIdAction,
            GoToTabNameAction,
            HalfPageScrollDownAction,
            HalfPageScrollDownByPaneIdAction,
            HalfPageScrollUpAction,
            HalfPageScrollUpByPaneIdAction,
            HideFloatingPanesAction,
            KeybindPipeAction,
            LaunchOrFocusPluginAction,
            LaunchPluginAction,
            ListClientsAction,
            ListPanesAction,
            ListTabsAction,
            MouseEventAction,
            MoveFocusAction,
            MoveFocusOrTabAction,
            MoveNotificationSelectionAction,
            MovePaneAction,
            MovePaneBackwardsAction,
            MovePaneBackwardsByPaneIdAction,
            MovePaneByPaneIdAction,
            MoveTabAction,
            MoveTabByTabIdAction,
            MoveTextSelectionAction,
            NewBlockingPaneAction,
            NewFloatingPaneAction,
            NewFloatingPluginPaneAction,
            NewInPlacePaneAction,
            NewInPlacePluginPaneAction,
            NewPaneAction,
            NewStackedPaneAction,
            NewTabAction,
            NewTiledPaneAction,
            NewTiledPluginPaneAction,
            NextPaneTabAction,

            NextSwapLayoutAction,
            NextSwapLayoutByTabIdAction,
            NoOpAction,
            NotifyAction,
            OverrideLayoutAction,
            PageScrollDownAction,
            PageScrollDownByPaneIdAction,
            PageScrollUpAction,
            PageScrollUpByPaneIdAction,
            PaneIdWithPlugin,
            PaneNameInputAction,
            PasteAction,
            PreviousPaneTabAction,
            PreviousSwapLayoutAction,
            PreviousSwapLayoutByTabIdAction,
            QueryTabNamesAction,
            QuitAction,
            RenamePaneByPaneIdAction,
            RenamePluginPaneAction,
            RenameSessionAction,
            RenameTabAction,
            RenameTabByIdAction,
            RenameTerminalPaneAction,
            ResizeAction,
            ResizeByPaneIdAction,
            RunAction,
            SaveSessionAction,
            ScrollDownAction,
            ScrollDownAtAction,
            ScrollDownByPaneIdAction,
            ScrollToBottomAction,
            ScrollToBottomByPaneIdAction,
            ScrollToNextPromptAction,
            ScrollToPreviousPromptAction,
            ScrollToTopAction,
            ScrollToTopByPaneIdAction,
            ScrollUpAction,
            ScrollUpAtAction,
            // Pane-targeting
            ScrollUpByPaneIdAction,
            SearchAction,
            SearchInputAction,
            SearchToggleOptionAction,
            SelectCommandAtScrollPositionAction,
            SetDarkThemeAction,
            SetLightThemeAction,
            SetPaneBorderlessAction,
            SetPaneColorAction,
            SetPaneFrameStyleAction,
            ShowFloatingPanesAction,
            SkipConfirmAction,
            StackPanesAction,
            StartOrReloadPluginAction,
            SwapTextSelectionEndsAction,
            SwitchFocusAction,
            SwitchModeForAllClientsAction,
            SwitchSessionAction,
            SwitchToModeAction,
            TabNameInputAction,
            ToggleActiveSyncTabAction,
            ToggleActiveSyncTabByTabIdAction,
            ToggleCommandPaletteAction,
            ToggleFloatingPanesAction,
            ToggleFloatingPanesByTabIdAction,
            ToggleFocusFullscreenAction,
            ToggleFocusNoUiFullscreenAction,
            ToggleFullscreenByPaneIdAction,
            ToggleGroupMarkingAction,
            ToggleHostFullscreenAction,
            ToggleMotherAction,
            ToggleMouseModeAction,
            ToggleNoUiFullscreenByPaneIdAction,
            ToggleNotificationDetailAction,
            TogglePaneBorderlessAction,
            TogglePaneEmbedOrFloatingAction,
            TogglePaneEmbedOrFloatingByPaneIdAction,
            TogglePaneFramesAction,
            TogglePaneInGroupAction,
            TogglePanePinnedAction,
            TogglePanePinnedByPaneIdAction,
            ToggleTabAction,
            ToggleThemeAction,
            UndoRenamePaneAction,
            UndoRenamePaneByPaneIdAction,
            UndoRenameTabAction,
            // Tab-targeting
            UndoRenameTabByTabIdAction,
            WarpPaneAction,
            WarpTabAction,
            WarpTabFromAction,
            WriteAction,
            WriteCharsAction,
            WriteCharsToPaneIdAction,
            WriteToPaneIdAction,
        };
        use std::collections::HashMap;

        let action_type = match action {
            yinz_types::input::actions::Action::Quit => ActionType::Quit(QuitAction {}),
            yinz_types::input::actions::Action::Write {
                key_with_modifier,
                bytes,
                is_kitty_keyboard_protocol,
            } => ActionType::Write(WriteAction {
                key_with_modifier: key_with_modifier.map(|k| k.into()),
                bytes: bytes.into_iter().map(|b| b as u32).collect(),
                is_kitty_keyboard_protocol,
            }),
            yinz_types::input::actions::Action::WriteChars { chars } => {
                ActionType::WriteChars(WriteCharsAction { chars })
            },
            yinz_types::input::actions::Action::WriteToPaneId { bytes, pane_id } => {
                ActionType::WriteToPaneId(WriteToPaneIdAction {
                    pane_id: Some(pane_id.into()),
                    bytes: bytes.into_iter().map(|b| b as u32).collect(),
                })
            },
            yinz_types::input::actions::Action::WriteCharsToPaneId { chars, pane_id } => {
                ActionType::WriteCharsToPaneId(WriteCharsToPaneIdAction {
                    pane_id: Some(pane_id.into()),
                    chars,
                })
            },
            yinz_types::input::actions::Action::Paste { chars, pane_id } => {
                ActionType::Paste(PasteAction {
                    chars,
                    pane_id: pane_id.map(|p| p.into()),
                })
            },
            yinz_types::input::actions::Action::SwitchToMode { input_mode } => {
                ActionType::SwitchToMode(SwitchToModeAction {
                    input_mode: input_mode_to_proto_i32(input_mode),
                })
            },
            yinz_types::input::actions::Action::SwitchModeForAllClients { input_mode } => {
                ActionType::SwitchModeForAllClients(SwitchModeForAllClientsAction {
                    input_mode: input_mode_to_proto_i32(input_mode),
                })
            },
            yinz_types::input::actions::Action::Resize { resize, direction } => {
                ActionType::Resize(ResizeAction {
                    resize: resize_to_proto_i32(resize),
                    direction: direction.map(direction_to_proto_i32),
                })
            },
            yinz_types::input::actions::Action::FocusNextPane => {
                ActionType::FocusNextPane(FocusNextPaneAction {})
            },
            yinz_types::input::actions::Action::FocusPreviousPane => {
                ActionType::FocusPreviousPane(FocusPreviousPaneAction {})
            },
            yinz_types::input::actions::Action::FocusPaneTabs => {
                ActionType::FocusPaneTabs(FocusPaneTabsAction {})
            },
            yinz_types::input::actions::Action::PreviousPaneTab => {
                ActionType::PreviousPaneTab(PreviousPaneTabAction {})
            },
            yinz_types::input::actions::Action::NextPaneTab => {
                ActionType::NextPaneTab(NextPaneTabAction {})
            },
            yinz_types::input::actions::Action::FocusLastPane => {
                ActionType::FocusLastPane(FocusLastPaneAction {})
            },
            yinz_types::input::actions::Action::FocusHostSession => {
                ActionType::FocusHostSession(FocusHostSessionAction {})
            },
            yinz_types::input::actions::Action::FocusGuestSession => {
                ActionType::FocusGuestSession(FocusGuestSessionAction {})
            },
            yinz_types::input::actions::Action::ToggleHostFullscreen => {
                ActionType::ToggleHostFullscreen(ToggleHostFullscreenAction {})
            },
            yinz_types::input::actions::Action::SwitchFocus => {
                ActionType::SwitchFocus(SwitchFocusAction {})
            },
            yinz_types::input::actions::Action::MoveFocus { direction } => {
                ActionType::MoveFocus(MoveFocusAction {
                    direction: direction_to_proto_i32(direction),
                })
            },
            yinz_types::input::actions::Action::MoveFocusOrTab { direction } => {
                ActionType::MoveFocusOrTab(MoveFocusOrTabAction {
                    direction: direction_to_proto_i32(direction),
                })
            },
            yinz_types::input::actions::Action::MovePane { direction } => {
                ActionType::MovePane(MovePaneAction {
                    direction: direction.map(direction_to_proto_i32),
                })
            },
            yinz_types::input::actions::Action::MovePaneBackwards => {
                ActionType::MovePaneBackwards(MovePaneBackwardsAction {})
            },
            yinz_types::input::actions::Action::ClearScreen => {
                ActionType::ClearScreen(ClearScreenAction {})
            },
            yinz_types::input::actions::Action::DumpScreen {
                file_path,
                include_scrollback,
                pane_id,
                ansi,
            } => {
                let dump_to_stdout = file_path.is_none();
                ActionType::DumpScreen(DumpScreenAction {
                    file_path: file_path.unwrap_or_default(),
                    include_scrollback,
                    pane_id: pane_id.map(|p| p.into()),
                    dump_to_stdout,
                    ansi,
                })
            },
            yinz_types::input::actions::Action::DumpLayout => {
                ActionType::DumpLayout(DumpLayoutAction {})
            },
            yinz_types::input::actions::Action::EditScrollback { ansi } => {
                ActionType::EditScrollback(EditScrollbackAction { ansi })
            },
            yinz_types::input::actions::Action::ScrollUp => ActionType::ScrollUp(ScrollUpAction {}),
            yinz_types::input::actions::Action::ScrollUpAt { position } => {
                ActionType::ScrollUpAt(ScrollUpAtAction {
                    position: Some(position.try_into()?),
                })
            },
            yinz_types::input::actions::Action::ScrollDown => {
                ActionType::ScrollDown(ScrollDownAction {})
            },
            yinz_types::input::actions::Action::ScrollDownAt { position } => {
                ActionType::ScrollDownAt(ScrollDownAtAction {
                    position: Some(position.try_into()?),
                })
            },
            yinz_types::input::actions::Action::ScrollToBottom => {
                ActionType::ScrollToBottom(ScrollToBottomAction {})
            },
            yinz_types::input::actions::Action::ScrollToTop => {
                ActionType::ScrollToTop(ScrollToTopAction {})
            },
            yinz_types::input::actions::Action::ScrollToPreviousPrompt => {
                ActionType::ScrollToPreviousPrompt(ScrollToPreviousPromptAction {})
            },
            yinz_types::input::actions::Action::ScrollToNextPrompt => {
                ActionType::ScrollToNextPrompt(ScrollToNextPromptAction {})
            },
            yinz_types::input::actions::Action::SelectCommandAtScrollPosition => {
                ActionType::SelectCommandAtScrollPosition(SelectCommandAtScrollPositionAction {})
            },
            yinz_types::input::actions::Action::CopyLastCommandOutput => {
                ActionType::CopyLastCommandOutput(CopyLastCommandOutputAction {})
            },
            yinz_types::input::actions::Action::MoveTextSelection { direction } => {
                ActionType::MoveTextSelection(MoveTextSelectionAction {
                    direction: direction_to_proto_i32(direction),
                })
            },
            yinz_types::input::actions::Action::SwapTextSelectionEnds => {
                ActionType::SwapTextSelectionEnds(SwapTextSelectionEndsAction {})
            },
            yinz_types::input::actions::Action::MoveNotificationSelection { direction } => {
                ActionType::MoveNotificationSelection(MoveNotificationSelectionAction {
                    direction: direction_to_proto_i32(direction),
                })
            },
            yinz_types::input::actions::Action::ActivateNotification => {
                ActionType::ActivateNotification(ActivateNotificationAction {})
            },
            yinz_types::input::actions::Action::ToggleNotificationDetail => {
                ActionType::ToggleNotificationDetail(ToggleNotificationDetailAction {})
            },
            yinz_types::input::actions::Action::PageScrollUp => {
                ActionType::PageScrollUp(PageScrollUpAction {})
            },
            yinz_types::input::actions::Action::PageScrollDown => {
                ActionType::PageScrollDown(PageScrollDownAction {})
            },
            yinz_types::input::actions::Action::HalfPageScrollUp => {
                ActionType::HalfPageScrollUp(HalfPageScrollUpAction {})
            },
            yinz_types::input::actions::Action::HalfPageScrollDown => {
                ActionType::HalfPageScrollDown(HalfPageScrollDownAction {})
            },
            yinz_types::input::actions::Action::ToggleFocusFullscreen => {
                ActionType::ToggleFocusFullscreen(ToggleFocusFullscreenAction {})
            },
            yinz_types::input::actions::Action::ToggleFocusNoUiFullscreen => {
                ActionType::ToggleFocusNoUiFullscreen(ToggleFocusNoUiFullscreenAction {})
            },
            yinz_types::input::actions::Action::ToggleCommandPalette => {
                ActionType::ToggleCommandPalette(ToggleCommandPaletteAction {})
            },
            yinz_types::input::actions::Action::ToggleMother => {
                ActionType::ToggleMother(ToggleMotherAction {})
            },
            yinz_types::input::actions::Action::TogglePaneFrames => {
                ActionType::TogglePaneFrames(TogglePaneFramesAction {})
            },
            yinz_types::input::actions::Action::SetPaneFrameStyle(style) => {
                let style = match style {
                    yinz_types::input::options::PaneFrameStyle::Full => "full",
                    yinz_types::input::options::PaneFrameStyle::Titles => "titles",
                    yinz_types::input::options::PaneFrameStyle::None => "none",
                };
                ActionType::SetPaneFrameStyle(SetPaneFrameStyleAction {
                    style: style.to_owned(),
                })
            },
            yinz_types::input::actions::Action::ToggleActiveSyncTab => {
                ActionType::ToggleActiveSyncTab(ToggleActiveSyncTabAction {})
            },
            yinz_types::input::actions::Action::NewPane {
                direction,
                pane_name,
                start_suppressed,
            } => ActionType::NewPane(NewPaneAction {
                direction: direction.map(direction_to_proto_i32),
                pane_name,
                start_suppressed,
                near_current_pane: false,
            }),
            yinz_types::input::actions::Action::EditFile {
                payload,
                direction,
                floating,
                in_place,
                close_replaced_pane,
                start_suppressed,
                placement,
                near_current_pane,
                no_focus,
                tab_id,
            } => ActionType::EditFile(EditFileAction {
                payload: Some(payload.into()),
                direction: direction.map(direction_to_proto_i32),
                floating,
                in_place,
                close_replaced_pane,
                start_suppressed,
                placement: Some(placement.into()),
                near_current_pane,
                tab_id: tab_id
                    .map(|t| u32::try_from(t.get()))
                    .transpose()
                    .context("tab_id is outside u32 range")?,
                no_focus,
            }),
            yinz_types::input::actions::Action::NewFloatingPane {
                command,
                pane_name,
                placement,
                near_current_pane,
                no_focus,
                tab_id,
            } => ActionType::NewFloatingPane(NewFloatingPaneAction {
                command: command.map(|c| c.into()),
                pane_name,
                placement: Some(placement.into()),
                near_current_pane,
                tab_id: tab_id
                    .map(|t| u32::try_from(t.get()))
                    .transpose()
                    .context("tab_id is outside u32 range")?,
                no_focus,
            }),
            yinz_types::input::actions::Action::NewTiledPane {
                direction,
                command,
                pane_name,
                near_current_pane,
                no_focus,
                borderless,
                tab_id,
            } => ActionType::NewTiledPane(NewTiledPaneAction {
                direction: direction.map(direction_to_proto_i32),
                command: command.map(|c| c.into()),
                pane_name,
                near_current_pane,
                borderless,
                tab_id: tab_id
                    .map(|t| u32::try_from(t.get()))
                    .transpose()
                    .context("tab_id is outside u32 range")?,
                no_focus,
            }),
            yinz_types::input::actions::Action::NewInPlacePane {
                command,
                pane_name,
                near_current_pane,
                no_focus,
                pane_id_to_replace,
                close_replaced_pane,
                tab_id,
            } => ActionType::NewInPlacePane(NewInPlacePaneAction {
                command: command.map(|c| c.into()),
                pane_name,
                near_current_pane,
                pane_id_to_replace: pane_id_to_replace.map(Into::into),
                close_replaced_pane,
                tab_id: tab_id
                    .map(|t| u32::try_from(t.get()))
                    .transpose()
                    .context("tab_id is outside u32 range")?,
                no_focus,
            }),
            yinz_types::input::actions::Action::NewStackedPane {
                command,
                pane_name,
                near_current_pane,
                no_focus,
                tab_id,
            } => ActionType::NewStackedPane(NewStackedPaneAction {
                command: command.map(|c| c.into()),
                pane_name,
                near_current_pane,
                tab_id: tab_id
                    .map(|t| u32::try_from(t.get()))
                    .transpose()
                    .context("tab_id is outside u32 range")?,
                no_focus,
            }),
            yinz_types::input::actions::Action::NewBlockingPane {
                placement,
                pane_name,
                command,
                unblock_condition,
                near_current_pane,
                no_focus,
                tab_id,
            } => ActionType::NewBlockingPane(NewBlockingPaneAction {
                placement: Some(placement.into()),
                pane_name,
                command: command.map(|c| c.into()),
                unblock_condition: unblock_condition.map(unblock_condition_to_proto_i32),
                near_current_pane,
                tab_id: tab_id
                    .map(|t| u32::try_from(t.get()))
                    .transpose()
                    .context("tab_id is outside u32 range")?,
                no_focus,
            }),
            yinz_types::input::actions::Action::TogglePaneEmbedOrFloating => {
                ActionType::TogglePaneEmbedOrFloating(TogglePaneEmbedOrFloatingAction {})
            },
            yinz_types::input::actions::Action::ToggleFloatingPanes => {
                ActionType::ToggleFloatingPanes(ToggleFloatingPanesAction {})
            },
            yinz_types::input::actions::Action::ShowFloatingPanes { tab_id } => {
                ActionType::ShowFloatingPanes(ShowFloatingPanesAction {
                    tab_id: tab_id
                        .map(|id| u32::try_from(id.get()))
                        .transpose()
                        .context("tab_id is outside u32 range")?,
                })
            },
            yinz_types::input::actions::Action::HideFloatingPanes { tab_id } => {
                ActionType::HideFloatingPanes(HideFloatingPanesAction {
                    tab_id: tab_id
                        .map(|id| u32::try_from(id.get()))
                        .transpose()
                        .context("tab_id is outside u32 range")?,
                })
            },
            yinz_types::input::actions::Action::AreFloatingPanesVisible { tab_id } => {
                ActionType::AreFloatingPanesVisible(AreFloatingPanesVisibleAction {
                    tab_id: tab_id
                        .map(|id| u32::try_from(id.get()))
                        .transpose()
                        .context("tab_id is outside u32 range")?,
                })
            },
            yinz_types::input::actions::Action::CloseFocus => {
                ActionType::CloseFocus(CloseFocusAction {})
            },
            yinz_types::input::actions::Action::PaneNameInput { input } => {
                ActionType::PaneNameInput(PaneNameInputAction {
                    input: input.into_iter().map(|b| b as u32).collect(),
                })
            },
            yinz_types::input::actions::Action::UndoRenamePane => {
                ActionType::UndoRenamePane(UndoRenamePaneAction {})
            },
            yinz_types::input::actions::Action::NewTab {
                layout_source,
                tiled_layout,
                floating_layouts,
                swap_tiled_layouts,
                swap_floating_layouts,
                tab_name,
                should_change_focus_to_new_tab,
                cwd,
                initial_panes,
                first_pane_unblock_condition,
            } => ActionType::NewTab(NewTabAction {
                layout_source: layout_source.map(|p| p.to_string_lossy().into_owned()),
                tiled_layout: tiled_layout.map(|l| l.into()),
                floating_layouts: floating_layouts.into_iter().map(|l| l.into()).collect(),
                swap_tiled_layouts: swap_tiled_layouts
                    .map(|layouts| layouts.into_iter().map(|l| l.into()).collect())
                    .unwrap_or_default(),
                swap_floating_layouts: swap_floating_layouts
                    .map(|layouts| layouts.into_iter().map(|l| l.into()).collect())
                    .unwrap_or_default(),
                tab_name,
                should_change_focus_to_new_tab,
                cwd: cwd.map(|p| p.to_string_lossy().to_string()),
                initial_panes: initial_panes
                    .map(|panes| panes.into_iter().map(|p| p.into()).collect())
                    .unwrap_or_default(),
                first_pane_unblock_condition: first_pane_unblock_condition
                    .map(unblock_condition_to_proto_i32),
            }),
            yinz_types::input::actions::Action::NoOp => ActionType::NoOp(NoOpAction {}),
            yinz_types::input::actions::Action::GoToNextTab => {
                ActionType::GoToNextTab(GoToNextTabAction {})
            },
            yinz_types::input::actions::Action::GoToPreviousTab => {
                ActionType::GoToPreviousTab(GoToPreviousTabAction {})
            },
            yinz_types::input::actions::Action::CloseTab => ActionType::CloseTab(CloseTabAction {}),
            yinz_types::input::actions::Action::GoToTab { index } => {
                ActionType::GoToTab(GoToTabAction { index: index.get() })
            },
            yinz_types::input::actions::Action::GoToTabName { name, create } => {
                ActionType::GoToTabName(GoToTabNameAction { name, create })
            },
            yinz_types::input::actions::Action::ToggleTab => {
                ActionType::ToggleTab(ToggleTabAction {})
            },
            yinz_types::input::actions::Action::TabNameInput { input } => {
                ActionType::TabNameInput(TabNameInputAction {
                    input: input.into_iter().map(|b| b as u32).collect(),
                })
            },
            yinz_types::input::actions::Action::UndoRenameTab => {
                ActionType::UndoRenameTab(UndoRenameTabAction {})
            },
            yinz_types::input::actions::Action::MoveTab { direction } => {
                ActionType::MoveTab(MoveTabAction {
                    direction: direction_to_proto_i32(direction),
                })
            },
            yinz_types::input::actions::Action::Run {
                command,
                near_current_pane,
                no_focus,
            } => ActionType::Run(RunAction {
                command: Some(command.into()),
                near_current_pane,
                no_focus,
            }),
            yinz_types::input::actions::Action::Detach => ActionType::Detach(DetachAction {}),
            yinz_types::input::actions::Action::SetDarkTheme => {
                ActionType::SetDarkTheme(SetDarkThemeAction {})
            },
            yinz_types::input::actions::Action::SetLightTheme => {
                ActionType::SetLightTheme(SetLightThemeAction {})
            },
            yinz_types::input::actions::Action::ToggleTheme => {
                ActionType::ToggleTheme(ToggleThemeAction {})
            },
            yinz_types::input::actions::Action::SwitchSession {
                name,
                tab_position,
                pane_id,
                layout,
                cwd,
            } => ActionType::SwitchSession(SwitchSessionAction {
                name: name.to_string(),
                tab_position: tab_position.map(|p| p.get()),
                pane_id: pane_id.map(|pane| {
                    let (pane_id, is_plugin) = pane.legacy_parts();
                    PaneIdWithPlugin { pane_id, is_plugin }
                }),
                layout: layout.as_ref().map(|l| l.clone().into()),
                cwd: cwd.as_ref().map(|p| p.to_string_lossy().to_string()),
            }),
            yinz_types::input::actions::Action::WarpPane {
                pane_id,
                to_session,
                placement,
            } => ActionType::WarpPane(WarpPaneAction {
                to_session: to_session.to_string(),
                pane_id: pane_id.map(|pane_id| pane_id.into()),
                placement: ProtobufWarpPlacement::from(placement) as i32,
            }),
            yinz_types::input::actions::Action::AdoptPane { payload, placement } => {
                ActionType::AdoptPane(AdoptPaneAction {
                    bracketed_paste: payload.bracketed_paste,
                    agent_input: payload
                        .agent_input
                        .as_ref()
                        .map(warped_agent_input_to_protobuf),
                    held: payload.held.as_ref().map(held_to_protobuf),
                    group: payload.group.as_deref().map(warped_tab_to_protobuf),
                    host_id: payload.host_id.clone().into_string(),
                    control_socket: payload.control_socket.to_string_lossy().to_string(),
                    title: payload.title.clone(),
                    caption: payload.caption.clone(),
                    contents: payload.contents.clone(),
                    agent_theme_json: payload
                        .agent_theme
                        .as_ref()
                        .and_then(|m| serde_json::to_string(m).ok()),
                    borderless: payload.borderless,
                    placement: ProtobufWarpPlacement::from(placement) as i32,
                    remote: payload.remote.as_ref().map(remote_handoff_to_protobuf),
                    agent: payload.agent.clone().map(agent_info_to_proto),
                })
            },
            yinz_types::input::actions::Action::WarpTab { tab_id, to_session } => {
                ActionType::WarpTab(WarpTabAction {
                    to_session: to_session.to_string(),
                    tab_id: tab_id
                        .map(|i| u32::try_from(i.get()))
                        .transpose()
                        .context("tab_id is outside u32 range")?,
                })
            },
            yinz_types::input::actions::Action::WarpTabFrom {
                from_session,
                tab_id,
            } => ActionType::WarpTabFrom(WarpTabFromAction {
                from_session: from_session.to_string(),
                tab_id: u32::try_from(tab_id.get()).context("tab_id is outside u32 range")?,
            }),
            yinz_types::input::actions::Action::AdoptTab { payload } => {
                ActionType::AdoptTab(warped_tab_to_protobuf(&payload))
            },
            yinz_types::input::actions::Action::LaunchOrFocusPlugin {
                plugin,
                should_float,
                move_to_focused_tab,
                should_open_in_place,
                close_replaced_pane,
                skip_cache,
                tab_id,
            } => ActionType::LaunchOrFocusPlugin(LaunchOrFocusPluginAction {
                plugin: Some(plugin.into()),
                should_float,
                move_to_focused_tab,
                should_open_in_place,
                close_replaced_pane,
                skip_cache,
                tab_id: tab_id
                    .map(|t| u32::try_from(t.get()))
                    .transpose()
                    .context("tab_id is outside u32 range")?,
            }),
            yinz_types::input::actions::Action::LaunchPlugin {
                plugin,
                should_float,
                should_open_in_place,
                close_replaced_pane,
                skip_cache,
                cwd,
                no_focus,
                tab_id,
            } => ActionType::LaunchPlugin(LaunchPluginAction {
                plugin: Some(plugin.into()),
                should_float,
                should_open_in_place,
                close_replaced_pane,
                skip_cache,
                cwd: cwd.map(|p| p.to_string_lossy().to_string()),
                tab_id: tab_id
                    .map(|t| u32::try_from(t.get()))
                    .transpose()
                    .context("tab_id is outside u32 range")?,
                no_focus,
            }),
            yinz_types::input::actions::Action::MouseEvent { event } => {
                ActionType::MouseEvent(MouseEventAction {
                    event: Some(event.try_into()?),
                })
            },
            yinz_types::input::actions::Action::Copy => ActionType::Copy(CopyAction {}),
            yinz_types::input::actions::Action::Confirm => ActionType::Confirm(ConfirmAction {}),
            yinz_types::input::actions::Action::Deny => ActionType::Deny(DenyAction {}),
            yinz_types::input::actions::Action::SkipConfirm { action } => {
                ActionType::SkipConfirm(Box::new(SkipConfirmAction {
                    action: Some(Box::new((*action).try_into()?)),
                }))
            },
            yinz_types::input::actions::Action::SearchInput { input } => {
                ActionType::SearchInput(SearchInputAction {
                    input: input.into_iter().map(|b| b as u32).collect(),
                })
            },
            yinz_types::input::actions::Action::Search { direction } => {
                ActionType::Search(SearchAction {
                    direction: search_direction_to_proto_i32(direction),
                })
            },
            yinz_types::input::actions::Action::SearchToggleOption { option } => {
                ActionType::SearchToggleOption(SearchToggleOptionAction {
                    option: search_option_to_proto_i32(option),
                })
            },
            yinz_types::input::actions::Action::ToggleMouseMode => {
                ActionType::ToggleMouseMode(ToggleMouseModeAction {})
            },
            yinz_types::input::actions::Action::PreviousSwapLayout => {
                ActionType::PreviousSwapLayout(PreviousSwapLayoutAction {})
            },
            yinz_types::input::actions::Action::NextSwapLayout => {
                ActionType::NextSwapLayout(NextSwapLayoutAction {})
            },
            yinz_types::input::actions::Action::OverrideLayout {
                tabs,
                retain_existing_terminal_panes,
                retain_existing_plugin_panes,
                apply_only_to_active_tab,
            } => ActionType::OverrideLayout(OverrideLayoutAction {
                tabs: tabs
                    .into_iter()
                    .map(|t| t.try_into())
                    .collect::<Result<_, _>>()?,
                retain_existing_terminal_panes,
                retain_existing_plugin_panes,
                apply_only_to_active_tab,
            }),
            yinz_types::input::actions::Action::QueryTabNames => {
                ActionType::QueryTabNames(QueryTabNamesAction {})
            },
            yinz_types::input::actions::Action::NewTiledPluginPane {
                plugin,
                pane_name,
                skip_cache,
                cwd,
                no_focus,
                tab_id,
                ..
            } => ActionType::NewTiledPluginPane(NewTiledPluginPaneAction {
                plugin: Some(plugin.into()),
                pane_name,
                skip_cache,
                cwd: cwd.map(|p| p.to_string_lossy().to_string()),
                tab_id: tab_id
                    .map(|t| u32::try_from(t.get()))
                    .transpose()
                    .context("tab_id is outside u32 range")?,
                no_focus,
            }),
            yinz_types::input::actions::Action::NewFloatingPluginPane {
                plugin,
                pane_name,
                skip_cache,
                cwd,
                placement,
                no_focus,
                tab_id,
                ..
            } => ActionType::NewFloatingPluginPane(NewFloatingPluginPaneAction {
                plugin: Some(plugin.into()),
                pane_name,
                skip_cache,
                cwd: cwd.map(|p| p.to_string_lossy().to_string()),
                placement: Some(placement.into()),
                tab_id: tab_id
                    .map(|t| u32::try_from(t.get()))
                    .transpose()
                    .context("tab_id is outside u32 range")?,
                no_focus,
            }),
            yinz_types::input::actions::Action::NewInPlacePluginPane {
                plugin,
                pane_name,
                skip_cache,
                close_replaced_pane,
                no_focus,
                tab_id,
                ..
            } => ActionType::NewInPlacePluginPane(NewInPlacePluginPaneAction {
                plugin: Some(plugin.into()),
                pane_name,
                skip_cache,
                close_replaced_pane,
                tab_id: tab_id
                    .map(|t| u32::try_from(t.get()))
                    .transpose()
                    .context("tab_id is outside u32 range")?,
                no_focus,
            }),
            yinz_types::input::actions::Action::StartOrReloadPlugin { plugin } => {
                ActionType::StartOrReloadPlugin(StartOrReloadPluginAction {
                    plugin: Some(plugin.into()),
                })
            },
            yinz_types::input::actions::Action::CloseTerminalPane { pane_id } => {
                ActionType::CloseTerminalPane(CloseTerminalPaneAction {
                    pane_id: pane_id.get(),
                })
            },
            yinz_types::input::actions::Action::ClosePluginPane { pane_id } => {
                ActionType::ClosePluginPane(ClosePluginPaneAction {
                    pane_id: pane_id.get(),
                })
            },
            yinz_types::input::actions::Action::FocusTerminalPaneWithId {
                pane_id,
                should_float_if_hidden,
                should_be_in_place_if_hidden,
            } => ActionType::FocusTerminalPaneWithId(FocusTerminalPaneWithIdAction {
                pane_id: pane_id.get(),
                should_float_if_hidden,
                should_be_in_place_if_hidden,
            }),
            yinz_types::input::actions::Action::FocusPluginPaneWithId {
                pane_id,
                should_float_if_hidden,
                should_be_in_place_if_hidden,
            } => ActionType::FocusPluginPaneWithId(FocusPluginPaneWithIdAction {
                pane_id: pane_id.get(),
                should_float_if_hidden,
                should_be_in_place_if_hidden,
            }),
            yinz_types::input::actions::Action::RenameTerminalPane { pane_id, name } => {
                ActionType::RenameTerminalPane(RenameTerminalPaneAction {
                    pane_id: pane_id.get(),
                    name: name.into_iter().map(|b| b as u32).collect(),
                })
            },
            yinz_types::input::actions::Action::RenamePluginPane { pane_id, name } => {
                ActionType::RenamePluginPane(RenamePluginPaneAction {
                    pane_id: pane_id.get(),
                    name: name.into_iter().map(|b| b as u32).collect(),
                })
            },
            yinz_types::input::actions::Action::RenameTab { tab_index, name } => {
                ActionType::RenameTab(RenameTabAction {
                    tab_index: tab_index.get(),
                    name: name.into_iter().map(|b| b as u32).collect(),
                })
            },
            yinz_types::input::actions::Action::GoToTabById { id } => {
                ActionType::GoToTabById(GoToTabByIdAction {
                    id: id.get() as u64,
                })
            },
            yinz_types::input::actions::Action::CloseTabById { id } => {
                ActionType::CloseTabById(CloseTabByIdAction {
                    id: id.get() as u64,
                })
            },
            yinz_types::input::actions::Action::RenameTabById { id, name } => {
                ActionType::RenameTabById(RenameTabByIdAction {
                    id: id.get() as u64,
                    name,
                })
            },
            yinz_types::input::actions::Action::ArrangePanesByProject => {
                ActionType::ArrangePanesByProject(ArrangePanesByProjectAction {})
            },
            yinz_types::input::actions::Action::BreakPane => {
                ActionType::BreakPane(BreakPaneAction {})
            },
            yinz_types::input::actions::Action::BreakPaneRight => {
                ActionType::BreakPaneRight(BreakPaneRightAction {})
            },
            yinz_types::input::actions::Action::BreakPaneLeft => {
                ActionType::BreakPaneLeft(BreakPaneLeftAction {})
            },
            yinz_types::input::actions::Action::RenameSession { name } => {
                ActionType::RenameSession(RenameSessionAction {
                    name: name.to_string(),
                })
            },
            yinz_types::input::actions::Action::Notify(text) => {
                ActionType::Notify(NotifyAction { text })
            },
            yinz_types::input::actions::Action::CliPipe {
                focus_plugin,
                pipe_id,
                name,
                payload,
                args,
                plugin,
                configuration,
                launch_new,
                skip_cache,
                floating,
                in_place,
                cwd,
                pane_title,
            } => ActionType::CliPipe(CliPipeAction {
                focus_plugin,
                pipe_id,
                name,
                payload,
                args: args
                    .map(|a| a.into_iter().collect::<HashMap<_, _>>())
                    .unwrap_or_default(),
                plugin,
                configuration: configuration
                    .map(|c| c.into_iter().collect::<HashMap<_, _>>())
                    .unwrap_or_default(),
                launch_new,
                skip_cache,
                floating,
                in_place,
                cwd: cwd.map(|p| p.to_string_lossy().to_string()),
                pane_title,
            }),
            yinz_types::input::actions::Action::KeybindPipe {
                name,
                payload,
                args,
                plugin,
                plugin_id,
                configuration,
                launch_new,
                skip_cache,
                floating,
                in_place,
                cwd,
                pane_title,
            } => ActionType::KeybindPipe(KeybindPipeAction {
                name,
                payload,
                args: args
                    .map(|a| a.into_iter().collect::<HashMap<_, _>>())
                    .unwrap_or_default(),
                plugin,
                plugin_id: plugin_id.map(|id| id.get()),
                configuration: configuration
                    .map(|c| c.into_iter().collect::<HashMap<_, _>>())
                    .unwrap_or_default(),
                launch_new,
                skip_cache,
                floating,
                in_place,
                cwd: cwd.map(|p| p.to_string_lossy().to_string()),
                pane_title,
            }),
            yinz_types::input::actions::Action::ListClients => {
                ActionType::ListClients(ListClientsAction {})
            },
            yinz_types::input::actions::Action::ListPanes {
                show_tab,
                show_command,
                show_state,
                show_geometry,
                show_all,
                output_json,
            } => ActionType::ListPanes(ListPanesAction {
                show_tab,
                show_command,
                show_state,
                show_geometry,
                show_all,
                output_json,
            }),
            yinz_types::input::actions::Action::TogglePanePinned => {
                ActionType::TogglePanePinned(TogglePanePinnedAction {})
            },
            yinz_types::input::actions::Action::StackPanes { pane_ids } => {
                ActionType::StackPanes(StackPanesAction {
                    pane_ids: pane_ids.into_iter().map(|id| id.into()).collect(),
                })
            },
            yinz_types::input::actions::Action::ChangeFloatingPaneCoordinates {
                pane_id,
                coordinates,
            } => ActionType::ChangeFloatingPaneCoordinates(ChangeFloatingPaneCoordinatesAction {
                pane_id: Some(pane_id.into()),
                coordinates: Some(coordinates.into()),
            }),
            yinz_types::input::actions::Action::TogglePaneBorderless { pane_id } => {
                ActionType::TogglePaneBorderless(TogglePaneBorderlessAction {
                    pane_id: Some(pane_id.into()),
                })
            },
            yinz_types::input::actions::Action::SetPaneBorderless {
                pane_id,
                borderless,
            } => ActionType::SetPaneBorderless(SetPaneBorderlessAction {
                pane_id: Some(pane_id.into()),
                borderless,
            }),
            yinz_types::input::actions::Action::TogglePaneInGroup => {
                ActionType::TogglePaneInGroup(TogglePaneInGroupAction {})
            },
            yinz_types::input::actions::Action::ToggleGroupMarking => {
                ActionType::ToggleGroupMarking(ToggleGroupMarkingAction {})
            },
            yinz_types::input::actions::Action::SaveSession => {
                ActionType::SaveSession(SaveSessionAction {})
            },
            yinz_types::input::actions::Action::Reconfigure {
                config,
                write_to_disk,
            } => ActionType::Reconfigure(
                crate::client_server_contract::client_server_contract::ReconfigureAction {
                    config,
                    write_to_disk,
                },
            ),
            yinz_types::input::actions::Action::SetPaneAgent { pane_id, agent } => {
                ActionType::SetPaneAgent(
                    crate::client_server_contract::client_server_contract::SetPaneAgentAction {
                        pane_id: Some(pane_id.into()),
                        agent: agent.map(agent_info_to_proto),
                    },
                )
            },
            yinz_types::input::actions::Action::AgentEvent(event) => {
                ActionType::AgentEvent(agent_event_to_proto(event))
            },
            yinz_types::input::actions::Action::ListTabs {
                show_state,
                show_dimensions,
                show_panes,
                show_layout,
                show_all,
                output_json,
            } => ActionType::ListTabs(ListTabsAction {
                show_state,
                show_dimensions,
                show_panes,
                show_layout,
                show_all,
                output_json,
            }),
            yinz_types::input::actions::Action::CurrentTabInfo { output_json } => {
                ActionType::CurrentTabInfo(CurrentTabInfoAction { output_json })
            },
            yinz_types::input::actions::Action::SetPaneColor { pane_id, fg, bg } => {
                ActionType::SetPaneColor(SetPaneColorAction {
                    pane_id: Some(pane_id.into()),
                    fg,
                    bg,
                })
            },
            // Pane-targeting CLI-only variants
            yinz_types::input::actions::Action::ScrollUpByPaneId { pane_id } => {
                ActionType::ScrollUpByPaneId(ScrollUpByPaneIdAction {
                    pane_id: Some(pane_id.into()),
                })
            },
            yinz_types::input::actions::Action::ScrollDownByPaneId { pane_id } => {
                ActionType::ScrollDownByPaneId(ScrollDownByPaneIdAction {
                    pane_id: Some(pane_id.into()),
                })
            },
            yinz_types::input::actions::Action::ScrollToTopByPaneId { pane_id } => {
                ActionType::ScrollToTopByPaneId(ScrollToTopByPaneIdAction {
                    pane_id: Some(pane_id.into()),
                })
            },
            yinz_types::input::actions::Action::ScrollToBottomByPaneId { pane_id } => {
                ActionType::ScrollToBottomByPaneId(ScrollToBottomByPaneIdAction {
                    pane_id: Some(pane_id.into()),
                })
            },
            yinz_types::input::actions::Action::PageScrollUpByPaneId { pane_id } => {
                ActionType::PageScrollUpByPaneId(PageScrollUpByPaneIdAction {
                    pane_id: Some(pane_id.into()),
                })
            },
            yinz_types::input::actions::Action::PageScrollDownByPaneId { pane_id } => {
                ActionType::PageScrollDownByPaneId(PageScrollDownByPaneIdAction {
                    pane_id: Some(pane_id.into()),
                })
            },
            yinz_types::input::actions::Action::HalfPageScrollUpByPaneId { pane_id } => {
                ActionType::HalfPageScrollUpByPaneId(HalfPageScrollUpByPaneIdAction {
                    pane_id: Some(pane_id.into()),
                })
            },
            yinz_types::input::actions::Action::HalfPageScrollDownByPaneId { pane_id } => {
                ActionType::HalfPageScrollDownByPaneId(HalfPageScrollDownByPaneIdAction {
                    pane_id: Some(pane_id.into()),
                })
            },
            yinz_types::input::actions::Action::ResizeByPaneId {
                pane_id,
                resize,
                direction,
            } => ActionType::ResizeByPaneId(ResizeByPaneIdAction {
                pane_id: Some(pane_id.into()),
                resize_action: Some(ResizeAction {
                    resize: resize_to_proto_i32(resize),
                    direction: direction.map(direction_to_proto_i32),
                }),
            }),
            yinz_types::input::actions::Action::MovePaneByPaneId { pane_id, direction } => {
                ActionType::MovePaneByPaneId(MovePaneByPaneIdAction {
                    pane_id: Some(pane_id.into()),
                    direction: direction.map(direction_to_proto_i32),
                })
            },
            yinz_types::input::actions::Action::MovePaneBackwardsByPaneId { pane_id } => {
                ActionType::MovePaneBackwardsByPaneId(MovePaneBackwardsByPaneIdAction {
                    pane_id: Some(pane_id.into()),
                })
            },
            yinz_types::input::actions::Action::ClearScreenByPaneId { pane_id } => {
                ActionType::ClearScreenByPaneId(ClearScreenByPaneIdAction {
                    pane_id: Some(pane_id.into()),
                })
            },
            yinz_types::input::actions::Action::EditScrollbackByPaneId { pane_id, ansi } => {
                ActionType::EditScrollbackByPaneId(EditScrollbackByPaneIdAction {
                    pane_id: Some(pane_id.into()),
                    ansi,
                })
            },
            yinz_types::input::actions::Action::ToggleFocusFullscreenByPaneId { pane_id } => {
                ActionType::ToggleFullscreenByPaneId(ToggleFullscreenByPaneIdAction {
                    pane_id: Some(pane_id.into()),
                })
            },
            yinz_types::input::actions::Action::ToggleFocusNoUiFullscreenByPaneId { pane_id } => {
                ActionType::ToggleNoUiFullscreenByPaneId(ToggleNoUiFullscreenByPaneIdAction {
                    pane_id: Some(pane_id.into()),
                })
            },
            yinz_types::input::actions::Action::TogglePaneEmbedOrFloatingByPaneId { pane_id } => {
                ActionType::TogglePaneEmbedOrFloatingByPaneId(
                    TogglePaneEmbedOrFloatingByPaneIdAction {
                        pane_id: Some(pane_id.into()),
                    },
                )
            },
            yinz_types::input::actions::Action::CloseFocusByPaneId { pane_id } => {
                ActionType::CloseFocusByPaneId(CloseFocusByPaneIdAction {
                    pane_id: Some(pane_id.into()),
                })
            },
            yinz_types::input::actions::Action::RenamePaneByPaneId { pane_id, name } => {
                ActionType::RenamePaneByPaneId(RenamePaneByPaneIdAction {
                    pane_id: pane_id.map(|id| id.into()),
                    name,
                })
            },
            yinz_types::input::actions::Action::UndoRenamePaneByPaneId { pane_id } => {
                ActionType::UndoRenamePaneByPaneId(UndoRenamePaneByPaneIdAction {
                    pane_id: Some(pane_id.into()),
                })
            },
            yinz_types::input::actions::Action::TogglePanePinnedByPaneId { pane_id } => {
                ActionType::TogglePanePinnedByPaneId(TogglePanePinnedByPaneIdAction {
                    pane_id: Some(pane_id.into()),
                })
            },
            yinz_types::input::actions::Action::FocusPaneByPaneId { pane_id } => {
                ActionType::FocusPaneByPaneId(FocusPaneByPaneIdAction {
                    pane_id: Some(pane_id.into()),
                })
            },
            // Tab-targeting CLI-only variants
            yinz_types::input::actions::Action::UndoRenameTabByTabId { id } => {
                ActionType::UndoRenameTabByTabId(UndoRenameTabByTabIdAction {
                    id: id.get() as u64,
                })
            },
            yinz_types::input::actions::Action::ToggleActiveSyncTabByTabId { id } => {
                ActionType::ToggleActiveSyncTabByTabId(ToggleActiveSyncTabByTabIdAction {
                    id: id.get() as u64,
                })
            },
            yinz_types::input::actions::Action::ToggleFloatingPanesByTabId { id } => {
                ActionType::ToggleFloatingPanesByTabId(ToggleFloatingPanesByTabIdAction {
                    id: id.get() as u64,
                })
            },
            yinz_types::input::actions::Action::PreviousSwapLayoutByTabId { id } => {
                ActionType::PreviousSwapLayoutByTabId(PreviousSwapLayoutByTabIdAction {
                    id: id.get() as u64,
                })
            },
            yinz_types::input::actions::Action::NextSwapLayoutByTabId { id } => {
                ActionType::NextSwapLayoutByTabId(NextSwapLayoutByTabIdAction {
                    id: id.get() as u64,
                })
            },
            yinz_types::input::actions::Action::MoveTabByTabId { id, direction } => {
                ActionType::MoveTabByTabId(MoveTabByTabIdAction {
                    id: id.get() as u64,
                    direction: direction_to_proto_i32(direction),
                })
            },
        };

        Ok(Self {
            action_type: Some(action_type),
        })
    }
}

impl TryFrom<crate::client_server_contract::client_server_contract::Action>
    for yinz_types::input::actions::Action
{
    type Error = anyhow::Error;
    fn try_from(
        action: crate::client_server_contract::client_server_contract::Action,
    ) -> Result<Self> {
        use crate::client_server_contract::client_server_contract::action::ActionType;

        let action_type = action
            .action_type
            .ok_or_else(|| anyhow!("Action missing action_type"))?;

        match action_type {
            ActionType::Quit(_) => Ok(yinz_types::input::actions::Action::Quit),
            ActionType::Write(write_action) => Ok(yinz_types::input::actions::Action::Write {
                key_with_modifier: write_action
                    .key_with_modifier
                    .map(|k| k.try_into())
                    .transpose()?,
                bytes: write_action.bytes.into_iter().map(|b| b as u8).collect(),
                is_kitty_keyboard_protocol: write_action.is_kitty_keyboard_protocol,
            }),
            ActionType::WriteChars(write_chars_action) => {
                Ok(yinz_types::input::actions::Action::WriteChars {
                    chars: write_chars_action.chars,
                })
            },
            ActionType::WriteToPaneId(write_to_pane_id_action) => {
                Ok(yinz_types::input::actions::Action::WriteToPaneId {
                    bytes: write_to_pane_id_action
                        .bytes
                        .into_iter()
                        .map(|b| b as u8)
                        .collect(),
                    pane_id: write_to_pane_id_action
                        .pane_id
                        .ok_or_else(|| anyhow!("WriteToPaneId missing pane_id"))?
                        .try_into()?,
                })
            },
            ActionType::WriteCharsToPaneId(write_chars_to_pane_id_action) => {
                Ok(yinz_types::input::actions::Action::WriteCharsToPaneId {
                    chars: write_chars_to_pane_id_action.chars,
                    pane_id: write_chars_to_pane_id_action
                        .pane_id
                        .ok_or_else(|| anyhow!("WriteCharsToPaneId missing pane_id"))?
                        .try_into()?,
                })
            },
            ActionType::Paste(paste_action) => Ok(yinz_types::input::actions::Action::Paste {
                chars: paste_action.chars,
                pane_id: paste_action.pane_id.map(|p| p.try_into()).transpose()?,
            }),
            ActionType::SwitchToMode(switch_mode_action) => {
                Ok(yinz_types::input::actions::Action::SwitchToMode {
                    input_mode: proto_i32_to_input_mode(switch_mode_action.input_mode)?,
                })
            },
            ActionType::SwitchModeForAllClients(switch_mode_action) => Ok(
                yinz_types::input::actions::Action::SwitchModeForAllClients {
                    input_mode: proto_i32_to_input_mode(switch_mode_action.input_mode)?,
                },
            ),
            ActionType::Resize(resize_action) => Ok(yinz_types::input::actions::Action::Resize {
                resize: proto_i32_to_resize(resize_action.resize)?,
                direction: resize_action
                    .direction
                    .map(proto_i32_to_direction)
                    .transpose()?,
            }),
            ActionType::FocusNextPane(_) => Ok(yinz_types::input::actions::Action::FocusNextPane),
            ActionType::FocusPreviousPane(_) => {
                Ok(yinz_types::input::actions::Action::FocusPreviousPane)
            },
            ActionType::FocusPaneTabs(_) => Ok(yinz_types::input::actions::Action::FocusPaneTabs),
            ActionType::PreviousPaneTab(_) => {
                Ok(yinz_types::input::actions::Action::PreviousPaneTab)
            },
            ActionType::NextPaneTab(_) => Ok(yinz_types::input::actions::Action::NextPaneTab),
            ActionType::FocusLastPane(_) => Ok(yinz_types::input::actions::Action::FocusLastPane),
            ActionType::SwitchFocus(_) => Ok(yinz_types::input::actions::Action::SwitchFocus),
            ActionType::MoveFocus(move_focus_action) => {
                Ok(yinz_types::input::actions::Action::MoveFocus {
                    direction: proto_i32_to_direction(move_focus_action.direction)?,
                })
            },
            ActionType::MoveFocusOrTab(move_focus_action) => {
                Ok(yinz_types::input::actions::Action::MoveFocusOrTab {
                    direction: proto_i32_to_direction(move_focus_action.direction)?,
                })
            },
            ActionType::MovePane(move_pane_action) => {
                Ok(yinz_types::input::actions::Action::MovePane {
                    direction: move_pane_action
                        .direction
                        .map(proto_i32_to_direction)
                        .transpose()?,
                })
            },
            ActionType::MovePaneBackwards(_) => {
                Ok(yinz_types::input::actions::Action::MovePaneBackwards)
            },
            ActionType::ClearScreen(_) => Ok(yinz_types::input::actions::Action::ClearScreen),
            ActionType::DumpScreen(dump_screen_action) => {
                let file_path = if dump_screen_action.dump_to_stdout {
                    None
                } else {
                    Some(dump_screen_action.file_path)
                };
                Ok(yinz_types::input::actions::Action::DumpScreen {
                    file_path,
                    include_scrollback: dump_screen_action.include_scrollback,
                    pane_id: dump_screen_action.pane_id.and_then(|p| p.try_into().ok()),
                    ansi: dump_screen_action.ansi,
                })
            },
            ActionType::DumpLayout(_) => Ok(yinz_types::input::actions::Action::DumpLayout),
            ActionType::SaveSession(_) => Ok(yinz_types::input::actions::Action::SaveSession),
            ActionType::EditScrollback(edit_scrollback_action) => {
                Ok(yinz_types::input::actions::Action::EditScrollback {
                    ansi: edit_scrollback_action.ansi,
                })
            },
            ActionType::ScrollUp(_) => Ok(yinz_types::input::actions::Action::ScrollUp),
            ActionType::ScrollUpAt(scroll_action) => {
                Ok(yinz_types::input::actions::Action::ScrollUpAt {
                    position: scroll_action
                        .position
                        .ok_or_else(|| anyhow!("ScrollUpAt missing position"))?
                        .try_into()?,
                })
            },
            ActionType::ScrollDown(_) => Ok(yinz_types::input::actions::Action::ScrollDown),
            ActionType::ScrollDownAt(scroll_action) => {
                Ok(yinz_types::input::actions::Action::ScrollDownAt {
                    position: scroll_action
                        .position
                        .ok_or_else(|| anyhow!("ScrollDownAt missing position"))?
                        .try_into()?,
                })
            },
            ActionType::ScrollToBottom(_) => Ok(yinz_types::input::actions::Action::ScrollToBottom),
            ActionType::ScrollToTop(_) => Ok(yinz_types::input::actions::Action::ScrollToTop),
            ActionType::ScrollToPreviousPrompt(_) => {
                Ok(yinz_types::input::actions::Action::ScrollToPreviousPrompt)
            },
            ActionType::ScrollToNextPrompt(_) => {
                Ok(yinz_types::input::actions::Action::ScrollToNextPrompt)
            },
            ActionType::SelectCommandAtScrollPosition(_) => {
                Ok(yinz_types::input::actions::Action::SelectCommandAtScrollPosition)
            },
            ActionType::CopyLastCommandOutput(_) => {
                Ok(yinz_types::input::actions::Action::CopyLastCommandOutput)
            },
            ActionType::MoveTextSelection(action) => {
                Ok(yinz_types::input::actions::Action::MoveTextSelection {
                    direction: proto_i32_to_direction(action.direction)?,
                })
            },
            ActionType::SwapTextSelectionEnds(_) => {
                Ok(yinz_types::input::actions::Action::SwapTextSelectionEnds)
            },
            ActionType::MoveNotificationSelection(action) => Ok(
                yinz_types::input::actions::Action::MoveNotificationSelection {
                    direction: proto_i32_to_direction(action.direction)?,
                },
            ),
            ActionType::ActivateNotification(_) => {
                Ok(yinz_types::input::actions::Action::ActivateNotification)
            },
            ActionType::ToggleNotificationDetail(_) => {
                Ok(yinz_types::input::actions::Action::ToggleNotificationDetail)
            },
            ActionType::PageScrollUp(_) => Ok(yinz_types::input::actions::Action::PageScrollUp),
            ActionType::PageScrollDown(_) => Ok(yinz_types::input::actions::Action::PageScrollDown),
            ActionType::HalfPageScrollUp(_) => {
                Ok(yinz_types::input::actions::Action::HalfPageScrollUp)
            },
            ActionType::HalfPageScrollDown(_) => {
                Ok(yinz_types::input::actions::Action::HalfPageScrollDown)
            },
            ActionType::ToggleFocusFullscreen(_) => {
                Ok(yinz_types::input::actions::Action::ToggleFocusFullscreen)
            },
            ActionType::ToggleFocusNoUiFullscreen(_) => {
                Ok(yinz_types::input::actions::Action::ToggleFocusNoUiFullscreen)
            },
            ActionType::ToggleCommandPalette(_) => {
                Ok(yinz_types::input::actions::Action::ToggleCommandPalette)
            },
            ActionType::ToggleMother(_) => Ok(yinz_types::input::actions::Action::ToggleMother),
            ActionType::TogglePaneFrames(_) => {
                Ok(yinz_types::input::actions::Action::TogglePaneFrames)
            },
            ActionType::SetPaneFrameStyle(set_pane_frame_style_action) => {
                let style = yinz_types::input::options::PaneFrameStyle::from_str(
                    &set_pane_frame_style_action.style,
                )
                .map_err(|e| anyhow!("{}", e))?;
                Ok(yinz_types::input::actions::Action::SetPaneFrameStyle(style))
            },
            ActionType::ToggleActiveSyncTab(_) => {
                Ok(yinz_types::input::actions::Action::ToggleActiveSyncTab)
            },
            ActionType::NewPane(new_pane_action) => {
                Ok(yinz_types::input::actions::Action::NewPane {
                    direction: new_pane_action
                        .direction
                        .map(proto_i32_to_direction)
                        .transpose()?,
                    pane_name: new_pane_action.pane_name,
                    start_suppressed: new_pane_action.start_suppressed,
                })
            },
            ActionType::EditFile(edit_file_action) => {
                Ok(yinz_types::input::actions::Action::EditFile {
                    payload: edit_file_action
                        .payload
                        .ok_or_else(|| anyhow!("EditFile missing payload"))?
                        .try_into()?,
                    direction: edit_file_action
                        .direction
                        .map(proto_i32_to_direction)
                        .transpose()?,
                    floating: edit_file_action.floating,
                    in_place: edit_file_action.in_place,
                    close_replaced_pane: edit_file_action.close_replaced_pane,
                    start_suppressed: edit_file_action.start_suppressed,
                    placement: edit_file_action
                        .placement
                        .ok_or_else(|| anyhow!("EditFile missing placement"))?
                        .try_into()?,
                    near_current_pane: edit_file_action.near_current_pane,
                    no_focus: edit_file_action.no_focus,
                    tab_id: edit_file_action.tab_id.map(|t| TabId::new(t as usize)),
                })
            },
            ActionType::NewFloatingPane(new_floating_action) => {
                Ok(yinz_types::input::actions::Action::NewFloatingPane {
                    command: new_floating_action
                        .command
                        .map(|c| c.try_into())
                        .transpose()?,
                    pane_name: new_floating_action.pane_name,
                    placement: new_floating_action
                        .placement
                        .ok_or_else(|| anyhow!("NewFloatingPane missing placement"))?
                        .try_into()?,
                    near_current_pane: new_floating_action.near_current_pane,
                    no_focus: new_floating_action.no_focus,
                    tab_id: new_floating_action.tab_id.map(|t| TabId::new(t as usize)),
                })
            },
            ActionType::NewTiledPane(new_tiled_action) => {
                Ok(yinz_types::input::actions::Action::NewTiledPane {
                    direction: new_tiled_action
                        .direction
                        .map(proto_i32_to_direction)
                        .transpose()?,
                    command: new_tiled_action.command.map(|c| c.try_into()).transpose()?,
                    pane_name: new_tiled_action.pane_name,
                    near_current_pane: new_tiled_action.near_current_pane,
                    no_focus: new_tiled_action.no_focus,
                    borderless: new_tiled_action.borderless,
                    tab_id: new_tiled_action.tab_id.map(|t| TabId::new(t as usize)),
                })
            },
            ActionType::NewInPlacePane(new_in_place_action) => {
                Ok(yinz_types::input::actions::Action::NewInPlacePane {
                    command: new_in_place_action
                        .command
                        .map(|c| c.try_into())
                        .transpose()?,
                    pane_name: new_in_place_action.pane_name,
                    near_current_pane: new_in_place_action.near_current_pane,
                    no_focus: new_in_place_action.no_focus,
                    pane_id_to_replace: new_in_place_action
                        .pane_id_to_replace
                        .and_then(|p| p.try_into().ok()),
                    close_replaced_pane: new_in_place_action.close_replaced_pane,
                    tab_id: new_in_place_action.tab_id.map(|t| TabId::new(t as usize)),
                })
            },
            ActionType::NewStackedPane(new_stacked_action) => {
                Ok(yinz_types::input::actions::Action::NewStackedPane {
                    command: new_stacked_action
                        .command
                        .map(|c| c.try_into())
                        .transpose()?,
                    pane_name: new_stacked_action.pane_name,
                    near_current_pane: new_stacked_action.near_current_pane,
                    no_focus: new_stacked_action.no_focus,
                    tab_id: new_stacked_action.tab_id.map(|t| TabId::new(t as usize)),
                })
            },
            ActionType::NewBlockingPane(new_blocking_action) => {
                Ok(yinz_types::input::actions::Action::NewBlockingPane {
                    placement: new_blocking_action
                        .placement
                        .ok_or_else(|| anyhow!("NewBlockingPane missing placement"))?
                        .try_into()?,
                    pane_name: new_blocking_action.pane_name,
                    command: new_blocking_action
                        .command
                        .map(|c| c.try_into())
                        .transpose()?,
                    unblock_condition: new_blocking_action
                        .unblock_condition
                        .map(proto_i32_to_unblock_condition)
                        .transpose()?,
                    near_current_pane: new_blocking_action.near_current_pane,
                    no_focus: new_blocking_action.no_focus,
                    tab_id: new_blocking_action.tab_id.map(|t| TabId::new(t as usize)),
                })
            },
            ActionType::TogglePaneEmbedOrFloating(_) => {
                Ok(yinz_types::input::actions::Action::TogglePaneEmbedOrFloating)
            },
            ActionType::ToggleFloatingPanes(_) => {
                Ok(yinz_types::input::actions::Action::ToggleFloatingPanes)
            },
            ActionType::ShowFloatingPanes(a) => {
                Ok(yinz_types::input::actions::Action::ShowFloatingPanes {
                    tab_id: a.tab_id.map(|id| TabId::new(id as usize)),
                })
            },
            ActionType::HideFloatingPanes(a) => {
                Ok(yinz_types::input::actions::Action::HideFloatingPanes {
                    tab_id: a.tab_id.map(|id| TabId::new(id as usize)),
                })
            },
            ActionType::AreFloatingPanesVisible(a) => Ok(
                yinz_types::input::actions::Action::AreFloatingPanesVisible {
                    tab_id: a.tab_id.map(|id| TabId::new(id as usize)),
                },
            ),
            ActionType::CloseFocus(_) => Ok(yinz_types::input::actions::Action::CloseFocus),
            ActionType::PaneNameInput(pane_name_action) => {
                Ok(yinz_types::input::actions::Action::PaneNameInput {
                    input: pane_name_action
                        .input
                        .into_iter()
                        .map(|b| b as u8)
                        .collect(),
                })
            },
            ActionType::UndoRenamePane(_) => Ok(yinz_types::input::actions::Action::UndoRenamePane),
            ActionType::NewTab(new_tab_action) => Ok(yinz_types::input::actions::Action::NewTab {
                layout_source: new_tab_action.layout_source.map(PathBuf::from),
                tiled_layout: new_tab_action
                    .tiled_layout
                    .map(|l| l.try_into())
                    .transpose()?,
                floating_layouts: new_tab_action
                    .floating_layouts
                    .into_iter()
                    .map(|l| l.try_into())
                    .collect::<Result<Vec<_>>>()?,
                swap_tiled_layouts: if new_tab_action.swap_tiled_layouts.is_empty() {
                    None
                } else {
                    Some(
                        new_tab_action
                            .swap_tiled_layouts
                            .into_iter()
                            .map(|l| l.try_into())
                            .collect::<Result<Vec<_>>>()?,
                    )
                },
                swap_floating_layouts: if new_tab_action.swap_floating_layouts.is_empty() {
                    None
                } else {
                    Some(
                        new_tab_action
                            .swap_floating_layouts
                            .into_iter()
                            .map(|l| l.try_into())
                            .collect::<Result<Vec<_>>>()?,
                    )
                },
                tab_name: new_tab_action.tab_name,
                should_change_focus_to_new_tab: new_tab_action.should_change_focus_to_new_tab,
                cwd: new_tab_action.cwd.map(PathBuf::from),
                initial_panes: if new_tab_action.initial_panes.is_empty() {
                    None
                } else {
                    Some(
                        new_tab_action
                            .initial_panes
                            .into_iter()
                            .map(|p| p.try_into())
                            .collect::<Result<Vec<_>>>()?,
                    )
                },

                first_pane_unblock_condition: new_tab_action
                    .first_pane_unblock_condition
                    .map(proto_i32_to_unblock_condition)
                    .transpose()?,
            }),
            ActionType::NoOp(_) => Ok(yinz_types::input::actions::Action::NoOp),
            ActionType::GoToNextTab(_) => Ok(yinz_types::input::actions::Action::GoToNextTab),
            ActionType::GoToPreviousTab(_) => {
                Ok(yinz_types::input::actions::Action::GoToPreviousTab)
            },
            ActionType::CloseTab(_) => Ok(yinz_types::input::actions::Action::CloseTab),
            ActionType::GoToTab(go_to_tab_action) => {
                Ok(yinz_types::input::actions::Action::GoToTab {
                    index: TabNumber::new(go_to_tab_action.index),
                })
            },
            ActionType::GoToTabName(go_to_tab_name_action) => {
                Ok(yinz_types::input::actions::Action::GoToTabName {
                    name: go_to_tab_name_action.name,
                    create: go_to_tab_name_action.create,
                })
            },
            ActionType::ToggleTab(_) => Ok(yinz_types::input::actions::Action::ToggleTab),
            ActionType::TabNameInput(tab_name_action) => {
                Ok(yinz_types::input::actions::Action::TabNameInput {
                    input: tab_name_action.input.into_iter().map(|b| b as u8).collect(),
                })
            },
            ActionType::UndoRenameTab(_) => Ok(yinz_types::input::actions::Action::UndoRenameTab),
            ActionType::MoveTab(move_tab_action) => {
                Ok(yinz_types::input::actions::Action::MoveTab {
                    direction: proto_i32_to_direction(move_tab_action.direction)?,
                })
            },
            ActionType::Run(run_action) => Ok(yinz_types::input::actions::Action::Run {
                command: run_action
                    .command
                    .ok_or_else(|| anyhow!("Run missing command"))?
                    .try_into()?,
                near_current_pane: run_action.near_current_pane,
                no_focus: run_action.no_focus,
            }),
            ActionType::Detach(_) => Ok(yinz_types::input::actions::Action::Detach),
            ActionType::SetDarkTheme(_) => Ok(yinz_types::input::actions::Action::SetDarkTheme),
            ActionType::SetLightTheme(_) => Ok(yinz_types::input::actions::Action::SetLightTheme),
            ActionType::ToggleTheme(_) => Ok(yinz_types::input::actions::Action::ToggleTheme),
            ActionType::SwitchSession(switch_session_action) => {
                Ok(yinz_types::input::actions::Action::SwitchSession {
                    name: switch_session_action.name.clone().into(),
                    tab_position: switch_session_action.tab_position.map(TabNumber::new),
                    pane_id: switch_session_action
                        .pane_id
                        .as_ref()
                        .map(|p| PaneId::from_legacy(p.pane_id, p.is_plugin)),
                    layout: switch_session_action
                        .layout
                        .map(|l| l.try_into())
                        .transpose()?,
                    cwd: switch_session_action.cwd.map(PathBuf::from),
                })
            },
            ActionType::WarpPane(warp_pane_action) => {
                Ok(yinz_types::input::actions::Action::WarpPane {
                    to_session: warp_pane_action.to_session.clone().into(),
                    pane_id: warp_pane_action
                        .pane_id
                        .map(|pane_id| pane_id.try_into())
                        .transpose()?,
                    placement: warp_placement_from_i32(warp_pane_action.placement)?,
                })
            },
            ActionType::AdoptPane(adopt_pane_action) => {
                Ok(yinz_types::input::actions::Action::AdoptPane {
                    placement: warp_placement_from_i32(adopt_pane_action.placement)?,
                    payload: Box::new(yinz_types::data::WarpedPane {
                        bracketed_paste: adopt_pane_action.bracketed_paste,
                        agent_input: adopt_pane_action
                            .agent_input
                            .clone()
                            .map(warped_agent_input_from_protobuf),
                        held: adopt_pane_action
                            .held
                            .clone()
                            .map(held_from_protobuf)
                            .transpose()?,
                        group: adopt_pane_action
                            .group
                            .clone()
                            .map(pane_group_from_protobuf)
                            .transpose()?
                            .map(Box::new),
                        host_id: SupervisorId::new(adopt_pane_action.host_id.clone()),
                        control_socket: PathBuf::from(&adopt_pane_action.control_socket),
                        title: adopt_pane_action.title.clone(),
                        caption: adopt_pane_action.caption.clone(),
                        contents: adopt_pane_action.contents.clone(),
                        agent_theme: adopt_pane_action
                            .agent_theme_json
                            .as_deref()
                            .and_then(|m| serde_json::from_str(m).ok()),
                        borderless: adopt_pane_action.borderless,
                        remote: adopt_pane_action
                            .remote
                            .clone()
                            .and_then(remote_handoff_from_protobuf),
                        agent: adopt_pane_action
                            .agent
                            .clone()
                            .map(agent_info_from_proto)
                            .transpose()?,
                    }),
                })
            },
            ActionType::WarpTab(warp_tab_action) => {
                Ok(yinz_types::input::actions::Action::WarpTab {
                    to_session: warp_tab_action.to_session.clone().into(),
                    tab_id: warp_tab_action.tab_id.map(|i| TabId::new(i as usize)),
                })
            },
            ActionType::WarpTabFrom(warp_tab_from_action) => {
                Ok(yinz_types::input::actions::Action::WarpTabFrom {
                    from_session: warp_tab_from_action.from_session.clone().into(),
                    tab_id: TabId::try_from(u64::from(warp_tab_from_action.tab_id))
                        .context("tab_id is outside usize range")?,
                })
            },
            ActionType::AdoptTab(adopt_tab_action) => {
                Ok(yinz_types::input::actions::Action::AdoptTab {
                    payload: Box::new(warped_tab_from_protobuf(adopt_tab_action)?),
                })
            },
            ActionType::LaunchOrFocusPlugin(launch_plugin_action) => {
                Ok(yinz_types::input::actions::Action::LaunchOrFocusPlugin {
                    plugin: launch_plugin_action
                        .plugin
                        .ok_or_else(|| anyhow!("LaunchOrFocusPlugin missing plugin"))?
                        .try_into()?,
                    should_float: launch_plugin_action.should_float,
                    move_to_focused_tab: launch_plugin_action.move_to_focused_tab,
                    should_open_in_place: launch_plugin_action.should_open_in_place,
                    close_replaced_pane: launch_plugin_action.close_replaced_pane,
                    skip_cache: launch_plugin_action.skip_cache,
                    tab_id: launch_plugin_action.tab_id.map(|t| TabId::new(t as usize)),
                })
            },
            ActionType::LaunchPlugin(launch_plugin_action) => {
                Ok(yinz_types::input::actions::Action::LaunchPlugin {
                    plugin: launch_plugin_action
                        .plugin
                        .ok_or_else(|| anyhow!("LaunchPlugin missing plugin"))?
                        .try_into()?,
                    should_float: launch_plugin_action.should_float,
                    should_open_in_place: launch_plugin_action.should_open_in_place,
                    close_replaced_pane: launch_plugin_action.close_replaced_pane,
                    skip_cache: launch_plugin_action.skip_cache,
                    cwd: launch_plugin_action.cwd.map(PathBuf::from),
                    no_focus: launch_plugin_action.no_focus,
                    tab_id: launch_plugin_action.tab_id.map(|t| TabId::new(t as usize)),
                })
            },
            ActionType::MouseEvent(mouse_event_action) => {
                Ok(yinz_types::input::actions::Action::MouseEvent {
                    event: mouse_event_action
                        .event
                        .ok_or_else(|| anyhow!("MouseEvent missing event"))?
                        .try_into()?,
                })
            },
            ActionType::Copy(_) => Ok(yinz_types::input::actions::Action::Copy),
            ActionType::Confirm(_) => Ok(yinz_types::input::actions::Action::Confirm),
            ActionType::Deny(_) => Ok(yinz_types::input::actions::Action::Deny),
            ActionType::SkipConfirm(skip_confirm_action) => {
                Ok(yinz_types::input::actions::Action::SkipConfirm {
                    action: Box::new(
                        skip_confirm_action
                            .action
                            .ok_or_else(|| anyhow!("SkipConfirm missing action"))?
                            .as_ref()
                            .clone()
                            .try_into()?,
                    ),
                })
            },
            ActionType::SearchInput(search_input_action) => {
                Ok(yinz_types::input::actions::Action::SearchInput {
                    input: search_input_action
                        .input
                        .into_iter()
                        .map(|b| b as u8)
                        .collect(),
                })
            },
            ActionType::Search(search_action) => Ok(yinz_types::input::actions::Action::Search {
                direction: proto_i32_to_search_direction(search_action.direction)?,
            }),
            ActionType::SearchToggleOption(search_toggle_action) => {
                Ok(yinz_types::input::actions::Action::SearchToggleOption {
                    option: proto_i32_to_search_option(search_toggle_action.option)?,
                })
            },
            ActionType::ToggleMouseMode(_) => {
                Ok(yinz_types::input::actions::Action::ToggleMouseMode)
            },
            ActionType::PreviousSwapLayout(_) => {
                Ok(yinz_types::input::actions::Action::PreviousSwapLayout)
            },
            ActionType::NextSwapLayout(_) => Ok(yinz_types::input::actions::Action::NextSwapLayout),
            ActionType::OverrideLayout(override_layout_action) => {
                Ok(yinz_types::input::actions::Action::OverrideLayout {
                    tabs: override_layout_action
                        .tabs
                        .into_iter()
                        .map(|t| t.try_into())
                        .collect::<Result<Vec<_>>>()?,
                    retain_existing_terminal_panes: override_layout_action
                        .retain_existing_terminal_panes,
                    retain_existing_plugin_panes: override_layout_action
                        .retain_existing_plugin_panes,
                    apply_only_to_active_tab: override_layout_action.apply_only_to_active_tab,
                })
            },
            ActionType::QueryTabNames(_) => Ok(yinz_types::input::actions::Action::QueryTabNames),
            ActionType::NewTiledPluginPane(new_tiled_plugin_action) => {
                Ok(yinz_types::input::actions::Action::NewTiledPluginPane {
                    plugin: new_tiled_plugin_action
                        .plugin
                        .ok_or_else(|| anyhow!("NewTiledPluginPane missing plugin"))?
                        .try_into()?,
                    pane_name: new_tiled_plugin_action.pane_name,
                    skip_cache: new_tiled_plugin_action.skip_cache,
                    cwd: new_tiled_plugin_action.cwd.map(PathBuf::from),
                    no_focus: new_tiled_plugin_action.no_focus,
                    tab_id: new_tiled_plugin_action
                        .tab_id
                        .map(|t| TabId::new(t as usize)),
                })
            },
            ActionType::NewFloatingPluginPane(new_floating_plugin_action) => {
                Ok(yinz_types::input::actions::Action::NewFloatingPluginPane {
                    plugin: new_floating_plugin_action
                        .plugin
                        .ok_or_else(|| anyhow!("NewFloatingPluginPane missing plugin"))?
                        .try_into()?,
                    pane_name: new_floating_plugin_action.pane_name,
                    skip_cache: new_floating_plugin_action.skip_cache,
                    cwd: new_floating_plugin_action.cwd.map(PathBuf::from),
                    placement: new_floating_plugin_action
                        .placement
                        .ok_or_else(|| anyhow!("NewFloatingPluginPane missing placement"))?
                        .try_into()?,
                    no_focus: new_floating_plugin_action.no_focus,
                    tab_id: new_floating_plugin_action
                        .tab_id
                        .map(|t| TabId::new(t as usize)),
                })
            },
            ActionType::NewInPlacePluginPane(new_in_place_plugin_action) => {
                Ok(yinz_types::input::actions::Action::NewInPlacePluginPane {
                    plugin: new_in_place_plugin_action
                        .plugin
                        .ok_or_else(|| anyhow!("NewInPlacePluginPane missing plugin"))?
                        .try_into()?,
                    pane_name: new_in_place_plugin_action.pane_name,
                    skip_cache: new_in_place_plugin_action.skip_cache,
                    close_replaced_pane: new_in_place_plugin_action.close_replaced_pane,
                    no_focus: new_in_place_plugin_action.no_focus,
                    tab_id: new_in_place_plugin_action
                        .tab_id
                        .map(|t| TabId::new(t as usize)),
                })
            },
            ActionType::StartOrReloadPlugin(start_plugin_action) => {
                Ok(yinz_types::input::actions::Action::StartOrReloadPlugin {
                    plugin: start_plugin_action
                        .plugin
                        .ok_or_else(|| anyhow!("StartOrReloadPlugin missing plugin"))?
                        .try_into()?,
                })
            },
            ActionType::CloseTerminalPane(close_pane_action) => {
                Ok(yinz_types::input::actions::Action::CloseTerminalPane {
                    pane_id: yinz_types::internal_ids::TerminalId::new(close_pane_action.pane_id),
                })
            },
            ActionType::ClosePluginPane(close_pane_action) => {
                Ok(yinz_types::input::actions::Action::ClosePluginPane {
                    pane_id: yinz_types::internal_ids::PluginId::new(close_pane_action.pane_id),
                })
            },
            ActionType::FocusTerminalPaneWithId(focus_pane_action) => Ok(
                yinz_types::input::actions::Action::FocusTerminalPaneWithId {
                    pane_id: yinz_types::internal_ids::TerminalId::new(focus_pane_action.pane_id),
                    should_float_if_hidden: focus_pane_action.should_float_if_hidden,
                    should_be_in_place_if_hidden: focus_pane_action.should_be_in_place_if_hidden,
                },
            ),
            ActionType::FocusPluginPaneWithId(focus_pane_action) => {
                Ok(yinz_types::input::actions::Action::FocusPluginPaneWithId {
                    pane_id: yinz_types::internal_ids::PluginId::new(focus_pane_action.pane_id),
                    should_float_if_hidden: focus_pane_action.should_float_if_hidden,
                    should_be_in_place_if_hidden: focus_pane_action.should_be_in_place_if_hidden,
                })
            },
            ActionType::RenameTerminalPane(rename_pane_action) => {
                Ok(yinz_types::input::actions::Action::RenameTerminalPane {
                    pane_id: yinz_types::internal_ids::TerminalId::new(rename_pane_action.pane_id),
                    name: rename_pane_action
                        .name
                        .into_iter()
                        .map(|b| b as u8)
                        .collect(),
                })
            },
            ActionType::RenamePluginPane(rename_pane_action) => {
                Ok(yinz_types::input::actions::Action::RenamePluginPane {
                    pane_id: yinz_types::internal_ids::PluginId::new(rename_pane_action.pane_id),
                    name: rename_pane_action
                        .name
                        .into_iter()
                        .map(|b| b as u8)
                        .collect(),
                })
            },
            ActionType::RenameTab(rename_tab_action) => {
                Ok(yinz_types::input::actions::Action::RenameTab {
                    tab_index: TabNumber::new(rename_tab_action.tab_index),
                    name: rename_tab_action
                        .name
                        .into_iter()
                        .map(|b| b as u8)
                        .collect(),
                })
            },
            ActionType::GoToTabById(go_to_tab_by_id_action) => {
                Ok(yinz_types::input::actions::Action::GoToTabById {
                    id: TabId::try_from(go_to_tab_by_id_action.id)
                        .context("tab_id is outside usize range")?,
                })
            },
            ActionType::CloseTabById(close_tab_by_id_action) => {
                Ok(yinz_types::input::actions::Action::CloseTabById {
                    id: TabId::try_from(close_tab_by_id_action.id)
                        .context("tab_id is outside usize range")?,
                })
            },
            ActionType::RenameTabById(rename_tab_by_id_action) => {
                Ok(yinz_types::input::actions::Action::RenameTabById {
                    id: TabId::try_from(rename_tab_by_id_action.id)
                        .context("tab_id is outside usize range")?,
                    name: rename_tab_by_id_action.name,
                })
            },
            ActionType::ArrangePanesByProject(_) => {
                Ok(yinz_types::input::actions::Action::ArrangePanesByProject)
            },
            ActionType::BreakPane(_) => Ok(yinz_types::input::actions::Action::BreakPane),
            ActionType::BreakPaneRight(_) => Ok(yinz_types::input::actions::Action::BreakPaneRight),
            ActionType::BreakPaneLeft(_) => Ok(yinz_types::input::actions::Action::BreakPaneLeft),
            ActionType::FocusHostSession(_) => {
                Ok(yinz_types::input::actions::Action::FocusHostSession)
            },
            ActionType::FocusGuestSession(_) => {
                Ok(yinz_types::input::actions::Action::FocusGuestSession)
            },
            ActionType::ToggleHostFullscreen(_) => {
                Ok(yinz_types::input::actions::Action::ToggleHostFullscreen)
            },
            ActionType::RenameSession(rename_session_action) => {
                Ok(yinz_types::input::actions::Action::RenameSession {
                    name: rename_session_action.name.into(),
                })
            },
            ActionType::Notify(notify_action) => Ok(yinz_types::input::actions::Action::Notify(
                notify_action.text,
            )),
            ActionType::CliPipe(cli_pipe_action) => {
                Ok(yinz_types::input::actions::Action::CliPipe {
                    focus_plugin: cli_pipe_action.focus_plugin,
                    pipe_id: cli_pipe_action.pipe_id,
                    name: cli_pipe_action.name,
                    payload: cli_pipe_action.payload,
                    args: if cli_pipe_action.args.is_empty() {
                        None
                    } else {
                        Some(cli_pipe_action.args.into_iter().collect())
                    },
                    plugin: cli_pipe_action.plugin,
                    configuration: if cli_pipe_action.configuration.is_empty() {
                        None
                    } else {
                        Some(cli_pipe_action.configuration.into_iter().collect())
                    },
                    launch_new: cli_pipe_action.launch_new,
                    skip_cache: cli_pipe_action.skip_cache,
                    floating: cli_pipe_action.floating,
                    in_place: cli_pipe_action.in_place,
                    cwd: cli_pipe_action.cwd.map(PathBuf::from),
                    pane_title: cli_pipe_action.pane_title,
                })
            },
            ActionType::KeybindPipe(keybind_pipe_action) => {
                Ok(yinz_types::input::actions::Action::KeybindPipe {
                    name: keybind_pipe_action.name,
                    payload: keybind_pipe_action.payload,
                    args: if keybind_pipe_action.args.is_empty() {
                        None
                    } else {
                        Some(keybind_pipe_action.args.into_iter().collect())
                    },
                    plugin: keybind_pipe_action.plugin,
                    plugin_id: keybind_pipe_action
                        .plugin_id
                        .map(yinz_types::internal_ids::PluginId::new),
                    configuration: if keybind_pipe_action.configuration.is_empty() {
                        None
                    } else {
                        Some(keybind_pipe_action.configuration.into_iter().collect())
                    },
                    launch_new: keybind_pipe_action.launch_new,
                    skip_cache: keybind_pipe_action.skip_cache,
                    floating: keybind_pipe_action.floating,
                    in_place: keybind_pipe_action.in_place,
                    cwd: keybind_pipe_action.cwd.map(PathBuf::from),
                    pane_title: keybind_pipe_action.pane_title,
                })
            },
            ActionType::ListClients(_) => Ok(yinz_types::input::actions::Action::ListClients),
            ActionType::ListPanes(list_panes_action) => {
                Ok(yinz_types::input::actions::Action::ListPanes {
                    show_tab: list_panes_action.show_tab,
                    show_command: list_panes_action.show_command,
                    show_state: list_panes_action.show_state,
                    show_geometry: list_panes_action.show_geometry,
                    show_all: list_panes_action.show_all,
                    output_json: list_panes_action.output_json,
                })
            },
            ActionType::ListTabs(list_tabs_action) => {
                Ok(yinz_types::input::actions::Action::ListTabs {
                    show_state: list_tabs_action.show_state,
                    show_dimensions: list_tabs_action.show_dimensions,
                    show_panes: list_tabs_action.show_panes,
                    show_layout: list_tabs_action.show_layout,
                    show_all: list_tabs_action.show_all,
                    output_json: list_tabs_action.output_json,
                })
            },
            ActionType::CurrentTabInfo(current_tab_info_action) => {
                Ok(yinz_types::input::actions::Action::CurrentTabInfo {
                    output_json: current_tab_info_action.output_json,
                })
            },
            ActionType::TogglePanePinned(_) => {
                Ok(yinz_types::input::actions::Action::TogglePanePinned)
            },
            ActionType::StackPanes(stack_panes_action) => {
                Ok(yinz_types::input::actions::Action::StackPanes {
                    pane_ids: stack_panes_action
                        .pane_ids
                        .into_iter()
                        .map(|id| id.try_into())
                        .collect::<Result<Vec<_>>>()?,
                })
            },
            ActionType::ChangeFloatingPaneCoordinates(change_coords_action) => Ok(
                yinz_types::input::actions::Action::ChangeFloatingPaneCoordinates {
                    pane_id: change_coords_action
                        .pane_id
                        .ok_or_else(|| anyhow!("ChangeFloatingPaneCoordinates missing pane_id"))?
                        .try_into()?,
                    coordinates: change_coords_action
                        .coordinates
                        .ok_or_else(|| {
                            anyhow!("ChangeFloatingPaneCoordinates missing coordinates")
                        })?
                        .try_into()?,
                },
            ),
            ActionType::TogglePaneBorderless(toggle_borderless_action) => {
                Ok(yinz_types::input::actions::Action::TogglePaneBorderless {
                    pane_id: toggle_borderless_action
                        .pane_id
                        .ok_or_else(|| anyhow!("TogglePaneBorderless missing pane_id"))?
                        .try_into()?,
                })
            },
            ActionType::SetPaneBorderless(set_borderless_action) => {
                Ok(yinz_types::input::actions::Action::SetPaneBorderless {
                    pane_id: set_borderless_action
                        .pane_id
                        .ok_or_else(|| anyhow!("SetPaneBorderless missing pane_id"))?
                        .try_into()?,
                    borderless: set_borderless_action.borderless,
                })
            },
            ActionType::Reconfigure(action) => {
                Ok(yinz_types::input::actions::Action::Reconfigure {
                    config: action.config,
                    write_to_disk: action.write_to_disk,
                })
            },
            ActionType::SetPaneAgent(action) => {
                Ok(yinz_types::input::actions::Action::SetPaneAgent {
                    pane_id: action
                        .pane_id
                        .ok_or_else(|| anyhow!("SetPaneAgent missing pane_id"))?
                        .try_into()?,
                    agent: action.agent.map(agent_info_from_proto).transpose()?,
                })
            },
            ActionType::AgentEvent(action) => Ok(yinz_types::input::actions::Action::AgentEvent(
                agent_event_from_proto(action)?,
            )),
            ActionType::TogglePaneInGroup(_) => {
                Ok(yinz_types::input::actions::Action::TogglePaneInGroup)
            },
            ActionType::ToggleGroupMarking(_) => {
                Ok(yinz_types::input::actions::Action::ToggleGroupMarking)
            },
            ActionType::SetPaneColor(set_pane_color_action) => {
                Ok(yinz_types::input::actions::Action::SetPaneColor {
                    pane_id: set_pane_color_action
                        .pane_id
                        .ok_or_else(|| anyhow!("SetPaneColor missing pane_id"))?
                        .try_into()?,
                    fg: set_pane_color_action.fg,
                    bg: set_pane_color_action.bg,
                })
            },
            // Pane-targeting CLI-only variants
            ActionType::ScrollUpByPaneId(a) => {
                Ok(yinz_types::input::actions::Action::ScrollUpByPaneId {
                    pane_id: a
                        .pane_id
                        .ok_or_else(|| anyhow!("ScrollUpByPaneId missing pane_id"))?
                        .try_into()?,
                })
            },
            ActionType::ScrollDownByPaneId(a) => {
                Ok(yinz_types::input::actions::Action::ScrollDownByPaneId {
                    pane_id: a
                        .pane_id
                        .ok_or_else(|| anyhow!("ScrollDownByPaneId missing pane_id"))?
                        .try_into()?,
                })
            },
            ActionType::ScrollToTopByPaneId(a) => {
                Ok(yinz_types::input::actions::Action::ScrollToTopByPaneId {
                    pane_id: a
                        .pane_id
                        .ok_or_else(|| anyhow!("ScrollToTopByPaneId missing pane_id"))?
                        .try_into()?,
                })
            },
            ActionType::ScrollToBottomByPaneId(a) => {
                Ok(yinz_types::input::actions::Action::ScrollToBottomByPaneId {
                    pane_id: a
                        .pane_id
                        .ok_or_else(|| anyhow!("ScrollToBottomByPaneId missing pane_id"))?
                        .try_into()?,
                })
            },
            ActionType::PageScrollUpByPaneId(a) => {
                Ok(yinz_types::input::actions::Action::PageScrollUpByPaneId {
                    pane_id: a
                        .pane_id
                        .ok_or_else(|| anyhow!("PageScrollUpByPaneId missing pane_id"))?
                        .try_into()?,
                })
            },
            ActionType::PageScrollDownByPaneId(a) => {
                Ok(yinz_types::input::actions::Action::PageScrollDownByPaneId {
                    pane_id: a
                        .pane_id
                        .ok_or_else(|| anyhow!("PageScrollDownByPaneId missing pane_id"))?
                        .try_into()?,
                })
            },
            ActionType::HalfPageScrollUpByPaneId(a) => Ok(
                yinz_types::input::actions::Action::HalfPageScrollUpByPaneId {
                    pane_id: a
                        .pane_id
                        .ok_or_else(|| anyhow!("HalfPageScrollUpByPaneId missing pane_id"))?
                        .try_into()?,
                },
            ),
            ActionType::HalfPageScrollDownByPaneId(a) => Ok(
                yinz_types::input::actions::Action::HalfPageScrollDownByPaneId {
                    pane_id: a
                        .pane_id
                        .ok_or_else(|| anyhow!("HalfPageScrollDownByPaneId missing pane_id"))?
                        .try_into()?,
                },
            ),
            ActionType::ResizeByPaneId(a) => {
                let resize_action = a
                    .resize_action
                    .ok_or_else(|| anyhow!("ResizeByPaneId missing resize_action"))?;
                let resize = proto_i32_to_resize(resize_action.resize)?;
                let direction = resize_action
                    .direction
                    .map(proto_i32_to_direction)
                    .transpose()?;
                Ok(yinz_types::input::actions::Action::ResizeByPaneId {
                    pane_id: a
                        .pane_id
                        .ok_or_else(|| anyhow!("ResizeByPaneId missing pane_id"))?
                        .try_into()?,
                    resize,
                    direction,
                })
            },
            ActionType::MovePaneByPaneId(a) => {
                let direction = a.direction.map(proto_i32_to_direction).transpose()?;
                Ok(yinz_types::input::actions::Action::MovePaneByPaneId {
                    pane_id: a
                        .pane_id
                        .ok_or_else(|| anyhow!("MovePaneByPaneId missing pane_id"))?
                        .try_into()?,
                    direction,
                })
            },
            ActionType::MovePaneBackwardsByPaneId(a) => Ok(
                yinz_types::input::actions::Action::MovePaneBackwardsByPaneId {
                    pane_id: a
                        .pane_id
                        .ok_or_else(|| anyhow!("MovePaneBackwardsByPaneId missing pane_id"))?
                        .try_into()?,
                },
            ),
            ActionType::ClearScreenByPaneId(a) => {
                Ok(yinz_types::input::actions::Action::ClearScreenByPaneId {
                    pane_id: a
                        .pane_id
                        .ok_or_else(|| anyhow!("ClearScreenByPaneId missing pane_id"))?
                        .try_into()?,
                })
            },
            ActionType::EditScrollbackByPaneId(a) => {
                Ok(yinz_types::input::actions::Action::EditScrollbackByPaneId {
                    pane_id: a
                        .pane_id
                        .ok_or_else(|| anyhow!("EditScrollbackByPaneId missing pane_id"))?
                        .try_into()?,
                    ansi: a.ansi,
                })
            },
            ActionType::ToggleFullscreenByPaneId(a) => Ok(
                yinz_types::input::actions::Action::ToggleFocusFullscreenByPaneId {
                    pane_id: a
                        .pane_id
                        .ok_or_else(|| anyhow!("ToggleFullscreenByPaneId missing pane_id"))?
                        .try_into()?,
                },
            ),
            ActionType::ToggleNoUiFullscreenByPaneId(a) => Ok(
                yinz_types::input::actions::Action::ToggleFocusNoUiFullscreenByPaneId {
                    pane_id: a
                        .pane_id
                        .ok_or_else(|| anyhow!("ToggleNoUiFullscreenByPaneId missing pane_id"))?
                        .try_into()?,
                },
            ),
            ActionType::TogglePaneEmbedOrFloatingByPaneId(a) => Ok(
                yinz_types::input::actions::Action::TogglePaneEmbedOrFloatingByPaneId {
                    pane_id: a
                        .pane_id
                        .ok_or_else(|| {
                            anyhow!("TogglePaneEmbedOrFloatingByPaneId missing pane_id")
                        })?
                        .try_into()?,
                },
            ),
            ActionType::CloseFocusByPaneId(a) => {
                Ok(yinz_types::input::actions::Action::CloseFocusByPaneId {
                    pane_id: a
                        .pane_id
                        .ok_or_else(|| anyhow!("CloseFocusByPaneId missing pane_id"))?
                        .try_into()?,
                })
            },
            ActionType::RenamePaneByPaneId(a) => {
                Ok(yinz_types::input::actions::Action::RenamePaneByPaneId {
                    pane_id: a.pane_id.map(|p| p.try_into()).transpose()?,
                    name: a.name,
                })
            },
            ActionType::UndoRenamePaneByPaneId(a) => {
                Ok(yinz_types::input::actions::Action::UndoRenamePaneByPaneId {
                    pane_id: a
                        .pane_id
                        .ok_or_else(|| anyhow!("UndoRenamePaneByPaneId missing pane_id"))?
                        .try_into()?,
                })
            },
            ActionType::TogglePanePinnedByPaneId(a) => Ok(
                yinz_types::input::actions::Action::TogglePanePinnedByPaneId {
                    pane_id: a
                        .pane_id
                        .ok_or_else(|| anyhow!("TogglePanePinnedByPaneId missing pane_id"))?
                        .try_into()?,
                },
            ),
            ActionType::FocusPaneByPaneId(a) => {
                Ok(yinz_types::input::actions::Action::FocusPaneByPaneId {
                    pane_id: a
                        .pane_id
                        .ok_or_else(|| anyhow!("FocusPaneByPaneId missing pane_id"))?
                        .try_into()?,
                })
            },
            // Tab-targeting CLI-only variants
            ActionType::UndoRenameTabByTabId(a) => {
                Ok(yinz_types::input::actions::Action::UndoRenameTabByTabId {
                    id: TabId::try_from(a.id).context("tab_id is outside usize range")?,
                })
            },
            ActionType::ToggleActiveSyncTabByTabId(a) => Ok(
                yinz_types::input::actions::Action::ToggleActiveSyncTabByTabId {
                    id: TabId::try_from(a.id).context("tab_id is outside usize range")?,
                },
            ),
            ActionType::ToggleFloatingPanesByTabId(a) => Ok(
                yinz_types::input::actions::Action::ToggleFloatingPanesByTabId {
                    id: TabId::try_from(a.id).context("tab_id is outside usize range")?,
                },
            ),
            ActionType::PreviousSwapLayoutByTabId(a) => Ok(
                yinz_types::input::actions::Action::PreviousSwapLayoutByTabId {
                    id: TabId::try_from(a.id).context("tab_id is outside usize range")?,
                },
            ),
            ActionType::NextSwapLayoutByTabId(a) => {
                Ok(yinz_types::input::actions::Action::NextSwapLayoutByTabId {
                    id: TabId::try_from(a.id).context("tab_id is outside usize range")?,
                })
            },
            ActionType::MoveTabByTabId(a) => {
                let direction = proto_i32_to_direction(a.direction)?;
                Ok(yinz_types::input::actions::Action::MoveTabByTabId {
                    id: TabId::try_from(a.id).context("tab_id is outside usize range")?,
                    direction,
                })
            },
        }
    }
}

impl From<yinz_types::data::KeyWithModifier>
    for crate::client_server_contract::client_server_contract::KeyWithModifier
{
    fn from(key: yinz_types::data::KeyWithModifier) -> Self {
        use crate::ipc::enum_conversions::{bare_key_to_proto_i32, key_modifier_to_proto_i32};

        // Handle character keys specially - store the character for Char variant
        let (bare_key_enum, char_data) = match &key.bare_key {
            yinz_types::data::BareKey::Char(c) => (
                crate::client_server_contract::client_server_contract::BareKey::Char as i32,
                Some(c.to_string()),
            ),
            other => (bare_key_to_proto_i32(*other), None),
        };

        Self {
            bare_key: bare_key_enum,
            key_modifiers: key
                .key_modifiers
                .into_iter()
                .map(key_modifier_to_proto_i32)
                .collect(),
            character: char_data,
        }
    }
}

impl TryFrom<crate::client_server_contract::client_server_contract::KeyWithModifier>
    for yinz_types::data::KeyWithModifier
{
    type Error = anyhow::Error;
    fn try_from(
        key: crate::client_server_contract::client_server_contract::KeyWithModifier,
    ) -> Result<Self> {
        use crate::ipc::enum_conversions::{bare_key_from_proto_i32, key_modifier_from_proto_i32};
        use std::collections::BTreeSet;

        // Handle character keys specially
        let bare_key = if key.bare_key
            == crate::client_server_contract::client_server_contract::BareKey::Char as i32
        {
            let character_str = key
                .character
                .ok_or_else(|| anyhow!("Character key missing character data"))?;
            let character = character_str
                .chars()
                .next()
                .ok_or_else(|| anyhow!("Empty character string"))?;
            yinz_types::data::BareKey::Char(character)
        } else {
            bare_key_from_proto_i32(key.bare_key)?
        };

        let key_modifiers: Result<BTreeSet<_>> = key
            .key_modifiers
            .into_iter()
            .map(key_modifier_from_proto_i32)
            .collect();

        Ok(Self {
            bare_key,
            key_modifiers: key_modifiers?,
        })
    }
}

impl From<yinz_types::data::SessionConnection>
    for crate::client_server_contract::client_server_contract::ConnectToSession
{
    fn from(connect: yinz_types::data::SessionConnection) -> Self {
        Self {
            name: connect.name.map(|name| name.into_string()),
            tab_position: connect.tab_position.map(|p| p.get()),
            pane_id: connect.pane_id.map(|pane| {
                let (id, is_plugin) = pane.legacy_parts();
                crate::client_server_contract::client_server_contract::PaneIdWithPlugin {
                    pane_id: id,
                    is_plugin,
                }
            }),
            layout: connect.layout.map(|l| l.into()),
            cwd: connect.cwd.map(|p| p.to_string_lossy().to_string()),
        }
    }
}

impl TryFrom<crate::client_server_contract::client_server_contract::ConnectToSession>
    for yinz_types::data::SessionConnection
{
    type Error = anyhow::Error;
    fn try_from(
        connect: crate::client_server_contract::client_server_contract::ConnectToSession,
    ) -> Result<Self> {
        Ok(Self {
            name: connect.name.map(Into::into),
            tab_position: connect.tab_position.map(TabNumber::new),
            pane_id: connect
                .pane_id
                .map(|p| PaneId::from_legacy(p.pane_id, p.is_plugin)),
            layout: connect.layout.map(|l| l.try_into()).transpose()?,
            cwd: connect.cwd.map(PathBuf::from),
        })
    }
}

impl From<yinz_types::data::LayoutInfo>
    for crate::client_server_contract::client_server_contract::LayoutInfo
{
    fn from(layout: yinz_types::data::LayoutInfo) -> Self {
        use crate::client_server_contract::client_server_contract::layout_info::LayoutType;
        let (layout_type, layout_metadata) = match layout {
            yinz_types::data::LayoutInfo::BuiltIn(name) => (LayoutType::BuiltinName(name), None),
            yinz_types::data::LayoutInfo::File(path, metadata) => {
                (LayoutType::FilePath(path), Some(metadata.into()))
            },
            yinz_types::data::LayoutInfo::Stringified(content) => {
                (LayoutType::Stringified(content), None)
            },
        };
        Self {
            layout_type: Some(layout_type),
            layout_metadata,
        }
    }
}

impl TryFrom<crate::client_server_contract::client_server_contract::LayoutInfo>
    for yinz_types::data::LayoutInfo
{
    type Error = anyhow::Error;
    fn try_from(
        layout: crate::client_server_contract::client_server_contract::LayoutInfo,
    ) -> Result<Self> {
        use crate::client_server_contract::client_server_contract::layout_info::LayoutType;
        match layout.layout_type {
            Some(LayoutType::BuiltinName(name)) => Ok(yinz_types::data::LayoutInfo::BuiltIn(name)),
            Some(LayoutType::FilePath(path)) => {
                let layout_metadata = layout
                    .layout_metadata
                    .map(|m| m.try_into())
                    .transpose()?
                    .unwrap_or_default();
                Ok(yinz_types::data::LayoutInfo::File(path, layout_metadata))
            },
            Some(LayoutType::Stringified(content)) => {
                Ok(yinz_types::data::LayoutInfo::Stringified(content))
            },
            None => Err(anyhow!("LayoutInfo missing layout_type")),
        }
    }
}

impl From<yinz_types::data::LayoutMetadata> for ProtoLayoutMetadata {
    fn from(metadata: yinz_types::data::LayoutMetadata) -> Self {
        ProtoLayoutMetadata {
            tabs: metadata.tabs.into_iter().map(|t| t.into()).collect(),
            creation_time: metadata.creation_time,
            update_time: metadata.update_time,
        }
    }
}

impl TryFrom<ProtoLayoutMetadata> for yinz_types::data::LayoutMetadata {
    type Error = anyhow::Error;
    fn try_from(proto_metadata: ProtoLayoutMetadata) -> Result<Self> {
        let tabs = proto_metadata
            .tabs
            .into_iter()
            .map(|t| t.try_into())
            .collect::<Result<Vec<_>>>()?;
        Ok(yinz_types::data::LayoutMetadata {
            tabs,
            creation_time: proto_metadata.creation_time,
            update_time: proto_metadata.update_time,
        })
    }
}

impl From<yinz_types::data::TabMetadata> for ProtoTabMetadata {
    fn from(metadata: yinz_types::data::TabMetadata) -> Self {
        ProtoTabMetadata {
            pane_metadata: metadata.panes.into_iter().map(|p| p.into()).collect(),
            name: metadata.name,
        }
    }
}

impl TryFrom<ProtoTabMetadata> for yinz_types::data::TabMetadata {
    type Error = anyhow::Error;
    fn try_from(proto_metadata: ProtoTabMetadata) -> Result<Self> {
        let panes = proto_metadata
            .pane_metadata
            .into_iter()
            .map(|p| p.try_into())
            .collect::<Result<Vec<_>>>()?;
        Ok(yinz_types::data::TabMetadata {
            panes,
            name: proto_metadata.name,
        })
    }
}

impl From<yinz_types::data::PaneMetadata> for ProtoPaneMetadata {
    fn from(metadata: yinz_types::data::PaneMetadata) -> Self {
        ProtoPaneMetadata {
            name: metadata.name,
            is_plugin: metadata.is_plugin,
            is_builtin_plugin: metadata.is_builtin_plugin,
        }
    }
}

impl TryFrom<ProtoPaneMetadata> for yinz_types::data::PaneMetadata {
    type Error = anyhow::Error;
    fn try_from(proto_metadata: ProtoPaneMetadata) -> Result<Self> {
        Ok(yinz_types::data::PaneMetadata {
            name: proto_metadata.name,
            is_plugin: proto_metadata.is_plugin,
            is_builtin_plugin: proto_metadata.is_builtin_plugin,
        })
    }
}

impl From<ExitReason> for ProtoExitReason {
    fn from(reason: ExitReason) -> Self {
        match reason {
            ExitReason::Normal => ProtoExitReason::Normal,
            ExitReason::NormalDetached => ProtoExitReason::NormalDetached,
            ExitReason::ForceDetached => ProtoExitReason::ForceDetached,
            ExitReason::CannotAttach => ProtoExitReason::CannotAttach,
            ExitReason::Disconnect => ProtoExitReason::Disconnect,
            ExitReason::KickedByHost => ProtoExitReason::KickedByHost,
            ExitReason::Error(_msg) => ProtoExitReason::Error,
            ExitReason::CustomExitStatus(_status) => ProtoExitReason::CustomExitStatus,
        }
    }
}

impl TryFrom<ProtoExitReason> for ExitReason {
    type Error = anyhow::Error;
    fn try_from(reason: ProtoExitReason) -> Result<Self> {
        match reason {
            ProtoExitReason::Normal => Ok(ExitReason::Normal),
            ProtoExitReason::NormalDetached => Ok(ExitReason::NormalDetached),
            ProtoExitReason::ForceDetached => Ok(ExitReason::ForceDetached),
            ProtoExitReason::CannotAttach => Ok(ExitReason::CannotAttach),
            ProtoExitReason::Disconnect => Ok(ExitReason::Disconnect),
            ProtoExitReason::KickedByHost => Ok(ExitReason::KickedByHost),
            ProtoExitReason::Error => Ok(ExitReason::Error("Protobuf error".to_string())),
            // The status rides in `ExitMsg.payload`, which a bare reason does not carry. The
            // message decoder handles this arm itself; answering `0` here would turn every
            // failure into a success for whoever reached for the enum conversion instead.
            ProtoExitReason::CustomExitStatus => Err(anyhow!(
                "a custom exit status needs its payload; decode the ExitMsg, not the reason"
            )),
            ProtoExitReason::Unspecified => Err(anyhow!("Unspecified exit reason")),
        }
    }
}

impl From<HostTerminalThemeMode> for ProtoHostTerminalThemeIndication {
    fn from(mode: HostTerminalThemeMode) -> Self {
        match mode {
            HostTerminalThemeMode::Dark => ProtoHostTerminalThemeIndication::Dark,
            HostTerminalThemeMode::Light => ProtoHostTerminalThemeIndication::Light,
        }
    }
}

impl From<ProtoHostTerminalThemeIndication> for HostTerminalThemeMode {
    fn from(mode: ProtoHostTerminalThemeIndication) -> Self {
        match mode {
            ProtoHostTerminalThemeIndication::Dark => HostTerminalThemeMode::Dark,
            ProtoHostTerminalThemeIndication::Light => HostTerminalThemeMode::Light,
        }
    }
}

// InputMode conversion helper functions
fn input_mode_to_proto_i32(mode: InputMode) -> i32 {
    match mode {
        InputMode::Normal => ProtoInputMode::Normal as i32,
        InputMode::Locked => ProtoInputMode::Locked as i32,
        InputMode::Resize => ProtoInputMode::Resize as i32,
        InputMode::Pane => ProtoInputMode::Pane as i32,
        InputMode::Tab => ProtoInputMode::Tab as i32,
        InputMode::Scroll => ProtoInputMode::Scroll as i32,
        InputMode::EnterSearch => ProtoInputMode::EnterSearch as i32,
        InputMode::Search => ProtoInputMode::Search as i32,
        InputMode::RenameTab => ProtoInputMode::RenameTab as i32,
        InputMode::RenamePane => ProtoInputMode::RenamePane as i32,
        InputMode::Session => ProtoInputMode::Session as i32,
        InputMode::Move => ProtoInputMode::Move as i32,
        InputMode::Prompt => ProtoInputMode::Prompt as i32,
        InputMode::Tmux => ProtoInputMode::Tmux as i32,
        InputMode::Agent => ProtoInputMode::Agent as i32,
        InputMode::Select => ProtoInputMode::Select as i32,
        InputMode::Notifications => ProtoInputMode::Notifications as i32,
    }
}

fn proto_i32_to_input_mode(i: i32) -> Result<InputMode> {
    match ProtoInputMode::try_from(i) {
        Ok(ProtoInputMode::Normal) => Ok(InputMode::Normal),
        Ok(ProtoInputMode::Locked) => Ok(InputMode::Locked),
        Ok(ProtoInputMode::Resize) => Ok(InputMode::Resize),
        Ok(ProtoInputMode::Pane) => Ok(InputMode::Pane),
        Ok(ProtoInputMode::Tab) => Ok(InputMode::Tab),
        Ok(ProtoInputMode::Scroll) => Ok(InputMode::Scroll),
        Ok(ProtoInputMode::EnterSearch) => Ok(InputMode::EnterSearch),
        Ok(ProtoInputMode::Search) => Ok(InputMode::Search),
        Ok(ProtoInputMode::RenameTab) => Ok(InputMode::RenameTab),
        Ok(ProtoInputMode::RenamePane) => Ok(InputMode::RenamePane),
        Ok(ProtoInputMode::Session) => Ok(InputMode::Session),
        Ok(ProtoInputMode::Move) => Ok(InputMode::Move),
        Ok(ProtoInputMode::Prompt) => Ok(InputMode::Prompt),
        Ok(ProtoInputMode::Tmux) => Ok(InputMode::Tmux),
        Ok(ProtoInputMode::Agent) => Ok(InputMode::Agent),
        Ok(ProtoInputMode::Select) => Ok(InputMode::Select),
        Ok(ProtoInputMode::Notifications) => Ok(InputMode::Notifications),
        _ => Err(anyhow!("Invalid InputMode value: {}", i)),
    }
}

// Additional helper functions for Action conversion
fn resize_to_proto_i32(resize: yinz_types::geometry::Resize) -> i32 {
    use crate::client_server_contract::client_server_contract::ResizeType;
    match resize {
        yinz_types::geometry::Resize::Increase => ResizeType::Increase as i32,
        yinz_types::geometry::Resize::Decrease => ResizeType::Decrease as i32,
    }
}

fn overlay_kind_to_proto_i32(kind: yinz_types::data::OverlayKind) -> i32 {
    use crate::client_server_contract::client_server_contract::OverlayKind as ProtoOverlayKind;
    match kind {
        yinz_types::data::OverlayKind::Dismissible => ProtoOverlayKind::Dismissible as i32,
        yinz_types::data::OverlayKind::Modal => ProtoOverlayKind::Modal as i32,
    }
}

fn overlay_kind_from_proto_i32(kind: i32) -> yinz_types::data::OverlayKind {
    use crate::client_server_contract::client_server_contract::OverlayKind as ProtoOverlayKind;
    match ProtoOverlayKind::try_from(kind) {
        Ok(ProtoOverlayKind::Modal) => yinz_types::data::OverlayKind::Modal,
        _ => yinz_types::data::OverlayKind::Dismissible,
    }
}

fn agent_state_to_proto(
    state: yinz_types::agent::AgentState,
) -> crate::client_server_contract::client_server_contract::AgentState {
    use crate::client_server_contract::client_server_contract::AgentState as ProtoAgentState;
    use yinz_types::agent::AgentState;
    match state {
        AgentState::Busy => ProtoAgentState::Busy,
        AgentState::Waiting => ProtoAgentState::Waiting,
        AgentState::Idle => ProtoAgentState::Idle,
        AgentState::Exited => ProtoAgentState::Exited,
    }
}

fn agent_state_from_proto(state: i32) -> Result<yinz_types::agent::AgentState> {
    use crate::client_server_contract::client_server_contract::AgentState as ProtoAgentState;
    use yinz_types::agent::AgentState;
    match ProtoAgentState::try_from(state) {
        Ok(ProtoAgentState::Busy) => Ok(AgentState::Busy),
        Ok(ProtoAgentState::Waiting) => Ok(AgentState::Waiting),
        Ok(ProtoAgentState::Idle) => Ok(AgentState::Idle),
        Ok(ProtoAgentState::Exited) => Ok(AgentState::Exited),
        _ => Err(anyhow!("Unknown agent state {state}")),
    }
}

fn agent_event_to_proto(
    event: yinz_types::agent::AgentEvent,
) -> crate::client_server_contract::client_server_contract::AgentEventAction {
    crate::client_server_contract::client_server_contract::AgentEventAction {
        pane_id: event.pane_id.get(),
        kind: event.kind,
        hook: event.hook,
        session_id: event.session_id,
        state: event.state.map(|state| agent_state_to_proto(state) as i32),
        detail: event.detail,
        prompt: event.prompt,
        last_message: event.last_message,
        tool_name: event.tool_name,
        cwd: event.cwd,
        raw: event.raw,
    }
}

fn agent_event_from_proto(
    action: crate::client_server_contract::client_server_contract::AgentEventAction,
) -> Result<yinz_types::agent::AgentEvent> {
    Ok(yinz_types::agent::AgentEvent {
        pane_id: yinz_types::internal_ids::TerminalId::new(action.pane_id),
        kind: action.kind,
        hook: action.hook,
        session_id: action.session_id,
        state: action.state.map(agent_state_from_proto).transpose()?,
        detail: action.detail,
        prompt: action.prompt,
        last_message: action.last_message,
        tool_name: action.tool_name,
        cwd: action.cwd,
        raw: action.raw,
    })
}

fn agent_info_to_proto(
    agent: yinz_types::agent::AgentInfo,
) -> crate::client_server_contract::client_server_contract::AgentInfo {
    use crate::client_server_contract::client_server_contract::{
        AgentInfo as ProtoAgentInfo, AgentSource as ProtoAgentSource, AgentState as ProtoAgentState,
    };
    use yinz_types::agent::{AgentSource, AgentState};
    let state = match agent.state {
        AgentState::Busy => ProtoAgentState::Busy,
        AgentState::Waiting => ProtoAgentState::Waiting,
        AgentState::Idle => ProtoAgentState::Idle,
        AgentState::Exited => ProtoAgentState::Exited,
    };
    let source = match agent.source {
        AgentSource::Spawned => ProtoAgentSource::Spawned,
        AgentSource::Reported => ProtoAgentSource::Reported,
        AgentSource::Registered => ProtoAgentSource::Registered,
        AgentSource::Detected => ProtoAgentSource::Detected,
    };
    ProtoAgentInfo {
        kind: agent.kind,
        state: state as i32,
        source: source as i32,
        detail: agent.detail,
        waiting_on: agent.waiting_on,
        wait_needs_a_person: agent.wait_needs_a_person,
        cwd: agent.cwd,
        session_id: agent.session_id,
        run_id: agent.run_id,
    }
}

fn agent_info_from_proto(
    proto: crate::client_server_contract::client_server_contract::AgentInfo,
) -> Result<yinz_types::agent::AgentInfo> {
    use crate::client_server_contract::client_server_contract::{
        AgentSource as ProtoAgentSource, AgentState as ProtoAgentState,
    };
    use yinz_types::agent::{AgentSource, AgentState};
    let state = match ProtoAgentState::try_from(proto.state) {
        Ok(ProtoAgentState::Busy) => AgentState::Busy,
        Ok(ProtoAgentState::Waiting) => AgentState::Waiting,
        Ok(ProtoAgentState::Idle) => AgentState::Idle,
        Ok(ProtoAgentState::Exited) => AgentState::Exited,
        _ => return Err(anyhow!("Unspecified agent state")),
    };
    let source = match ProtoAgentSource::try_from(proto.source) {
        Ok(ProtoAgentSource::Spawned) => AgentSource::Spawned,
        Ok(ProtoAgentSource::Reported) => AgentSource::Reported,
        Ok(ProtoAgentSource::Registered) => AgentSource::Registered,
        Ok(ProtoAgentSource::Detected) => AgentSource::Detected,
        _ => return Err(anyhow!("Unspecified agent source")),
    };
    Ok(yinz_types::agent::AgentInfo {
        kind: proto.kind,
        state,
        source,
        detail: proto.detail,
        waiting_on: proto.waiting_on,
        wait_needs_a_person: proto.wait_needs_a_person,
        cwd: proto.cwd,
        session_id: proto.session_id,
        run_id: proto.run_id,
    })
}

fn direction_to_proto_i32(direction: yinz_types::geometry::Direction) -> i32 {
    use crate::client_server_contract::client_server_contract::Direction as ProtoDirection;
    match direction {
        yinz_types::geometry::Direction::Left => ProtoDirection::Left as i32,
        yinz_types::geometry::Direction::Right => ProtoDirection::Right as i32,
        yinz_types::geometry::Direction::Up => ProtoDirection::Up as i32,
        yinz_types::geometry::Direction::Down => ProtoDirection::Down as i32,
    }
}

fn search_direction_to_proto_i32(direction: yinz_types::input::actions::SearchDirection) -> i32 {
    use crate::client_server_contract::client_server_contract::SearchDirection as ProtoSearchDirection;
    match direction {
        yinz_types::input::actions::SearchDirection::Up => ProtoSearchDirection::Up as i32,
        yinz_types::input::actions::SearchDirection::Down => ProtoSearchDirection::Down as i32,
    }
}

fn search_option_to_proto_i32(option: yinz_types::input::actions::SearchOption) -> i32 {
    use crate::client_server_contract::client_server_contract::SearchOption as ProtoSearchOption;
    match option {
        yinz_types::input::actions::SearchOption::CaseSensitivity => {
            ProtoSearchOption::CaseSensitivity as i32
        },
        yinz_types::input::actions::SearchOption::Wrap => ProtoSearchOption::Wrap as i32,
        yinz_types::input::actions::SearchOption::WholeWord => ProtoSearchOption::WholeWord as i32,
    }
}

fn unblock_condition_to_proto_i32(condition: yinz_types::data::UnblockCondition) -> i32 {
    use crate::client_server_contract::client_server_contract::UnblockCondition as ProtoUnblockCondition;
    match condition {
        yinz_types::data::UnblockCondition::OnExitSuccess => {
            ProtoUnblockCondition::OnExitSuccess as i32
        },
        yinz_types::data::UnblockCondition::OnExitFailure => {
            ProtoUnblockCondition::OnExitFailure as i32
        },
        yinz_types::data::UnblockCondition::OnAnyExit => ProtoUnblockCondition::OnAnyExit as i32,
    }
}

// Reverse helper functions for Action conversion

fn proto_i32_to_resize(resize: i32) -> Result<yinz_types::geometry::Resize> {
    use crate::client_server_contract::client_server_contract::ResizeType as ProtoResize;
    let proto_resize = match resize {
        x if x == ProtoResize::Increase as i32 => ProtoResize::Increase,
        x if x == ProtoResize::Decrease as i32 => ProtoResize::Decrease,
        _ => return Err(anyhow!("Invalid ResizeType: {}", resize)),
    };
    match proto_resize {
        ProtoResize::Increase => Ok(yinz_types::geometry::Resize::Increase),
        ProtoResize::Decrease => Ok(yinz_types::geometry::Resize::Decrease),
        ProtoResize::Unspecified => Err(anyhow!("Unspecified ResizeType")),
    }
}

fn proto_i32_to_direction(direction: i32) -> Result<yinz_types::geometry::Direction> {
    use crate::client_server_contract::client_server_contract::Direction as ProtoDirection;
    let proto_direction = match direction {
        x if x == ProtoDirection::Left as i32 => ProtoDirection::Left,
        x if x == ProtoDirection::Right as i32 => ProtoDirection::Right,
        x if x == ProtoDirection::Up as i32 => ProtoDirection::Up,
        x if x == ProtoDirection::Down as i32 => ProtoDirection::Down,
        _ => return Err(anyhow!("Invalid Direction: {}", direction)),
    };
    match proto_direction {
        ProtoDirection::Left => Ok(yinz_types::geometry::Direction::Left),
        ProtoDirection::Right => Ok(yinz_types::geometry::Direction::Right),
        ProtoDirection::Up => Ok(yinz_types::geometry::Direction::Up),
        ProtoDirection::Down => Ok(yinz_types::geometry::Direction::Down),
        ProtoDirection::Unspecified => Err(anyhow!("Unspecified direction")),
    }
}

fn proto_i32_to_search_direction(
    direction: i32,
) -> Result<yinz_types::input::actions::SearchDirection> {
    use crate::client_server_contract::client_server_contract::SearchDirection as ProtoSearchDirection;
    let proto_direction = match direction {
        x if x == ProtoSearchDirection::Up as i32 => ProtoSearchDirection::Up,
        x if x == ProtoSearchDirection::Down as i32 => ProtoSearchDirection::Down,
        _ => return Err(anyhow!("Invalid SearchDirection: {}", direction)),
    };
    match proto_direction {
        ProtoSearchDirection::Up => Ok(yinz_types::input::actions::SearchDirection::Up),
        ProtoSearchDirection::Down => Ok(yinz_types::input::actions::SearchDirection::Down),
        ProtoSearchDirection::Unspecified => Err(anyhow!("Unspecified search direction")),
    }
}

fn proto_i32_to_search_option(option: i32) -> Result<yinz_types::input::actions::SearchOption> {
    use crate::client_server_contract::client_server_contract::SearchOption as ProtoSearchOption;
    let proto_option = match option {
        x if x == ProtoSearchOption::CaseSensitivity as i32 => ProtoSearchOption::CaseSensitivity,
        x if x == ProtoSearchOption::WholeWord as i32 => ProtoSearchOption::WholeWord,
        x if x == ProtoSearchOption::Wrap as i32 => ProtoSearchOption::Wrap,
        _ => return Err(anyhow!("Invalid SearchOption: {}", option)),
    };
    match proto_option {
        ProtoSearchOption::CaseSensitivity => {
            Ok(yinz_types::input::actions::SearchOption::CaseSensitivity)
        },
        ProtoSearchOption::Wrap => Ok(yinz_types::input::actions::SearchOption::Wrap),
        ProtoSearchOption::WholeWord => Ok(yinz_types::input::actions::SearchOption::WholeWord),
        ProtoSearchOption::Unspecified => Err(anyhow!("Unspecified search option")),
    }
}

fn proto_i32_to_unblock_condition(condition: i32) -> Result<yinz_types::data::UnblockCondition> {
    use crate::client_server_contract::client_server_contract::UnblockCondition as ProtoUnblockCondition;
    let proto_condition = match condition {
        x if x == ProtoUnblockCondition::OnExitSuccess as i32 => {
            ProtoUnblockCondition::OnExitSuccess
        },
        x if x == ProtoUnblockCondition::OnExitFailure as i32 => {
            ProtoUnblockCondition::OnExitFailure
        },
        x if x == ProtoUnblockCondition::OnAnyExit as i32 => ProtoUnblockCondition::OnAnyExit,
        _ => return Err(anyhow!("Invalid UnblockCondition: {}", condition)),
    };
    match proto_condition {
        ProtoUnblockCondition::OnExitSuccess => {
            Ok(yinz_types::data::UnblockCondition::OnExitSuccess)
        },
        ProtoUnblockCondition::OnExitFailure => {
            Ok(yinz_types::data::UnblockCondition::OnExitFailure)
        },
        ProtoUnblockCondition::OnAnyExit => Ok(yinz_types::data::UnblockCondition::OnAnyExit),
        ProtoUnblockCondition::Unspecified => Err(anyhow!("Unspecified unblock condition")),
    }
}

// Position conversion
impl TryFrom<yinz_types::position::Position>
    for crate::client_server_contract::client_server_contract::Position
{
    type Error = anyhow::Error;
    fn try_from(pos: yinz_types::position::Position) -> Result<Self> {
        Ok(Self {
            line: i32::try_from(pos.line).context("position line exceeds i32")?,
            column: u64::try_from(pos.column.get()).context("position column exceeds u64")?,
        })
    }
}

// Reverse Position conversion
impl TryFrom<crate::client_server_contract::client_server_contract::Position>
    for yinz_types::position::Position
{
    type Error = anyhow::Error;
    fn try_from(
        pos: crate::client_server_contract::client_server_contract::Position,
    ) -> Result<Self> {
        Ok(Self {
            line: yinz_types::position::Line::new(
                isize::try_from(pos.line).context("position line out of range")?,
            ),
            column: yinz_types::position::Column::try_from(pos.column)
                .context("position column out of range")?,
        })
    }
}

// OpenFilePayload conversion
impl From<yinz_types::input::command::OpenFilePayload>
    for crate::client_server_contract::client_server_contract::OpenFilePayload
{
    fn from(payload: yinz_types::input::command::OpenFilePayload) -> Self {
        Self {
            file_to_open: payload.path.to_string_lossy().to_string(),
            line_number: payload.line_number.map(|n| n as u32),
            cwd: payload.cwd.map(|p| p.to_string_lossy().to_string()),
            originating_plugin: payload.originating_plugin.map(|op| op.into()),
        }
    }
}

// Reverse OpenFilePayload conversion
impl TryFrom<crate::client_server_contract::client_server_contract::OpenFilePayload>
    for yinz_types::input::command::OpenFilePayload
{
    type Error = anyhow::Error;
    fn try_from(
        payload: crate::client_server_contract::client_server_contract::OpenFilePayload,
    ) -> Result<Self> {
        Ok(Self {
            path: PathBuf::from(payload.file_to_open),
            line_number: payload.line_number.map(|n| n as usize),
            cwd: payload.cwd.map(PathBuf::from),
            originating_plugin: payload
                .originating_plugin
                .map(|op| op.try_into())
                .transpose()?,
        })
    }
}

// PaneId conversion
impl From<yinz_types::data::PaneId>
    for crate::client_server_contract::client_server_contract::PaneId
{
    fn from(pane_id: yinz_types::data::PaneId) -> Self {
        use crate::client_server_contract::client_server_contract::pane_id::PaneType;
        match pane_id {
            yinz_types::data::PaneId::Terminal(id) => Self {
                pane_type: Some(PaneType::Terminal(id.get())),
            },
            yinz_types::data::PaneId::Plugin(id) => Self {
                pane_type: Some(PaneType::Plugin(id.get())),
            },
        }
    }
}

// Reverse PaneId conversion
impl TryFrom<crate::client_server_contract::client_server_contract::PaneId>
    for yinz_types::data::PaneId
{
    type Error = anyhow::Error;
    fn try_from(
        pane_id: crate::client_server_contract::client_server_contract::PaneId,
    ) -> Result<Self> {
        use crate::client_server_contract::client_server_contract::pane_id::PaneType;
        match pane_id
            .pane_type
            .ok_or_else(|| anyhow!("PaneId missing pane_type"))?
        {
            PaneType::Terminal(id) => Ok(yinz_types::data::PaneId::Terminal(
                yinz_types::internal_ids::TerminalId::new(id),
            )),
            PaneType::Plugin(id) => Ok(yinz_types::data::PaneId::Plugin(
                yinz_types::internal_ids::PluginId::new(id),
            )),
        }
    }
}

// FloatingCoordinate conversion - SplitSize to FloatingCoordinate
impl From<yinz_types::geometry::SplitSize>
    for crate::client_server_contract::client_server_contract::FloatingCoordinate
{
    fn from(size: yinz_types::geometry::SplitSize) -> Self {
        match size {
            yinz_types::geometry::SplitSize::Percent(p) => Self {
                coordinate_type: Some(crate::client_server_contract::client_server_contract::floating_coordinate::CoordinateType::Percent(p as f32)),
            },
            yinz_types::geometry::SplitSize::Fixed(f) => Self {
                coordinate_type: Some(crate::client_server_contract::client_server_contract::floating_coordinate::CoordinateType::Fixed(f as u32)),
            },
        }
    }
}

// Reverse FloatingCoordinate conversion
impl TryFrom<crate::client_server_contract::client_server_contract::FloatingCoordinate>
    for yinz_types::geometry::SplitSize
{
    type Error = anyhow::Error;
    fn try_from(
        coord: crate::client_server_contract::client_server_contract::FloatingCoordinate,
    ) -> Result<Self> {
        use crate::client_server_contract::client_server_contract::floating_coordinate::CoordinateType;
        match coord
            .coordinate_type
            .ok_or_else(|| anyhow!("FloatingCoordinate missing coordinate_type"))?
        {
            CoordinateType::Percent(p) => Ok(yinz_types::geometry::SplitSize::Percent(p as usize)),
            CoordinateType::Fixed(f) => Ok(yinz_types::geometry::SplitSize::Fixed(f as usize)),
        }
    }
}

// FloatingCoordinate conversion - PercentOrFixed to FloatingCoordinate
impl From<yinz_types::geometry::PercentOrFixed>
    for crate::client_server_contract::client_server_contract::FloatingCoordinate
{
    fn from(size: yinz_types::geometry::PercentOrFixed) -> Self {
        match size {
            yinz_types::geometry::PercentOrFixed::Percent(p) => Self {
                coordinate_type: Some(crate::client_server_contract::client_server_contract::floating_coordinate::CoordinateType::Percent(p as f32)),
            },
            yinz_types::geometry::PercentOrFixed::Fixed(f) => Self {
                coordinate_type: Some(crate::client_server_contract::client_server_contract::floating_coordinate::CoordinateType::Fixed(f as u32)),
            },
        }
    }
}

// Reverse FloatingCoordinate conversion for PercentOrFixed
impl TryFrom<crate::client_server_contract::client_server_contract::FloatingCoordinate>
    for yinz_types::geometry::PercentOrFixed
{
    type Error = anyhow::Error;
    fn try_from(
        coord: crate::client_server_contract::client_server_contract::FloatingCoordinate,
    ) -> Result<Self> {
        use crate::client_server_contract::client_server_contract::floating_coordinate::CoordinateType;
        match coord
            .coordinate_type
            .ok_or_else(|| anyhow!("FloatingCoordinate missing coordinate_type"))?
        {
            CoordinateType::Percent(p) => {
                Ok(yinz_types::geometry::PercentOrFixed::Percent(p as usize))
            },
            CoordinateType::Fixed(f) => Ok(yinz_types::geometry::PercentOrFixed::Fixed(f as usize)),
        }
    }
}

// FloatingPaneCoordinates conversion
impl From<yinz_types::geometry::FloatingPaneCoordinates>
    for crate::client_server_contract::client_server_contract::FloatingPaneCoordinates
{
    fn from(coords: yinz_types::geometry::FloatingPaneCoordinates) -> Self {
        Self {
            x: coords.x.map(|x| x.into()),
            y: coords.y.map(|y| y.into()),
            width: coords.width.map(|w| w.into()),
            height: coords.height.map(|h| h.into()),
            pinned: coords.pinned,
            borderless: coords.borderless,
        }
    }
}

// Reverse FloatingPaneCoordinates conversion
impl TryFrom<crate::client_server_contract::client_server_contract::FloatingPaneCoordinates>
    for yinz_types::geometry::FloatingPaneCoordinates
{
    type Error = anyhow::Error;
    fn try_from(
        coords: crate::client_server_contract::client_server_contract::FloatingPaneCoordinates,
    ) -> Result<Self> {
        Ok(Self {
            x: coords.x.map(|x| x.try_into()).transpose()?,
            y: coords.y.map(|y| y.try_into()).transpose()?,
            width: coords.width.map(|w| w.try_into()).transpose()?,
            height: coords.height.map(|h| h.try_into()).transpose()?,
            pinned: coords.pinned,
            borderless: coords.borderless,
        })
    }
}

// NewPanePlacement conversion
impl From<yinz_types::data::NewPanePlacement>
    for crate::client_server_contract::client_server_contract::NewPanePlacement
{
    fn from(placement: yinz_types::data::NewPanePlacement) -> Self {
        use crate::client_server_contract::client_server_contract::new_pane_placement::PlacementType;
        use crate::client_server_contract::client_server_contract::{
            NoPreferencePlacement, StackedPlacement, TiledPlacement,
        };
        let placement_type = match placement {
            yinz_types::data::NewPanePlacement::NoPreference {
                borderless: Some(b),
            } => PlacementType::NoPreferenceWithOptions(NoPreferencePlacement {
                borderless: Some(b),
            }),
            yinz_types::data::NewPanePlacement::NoPreference { borderless: None } => {
                PlacementType::NoPreference(true)
            },
            yinz_types::data::NewPanePlacement::Tiled {
                direction,
                borderless: Some(b),
            } => PlacementType::TiledWithOptions(TiledPlacement {
                direction: direction.map(direction_to_proto_i32),
                borderless: Some(b),
            }),
            yinz_types::data::NewPanePlacement::Tiled {
                direction,
                borderless: None,
            } => PlacementType::Tiled(direction.map(direction_to_proto_i32).unwrap_or(0)),
            yinz_types::data::NewPanePlacement::Floating(coords) => {
                PlacementType::Floating(coords.map(|c| c.into()).unwrap_or_default())
            },
            yinz_types::data::NewPanePlacement::InPlace {
                pane_id_to_replace,
                close_replaced_pane,
                borderless,
            } => PlacementType::InPlace(
                crate::client_server_contract::client_server_contract::NewPanePlacementInPlace {
                    pane_id_to_replace: pane_id_to_replace.map(|id| id.into()),
                    close_replaced_pane,
                    borderless,
                },
            ),
            yinz_types::data::NewPanePlacement::Stacked {
                pane_id_to_stack_under,
                borderless: Some(b),
            } => PlacementType::StackedWithOptions(StackedPlacement {
                pane_id_to_stack_under: pane_id_to_stack_under.map(|id| id.into()),
                borderless: Some(b),
            }),
            yinz_types::data::NewPanePlacement::Stacked {
                pane_id_to_stack_under,
                borderless: None,
            } => PlacementType::Stacked(
                pane_id_to_stack_under
                    .map(|id| id.into())
                    .unwrap_or_default(),
            ),
            yinz_types::data::NewPanePlacement::Overlay(overlay) => PlacementType::Overlay(
                crate::client_server_contract::client_server_contract::OverlayPlacement {
                    background_opacity: overlay.background_opacity.map(u32::from),
                    kind: overlay_kind_to_proto_i32(overlay.kind),
                    coordinates: overlay.coordinates.map(|c| c.into()),
                },
            ),
        };
        Self {
            placement_type: Some(placement_type),
        }
    }
}

// Reverse NewPanePlacement conversion
impl TryFrom<crate::client_server_contract::client_server_contract::NewPanePlacement>
    for yinz_types::data::NewPanePlacement
{
    type Error = anyhow::Error;
    fn try_from(
        placement: crate::client_server_contract::client_server_contract::NewPanePlacement,
    ) -> Result<Self> {
        use crate::client_server_contract::client_server_contract::new_pane_placement::PlacementType;
        match placement
            .placement_type
            .ok_or_else(|| anyhow!("NewPanePlacement missing placement_type"))?
        {
            // New fields (with borderless support) take priority
            PlacementType::NoPreferenceWithOptions(opts) => {
                Ok(yinz_types::data::NewPanePlacement::NoPreference {
                    borderless: opts.borderless,
                })
            },
            PlacementType::TiledWithOptions(opts) => {
                let direction = opts.direction.map(proto_i32_to_direction).transpose()?;
                Ok(yinz_types::data::NewPanePlacement::Tiled {
                    direction,
                    borderless: opts.borderless,
                })
            },
            PlacementType::StackedWithOptions(opts) => {
                let pane_id = opts
                    .pane_id_to_stack_under
                    .map(|id| id.try_into())
                    .transpose()?;
                Ok(yinz_types::data::NewPanePlacement::Stacked {
                    pane_id_to_stack_under: pane_id,
                    borderless: opts.borderless,
                })
            },
            // Legacy fields (without borderless support)
            PlacementType::NoPreference(_) => {
                Ok(yinz_types::data::NewPanePlacement::NoPreference { borderless: None })
            },
            PlacementType::Tiled(direction) => {
                let direction = if direction == 0 {
                    None
                } else {
                    Some(proto_i32_to_direction(direction)?)
                };
                Ok(yinz_types::data::NewPanePlacement::Tiled {
                    direction,
                    borderless: None,
                })
            },
            PlacementType::Floating(coords) => {
                let coords = if coords == Default::default() {
                    None
                } else {
                    Some(coords.try_into()?)
                };
                Ok(yinz_types::data::NewPanePlacement::Floating(coords))
            },
            PlacementType::InPlace(in_place) => Ok(yinz_types::data::NewPanePlacement::InPlace {
                pane_id_to_replace: in_place
                    .pane_id_to_replace
                    .map(|id| id.try_into())
                    .transpose()?,
                close_replaced_pane: in_place.close_replaced_pane,
                borderless: in_place.borderless,
            }),
            PlacementType::Stacked(pane_id) => {
                let pane_id = if pane_id == Default::default() {
                    None
                } else {
                    Some(pane_id.try_into()?)
                };
                Ok(yinz_types::data::NewPanePlacement::Stacked {
                    pane_id_to_stack_under: pane_id,
                    borderless: None,
                })
            },
            PlacementType::Overlay(overlay) => {
                let background_opacity = overlay
                    .background_opacity
                    .map(u8::try_from)
                    .transpose()
                    .context("invalid background opacity")?;
                yinz_types::data::validate_background_opacity(background_opacity)
                    .map_err(|error| anyhow!(error))?;
                let coordinates = match overlay.coordinates {
                    Some(coordinates) => Some(coordinates.try_into()?),
                    None => None,
                };
                Ok(yinz_types::data::NewPanePlacement::Overlay(
                    yinz_types::data::OverlayPlacement {
                        background_opacity,
                        kind: overlay_kind_from_proto_i32(overlay.kind),
                        coordinates,
                    },
                ))
            },
        }
    }
}

// MouseEvent conversion
impl TryFrom<yinz_types::mouse::MouseEvent>
    for crate::client_server_contract::client_server_contract::MouseEvent
{
    type Error = anyhow::Error;
    fn try_from(event: yinz_types::mouse::MouseEvent) -> Result<Self> {
        use crate::client_server_contract::client_server_contract::{
            MouseEventType as ProtoMouseEventType, Position,
        };

        let position = Position::try_from(event.position)?;

        let event_type = match event.event_type {
            yinz_types::mouse::MouseEventType::Press => ProtoMouseEventType::Press as i32,
            yinz_types::mouse::MouseEventType::Release => ProtoMouseEventType::Release as i32,
            yinz_types::mouse::MouseEventType::Motion => ProtoMouseEventType::Motion as i32,
        };

        Ok(Self {
            event_type,
            left: event.left,
            right: event.right,
            middle: event.middle,
            wheel_up: event.wheel_up,
            wheel_down: event.wheel_down,
            wheel_left: event.wheel_left,
            wheel_right: event.wheel_right,
            shift: event.shift,
            alt: event.alt,
            ctrl: event.ctrl,
            position: Some(position),
        })
    }
}

// RunCommandAction conversion
impl From<yinz_types::input::command::RunCommandAction>
    for crate::client_server_contract::client_server_contract::RunCommandAction
{
    fn from(action: yinz_types::input::command::RunCommandAction) -> Self {
        Self {
            command: action.command.to_string_lossy().to_string(),
            args: action.args,
            cwd: action.cwd.map(|p| p.to_string_lossy().to_string()),
            direction: action.direction.map(direction_to_proto_i32),
            hold_on_close: action.hold_on_close,
            hold_on_start: action.hold_on_start,
            originating_plugin: action.originating_plugin.map(|op| op.into()),
            use_terminal_title: action.use_terminal_title,
            remote_default_shell: action.remote_default_shell,
            target: Some(pane_target_to_protobuf(&action.target)),
        }
    }
}

// OriginatingPlugin conversion
impl From<yinz_types::data::OriginatingPlugin>
    for crate::client_server_contract::client_server_contract::OriginatingPlugin
{
    fn from(orig: yinz_types::data::OriginatingPlugin) -> Self {
        use std::collections::HashMap;
        let context: HashMap<String, String> = orig.context.into_iter().collect();

        Self {
            deno_generation: orig.deno_generation,
            plugin_id: orig.plugin_id.get(),
            client_id: u32::from(orig.client_id.get()),
            context,
        }
    }
}

// OriginatingPlugin reverse conversion
impl TryFrom<crate::client_server_contract::client_server_contract::OriginatingPlugin>
    for yinz_types::data::OriginatingPlugin
{
    type Error = anyhow::Error;

    fn try_from(
        orig: crate::client_server_contract::client_server_contract::OriginatingPlugin,
    ) -> Result<Self> {
        use std::collections::BTreeMap;
        let context: BTreeMap<String, String> = orig.context.into_iter().collect();

        Ok(Self {
            deno_generation: orig.deno_generation,
            plugin_id: yinz_types::internal_ids::PluginId::new(orig.plugin_id),
            client_id: ClientId::try_from(orig.client_id)?,
            context,
        })
    }
}

// SplitDirection conversion helper
fn split_direction_to_proto_i32(direction: yinz_types::geometry::SplitDirection) -> i32 {
    use crate::client_server_contract::client_server_contract::SplitDirection as ProtoSplitDirection;
    match direction {
        yinz_types::geometry::SplitDirection::Horizontal => ProtoSplitDirection::Horizontal as i32,
        yinz_types::geometry::SplitDirection::Vertical => ProtoSplitDirection::Vertical as i32,
    }
}

// SplitSize conversion
impl From<yinz_types::geometry::SplitSize>
    for crate::client_server_contract::client_server_contract::SplitSize
{
    fn from(size: yinz_types::geometry::SplitSize) -> Self {
        use crate::client_server_contract::client_server_contract::split_size::SizeType;
        match size {
            yinz_types::geometry::SplitSize::Percent(p) => Self {
                size_type: Some(SizeType::Percent(p as u32)),
            },
            yinz_types::geometry::SplitSize::Fixed(f) => Self {
                size_type: Some(SizeType::Fixed(f as u32)),
            },
        }
    }
}

// PercentOrFixed conversion
impl From<yinz_types::geometry::PercentOrFixed>
    for crate::client_server_contract::client_server_contract::PercentOrFixed
{
    fn from(size: yinz_types::geometry::PercentOrFixed) -> Self {
        use crate::client_server_contract::client_server_contract::percent_or_fixed::SizeType;
        match size {
            yinz_types::geometry::PercentOrFixed::Percent(p) => Self {
                size_type: Some(SizeType::Percent(p as u32)),
            },
            yinz_types::geometry::PercentOrFixed::Fixed(f) => Self {
                size_type: Some(SizeType::Fixed(f as u32)),
            },
        }
    }
}

// Run conversion
impl From<yinz_types::input::layout::Run>
    for crate::client_server_contract::client_server_contract::Run
{
    fn from(run: yinz_types::input::layout::Run) -> Self {
        use crate::client_server_contract::client_server_contract::run::RunType;
        match run {
            yinz_types::input::layout::Run::Command(cmd) => Self {
                run_type: Some(RunType::Command(
                    crate::client_server_contract::client_server_contract::RunCommandAction {
                        command: cmd.command.to_string_lossy().to_string(),
                        args: cmd.args,
                        cwd: cmd.cwd.map(|p| p.to_string_lossy().to_string()),
                        direction: None, // RunCommand doesn't have direction field
                        hold_on_close: cmd.hold_on_close,
                        hold_on_start: cmd.hold_on_start,
                        originating_plugin: cmd.originating_plugin.map(|op| op.into()),
                        use_terminal_title: cmd.use_terminal_title,
                        remote_default_shell: cmd.remote_default_shell,
                        target: Some(pane_target_to_protobuf(&cmd.target)),
                    },
                )),
            },
            yinz_types::input::layout::Run::Plugin(plugin) => Self {
                run_type: Some(RunType::Plugin(plugin.into())),
            },
            yinz_types::input::layout::Run::EditFile(path, line_number, cwd) => Self {
                run_type: Some(RunType::EditFile(
                    crate::client_server_contract::client_server_contract::RunEditFileAction {
                        file_path: path.to_string_lossy().to_string(),
                        line_number: line_number.map(|n| n as u32),
                        cwd: cwd.map(|p| p.to_string_lossy().to_string()),
                    },
                )),
            },
            yinz_types::input::layout::Run::Cwd(path) => Self {
                run_type: Some(RunType::Cwd(path.to_string_lossy().to_string())),
            },
        }
    }
}

// TabLayoutInfo conversion
impl TryFrom<yinz_types::input::layout::TabLayoutInfo>
    for crate::client_server_contract::client_server_contract::TabLayoutInfo
{
    type Error = anyhow::Error;
    fn try_from(tab_info: yinz_types::input::layout::TabLayoutInfo) -> Result<Self> {
        Ok(Self {
            tab_index: u32::try_from(tab_info.tab_index.get())
                .context("tab position is outside u32 range")?,
            tab_name: tab_info.tab_name,
            tiled_layout: Some(tab_info.tiled_layout.into()),
            floating_layouts: tab_info
                .floating_layouts
                .into_iter()
                .map(|l| l.into())
                .collect(),
            swap_tiled_layouts: tab_info
                .swap_tiled_layouts
                .unwrap_or_default()
                .into_iter()
                .map(|l| l.into())
                .collect(),
            swap_floating_layouts: tab_info
                .swap_floating_layouts
                .unwrap_or_default()
                .into_iter()
                .map(|l| l.into())
                .collect(),
        })
    }
}

impl TryFrom<crate::client_server_contract::client_server_contract::TabLayoutInfo>
    for yinz_types::input::layout::TabLayoutInfo
{
    type Error = anyhow::Error;

    fn try_from(
        protobuf_tab: crate::client_server_contract::client_server_contract::TabLayoutInfo,
    ) -> Result<Self> {
        Ok(yinz_types::input::layout::TabLayoutInfo {
            tab_index: TabPosition::try_from(u64::from(protobuf_tab.tab_index))
                .context("tab position is outside usize range")?,
            tab_name: protobuf_tab.tab_name.filter(|s| !s.is_empty()),
            tiled_layout: protobuf_tab
                .tiled_layout
                .ok_or_else(|| anyhow!("missing tiled_layout"))?
                .try_into()?,
            floating_layouts: protobuf_tab
                .floating_layouts
                .into_iter()
                .map(|l| l.try_into())
                .collect::<Result<Vec<_>>>()?,
            swap_tiled_layouts: if protobuf_tab.swap_tiled_layouts.is_empty() {
                None
            } else {
                Some(
                    protobuf_tab
                        .swap_tiled_layouts
                        .into_iter()
                        .map(|l| l.try_into())
                        .collect::<Result<Vec<_>>>()?,
                )
            },
            swap_floating_layouts: if protobuf_tab.swap_floating_layouts.is_empty() {
                None
            } else {
                Some(
                    protobuf_tab
                        .swap_floating_layouts
                        .into_iter()
                        .map(|l| l.try_into())
                        .collect::<Result<Vec<_>>>()?,
                )
            },
        })
    }
}

// TiledPaneLayout conversion
impl From<yinz_types::input::layout::TiledPaneLayout>
    for crate::client_server_contract::client_server_contract::TiledPaneLayout
{
    fn from(layout: yinz_types::input::layout::TiledPaneLayout) -> Self {
        Self {
            pane_tabs_json: layout
                .pane_tabs
                .as_ref()
                .and_then(|tabs| serde_json::to_string(tabs).ok()),
            children_split_direction: split_direction_to_proto_i32(layout.children_split_direction),
            name: layout.name,
            children: layout.children.into_iter().map(|c| c.into()).collect(),
            split_size: layout.split_size.map(|s| s.into()),
            run: layout.run.map(|r| r.into()),
            borderless: layout.borderless,
            focus: layout.focus.map(|f| f.to_string()),
            exclude_from_sync: layout.exclude_from_sync,
            children_are_stacked: layout.children_are_stacked,
            external_children_index: layout.external_children_index.map(|l| l as u32),
            is_expanded_in_stack: layout.is_expanded_in_stack,
            hide_floating_panes: layout.hide_floating_panes,
            user_named: layout.user_named,
            pane_initial_contents: layout.pane_initial_contents,
            default_fg: layout.default_fg,
            default_bg: layout.default_bg,
        }
    }
}

impl From<yinz_types::input::layout::FloatingPaneLayout>
    for crate::client_server_contract::client_server_contract::FloatingPaneLayout
{
    fn from(layout: yinz_types::input::layout::FloatingPaneLayout) -> Self {
        Self {
            pane_tabs_json: layout
                .pane_tabs
                .as_ref()
                .and_then(|tabs| serde_json::to_string(tabs).ok()),
            name: layout.name,
            height: layout.height.map(|h| h.into()),
            width: layout.width.map(|w| w.into()),
            x: layout.x.map(|x| x.into()),
            y: layout.y.map(|y| y.into()),
            pinned: layout.pinned,
            run: layout.run.map(|r| r.into()),
            focus: layout.focus,
            already_running: layout.already_running,
            pane_initial_contents: layout.pane_initial_contents,
            logical_position: layout.logical_position.map(|l| l as u32),
            borderless: layout.borderless,
            default_fg: layout.default_fg,
            default_bg: layout.default_bg,
        }
    }
}

impl From<yinz_types::input::layout::SwapTiledLayout>
    for crate::client_server_contract::client_server_contract::SwapTiledLayout
{
    fn from(layout: yinz_types::input::layout::SwapTiledLayout) -> Self {
        use crate::client_server_contract::client_server_contract::LayoutConstraintTiledPair;

        let constraint_map = layout
            .0
            .into_iter()
            .map(|(constraint, tiled_layout)| LayoutConstraintTiledPair {
                constraint: Some(constraint.into()),
                layout: Some(tiled_layout.into()),
            })
            .collect();

        Self {
            constraint_map,
            name: layout.1,
        }
    }
}

impl From<yinz_types::input::layout::SwapFloatingLayout>
    for crate::client_server_contract::client_server_contract::SwapFloatingLayout
{
    fn from(layout: yinz_types::input::layout::SwapFloatingLayout) -> Self {
        use crate::client_server_contract::client_server_contract::LayoutConstraintFloatingPair;

        let constraint_map = layout
            .0
            .into_iter()
            .map(
                |(constraint, floating_layouts)| LayoutConstraintFloatingPair {
                    constraint: Some(constraint.into()),
                    layouts: floating_layouts.into_iter().map(|l| l.into()).collect(),
                },
            )
            .collect();

        Self {
            constraint_map,
            name: layout.1,
        }
    }
}

// PluginUserConfiguration conversion
impl From<yinz_types::input::layout::PluginUserConfiguration>
    for crate::client_server_contract::client_server_contract::PluginUserConfiguration
{
    fn from(config: yinz_types::input::layout::PluginUserConfiguration) -> Self {
        Self {
            configuration: config.inner().clone().into_iter().collect(), // Convert BTreeMap to HashMap
        }
    }
}

// LayoutConstraint conversion
impl From<yinz_types::input::layout::LayoutConstraint>
    for crate::client_server_contract::client_server_contract::LayoutConstraintWithValue
{
    fn from(constraint: yinz_types::input::layout::LayoutConstraint) -> Self {
        use crate::client_server_contract::client_server_contract::LayoutConstraint as ProtoLayoutConstraint;
        match constraint {
            yinz_types::input::layout::LayoutConstraint::MaxPanes(n) => Self {
                constraint_type: ProtoLayoutConstraint::MaxPanes as i32,
                value: Some(n as u32),
            },
            yinz_types::input::layout::LayoutConstraint::MinPanes(n) => Self {
                constraint_type: ProtoLayoutConstraint::MinPanes as i32,
                value: Some(n as u32),
            },
            yinz_types::input::layout::LayoutConstraint::ExactPanes(n) => Self {
                constraint_type: ProtoLayoutConstraint::ExactPanes as i32,
                value: Some(n as u32),
            },
            yinz_types::input::layout::LayoutConstraint::NoConstraint => Self {
                constraint_type: ProtoLayoutConstraint::NoConstraint as i32,
                value: None,
            },
        }
    }
}

// RunPlugin conversion
impl From<yinz_types::input::layout::RunPlugin>
    for crate::client_server_contract::client_server_contract::RunPlugin
{
    fn from(plugin: yinz_types::input::layout::RunPlugin) -> Self {
        Self {
            allow_exec_host_cmd: plugin._allow_exec_host_cmd,
            location: Some(plugin.location.into()),
            configuration: Some(plugin.configuration.into()),
            initial_cwd: plugin.initial_cwd.map(|p| p.display().to_string()),
            relative_path: plugin.relative_path.map(|p| p.display().to_string()),
            declared_path: plugin.declared_path.map(|p| p.display().to_string()),
        }
    }
}

// PluginAlias conversion
impl From<yinz_types::input::layout::PluginAlias>
    for crate::client_server_contract::client_server_contract::PluginAlias
{
    fn from(plugin: yinz_types::input::layout::PluginAlias) -> Self {
        Self {
            name: plugin.name,
            configuration: plugin.configuration.map(|c| c.into()),
            initial_cwd: plugin.initial_cwd.map(|i| i.display().to_string()),
            run_plugin: plugin.run_plugin.map(|r| r.into()),
        }
    }
}

// RunPluginLocation conversion
impl From<yinz_types::input::layout::RunPluginLocation>
    for crate::client_server_contract::client_server_contract::RunPluginLocationData
{
    fn from(location: yinz_types::input::layout::RunPluginLocation) -> Self {
        use crate::client_server_contract::client_server_contract::{
            run_plugin_location_data::LocationData, RunPluginLocation as ProtoRunPluginLocation,
        };
        match location {
            yinz_types::input::layout::RunPluginLocation::Git(plugin) => Self {
                location_type: ProtoRunPluginLocation::Git as i32,
                location_data: Some(LocationData::GitDescriptor(plugin.descriptor())),
            },
            yinz_types::input::layout::RunPluginLocation::File(path) => Self {
                location_type: ProtoRunPluginLocation::File as i32,
                location_data: Some(LocationData::FilePath(path.to_string_lossy().to_string())),
            },
            yinz_types::input::layout::RunPluginLocation::Yinz(tag) => Self {
                location_type: ProtoRunPluginLocation::Yinz as i32,
                location_data: Some(LocationData::YinzTag(
                    crate::client_server_contract::client_server_contract::PluginTag {
                        tag: tag.to_string(),
                    },
                )),
            },
        }
    }
}

// RunPluginOrAlias conversion
impl From<yinz_types::input::layout::RunPluginOrAlias>
    for crate::client_server_contract::client_server_contract::RunPluginOrAlias
{
    fn from(plugin: yinz_types::input::layout::RunPluginOrAlias) -> Self {
        use crate::client_server_contract::client_server_contract::run_plugin_or_alias::PluginType;
        match plugin {
            yinz_types::input::layout::RunPluginOrAlias::RunPlugin(run_plugin) => Self {
                plugin_type: Some(PluginType::Plugin(run_plugin.into())),
            },
            yinz_types::input::layout::RunPluginOrAlias::Alias(alias) => Self {
                plugin_type: Some(PluginType::Alias(alias.into())),
            },
        }
    }
}

// CommandOrPlugin conversion
impl From<yinz_types::data::CommandOrPlugin>
    for crate::client_server_contract::client_server_contract::CommandOrPlugin
{
    fn from(cmd_or_plugin: yinz_types::data::CommandOrPlugin) -> Self {
        use crate::client_server_contract::client_server_contract::command_or_plugin::CommandOrPluginType;
        use crate::client_server_contract::client_server_contract::CommandOrPluginFile;
        match cmd_or_plugin {
            yinz_types::data::CommandOrPlugin::Command(cmd) => Self {
                command_or_plugin_type: Some(CommandOrPluginType::Command(cmd.into())),
            },
            yinz_types::data::CommandOrPlugin::Plugin(plugin) => Self {
                command_or_plugin_type: Some(CommandOrPluginType::Plugin(plugin.into())),
            },
            yinz_types::data::CommandOrPlugin::File(f) => Self {
                command_or_plugin_type: Some(CommandOrPluginType::File(CommandOrPluginFile {
                    path: f.path.display().to_string(),
                    line_number: f.line_number.map(|n| n as i32),
                    cwd: f.cwd.map(|c| c.display().to_string()),
                })),
            },
        }
    }
}

impl TryFrom<crate::client_server_contract::client_server_contract::CommandOrPlugin>
    for yinz_types::data::CommandOrPlugin
{
    type Error = anyhow::Error;

    fn try_from(
        proto: crate::client_server_contract::client_server_contract::CommandOrPlugin,
    ) -> Result<Self> {
        use crate::client_server_contract::client_server_contract::command_or_plugin::CommandOrPluginType;

        let cmd_or_plugin_type = proto
            .command_or_plugin_type
            .ok_or_else(|| anyhow!("CommandOrPlugin missing command_or_plugin_type"))?;
        match cmd_or_plugin_type {
            CommandOrPluginType::Command(cmd) => {
                Ok(yinz_types::data::CommandOrPlugin::Command(cmd.try_into()?))
            },
            CommandOrPluginType::Plugin(plugin) => Ok(yinz_types::data::CommandOrPlugin::Plugin(
                plugin.try_into()?,
            )),
            CommandOrPluginType::File(f) => Ok(yinz_types::data::CommandOrPlugin::File(
                yinz_types::data::FileToOpen {
                    path: std::path::PathBuf::from(&f.path),
                    line_number: f.line_number.map(|n| n as usize),
                    cwd: f.cwd.map(std::path::PathBuf::from),
                },
            )),
        }
    }
}

// Run reverse conversion
impl TryFrom<crate::client_server_contract::client_server_contract::Run>
    for yinz_types::input::layout::Run
{
    type Error = anyhow::Error;

    fn try_from(run: crate::client_server_contract::client_server_contract::Run) -> Result<Self> {
        use crate::client_server_contract::client_server_contract::run::RunType;

        let run_type = run
            .run_type
            .ok_or_else(|| anyhow!("Run missing run_type"))?;
        match run_type {
            RunType::Command(cmd) => Ok(yinz_types::input::layout::Run::Command(
                yinz_types::input::command::RunCommand {
                    agent_theme: None,
                    claude_settings: None,
                    workflow_owner: None,
                    command: std::path::PathBuf::from(cmd.command),
                    args: cmd.args,
                    cwd: cmd.cwd.map(std::path::PathBuf::from),
                    hold_on_close: cmd.hold_on_close,
                    hold_on_start: cmd.hold_on_start,
                    originating_plugin: cmd
                        .originating_plugin
                        .map(|op| op.try_into())
                        .transpose()?,
                    use_terminal_title: cmd.use_terminal_title,
                    remote_default_shell: cmd.remote_default_shell,
                    target: pane_target_from_protobuf(cmd.target)?,
                },
            )),
            RunType::EditFile(edit) => Ok(yinz_types::input::layout::Run::EditFile(
                std::path::PathBuf::from(edit.file_path),
                edit.line_number.map(|n| n as usize),
                edit.cwd.map(std::path::PathBuf::from),
            )),
            RunType::Cwd(cwd_str) => Ok(yinz_types::input::layout::Run::Cwd(
                std::path::PathBuf::from(cwd_str),
            )),
            RunType::Plugin(plugin) => {
                Ok(yinz_types::input::layout::Run::Plugin(plugin.try_into()?))
            },
        }
    }
}

// PercentOrFixed reverse conversion
impl TryFrom<crate::client_server_contract::client_server_contract::PercentOrFixed>
    for yinz_types::geometry::PercentOrFixed
{
    type Error = anyhow::Error;

    fn try_from(
        value: crate::client_server_contract::client_server_contract::PercentOrFixed,
    ) -> Result<Self> {
        use crate::client_server_contract::client_server_contract::percent_or_fixed::SizeType;

        let size_type = value
            .size_type
            .ok_or_else(|| anyhow!("PercentOrFixed missing size_type"))?;
        match size_type {
            SizeType::Percent(percent) => Ok(yinz_types::geometry::PercentOrFixed::Percent(
                percent as usize,
            )),
            SizeType::Fixed(fixed) => {
                Ok(yinz_types::geometry::PercentOrFixed::Fixed(fixed as usize))
            },
        }
    }
}

// ===== REVERSE CONVERSIONS =====

// MouseEvent reverse conversion
impl TryFrom<crate::client_server_contract::client_server_contract::MouseEvent>
    for yinz_types::mouse::MouseEvent
{
    type Error = anyhow::Error;

    fn try_from(
        event: crate::client_server_contract::client_server_contract::MouseEvent,
    ) -> Result<Self> {
        use crate::client_server_contract::client_server_contract::MouseEventType as ProtoMouseEventType;

        let event_type = match event.event_type {
            x if x == ProtoMouseEventType::Press as i32 => yinz_types::mouse::MouseEventType::Press,
            x if x == ProtoMouseEventType::Release as i32 => {
                yinz_types::mouse::MouseEventType::Release
            },
            x if x == ProtoMouseEventType::Motion as i32 => {
                yinz_types::mouse::MouseEventType::Motion
            },
            _ => return Err(anyhow!("Invalid MouseEventType: {}", event.event_type)),
        };

        let position = event
            .position
            .ok_or_else(|| anyhow!("MouseEvent missing position"))?
            .try_into()?;

        Ok(yinz_types::mouse::MouseEvent {
            event_type,
            left: event.left,
            right: event.right,
            middle: event.middle,
            wheel_up: event.wheel_up,
            wheel_down: event.wheel_down,
            wheel_left: event.wheel_left,
            wheel_right: event.wheel_right,
            shift: event.shift,
            alt: event.alt,
            ctrl: event.ctrl,
            position,
        })
    }
}

// RunCommandAction reverse conversion
impl TryFrom<crate::client_server_contract::client_server_contract::RunCommandAction>
    for yinz_types::input::command::RunCommandAction
{
    type Error = anyhow::Error;

    fn try_from(
        action: crate::client_server_contract::client_server_contract::RunCommandAction,
    ) -> Result<Self> {
        Ok(yinz_types::input::command::RunCommandAction {
            command: std::path::PathBuf::from(action.command),
            args: action.args,
            cwd: action.cwd.map(std::path::PathBuf::from),
            direction: action.direction.map(proto_i32_to_direction).transpose()?,
            hold_on_close: action.hold_on_close,
            hold_on_start: action.hold_on_start,
            originating_plugin: action
                .originating_plugin
                .map(|op| op.try_into())
                .transpose()?,
            use_terminal_title: action.use_terminal_title,
            remote_default_shell: action.remote_default_shell,
            target: pane_target_from_protobuf(action.target)?,
        })
    }
}

// TiledPaneLayout reverse conversion
impl TryFrom<crate::client_server_contract::client_server_contract::TiledPaneLayout>
    for yinz_types::input::layout::TiledPaneLayout
{
    type Error = anyhow::Error;

    fn try_from(
        layout: crate::client_server_contract::client_server_contract::TiledPaneLayout,
    ) -> Result<Self> {
        use yinz_types::geometry::{SplitDirection, SplitSize};
        use yinz_types::input::layout::TiledPaneLayout;

        let children_split_direction = match layout.children_split_direction {
            x if x
                == crate::client_server_contract::client_server_contract::SplitDirection::Horizontal
                    as i32 =>
            {
                SplitDirection::Horizontal
            },
            x if x
                == crate::client_server_contract::client_server_contract::SplitDirection::Vertical
                    as i32 =>
            {
                SplitDirection::Vertical
            },
            _ => SplitDirection::Horizontal, // default
        };

        let children: Result<Vec<_>> = layout.children.into_iter().map(|c| c.try_into()).collect();
        let run = layout.run.map(|r| r.try_into()).transpose()?;

        let split_size = layout.split_size.and_then(|size| {
            use crate::client_server_contract::client_server_contract::split_size::SizeType;
            match size.size_type {
                Some(SizeType::Percent(percent)) => Some(SplitSize::Percent(percent as usize)),
                Some(SizeType::Fixed(fixed)) => Some(SplitSize::Fixed(fixed as usize)),
                None => None,
            }
        });

        Ok(TiledPaneLayout {
            pane_tabs: layout
                .pane_tabs_json
                .as_deref()
                .map(|json| {
                    let tabs: yinz_types::pane_tabs::PaneTabsLayout =
                        serde_json::from_str(json).context("invalid pane tabs layout")?;
                    tabs.validate().map_err(|error| anyhow!(error))?;
                    Ok::<_, anyhow::Error>(tabs)
                })
                .transpose()?,
            children_split_direction,
            name: layout.name,
            children: children?,
            split_size,
            run,
            borderless: layout.borderless,
            focus: layout.focus.map(|f| f == "true"), // Convert string to bool
            external_children_index: layout.external_children_index.map(|l| l as usize),
            children_are_stacked: layout.children_are_stacked,
            is_expanded_in_stack: layout.is_expanded_in_stack,
            exclude_from_sync: layout.exclude_from_sync,
            run_instructions_to_ignore: vec![], // not represented in protobuf
            hide_floating_panes: layout.hide_floating_panes,
            user_named: layout.user_named,
            pane_initial_contents: layout.pane_initial_contents,
            default_fg: layout.default_fg,
            default_bg: layout.default_bg,
        })
    }
}

// FloatingPaneLayout reverse conversion
impl TryFrom<crate::client_server_contract::client_server_contract::FloatingPaneLayout>
    for yinz_types::input::layout::FloatingPaneLayout
{
    type Error = anyhow::Error;

    fn try_from(
        layout: crate::client_server_contract::client_server_contract::FloatingPaneLayout,
    ) -> Result<Self> {
        let run = layout.run.map(|r| r.try_into()).transpose()?;
        let height = layout.height.map(|h| h.try_into()).transpose()?;
        let width = layout.width.map(|w| w.try_into()).transpose()?;
        let x = layout.x.map(|x| x.try_into()).transpose()?;
        let y = layout.y.map(|y| y.try_into()).transpose()?;

        Ok(yinz_types::input::layout::FloatingPaneLayout {
            pane_tabs: layout
                .pane_tabs_json
                .as_deref()
                .map(|json| {
                    let tabs: yinz_types::pane_tabs::PaneTabsLayout =
                        serde_json::from_str(json).context("invalid pane tabs layout")?;
                    tabs.validate().map_err(|error| anyhow!(error))?;
                    Ok::<_, anyhow::Error>(tabs)
                })
                .transpose()?,
            name: layout.name,
            height,
            width,
            x,
            y,
            pinned: layout.pinned,
            run,
            focus: layout.focus,
            already_running: layout.already_running,
            pane_initial_contents: layout.pane_initial_contents,
            logical_position: layout.logical_position.map(|p| p as usize),
            borderless: layout.borderless,
            default_fg: layout.default_fg,
            default_bg: layout.default_bg,
        })
    }
}

// SwapTiledLayout reverse conversion
impl TryFrom<crate::client_server_contract::client_server_contract::SwapTiledLayout>
    for yinz_types::input::layout::SwapTiledLayout
{
    type Error = anyhow::Error;

    fn try_from(
        layout: crate::client_server_contract::client_server_contract::SwapTiledLayout,
    ) -> Result<Self> {
        let constraint_map: Result<BTreeMap<_, _>> = layout
            .constraint_map
            .into_iter()
            .map(|pair| {
                Ok((
                    pair.constraint
                        .ok_or_else(|| anyhow!("Missing constraint"))?
                        .try_into()?,
                    pair.layout
                        .ok_or_else(|| anyhow!("Missing layout"))?
                        .try_into()?,
                ))
            })
            .collect();
        Ok((constraint_map?, layout.name))
    }
}

// SwapFloatingLayout reverse conversion
impl TryFrom<crate::client_server_contract::client_server_contract::SwapFloatingLayout>
    for yinz_types::input::layout::SwapFloatingLayout
{
    type Error = anyhow::Error;

    fn try_from(
        layout: crate::client_server_contract::client_server_contract::SwapFloatingLayout,
    ) -> Result<Self> {
        let constraint_map: Result<BTreeMap<_, _>> = layout
            .constraint_map
            .into_iter()
            .map(|pair| {
                let floating_layouts: Result<Vec<_>> =
                    pair.layouts.into_iter().map(|l| l.try_into()).collect();
                Ok((
                    pair.constraint
                        .ok_or_else(|| anyhow!("Missing constraint"))?
                        .try_into()?,
                    floating_layouts?,
                ))
            })
            .collect();

        Ok((constraint_map?, layout.name))
    }
}

// PluginUserConfiguration reverse conversion
impl TryFrom<crate::client_server_contract::client_server_contract::PluginUserConfiguration>
    for yinz_types::input::layout::PluginUserConfiguration
{
    type Error = anyhow::Error;

    fn try_from(
        config: crate::client_server_contract::client_server_contract::PluginUserConfiguration,
    ) -> Result<Self> {
        let btree_map: BTreeMap<String, String> = config.configuration.into_iter().collect();
        Ok(yinz_types::input::layout::PluginUserConfiguration::new(
            btree_map,
        ))
    }
}

// LayoutConstraint reverse conversion
impl TryFrom<crate::client_server_contract::client_server_contract::LayoutConstraintWithValue>
    for yinz_types::input::layout::LayoutConstraint
{
    type Error = anyhow::Error;

    fn try_from(
        constraint: crate::client_server_contract::client_server_contract::LayoutConstraintWithValue,
    ) -> Result<Self> {
        use crate::client_server_contract::client_server_contract::LayoutConstraint as ProtoLayoutConstraint;
        match constraint.constraint_type {
            x if x == ProtoLayoutConstraint::MaxPanes as i32 => {
                let value = constraint
                    .value
                    .ok_or_else(|| anyhow!("MaxPanes constraint missing value"))?
                    as usize;
                Ok(yinz_types::input::layout::LayoutConstraint::MaxPanes(value))
            },
            x if x == ProtoLayoutConstraint::MinPanes as i32 => {
                let value = constraint
                    .value
                    .ok_or_else(|| anyhow!("MinPanes constraint missing value"))?
                    as usize;
                Ok(yinz_types::input::layout::LayoutConstraint::MinPanes(value))
            },
            x if x == ProtoLayoutConstraint::ExactPanes as i32 => {
                let value = constraint
                    .value
                    .ok_or_else(|| anyhow!("ExactPanes constraint missing value"))?
                    as usize;
                Ok(yinz_types::input::layout::LayoutConstraint::ExactPanes(
                    value,
                ))
            },
            x if x == ProtoLayoutConstraint::NoConstraint as i32 => {
                Ok(yinz_types::input::layout::LayoutConstraint::NoConstraint)
            },
            _ => Err(anyhow!(
                "Invalid LayoutConstraint type: {}",
                constraint.constraint_type
            )),
        }
    }
}

// RunPlugin reverse conversion
impl TryFrom<crate::client_server_contract::client_server_contract::RunPlugin>
    for yinz_types::input::layout::RunPlugin
{
    type Error = anyhow::Error;

    fn try_from(
        plugin: crate::client_server_contract::client_server_contract::RunPlugin,
    ) -> Result<Self> {
        let location = plugin
            .location
            .ok_or_else(|| anyhow!("RunPlugin missing location"))?
            .try_into()?;
        let configuration = plugin
            .configuration
            .ok_or_else(|| anyhow!("RunPlugin missing configuration"))?
            .try_into()?;
        let initial_cwd = plugin.initial_cwd.map(std::path::PathBuf::from);
        let declared_path = plugin.declared_path.map(std::path::PathBuf::from);
        let relative_path = plugin.relative_path.map(std::path::PathBuf::from);

        Ok(yinz_types::input::layout::RunPlugin {
            _allow_exec_host_cmd: plugin.allow_exec_host_cmd,
            location,
            configuration,
            initial_cwd,
            relative_path,
            declared_path,
        })
    }
}

// PluginAlias reverse conversion
impl TryFrom<crate::client_server_contract::client_server_contract::PluginAlias>
    for yinz_types::input::layout::PluginAlias
{
    type Error = anyhow::Error;

    fn try_from(
        plugin_alias: crate::client_server_contract::client_server_contract::PluginAlias,
    ) -> Result<Self> {
        let run_plugin = plugin_alias.run_plugin.and_then(|r| r.try_into().ok());
        let configuration = plugin_alias.configuration.and_then(|c| c.try_into().ok());
        let initial_cwd = plugin_alias.initial_cwd.map(std::path::PathBuf::from);
        Ok(yinz_types::input::layout::PluginAlias {
            name: plugin_alias.name,
            configuration,
            initial_cwd,
            run_plugin,
        })
    }
}

// RunPluginLocation reverse conversion
impl TryFrom<crate::client_server_contract::client_server_contract::RunPluginLocationData>
    for yinz_types::input::layout::RunPluginLocation
{
    type Error = anyhow::Error;

    fn try_from(
        location: crate::client_server_contract::client_server_contract::RunPluginLocationData,
    ) -> Result<Self> {
        use crate::client_server_contract::client_server_contract::{
            run_plugin_location_data::LocationData, RunPluginLocation as ProtoRunPluginLocation,
        };

        let location_data = location
            .location_data
            .ok_or_else(|| anyhow!("RunPluginLocationData missing location_data"))?;
        match location.location_type {
            x if x == ProtoRunPluginLocation::Git as i32 => {
                if let LocationData::GitDescriptor(value) = location_data {
                    Ok(Self::Git(
                        yinz_types::input::git_plugin::GitPlugin::from_descriptor(&value)
                            .map_err(|e| anyhow!(e))?,
                    ))
                } else {
                    Err(anyhow!("Git location type but wrong data variant"))
                }
            },
            x if x == ProtoRunPluginLocation::File as i32 => {
                if let LocationData::FilePath(path) = location_data {
                    Ok(yinz_types::input::layout::RunPluginLocation::File(
                        std::path::PathBuf::from(path),
                    ))
                } else {
                    Err(anyhow!("File location type but wrong data variant"))
                }
            },
            x if x == ProtoRunPluginLocation::Yinz as i32 => {
                if let LocationData::YinzTag(tag) = location_data {
                    Ok(yinz_types::input::layout::RunPluginLocation::Yinz(
                        yinz_types::data::PluginTag::new(tag.tag),
                    ))
                } else {
                    Err(anyhow!("Yinz location type but wrong data variant"))
                }
            },
            _ => Err(anyhow!(
                "Invalid RunPluginLocation type: {}",
                location.location_type
            )),
        }
    }
}

// RunPluginOrAlias reverse conversion
impl TryFrom<crate::client_server_contract::client_server_contract::RunPluginOrAlias>
    for yinz_types::input::layout::RunPluginOrAlias
{
    type Error = anyhow::Error;

    fn try_from(
        plugin: crate::client_server_contract::client_server_contract::RunPluginOrAlias,
    ) -> Result<Self> {
        use crate::client_server_contract::client_server_contract::run_plugin_or_alias::PluginType;

        let plugin_type = plugin
            .plugin_type
            .ok_or_else(|| anyhow!("RunPluginOrAlias missing plugin_type"))?;
        match plugin_type {
            PluginType::Plugin(run_plugin) => Ok(
                yinz_types::input::layout::RunPluginOrAlias::RunPlugin(run_plugin.try_into()?),
            ),
            PluginType::Alias(plugin_alias) => Ok(
                yinz_types::input::layout::RunPluginOrAlias::Alias(plugin_alias.try_into()?),
            ),
        }
    }
}

#[cfg(test)]
#[path = "unit/bubblewrap_decode.rs"]
mod bubblewrap_decode;

#[cfg(test)]
#[path = "unit/retired_targets.rs"]
mod retired_targets;
