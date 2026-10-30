//! Plugin lifecycle, messaging, metadata, and configuration commands.

use super::permissions::has_permission;
use super::write_response;
use crate::plugin_bridge::handle_plugin_crash;
use crate::plugin_map::PluginEnv;
use crate::plugin_panes::Surface;
use crate::{PaneId, PluginInstruction};
use prost::Message;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process;
use std::time::Duration;
use yinz_bus::screen::ScreenInstruction;
use yinz_bus::thread_bus::ThreadSenders;
use yinz_bus::ServerInstruction;
use yinz_plugin_api::plugin_command::{
    fs_read_response, read_plugin_log_response, ProtobufFsReadResponse,
    ProtobufGenerateRandomNameResponse, ProtobufGetLocalTimeResponse,
    ProtobufGetLocalUtcOffsetResponse, ProtobufGetPluginAliasConfigurationResponse,
    ProtobufGetSessionEnvironmentVariablesResponse, ProtobufGetUserPluginDirResponse,
    ProtobufReadPluginLogResponse,
};
use yinz_plugin_api::plugin_ids::{ProtobufPluginIds, ProtobufYinzVersion};
use yinz_runtime::consts::{VERSION, YINZ_SOCK_DIR};
use yinz_runtime::errors::prelude::*;
use yinz_runtime::home::{absolute_lexical, default_user_plugin_dir};
use yinz_session::sessions::generate_random_name as generate_random_name_impl;
use yinz_types::data::{
    Event, EventType, InputMode, MessageToPlugin, PermissionType, PluginIds, PluginMessage,
};
use yinz_types::input::actions::Action;
use yinz_types::input::keybinds::Keybinds;
use yinz_types::input::options::Options;
use yinz_types::internal_ids::{PluginId, ProcessId};
use yinz_types::keys::KeyWithModifier;
use yinz_types::units::SignedUnixMillis;

pub(super) fn write_project_response(
    env: &mut PluginEnv,
    result: Result<serde_json::Value, String>,
) -> Result<()> {
    let response = yinz_plugin_api::plugin_command::ProtobufAgentProvidersResponse {
        json: serde_json::to_string(&result).context("serialize project provider response")?,
    };
    write_response(env, &response.encode_to_vec())
}

pub(crate) fn broadcast_project_providers(senders: &ThreadSenders) {
    crate::project_provider::invalidate();
    senders
        .send_to_screen(ScreenInstruction::InvalidateAgentEventProjects)
        .context("invalidate project memberships")
        .non_fatal();
    senders
        .send_to_plugin(PluginInstruction::Update(vec![(
            None,
            None,
            Event::ProjectProvidersChanged(yinz_types::project_provider::providers()),
        )]))
        .context("broadcast project providers")
        .non_fatal();
}

pub(super) fn write_agent_providers_response(
    env: &mut PluginEnv,
    result: Result<Vec<yinz_types::agent::provider::AgentProvider>, String>,
) -> Result<()> {
    let response = yinz_plugin_api::plugin_command::ProtobufAgentProvidersResponse {
        json: serde_json::to_string(&result).context("serialize agent provider response")?,
    };
    write_response(env, &response.encode_to_vec())
}

pub(crate) fn broadcast_agent_providers(senders: &ThreadSenders) {
    senders
        .send_to_plugin(PluginInstruction::Update(vec![(
            None,
            None,
            Event::AgentProvidersChanged(yinz_types::agent::provider::providers()),
        )]))
        .context("broadcast agent providers")
        .non_fatal();
}

pub(super) fn subscribe(env: &PluginEnv, event_list: HashSet<EventType>) -> Result<()> {
    let background_subscriptions = {
        let mut subscriptions = env.subscriptions.lock().to_anyhow()?;
        subscriptions.extend(event_list.clone());
        env.routes_events_by_registry()
            .then(|| subscriptions.clone())
    };
    env.senders
        .send_to_plugin(PluginInstruction::PluginSubscribedToEvents(
            env.plugin_id,
            env.client_id,
            event_list,
            // The plugin thread must not lock this guest to inspect its metadata:
            // the guest may already be waiting for a synchronous reply from it.
            background_subscriptions,
            env.instance_id,
        ))
}

