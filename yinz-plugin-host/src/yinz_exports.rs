#[path = "agent_control.rs"]
mod agent_control;
#[path = "palette_agents.rs"]
pub(crate) mod palette_agents;
#[path = "palette_notes.rs"]
pub(crate) mod palette_notes;
pub(crate) use agent_control::agent_event_projects;
pub use agent_control::control_agent;
#[path = "multipanes.rs"]
mod multipanes;

#[cfg(test)]
#[path = "unit/agent_spawn_tests.rs"]
mod agent_spawn_tests;
#[cfg(test)]
#[path = "unit/layout_cwd_tests.rs"]
mod layout_cwd_tests;
#[cfg(test)]
#[path = "unit/plugin_ids_tests.rs"]
mod plugin_ids_tests;

use super::PluginInstruction;
use crate::plugin_map::{PluginEnv, ResponseSink};
use crate::wasm_bridge::handle_plugin_crash;
use anyhow::ensure;
use log::warn;
use serde::Serialize;
use std::collections::BTreeSet;
use std::os::unix::fs::OpenOptionsExt;
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    process,
    sync::atomic,
    thread,
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
use wasmi::{Caller, Linker};
use yinz_bus::background_jobs::BackgroundJob;
use yinz_bus::global_async_runtime::get_tokio_runtime;
use yinz_bus::pty::{ClientTabIndexOrPaneId, PtyInstruction};
use yinz_bus::route::NotificationEnd;
use yinz_bus::thread_bus::ThreadSenders;
use yinz_bus::ServerInstruction;
use yinz_config::permission::PermissionCache;
use yinz_ipc::consts::ipc_connect;
use yinz_routing::{route_action, wait_for_action_completion};
use yinz_runtime::home::{absolute_lexical, default_layout_dir, default_user_plugin_dir};
use yinz_storage::data_access::DataAccess;
use yinz_storage::plugin_data_store;
use yinz_types::agent::control::{AgentOutcome, AgentResponse};
use yinz_types::agent::{AgentLaunchResources, SpawnedAgent};
use yinz_types::data::EnqueueQueueJobResponse;
use yinz_types::internal_ids::DataWatchId;
use yinz_types::internal_ids::ProcessId;
use yinz_types::internal_ids::QueueJobId;
use yinz_types::internal_ids::SessionName;
use yinz_types::internal_ids::SupervisorId;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabNumber;
use yinz_types::internal_ids::TabPosition;
use yinz_types::internal_ids::{PluginId, TerminalId};
use yinz_types::keys::KeyWithModifier;
use yinz_types::units::{FractionalSeconds, SignedUnixMillis};
use yinz_types::{
    agent::{AgentInfo, AgentSpawnSpec},
    command::PaneTarget,
};
use yinz_types::{command::GitCheckout, geometry::FloatingPaneCoordinates};
use {yinz_ipc::ipc::ClientToServerMsg, yinz_ipc::ipc::IpcSenderWithContext};
use {
    yinz_session::sessions::generate_random_name as generate_random_name_impl,
    yinz_session::sessions::validate_session_name,
};
use {
    yinz_types::data::validate_namespace, yinz_types::data::DataApplyResponse,
    yinz_types::data::DataCatalogResponse, yinz_types::data::DataCountResponse,
    yinz_types::data::DataDefineResponse, yinz_types::data::DataEntityInfo,
    yinz_types::data::DataError, yinz_types::data::DataFetchResponse,
    yinz_types::data::DataUnwatchResponse, yinz_types::data::DataWatchResponse,
    yinz_types::data::EntityDef, yinz_types::data::FetchRequest, yinz_types::data::Mutation,
    yinz_types::data::WatchMode, yinz_types::data::SHARED_NAMESPACE_PREFIX,
};
use {
    yinz_types::data::BreakPanesToNewTabResponse, yinz_types::data::BreakPanesToTabWithIdResponse,
    yinz_types::data::BreakPanesToTabWithIndexResponse, yinz_types::data::CommandType,
    yinz_types::data::DeleteAllDeadSessionsResponse, yinz_types::data::DeleteDeadSessionResponse,
    yinz_types::data::DeleteLayoutResponse, yinz_types::data::EditLayoutResponse,
    yinz_types::data::Event, yinz_types::data::FileMetadata,
    yinz_types::data::FocusOrCreateTabResponse, yinz_types::data::GetFocusedPaneInfoResponse,
    yinz_types::data::GetPaneCwdResponse, yinz_types::data::GetPaneGitProbeResponse,
    yinz_types::data::GetPanePidResponse, yinz_types::data::GetPaneRunningCommandResponse,
    yinz_types::data::HttpVerb, yinz_types::data::KillSessionsResponse,
    yinz_types::data::LayoutInfo, yinz_types::data::LayoutMetadata,
    yinz_types::data::LayoutParsingError, yinz_types::data::ListOrphanedPanesResponse,
    yinz_types::data::MessageToPlugin, yinz_types::data::NewPanePlacement,
    yinz_types::data::NewTabResponse, yinz_types::data::NewTabUnfocusedResponse,
    yinz_types::data::NewTiledPaneInTabResponse, yinz_types::data::NotifyClick,
    yinz_types::data::OpenCommandPaneBackgroundResponse,
    yinz_types::data::OpenCommandPaneFloatingNearPluginResponse,
    yinz_types::data::OpenCommandPaneFloatingResponse,
    yinz_types::data::OpenCommandPaneInGitCheckoutResponse,
    yinz_types::data::OpenCommandPaneInPlaceOfPaneIdResponse,
    yinz_types::data::OpenCommandPaneInPlaceOfPluginResponse,
    yinz_types::data::OpenCommandPaneInPlaceResponse,
    yinz_types::data::OpenCommandPaneOverlayResponse, yinz_types::data::OpenCommandPaneResponse,
    yinz_types::data::OpenEditPaneInPlaceOfPaneIdResponse,
    yinz_types::data::OpenFileFloatingNearPluginResponse,
    yinz_types::data::OpenFileFloatingResponse, yinz_types::data::OpenFileInPlaceOfPluginResponse,
    yinz_types::data::OpenFileInPlaceResponse, yinz_types::data::OpenFileNearPluginResponse,
    yinz_types::data::OpenFileResponse, yinz_types::data::OpenPaneInNewTabResponse,
    yinz_types::data::OpenPluginPaneFloatingResponse,
    yinz_types::data::OpenTerminalFloatingNearPluginResponse,
    yinz_types::data::OpenTerminalFloatingResponse,
    yinz_types::data::OpenTerminalInPlaceOfPluginResponse,
    yinz_types::data::OpenTerminalInPlaceResponse,
    yinz_types::data::OpenTerminalNearPluginResponse,
    yinz_types::data::OpenTerminalPaneInPlaceOfPaneIdResponse,
    yinz_types::data::OpenTerminalResponse, yinz_types::data::OrphanedPane,
    yinz_types::data::OverlayPlacement, yinz_types::data::PaneFrameStyle,
    yinz_types::data::PaneScrollbackResponse, yinz_types::data::PermissionStatus,
    yinz_types::data::PermissionType, yinz_types::data::RegexHighlight,
    yinz_types::data::RenameLayoutResponse, yinz_types::data::SaveLayoutResponse,
    yinz_types::data::SessionConnection, yinz_types::data::SpawnAgentResponse,
    yinz_types::data::TabMetadata, yinz_types::data::WarpPaneResponse,
    yinz_types::data::WarpPlacement, yinz_types::data::AGENT_PANE_CONTEXT,
};

use crate::PaneId;
use kdl::KdlDocument;
use yinz_bus::screen::{NotificationClick, ScreenInstruction};

use prost::Message;
use yinz_runtime::errors::prelude::*;
use yinz_types::{
    command::CommandToRun,
    geometry::{Direction, Resize, ResizeStrategy},
};
use {
    yinz_config::config::ConfigError,
    yinz_plugin_api::event::layout_parsing_error::ErrorType as ProtobufLayoutParsingErrorType,
    yinz_plugin_api::event::ProtobufLayoutParsingError,
    yinz_plugin_api::event::ProtobufPaneScrollbackResponse,
    yinz_plugin_api::event::ProtobufSyntaxError,
    yinz_plugin_api::plugin_command::dump_layout_response,
    yinz_plugin_api::plugin_command::dump_session_layout_response,
    yinz_plugin_api::plugin_command::fs_list_response,
    yinz_plugin_api::plugin_command::fs_metadata_response,
    yinz_plugin_api::plugin_command::fs_mutate_response,
    yinz_plugin_api::plugin_command::fs_read_response,
    yinz_plugin_api::plugin_command::hide_floating_panes_response,
    yinz_plugin_api::plugin_command::parse_layout_response,
    yinz_plugin_api::plugin_command::read_plugin_log_response,
    yinz_plugin_api::plugin_command::save_session_response,
    yinz_plugin_api::plugin_command::show_floating_panes_response,
    yinz_plugin_api::plugin_command::ProtobufBreakPanesToNewTabResponse,
    yinz_plugin_api::plugin_command::ProtobufBreakPanesToTabWithIdResponse,
    yinz_plugin_api::plugin_command::ProtobufBreakPanesToTabWithIndexResponse,
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
    yinz_plugin_api::plugin_command::ProtobufFsEntry,
    yinz_plugin_api::plugin_command::ProtobufFsEntryList,
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
    yinz_plugin_api::plugin_command::ProtobufNewTabUnfocusedResponse,
    yinz_plugin_api::plugin_command::ProtobufNewTabsResponse,
    yinz_plugin_api::plugin_command::ProtobufNewTiledPaneInTabResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenCommandPaneBackgroundResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenCommandPaneFloatingNearPluginResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenCommandPaneFloatingResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenCommandPaneInGitCheckoutResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenCommandPaneInPlaceOfPaneIdResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenCommandPaneInPlaceOfPluginResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenCommandPaneInPlaceResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenCommandPaneNearPluginResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenCommandPaneOverlayResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenCommandPaneResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenEditPaneInPlaceOfPaneIdResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenFileFloatingNearPluginResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenFileFloatingResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenFileInPlaceOfPluginResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenFileInPlaceResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenFileNearPluginResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenFileResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenPaneInNewTabResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenPluginPaneFloatingResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenTerminalFloatingNearPluginResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenTerminalFloatingResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenTerminalInPlaceOfPluginResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenTerminalInPlaceResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenTerminalNearPluginResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenTerminalPaneInPlaceOfPaneIdResponse,
    yinz_plugin_api::plugin_command::ProtobufOpenTerminalResponse,
    yinz_plugin_api::plugin_command::ProtobufParseLayoutResponse,
    yinz_plugin_api::plugin_command::ProtobufPluginCommand,
    yinz_plugin_api::plugin_command::ProtobufReadPluginLogResponse,
    yinz_plugin_api::plugin_command::ProtobufRenameLayoutResponse,
    yinz_plugin_api::plugin_command::ProtobufSaveLayoutResponse,
    yinz_plugin_api::plugin_command::ProtobufSaveSessionResponse,
    yinz_plugin_api::plugin_command::ProtobufShowFloatingPanesResponse,
    yinz_plugin_api::plugin_command::ProtobufSpawnAgentResponse,
    yinz_plugin_api::plugin_command::ProtobufWarpPaneResponse,
    yinz_plugin_api::plugin_ids::ProtobufPluginIds,
    yinz_plugin_api::plugin_ids::ProtobufYinzVersion, yinz_runtime::consts::VERSION,
    yinz_runtime::consts::YINZ_SESSION_INFO_CACHE_DIR, yinz_runtime::consts::YINZ_SOCK_DIR,
    yinz_runtime::consts::YINZ_TMP_DIR, yinz_types::data::CommandOrPlugin,
    yinz_types::data::EventType, yinz_types::data::FileToOpen, yinz_types::data::InputMode,
    yinz_types::data::PluginCommand, yinz_types::data::PluginIds, yinz_types::data::PluginMessage,
    yinz_types::input::actions::Action, yinz_types::input::command::OpenFilePayload,
    yinz_types::input::command::RunCommand, yinz_types::input::command::RunCommandAction,
    yinz_types::input::command::TerminalAction, yinz_types::input::keybinds::Keybinds,
    yinz_types::input::layout::Layout, yinz_types::input::layout::Run,
    yinz_types::input::layout::TabLayoutInfo, yinz_types::input::options::Options,
};

macro_rules! apply_action {
    ($action:ident, $error_message:ident, $env: ident) => {
        match route_action(
            $action,
            $env.client_id,
            None,
            Some(PaneId::Plugin($env.plugin_id)),
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

fn translate_plugin_path(env: &PluginEnv, path: PathBuf) -> PathBuf {
    if let Ok(stripped) = path.strip_prefix("/host") {
        env.plugin_cwd.join(stripped)
    } else if let Ok(stripped) = path.strip_prefix("/data") {
        env.plugin_own_data_dir.join(stripped)
    } else if let Ok(stripped) = path.strip_prefix("/cache") {
        env.plugin_own_cache_dir.join(stripped)
    } else if let Ok(stripped) = path.strip_prefix("/tmp") {
        YINZ_TMP_DIR.join(stripped)
    } else if path.is_relative() {
        env.plugin_cwd.join(path)
    } else {
        // Absolute path not in any plugin special folder — pass through
        path
    }
}

/// Prepare provider hooks and select a mother profile before either palette or plugin launch.
fn prepare_agent_command(command: &mut CommandToRun) -> Result<(), String> {
    yinz_runtime::agent::pi::prepare_command(command)?;
    if let (Some(root), Some(kind)) = (
        command.target.bubblewrap_root(),
        yinz_types::agent::kind_from_program_and_args(&command.path, &command.args),
    ) {
        let profile = yinz_config::project_config::bubblewrap::Environment::at_root(root)
            .for_agent(&kind, command.cwd.as_deref())
            .map_err(|error| format!("cannot prepare agent sandbox: {error:#}"))?;
        command.target = PaneTarget::bubblewrap(profile.root())?;
    }
    Ok(())
}

/// The `RunCommandAction` behind every plugin call that opens or re-runs a
/// command pane: the plugin's `CommandToRun` with its cwd resolved through the
/// plugin mounts (`command_cwd`), tagged as this plugin's so the pane's exit
/// comes back to it with `context`. Placement is the caller's to set; the
/// defaults here are the ones every caller shared - held on close, not
/// suspended on start.
fn run_command_action(
    env: &PluginEnv,
    mut command_to_run: CommandToRun,
    default_to_plugin_cwd: bool,
    context: BTreeMap<String, String>,
) -> Option<RunCommandAction> {
    let cwd = if env.plugin_config.is_builtin()
        && (context.contains_key("swamp_owner")
            || context
                .get("agent_agent_host_cwd")
                .is_some_and(|value| value == "true"))
    {
        command_to_run.cwd.clone()
    } else {
        command_cwd(env, &command_to_run, default_to_plugin_cwd)
    };
    command_to_run.cwd = cwd;
    command_action_with_resolved_cwd(env, command_to_run, context)
}

fn command_action_with_resolved_cwd(
    env: &PluginEnv,
    mut command_to_run: CommandToRun,
    context: BTreeMap<String, String>,
) -> Option<RunCommandAction> {
    if let Err(error) = prepare_agent_command(&mut command_to_run) {
        log::error!("cannot prepare agent command: {error}");
        notify(env, error, None, None);
        return None;
    }
    Some(RunCommandAction {
        command: command_to_run.path,
        args: command_to_run.args,
        cwd: command_to_run.cwd,
        direction: None,
        hold_on_close: true,
        hold_on_start: false,
        originating_plugin: Some(env.originating_plugin(context)),
        use_terminal_title: false,
        target: command_to_run.target,
        remote_default_shell: command_to_run.remote_default_shell,
    })
}

/// Where a plugin's command starts. Locally the plugin mounts are translated
/// and, when the caller asks, a missing cwd becomes the plugin's own. A remote
/// target's cwd is a path on that machine and is passed through as written;
/// with none given the pane starts wherever that machine logs the user in,
/// never in the plugin's cwd, which is a path here.
fn command_cwd(
    env: &PluginEnv,
    command: &CommandToRun,
    default_to_plugin_cwd: bool,
) -> Option<PathBuf> {
    if command.target.is_remote() {
        if let Some(cwd) = command.cwd.as_deref() {
            if ["/host", "/data", "/cache"]
                .iter()
                .any(|mount| cwd.starts_with(mount))
            {
                Err::<(), _>(anyhow!(
                    "plugin {} sent the plugin-mount path {} as the cwd of a remote command; \
                     remote cwds are paths on the remote machine",
                    env.name(),
                    cwd.display()
                ))
                .non_fatal();
            }
        }
        command.cwd.clone()
    } else {
        command
            .cwd
            .clone()
            .map(|cwd| translate_plugin_path(env, cwd))
            .or_else(|| default_to_plugin_cwd.then(|| env.plugin_cwd.clone()))
    }
}

pub fn yinz_exports(linker: &mut Linker<PluginEnv>) {
    linker
        .func_wrap("yinz", "host_run_plugin_command", host_run_plugin_command)
        .unwrap();
}

fn host_run_plugin_command(mut caller: Caller<'_, PluginEnv>) {
    let env = caller.data_mut();
    wasi_read_bytes(env)
        .and_then(|bytes| {
            let command: ProtobufPluginCommand = ProtobufPluginCommand::decode(bytes.as_slice())?;
            let command: PluginCommand = command
                .try_into()
                .map_err(|e| anyhow!("failed to convert serialized command: {}", e))?;
            run_plugin_command(env, command)
        })
        .with_context(|| format!("failed to run plugin command {}", env.name()))
        .non_fatal();
}

/// Run one host command on behalf of a plugin, after checking that the plugin is permitted to.
///
/// This is the whole plugin capability surface, and it is deliberately independent of how the
/// plugin was written: the wasm host decodes a protobuf command off a WASI pipe and calls this,
/// and the TypeScript host builds the same `PluginCommand` from a TypeScript call and calls this. Neither the
/// permission check nor any individual handler knows which runtime it is serving.
pub fn run_plugin_command(env: &mut PluginEnv, command: PluginCommand) -> Result<()> {
    let plugin_command = env.name();
    let err_context = || format!("failed to run plugin command {}", plugin_command);
    match check_command_permission(env, &command) {
        (PermissionStatus::Granted, _) => match command {
            PluginCommand::CommandPalette(mut command) => {
                let generation = env
                    .deno_generation
                    .context("Command palettes require a Deno plugin")?;
                anyhow::ensure!(
                    env.deno_workers
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
                        if let Ok(reply) = result {
                            if let Err(error) = reply.validate() {
                                env.senders
                                    .send_to_plugin(PluginInstruction::DenoPalette(
                                        yinz_bus::command_palette::Delivery {
                                            owner: owner.clone(),
                                            client: env.client_id,
                                            message:
                                                yinz_types::command_palette::Message::Release {
                                                    handles: reply.handles(),
                                                },
                                            active: None,
                                        },
                                    ))
                                    .context("releasing invalid palette reply")?;
                                *result = Err(error);
                            }
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
            PluginCommand::RegisterAgentProvider(provider) => {
                let typescript = env.plugin_config.is_builtin_deno()
                    || matches!(
                        env.resolved_source_path
                            .as_deref()
                            .unwrap_or(&env.plugin_config.path)
                            .extension()
                            .and_then(|extension| extension.to_str()),
                        Some("ts" | "mts")
                    );
                let result = match env.deno_generation.filter(|_| typescript) {
                    _ if env.unavailable.load(atomic::Ordering::Acquire)
                        || env.cancel.load(atomic::Ordering::Acquire) =>
                    {
                        Err("agent provider plugin is no longer available".to_owned())
                    },
                    None => {
                        Err("agent providers require a TypeScript (.ts or .mts) plugin".to_owned())
                    },
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
                if let yinz_types::agent::control::AgentRequest::Spawn {
                    cwd,
                    project,
                    remote_host,
                    ..
                } = &mut request
                {
                    if remote_host.is_none() {
                        if let Some(path) = cwd.as_mut().filter(|path| path.is_relative()) {
                            *path = translate_plugin_path(env, path.clone());
                        }
                    }
                    if cwd.is_none() && project.is_none() && remote_host.is_none() {
                        *cwd = Some(env.plugin_cwd.clone());
                    }
                }
                let senders = env.senders.clone();
                let data = env.data_access.clone();
                let target = env.plugin_id;
                let client = env.client_id;
                let generation = env.deno_generation;
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
            PluginCommand::SwampRequest {
                directory,
                id,
                operation,
                payload,
            } => {
                if !env.plugin_config.is_builtin() {
                    return Err(anyhow!("managed Swamp requests require a builtin plugin"));
                }
                swamp_request(env, directory, id, operation, payload);
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
                wasi_write_object(env, &response.encode_to_vec())?;
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
                        env.deno_generation,
                    ))?;
            },
            PluginCommand::Subscribe(event_list) => subscribe(env, event_list)?,
            PluginCommand::Unsubscribe(event_list) => unsubscribe(env, event_list)?,
            PluginCommand::SetSelectable(selectable) => set_selectable(env, selectable),
            PluginCommand::ShowCursor(cursor_position) => show_cursor(env, cursor_position),
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
                crate::deno::testing::start(env, name, options, request_id)?;
            },
            PluginCommand::CancelPluginTest(id) => crate::deno::testing::cancel(env, &id),
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
            PluginCommand::Pick(command) => crate::wasm_bridge::picker::dispatch(env, command)?,
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
                open_terminal_in_place_of_plugin(env, cwd.path, close_plugin_after_replace)
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
            ),
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
            PluginCommand::DataUnwatch { watch_id } => data_unwatch(env, watch_id),
            PluginCommand::ScanHostFolder(folder_to_scan) => scan_host_folder(env, folder_to_scan),
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
            PluginCommand::FsMetadata { path } => fs_metadata(env, path),
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
            PluginCommand::DumpSessionLayout { tab_index } => dump_session_layout(env, tab_index),
            PluginCommand::CloseSelf => close_self(env),
            PluginCommand::EnqueueQueueJob {
                queue,
                plugin_url,
                config,
                context,
            } => enqueue_queue_job(env, queue, plugin_url, config, context),
            PluginCommand::ConfigureQueue { queue, concurrency } => {
                configure_queue(env, queue, concurrency)
            },
            PluginCommand::CancelQueueJob { job_id } => cancel_queue_job(env, job_id),
            PluginCommand::QueueStatus { queue } => queue_status(env, queue),
            PluginCommand::FinishQueueJob { success, payload } => {
                finish_queue_job(env, success, payload)
            },
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
            PluginCommand::SetProjectContext(projects, panes) => {
                env.senders
                    .send_to_screen(ScreenInstruction::SetProjectContext(projects, panes))
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
            } => load_new_plugin(env, url, config, load_in_background, skip_plugin_cache),
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
            ) => {
                open_file_in_place_of_plugin(env, file_to_open, close_plugin_after_replace, context)
            },
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
            PluginCommand::SetRelatedPanes(pane_ids) => {
                set_related_panes(env, pane_ids.into_iter().collect())
            },
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
            ),
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
            ),
            PluginCommand::OpenTerminalPaneInPlaceOfPaneId(pane_id, cwd, close_replaced_pane) => {
                open_terminal_pane_in_place_of_pane_id(env, pane_id, cwd, close_replaced_pane)
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
            ),
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
                PluginCommand::RegisterAgentProvider(_) | PluginCommand::GetAgentProviders
            ) {
                return write_agent_providers_response(
                    env,
                    Err(format!("Permission denied: {permission:?}")),
                );
            }
            if matches!(command, PluginCommand::PaneTabs(_)) {
                let result: yinz_types::pane_tabs::PaneTabsResponse =
                    Err(format!("Permission denied: {permission:?}"));
                let response = yinz_plugin_api::plugin_command::ProtobufPaneTabsResponse {
                    json: serde_json::to_string(&result)
                        .context("failed to encode pane tabs denial")?,
                };
                return wasi_write_object(env, &response.encode_to_vec());
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
                return wasi_write_object(env, &response.encode_to_vec());
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
                wasi_write_object(env, &response).with_context(err_context)?;
            }
        },
    };
    Ok(())
}

fn write_agent_providers_response(
    env: &mut PluginEnv,
    result: Result<Vec<yinz_types::agent::provider::AgentProvider>, String>,
) -> Result<()> {
    let response = yinz_plugin_api::plugin_command::ProtobufAgentProvidersResponse {
        json: serde_json::to_string(&result).context("serialize agent provider response")?,
    };
    wasi_write_object(env, &response.encode_to_vec())
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

fn subscribe(env: &PluginEnv, event_list: HashSet<EventType>) -> Result<()> {
    let background_subscriptions = {
        let mut subscriptions = env.subscriptions.lock().to_anyhow()?;
        subscriptions.extend(event_list.clone());
        env.tab_index.is_none().then(|| subscriptions.clone())
    };
    env.senders
        .send_to_plugin(PluginInstruction::PluginSubscribedToEvents(
            env.plugin_id,
            env.client_id,
            event_list,
            // The plugin thread must not lock this guest to inspect its metadata:
            // the guest may already be waiting for a synchronous reply from it.
            background_subscriptions,
        ))
}

fn unblock_cli_pipe_input(env: &PluginEnv, pipe_name: String) {
    env.input_pipes_to_unblock.lock().unwrap().insert(pipe_name);
}

fn block_cli_pipe_input(env: &PluginEnv, pipe_name: String) {
    env.input_pipes_to_block.lock().unwrap().insert(pipe_name);
}

fn cli_pipe_output(env: &PluginEnv, pipe_name: String, output: String) -> Result<()> {
    env.senders
        .send_to_server(ServerInstruction::CliPipeOutput(pipe_name, output))
        .context("failed to send pipe output")
}

fn message_to_plugin(env: &PluginEnv, mut message_to_plugin: MessageToPlugin) -> Result<()> {
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

fn unsubscribe(env: &PluginEnv, event_list: HashSet<EventType>) -> Result<()> {
    let background_subscriptions = {
        let mut subscriptions = env.subscriptions.lock().to_anyhow()?;
        subscriptions.retain(|k| !event_list.contains(k));
        env.tab_index.is_none().then(|| subscriptions.clone())
    };
    if background_subscriptions.is_some() {
        env.senders
            .send_to_plugin(PluginInstruction::PluginSubscribedToEvents(
                env.plugin_id,
                env.client_id,
                HashSet::new(),
                background_subscriptions,
            ))?;
    }
    Ok(())
}

fn set_selectable(env: &PluginEnv, selectable: bool) {
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

fn show_cursor(env: &PluginEnv, cursor_position: Option<(usize, usize)>) {
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

/// Grant another plugin permissions ahead of its prompt. Refused for anything but a builtin:
/// `check_command_permission` already waves a builtin through, so the gate has to be here, and
/// a non-builtin reaching it would be a plugin handing out authority it was never given.
fn grant_plugin_permissions(
    env: &PluginEnv,
    plugin_url: String,
    permissions: Vec<PermissionType>,
) -> Result<()> {
    if !env.plugin_config.is_builtin() {
        return Err(anyhow!(
            "only a builtin plugin may grant permissions to another plugin; {} is not one",
            env.name()
        ));
    }
    env.senders
        .send_to_plugin(PluginInstruction::GrantPermissionsToLocation {
            plugin_url,
            permissions,
        })
        .with_context(|| format!("failed to grant permissions from {}", env.name()))
}

fn request_permission(env: &PluginEnv, permissions: Vec<PermissionType>) -> Result<()> {
    // Before either branch: from here on this plugin is waiting for an answer, and a denial it
    // meets in the meantime is a timing problem rather than a missing request. See
    // [`permission_answer_pending`].
    env.permissions_requested
        .store(true, atomic::Ordering::SeqCst);
    // Cached grants take effect synchronously; every answer still carries its runtime identity.
    use yinz_types::input::layout::RunPluginLocation;
    let location = match (&env.plugin_config.location, &env.resolved_source_path) {
        (RunPluginLocation::File(_), Some(path)) => RunPluginLocation::File(
            yinz_runtime::common_path::source_path(path, &env.plugin_cwd),
        ),
        (location, _) => location.clone(),
    };
    if PermissionCache::from_path_or_default(None)
        .check_permissions(location.to_string(), &permissions)
    {
        env.permissions
            .lock()
            .unwrap()
            .get_or_insert_with(HashSet::new)
            .extend(permissions.iter().copied());
    }
    env.senders
        .send_to_plugin(PluginInstruction::RequestPermissions {
            plugin_id: env.plugin_id,
            instance_id: env.instance_id,
            client_id: env.client_id,
            permissions,
        })
}

fn get_plugin_ids(env: &mut PluginEnv) {
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
            wasi_write_object(env, &serialized.encode_to_vec())?;
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

fn max_session_name_bytes(socket_dir: &Path) -> usize {
    // Joining a one-byte name accounts for both the separator and the terminating NUL.
    yinz_runtime::consts::YINZ_SOCK_MAX_LENGTH
        .saturating_sub(socket_dir.join("x").as_os_str().len())
}

fn get_yinz_version(env: &mut PluginEnv) {
    let protobuf_yinz_version = ProtobufYinzVersion {
        version: VERSION.to_owned(),
    };
    wasi_write_object(env, &protobuf_yinz_version.encode_to_vec())
        .with_context(|| {
            format!(
                "failed to request yinz version from host for plugin {}",
                env.name()
            )
        })
        .non_fatal();
}

fn generate_random_name(env: &mut PluginEnv) {
    let name = generate_random_name_impl();
    let response = ProtobufGenerateRandomNameResponse { name };
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| {
            format!(
                "failed to send generated random name to plugin {}",
                env.name()
            )
        })
        .non_fatal();
}

fn dump_layout(env: &mut PluginEnv, layout_name: String) {
    let layout_path = PathBuf::from(&layout_name);

    let layout_dir = env.layout_dir.clone().or_else(default_layout_dir);
    let result = validate_plugin_layout_path(&layout_name, layout_dir.as_ref())
        .map_err(|e| e.to_string())
        .and_then(|_| {
            yinz_config::layout::stringified_from_dir(&layout_path, layout_dir.as_ref())
                .map_err(|e| e.to_string())
        });
    let response = match result {
        Ok((_, layout_content, _)) => ProtobufDumpLayoutResponse {
            result: Some(dump_layout_response::Result::LayoutContent(layout_content)),
        },
        Err(e) => ProtobufDumpLayoutResponse {
            result: Some(dump_layout_response::Result::Error(e.to_string())),
        },
    };

    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| format!("failed to send layout dump to plugin {}", env.name()))
        .non_fatal();
}

fn get_layout_dir(env: &mut PluginEnv) {
    // Get layout dir from env or fall back to default
    let layout_dir = env.layout_dir.clone().or_else(default_layout_dir);

    // Convert PathBuf to String
    let layout_dir_string = layout_dir
        .and_then(|path| path.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| String::from(""));

    let response = ProtobufGetLayoutDirResponse {
        layout_dir: layout_dir_string,
    };

    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| format!("failed to send layout dir to plugin {}", env.name()))
        .non_fatal();
}

/// Answer with the folder holding the user's own plugins.
///
/// An empty string means the folder could not be resolved at all, which is the same convention
/// `get_layout_dir` uses - a plugin that cannot be told where to write says so rather than
/// guessing at a path.
fn get_user_plugin_dir(env: &mut PluginEnv) {
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

    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| format!("failed to send user plugin dir to plugin {}", env.name()))
        .non_fatal();
}

fn get_session_environment_variables(env: &mut PluginEnv) {
    use yinz_plugin_api::plugin_command::EnvVariable as ProtobufEnvVariable;

    let env_vars: Vec<ProtobufEnvVariable> = env
        .session_env_vars
        .iter()
        .map(|(name, value)| ProtobufEnvVariable {
            name: name.clone(),
            value: value.clone(),
        })
        .collect();

    let response = ProtobufGetSessionEnvironmentVariablesResponse { env_vars };

    wasi_write_object(env, &response.encode_to_vec())
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
fn get_plugin_alias_configuration(env: &mut PluginEnv, alias: String) {
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

    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| {
            format!(
                "failed to send the configuration of plugin alias {alias} to plugin {}",
                env.name()
            )
        })
        .non_fatal();
}

fn open_command_pane_in_new_tab(
    env: &mut PluginEnv,
    command_to_run: CommandToRun,
    context: BTreeMap<String, String>,
) {
    let Some(run_command_action) = run_command_action(env, command_to_run, true, context) else {
        return;
    };
    let initial_panes = Some(vec![CommandOrPlugin::Command(run_command_action)]);
    let action = Action::NewTab {
        layout_source: None,
        tiled_layout: None,
        floating_layouts: vec![],
        swap_tiled_layouts: None,
        swap_floating_layouts: None,
        tab_name: None,
        should_change_focus_to_new_tab: true,
        cwd: None,
        initial_panes,
        first_pane_unblock_condition: None,
    };
    let error_msg = || "Failed to open command pane in new tab".to_string();
    let result = apply_action!(action, error_msg, env);

    let tab_id = result.as_ref().and_then(|r| r.affected_tab_id);
    let pane_id = result.as_ref().and_then(|r| r.affected_pane_id);
    let response =
        ProtobufOpenPaneInNewTabResponse::from(OpenPaneInNewTabResponse { tab_id, pane_id });
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_command_pane_in_new_tab response".to_string())
        .non_fatal();
}

fn open_plugin_pane_in_new_tab(
    env: &mut PluginEnv,
    plugin_url: String,
    configuration: BTreeMap<String, String>,
    context: BTreeMap<String, String>,
) {
    let run_plugin_or_alias = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        &plugin_url,
        &Some(configuration),
        None,
        Some(env.plugin_cwd.clone()),
    );
    let run_plugin_or_alias = match run_plugin_or_alias {
        Ok(r) => r,
        Err(e) => {
            log::error!("Failed to parse plugin url '{}': {}", plugin_url, e);
            let response =
                ProtobufOpenPaneInNewTabResponse::from(OpenPaneInNewTabResponse::default());
            wasi_write_object(env, &response.encode_to_vec())
                .with_context(|| {
                    "failed to write open_plugin_pane_in_new_tab error response".to_string()
                })
                .non_fatal();
            return;
        },
    };
    let _ = context; // context is not currently used for plugin panes
    let initial_panes = Some(vec![CommandOrPlugin::Plugin(run_plugin_or_alias)]);
    let action = Action::NewTab {
        layout_source: None,
        tiled_layout: None,
        floating_layouts: vec![],
        swap_tiled_layouts: None,
        swap_floating_layouts: None,
        tab_name: None,
        should_change_focus_to_new_tab: true,
        cwd: None,
        initial_panes,
        first_pane_unblock_condition: None,
    };
    let error_msg = || "Failed to open plugin pane in new tab".to_string();
    let result = apply_action!(action, error_msg, env);

    let tab_id = result.as_ref().and_then(|r| r.affected_tab_id);
    let pane_id = result.as_ref().and_then(|r| r.affected_pane_id);
    let response =
        ProtobufOpenPaneInNewTabResponse::from(OpenPaneInNewTabResponse { tab_id, pane_id });
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_plugin_pane_in_new_tab response".to_string())
        .non_fatal();
}

fn open_plugin_pane_floating(
    env: &mut PluginEnv,
    plugin_url: String,
    configuration: BTreeMap<String, String>,
    floating_pane_coordinates: Option<FloatingPaneCoordinates>,
    context: BTreeMap<String, String>,
) {
    let run_plugin_or_alias = yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
        &plugin_url,
        &Some(configuration),
        None,
        Some(env.plugin_cwd.clone()),
    );
    let run_plugin_or_alias = match run_plugin_or_alias {
        Ok(r) => r,
        Err(e) => {
            log::error!("Failed to parse plugin url '{}': {}", plugin_url, e);
            let response = ProtobufOpenPluginPaneFloatingResponse::from(
                OpenPluginPaneFloatingResponse::default(),
            );
            wasi_write_object(env, &response.encode_to_vec())
                .with_context(|| {
                    "failed to write open_plugin_pane_floating error response".to_string()
                })
                .non_fatal();
            return;
        },
    };
    let _ = context; // context is not currently used for plugin panes
    let action = Action::NewFloatingPluginPane {
        plugin: run_plugin_or_alias,
        pane_name: None,
        skip_cache: false,
        cwd: Some(env.plugin_cwd.clone()),
        placement: NewPanePlacement::Floating(floating_pane_coordinates),
        no_focus: false,
        tab_id: None,
    };
    let error_msg = || "Failed to open floating plugin pane".to_string();
    let result = apply_action!(action, error_msg, env);

    let pane_id: OpenPluginPaneFloatingResponse = result.and_then(|r| r.affected_pane_id);
    let response = ProtobufOpenPluginPaneFloatingResponse::from(pane_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_plugin_pane_floating response".to_string())
        .non_fatal();
}

fn open_editor_pane_in_new_tab(
    env: &mut PluginEnv,
    file_to_open: FileToOpen,
    context: BTreeMap<String, String>,
) {
    let _ = context; // context is not currently used for editor panes in new tab
    let path = translate_plugin_path(env, file_to_open.path);
    let cwd = file_to_open
        .cwd
        .map(|cwd| translate_plugin_path(env, cwd))
        .or_else(|| Some(env.plugin_cwd.clone()));
    let file_to_open = FileToOpen {
        path,
        cwd,
        ..file_to_open
    };
    let initial_panes = Some(vec![CommandOrPlugin::File(file_to_open)]);
    let action = Action::NewTab {
        layout_source: None,
        tiled_layout: None,
        floating_layouts: vec![],
        swap_tiled_layouts: None,
        swap_floating_layouts: None,
        tab_name: None,
        should_change_focus_to_new_tab: true,
        cwd: None,
        initial_panes,
        first_pane_unblock_condition: None,
    };
    let error_msg = || "Failed to open editor pane in new tab".to_string();
    let result = apply_action!(action, error_msg, env);

    let tab_id = result.as_ref().and_then(|r| r.affected_tab_id);
    let pane_id = result.as_ref().and_then(|r| r.affected_pane_id);
    let response =
        ProtobufOpenPaneInNewTabResponse::from(OpenPaneInNewTabResponse { tab_id, pane_id });
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_editor_pane_in_new_tab response".to_string())
        .non_fatal();
}

fn get_focused_pane_info(env: &mut PluginEnv) {
    use crossbeam::channel::RecvTimeoutError;
    use std::time::Duration;

    // Bound up front rather than captured: the closure would otherwise hold an immutable
    // borrow of `env` across the `wasi_write_object` that replies to the plugin.
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
    wasi_write_object(env, &protobuf_response.encode_to_vec())
        .with_context(err_context)
        .non_fatal();
}

