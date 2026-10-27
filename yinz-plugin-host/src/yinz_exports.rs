//! Permission-gated plugin command dispatch; handlers are grouped by host responsibility.

macro_rules! apply_action {
    ($action:ident, $error_message:ident, $env: ident) => {
        match yinz_routing::route_action(
            $action,
            $env.client_id,
            None,
            // A headless plugin has no pane to place things near: they go by the client.
            $env.own_pane(),
            $env.senders.clone(),
            $env.default_shell.clone(),
            None,
            $env.default_mode.clone(),
            None,
            false,
            false, // is_client_input
        ) {
            Ok((_, result)) => result,
            Err(e) => {
                log::error!("{}: {:?}", $error_message(), e);
                None
            },
        }
    };
}

#[path = "agent_control.rs"]
mod agent_control;
mod agent_launch;
#[cfg(test)]
#[path = "unit/agent_spawn_tests.rs"]
mod agent_spawn_tests;
mod data;
mod execution;
mod filesystem;
#[cfg(test)]
#[path = "unit/layout_cwd_tests.rs"]
mod layout_cwd_tests;
mod layouts;
#[path = "multipanes.rs"]
mod multipanes;
#[path = "palette_agents.rs"]
pub(crate) mod palette_agents;
mod pane_control;
mod pane_creation;
#[cfg(test)]
#[path = "unit/permission_tests.rs"]
mod permission_tests;
mod permissions;
#[cfg(test)]
#[path = "unit/plugin_ids_tests.rs"]
mod plugin_ids_tests;
#[cfg(test)]
#[path = "unit/plugin_log_tests.rs"]
mod plugin_log_tests;
mod plugins;
mod queries;
mod sessions;
#[cfg(test)]
#[path = "unit/validation_tests.rs"]
mod validation_tests;

use crate::plugin_map::PluginEnv;
use crate::plugin_panes::Surface;
use crate::PluginInstruction;
pub(crate) use agent_control::agent_event_projects;
pub use agent_control::control_agent;
pub use agent_control::spawn_agent_for_cli;
use agent_launch::spawn_agent;
pub(crate) use data::notify_data_watchers;
pub(crate) use data::poll_data_watchers;
use data::{
    data_apply, data_catalog, data_count, data_define_entity, data_fetch, data_unwatch, data_watch,
};
use execution::{cancel_web_request, exec_cmd, run_command, set_timeout, web_request};
use filesystem::{
    fs_create_dir, fs_list, fs_metadata, fs_read, fs_remove, fs_rename, fs_write, scan_host_folder,
};
use layouts::{
    delete_layout, dump_layout, edit_layout, get_layout_dir, new_tab, new_tab_unfocused,
    new_tabs_with_layout, new_tabs_with_layout_info, new_tiled_pane_in_tab, override_layout,
    parse_layout, rename_layout, save_layout,
};
use pane_control::{
    break_panes_to_new_tab, break_panes_to_tab_with_id, break_panes_to_tab_with_index,
    change_floating_panes_coordinates, change_host_folder, change_overlay_coordinates,
    clear_pane_highlights, clear_screen, clear_screen_for_pane_id, close_focus, close_focused_tab,
    close_multiple_panes, close_plugin_pane, close_self, close_tab_with_id, close_tab_with_index,
    close_terminal_pane, copy_to_clipboard, edit_scrollback, edit_scrollback_for_pane_with_id,
    embed_multiple_panes, float_multiple_panes, focus_host_session, focus_last_pane,
    focus_next_pane, focus_or_create_tab, focus_plugin_pane, focus_previous_pane,
    focus_terminal_pane, go_to_next_tab, go_to_previous_tab, go_to_tab, go_to_tab_name,
    go_to_tab_with_id, group_and_ungroup_panes, hide_floating_panes, hide_pane_with_id, hide_self,
    highlight_and_unhighlight_panes, move_focus, move_focus_or_tab, move_pane,
    move_pane_with_direction, move_pane_with_pane_id, move_pane_with_pane_id_in_direction,
    new_pane, next_swap_layout, notify, page_scroll_down, page_scroll_down_in_pane_id,
    page_scroll_up, page_scroll_up_in_pane_id, previous_swap_layout, rename_plugin_pane,
    rename_tab, rename_tab_with_id, rename_terminal_pane, replace_pane_with_existing_pane, resize,
    resize_pane_with_id, resize_with_direction, reveal_pane_with_id, scroll_down,
    scroll_down_in_pane_id, scroll_to_bottom, scroll_to_bottom_in_pane_id, scroll_to_top,
    scroll_to_top_in_pane_id, scroll_up, scroll_up_in_pane_id, send_sigint_to_pane_id,
    send_sigkill_to_pane_id, set_floating_pane_pinned, set_pane_agent, set_pane_agent_run,
    set_pane_borderless, set_pane_color, set_pane_frame_style, set_pane_regex_highlights,
    set_pane_subtitle, set_pane_title_prefix, set_self_mouse_selection_support, set_tab_projects,
    show_floating_panes, show_pane_with_id, show_self, stack_panes, switch_tab_to,
    switch_tab_to_id, switch_to_mode, toggle_active_tab_sync, toggle_floating_panes,
    toggle_focus_fullscreen, toggle_focus_no_ui_fullscreen, toggle_pane_borderless,
    toggle_pane_embed_or_eject, toggle_pane_embed_or_eject_for_pane_id, toggle_pane_frames,
    toggle_pane_id_fullscreen, toggle_tab, undo_rename_pane, undo_rename_tab, write, write_chars,
    write_chars_to_pane_id, write_to_pane_id,
};
use pane_creation::{
    open_command_pane, open_command_pane_background, open_command_pane_floating,
    open_command_pane_floating_near_plugin, open_command_pane_in_git_checkout_near_plugin,
    open_command_pane_in_new_tab, open_command_pane_in_place,
    open_command_pane_in_place_of_pane_id, open_command_pane_in_place_of_plugin,
    open_command_pane_near_plugin, open_command_pane_overlay, open_edit_pane_in_place_of_pane_id,
    open_editor_pane_in_new_tab, open_file, open_file_floating, open_file_floating_near_plugin,
    open_file_in_place, open_file_in_place_of_plugin, open_file_near_plugin,
    open_plugin_pane_floating, open_plugin_pane_in_new_tab, open_plugin_pane_in_place_of_pane_id,
    open_terminal, open_terminal_floating, open_terminal_floating_near_plugin,
    open_terminal_in_place, open_terminal_in_place_of_plugin, open_terminal_near_plugin,
    open_terminal_pane_in_place_of_pane_id, rerun_command_pane, run_action, run_command_in_pane,
    translate_plugin_path,
};
pub(crate) use permissions::check_command_permission;
pub(crate) use permissions::permission_answer_pending;
use permissions::{grant_plugin_permissions, request_permission};
pub(crate) use plugins::broadcast_agent_providers;
pub(crate) use plugins::broadcast_project_providers;
use plugins::{
    block_cli_pipe_input, check_plugin, clear_key_presses_intercepts, cli_pipe_output,
    configuration_editor, generate_random_name, get_cell_pixel_size, get_local_time,
    get_local_utc_offset, get_plugin_alias_configuration, get_plugin_ids, get_plugin_ids_for_url,
    get_plugin_info, get_session_environment_variables, get_user_plugin_dir, get_yinz_version,
    intercept_key_presses, load_new_plugin, message_to_plugin, post_message_to,
    post_message_to_plugin, project_config, read_plugin_log, rebind_keys, reconfigure,
    register_default_keybindings, reload_plugin, reload_plugin_by_url, report_panic,
    set_selectable, show_cursor, start_or_reload_plugin, subscribe, unblock_cli_pipe_input,
    unsubscribe, write_agent_providers_response, write_project_response,
};
use prost::Message;
use queries::{
    get_focused_pane_info, get_pane_cwd, get_pane_git_probe, get_pane_info, get_pane_pid,
    get_pane_running_command, get_pane_scrollback, get_tab_info, list_clients, list_notifications,
};
pub(crate) use sessions::resolve_host_directory;
pub(crate) use sessions::session_list_snapshot;
use sessions::{
    adopt_orphaned_pane, current_session_last_saved_time, delete_all_dead_sessions,
    delete_all_dead_sessions_and_reply, delete_dead_session, delete_dead_session_and_reply, detach,
    disconnect_other_clients, dump_session_layout, get_disconnect_targets, get_session_list,
    kill_sessions, kill_sessions_and_reply, list_orphaned_panes, quit_yinz, rename_session,
    save_session, set_quit_notice, switch_session, warp_pane_from_session, warp_pane_to_session,
    warp_tab_from_session, warp_tab_to_session,
};
use std::sync::atomic;
use yinz_bus::global_async_runtime::get_tokio_runtime;
use yinz_bus::screen::ScreenInstruction;
use yinz_plugin_api::plugin_command::{
    ProtobufDeleteAllDeadSessionsResponse, ProtobufDeleteDeadSessionResponse,
    ProtobufKillSessionsResponse,
};
use yinz_runtime::errors::prelude::*;
use yinz_types::data::{CommandType, Event, PermissionStatus, PluginCommand};
use yinz_types::internal_ids::TabNumber;
use yinz_types::own_pane::{own_pane_command, own_pane_refusal};

