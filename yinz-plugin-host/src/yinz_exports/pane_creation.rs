//! Pane launch commands and plugin-relative command paths.

use super::agent_launch::{prepare_git_checkout, wait_for_agent_placement};
use super::pane_control::notify;
use super::write_response;
use crate::plugin_map::PluginEnv;
use crate::{PaneId, PluginInstruction};
use prost::Message;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use tokio::sync::oneshot;
use yinz_bus::global_async_runtime::get_tokio_runtime;
use yinz_bus::pty::{ClientTabIndexOrPaneId, PtyInstruction, ReplacedPane};
use yinz_bus::route::NotificationEnd;
use yinz_bus::screen::ScreenInstruction;
use yinz_plugin_api::plugin_command::{
    ProtobufOpenCommandPaneBackgroundResponse, ProtobufOpenCommandPaneFloatingNearPluginResponse,
    ProtobufOpenCommandPaneFloatingResponse, ProtobufOpenCommandPaneInGitCheckoutResponse,
    ProtobufOpenCommandPaneInPlaceOfPaneIdResponse, ProtobufOpenCommandPaneInPlaceOfPluginResponse,
    ProtobufOpenCommandPaneInPlaceResponse, ProtobufOpenCommandPaneNearPluginResponse,
    ProtobufOpenCommandPaneOverlayResponse, ProtobufOpenCommandPaneResponse,
    ProtobufOpenEditPaneInPlaceOfPaneIdResponse, ProtobufOpenFileFloatingNearPluginResponse,
    ProtobufOpenFileFloatingResponse, ProtobufOpenFileInPlaceOfPluginResponse,
    ProtobufOpenFileInPlaceResponse, ProtobufOpenFileNearPluginResponse, ProtobufOpenFileResponse,
    ProtobufOpenPaneInNewTabResponse, ProtobufOpenPluginPaneFloatingResponse,
    ProtobufOpenPluginPaneInPlaceOfPaneIdResponse, ProtobufOpenTerminalFloatingNearPluginResponse,
    ProtobufOpenTerminalFloatingResponse, ProtobufOpenTerminalInPlaceOfPluginResponse,
    ProtobufOpenTerminalInPlaceResponse, ProtobufOpenTerminalNearPluginResponse,
    ProtobufOpenTerminalPaneInPlaceOfPaneIdResponse, ProtobufOpenTerminalResponse,
};
use yinz_routing::{route_action, wait_for_action_completion};
use yinz_runtime::consts::YINZ_PLUGIN_TMP_DIR;
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::AgentLaunchResources;
use yinz_types::command::{CommandToRun, GitCheckout};
use yinz_types::data::{
    CommandOrPlugin, Event, FileToOpen, NewPanePlacement, OpenCommandPaneBackgroundResponse,
    OpenCommandPaneFloatingNearPluginResponse, OpenCommandPaneFloatingResponse,
    OpenCommandPaneInGitCheckoutResponse, OpenCommandPaneInPlaceOfPaneIdResponse,
    OpenCommandPaneInPlaceOfPluginResponse, OpenCommandPaneInPlaceResponse,
    OpenCommandPaneOverlayResponse, OpenCommandPaneResponse, OpenEditPaneInPlaceOfPaneIdResponse,
    OpenFileFloatingNearPluginResponse, OpenFileFloatingResponse, OpenFileInPlaceOfPluginResponse,
    OpenFileInPlaceResponse, OpenFileNearPluginResponse, OpenFileResponse,
    OpenPaneInNewTabResponse, OpenPluginPaneFloatingResponse,
    OpenTerminalFloatingNearPluginResponse, OpenTerminalFloatingResponse,
    OpenTerminalInPlaceOfPluginResponse, OpenTerminalInPlaceResponse,
    OpenTerminalNearPluginResponse, OpenTerminalPaneInPlaceOfPaneIdResponse, OpenTerminalResponse,
    OverlayPlacement, AGENT_PANE_CONTEXT,
};
use yinz_types::geometry::{Direction, FloatingPaneCoordinates};
use yinz_types::input::actions::Action;
use yinz_types::input::command::{OpenFilePayload, RunCommand, RunCommandAction, TerminalAction};
use yinz_types::internal_ids::TerminalId;