fn check_plugin(
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
    wasi_write_object(env, &response.encode_to_vec())
        .context("send plugin check report")
        .non_fatal();
}

/// Seconds east of UTC here, at the instant `epoch_ms`.
///
/// The server answers this because it is the only side that can: a plugin is wasm, WASI hands it
/// none of the session's environment, and nothing that reads a zoneinfo database links into one.
/// So every absolute date or time a plugin draws is derived from this.
///
/// The offset is taken *at that instant* rather than now, so a timestamp from before a
/// daylight-saving change reads back with the offset that was in force when it was taken. An
/// instant `chrono` cannot place - out of range, or an hour that a clock change made ambiguous
/// or skipped - falls back to the offset in force now, which is off by an hour at worst and is
/// still the right answer for every timestamp taken today.
fn get_local_utc_offset(env: &mut PluginEnv, epoch_ms: SignedUnixMillis) {
    use chrono::{Local, Offset, TimeZone};
    let offset_seconds = Local
        .timestamp_millis_opt(epoch_ms.get())
        .single()
        .map(|at| at.offset().fix().local_minus_utc())
        .unwrap_or_else(|| Local::now().offset().fix().local_minus_utc());
    let response = ProtobufGetLocalUtcOffsetResponse { offset_seconds };
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| {
            format!(
                "failed to send utc offset response to plugin {}",
                env.name()
            )
        })
        .non_fatal();
}

fn plugin_log_lines(env: &PluginEnv, name: &str, lines: usize, log_file: &Path) -> Result<String> {
    let dir = env.user_plugin_dir.clone().or_else(default_user_plugin_dir);
    crate::plugin_authoring::resolve_log_target(
        name,
        dir.as_deref(),
        &env.yinz_data_dir,
        &env.plugin_aliases.lock().unwrap(),
        &env.plugin_cwd,
    )
    .and_then(|target| crate::plugin_authoring::log_target_tail(log_file, &target, lines))
}

/// Read a name, configured alias, or explicit source path using the caller's cwd.
fn read_plugin_log(env: &mut PluginEnv, name: String, lines: usize) {
    let result = plugin_log_lines(env, &name, lines, &yinz_runtime::consts::YINZ_TMP_LOG_FILE);
    let response = ProtobufReadPluginLogResponse {
        result: Some(match result {
            Ok(lines) => read_plugin_log_response::Result::Lines(lines),
            Err(e) => read_plugin_log_response::Result::Error(format!("{:#}", e)),
        }),
    };
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| format!("failed to send log lines to plugin {}", env.name()))
        .non_fatal();
}

fn parse_layout(env: &mut PluginEnv, layout_string: String) {
    use std::time::{SystemTime, UNIX_EPOCH};

    // Parse the KDL layout - same parameters as list_available_layouts in layout.rs:1260
    let parse_result = yinz_config::kdl::layout::from_kdl(
        &layout_string,
        Some("parse_layout_api".to_string()), // file_name for error reporting
        None,                                 // no swap layouts
        None,                                 // no cwd
    );

    let response = match parse_result {
        Ok(layout) => {
            // Extract tabs from layout
            // This logic matches LayoutMetadata::from in data.rs:1762-1772
            let layout_tabs = layout.tabs();
            let tabs = if layout_tabs.is_empty() {
                // Use default template if no explicit tabs defined
                let (tiled_pane_layout, floating_pane_layout) = layout.new_tab();
                vec![TabMetadata::from(&(
                    None,
                    tiled_pane_layout,
                    floating_pane_layout,
                ))]
            } else {
                layout
                    .tabs()
                    .into_iter()
                    .map(|tab| TabMetadata::from(&tab))
                    .collect()
            };

            // Get current time as Unix epoch timestamp
            let current_time = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .to_string();

            let layout_metadata = LayoutMetadata {
                tabs,
                creation_time: current_time.clone(),
                update_time: current_time,
            };

            // Convert LayoutMetadata to protobuf
            match layout_metadata.try_into() {
                Ok(protobuf_metadata) => ProtobufParseLayoutResponse {
                    result: Some(parse_layout_response::Result::Metadata(protobuf_metadata)),
                },
                Err(e) => {
                    log::error!("Failed to convert metadata to protobuf: {}", e);
                    // Fallback to SyntaxError if conversion fails
                    let error = LayoutParsingError::SyntaxError;
                    let protobuf_error = error.try_into().unwrap_or(ProtobufLayoutParsingError {
                        error_type: Some(ProtobufLayoutParsingErrorType::SyntaxError(
                            ProtobufSyntaxError {},
                        )),
                    });
                    ProtobufParseLayoutResponse {
                        result: Some(parse_layout_response::Result::Error(protobuf_error)),
                    }
                },
            }
        },
        Err(config_error) => {
            // Build LayoutParsingError following list_available_layouts pattern
            // See yinz-types/src/input/layout.rs
            let file_name = "parse_layout_api".to_string();
            let source_code = layout_string;

            let error = match config_error {
                ConfigError::KdlError(kdl_err) => LayoutParsingError::KdlError {
                    kdl_error: Box::new(kdl_err.into()),
                    file_name,
                    source_code,
                },
                _ => LayoutParsingError::SyntaxError,
            };

            // Convert LayoutParsingError to protobuf
            // TryFrom is implemented in yinz-plugin-api/src/event.rs
            let protobuf_error = error.try_into().unwrap_or_else(|e| {
                log::error!("Failed to convert error to protobuf: {}", e);
                ProtobufLayoutParsingError {
                    error_type: Some(ProtobufLayoutParsingErrorType::SyntaxError(
                        ProtobufSyntaxError {},
                    )),
                }
            });

            ProtobufParseLayoutResponse {
                result: Some(parse_layout_response::Result::Error(protobuf_error)),
            }
        },
    };

    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| {
            format!(
                "failed to send parse layout response to plugin {}",
                env.name()
            )
        })
        .non_fatal();
}

