//! Session lifecycle, persistence, and cross-session pane transfers.

use super::layouts::ensure_layout_info_permissions;
use super::pane_creation::translate_plugin_path;
use super::write_response;
use crate::plugin_map::PluginEnv;
use crate::{PaneId, PluginInstruction};
use anyhow::ensure;
use prost::Message;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::oneshot;
use yinz_bus::global_async_runtime::get_tokio_runtime;
use yinz_bus::route::NotificationEnd;
use yinz_bus::screen::ScreenInstruction;
use yinz_bus::ServerInstruction;
use yinz_ipc::consts::ipc_connect;
use yinz_ipc::ipc::{ClientToServerMsg, IpcSenderWithContext};
use yinz_plugin_api::plugin_command::{
    dump_session_layout_response, save_session_response, ProtobufDeleteAllDeadSessionsResponse,
    ProtobufDeleteDeadSessionResponse, ProtobufDumpSessionLayoutResponse,
    ProtobufGetSessionListResponse, ProtobufKillSessionsResponse,
    ProtobufListOrphanedPanesResponse, ProtobufSaveSessionResponse, ProtobufWarpPaneResponse,
};
use yinz_routing::wait_for_action_completion;
use yinz_runtime::consts::{YINZ_SESSION_INFO_CACHE_DIR, YINZ_SOCK_DIR};
use yinz_runtime::errors::prelude::*;
use yinz_types::data::{
    DeleteAllDeadSessionsResponse, DeleteDeadSessionResponse, KillSessionsResponse, LayoutInfo,
    ListOrphanedPanesResponse, OrphanedPane, SessionConnection, WarpPaneResponse, WarpPlacement,
};
use yinz_types::input::actions::Action;
use yinz_types::internal_ids::{SessionName, SupervisorId, TabId, TabNumber, TabPosition};

pub(super) fn detach(env: &PluginEnv) {
    let action = Action::Detach;
    let error_msg = || "Failed to detach".to_string();
    apply_action!(action, error_msg, env);
}

pub(crate) fn resolve_host_directory(env: &PluginEnv, path: &str) -> Result<PathBuf> {
    let path = yinz_runtime::shared::expand_path(path).map_err(anyhow::Error::msg)?;
    let path = if path.is_relative() {
        env.plugin_cwd.join(path)
    } else {
        path
    };
    ensure!(
        path.is_dir(),
        "Directory does not exist or is not accessible"
    );
    Ok(path)
}

pub(super) fn switch_session(
    env: &PluginEnv,
    session_name: Option<SessionName>,
    tab_position: Option<TabPosition>,
    pane_id: Option<yinz_types::pane_id::PaneId>,
    layout: Option<LayoutInfo>,
    cwd: Option<PathBuf>,
) -> Result<()> {
    let err_context = || "Failed to switch session".to_string();
    if let Some(layout) = layout.as_ref() {
        ensure_layout_info_permissions(env, layout)?;
    }

    let (completion_tx, completion_rx) = oneshot::channel();

    if session_name
        .as_ref()
        .map(|s| s.as_str().contains('/'))
        .unwrap_or(false)
    {
        log::error!("Session names cannot contain \'/\'");
    } else {
        let client_id = env.client_id;
        let tab_position = tab_position
            .map(TabNumber::try_from)
            .transpose()
            .map_err(anyhow::Error::msg)?;
        let cwd = if matches!(&env.plugin_config.location, yinz_types::input::layout::RunPluginLocation::Yinz(tag) if tag.to_string() == "session-palette")
        {
            // The session palette edits host directories, including the host's /tmp.
            cwd.map(|path| resolve_host_directory(env, &path.to_string_lossy()))
                .transpose()?
        } else {
            cwd.map(|c| translate_plugin_path(env, c))
                .or_else(|| Some(env.plugin_cwd.clone()))
        };
        let connect_to_session = SessionConnection {
            name: session_name,
            tab_position,
            pane_id,
            layout,
            cwd,
        };
        env.senders
            .send_to_server(ServerInstruction::SwitchSession(
                connect_to_session,
                client_id,
                Some(NotificationEnd::new(completion_tx)),
            ))
            .with_context(err_context)?;
        let wait_forever = false;
        let _ = wait_for_action_completion(completion_rx, "switch_session", wait_forever);
    }
    Ok(())
}