pub(super) fn unblock_cli_pipe_input(env: &PluginEnv, pipe_name: String) {
    env.input_pipes_to_unblock.lock().unwrap().insert(pipe_name);
}

pub(super) fn block_cli_pipe_input(env: &PluginEnv, pipe_name: String) {
    env.input_pipes_to_block.lock().unwrap().insert(pipe_name);
}

pub(super) fn cli_pipe_output(env: &PluginEnv, pipe_name: String, output: String) -> Result<()> {
    env.senders
        .send_to_server(ServerInstruction::CliPipeOutput(pipe_name, output))
        .context("failed to send pipe output")
}

pub(super) fn message_to_plugin(
    env: &PluginEnv,
    mut message_to_plugin: MessageToPlugin,
) -> Result<()> {
    if message_to_plugin.plugin_url.as_deref() == Some("yinz:OWN_URL") {
        message_to_plugin.plugin_url = Some(env.plugin_config.location.display());
    }
    if !message_to_plugin.has_cwd() {
        message_to_plugin =
            message_to_plugin.new_plugin_instance_should_have_cwd(env.plugin_cwd.clone());
    }
    if !message_to_plugin.plugin_config.contains_key("caller_cwd") {
        message_to_plugin.plugin_config.insert(
            "caller_cwd".to_owned(),
            env.plugin_cwd.display().to_string(),
        );
    }
    env.senders
        .send_to_plugin(PluginInstruction::MessageFromPlugin {
            source_plugin_id: env.plugin_id,
            message: message_to_plugin,
        })
        .context("failed to send message to plugin")
}

pub(super) fn unsubscribe(env: &PluginEnv, event_list: HashSet<EventType>) -> Result<()> {
    let background_subscriptions = {
        let mut subscriptions = env.subscriptions.lock().to_anyhow()?;
        subscriptions.retain(|k| !event_list.contains(k));
        env.routes_events_by_registry()
            .then(|| subscriptions.clone())
    };
    if background_subscriptions.is_some() {
        env.senders
            .send_to_plugin(PluginInstruction::PluginSubscribedToEvents(
                env.plugin_id,
                env.client_id,
                HashSet::new(),
                background_subscriptions,
                env.instance_id,
            ))?;
    }
    Ok(())
}

pub(super) fn set_selectable(env: &PluginEnv, selectable: bool) {
    env.senders
        .send_to_screen(ScreenInstruction::SetSelectable(
            PaneId::Plugin(env.plugin_id),
            selectable,
        ))
        .with_context(|| {
            format!(
                "failed to set plugin {} selectable from plugin {}",
                selectable,
                env.name()
            )
        })
        .non_fatal();
}

pub(super) fn show_cursor(env: &PluginEnv, cursor_position: Option<(usize, usize)>) {
    env.senders
        .send_to_screen(ScreenInstruction::ShowPluginCursor(
            env.plugin_id,
            env.client_id,
            cursor_position,
        ))
        .with_context(|| {
            format!(
                "failed to {} plugin cursor from plugin {}",
                if cursor_position.is_some() {
                    "show"
                } else {
                    "hide"
                },
                env.name()
            )
        })
        .non_fatal();
}