fn open_file(env: &mut PluginEnv, file_to_open: FileToOpen, context: BTreeMap<String, String>) {
    let error_msg = || format!("failed to open file in plugin {}", env.name());
    let floating = false;
    let in_place = false;
    let start_suppressed = false;
    let path = translate_plugin_path(env, file_to_open.path);
    let cwd = file_to_open
        .cwd
        .map(|cwd| translate_plugin_path(env, cwd))
        .or_else(|| Some(env.plugin_cwd.clone()));
    let action = Action::EditFile {
        payload: OpenFilePayload::new(path, file_to_open.line_number, cwd)
            .with_originating_plugin(env.originating_plugin(context)),
        direction: None,
        floating,
        in_place,
        close_replaced_pane: false,
        start_suppressed,
        placement: NewPanePlacement::Floating(None),
        near_current_pane: false,
        no_focus: false,
        tab_id: None,
    };
    let result = apply_action!(action, error_msg, env);

    // Extract pane_id from result and convert to yinz_types PaneId
    let pane_id: OpenFileResponse = result.and_then(|r| r.affected_pane_id);

    // Write response to plugin
    let response = ProtobufOpenFileResponse::from(pane_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_file response".to_string())
        .non_fatal();
}

fn run_action(env: &PluginEnv, mut action: Action, context: BTreeMap<String, String>) {
    let ordered_input = matches!(&action, Action::Write { key_with_modifier: Some(_), bytes, .. } if bytes.is_empty());
    // Clone the necessary data to move into the thread
    action.populate_originating_plugin(env.originating_plugin(context.clone()));
    let action_clone = action.clone();
    let deno_generation = env.deno_generation;
    let client_id = env.client_id;
    let plugin_id = env.plugin_id;
    let senders = env.senders.clone();
    let default_shell = env.default_shell.clone();
    let default_mode = env.default_mode;
    let plugin_name = env.name().to_string();

    let run = move || {
        // Execute the action and capture the result
        let pane_id = match route_action(
            action,
            client_id,
            None,
            Some(PaneId::Plugin(plugin_id)),
            senders.clone(),
            default_shell,
            None,
            default_mode,
            None,
            false,
            false, // is_client_input
        ) {
            Ok((_should_break, result)) => {
                // Extract pane_id from ActionCompletionResult
                result.and_then(|r| r.affected_pane_id)
            },
            Err(e) => {
                log::error!("failed to run action in plugin {}: {:?}", plugin_name, e);
                None
            },
        };

        // After action completes, send ActionComplete event with context
        // Convert server PaneId to yinz_types PaneId
        let pane_id_for_event = pane_id;
        let instruction = PluginInstruction::for_plugin_event(
            plugin_id,
            client_id,
            deno_generation,
            Event::ActionComplete(action_clone, pane_id_for_event, context),
        );

        // Send the ActionComplete event back to the plugin
        if let Err(e) = senders.send_to_plugin(instruction) {
            log::error!("Failed to send ActionComplete event: {:?}", e);
        }
    };
    // Forwarded keys (already parsed, without raw bytes) must reach the screen in plugin
    // event order, eg. Right then Enter on a native pane bar.
    if ordered_input {
        run();
    } else {
        thread::spawn(run);
    }
}

fn open_file_floating(
    env: &mut PluginEnv,
    file_to_open: FileToOpen,
    floating_pane_coordinates: Option<FloatingPaneCoordinates>,
    context: BTreeMap<String, String>,
) {
    let error_msg = || format!("failed to open file in plugin {}", env.name());
    let floating = true;
    let in_place = false;
    let start_suppressed = false;
    let path = translate_plugin_path(env, file_to_open.path);
    let cwd = file_to_open
        .cwd
        .map(|cwd| translate_plugin_path(env, cwd))
        .or_else(|| Some(env.plugin_cwd.clone()));
    let action = Action::EditFile {
        payload: OpenFilePayload::new(path, file_to_open.line_number, cwd)
            .with_originating_plugin(env.originating_plugin(context)),
        direction: None,
        floating,
        in_place,
        close_replaced_pane: false,
        start_suppressed,
        placement: NewPanePlacement::Floating(floating_pane_coordinates),
        near_current_pane: false,
        no_focus: false,
        tab_id: None,
    };
    let result = apply_action!(action, error_msg, env);

    // Extract pane_id from result and convert to yinz_types PaneId
    let pane_id: OpenFileFloatingResponse = result.and_then(|r| r.affected_pane_id);

    // Write response to plugin
    let response = ProtobufOpenFileFloatingResponse::from(pane_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_file_floating response".to_string())
        .non_fatal();
}

fn open_file_in_place(
    env: &mut PluginEnv,
    file_to_open: FileToOpen,
    context: BTreeMap<String, String>,
) {
    let error_msg = || format!("failed to open file in plugin {}", env.name());
    let floating = false;
    let in_place = true;
    let start_suppressed = false;
    let path = translate_plugin_path(env, file_to_open.path);
    let cwd = file_to_open
        .cwd
        .map(|cwd| translate_plugin_path(env, cwd))
        .or_else(|| Some(env.plugin_cwd.clone()));

    let action = Action::EditFile {
        payload: OpenFilePayload::new(path, file_to_open.line_number, cwd)
            .with_originating_plugin(env.originating_plugin(context)),
        direction: None,
        floating,
        in_place,
        close_replaced_pane: false,
        start_suppressed,
        placement: NewPanePlacement::Floating(None),
        near_current_pane: false,
        no_focus: false,
        tab_id: None,
    };
    let result = apply_action!(action, error_msg, env);

    // Extract pane_id from result and convert to yinz_types PaneId
    let pane_id: OpenFileInPlaceResponse = result.and_then(|r| r.affected_pane_id);

    // Write response to plugin
    let response = ProtobufOpenFileInPlaceResponse::from(pane_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_file_in_place response".to_string())
        .non_fatal();
}

fn open_file_near_plugin(
    env: &mut PluginEnv,
    file_to_open: FileToOpen,
    context: BTreeMap<String, String>,
) {
    let cwd = file_to_open
        .cwd
        .map(|cwd| translate_plugin_path(env, cwd))
        .or_else(|| Some(env.plugin_cwd.clone()));
    let path = translate_plugin_path(env, file_to_open.path);
    let open_file_payload = OpenFilePayload::new(path, file_to_open.line_number, cwd)
        .with_originating_plugin(env.originating_plugin(context));
    let title = format!("Editing: {}", open_file_payload.path.display());
    let start_suppressed = false;
    let open_file = TerminalAction::OpenFile(open_file_payload);

    // Create completion channel
    let (completion_tx, completion_rx) = oneshot::channel();
    let pty_instr = PtyInstruction::SpawnTerminal(
        Some(open_file),
        Some(title),
        NewPanePlacement::default(),
        start_suppressed,
        ClientTabIndexOrPaneId::PaneIdOrClient(PaneId::Plugin(env.plugin_id), env.client_id),
        Some(NotificationEnd::new(completion_tx)),
        false, // set_blocking
    );
    let _ = env.senders.send_to_pty(pty_instr);

    // Wait for completion
    let result = wait_for_action_completion(completion_rx, "open_file_near_plugin", false);
    let pane_id: OpenFileNearPluginResponse = result.affected_pane_id;

    // Write response to plugin
    let response = ProtobufOpenFileNearPluginResponse::from(pane_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_file_near_plugin response".to_string())
        .non_fatal();
}

fn open_file_floating_near_plugin(
    env: &mut PluginEnv,
    file_to_open: FileToOpen,
    floating_pane_coordinates: Option<FloatingPaneCoordinates>,
    context: BTreeMap<String, String>,
) {
    let cwd = file_to_open
        .cwd
        .map(|cwd| translate_plugin_path(env, cwd))
        .or_else(|| Some(env.plugin_cwd.clone()));
    let path = translate_plugin_path(env, file_to_open.path);
    let open_file_payload = OpenFilePayload::new(path, file_to_open.line_number, cwd)
        .with_originating_plugin(env.originating_plugin(context));
    let title = format!("Editing: {}", open_file_payload.path.display());
    let start_suppressed = false;
    let open_file = TerminalAction::OpenFile(open_file_payload);

    // Create completion channel
    let (completion_tx, completion_rx) = oneshot::channel();
    let pty_instr = PtyInstruction::SpawnTerminal(
        Some(open_file),
        Some(title),
        NewPanePlacement::Floating(floating_pane_coordinates),
        start_suppressed,
        ClientTabIndexOrPaneId::PaneIdOrClient(PaneId::Plugin(env.plugin_id), env.client_id),
        Some(NotificationEnd::new(completion_tx)),
        false, // set_blocking
    );
    let _ = env.senders.send_to_pty(pty_instr);

    // Wait for completion
    let result = wait_for_action_completion(completion_rx, "open_file_floating_near_plugin", false);
    let pane_id: OpenFileFloatingNearPluginResponse = result.affected_pane_id;

    // Write response to plugin
    let response = ProtobufOpenFileFloatingNearPluginResponse::from(pane_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_file_floating_near_plugin response".to_string())
        .non_fatal();
}

fn open_file_in_place_of_plugin(
    env: &mut PluginEnv,
    file_to_open: FileToOpen,
    close_plugin_after_replace: bool,
    context: BTreeMap<String, String>,
) {
    let cwd = file_to_open
        .cwd
        .map(|cwd| translate_plugin_path(env, cwd))
        .or_else(|| Some(env.plugin_cwd.clone()));
    let path = translate_plugin_path(env, file_to_open.path);
    let open_file_payload = OpenFilePayload::new(path, file_to_open.line_number, cwd)
        .with_originating_plugin(env.originating_plugin(context));
    let title = format!("Editing: {}", open_file_payload.path.display());
    let open_file = TerminalAction::OpenFile(open_file_payload);

    // Create completion channel
    let (completion_tx, completion_rx) = oneshot::channel();
    let pty_instr = PtyInstruction::SpawnInPlaceTerminal(
        Some(open_file),
        Some(title),
        close_plugin_after_replace,
        ClientTabIndexOrPaneId::PaneId(PaneId::Plugin(env.plugin_id)),
        Some(NotificationEnd::new(completion_tx)),
    );
    let _ = env.senders.send_to_pty(pty_instr);

    // Wait for completion
    let result = wait_for_action_completion(completion_rx, "open_file_in_place_of_plugin", false);
    let pane_id: OpenFileInPlaceOfPluginResponse = result.affected_pane_id;

    // Write response to plugin
    let response = ProtobufOpenFileInPlaceOfPluginResponse::from(pane_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_file_in_place_of_plugin response".to_string())
        .non_fatal();
}

fn open_terminal(env: &mut PluginEnv, cwd: PathBuf) {
    let error_msg = || format!("failed to open file in plugin {}", env.name());
    let cwd = translate_plugin_path(env, cwd);
    let mut default_shell = env.default_shell.clone().unwrap_or_else(|| {
        TerminalAction::RunCommand(RunCommand {
            command: env.path_to_default_shell.clone(),
            use_terminal_title: true,
            ..Default::default()
        })
    });
    default_shell.change_cwd(cwd);
    let run_command_action: Option<RunCommandAction> = match default_shell {
        TerminalAction::RunCommand(run_command) => Some(run_command.into()),
        _ => None,
    };
    let action = Action::NewTiledPane {
        direction: None,
        command: run_command_action,
        pane_name: None,
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        tab_id: None,
    };
    let result = apply_action!(action, error_msg, env);

    // Extract pane_id from result and convert to yinz_types PaneId
    let pane_id: OpenTerminalResponse = result.and_then(|r| r.affected_pane_id);

    // Write response to plugin
    let response = ProtobufOpenTerminalResponse::from(pane_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_terminal response".to_string())
        .non_fatal();
}

fn open_terminal_near_plugin(env: &mut PluginEnv, cwd: PathBuf) {
    let cwd = translate_plugin_path(env, cwd);
    let mut default_shell = env.default_shell.clone().unwrap_or_else(|| {
        TerminalAction::RunCommand(RunCommand {
            command: env.path_to_default_shell.clone(),
            use_terminal_title: true,
            ..Default::default()
        })
    });
    let name: Option<String> = None;
    default_shell.change_cwd(cwd);

    // Create completion channel
    let (completion_tx, completion_rx) = oneshot::channel();
    let _ = env.senders.send_to_pty(PtyInstruction::SpawnTerminal(
        Some(default_shell),
        name,
        NewPanePlacement::Tiled {
            direction: None,
            borderless: None,
        },
        false,
        ClientTabIndexOrPaneId::PaneIdOrClient(PaneId::Plugin(env.plugin_id), env.client_id),
        Some(NotificationEnd::new(completion_tx)),
        false, // set_blocking
    ));

    // Wait for completion
    let result = wait_for_action_completion(completion_rx, "open_terminal_near_plugin", false);
    let pane_id: OpenTerminalNearPluginResponse = result.affected_pane_id;

    // Write response to plugin
    let response = ProtobufOpenTerminalNearPluginResponse::from(pane_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_terminal_near_plugin response".to_string())
        .non_fatal();
}

fn open_terminal_floating(
    env: &mut PluginEnv,
    cwd: PathBuf,
    floating_pane_coordinates: Option<FloatingPaneCoordinates>,
) {
    let error_msg = || format!("failed to open file in plugin {}", env.name());
    let cwd = translate_plugin_path(env, cwd);
    let mut default_shell = env.default_shell.clone().unwrap_or_else(|| {
        TerminalAction::RunCommand(RunCommand {
            command: env.path_to_default_shell.clone(),
            use_terminal_title: true,
            ..Default::default()
        })
    });
    default_shell.change_cwd(cwd);
    let run_command_action: Option<RunCommandAction> = match default_shell {
        TerminalAction::RunCommand(run_command) => Some(run_command.into()),
        _ => None,
    };
    let action = Action::NewFloatingPane {
        command: run_command_action,
        pane_name: None,
        placement: NewPanePlacement::Floating(floating_pane_coordinates),
        near_current_pane: false,
        no_focus: false,
        tab_id: None,
    };
    let result = apply_action!(action, error_msg, env);

    // Extract pane_id from result and convert to yinz_types PaneId
    let pane_id: OpenTerminalFloatingResponse = result.and_then(|r| r.affected_pane_id);

    // Write response to plugin
    let response = ProtobufOpenTerminalFloatingResponse::from(pane_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_terminal_floating response".to_string())
        .non_fatal();
}

fn open_terminal_floating_near_plugin(
    env: &mut PluginEnv,
    cwd: PathBuf,
    floating_pane_coordinates: Option<FloatingPaneCoordinates>,
) {
    let cwd = translate_plugin_path(env, cwd);
    let mut default_shell = env.default_shell.clone().unwrap_or_else(|| {
        TerminalAction::RunCommand(RunCommand {
            command: env.path_to_default_shell.clone(),
            use_terminal_title: true,
            ..Default::default()
        })
    });
    default_shell.change_cwd(cwd);
    let name: Option<String> = None;

    // Create completion channel
    let (completion_tx, completion_rx) = oneshot::channel();
    let _ = env.senders.send_to_pty(PtyInstruction::SpawnTerminal(
        Some(default_shell),
        name,
        NewPanePlacement::Floating(floating_pane_coordinates),
        false,
        ClientTabIndexOrPaneId::PaneIdOrClient(PaneId::Plugin(env.plugin_id), env.client_id),
        Some(NotificationEnd::new(completion_tx)),
        false, // set_blocking
    ));

    // Wait for completion
    let result =
        wait_for_action_completion(completion_rx, "open_terminal_floating_near_plugin", false);
    let pane_id: OpenTerminalFloatingNearPluginResponse = result.affected_pane_id;

    // Write response to plugin
    let response = ProtobufOpenTerminalFloatingNearPluginResponse::from(pane_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_terminal_floating_near_plugin response".to_string())
        .non_fatal();
}

fn open_terminal_in_place(env: &mut PluginEnv, cwd: PathBuf) {
    let error_msg = || format!("failed to open file in plugin {}", env.name());
    let cwd = translate_plugin_path(env, cwd);
    let mut default_shell = env.default_shell.clone().unwrap_or_else(|| {
        TerminalAction::RunCommand(RunCommand {
            command: env.path_to_default_shell.clone(),
            use_terminal_title: true,
            ..Default::default()
        })
    });
    default_shell.change_cwd(cwd);
    let run_command_action: Option<RunCommandAction> = match default_shell {
        TerminalAction::RunCommand(run_command) => Some(run_command.into()),
        _ => None,
    };
    let action = Action::NewInPlacePane {
        command: run_command_action,
        pane_name: None,
        near_current_pane: false,
        no_focus: false,
        pane_id_to_replace: None,
        close_replaced_pane: false,
        tab_id: None,
    };
    let result = apply_action!(action, error_msg, env);

    // Extract pane_id from result and convert to yinz_types PaneId
    let pane_id: OpenTerminalInPlaceResponse = result.and_then(|r| r.affected_pane_id);

    // Write response to plugin
    let response = ProtobufOpenTerminalInPlaceResponse::from(pane_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_terminal_in_place response".to_string())
        .non_fatal();
}

fn open_terminal_in_place_of_plugin(
    env: &mut PluginEnv,
    cwd: PathBuf,
    close_plugin_after_replace: bool,
) {
    let cwd = translate_plugin_path(env, cwd);
    let mut default_shell = env.default_shell.clone().unwrap_or_else(|| {
        TerminalAction::RunCommand(RunCommand {
            command: env.path_to_default_shell.clone(),
            use_terminal_title: true,
            ..Default::default()
        })
    });
    default_shell.change_cwd(cwd);
    let name: Option<String> = None;

    // Create completion channel
    let (completion_tx, completion_rx) = oneshot::channel();
    let _ = env
        .senders
        .send_to_pty(PtyInstruction::SpawnInPlaceTerminal(
            Some(default_shell),
            name,
            close_plugin_after_replace,
            ClientTabIndexOrPaneId::PaneId(PaneId::Plugin(env.plugin_id)),
            Some(NotificationEnd::new(completion_tx)),
        ));

    // Wait for completion
    let result =
        wait_for_action_completion(completion_rx, "open_terminal_in_place_of_plugin", false);
    let pane_id: OpenTerminalInPlaceOfPluginResponse = result.affected_pane_id;

    // Write response to plugin
    let response = ProtobufOpenTerminalInPlaceOfPluginResponse::from(pane_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_terminal_in_place_of_plugin response".to_string())
        .non_fatal();
}

fn open_command_pane_in_place_of_plugin(
    env: &mut PluginEnv,
    command_to_run: CommandToRun,
    close_plugin_after_replace: bool,
    context: BTreeMap<String, String>,
) {
    let name: Option<String> = None;
    let Some(run_command_action) = run_command_action(env, command_to_run, false, context) else {
        return;
    };
    let run_cmd = TerminalAction::RunCommand(run_command_action.into());

    // Create completion channel
    let (completion_tx, completion_rx) = oneshot::channel();
    let _ = env
        .senders
        .send_to_pty(PtyInstruction::SpawnInPlaceTerminal(
            Some(run_cmd),
            name,
            close_plugin_after_replace,
            ClientTabIndexOrPaneId::PaneId(PaneId::Plugin(env.plugin_id)),
            Some(NotificationEnd::new(completion_tx)),
        ));

    // Wait for completion
    let result =
        wait_for_action_completion(completion_rx, "open_command_pane_in_place_of_plugin", false);
    let pane_id: OpenCommandPaneInPlaceOfPluginResponse = result.affected_pane_id;

    // Write response to plugin
    let response = ProtobufOpenCommandPaneInPlaceOfPluginResponse::from(pane_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| {
            "failed to write open_command_pane_in_place_of_plugin response".to_string()
        })
        .non_fatal();
}

fn open_terminal_pane_in_place_of_pane_id(
    env: &mut PluginEnv,
    pane_id_to_replace: yinz_types::data::PaneId,
    cwd: FileToOpen,
    close_replaced_pane: bool,
) {
    let cwd_path = translate_plugin_path(env, cwd.path);
    let mut default_shell = env.default_shell.clone().unwrap_or_else(|| {
        TerminalAction::RunCommand(RunCommand {
            command: env.path_to_default_shell.clone(),
            use_terminal_title: true,
            ..Default::default()
        })
    });
    default_shell.change_cwd(cwd_path);
    let name: Option<String> = None;

    let (completion_tx, completion_rx) = oneshot::channel();
    let _ = env
        .senders
        .send_to_pty(PtyInstruction::SpawnInPlaceTerminal(
            Some(default_shell),
            name,
            close_replaced_pane,
            ClientTabIndexOrPaneId::PaneId(pane_id_to_replace),
            Some(NotificationEnd::new(completion_tx)),
        ));

    let result = wait_for_action_completion(
        completion_rx,
        "open_terminal_pane_in_place_of_pane_id",
        false,
    );
    let pane_id: OpenTerminalPaneInPlaceOfPaneIdResponse = result.affected_pane_id;

    let response = ProtobufOpenTerminalPaneInPlaceOfPaneIdResponse::from(pane_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| {
            "failed to write open_terminal_pane_in_place_of_pane_id response".to_string()
        })
        .non_fatal();
}

fn open_command_pane_in_place_of_pane_id(
    env: &mut PluginEnv,
    pane_id_to_replace: yinz_types::data::PaneId,
    command_to_run: CommandToRun,
    close_replaced_pane: bool,
    context: BTreeMap<String, String>,
) {
    let name: Option<String> = None;
    let Some(run_command_action) = run_command_action(env, command_to_run, false, context) else {
        return;
    };
    let run_cmd = TerminalAction::RunCommand(run_command_action.into());

    let (completion_tx, completion_rx) = oneshot::channel();
    let _ = env
        .senders
        .send_to_pty(PtyInstruction::SpawnInPlaceTerminal(
            Some(run_cmd),
            name,
            close_replaced_pane,
            ClientTabIndexOrPaneId::PaneId(pane_id_to_replace),
            Some(NotificationEnd::new(completion_tx)),
        ));

    let result = wait_for_action_completion(
        completion_rx,
        "open_command_pane_in_place_of_pane_id",
        false,
    );
    let pane_id: OpenCommandPaneInPlaceOfPaneIdResponse = result.affected_pane_id;

    let response = ProtobufOpenCommandPaneInPlaceOfPaneIdResponse::from(pane_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| {
            "failed to write open_command_pane_in_place_of_pane_id response".to_string()
        })
        .non_fatal();
}

fn open_edit_pane_in_place_of_pane_id(
    env: &mut PluginEnv,
    pane_id_to_replace: yinz_types::data::PaneId,
    file_to_open: FileToOpen,
    close_replaced_pane: bool,
    context: BTreeMap<String, String>,
) {
    let cwd = file_to_open
        .cwd
        .map(|cwd| translate_plugin_path(env, cwd))
        .or_else(|| Some(env.plugin_cwd.clone()));
    let path = translate_plugin_path(env, file_to_open.path);
    let open_file_payload = OpenFilePayload::new(path, file_to_open.line_number, cwd)
        .with_originating_plugin(env.originating_plugin(context));
    let title = format!("Editing: {}", open_file_payload.path.display());
    let open_file = TerminalAction::OpenFile(open_file_payload);

    let (completion_tx, completion_rx) = oneshot::channel();
    let pty_instr = PtyInstruction::SpawnInPlaceTerminal(
        Some(open_file),
        Some(title),
        close_replaced_pane,
        ClientTabIndexOrPaneId::PaneId(pane_id_to_replace),
        Some(NotificationEnd::new(completion_tx)),
    );
    let _ = env.senders.send_to_pty(pty_instr);

    let result =
        wait_for_action_completion(completion_rx, "open_edit_pane_in_place_of_pane_id", false);
    let pane_id: OpenEditPaneInPlaceOfPaneIdResponse = result.affected_pane_id;

    let response = ProtobufOpenEditPaneInPlaceOfPaneIdResponse::from(pane_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_edit_pane_in_place_of_pane_id response".to_string())
        .non_fatal();
}

fn open_command_pane(
    env: &mut PluginEnv,
    command_to_run: CommandToRun,
    context: BTreeMap<String, String>,
) {
    let error_msg = || format!("failed to open command in plugin {}", env.name());
    let direction: Option<Direction> = None;
    let name: Option<String> = None;
    let Some(run_command_action) = run_command_action(env, command_to_run, false, context) else {
        return;
    };
    let action = Action::NewTiledPane {
        direction,
        command: Some(run_command_action),
        pane_name: name,
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        tab_id: None,
    };
    let result = apply_action!(action, error_msg, env);

    // Extract pane_id from result and convert to yinz_types PaneId
    let pane_id: OpenCommandPaneResponse = result.and_then(|r| r.affected_pane_id);

    // Write response to plugin
    let response = ProtobufOpenCommandPaneResponse::from(pane_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_command_pane response".to_string())
        .non_fatal();
}

fn open_command_pane_near_plugin(
    env: &mut PluginEnv,
    command_to_run: CommandToRun,
    context: BTreeMap<String, String>,
) {
    let pane_id =
        spawn_command_pane_near_plugin(env, command_to_run, context, None, false, false).ok();
    let response = ProtobufOpenCommandPaneNearPluginResponse::from(pane_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_command_pane_near_plugin response".to_string())
        .non_fatal();
}

fn spawn_command_pane_near_plugin(
    env: &PluginEnv,
    command_to_run: CommandToRun,
    context: BTreeMap<String, String>,
    name: Option<String>,
    cwd_already_resolved: bool,
    start_suppressed: bool,
) -> Result<PaneId, String> {
    let agent = context.contains_key(AGENT_PANE_CONTEXT);
    let preserve_cwd = cwd_already_resolved && !command_to_run.target.is_remote();
    // Checkout launchers already resolved host paths, including names like /tmp and /host.
    let run_command_action = if preserve_cwd {
        command_action_with_resolved_cwd(env, command_to_run, context)
    } else {
        run_command_action(env, command_to_run, false, context)
    }
    .ok_or_else(|| "invalid pane command".to_owned())?;
    let run_cmd = TerminalAction::RunCommand(run_command_action.into());

    // Create completion channel
    let (completion_tx, completion_rx) = oneshot::channel();
    let mut completion = NotificationEnd::new(completion_tx);
    completion.wait_for_agent_start = agent;
    env.senders
        .send_to_pty(PtyInstruction::SpawnTerminal(
            Some(run_cmd),
            name,
            NewPanePlacement::Tiled {
                direction: None,
                borderless: None,
            },
            start_suppressed,
            ClientTabIndexOrPaneId::PaneIdOrClient(PaneId::Plugin(env.plugin_id), env.client_id),
            Some(completion),
            false, // set_blocking
        ))
        .map_err(|error| format!("failed to request pane: {error:#}"))?;

    // Wait for completion
    let result = if agent {
        wait_for_agent_placement(
            completion_rx,
            &AgentLaunchResources::default(),
            Duration::from_secs(120),
        )
        .map_err(|error| error.to_string())?
    } else {
        wait_for_action_completion(completion_rx, "open_command_pane_near_plugin", false)
    };
    if let Some(error) = result.error_message {
        return Err(error);
    }
    result
        .affected_pane_id
        .ok_or_else(|| "Yinz did not open the agent pane".to_owned())
}

fn open_command_pane_in_git_checkout_near_plugin(
    env: &mut PluginEnv,
    mut command_to_run: CommandToRun,
    source_directory: PathBuf,
    checkout: GitCheckout,
) {
    let result: OpenCommandPaneInGitCheckoutResponse =
        prepare_git_checkout(env, source_directory, checkout).and_then(|cwd| {
            command_to_run.cwd = Some(cwd);
            // The marker's value is the agent's kind - the program - which is what the pane's
            // agent record starts from. See `agent_from_run` in `panes::terminal_pane`.
            let kind = yinz_types::agent::agent_kind_of(&command_to_run.path, &command_to_run.args)
                .unwrap_or_else(|| "true".to_owned());
            spawn_command_pane_near_plugin(
                env,
                command_to_run,
                BTreeMap::from([(AGENT_PANE_CONTEXT.to_owned(), kind)]),
                None,
                true,
                false,
            )
        });
    let response = ProtobufOpenCommandPaneInGitCheckoutResponse::from(result);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_command_pane_in_git_checkout response".to_string())
        .non_fatal();
}

/// Start a coding agent near this plugin: the launch planner every agent launcher uses, the
/// checkout the spec asks for, the pane, and the agent record - one road for agent's
/// `agent_spawn`, the `new-agent` modal and a TypeScript plugin's `yinz.agent.spawn`.
fn spawn_agent(
    env: &mut PluginEnv,
    mut spec: AgentSpawnSpec,
    source_directory: Option<PathBuf>,
    cwd: Option<PathBuf>,
    name: Option<String>,
    mut context: BTreeMap<String, String>,
    start_suppressed: bool,
) {
    let result: SpawnAgentResponse = (|| {
        if let Some(owner) = context.get("swamp_owner") {
            if owner.is_empty() || owner.len() > 4096 {
                return Err("invalid workflow ownership".into());
            }
            spec.env.insert("YINZ_SWAMP_OWNER".into(), owner.clone());
        }
        context.remove("agent_workflow_owner");
        if let Some(role) = context.get("agent_workflow_role") {
            if !["message", "repair"].contains(&role.as_str())
                || context
                    .get("agent_workflow_run")
                    .is_none_or(|run| run.is_empty())
            {
                return Err(
                    "workflow helpers require a run identifier and message or repair role".into(),
                );
            }
            if spec.branch.is_some() || spec.worktree == Some(true) {
                return Err("workflow helpers must run in the existing host checkout".into());
            }
            context.insert("agent_workflow_owner".into(), env.plugin_id.to_string());
        }
        let host_workflow = context.contains_key("agent_workflow_role");
        let host_paths = host_workflow
            || (env.plugin_config.is_builtin()
                && (context.contains_key("swamp_owner")
                    || context
                        .get("agent_agent_host_cwd")
                        .is_some_and(|value| value == "true")));
        let source = source_directory
            .map(|path| {
                if host_paths && path.is_absolute() {
                    path
                } else {
                    translate_plugin_path(env, path)
                }
            })
            .unwrap_or_else(|| env.plugin_cwd.clone());
        let cwd = cwd.map(|path| {
            if host_paths && path.is_absolute() {
                path
            } else {
                translate_plugin_path(env, path)
            }
        });
        let prepared = prepare_agent(&spec, source, cwd, host_workflow, &env.data_access)
            .map_err(|error| error.to_string())?;
        let resources = prepared.resources();
        let kind = yinz_types::agent::agent_kind_of(&prepared.command.path, &prepared.command.args)
            .unwrap_or_else(|| "true".to_owned());
        context.insert(AGENT_PANE_CONTEXT.to_owned(), kind);
        spawn_command_pane_near_plugin(env, prepared.command, context, name, true, start_suppressed)
            .map(|pane_id| {
                if let Some(cwd) = &resources.cwd {
                    prewarm_agent_workflows(cwd.clone());
                }
                SpawnedAgent {
                    pane_id,
                    branch: resources.branch,
                    cwd: resources.cwd,
                    ..Default::default()
                }
            })
    })();
    let response = ProtobufSpawnAgentResponse::from(result);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write spawn_agent response".to_string())
        .non_fatal();
}

#[derive(Debug)]
struct PreparedAgent {
    command: CommandToRun,
    branch: Option<String>,
    cwd: PathBuf,
    /// The branch asked for, when `branch` is a different one.
    requested_branch: Option<String>,
    reused: bool,
}

impl PreparedAgent {
    fn resources(&self) -> AgentLaunchResources {
        AgentLaunchResources {
            tab_id: None,
            created_tab_id: None,
            cwd: Some(self.cwd.clone()),
            branch: self.branch.clone(),
            requested_branch: self.requested_branch.clone(),
            reused: self.reused,
        }
    }
}

#[derive(Debug)]
struct AgentSpawnError(Box<AgentResponse>);

impl AgentSpawnError {
    fn failed(message: impl Into<String>, resources: Option<AgentLaunchResources>) -> Self {
        let mut response = AgentResponse::new(AgentOutcome::Failed);
        response.message = Some(format!(
            "{}; inspect the launch resources and agent list before retrying",
            message.into()
        ));
        response.launch_resources = resources;
        Self(Box::new(response))
    }

    fn into_response(self) -> AgentResponse {
        *self.0
    }
}

impl From<String> for AgentSpawnError {
    fn from(message: String) -> Self {
        let mut response = AgentResponse::new(AgentOutcome::Rejected);
        response.message = Some(message);
        Self(Box::new(response))
    }
}

impl From<&str> for AgentSpawnError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

impl std::fmt::Display for AgentSpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.message.as_deref().unwrap_or("agent launch failed"))
    }
}

/// Shared checkout preparation for plugin and direct CLI launches. Paths are already host paths.
fn prepare_agent(
    spec: &AgentSpawnSpec,
    source: PathBuf,
    cwd: Option<PathBuf>,
    host_workflow: bool,
    data_access: &DataAccess,
) -> Result<PreparedAgent, AgentSpawnError> {
    let (mut command, mut checkout) = yinz_runtime::agent::launch::plan(spec)?;
    let bubblewrap = if spec.bubblewrap {
        if !command.target.is_local() {
            return Err("Bubblewrap cannot be combined with SSH".into());
        }
        let validated_cwd = match &checkout {
            GitCheckout::Current => cwd.as_deref().unwrap_or(&source),
            _ => &source,
        }
        .canonicalize()
        .map_err(|error| format!("resolve Bubblewrap launch directory: {error}"))?;
        let environment =
            yinz_config::project_config::bubblewrap::Environment::for_project(&validated_cwd)
                .map_err(|error| format!("{error:#}"))?;
        environment
            .preflight()
            .map_err(|error| format!("{error:#}"))?;
        environment
            .validate_spawn_env(&spec.env)
            .map_err(|error| format!("{error:#}"))?;
        Some((environment, validated_cwd))
    } else {
        None
    };

    apply_project_worktree_dir(&mut checkout, spec.worktree_dir.as_deref(), &source, || {
        data_access.plugin_data().fetch(
            project_data::store::SHARED,
            &FetchRequest::entity("project").limit(project_data::store::LIST_FETCH_LIMIT),
        )
    })?;
    let requested_branch = match &checkout {
        GitCheckout::NewBranch { branch, .. } | GitCheckout::NewWorktree { branch, .. } => {
            Some(branch.clone())
        },
        GitCheckout::Current => None,
    };
    // An agent loose in the checkout the user is working in - or in a folder that is no
    // repository at all - is a thing to ask for, not a thing to fall into by leaving
    // `branch` out. The resolved folder is what is checked, not the spec: a branch held
    // by another worktree sends the agent there, and that is a worktree.
    //
    // Workflow helpers are the exception, and were before this: they are *required* to
    // run in the existing host checkout (see above), so refusing them for it would leave
    // them nowhere to run.
    let guard = if spec.allow_outside_worktree.unwrap_or(false) || host_workflow {
        WorktreeGuard::Waived
    } else {
        WorktreeGuard::Required
    };
    // Resolve reporting assets before Git changes, so an unwritable data directory cannot
    // strand a fresh checkout. A reused opening prompt only changes the final command text.
    yinz_runtime::agent::pi::prepare_command(&mut command)?;
    let mut attempted = None;
    let prepared = agent_folder_tracked(
        checkout,
        cwd,
        source,
        spec.reuse.unwrap_or(false),
        guard,
        &mut attempted,
        bubblewrap
            .as_ref()
            .map(|(environment, _)| environment.root()),
    )
    .map_err(|error| match attempted {
        Some(resources) => AgentSpawnError::failed(error, Some(resources)),
        None => error.into(),
    })?;
    let resources = AgentLaunchResources {
        tab_id: None,
        created_tab_id: None,
        cwd: Some(prepared.path.clone()),
        branch: prepared.branch.clone(),
        requested_branch: requested_branch
            .clone()
            .filter(|requested| prepared.branch.as_ref() != Some(requested)),
        reused: prepared.reused_worktree,
    };
    if prepared.reused_worktree && spec.reuse_prompt.is_some() {
        command = prepared
            .agent_command(spec)
            .map_err(|error| AgentSpawnError::failed(error, Some(resources.clone())))?;
        yinz_runtime::agent::pi::prepare_command(&mut command)
            .map_err(|error| AgentSpawnError::failed(error, Some(resources.clone())))?;
    }
    let reused = prepared.reused_worktree;
    let branch = prepared.branch.or_else(|| {
        worktree_at(&prepared.path, &prepared.path)
            .ok()
            .flatten()
            .and_then(|worktree| worktree.branch)
    });
    let folder = prepared.path;
    command.cwd = Some(folder.clone());
    if let Some((environment, validated_cwd)) = bubblewrap {
        // Only a different destination needs another check here; launch and exec revalidate it.
        if folder.canonicalize().ok().as_ref() != Some(&validated_cwd) {
            environment
                .validate_cwd(&folder)
                .map_err(|error| AgentSpawnError::failed(format!("{error:#}"), Some(resources)))?;
        }
        command.target = PaneTarget::Bubblewrap {
            root: environment.root().to_path_buf(),
        };
    }
    let requested_branch = requested_branch.filter(|requested| branch.as_ref() != Some(requested));
    Ok(PreparedAgent {
        command,
        branch,
        cwd: folder,
        requested_branch,
        reused,
    })
}

fn prewarm_agent_workflows(folder: PathBuf) {
    get_tokio_runtime().spawn_blocking(move || {
        if let Ok(config) = yinz_config::project_config::access(&folder, None) {
            if config.error.is_none() && config.config.workflow.is_some() {
                yinz_swamp::prewarm(&config.root).non_fatal();
            }
        }
    });
}

/// A CLI-spawned agent pane's title, such as `claude (haiku)`.
///
/// Without one the title is the whole command line, which for claude carries kilobytes of
/// hook wiring. `None` for a raw command, which keeps that default.
fn agent_pane_title(spec: &AgentSpawnSpec) -> Option<String> {
    let nonblank = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let kind = yinz_runtime::agent::launch::kind_for_spec(spec)?;
    Some(match nonblank(&spec.model) {
        Some(model) => format!("{kind} ({model})"),
        None => kind,
    })
}

/// Launch an agent for a direct CLI request, using the same planner and checkout policy as plugins.
pub fn spawn_agent_for_cli(
    spec: AgentSpawnSpec,
    cwd: Option<PathBuf>,
    project: Option<String>,
    remote_host: Option<String>,
    client_id: crate::ClientId,
    senders: &ThreadSenders,
    data_access: &DataAccess,
) -> SpawnAgentResponse {
    spawn_agent_for_control(
        yinz_types::agent::control::AgentPlacement::Automatic,
        spec,
        cwd,
        project,
        remote_host,
        client_id,
        senders,
        data_access,
    )
    .map_err(|error| error.to_string())
}

/// Where an agent's pane goes unless the launch says otherwise.
const TILED: NewPanePlacement = NewPanePlacement::Tiled {
    direction: None,
    borderless: None,
};

fn spawn_agent_for_control(
    placement: yinz_types::agent::control::AgentPlacement,
    spec: AgentSpawnSpec,
    cwd: Option<PathBuf>,
    project: Option<String>,
    remote_host: Option<String>,
    client_id: crate::ClientId,
    senders: &ThreadSenders,
    data_access: &DataAccess,
) -> Result<SpawnedAgent, AgentSpawnError> {
    spawn_agent_prepared(placement, false, spec, client_id, senders, |spec| {
        agent_control::prepare_automation_agent(spec, cwd, project, remote_host, data_access)
    })
}

/// `placement` picks the tab; mother goes on the overlay surface. Only a tiled pane may
/// be redirected to a roomier tab (`Screen::agent_pane_target`).
fn spawn_agent_prepared(
    placement: yinz_types::agent::control::AgentPlacement,
    is_mother: bool,
    mut spec: AgentSpawnSpec,
    client_id: crate::ClientId,
    senders: &ThreadSenders,
    prepare: impl FnOnce(&mut AgentSpawnSpec) -> Result<PreparedAgent, AgentSpawnError>,
) -> Result<SpawnedAgent, AgentSpawnError> {
    let surface = if is_mother {
        NewPanePlacement::Overlay(OverlayPlacement::mother())
    } else {
        TILED
    };
    let started = Instant::now();
    if spec.prompt.is_some() || spec.reuse_prompt.is_some() {
        return Err(
            "spawn does not accept prompt or reuse_prompt; spawn, wait, then prompt".into(),
        );
    }
    use yinz_types::agent::control::AgentPlacement;
    placement.validate()?;
    let mut destination = match &placement {
        AgentPlacement::Tab(tab_id) => Some(
            resolve_agent_tab(senders, placement.clone(), None, client_id).map_err(|error| {
                let mut failure = AgentSpawnError::from(error);
                failure.0.launch_resources = Some(AgentLaunchResources {
                    tab_id: Some(*tab_id),
                    ..Default::default()
                });
                failure
            })?,
        ),
        _ => None,
    };
    let prepared = prepare(&mut spec).map_err(|mut failure| {
        if let Some(destination) = &destination {
            failure
                .0
                .launch_resources
                .get_or_insert_with(Default::default)
                .tab_id = Some(destination.tab_id);
        }
        failure
    })?;
    let mut resources = prepared.resources();
    resources.tab_id = destination.as_ref().map(|target| target.tab_id);
    let command = prepared.command;
    let title = if is_mother {
        yinz_runtime::agent::launch::kind_for_spec(&spec).map(|kind| format!("mother: {kind}"))
    } else {
        agent_pane_title(&spec)
    };
    let action = RunCommandAction {
        command: command.path,
        args: command.args,
        cwd: command.cwd,
        hold_on_close: true,
        target: command.target,
        remote_default_shell: command.remote_default_shell,
        ..Default::default()
    };
    let action: RunCommand = action.into();
    if matches!(placement, AgentPlacement::NewTab(_)) {
        destination = Some(
            resolve_agent_tab(
                senders,
                placement,
                Some(Run::Command(action.clone())),
                client_id,
            )
            .map_err(|error| AgentSpawnError::failed(error, Some(resources.clone())))?,
        );
        resources.tab_id = destination.as_ref().map(|target| target.tab_id);
        resources.created_tab_id = resources.tab_id;
    }
    let (completion_tx, completion_rx) = oneshot::channel();
    let mut completion = NotificationEnd::new(completion_tx);
    completion.wait_for_agent_start = true;
    completion.is_mother = is_mother;
    completion.agent_tab = destination.clone();
    completion.created_tab_id = resources.created_tab_id;
    let target = destination.as_ref().map_or(
        ClientTabIndexOrPaneId::ClientIdNoFocus(client_id),
        |destination| ClientTabIndexOrPaneId::TabIndexNoFocus(destination.tab_id),
    );
    senders
        .send_to_pty(PtyInstruction::SpawnTerminal(
            Some(TerminalAction::RunCommand(action)),
            title,
            surface,
            false,
            target,
            Some(completion),
            false,
        ))
        .map_err(|error| {
            AgentSpawnError::failed(
                format!(
                    "failed to request agent pane: {error:#}{}",
                    cleanup_agent_tab(senders, destination.as_ref(), &mut resources)
                ),
                Some(resources.clone()),
            )
        })?;
    let result = wait_for_agent_placement(
        completion_rx,
        &resources,
        Duration::from_secs(120).saturating_sub(started.elapsed()),
    )?;
    destination = destination.or(result.agent_tab);
    resources.tab_id = resources
        .tab_id
        .or(destination.as_ref().map(|target| target.tab_id));
    resources.created_tab_id = resources.created_tab_id.or(result.created_tab_id);
    let definitive_failure = result.error_message.is_some();
    let error = result.error_message.or_else(|| {
        if result.affected_pane_id.is_none() {
            Some("agent pane creation outcome is unknown".into())
        } else if result.affected_tab_id.is_none() {
            Some("agent destination outcome is unknown".into())
        } else {
            None
        }
    });
    if let Some(error) = error {
        let cleanup = if definitive_failure {
            cleanup_agent_tab(senders, destination.as_ref(), &mut resources)
        } else {
            String::new()
        };
        let mut failure =
            AgentSpawnError::failed(format!("{error}{cleanup}"), Some(resources.clone()));
        if let Some(pane_id) = result.affected_pane_id {
            failure.0.spawned = Some(SpawnedAgent {
                pane_id,
                tab_id: result.affected_tab_id,
                created_tab_id: resources.created_tab_id,
                branch: resources.branch.clone(),
                cwd: resources.cwd.clone(),
                requested_branch: resources.requested_branch.clone(),
                reused: resources.reused,
                occupied_by: Vec::new(),
            });
            if resources.tab_id.is_none() && resources.created_tab_id.is_none() {
                failure.0.launch_resources = None;
            }
        }
        return Err(failure);
    }
    let pane_id = result.affected_pane_id.ok_or_else(|| {
        AgentSpawnError::failed(
            "agent pane creation outcome is unknown",
            Some(resources.clone()),
        )
    })?;
    let tab_id = result.affected_tab_id.ok_or_else(|| {
        AgentSpawnError::failed(
            "agent destination outcome is unknown",
            Some(resources.clone()),
        )
    })?;
    if let Some(cwd) = &resources.cwd {
        prewarm_agent_workflows(cwd.clone());
    }
    Ok(SpawnedAgent {
        tab_id: Some(tab_id),
        created_tab_id: resources.created_tab_id,
        pane_id,
        branch: resources.branch,
        cwd: resources.cwd,
        requested_branch: resources.requested_branch,
        reused: resources.reused,
        occupied_by: Vec::new(),
    })
}

/// A timeout ends the caller's wait, not a process or layout that may still complete.
fn wait_for_agent_placement(
    receiver: oneshot::Receiver<yinz_bus::route::ActionCompletionResult>,
    resources: &AgentLaunchResources,
    timeout: Duration,
) -> Result<yinz_bus::route::ActionCompletionResult, AgentSpawnError> {
    match get_tokio_runtime().block_on(async { tokio::time::timeout(timeout, receiver).await }) {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(_)) => Err(AgentSpawnError::failed(
            "agent placement outcome is unknown",
            Some(resources.clone()),
        )),
        Err(_) => {
            let mut failure = AgentSpawnError::failed(
                "agent launch timed out; outcome is unknown and a late placement may still complete",
                Some(resources.clone()),
            );
            failure.0.outcome = AgentOutcome::TimedOut;
            Err(failure)
        },
    }
}

/// Resolve existing destinations before checkout preparation; create new ones afterwards.
fn resolve_agent_tab(
    senders: &ThreadSenders,
    placement: yinz_types::agent::control::AgentPlacement,
    invoked_with: Option<Run>,
    client_id: crate::ClientId,
) -> Result<yinz_bus::route::AgentTabPlacement, String> {
    let (reply, receiver) = std::sync::mpsc::channel();
    senders
        .send_to_screen(ScreenInstruction::ResolveAgentTab {
            placement,
            invoked_with,
            client_id,
            reply,
        })
        .map_err(|error| format!("cannot request agent destination: {error:#}"))?;
    receiver.recv().map_err(|error| {
        format!("agent destination outcome unknown: {error}; inspect tabs before retrying")
    })?
}

fn cleanup_agent_tab(
    senders: &ThreadSenders,
    destination: Option<&yinz_bus::route::AgentTabPlacement>,
    resources: &mut AgentLaunchResources,
) -> String {
    let Some(destination) = destination.filter(|target| target.created) else {
        return if resources.created_tab_id.is_some() {
            "; created tab retained; inspect it before retrying".into()
        } else {
            String::new()
        };
    };
    let (reply, receiver) = std::sync::mpsc::channel();
    let cleaned = senders
        .send_to_screen(ScreenInstruction::CleanupAgentTab(
            destination.clone(),
            reply,
        ))
        .is_ok()
        && receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap_or(false);
    if cleaned {
        resources.created_tab_id = None;
        "; launch-created tab is closed".into()
    } else {
        "; launch-created tab retained or cleanup uncertain; inspect it before retrying".into()
    }
}

/// Read saved defaults only for a new worktree with no launch override. Fetching all
/// columns also works before the nullable setting has been added to an old registry.
fn apply_project_worktree_dir(
    checkout: &mut GitCheckout,
    explicit: Option<&str>,
    source: &std::path::Path,
    fetch: impl FnOnce() -> Result<yinz_types::data::DataRows, DataError>,
) -> Result<(), String> {
    let GitCheckout::NewWorktree { branch, path, .. } = checkout else {
        return Ok(());
    };
    if explicit.is_some() {
        return Ok(());
    }
    let rows = match fetch() {
        Ok(rows) => rows,
        Err(DataError::UnknownEntity(_)) => return Ok(()),
        Err(error) => {
            return Err(format!(
                "could not read project worktree directory: {error}"
            ))
        },
    };
    let mut projects = project_data::projects::from_rows(&rows);
    // Match filesystem identities before lexical containment, including /var aliases on macOS.
    for project in projects.values_mut() {
        if let Ok(root) = std::fs::canonicalize(&project.root) {
            project.root = root.to_string_lossy().into_owned();
        }
    }
    let source = source.canonicalize().unwrap_or_else(|_| source.to_owned());
    let location = project_data::location::Location::local(source.to_string_lossy());
    let project = match project_data::projects::for_cwd(&projects, &location) {
        Some(project) => Some(project),
        None if projects.is_empty() => None,
        None => {
            let checkouts = worktrees(&source)?;
            let (primary, linked) = checkouts
                .split_first()
                .ok_or_else(|| "git could not find the primary worktree".to_owned())?;
            project_data::projects::for_cwd(
                &projects,
                &project_data::location::Location::local(primary.path.to_string_lossy()),
            )
            .or_else(|| {
                let mut matches = projects.values().filter(|project| {
                    linked.iter().any(|checkout| {
                        same_path(std::path::Path::new(&project.root), &checkout.path)
                    })
                });
                let project = matches.next()?;
                // Several linked roots can be registered with different defaults.
                matches.next().is_none().then_some(project)
            })
        },
    };
    if let Some(dir) = project.and_then(|project| project.worktree_dir.as_deref()) {
        if dir.contains('\0') {
            return Err("worktree_dir must not contain NUL bytes".to_owned());
        }
        *path = yinz_runtime::agent::launch::worktree_path_in(Some(dir), branch).into();
    }
    Ok(())
}

/// Where a spawned agent's pane runs. A branch means git work in `source`, the repository, and
/// the pane lands in the checkout that makes. Without one there is no git work at all: the
/// pane runs in `cwd` when the caller named one, and otherwise in `source` as it stands -
/// agent sends the focused pane's folder, a TypeScript plugin's default is its own - never the
/// repository root, which `command=zsh` in a subfolder of a checkout does not want and which
/// costs a `git rev-parse` to find.
#[cfg(test)]
fn agent_folder(
    checkout: GitCheckout,
    cwd: Option<PathBuf>,
    source: PathBuf,
    reuse: bool,
    guard: WorktreeGuard,
) -> Result<PreparedCheckout, String> {
    agent_folder_tracked(checkout, cwd, source, reuse, guard, &mut None, None)
}

fn validate_checkout_root(path: &Path, root: Option<&Path>) -> Result<(), String> {
    if let Some(root) = root {
        let resolved = resolve_fs_ancestors(path).map_err(|error| format!("{error:#}"))?;
        if !resolved.starts_with(root) {
            return Err(format!(
                "Bubblewrap checkout must be inside {}",
                root.display()
            ));
        }
    }
    Ok(())
}

fn agent_folder_tracked(
    checkout: GitCheckout,
    cwd: Option<PathBuf>,
    source: PathBuf,
    reuse: bool,
    guard: WorktreeGuard,
    attempted: &mut Option<AgentLaunchResources>,
    bubblewrap_root: Option<&Path>,
) -> Result<PreparedCheckout, String> {
    match (checkout, cwd) {
        (GitCheckout::Current, Some(cwd)) => {
            guard.check(&cwd)?;
            Ok(PreparedCheckout {
                reused_worktree: false,
                path: cwd,
                branch: None,
            })
        },
        (GitCheckout::Current, None) => {
            guard.check(&source)?;
            Ok(PreparedCheckout {
                reused_worktree: false,
                path: source,
                branch: None,
            })
        },
        (checkout, _) => prepare_git_checkout_in_tracked(
            source,
            checkout,
            reuse,
            guard,
            attempted,
            bubblewrap_root,
        ),
    }
}

/// Whether the folder an agent is about to be left in has to be a *linked* worktree.
///
/// Asked at each point where a destination is settled and **before any git work is done**,
/// rather than of the folder a finished checkout answers with: a refusal that has already
/// switched the user's checkout onto a new branch has left them worse off than the spawn
/// they asked for.
#[derive(Clone, Copy, PartialEq)]
enum WorktreeGuard {
    Required,
    Waived,
}

impl WorktreeGuard {
    fn check(self, path: &std::path::Path) -> Result<(), String> {
        if self == WorktreeGuard::Waived || linked_worktree(path)? {
            return Ok(());
        }
        Err(format!(
            "{} is not a linked git worktree; give a branch to cut one, or set \
             allow_outside_worktree",
            path.display()
        ))
    }
}

fn prepare_git_checkout(
    env: &PluginEnv,
    source_directory: PathBuf,
    checkout: GitCheckout,
) -> Result<PathBuf, String> {
    let source_directory = translate_plugin_path(env, source_directory);
    // Reuse, because that is what this call has always meant and what a plugin opening a
    // command pane in a checkout asks for: the branch it names is the work it is pointing
    // at. Only `spawn_agent` asks for the other thing, and it says so.
    // No guard: this command has never had one. It puts a *plugin's* command pane in a
    // checkout rather than leaving an agent loose in one.
    prepare_git_checkout_in(source_directory, checkout, true, WorktreeGuard::Waived)
        .map(|prepared| prepared.path)
}

/// A checkout the host has made or found: the folder to run in, and the branch it holds when
/// there is one. The branch is answered rather than assumed because it is not always the one
/// asked for - a taken name is cut afresh with a suffix.
#[derive(Debug)]
struct PreparedCheckout {
    path: PathBuf,
    branch: Option<String>,
    reused_worktree: bool,
}

impl PreparedCheckout {
    fn agent_command(&self, spec: &AgentSpawnSpec) -> Result<CommandToRun, String> {
        let mut spec = spec.clone();
        if self.reused_worktree {
            if let Some(prompt) = spec.reuse_prompt.take() {
                spec.prompt = Some(prompt);
            }
        }
        yinz_runtime::agent::launch::plan(&spec).map(|(command, _)| command)
    }
}

/// Put the repository into the state the request describes, and answer with the
/// folder the command should run in.
///
/// With `reuse`, "new" in [`GitCheckout::NewBranch`] and [`GitCheckout::NewWorktree`]
/// means *the one asked for*, created if it is not there yet: a branch or a worktree
/// that already exists is the work the caller is pointing at, and the agent belongs in
/// it. Only a path that exists without being a worktree of this repository still fails,
/// because that is a genuine clash rather than something to reuse.
///
/// Without it, a name already taken is a collision and the next free one is cut instead -
/// `<branch>-2`, `<branch>-3`, and the worktree renamed with it, so the two never come
/// apart. Two agents asked for the same branch by two different people is the case this
/// is for: joining them silently is how one of them loses work.
fn prepare_git_checkout_in(
    source_directory: PathBuf,
    checkout: GitCheckout,
    reuse: bool,
    guard: WorktreeGuard,
) -> Result<PreparedCheckout, String> {
    prepare_git_checkout_in_tracked(source_directory, checkout, reuse, guard, &mut None, None)
}

fn prepare_git_checkout_in_tracked(
    source_directory: PathBuf,
    checkout: GitCheckout,
    reuse: bool,
    guard: WorktreeGuard,
    attempted: &mut Option<AgentLaunchResources>,
    bubblewrap_root: Option<&Path>,
) -> Result<PreparedCheckout, String> {
    let mut before_git_write = |path: &Path, branch: &str, requested: &str| {
        validate_checkout_root(path, bubblewrap_root)?;
        *attempted = Some(AgentLaunchResources {
            tab_id: None,
            created_tab_id: None,
            cwd: Some(path.to_owned()),
            branch: Some(branch.to_owned()),
            requested_branch: (branch != requested).then(|| requested.to_owned()),
            reused: false,
        });
        Ok::<(), String>(())
    };
    let root_output = process::Command::new("git")
        .arg("-C")
        .arg(&source_directory)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|error| format!("failed to run git: {error}"))?;
    if !root_output.status.success() {
        return Err(git_error("find the repository root", &root_output.stderr));
    }
    let root = PathBuf::from(String::from_utf8_lossy(&root_output.stdout).trim());
    match checkout {
        GitCheckout::Current => {
            guard.check(&root)?;
            Ok(PreparedCheckout {
                reused_worktree: false,
                path: root,
                branch: None,
            })
        },
        GitCheckout::NewBranch { branch, base } => {
            validate_git_names(&root, &branch, &base)?;
            if reuse {
                // A branch checked out somewhere else cannot be switched to here -
                // git allows one checkout of a branch at a time - so the agent goes
                // to the worktree already holding it rather than nowhere at all.
                if let Some(worktree) = worktree_holding(&root, &branch)? {
                    guard.check(&worktree)?;
                    return Ok(PreparedCheckout {
                        reused_worktree: true,
                        path: worktree,
                        branch: Some(branch),
                    });
                }
            }
            // Every route left lands in `root` - this checkout moves to the branch - so
            // the guard is settled here, before the switch that would otherwise leave a
            // refused request with the user's checkout already moved.
            guard.check(&root)?;
            if reuse && branch_exists(&root, &branch) {
                before_git_write(&root, &branch, &branch)?;
                run_git(&root, &["switch", &branch], "switch to the branch")?;
                return Ok(PreparedCheckout {
                    reused_worktree: false,
                    path: root,
                    branch: Some(branch),
                });
            }
            let requested = branch.clone();
            let branch = if reuse {
                branch
            } else {
                free_branch(&root, &branch)?
            };
            before_git_write(&root, &branch, &requested)?;
            run_git(
                &root,
                &["switch", "-c", &branch, &base],
                "create and switch to the branch",
            )?;
            Ok(PreparedCheckout {
                reused_worktree: false,
                path: root,
                branch: Some(branch),
            })
        },
        GitCheckout::NewWorktree { branch, base, path } => {
            // Anchored to the primary checkout, not to the caller's. A relative path
            // joined onto the caller's root nests a worktree inside a worktree
            // whenever the agent asking for one is already sitting in a worktree,
            // and a nested worktree is stranded the moment its parent is removed.
            let root = primary_worktree(&root)?;
            validate_git_names(&root, &branch, &base)?;
            let path = if path.is_absolute() {
                path
            } else {
                root.join(path)
            };
            if reuse {
                // The path first, because it is the more specific of the two ways a
                // request can already be satisfied: a worktree sitting there is the
                // folder the caller named, whatever branch it happens to hold, and
                // nothing about it is changed. That branch is what is answered - the
                // one asked for was not used - and a detached checkout holds none.
                if let Some(existing) = worktree_at(&root, &path)? {
                    guard.check(&existing.path)?;
                    return Ok(PreparedCheckout {
                        reused_worktree: true,
                        path: existing.path,
                        branch: existing.branch,
                    });
                }
                if let Some(existing) = worktree_holding(&root, &branch)? {
                    guard.check(&existing)?;
                    return Ok(PreparedCheckout {
                        reused_worktree: true,
                        path: existing,
                        branch: Some(branch),
                    });
                }
                let path_string = path.to_string_lossy().into_owned();
                // An existing branch already has a history, so it is checked out as
                // it stands and the base has nothing to say about it.
                // What is cut below needs no check and could not have one: `git worktree
                // add` always makes a linked worktree, and `linked_worktree` of a folder
                // that does not exist yet answers false.
                let args: Vec<&str> = if branch_exists(&root, &branch) {
                    vec!["worktree", "add", "--", &path_string, &branch]
                } else {
                    vec!["worktree", "add", "-b", &branch, "--", &path_string, &base]
                };
                before_git_write(&path, &branch, &branch)?;
                run_git(&root, &args, "create the worktree")?;
                exclude_nested_worktree(&root, &path);
                return Ok(PreparedCheckout {
                    reused_worktree: false,
                    path,
                    branch: Some(branch),
                });
            }
            // A worktree is named after its branch, so one search settles both: the
            // first suffix whose branch does not exist and whose folder is free.
            let requested = branch.clone();
            let (branch, path) = free_worktree(&root, &path, &branch)?;
            let path_string = path.to_string_lossy().into_owned();
            before_git_write(&path, &branch, &requested)?;
            run_git(
                &root,
                &["worktree", "add", "-b", &branch, "--", &path_string, &base],
                "create the worktree",
            )?;
            exclude_nested_worktree(&root, &path);
            Ok(PreparedCheckout {
                reused_worktree: false,
                path,
                branch: Some(branch),
            })
        },
    }
}

/// Keep a worktree made inside the primary checkout out of that checkout's `git status`.
///
/// `.worktrees/` sits in the user's own tree, so without this `git status` lists it and
/// `git add -A` records every worktree in it as an embedded repository. The entry names the
/// worktree itself, never a folder above it: a `worktree_dir` such as `src/worktrees` must
/// not hide the rest of `src`. It goes to the repository's local `info/exclude`, never to a
/// tracked `.gitignore`, and only when nothing ignores the worktree already. A failure here
/// is logged: the worktree itself is fine.
fn exclude_nested_worktree(root: &std::path::Path, path: &std::path::Path) {
    let result = (|| -> Result<(), String> {
        let canonical = |path: &std::path::Path| {
            path.canonicalize()
                .map_err(|error| format!("cannot resolve {}: {error}", path.display()))
        };
        let (root, path) = (canonical(root)?, canonical(path)?);
        let Some(relative) = path
            .strip_prefix(&root)
            .ok()
            .map(|relative| {
                relative
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .filter(|relative| !relative.is_empty())
        else {
            return Ok(());
        };
        let ignored = process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["check-ignore", "-q", "--", &format!("{relative}/")])
            .status()
            .map_err(|error| format!("failed to run git: {error}"))?;
        if ignored.success() {
            return Ok(());
        }
        let output = process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["rev-parse", "--git-common-dir"])
            .output()
            .map_err(|error| format!("failed to run git: {error}"))?;
        if !output.status.success() {
            return Err(git_error("find the git directory", &output.stderr));
        }
        let info = root
            .join(String::from_utf8_lossy(&output.stdout).trim())
            .join("info");
        std::fs::create_dir_all(&info)
            .map_err(|error| format!("cannot create {}: {error}", info.display()))?;
        let exclude = info.join("exclude");
        let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&exclude)
            .map_err(|error| format!("cannot open {}: {error}", exclude.display()))?;
        let separator = if existing.is_empty() || existing.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        std::io::Write::write_all(
            &mut file,
            format!(
                "{separator}# Agent worktree made by yinz\n/{}/\n",
                gitignore_literal(&relative)
            )
            .as_bytes(),
        )
        .map_err(|error| format!("cannot write {}: {error}", exclude.display()))
    })();
    if let Err(error) = result {
        log::warn!(
            "could not exclude worktree {} from git status: {error}",
            path.display()
        );
    }
}

/// `path` with its gitignore wildcards escaped, so the pattern matches only that folder.
/// The written `/{path}/` already keeps a leading `!` or `#` and trailing spaces literal.
fn gitignore_literal(path: &str) -> String {
    let mut pattern = String::with_capacity(path.len());
    for character in path.chars() {
        if matches!(character, '\\' | '*' | '?' | '[') {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern
}

/// How many suffixes are tried before a request is answered with a refusal rather than
/// another `git worktree list`. A hundred agents on one branch name is not a case to serve
/// quietly; it is a case to say something about.
const FREE_NAME_ATTEMPTS: u32 = 100;

/// `name`, `name-2`, `name-3`, … — the names a collision walks through.
fn suffixed(name: &str, attempt: u32) -> String {
    if attempt == 0 {
        name.to_owned()
    } else {
        format!("{name}-{}", attempt + 1)
    }
}

/// The first of those names no branch has taken.
fn free_branch(root: &std::path::Path, branch: &str) -> Result<String, String> {
    (0..FREE_NAME_ATTEMPTS)
        .map(|attempt| suffixed(branch, attempt))
        .find(|candidate| !branch_exists(root, candidate))
        .ok_or_else(|| format!("git could not find a free branch name near {branch}"))
}

/// The first name free as *both* a branch and a folder, with the worktree renamed to match -
/// the same suffix on the branch and on the folder's last component, so the two never come
/// apart.
///
/// One listing for the whole search rather than one per candidate: the answer only has to be
/// free of what is there now, and a race with another `git worktree add` is caught by git
/// itself. A folder that is not a worktree counts as taken here rather than as the hard
/// failure it is under reuse — the point of a suffix is to step around what is in the way.
fn free_worktree(
    root: &std::path::Path,
    path: &std::path::Path,
    branch: &str,
) -> Result<(String, PathBuf), String> {
    let parent = path.parent().unwrap_or(root).to_path_buf();
    // The folder's own last component carries the suffix, not the whole branch name: a
    // branch with a `/` in it is laid out as folders (`.worktrees/feature/foo`), and
    // joining `feature/foo` onto that folder's parent would bury the worktree a level
    // deeper than the path the request was planned and confirmed with.
    let leaf = path
        .file_name()
        .map(|leaf| leaf.to_string_lossy().into_owned())
        .ok_or_else(|| format!("{} is not a usable worktree path", path.display()))?;
    let existing = worktrees(root)?;
    for attempt in 0..FREE_NAME_ATTEMPTS {
        let candidate = suffixed(branch, attempt);
        let path = parent.join(suffixed(&leaf, attempt));
        let taken = branch_exists(root, &candidate)
            || path.exists()
            || existing
                .iter()
                .any(|worktree| same_path(&worktree.path, &path));
        if !taken {
            return Ok((candidate, path));
        }
    }
    Err(format!(
        "git could not find a free worktree name near {branch}"
    ))
}

/// Whether `folder` is a *linked* worktree: a checkout of a repository that is not that
/// repository's primary one.
///
/// A folder that is no repository at all answers false rather than failing — "not a
/// worktree" is the answer being asked for, and a spawn outside a repository is exactly one
/// of the cases the caller has to own up to.
fn linked_worktree(folder: &std::path::Path) -> Result<bool, String> {
    let output = process::Command::new("git")
        .arg("-C")
        .arg(folder)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|error| format!("failed to run git: {error}"))?;
    if !output.status.success() {
        return Ok(false);
    }
    let root = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let primary = primary_worktree(&root)?;
    Ok(!same_path(&root, &primary))
}

/// The repository's primary worktree — the checkout a relative worktree path is
/// resolved against.
///
/// `git rev-parse --show-toplevel` answers the *caller's* worktree, which is the
/// wrong anchor for `git worktree add`. `git worktree list --porcelain` always
/// lists the primary worktree first and answers its real checkout path. Taking the
/// parent of `--git-common-dir` instead would assume the common directory is a
/// `.git` sitting inside its checkout, which `--separate-git-dir` and `$GIT_DIR`
/// both break.
fn primary_worktree(root: &std::path::Path) -> Result<PathBuf, String> {
    worktrees(root)?
        .into_iter()
        .next()
        .map(|worktree| worktree.path)
        .ok_or_else(|| "git could not find the primary worktree".to_owned())
}

/// One checkout of a repository: where it is, and which branch it holds.
///
/// `branch` is `None` for a detached head and for the bare entry a bare
/// repository lists first - neither can be reused for a branch request.
struct Worktree {
    path: PathBuf,
    branch: Option<String>,
}

/// Every checkout of the repository, primary first.
///
/// One `git worktree list --porcelain` for all three questions asked of it
/// (which folder anchors a relative path, is this folder already a worktree, who
/// holds this branch), so a request costs one process rather than three.
fn worktrees(root: &std::path::Path) -> Result<Vec<Worktree>, String> {
    let output = process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .map_err(|error| format!("failed to run git: {error}"))?;
    if !output.status.success() {
        return Err(git_error("list the worktrees", &output.stderr));
    }
    Ok(parse_worktrees(&String::from_utf8_lossy(&output.stdout)))
}

/// Porcelain blocks, one per checkout, separated by blank lines: a `worktree`
/// line opens each and `branch refs/heads/<name>` follows it unless the checkout
/// is detached or bare.
fn parse_worktrees(porcelain: &str) -> Vec<Worktree> {
    let mut worktrees: Vec<Worktree> = vec![];
    for line in porcelain.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if !path.is_empty() {
                worktrees.push(Worktree {
                    path: PathBuf::from(path),
                    branch: None,
                });
            }
        } else if let Some(reference) = line.strip_prefix("branch ") {
            if let Some(worktree) = worktrees.last_mut() {
                worktree.branch = reference
                    .strip_prefix("refs/heads/")
                    .filter(|branch| !branch.is_empty())
                    .map(str::to_owned);
            }
        }
    }
    worktrees
}

/// The checkout already sitting at `path`, if one is - branch included, because the
/// caller reuses it as it stands and has to report the branch it holds rather than the
/// one that was asked for.
fn worktree_at(root: &std::path::Path, path: &std::path::Path) -> Result<Option<Worktree>, String> {
    Ok(worktrees(root)?
        .into_iter()
        .find(|worktree| same_path(&worktree.path, path)))
}

/// The checkout that has `branch` out, if any has.
fn worktree_holding(root: &std::path::Path, branch: &str) -> Result<Option<PathBuf>, String> {
    Ok(worktrees(root)?
        .into_iter()
        .find(|worktree| worktree.branch.as_deref() == Some(branch))
        .map(|worktree| worktree.path))
}

/// Whether `branch` is a local branch of this repository.
///
/// Exit status only: "no such branch" is an answer here, not a failure worth a
/// message, so this deliberately does not go through `run_git`.
fn branch_exists(root: &std::path::Path, branch: &str) -> bool {
    process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["show-ref", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Whether two paths name the same folder.
///
/// Git answers with physical paths while a requested worktree path is usually
/// one that does not exist yet, so the resolved comparison is tried first and
/// the literal one is what is left when either side cannot be resolved.
fn same_path(one: &std::path::Path, other: &std::path::Path) -> bool {
    match (std::fs::canonicalize(one), std::fs::canonicalize(other)) {
        (Ok(one), Ok(other)) => one == other,
        _ => one == other,
    }
}

fn validate_git_names(root: &std::path::Path, branch: &str, base: &str) -> Result<(), String> {
    if branch.trim().is_empty() || base.trim().is_empty() || base.starts_with('-') {
        return Err("branch and base must be non-empty, and base cannot start with '-'".to_owned());
    }
    run_git(
        root,
        &["check-ref-format", "--branch", branch],
        "validate the branch",
    )
}

fn run_git(root: &std::path::Path, args: &[&str], operation: &str) -> Result<(), String> {
    let output = process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|error| format!("failed to run git: {error}"))?;
    output
        .status
        .success()
        .then_some(())
        .ok_or_else(|| git_error(operation, &output.stderr))
}

fn git_error(operation: &str, stderr: &[u8]) -> String {
    let detail = String::from_utf8_lossy(stderr).trim().to_owned();
    if detail.is_empty() {
        format!("git could not {operation}")
    } else {
        format!("git could not {operation}: {detail}")
    }
}

fn open_command_pane_floating(
    env: &mut PluginEnv,
    command_to_run: CommandToRun,
    floating_pane_coordinates: Option<FloatingPaneCoordinates>,
    context: BTreeMap<String, String>,
) {
    let error_msg = || format!("failed to open command in plugin {}", env.name());
    let name: Option<String> = None;
    let Some(run_command_action) = run_command_action(env, command_to_run, false, context) else {
        return;
    };
    let action = Action::NewFloatingPane {
        command: Some(run_command_action),
        pane_name: name,
        placement: NewPanePlacement::Floating(floating_pane_coordinates),
        near_current_pane: false,
        no_focus: false,
        tab_id: None,
    };
    let result = apply_action!(action, error_msg, env);

    // Extract pane_id from result and convert to yinz_types PaneId
    let pane_id: OpenCommandPaneFloatingResponse = result.and_then(|r| r.affected_pane_id);

    // Write response to plugin
    let response = ProtobufOpenCommandPaneFloatingResponse::from(pane_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_command_pane_floating response".to_string())
        .non_fatal();
}

/// Open a command pane directly onto the session surface.
///
/// The placement travels whole rather than being flattened to coordinates: a terminal pane has no
/// plugin configuration for the server to read overlay-ness back out of, so this is the only thing
/// that knows whether the pane is a modal.
fn open_command_pane_overlay(
    env: &mut PluginEnv,
    command_to_run: CommandToRun,
    placement: OverlayPlacement,
    context: BTreeMap<String, String>,
) {
    if let Some(opening) = &env.pick_opening {
        opening.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    if env
        .pick_cancelled
        .as_ref()
        .is_some_and(|cancelled| cancelled.load(std::sync::atomic::Ordering::SeqCst))
    {
        crate::wasm_bridge::picker::attach_opened_pane(env, None).non_fatal();
        let response = ProtobufOpenCommandPaneOverlayResponse::from(None);
        wasi_write_object(env, &response.encode_to_vec())
            .context("reject cancelled picker overlay")
            .non_fatal();
        return;
    }
    let error_msg = || format!("failed to open command in plugin {}", env.name());
    let Some(run_command_action) = run_command_action(env, command_to_run, false, context) else {
        crate::wasm_bridge::picker::attach_opened_pane(env, None).non_fatal();
        return;
    };
    let action = Action::NewFloatingPane {
        command: Some(run_command_action),
        pane_name: None,
        placement: NewPanePlacement::Overlay(placement),
        near_current_pane: false,
        no_focus: false,
        tab_id: None,
    };
    let result = apply_action!(action, error_msg, env);

    let pane_id: OpenCommandPaneOverlayResponse = result.and_then(|r| r.affected_pane_id);
    crate::wasm_bridge::picker::attach_opened_pane(env, pane_id).non_fatal();
    let response = ProtobufOpenCommandPaneOverlayResponse::from(pane_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_command_pane_overlay response".to_string())
        .non_fatal();
}

fn open_command_pane_floating_near_plugin(
    env: &mut PluginEnv,
    command_to_run: CommandToRun,
    floating_pane_coordinates: Option<FloatingPaneCoordinates>,
    context: BTreeMap<String, String>,
) {
    let name: Option<String> = None;
    let Some(run_command_action) = run_command_action(env, command_to_run, false, context) else {
        return;
    };
    let run_cmd = TerminalAction::RunCommand(run_command_action.into());

    // Create completion channel
    let (completion_tx, completion_rx) = oneshot::channel();
    let _ = env.senders.send_to_pty(PtyInstruction::SpawnTerminal(
        Some(run_cmd),
        name,
        NewPanePlacement::Floating(floating_pane_coordinates),
        false,
        ClientTabIndexOrPaneId::PaneIdOrClient(PaneId::Plugin(env.plugin_id), env.client_id),
        Some(NotificationEnd::new(completion_tx)),
        false, // set_blocking
    ));

    // Wait for completion
    let result = wait_for_action_completion(
        completion_rx,
        "open_command_pane_floating_near_plugin",
        false,
    );
    let pane_id: OpenCommandPaneFloatingNearPluginResponse = result.affected_pane_id;

    // Write response to plugin
    let response = ProtobufOpenCommandPaneFloatingNearPluginResponse::from(pane_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| {
            "failed to write open_command_pane_floating_near_plugin response".to_string()
        })
        .non_fatal();
}

fn open_command_pane_in_place(
    env: &mut PluginEnv,
    command_to_run: CommandToRun,
    context: BTreeMap<String, String>,
) {
    let error_msg = || format!("failed to open command in plugin {}", env.name());
    let name: Option<String> = None;
    let Some(run_command_action) = run_command_action(env, command_to_run, false, context) else {
        return;
    };
    let action = Action::NewInPlacePane {
        command: Some(run_command_action),
        pane_name: name,
        near_current_pane: false,
        no_focus: false,
        pane_id_to_replace: None,
        close_replaced_pane: false,
        tab_id: None,
    };
    let result = apply_action!(action, error_msg, env);

    // Extract pane_id from result and convert to yinz_types PaneId
    let pane_id: OpenCommandPaneInPlaceResponse = result.and_then(|r| r.affected_pane_id);

    // Write response to plugin
    let response = ProtobufOpenCommandPaneInPlaceResponse::from(pane_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_command_pane_in_place response".to_string())
        .non_fatal();
}

fn open_command_pane_background(
    env: &mut PluginEnv,
    command_to_run: CommandToRun,
    context: BTreeMap<String, String>,
) {
    let start_suppressed = true;
    let name: Option<String> = None;
    let Some(run_command_action) = run_command_action(env, command_to_run, true, context) else {
        return;
    };
    let run_cmd = TerminalAction::RunCommand(run_command_action.into());

    // Create completion channel
    let (completion_tx, completion_rx) = oneshot::channel();
    let _ = env.senders.send_to_pty(PtyInstruction::SpawnTerminal(
        Some(run_cmd),
        name,
        NewPanePlacement::default(),
        start_suppressed,
        ClientTabIndexOrPaneId::ClientId(env.client_id),
        Some(NotificationEnd::new(completion_tx)),
        false, // set_blocking
    ));

    // Wait for completion
    // The pane can start after the generic one-second UI deadline. Its caller needs
    // the actual identity to observe that process without dispatching it twice.
    let result = wait_for_action_completion(completion_rx, "open_command_pane_background", true);
    let pane_id: OpenCommandPaneBackgroundResponse = result.affected_pane_id;

    // Write response to plugin
    let response = ProtobufOpenCommandPaneBackgroundResponse::from(pane_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_command_pane_background response".to_string())
        .non_fatal();
}

fn rerun_command_pane(env: &PluginEnv, terminal_pane_id: TerminalId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::RerunCommandPane(terminal_pane_id, None));
}

/// Run a *different* command in an existing command pane, reusing its pane id. The `context`
/// round-trips on the resulting `CommandPaneExited`, addressed to this plugin.
fn run_command_in_pane(
    env: &PluginEnv,
    pane_id: PaneId,
    command_to_run: CommandToRun,
    context: BTreeMap<String, String>,
    keep_scrollback: bool,
) {
    let PaneId::Terminal(terminal_pane_id) = pane_id else {
        log::error!("cannot run a command in a plugin pane");
        return;
    };
    let Some(command) = run_command_action(env, command_to_run, true, context) else {
        return;
    };
    let run_command: RunCommand = command.into();
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::RunCommandInPane(
            terminal_pane_id,
            run_command,
            keep_scrollback,
        ));
}

fn switch_tab_to(env: &PluginEnv, tab_idx: TabNumber) {
    env.senders
        .send_to_screen(ScreenInstruction::GoToTab(
            tab_idx,
            Some(env.client_id),
            None,
        ))
        .with_context(|| {
            format!(
                "failed to switch to tab {tab_idx} from plugin {}",
                env.name()
            )
        })
        .non_fatal();
}

fn timeout_duration(secs: FractionalSeconds) -> Result<Duration> {
    Duration::try_from_secs_f64(secs.get()).map_err(anyError::new)
}

fn set_timeout(env: &PluginEnv, secs: FractionalSeconds) {
    let duration = match timeout_duration(secs) {
        Ok(duration) => duration,
        Err(e) => {
            Err::<(), _>(e)
                .with_context(|| format!("invalid timeout of {secs} s for plugin {}", env.name()))
                .non_fatal();
            return;
        },
    };
    let senders = yinz_bus::thread_bus::ThreadSenders {
        to_plugin: env.senders.to_plugin.clone(),
        should_silently_fail: env.senders.should_silently_fail,
        ..Default::default()
    };
    let update_target = env.plugin_id;
    let deno_generation = env.deno_generation;
    let client_id = env.client_id;
    let plugin_name = env.name();
    // Use tokio runtime for async I/O (timer operation)
    let start_time = Instant::now();
    get_tokio_runtime().spawn(async move {
        tokio::time::sleep(duration).await;
        // This measures the host timer alone. The `Timer` event then waits on the plugin bus, in
        // `cached_events_for_pending_plugins` while any instance of this plugin id is loading,
        // reloading or awaiting permission, and behind earlier work on the plugin's executor or
        // Deno worker, which can add seconds. A plugin that needs to know how long it really
        // waited reads its own clock.
        let elapsed_time = Instant::now().duration_since(start_time).as_secs_f64();

        senders
            .send_to_plugin(PluginInstruction::for_plugin_event(
                update_target,
                client_id,
                deno_generation,
                Event::Timer(FractionalSeconds::new(elapsed_time)),
            ))
            .with_context(|| {
                format!(
                    "failed to set host timeout of {secs} s for plugin {}",
                    plugin_name
                )
            })
            .non_fatal();
    });
}

fn exec_cmd(env: &PluginEnv, mut command_line: Vec<String>) {
    log::warn!("The ExecCmd plugin command is deprecated and will be removed in a future version. Please use RunCmd instead (it has all the things and can even show you STDOUT/STDERR and an exit code!)");
    if command_line.is_empty() {
        log::warn!("Cannot execute an empty command");
        return;
    }
    let err_context = || {
        format!(
            "failed to execute command on host for plugin '{}'",
            env.name()
        )
    };
    let command = command_line.remove(0);

    // Bail out if we're forbidden to run command
    if !env.plugin_config._allow_exec_host_cmd {
        warn!("This plugin isn't allow to run command in host side, skip running this command: '{cmd} {args}'.",
        	cmd = command, args = command_line.join(" "));
        return;
    }

    // Here, we don't wait the command to finish
    process::Command::new(command)
        .args(command_line)
        .spawn()
        .with_context(err_context)
        .non_fatal();
}

fn swamp_request(
    env: &PluginEnv,
    directory: PathBuf,
    id: String,
    operation: String,
    payload: String,
) {
    let sender = env.senders.to_plugin.clone();
    let target = env.plugin_id;
    let client = env.client_id;
    let generation = env.deno_generation;
    get_tokio_runtime().spawn(async move {
        let emit = |frame: serde_json::Value| {
            if let Some(sender) = &sender {
                sender.send(PluginInstruction::for_plugin_event(target, client, generation,
                    Event::CustomMessage("swamp-response".into(), frame.to_string()))).to_anyhow().context("cannot deliver Swamp response").non_fatal();
            }
        };
        let result: Result<()> = async {
            let config = yinz_config::project_config::access(&directory, None).map_err(|e| anyhow!(e))?;
            if let Some(error) = config.error { return Err(anyhow!(error)); }
            let root = config.root;
            let payload: serde_json::Value = serde_json::from_str(&payload).context("invalid Swamp request payload")?;
            let progress = |payload| emit(serde_json::json!({"type":"managed.progress","id":id,"payload":payload}));
            if operation == "managed.endpoint" {
                let value = yinz_swamp::public_endpoint(&root)?;
                emit(serde_json::json!({"type":operation,"id":id,"payload":{"data":value}}));
            } else if operation == "workflow.get" {
                let workflow = payload["workflowIdOrName"].as_str().context("workflow is required")?;
                let value = yinz_swamp::definitions::get(root, workflow, &progress).await?;
                emit(serde_json::json!({"type":operation,"id":id,"payload":{"data":value}}));
            } else if matches!(operation.as_str(), "workflow.edit" | "workflow.delete") {
                let value = yinz_swamp::definitions::change(root, &operation, payload, &progress).await?;
                emit(serde_json::json!({"type":operation,"id":id,"payload":{"data":value}}));
            } else if operation == "workflow.cancel" {
                let warm = root.clone();
                let warm_id = id.clone();
                tokio::task::spawn_blocking(move || yinz_swamp::prewarm_request(&warm, &warm_id)).await.context("Swamp startup failed")??;
                let run = payload["runId"].as_str().context("cancellation requires a run ID")?.to_owned();
                let value = yinz_swamp::cancel(root, run).await?;
                emit(serde_json::json!({"type":"workflow.cancel", "id":id, "payload":{"data":value}}));
            } else {
                let mut events = yinz_swamp::request(root, id.clone(), operation, payload).await?;
                while let Some(frame) = events.recv().await { emit(frame); }
            }
            Ok(())
        }.await;
        if let Err(error) = result {
            let message = error.to_string();
            let detail = serde_json::from_str::<serde_json::Value>(&message).ok()
                .filter(|value| value["code"].is_string() && value["message"].is_string())
                .unwrap_or_else(|| serde_json::json!({"code":"host_error","message":message}));
            emit(serde_json::json!({"type":"error", "id":id, "error":detail}));
        }
    });
}

fn run_command(
    env: &PluginEnv,
    mut command_line: Vec<String>,
    env_variables: BTreeMap<String, String>,
    cwd: PathBuf,
    context: BTreeMap<String, String>,
    max_output_bytes: Option<usize>,
    stream_output: Option<bool>,
) {
    if command_line.is_empty() {
        log::error!("Command cannot be empty");
    } else {
        let command = command_line.remove(0);
        let cwd = if env.plugin_config.is_builtin() && context.contains_key("swamp_owner") {
            cwd
        } else {
            translate_plugin_path(env, cwd)
        };
        let _ = env
            .senders
            .send_to_background_jobs(BackgroundJob::RunCommand(
                env.plugin_id,
                env.client_id,
                command,
                command_line,
                env_variables,
                cwd,
                context,
                max_output_bytes,
                stream_output,
                env.deno_generation,
            ));
    }
}

fn web_request(
    env: &PluginEnv,
    url: String,
    verb: HttpVerb,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
    context: BTreeMap<String, String>,
    stream_id: Option<String>,
    proxy: Option<String>,
) {
    let _ = env
        .senders
        .send_to_background_jobs(BackgroundJob::WebRequest(
            env.plugin_id,
            env.client_id,
            url,
            verb,
            headers,
            body,
            context,
            stream_id,
            proxy,
            env.deno_generation,
        ));
}

fn cancel_web_request(env: &PluginEnv, stream_id: String) {
    let _ = env
        .senders
        .send_to_background_jobs(BackgroundJob::CancelWebRequest(
            env.plugin_id,
            env.client_id,
            stream_id,
        ));
}

fn post_message_to(env: &PluginEnv, plugin_message: PluginMessage) -> Result<()> {
    let worker_name = plugin_message
        .worker_name
        .ok_or(anyhow!("Worker name not specified in message to worker"))?;
    if let Some(workers) = &env.deno_workers {
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

fn post_message_to_plugin(env: &PluginEnv, plugin_message: PluginMessage) -> Result<()> {
    if let Some(worker_name) = plugin_message.worker_name {
        return Err(anyhow!(
            "Worker name (\"{}\") should not be specified in message to plugin",
            worker_name
        ));
    }
    if let Some(workers) = &env.deno_workers {
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

fn hide_self(env: &PluginEnv) -> Result<()> {
    env.senders
        .send_to_screen(ScreenInstruction::SuppressPane(
            PaneId::Plugin(env.plugin_id),
            env.client_id,
        ))
        .with_context(|| "failed to hide self".to_string())
}

fn hide_pane_with_id(env: &PluginEnv, pane_id: PaneId) -> Result<()> {
    env.senders
        .send_to_screen(ScreenInstruction::SuppressPane(pane_id, env.client_id))
        .with_context(|| "failed to hide self".to_string())
}

fn show_self(env: &PluginEnv, should_float_if_hidden: bool) {
    let action = Action::FocusPluginPaneWithId {
        pane_id: env.plugin_id,
        should_float_if_hidden,
        should_be_in_place_if_hidden: false,
    };
    let error_msg = || "Failed to show self for plugin".to_string();
    apply_action!(action, error_msg, env);
}

fn show_pane_with_id(
    env: &PluginEnv,
    pane_id: PaneId,
    should_float_if_hidden: bool,
    should_focus_pane: bool,
) {
    if should_focus_pane {
        let _ = env
            .senders
            .send_to_screen(ScreenInstruction::FocusPaneWithId(
                pane_id,
                should_float_if_hidden,
                false,
                env.client_id,
                None,
            ));
    } else {
        let _ = env
            .senders
            .send_to_screen(ScreenInstruction::UnsuppressOrExpandPane(
                pane_id,
                should_float_if_hidden,
            ));
    }
}

fn close_self(env: &PluginEnv) {
    env.senders
        .send_to_screen(ScreenInstruction::CloseSelf(env.plugin_id, env.client_id))
        .with_context(|| "failed to close self".to_string())
        .non_fatal();
    env.senders
        .send_to_plugin(PluginInstruction::Unload(env.plugin_id))
        .with_context(|| "failed to close self".to_string())
        .non_fatal();
}

/// Queue commands that answer do so from the shared `Queues` rather than by asking the plugin
/// thread, because a plugin that blocks on the plugin thread while another plugin is loading
/// deadlocks: the loader holds the plugin map and takes every running plugin's lock, this
/// plugin's included. Only what needs the plugin thread - starting and stopping jobs - goes
/// there, and never with a reply to wait for.
fn enqueue_queue_job(
    env: &mut PluginEnv,
    queue: String,
    plugin_url: String,
    mut config: BTreeMap<String, String>,
    context: BTreeMap<String, String>,
) {
    use yinz_plugin_api::plugin_command::ProtobufEnqueueQueueJobResponse;

    let plugin_url = if plugin_url == "yinz:OWN_URL" {
        env.plugin_config.location.display()
    } else {
        plugin_url
    };
    // The same courtesy `message_to_plugin` extends: a job usually wants to work where its
    // enqueuer works.
    config
        .entry("caller_cwd".to_owned())
        .or_insert_with(|| env.plugin_cwd.display().to_string());
    let response = if queue.is_empty() {
        EnqueueQueueJobResponse::Err("a queue needs a name".to_owned())
    } else {
        let resolved = {
            let plugin_aliases = env.plugin_aliases.lock().unwrap();
            yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
                &plugin_url,
                &Some(config),
                Some(&plugin_aliases),
                Some(env.plugin_cwd.clone()),
            )
            .and_then(|mut run_plugin_or_alias| {
                run_plugin_or_alias.populate_run_plugin_if_needed(&plugin_aliases);
                if run_plugin_or_alias.get_run_plugin().is_some() {
                    Ok(run_plugin_or_alias)
                } else {
                    Err(format!("{plugin_url} is not a plugin or a known alias"))
                }
            })
        };
        match resolved {
            Ok(run_plugin_or_alias) => {
                let job_id = env.queues.lock().unwrap().enqueue(
                    &queue,
                    (env.plugin_id, env.client_id),
                    env.deno_generation,
                    run_plugin_or_alias,
                    context,
                    Some(env.plugin_cwd.clone()),
                );
                env.senders
                    .send_to_plugin(PluginInstruction::QueuePump { queue })
                    .with_context(|| "failed to pump queue".to_string())
                    .non_fatal();
                EnqueueQueueJobResponse::Ok(job_id)
            },
            Err(e) => EnqueueQueueJobResponse::Err(e),
        }
    };
    let response: ProtobufEnqueueQueueJobResponse = response.into();
    let _ = wasi_write_object(env, &response.encode_to_vec());
}

fn configure_queue(env: &PluginEnv, queue: String, concurrency: usize) {
    env.senders
        .send_to_plugin(PluginInstruction::QueueConfigure { queue, concurrency })
        .with_context(|| "failed to configure queue".to_string())
        .non_fatal();
}

fn cancel_queue_job(env: &PluginEnv, job_id: QueueJobId) {
    env.senders
        .send_to_plugin(PluginInstruction::QueueCancelJob { job_id })
        .with_context(|| "failed to cancel queue job".to_string())
        .non_fatal();
}

fn queue_status(env: &mut PluginEnv, queue: Option<String>) {
    use yinz_plugin_api::plugin_command::ProtobufQueueStatusResponse;

    let queues = env.queues.lock().unwrap().snapshot(queue.as_deref());
    let response: ProtobufQueueStatusResponse = queues.into();
    let _ = wasi_write_object(env, &response.encode_to_vec());
}

/// The job reports and the host unloads it — the host, not the job, because the outcome has
/// to be recorded before the unload frees the slot, and only the plugin thread can order those.
fn finish_queue_job(env: &PluginEnv, success: bool, payload: Option<String>) {
    env.senders
        .send_to_plugin(PluginInstruction::QueueFinishJob {
            plugin_id: env.plugin_id,
            success,
            payload,
        })
        .with_context(|| "failed to finish queue job".to_string())
        .non_fatal();
}

fn reconfigure(env: &PluginEnv, new_config: String, write_config_to_disk: bool) -> Result<()> {
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

fn rebind_keys(
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

fn register_default_keybindings(env: &PluginEnv, keybindings: String) -> Result<()> {
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
                {
                    if name.starts_with("__yinz_default_keybinding:") {
                        *plugin_id = Some(env.plugin_id);
                    }
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

fn switch_to_mode(env: &PluginEnv, input_mode: InputMode) {
    let action = Action::SwitchToMode { input_mode };
    let error_msg = || format!("failed to switch to mode in plugin {}", env.name());
    apply_action!(action, error_msg, env);
}

fn ensure_plugin_permissions(
    env: &PluginEnv,
    permissions: impl IntoIterator<Item = PermissionType>,
) -> Result<()> {
    let missing: Vec<_> = permissions
        .into_iter()
        .filter(|permission| !has_permission(env, *permission))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(
            "layout needs permissions this plugin has not been granted: {}",
            missing
                .into_iter()
                .map(|permission| permission.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}

fn layout_run_permissions(layout: &Layout) -> BTreeSet<PermissionType> {
    let mut permissions = BTreeSet::new();
    let mut inspect =
        |tiled_layout: &yinz_types::input::layout::TiledPaneLayout,
         floating_layouts: &[yinz_types::input::layout::FloatingPaneLayout]| {
            let runs = tiled_layout
                .extract_run_instructions()
                .into_iter()
                .flatten()
                .chain(
                    floating_layouts
                        .iter()
                        .filter(|pane| !pane.already_running)
                        .filter_map(|pane| pane.run.clone()),
                );
            for run in runs {
                match run {
                    Run::Command(_) => {
                        permissions.insert(PermissionType::RunCommands);
                    },
                    Run::EditFile(..) => {
                        permissions.insert(PermissionType::OpenFiles);
                    },
                    Run::Plugin(_) => {
                        permissions.insert(PermissionType::OpenTerminalsOrPlugins);
                    },
                    Run::Cwd(_) => {},
                }
            }
        };
    if layout.tabs.is_empty() {
        if let Some((tiled_layout, floating_layouts)) = layout.template.as_ref() {
            inspect(tiled_layout, floating_layouts);
        }
    } else {
        for (_, tiled_layout, floating_layouts) in &layout.tabs {
            inspect(tiled_layout, floating_layouts);
        }
    }
    permissions
}

fn validate_plugin_layout_path(layout_name: &str, layout_dir: Option<&PathBuf>) -> Result<()> {
    let requested = Path::new(layout_name);
    if requested.components().any(|component| {
        matches!(
            component,
            Component::Prefix(_) | Component::RootDir | Component::ParentDir
        )
    }) {
        return Err(anyhow!(
            "plugin layout path must stay inside the layout directory: {}",
            requested.display()
        ));
    }

    let Some(layout_dir) = layout_dir.cloned().or_else(default_layout_dir) else {
        return Ok(());
    };
    let requested = layout_dir.join(requested);
    let candidates = [
        requested.clone(),
        requested.with_extension("kdl"),
        requested.with_extension("swap.kdl"),
    ];
    if !candidates.iter().any(|candidate| candidate.exists()) {
        return Ok(());
    }

    let canonical_root = layout_dir.canonicalize().with_context(|| {
        format!(
            "failed to resolve layout directory {}",
            layout_dir.display()
        )
    })?;
    for candidate in candidates
        .into_iter()
        .filter(|candidate| candidate.exists())
    {
        let canonical_candidate = candidate
            .canonicalize()
            .with_context(|| format!("failed to resolve layout path {}", candidate.display()))?;
        if !canonical_candidate.starts_with(&canonical_root) {
            return Err(anyhow!(
                "plugin layout path resolves outside the layout directory: {}",
                candidate.display()
            ));
        }
    }
    Ok(())
}

fn ensure_layout_info_permissions(env: &PluginEnv, layout_info: &LayoutInfo) -> Result<()> {
    match layout_info {
        LayoutInfo::File(layout_name, _) => {
            validate_plugin_layout_path(layout_name, env.layout_dir.as_ref())?;
            ensure_plugin_permissions(
                env,
                [
                    PermissionType::OpenFiles,
                    PermissionType::RunCommands,
                    PermissionType::OpenTerminalsOrPlugins,
                ],
            )
        },
        LayoutInfo::BuiltIn(_) | LayoutInfo::Stringified(_) => {
            let layout =
                yinz_config::layout::from_layout_info(&env.layout_dir, layout_info.clone())
                    .map_err(|e| anyhow!("Failed to parse layout: {:?}", e))?;
            ensure_plugin_permissions(env, layout_run_permissions(&layout))
        },
    }
}

fn layout_from_plugin_info(env: &PluginEnv, layout_info: LayoutInfo) -> Result<Layout> {
    ensure_layout_info_permissions(env, &layout_info)?;
    yinz_config::layout::from_layout_info(&env.layout_dir, layout_info)
        .map_err(|e| anyhow!("Failed to parse layout: {:?}", e))
}

fn new_tabs_with_layout(env: &mut PluginEnv, raw_layout: &str) -> Result<()> {
    let layout = yinz_config::layout::from_str(
        raw_layout,
        format!("Layout from plugin: {}", env.name()),
        None,
        None,
    )
    .map_err(|e| anyhow!("Failed to parse layout: {:?}", e))?;
    ensure_plugin_permissions(env, layout_run_permissions(&layout))?;
    apply_layout(env, layout);
    Ok(())
}

fn new_tabs_with_layout_info(env: &mut PluginEnv, layout_info: LayoutInfo) -> Result<()> {
    let layout = layout_from_plugin_info(env, layout_info)?;
    apply_layout(env, layout);
    Ok(())
}

fn apply_layout(env: &mut PluginEnv, layout: Layout) {
    let mut tabs_to_open = vec![];
    let tabs = layout.tabs();
    // KDL already embeds layout/tab/pane cwds in the runs. Leave the tab override
    // unset so panes without a cwd inherit normally, as with new_tab(None, None).
    let cwd = None;
    if tabs.is_empty() {
        let swap_tiled_layouts = Some(layout.swap_tiled_layouts.clone());
        let swap_floating_layouts = Some(layout.swap_floating_layouts.clone());
        let action = Action::NewTab {
            layout_source: None,
            tiled_layout: layout.template.as_ref().map(|t| t.0.clone()),
            floating_layouts: layout.template.map(|t| t.1).unwrap_or_default(),
            swap_tiled_layouts,
            swap_floating_layouts,
            tab_name: None,
            should_change_focus_to_new_tab: true,
            cwd,
            initial_panes: None,
            first_pane_unblock_condition: None,
        };
        tabs_to_open.push(action);
    } else {
        let focused_tab_index = layout.focused_tab_index().unwrap_or(TabPosition::new(0));
        for (tab_index, (tab_name, tiled_pane_layout, floating_pane_layout)) in
            layout.tabs().into_iter().enumerate()
        {
            let should_focus_tab = TabPosition::new(tab_index) == focused_tab_index;
            let swap_tiled_layouts = Some(layout.swap_tiled_layouts.clone());
            let swap_floating_layouts = Some(layout.swap_floating_layouts.clone());
            let action = Action::NewTab {
                layout_source: None,
                tiled_layout: Some(tiled_pane_layout),
                floating_layouts: floating_pane_layout,
                swap_tiled_layouts,
                swap_floating_layouts,
                tab_name,
                should_change_focus_to_new_tab: should_focus_tab,
                cwd: cwd.clone(),
                initial_panes: None,
                first_pane_unblock_condition: None,
            };
            tabs_to_open.push(action);
        }
    }
    let mut tab_ids = Vec::new();

    for action in tabs_to_open {
        let error_msg = || "Failed to create layout tab".to_string();
        let result = apply_action!(action, error_msg, env);

        // Collect tab ID from each action
        if let Some(tab_id) = result.and_then(|r| r.affected_tab_id) {
            tab_ids.push(tab_id);
        }
    }

    // Write response
    let response = ProtobufNewTabsResponse::from(tab_ids);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write layout tabs response".to_string())
        .non_fatal();
}

fn new_tab(env: &mut PluginEnv, name: Option<String>, cwd: Option<String>) {
    let cwd = cwd.map(|c| translate_plugin_path(env, PathBuf::from(c)));
    let action = Action::NewTab {
        layout_source: None,
        tiled_layout: None,
        floating_layouts: vec![],
        swap_tiled_layouts: None,
        swap_floating_layouts: None,
        tab_name: name,
        should_change_focus_to_new_tab: true,
        cwd,
        initial_panes: None,
        first_pane_unblock_condition: None,
    };
    let error_msg = || "Failed to open new tab".to_string();
    let result = apply_action!(action, error_msg, env);

    // Extract tab_id from result - return None if not present
    let tab_id: NewTabResponse = result.and_then(|r| r.affected_tab_id);

    // Write response to plugin
    let response = ProtobufNewTabResponse::from(tab_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write new_tab response".to_string())
        .non_fatal();
}

fn new_tab_unfocused(env: &mut PluginEnv, name: Option<String>, cwd: Option<String>) {
    let cwd = cwd.map(|c| translate_plugin_path(env, PathBuf::from(c)));
    let action = Action::NewTab {
        layout_source: None,
        tiled_layout: None,
        floating_layouts: vec![],
        swap_tiled_layouts: None,
        swap_floating_layouts: None,
        tab_name: name,
        should_change_focus_to_new_tab: false,
        cwd,
        initial_panes: None,
        first_pane_unblock_condition: None,
    };
    let error_msg = || "Failed to open new tab (unfocused)".to_string();
    let result = apply_action!(action, error_msg, env);

    let tab_id: NewTabUnfocusedResponse = result.and_then(|r| r.affected_tab_id);

    let response = ProtobufNewTabUnfocusedResponse::from(tab_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write new_tab_unfocused response".to_string())
        .non_fatal();
}

fn new_tiled_pane_in_tab(env: &mut PluginEnv, tab_id: TabId) {
    let error_msg = || format!("failed to open new tiled pane in tab {}", tab_id);
    let default_shell = env.default_shell.clone().unwrap_or_else(|| {
        TerminalAction::RunCommand(RunCommand {
            command: env.path_to_default_shell.clone(),
            use_terminal_title: true,
            ..Default::default()
        })
    });
    let run_command_action: Option<RunCommandAction> = match default_shell {
        TerminalAction::RunCommand(run_command) => Some(run_command.into()),
        _ => None,
    };
    let action = Action::NewTiledPane {
        direction: None,
        command: run_command_action,
        pane_name: None,
        near_current_pane: false,
        no_focus: false,
        borderless: None,
        tab_id: Some(tab_id),
    };
    let result = apply_action!(action, error_msg, env);

    let pane_id: NewTiledPaneInTabResponse = result.and_then(|r| r.affected_pane_id);

    let response = ProtobufNewTiledPaneInTabResponse::from(pane_id);
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| "failed to write new_tiled_pane_in_tab response".to_string())
        .non_fatal();
}

fn go_to_next_tab(env: &PluginEnv) {
    let action = Action::GoToNextTab;
    let error_msg = || "Failed to go to next tab".to_string();
    apply_action!(action, error_msg, env);
}

fn go_to_previous_tab(env: &PluginEnv) {
    let action = Action::GoToPreviousTab;
    let error_msg = || "Failed to go to previous tab".to_string();
    apply_action!(action, error_msg, env);
}

fn resize(env: &PluginEnv, resize: Resize) {
    let error_msg = || format!("failed to resize in plugin {}", env.name());
    let action = Action::Resize {
        resize,
        direction: None,
    };
    apply_action!(action, error_msg, env);
}

fn resize_with_direction(env: &PluginEnv, resize: ResizeStrategy) {
    let error_msg = || format!("failed to resize in plugin {}", env.name());
    let action = Action::Resize {
        resize: resize.resize,
        direction: resize.direction,
    };
    apply_action!(action, error_msg, env);
}

fn focus_next_pane(env: &PluginEnv) {
    let action = Action::FocusNextPane;
    let error_msg = || "Failed to focus next pane".to_string();
    apply_action!(action, error_msg, env);
}

fn focus_previous_pane(env: &PluginEnv) {
    let action = Action::FocusPreviousPane;
    let error_msg = || "Failed to focus previous pane".to_string();
    apply_action!(action, error_msg, env);
}

fn focus_last_pane(env: &PluginEnv) {
    let action = Action::FocusLastPane;
    let error_msg = || "Failed to focus last pane".to_string();
    apply_action!(action, error_msg, env);
}

fn move_focus(env: &PluginEnv, direction: Direction) {
    let error_msg = || format!("failed to move focus in plugin {}", env.name());
    let action = Action::MoveFocus { direction };
    apply_action!(action, error_msg, env);
}

fn move_focus_or_tab(env: &PluginEnv, direction: Direction) {
    let error_msg = || format!("failed to move focus in plugin {}", env.name());
    let action = Action::MoveFocusOrTab { direction };
    apply_action!(action, error_msg, env);
}

fn detach(env: &PluginEnv) {
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

fn switch_session(
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

fn delete_dead_session(session_name: &SessionName) -> Result<()> {
    delete_dead_session_in(&YINZ_SESSION_INFO_CACHE_DIR, session_name)
}

fn delete_dead_session_in(session_info_cache_dir: &Path, session_name: &SessionName) -> Result<()> {
    validate_session_name(session_name.as_str()).map_err(|e| anyhow!(e))?;
    std::fs::remove_dir_all(session_info_cache_dir.join(session_name))
        .with_context(|| format!("Failed to delete dead session: {:?}", session_name))
}

fn delete_all_dead_sessions() -> Result<()> {
    use yinz_ipc::consts::{is_ipc_socket, is_session_socket_name};
    let mut live_sessions = vec![];
    if let Ok(files) = std::fs::read_dir(&*YINZ_SOCK_DIR) {
        files.for_each(|file| {
            if let Ok(file) = file {
                if let Ok(file_name) = file.file_name().into_string() {
                    if is_ipc_socket(&file.file_type().unwrap())
                        && is_session_socket_name(&file_name)
                    {
                        live_sessions.push(SessionName::new(file_name));
                    }
                }
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

fn edit_scrollback(env: &PluginEnv) {
    let action = Action::EditScrollback { ansi: false };
    let error_msg = || "Failed to edit scrollback".to_string();
    apply_action!(action, error_msg, env);
}

fn write(env: &PluginEnv, bytes: Vec<u8>) {
    let error_msg = || format!("failed to write in plugin {}", env.name());
    let action = Action::Write {
        key_with_modifier: None,
        bytes,
        is_kitty_keyboard_protocol: false,
    };
    apply_action!(action, error_msg, env);
}

fn write_chars(env: &PluginEnv, chars_to_write: String) {
    let error_msg = || format!("failed to write in plugin {}", env.name());
    let action = Action::WriteChars {
        chars: chars_to_write,
    };
    apply_action!(action, error_msg, env);
}

fn copy_to_clipboard(env: &PluginEnv, text: String) -> Result<()> {
    env.senders
        .send_to_screen(ScreenInstruction::CopyTextToClipboard(text, env.plugin_id))
        .with_context(|| format!("failed to copy to clipboard in plugin {}", env.name()))
}

fn toggle_tab(env: &PluginEnv) {
    let error_msg = || "Failed to toggle tab".to_string();
    let action = Action::ToggleTab;
    apply_action!(action, error_msg, env);
}

fn move_pane(env: &PluginEnv) {
    let error_msg = || format!("failed to move pane in plugin {}", env.name());
    let action = Action::MovePane { direction: None };
    apply_action!(action, error_msg, env);
}

fn move_pane_with_direction(env: &PluginEnv, direction: Direction) {
    let error_msg = || format!("failed to move pane in plugin {}", env.name());
    let action = Action::MovePane {
        direction: Some(direction),
    };
    apply_action!(action, error_msg, env);
}

fn clear_screen(env: &PluginEnv) {
    let error_msg = || format!("failed to clear screen in plugin {}", env.name());
    let action = Action::ClearScreen;
    apply_action!(action, error_msg, env);
}
fn scroll_up(env: &PluginEnv) {
    let error_msg = || format!("failed to scroll up in plugin {}", env.name());
    let action = Action::ScrollUp;
    apply_action!(action, error_msg, env);
}

fn scroll_down(env: &PluginEnv) {
    let error_msg = || format!("failed to scroll down in plugin {}", env.name());
    let action = Action::ScrollDown;
    apply_action!(action, error_msg, env);
}

fn scroll_to_top(env: &PluginEnv) {
    let error_msg = || format!("failed to scroll in plugin {}", env.name());
    let action = Action::ScrollToTop;
    apply_action!(action, error_msg, env);
}

fn scroll_to_bottom(env: &PluginEnv) {
    let error_msg = || format!("failed to scroll in plugin {}", env.name());
    let action = Action::ScrollToBottom;
    apply_action!(action, error_msg, env);
}

fn page_scroll_up(env: &PluginEnv) {
    let error_msg = || format!("failed to scroll in plugin {}", env.name());
    let action = Action::PageScrollUp;
    apply_action!(action, error_msg, env);
}

fn page_scroll_down(env: &PluginEnv) {
    let error_msg = || format!("failed to scroll in plugin {}", env.name());
    let action = Action::PageScrollDown;
    apply_action!(action, error_msg, env);
}

fn toggle_focus_fullscreen(env: &PluginEnv) {
    let error_msg = || format!("failed to toggle full screen in plugin {}", env.name());
    let action = Action::ToggleFocusFullscreen;
    apply_action!(action, error_msg, env);
}

fn toggle_focus_no_ui_fullscreen(env: &PluginEnv) {
    let error_msg = || {
        format!(
            "failed to toggle no-ui full screen in plugin {}",
            env.name()
        )
    };
    let action = Action::ToggleFocusNoUiFullscreen;
    apply_action!(action, error_msg, env);
}

fn focus_host_session(env: &PluginEnv) {
    env.senders
        .send_to_screen(ScreenInstruction::FocusHostSession(env.client_id, None))
        .with_context(|| format!("failed to focus host session from plugin {}", env.name()))
        .non_fatal();
}

fn toggle_pane_frames(env: &PluginEnv) {
    let error_msg = || format!("failed to toggle full screen in plugin {}", env.name());
    let action = Action::TogglePaneFrames;
    apply_action!(action, error_msg, env);
}

fn set_pane_frame_style(env: &PluginEnv, pane_frame_style: PaneFrameStyle) {
    let error_msg = || format!("failed to set pane frame style in plugin {}", env.name());
    let action = Action::SetPaneFrameStyle(pane_frame_style);
    apply_action!(action, error_msg, env);
}

fn toggle_pane_embed_or_eject(env: &PluginEnv) {
    let error_msg = || {
        format!(
            "failed to toggle pane embed or eject in plugin {}",
            env.name()
        )
    };
    let action = Action::TogglePaneEmbedOrFloating;
    apply_action!(action, error_msg, env);
}

fn toggle_floating_panes(env: &PluginEnv, tab_id: Option<TabId>) {
    let error_msg = || format!("failed to toggle floating panes in plugin {}", env.name());
    let action = match tab_id {
        Some(id) => Action::ToggleFloatingPanesByTabId { id },
        None => Action::ToggleFloatingPanes,
    };
    apply_action!(action, error_msg, env);
}

fn new_pane(env: &PluginEnv) {
    let error_msg = || format!("failed to open new pane in plugin {}", env.name());
    let action = Action::NewPane {
        direction: None,
        pane_name: None,
        start_suppressed: false,
    };
    apply_action!(action, error_msg, env);
}

fn undo_rename_pane(env: &PluginEnv) {
    let error_msg = || format!("failed to undo rename pane in plugin {}", env.name());
    let action = Action::UndoRenamePane;
    apply_action!(action, error_msg, env);
}

fn close_focus(env: &PluginEnv) {
    let error_msg = || format!("failed to close focused pane in plugin {}", env.name());
    let action = Action::CloseFocus;
    apply_action!(action, error_msg, env);
}

fn toggle_active_tab_sync(env: &PluginEnv) {
    let error_msg = || format!("failed to toggle active tab sync in plugin {}", env.name());
    let action = Action::ToggleActiveSyncTab;
    apply_action!(action, error_msg, env);
}

fn close_focused_tab(env: &PluginEnv) {
    let error_msg = || format!("failed to close active tab in plugin {}", env.name());
    let action = Action::CloseTab;
    apply_action!(action, error_msg, env);
}

fn undo_rename_tab(env: &PluginEnv) {
    let error_msg = || format!("failed to undo rename tab in plugin {}", env.name());
    let action = Action::UndoRenameTab;
    apply_action!(action, error_msg, env);
}

fn quit_yinz(env: &PluginEnv) {
    let error_msg = || format!("failed to quit yinz in plugin {}", env.name());
    let action = Action::Quit;
    apply_action!(action, error_msg, env);
}

fn previous_swap_layout(env: &PluginEnv) {
    let error_msg = || format!("failed to switch swap layout in plugin {}", env.name());
    let action = Action::PreviousSwapLayout;
    apply_action!(action, error_msg, env);
}

fn next_swap_layout(env: &PluginEnv) {
    let error_msg = || format!("failed to switch swap layout in plugin {}", env.name());
    let action = Action::NextSwapLayout;
    apply_action!(action, error_msg, env);
}

fn go_to_tab_name(env: &PluginEnv, tab_name: String) {
    let error_msg = || format!("failed to change tab in plugin {}", env.name());
    let create = false;
    let action = Action::GoToTabName {
        name: tab_name,
        create,
    };
    apply_action!(action, error_msg, env);
}

fn focus_or_create_tab(env: &mut PluginEnv, tab_name: String) {
    let error_msg = || format!("failed to change or create tab in plugin {}", env.name());
    let create = true;
    let action = Action::GoToTabName {
        name: tab_name,
        create,
    };
    let result = apply_action!(action, error_msg, env);

    // Return Some(tab_id) if tab was created or focused, None on error
    let tab_id: FocusOrCreateTabResponse = result.and_then(|r| r.affected_tab_id);

    let response = ProtobufFocusOrCreateTabResponse::from(tab_id);
    wasi_write_object(env, &response.encode_to_vec()).non_fatal();
}

fn go_to_tab(env: &PluginEnv, tab_index: TabNumber) {
    let error_msg = || format!("failed to change tab focus in plugin {}", env.name());
    let action = Action::GoToTab { index: tab_index };
    apply_action!(action, error_msg, env);
}

fn start_or_reload_plugin(env: &PluginEnv, url: &str) -> Result<()> {
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

fn get_plugin_info(env: &mut PluginEnv, id: Option<PluginId>) -> Result<()> {
    let plugin_id = id.unwrap_or(env.plugin_id);
    let background = if plugin_id == env.plugin_id {
        Some(env.background)
    } else {
        env.plugin_roles.lock().unwrap().get(&plugin_id).copied()
    };
    let info = background.map(|background| yinz_types::data::PluginInstanceInfo {
        plugin_id,
        background,
    });
    let response = yinz_plugin_api::plugin_command::ProtobufGetPluginInfoResponse {
        json: serde_json::to_string(&info).context("encode plugin info")?,
    };
    wasi_write_object(env, &response.encode_to_vec()).context("write plugin info")
}

fn get_plugin_ids_for_url(
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
    wasi_write_object(env, &response.encode_to_vec()).context("write plugin identity response")
}

fn reload_plugin_by_url(env: &PluginEnv, url: String) -> Result<()> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let run_plugin_or_alias =
        yinz_runtime::plugin_location::run_plugin_or_alias_from_url(&url, &None, None, Some(cwd))
            .map_err(|e| anyhow!("Failed to parse plugin location: {}", e))?;
    env.senders
        .send_to_plugin(PluginInstruction::ReloadPluginByUrl(run_plugin_or_alias))
        .with_context(|| format!("failed to reload plugin at {url}"))
}

fn close_terminal_pane(env: &PluginEnv, terminal_pane_id: TerminalId) {
    let error_msg = || format!("failed to change tab focus in plugin {}", env.name());
    let action = Action::CloseTerminalPane {
        pane_id: terminal_pane_id,
    };
    apply_action!(action, error_msg, env);
    env.senders
        .send_to_pty(PtyInstruction::ClosePane(
            PaneId::Terminal(terminal_pane_id),
            None,
        ))
        .non_fatal();
}

fn close_plugin_pane(env: &PluginEnv, plugin_pane_id: PluginId) {
    let error_msg = || format!("failed to change tab focus in plugin {}", env.name());
    let action = Action::ClosePluginPane {
        pane_id: plugin_pane_id,
    };
    apply_action!(action, error_msg, env);
    env.senders
        .send_to_plugin(PluginInstruction::Unload(plugin_pane_id))
        .non_fatal();
}

fn focus_terminal_pane(
    env: &PluginEnv,
    terminal_pane_id: TerminalId,
    should_float_if_hidden: bool,
    should_be_in_place_if_hidden: bool,
) {
    let action = Action::FocusTerminalPaneWithId {
        pane_id: terminal_pane_id,
        should_float_if_hidden,
        should_be_in_place_if_hidden,
    };
    let error_msg = || "Failed to focus terminal pane".to_string();
    apply_action!(action, error_msg, env);
}

fn focus_plugin_pane(
    env: &PluginEnv,
    plugin_pane_id: PluginId,
    should_float_if_hidden: bool,
    should_be_in_place_if_hidden: bool,
) {
    let action = Action::FocusPluginPaneWithId {
        pane_id: plugin_pane_id,
        should_float_if_hidden,
        should_be_in_place_if_hidden,
    };
    let error_msg = || "Failed to focus plugin pane".to_string();
    apply_action!(action, error_msg, env);
}

fn rename_terminal_pane(env: &PluginEnv, terminal_pane_id: TerminalId, new_name: &str) {
    let error_msg = || "Failed to rename terminal pane".to_string();
    let rename_pane_action = Action::RenameTerminalPane {
        pane_id: terminal_pane_id,
        name: new_name.as_bytes().to_vec(),
    };
    apply_action!(rename_pane_action, error_msg, env);
}

fn rename_plugin_pane(env: &PluginEnv, plugin_pane_id: PluginId, new_name: &str) {
    let error_msg = || "Failed to rename plugin pane".to_string();
    let rename_pane_action = Action::RenamePluginPane {
        pane_id: plugin_pane_id,
        name: new_name.as_bytes().to_vec(),
    };
    apply_action!(rename_pane_action, error_msg, env);
}

fn rename_tab(env: &PluginEnv, tab_position: TabNumber, new_name: &str) {
    let error_msg = || "Failed to rename tab".to_string();
    let rename_tab_action = Action::RenameTab {
        // The command dispatcher converts the plugin position to a tab number.
        tab_index: tab_position,
        name: new_name.as_bytes().to_vec(),
    };
    apply_action!(rename_tab_action, error_msg, env);
}

fn rename_session(env: &PluginEnv, new_session_name: SessionName) {
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

fn notify(env: &PluginEnv, text: String, pane_id: Option<PaneId>, click: Option<NotifyClick>) {
    let error_msg = || format!("failed to show notification from plugin {}", env.name());
    // Stamped here rather than carried up from the plugin: a toast may only ever call back the
    // plugin that raised it, so the id is the host's to supply. `owner_client_id` and not
    // `client_id`, because this names an *instance* rather than a turn - it is the key the plugin
    // map holds this one under, which is what the delivery matches on. Same distinction
    // `post_message_to` makes for a worker.
    let on_click = click.map(|click| NotificationClick {
        plugin_id: env.plugin_id,
        client_id: env.owner_client_id,
        message: click.message,
        payload: click.payload,
    });
    env.senders
        .send_to_screen(ScreenInstruction::PluginNotify {
            text,
            pane_id,
            on_click,
            plugin_id: env.plugin_id,
            completion: None,
        })
        .with_context(error_msg)
        .non_fatal();
}

fn disconnect_other_clients(env: &PluginEnv) {
    let _ = env
        .senders
        .send_to_server(ServerInstruction::DisconnectAllClientsExcept(env.client_id))
        .context("failed to send disconnect other clients instruction");
}

fn kill_sessions(session_names: Vec<SessionName>) {
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

fn kill_sessions_and_reply(env: &mut PluginEnv, session_names: Vec<SessionName>) {
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
    wasi_write_object(env, &protobuf_response.encode_to_vec())
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

fn warp_pane_to_session(
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

fn warp_pane_from_session(
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

fn warp_tab_to_session(env: &mut PluginEnv, tab_id: TabId, to_session: SessionName) {
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

fn warp_tab_from_session(env: &mut PluginEnv, tab_id: TabId, from_session: SessionName) {
    let result = yinz_bus::warp::warp_tab_from_session(
        &env.server_session.current(),
        &from_session,
        tab_id,
        yinz_bus::warp::WARP_PULL_TIMEOUT,
    );
    write_warp_response(env, result);
}

#[cfg(unix)]
fn adopt_orphaned_pane(env: &mut PluginEnv, host_id: SupervisorId, placement: WarpPlacement) {
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
fn adopt_orphaned_pane(env: &PluginEnv, _host_id: SupervisorId, _placement: WarpPlacement) {
    write_warp_response(env, Err(NO_DETACHABLE_PANES.to_owned()));
}

#[cfg(unix)]
fn list_orphaned_panes(env: &mut PluginEnv) {
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
fn list_orphaned_panes(env: &PluginEnv) {
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
    wasi_write_object(
        env,
        &ProtobufWarpPaneResponse::from(response).encode_to_vec(),
    )
    .with_context(|| "failed to write the warp response".to_string())
    .non_fatal();
}

fn write_orphaned_pane_list(env: &mut PluginEnv, response: ListOrphanedPanesResponse) {
    wasi_write_object(
        env,
        &ProtobufListOrphanedPanesResponse::from(response).encode_to_vec(),
    )
    .with_context(|| "failed to write the orphaned pane list".to_string())
    .non_fatal();
}

// ------------------------------------------------------------------ data store
//
// These are all "purely local work" handlers: they run on the calling plugin's pinned executor
// thread, do their work against the shared SQLite store, and write the response inline. No other
// yinz thread is involved, and nothing is delivered as an event — except the watch notifications,
// which are queued through the plugin thread rather than dispatched here (see `notify_watchers`).

/// Work out which namespace a command means, and whether this plugin may use it.
///
/// `None` is the plugin's own private namespace, which it cannot name and therefore cannot mistake
/// for anyone else's. Anything else must be `shared:`-prefixed and needs `SharedPluginData`.
///
/// The shared check lives here rather than in `check_command_permission` because that sees only the
/// command, not its arguments — the same reason event permissions are checked separately.
fn resolve_data_namespace(env: &PluginEnv, namespace: Option<String>) -> Result<String, DataError> {
    let Some(namespace) = namespace else {
        return Ok(plugin_data_store::private_namespace(
            env.data_owner.as_ref(),
        ));
    };
    validate_namespace(Some(&namespace))?;
    if !may_reach_shared_namespaces(env) {
        return Err(DataError::PermissionDenied(format!(
            "reading or writing the shared namespace `{}` needs the SharedPluginData permission",
            namespace
        )));
    }
    Ok(namespace)
}

/// Whether this plugin may reach `shared:` namespaces at all.
///
/// Split out of [`resolve_data_namespace`] because the catalog asks the same question without
/// having a namespace to resolve: it is deciding what to *list*, not what to open.
fn may_reach_shared_namespaces(env: &PluginEnv) -> bool {
    env.plugin_config.is_builtin()
        || env
            .permissions
            .lock()
            .unwrap()
            .as_ref()
            .map(|permissions| permissions.contains(&PermissionType::SharedPluginData))
            .unwrap_or(false)
}

/// The namespace a fetch means, resolved from the request it is carried in.
fn resolve_fetch_namespace(env: &PluginEnv, request: &FetchRequest) -> Result<String, DataError> {
    resolve_data_namespace(env, request.namespace.clone())
}

fn data_define_entity(
    env: &mut PluginEnv,
    namespace: Option<String>,
    def: EntityDef,
    allow_destructive: bool,
) {
    let plugin_url = env.data_owner.to_string();
    let response = match resolve_data_namespace(env, namespace).and_then(|namespace| {
        env.data_access.plugin_data().define_entity(
            &namespace,
            &plugin_url,
            &def,
            allow_destructive,
        )
    }) {
        Ok(info) => DataDefineResponse::Ok(info),
        Err(e) => DataDefineResponse::Err(e.to_string()),
    };
    write_data_response(
        env,
        ProtobufDataDefineResponse::from(response).encode_to_vec(),
        "define response",
    );
}

fn data_apply(env: &mut PluginEnv, namespace: Option<String>, mutations: Vec<Mutation>) {
    let response = match resolve_data_namespace(env, namespace).and_then(|namespace| {
        Ok((
            namespace.clone(),
            env.data_access
                .plugin_data()
                .apply(&namespace, &mutations)?,
        ))
    }) {
        Ok((namespace, outcome)) => {
            notify_watchers(env, &namespace, &outcome.touched_entities);
            DataApplyResponse::Ok(outcome.report)
        },
        // A conflict is its own answer, not an error: the caller asked for a compare-and-set and
        // is being told the comparison failed, which is a race to retry rather than a request to
        // fix.
        Err(DataError::Conflict(message)) => DataApplyResponse::Conflict(message),
        Err(e) => DataApplyResponse::Err(e.to_string()),
    };
    write_data_response(
        env,
        ProtobufDataApplyResponse::from(response).encode_to_vec(),
        "apply response",
    );
}

fn data_fetch(env: &mut PluginEnv, request: FetchRequest) {
    let response = match resolve_fetch_namespace(env, &request)
        .and_then(|namespace| env.data_access.plugin_data().fetch(&namespace, &request))
    {
        Ok(rows) => DataFetchResponse::Ok(rows),
        Err(e) => DataFetchResponse::Err(e.to_string()),
    };
    write_data_response(
        env,
        ProtobufDataFetchResponse::from(response).encode_to_vec(),
        "fetch response",
    );
}

fn data_count(env: &mut PluginEnv, request: FetchRequest) {
    let response = match resolve_fetch_namespace(env, &request)
        .and_then(|namespace| env.data_access.plugin_data().count(&namespace, &request))
    {
        Ok(count) => DataCountResponse::Ok(count),
        Err(e) => DataCountResponse::Err(e.to_string()),
    };
    write_data_response(
        env,
        ProtobufDataCountResponse::from(response).encode_to_vec(),
        "count response",
    );
}

fn data_catalog(env: &mut PluginEnv, namespace: Option<String>) {
    let response = match catalog_entities(env, namespace) {
        Ok(entities) => DataCatalogResponse::Ok(entities),
        Err(e) => DataCatalogResponse::Err(e.to_string()),
    };
    write_data_response(
        env,
        ProtobufDataCatalogResponse::from(response).encode_to_vec(),
        "catalog response",
    );
}

/// The entities this plugin may see.
///
/// A named namespace goes through [`resolve_data_namespace`], so listing a `shared:` one costs
/// exactly the permission fetching from it would.
///
/// An unnamed one is the discovery case, and is the one place `None` does not mean "my own
/// namespace": that would answer only with what the caller declared itself, which it already
/// knows. It is the plugin's own namespace *plus* every `shared:` one, once it holds
/// `SharedPluginData`.
///
/// Another plugin's private namespace is never listed, whatever the permissions. `SharedPluginData`
/// is a claim on `shared:`, and this is the only call that could otherwise enumerate namespaces
/// nobody chose to publish.
fn catalog_entities(
    env: &PluginEnv,
    namespace: Option<String>,
) -> Result<Vec<DataEntityInfo>, DataError> {
    if namespace.is_some() {
        let namespace = resolve_data_namespace(env, namespace)?;
        return env.data_access.plugin_data().catalog(Some(&namespace));
    }
    let own = resolve_data_namespace(env, None)?;
    let shared_visible = may_reach_shared_namespaces(env);
    Ok(env
        .data_access
        .plugin_data()
        .catalog(None)?
        .into_iter()
        .filter(|entity| {
            entity.namespace == own
                || (shared_visible && entity.namespace.starts_with(SHARED_NAMESPACE_PREFIX))
        })
        .collect())
}

fn data_watch(env: &mut PluginEnv, request: FetchRequest, mode: WatchMode) {
    let response = match resolve_fetch_namespace(env, &request).and_then(|namespace| {
        let mut generations = env.deno_watch_generations.lock().unwrap();
        let handle = env.data_access.plugin_data().register_watch(
            &namespace,
            env.plugin_id,
            env.client_id,
            request.clone(),
            mode,
        )?;
        if let Some(generation) = env.deno_generation {
            generations.insert(handle.watch_id, generation);
        }
        Ok(handle)
    }) {
        Ok(handle) => {
            // Event delivery is gated on the plugin's subscriptions, so a plugin that watched but
            // forgot to subscribe would silently never hear anything. Subscribing it here makes
            // `data_watch` mean what it says.
            env.subscriptions
                .lock()
                .unwrap()
                .insert(EventType::PluginDataChanged);
            DataWatchResponse::Ok(handle)
        },
        Err(e) => DataWatchResponse::Err(e.to_string()),
    };
    write_data_response(
        env,
        ProtobufDataWatchResponse::from(response).encode_to_vec(),
        "watch response",
    );
}

fn data_unwatch(env: &mut PluginEnv, watch_id: DataWatchId) {
    let mut generations = env.deno_watch_generations.lock().unwrap();
    let response = match env
        .data_access
        .plugin_data()
        .unregister_watch(env.plugin_id, watch_id)
    {
        Ok(()) => {
            generations.remove(&watch_id);
            DataUnwatchResponse::Ok
        },
        Err(e) => DataUnwatchResponse::Err(e.to_string()),
    };
    drop(generations);
    write_data_response(
        env,
        ProtobufDataUnwatchResponse::from(response).encode_to_vec(),
        "unwatch response",
    );
}

/// Re-run every watch the write could have affected, and queue an event for each whose results
/// actually changed.
///
/// The events are *queued* through the plugin thread rather than dispatched here. This handler runs
/// on a pinned executor thread that holds the calling plugin's `running_plugin` lock, so calling
/// back into the plugin directly would deadlock whenever a plugin watches something it also writes
/// — which is the normal case, not an exotic one.
fn notify_watchers(env: &PluginEnv, namespace: &str, touched_entities: &BTreeSet<String>) {
    notify_data_watchers(
        env.data_access.plugin_data(),
        &env.deno_watch_generations,
        &env.senders,
        namespace,
        touched_entities,
    );
}

pub(crate) fn notify_data_watchers(
    store: &yinz_storage::plugin_data_store::PluginDataStore,
    watch_generations: &super::plugin_map::DenoWatchGenerations,
    senders: &yinz_bus::thread_bus::ThreadSenders,
    namespace: &str,
    touched_entities: &BTreeSet<String>,
) {
    if namespace == project_data::store::SHARED
        && (touched_entities.contains("project") || touched_entities.contains("agent_assignment"))
    {
        senders
            .send_to_screen(ScreenInstruction::InvalidateAgentEventProjects)
            .non_fatal();
    }
    // Refresh and recipient lookup must be atomic with registration/unwatch/disposal.
    let generations = watch_generations.lock().unwrap();
    let changed = match store.refresh_watches(namespace, touched_entities) {
        Ok(changed) => changed,
        Err(e) => {
            log::error!("failed to refresh data store watches: {}", e);
            return;
        },
    };
    for change in changed {
        let event = Event::PluginDataChanged {
            watch_id: change.watch_id,
            truncated: change.rows.truncated,
            rows: change.rows,
        };
        senders
            .send_to_plugin(PluginInstruction::for_plugin_event(
                change.plugin_id,
                change.client_id,
                generations.get(&change.watch_id).copied(),
                event,
            ))
            .with_context(|| "failed to deliver a data store change".to_string())
            .non_fatal();
    }
}

fn write_data_response(env: &mut PluginEnv, encoded: Vec<u8>, what: &str) {
    wasi_write_object(env, &encoded)
        .with_context(|| format!("failed to write the data store {}", what))
        .non_fatal();
}

fn delete_dead_session_and_reply(env: &mut PluginEnv, session_name: SessionName) {
    let response = match delete_dead_session(&session_name) {
        Ok(()) => DeleteDeadSessionResponse::Ok,
        Err(e) => DeleteDeadSessionResponse::Err(format!("{e:#}")),
    };
    let protobuf_response = ProtobufDeleteDeadSessionResponse::from(response);
    wasi_write_object(env, &protobuf_response.encode_to_vec())
        .with_context(|| "failed to write delete_dead_session response".to_string())
        .non_fatal();
}

#[cfg(test)]
mod validation_tests {
    use super::*;

    #[test]
    fn dead_session_deletion_stays_inside_its_cache() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(cache.join("valid")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("sentinel"), "keep").unwrap();

        delete_dead_session_in(&cache, &yinz_types::internal_ids::SessionName::new("valid"))
            .unwrap();
        assert!(!cache.join("valid").exists());

        for invalid in ["", ".", "..", "../outside", outside.to_str().unwrap()] {
            assert!(
                delete_dead_session_in(
                    &cache,
                    &yinz_types::internal_ids::SessionName::new(invalid)
                )
                .is_err(),
                "{invalid:?}"
            );
        }
        assert!(outside.join("sentinel").exists());
    }

    #[test]
    fn invalid_plugin_timeouts_are_rejected() {
        for invalid in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX] {
            assert!(
                timeout_duration(yinz_types::units::FractionalSeconds::new(invalid)).is_err(),
                "{invalid}"
            );
        }
        assert_eq!(
            timeout_duration(yinz_types::units::FractionalSeconds::new(0.5)).unwrap(),
            Duration::from_millis(500)
        );
    }
}

fn delete_all_dead_sessions_and_reply(env: &mut PluginEnv) {
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
    wasi_write_object(env, &protobuf_response.encode_to_vec())
        .with_context(|| "failed to write delete_all_dead_sessions response".to_string())
        .non_fatal();
}

// note this removes the requesting plugin
fn dump_session_layout(env: &mut PluginEnv, tab_index: Option<TabId>) {
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
        let _ = wasi_write_object(env, &error_response.encode_to_vec());
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
                    env.deno_generation
                );
                let error_response = ProtobufDumpSessionLayoutResponse {
                    result: Some(dump_session_layout_response::Result::Error(format!(
                        "Timeout waiting for session layout: {}",
                        e
                    ))),
                    metadata: None,
                };
                let _ = wasi_write_object(env, &error_response.encode_to_vec());
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

    // Write response back to plugin via WASI pipe
    let _ = wasi_write_object(env, &protobuf_response.encode_to_vec())
        .with_context(|| format!("failed to send session layout to plugin {}", env.name()));
}

fn save_session(env: &mut PluginEnv) {
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

    let _ = wasi_write_object(env, &response.encode_to_vec());
}

fn show_floating_panes(env: &mut PluginEnv, tab_id: Option<TabId>) {
    use show_floating_panes_response::Result as ShowResult;
    let (completion_tx, completion_rx) = oneshot::channel();
    let send_result = env
        .senders
        .send_to_screen(ScreenInstruction::ShowFloatingPanes {
            client_id: env.client_id,
            tab_id,
            completion: Some(NotificationEnd::new(completion_tx)),
        });
    let response = if let Err(e) = send_result {
        ProtobufShowFloatingPanesResponse {
            result: Some(ShowResult::Error(format!("{}", e))),
        }
    } else {
        let result = wait_for_action_completion(completion_rx, "show_floating_panes", false);
        match result.exit_status {
            Some(0) => ProtobufShowFloatingPanesResponse {
                result: Some(ShowResult::Success(true)),
            },
            Some(2) => ProtobufShowFloatingPanesResponse {
                result: Some(ShowResult::Success(false)),
            },
            _ => ProtobufShowFloatingPanesResponse {
                result: Some(ShowResult::Error("Tab not found".to_string())),
            },
        }
    };
    let _ = wasi_write_object(env, &response.encode_to_vec());
}

fn hide_floating_panes(env: &mut PluginEnv, tab_id: Option<TabId>) {
    use hide_floating_panes_response::Result as HideResult;
    let (completion_tx, completion_rx) = oneshot::channel();
    let send_result = env
        .senders
        .send_to_screen(ScreenInstruction::HideFloatingPanes {
            client_id: env.client_id,
            tab_id,
            completion: Some(NotificationEnd::new(completion_tx)),
        });
    let response = if let Err(e) = send_result {
        ProtobufHideFloatingPanesResponse {
            result: Some(HideResult::Error(format!("{}", e))),
        }
    } else {
        let result = wait_for_action_completion(completion_rx, "hide_floating_panes", false);
        match result.exit_status {
            Some(0) => ProtobufHideFloatingPanesResponse {
                result: Some(HideResult::Success(true)),
            },
            Some(2) => ProtobufHideFloatingPanesResponse {
                result: Some(HideResult::Success(false)),
            },
            _ => ProtobufHideFloatingPanesResponse {
                result: Some(HideResult::Error("Tab not found".to_string())),
            },
        }
    };
    let _ = wasi_write_object(env, &response.encode_to_vec());
}

fn set_pane_regex_highlights(
    env: &PluginEnv,
    pane_id: yinz_types::data::PaneId,
    highlights: Vec<RegexHighlight>,
) {
    let err_context = || "failed to set pane regex highlights".to_string();
    let pane_id: PaneId = pane_id;
    env.senders
        .send_to_screen(ScreenInstruction::SetPluginRegexHighlights {
            pane_id,
            plugin_id: env.plugin_id,
            highlights,
        })
        .with_context(err_context)
        .non_fatal();
}

fn clear_pane_highlights(env: &PluginEnv, pane_id: yinz_types::data::PaneId) {
    let err_context = || "failed to clear pane highlights".to_string();
    let pane_id: PaneId = pane_id;
    env.senders
        .send_to_screen(ScreenInstruction::ClearPluginHighlights {
            pane_id,
            plugin_id: env.plugin_id,
        })
        .with_context(err_context)
        .non_fatal();
}

fn current_session_last_saved_time(env: &mut PluginEnv) {
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
    let _ = wasi_write_object(env, &response.encode_to_vec());
}

fn get_pane_info(env: &mut PluginEnv, pane_id: yinz_types::data::PaneId) {
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
    let _ = wasi_write_object(env, &response.encode_to_vec());
}

fn get_tab_info(env: &mut PluginEnv, tab_id: TabId) {
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
    let _ = wasi_write_object(env, &response.encode_to_vec());
}

fn list_notifications(env: &mut PluginEnv) {
    let (response_sender, response_receiver) = crossbeam::channel::bounded(1);
    let notifications = if env
        .senders
        .send_to_screen(ScreenInstruction::ListNotifications {
            response_channel: response_sender,
        })
        .is_ok()
    {
        response_receiver
            .recv_timeout(Duration::from_millis(100))
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
    let _ = wasi_write_object(env, &response.encode_to_vec());
}

fn list_clients(env: &PluginEnv) {
    let _ = env.senders.to_screen.as_ref().map(|sender| {
        sender.send(ScreenInstruction::ListClientsToPlugin(
            env.plugin_id,
            env.client_id,
            env.deno_generation,
        ))
    });
}

fn change_host_folder(env: &PluginEnv, new_host_folder: PathBuf) {
    let _ = env.senders.to_plugin.as_ref().map(|sender| {
        sender.send(PluginInstruction::ChangePluginHostDir(
            new_host_folder,
            env.plugin_id,
            env.client_id,
            env.deno_generation,
        ))
    });
}

fn set_floating_pane_pinned(env: &PluginEnv, pane_id: PaneId, should_be_pinned: bool) {
    let _ = env.senders.to_screen.as_ref().map(|sender| {
        sender.send(ScreenInstruction::SetFloatingPanePinned(
            pane_id,
            should_be_pinned,
        ))
    });
}

fn stack_panes(env: &PluginEnv, pane_ids: Vec<PaneId>) {
    let _ =
        env.senders
            .send_to_screen(ScreenInstruction::StackPanes(pane_ids, env.client_id, None));
}

/// Move or resize overlays, within the overlay collection.
///
/// The sibling of `change_floating_panes_coordinates`, and deliberately not the same instruction:
/// an overlay is not on the tab's floating layer, and a pane that is on it does not become one by
/// being resized.
fn change_overlay_coordinates(
    env: &PluginEnv,
    pane_ids_and_coordinates: Vec<(PaneId, FloatingPaneCoordinates)>,
    override_user_placement: bool,
) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::ChangeOverlayCoordinates(
            pane_ids_and_coordinates,
            override_user_placement,
            None,
        ));
}

fn change_floating_panes_coordinates(
    env: &PluginEnv,
    pane_ids_and_coordinates: Vec<(PaneId, FloatingPaneCoordinates)>,
) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::ChangeFloatingPanesCoordinates(
            pane_ids_and_coordinates,
            None,
        ));
}

fn toggle_pane_borderless(env: &PluginEnv, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::TogglePaneBorderless(pane_id, None));
}

fn set_pane_borderless(env: &PluginEnv, pane_id: PaneId, borderless: bool) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::SetPaneBorderless(
            pane_id, borderless, None,
        ));
}

fn set_pane_agent(env: &PluginEnv, pane_id: yinz_types::data::PaneId, agent: Option<AgentInfo>) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::SetPaneAgent(pane_id, agent, None));
}

fn set_pane_agent_run(env: &PluginEnv, pane_id: yinz_types::data::PaneId, run_id: Option<String>) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::SetPaneAgentRun(pane_id, run_id, None));
}

/// What this plugin wants said, and decided, when the session is about to end.
///
/// Keyed by plugin id rather than by pane id: it is the plugin that has something outstanding,
/// and a plugin that moves pane - agent does, between tabs - has not changed its mind about the
/// quit on the way.
fn set_quit_notice(env: &PluginEnv, notice: Option<String>, guard: bool) {
    let _ = env.senders.send_to_screen(ScreenInstruction::SetQuitNotice(
        env.plugin_id,
        notice,
        guard,
    ));
}

fn set_pane_color(env: &PluginEnv, pane_id: PaneId, fg: Option<String>, bg: Option<String>) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::SetPaneColor(pane_id, fg, bg, None));
}

/// The maximum number of bytes an `FsRead` will return, whatever the plugin asks for.
///
/// These commands answer inline on the plugin thread, so a read is a pause for every plugin
/// pinned to that thread. Larger files are read in ranges so one call cannot buffer the entire
/// file or monopolize the thread with an unbounded read.
const MAX_FS_READ_BYTES: usize = 8 * 1024 * 1024;

/// Where a plugin-visible path actually lives, and whether it left the plugin's own folders.
struct ResolvedPath {
    real: PathBuf,
    /// True when the path named no virtual root and so points somewhere in the wider filesystem.
    /// Reaching one of these needs `FullHdAccess` on top of the read/write permission.
    escaped_roots: bool,
}

/// Resolve a plugin-visible path to a real one, refusing to leave the root it named.
///
/// This is deliberately *not* [`translate_plugin_path`], which the pane-opening commands use.
/// That one maps the four prefixes and stops: `/host/../../etc/passwd` resolves straight out of
/// `/host`, and any genuine absolute path beginning `/tmp` is silently rewritten into yinz's
/// own temp dir. Both are tolerable when the result is handed to an editor the user is watching,
/// and neither is tolerable when the result is read or written on the plugin's say-so.
///
/// So this follows [`scan_host_folder`] instead: canonicalize the root and the target (or its
/// parent for operations on the directory entry), and refuse anything that does not land inside.
/// Canonicalizing catches symlinks pointing out of the root along the path being accessed.
///
/// A path naming no virtual root is resolved against the real filesystem and marked
/// `escaped_roots`, so that the caller can ask for `FullHdAccess`.
/// The virtual root a plugin's own plugin folder is mounted at. Present only when the session
/// has one configured or defaulted; a write under it needs `AuthorPlugins` on top of `WriteFiles`.
pub const PLUGINS_FS_ROOT: &str = "/plugins";

/// The folders a plugin may name, by their virtual roots. `/plugins` is the user's plugin folder
/// and is absent when there is none, so nothing can be written into a folder nobody chose.
fn fs_roots(env: &PluginEnv) -> Vec<(&'static str, PathBuf)> {
    let mut roots = vec![
        ("/host", env.plugin_cwd.clone()),
        ("/data", env.plugin_own_data_dir.clone()),
        ("/cache", env.plugin_own_cache_dir.clone()),
        ("/tmp", YINZ_TMP_DIR.to_path_buf()),
    ];
    if let Some(user_plugin_dir) = env.user_plugin_dir.as_ref() {
        roots.push((PLUGINS_FS_ROOT, user_plugin_dir.clone()));
    }
    roots
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FsPathResolution {
    FollowSymlink,
    PreserveEntry,
}

/// Resolve existing ancestors, retaining missing components for paths that may be created later.
/// Roots need the same resolution as requested paths when a symlink precedes a missing folder.
fn resolve_fs_ancestors(path: &Path) -> Result<PathBuf> {
    let mut existing = path;
    // Collected outermost-last, then re-attached in order. Joining onto an empty `PathBuf` as we
    // go would leave a trailing separator, which asks the OS for a directory.
    let mut tail = vec![];
    loop {
        match existing.canonicalize() {
            Ok(canonical) => {
                return Ok(tail
                    .iter()
                    .rev()
                    .fold(canonical, |path: PathBuf, name| path.join(name)));
            },
            Err(error) => {
                // A dangling link is an existing entry, not a missing path component.
                // Reattaching it unchecked would let a subsequent create follow it outside
                // the root. Entry operations resolve only the parent to preserve the link.
                ensure!(
                    !existing
                        .symlink_metadata()
                        .is_ok_and(|metadata| metadata.file_type().is_symlink()),
                    "'{}' contains an unresolved symlink; refusing to follow it",
                    path.display()
                );
                if error.kind() != std::io::ErrorKind::NotFound {
                    return Err(error)
                        .with_context(|| format!("failed to resolve '{}'", path.display()));
                }
                match (existing.parent(), existing.file_name()) {
                    (Some(parent), Some(name)) => {
                        tail.push(name.to_os_string());
                        existing = parent;
                    },
                    _ => {
                        return Err(anyhow!(
                            "'{}' could not be resolved to a real path",
                            path.display()
                        ))
                    },
                }
            },
        }
    }
}

fn resolve_plugin_fs_path(
    env: &PluginEnv,
    path: &Path,
    resolution: FsPathResolution,
) -> Result<ResolvedPath> {
    let roots = fs_roots(env);

    let (prefix, root, relative) = match roots.iter().find_map(|(prefix, root)| {
        path.strip_prefix(prefix)
            .ok()
            .map(|rest| (*prefix, root, rest))
    }) {
        Some((prefix, root, rest)) => (Some(prefix), root.clone(), rest.to_path_buf()),
        // A relative path is the plugin's cwd, the same reading `translate_plugin_path` gives it.
        None if path.is_relative() => (Some("/host"), env.plugin_cwd.clone(), path.to_path_buf()),
        // FullHdAccess paths still resolve links before O_NOFOLLOW opens their final entry.
        // They use the real filesystem root, with the permission check applied below.
        None => (None, PathBuf::from("/"), path.to_path_buf()),
    };

    let canonical_root = resolve_fs_ancestors(&root)?;
    let joined: PathBuf = canonical_root.join(&relative).components().collect();

    let resolved = match (resolution, relative.file_name(), joined.parent()) {
        // Remove, rename and symlink metadata operate on the entry, not its target. Resolve
        // its parent as usual. A virtual root (no relative file name) still resolves in full.
        (FsPathResolution::PreserveEntry, Some(name), Some(parent)) => {
            resolve_fs_ancestors(parent)?.join(name)
        },
        _ => resolve_fs_ancestors(&joined)?,
    };

    if !resolved.starts_with(&canonical_root) {
        // Name the link when there is one: a plugin that wrote `link.ts` and is refused
        // should hear that it is a link, not merely that it points somewhere.
        let through_a_symlink = joined
            .ancestors()
            .take_while(|ancestor| ancestor.starts_with(&canonical_root))
            .any(|ancestor| {
                ancestor
                    .symlink_metadata()
                    .is_ok_and(|metadata| metadata.file_type().is_symlink())
            });
        if through_a_symlink {
            return Err(anyhow!(
                "'{}' is a symlink pointing outside the plugin's own folders; refusing",
                path.display()
            ));
        }
        return Err(anyhow!(
            "'{}' points outside the plugin's own folders",
            path.display()
        ));
    }

    Ok(ResolvedPath {
        real: resolved,
        escaped_roots: prefix.is_none(),
    })
}

/// What an `Fs*` command is about to do with a path, for the rules that depend on it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FsIntent {
    Read,
    Write,
}

/// Resolve a path for one of the `Fs*` commands, applying the rules `command_permission`
/// cannot: `FullHdAccess` for a path outside every root, and `AuthorPlugins` for a write under
/// `/plugins`.
///
/// Checked here rather than in `command_permission` because that only sees the command, and
/// where a path lands is not knowable without resolving it first. The data store does the same
/// thing for `shared:` namespaces in `resolve_data_namespace`.
fn resolve_fs_path_with_permission(
    env: &PluginEnv,
    path: &Path,
    intent: FsIntent,
    resolution: FsPathResolution,
) -> Result<PathBuf> {
    let resolved = resolve_plugin_fs_path(env, path, resolution)?;
    if resolved.escaped_roots && !has_permission(env, PermissionType::FullHdAccess) {
        return Err(anyhow!(
            "'{}' is outside the plugin's own folders (/host, /data, /cache, /tmp, /plugins), \
             which needs the 'FullHdAccess' permission",
            path.display()
        ));
    }
    if intent == FsIntent::Write
        && env.user_plugin_dir.is_some()
        && !has_permission(env, PermissionType::AuthorPlugins)
    {
        let plugin_dir = resolve_plugin_fs_path(
            env,
            Path::new(PLUGINS_FS_ROOT),
            FsPathResolution::FollowSymlink,
        )?
        .real;
        // Authority follows the physical directory, including aliases through /host and
        // absolute paths. Removing or renaming an ancestor also changes the plugin folder.
        if resolved.real.starts_with(&plugin_dir)
            || (resolution == FsPathResolution::PreserveEntry
                && plugin_dir.starts_with(&resolved.real))
        {
            return Err(anyhow!(
                "'{}' changes the plugin folder, which needs the 'AuthorPlugins' permission",
                path.display()
            ));
        }
    }
    Ok(resolved.real)
}

/// Whether the plugin holds `permission`. Built-in plugins hold everything, as they do in
/// [`check_command_permission`].
fn has_permission(env: &PluginEnv, permission: PermissionType) -> bool {
    if env.plugin_config.is_builtin() {
        return true;
    }
    env.permissions
        .lock()
        .unwrap()
        .as_ref()
        .map(|granted| granted.contains(&permission))
        .unwrap_or(false)
}

/// Re-spell a real path in the virtual root the plugin used, so that a listing can be fed back in.
fn virtual_path(env: &PluginEnv, real: &Path, requested: &Path) -> PathBuf {
    for (prefix, root) in fs_roots(env).iter() {
        if !requested.starts_with(prefix) && !(requested.is_relative() && *prefix == "/host") {
            continue;
        }
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.clone());
        if let Ok(rest) = real.strip_prefix(&canonical_root) {
            return PathBuf::from(prefix).join(rest);
        }
    }
    real.to_path_buf()
}

fn project_config(
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
        let root = yinz_config::project_config::control_root(&root).map_err(|e| anyhow!(e))?;
        if !root.starts_with(&own) && !has_permission(env, PermissionType::FullHdAccess) {
            return Err(anyhow!(
                "project control checkout outside the plugin directory requires FullHdAccess"
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
    wasi_write_object(env, &response.encode_to_vec())
        .context("could not send project configuration")
        .non_fatal();
}

fn fs_read(env: &mut PluginEnv, path: PathBuf, max_bytes: Option<usize>, offset: Option<u64>) {
    let cap = max_bytes
        .unwrap_or(MAX_FS_READ_BYTES)
        .min(MAX_FS_READ_BYTES);
    let result = resolve_fs_path_with_permission(
        env,
        &path,
        FsIntent::Read,
        FsPathResolution::FollowSymlink,
    )
    .and_then(|real| {
        // A FIFO can replace a file between path resolution and open. Do not let its open
        // block this thread, and check the opened file rather than a stale path's metadata.
        let mut file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
            .open(&real)
            .with_context(|| format!("failed to open '{}'", path.display()))?;
        let metadata = file
            .metadata()
            .with_context(|| format!("failed to inspect '{}'", path.display()))?;
        ensure!(
            metadata.is_file(),
            "'{}' is not a regular file",
            path.display()
        );
        if offset.is_none() && metadata.len() > cap as u64 {
            return Err(anyhow!(
                "'{}' is {} bytes, more than the {} byte limit for a plugin read",
                path.display(),
                metadata.len(),
                cap
            ));
        }
        if let Some(offset) = offset {
            file.seek(SeekFrom::Start(offset))
                .with_context(|| format!("failed to seek '{}'", path.display()))?;
        }
        // Bound the actual read too: the file can grow after metadata was read. The extra
        // byte detects truncation for whole-file reads without buffering the entire file.
        let mut contents = Vec::new();
        file.take((cap + usize::from(offset.is_none())) as u64)
            .read_to_end(&mut contents)
            .with_context(|| format!("failed to read '{}'", path.display()))?;
        ensure!(
            contents.len() <= cap,
            "'{}' grew beyond the {} byte limit for a plugin read",
            path.display(),
            cap
        );
        Ok(contents)
    });

    let response = ProtobufFsReadResponse {
        result: Some(match result {
            Ok(contents) => fs_read_response::Result::Contents(contents),
            Err(e) => fs_read_response::Result::Error(format!("{:#}", e)),
        }),
    };
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| format!("failed to send a file to plugin {}", env.name()))
        .non_fatal();
}

fn fs_list(env: &mut PluginEnv, path: PathBuf) {
    let result = resolve_fs_path_with_permission(
        env,
        &path,
        FsIntent::Read,
        FsPathResolution::FollowSymlink,
    )
    .and_then(|real| {
        let entries =
            fs::read_dir(&real).with_context(|| format!("failed to list '{}'", path.display()))?;
        let mut listed = vec![];
        for entry in entries.flatten() {
            let metadata: Option<FileMetadata> = entry.metadata().ok().map(|m| m.into());
            listed.push(ProtobufFsEntry {
                path: virtual_path(env, &entry.path(), &path)
                    .display()
                    .to_string(),
                // `ProtobufFileMetadata` converts from the `Option`, carrying "unread" in its
                // own `metadata_is_set` flag rather than in the absence of the message.
                metadata: Some(metadata.into()),
            });
        }
        listed.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(listed)
    });

    let response = ProtobufFsListResponse {
        result: Some(match result {
            Ok(entries) => fs_list_response::Result::Entries(ProtobufFsEntryList { entries }),
            Err(e) => fs_list_response::Result::Error(format!("{:#}", e)),
        }),
    };
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| format!("failed to send a listing to plugin {}", env.name()))
        .non_fatal();
}

fn fs_metadata(env: &mut PluginEnv, path: PathBuf) {
    let result = resolve_fs_path_with_permission(
        env,
        &path,
        FsIntent::Read,
        FsPathResolution::PreserveEntry,
    )
    .map(|real| fs::symlink_metadata(&real).ok().map(FileMetadata::from));

    let response = ProtobufFsMetadataResponse {
        result: Some(match result {
            Ok(Some(metadata)) => fs_metadata_response::Result::Metadata(Some(metadata).into()),
            Ok(None) => fs_metadata_response::Result::NotFound(true),
            Err(e) => fs_metadata_response::Result::Error(format!("{:#}", e)),
        }),
    };
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| format!("failed to send metadata to plugin {}", env.name()))
        .non_fatal();
}

fn fs_write(
    env: &mut PluginEnv,
    path: PathBuf,
    contents: Vec<u8>,
    append: bool,
    create_dirs: bool,
) {
    let result = resolve_fs_path_with_permission(
        env,
        &path,
        FsIntent::Write,
        FsPathResolution::FollowSymlink,
    )
    .and_then(|real| {
        if create_dirs {
            if let Some(parent) = real.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create the folders above '{}'", path.display())
                })?;
            }
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .append(append)
            // Resolve legitimate links before opening; refuse a replacement link or a FIFO
            // without waiting for another process. Validate before truncating or writing.
            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
            .open(&real)
            .with_context(|| format!("failed to open '{}' for writing", path.display()))?;
        ensure!(
            file.metadata()
                .with_context(|| format!("failed to inspect '{}'", path.display()))?
                .is_file(),
            "'{}' is not a regular file",
            path.display()
        );
        if !append {
            file.set_len(0)
                .with_context(|| format!("failed to truncate '{}'", path.display()))?;
        }
        file.write_all(&contents)
            .with_context(|| format!("failed to write '{}'", path.display()))
    });
    write_fs_mutate_response(env, result);
}

fn fs_remove(env: &mut PluginEnv, path: PathBuf, recursive: bool) {
    let result = resolve_fs_path_with_permission(
        env,
        &path,
        FsIntent::Write,
        FsPathResolution::PreserveEntry,
    )
    .and_then(|real| {
        let metadata = fs::symlink_metadata(&real)
            .with_context(|| format!("failed to remove '{}'", path.display()))?;
        if metadata.is_dir() {
            if recursive {
                fs::remove_dir_all(&real)
            } else {
                fs::remove_dir(&real)
            }
        } else {
            fs::remove_file(&real)
        }
        .with_context(|| format!("failed to remove '{}'", path.display()))
    });
    write_fs_mutate_response(env, result);
}

fn fs_create_dir(env: &mut PluginEnv, path: PathBuf, all: bool) {
    let result = resolve_fs_path_with_permission(
        env,
        &path,
        FsIntent::Write,
        FsPathResolution::FollowSymlink,
    )
    .and_then(|real| {
        if all {
            fs::create_dir_all(&real)
        } else {
            fs::create_dir(&real)
        }
        .with_context(|| format!("failed to create '{}'", path.display()))
    });
    write_fs_mutate_response(env, result);
}

fn fs_rename(env: &mut PluginEnv, from: PathBuf, to: PathBuf) {
    // Both endpoints are resolved independently: renaming *out* of the plugin's folders is as
    // much of an escape as reading outside them.
    let result = resolve_fs_path_with_permission(
        env,
        &from,
        FsIntent::Write,
        FsPathResolution::PreserveEntry,
    )
    .and_then(|real_from| {
        let real_to = resolve_fs_path_with_permission(
            env,
            &to,
            FsIntent::Write,
            FsPathResolution::PreserveEntry,
        )?;
        fs::rename(&real_from, &real_to).with_context(|| {
            format!(
                "failed to rename '{}' to '{}'",
                from.display(),
                to.display()
            )
        })
    });
    write_fs_mutate_response(env, result);
}

fn write_fs_mutate_response(env: &mut PluginEnv, result: Result<()>) {
    let response = ProtobufFsMutateResponse {
        result: Some(match result {
            Ok(()) => fs_mutate_response::Result::Success(true),
            Err(e) => fs_mutate_response::Result::Error(format!("{:#}", e)),
        }),
    };
    wasi_write_object(env, &response.encode_to_vec())
        .with_context(|| format!("failed to answer a filesystem command for {}", env.name()))
        .non_fatal();
}

fn scan_host_folder(env: &PluginEnv, folder_to_scan: PathBuf) {
    if !folder_to_scan.starts_with("/host") {
        log::error!(
            "Can only scan files in the /host filesystem, found: {}",
            folder_to_scan.display()
        );
        return;
    }
    let plugin_host_folder = env
        .plugin_cwd
        .canonicalize()
        .unwrap_or_else(|_| env.plugin_cwd.clone());
    let folder_to_scan = plugin_host_folder.join(folder_to_scan.strip_prefix("/host").unwrap());
    match folder_to_scan.canonicalize() {
        Ok(folder_to_scan) => {
            if !folder_to_scan.starts_with(&plugin_host_folder) {
                log::error!(
                    "Can only scan files in the plugin filesystem: {}, found: {}",
                    plugin_host_folder.display(),
                    folder_to_scan.display()
                );
                return;
            }
            let reading_folder = std::fs::read_dir(&folder_to_scan);
            match reading_folder {
                Ok(reading_folder) => {
                    let send_plugin_instructions = env.senders.to_plugin.clone();
                    let update_target = env.plugin_id;
                    let deno_generation = env.deno_generation;
                    let client_id = env.client_id;
                    thread::spawn({
                        move || {
                            let mut paths_in_folder = vec![];
                            for entry in reading_folder.flatten() {
                                let entry_metadata = entry.metadata().ok().map(|m| m.into());
                                paths_in_folder.push((
                                    PathBuf::from("/host").join(
                                        entry.path().strip_prefix(&plugin_host_folder).unwrap(),
                                    ),
                                    entry_metadata,
                                ));
                            }
                            send_plugin_instructions
                                .ok_or(anyhow!("found no sender to send plugin instruction to"))
                                .map(|sender| {
                                    let _ = sender.send(PluginInstruction::for_plugin_event(
                                        update_target,
                                        client_id,
                                        deno_generation,
                                        Event::FileSystemUpdate(paths_in_folder),
                                    ));
                                })
                                .non_fatal();
                        }
                    });
                },
                Err(e) => {
                    log::error!("Failed to read folder {}: {e}", folder_to_scan.display());
                },
            }
        },
        Err(e) => {
            log::error!(
                "Failed to canonicalize path {folder_to_scan:?} when scanning folder: {:?}",
                e
            );
        },
    }
}

fn resize_pane_with_id(env: &PluginEnv, resize: ResizeStrategy, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::ResizePaneWithId(resize, pane_id));
}

fn edit_scrollback_for_pane_with_id(env: &PluginEnv, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::EditScrollbackForPaneWithId(
            pane_id, None,
        ));
}

fn get_pane_scrollback(env: &mut PluginEnv, pane_id: PaneId, get_full_scrollback: bool) {
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
            wasi_write_object(env, &serialized.encode_to_vec())?;
            Ok(())
        })
        .with_context(err_context)
        .non_fatal();
}

fn write_to_pane_id(env: &PluginEnv, bytes: Vec<u8>, pane_id: PaneId) {
    let (completion_tx, completion_rx) = oneshot::channel();
    let send_result = env.senders.send_to_screen(ScreenInstruction::WriteToPaneId(
        bytes,
        pane_id,
        Some(NotificationEnd::new(completion_tx)),
    ));
    if send_result.is_ok() {
        let wait_forever = false;
        let _ = wait_for_action_completion(completion_rx, "write_to_pane_id", wait_forever);
    }
}

fn write_chars_to_pane_id(env: &PluginEnv, chars: String, pane_id: PaneId) {
    let bytes = chars.into_bytes();
    let (completion_tx, completion_rx) = oneshot::channel();
    let send_result = env.senders.send_to_screen(ScreenInstruction::WriteToPaneId(
        bytes,
        pane_id,
        Some(NotificationEnd::new(completion_tx)),
    ));
    if send_result.is_ok() {
        let wait_forever = false;
        let _ = wait_for_action_completion(completion_rx, "write_chars_to_pane_id", wait_forever);
    }
}

fn send_sigint_to_pane_id(env: &PluginEnv, pane_id: PaneId) {
    let err_context = || {
        format!(
            "failed to send SIGINT to pane {:?} from plugin {}",
            pane_id,
            env.name()
        )
    };

    env.senders
        .send_to_pty(PtyInstruction::SendSigintToPaneId(pane_id))
        .with_context(err_context)
        .non_fatal();
}

fn send_sigkill_to_pane_id(env: &PluginEnv, pane_id: PaneId) {
    let err_context = || {
        format!(
            "failed to send SIGKILL to pane {:?} from plugin {}",
            pane_id,
            env.name()
        )
    };

    env.senders
        .send_to_pty(PtyInstruction::SendSigkillToPaneId(pane_id))
        .with_context(err_context)
        .non_fatal();
}

fn get_pane_pid(env: &mut PluginEnv, pane_id: PaneId) {
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

    // Encode response and write to plugin's stdin
    let protobuf_response = ProtobufGetPanePidResponse::from(response);
    let serialized = protobuf_response.encode_to_vec();
    wasi_write_object(env, &serialized)
        .with_context(err_context)
        .non_fatal();
}

fn get_pane_running_command(env: &mut PluginEnv, pane_id: PaneId) {
    let (sender, receiver) = crossbeam::channel::bounded(1);

    send_get_pane_running_command_request(env, pane_id, sender);
    let response = await_pane_running_command_response(env, pane_id, receiver);
    write_pane_running_command_response(env, pane_id, response);
}

fn get_disconnect_targets(env: &mut PluginEnv) {
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
            client_ids: targets.into_iter().map(|id| u32::from(id.get())).collect(),
            error: None,
        },
        Err(error) => ProtobufGetDisconnectTargetsResponse {
            client_ids: vec![],
            error: Some(format!("{error:#}")),
        },
    };
    wasi_write_object(env, &response.encode_to_vec())
        .context("write disconnect-target response")
        .non_fatal();
}

fn get_session_list(env: &mut PluginEnv) {
    use yinz_bus::background_jobs::{scan_session_list_default_dirs, session_scan_state};
    use {yinz_types::data::GetSessionListResponse, yinz_types::data::SessionListSnapshot};

    let response = match session_scan_state() {
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
            GetSessionListResponse::Ok(snapshot)
        },
        None => GetSessionListResponse::Err("Session-scan state not initialized".to_string()),
    };

    let protobuf_response = ProtobufGetSessionListResponse::from(response);
    wasi_write_object(env, &protobuf_response.encode_to_vec())
        .with_context(|| "failed to write get_session_list response".to_string())
        .non_fatal();
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
    wasi_write_object(env, &protobuf_response.encode_to_vec())
        .with_context(|| format!("Failed to get running command for pane {:?}", pane_id))
        .non_fatal();
}

fn get_pane_cwd(env: &mut PluginEnv, pane_id: PaneId) {
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
    wasi_write_object(env, &protobuf_response.encode_to_vec())
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
fn get_pane_git_probe(
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
    wasi_write_object(env, &protobuf_response.encode_to_vec())
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

fn save_layout(env: &mut PluginEnv, layout_name: String, layout_kdl: String, overwrite: bool) {
    let err_context = || format!("Failed to save layout '{}'", layout_name);

    let response = try_save_layout(env, &layout_name, &layout_kdl, overwrite)
        .map(|_| SaveLayoutResponse::Ok(()))
        .unwrap_or_else(SaveLayoutResponse::Err);

    // Serialize and send response back to plugin
    let protobuf_response = ProtobufSaveLayoutResponse::from(response);
    let serialized = protobuf_response.encode_to_vec();
    wasi_write_object(env, &serialized)
        .with_context(err_context)
        .non_fatal();
}

fn try_save_layout(
    env: &PluginEnv,
    layout_name: &str,
    layout_kdl: &str,
    overwrite: bool,
) -> Result<(), String> {
    // Step 1: Sanitize layout name for path traversal and invalid characters
    let safe_name = sanitize_layout_name(layout_name)?;

    // Step 2: Validate the layout by parsing it
    yinz_config::kdl::layout::from_kdl(
        layout_kdl,
        Some(format!("{}.kdl", safe_name)),
        None, // No swap layouts
        None, // No cwd override
    )
    .map_err(|config_error| format!("Invalid layout KDL: {:?}", config_error))?;

    // Step 3: Get layout_dir from PluginEnv
    let layout_dir = env
        .layout_dir
        .clone()
        .or_else(default_layout_dir)
        .ok_or_else(|| "Layout directory not found".to_string())?;

    let file_path = layout_dir.join(format!("{}.kdl", safe_name));
    std::fs::create_dir_all(&layout_dir)
        .map_err(|e| format!("Failed to create layout directory: {}", e))?;

    let mut parsed_layout: KdlDocument = layout_kdl
        .parse()
        .map_err(|e| format!("Invalid layout KDL: {}", e))?;
    parsed_layout.fmt();
    // Publish a complete file, without following a destination symlink or racing another save.
    let mut staged = tempfile::NamedTempFile::new_in(&layout_dir)
        .map_err(|e| format!("Failed to stage layout file: {}", e))?;
    staged
        .write_all(parsed_layout.to_string().as_bytes())
        .and_then(|_| staged.as_file().sync_all())
        .map_err(|e| format!("Failed to write layout file: {}", e))?;
    let published = if overwrite {
        staged.persist(&file_path)
    } else {
        staged.persist_noclobber(&file_path)
    };
    published.map_err(|e| {
        if e.error.kind() == std::io::ErrorKind::AlreadyExists {
            format!(
                "Layout file '{}' already exists. Use overwrite flag to replace it.",
                safe_name
            )
        } else {
            format!("Failed to publish layout file: {}", e)
        }
    })?;

    Ok(())
}

fn delete_layout(env: &mut PluginEnv, layout_name: String) {
    let err_context = || format!("Failed to delete layout '{}'", layout_name);

    let response = try_delete_layout(env, &layout_name)
        .map(|_| DeleteLayoutResponse::Ok(()))
        .unwrap_or_else(DeleteLayoutResponse::Err);

    let protobuf_response: ProtobufDeleteLayoutResponse = response.into();
    let serialized = protobuf_response.encode_to_vec();
    wasi_write_object(env, &serialized)
        .with_context(err_context)
        .non_fatal();
}

fn rename_layout(env: &mut PluginEnv, old_layout_name: String, new_layout_name: String) {
    let err_context = || {
        format!(
            "Failed to rename layout '{}' to '{}'",
            old_layout_name, new_layout_name
        )
    };

    let response = try_rename_layout(env, &old_layout_name, &new_layout_name)
        .map(|_| RenameLayoutResponse::Ok(()))
        .unwrap_or_else(RenameLayoutResponse::Err);

    let protobuf_response: ProtobufRenameLayoutResponse = response.into();
    let serialized = protobuf_response.encode_to_vec();
    wasi_write_object(env, &serialized)
        .with_context(err_context)
        .non_fatal();
}

fn try_delete_layout(env: &PluginEnv, layout_name: &str) -> Result<(), String> {
    // Sanitize the layout name to prevent directory traversal
    let safe_name =
        sanitize_layout_name(layout_name).map_err(|e| format!("Invalid layout name: {}", e))?;

    // Get the layout directory from PluginEnv
    let layout_dir = env
        .layout_dir
        .clone()
        .or_else(default_layout_dir)
        .ok_or_else(|| "Layout directory not found".to_string())?;

    // Construct the full file path
    let file_path = layout_dir.join(format!("{}.kdl", safe_name));

    // Check if the file exists
    if !file_path.exists() {
        return Err(format!("Layout '{}' not found", safe_name));
    }

    // Delete the file
    std::fs::remove_file(&file_path).map_err(|e| format!("Failed to delete layout file: {}", e))?;

    Ok(())
}

fn try_rename_layout(
    env: &PluginEnv,
    old_layout_name: &str,
    new_layout_name: &str,
) -> Result<(), String> {
    // Step 1: Sanitize both layout names
    let safe_old_name = sanitize_layout_name(old_layout_name)
        .map_err(|e| format!("Invalid old layout name: {}", e))?;

    let safe_new_name = sanitize_layout_name(new_layout_name)
        .map_err(|e| format!("Invalid new layout name: {}", e))?;

    // Step 2: Get layout directory from PluginEnv
    let layout_dir = env
        .layout_dir
        .clone()
        .or_else(default_layout_dir)
        .ok_or_else(|| "Layout directory not found".to_string())?;

    // Step 3: Construct file paths
    let old_file_path = layout_dir.join(format!("{}.kdl", safe_old_name));
    let new_file_path = layout_dir.join(format!("{}.kdl", safe_new_name));

    // Step 4: Check if source file exists
    if !old_file_path.exists() {
        return Err(format!("Layout '{}' not found", safe_old_name));
    }

    rename_layout_file(&old_file_path, &new_file_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            format!(
                "Layout '{}' already exists. Cannot rename '{}' to existing layout name.",
                safe_new_name, safe_old_name
            )
        } else {
            format!("Failed to rename layout file: {error}")
        }
    })?;
    Ok(())
}

fn rename_layout_file(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        use nix::fcntl::{renameat2, RenameFlags, AT_FDCWD};
        renameat2(AT_FDCWD, from, AT_FDCWD, to, RenameFlags::RENAME_NOREPLACE)
            .map_err(std::io::Error::from)
    }
    #[cfg(any(target_os = "macos", all(target_os = "linux", not(target_env = "gnu"))))]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let from = CString::new(from.as_os_str().as_bytes())?;
        let to = CString::new(to.as_os_str().as_bytes())?;
        // Both paths are valid NUL-terminated strings and remain alive for the syscall.
        #[cfg(target_os = "macos")]
        let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
        #[cfg(target_os = "linux")]
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (from, to);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "atomic layout rename without replacement is unavailable on this platform",
        ))
    }
}

fn edit_layout(env: &mut PluginEnv, layout_name: String, context: BTreeMap<String, String>) {
    let err_context = || format!("Failed to edit layout '{}'", layout_name);

    let response = try_edit_layout(env, &layout_name, context)
        .map(|_| EditLayoutResponse::Ok(()))
        .unwrap_or_else(EditLayoutResponse::Err);

    let protobuf_response: ProtobufEditLayoutResponse = response.into();
    let serialized = protobuf_response.encode_to_vec();
    wasi_write_object(env, &serialized)
        .with_context(err_context)
        .non_fatal();
}

fn try_edit_layout(
    env: &PluginEnv,
    layout_name: &str,
    context: BTreeMap<String, String>,
) -> Result<(), String> {
    // Sanitize the layout name to prevent directory traversal
    let safe_name =
        sanitize_layout_name(layout_name).map_err(|e| format!("Invalid layout name: {}", e))?;

    // Get the layout directory from PluginEnv
    let layout_dir = env
        .layout_dir
        .clone()
        .or_else(default_layout_dir)
        .ok_or_else(|| "Layout directory not found".to_string())?;

    // Construct the full file path
    let file_path = layout_dir.join(format!("{}.kdl", safe_name));

    // Create FileToOpen for the layout file
    let file_to_open = FileToOpen {
        path: file_path,
        line_number: None,
        cwd: Some(layout_dir.clone()),
    };

    // Create an Action::EditFile
    let action = Action::EditFile {
        payload: OpenFilePayload::new(
            file_to_open.path,
            file_to_open.line_number,
            file_to_open.cwd,
        )
        .with_originating_plugin(env.originating_plugin(context)),
        direction: None,
        floating: false,
        in_place: true,
        close_replaced_pane: false,
        start_suppressed: false,
        placement: NewPanePlacement::Floating(None),
        near_current_pane: true,
        no_focus: false,
        tab_id: None,
    };
    // Route the action - this is fallible
    route_action(
        action,
        env.client_id,
        None,
        Some(PaneId::Plugin(env.plugin_id)),
        env.senders.clone(),
        env.default_shell.clone(),
        None,
        env.default_mode,
        None,
        false,
        false, // is_client_input
    )
    .map(|_| ())
    .map_err(|e| format!("Failed to route edit action: {:?}", e))
}

/// Sanitize layout name to prevent path traversal and invalid filenames
/// Returns the sanitized name or an error message
fn sanitize_layout_name(name: &str) -> Result<String, String> {
    // Check for empty name
    if name.is_empty() {
        return Err("Layout name cannot be empty".to_string());
    }

    // Check for path separators (prevent directory traversal)
    if name.contains('/') || name.contains('\\') {
        return Err("Layout name cannot contain path separators".to_string());
    }

    // Check for parent directory references
    if name.contains("..") {
        return Err("Layout name cannot contain '..'".to_string());
    }

    // Check length (most filesystems have 255 char limit, leave room for .kdl)
    if name.len() > 250 {
        return Err("Layout name too long (max 250 characters)".to_string());
    }

    // Only allow alphanumeric, hyphens, underscores, and spaces
    let valid_chars = name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == ' ');

    if !valid_chars {
        return Err(
            "Layout name can only contain letters, numbers, hyphens, underscores, and spaces"
                .to_string(),
        );
    }

    Ok(name.to_string())
}

fn move_pane_with_pane_id(env: &PluginEnv, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::MovePaneWithPaneId(pane_id));
}

fn move_pane_with_pane_id_in_direction(env: &PluginEnv, pane_id: PaneId, direction: Direction) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::MovePaneWithPaneIdInDirection(
            pane_id, direction,
        ));
}

fn clear_screen_for_pane_id(env: &PluginEnv, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::ClearScreenForPaneId(pane_id));
}

fn scroll_up_in_pane_id(env: &PluginEnv, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::ScrollUpInPaneId(pane_id));
}

fn scroll_down_in_pane_id(env: &PluginEnv, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::ScrollDownInPaneId(pane_id));
}

fn scroll_to_top_in_pane_id(env: &PluginEnv, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::ScrollToTopInPaneId(pane_id));
}

