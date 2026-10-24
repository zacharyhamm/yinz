//! Application-state queries and their correlated responses.

use super::write_response;
use crate::plugin_map::PluginEnv;
use crate::PaneId;
use prost::Message;
use std::time::Duration;
use yinz_bus::pty::PtyInstruction;
use yinz_bus::screen::ScreenInstruction;
use yinz_plugin_api::event::ProtobufPaneScrollbackResponse;
use yinz_plugin_api::plugin_command::{
    ProtobufGetFocusedPaneInfoResponse, ProtobufGetPaneCwdResponse,
    ProtobufGetPaneGitProbeResponse, ProtobufGetPaneInfoResponse, ProtobufGetPanePidResponse,
    ProtobufGetPaneRunningCommandResponse, ProtobufGetTabInfoResponse,
    ProtobufListNotificationsResponse,
};
use yinz_runtime::errors::prelude::*;
use yinz_types::data::{
    GetFocusedPaneInfoResponse, GetPaneCwdResponse, GetPaneGitProbeResponse, GetPanePidResponse,
    GetPaneRunningCommandResponse, PaneScrollbackResponse,
};
use yinz_types::internal_ids::TabId;

pub(super) fn get_focused_pane_info(env: &mut PluginEnv) {
    use crossbeam::channel::RecvTimeoutError;
    use std::time::Duration;

    // Bound up front rather than captured: the closure would otherwise hold an immutable
    // borrow of `env` across the `write_response` that replies to the plugin.
    let plugin_name = env.name();
    let client_id = env.client_id;
    let err_context = || {
        format!(
            "failed to get focused pane info for client {:?} from plugin {}",
            client_id, plugin_name
        )
    };

    // Create oneshot channel for response
    let (response_sender, response_receiver) = crossbeam::channel::bounded(1);

    // Send request to screen thread
    env.senders
        .send_to_screen(ScreenInstruction::GetFocusedPaneInfo {
            client_id: env.client_id,
            response_channel: response_sender,
        })
        .with_context(err_context)
        .non_fatal();

    // Block waiting for response with 100ms timeout
    let response = match response_receiver.recv_timeout(Duration::from_millis(100)) {
        Ok(response) => response,
        Err(RecvTimeoutError::Timeout) => {
            log::error!(
                "GetFocusedPaneInfo timed out for plugin {} for client {:?}",
                env.plugin_id,
                env.client_id
            );
            GetFocusedPaneInfoResponse::Err("Timeout retrieving focused pane info".to_string())
        },
        Err(RecvTimeoutError::Disconnected) => {
            log::error!(
                "GetFocusedPaneInfo channel disconnected for plugin {}",
                env.plugin_id
            );
            GetFocusedPaneInfoResponse::Err(
                "Channel disconnected while retrieving focused pane info".to_string(),
            )
        },
    };

    // Convert to protobuf and write response back to plugin
    let protobuf_response = ProtobufGetFocusedPaneInfoResponse::from(response);
    write_response(env, &protobuf_response.encode_to_vec())
        .with_context(err_context)
        .non_fatal();
}

pub(super) fn get_pane_info(env: &mut PluginEnv, pane_id: yinz_types::data::PaneId) {
    use crossbeam::channel::RecvTimeoutError;
    use std::time::Duration;

    let err_context = || {
        format!(
            "failed to get pane info for pane {:?} from plugin {}",
            pane_id,
            env.name()
        )
    };

    // Create channel for response
    let (response_sender, response_receiver) = crossbeam::channel::bounded(1);

    // Convert from plugin PaneId to server PaneId
    let server_pane_id: PaneId = pane_id;

    // Send request to screen thread
    env.senders
        .send_to_screen(ScreenInstruction::GetPaneInfo {
            pane_id: server_pane_id,
            response_channel: response_sender,
        })
        .with_context(err_context)
        .non_fatal();

    // Block waiting for response with 100ms timeout
    let pane_info = match response_receiver.recv_timeout(Duration::from_millis(100)) {
        Ok(Some(pane_info)) => {
            // Convert PaneInfo to ProtobufPaneInfo
            match pane_info.try_into() {
                Ok(protobuf_pane_info) => Some(protobuf_pane_info),
                Err(e) => {
                    log::error!(
                        "Failed to convert PaneInfo to protobuf for pane {:?}: {}",
                        pane_id,
                        e
                    );
                    None
                },
            }
        },
        Ok(None) => None, // Pane not found
        Err(RecvTimeoutError::Timeout) => {
            log::error!(
                "GetPaneInfo timed out for pane {:?} from plugin {}",
                pane_id,
                env.plugin_id
            );
            None
        },
        Err(RecvTimeoutError::Disconnected) => {
            log::error!(
                "GetPaneInfo channel disconnected for plugin {}",
                env.plugin_id
            );
            None
        },
    };

    let response = ProtobufGetPaneInfoResponse { pane_info };
    let _ = write_response(env, &response.encode_to_vec());
}