pub(super) fn get_cell_pixel_size(env: &mut PluginEnv) -> Result<()> {
    let (response_channel, receiver) = crossbeam::channel::bounded(1);
    env.senders
        .send_to_screen(ScreenInstruction::GetCellPixelSize {
            client_id: env.client_id,
            response_channel,
        })
        .context("request acting client's cell pixel size")?;
    let size = receiver
        .recv_timeout(Duration::from_secs(5))
        .context("wait for acting client's cell pixel size")?;
    let response = yinz_plugin_api::plugin_command::ProtobufGetCellPixelSizeResponse {
        size: size
            .map(TryInto::try_into)
            .transpose()
            .map_err(|error| anyhow!("{error}"))?,
    };
    write_response(env, &response.encode_to_vec()).context("write cell pixel size response")
}

pub(super) fn get_plugin_ids(env: &mut PluginEnv) {
    let ids = PluginIds {
        plugin_id: env.plugin_id,
        yinz_pid: ProcessId::new(process::id()),
        initial_cwd: env.plugin_cwd.clone(),
        client_id: env.client_id,
        hostname: yinz_runtime::host_identity::host_display_name(),
        host_id: yinz_runtime::host_identity::host_id(),
        max_session_name_bytes: Some(max_session_name_bytes(&YINZ_SOCK_DIR)),
    };
    ProtobufPluginIds::try_from(ids)
        .map_err(|e| anyhow!("Failed to serialized plugin ids: {}", e))
        .and_then(|serialized| {
            write_response(env, &serialized.encode_to_vec())?;
            Ok(())
        })
        .with_context(|| {
            format!(
                "failed to query plugin IDs from host for plugin {}",
                env.name()
            )
        })
        .non_fatal();
}

pub(super) fn max_session_name_bytes(socket_dir: &Path) -> usize {
    // Joining a one-byte name accounts for both the separator and the terminating NUL.
    yinz_runtime::consts::YINZ_SOCK_MAX_LENGTH
        .saturating_sub(socket_dir.join("x").as_os_str().len())
}

pub(super) fn get_yinz_version(env: &mut PluginEnv) {
    let protobuf_yinz_version = ProtobufYinzVersion {
        version: VERSION.to_owned(),
    };
    write_response(env, &protobuf_yinz_version.encode_to_vec())
        .with_context(|| {
            format!(
                "failed to request yinz version from host for plugin {}",
                env.name()
            )
        })
        .non_fatal();
}

pub(super) fn generate_random_name(env: &mut PluginEnv) {
    let name = generate_random_name_impl();
    let response = ProtobufGenerateRandomNameResponse { name };
    write_response(env, &response.encode_to_vec())
        .with_context(|| {
            format!(
                "failed to send generated random name to plugin {}",
                env.name()
            )
        })
        .non_fatal();
}

/// Answer with the folder holding the user's own plugins.
///
/// An empty string means the folder could not be resolved at all, which is the same convention
/// `get_layout_dir` uses - a plugin that cannot be told where to write says so rather than
/// guessing at a path.
pub(super) fn get_user_plugin_dir(env: &mut PluginEnv) {
    // Absolutized here as well as in `home.rs`, because this is the one place the folder crosses
    // into a plugin: a plugin has no cwd to join a relative path onto, so whatever route the
    // value took to `PluginEnv` it must arrive absolute or not at all.
    let user_plugin_dir = env
        .user_plugin_dir
        .clone()
        .or_else(default_user_plugin_dir)
        .map(|dir| absolute_lexical(&dir));

    let user_plugin_dir_string = user_plugin_dir
        .and_then(|path| path.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| String::from(""));

    let response = ProtobufGetUserPluginDirResponse {
        user_plugin_dir: user_plugin_dir_string,
    };

    write_response(env, &response.encode_to_vec())
        .with_context(|| format!("failed to send user plugin dir to plugin {}", env.name()))
        .non_fatal();
}