fn scroll_to_bottom_in_pane_id(env: &PluginEnv, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::ScrollToBottomInPaneId(pane_id));
}

fn page_scroll_up_in_pane_id(env: &PluginEnv, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::PageScrollUpInPaneId(pane_id));
}

fn page_scroll_down_in_pane_id(env: &PluginEnv, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::PageScrollDownInPaneId(pane_id));
}

fn toggle_pane_id_fullscreen(env: &PluginEnv, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::TogglePaneIdFullscreen(pane_id));
}

fn toggle_pane_embed_or_eject_for_pane_id(env: &PluginEnv, pane_id: PaneId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::TogglePaneEmbedOrEjectForPaneId(pane_id));
}

fn close_tab_with_index(env: &PluginEnv, tab_index: TabPosition) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::CloseTabWithIndex(tab_index));
}

fn break_panes_to_new_tab(
    env: &mut PluginEnv,
    pane_ids: Vec<PaneId>,
    new_tab_name: Option<String>,
    should_change_focus_to_new_tab: bool,
) {
    let default_shell = env.default_shell.clone().or_else(|| {
        Some(TerminalAction::RunCommand(RunCommand {
            command: env.path_to_default_shell.clone(),
            use_terminal_title: true,
            ..Default::default()
        }))
    });

    // Create completion channel to receive tab ID
    let (tx, rx) = tokio::sync::oneshot::channel();
    let completion_tx = Some(NotificationEnd::new(tx));

    let result = env
        .senders
        .send_to_screen(ScreenInstruction::BreakPanesToNewTab {
            pane_ids,
            default_shell,
            new_tab_name,
            should_change_focus_to_new_tab,
            client_id: env.client_id,
            completion_tx,
        });

    let tab_id: BreakPanesToNewTabResponse = if result.is_ok() {
        wait_for_action_completion(rx, "break_panes_to_new_tab", false).affected_tab_id
    } else {
        None
    };

    let response = ProtobufBreakPanesToNewTabResponse::from(tab_id);
    wasi_write_object(env, &response.encode_to_vec()).non_fatal();
}