pub(super) fn get_tab_info(env: &mut PluginEnv, tab_id: TabId) {
    use crossbeam::channel::RecvTimeoutError;
    use std::time::Duration;

    let err_context = || {
        format!(
            "failed to get tab info for tab {} from plugin {}",
            tab_id,
            env.name()
        )
    };

    // Create channel for response
    let (response_sender, response_receiver) = crossbeam::channel::bounded(1);

    // Send request to screen thread
    env.senders
        .send_to_screen(ScreenInstruction::GetTabInfo {
            tab_id,
            response_channel: response_sender,
        })
        .with_context(err_context)
        .non_fatal();

    // Block waiting for response with 100ms timeout
    let tab_info = match response_receiver.recv_timeout(Duration::from_millis(100)) {
        Ok(Some(tab_info)) => {
            // Convert TabInfo to ProtobufTabInfo
            match tab_info.try_into() {
                Ok(protobuf_tab_info) => Some(protobuf_tab_info),
                Err(e) => {
                    log::error!(
                        "Failed to convert TabInfo to protobuf for tab {}: {}",
                        tab_id,
                        e
                    );
                    None
                },
            }
        },
        Ok(None) => None, // Tab not found
        Err(RecvTimeoutError::Timeout) => {
            log::error!(
                "GetTabInfo timed out for tab {} from plugin {}",
                tab_id,
                env.plugin_id
            );
            None
        },
        Err(RecvTimeoutError::Disconnected) => {
            log::error!(
                "GetTabInfo channel disconnected for plugin {}",
                env.plugin_id
            );
            None
        },
    };

    let response = ProtobufGetTabInfoResponse { tab_info };
    let _ = write_response(env, &response.encode_to_vec());
}

pub(super) fn list_notifications(env: &mut PluginEnv) {
    let (response_sender, response_receiver) = crossbeam::channel::bounded(1);
    let notifications = if env
        .senders
        .send_to_screen(ScreenInstruction::ListNotifications {
            response_channel: response_sender,
        })
        .is_ok()
    {
        response_receiver
            // Client attachment can keep the screen busy beyond a frame interval.
            .recv_timeout(Duration::from_secs(1))
            .map_err(|error| match error {
                crossbeam::channel::RecvTimeoutError::Timeout => {
                    "Timed out listing notifications".to_owned()
                },
                crossbeam::channel::RecvTimeoutError::Disconnected => {
                    "Screen channel disconnected while listing notifications".to_owned()
                },
            })
    } else {
        Err("Failed to request notifications from screen".to_owned())
    };
    let response = match notifications {
        Ok(notifications) => ProtobufListNotificationsResponse {
            notifications: notifications.into_iter().map(Into::into).collect(),
            error: None,
        },
        Err(error) => ProtobufListNotificationsResponse {
            notifications: vec![],
            error: Some(error),
        },
    };
    let _ = write_response(env, &response.encode_to_vec());
}

pub(super) fn list_clients(env: &PluginEnv) {
    let _ = env.senders.to_screen.as_ref().map(|sender| {
        sender.send(ScreenInstruction::ListClientsToPlugin(
            env.plugin_id,
            env.client_id,
            env.generation,
        ))
    });
}

pub(super) fn get_pane_scrollback(env: &mut PluginEnv, pane_id: PaneId, get_full_scrollback: bool) {
    use crossbeam::channel::RecvTimeoutError;
    use std::time::Duration;

    // See the note in `get_focused_pane_info`: bound up front so the closure does not hold a
    // borrow of `env` across the reply.
    let plugin_name = env.name();
    let err_context = || {
        format!(
            "failed to get pane scrollback for pane {:?} from plugin {}",
            pane_id, plugin_name
        )
    };

    // Create oneshot channel for response
    let (response_sender, response_receiver) = crossbeam::channel::bounded(1);

    // Send request to screen thread
    env.senders
        .send_to_screen(ScreenInstruction::GetPaneScrollback {
            pane_id,
            client_id: env.client_id,
            get_full_scrollback,
            response_channel: response_sender,
        })
        .with_context(err_context)
        .non_fatal();

    // Block waiting for response with 5 second timeout
    let response = match response_receiver.recv_timeout(Duration::from_secs(5)) {
        Ok(response) => response,
        Err(RecvTimeoutError::Timeout) => {
            log::error!(
                "GetPaneScrollback timed out after 5s for plugin {} requesting pane {:?}",
                env.plugin_id,
                pane_id
            );
            PaneScrollbackResponse::Err(format!(
                "Timeout retrieving scrollback for pane {:?}",
                pane_id
            ))
        },
        Err(RecvTimeoutError::Disconnected) => {
            log::error!(
                "GetPaneScrollback channel disconnected for plugin {} requesting pane {:?}",
                env.plugin_id,
                pane_id
            );
            PaneScrollbackResponse::Err(format!(
                "Channel disconnected while retrieving scrollback for pane {:?}",
                pane_id
            ))
        },
    };

    // Convert to protobuf and write back to plugin
    ProtobufPaneScrollbackResponse::try_from(response)
        .map_err(|e| anyhow!("Failed to serialize pane scrollback response: {}", e))
        .and_then(|serialized| {
            write_response(env, &serialized.encode_to_vec())?;
            Ok(())
        })
        .with_context(err_context)
        .non_fatal();
}