pub(super) fn get_session_environment_variables(env: &mut PluginEnv) {
    use yinz_plugin_api::plugin_command::EnvVariable as ProtobufEnvVariable;

    let mut session_env_vars = env.session_env_vars.clone();
    session_env_vars.extend(yinz_runtime::envs::child_session_env(
        &env.server_session.current(),
    ));
    let env_vars: Vec<ProtobufEnvVariable> = session_env_vars
        .iter()
        .map(|(name, value)| ProtobufEnvVariable {
            name: name.clone(),
            value: value.clone(),
        })
        .collect();

    let response = ProtobufGetSessionEnvironmentVariablesResponse { env_vars };

    write_response(env, &response.encode_to_vec())
        .with_context(|| {
            format!(
                "failed to send session environment variables to plugin {}",
                env.name()
            )
        })
        .non_fatal();
}

/// Answer with the KDL configuration declared for one plugin alias.
///
/// An alias nobody declared answers with an empty map rather than an error: a plugin has
/// nothing different to do with "no such alias" than with "that alias configures nothing",
/// and this command must always write *something* back — the caller is blocked on a read.
pub(super) fn get_plugin_alias_configuration(env: &mut PluginEnv, alias: String) {
    let configuration = env
        .plugin_aliases
        .lock()
        .unwrap()
        .aliases
        .get(&alias)
        .map(|run_plugin| run_plugin.configuration.inner().clone())
        .unwrap_or_default();

    let response = ProtobufGetPluginAliasConfigurationResponse {
        configuration: configuration.into_iter().collect(),
    };

    write_response(env, &response.encode_to_vec())
        .with_context(|| {
            format!(
                "failed to send the configuration of plugin alias {alias} to plugin {}",
                env.name()
            )
        })
        .non_fatal();
}

pub(super) fn check_plugin(
    env: &mut PluginEnv,
    name: String,
    source: String,
    mode: yinz_types::authoring::PluginCheckMode,
) {
    use yinz_plugin_api::plugin_command::{check_plugin_response, ProtobufCheckPluginResponse};
    let result = (|| -> Result<String> {
        let dir = env
            .user_plugin_dir
            .clone()
            .or_else(default_user_plugin_dir)
            .context("no plugin folder is configured; set user_plugin_dir")?;
        let path =
            yinz_plugin_api::authoring::resolve_plugin_file(&dir, &name).map_err(|e| anyhow!(e))?;
        // Semantic checks may read imports. Refuse symlinks in either authoring component.
        if path.is_symlink() || path.parent().is_some_and(std::path::Path::is_symlink) {
            return Err(anyhow!(
                "plugin file or folder is a symlink; refusing to check"
            ));
        }
        let report = crate::plugin_authoring::check_source(&path, &source, mode);
        serde_json::to_string(&report).context("serialize plugin check report")
    })();
    let response = ProtobufCheckPluginResponse {
        result: Some(match result {
            Ok(report) => check_plugin_response::Result::ReportJson(report),
            Err(error) => check_plugin_response::Result::Error(format!("{error:#}")),
        }),
    };
    write_response(env, &response.encode_to_vec())
        .context("send plugin check report")
        .non_fatal();
}

/// Seconds east of UTC here, at the instant `epoch_ms`.
///
/// The guest has no ambient environment or timezone database. The host supplies the
/// session's local offset for formatting absolute timestamps.
///
/// The offset is taken *at that instant* rather than now, so a timestamp from before a
/// daylight-saving change reads back with the offset that was in force when it was taken. An
/// instant `chrono` cannot place - out of range, or an hour that a clock change made ambiguous
/// or skipped - falls back to the offset in force now, which is off by an hour at worst and is
/// still the right answer for every timestamp taken today.
pub(super) fn get_local_utc_offset(env: &mut PluginEnv, epoch_ms: SignedUnixMillis) {
    use chrono::{Local, Offset, TimeZone};
    let offset_seconds = Local
        .timestamp_millis_opt(epoch_ms.get())
        .single()
        .map(|at| at.offset().fix().local_minus_utc())
        .unwrap_or_else(|| Local::now().offset().fix().local_minus_utc());
    let response = ProtobufGetLocalUtcOffsetResponse { offset_seconds };
    write_response(env, &response.encode_to_vec())
        .with_context(|| {
            format!(
                "failed to send utc offset response to plugin {}",
                env.name()
            )
        })
        .non_fatal();
}