pub(super) fn delete_dead_session(session_name: &SessionName) -> Result<()> {
    yinz_session::sessions::delete_session(session_name, false)
        .with_context(|| format!("Failed to delete dead session: {:?}", session_name))
}

pub(super) fn delete_all_dead_sessions() -> Result<()> {
    use yinz_ipc::consts::{is_ipc_socket, is_session_socket_name};
    let mut live_sessions = vec![];
    if let Ok(files) = std::fs::read_dir(&*YINZ_SOCK_DIR) {
        files.for_each(|file| {
            if let Ok(file) = file
                && let Ok(file_name) = file.file_name().into_string()
                && is_ipc_socket(&file.file_type().unwrap())
                && is_session_socket_name(&file_name)
            {
                live_sessions.push(SessionName::new(file_name));
            }
        });
    }
    let dead_sessions: Vec<SessionName> = match std::fs::read_dir(&*YINZ_SESSION_INFO_CACHE_DIR) {
        Ok(files_in_session_info_folder) => {
            let files_that_are_folders = files_in_session_info_folder
                .filter_map(|f| f.ok().map(|f| f.path()))
                .filter(|f| f.is_dir());
            files_that_are_folders
                .filter_map(|folder_name| {
                    let session_name = SessionName::new(folder_name.file_name()?.to_str()?);
                    if live_sessions.contains(&session_name) {
                        // this is not a dead session...
                        return None;
                    }
                    Some(session_name)
                })
                .collect()
        },
        Err(e) => {
            log::error!("Failed to read session info cache dir: {:?}", e);
            vec![]
        },
    };
    for session in dead_sessions {
        delete_dead_session(&session)?;
    }
    Ok(())
}

pub(super) fn quit_yinz(env: &PluginEnv) {
    let error_msg = || format!("failed to quit yinz in plugin {}", env.name());
    let action = Action::Quit;
    apply_action!(action, error_msg, env);
}

pub(super) fn rename_session(env: &PluginEnv, new_session_name: SessionName) {
    let error_msg = || format!("failed to rename session in plugin {}", env.name());
    if new_session_name.as_str().contains('/') {
        log::error!("Session names cannot contain \'/\'");
    } else {
        let action = Action::RenameSession {
            name: new_session_name,
        };
        apply_action!(action, error_msg, env);
    }
}

pub(super) fn disconnect_other_clients(env: &PluginEnv) {
    let _ = env
        .senders
        .send_to_server(ServerInstruction::DisconnectAllClientsExcept(env.client_id))
        .context("failed to send disconnect other clients instruction");
}

pub(super) fn kill_sessions(session_names: Vec<SessionName>) {
    for session_name in session_names {
        let path = &*YINZ_SOCK_DIR.join(&session_name);
        match ipc_connect(path) {
            Ok(stream) => {
                let _ = IpcSenderWithContext::<ClientToServerMsg>::new(stream)
                    .send_client_msg(ClientToServerMsg::KillSession);
            },
            Err(e) => {
                log::error!("Failed to kill session {}: {:?}", session_name, e);
            },
        };
    }
}

// Wedge timeout: only guards against a peer that neither shuts down nor
// crashes. Normal kill latency is tens of milliseconds (peer's route loop
// reads the message, server sends Exit back), so 500 ms is several times the
// expected worst case while still feeling instant to the user. Applied as a
// single budget over the whole batch -- per-session kills are issued
// concurrently so killing many sessions does not multiply the wait.
const KILL_WEDGE_TIMEOUT: Duration = Duration::from_millis(500);