fn break_panes_to_tab_with_index(
    env: &mut PluginEnv,
    pane_ids: Vec<PaneId>,
    should_change_focus_to_new_tab: bool,
    tab_index: TabPosition,
) {
    // Create completion channel to receive tab ID
    let (tx, rx) = tokio::sync::oneshot::channel();
    let completion_tx = Some(NotificationEnd::new(tx));

    let result = env
        .senders
        .send_to_screen(ScreenInstruction::BreakPanesToTabWithIndex {
            pane_ids,
            tab_index,
            client_id: env.client_id,
            should_change_focus_to_new_tab,
            completion_tx,
        });

    let tab_id: BreakPanesToTabWithIndexResponse = if result.is_ok() {
        wait_for_action_completion(rx, "break_panes_to_tab_with_index", false).affected_tab_id
    } else {
        None
    };

    let response = ProtobufBreakPanesToTabWithIndexResponse::from(tab_id);
    wasi_write_object(env, &response.encode_to_vec()).non_fatal();
}

fn switch_tab_to_id(env: &PluginEnv, tab_id: TabId) {
    env.senders
        .send_to_screen(ScreenInstruction::GoToTabWithId(
            tab_id,
            Some(env.client_id),
            None,
        ))
        .with_context(|| {
            format!(
                "failed to switch to tab {} from plugin {}",
                tab_id,
                env.name()
            )
        })
        .non_fatal();
}