pub(super) fn plugin_log_lines(
    env: &PluginEnv,
    name: &str,
    lines: usize,
    log_file: &Path,
) -> Result<String> {
    let dir = env.user_plugin_dir.clone().or_else(default_user_plugin_dir);
    crate::plugin_authoring::resolve_log_target(
        name,
        dir.as_deref(),
        &env.plugin_aliases.lock().unwrap(),
        &env.plugin_cwd,
    )
    .and_then(|target| crate::plugin_authoring::log_target_tail(log_file, &target, lines))
}

/// Read a name, configured alias, or explicit source path using the caller's cwd.
pub(super) fn read_plugin_log(env: &mut PluginEnv, name: String, lines: usize) {
    let result = plugin_log_lines(env, &name, lines, &yinz_runtime::consts::YINZ_TMP_LOG_FILE);
    let response = ProtobufReadPluginLogResponse {
        result: Some(match result {
            Ok(lines) => read_plugin_log_response::Result::Lines(lines),
            Err(e) => read_plugin_log_response::Result::Error(format!("{:#}", e)),
        }),
    };
    write_response(env, &response.encode_to_vec())
        .with_context(|| format!("failed to send log lines to plugin {}", env.name()))
        .non_fatal();
}

pub(super) fn post_message_to(env: &PluginEnv, plugin_message: PluginMessage) -> Result<()> {
    let worker_name = plugin_message
        .worker_name
        .ok_or(anyhow!("Worker name not specified in message to worker"))?;
    if let Some(workers) = &env.workers {
        return workers.post(
            &worker_name,
            env.client_id,
            plugin_message.name,
            plugin_message.payload,
        );
    }
    env.senders
        .send_to_plugin(PluginInstruction::PostMessagesToPluginWorker(
            env.plugin_id,
            // workers are keyed in the PluginMap, so this is the owner and not the acting client
            env.owner_client_id,
            worker_name,
            vec![(plugin_message.name, plugin_message.payload)],
        ))
}

pub(super) fn post_message_to_plugin(env: &PluginEnv, plugin_message: PluginMessage) -> Result<()> {
    if let Some(worker_name) = plugin_message.worker_name {
        return Err(anyhow!(
            "Worker name (\"{}\") should not be specified in message to plugin",
            worker_name
        ));
    }
    if let Some(workers) = &env.workers {
        return env
            .senders
            .send_to_plugin(PluginInstruction::for_plugin_event(
                env.plugin_id,
                env.client_id,
                Some(workers.parent_generation),
                Event::CustomMessage(plugin_message.name, plugin_message.payload),
            ));
    }
    env.senders
        .send_to_plugin(PluginInstruction::PostMessageToPlugin(
            env.plugin_id,
            env.client_id,
            plugin_message.name,
            plugin_message.payload,
        ))
}

pub(super) fn reconfigure(
    env: &PluginEnv,
    new_config: String,
    write_config_to_disk: bool,
) -> Result<()> {
    let err_context = || "Failed to reconfigure";
    let client_id = env.client_id;
    env.senders
        .send_to_server(ServerInstruction::Reconfigure {
            client_id,
            config: new_config,
            write_config_to_disk,
        })
        .with_context(err_context)?;
    Ok(())
}

pub(super) fn rebind_keys(
    env: &PluginEnv,
    keys_to_rebind: Vec<(InputMode, KeyWithModifier, Vec<Action>)>,
    keys_to_unbind: Vec<(InputMode, KeyWithModifier)>,
    write_config_to_disk: bool,
) -> Result<()> {
    let err_context = || "Failed to rebind_keys";
    let client_id = env.client_id;
    env.senders
        .send_to_server(ServerInstruction::RebindKeys {
            client_id,
            keys_to_rebind,
            keys_to_unbind,
            write_config_to_disk,
            plugin_id: None,
        })
        .with_context(err_context)?;
    Ok(())
}