/// The virtual root a plugin's own plugin folder is mounted at. Present only when the session
/// has one configured or defaulted; a write under it needs `AuthorPlugins` on top of `WriteFiles`.
pub const PLUGINS_FS_ROOT: &str = "/plugins";

/// Run one host command on behalf of a plugin, after checking that the plugin is permitted to.
///
/// This is the complete host capability surface. The runtime decodes a typed command,
/// then this dispatcher checks permissions before invoking its handler.
pub fn run_plugin_command(env: &mut PluginEnv, command: PluginCommand) -> Result<()> {
    let plugin_command = env.name();
    let err_context = || format!("failed to run plugin command {}", plugin_command);
    let permission = check_command_permission(env, &command);
    if permission.0 == PermissionStatus::Granted
        && env.surface == Surface::Headless
        && let Some(name) = own_pane_command(&command, env.plugin_id)
    {
        bail!("{}", own_pane_refusal(name, &env.name()));
    }
    match permission {
        (PermissionStatus::Granted, _) => match command {
            PluginCommand::CommandPalette(mut command) => {
                let generation = env
                    .generation
                    .context("Command palettes require a live plugin instance")?;
                anyhow::ensure!(
                    env.workers
                        .as_ref()
                        .is_none_or(|w| w.parent_generation == generation),
                    "Background workers cannot register palette providers"
                );
                anyhow::ensure!(
                    !env.unavailable.load(atomic::Ordering::Acquire)
                        && !env.cancel.load(atomic::Ordering::Acquire),
                    "Palette plugin is no longer available"
                );
                let owner = yinz_bus::command_palette::Owner {
                    plugin_id: env.plugin_id,
                    generation,
                    client_id: (!env.plugin_config.is_shared()).then_some(env.owner_client_id),
                };
                match &mut command {
                    yinz_types::command_palette::Command::Register {
                        id,
                        title,
                        registration,
                        ..
                    } => {
                        for value in [id, title, registration] {
                            yinz_types::command_palette::label(value).map_err(|e| anyhow!(e))?;
                        }
                        anyhow::ensure!(
                            serde_json::to_vec(&command)
                                .context("encoding palette registration")?
                                .len()
                                <= yinz_types::command_palette::MAX_DISPLAY_BYTES,
                            "Palette registration exceeds 256 KiB"
                        );
                    },
                    yinz_types::command_palette::Command::Reply { result, .. } => {
                        if let Ok(reply) = result
                            && let Err(error) = reply.validate()
                        {
                            env.senders
                                .send_to_plugin(PluginInstruction::PluginPalette(
                                    yinz_bus::command_palette::Delivery {
                                        owner: owner.clone(),
                                        client: env.client_id,
                                        message: yinz_types::command_palette::Message::Release {
                                            handles: reply.handles(),
                                        },
                                        active: None,
                                    },
                                ))
                                .context("releasing invalid palette reply")?;
                            *result = Err(error);
                        }
                        if let Err(error) = result {
                            *error = error
                                .chars()
                                .filter(|c| !c.is_control())
                                .take(2048)
                                .collect();
                        }
                    },
                    _ => {},
                }
                env.senders
                    .send_to_screen(ScreenInstruction::CommandPalette(
                        yinz_bus::command_palette::Instruction::Command {
                            owner,
                            client: env.client_id,
                            unavailable: env.unavailable.clone(),
                            command,
                        },
                    ))
                    .context("sending palette command")?;
            },
            PluginCommand::RegisterProjectProvider(provider) => {
                let supported_source = env.plugin_config.is_builtin()
                    || yinz_plugin_api::authoring::is_script_plugin(
                        env.resolved_source_path
                            .as_deref()
                            .unwrap_or(&env.plugin_config.path),
                    );
                let result = match env.generation.filter(|_| supported_source) {
                    _ if env.unavailable.load(atomic::Ordering::Acquire)
                        || env.cancel.load(atomic::Ordering::Acquire) =>
                    {
                        Err("project provider plugin is no longer available".to_owned())
                    },
                    None => Err("project providers require a Scheme plugin instance".to_owned()),
                    Some(_) if !env.plugin_config.is_shared() => Err(
                        "project provider plugins require shared true in their configuration"
                            .to_owned(),
                    ),
                    Some(generation) => yinz_types::project_provider::register(
                        provider,
                        generation,
                        env.plugin_id.get(),
                    ),
                };
                if result.is_ok() {
                    broadcast_project_providers(&env.senders);
                }
                write_project_response(
                    env,
                    result.map(|()| serde_json::json!(yinz_types::project_provider::providers())),
                )?;
            },
            PluginCommand::ResolveProject { directory } => {
                let result = agent_control::projects(&env.data_access)
                    .and_then(|rows| {
                        crate::project_provider::resolve_directory(
                            &env.plugin_cwd.join(directory),
                            &project_data::projects::from_rows(&rows),
                            crate::project_provider::snapshot(),
                        )
                    })
                    .and_then(|project| {
                        serde_json::to_value(project).map_err(|error| error.to_string())
                    });
                write_project_response(env, result)?;
            },
            PluginCommand::GetProjectProviders => {
                write_project_response(
                    env,
                    Ok(serde_json::json!(yinz_types::project_provider::providers())),
                )?;
            },
            PluginCommand::ValidateProjectMatch(value) => {
                let result = crate::project_provider::service(env)
                    .and_then(|_| crate::project_provider::validate(value));
                write_project_response(env, result)?;
            },
            PluginCommand::PublishProjectLocations(value) => {
                let result = crate::project_provider::service(env)
                    .and_then(|generation| crate::project_provider::publish(generation, value));
                if matches!(result, Ok(true)) {
                    env.senders
                        .send_to_screen(ScreenInstruction::InvalidateAgentEventProjects)
                        .context("invalidate project memberships")?;
                }
                write_project_response(env, result.map(|_| serde_json::Value::Null))?;
            },
            PluginCommand::RegisterAgentProvider(provider) => {
                let supported_source = env.plugin_config.is_builtin()
                    || yinz_plugin_api::authoring::is_script_plugin(
                        env.resolved_source_path
                            .as_deref()
                            .unwrap_or(&env.plugin_config.path),
                    );
                let result = match env.generation.filter(|_| supported_source) {
                    _ if env.unavailable.load(atomic::Ordering::Acquire)
                        || env.cancel.load(atomic::Ordering::Acquire) =>
                    {
                        Err("agent provider plugin is no longer available".to_owned())
                    },
                    None => Err("agent providers require a Scheme plugin instance".to_owned()),
                    Some(_) if !env.plugin_config.is_shared() => Err(
                        "agent provider plugins require shared true in their configuration"
                            .to_owned(),
                    ),
                    Some(generation) => yinz_types::agent::provider::register(generation, provider),
                };
                if result.is_ok() {
                    broadcast_agent_providers(&env.senders);
                }
                write_agent_providers_response(
                    env,
                    result.map(|()| yinz_types::agent::provider::providers()),
                )?;
            },
            PluginCommand::GetAgentProviders => {
                write_agent_providers_response(env, Ok(yinz_types::agent::provider::providers()))?;
            },
            PluginCommand::AgentControl { id, request } => {
                let mut request: yinz_types::agent::control::AgentRequest =
                    serde_json::from_str(&request).context("decode agent request")?;
                if let yinz_types::agent::control::AgentRequest::SpawnNewProject {
                    destination, ..
                } = &mut request
                    && destination.is_relative()
                {
                    *destination = translate_plugin_path(env, destination.clone());
                }
                if let yinz_types::agent::control::AgentRequest::Spawn {
                    cwd,
                    project,
                    remote_host,
                    ..
                } = &mut request
                {
                    if remote_host.is_none()
                        && let Some(path) = cwd.as_mut().filter(|path| path.is_relative())
                    {
                        *path = translate_plugin_path(env, path.clone());
                    }
                    if cwd.is_none() && project.is_none() && remote_host.is_none() {
                        *cwd = Some(env.plugin_cwd.clone());
                    }
                }
                let senders = env.senders.clone();
                let data = env.data_access.clone();
                let target = env.plugin_id;
                let client = env.client_id;
                let generation = env.generation;
                get_tokio_runtime().spawn_blocking(move || {
                    let response = control_agent(request, client, &senders, &data);
                    let mut payload = serde_json::json!({"id":id,"response":response});
                    if let Some(spawned) = &response.spawned {
                        payload["response"]["spawned"]["pane_id"] =
                            spawned.pane_id.to_string().into();
                    }
                    let payload = payload.to_string();
                    senders
                        .send_to_plugin(PluginInstruction::for_plugin_event(
                            target,
                            client,
                            generation,
                            Event::CustomMessage("agent-response".into(), payload),
                        ))
                        .context("deliver agent response")
                        .non_fatal();
                });
            },
            PluginCommand::PaneTabs(mut command) => {
                if let yinz_types::pane_tabs::PaneTabsCommand::Mount { layout, .. } = &mut command {
                    let aliases = env.plugin_aliases.lock().unwrap();
                    for descriptor in &mut layout.tabs {
                        descriptor.populate_plugin_alias(&aliases);
                    }
                }
                let (response, receive) = crossbeam::channel::bounded(1);
                env.senders
                    .send_to_screen(ScreenInstruction::PaneTabs {
                        command,
                        client_id: env.client_id,
                        response,
                    })
                    .context("failed to send pane tabs command")?;
                let result = receive.recv().context("pane tabs response disconnected")?;
                let response = yinz_plugin_api::plugin_command::ProtobufPaneTabsResponse {
                    json: serde_json::to_string(&result)
                        .context("failed to encode pane tabs response")?,
                };
                write_response(env, &response.encode_to_vec())?;
            },
            command @ (PluginCommand::CreateMultipane(_)
            | PluginCommand::UpdateMultipane(_)
            | PluginCommand::ReplaceMultipaneChild(_)) => multipanes::dispatch(env, command)?,
            PluginCommand::SubscribePaneOutput(context) => {
                if context.as_ref().is_some_and(|c| c.is_empty()) {
                    return Err(anyhow!(
                        "Pane output subscriptions require a nonempty context filter"
                    ));
                }
                env.senders
                    .send_to_screen(ScreenInstruction::SubscribePaneOutput(
                        env.plugin_id,
                        env.client_id,
                        context,
                        env.generation,
                    ))?;
            },
            PluginCommand::Subscribe(event_list) => subscribe(env, event_list)?,
            PluginCommand::Unsubscribe(event_list) => unsubscribe(env, event_list)?,
            PluginCommand::SetSelectable(selectable) => set_selectable(env, selectable),
            PluginCommand::ShowCursor(cursor_position) => show_cursor(env, cursor_position),
            PluginCommand::GetCellPixelSize => get_cell_pixel_size(env)?,
            PluginCommand::GetPluginIds => get_plugin_ids(env),
            PluginCommand::GetYinzVersion => get_yinz_version(env),
            PluginCommand::GenerateRandomName => generate_random_name(env),
            PluginCommand::DumpLayout(layout_name) => dump_layout(env, layout_name),
            PluginCommand::ParseLayout(layout_string) => parse_layout(env, layout_string),
            PluginCommand::TestPlugin {
                name,
                options,
                request_id,
            } => {
                crate::testing::start(env, name, options, request_id)?;
            },
            PluginCommand::CancelPluginTest(id) => crate::testing::cancel(env, &id),
            PluginCommand::CheckPlugin { name, source, mode } => {
                check_plugin(env, name, source, mode)
            },
            PluginCommand::SetPaneAgent { pane_id, agent } => set_pane_agent(env, pane_id, agent),
            PluginCommand::SetPaneAgentRun { pane_id, run_id } => {
                set_pane_agent_run(env, pane_id, run_id)
            },
            PluginCommand::SpawnAgent {
                spec,
                source_directory,
                cwd,
                name,
                context,

                start_suppressed,
            } => spawn_agent(
                env,
                spec,
                source_directory,
                cwd,
                name,
                context,
                start_suppressed,
            ),
            PluginCommand::SetQuitNotice { notice, guard } => set_quit_notice(env, notice, guard),
            PluginCommand::Pick(command) => crate::plugin_bridge::picker::dispatch(env, command)?,
            PluginCommand::GetPluginInfo(id) => get_plugin_info(env, id)?,
            PluginCommand::GetPluginIdsForUrl(url) => get_plugin_ids_for_url(env, url, None)?,
            PluginCommand::GetPluginIdsForUrlWithConfiguration { url, configuration } => {
                get_plugin_ids_for_url(env, url, Some(configuration))?
            },
            PluginCommand::ReloadPluginByUrl(url) => reload_plugin_by_url(env, url)?,
            PluginCommand::ReadPluginLog { name, lines } => read_plugin_log(env, name, lines),
            PluginCommand::GetLayoutDir => get_layout_dir(env),
            PluginCommand::GetUserPluginDir => get_user_plugin_dir(env),
            PluginCommand::GetFocusedPaneInfo => get_focused_pane_info(env),
            PluginCommand::SaveSession => save_session(env),
            PluginCommand::CurrentSessionLastSavedTime => current_session_last_saved_time(env),
            PluginCommand::GetPaneInfo(pane_id) => get_pane_info(env, pane_id),
            PluginCommand::GetTabInfo(tab_id) => get_tab_info(env, tab_id),
            PluginCommand::ResolveNotification { id, decision } => {
                env.senders
                    .send_to_screen(ScreenInstruction::ResolveNotification {
                        id,
                        decision,
                        plugin_id: env.plugin_id,
                        instance_id: env.instance_id,
                    })
                    .context("resolve notification")
                    .non_fatal();
            },
            PluginCommand::ListNotifications => list_notifications(env),
            PluginCommand::OpenFile(file_to_open, context) => open_file(env, file_to_open, context),
            PluginCommand::OpenFileFloating(file_to_open, floating_pane_coordinates, context) => {
                open_file_floating(env, file_to_open, floating_pane_coordinates, context)
            },
            PluginCommand::OpenTerminal(cwd) => open_terminal(env, cwd.path),
            PluginCommand::OpenTerminalNearPlugin(cwd) => open_terminal_near_plugin(env, cwd.path),
            PluginCommand::OpenTerminalFloating(cwd, floating_pane_coordinates) => {
                open_terminal_floating(env, cwd.path, floating_pane_coordinates)
            },
            PluginCommand::OpenTerminalFloatingNearPlugin(cwd, floating_pane_coordinates) => {
                open_terminal_floating_near_plugin(env, cwd.path, floating_pane_coordinates)
            },
            PluginCommand::OpenCommandPane(command_to_run, context) => {
                open_command_pane(env, command_to_run, context)
            },
            PluginCommand::OpenCommandPaneNearPlugin(command_to_run, context) => {
                open_command_pane_near_plugin(env, command_to_run, context)
            },
            PluginCommand::OpenCommandPaneInGitCheckoutNearPlugin(
                command_to_run,
                source_directory,
                checkout,
            ) => open_command_pane_in_git_checkout_near_plugin(
                env,
                command_to_run,
                source_directory,
                checkout,
            ),
            PluginCommand::OpenCommandPaneFloating(
                command_to_run,
                floating_pane_coordinates,
                context,
            ) => {
                open_command_pane_floating(env, command_to_run, floating_pane_coordinates, context)
            },
            PluginCommand::OpenCommandPaneOverlay(command_to_run, placement, context) => {
                open_command_pane_overlay(env, command_to_run, placement, context)
            },
            PluginCommand::OpenCommandPaneFloatingNearPlugin(
                command_to_run,
                floating_pane_coordinates,
                context,
            ) => open_command_pane_floating_near_plugin(
                env,
                command_to_run,
                floating_pane_coordinates,
                context,
            ),
            PluginCommand::SwitchTabTo(tab_index) => switch_tab_to(env, tab_index),
            PluginCommand::SetTimeout(seconds) => set_timeout(env, seconds),
            PluginCommand::ExecCmd(command_line) => exec_cmd(env, command_line),
            PluginCommand::RunCommand(
                command_line,
                env_variables,
                cwd,
                context,
                max_output_bytes,
                stream_output,
            ) => run_command(
                env,
                command_line,
                env_variables,
                cwd,
                context,
                max_output_bytes,
                stream_output,
            ),
            PluginCommand::WebRequest(url, verb, headers, body, context, stream_id, proxy) => {
                web_request(env, url, verb, headers, body, context, stream_id, proxy)
            },
            PluginCommand::CancelWebRequest(stream_id) => cancel_web_request(env, stream_id),
            PluginCommand::PostMessageTo(plugin_message) => post_message_to(env, plugin_message)?,
            PluginCommand::PostMessageToPlugin(plugin_message) => {
                post_message_to_plugin(env, plugin_message)?
            },
            PluginCommand::HideSelf => hide_self(env)?,
            PluginCommand::ShowSelf(should_float_if_hidden) => {
                show_self(env, should_float_if_hidden)
            },
            PluginCommand::SwitchToMode(input_mode) => switch_to_mode(env, input_mode),
            PluginCommand::NewTabsWithLayout(raw_layout) => new_tabs_with_layout(env, &raw_layout)?,
            PluginCommand::NewTabsWithLayoutInfo(layout_info) => {
                new_tabs_with_layout_info(env, layout_info)?
            },
            PluginCommand::OverrideLayout(
                layout_info,
                retain_existing_terminal_panes,
                retain_existing_plugin_panes,
                apply_only_to_active_tab,
                context,
            ) => override_layout(
                env,
                layout_info,
                retain_existing_terminal_panes,
                retain_existing_plugin_panes,
                apply_only_to_active_tab,
                context,
            )?,
            PluginCommand::SaveLayout {
                layout_name,
                layout_kdl,
                overwrite,
            } => save_layout(env, layout_name, layout_kdl, overwrite),
            PluginCommand::DeleteLayout { layout_name } => delete_layout(env, layout_name),
            PluginCommand::RenameLayout {
                old_layout_name,
                new_layout_name,
            } => rename_layout(env, old_layout_name, new_layout_name),
            PluginCommand::EditLayout {
                layout_name,
                context,
            } => edit_layout(env, layout_name, context),
            PluginCommand::NewTab { name, cwd } => new_tab(env, name, cwd),
            PluginCommand::NewTabUnfocused { name, cwd } => new_tab_unfocused(env, name, cwd),
            PluginCommand::NewTiledPaneInTab { tab_id } => new_tiled_pane_in_tab(env, tab_id),
            PluginCommand::ToggleFloatingPanes { tab_id } => toggle_floating_panes(env, tab_id),
            PluginCommand::NewPane => new_pane(env),
            PluginCommand::GoToNextTab => go_to_next_tab(env),
            PluginCommand::GoToPreviousTab => go_to_previous_tab(env),
            PluginCommand::Resize(resize_payload) => resize(env, resize_payload),
            PluginCommand::ResizeWithDirection(resize_strategy) => {
                resize_with_direction(env, resize_strategy)
            },
            PluginCommand::FocusNextPane => focus_next_pane(env),
            PluginCommand::FocusPreviousPane => focus_previous_pane(env),
            PluginCommand::FocusLastPane => focus_last_pane(env),
            PluginCommand::MoveFocus(direction) => move_focus(env, direction),
            PluginCommand::MoveFocusOrTab(direction) => move_focus_or_tab(env, direction),
            PluginCommand::Detach => detach(env),
            PluginCommand::EditScrollback => edit_scrollback(env),
            PluginCommand::Write(bytes) => write(env, bytes),
            PluginCommand::WriteChars(chars) => write_chars(env, chars),
            PluginCommand::CopyToClipboard(text) => copy_to_clipboard(env, text)?,
            PluginCommand::ToggleTab => toggle_tab(env),
            PluginCommand::MovePane => move_pane(env),
            PluginCommand::MovePaneWithDirection(direction) => {
                move_pane_with_direction(env, direction)
            },
            PluginCommand::ClearScreen => clear_screen(env),
            PluginCommand::ScrollUp => scroll_up(env),
            PluginCommand::ScrollDown => scroll_down(env),
            PluginCommand::ScrollToTop => scroll_to_top(env),
            PluginCommand::ScrollToBottom => scroll_to_bottom(env),
            PluginCommand::PageScrollUp => page_scroll_up(env),
            PluginCommand::PageScrollDown => page_scroll_down(env),
            PluginCommand::ToggleFocusFullscreen => toggle_focus_fullscreen(env),
            PluginCommand::ToggleFocusNoUiFullscreen => toggle_focus_no_ui_fullscreen(env),
            PluginCommand::FocusHostSession => focus_host_session(env),
            PluginCommand::TogglePaneFrames => toggle_pane_frames(env),
            PluginCommand::SetPaneFrameStyle(pane_frame_style) => {
                set_pane_frame_style(env, pane_frame_style)
            },
            PluginCommand::TogglePaneEmbedOrEject => toggle_pane_embed_or_eject(env),
            PluginCommand::UndoRenamePane => undo_rename_pane(env),
            PluginCommand::CloseFocus => close_focus(env),
            PluginCommand::ToggleActiveTabSync => toggle_active_tab_sync(env),
            PluginCommand::CloseFocusedTab => close_focused_tab(env),
            PluginCommand::UndoRenameTab => undo_rename_tab(env),
            PluginCommand::QuitYinz => quit_yinz(env),
            PluginCommand::PreviousSwapLayout => previous_swap_layout(env),
            PluginCommand::NextSwapLayout => next_swap_layout(env),
            PluginCommand::GoToTabName(tab_name) => go_to_tab_name(env, tab_name),
            PluginCommand::FocusOrCreateTab(tab_name) => focus_or_create_tab(env, tab_name),
            PluginCommand::GoToTab(tab_index) => go_to_tab(
                env,
                TabNumber::try_from(tab_index).map_err(anyhow::Error::msg)?,
            ),
            PluginCommand::StartOrReloadPlugin(plugin_url) => {
                start_or_reload_plugin(env, &plugin_url)?
            },
            PluginCommand::CloseTerminalPane(terminal_pane_id) => {
                close_terminal_pane(env, terminal_pane_id)
            },
            PluginCommand::ClosePluginPane(plugin_pane_id) => {
                close_plugin_pane(env, plugin_pane_id)
            },
            PluginCommand::FocusTerminalPane(
                terminal_pane_id,
                should_float_if_hidden,
                should_be_in_place_if_hidden,
            ) => focus_terminal_pane(
                env,
                terminal_pane_id,
                should_float_if_hidden,
                should_be_in_place_if_hidden,
            ),
            PluginCommand::FocusPluginPane(
                plugin_pane_id,
                should_float_if_hidden,
                should_be_in_place_if_hidden,
            ) => focus_plugin_pane(
                env,
                plugin_pane_id,
                should_float_if_hidden,
                should_be_in_place_if_hidden,
            ),
            PluginCommand::RenameTerminalPane(terminal_pane_id, new_name) => {
                rename_terminal_pane(env, terminal_pane_id, &new_name)
            },
            PluginCommand::RenamePluginPane(plugin_pane_id, new_name) => {
                rename_plugin_pane(env, plugin_pane_id, &new_name)
            },
            PluginCommand::SetPaneTitlePrefix { pane_id, prefix } => {
                set_pane_title_prefix(env, pane_id, prefix)
            },
            PluginCommand::SetPaneSubtitle { pane_id, subtitle } => {
                set_pane_subtitle(env, pane_id, subtitle)
            },
            PluginCommand::RenameTab(tab_index, new_name) => rename_tab(
                env,
                TabNumber::try_from(tab_index).map_err(anyhow::Error::msg)?,
                &new_name,
            ),
            PluginCommand::ReportPanic(crash_payload) => report_panic(env, &crash_payload),
            PluginCommand::RequestPluginPermissions(permissions) => {
                request_permission(env, permissions)?
            },
            PluginCommand::GrantPluginPermissions {
                plugin_url,
                permissions,
            } => grant_plugin_permissions(env, plugin_url, permissions)?,
            PluginCommand::SwitchSession(connect_to_session) => switch_session(
                env,
                connect_to_session.name,
                connect_to_session.tab_position,
                connect_to_session.pane_id,
                connect_to_session.layout,
                connect_to_session.cwd,
            )?,
            PluginCommand::DeleteDeadSession(session_name) => delete_dead_session(&session_name)?,
            PluginCommand::DeleteAllDeadSessions => delete_all_dead_sessions()?,
            PluginCommand::OpenFileInPlace(file_to_open, context) => {
                open_file_in_place(env, file_to_open, context)
            },
            PluginCommand::OpenTerminalInPlace(cwd) => open_terminal_in_place(env, cwd.path),
            PluginCommand::OpenTerminalInPlaceOfPlugin(cwd, close_plugin_after_replace) => {
                open_terminal_in_place_of_plugin(env, cwd.path, close_plugin_after_replace)?
            },
            PluginCommand::OpenCommandPaneInPlace(command_to_run, context) => {
                open_command_pane_in_place(env, command_to_run, context)
            },
            PluginCommand::OpenCommandPaneInPlaceOfPlugin(
                command_to_run,
                close_plugin_after_replace,
                context,
            ) => open_command_pane_in_place_of_plugin(
                env,
                command_to_run,
                close_plugin_after_replace,
                context,
            )?,
            PluginCommand::RenameSession(new_session_name) => rename_session(env, new_session_name),
            PluginCommand::Notify(text, pane_id, click) => notify(env, text, pane_id, click),
            PluginCommand::UnblockCliPipeInput(pipe_name) => unblock_cli_pipe_input(env, pipe_name),
            PluginCommand::BlockCliPipeInput(pipe_name) => block_cli_pipe_input(env, pipe_name),
            PluginCommand::CliPipeOutput(pipe_name, output) => {
                cli_pipe_output(env, pipe_name, output)?
            },
            PluginCommand::MessageToPlugin(message) => message_to_plugin(env, message)?,
            PluginCommand::DisconnectOtherClients => disconnect_other_clients(env),
            PluginCommand::KillSessions(session_list) => kill_sessions(session_list),
            PluginCommand::KillSessionsAndReply(session_list) => {
                kill_sessions_and_reply(env, session_list)
            },
            PluginCommand::DeleteDeadSessionAndReply(session_name) => {
                delete_dead_session_and_reply(env, session_name)
            },
            PluginCommand::DeleteAllDeadSessionsAndReply => delete_all_dead_sessions_and_reply(env),
            PluginCommand::WarpPaneToSession {
                pane_id,
                to_session,
                placement,
            } => warp_pane_to_session(env, pane_id, to_session, placement),
            PluginCommand::WarpPaneFromSession {
                pane_id,
                from_session,
                placement,
            } => warp_pane_from_session(env, pane_id, from_session, placement),
            PluginCommand::ListOrphanedPanes => list_orphaned_panes(env),
            PluginCommand::WarpTabToSession { tab_id, to_session } => {
                warp_tab_to_session(env, tab_id, to_session)
            },
            PluginCommand::WarpTabFromSession {
                tab_id,
                from_session,
            } => warp_tab_from_session(env, tab_id, from_session),
            PluginCommand::AdoptOrphanedPane { host_id, placement } => {
                adopt_orphaned_pane(env, host_id, placement)
            },
            PluginCommand::DataDefineEntity {
                namespace,
                def,
                allow_destructive,
            } => data_define_entity(env, namespace, def, allow_destructive),
            PluginCommand::DataApply {
                namespace,
                mutations,
            } => data_apply(env, namespace, mutations),
            PluginCommand::DataFetch { request } => data_fetch(env, request),
            PluginCommand::DataCount { request } => data_count(env, request),
            PluginCommand::DataWatch { request, mode } => data_watch(env, request, mode),
            PluginCommand::DataCatalog { namespace } => data_catalog(env, namespace),
            PluginCommand::GetLocalUtcOffset { epoch_ms } => get_local_utc_offset(env, epoch_ms),
            PluginCommand::GetLocalTime { epoch_ms } => get_local_time(env, epoch_ms),
            PluginCommand::DataUnwatch { watch_id } => data_unwatch(env, watch_id),
            PluginCommand::ScanHostFolder(folder_to_scan) => scan_host_folder(env, folder_to_scan),
            PluginCommand::InspectConfiguration(theme) => configuration_editor(env, None, theme),
            PluginCommand::EditConfiguration(edit) => configuration_editor(env, Some(edit), None),
            PluginCommand::ProjectConfigRead { directory } => project_config(env, directory, None),
            PluginCommand::ProjectConfigWrite {
                directory,
                source,
                expected,
            } => project_config(env, directory, Some((source, expected))),
            PluginCommand::FsRead {
                path,
                max_bytes,
                offset,
            } => fs_read(env, path, max_bytes, offset),
            PluginCommand::FsList { path } => fs_list(env, path),
            PluginCommand::FsMetadata {
                path,
                follow_symlinks,
            } => fs_metadata(env, path, follow_symlinks),
            PluginCommand::FsWrite {
                path,
                contents,
                append,
                create_dirs,
            } => fs_write(env, path, contents, append, create_dirs),
            PluginCommand::FsRemove { path, recursive } => fs_remove(env, path, recursive),
            PluginCommand::FsCreateDir { path, all } => fs_create_dir(env, path, all),
            PluginCommand::FsRename { from, to } => fs_rename(env, from, to),
            PluginCommand::WatchFilesystem => env.watch_filesystem(),
            PluginCommand::GetDisconnectTargets => get_disconnect_targets(env),
            PluginCommand::GetSessionList => get_session_list(env),
            PluginCommand::GetSessionInventory(id) => {
                crate::session_destinations::inventory(env, id)?
            },
            PluginCommand::SwitchSessionDestination(request) => {
                crate::session_destinations::switch(env, request)?
            },
            PluginCommand::DumpSessionLayout { tab_index } => dump_session_layout(env, tab_index),
            PluginCommand::CloseSelf => close_self(env),
            PluginCommand::Reconfigure(new_config, write_config_to_disk) => {
                reconfigure(env, new_config, write_config_to_disk)?
            },
            PluginCommand::HidePaneWithId(pane_id) => hide_pane_with_id(env, pane_id)?,
            PluginCommand::ShowPaneWithId(pane_id, should_float_if_hidden, should_focus_pane) => {
                show_pane_with_id(env, pane_id, should_float_if_hidden, should_focus_pane)
            },
            PluginCommand::OpenCommandPaneBackground(command_to_run, context) => {
                open_command_pane_background(env, command_to_run, context)
            },
            PluginCommand::RerunCommandPane(terminal_pane_id) => {
                rerun_command_pane(env, terminal_pane_id)
            },
            PluginCommand::RestartAgentPane {
                pane_id,
                prompt,
                session,
                context,
            } => {
                let (response, receive) = std::sync::mpsc::channel();
                env.senders
                    .send_to_screen(ScreenInstruction::RestartAgentPane {
                        pane_id,
                        prompt,
                        session,
                        originating_plugin: yinz_types::data::OriginatingPlugin::new(
                            env.plugin_id,
                            env.client_id,
                            context,
                        ),
                        response,
                    })
                    .context("route agent restart")?;
                let result = receive
                    .recv_timeout(std::time::Duration::from_secs(30))
                    .context("agent restart outcome unknown; inspect the pane before retrying")?;
                let response = yinz_plugin_api::plugin_command::ProtobufRestartAgentPaneResponse {
                    json: serde_json::to_string(&result)
                        .context("encode agent restart response")?,
                };
                write_response(env, &response.encode_to_vec())?;
            },
            PluginCommand::RunCommandInPane {
                pane_id,
                command,
                context,
                keep_scrollback,
            } => run_command_in_pane(env, pane_id, command, context, keep_scrollback),
            PluginCommand::ResizePaneIdWithDirection(resize, pane_id) => {
                resize_pane_with_id(env, resize, pane_id)
            },
            PluginCommand::EditScrollbackForPaneWithId(pane_id) => {
                edit_scrollback_for_pane_with_id(env, pane_id)
            },
            PluginCommand::GetPaneScrollback {
                pane_id,
                get_full_scrollback,
            } => get_pane_scrollback(env, pane_id, get_full_scrollback),
            PluginCommand::WriteToPaneId(bytes, pane_id) => write_to_pane_id(env, bytes, pane_id),
            PluginCommand::WriteCharsToPaneId(chars, pane_id) => {
                write_chars_to_pane_id(env, chars, pane_id)
            },
            PluginCommand::SendSigintToPaneId(pane_id) => send_sigint_to_pane_id(env, pane_id),
            PluginCommand::SendSigkillToPaneId(pane_id) => send_sigkill_to_pane_id(env, pane_id),
            PluginCommand::GetPanePid { pane_id } => get_pane_pid(env, pane_id),
            PluginCommand::GetPaneRunningCommand { pane_id } => {
                get_pane_running_command(env, pane_id)
            },
            PluginCommand::GetPaneCwd { pane_id } => get_pane_cwd(env, pane_id),
            PluginCommand::GetPaneGitProbe {
                pane_id,
                cwd,
                refresh_token,
            } => get_pane_git_probe(env, pane_id, cwd, refresh_token),
            PluginCommand::MovePaneWithPaneId(pane_id) => move_pane_with_pane_id(env, pane_id),
            PluginCommand::MovePaneWithPaneIdInDirection(pane_id, direction) => {
                move_pane_with_pane_id_in_direction(env, pane_id, direction)
            },
            PluginCommand::ClearScreenForPaneId(pane_id) => clear_screen_for_pane_id(env, pane_id),
            PluginCommand::ScrollUpInPaneId(pane_id) => scroll_up_in_pane_id(env, pane_id),
            PluginCommand::ScrollDownInPaneId(pane_id) => scroll_down_in_pane_id(env, pane_id),
            PluginCommand::ScrollToTopInPaneId(pane_id) => scroll_to_top_in_pane_id(env, pane_id),
            PluginCommand::ScrollToBottomInPaneId(pane_id) => {
                scroll_to_bottom_in_pane_id(env, pane_id)
            },
            PluginCommand::PageScrollUpInPaneId(pane_id) => page_scroll_up_in_pane_id(env, pane_id),
            PluginCommand::PageScrollDownInPaneId(pane_id) => {
                page_scroll_down_in_pane_id(env, pane_id)
            },
            PluginCommand::TogglePaneIdFullscreen(pane_id) => {
                toggle_pane_id_fullscreen(env, pane_id)
            },
            PluginCommand::TogglePaneEmbedOrEjectForPaneId(pane_id) => {
                toggle_pane_embed_or_eject_for_pane_id(env, pane_id)
            },
            PluginCommand::CloseTabWithIndex(tab_index) => close_tab_with_index(env, tab_index),
            PluginCommand::BreakPanesToNewTab(
                pane_ids,
                new_tab_name,
                should_change_focus_to_new_tab,
            ) => break_panes_to_new_tab(
                env,
                pane_ids.into_iter().collect(),
                new_tab_name,
                should_change_focus_to_new_tab,
            ),
            PluginCommand::BreakPanesToTabWithIndex(
                pane_ids,
                should_change_focus_to_new_tab,
                tab_index,
            ) => break_panes_to_tab_with_index(
                env,
                pane_ids.into_iter().collect(),
                tab_index,
                should_change_focus_to_new_tab,
            ),
            PluginCommand::SwitchTabToId(tab_id) => switch_tab_to_id(env, tab_id),
            PluginCommand::GoToTabWithId(tab_id) => go_to_tab_with_id(env, tab_id),
            PluginCommand::CloseTabWithId(tab_id) => close_tab_with_id(env, tab_id),
            PluginCommand::RenameTabWithId(tab_id, new_name) => {
                rename_tab_with_id(env, tab_id, &new_name)
            },
            PluginCommand::ArrangeProjectPanes(groups) => {
                env.senders
                    .send_to_screen(ScreenInstruction::ArrangeProjectPanes(
                        groups,
                        env.client_id,
                    ))
                    .non_fatal();
            },
            PluginCommand::SetProjectContext(projects, panes, sources) => {
                env.senders
                    .send_to_screen(ScreenInstruction::SetProjectContext(
                        projects, panes, sources,
                    ))
                    .context("failed to set project context")
                    .non_fatal();
            },
            PluginCommand::SetTabProjects(tab_projects) => set_tab_projects(env, tab_projects),
            PluginCommand::BreakPanesToTabWithId(pane_ids, tab_id, should_change_focus) => {
                break_panes_to_tab_with_id(
                    env,
                    pane_ids.into_iter().collect(),
                    should_change_focus,
                    tab_id,
                )
            },
            PluginCommand::ReloadPlugin(plugin_id) => reload_plugin(env, plugin_id),
            PluginCommand::LoadNewPlugin {
                url,
                config,
                load_in_background,
                skip_plugin_cache,
                cwd,
            } => load_new_plugin(env, url, config, load_in_background, skip_plugin_cache, cwd),
            PluginCommand::RebindKeys {
                keys_to_rebind,
                keys_to_unbind,
                write_config_to_disk,
            } => rebind_keys(env, keys_to_rebind, keys_to_unbind, write_config_to_disk)?,
            PluginCommand::RegisterDefaultKeybindings(keybindings) => {
                register_default_keybindings(env, keybindings)?
            },
            PluginCommand::ListClients => list_clients(env),
            PluginCommand::ChangeHostFolder(new_host_folder) => {
                change_host_folder(env, new_host_folder)
            },
            PluginCommand::SetFloatingPanePinned(pane_id, should_be_pinned) => {
                set_floating_pane_pinned(env, pane_id, should_be_pinned)
            },
            PluginCommand::StackPanes(pane_ids) => stack_panes(env, pane_ids.into_iter().collect()),
            PluginCommand::ChangeFloatingPanesCoordinates(pane_ids_and_coordinates) => {
                change_floating_panes_coordinates(
                    env,
                    pane_ids_and_coordinates.into_iter().collect(),
                )
            },
            PluginCommand::TogglePaneBorderless(pane_id) => toggle_pane_borderless(env, pane_id),
            PluginCommand::SetPaneBorderless(pane_id, borderless) => {
                set_pane_borderless(env, pane_id, borderless)
            },
            PluginCommand::SetPaneColor(pane_id, fg, bg) => set_pane_color(env, pane_id, fg, bg),
            PluginCommand::OpenFileNearPlugin(file_to_open, context) => {
                open_file_near_plugin(env, file_to_open, context)
            },
            PluginCommand::OpenFileFloatingNearPlugin(
                file_to_open,
                floating_pane_coordinates,
                context,
            ) => open_file_floating_near_plugin(
                env,
                file_to_open,
                floating_pane_coordinates,
                context,
            ),
            PluginCommand::OpenFileInPlaceOfPlugin(
                file_to_open,
                close_plugin_after_replace,
                context,
            ) => open_file_in_place_of_plugin(
                env,
                file_to_open,
                close_plugin_after_replace,
                context,
            )?,
            PluginCommand::GroupAndUngroupPanes(
                panes_to_group,
                panes_to_ungroup,
                for_all_clients,
            ) => group_and_ungroup_panes(
                env,
                panes_to_group.into_iter().collect(),
                panes_to_ungroup.into_iter().collect(),
                for_all_clients,
            ),
            PluginCommand::HighlightAndUnhighlightPanes(
                panes_to_highlight,
                panes_to_unhighlight,
            ) => highlight_and_unhighlight_panes(
                env,
                panes_to_highlight.into_iter().collect(),
                panes_to_unhighlight.into_iter().collect(),
            ),
            PluginCommand::RevealPaneWithId(pane_id, label) => {
                reveal_pane_with_id(env, pane_id, label)
            },
            PluginCommand::CloseMultiplePanes(pane_ids) => {
                close_multiple_panes(env, pane_ids.into_iter().collect())
            },
            PluginCommand::FloatMultiplePanes(pane_ids) => {
                float_multiple_panes(env, pane_ids.into_iter().collect())
            },
            PluginCommand::EmbedMultiplePanes(pane_ids) => {
                embed_multiple_panes(env, pane_ids.into_iter().collect())
            },
            PluginCommand::SetSelfMouseSelectionSupport(selection_support) => {
                set_self_mouse_selection_support(env, selection_support);
            },
            PluginCommand::InterceptKeyPresses => intercept_key_presses(env),
            PluginCommand::ClearKeyPressesIntercepts => clear_key_presses_intercepts(env),
            PluginCommand::ReplacePaneWithExistingPane(
                pane_id_to_replace,
                existing_pane_id,
                suppress_replaced_pane,
            ) => replace_pane_with_existing_pane(
                env,
                pane_id_to_replace,
                existing_pane_id,
                suppress_replaced_pane,
            )?,
            PluginCommand::RunAction(action, context) => run_action(env, action, context),
            PluginCommand::GetSessionEnvironmentVariables => {
                get_session_environment_variables(env);
            },
            PluginCommand::GetPluginAliasConfiguration(alias) => {
                get_plugin_alias_configuration(env, alias);
            },
            PluginCommand::OpenCommandPaneInNewTab(command_to_run, context) => {
                open_command_pane_in_new_tab(env, command_to_run, context);
            },
            PluginCommand::OpenPluginPaneInNewTab {
                plugin_url,
                configuration,
                context,
            } => {
                open_plugin_pane_in_new_tab(env, plugin_url, configuration, context);
            },
            PluginCommand::OpenPluginPaneFloating {
                plugin_url,
                configuration,
                floating_pane_coordinates,
                context,
            } => {
                open_plugin_pane_floating(
                    env,
                    plugin_url,
                    configuration,
                    floating_pane_coordinates,
                    context,
                );
            },
            PluginCommand::OpenEditorPaneInNewTab(file_to_open, context) => {
                open_editor_pane_in_new_tab(env, file_to_open, context);
            },
            PluginCommand::OpenCommandPaneInPlaceOfPaneId(
                pane_id,
                command_to_run,
                close_replaced_pane,
                context,
            ) => open_command_pane_in_place_of_pane_id(
                env,
                pane_id,
                command_to_run,
                close_replaced_pane,
                context,
            )?,
            PluginCommand::OpenTerminalPaneInPlaceOfPaneId(pane_id, cwd, close_replaced_pane) => {
                open_terminal_pane_in_place_of_pane_id(env, pane_id, cwd, close_replaced_pane)?
            },
            PluginCommand::OpenEditPaneInPlaceOfPaneId(
                pane_id,
                file_to_open,
                close_replaced_pane,
                context,
            ) => open_edit_pane_in_place_of_pane_id(
                env,
                pane_id,
                file_to_open,
                close_replaced_pane,
                context,
            )?,
            PluginCommand::OpenPluginPaneInPlaceOfPaneId {
                pane_id,
                plugin_url,
                configuration,
                context,
            } => open_plugin_pane_in_place_of_pane_id(
                env,
                pane_id,
                plugin_url,
                configuration,
                context,
            )?,
            PluginCommand::ShowFloatingPanes { tab_id } => show_floating_panes(env, tab_id),
            PluginCommand::HideFloatingPanes { tab_id } => hide_floating_panes(env, tab_id),
            PluginCommand::SetPaneRegexHighlights(pane_id, highlights) => {
                set_pane_regex_highlights(env, pane_id, highlights)
            },
            PluginCommand::SetContextMenu { request_id, items } => {
                let _ = env
                    .senders
                    .send_to_screen(ScreenInstruction::SetContextMenu {
                        request_id,
                        items,
                        // So a selection out of this menu comes back to us and to nobody else.
                        plugin_id: Some(env.plugin_id),
                        client_id: env.client_id,
                    });
            },
            PluginCommand::ClearPaneHighlights(pane_id) => clear_pane_highlights(env, pane_id),
            PluginCommand::ChangeOverlayCoordinates {
                placements,
                override_user_placement,
            } => change_overlay_coordinates(
                env,
                placements.into_iter().collect(),
                override_user_placement,
            ),
        },
        (PermissionStatus::Denied, permission) => {
            if matches!(
                command,
                PluginCommand::RegisterProjectProvider(_)
                    | PluginCommand::GetProjectProviders
                    | PluginCommand::ResolveProject { .. }
                    | PluginCommand::ValidateProjectMatch(_)
                    | PluginCommand::PublishProjectLocations(_)
            ) {
                return write_project_response(
                    env,
                    Err(format!("Permission denied: {permission:?}")),
                );
            }

            if matches!(
                command,
                PluginCommand::RegisterAgentProvider(_) | PluginCommand::GetAgentProviders
            ) {
                return write_agent_providers_response(
                    env,
                    Err(format!("Permission denied: {permission:?}")),
                );
            }
            if matches!(command, PluginCommand::RestartAgentPane { .. }) {
                let result: Result<yinz_types::agent::control::RestartedAgent, String> =
                    Err(format!("Permission denied: {permission:?}"));
                let response = yinz_plugin_api::plugin_command::ProtobufRestartAgentPaneResponse {
                    json: serde_json::to_string(&result).context("encode agent restart denial")?,
                };
                return write_response(env, &response.encode_to_vec());
            }
            if matches!(command, PluginCommand::PaneTabs(_)) {
                let result: yinz_types::pane_tabs::PaneTabsResponse =
                    Err(format!("Permission denied: {permission:?}"));
                let response = yinz_plugin_api::plugin_command::ProtobufPaneTabsResponse {
                    json: serde_json::to_string(&result)
                        .context("failed to encode pane tabs denial")?,
                };
                return write_response(env, &response.encode_to_vec());
            }
            if matches!(
                command,
                PluginCommand::CreateMultipane(_)
                    | PluginCommand::UpdateMultipane(_)
                    | PluginCommand::ReplaceMultipaneChild(_)
            ) {
                let result: yinz_types::multipane::MultipaneResponse =
                    Err(format!("Permission denied: {permission:?}"));
                let response = yinz_plugin_api::plugin_command::ProtobufMultipaneResponse {
                    json: serde_json::to_string(&result)?,
                };
                return write_response(env, &response.encode_to_vec());
            }
            let kind = CommandType::from(&command);
            log::error!(
                "Plugin '{}' permission '{}' denied - Command '{:?}' denied",
                env.name(),
                permission
                    .map(|p| p.to_string())
                    .unwrap_or("UNKNOWN".to_owned()),
                kind
            );
            // Session mutation replies use a missing error to mean success, so denial must
            // carry an error. Other reply-bearing commands retain their empty protobuf reply
            // for compatibility with guests that unwrap decoding it.
            if crate::response::decode_response(kind, &[]).is_some() {
                let response = match kind {
                    CommandType::GetPluginIdsForUrl
                    | CommandType::GetPluginIdsForUrlWithConfiguration => {
                        yinz_plugin_api::plugin_command::ProtobufGetPluginIdsForUrlResponse {
                            plugin_ids: vec![],
                            error: Some("Permission denied: ReadApplicationState".to_owned()),
                        }
                        .encode_to_vec()
                    },
                    CommandType::GetDisconnectTargets => {
                        yinz_plugin_api::plugin_command::ProtobufGetDisconnectTargetsResponse {
                            client_ids: vec![],
                            error: Some("Permission denied: ReadApplicationState".to_owned()),
                        }
                        .encode_to_vec()
                    },
                    CommandType::KillSessionsAndReply => ProtobufKillSessionsResponse {
                        error: Some("Permission denied: ChangeApplicationState".to_owned()),
                    }
                    .encode_to_vec(),
                    CommandType::DeleteDeadSessionAndReply => ProtobufDeleteDeadSessionResponse {
                        error: Some("Permission denied: ChangeApplicationState".to_owned()),
                    }
                    .encode_to_vec(),
                    CommandType::DeleteAllDeadSessionsAndReply => {
                        ProtobufDeleteAllDeadSessionsResponse {
                            error: Some("Permission denied: ChangeApplicationState".to_owned()),
                        }
                        .encode_to_vec()
                    },
                    _ => Vec::new(),
                };
                write_response(env, &response).with_context(err_context)?;
            }
        },
    };
    Ok(())
}

// Helper Functions ---------------------------------------------------------------------------------------------------

pub fn write_response(plugin_env: &mut PluginEnv, bytes: &[u8]) -> Result<()> {
    plugin_env.response_sink = Some(bytes.to_vec());
    Ok(())
}