pub(super) fn translate_plugin_path(env: &PluginEnv, path: PathBuf) -> PathBuf {
    if let Ok(stripped) = path.strip_prefix("/host") {
        env.plugin_cwd.join(stripped)
    } else if let Ok(stripped) = path.strip_prefix("/data") {
        env.plugin_own_data_dir.join(stripped)
    } else if let Ok(stripped) = path.strip_prefix("/cache") {
        env.plugin_own_cache_dir.join(stripped)
    } else if let Ok(stripped) = path.strip_prefix("/tmp") {
        YINZ_PLUGIN_TMP_DIR.join(stripped)
    } else if path.is_relative() {
        env.plugin_cwd.join(path)
    } else {
        // Absolute path not in any plugin special folder — pass through
        path
    }
}

/// The `RunCommandAction` behind every plugin call that opens or re-runs a
/// command pane: the plugin's `CommandToRun` with its cwd resolved through the
/// plugin mounts (`command_cwd`), tagged as this plugin's so the pane's exit
/// comes back to it with `context`. Placement is the caller's to set; the
/// defaults here are the ones every caller shared - held on close, not
/// suspended on start.
pub(super) fn run_command_action(
    env: &PluginEnv,
    mut command_to_run: CommandToRun,
    default_to_plugin_cwd: bool,
    context: BTreeMap<String, String>,
) -> Option<RunCommandAction> {
    let cwd = if env.plugin_config.is_builtin()
        && context
            .get("agent_agent_host_cwd")
            .is_some_and(|value| value == "true")
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
    if let Err(error) = yinz_runtime::agent::pi::prepare_command(&mut command_to_run) {
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
        if let Some(cwd) = command.cwd.as_deref()
            && ["/host", "/data", "/cache"]
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
        command.cwd.clone()
    } else {
        command
            .cwd
            .clone()
            .map(|cwd| translate_plugin_path(env, cwd))
            .or_else(|| default_to_plugin_cwd.then(|| env.plugin_cwd.clone()))
    }
}

pub(super) fn open_command_pane_in_new_tab(
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
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_command_pane_in_new_tab response".to_string())
        .non_fatal();
}

pub(super) fn open_plugin_pane_in_new_tab(
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
            write_response(env, &response.encode_to_vec())
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
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_plugin_pane_in_new_tab response".to_string())
        .non_fatal();
}

pub(super) fn open_plugin_pane_floating(
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
            write_response(env, &response.encode_to_vec())
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
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_plugin_pane_floating response".to_string())
        .non_fatal();
}

pub(super) fn open_editor_pane_in_new_tab(
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
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_editor_pane_in_new_tab response".to_string())
        .non_fatal();
}

pub(super) fn open_file(
    env: &mut PluginEnv,
    file_to_open: FileToOpen,
    context: BTreeMap<String, String>,
) {
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
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_file response".to_string())
        .non_fatal();
}

pub(super) fn run_action(env: &PluginEnv, mut action: Action, context: BTreeMap<String, String>) {
    let ordered_input = matches!(&action, Action::Write { key_with_modifier: Some(_), bytes, .. } if bytes.is_empty());
    // Clone the necessary data to move into the thread
    action.populate_originating_plugin(env.originating_plugin(context.clone()));
    let action_clone = action.clone();
    let generation = env.generation;
    let client_id = env.client_id;
    let plugin_id = env.plugin_id;
    // A headless plugin has no pane to place things near: they go by the client.
    let source_pane = env.own_pane();
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
            source_pane,
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
            generation,
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

pub(super) fn open_file_floating(
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
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_file_floating response".to_string())
        .non_fatal();
}

pub(super) fn open_file_in_place(
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
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_file_in_place response".to_string())
        .non_fatal();
}

pub(super) fn open_file_near_plugin(
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
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_file_near_plugin response".to_string())
        .non_fatal();
}

pub(super) fn open_file_floating_near_plugin(
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
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_file_floating_near_plugin response".to_string())
        .non_fatal();
}

/// The pane an in-place command opened, or why it could not open one. A target that is
/// missing or suppressed is refused rather than covered, and the refusal rejects the
/// command instead of answering with the id of a pane that was never placed.
pub(super) fn in_place_pane_id(
    result: yinz_bus::route::ActionCompletionResult,
) -> Result<Option<PaneId>> {
    match result.error_message {
        Some(error) => Err(anyhow!(error)),
        None => Ok(result.affected_pane_id),
    }
}

pub(super) fn open_file_in_place_of_plugin(
    env: &mut PluginEnv,
    file_to_open: FileToOpen,
    close_plugin_after_replace: bool,
    context: BTreeMap<String, String>,
) -> Result<()> {
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
        ReplacedPane::lent_unless_closed(close_plugin_after_replace),
        ClientTabIndexOrPaneId::PaneId(PaneId::Plugin(env.plugin_id)),
        Some(NotificationEnd::new(completion_tx)),
    );
    let _ = env.senders.send_to_pty(pty_instr);

    // Wait for completion
    let result = wait_for_action_completion(completion_rx, "open_file_in_place_of_plugin", false);
    let pane_id: OpenFileInPlaceOfPluginResponse = in_place_pane_id(result)?;

    // Write response to plugin
    let response = ProtobufOpenFileInPlaceOfPluginResponse::from(pane_id);
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_file_in_place_of_plugin response".to_string())
        .non_fatal();
    Ok(())
}