pub(super) fn register_default_keybindings(env: &PluginEnv, keybindings: String) -> Result<()> {
    let parsed = yinz_config::kdl::keybinds::from_string(
        keybindings,
        Keybinds::default(),
        &Options::default(),
    )
    .map_err(|error| anyhow!("{error}"))?;
    let mut keys_to_rebind = vec![];
    for (mode, bindings) in parsed.0 {
        for (key, mut actions) in bindings {
            for action in &mut actions {
                if let Action::KeybindPipe {
                    name: Some(name),
                    plugin_id,
                    ..
                } = action
                    && name.starts_with("__yinz_default_keybinding:")
                {
                    *plugin_id = Some(env.plugin_id);
                }
            }
            keys_to_rebind.push((mode, key, actions));
        }
    }
    env.senders
        .send_to_server(ServerInstruction::RebindKeys {
            client_id: env.client_id,
            keys_to_rebind,
            keys_to_unbind: vec![],
            write_config_to_disk: false,
            plugin_id: Some(env.plugin_id),
        })
        .context("failed to register default keybindings")
}

pub(super) fn start_or_reload_plugin(env: &PluginEnv, url: &str) -> Result<()> {
    let error_msg = || format!("failed to start or reload plugin in plugin {}", env.name());
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let run_plugin_or_alias =
        yinz_runtime::plugin_location::run_plugin_or_alias_from_url(url, &None, None, Some(cwd))
            .map_err(|e| anyhow!("Failed to parse plugin location: {}", e))?;
    let action = Action::StartOrReloadPlugin {
        plugin: run_plugin_or_alias,
    };
    apply_action!(action, error_msg, env);
    Ok(())
}

pub(super) fn get_plugin_info(env: &mut PluginEnv, id: Option<PluginId>) -> Result<()> {
    let plugin_id = id.unwrap_or(env.plugin_id);
    let info = if plugin_id == env.plugin_id {
        Some((env.background, Some(env.surface == Surface::Pane)))
    } else {
        let panes = env.panes.lock().unwrap();
        panes
            .contains(plugin_id)
            .then(|| (panes.background(plugin_id), panes.presence(plugin_id)))
    };
    let info = info.map(|(background, pane)| yinz_types::data::PluginInstanceInfo {
        plugin_id,
        background,
        pane,
    });
    let response = yinz_plugin_api::plugin_command::ProtobufGetPluginInfoResponse {
        json: serde_json::to_string(&info).context("encode plugin info")?,
    };
    write_response(env, &response.encode_to_vec()).context("write plugin info")
}

pub(super) fn get_plugin_ids_for_url(
    env: &mut PluginEnv,
    url: String,
    configuration: Option<BTreeMap<String, String>>,
) -> Result<()> {
    let (response_channel, response) = crossbeam::channel::bounded(1);
    let result = env
        .senders
        .send_to_plugin(PluginInstruction::GetPluginIdsForUrl {
            url,
            configuration,
            response_channel,
        })
        .context("send plugin identity query")
        .and_then(|()| {
            response
                .recv_timeout(std::time::Duration::from_secs(10))
                .context("receive plugin identity query")
        })
        .map_err(|error| format!("{error:#}"))
        .and_then(|result| result);
    let response = match result {
        Ok(ids) => yinz_plugin_api::plugin_command::ProtobufGetPluginIdsForUrlResponse {
            plugin_ids: ids.into_iter().map(|id| id.get()).collect(),
            error: None,
        },
        Err(error) => yinz_plugin_api::plugin_command::ProtobufGetPluginIdsForUrlResponse {
            plugin_ids: vec![],
            error: Some(error),
        },
    };
    write_response(env, &response.encode_to_vec()).context("write plugin identity response")
}