pub(super) fn kill_sessions_and_reply(env: &mut PluginEnv, session_names: Vec<SessionName>) {
    use tokio::task::JoinSet;
    let runtime = get_tokio_runtime();
    let result: Result<(), String> = runtime.block_on(async {
        let mut set: JoinSet<(SessionName, std::io::Result<()>)> = JoinSet::new();
        for name in session_names {
            let path = YINZ_SOCK_DIR.join(&name);
            set.spawn(async move {
                let res = yinz_ipc::ipc::async_send_kill_and_await(&path).await;
                (name, res)
            });
        }
        let drain = async {
            let mut first_err: Option<String> = None;
            while let Some(joined) = set.join_next().await {
                match joined {
                    Ok((_name, Ok(()))) => {},
                    Ok((name, Err(e))) => {
                        if first_err.is_none() {
                            first_err = Some(format!("Failed to kill session {}: {}", name, e));
                        }
                    },
                    Err(e) => {
                        if first_err.is_none() {
                            first_err = Some(format!("Internal error in kill task: {}", e));
                        }
                    },
                }
            }
            match first_err {
                Some(e) => Err(e),
                None => Ok(()),
            }
        };
        match tokio::time::timeout(KILL_WEDGE_TIMEOUT, drain).await {
            Ok(res) => res,
            Err(_) => {
                Err("Timed out waiting for one or more sessions to acknowledge kill".to_string())
            },
        }
    });
    let response = match result {
        Ok(()) => KillSessionsResponse::Ok,
        Err(e) => KillSessionsResponse::Err(e),
    };
    let protobuf_response = ProtobufKillSessionsResponse::from(response);
    write_response(env, &protobuf_response.encode_to_vec())
        .with_context(|| "failed to write kill_sessions response".to_string())
        .non_fatal();
}

// ---------------------------------------------------------------------------
// Warping panes between sessions
// ---------------------------------------------------------------------------
//
// These block the calling plugin's thread for as long as the warp takes, which
// can be seconds — a supervisor is released here, attached over a socket there,
// and the pane's scrollback travels with it. That is a real cost (the pinned
// executor co-pins plugins once it is at capacity, so a long block can stall a
// neighbour), and it is paid deliberately: the alternative is handing a plugin
// an acknowledgement and no way to learn whether its pane arrived.
//
// What keeps it cheap in practice is that `crate::warp` checks the far session
// is reachable before anything is released, so the overwhelmingly common failure
// — a session name that does not exist — costs a `connect(2)`.

pub(super) fn warp_pane_to_session(
    env: &mut PluginEnv,
    pane_id: PaneId,
    to_session: SessionName,
    placement: WarpPlacement,
) {
    let (result, hold) = yinz_bus::warp::warp_pane_to_session(
        &env.senders,
        env.client_id,
        // Always explicit, never "whichever pane is focused": a plugin is acting
        // on a snapshot it took earlier, and for a shared plugin instance
        // `env.client_id` is whichever client's event is being handled.
        Some(pane_id),
        &env.server_session.current(),
        &to_session,
        placement,
        yinz_bus::warp::PLUGIN_WARP_TIMEOUT,
    );
    write_warp_response(env, result);
    // As in the CLI path: the session stays open until the answer is written.
    drop(hold);
}

pub(super) fn warp_pane_from_session(
    env: &mut PluginEnv,
    pane_id: PaneId,
    from_session: SessionName,
    placement: WarpPlacement,
) {
    let result = yinz_bus::warp::warp_pane_from_session(
        &env.server_session.current(),
        &from_session,
        pane_id,
        placement,
        yinz_bus::warp::WARP_PULL_TIMEOUT,
    );
    write_warp_response(env, result);
}

pub(super) fn warp_tab_to_session(env: &mut PluginEnv, tab_id: TabId, to_session: SessionName) {
    let result = yinz_bus::warp::warp_tab_to_session(
        &env.senders,
        env.client_id,
        // Explicit for the same reason the pane version is: a plugin acts on a
        // snapshot, and `env.client_id` is whichever client's event is in hand.
        Some(tab_id),
        &env.server_session.current(),
        &to_session,
        yinz_bus::warp::PLUGIN_WARP_TIMEOUT,
    );
    write_warp_response(env, result);
}

pub(super) fn warp_tab_from_session(env: &mut PluginEnv, tab_id: TabId, from_session: SessionName) {
    let result = yinz_bus::warp::warp_tab_from_session(
        &env.server_session.current(),
        &from_session,
        tab_id,
        yinz_bus::warp::WARP_PULL_TIMEOUT,
    );
    write_warp_response(env, result);
}

#[cfg(unix)]
pub(super) fn adopt_orphaned_pane(
    env: &mut PluginEnv,
    host_id: SupervisorId,
    placement: WarpPlacement,
) {
    let result = crate::pty_host::cli::adopt_payload(host_id.as_str())
        .map_err(|e| format!("{e:#}"))
        .and_then(|payload| {
            yinz_bus::warp::adopt_pane_here(
                &env.senders,
                env.client_id,
                Box::new(payload),
                placement,
                None,
                yinz_bus::warp::PLUGIN_WARP_TIMEOUT,
            )
        });
    write_warp_response(env, result);
}

