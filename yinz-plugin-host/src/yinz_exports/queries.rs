//! Application-state queries and their correlated responses.

use crate::plugin_map::PluginEnv;
use crate::response::{value, DecodedResponse};
use crate::PaneId;
use std::time::Duration;
use yinz_bus::pty::PtyInstruction;
use yinz_bus::screen::ScreenInstruction;
use yinz_runtime::errors::prelude::*;
use yinz_types::data::{
    GetFocusedPaneInfoResponse, GetPaneCwdResponse, GetPaneGitProbeResponse, GetPanePidResponse,
    GetPaneRunningCommandResponse, PaneScrollbackResponse,
};
use yinz_types::internal_ids::TabId;

pub(super) fn get_focused_pane_info(env: &mut PluginEnv) -> Result<DecodedResponse> {
    use crossbeam::channel::RecvTimeoutError;
    use std::time::Duration;

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

    match response {
        GetFocusedPaneInfoResponse::Ok { tab_id, pane_id } => value(
            serde_json::json!({"tab_id": u32::try_from(tab_id.get()).map_err(|_| anyhow!("tab_id is outside u32 range"))?, "pane_id":pane_id}),
        ),
        GetFocusedPaneInfoResponse::Err(error) => Err(anyhow!(error)),
    }
}

/// Guest pane and tab snapshots historically expose unsigned 32-bit sizes.
pub(super) fn project_pane(pane: &mut yinz_types::data::PaneInfo) -> bool {
    for index in pane.index_in_pane_group.values_mut() {
        *index = *index as u32 as usize;
    }
    if let yinz_types::command::PaneTarget::Bubblewrap { root } = &mut pane.target {
        *root = root.display().to_string().into();
    }
    u32::try_from(pane.pane_x).is_ok()
        && u32::try_from(pane.pane_y).is_ok()
        && u32::try_from(pane.pane_content_x).is_ok()
        && u32::try_from(pane.pane_content_y).is_ok()
        && u32::try_from(pane.pane_columns).is_ok()
        && u32::try_from(pane.pane_rows).is_ok()
        && u32::try_from(pane.pane_content_columns).is_ok()
        && u32::try_from(pane.pane_content_rows).is_ok()
        && pane
            .cursor_coordinates_in_pane
            .is_none_or(|cursor| i64::try_from(cursor.x).is_ok() && i64::try_from(cursor.y).is_ok())
}

pub(super) fn project_tab(tab: &mut yinz_types::data::TabInfo) -> bool {
    tab.panes_to_hide = tab.panes_to_hide as u32 as usize;
    tab.selectable_tiled_panes_count = tab.selectable_tiled_panes_count as u32 as usize;
    tab.selectable_floating_panes_count = tab.selectable_floating_panes_count as u32 as usize;
    u32::try_from(tab.tab_id.get()).is_ok()
        && u32::try_from(tab.position.get()).is_ok()
        && u32::try_from(tab.viewport_rows).is_ok()
        && u32::try_from(tab.viewport_columns).is_ok()
        && u32::try_from(tab.display_area_rows).is_ok()
        && u32::try_from(tab.display_area_columns).is_ok()
}

pub(super) fn get_pane_info(
    env: &mut PluginEnv,
    pane_id: yinz_types::data::PaneId,
) -> Result<DecodedResponse> {
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
    let mut pane_info = match response_receiver.recv_timeout(Duration::from_millis(100)) {
        Ok(Some(pane_info)) => Some(pane_info),
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

    value(pane_info.take_if(project_pane))
}

pub(super) fn get_tab_info(env: &mut PluginEnv, tab_id: TabId) -> Result<DecodedResponse> {
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
    let mut tab_info = match response_receiver.recv_timeout(Duration::from_millis(100)) {
        Ok(Some(tab_info)) => Some(tab_info),
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

    value(tab_info.take_if(project_tab))
}

pub(super) fn list_notifications(env: &mut PluginEnv) -> Result<DecodedResponse> {
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
    value(notifications.map_err(anyhow::Error::msg)?)
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

pub(super) fn get_pane_scrollback(
    env: &mut PluginEnv,
    pane_id: PaneId,
    get_full_scrollback: bool,
) -> Result<DecodedResponse> {
    use crossbeam::channel::RecvTimeoutError;
    use std::time::Duration;

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

    match response {
        PaneScrollbackResponse::Ok(contents) => {
            let valid = contents.cursor.is_none_or(|cursor| {
                i64::try_from(cursor.x).is_ok() && i64::try_from(cursor.y).is_ok()
            }) && contents.selected_text.as_ref().is_none_or(|selection| {
                [selection.start, selection.end]
                    .iter()
                    .all(|pos| i64::try_from(pos.column).is_ok() && i64::try_from(pos.line).is_ok())
            });
            value(valid.then_some(contents))
        },
        PaneScrollbackResponse::Err(error) => Err(anyhow!(error)),
    }
}

pub(super) fn get_pane_pid(env: &mut PluginEnv, pane_id: PaneId) -> Result<DecodedResponse> {
    use crossbeam::channel::RecvTimeoutError;
    use std::time::Duration;

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

    match response {
        GetPanePidResponse::Ok(pid) => value(pid.legacy_signed_bits()),
        GetPanePidResponse::Err(error) => Err(anyhow!(error)),
    }
}

pub(super) fn get_pane_running_command(
    env: &mut PluginEnv,
    pane_id: PaneId,
) -> Result<DecodedResponse> {
    let (sender, receiver) = crossbeam::channel::bounded(1);

    send_get_pane_running_command_request(env, pane_id, sender);
    let response = await_pane_running_command_response(env, pane_id, receiver);
    match response {
        GetPaneRunningCommandResponse::Ok(args) => value(args),
        GetPaneRunningCommandResponse::Err(error) => Err(anyhow!(error)),
    }
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

pub(super) fn get_pane_cwd(env: &mut PluginEnv, pane_id: PaneId) -> Result<DecodedResponse> {
    let (sender, receiver) = crossbeam::channel::bounded(1);

    send_get_pane_cwd_request(env, pane_id, sender);
    let response = await_pane_cwd_response(env, pane_id, receiver);
    match response {
        GetPaneCwdResponse::Ok(path) => value(path.to_str().unwrap_or_default()),
        GetPaneCwdResponse::Err(error) => Err(anyhow!(error)),
    }
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
) -> Result<DecodedResponse> {
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

    match response {
        GetPaneGitProbeResponse::Ok(probe) => crate::response::git_probe(probe),
        GetPaneGitProbeResponse::Pending => value(()),
        GetPaneGitProbeResponse::Err(error) => Err(anyhow!(error)),
    }
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