pub(super) fn reload_plugin_by_url(env: &PluginEnv, url: String) -> Result<()> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let run_plugin_or_alias =
        yinz_runtime::plugin_location::run_plugin_or_alias_from_url(&url, &None, None, Some(cwd))
            .map_err(|e| anyhow!("Failed to parse plugin location: {}", e))?;
    env.senders
        .send_to_plugin(PluginInstruction::ReloadPluginByUrl(run_plugin_or_alias))
        .with_context(|| format!("failed to reload plugin at {url}"))
}

pub(super) fn configuration_editor(
    env: &mut PluginEnv,
    edit: Option<yinz_types::configuration::Edit>,
    theme: Option<String>,
) {
    let (reply, receiver) = crossbeam::channel::bounded(1);
    let result = (|| -> Result<Vec<u8>> {
        env.senders
            .send_to_server(ServerInstruction::ConfigurationEditor { edit, theme, reply })
            .context("requesting configuration editor")?;
        let snapshot = receiver
            .recv_timeout(std::time::Duration::from_secs(30))
            .context("configuration editor did not reply")?
            .map_err(|e| anyhow!("{e}"))?;
        serde_json::to_vec(&snapshot).context("encoding configuration snapshot")
    })();
    let response = ProtobufFsReadResponse {
        result: Some(match result {
            Ok(contents) => fs_read_response::Result::Contents(contents),
            Err(error) => fs_read_response::Result::Error(format!("{error:#}")),
        }),
    };
    write_response(env, &response.encode_to_vec())
        .context("replying to configuration editor")
        .non_fatal();
}

pub(super) fn project_config(
    env: &mut PluginEnv,
    directory: PathBuf,
    write: Option<(String, Option<String>)>,
) {
    let result = (|| -> Result<Vec<u8>> {
        let directory = if directory.is_absolute() {
            directory
        } else {
            env.plugin_cwd.join(directory)
        };
        let root = yinz_config::project_config::root(&directory).map_err(|e| anyhow!(e))?;
        let own = env
            .plugin_cwd
            .canonicalize()
            .context("cannot resolve plugin directory")?;
        if !root.starts_with(&own) && !has_permission(env, PermissionType::FullHdAccess) {
            return Err(anyhow!(
                "project configuration outside the plugin directory requires FullHdAccess"
            ));
        }
        let snapshot = if let Some((source, expected)) = write {
            yinz_config::project_config::access_root(&root, Some((&source, expected.as_deref())))
                .map_err(|e| anyhow!(e))?
        } else {
            yinz_config::project_config::access_root(&root, None).map_err(|e| anyhow!(e))?
        };
        serde_json::to_vec(&snapshot).context("could not serialize project configuration")
    })();
    let response = ProtobufFsReadResponse {
        result: Some(match result {
            Ok(contents) => fs_read_response::Result::Contents(contents),
            Err(e) => fs_read_response::Result::Error(format!("{e:#}")),
        }),
    };
    write_response(env, &response.encode_to_vec())
        .context("could not send project configuration")
        .non_fatal();
}

pub(super) fn reload_plugin(env: &PluginEnv, plugin_id: PluginId) {
    let _ = env
        .senders
        .send_to_plugin(PluginInstruction::ReloadPluginWithId(plugin_id));
}