pub(super) fn open_terminal(env: &mut PluginEnv, cwd: PathBuf) {
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
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_terminal response".to_string())
        .non_fatal();
}

pub(super) fn open_terminal_near_plugin(env: &mut PluginEnv, cwd: PathBuf) {
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
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_terminal_near_plugin response".to_string())
        .non_fatal();
}

pub(super) fn open_terminal_floating(
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
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_terminal_floating response".to_string())
        .non_fatal();
}

pub(super) fn open_terminal_floating_near_plugin(
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
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_terminal_floating_near_plugin response".to_string())
        .non_fatal();
}

pub(super) fn open_terminal_in_place(env: &mut PluginEnv, cwd: PathBuf) {
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
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_terminal_in_place response".to_string())
        .non_fatal();
}

pub(super) fn open_terminal_in_place_of_plugin(
    env: &mut PluginEnv,
    cwd: PathBuf,
    close_plugin_after_replace: bool,
) -> Result<()> {
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
            ReplacedPane::lent_unless_closed(close_plugin_after_replace),
            ClientTabIndexOrPaneId::PaneId(PaneId::Plugin(env.plugin_id)),
            Some(NotificationEnd::new(completion_tx)),
        ));

    // Wait for completion
    let result =
        wait_for_action_completion(completion_rx, "open_terminal_in_place_of_plugin", false);
    let pane_id: OpenTerminalInPlaceOfPluginResponse = in_place_pane_id(result)?;

    // Write response to plugin
    let response = ProtobufOpenTerminalInPlaceOfPluginResponse::from(pane_id);
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_terminal_in_place_of_plugin response".to_string())
        .non_fatal();
    Ok(())
}

pub(super) fn open_command_pane_in_place_of_plugin(
    env: &mut PluginEnv,
    command_to_run: CommandToRun,
    close_plugin_after_replace: bool,
    context: BTreeMap<String, String>,
) -> Result<()> {
    let name: Option<String> = None;
    let Some(run_command_action) = run_command_action(env, command_to_run, false, context) else {
        return Ok(());
    };
    let run_cmd = TerminalAction::RunCommand(run_command_action.into());

    // Create completion channel
    let (completion_tx, completion_rx) = oneshot::channel();
    let _ = env
        .senders
        .send_to_pty(PtyInstruction::SpawnInPlaceTerminal(
            Some(run_cmd),
            name,
            ReplacedPane::lent_unless_closed(close_plugin_after_replace),
            ClientTabIndexOrPaneId::PaneId(PaneId::Plugin(env.plugin_id)),
            Some(NotificationEnd::new(completion_tx)),
        ));

    // Wait for completion
    let result =
        wait_for_action_completion(completion_rx, "open_command_pane_in_place_of_plugin", false);
    let pane_id: OpenCommandPaneInPlaceOfPluginResponse = in_place_pane_id(result)?;

    // Write response to plugin
    let response = ProtobufOpenCommandPaneInPlaceOfPluginResponse::from(pane_id);
    write_response(env, &response.encode_to_vec())
        .with_context(|| {
            "failed to write open_command_pane_in_place_of_plugin response".to_string()
        })
        .non_fatal();
    Ok(())
}