fn go_to_tab_with_id(env: &PluginEnv, tab_id: TabId) {
    env.senders
        .send_to_screen(ScreenInstruction::GoToTabWithId(
            tab_id,
            Some(env.client_id),
            None,
        ))
        .with_context(|| format!("failed to go to tab {} from plugin {}", tab_id, env.name()))
        .non_fatal();
}

fn close_tab_with_id(env: &PluginEnv, tab_id: TabId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::CloseTabWithId(tab_id, None));
}

fn set_tab_projects(env: &PluginEnv, tab_projects: BTreeMap<TabId, String>) {
    env.senders
        .send_to_screen(ScreenInstruction::SetTabProjects(tab_projects))
        .context("Failed to set tab projects")
        .non_fatal();
}

fn rename_tab_with_id(env: &PluginEnv, tab_id: TabId, new_name: &str) {
    let rename_tab_action =
        ScreenInstruction::RenameTabWithId(tab_id, new_name.as_bytes().to_vec(), None);
    env.senders
        .send_to_screen(rename_tab_action)
        .with_context(|| format!("Failed to rename tab {}", tab_id))
        .non_fatal();
}

fn break_panes_to_tab_with_id(
    env: &mut PluginEnv,
    pane_ids: Vec<PaneId>,
    should_change_focus_to_target_tab: bool,
    tab_id: TabId,
) {
    // Create completion channel to receive tab ID
    let (tx, rx) = tokio::sync::oneshot::channel();
    let completion_tx = Some(NotificationEnd::new(tx));

    let result = env
        .senders
        .send_to_screen(ScreenInstruction::BreakPanesToTabWithId {
            pane_ids,
            tab_id,
            should_change_focus_to_target_tab,
            client_id: env.client_id,
            completion_tx,
        });

    let result_tab_id: BreakPanesToTabWithIdResponse = if result.is_ok() {
        wait_for_action_completion(rx, "break_panes_to_tab_with_id", false).affected_tab_id
    } else {
        None
    };

    let response = ProtobufBreakPanesToTabWithIdResponse::from(result_tab_id);
    wasi_write_object(env, &response.encode_to_vec()).non_fatal();
}

fn reload_plugin(env: &PluginEnv, plugin_id: PluginId) {
    let _ = env
        .senders
        .send_to_plugin(PluginInstruction::ReloadPluginWithId(plugin_id));
}

fn load_new_plugin(
    env: &PluginEnv,
    url: String,
    config: BTreeMap<String, String>,
    load_in_background: bool,
    skip_plugin_cache: bool,
) {
    let url = if &url == "yinz:OWN_URL" {
        env.plugin_config.location.display()
    } else {
        url
    };
    if load_in_background {
        match yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
            &url,
            &Some(config),
            None,
            Some(env.plugin_cwd.clone()),
        ) {
            Ok(run_plugin_or_alias) => {
                let _ = env
                    .senders
                    .send_to_plugin(PluginInstruction::LoadBackgroundPlugin(
                        run_plugin_or_alias,
                        env.client_id,
                    ));
            },
            Err(e) => {
                log::error!("Failed to load new plugin: {:?}", e);
            },
        }
    } else {
        let should_float = Some(true);
        let should_be_open_in_place = false;
        let pane_title = None;
        let tab_index = None;
        let pane_id_to_replace = None;
        let client_id = env.client_id;
        let size = Default::default();
        let cwd = Some(env.plugin_cwd.clone());
        let skip_cache = skip_plugin_cache;
        match yinz_runtime::plugin_location::run_plugin_or_alias_from_url(
            &url,
            &Some(config),
            None,
            Some(env.plugin_cwd.clone()),
        ) {
            Ok(run_plugin_or_alias) => {
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
            },
            Err(e) => {
                log::error!("Failed to load new plugin: {:?}", e);
            },
        }
    }
}

fn group_and_ungroup_panes(
    env: &PluginEnv,
    panes_to_group: Vec<PaneId>,
    panes_to_ungroup: Vec<PaneId>,
    for_all_clients: bool,
) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::GroupAndUngroupPanes(
            panes_to_group,
            panes_to_ungroup,
            for_all_clients,
            env.client_id,
        ));
}

fn highlight_and_unhighlight_panes(
    env: &PluginEnv,
    panes_to_highlight: Vec<PaneId>,
    panes_to_unhighlight: Vec<PaneId>,
) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::HighlightAndUnhighlightPanes(
            panes_to_highlight,
            panes_to_unhighlight,
            env.client_id,
        ));
}

fn set_related_panes(env: &PluginEnv, pane_ids: HashSet<PaneId>) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::SetRelatedPanes(pane_ids));
}

fn reveal_pane_with_id(env: &PluginEnv, pane_id: PaneId, label: Option<String>) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::RevealPaneWithId(
            pane_id,
            label,
            env.client_id,
        ));
}

fn set_pane_title_prefix(env: &PluginEnv, pane_id: PaneId, prefix: Option<String>) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::SetPaneTitlePrefix(pane_id, prefix));
}
fn set_pane_subtitle(env: &PluginEnv, pane_id: PaneId, subtitle: Option<String>) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::SetPaneSubtitle(pane_id, subtitle));
}

fn close_multiple_panes(env: &PluginEnv, pane_ids: Vec<PaneId>) {
    for pane_id in pane_ids {
        match pane_id {
            PaneId::Terminal(terminal_pane_id) => {
                close_terminal_pane(env, terminal_pane_id);
            },
            PaneId::Plugin(plugin_pane_id) => {
                close_plugin_pane(env, plugin_pane_id);
            },
        }
    }
}

fn float_multiple_panes(env: &PluginEnv, pane_ids: Vec<PaneId>) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::FloatMultiplePanes(
            pane_ids,
            env.client_id,
        ));
}

fn embed_multiple_panes(env: &PluginEnv, pane_ids: Vec<PaneId>) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::EmbedMultiplePanes(
            pane_ids,
            env.client_id,
        ));
}

fn set_self_mouse_selection_support(env: &PluginEnv, selection_support: bool) {
    env.senders
        .send_to_screen(ScreenInstruction::SetMouseSelectionSupport(
            PaneId::Plugin(env.plugin_id),
            selection_support,
        ))
        .with_context(|| {
            format!(
                "failed to set plugin {} selectable from plugin {}",
                selection_support,
                env.name()
            )
        })
        .non_fatal();
}

fn intercept_key_presses(env: &mut PluginEnv) {
    env.intercepting_key_presses = true;
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::InterceptKeyPresses(
            env.plugin_id,
            env.client_id,
        ));
}

fn clear_key_presses_intercepts(env: &mut PluginEnv) {
    env.intercepting_key_presses = false;
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::ClearKeyPressesIntercepts(
            env.plugin_id,
            env.client_id,
        ));
}