pub(super) fn load_new_plugin(
    env: &PluginEnv,
    url: String,
    config: BTreeMap<String, String>,
    load_in_background: bool,
    skip_plugin_cache: bool,
    cwd: Option<PathBuf>,
) {
    let url = if &url == "yinz:OWN_URL" {
        env.plugin_config.location.display()
    } else {
        url
    };
    let resolved_cwd = cwd.clone().unwrap_or_else(|| env.plugin_cwd.clone());
    let parsed = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        &url,
        &Some(config),
        Some(&env.plugin_aliases.lock().unwrap()),
        Some(resolved_cwd.clone()),
    );
    let mut run_plugin_or_alias = match parsed {
        Ok(plugin) => plugin,
        Err(error) => {
            log::error!("Failed to load new plugin: {error}");
            return;
        },
    };
    if load_in_background {
        if let Some(cwd) = cwd
            && run_plugin_or_alias.get_initial_cwd().is_none()
        {
            run_plugin_or_alias.add_initial_cwd(&cwd);
        }
        let _ = env
            .senders
            .send_to_plugin(PluginInstruction::LoadBackgroundPlugin(
                run_plugin_or_alias,
                env.client_id,
            ));
    } else {
        let should_float = Some(true);
        let should_be_open_in_place = false;
        let pane_title = None;
        let tab_index = None;
        let pane_id_to_replace = None;
        let client_id = env.client_id;
        let size = Default::default();
        let cwd = Some(resolved_cwd);
        let skip_cache = skip_plugin_cache;
        let _ = env.senders.send_to_plugin(PluginInstruction::Load(
            should_float,
            should_be_open_in_place,
            false, // close_replaced_pane
            pane_title,
            run_plugin_or_alias,
            tab_index,
            pane_id_to_replace,
            client_id,
            size,
            cwd,
            None,
            skip_cache,
            None,
            None,
            None,
            None,
        ));
    }
}

pub(super) fn intercept_key_presses(env: &mut PluginEnv) {
    env.intercepting_key_presses = true;
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::InterceptKeyPresses(
            env.plugin_id,
            env.client_id,
        ));
}

pub(super) fn clear_key_presses_intercepts(env: &mut PluginEnv) {
    env.intercepting_key_presses = false;
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::ClearKeyPressesIntercepts(
            env.plugin_id,
            env.client_id,
        ));
}

// Custom panic handler for plugins.
//
// This is called when a panic occurs in a plugin. Since most panics will likely originate in the
// code trying to deserialize an `Event` upon a plugin state update, we read some panic message,
// formatted as string from the plugin.
pub(super) fn report_panic(env: &PluginEnv, msg: &str) {
    log::error!("PANIC IN PLUGIN!\n\r{}", msg);
    handle_plugin_crash(
        env.plugin_id,
        env.generation,
        Some(env.instance_id),
        msg.to_owned(),
        env.senders.clone(),
    );
}

/// Use the host timezone database: an offset alone cannot distinguish CDT from other zones.
fn local_time(epoch_ms: SignedUnixMillis) -> Result<String> {
    let seconds = epoch_ms.get().div_euclid(1000);
    let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
    // SAFETY: both pointers refer to valid storage; localtime_r initializes local on success.
    let result = unsafe { libc::localtime_r(&seconds, local.as_mut_ptr()) };
    anyhow::ensure!(!result.is_null(), "local timestamp is out of range");
    let mut text = [0_u8; 256];
    // SAFETY: local was initialized above and strftime receives the buffer's actual size.
    let len = unsafe {
        libc::strftime(
            text.as_mut_ptr().cast(),
            text.len(),
            c"%b %e, %Y, %l:%M %p %Z".as_ptr(),
            local.as_ptr(),
        )
    };
    anyhow::ensure!(len > 0, "local date exceeded formatting buffer");
    Ok(std::str::from_utf8(&text[..len])
        .context("decoding local date")?
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" "))
}

pub(super) fn get_local_time(env: &mut PluginEnv, epoch_ms: SignedUnixMillis) {
    let response = ProtobufGetLocalTimeResponse {
        text: local_time(epoch_ms)
            .context("formatting local date")
            .unwrap_or_else(|error| {
                Err::<(), _>(error).non_fatal();
                "Date unavailable".into()
            }),
    };
    write_response(env, &response.encode_to_vec())
        .context("sending local date response")
        .non_fatal();
}

#[cfg(test)]
#[path = "../unit/local_time.rs"]
mod local_time_tests;