#[cfg(not(unix))]
pub(super) fn adopt_orphaned_pane(
    env: &PluginEnv,
    _host_id: SupervisorId,
    _placement: WarpPlacement,
) {
    write_warp_response(env, Err(NO_DETACHABLE_PANES.to_owned()));
}

#[cfg(unix)]
pub(super) fn list_orphaned_panes(env: &mut PluginEnv) {
    let response = match crate::pty_host::cli::statuses() {
        Ok(statuses) => ListOrphanedPanesResponse::Ok(
            statuses
                .iter()
                // An attached supervisor is just a normal pane in some session,
                // already reachable through `warp_pane_from_session`. Offering it
                // here too would give a caller two ways to do one thing.
                .filter(|status| !status.attached)
                .map(|status| OrphanedPane {
                    host_id: status.host_id.clone(),
                    session_name: status.session_name.clone(),
                    command: status.command.clone(),
                    cwd: status.cwd.clone(),
                    child_pid: status.child_pid,
                    started_at_unix: status.started_at_unix,
                    exit_status: status.child_exit_status,
                    buffered_bytes: status.buffered_bytes as u64,
                })
                .collect(),
        ),
        Err(e) => ListOrphanedPanesResponse::Err(format!("{e:#}")),
    };
    write_orphaned_pane_list(env, response);
}

#[cfg(not(unix))]
pub(super) fn list_orphaned_panes(env: &PluginEnv) {
    // Deliberately an error and not an empty list: an empty list says "there is
    // nothing to recover", which is a different claim and a false one.
    write_orphaned_pane_list(
        env,
        ListOrphanedPanesResponse::Err(NO_DETACHABLE_PANES.to_owned()),
    );
}

#[cfg(not(unix))]
const NO_DETACHABLE_PANES: &str = "detachable panes are not supported on this platform";

fn write_warp_response(env: &mut PluginEnv, result: Result<String, String>) {
    let response = match result {
        Ok(message) => WarpPaneResponse::Ok(message),
        Err(message) => WarpPaneResponse::Err(message),
    };
    write_response(
        env,
        &ProtobufWarpPaneResponse::from(response).encode_to_vec(),
    )
    .with_context(|| "failed to write the warp response".to_string())
    .non_fatal();
}

fn write_orphaned_pane_list(env: &mut PluginEnv, response: ListOrphanedPanesResponse) {
    write_response(
        env,
        &ProtobufListOrphanedPanesResponse::from(response).encode_to_vec(),
    )
    .with_context(|| "failed to write the orphaned pane list".to_string())
    .non_fatal();
}

pub(super) fn delete_dead_session_and_reply(env: &mut PluginEnv, session_name: SessionName) {
    let response = match delete_dead_session(&session_name) {
        Ok(()) => DeleteDeadSessionResponse::Ok,
        Err(e) => DeleteDeadSessionResponse::Err(format!("{e:#}")),
    };
    let protobuf_response = ProtobufDeleteDeadSessionResponse::from(response);
    write_response(env, &protobuf_response.encode_to_vec())
        .with_context(|| "failed to write delete_dead_session response".to_string())
        .non_fatal();
}

pub(super) fn delete_all_dead_sessions_and_reply(env: &mut PluginEnv) {
    // Same budget as kill-all so the UX of "y to confirm" is consistent: the
    // host-side fs work for many dead sessions is bounded in wall time.
    let runtime = get_tokio_runtime();
    let result: Result<(), String> = runtime.block_on(async {
        let fs_task = tokio::task::spawn_blocking(delete_all_dead_sessions);
        match tokio::time::timeout(KILL_WEDGE_TIMEOUT, fs_task).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(e))) => Err(format!("Failed to delete dead sessions: {}", e)),
            Ok(Err(e)) => Err(format!("Internal error in delete-all task: {}", e)),
            Err(_) => Err("Timed out deleting dead sessions".to_string()),
        }
    });
    let response = match result {
        Ok(()) => DeleteAllDeadSessionsResponse::Ok,
        Err(e) => DeleteAllDeadSessionsResponse::Err(e),
    };
    let protobuf_response = ProtobufDeleteAllDeadSessionsResponse::from(response);
    write_response(env, &protobuf_response.encode_to_vec())
        .with_context(|| "failed to write delete_all_dead_sessions response".to_string())
        .non_fatal();
}