fn replace_pane_with_existing_pane(
    env: &mut PluginEnv,
    pane_to_replace: PaneId,
    existing_pane: PaneId,
    suppress_replaced_pane: bool,
) {
    let (completion_tx, completion_rx) = oneshot::channel();
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::ReplacePaneWithExistingPane(
            pane_to_replace,
            existing_pane,
            suppress_replaced_pane,
            Some(NotificationEnd::new(completion_tx)),
        ));
    let _ = wait_for_action_completion(completion_rx, "replace_pane_with_existing_pane", false);
}

fn override_layout(
    env: &mut PluginEnv,
    layout_info: LayoutInfo,
    retain_existing_terminal_panes: bool,
    retain_existing_plugin_panes: bool,
    apply_only_to_active_tab: bool,
    context: BTreeMap<String, String>,
) -> Result<()> {
    let layout = layout_from_plugin_info(env, layout_info)?;

    // Convert all tabs to Vec<TabLayoutInfo>
    let tabs: Vec<TabLayoutInfo> = layout
        .tabs
        .iter()
        .enumerate()
        .map(|(index, (tab_name, tiled, floating))| TabLayoutInfo {
            tab_index: TabPosition::new(index),
            tab_name: tab_name.clone(),
            tiled_layout: tiled.clone(),
            floating_layouts: floating.clone(),
            swap_tiled_layouts: Some(layout.swap_tiled_layouts.clone()),
            swap_floating_layouts: Some(layout.swap_floating_layouts.clone()),
        })
        .collect();

    // If no tabs, create default tab
    let tabs = if tabs.is_empty() {
        let (tiled, floating) = layout.new_tab();
        vec![TabLayoutInfo {
            tab_index: TabPosition::new(0),
            tab_name: None,
            tiled_layout: tiled,
            floating_layouts: floating,
            swap_tiled_layouts: Some(layout.swap_tiled_layouts),
            swap_floating_layouts: Some(layout.swap_floating_layouts),
        }]
    } else {
        tabs
    };

    let action = Action::OverrideLayout {
        tabs,
        retain_existing_terminal_panes,
        retain_existing_plugin_panes,
        apply_only_to_active_tab,
    };
    run_action(env, action, context);
    Ok(())
}

// Custom panic handler for plugins.
//
// This is called when a panic occurs in a plugin. Since most panics will likely originate in the
// code trying to deserialize an `Event` upon a plugin state update, we read some panic message,
// formatted as string from the plugin.
fn report_panic(env: &PluginEnv, msg: &str) {
    log::error!("PANIC IN PLUGIN!\n\r{}", msg);
    handle_plugin_crash(
        env.plugin_id,
        env.deno_generation,
        Some(env.instance_id),
        msg.to_owned(),
        env.senders.clone(),
    );
}

// Helper Functions ---------------------------------------------------------------------------------------------------

pub fn wasi_read_string(plugin_env: &PluginEnv) -> Result<String> {
    let err_context = || "failed to read string from WASI env".to_string();

    let mut buf = vec![];
    plugin_env
        .stdout_pipe
        .lock()
        .unwrap()
        .read_to_end(&mut buf)
        .map_err(anyError::new)
        .with_context(err_context)?;
    let buf = String::from_utf8_lossy(&buf);

    // https://stackoverflow.com/questions/66450942/in-rust-is-there-a-way-to-make-literal-newlines-in-r-using-windows-c
    Ok(buf.replace("\n", "\n\r"))
}

pub fn wasi_write_string(plugin_env: &mut PluginEnv, buf: &str) -> Result<()> {
    if let ResponseSink::Capture(captured) = &mut plugin_env.response_sink {
        *captured = Some(buf.to_owned());
        return Ok(());
    }
    let mut stdin = plugin_env.stdin_pipe.lock().unwrap();
    writeln!(stdin, "{}\r", buf)
        .map_err(anyError::new)
        .with_context(|| "failed to write string to WASI env".to_string())
}

pub fn wasi_write_object(
    plugin_env: &mut PluginEnv,
    object: &(impl Serialize + ?Sized),
) -> Result<()> {
    serde_json::to_string(&object)
        .map_err(anyError::new)
        .and_then(|string| wasi_write_string(plugin_env, &string))
        .with_context(|| "failed to serialize object for WASI env".to_string())
}

pub fn wasi_read_bytes(plugin_env: &PluginEnv) -> Result<Vec<u8>> {
    wasi_read_string(plugin_env)
        .and_then(|string| serde_json::from_str(&string).map_err(anyError::new))
        .with_context(|| "failed to deserialize object from WASI env".to_string())
}

/// Whether this plugin has asked for permissions and is still waiting to hear back.
///
/// The window is real and every plugin passes through it: the grant, cached or freshly prompted
/// for, arrives as a `PermissionRequestResult` event, and `load` runs before any event does, so
/// a plugin that asks in `load` is refused everything for the rest of it however long ago the
/// permission was granted. A denial made in that window has a different fix from one made after
/// (wait for the answer, rather than ask), and a different fix again from the one a plugin that
/// never asked at all needs, which is why the request is tracked separately from its answer.
/// Only the Deno runtime can pass the difference on: a wasm guest is mid-call
/// and cannot be handed a refusal at all.
pub(crate) fn permission_answer_pending(plugin_env: &PluginEnv) -> bool {
    !plugin_env.plugin_config.is_builtin()
        && plugin_env
            .permissions_requested
            .load(atomic::Ordering::SeqCst)
        && plugin_env.permissions.lock().unwrap().is_none()
}

/// Whether `plugin_env` may run `command`, and which permission it is short of if not.
///
/// `pub(crate)` for the TypeScript runtime, which asks before dispatching. A wasm plugin cannot be told
/// no - the denied branch of [`run_plugin_command`] logs and returns, because the guest is
/// mid-call and has no way to receive a refusal - but a TypeScript plugin's command is an ordinary
/// function call that can raise.
pub(crate) fn check_command_permission(
    plugin_env: &PluginEnv,
    command: &PluginCommand,
) -> (PermissionStatus, Option<PermissionType>) {
    if matches!(command, PluginCommand::Pick(command) if !matches!(command, yinz_types::pick::PickCommand::Start { .. }))
    {
        return (PermissionStatus::Granted, None);
    }
    if matches!(command, PluginCommand::GetPluginInfo(id) if id.is_none_or(|id| id == plugin_env.plugin_id))
    {
        return (PermissionStatus::Granted, None);
    }
    if matches!(command, PluginCommand::SwampRequest { .. })
        && !plugin_env.plugin_config.is_builtin()
    {
        return (PermissionStatus::Denied, Some(PermissionType::RunCommands));
    }
    if plugin_env.plugin_config.is_builtin() {
        // built-in plugins can do all the things because they're part of the application and
        // there's no use to deny them anything
        return (PermissionStatus::Granted, None);
    }
    if matches!(
        command,
        PluginCommand::PaneTabs(yinz_types::pane_tabs::PaneTabsCommand::Mount { .. })
    ) && !plugin_env
        .permissions
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|permissions| permissions.contains(&PermissionType::OpenTerminalsOrPlugins))
    {
        return (
            PermissionStatus::Denied,
            Some(PermissionType::OpenTerminalsOrPlugins),
        );
    }
    let children: &[yinz_types::multipane::MultipaneChild] = match command {
        PluginCommand::CreateMultipane(request) => &request.children,
        PluginCommand::ReplaceMultipaneChild(request) => std::slice::from_ref(&request.child),
        _ => &[],
    };
    for child in children {
        let permission = match child {
            yinz_types::multipane::MultipaneChild::Existing(_) => continue,
            yinz_types::multipane::MultipaneChild::Terminal(_) => PermissionType::RunCommands,
            yinz_types::multipane::MultipaneChild::Plugin { .. } => {
                PermissionType::OpenTerminalsOrPlugins
            },
        };
        if !plugin_env
            .permissions
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|permissions| permissions.contains(&permission))
        {
            return (PermissionStatus::Denied, Some(permission));
        }
    }
    // A syntax check of an inline source reads no file - the handler follows imports only in
    // `Semantic` mode - so it needs no permission.
    if let PluginCommand::CheckPlugin {
        mode: yinz_types::authoring::PluginCheckMode::Syntax,
        ..
    } = command
    {
        return (PermissionStatus::Granted, None);
    }
    // `None` is an explicit verdict from an exhaustive match — the command needs no permission —
    // never "not yet classified": a new variant fails to compile in `command_permission`.
    let Some(permission) = command_permission(CommandType::from(command)) else {
        return (PermissionStatus::Granted, None);
    };

    if let Some(permissions) = plugin_env.permissions.lock().unwrap().as_ref() {
        if permissions.contains(&permission) {
            return (PermissionStatus::Granted, None);
        }
    }

    (PermissionStatus::Denied, Some(permission))
}

/// The permission a command needs, or `None` when it needs none.
///
/// The match is exhaustive on purpose: `check_command_permission` grants a `None`, so a catch-all
/// arm would make every variant added later fail open. Each `None` says why below.
fn command_permission(kind: CommandType) -> Option<PermissionType> {
    Some(match kind {
        // Bootstrap and the plugin's own pane: nothing here reaches outside the plugin.
        CommandType::ResolveNotification
        | CommandType::RequestPluginPermissions
        | CommandType::Subscribe
        | CommandType::Unsubscribe
        | CommandType::SetSelectable
        | CommandType::SetSelfMouseSelectionSupport
        | CommandType::ShowCursor
        | CommandType::HideSelf
        | CommandType::ShowSelf
        | CommandType::CloseSelf
        | CommandType::SetTimeout
        | CommandType::ReportPanic
        | CommandType::GetPluginIds
        | CommandType::GetYinzVersion => return None,
        // A message to the plugin's own worker, and the worker's reply to the plugin: both stay
        // inside the plugin (`post_message_to_plugin` always addresses `env.plugin_id`).
        CommandType::PostMessageTo | CommandType::PostMessageToPlugin => return None,
        // The host folder is preopened to the plugin as `/host` at load, so listing or watching
        // it grants nothing the WASI mount does not. Moving it is `ChangeHostFolder`, below.
        CommandType::ScanHostFolder | CommandType::WatchFilesystem => return None,
        CommandType::OpenFile
        | CommandType::OpenFileFloating
        | CommandType::OpenFileNearPlugin
        | CommandType::OpenFileFloatingNearPlugin
        | CommandType::OpenFileInPlaceOfPlugin
        | CommandType::OpenFileInPlace
        | CommandType::OpenEditPaneInPlaceOfPaneId => PermissionType::OpenFiles,
        CommandType::OpenTerminal
        | CommandType::OpenTerminalNearPlugin
        | CommandType::StartOrReloadPlugin
        | CommandType::OpenTerminalFloating
        | CommandType::OpenTerminalFloatingNearPlugin
        | CommandType::OpenTerminalInPlace
        | CommandType::OpenTerminalInPlaceOfPlugin
        | CommandType::OpenPluginPaneInNewTab
        | CommandType::OpenPluginPaneFloating
        | CommandType::LoadNewPlugin
        | CommandType::NewTiledPaneInTab
        | CommandType::OpenTerminalPaneInPlaceOfPaneId => PermissionType::OpenTerminalsOrPlugins,
        CommandType::OpenCommandPane
        | CommandType::OpenCommandPaneNearPlugin
        | CommandType::OpenCommandPaneInGitCheckoutNearPlugin
        | CommandType::OpenCommandPaneFloating
        | CommandType::OpenCommandPaneOverlay
        | CommandType::OpenCommandPaneFloatingNearPlugin
        | CommandType::OpenCommandPaneInPlace
        | CommandType::OpenCommandPaneInPlaceOfPlugin
        | CommandType::OpenCommandPaneBackground
        | CommandType::OpenCommandPaneInPlaceOfPaneId
        | CommandType::RunCommand
        | CommandType::AgentControl
        | CommandType::RegisterAgentProvider
        | CommandType::GetAgentProviders
        | CommandType::SwampRequest
        | CommandType::RunCommandInPane
        | CommandType::SpawnAgent
        | CommandType::ExecCmd => PermissionType::RunCommands,
        CommandType::WebRequest | CommandType::CancelWebRequest => PermissionType::WebAccess,
        CommandType::Write
        | CommandType::WriteChars
        | CommandType::WriteToPaneId
        | CommandType::WriteCharsToPaneId => PermissionType::WriteToStdin,
        CommandType::CopyToClipboard => PermissionType::WriteToClipboard,
        CommandType::SwitchTabTo
        | CommandType::SwitchToMode
        | CommandType::NewTabsWithLayout
        | CommandType::NewTabsWithLayoutInfo
        | CommandType::NewTab
        | CommandType::NewTabUnfocused
        | CommandType::GoToNextTab
        | CommandType::GoToPreviousTab
        | CommandType::Resize
        | CommandType::ResizeWithDirection
        | CommandType::FocusNextPane
        | CommandType::FocusPreviousPane
        | CommandType::FocusLastPane
        | CommandType::MoveFocus
        | CommandType::MoveFocusOrTab
        | CommandType::Detach
        | CommandType::EditScrollback
        | CommandType::EditScrollbackForPaneWithId
        | CommandType::ToggleTab
        | CommandType::MovePane
        | CommandType::MovePaneWithDirection
        | CommandType::MovePaneWithPaneId
        | CommandType::MovePaneWithPaneIdInDirection
        | CommandType::ClearScreen
        | CommandType::ClearScreenForPaneId
        | CommandType::ScrollUp
        | CommandType::ScrollUpInPaneId
        | CommandType::ScrollDown
        | CommandType::ScrollDownInPaneId
        | CommandType::ScrollToTop
        | CommandType::ScrollToTopInPaneId
        | CommandType::ScrollToBottom
        | CommandType::ScrollToBottomInPaneId
        | CommandType::PageScrollUp
        | CommandType::PageScrollUpInPaneId
        | CommandType::PageScrollDown
        | CommandType::PageScrollDownInPaneId
        | CommandType::ToggleFocusFullscreen
        | CommandType::ToggleFocusNoUiFullscreen
        | CommandType::FocusHostSession
        | CommandType::TogglePaneIdFullscreen
        | CommandType::TogglePaneFrames
        | CommandType::SetPaneFrameStyle
        | CommandType::ToggleFloatingPanes
        | CommandType::NewPane
        | CommandType::TogglePaneEmbedOrEject
        | CommandType::TogglePaneEmbedOrEjectForPaneId
        | CommandType::UndoRenamePane
        | CommandType::CloseFocus
        | CommandType::ToggleActiveTabSync
        | CommandType::CloseFocusedTab
        | CommandType::UndoRenameTab
        | CommandType::QuitYinz
        | CommandType::PreviousSwapLayout
        | CommandType::NextSwapLayout
        | CommandType::GoToTabName
        | CommandType::FocusOrCreateTab
        | CommandType::GoToTab
        | CommandType::CloseTerminalPane
        | CommandType::ClosePluginPane
        | CommandType::FocusTerminalPane
        | CommandType::FocusPluginPane
        | CommandType::RenameTerminalPane
        | CommandType::RenamePluginPane
        | CommandType::SetPaneTitlePrefix
        | CommandType::SetPaneSubtitle
        | CommandType::SwitchSession
        | CommandType::DeleteDeadSession
        | CommandType::DeleteAllDeadSessions
        | CommandType::RenameSession
        | CommandType::Notify
        | CommandType::RenameTab
        | CommandType::DisconnectOtherClients
        | CommandType::ShowPaneWithId
        | CommandType::HidePaneWithId
        | CommandType::RerunCommandPane
        | CommandType::ResizePaneIdWithDirection
        | CommandType::CloseTabWithIndex
        | CommandType::BreakPanesToNewTab
        | CommandType::BreakPanesToTabWithIndex
        | CommandType::SwitchTabToId
        | CommandType::GoToTabWithId
        | CommandType::CloseTabWithId
        | CommandType::RenameTabWithId
        | CommandType::ArrangeProjectPanes
        | CommandType::SetProjectContext
        | CommandType::SetTabProjects
        | CommandType::BreakPanesToTabWithId
        | CommandType::ReloadPlugin
        | CommandType::SetFloatingPanePinned
        | CommandType::StackPanes
        | CommandType::ChangeFloatingPanesCoordinates
        | CommandType::PaneTabs
        | CommandType::CreateMultipane
        | CommandType::UpdateMultipane
        | CommandType::ReplaceMultipaneChild
        | CommandType::ChangeOverlayCoordinates
        | CommandType::TogglePaneBorderless
        | CommandType::SetPaneBorderless
        | CommandType::SetPaneColor
        | CommandType::SetPaneAgent
        | CommandType::SetPaneAgentRun
        | CommandType::SetQuitNotice
        | CommandType::GroupAndUngroupPanes
        | CommandType::HighlightAndUnhighlightPanes
        | CommandType::SetRelatedPanes
        | CommandType::RevealPaneWithId
        | CommandType::CloseMultiplePanes
        | CommandType::FloatMultiplePanes
        | CommandType::EmbedMultiplePanes
        | CommandType::ReplacePaneWithExistingPane
        | CommandType::KillSessions
        | CommandType::KillSessionsAndReply
        | CommandType::DeleteDeadSessionAndReply
        | CommandType::DeleteAllDeadSessionsAndReply
        | CommandType::SendSigintToPaneId
        | CommandType::SendSigkillToPaneId
        | CommandType::OverrideLayout
        | CommandType::SaveLayout
        | CommandType::DeleteLayout
        | CommandType::RenameLayout
        | CommandType::EditLayout
        | CommandType::ShowFloatingPanes
        | CommandType::HideFloatingPanes
        | CommandType::SetPaneRegexHighlights
        | CommandType::ClearPaneHighlights
        | CommandType::WarpPaneToSession
        | CommandType::WarpPaneFromSession
        | CommandType::WarpTabToSession
        | CommandType::WarpTabFromSession
        | CommandType::AdoptOrphanedPane => PermissionType::ChangeApplicationState,
        CommandType::UnblockCliPipeInput
        | CommandType::BlockCliPipeInput
        | CommandType::CliPipeOutput => PermissionType::ReadCliPipes,
        CommandType::MessageToPlugin => PermissionType::MessageAndLaunchOtherPlugins,
        CommandType::ListClients
        | CommandType::DumpSessionLayout
        | CommandType::GetPanePid
        | CommandType::GetPaneRunningCommand
        | CommandType::GetPaneCwd
        | CommandType::GetPaneGitProbe
        | CommandType::GenerateRandomName
        | CommandType::GetLayoutDir
        | CommandType::GetUserPluginDir
        | CommandType::GetFocusedPaneInfo
        | CommandType::DumpLayout
        | CommandType::ParseLayout
        | CommandType::SaveSession
        | CommandType::CurrentSessionLastSavedTime
        | CommandType::GetPluginInfo
        | CommandType::GetPaneInfo
        | CommandType::GetTabInfo
        | CommandType::GetDisconnectTargets
        | CommandType::GetSessionList
        | CommandType::ListNotifications
        | CommandType::ListOrphanedPanes
        | CommandType::CommandPalette => PermissionType::ReadApplicationState,
        // One permission covers reading and writing the plugin's *own* namespace. Reaching a
        // `shared:` namespace additionally needs `SharedPluginData`, which is checked in the
        // handlers — this match sees only the command, not the namespace it names.
        CommandType::DataDefineEntity
        | CommandType::DataApply
        | CommandType::DataFetch
        | CommandType::DataCount
        | CommandType::DataWatch
        | CommandType::DataUnwatch
        | CommandType::DataCatalog => PermissionType::PluginData,
        CommandType::RebindKeys
        | CommandType::RegisterDefaultKeybindings
        | CommandType::Reconfigure => PermissionType::Reconfigure,
        CommandType::ChangeHostFolder => PermissionType::FullHdAccess,
        // Nominal: the handler refuses anything that is not a builtin, and a builtin is never
        // asked. Listed so that the command is not permissionless on paper.
        CommandType::GrantPluginPermissions => PermissionType::ChangeApplicationState,
        CommandType::TestPlugin | CommandType::CancelPluginTest => PermissionType::AuthorPlugins,
        CommandType::ProjectConfigRead
        | CommandType::CheckPlugin
        | CommandType::FsRead
        | CommandType::FsList
        | CommandType::FsMetadata => PermissionType::ReadFiles,
        CommandType::ProjectConfigWrite
        | CommandType::FsWrite
        | CommandType::FsRemove
        | CommandType::FsCreateDir
        | CommandType::FsRename => PermissionType::WriteFiles,
        CommandType::InterceptKeyPresses | CommandType::ClearKeyPressesIntercepts => {
            PermissionType::InterceptInput
        },
        CommandType::GetPaneScrollback | CommandType::SubscribePaneOutput => {
            PermissionType::ReadPaneContents
        },
        // The same gate `RunAction` sits behind, and for the same reason: a menu item is a list
        // of actions the server will run as the user, whichever way round it was assembled.
        CommandType::RunAction | CommandType::SetContextMenu => PermissionType::RunActionsAsUser,
        CommandType::GetSessionEnvironmentVariables => {
            PermissionType::ReadSessionEnvironmentVariables
        },
        // Another plugin's configuration is session state, and is read the same way the
        // session list and the pane manifest are.
        CommandType::GetPluginAliasConfiguration
        | CommandType::GetPluginIdsForUrl
        | CommandType::GetPluginIdsForUrlWithConfiguration => PermissionType::ReadApplicationState,
        CommandType::OpenCommandPaneInNewTab => PermissionType::RunCommands,
        CommandType::OpenEditorPaneInNewTab => PermissionType::OpenFiles,
        // Enqueueing is launching another plugin, just later.
        CommandType::Pick | CommandType::EnqueueQueueJob => {
            PermissionType::MessageAndLaunchOtherPlugins
        },
        // Raising a queue's parallelism or killing someone else's job reaches past the caller's
        // own work, which is more than "may launch plugins" should grant.
        CommandType::ConfigureQueue | CommandType::CancelQueueJob => {
            PermissionType::ChangeApplicationState
        },
        CommandType::QueueStatus => PermissionType::ReadApplicationState,
        // A job reporting on itself needs nothing, like `CloseSelf`: it runs headless, and a
        // permission prompt it raised would have no pane for anyone to answer it in.
        CommandType::FinishQueueJob => return None,
        // A clock. It says the machine's offset from UTC and nothing about the session,
        // which is less than `GetYinzVersion` already says, and a permission prompt in
        // front of drawing a timestamp would be answered blind every time.
        CommandType::GetLocalUtcOffset => return None,
        // Reloading a plugin someone wrote is the second half of writing it.
        CommandType::ReloadPluginByUrl => PermissionType::AuthorPlugins,
        // Reading a plugin's log lines is the authoring loop's "what did it say" step, the same
        // act as `yinz plugins log`; it is the log of the whole server, so not `ReadFiles`.
        CommandType::ReadPluginLog => PermissionType::AuthorPlugins,
    })
}

#[cfg(test)]
mod permission_tests {
    use super::*;
    use yinz_types::input::layout::RunPluginOrAlias;

    #[test]
    fn plugin_info_self_inspection_needs_no_permission() {
        let env = crate::test_env::test_env();
        for id in [None, Some(env.plugin_id)] {
            assert_eq!(
                check_command_permission(&env, &PluginCommand::GetPluginInfo(id)),
                (PermissionStatus::Granted, None)
            );
        }
        assert_eq!(
            check_command_permission(
                &env,
                &PluginCommand::GetPluginInfo(Some(PluginId::new(env.plugin_id.get() + 1)))
            ),
            (
                PermissionStatus::Denied,
                Some(PermissionType::ReadApplicationState)
            )
        );
    }

    #[test]
    fn plugin_url_query_denial_is_an_error_not_an_empty_match() {
        let mut env = crate::test_env::test_env();
        env.response_sink = ResponseSink::Capture(None);
        let command = PluginCommand::GetPluginIdsForUrl("file:/plugin.ts".into());
        assert_eq!(
            command_permission(CommandType::GetPluginIdsForUrl),
            Some(PermissionType::ReadApplicationState)
        );
        run_plugin_command(&mut env, command).unwrap();
        let bytes: Vec<u8> = serde_json::from_str(&env.response_sink.take().unwrap()).unwrap();
        let reply = yinz_plugin_api::plugin_command::ProtobufGetPluginIdsForUrlResponse::decode(
            bytes.as_slice(),
        )
        .unwrap();
        assert_eq!(
            reply.error.as_deref(),
            Some("Permission denied: ReadApplicationState")
        );
        assert!(
            crate::response::decode_response(CommandType::GetPluginIdsForUrl, &bytes)
                .unwrap()
                .is_err()
        );
    }

    #[test]
    fn a_missing_plugin_root_under_a_symlink_does_not_block_data_writes() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config");
        fs::create_dir(&config).unwrap();
        let alias = temp.path().join("config-link");
        std::os::unix::fs::symlink(&config, &alias).unwrap();
        let plugin_dir = alias.join("plugins");
        let mut env = crate::test_env::test_env();
        env.plugin_own_data_dir = temp.path().to_owned();
        env.user_plugin_dir = Some(plugin_dir.clone());
        *env.permissions.lock().unwrap() = Some(HashSet::from([PermissionType::WriteFiles]));
        env.response_sink = ResponseSink::Capture(None);