pub(super) fn open_terminal_pane_in_place_of_pane_id(
    env: &mut PluginEnv,
    pane_id_to_replace: yinz_types::data::PaneId,
    cwd: FileToOpen,
    close_replaced_pane: bool,
) -> Result<()> {
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
            ReplacedPane::lent_unless_closed(close_replaced_pane),
            ClientTabIndexOrPaneId::PaneId(pane_id_to_replace),
            Some(NotificationEnd::new(completion_tx)),
        ));

    let result = wait_for_action_completion(
        completion_rx,
        "open_terminal_pane_in_place_of_pane_id",
        false,
    );
    let pane_id: OpenTerminalPaneInPlaceOfPaneIdResponse = in_place_pane_id(result)?;

    let response = ProtobufOpenTerminalPaneInPlaceOfPaneIdResponse::from(pane_id);
    write_response(env, &response.encode_to_vec())
        .with_context(|| {
            "failed to write open_terminal_pane_in_place_of_pane_id response".to_string()
        })
        .non_fatal();
    Ok(())
}

pub(super) fn open_command_pane_in_place_of_pane_id(
    env: &mut PluginEnv,
    pane_id_to_replace: yinz_types::data::PaneId,
    command_to_run: CommandToRun,
    close_replaced_pane: bool,
    context: BTreeMap<String, String>,
) -> Result<()> {
    let name: Option<String> = None;
    let Some(run_command_action) = run_command_action(env, command_to_run, false, context) else {
        return Ok(());
    };
    let run_cmd = TerminalAction::RunCommand(run_command_action.into());

    let (completion_tx, completion_rx) = oneshot::channel();
    let _ = env
        .senders
        .send_to_pty(PtyInstruction::SpawnInPlaceTerminal(
            Some(run_cmd),
            name,
            ReplacedPane::lent_unless_closed(close_replaced_pane),
            ClientTabIndexOrPaneId::PaneId(pane_id_to_replace),
            Some(NotificationEnd::new(completion_tx)),
        ));

    let result = wait_for_action_completion(
        completion_rx,
        "open_command_pane_in_place_of_pane_id",
        false,
    );
    let pane_id: OpenCommandPaneInPlaceOfPaneIdResponse = in_place_pane_id(result)?;

    let response = ProtobufOpenCommandPaneInPlaceOfPaneIdResponse::from(pane_id);
    write_response(env, &response.encode_to_vec())
        .with_context(|| {
            "failed to write open_command_pane_in_place_of_pane_id response".to_string()
        })
        .non_fatal();
    Ok(())
}

/// Load a plugin in place of `pane_id`, which lends the new pane its slot: it comes back
/// when the plugin pane closes, however that happens. Waits for the pane to be placed, so a
/// missing or suppressed target, or an instance that already exists, rejects the command.
pub(super) fn open_plugin_pane_in_place_of_pane_id(
    env: &mut PluginEnv,
    pane_id: PaneId,
    plugin_url: String,
    configuration: BTreeMap<String, String>,
    context: BTreeMap<String, String>,
) -> Result<()> {
    let _ = context; // context is not currently used for plugin panes
    let (completion_tx, completion_rx) = oneshot::channel();
    env.senders
        .send_to_screen(ScreenInstruction::OpenPluginPaneInPlaceOfPaneId {
            pane_id,
            plugin_url,
            configuration,
            cwd: env.plugin_cwd.clone(),
            client_id: env.client_id,
            completion: NotificationEnd::new(completion_tx),
        })
        .context("route in-place plugin pane")?;
    // Three threads answer this, one after the other, so the default action budget is
    // too tight; a dropped completion arrives as an unconfirmed result rather than a hang.
    let result = get_tokio_runtime()
        .block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(10), completion_rx).await
        })
        .context("in-place plugin pane outcome unknown; inspect the pane before retrying")?
        .context("in-place plugin pane was dropped before it was placed")?;
    let pane_id = in_place_pane_id(result)?.context("in-place plugin pane was not placed")?;
    let response = ProtobufOpenPluginPaneInPlaceOfPaneIdResponse::from(Some(pane_id));
    write_response(env, &response.encode_to_vec())
        .context("failed to write open_plugin_pane_in_place_of_pane_id response")
}