// note this removes the requesting plugin
pub(super) fn dump_session_layout(env: &mut PluginEnv, tab_index: Option<TabId>) {
    use crate::DumpSessionLayoutResponse;
    let trace = yinz_bus::plugins::LayoutQueryTrace::default();
    trace.mark("screen queued");

    // Create oneshot channel for response
    let (response_sender, response_receiver) = crossbeam::channel::bounded(1);

    // Send request to screen thread
    let send_result = env
        .senders
        .send_to_screen(ScreenInstruction::DumpLayoutToPlugin {
            plugin_id: env.plugin_id,
            tab_index,
            response_channel: response_sender,
            trace: trace.clone(),
        });

    if let Err(e) = send_result {
        let error_response = ProtobufDumpSessionLayoutResponse {
            result: Some(dump_session_layout_response::Result::Error(format!(
                "Failed to send dump layout request: {}",
                e
            ))),
            metadata: None,
        };
        let _ = write_response(env, &error_response.encode_to_vec());
        return;
    }

    // Wait for response with 1 second timeout
    let response: DumpSessionLayoutResponse =
        match response_receiver.recv_timeout(Duration::from_secs(1)) {
            Ok(resp) => resp,
            Err(e) => {
                log::error!(
                    "layout query timeout: pid={} plugin={} generation={:?} {trace:?}",
                    std::process::id(),
                    env.plugin_id,
                    env.generation
                );
                let error_response = ProtobufDumpSessionLayoutResponse {
                    result: Some(dump_session_layout_response::Result::Error(format!(
                        "Timeout waiting for session layout: {}",
                        e
                    ))),
                    metadata: None,
                };
                let _ = write_response(env, &error_response.encode_to_vec());
                return;
            },
        };

    // Convert LayoutMetadata to protobuf
    let protobuf_metadata = response
        .metadata
        .and_then(|metadata| match metadata.try_into() {
            Ok(pb) => Some(pb),
            Err(e) => {
                log::error!("Failed to convert LayoutMetadata to protobuf: {}", e);
                None
            },
        });

    // Build protobuf response
    let protobuf_response = match response.layout_result {
        Ok(layout_content) => ProtobufDumpSessionLayoutResponse {
            result: Some(dump_session_layout_response::Result::LayoutContent(
                layout_content,
            )),
            metadata: protobuf_metadata,
        },
        Err(error) => ProtobufDumpSessionLayoutResponse {
            result: Some(dump_session_layout_response::Result::Error(error)),
            metadata: None,
        },
    };

    // Return the response to the plugin dispatcher
    let _ = write_response(env, &protobuf_response.encode_to_vec())
        .with_context(|| format!("failed to send session layout to plugin {}", env.name()));
}

pub(super) fn save_session(env: &mut PluginEnv) {
    use save_session_response::Result as SaveSessionResult;
    let (completion_tx, completion_rx) = oneshot::channel();

    let send_result = env.senders.send_to_screen(ScreenInstruction::SaveSession(
        env.client_id,
        Some(NotificationEnd::new(completion_tx)),
    ));

    let response = if let Err(e) = send_result {
        ProtobufSaveSessionResponse {
            result: Some(SaveSessionResult::Error(format!(
                "Failed to send save request: {}",
                e
            ))),
        }
    } else {
        let wait_forever = false;
        let _result = wait_for_action_completion(completion_rx, "save_session", wait_forever);
        ProtobufSaveSessionResponse {
            result: Some(SaveSessionResult::Success(true)),
        }
    };

    let _ = write_response(env, &response.encode_to_vec());
}