pub(super) fn get_pane_pid(env: &mut PluginEnv, pane_id: PaneId) {
    use crossbeam::channel::RecvTimeoutError;
    use std::time::Duration;

    // See the note in `get_focused_pane_info`: bound up front so the closure does not hold a
    // borrow of `env` across the reply.
    let plugin_name = env.name();
    let err_context = || {
        format!(
            "failed to get PID for pane {:?} from plugin {}",
            pane_id, plugin_name
        )
    };

    // Create oneshot channel for response
    let (response_sender, response_receiver) = crossbeam::channel::bounded(1);

    // Send request directly to PTY thread
    env.senders
        .send_to_pty(PtyInstruction::GetPanePid {
            pane_id,
            response_channel: response_sender,
        })
        .with_context(err_context)
        .non_fatal();

    // Block waiting for response with 100ms timeout
    let response = match response_receiver.recv_timeout(Duration::from_millis(100)) {
        Ok(response) => response,
        Err(RecvTimeoutError::Timeout) => {
            log::error!(
                "GetPanePid timed out after 5s for plugin {} requesting pane {:?}",
                env.plugin_id,
                pane_id
            );
            GetPanePidResponse::Err(format!("Timeout retrieving PID for pane {:?}", pane_id))
        },
        Err(RecvTimeoutError::Disconnected) => {
            log::error!(
                "GetPanePid channel disconnected for plugin {} requesting pane {:?}",
                env.plugin_id,
                pane_id
            );
            GetPanePidResponse::Err(format!(
                "Channel disconnected while retrieving PID for pane {:?}",
                pane_id
            ))
        },
    };

    // Encode the synchronous command response.
    let protobuf_response = ProtobufGetPanePidResponse::from(response);
    let serialized = protobuf_response.encode_to_vec();
    write_response(env, &serialized)
        .with_context(err_context)
        .non_fatal();
}

pub(super) fn get_pane_running_command(env: &mut PluginEnv, pane_id: PaneId) {
    let (sender, receiver) = crossbeam::channel::bounded(1);

    send_get_pane_running_command_request(env, pane_id, sender);
    let response = await_pane_running_command_response(env, pane_id, receiver);
    write_pane_running_command_response(env, pane_id, response);
}

fn send_get_pane_running_command_request(
    env: &PluginEnv,
    pane_id: PaneId,
    sender: crossbeam::channel::Sender<GetPaneRunningCommandResponse>,
) {
    env.senders
        .send_to_pty(PtyInstruction::GetPaneRunningCommand {
            pane_id,
            response_channel: sender,
        })
        .with_context(|| format!("Failed to get running command for pane {:?}", pane_id))
        .non_fatal();
}

fn await_pane_running_command_response(
    env: &PluginEnv,
    pane_id: PaneId,
    receiver: crossbeam::channel::Receiver<GetPaneRunningCommandResponse>,
) -> GetPaneRunningCommandResponse {
    use crossbeam::channel::RecvTimeoutError;
    use std::time::Duration;

    match receiver.recv_timeout(Duration::from_millis(100)) {
        Ok(response) => response,
        Err(RecvTimeoutError::Timeout) => {
            log::error!(
                "GetPaneRunningCommand timed out for plugin {} requesting pane {:?}",
                env.plugin_id,
                pane_id
            );
            GetPaneRunningCommandResponse::Err(format!(
                "Timeout retrieving running command for pane {:?}",
                pane_id
            ))
        },
        Err(RecvTimeoutError::Disconnected) => {
            log::error!(
                "GetPaneRunningCommand channel disconnected for plugin {} requesting pane {:?}",
                env.plugin_id,
                pane_id
            );
            GetPaneRunningCommandResponse::Err(format!(
                "Channel disconnected while retrieving running command for pane {:?}",
                pane_id
            ))
        },
    }
}

fn write_pane_running_command_response(
    env: &mut PluginEnv,
    pane_id: PaneId,
    response: GetPaneRunningCommandResponse,
) {
    let protobuf_response = ProtobufGetPaneRunningCommandResponse::from(response);
    write_response(env, &protobuf_response.encode_to_vec())
        .with_context(|| format!("Failed to get running command for pane {:?}", pane_id))
        .non_fatal();
}