        run_plugin_command(
            &mut env,
            PluginCommand::FsWrite {
                path: "/data/output.txt".into(),
                contents: b"output".to_vec(),
                append: false,
                create_dirs: false,
            },
        )
        .unwrap();
        let bytes: Vec<u8> = serde_json::from_str(&env.response_sink.take().unwrap()).unwrap();
        assert_eq!(
            ProtobufFsMutateResponse::decode(bytes.as_slice())
                .unwrap()
                .result,
            Some(fs_mutate_response::Result::Success(true))
        );
        assert_eq!(fs::read(temp.path().join("output.txt")).unwrap(), b"output");
        assert!(
            !plugin_dir.exists(),
            "an unrelated write must not create /plugins"
        );
    }

    #[test]
    fn a_missing_plugin_root_under_a_symlink_still_requires_authoring_permission() {
        for suffix in ["plugins", "missing/nested/plugins"] {
            for prefix in [
                "/plugins".to_owned(),
                format!("/host/config-link/{suffix}"),
                format!("/host/config/{suffix}"),
            ] {
                let temp = tempfile::tempdir().unwrap();
                let config = temp.path().join("config");
                fs::create_dir(&config).unwrap();
                let alias = temp.path().join("config-link");
                std::os::unix::fs::symlink(&config, &alias).unwrap();
                let plugin_dir = alias.join(suffix);
                let mut env = crate::test_env::test_env();
                env.plugin_cwd = temp.path().to_owned();
                env.user_plugin_dir = Some(plugin_dir.clone());
                *env.permissions.lock().unwrap() =
                    Some(HashSet::from([PermissionType::WriteFiles]));
                env.response_sink = ResponseSink::Capture(None);
                let path = PathBuf::from(prefix).join("new/nested/plugin.ts");

                for resolution in [
                    FsPathResolution::FollowSymlink,
                    FsPathResolution::PreserveEntry,
                ] {
                    let error =
                        resolve_fs_path_with_permission(&env, &path, FsIntent::Write, resolution)
                            .unwrap_err();
                    assert!(
                        error.to_string().contains("AuthorPlugins"),
                        "{path:?}: {error:#}"
                    );
                }
                for authorized in [false, true] {
                    if authorized {
                        env.permissions
                            .lock()
                            .unwrap()
                            .as_mut()
                            .unwrap()
                            .insert(PermissionType::AuthorPlugins);
                    }
                    run_plugin_command(
                        &mut env,
                        PluginCommand::FsWrite {
                            path: path.clone(),
                            contents: b"export {};".to_vec(),
                            append: false,
                            create_dirs: true,
                        },
                    )
                    .unwrap();
                    let bytes: Vec<u8> =
                        serde_json::from_str(&env.response_sink.take().unwrap()).unwrap();
                    let result = ProtobufFsMutateResponse::decode(bytes.as_slice())
                        .unwrap()
                        .result;
                    if authorized {
                        assert_eq!(
                            result,
                            Some(fs_mutate_response::Result::Success(true)),
                            "{path:?}"
                        );
                        assert_eq!(
                            fs::read(config.join(suffix).join("new/nested/plugin.ts")).unwrap(),
                            b"export {};"
                        );
                    } else {
                        assert!(
                            matches!(result, Some(fs_mutate_response::Result::Error(error)) if error.contains("AuthorPlugins")),
                            "{path:?}"
                        );
                        assert!(
                            !plugin_dir.exists(),
                            "a denied write must not create /plugins"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn authoring_permission_follows_plugin_directory_aliases_and_ancestors() {
        let temp = tempfile::tempdir().unwrap();
        let plugin_dir = temp.path().join("plugins");
        fs::create_dir(&plugin_dir).unwrap();
        std::os::unix::fs::symlink(&plugin_dir, temp.path().join("alias")).unwrap();
        let mut env = crate::test_env::test_env();
        env.plugin_cwd = temp.path().to_owned();
        env.user_plugin_dir = Some(plugin_dir);
        for path in [
            "/plugins/new.ts",
            "/host/plugins/new.ts",
            "/host/alias/new.ts",
            "plugins/new.ts",
            "/host/plugins",
        ] {
            for resolution in [
                FsPathResolution::FollowSymlink,
                FsPathResolution::PreserveEntry,
            ] {
                assert!(
                    resolve_fs_path_with_permission(
                        &env,
                        Path::new(path),
                        FsIntent::Write,
                        resolution
                    )
                    .unwrap_err()
                    .to_string()
                    .contains("AuthorPlugins"),
                    "{path}"
                );
                assert!(
                    resolve_fs_path_with_permission(
                        &env,
                        Path::new(path),
                        FsIntent::Read,
                        resolution
                    )
                    .is_ok(),
                    "{path}"
                );
            }
        }
        assert!(resolve_fs_path_with_permission(
            &env,
            Path::new("/host/unrelated"),
            FsIntent::Write,
            FsPathResolution::FollowSymlink
        )
        .is_ok());
        assert!(resolve_fs_path_with_permission(
            &env,
            Path::new("/host"),
            FsIntent::Write,
            FsPathResolution::PreserveEntry
        )
        .unwrap_err()
        .to_string()
        .contains("AuthorPlugins"));
        // Ensuring an existing ancestor directory is harmless; only removing or renaming
        // that entry changes the plugin folder below it.
        *env.permissions.lock().unwrap() = Some(HashSet::from([PermissionType::WriteFiles]));
        env.response_sink = ResponseSink::Capture(None);
        run_plugin_command(
            &mut env,
            PluginCommand::FsCreateDir {
                path: PathBuf::from("/host"),
                all: true,
            },
        )
        .unwrap();
        let bytes: Vec<u8> = serde_json::from_str(&env.response_sink.take().unwrap()).unwrap();
        assert!(matches!(
            ProtobufFsMutateResponse::decode(bytes.as_slice())
                .unwrap()
                .result,
            Some(fs_mutate_response::Result::Success(true))
        ));
        *env.permissions.lock().unwrap() = Some(HashSet::from([PermissionType::AuthorPlugins]));
        assert!(resolve_fs_path_with_permission(
            &env,
            Path::new("/host/plugins/new.ts"),
            FsIntent::Write,
            FsPathResolution::FollowSymlink
        )
        .is_ok());
    }

    #[test]
    fn disconnect_target_queries_isolate_late_replies_and_report_failures() {
        use yinz_plugin_api::plugin_command::ProtobufGetDisconnectTargetsResponse;
        use yinz_runtime::channels::{unbounded, SenderWithContext};
        use yinz_types::internal_ids::ClientId;
        let mut env = crate::test_env::test_env();
        env.response_sink = ResponseSink::Capture(None);
        let (sender, server) = unbounded();
        env.senders.to_server = Some(SenderWithContext::new(sender));
        let read = |env: &mut PluginEnv| {
            let bytes: Vec<u8> = serde_json::from_str(&env.response_sink.take().unwrap()).unwrap();
            ProtobufGetDisconnectTargetsResponse::decode(bytes.as_slice()).unwrap()
        };
        assert_eq!(
            check_command_permission(&env, &PluginCommand::GetDisconnectTargets),
            (
                PermissionStatus::Denied,
                Some(PermissionType::ReadApplicationState)
            )
        );
        run_plugin_command(&mut env, PluginCommand::GetDisconnectTargets).unwrap();
        assert!(read(&mut env).error.unwrap().contains("Permission denied"));
        assert!(server.try_recv().is_err());
        *env.permissions.lock().unwrap() =
            Some(HashSet::from([PermissionType::ReadApplicationState]));
        run_plugin_command(&mut env, PluginCommand::GetDisconnectTargets).unwrap();
        assert!(read(&mut env)
            .error
            .unwrap()
            .contains("receive disconnect targets"));
        let ServerInstruction::GetDisconnectTargets(_, stale) = server.recv().unwrap().0 else {
            panic!()
        };
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let ServerInstruction::GetDisconnectTargets(_, fresh) = server.recv().unwrap().0
                else {
                    panic!()
                };
                assert!(stale.send(vec![ClientId::new(9)]).is_err());
                fresh
                    .send(vec![ClientId::new(2), ClientId::new(3)])
                    .unwrap();
            });
            run_plugin_command(&mut env, PluginCommand::GetDisconnectTargets).unwrap();
        });
        let response = read(&mut env);
        assert_eq!(response.client_ids, vec![2, 3]);
        assert!(response.error.is_none());
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let _ = server.recv().unwrap();
            });
            run_plugin_command(&mut env, PluginCommand::GetDisconnectTargets).unwrap();
        });
        assert!(read(&mut env).error.unwrap().contains("disconnected"));
        drop(server);
        env.senders.should_silently_fail = false;
        run_plugin_command(&mut env, PluginCommand::GetDisconnectTargets).unwrap();
        assert!(read(&mut env)
            .error
            .unwrap()
            .contains("send disconnect-target query"));
    }

    #[test]
    fn denied_session_commands_reply_with_permission_errors() {
        let mut env = crate::test_env::test_env();
        env.response_sink = ResponseSink::Capture(None);
        for command in [
            PluginCommand::KillSessionsAndReply(vec!["test-session".into()]),
            PluginCommand::DeleteDeadSessionAndReply("test-session".into()),
            PluginCommand::DeleteAllDeadSessionsAndReply,
        ] {
            let kind = CommandType::from(&command);
            assert_eq!(
                check_command_permission(&env, &command),
                (
                    PermissionStatus::Denied,
                    Some(PermissionType::ChangeApplicationState)
                )
            );
            run_plugin_command(&mut env, command).unwrap();
            let bytes: Vec<u8> = serde_json::from_str(&env.response_sink.take().unwrap()).unwrap();
            let error = match kind {
                CommandType::KillSessionsAndReply => {
                    ProtobufKillSessionsResponse::decode(bytes.as_slice())
                        .unwrap()
                        .error
                },
                CommandType::DeleteDeadSessionAndReply => {
                    ProtobufDeleteDeadSessionResponse::decode(bytes.as_slice())
                        .unwrap()
                        .error
                },
                CommandType::DeleteAllDeadSessionsAndReply => {
                    ProtobufDeleteAllDeadSessionsResponse::decode(bytes.as_slice())
                        .unwrap()
                        .error
                },
                _ => unreachable!(),
            };
            assert_eq!(
                error.as_deref(),
                Some("Permission denied: ChangeApplicationState"),
                "{kind}"
            );
        }
    }

    #[test]
    fn other_denied_commands_keep_their_empty_reply() {
        let mut env = crate::test_env::test_env();
        env.response_sink = ResponseSink::Capture(None);
        run_plugin_command(&mut env, PluginCommand::GetSessionList).unwrap();
        let bytes: Vec<u8> = serde_json::from_str(&env.response_sink.take().unwrap()).unwrap();
        assert!(bytes.is_empty());
    }

    /// `prepare_git_checkout_in` asking for reuse, which is what every test written before
    /// reuse became a request meant by it: an existing branch or worktree is the work being
    /// pointed at. The tests whose subject *is* the other behaviour call the real one with
    /// `false` and say so.
    fn reusing_checkout_in(
        source_directory: PathBuf,
        checkout: GitCheckout,
    ) -> Result<PathBuf, String> {
        prepare_git_checkout_in(source_directory, checkout, true, WorktreeGuard::Waived)
            .map(|prepared| prepared.path)
    }

    /// The same call without reuse: a name already taken is stepped around rather than
    /// joined. The guard is waived here as it is above - the tests whose subject *is* the
    /// guard pass `WorktreeGuard::Required` themselves.
    fn cutting_checkout_in(
        source_directory: PathBuf,
        checkout: GitCheckout,
    ) -> Result<PreparedCheckout, String> {
        prepare_git_checkout_in(source_directory, checkout, false, WorktreeGuard::Waived)
    }

    /// `agent_folder` likewise, answering the folder alone.
    fn agent_folder_reusing(
        checkout: GitCheckout,
        cwd: Option<PathBuf>,
        source: PathBuf,
    ) -> Result<PathBuf, String> {
        agent_folder(checkout, cwd, source, true, WorktreeGuard::Waived)
            .map(|prepared| prepared.path)
    }

    fn repository() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "test@example.com"],
            vec!["config", "user.name", "Test"],
            vec!["commit", "--allow-empty", "-q", "-m", "initial"],
        ] {
            assert!(process::Command::new("git")
                .arg("-C")
                .arg(temp.path())
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        temp
    }

    /// Every command that runs without a permission, pinned so a reviewer sees the list change —
    /// in either direction: a command gated here must not be ungated in `command_permission`,
    /// and every command not listed here must need something.
    #[test]
    fn the_commands_that_need_no_permission_are_exactly_these() {
        use strum::IntoEnumIterator;
        let ungated = [
            CommandType::ResolveNotification,
            CommandType::RequestPluginPermissions,
            CommandType::Subscribe,
            CommandType::Unsubscribe,
            CommandType::SetSelectable,
            CommandType::SetSelfMouseSelectionSupport,
            CommandType::ShowCursor,
            CommandType::HideSelf,
            CommandType::ShowSelf,
            CommandType::CloseSelf,
            CommandType::SetTimeout,
            CommandType::ReportPanic,
            CommandType::GetPluginIds,
            CommandType::GetYinzVersion,
            CommandType::PostMessageTo,
            CommandType::PostMessageToPlugin,
            CommandType::ScanHostFolder,
            CommandType::WatchFilesystem,
            CommandType::FinishQueueJob,
            CommandType::GetLocalUtcOffset,
        ];
        for kind in CommandType::iter() {
            assert_eq!(
                command_permission(kind).is_none(),
                ungated.contains(&kind),
                "{kind}"
            );
        }
    }

    /// `MessageToPlugin` reaches another plugin; `PostMessageToPlugin` only ever reaches the
    /// sender itself, so a plugin's workers can answer it without a permission.
    #[test]
    fn a_message_to_another_plugin_needs_a_permission_but_a_message_to_self_does_not() {
        assert_eq!(
            command_permission(CommandType::MessageToPlugin),
            Some(PermissionType::MessageAndLaunchOtherPlugins)
        );
        assert_eq!(command_permission(CommandType::PostMessageToPlugin), None);
    }

    #[test]
    fn pane_tabs_mount_requires_application_state_and_plugin_launch_permissions() {
        use yinz_types::pane_tabs::*;
        let env = crate::test_env::test_env();
        let command = PluginCommand::PaneTabs(PaneTabsCommand::Mount {
            pane_id: PaneId::Plugin(PluginId::new(1)),
            existing: 0,
            layout: PaneTabsLayout {
                active: 0,
                tabs: vec![PaneTabDescriptor {
                    label: "Files".into(),
                    location: "yinz:strider".into(),
                    initial_cwd: None,
                    configuration: BTreeMap::new(),
                }],
            },
        });
        for (permissions, expected) in [
            (
                vec![PermissionType::ChangeApplicationState],
                (
                    PermissionStatus::Denied,
                    Some(PermissionType::OpenTerminalsOrPlugins),
                ),
            ),
            (
                vec![PermissionType::OpenTerminalsOrPlugins],
                (
                    PermissionStatus::Denied,
                    Some(PermissionType::ChangeApplicationState),
                ),
            ),
            (
                vec![
                    PermissionType::ChangeApplicationState,
                    PermissionType::OpenTerminalsOrPlugins,
                ],
                (PermissionStatus::Granted, None),
            ),
        ] {
            *env.permissions.lock().unwrap() = Some(permissions.into_iter().collect());
            assert_eq!(check_command_permission(&env, &command), expected);
        }
    }

    #[test]
    fn multipane_adoption_and_launch_require_the_existing_permission_categories() {
        use yinz_types::multipane::*;
        let env = crate::test_env::test_env();
        let terminal = PaneId::Terminal(TerminalId::new(2));
        let command = |child| {
            PluginCommand::CreateMultipane(CreateMultipane {
                children: [MultipaneChild::Existing(terminal), child],
                placement: OverlayPlacement::default(),
                split: MultipaneSplit {
                    direction: MultipaneDirection::LeftRight,
                    ratio: 5000,
                },
                focus: None,
                min_cells: 0,
            })
        };
        let existing = command(MultipaneChild::Existing(PaneId::Plugin(PluginId::new(3))));
        let launch = command(MultipaneChild::Terminal(CommandToRun {
            path: "sh".into(),
            ..Default::default()
        }));
        let plugin = command(MultipaneChild::Plugin {
            url: "file:/test.wasm".into(),
            configuration: BTreeMap::new(),
        });
        *env.permissions.lock().unwrap() =
            Some(HashSet::from([PermissionType::ChangeApplicationState]));
        assert_eq!(
            check_command_permission(&env, &existing),
            (PermissionStatus::Granted, None)
        );
        assert_eq!(
            check_command_permission(&env, &launch),
            (PermissionStatus::Denied, Some(PermissionType::RunCommands))
        );
        assert_eq!(
            check_command_permission(&env, &plugin),
            (
                PermissionStatus::Denied,
                Some(PermissionType::OpenTerminalsOrPlugins)
            )
        );
        *env.permissions.lock().unwrap() = Some(HashSet::from([
            PermissionType::RunCommands,
            PermissionType::OpenTerminalsOrPlugins,
        ]));
        for command in [existing, launch, plugin] {
            assert_eq!(
                check_command_permission(&env, &command),
                (
                    PermissionStatus::Denied,
                    Some(PermissionType::ChangeApplicationState)
                )
            );
        }
    }

    #[test]
    fn a_syntax_check_needs_no_permission_but_a_semantic_check_reads_files() {
        use std::sync::{Arc, Mutex};
        use yinz_types::authoring::PluginCheckMode;
        let mut env = crate::test_env::test_env();
        env.permissions = Arc::new(Mutex::new(Some(HashSet::new())));
        let check = |mode| PluginCommand::CheckPlugin {
            name: "check.ts".into(),
            source: "export default {};".into(),
            mode,
        };
        assert_eq!(
            check_command_permission(&env, &check(PluginCheckMode::Syntax)),
            (PermissionStatus::Granted, None)
        );
        for mode in [
            PluginCheckMode::Semantic,
            PluginCheckMode::Format,
            PluginCheckMode::Prepare,
        ] {
            assert_eq!(
                check_command_permission(&env, &check(mode)),
                (PermissionStatus::Denied, Some(PermissionType::ReadFiles))
            );
        }
    }

    #[test]
    fn project_configuration_requires_file_permissions() {
        assert_eq!(
            command_permission(CommandType::ProjectConfigRead),
            Some(PermissionType::ReadFiles)
        );
        assert_eq!(
            command_permission(CommandType::ProjectConfigWrite),
            Some(PermissionType::WriteFiles)
        );
    }

    #[test]
    fn moving_focus_needs_the_same_permission_in_every_direction() {
        for kind in [
            CommandType::FocusNextPane,
            CommandType::FocusPreviousPane,
            CommandType::FocusLastPane,
        ] {
            assert_eq!(
                command_permission(kind),
                Some(PermissionType::ChangeApplicationState),
                "{kind}"
            );
        }
    }

    #[test]
    fn loading_a_new_plugin_needs_plugin_launch_permission() {
        assert_eq!(
            command_permission(CommandType::LoadNewPlugin),
            Some(PermissionType::OpenTerminalsOrPlugins)
        );
        assert_eq!(
            command_permission(CommandType::ReloadPlugin),
            Some(PermissionType::ChangeApplicationState)
        );
    }

    #[test]
    fn layout_permissions_cover_every_active_startup_pane_kind() {
        let tiled_layout = yinz_types::input::layout::TiledPaneLayout {
            children: vec![
                yinz_types::input::layout::TiledPaneLayout {
                    run: Some(Run::Command(RunCommand::default())),
                    ..Default::default()
                },
                yinz_types::input::layout::TiledPaneLayout {
                    run: Some(Run::EditFile(PathBuf::from("active"), None, None)),
                    ..Default::default()
                },
                yinz_types::input::layout::TiledPaneLayout {
                    run: Some(Run::Plugin(RunPluginOrAlias::default())),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let layout = Layout {
            template: Some((tiled_layout, vec![])),
            ..Default::default()
        };
        assert_eq!(
            layout_run_permissions(&layout),
            BTreeSet::from([
                PermissionType::OpenFiles,
                PermissionType::RunCommands,
                PermissionType::OpenTerminalsOrPlugins,
            ])
        );
    }

    #[test]
    fn layout_permissions_ignore_non_startup_work() {
        let ignored = Some(Run::Command(RunCommand::default()));
        let tiled_layout = yinz_types::input::layout::TiledPaneLayout {
            run: ignored.clone(),
            run_instructions_to_ignore: vec![ignored],
            ..Default::default()
        };
        let already_running = yinz_types::input::layout::FloatingPaneLayout {
            run: Some(Run::Plugin(RunPluginOrAlias::default())),
            already_running: true,
            ..Default::default()
        };
        let swap_layout = yinz_types::input::layout::TiledPaneLayout {
            run: Some(Run::EditFile(PathBuf::from("swap"), None, None)),
            ..Default::default()
        };
        let layout = Layout {
            template: Some((tiled_layout, vec![already_running])),
            swap_layouts: vec![(swap_layout, vec![])],
            ..Default::default()
        };
        assert!(layout_run_permissions(&layout).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn plugin_layout_paths_cannot_escape_the_layout_directory() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let layout_dir = temp.path().join("layouts");
        std::fs::create_dir(&layout_dir).unwrap();
        std::fs::write(layout_dir.join("safe.kdl"), "layout {}").unwrap();
        let outside = temp.path().join("outside.kdl");
        std::fs::write(&outside, "layout {}").unwrap();

        validate_plugin_layout_path("safe", Some(&layout_dir)).unwrap();
        symlink(layout_dir.join("safe.kdl"), layout_dir.join("inside.kdl")).unwrap();
        validate_plugin_layout_path("inside", Some(&layout_dir)).unwrap();

        assert!(validate_plugin_layout_path("../outside", Some(&layout_dir)).is_err());
        assert!(validate_plugin_layout_path(outside.to_str().unwrap(), Some(&layout_dir)).is_err());

        symlink(&outside, layout_dir.join("escape.kdl")).unwrap();
        assert!(validate_plugin_layout_path("escape", Some(&layout_dir)).is_err());

        symlink(&outside, layout_dir.join("safe.swap.kdl")).unwrap();
        assert!(validate_plugin_layout_path("safe", Some(&layout_dir)).is_err());
    }

    #[test]
    fn saving_layouts_preserves_existing_files_until_replacement_succeeds() {
        let temp = tempfile::tempdir().unwrap();
        let mut env = crate::test_env::test_env();
        let layout_dir = temp.path().join("layouts");
        env.layout_dir = Some(layout_dir.clone());
        try_save_layout(&env, "saved", "layout {}", false).unwrap();
        let path = layout_dir.join("saved.kdl");
        let original = std::fs::read(&path).unwrap();

        assert!(try_save_layout(&env, "saved", "layout { pane; }", false)
            .unwrap_err()
            .contains("already exists"));
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert!(try_save_layout(&env, "saved", "invalid {", true).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);

        try_save_layout(&env, "saved", "layout { pane; }", true).unwrap();
        assert!(std::fs::read_to_string(path).unwrap().contains("pane"));
        assert_eq!(std::fs::read_dir(layout_dir).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn renaming_layouts_preserves_occupied_destinations_including_dangling_links() {
        let temp = tempfile::tempdir().unwrap();
        let mut env = crate::test_env::test_env();
        env.layout_dir = Some(temp.path().to_path_buf());
        let source = temp.path().join("source.kdl");
        let destination = temp.path().join("destination.kdl");
        std::fs::write(&source, "layout {}").unwrap();
        std::os::unix::fs::symlink(temp.path().join("missing"), &destination).unwrap();
        assert!(try_rename_layout(&env, "source", "destination")
            .unwrap_err()
            .contains("already exists"));
        assert!(destination.is_symlink());
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "layout {}");

        std::fs::remove_file(&destination).unwrap();
        std::fs::write(&destination, "layout { pane; }").unwrap();
        assert!(try_rename_layout(&env, "source", "destination").is_err());
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            "layout { pane; }"
        );

        std::fs::remove_file(&destination).unwrap();
        try_rename_layout(&env, "source", "destination").unwrap();
        assert!(!source.exists());
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "layout {}");
    }

    #[cfg(unix)]
    #[test]
    fn saving_layouts_never_writes_through_destination_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let mut env = crate::test_env::test_env();
        let layout_dir = temp.path().join("layouts");
        std::fs::create_dir(&layout_dir).unwrap();
        env.layout_dir = Some(layout_dir.clone());
        let outside = temp.path().join("outside");
        let destination = layout_dir.join("linked.kdl");
        symlink(&outside, &destination).unwrap();

        // A dangling link is still occupied, even though Path::exists reports false.
        assert!(try_save_layout(&env, "linked", "layout {}", false).is_err());
        assert!(!outside.exists());
        std::fs::write(&outside, "keep").unwrap();
        assert!(try_save_layout(&env, "linked", "layout {}", false).is_err());
        try_save_layout(&env, "linked", "layout {}", true).unwrap();
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep");
        assert!(!std::fs::symlink_metadata(destination).unwrap().is_symlink());
        assert_eq!(std::fs::read_dir(layout_dir).unwrap().count(), 1);
    }

    #[test]
    fn project_worktree_defaults_apply_from_primary_and_external_checkouts() {
        use project_data::{projects, store};
        let first = repository();
        let second = repository();
        let external = tempfile::tempdir().unwrap();
        let linked = external.path().join("linked source");
        run_git(
            first.path(),
            &[
                "worktree",
                "add",
                "-b",
                "external",
                linked.to_str().unwrap(),
            ],
            "link",
        )
        .unwrap();
        let mut a = projects::validate("a", resolved(first.path()).to_str().unwrap()).unwrap();
        let relative = format!(
            "../{}/relative trees",
            external.path().file_name().unwrap().to_str().unwrap()
        );
        a.worktree_dir = Some(relative.clone());
        let mut b = projects::validate("b", resolved(second.path()).to_str().unwrap()).unwrap();
        b.worktree_dir = Some(
            external
                .path()
                .join("absolute trees")
                .to_string_lossy()
                .into_owned(),
        );
        let registry: projects::Registry = [(a.name.clone(), a), (b.name.clone(), b)]
            .into_iter()
            .collect();
        // The host reads the real shared store, including when the caller is a direct plugin.
        let db_dir = tempfile::tempdir().unwrap();
        let db = plugin_data_store::PluginDataStore::new(db_dir.path());
        db.define_entity(store::SHARED, "agents", &store::project_def(), false)
            .unwrap();
        for project in registry.values() {
            db.apply(store::SHARED, &[projects::to_mutation(project)])
                .unwrap();
        }
        for (source, branch, expected) in [
            (
                first.path(),
                "primary-default",
                first.path().join(&relative).join("primary-default"),
            ),
            (
                linked.as_path(),
                "linked-default",
                first.path().join(&relative).join("linked-default"),
            ),
            (
                second.path(),
                "absolute-default",
                external.path().join("absolute trees/absolute-default"),
            ),
        ] {
            let mut checkout = GitCheckout::NewWorktree {
                branch: branch.into(),
                base: "main".into(),
                path: format!(".worktrees/{branch}").into(),
            };
            apply_project_worktree_dir(&mut checkout, None, source, || {
                db.fetch(store::SHARED, &FetchRequest::entity("project"))
            })
            .unwrap();
            let made = agent_folder(
                checkout,
                None,
                source.into(),
                false,
                WorktreeGuard::Required,
            )
            .unwrap();
            assert_eq!(resolved(&made.path), resolved(&expected));
        }
    }

    #[test]
    fn project_worktree_linked_defaults_follow_repository_membership() {
        use project_data::projects;
        let repo = repository();
        let unrelated = repository();
        let external = tempfile::tempdir().unwrap();
        let linked = external.path().join("registered");
        let sibling = external.path().join("sibling");
        for (branch, path) in [("linked", &linked), ("sibling", &sibling)] {
            run_git(
                repo.path(),
                &["worktree", "add", "-b", branch, path.to_str().unwrap()],
                "link",
            )
            .unwrap();
        }
        let alias = external.path().join("linked-alias");
        std::os::unix::fs::symlink(&linked, &alias).unwrap();
        let mut registry = projects::Registry::new();
        // Each stage adds a competing registration. Direct matches win, then the
        // primary root; otherwise linked roots must identify exactly one project.
        for (stage, root, dir, sources) in [
            (
                "linked",
                linked.as_path(),
                "saved",
                vec![
                    (repo.path(), "saved"),
                    (linked.as_path(), "saved"),
                    (alias.as_path(), "saved"),
                    (sibling.as_path(), "saved"),
                    (unrelated.path(), ".worktrees"),
                ],
            ),
            (
                "ambiguous",
                sibling.as_path(),
                "sibling-saved",
                vec![
                    (repo.path(), ".worktrees"),
                    (linked.as_path(), "saved"),
                    (alias.as_path(), "saved"),
                    (sibling.as_path(), "sibling-saved"),
                ],
            ),
            (
                "primary",
                repo.path(),
                "primary-saved",
                vec![
                    (repo.path(), "primary-saved"),
                    (linked.as_path(), "saved"),
                    (alias.as_path(), "saved"),
                    (sibling.as_path(), "sibling-saved"),
                ],
            ),
        ] {
            let mut project = projects::validate(stage, resolved(root).to_str().unwrap()).unwrap();
            project.worktree_dir = Some(dir.into());
            registry.insert(project.name.clone(), project);
            for (index, (source, expected_dir)) in sources.into_iter().enumerate() {
                let branch = format!("{stage}-{index}");
                let mut checkout = GitCheckout::NewWorktree {
                    branch: branch.clone(),
                    base: "main".into(),
                    path: format!(".worktrees/{branch}").into(),
                };
                apply_project_worktree_dir(&mut checkout, None, source, || {
                    Ok(projects::to_rows(&registry))
                })
                .unwrap();
                let made = agent_folder(
                    checkout,
                    None,
                    source.into(),
                    false,
                    WorktreeGuard::Required,
                )
                .unwrap();
                let primary = if source == unrelated.path() {
                    unrelated.path()
                } else {
                    repo.path()
                };
                assert_eq!(
                    resolved(&made.path),
                    resolved(&primary.join(expected_dir).join(branch)),
                );
            }
        }
    }

    #[test]
    fn project_worktree_lookup_preserves_overrides_and_surfaces_store_errors() {
        let mut checkout = GitCheckout::NewWorktree {
            branch: "topic".into(),
            base: "main".into(),
            path: "/explicit/topic".into(),
        };
        apply_project_worktree_dir(&mut checkout, Some("/explicit"), "/unused".as_ref(), || {
            panic!("explicit launch must not read projects")
        })
        .unwrap();
        assert!(
            matches!(&checkout, GitCheckout::NewWorktree { path, .. } if path == std::path::Path::new("/explicit/topic"))
        );
        for mut unchanged in [
            GitCheckout::Current,
            GitCheckout::NewBranch {
                branch: "topic".into(),
                base: "main".into(),
            },
        ] {
            apply_project_worktree_dir(&mut unchanged, None, "/unused".as_ref(), || {
                panic!("no worktree must not read projects")
            })
            .unwrap();
        }
        apply_project_worktree_dir(&mut checkout, None, "/unused".as_ref(), || {
            Err(DataError::UnknownEntity("project".into()))
        })
        .unwrap();
        let error = apply_project_worktree_dir(&mut checkout, None, "/unused".as_ref(), || {
            Err(DataError::Storage("offline".into()))
        })
        .unwrap_err();
        assert!(error.contains("offline"));
        apply_project_worktree_dir(&mut checkout, None, "/unused".as_ref(), || {
            Ok(project_data::projects::to_rows(&Default::default()))
        })
        .unwrap();
    }

    #[test]
    fn git_checkout_request_creates_a_worktree_from_the_base() {
        let repository = repository();
        let target = repository.path().join(".worktrees/topic");
        let cwd = reusing_checkout_in(
            repository.path().to_owned(),
            GitCheckout::NewWorktree {
                branch: "topic".to_owned(),
                base: "main".to_owned(),
                path: PathBuf::from(".worktrees/topic"),
            },
        )
        .unwrap();
        // Resolved: the worktree path here is built from `git rev-parse
        // --show-toplevel`, and on macOS git answers the physical path while
        // `tempfile` handed out one under a symlinked `/var`. The absolute-path
        // case below is deliberately *not* resolved - there the point is that the
        // path is honoured exactly as given.
        assert_eq!(resolved(&cwd), resolved(&target));
        assert!(std::fs::read_to_string(target.join(".git"))
            .unwrap()
            .starts_with("gitdir:"));
    }

    /// A linked worktree of `primary`, nested inside it the way this repository
    /// keeps its own.
    fn worktree(primary: &std::path::Path, name: &str, branch: &str) -> PathBuf {
        let path = primary.join(".worktrees").join(name);
        assert!(process::Command::new("git")
            .arg("-C")
            .arg(primary)
            .args(["worktree", "add", "-b", branch])
            .arg(&path)
            .status()
            .unwrap()
            .success());
        path
    }

    /// `tempfile` hands out a path under a symlinked `/var` on macOS while git
    /// answers the physical one, so paths only compare after resolving.
    fn resolved(path: &std::path::Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap()
    }

    /// With no branch there is no git work: the pane runs in the folder the caller named,
    /// else in the source as given - a subfolder of a checkout stays that subfolder, and is
    /// not swapped for the repository root the way a checkout request resolves it.
    #[test]
    fn an_agent_with_no_branch_runs_where_it_was_asked_and_never_at_the_repository_root() {
        let repository = repository();
        let subdirectory = repository.path().join("src/nested");
        std::fs::create_dir_all(&subdirectory).unwrap();
        let elsewhere = repository.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();

        assert_eq!(
            agent_folder_reusing(GitCheckout::Current, None, subdirectory.clone()).unwrap(),
            subdirectory
        );
        assert_eq!(
            agent_folder_reusing(
                GitCheckout::Current,
                Some(elsewhere.clone()),
                subdirectory.clone()
            )
            .unwrap(),
            elsewhere
        );
        // A branch is git work in the repository, resolved from the subfolder to its root,
        // and the pane lands in what it makes - `cwd` says nothing there.
        let checkout = GitCheckout::NewBranch {
            branch: "topic".to_owned(),
            base: "main".to_owned(),
        };
        let switched = agent_folder_reusing(checkout, Some(elsewhere), subdirectory).unwrap();
        assert_eq!(resolved(&switched), resolved(repository.path()));
    }

    #[test]
    fn a_worktree_request_from_inside_a_worktree_anchors_to_the_primary() {
        let repository = repository();
        let first = worktree(repository.path(), "first", "first");
        let cwd = reusing_checkout_in(
            first.clone(),
            GitCheckout::NewWorktree {
                branch: "second".to_owned(),
                base: "main".to_owned(),
                path: PathBuf::from(".worktrees/second"),
            },
        )
        .unwrap();
        // Checked before the equality, whose expected path does not exist when this
        // regresses — a missing-path panic would hide what actually went wrong.
        assert!(
            !resolved(&cwd).starts_with(resolved(&first)),
            "{cwd:?} must not be nested inside {first:?}"
        );
        assert_eq!(
            resolved(&cwd),
            resolved(&repository.path().join(".worktrees/second")),
            "the second worktree is a sibling of the first, not a child of it"
        );
        assert!(std::fs::read_to_string(cwd.join(".git"))
            .unwrap()
            .starts_with("gitdir:"));
    }

    #[test]
    fn an_absolute_worktree_path_is_honoured_from_inside_a_worktree() {
        let repository = repository();
        let first = worktree(repository.path(), "first", "first");
        let target = repository.path().join("elsewhere/second");
        let cwd = reusing_checkout_in(
            first,
            GitCheckout::NewWorktree {
                branch: "second".to_owned(),
                base: "main".to_owned(),
                path: target.clone(),
            },
        )
        .unwrap();
        assert_eq!(cwd, target);
        assert!(std::fs::read_to_string(target.join(".git"))
            .unwrap()
            .starts_with("gitdir:"));
    }

    #[test]
    fn branch_only_switches_the_current_checkout() {
        let repository = repository();
        reusing_checkout_in(
            repository.path().to_owned(),
            GitCheckout::NewBranch {
                branch: "topic".to_owned(),
                base: "main".to_owned(),
            },
        )
        .unwrap();
        let branch = process::Command::new("git")
            .arg("-C")
            .arg(repository.path())
            .args(["branch", "--show-current"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&branch.stdout).trim(), "topic");
    }

    /// The branch a checkout is on, or `None` when it is detached.
    fn branch_of(worktree: &std::path::Path) -> Option<String> {
        let output = process::Command::new("git")
            .arg("-C")
            .arg(worktree)
            .args(["branch", "--show-current"])
            .output()
            .unwrap();
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        (!branch.is_empty()).then_some(branch)
    }

    /// Create a local branch without checking it out anywhere.
    fn branch(primary: &std::path::Path, name: &str) {
        assert!(process::Command::new("git")
            .arg("-C")
            .arg(primary)
            .args(["branch", name])
            .status()
            .unwrap()
            .success());
    }

    /// The point of the whole reuse path: an agent asked for where work already
    /// is, rather than an error saying it is already there.
    #[test]
    fn a_worktree_that_already_exists_at_the_path_is_reused_as_it_stands() {
        let repository = repository();
        let existing = worktree(repository.path(), "topic", "topic");
        let prepared = prepare_git_checkout_in(
            repository.path().to_owned(),
            GitCheckout::NewWorktree {
                // Neither the branch nor the base the worktree was made with:
                // the folder is the request, and it is used as it stands.
                branch: "other".to_owned(),
                base: "main".to_owned(),
                path: PathBuf::from(".worktrees/topic"),
            },
            true,
            WorktreeGuard::Waived,
        )
        .unwrap();
        assert_eq!(resolved(&prepared.path), resolved(&existing));
        // The branch the folder holds, not the one that was asked for and not used:
        // that is what the agent is reported as sitting on.
        assert_eq!(prepared.branch.as_deref(), Some("topic"));
        assert_eq!(branch_of(&existing).as_deref(), Some("topic"));
        // Nothing was created for the branch that was asked for and not used.
        assert!(!branch_exists(repository.path(), "other"));
    }

    #[test]
    fn triage_prompts_follow_each_actual_worktree_outcome() {
        let first = repository();
        let second = repository();
        let mut spec = AgentSpawnSpec {
            kind: Some("claude".into()),
            model: Some("opus".into()),
            branch: Some("issue-2180".into()),
            reuse: Some(true),
            prompt: Some("triage issue 2180".into()),
            reuse_prompt: Some("resume issue 2180".into()),
            ..Default::default()
        };
        let prepare = |root: &std::path::Path, spec: &AgentSpawnSpec| {
            let (_, checkout) = yinz_runtime::agent::launch::plan(spec).unwrap();
            prepare_git_checkout_in(root.to_owned(), checkout, true, WorktreeGuard::Required)
                .unwrap()
        };
        // A branch without a checkout is still a new worktree.
        branch(first.path(), "issue-2180");
        let new = prepare(first.path(), &spec);
        assert!(!new.reused_worktree);
        assert_eq!(
            new.agent_command(&spec).unwrap().args.last().unwrap(),
            "triage issue 2180"
        );
        let reused = prepare(first.path(), &spec);
        let other_new = prepare(second.path(), &spec);
        assert!(reused.reused_worktree);
        assert!(!other_new.reused_worktree);
        assert_eq!(
            reused.agent_command(&spec).unwrap().args.last().unwrap(),
            "resume issue 2180"
        );
        assert_eq!(
            other_new.agent_command(&spec).unwrap().args.last().unwrap(),
            "triage issue 2180"
        );
        // Branch lookup at a different path is reuse too.
        spec.worktree_dir = Some("elsewhere".into());
        let moved = prepare(first.path(), &spec);
        assert!(moved.reused_worktree);
        assert_eq!(moved.path, reused.path);
        assert_eq!(
            moved.agent_command(&spec).unwrap().args.last().unwrap(),
            "resume issue 2180"
        );
        spec.reuse_prompt = None;
        assert_eq!(
            moved.agent_command(&spec).unwrap().args.last().unwrap(),
            "triage issue 2180"
        );
    }

    /// Git allows one checkout of a branch at a time, so the worktree holding it
    /// is the only place an agent can work on it.
    #[test]
    fn a_branch_checked_out_elsewhere_answers_with_that_worktree() {
        let repository = repository();
        let existing = worktree(repository.path(), "first", "topic");
        let cwd = reusing_checkout_in(
            repository.path().to_owned(),
            GitCheckout::NewWorktree {
                branch: "topic".to_owned(),
                base: "main".to_owned(),
                // A path of its own, which is not where the branch turned out to
                // live.
                path: PathBuf::from(".worktrees/second"),
            },
        )
        .unwrap();
        assert_eq!(resolved(&cwd), resolved(&existing));
        assert!(!repository.path().join(".worktrees/second").exists());
    }

    #[test]
    fn an_existing_branch_gets_a_worktree_without_being_recreated() {
        let repository = repository();
        branch(repository.path(), "topic");
        let cwd = reusing_checkout_in(
            repository.path().to_owned(),
            GitCheckout::NewWorktree {
                branch: "topic".to_owned(),
                // Not a branch at all: an existing branch has its own history,
                // so the base is never consulted.
                base: "no-such-base".to_owned(),
                path: PathBuf::from(".worktrees/topic"),
            },
        )
        .unwrap();
        assert_eq!(
            resolved(&cwd),
            resolved(&repository.path().join(".worktrees/topic"))
        );
        assert_eq!(branch_of(&cwd).as_deref(), Some("topic"));
    }

    #[test]
    fn an_existing_branch_is_switched_to_rather_than_recreated() {
        let repository = repository();
        branch(repository.path(), "topic");
        let cwd = reusing_checkout_in(
            repository.path().to_owned(),
            GitCheckout::NewBranch {
                branch: "topic".to_owned(),
                base: "no-such-base".to_owned(),
            },
        )
        .unwrap();
        assert_eq!(resolved(&cwd), resolved(repository.path()));
        assert_eq!(branch_of(repository.path()).as_deref(), Some("topic"));
    }

    #[test]
    fn a_branch_request_for_a_branch_held_elsewhere_answers_with_that_worktree() {
        let repository = repository();
        let existing = worktree(repository.path(), "first", "topic");
        let cwd = reusing_checkout_in(
            repository.path().to_owned(),
            GitCheckout::NewBranch {
                branch: "topic".to_owned(),
                base: "main".to_owned(),
            },
        )
        .unwrap();
        assert_eq!(resolved(&cwd), resolved(&existing));
        // The checkout the request came from was left where it was.
        assert_eq!(branch_of(repository.path()).as_deref(), Some("main"));
    }

    /// A folder in the way that is not a worktree is a real clash, and still one
    /// git gets to report.
    #[test]
    fn a_path_that_is_not_a_worktree_still_fails() {
        let repository = repository();
        let occupied = repository.path().join(".worktrees/topic");
        std::fs::create_dir_all(&occupied).unwrap();
        std::fs::write(occupied.join("file"), "content").unwrap();
        let error = reusing_checkout_in(
            repository.path().to_owned(),
            GitCheckout::NewWorktree {
                branch: "topic".to_owned(),
                base: "main".to_owned(),
                path: PathBuf::from(".worktrees/topic"),
            },
        )
        .unwrap_err();
        assert!(
            error.starts_with("git could not create the worktree"),
            "{error}"
        );
    }

    /// Without reuse, a name already taken is stepped around rather than joined - and
    /// the worktree is renamed with its branch, because a worktree is named after the
    /// branch it holds and that is the whole of 116.
    #[test]
    fn a_taken_name_is_cut_afresh_with_the_worktree_renamed_to_match() {
        let repository = repository();
        let request = || GitCheckout::NewWorktree {
            branch: "topic".to_owned(),
            base: "main".to_owned(),
            path: PathBuf::from(".worktrees/topic"),
        };
        let first = cutting_checkout_in(repository.path().to_owned(), request()).unwrap();
        assert_eq!(first.branch.as_deref(), Some("topic"));
        assert_eq!(
            resolved(&first.path),
            resolved(&repository.path().join(".worktrees/topic"))
        );

        let second = cutting_checkout_in(repository.path().to_owned(), request()).unwrap();
        assert_eq!(second.branch.as_deref(), Some("topic-2"));
        assert_eq!(
            resolved(&second.path),
            resolved(&repository.path().join(".worktrees/topic-2"))
        );
        let third = cutting_checkout_in(repository.path().to_owned(), request()).unwrap();
        assert_eq!(third.branch.as_deref(), Some("topic-3"));

        // Reuse is the other answer to the same request: the first worktree, as it stands.
        let reused = reusing_checkout_in(repository.path().to_owned(), request()).unwrap();
        assert_eq!(resolved(&reused), resolved(&first.path));
    }

    /// A folder in the way that is not a worktree is a hard failure under reuse (above);
    /// without it there is nothing to reuse and everything to step around, so the name
    /// moves on instead.
    #[test]
    fn a_folder_in_the_way_is_stepped_around_rather_than_failed_without_reuse() {
        let repository = repository();
        let occupied = repository.path().join(".worktrees/topic");
        std::fs::create_dir_all(&occupied).unwrap();
        std::fs::write(occupied.join("file"), "content").unwrap();
        let prepared = cutting_checkout_in(
            repository.path().to_owned(),
            GitCheckout::NewWorktree {
                branch: "topic".to_owned(),
                base: "main".to_owned(),
                path: PathBuf::from(".worktrees/topic"),
            },
        )
        .unwrap();
        assert_eq!(prepared.branch.as_deref(), Some("topic-2"));
    }

    /// The branch-only shape says the same thing: an existing `topic` is not joined, and
    /// the checkout moves to the next free name.
    #[test]
    fn a_branch_without_reuse_is_cut_afresh_rather_than_switched_to() {
        let repository = repository();
        let request = || GitCheckout::NewBranch {
            branch: "topic".to_owned(),
            base: "main".to_owned(),
        };
        let first = cutting_checkout_in(repository.path().to_owned(), request()).unwrap();
        assert_eq!(first.branch.as_deref(), Some("topic"));
        let second = cutting_checkout_in(repository.path().to_owned(), request()).unwrap();
        assert_eq!(second.branch.as_deref(), Some("topic-2"));
        assert_eq!(branch_of(repository.path()).as_deref(), Some("topic-2"));
    }

    /// A branch with a `/` in it is laid out as folders, and the folder it asks for is the
    /// one `launch::worktree_path_in` planned and the confirmation promised - not a level
    /// deeper. The suffix a collision cuts goes on the branch and on that folder's last
    /// component together.
    #[test]
    fn a_branch_with_a_slash_keeps_the_worktree_path_it_was_planned_with() {
        let repository = repository();
        let request = || GitCheckout::NewWorktree {
            branch: "feature/foo".to_owned(),
            base: "main".to_owned(),
            path: PathBuf::from(".worktrees/feature/foo"),
        };
        let first = cutting_checkout_in(repository.path().to_owned(), request()).unwrap();
        assert_eq!(first.branch.as_deref(), Some("feature/foo"));
        assert_eq!(
            resolved(&first.path),
            resolved(&repository.path().join(".worktrees/feature/foo"))
        );

        let second = cutting_checkout_in(repository.path().to_owned(), request()).unwrap();
        assert_eq!(second.branch.as_deref(), Some("feature/foo-2"));
        assert_eq!(
            resolved(&second.path),
            resolved(&repository.path().join(".worktrees/feature/foo-2"))
        );
    }

    /// The guard is asked *before* the git work, not of the folder it produced: a refusal
    /// that had already switched the user's checkout onto a new branch would leave them
    /// worse off than the spawn they asked for.
    #[test]
    fn a_refused_branch_leaves_the_checkout_where_it_was() {
        let repository = repository();
        let error = prepare_git_checkout_in(
            repository.path().to_owned(),
            GitCheckout::NewBranch {
                branch: "topic".to_owned(),
                base: "main".to_owned(),
            },
            false,
            WorktreeGuard::Required,
        )
        .unwrap_err();
        assert!(error.contains("is not a linked git worktree"), "{error}");
        assert_eq!(branch_of(repository.path()).as_deref(), Some("main"));
        assert!(!branch_exists(repository.path(), "topic"));
    }

    /// The guard is about the destination rather than the spec: the same request from
    /// inside a linked worktree moves that worktree, and is allowed.
    #[test]
    fn a_branch_cut_inside_a_linked_worktree_passes_the_guard() {
        let repository = repository();
        let linked = worktree(repository.path(), "other", "other");
        let prepared = prepare_git_checkout_in(
            linked.clone(),
            GitCheckout::NewBranch {
                branch: "topic".to_owned(),
                base: "main".to_owned(),
            },
            false,
            WorktreeGuard::Required,
        )
        .unwrap();
        assert_eq!(prepared.branch.as_deref(), Some("topic"));
        assert_eq!(resolved(&prepared.path), resolved(&linked));
        assert_eq!(branch_of(&linked).as_deref(), Some("topic"));
        // The primary checkout was not touched.
        assert_eq!(branch_of(repository.path()).as_deref(), Some("main"));
    }

    /// The no-git-work shapes are guarded too - a spawn with no branch is the case the
    /// guard was written for - and `agent_folder` owns that check for the folder it is
    /// handed as much as for the repository root.
    #[test]
    fn a_folder_that_is_no_linked_worktree_is_refused_without_git_work() {
        let repository = repository();
        let error = agent_folder(
            GitCheckout::Current,
            None,
            repository.path().to_owned(),
            false,
            WorktreeGuard::Required,
        )
        .unwrap_err();
        assert!(error.contains("is not a linked git worktree"), "{error}");
        let outside = tempfile::tempdir().unwrap();
        let error = agent_folder(
            GitCheckout::Current,
            Some(outside.path().to_owned()),
            repository.path().to_owned(),
            false,
            WorktreeGuard::Required,
        )
        .unwrap_err();
        assert!(error.contains("is not a linked git worktree"), "{error}");
        // And a linked worktree is what it asks for.
        let linked = worktree(repository.path(), "topic", "topic");
        let prepared = agent_folder(
            GitCheckout::Current,
            Some(linked.clone()),
            repository.path().to_owned(),
            false,
            WorktreeGuard::Required,
        )
        .unwrap();
        assert_eq!(prepared.path, linked);
    }

    /// What the guard in `spawn_agent` asks of a folder before an agent is left in it.
    #[test]
    fn only_a_linked_checkout_is_a_linked_worktree() {
        let repository = repository();
        assert!(!linked_worktree(repository.path()).unwrap());
        let linked = worktree(repository.path(), "topic", "topic");
        assert!(linked_worktree(&linked).unwrap());
        // A subfolder of one is inside it, and answers for it.
        let nested = linked.join("src");
        std::fs::create_dir_all(&nested).unwrap();
        assert!(linked_worktree(&nested).unwrap());
        // A folder that is no repository at all is not a worktree either, and saying so
        // is the answer rather than an error: that spawn is one the caller must own.
        let outside = tempfile::tempdir().unwrap();
        assert!(!linked_worktree(outside.path()).unwrap());
    }

    #[test]
    fn the_porcelain_listing_reads_a_path_and_a_branch_per_checkout() {
        let worktrees = parse_worktrees(
            "worktree /work/repo\nHEAD abcd\nbranch refs/heads/main\n\
             \nworktree /work/repo/.worktrees/topic\nHEAD ef01\nbranch refs/heads/topic\n\
             \nworktree /work/detached\nHEAD 2345\ndetached\n",
        );
        let read: Vec<_> = worktrees
            .iter()
            .map(|worktree| (worktree.path.clone(), worktree.branch.clone()))
            .collect();
        assert_eq!(
            read,
            [
                (PathBuf::from("/work/repo"), Some("main".to_owned())),
                (
                    PathBuf::from("/work/repo/.worktrees/topic"),
                    Some("topic".to_owned())
                ),
                (PathBuf::from("/work/detached"), None),
            ]
        );
    }
}

#[cfg(test)]
#[path = "unit/plugin_log_tests.rs"]
mod plugin_log_tests;