pub(super) fn open_edit_pane_in_place_of_pane_id(
    env: &mut PluginEnv,
    pane_id_to_replace: yinz_types::data::PaneId,
    file_to_open: FileToOpen,
    close_replaced_pane: bool,
    context: BTreeMap<String, String>,
) -> Result<()> {
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
        ReplacedPane::lent_unless_closed(close_replaced_pane),
        ClientTabIndexOrPaneId::PaneId(pane_id_to_replace),
        Some(NotificationEnd::new(completion_tx)),
    );
    let _ = env.senders.send_to_pty(pty_instr);

    let result =
        wait_for_action_completion(completion_rx, "open_edit_pane_in_place_of_pane_id", false);
    let pane_id: OpenEditPaneInPlaceOfPaneIdResponse = in_place_pane_id(result)?;

    let response = ProtobufOpenEditPaneInPlaceOfPaneIdResponse::from(pane_id);
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_edit_pane_in_place_of_pane_id response".to_string())
        .non_fatal();
    Ok(())
}

pub(super) fn open_command_pane(
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
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_command_pane response".to_string())
        .non_fatal();
}

pub(super) fn open_command_pane_near_plugin(
    env: &mut PluginEnv,
    command_to_run: CommandToRun,
    context: BTreeMap<String, String>,
) {
    let pane_id =
        spawn_command_pane_near_plugin(env, command_to_run, context, None, false, false).ok();
    let response = ProtobufOpenCommandPaneNearPluginResponse::from(pane_id);
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_command_pane_near_plugin response".to_string())
        .non_fatal();
}

pub(super) fn spawn_command_pane_near_plugin(
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

pub(super) fn open_command_pane_in_git_checkout_near_plugin(
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
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_command_pane_in_git_checkout response".to_string())
        .non_fatal();
}

pub(super) fn open_command_pane_floating(
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
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_command_pane_floating response".to_string())
        .non_fatal();
}

/// Open a command pane directly onto the session surface.
///
/// The placement travels whole rather than being flattened to coordinates: a terminal pane has no
/// plugin configuration for the server to read overlay-ness back out of, so this is the only thing
/// that knows whether the pane is a modal.
pub(super) fn open_command_pane_overlay(
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
        crate::plugin_bridge::picker::attach_opened_pane(env, None).non_fatal();
        let response = ProtobufOpenCommandPaneOverlayResponse::from(None);
        write_response(env, &response.encode_to_vec())
            .context("reject cancelled picker overlay")
            .non_fatal();
        return;
    }
    let error_msg = || format!("failed to open command in plugin {}", env.name());
    let Some(run_command_action) = run_command_action(env, command_to_run, false, context) else {
        crate::plugin_bridge::picker::attach_opened_pane(env, None).non_fatal();
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
    crate::plugin_bridge::picker::attach_opened_pane(env, pane_id).non_fatal();
    let response = ProtobufOpenCommandPaneOverlayResponse::from(pane_id);
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_command_pane_overlay response".to_string())
        .non_fatal();
}

pub(super) fn open_command_pane_floating_near_plugin(
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
    write_response(env, &response.encode_to_vec())
        .with_context(|| {
            "failed to write open_command_pane_floating_near_plugin response".to_string()
        })
        .non_fatal();
}

pub(super) fn open_command_pane_in_place(
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
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_command_pane_in_place response".to_string())
        .non_fatal();
}

pub(super) fn open_command_pane_background(
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
    write_response(env, &response.encode_to_vec())
        .with_context(|| "failed to write open_command_pane_background response".to_string())
        .non_fatal();
}

pub(super) fn rerun_command_pane(env: &PluginEnv, terminal_pane_id: TerminalId) {
    let _ = env
        .senders
        .send_to_screen(ScreenInstruction::RerunCommandPane(terminal_pane_id, None));
}

/// Run a *different* command in an existing command pane, reusing its pane id. The `context`
/// round-trips on the resulting `CommandPaneExited`, addressed to this plugin.
pub(super) fn run_command_in_pane(
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