pub(super) fn get_pane_cwd(env: &mut PluginEnv, pane_id: PaneId) {
    let (sender, receiver) = crossbeam::channel::bounded(1);

    send_get_pane_cwd_request(env, pane_id, sender);
    let response = await_pane_cwd_response(env, pane_id, receiver);
    write_pane_cwd_response(env, pane_id, response);
}

fn send_get_pane_cwd_request(
    env: &PluginEnv,
    pane_id: PaneId,
    sender: crossbeam::channel::Sender<GetPaneCwdResponse>,
) {
    env.senders
        .send_to_pty(PtyInstruction::GetPaneCwd {
            pane_id,
            response_channel: sender,
        })
        .with_context(|| format!("Failed to get CWD for pane {:?}", pane_id))
        .non_fatal();
}

fn await_pane_cwd_response(
    env: &PluginEnv,
    pane_id: PaneId,
    receiver: crossbeam::channel::Receiver<GetPaneCwdResponse>,
) -> GetPaneCwdResponse {
    use crossbeam::channel::RecvTimeoutError;
    use std::time::Duration;

    match receiver.recv_timeout(Duration::from_millis(100)) {
        Ok(response) => response,
        Err(RecvTimeoutError::Timeout) => {
            log::error!(
                "GetPaneCwd timed out for plugin {} requesting pane {:?}",
                env.plugin_id,
                pane_id
            );
            GetPaneCwdResponse::Err(format!("Timeout retrieving CWD for pane {:?}", pane_id))
        },
        Err(RecvTimeoutError::Disconnected) => {
            log::error!(
                "GetPaneCwd channel disconnected for plugin {} requesting pane {:?}",
                env.plugin_id,
                pane_id
            );
            GetPaneCwdResponse::Err(format!(
                "Channel disconnected while retrieving CWD for pane {:?}",
                pane_id
            ))
        },
    }
}

fn write_pane_cwd_response(env: &mut PluginEnv, pane_id: PaneId, response: GetPaneCwdResponse) {
    let protobuf_response = ProtobufGetPaneCwdResponse::from(response);
    write_response(env, &protobuf_response.encode_to_vec())
        .with_context(|| format!("Failed to get CWD for pane {:?}", pane_id))
        .non_fatal();
}

/// The far machine's Git probe for a remote pane, from the cache the broker
/// pushes up.
///
/// One channel round trip and no process: unlike the local path, which spawns
/// two `git` processes per folder, this is only a read of what has already
/// arrived. A refresh token also requests a new probe; only its correlated result
/// can answer that request. The plugin polls without blocking the PTY thread.
pub(super) fn get_pane_git_probe(
    env: &mut PluginEnv,
    pane_id: PaneId,
    cwd: String,
    refresh_token: Option<String>,
) {
    let (sender, receiver) = crossbeam::channel::bounded(1);

    env.senders
        .send_to_pty(PtyInstruction::GetPaneGitProbe {
            refresh_token,
            pane_id,
            cwd,
            response_channel: sender,
        })
        .with_context(|| format!("Failed to get the git probe for pane {:?}", pane_id))
        .non_fatal();

    let response = await_pane_git_probe_response(env, pane_id, receiver);

    let protobuf_response = ProtobufGetPaneGitProbeResponse::from(response);
    write_response(env, &protobuf_response.encode_to_vec())
        .with_context(|| format!("Failed to get the git probe for pane {:?}", pane_id))
        .non_fatal();
}

fn await_pane_git_probe_response(
    env: &PluginEnv,
    pane_id: PaneId,
    receiver: crossbeam::channel::Receiver<GetPaneGitProbeResponse>,
) -> GetPaneGitProbeResponse {
    use crossbeam::channel::RecvTimeoutError;
    use std::time::Duration;

    match receiver.recv_timeout(Duration::from_millis(100)) {
        Ok(response) => response,
        Err(RecvTimeoutError::Timeout) => {
            log::error!(
                "GetPaneGitProbe timed out for plugin {} requesting pane {:?}",
                env.plugin_id,
                pane_id
            );
            GetPaneGitProbeResponse::Err(format!(
                "Timeout retrieving the git probe for pane {:?}",
                pane_id
            ))
        },
        Err(RecvTimeoutError::Disconnected) => {
            log::error!(
                "GetPaneGitProbe channel disconnected for plugin {} requesting pane {:?}",
                env.plugin_id,
                pane_id
            );
            GetPaneGitProbeResponse::Err(format!(
                "Channel disconnected while retrieving the git probe for pane {:?}",
                pane_id
            ))
        },
    }
}