pub(super) fn current_session_last_saved_time(env: &mut PluginEnv) {
    use yinz_plugin_api::plugin_command::ProtobufCurrentSessionLastSavedTimeResponse;

    let (response_tx, response_rx) = crossbeam::channel::bounded(1);
    let send_result = env
        .senders
        .send_to_plugin(PluginInstruction::GetLastSessionSaveTime {
            response_channel: response_tx,
        });

    let saved_timestamp_millis = if send_result.is_ok() {
        response_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .ok()
            .flatten()
    } else {
        None
    };

    let timestamp_millis = saved_timestamp_millis.map(|saved_time| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        now.saturating_sub(saved_time)
    });

    let response = ProtobufCurrentSessionLastSavedTimeResponse { timestamp_millis };
    let _ = write_response(env, &response.encode_to_vec());
}

/// What this plugin wants said, and decided, when the session is about to end.
///
/// Keyed by plugin id rather than by pane id: it is the plugin that has something outstanding,
/// and a plugin that moves pane - agent does, between tabs - has not changed its mind about the
/// quit on the way.
pub(super) fn set_quit_notice(env: &PluginEnv, notice: Option<String>, guard: bool) {
    let _ = env.senders.send_to_screen(ScreenInstruction::SetQuitNotice(
        env.plugin_id,
        notice,
        guard,
    ));
}

pub(super) fn get_disconnect_targets(env: &mut PluginEnv) {
    use yinz_plugin_api::plugin_command::ProtobufGetDisconnectTargetsResponse;
    let (sender, receiver) = crossbeam::channel::bounded(1);
    let result = env
        .senders
        .send_to_server(ServerInstruction::GetDisconnectTargets(
            env.client_id,
            sender,
        ))
        .context("send disconnect-target query")
        .and_then(|()| {
            receiver
                .recv_timeout(std::time::Duration::from_millis(100))
                .context("receive disconnect targets")
        });
    let response = match result {
        Ok(targets) => ProtobufGetDisconnectTargetsResponse {
            client_ids: targets.into_iter().map(|id| id.get()).collect(),
            error: None,
        },
        Err(error) => ProtobufGetDisconnectTargetsResponse {
            client_ids: vec![],
            error: Some(format!("{error:#}")),
        },
    };
    write_response(env, &response.encode_to_vec())
        .context("write disconnect-target response")
        .non_fatal();
}

pub(crate) fn session_list_snapshot(
    env: &PluginEnv,
) -> Result<yinz_types::data::SessionListSnapshot> {
    use yinz_bus::background_jobs::{scan_session_list_default_dirs, session_scan_state};
    use yinz_types::data::SessionListSnapshot;

    match session_scan_state() {
        Some(state) => {
            let (session_name, mut current, plugin_list) = {
                let name = state.current_session_name.lock().unwrap().clone();
                let info = state.current_session_info.lock().unwrap().clone();
                let plugins = state.current_session_plugin_list.lock().unwrap().clone();
                (name, info, plugins)
            };

            let (mut live_sessions_map, mut resurrectable_sessions_map) =
                scan_session_list_default_dirs(
                    &session_name,
                    &current.available_layouts,
                    &plugin_list,
                );

            // Disk metadata can lag and omits pane targets. The host's current snapshot
            // is authoritative for local panes, even before the first serialization.
            if !current.name.is_empty() {
                current.name = session_name.clone();
                current.is_current_session = true;
                current.populate_plugin_list(plugin_list.into_iter().collect());
                resurrectable_sessions_map.remove(&session_name);
                live_sessions_map.insert(session_name, current);
            }

            let _ = env
                .senders
                .send_to_screen(ScreenInstruction::UpdateSessionInfos(
                    live_sessions_map.clone(),
                    resurrectable_sessions_map.clone(),
                ));

            let snapshot = SessionListSnapshot {
                live_sessions: live_sessions_map.into_values().collect(),
                resurrectable_sessions: resurrectable_sessions_map.into_iter().collect(),
            };
            Ok(snapshot)
        },
        None => Err(anyhow!("Session-scan state not initialized")),
    }
}

pub(super) fn get_session_list(env: &mut PluginEnv) {
    let response = match session_list_snapshot(env) {
        Ok(snapshot) => yinz_types::data::GetSessionListResponse::Ok(snapshot),
        Err(error) => yinz_types::data::GetSessionListResponse::Err(format!("{error:#}")),
    };
    let protobuf_response = ProtobufGetSessionListResponse::from(response);
    write_response(env, &protobuf_response.encode_to_vec())
        .with_context(|| "failed to write get_session_list response".to_string())
        .non_fatal();
}
