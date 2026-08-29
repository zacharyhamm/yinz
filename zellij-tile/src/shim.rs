use serde::{de::DeserializeOwned, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::{
    io,
    path::{Path, PathBuf},
};
use zellij_utils::data::*;
use zellij_utils::errors::prelude::*;
use zellij_utils::input::actions::Action;
pub use zellij_utils::input::layout::PercentOrFixed;
pub use zellij_utils::plugin_api;
use zellij_utils::plugin_api::event::ProtobufPaneScrollbackResponse;
use zellij_utils::plugin_api::generated_api::api::plugin_command::{
    hide_floating_panes_response, save_session_response, show_floating_panes_response,
};
use zellij_utils::plugin_api::plugin_command::{
    dump_layout_response, dump_session_layout_response, fs_list_response, fs_metadata_response,
    fs_mutate_response, fs_read_response, get_focused_pane_info_response, get_pane_cwd_response,
    get_pane_git_probe_response, get_pane_running_command_response, get_session_list_response,
    parse_layout_response, CreateTokenResponse, ListTokensResponse,
    ProtobufBreakPanesToNewTabResponse, ProtobufBreakPanesToTabWithIdResponse,
    ProtobufBreakPanesToTabWithIndexResponse, ProtobufCheckLuaPluginResponse,
    ProtobufCurrentSessionLastSavedTimeResponse, ProtobufDataApplyResponse,
    ProtobufDataCountResponse, ProtobufDataDefineResponse, ProtobufDataFetchResponse,
    ProtobufDataUnwatchResponse, ProtobufDataWatchResponse, ProtobufDeleteAllDeadSessionsResponse,
    ProtobufDeleteDeadSessionResponse, ProtobufDeleteLayoutResponse, ProtobufDumpLayoutResponse,
    ProtobufDumpSessionLayoutResponse, ProtobufEditLayoutResponse, ProtobufEnqueueQueueJobResponse,
    ProtobufFocusOrCreateTabResponse, ProtobufFsListResponse, ProtobufFsMetadataResponse,
    ProtobufFsMutateResponse, ProtobufFsReadResponse, ProtobufGenerateRandomNameResponse,
    ProtobufGetFocusedPaneInfoResponse, ProtobufGetLayoutDirResponse, ProtobufGetPaneCwdResponse,
    ProtobufGetPaneGitProbeResponse, ProtobufGetPaneInfoResponse, ProtobufGetPanePidResponse,
    ProtobufGetPaneRunningCommandResponse, ProtobufGetPluginAliasConfigurationResponse,
    ProtobufGetSessionEnvironmentVariablesResponse, ProtobufGetSessionListResponse,
    ProtobufGetTabInfoResponse, ProtobufGetUserPluginDirResponse,
    ProtobufHideFloatingPanesResponse, ProtobufKillSessionsResponse,
    ProtobufListNotificationsResponse, ProtobufListOrphanedPanesResponse,
    ProtobufListRecordingsResponse, ProtobufNewTabResponse, ProtobufNewTabUnfocusedResponse,
    ProtobufNewTabsResponse, ProtobufNewTiledPaneInTabResponse,
    ProtobufOpenCommandPaneBackgroundResponse, ProtobufOpenCommandPaneFloatingNearPluginResponse,
    ProtobufOpenCommandPaneFloatingResponse, ProtobufOpenCommandPaneInGitCheckoutResponse,
    ProtobufOpenCommandPaneInPlaceOfPaneIdResponse, ProtobufOpenCommandPaneInPlaceOfPluginResponse,
    ProtobufOpenCommandPaneInPlaceResponse, ProtobufOpenCommandPaneNearPluginResponse,
    ProtobufOpenCommandPaneOverlayResponse, ProtobufOpenCommandPaneResponse,
    ProtobufOpenEditPaneInPlaceOfPaneIdResponse, ProtobufOpenFileFloatingNearPluginResponse,
    ProtobufOpenFileFloatingResponse, ProtobufOpenFileInPlaceOfPluginResponse,
    ProtobufOpenFileInPlaceResponse, ProtobufOpenFileNearPluginResponse, ProtobufOpenFileResponse,
    ProtobufOpenPaneInNewTabResponse, ProtobufOpenPluginPaneFloatingResponse,
    ProtobufOpenTerminalFloatingNearPluginResponse, ProtobufOpenTerminalFloatingResponse,
    ProtobufOpenTerminalInPlaceOfPluginResponse, ProtobufOpenTerminalInPlaceResponse,
    ProtobufOpenTerminalNearPluginResponse, ProtobufOpenTerminalPaneInPlaceOfPaneIdResponse,
    ProtobufOpenTerminalResponse, ProtobufParseLayoutResponse, ProtobufPluginCommand,
    ProtobufQueueStatusResponse, ProtobufRenameLayoutResponse, ProtobufSaveLayoutResponse,
    ProtobufSaveSessionResponse, ProtobufSearchRecordingsResponse,
    ProtobufShowFloatingPanesResponse, ProtobufWarpPaneResponse, RenameWebTokenResponse,
    RevokeAllWebTokensResponse, RevokeTokenResponse,
};
use zellij_utils::plugin_api::plugin_ids::{ProtobufPluginIds, ProtobufZellijVersion};

pub use super::ui_components::*;
pub use prost::{self, *};

// Subscription Handling

/// Subscribe to a list of [`Event`]s represented by their [`EventType`]s that will then trigger the `update` method
pub fn subscribe(event_types: &[EventType]) {
    let event_types: HashSet<EventType> = event_types.iter().cloned().collect();
    let plugin_command = PluginCommand::Subscribe(event_types);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Unsubscribe to a list of [`Event`]s represented by their [`EventType`]s.
pub fn unsubscribe(event_types: &[EventType]) {
    let event_types: HashSet<EventType> = event_types.iter().cloned().collect();
    let plugin_command = PluginCommand::Unsubscribe(event_types);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

// Plugin Settings

/// Sets the plugin as selectable or unselectable to the user. Unselectable plugins might be desired when they do not accept user input.
pub fn set_selectable(selectable: bool) {
    let plugin_command = PluginCommand::SetSelectable(selectable);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Shows the cursor at specific coordinates or hides it
///
/// # Arguments
/// * `cursor_position` - None to hide cursor, Some((x, y)) to show at coordinates
pub fn show_cursor(cursor_position: Option<(usize, usize)>) {
    #[cfg(test)]
    LAST_CURSOR_POSITION.with(|last| *last.borrow_mut() = Some(cursor_position));
    let plugin_command = PluginCommand::ShowCursor(cursor_position);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

#[cfg(test)]
thread_local! {
    static LAST_CURSOR_POSITION: std::cell::RefCell<Option<Option<(usize, usize)>>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
pub(crate) fn take_last_cursor_position() -> Option<Option<(usize, usize)>> {
    LAST_CURSOR_POSITION.with(|last| last.borrow_mut().take())
}

pub fn request_permission(permissions: &[PermissionType]) {
    let plugin_command = PluginCommand::RequestPluginPermissions(permissions.into());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

// Query Functions
/// Returns the unique Zellij pane ID for the plugin as well as the Zellij process id.
pub fn get_plugin_ids() -> PluginIds {
    let plugin_command = PluginCommand::GetPluginIds;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
    let protobuf_plugin_ids =
        ProtobufPluginIds::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    PluginIds::try_from(protobuf_plugin_ids).unwrap()
}

/// Returns the version of the running Zellij instance - can be useful to check plugin compatibility
pub fn get_zellij_version() -> String {
    let plugin_command = PluginCommand::GetZellijVersion;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
    let protobuf_zellij_version =
        ProtobufZellijVersion::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    protobuf_zellij_version.version
}

/// Generates a random human-readable name using Zellij's curated word lists.
/// Returns a name in the format AdjectiveNoun (e.g., "BraveRustacean", "ZippyWeasel").
///
/// This uses the same word lists as session name generation, providing
/// approximately 4,096 unique combinations.
pub fn generate_random_name() -> String {
    let plugin_command = PluginCommand::GenerateRandomName;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
    let response =
        ProtobufGenerateRandomNameResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    response.name
}

/// Dumps a layout by name and returns its KDL content as a String
///
/// Supports both built-in layouts (eg. "default", "compact", "welcome")
/// and custom layouts from the plugin's layout directory.
pub fn dump_layout(layout_name: &str) -> Result<String, String> {
    // Create the plugin command with the layout name
    let plugin_command = PluginCommand::DumpLayout(layout_name.to_string());

    // Convert to protobuf and encode
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());

    // Call the host function (blocks until response)
    unsafe { host_run_plugin_command() };

    // Read and decode the response
    let response_bytes =
        bytes_from_stdin().map_err(|e| format!("Failed to read response from stdin: {:?}", e))?;

    let protobuf_response = ProtobufDumpLayoutResponse::decode(response_bytes.as_slice())
        .map_err(|e| format!("Failed to decode protobuf response: {}", e))?;

    // Extract result from the oneof field
    match protobuf_response.result {
        Some(dump_layout_response::Result::LayoutContent(content)) => Ok(content),
        Some(dump_layout_response::Result::Error(error)) => Err(error),
        None => Err("Server returned empty response".to_string()),
    }
}

/// Returns the path to the layout directory.
///
/// This is the directory where Zellij looks for layout files. It can be:
/// - The directory specified via CLI `--layout-dir` flag
/// - The directory specified in the config file
/// - The directory specified via ZELLIJ_LAYOUT_DIR env var
/// - The default: `~/.config/zellij/layouts`
///
/// # Returns
/// A String containing the absolute path to the layout directory.
/// Returns an empty string if the layout directory cannot be determined (rare edge case).
pub fn get_layout_dir() -> String {
    let plugin_command = PluginCommand::GetLayoutDir;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
    let response =
        ProtobufGetLayoutDirResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    response.layout_dir
}

/// The folder Zellij looks in for the user's own plugins.
///
/// # Returns
/// A String containing the absolute path to that folder.
/// Returns an empty string if it cannot be determined, in which case there is nowhere to write
/// a plugin and the caller should say so rather than pick a path of its own.
pub fn get_user_plugin_dir() -> String {
    let plugin_command = PluginCommand::GetUserPluginDir;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
    let response =
        ProtobufGetUserPluginDirResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    response.user_plugin_dir
}

pub fn get_session_environment_variables() -> BTreeMap<String, String> {
    let plugin_command = PluginCommand::GetSessionEnvironmentVariables;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();

    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response = ProtobufGetSessionEnvironmentVariablesResponse::decode(
        bytes_from_stdin().unwrap().as_slice(),
    )
    .unwrap();

    response
        .env_vars
        .into_iter()
        .map(|env_var| (env_var.name, env_var.value))
        .collect()
}

/// The KDL configuration block declared for `alias` in the `plugins { }` section of the
/// user's config.
///
/// This is how a plugin reads settings that a *different* plugin was configured with. Its
/// own configuration already arrives through [`ZellijPlugin::load`], and
/// `SessionInfo.plugins` covers plugins that are currently loaded - but only those, so it
/// cannot answer for an alias whose plugin has never been launched in this session.
///
/// An alias nobody declared answers with an empty map: "nothing is configured under that
/// name", which is the same thing a declared-but-empty block means.
pub fn get_plugin_alias_configuration(alias: &str) -> BTreeMap<String, String> {
    let plugin_command = PluginCommand::GetPluginAliasConfiguration(alias.to_owned());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();

    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufGetPluginAliasConfigurationResponse::decode(bytes_from_stdin().unwrap().as_slice())
            .unwrap();

    response.configuration.into_iter().collect()
}

/// Returns the focused pane ID and tab index for the client associated with this plugin.
pub fn get_focused_pane_info() -> Result<(usize, PaneId), String> {
    let plugin_command = PluginCommand::GetFocusedPaneInfo;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let protobuf_response =
        ProtobufGetFocusedPaneInfoResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();

    match protobuf_response.result {
        Some(get_focused_pane_info_response::Result::FocusedPaneInfo(info)) => {
            let tab_index = info.focused_tab_index as usize;
            match info.focused_pane_id {
                Some(pb_pane_id) => match pb_pane_id.try_into() {
                    Ok(pane_id) => Ok((tab_index, pane_id)),
                    Err(_) => Err("Invalid pane_id in response".to_string()),
                },
                None => Err("Missing pane_id in response".to_string()),
            }
        },
        Some(get_focused_pane_info_response::Result::Error(err)) => Err(err),
        None => Err("Empty response from host".to_string()),
    }
}

/// Query information about a specific pane by its PaneId.
///
/// This synchronously queries Zellij for detailed information about the pane with the given ID,
/// including its position, size, state, and other metadata.
///
/// # Parameters
///
/// - `pane_id`: The ID of the pane to query
///
/// # Returns
///
/// - `Some(PaneInfo)` if the pane exists and information was successfully retrieved
/// - `None` if the pane does not exist or could not be found
///
/// # Example
///
/// ```no_run
/// use zellij_tile::prelude::*;
///
/// // Query info for a specific pane
/// let pane_id = PaneId::Terminal(1);
/// match get_pane_info(pane_id) {
///     Some(info) => {
///         println!("Pane title: {}", info.title);
///         println!("Pane is focused: {}", info.is_focused);
///         println!("Pane position: ({}, {})", info.pane_x, info.pane_y);
///     },
///     None => println!("Pane not found"),
/// }
/// ```
pub fn get_pane_info(pane_id: PaneId) -> Option<PaneInfo> {
    let plugin_command = PluginCommand::GetPaneInfo(pane_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let protobuf_response =
        ProtobufGetPaneInfoResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();

    protobuf_response
        .pane_info
        .and_then(|pb_pane_info| pb_pane_info.try_into().ok())
}

/// Query information about a specific tab by its tab ID.
///
/// This synchronously queries Zellij for detailed information about the tab with the given ID,
/// including its name, position, active state, pane counts, and other metadata.
///
/// # Parameters
///
/// - `tab_id`: The stable ID of the tab to query
///
/// # Returns
///
/// - `Some(TabInfo)` if the tab exists and information was successfully retrieved
/// - `None` if the tab does not exist or could not be found
///
/// # Example
///
/// ```no_run
/// use zellij_tile::prelude::*;
///
/// // Query info for a specific tab
/// let tab_id = 3;
/// match get_tab_info(tab_id) {
///     Some(info) => {
///         println!("Tab name: {}", info.name);
///         println!("Tab position: {}", info.position);
///         println!("Tab is active: {}", info.active);
///         println!("Tiled panes: {}", info.selectable_tiled_panes_count);
///     },
///     None => println!("Tab not found"),
/// }
/// ```
pub fn get_tab_info(tab_id: usize) -> Option<TabInfo> {
    let plugin_command = PluginCommand::GetTabInfo(tab_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let protobuf_response =
        ProtobufGetTabInfoResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();

    protobuf_response
        .tab_info
        .and_then(|pb_tab_info| pb_tab_info.try_into().ok())
}

/// Save the current session state to disk immediately.
///
/// This triggers an immediate write of the current session metadata and layout
/// to the session cache directory (~/.cache/zellij/cv1/session_info/<session_name>/).
///
/// # Returns
///
/// - `Ok(())` if the save request was successfully sent
/// - `Err(String)` if there was an error sending the request
///
/// # Example
///
/// ```no_run
/// use zellij_tile::prelude::*;
///
/// // Save the current session
/// match save_session() {
///     Ok(()) => println!("Session saved successfully"),
///     Err(e) => eprintln!("Failed to save session: {}", e),
/// }
/// ```
pub fn save_session() -> Result<(), String> {
    let plugin_command = PluginCommand::SaveSession;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());

    unsafe { host_run_plugin_command() };

    let response_bytes =
        bytes_from_stdin().map_err(|e| format!("Failed to read response: {:?}", e))?;
    let protobuf_response = ProtobufSaveSessionResponse::decode(response_bytes.as_slice())
        .map_err(|e| format!("Failed to decode response: {}", e))?;

    match protobuf_response.result {
        Some(save_session_response::Result::Success(_)) => Ok(()),
        Some(save_session_response::Result::Error(error)) => Err(error),
        None => Err("Server returned empty response".to_string()),
    }
}

/// Returns the elapsed time in milliseconds since the current session state was last saved to disk.
///
/// Returns `None` if the session has never been saved during this session.
/// The returned value is the number of milliseconds elapsed since the last save, not a Unix epoch timestamp.
///
/// # Example
///
/// ```no_run
/// use zellij_tile::prelude::*;
///
/// if let Some(elapsed_millis) = current_session_last_saved_time() {
///     println!("Session was last saved {} ms ago", elapsed_millis);
/// } else {
///     println!("Session has not been saved yet");
/// }
/// ```
pub fn current_session_last_saved_time() -> Option<u64> {
    let plugin_command = PluginCommand::CurrentSessionLastSavedTime;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let protobuf_response =
        ProtobufCurrentSessionLastSavedTimeResponse::decode(bytes_from_stdin().unwrap().as_slice())
            .unwrap();

    protobuf_response.timestamp_millis
}

// Host Functions

/// Open a file in the user's default `$EDITOR` in a new pane
pub fn open_file(file_to_open: FileToOpen, context: BTreeMap<String, String>) -> Option<PaneId> {
    let plugin_command = PluginCommand::OpenFile(file_to_open, context);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response = match bytes_from_stdin() {
        Ok(bytes_from_stdin) => ProtobufOpenFileResponse::decode(bytes_from_stdin.as_slice()).ok(),
        Err(e) => {
            eprintln!("{}", e);
            None
        },
    };
    response.and_then(|r| OpenFileResponse::try_from(r).ok().flatten())
}

/// Open a file in the user's default `$EDITOR` in a new floating pane
pub fn open_file_floating(
    file_to_open: FileToOpen,
    coordinates: Option<FloatingPaneCoordinates>,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command = PluginCommand::OpenFileFloating(file_to_open, coordinates, context);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufOpenFileFloatingResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    OpenFileFloatingResponse::try_from(response).unwrap()
}

/// Open a file in the user's default `$EDITOR`, replacing the focused pane
pub fn open_file_in_place(
    file_to_open: FileToOpen,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command = PluginCommand::OpenFileInPlace(file_to_open, context);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufOpenFileInPlaceResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    OpenFileInPlaceResponse::try_from(response).unwrap()
}

/// Open a file in the user's default `$EDITOR` in a new pane near th eplugin
pub fn open_file_near_plugin(
    file_to_open: FileToOpen,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command = PluginCommand::OpenFileNearPlugin(file_to_open, context);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufOpenFileNearPluginResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    OpenFileNearPluginResponse::try_from(response).unwrap()
}

/// Open a file in the user's default `$EDITOR` in a new floating pane near the plugin
pub fn open_file_floating_near_plugin(
    file_to_open: FileToOpen,
    coordinates: Option<FloatingPaneCoordinates>,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command =
        PluginCommand::OpenFileFloatingNearPlugin(file_to_open, coordinates, context);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufOpenFileFloatingNearPluginResponse::decode(bytes_from_stdin().unwrap().as_slice())
            .unwrap();
    OpenFileFloatingNearPluginResponse::try_from(response).unwrap()
}

/// Open a file in the user's default `$EDITOR`, replacing the plugin pane
pub fn open_file_in_place_of_plugin(
    file_to_open: FileToOpen,
    close_plugin_after_replace: bool,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command =
        PluginCommand::OpenFileInPlaceOfPlugin(file_to_open, close_plugin_after_replace, context);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufOpenFileInPlaceOfPluginResponse::decode(bytes_from_stdin().unwrap().as_slice())
            .unwrap();
    OpenFileInPlaceOfPluginResponse::try_from(response).unwrap()
}
/// Open a new terminal pane to the specified location on the host filesystem
pub fn open_terminal<P: AsRef<Path>>(path: P) -> Option<PaneId> {
    let file_to_open = FileToOpen::new(path.as_ref());
    let plugin_command = PluginCommand::OpenTerminal(file_to_open);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufOpenTerminalResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    OpenTerminalResponse::try_from(response).unwrap()
}

/// Open a new terminal pane to the specified location on the host filesystem
/// This variant is identical to open_terminal, excpet it opens it near the plugin regardless of
/// whether the user was focused on it or not
pub fn open_terminal_near_plugin<P: AsRef<Path>>(path: P) -> Option<PaneId> {
    let file_to_open = FileToOpen::new(path.as_ref());
    let plugin_command = PluginCommand::OpenTerminalNearPlugin(file_to_open);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufOpenTerminalNearPluginResponse::decode(bytes_from_stdin().unwrap().as_slice())
            .unwrap();
    OpenTerminalNearPluginResponse::try_from(response).unwrap()
}

/// Open a new floating terminal pane to the specified location on the host filesystem
pub fn open_terminal_floating<P: AsRef<Path>>(
    path: P,
    coordinates: Option<FloatingPaneCoordinates>,
) -> Option<PaneId> {
    let file_to_open = FileToOpen::new(path.as_ref());
    let plugin_command = PluginCommand::OpenTerminalFloating(file_to_open, coordinates);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufOpenTerminalFloatingResponse::decode(bytes_from_stdin().unwrap().as_slice())
            .unwrap();
    OpenTerminalFloatingResponse::try_from(response).unwrap()
}

/// Open a new floating terminal pane to the specified location on the host filesystem
/// This variant is identical to open_terminal_floating, excpet it opens it near the plugin regardless of
/// whether the user was focused on it or not
pub fn open_terminal_floating_near_plugin<P: AsRef<Path>>(
    path: P,
    coordinates: Option<FloatingPaneCoordinates>,
) -> Option<PaneId> {
    let file_to_open = FileToOpen::new(path.as_ref());
    let plugin_command = PluginCommand::OpenTerminalFloatingNearPlugin(file_to_open, coordinates);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response = ProtobufOpenTerminalFloatingNearPluginResponse::decode(
        bytes_from_stdin().unwrap().as_slice(),
    )
    .unwrap();
    OpenTerminalFloatingNearPluginResponse::try_from(response).unwrap()
}

/// Open a new terminal pane to the specified location on the host filesystem, temporarily
/// replacing the focused pane
pub fn open_terminal_in_place<P: AsRef<Path>>(path: P) -> Option<PaneId> {
    let file_to_open = FileToOpen::new(path.as_ref());
    let plugin_command = PluginCommand::OpenTerminalInPlace(file_to_open);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufOpenTerminalInPlaceResponse::decode(bytes_from_stdin().unwrap().as_slice())
            .unwrap();
    OpenTerminalInPlaceResponse::try_from(response).unwrap()
}

/// Open a new terminal pane to the specified location on the host filesystem, temporarily
/// replacing the plugin pane
pub fn open_terminal_in_place_of_plugin<P: AsRef<Path>>(
    path: P,
    close_plugin_after_replace: bool,
) -> Option<PaneId> {
    let file_to_open = FileToOpen::new(path.as_ref());
    let plugin_command =
        PluginCommand::OpenTerminalInPlaceOfPlugin(file_to_open, close_plugin_after_replace);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufOpenTerminalInPlaceOfPluginResponse::decode(bytes_from_stdin().unwrap().as_slice())
            .unwrap();
    OpenTerminalInPlaceOfPluginResponse::try_from(response).unwrap()
}

/// Open a new command pane with the specified command and args (this sort of pane allows the user to control the command, re-run it and see its exit status through the Zellij UI).
pub fn open_command_pane(
    command_to_run: CommandToRun,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command = PluginCommand::OpenCommandPane(command_to_run, context);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufOpenCommandPaneResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    OpenCommandPaneResponse::try_from(response).unwrap()
}

/// Open a new command pane with the specified command and args (this sort of pane allows the user to control the command, re-run it and see its exit status through the Zellij UI).
/// This variant is the same as `open_command_pane` except it opens the pane in the same tab as the
/// plugin regardless of whether the user is focused on it
pub fn open_command_pane_near_plugin(
    command_to_run: CommandToRun,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command = PluginCommand::OpenCommandPaneNearPlugin(command_to_run, context);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufOpenCommandPaneNearPluginResponse::decode(bytes_from_stdin().unwrap().as_slice())
            .unwrap();
    OpenCommandPaneNearPluginResponse::try_from(response).unwrap()
}

/// Perform local Git checkout setup on the host, then open a command pane in its root.
pub fn open_command_pane_in_git_checkout_near_plugin(
    command_to_run: CommandToRun,
    source_directory: PathBuf,
    checkout: GitCheckout,
) -> Result<PaneId, String> {
    let plugin_command = PluginCommand::OpenCommandPaneInGitCheckoutNearPlugin(
        command_to_run,
        source_directory,
        checkout,
    );
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
    let response = ProtobufOpenCommandPaneInGitCheckoutResponse::decode(
        bytes_from_stdin()
            .map_err(|error| format!("Failed to read response: {error}"))?
            .as_slice(),
    )
    .map_err(|error| format!("Failed to decode response: {error}"))?;
    OpenCommandPaneInGitCheckoutResponse::try_from(response).map_err(|error| error.to_owned())?
}

/// Open a new floating command pane with the specified command and args (this sort of pane allows the user to control the command, re-run it and see its exit status through the Zellij UI).
pub fn open_command_pane_floating(
    command_to_run: CommandToRun,
    coordinates: Option<FloatingPaneCoordinates>,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command =
        PluginCommand::OpenCommandPaneFloating(command_to_run, coordinates, context);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufOpenCommandPaneFloatingResponse::decode(bytes_from_stdin().unwrap().as_slice())
            .unwrap();
    OpenCommandPaneFloatingResponse::try_from(response).unwrap()
}

/// Open a command pane as an overlay: over the whole session surface, above the tab's own panes,
/// rather than on its floating layer.
///
/// A terminal pane carries no plugin configuration, so the placement is the only thing that can
/// say it is an overlay - and whether it is a modal, which may not have focus taken from it.
pub fn open_command_pane_overlay(
    command_to_run: CommandToRun,
    placement: OverlayPlacement,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command = PluginCommand::OpenCommandPaneOverlay(command_to_run, placement, context);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufOpenCommandPaneOverlayResponse::decode(bytes_from_stdin().unwrap().as_slice())
            .unwrap();
    OpenCommandPaneOverlayResponse::try_from(response).unwrap()
}

/// Open a new floating command pane with the specified command and args (this sort of pane allows the user to control the command, re-run it and see its exit status through the Zellij UI).
/// This variant is the same as `open_command_pane_floating` except it opens the pane in the same tab as the
/// plugin regardless of whether the user is focused on it
pub fn open_command_pane_floating_near_plugin(
    command_to_run: CommandToRun,
    coordinates: Option<FloatingPaneCoordinates>,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command =
        PluginCommand::OpenCommandPaneFloatingNearPlugin(command_to_run, coordinates, context);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response = ProtobufOpenCommandPaneFloatingNearPluginResponse::decode(
        bytes_from_stdin().unwrap().as_slice(),
    )
    .unwrap();
    OpenCommandPaneFloatingNearPluginResponse::try_from(response).unwrap()
}

/// Open a new in place command pane with the specified command and args (this sort of pane allows the user to control the command, re-run it and see its exit status through the Zellij UI).
pub fn open_command_pane_in_place(
    command_to_run: CommandToRun,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command = PluginCommand::OpenCommandPaneInPlace(command_to_run, context);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufOpenCommandPaneInPlaceResponse::decode(bytes_from_stdin().unwrap().as_slice())
            .unwrap();
    OpenCommandPaneInPlaceResponse::try_from(response).unwrap()
}

/// Open a new in place command pane with the specified command and args (this sort of pane allows the user to control the command, re-run it and see its exit status through the Zellij UI).
/// This variant is the same as open_command_pane_in_place, except that it always replaces the
/// plugin pane rather than whichever pane the user is focused on
pub fn open_command_pane_in_place_of_plugin(
    command_to_run: CommandToRun,
    close_plugin_after_replace: bool,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command = PluginCommand::OpenCommandPaneInPlaceOfPlugin(
        command_to_run,
        close_plugin_after_replace,
        context,
    );
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response = ProtobufOpenCommandPaneInPlaceOfPluginResponse::decode(
        bytes_from_stdin().unwrap().as_slice(),
    )
    .unwrap();
    OpenCommandPaneInPlaceOfPluginResponse::try_from(response).unwrap()
}

/// Opens a command pane in place of the pane identified by `pane_id`.
/// Unlike `open_command_pane_in_place`, this targets an arbitrary pane by ID rather than the
/// focused pane, and does not change focus. If `close_replaced_pane` is false, the replaced
/// pane is suppressed and restored when the new pane closes; if true, it is permanently closed.
/// Returns the `PaneId` of the newly opened pane, or `None` if the operation failed.
pub fn open_command_pane_in_place_of_pane_id(
    pane_id: PaneId,
    command_to_run: CommandToRun,
    close_replaced_pane: bool,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command = PluginCommand::OpenCommandPaneInPlaceOfPaneId(
        pane_id,
        command_to_run,
        close_replaced_pane,
        context,
    );
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response = ProtobufOpenCommandPaneInPlaceOfPaneIdResponse::decode(
        bytes_from_stdin().unwrap().as_slice(),
    )
    .unwrap();
    OpenCommandPaneInPlaceOfPaneIdResponse::try_from(response).unwrap()
}

/// Opens a terminal pane in place of the pane identified by `pane_id`.
/// Unlike `open_terminal_in_place`, this targets an arbitrary pane by ID rather than the
/// focused pane, and does not change focus. If `close_replaced_pane` is false, the replaced
/// pane is suppressed and restored when the new pane closes; if true, it is permanently closed.
/// `cwd` sets the working directory for the new terminal. Returns the `PaneId` of the newly
/// opened pane, or `None` if the operation failed.
pub fn open_terminal_pane_in_place_of_pane_id<P: AsRef<Path>>(
    pane_id: PaneId,
    cwd: P,
    close_replaced_pane: bool,
) -> Option<PaneId> {
    let file_to_open = FileToOpen {
        path: cwd.as_ref().to_path_buf(),
        ..Default::default()
    };
    let plugin_command =
        PluginCommand::OpenTerminalPaneInPlaceOfPaneId(pane_id, file_to_open, close_replaced_pane);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response = ProtobufOpenTerminalPaneInPlaceOfPaneIdResponse::decode(
        bytes_from_stdin().unwrap().as_slice(),
    )
    .unwrap();
    OpenTerminalPaneInPlaceOfPaneIdResponse::try_from(response).unwrap()
}

/// Opens an editor pane in place of the pane identified by `pane_id`.
/// Unlike `open_file_in_place`, this targets an arbitrary pane by ID rather than the
/// focused pane, and does not change focus. If `close_replaced_pane` is false, the replaced
/// pane is suppressed and restored when the new pane closes; if true, it is permanently closed.
/// Returns the `PaneId` of the newly opened pane, or `None` if the operation failed.
pub fn open_edit_pane_in_place_of_pane_id(
    pane_id: PaneId,
    file_to_open: FileToOpen,
    close_replaced_pane: bool,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command = PluginCommand::OpenEditPaneInPlaceOfPaneId(
        pane_id,
        file_to_open,
        close_replaced_pane,
        context,
    );
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufOpenEditPaneInPlaceOfPaneIdResponse::decode(bytes_from_stdin().unwrap().as_slice())
            .unwrap();
    OpenEditPaneInPlaceOfPaneIdResponse::try_from(response).unwrap()
}

/// Open a new hidden (background) command pane with the specified command and args (this sort of pane allows the user to control the command, re-run it and see its exit status through the Zellij UI).
pub fn open_command_pane_background(
    command_to_run: CommandToRun,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command = PluginCommand::OpenCommandPaneBackground(command_to_run, context);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufOpenCommandPaneBackgroundResponse::decode(bytes_from_stdin().unwrap().as_slice())
            .unwrap();
    OpenCommandPaneBackgroundResponse::try_from(response).unwrap()
}

/// Change the focused tab to the specified index (corresponding with the default tab names, to starting at `1`, `0` will be considered as `1`).
pub fn switch_tab_to(tab_idx: u32) {
    let plugin_command = PluginCommand::SwitchTabTo(tab_idx);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Set a timeout in seconds (or fractions thereof) after which the plugins [update](./plugin-api-events#update) method will be called with the [`Timer`](./plugin-api-events.md#timer) event.
pub fn set_timeout(secs: f64) {
    let plugin_command = PluginCommand::SetTimeout(secs);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

#[doc(hidden)]
pub fn exec_cmd(cmd: &[&str]) {
    let plugin_command =
        PluginCommand::ExecCmd(cmd.iter().cloned().map(|s| s.to_owned()).collect());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Run this command in the background on the host machine, optionally being notified of its output
/// if subscribed to the `RunCommandResult` Event
pub fn run_command(cmd: &[&str], context: BTreeMap<String, String>) {
    let plugin_command = PluginCommand::RunCommand(
        cmd.iter().cloned().map(|s| s.to_owned()).collect(),
        BTreeMap::new(),
        PathBuf::from("."),
        context,
        None,
    );
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Run this command in the background on the host machine, providing environment variables and a
/// cwd. Optionally being notified of its output if subscribed to the `RunCommandResult` Event
pub fn run_command_with_env_variables_and_cwd(
    cmd: &[&str],
    env_variables: BTreeMap<String, String>,
    cwd: PathBuf,
    context: BTreeMap<String, String>,
) {
    let plugin_command = PluginCommand::RunCommand(
        cmd.iter().cloned().map(|s| s.to_owned()).collect(),
        env_variables,
        cwd,
        context,
        None,
    );
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Run this command in the background on the host machine while retaining at most
/// `max_output_bytes` from each output stream.
pub fn run_command_with_output_limit(
    cmd: &[&str],
    env_variables: BTreeMap<String, String>,
    cwd: PathBuf,
    context: BTreeMap<String, String>,
    max_output_bytes: usize,
) {
    let plugin_command = PluginCommand::RunCommand(
        cmd.iter().cloned().map(|s| s.to_owned()).collect(),
        env_variables,
        cwd,
        context,
        Some(max_output_bytes),
    );
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Make a web request, optionally being notified of its output
/// if subscribed to the `WebRequestResult` Event, the context will be returned verbatim in this
/// event and can be used for eg. marking the request_id
pub fn web_request<S>(
    url: S,
    verb: HttpVerb,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
    context: BTreeMap<String, String>,
    proxy: Option<String>,
) where
    S: AsRef<str> + ToString,
{
    let plugin_command =
        PluginCommand::WebRequest(url.to_string(), verb, headers, body, context, None, proxy);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Make a web request whose response body is streamed back in chunks rather than buffered,
/// eg. for a `text/event-stream` (SSE) response from an LLM endpoint.
///
/// Subscribe to both `WebRequestResult` and `WebRequestStreamChunk`. Every stream begins with a
/// `WebRequestResult` and ends with exactly one `WebRequestStreamChunk` carrying `done: true` -
/// on clean end of stream, on network error and on cancellation alike, with `error` set in the
/// latter two cases. In between:
///
/// - on a 2xx status: the `WebRequestResult` carries the status and headers with an **empty**
///   body, and the body arrives as `WebRequestStreamChunk` events with `done: false`
/// - on a non-2xx status: the `WebRequestResult` carries the full body exactly as
///   [`web_request`] would deliver it, and there are no data chunks (so an error JSON from the
///   endpoint arrives whole rather than as stream frames)
///
/// `stream_id` correlates the chunks and is the handle for [`cancel_web_request`]. It is scoped
/// to this plugin, so it need only be unique among this plugin's in-flight streams. Parsing the
/// stream framing (SSE, NDJSON, ...) is up to the plugin.
pub fn web_request_stream<I: ToString, S: ToString>(
    stream_id: I,
    url: S,
    verb: HttpVerb,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
    context: BTreeMap<String, String>,
    proxy: Option<String>,
) {
    let plugin_command = PluginCommand::WebRequest(
        url.to_string(),
        verb,
        headers,
        body,
        context,
        Some(stream_id.to_string()),
        proxy,
    );
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Cancel an in-flight [`web_request_stream`]. A final `WebRequestStreamChunk` with `done: true`
/// and `error: Some(_)` is still delivered. Cancelling an unknown or already finished stream is
/// a no-op.
pub fn cancel_web_request<S: ToString>(stream_id: S) {
    let plugin_command = PluginCommand::CancelWebRequest(stream_id.to_string());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Hide the plugin pane (suppress it) from the UI
pub fn hide_self() {
    let plugin_command = PluginCommand::HideSelf;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Hide the pane (suppress it) with the specified [PaneId] from the UI
pub fn hide_pane_with_id(pane_id: PaneId) {
    let plugin_command = PluginCommand::HidePaneWithId(pane_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Show the plugin pane (unsuppress it if it is suppressed), focus it and switch to its tab
pub fn show_self(should_float_if_hidden: bool) {
    let plugin_command = PluginCommand::ShowSelf(should_float_if_hidden);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Show the pane (unsuppress it if it is suppressed) with the specified [PaneId], focus it and switch to its tab
pub fn show_pane_with_id(pane_id: PaneId, should_float_if_hidden: bool, should_focus_pane: bool) {
    let plugin_command =
        PluginCommand::ShowPaneWithId(pane_id, should_float_if_hidden, should_focus_pane);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Close this plugin pane
pub fn close_self() {
    let plugin_command = PluginCommand::CloseSelf;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Switch to the specified Input Mode (eg. `Normal`, `Tab`, `Pane`)
pub fn switch_to_input_mode(mode: &InputMode) {
    let plugin_command = PluginCommand::SwitchToMode(*mode);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Provide a stringified [`layout`](https://zellij.dev/documentation/layouts.html) to be applied to the current session. If the layout has multiple tabs, they will all be opened.
pub fn new_tabs_with_layout(layout: &str) -> Vec<usize> {
    let plugin_command = PluginCommand::NewTabsWithLayout(layout.to_owned());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response = ProtobufNewTabsResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    NewTabsResponse::try_from(response).unwrap()
}

/// Provide a LayoutInfo to be applied to the current session in a new tab. If the layout has multiple tabs, they will all be opened.
pub fn new_tabs_with_layout_info<L: AsRef<LayoutInfo>>(layout_info: L) -> Vec<usize> {
    let plugin_command = PluginCommand::NewTabsWithLayoutInfo(layout_info.as_ref().clone());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response = ProtobufNewTabsResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    NewTabsResponse::try_from(response).unwrap()
}

/// Open a new tab with the default layout
pub fn new_tab<S>(name: Option<S>, cwd: Option<S>) -> Option<usize>
where
    S: AsRef<str> + ToString,
{
    let name = name.map(|s| s.to_string());
    let cwd = cwd.map(|s| s.to_string());
    let plugin_command = PluginCommand::NewTab { name, cwd };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response = ProtobufNewTabResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    NewTabResponse::try_from(response).unwrap()
}

pub fn new_tab_unfocused<S>(name: Option<S>, cwd: Option<S>) -> Option<usize>
where
    S: AsRef<str> + ToString,
{
    let name = name.map(|s| s.to_string());
    let cwd = cwd.map(|s| s.to_string());
    let plugin_command = PluginCommand::NewTabUnfocused { name, cwd };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufNewTabUnfocusedResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    NewTabUnfocusedResponse::try_from(response).unwrap()
}

pub fn new_tiled_pane_in_tab(tab_position: usize) -> Option<PaneId> {
    let plugin_command = PluginCommand::NewTiledPaneInTab { tab_position };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufNewTiledPaneInTabResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    NewTiledPaneInTabResponse::try_from(response).unwrap()
}

/// Opens a new tab with a command pane running `command_to_run`.
/// Returns `(tab_id, pane_id)` of the created tab and pane, or `None` if unavailable.
pub fn open_command_pane_in_new_tab(
    command_to_run: CommandToRun,
    context: BTreeMap<String, String>,
) -> (Option<usize>, Option<PaneId>) {
    let plugin_command = PluginCommand::OpenCommandPaneInNewTab(command_to_run, context);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufOpenPaneInNewTabResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    let result = OpenPaneInNewTabResponse::try_from(response).unwrap();
    (result.tab_id, result.pane_id)
}

/// Opens a new tab with a plugin pane loaded from `plugin_url`.
/// `plugin_url` can be a path (`file:/path/to/plugin.wasm`) or a named alias.
/// Returns `(tab_id, pane_id)` of the created tab and pane.
pub fn open_plugin_pane_in_new_tab(
    plugin_url: impl ToString,
    configuration: BTreeMap<String, String>,
    context: BTreeMap<String, String>,
) -> (Option<usize>, Option<PaneId>) {
    let plugin_command = PluginCommand::OpenPluginPaneInNewTab {
        plugin_url: plugin_url.to_string(),
        configuration,
        context,
    };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufOpenPaneInNewTabResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    let result = OpenPaneInNewTabResponse::try_from(response).unwrap();
    (result.tab_id, result.pane_id)
}

/// Open a new floating plugin pane with the specified plugin URL and configuration.
/// Returns the pane ID of the newly created plugin pane, if successful.
pub fn open_plugin_pane_floating(
    plugin_url: &str,
    configuration: BTreeMap<String, String>,
    coordinates: Option<FloatingPaneCoordinates>,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command = PluginCommand::OpenPluginPaneFloating {
        plugin_url: plugin_url.to_owned(),
        configuration,
        floating_pane_coordinates: coordinates,
        context,
    };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufOpenPluginPaneFloatingResponse::decode(bytes_from_stdin().unwrap().as_slice())
            .unwrap();
    OpenPluginPaneFloatingResponse::try_from(response).unwrap()
}

/// Opens a new tab with an editor pane for `file_to_open`.
/// Returns `(tab_id, pane_id)` of the created tab and pane.
pub fn open_editor_pane_in_new_tab(
    file_to_open: FileToOpen,
    context: BTreeMap<String, String>,
) -> (Option<usize>, Option<PaneId>) {
    let plugin_command = PluginCommand::OpenEditorPaneInNewTab(file_to_open, context);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufOpenPaneInNewTabResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    let result = OpenPaneInNewTabResponse::try_from(response).unwrap();
    (result.tab_id, result.pane_id)
}

/// Change focus to the next tab or loop back to the first
pub fn go_to_next_tab() {
    let plugin_command = PluginCommand::GoToNextTab;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Change focus to the previous tab or loop back to the last
pub fn go_to_previous_tab() {
    let plugin_command = PluginCommand::GoToPreviousTab;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn report_panic(info: &std::panic::PanicHookInfo) {
    let panic_payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
        s.to_string()
    } else {
        "<NO PAYLOAD>".to_string()
    };
    let panic_stringified = format!("{}\n\r{:#?}", panic_payload, info).replace("\n", "\r\n");
    let plugin_command = PluginCommand::ReportPanic(panic_stringified);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Either Increase or Decrease the size of the focused pane
pub fn resize_focused_pane(resize: Resize) {
    let plugin_command = PluginCommand::Resize(resize);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Either Increase or Decrease the size of the focused pane in a specified direction (eg. `Left`, `Right`, `Up`, `Down`).
pub fn resize_focused_pane_with_direction(resize: Resize, direction: Direction) {
    let resize_strategy = ResizeStrategy {
        resize,
        direction: Some(direction),
        invert_on_boundaries: false,
    };
    let plugin_command = PluginCommand::ResizeWithDirection(resize_strategy);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Change focus tot he next pane in chronological order
pub fn focus_next_pane() {
    let plugin_command = PluginCommand::FocusNextPane;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Change focus to the previous pane in chronological order
pub fn focus_previous_pane() {
    let plugin_command = PluginCommand::FocusPreviousPane;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Change focus to the previously focused pane
pub fn focus_last_pane() {
    let plugin_command = PluginCommand::FocusLastPane;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Change the focused pane in the specified direction
pub fn move_focus(direction: Direction) {
    let plugin_command = PluginCommand::MoveFocus(direction);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Change the focused pane in the specified direction, if the pane is on the edge of the screen, the next tab is focused (next if right edge, previous if left edge).
pub fn move_focus_or_tab(direction: Direction) {
    let plugin_command = PluginCommand::MoveFocusOrTab(direction);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Detach the user from the active session
pub fn detach() {
    let plugin_command = PluginCommand::Detach;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Edit the scrollback of the focused pane in the user's default `$EDITOR`
pub fn edit_scrollback() {
    let plugin_command = PluginCommand::EditScrollback;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Write bytes to the `STDIN` of the focused pane
pub fn write(bytes: Vec<u8>) {
    let plugin_command = PluginCommand::Write(bytes);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Write characters to the `STDIN` of the focused pane
pub fn write_chars(chars: &str) {
    let plugin_command = PluginCommand::WriteChars(chars.to_owned());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Copy arbitrary text to the user's clipboard
///
/// Respects the user's configured clipboard destination (system clipboard or primary selection).
/// Requires the WriteToClipboard permission.
pub fn copy_to_clipboard(text: impl Into<String>) {
    let plugin_command = PluginCommand::CopyToClipboard(text.into());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Focused the previously focused tab (regardless of the tab position)
pub fn toggle_tab() {
    let plugin_command = PluginCommand::ToggleTab;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Switch the position of the focused pane with a different pane
pub fn move_pane() {
    let plugin_command = PluginCommand::MovePane;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Switch the position of the focused pane with a different pane in the specified direction (eg. `Down`, `Up`, `Left`, `Right`).
pub fn move_pane_with_direction(direction: Direction) {
    let plugin_command = PluginCommand::MovePaneWithDirection(direction);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Clear the scroll buffer of the focused pane
pub fn clear_screen() {
    let plugin_command = PluginCommand::ClearScreen;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Scroll the focused pane up 1 line
pub fn scroll_up() {
    let plugin_command = PluginCommand::ScrollUp;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Scroll the focused pane down 1 line
pub fn scroll_down() {
    let plugin_command = PluginCommand::ScrollDown;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Scroll the focused pane all the way to the top of the scrollbuffer
pub fn scroll_to_top() {
    let plugin_command = PluginCommand::ScrollToTop;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Scroll the focused pane all the way to the bottom of the scrollbuffer
pub fn scroll_to_bottom() {
    let plugin_command = PluginCommand::ScrollToBottom;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Scroll the focused pane up one page
pub fn page_scroll_up() {
    let plugin_command = PluginCommand::PageScrollUp;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Scroll the focused pane down one page
pub fn page_scroll_down() {
    let plugin_command = PluginCommand::PageScrollDown;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Toggle the focused pane to be fullscreen or normal sized
pub fn toggle_focus_fullscreen() {
    let plugin_command = PluginCommand::ToggleFocusFullscreen;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn toggle_focus_no_ui_fullscreen() {
    let plugin_command = PluginCommand::ToggleFocusNoUiFullscreen;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn focus_host_session() {
    let plugin_command = PluginCommand::FocusHostSession;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Toggle the UI pane frames on or off
pub fn toggle_pane_frames() {
    let plugin_command = PluginCommand::TogglePaneFrames;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn set_pane_frame_style(pane_frame_style: PaneFrameStyle) {
    let plugin_command = PluginCommand::SetPaneFrameStyle(pane_frame_style);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Embed the currently focused pane (make it stop floating) or turn it to a float pane if it is not
pub fn toggle_pane_embed_or_eject() {
    let plugin_command = PluginCommand::TogglePaneEmbedOrEject;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn undo_rename_pane() {
    let plugin_command = PluginCommand::UndoRenamePane;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Close the focused pane
pub fn close_focus() {
    let plugin_command = PluginCommand::CloseFocus;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn new_pane() {
    let plugin_command = PluginCommand::NewPane;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn toggle_floating_panes(tab_id: Option<u64>) {
    let plugin_command = PluginCommand::ToggleFloatingPanes { tab_id };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Turn the `STDIN` synchronization of the current tab on or off
pub fn toggle_active_tab_sync() {
    let plugin_command = PluginCommand::ToggleActiveTabSync;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Close the focused tab
pub fn close_focused_tab() {
    let plugin_command = PluginCommand::CloseFocusedTab;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn undo_rename_tab() {
    let plugin_command = PluginCommand::UndoRenameTab;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Compeltely quit Zellij for this and all other connected clients
pub fn quit_zellij() {
    let plugin_command = PluginCommand::QuitZellij;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Change to the previous [swap layout](https://zellij.dev/documentation/swap-layouts.html)
pub fn previous_swap_layout() {
    let plugin_command = PluginCommand::PreviousSwapLayout;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Change to the next [swap layout](https://zellij.dev/documentation/swap-layouts.html)
pub fn next_swap_layout() {
    let plugin_command = PluginCommand::NextSwapLayout;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Change focus to the tab with the specified name
pub fn go_to_tab_name(tab_name: &str) {
    let plugin_command = PluginCommand::GoToTabName(tab_name.to_owned());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Change focus to the tab with the specified name or create it if it does not exist
pub fn focus_or_create_tab(tab_name: &str) -> Option<usize> {
    let plugin_command = PluginCommand::FocusOrCreateTab(tab_name.to_owned());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufFocusOrCreateTabResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    FocusOrCreateTabResponse::try_from(response).unwrap()
}

pub fn go_to_tab(tab_index: u32) {
    let plugin_command = PluginCommand::GoToTab(tab_index);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Grant `permissions` to the plugin at `plugin_url` before it asks, as if the user had answered
/// its prompt. Only a builtin plugin may call this; for anything else the host refuses and logs.
/// The grant is remembered in the permissions cache and applied to any running instance.
pub fn grant_plugin_permissions(plugin_url: &str, permissions: &[PermissionType]) {
    let plugin_command = PluginCommand::GrantPluginPermissions {
        plugin_url: plugin_url.to_owned(),
        permissions: permissions.to_vec(),
    };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn start_or_reload_plugin(url: &str) {
    let plugin_command = PluginCommand::StartOrReloadPlugin(url.to_owned());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Closes a terminal pane with the specified id
pub fn close_terminal_pane(terminal_pane_id: u32) {
    let plugin_command = PluginCommand::CloseTerminalPane(terminal_pane_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Closes a plugin pane with the specified id
pub fn close_plugin_pane(plugin_pane_id: u32) {
    let plugin_command = PluginCommand::ClosePluginPane(plugin_pane_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Changes the focus to the terminal pane with the specified id, unsuppressing it if it was suppressed and switching to its tab and layer (eg. floating/tiled).
pub fn focus_terminal_pane(
    terminal_pane_id: u32,
    should_float_if_hidden: bool,
    should_be_in_place_if_hidden: bool,
) {
    let plugin_command = PluginCommand::FocusTerminalPane(
        terminal_pane_id,
        should_float_if_hidden,
        should_be_in_place_if_hidden,
    );
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Changes the focus to the plugin pane with the specified id, unsuppressing it if it was suppressed and switching to its tab and layer (eg. floating/tiled).
pub fn focus_plugin_pane(
    plugin_pane_id: u32,
    should_float_if_hidden: bool,
    should_be_in_place_if_hidden: bool,
) {
    let plugin_command = PluginCommand::FocusPluginPane(
        plugin_pane_id,
        should_float_if_hidden,
        should_be_in_place_if_hidden,
    );
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Changes the name (the title that appears in the UI) of the terminal pane with the specified id.
pub fn rename_terminal_pane<S>(terminal_pane_id: u32, new_name: S)
where
    S: AsRef<str> + ToString,
{
    let plugin_command = PluginCommand::RenameTerminalPane(terminal_pane_id, new_name.to_string());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Changes the name (the title that appears in the UI) of the plugin pane with the specified id.
pub fn rename_plugin_pane<S>(plugin_pane_id: u32, new_name: S)
where
    S: AsRef<str> + ToString,
{
    let plugin_command = PluginCommand::RenamePluginPane(plugin_pane_id, new_name.to_string());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Changes the name (the title that appears in the UI) of the tab with the specified position.
///
/// `tab_position` is the 0-based display position, matching [`go_to_tab`]. A position
/// shifts whenever a tab closes; use [`rename_tab_with_id`] to name a tab that stays
/// the same one.
pub fn rename_tab<S>(tab_position: u32, new_name: S)
where
    S: AsRef<str> + ToString,
{
    let plugin_command = PluginCommand::RenameTab(tab_position, new_name.to_string());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Changes the name (the title that appears in the UI) of the tab with the specified id.
pub fn rename_tab_with_id<S>(tab_id: u64, new_name: S)
where
    S: AsRef<str> + ToString,
{
    let plugin_command = PluginCommand::RenameTabWithId(tab_id, new_name.to_string());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Tells the host which project each tab is working on, keyed by tab id.
///
/// Not a rename: the name is shown in place of the tab's default "Tab #N", and a
/// tab the user named keeps that name. Two tabs on the same project are told
/// apart with a `#1`/`#2` suffix. The map is the whole truth each time it is
/// sent, so a tab left out of it goes back to its own name.
pub fn set_tab_projects(tab_projects: BTreeMap<u64, String>) {
    let plugin_command = PluginCommand::SetTabProjects(tab_projects);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Switch to a session with the given name, create one if no name is given
pub fn switch_session(name: Option<&str>) {
    let plugin_command = PluginCommand::SwitchSession(ConnectToSession {
        name: name.map(|n| n.to_string()),
        ..Default::default()
    });
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Switch to a session with the given name, create one if no name is given
pub fn switch_session_with_layout(name: Option<&str>, layout: LayoutInfo, cwd: Option<PathBuf>) {
    let plugin_command = PluginCommand::SwitchSession(ConnectToSession {
        name: name.map(|n| n.to_string()),
        layout: Some(layout),
        cwd,
        ..Default::default()
    });
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Switch to a session with the given name, create one if no name is given
pub fn switch_session_with_cwd(name: Option<&str>, cwd: Option<PathBuf>) {
    let plugin_command = PluginCommand::SwitchSession(ConnectToSession {
        name: name.map(|n| n.to_string()),
        cwd,
        ..Default::default()
    });
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Switch to a session with the given name, focusing either the provided pane_id or the provided
/// tab position (in that order)
pub fn switch_session_with_focus(
    name: &str,
    tab_position: Option<usize>,
    pane_id: Option<(u32, bool)>,
) {
    let plugin_command = PluginCommand::SwitchSession(ConnectToSession {
        name: Some(name.to_owned()),
        tab_position,
        pane_id,
        ..Default::default()
    });
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Permanently delete a resurrectable session with the given name.
///
/// Returns `Ok(())` if the session's cache directory was removed; `Err(...)`
/// if the underlying `remove_dir_all` failed (e.g. permissions, missing path).
pub fn delete_dead_session(name: &str) -> Result<(), String> {
    let plugin_command = PluginCommand::DeleteDeadSessionAndReply(name.to_owned());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response_bytes = bytes_from_stdin()
        .map_err(|e| format!("Failed to read DeleteDeadSession response: {}", e))?;
    let protobuf_response = ProtobufDeleteDeadSessionResponse::decode(response_bytes.as_slice())
        .map_err(|e| format!("Malformed DeleteDeadSession response: {}", e))?;
    match protobuf_response.error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Permanently delete all resurrectable sessions on this machine.
///
/// Bounded by a host-side wedge timeout (matching the kill-all budget) so a
/// pathological filesystem cannot freeze the calling plugin.
pub fn delete_all_dead_sessions() -> Result<(), String> {
    let plugin_command = PluginCommand::DeleteAllDeadSessionsAndReply;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response_bytes = bytes_from_stdin()
        .map_err(|e| format!("Failed to read DeleteAllDeadSessions response: {}", e))?;
    let protobuf_response =
        ProtobufDeleteAllDeadSessionsResponse::decode(response_bytes.as_slice())
            .map_err(|e| format!("Malformed DeleteAllDeadSessions response: {}", e))?;
    match protobuf_response.error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Rename the current session
pub fn rename_session(name: &str) {
    let plugin_command = PluginCommand::RenameSession(name.to_owned());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Show a notification that dismisses itself after a timeout. Its corner and timeout come from the
/// `notification_position` and `notification_timeout` config options.
pub fn notify(text: &str) {
    let plugin_command = PluginCommand::Notify(text.to_owned(), None);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Show a notification that focuses `pane_id` when clicked.
pub fn notify_with_pane_id(text: &str, pane_id: PaneId) {
    let plugin_command = PluginCommand::Notify(text.to_owned(), Some(pane_id));
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Unblock the input side of a pipe, requesting the next message be sent if there is one
pub fn unblock_cli_pipe_input(pipe_name: &str) {
    let plugin_command = PluginCommand::UnblockCliPipeInput(pipe_name.to_owned());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Block the input side of a pipe, will only be released once this or another plugin unblocks it
pub fn block_cli_pipe_input(pipe_name: &str) {
    let plugin_command = PluginCommand::BlockCliPipeInput(pipe_name.to_owned());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Send output to the output side of a pipe, ths does not affect the input side of same pipe
pub fn cli_pipe_output(pipe_name: &str, output: &str) {
    let plugin_command = PluginCommand::CliPipeOutput(pipe_name.to_owned(), output.to_owned());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Send a message to a plugin, it will be launched if it is not already running
pub fn pipe_message_to_plugin(message_to_plugin: MessageToPlugin) {
    let plugin_command = PluginCommand::MessageToPlugin(message_to_plugin);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Disconnect all other clients from the current session
pub fn disconnect_other_clients() {
    let plugin_command = PluginCommand::DisconnectOtherClients;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Kill all Zellij sessions in the list.
///
/// Waits for each peer session to acknowledge the kill (over its own IPC
/// socket -- the server-side handler reads back the `Exit` message the peer
/// sends as part of its shutdown, or a stream-close if the peer dies first).
/// Returns once every kill has been confirmed or one has failed. A wedged
/// peer is bounded by a short server-side wedge timeout.
pub fn kill_sessions<S>(session_names: &[S]) -> Result<(), String>
where
    S: AsRef<str> + ToString,
{
    let plugin_command =
        PluginCommand::KillSessionsAndReply(session_names.iter().map(|s| s.to_string()).collect());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response_bytes =
        bytes_from_stdin().map_err(|e| format!("Failed to read KillSessions response: {}", e))?;
    let protobuf_response = ProtobufKillSessionsResponse::decode(response_bytes.as_slice())
        .map_err(|e| format!("Malformed KillSessions response: {}", e))?;
    match protobuf_response.error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Move a pane, and the program running in it, into another session.
///
/// The process is not restarted: same pid, same working directory, same open
/// files. The pane's scrollback travels with it.
///
/// Blocks until the pane has landed in the other session, or until the move has
/// failed and been rolled back. That can take seconds — a supervisor is released
/// here and re-attached there — and the plugin's thread is held for the
/// duration, which at executor capacity can also delay a co-pinned plugin. An
/// unreachable target session is refused before anything is released, so the
/// common mistake is cheap.
///
/// `Ok` carries a line to show the user; `Err` carries the reason, which in the
/// worst case is also the only place the recovery instructions exist.
///
/// # Permissions Required
/// * `ChangeApplicationState`
pub fn warp_pane_to_session(
    pane_id: PaneId,
    to_session: &str,
    placement: WarpPlacement,
) -> Result<String, String> {
    warp_request(PluginCommand::WarpPaneToSession {
        pane_id,
        to_session: to_session.to_owned(),
        placement,
    })
}

/// Bring a pane from another session into this one.
///
/// The mirror of [`warp_pane_to_session`]: the source session does the work, so
/// `pane_id` is an id in *that* session's numbering, not this one's.
///
/// # Permissions Required
/// * `ChangeApplicationState`
pub fn warp_pane_from_session(
    pane_id: PaneId,
    from_session: &str,
    placement: WarpPlacement,
) -> Result<String, String> {
    warp_request(PluginCommand::WarpPaneFromSession {
        pane_id,
        from_session: from_session.to_owned(),
        placement,
    })
}

/// Move a whole tab, and everything running in it, into another session.
///
/// The tab arrives as a tab, arranged the way it was here. None of its processes
/// are restarted. If the other session's terminal is a different size, the
/// arrangement is scaled to fit rather than clipped, because what travels is a
/// layout rather than a set of pane sizes.
///
/// Plugin panes are the exception to "nothing is restarted": they have no
/// process to hand over, so the receiving session starts them afresh in the same
/// slots. A plugin's own state does not survive the trip.
///
/// Blocks on the same terms as [`warp_pane_to_session`], and for longer — a tab's
/// worth of supervisors have to be released here and re-attached there.
///
/// Despite the name, `tab_index` is the tab's **stable id**, not its display
/// position: `Screen` keys its tabs by id and looks this value up there directly.
/// Passing a position warps whichever tab happens to own that number as an id,
/// which stops being the same tab as soon as the session closes one.
///
/// # Permissions Required
/// * `ChangeApplicationState`
pub fn warp_tab_to_session(tab_index: usize, to_session: &str) -> Result<String, String> {
    warp_request(PluginCommand::WarpTabToSession {
        tab_index,
        to_session: to_session.to_owned(),
    })
}

/// Bring a tab from another session into this one.
///
/// The mirror of [`warp_tab_to_session`]: the source session does the work, so
/// `tab_index` is a stable tab id in *that* session's numbering, not this one's,
/// and not a display position in either.
///
/// # Permissions Required
/// * `ChangeApplicationState`
pub fn warp_tab_from_session(tab_index: usize, from_session: &str) -> Result<String, String> {
    warp_request(PluginCommand::WarpTabFromSession {
        tab_index,
        from_session: from_session.to_owned(),
    })
}

/// Take an orphaned pane over into this session, reconnecting it to its
/// still-running process. `host_id` comes from [`list_orphaned_panes`].
///
/// # Permissions Required
/// * `ChangeApplicationState`
pub fn adopt_orphaned_pane(host_id: &str, placement: WarpPlacement) -> Result<String, String> {
    warp_request(PluginCommand::AdoptOrphanedPane {
        host_id: host_id.to_owned(),
        placement,
    })
}

fn warp_request(plugin_command: PluginCommand) -> Result<String, String> {
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response_bytes =
        bytes_from_stdin().map_err(|e| format!("Failed to read warp response: {}", e))?;
    let protobuf_response = ProtobufWarpPaneResponse::decode(response_bytes.as_slice())
        .map_err(|e| format!("Malformed warp response: {}", e))?;
    match WarpPaneResponse::try_from(protobuf_response) {
        Ok(WarpPaneResponse::Ok(message)) => Ok(message),
        Ok(WarpPaneResponse::Err(error)) => Err(error),
        Err(e) => Err(format!("Malformed warp response: {}", e)),
    }
}

/// Panes whose session died but whose process is still running, waiting to be
/// recovered with [`adopt_orphaned_pane`].
///
/// Empty unless a zellij server was killed rather than shut down. Panes that are
/// merely in another *live* session are not listed here — those move with
/// [`warp_pane_from_session`].
///
/// # Permissions Required
/// * `ReadApplicationState`
pub fn list_orphaned_panes() -> Result<Vec<OrphanedPane>, String> {
    let plugin_command = PluginCommand::ListOrphanedPanes;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response_bytes = bytes_from_stdin()
        .map_err(|e| format!("Failed to read ListOrphanedPanes response: {}", e))?;
    let protobuf_response = ProtobufListOrphanedPanesResponse::decode(response_bytes.as_slice())
        .map_err(|e| format!("Malformed ListOrphanedPanes response: {}", e))?;
    match ListOrphanedPanesResponse::try_from(protobuf_response) {
        Ok(ListOrphanedPanesResponse::Ok(panes)) => Ok(panes),
        Ok(ListOrphanedPanesResponse::Err(error)) => Err(error),
        Err(e) => Err(format!("Malformed ListOrphanedPanes response: {}", e)),
    }
}

// ------------------------------------------------------------------ data store

/// Issue a data store command and decode its response.
///
/// Every one of these is a blocking round trip: the request goes out on stdout, the host handles it
/// on this plugin's own executor thread, and the response is waiting on stdin by the time
/// `host_run_plugin_command` returns.
fn data_request<T, P>(plugin_command: PluginCommand, what: &str) -> Result<T, String>
where
    P: Message + Default,
    T: TryFrom<P, Error = &'static str>,
{
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command
        .try_into()
        .map_err(|e| format!("Failed to encode the {} request: {}", what, e))?;
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    // A denied permission is a host-side log and no response at all, so this read comes back empty
    // rather than hanging — which is why these return `Result` instead of unwrapping.
    let response_bytes =
        bytes_from_stdin().map_err(|e| format!("Failed to read the {} response: {}", what, e))?;
    let protobuf_response = P::decode(response_bytes.as_slice())
        .map_err(|e| format!("Malformed {} response: {}", what, e))?;
    T::try_from(protobuf_response).map_err(|e| format!("Malformed {} response: {}", what, e))
}

/// Declare an entity in the data store, creating or migrating its storage to match.
///
/// Idempotent: re-declaring an identical definition does nothing. Adding an attribute, or marking
/// one indexed or full-text, migrates in place and keeps the rows already stored. Removing an
/// attribute or changing its type is refused unless `allow_destructive` is set *and* the
/// definition's version is above the stored one.
///
/// Pass `None` for `namespace` to use the plugin's own private one, which is the usual case. A
/// `shared:`-prefixed namespace is shared with other plugins and needs an extra permission.
///
/// ```no_run
/// use zellij_tile::prelude::*;
///
/// fn main() -> Result<(), String> {
/// data_define_entity(
///     None,
///     &EntityDef::new("agent", 1)
///         .attr(AttrDef::text("project").indexed())
///         .attr(AttrDef::text("summary").full_text())
///         .attr(AttrDef::int("updated_at_ms").indexed()),
///     false,
/// )?;
/// Ok(())
/// }
/// ```
///
/// # Permissions Required
///
/// `PermissionType::PluginData`, plus `PermissionType::SharedPluginData` for a `shared:` namespace.
pub fn data_define_entity(
    namespace: Option<&str>,
    def: &EntityDef,
    allow_destructive: bool,
) -> Result<DataSchemaInfo, String> {
    // Checked here as well as in the host so that a typo is caught without a round trip, and with
    // the same message either way.
    def.validate().map_err(|e| e.to_string())?;
    let response: DataDefineResponse = data_request::<_, ProtobufDataDefineResponse>(
        PluginCommand::DataDefineEntity {
            namespace: namespace.map(str::to_owned),
            def: def.clone(),
            allow_destructive,
        },
        "define entity",
    )?;
    match response {
        DataDefineResponse::Ok(info) => Ok(info),
        DataDefineResponse::Err(error) => Err(error),
    }
}

/// Apply a batch of writes in one transaction, all or nothing.
///
/// ```no_run
/// use zellij_tile::prelude::*;
///
/// fn main() -> Result<(), String> {
/// let cutoff = 0_i64;
/// data_apply(None, vec![
///     Mutation::put("agent").uid("a1").set("project", "zellij").build(),
///     Mutation::delete("agent", Pred::lt("updated_at_ms", cutoff)),
/// ])?;
/// Ok(())
/// }
/// ```
///
/// # Permissions Required
///
/// `PermissionType::PluginData`, plus `PermissionType::SharedPluginData` for a `shared:` namespace.
pub fn data_apply(
    namespace: Option<&str>,
    mutations: Vec<Mutation>,
) -> Result<ApplyReport, String> {
    for mutation in &mutations {
        mutation.validate().map_err(|e| e.to_string())?;
    }
    let response: DataApplyResponse = data_request::<_, ProtobufDataApplyResponse>(
        PluginCommand::DataApply {
            namespace: namespace.map(str::to_owned),
            mutations,
        },
        "apply",
    )?;
    match response {
        DataApplyResponse::Ok(report) => Ok(report),
        DataApplyResponse::Err(error) => Err(error),
    }
}

/// Read rows from the data store.
///
/// The result is capped at [`MAX_FETCH_ROWS`] rows and a byte budget, whichever comes first; a
/// truncated result says so and carries a cursor to page with.
///
/// # Permissions Required
///
/// `PermissionType::PluginData`, plus `PermissionType::SharedPluginData` for a `shared:` namespace.
pub fn data_fetch(request: &FetchRequest) -> Result<DataRows, String> {
    request.validate().map_err(|e| e.to_string())?;
    let response: DataFetchResponse = data_request::<_, ProtobufDataFetchResponse>(
        PluginCommand::DataFetch {
            request: request.clone(),
        },
        "fetch",
    )?;
    match response {
        DataFetchResponse::Ok(rows) => Ok(rows),
        DataFetchResponse::Err(error) => Err(error),
    }
}

/// Count the rows a fetch would match, ignoring its limit, offset and cursor.
///
/// # Permissions Required
///
/// `PermissionType::PluginData`, plus `PermissionType::SharedPluginData` for a `shared:` namespace.
pub fn data_count(request: &FetchRequest) -> Result<u64, String> {
    request.validate().map_err(|e| e.to_string())?;
    let response: DataCountResponse = data_request::<_, ProtobufDataCountResponse>(
        PluginCommand::DataCount {
            request: request.clone(),
        },
        "count",
    )?;
    match response {
        DataCountResponse::Ok(count) => Ok(count),
        DataCountResponse::Err(error) => Err(error),
    }
}

/// Register a standing fetch, and be sent an [`Event::PluginDataChanged`] whenever its results
/// change.
///
/// The rows as they are now come back with the handle, so there is no gap between fetching and
/// watching in which a change could be missed. Subscribing to `EventType::PluginDataChanged` is
/// done for you.
///
/// Changes are reported only when the result set actually differs, so writing the same values again
/// is silent.
///
/// # Permissions Required
///
/// `PermissionType::PluginData`, plus `PermissionType::SharedPluginData` for a `shared:` namespace.
pub fn data_watch(request: &FetchRequest, mode: WatchMode) -> Result<DataWatchHandle, String> {
    request.validate().map_err(|e| e.to_string())?;
    let response: DataWatchResponse = data_request::<_, ProtobufDataWatchResponse>(
        PluginCommand::DataWatch {
            request: request.clone(),
            mode,
        },
        "watch",
    )?;
    match response {
        DataWatchResponse::Ok(handle) => Ok(handle),
        DataWatchResponse::Err(error) => Err(error),
    }
}

/// Cancel a watch registered with [`data_watch`].
///
/// Watches are dropped automatically when the plugin unloads, so this is only needed for a plugin
/// that stops caring while it is still running.
///
/// # Permissions Required
///
/// `PermissionType::PluginData`
pub fn data_unwatch(watch_id: u64) -> Result<(), String> {
    let response: DataUnwatchResponse = data_request::<_, ProtobufDataUnwatchResponse>(
        PluginCommand::DataUnwatch { watch_id },
        "unwatch",
    )?;
    match response {
        DataUnwatchResponse::Ok => Ok(()),
        DataUnwatchResponse::Err(error) => Err(error),
    }
}

/// Terminal operations on a [`FetchRequest`], so that a query reads as one expression.
///
/// An extension trait rather than inherent methods because `FetchRequest` is defined in
/// `zellij-utils`, which cannot reach the host-call shims here.
///
/// ```no_run
/// use zellij_tile::prelude::*;
///
/// fn main() -> Result<(), String> {
/// let rows = FetchRequest::entity("agent")
///     .filter(Pred::eq("project", "zellij"))
///     .search("rebase conflict")
///     .sort_by("updated_at_ms", SortDir::Desc)
///     .limit(50)
///     .fetch()?;
/// Ok(())
/// }
/// ```
pub trait FetchRequestExt {
    /// Run this fetch. See [`data_fetch`].
    fn fetch(&self) -> Result<DataRows, String>;
    /// Count what this fetch would match. See [`data_count`].
    fn count(&self) -> Result<u64, String>;
    /// Watch this fetch. See [`data_watch`].
    fn watch(&self, mode: WatchMode) -> Result<DataWatchHandle, String>;
}

impl FetchRequestExt for FetchRequest {
    fn fetch(&self) -> Result<DataRows, String> {
        data_fetch(self)
    }

    fn count(&self) -> Result<u64, String> {
        data_count(self)
    }

    fn watch(&self, mode: WatchMode) -> Result<DataWatchHandle, String> {
        data_watch(self, mode)
    }
}

/// Scan a specific folder in the host filesystem (this is a hack around some WASI runtime performance
/// issues), will not follow symlinks
pub fn scan_host_folder<S: AsRef<Path>>(folder_to_scan: &S) {
    let plugin_command = PluginCommand::ScanHostFolder(folder_to_scan.as_ref().to_path_buf());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn set_soft_keyboard(on: bool) {
    let plugin_command = PluginCommand::SetSoftKeyboard(on);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Start watching the host folder for filesystem changes (Note: somewhat unstable at the time
/// being)
pub fn watch_filesystem() {
    let plugin_command = PluginCommand::WatchFilesystem;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Get the serialized session layout in KDL format synchronously
/// note: this removes the requesting plugin from the dumped layout
pub fn dump_session_layout() -> Result<(String, Option<LayoutMetadata>), String> {
    dump_session_layout_impl(None)
}

/// Get the serialized layout for a specific tab in KDL format synchronously
/// note: this removes the requesting plugin from the dumped layout
pub fn dump_session_layout_for_tab(
    tab_index: usize,
) -> Result<(String, Option<LayoutMetadata>), String> {
    dump_session_layout_impl(Some(tab_index))
}

fn dump_session_layout_impl(
    tab_index: Option<usize>,
) -> Result<(String, Option<LayoutMetadata>), String> {
    let plugin_command = PluginCommand::DumpSessionLayout { tab_index };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());

    unsafe { host_run_plugin_command() };

    let response_bytes =
        bytes_from_stdin().map_err(|e| format!("Failed to read response from stdin: {:?}", e))?;
    let protobuf_response = ProtobufDumpSessionLayoutResponse::decode(response_bytes.as_slice())
        .map_err(|e| format!("Failed to decode protobuf response: {}", e))?;

    // Extract metadata if present
    let metadata = protobuf_response
        .metadata
        .and_then(|pb_metadata| pb_metadata.try_into().ok());

    match protobuf_response.result {
        Some(dump_session_layout_response::Result::LayoutContent(content)) => {
            Ok((content, metadata))
        },
        Some(dump_session_layout_response::Result::Error(error)) => Err(error),
        None => Err("Server returned empty response".to_string()),
    }
}

/// Compiles `source` as a Lua plugin without loading it. `Err` carries the compile error, which
/// names `name` the way a load error would (`keys.lua:3: ...`). A plugin that compiles can still
/// fail at load, so this is the check to run *before* a write, not a promise about the reload.
pub fn check_lua_plugin(name: &str, source: &str) -> Result<(), String> {
    let plugin_command = PluginCommand::CheckLuaPlugin {
        name: name.to_string(),
        source: source.to_string(),
    };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
    let response_bytes = bytes_from_stdin().map_err(|e| e.to_string())?;
    let response = ProtobufCheckLuaPluginResponse::decode(response_bytes.as_slice())
        .map_err(|e| e.to_string())?;
    match response.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Files, through the host
// ---------------------------------------------------------------------------
//
// Paths are the plugin's: `/host` (its cwd), `/data`, `/cache`, `/tmp` and `/plugins` (the user's
// plugin folder, when one is configured), or a relative path under `/host`. Anything else needs
// `FullHdAccess`. Reading needs `ReadFiles`, writing `WriteFiles` - and writing under `/plugins`
// `AuthorPlugins`. The host resolves symlinks and refuses a path that leaves its root, which is
// what makes these safer than reaching the same folders through WASI directly. Each answers
// synchronously, so a plugin can read a file in one expression.

fn fs_command_response<T: prost::Message + Default>(
    plugin_command: PluginCommand,
) -> Result<T, String> {
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
    let response_bytes = bytes_from_stdin().map_err(|e| e.to_string())?;
    T::decode(response_bytes.as_slice()).map_err(|e| e.to_string())
}

fn fs_mutate(plugin_command: PluginCommand) -> Result<(), String> {
    let response: ProtobufFsMutateResponse = fs_command_response(plugin_command)?;
    match response.result {
        Some(fs_mutate_response::Result::Success(_)) => Ok(()),
        Some(fs_mutate_response::Result::Error(error)) => Err(error),
        None => Err("zellij answered with nothing".to_owned()),
    }
}

/// The contents of `path`, up to `max_bytes` (the host's own cap otherwise).
pub fn fs_read(path: &Path, max_bytes: Option<usize>) -> Result<Vec<u8>, String> {
    let response: ProtobufFsReadResponse = fs_command_response(PluginCommand::FsRead {
        path: path.to_path_buf(),
        max_bytes,
    })?;
    match response.result {
        Some(fs_read_response::Result::Contents(contents)) => Ok(contents),
        Some(fs_read_response::Result::Error(error)) => Err(error),
        None => Err("zellij answered with nothing".to_owned()),
    }
}

/// The entries of the folder at `path`, one level deep.
pub fn fs_list(path: &Path) -> Result<Vec<FsEntry>, String> {
    let response: ProtobufFsListResponse = fs_command_response(PluginCommand::FsList {
        path: path.to_path_buf(),
    })?;
    match response.result {
        Some(fs_list_response::Result::Entries(list)) => Ok(list
            .entries
            .into_iter()
            .map(|entry| FsEntry {
                path: entry.path,
                metadata: entry
                    .metadata
                    .as_ref()
                    .and_then(Option::<FileMetadata>::from),
            })
            .collect()),
        Some(fs_list_response::Result::Error(error)) => Err(error),
        None => Err("zellij answered with nothing".to_owned()),
    }
}

/// What is at `path`: `Ok(None)` when nothing is, which is an answer rather than an error.
pub fn fs_metadata(path: &Path) -> Result<Option<FileMetadata>, String> {
    let response: ProtobufFsMetadataResponse = fs_command_response(PluginCommand::FsMetadata {
        path: path.to_path_buf(),
    })?;
    match response.result {
        Some(fs_metadata_response::Result::Metadata(metadata)) => {
            Ok(Option::<FileMetadata>::from(&metadata))
        },
        Some(fs_metadata_response::Result::NotFound(_)) => Ok(None),
        Some(fs_metadata_response::Result::Error(error)) => Err(error),
        None => Err("zellij answered with nothing".to_owned()),
    }
}

/// Write `contents` to `path`, replacing the file - or adding to it with `append` - and making
/// the folders above it first with `create_dirs`.
pub fn fs_write(
    path: &Path,
    contents: &[u8],
    append: bool,
    create_dirs: bool,
) -> Result<(), String> {
    fs_mutate(PluginCommand::FsWrite {
        path: path.to_path_buf(),
        contents: contents.to_vec(),
        append,
        create_dirs,
    })
}

/// Remove the file or - with `recursive` - the folder at `path`.
pub fn fs_remove(path: &Path, recursive: bool) -> Result<(), String> {
    fs_mutate(PluginCommand::FsRemove {
        path: path.to_path_buf(),
        recursive,
    })
}

/// Make the folder at `path`, and with `all` every folder above it.
pub fn fs_create_dir(path: &Path, all: bool) -> Result<(), String> {
    fs_mutate(PluginCommand::FsCreateDir {
        path: path.to_path_buf(),
        all,
    })
}

/// Move `from` to `to`, both under the same rules.
pub fn fs_rename(from: &Path, to: &Path) -> Result<(), String> {
    fs_mutate(PluginCommand::FsRename {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
    })
}

/// Parses a KDL layout string and returns LayoutMetadata
pub fn parse_layout(layout_string: &str) -> Result<LayoutMetadata, LayoutParsingError> {
    let plugin_command = PluginCommand::ParseLayout(layout_string.to_string());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());

    unsafe { host_run_plugin_command() };

    let response_bytes = bytes_from_stdin().map_err(|_| LayoutParsingError::SyntaxError)?;

    let protobuf_response = ProtobufParseLayoutResponse::decode(response_bytes.as_slice())
        .map_err(|_| LayoutParsingError::SyntaxError)?;

    match protobuf_response.result {
        Some(parse_layout_response::Result::Metadata(metadata)) => metadata
            .try_into()
            .map_err(|_| LayoutParsingError::SyntaxError),
        Some(parse_layout_response::Result::Error(error)) => Err(error
            .try_into()
            .map_err(|_| LayoutParsingError::SyntaxError)?),
        None => Err(LayoutParsingError::SyntaxError),
    }
}

/// List all session recordings (newest session first), if session recording is enabled.
///
/// Requires the `ReadPaneContents` permission.
pub fn list_recordings() -> Vec<RecordingSummary> {
    let plugin_command = PluginCommand::ListRecordings;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let protobuf_response =
        ProtobufListRecordingsResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    protobuf_response
        .recordings
        .into_iter()
        .map(|r| r.into())
        .collect()
}

/// List every toast received by the current live session, oldest first.
///
/// Requires the `ReadApplicationState` permission.
pub fn list_notifications() -> Result<Vec<NotificationInfo>, String> {
    let plugin_command = PluginCommand::ListNotifications;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response_bytes = bytes_from_stdin()
        .map_err(|error| format!("Failed to read ListNotifications response: {error}"))?;
    let response = ProtobufListNotificationsResponse::decode(response_bytes.as_slice())
        .map_err(|error| format!("Malformed ListNotifications response: {error}"))?;
    match response.error {
        Some(error) => Err(error),
        None => Ok(response.notifications.into_iter().map(Into::into).collect()),
    }
}

/// Full-text search the recorded terminal output of all session recordings, returning at most
/// `limit` hits.
///
/// Requires the `ReadPaneContents` permission. Note this brings the search index up to date
/// first, so the first call after a lot of recording activity can take a while.
pub fn search_recordings(query: &str, limit: usize) -> Vec<SearchHit> {
    let plugin_command = PluginCommand::SearchRecordings {
        query: query.to_owned(),
        limit,
    };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let protobuf_response =
        ProtobufSearchRecordingsResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    protobuf_response
        .hits
        .into_iter()
        .map(|h| h.into())
        .collect()
}

/// Get a list of clients, their focused pane and running command or focused plugin back as an
/// Event::ListClients (note: this event must be subscribed to)
pub fn list_clients() {
    let plugin_command = PluginCommand::ListClients;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Change configuration for the current user
pub fn reconfigure(new_config: String, save_configuration_file: bool) {
    let plugin_command = PluginCommand::Reconfigure(new_config, save_configuration_file);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Re-run command in pane
pub fn rerun_command_pane(terminal_pane_id: u32) {
    let plugin_command = PluginCommand::RerunCommandPane(terminal_pane_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Sugar for close_terminal_pane and close_plugin_pane
pub fn close_pane_with_id(pane_id: PaneId) {
    let plugin_command = match pane_id {
        PaneId::Terminal(terminal_pane_id) => PluginCommand::CloseTerminalPane(terminal_pane_id),
        PaneId::Plugin(plugin_pane_id) => PluginCommand::ClosePluginPane(plugin_pane_id),
    };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Resize the specified pane (increase/decrease) with an optional direction (left/right/up/down)
pub fn resize_pane_with_id(resize_strategy: ResizeStrategy, pane_id: PaneId) {
    let plugin_command = PluginCommand::ResizePaneIdWithDirection(resize_strategy, pane_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Changes the focus to the pane with the specified id, unsuppressing it if it was suppressed and switching to its tab and layer (eg. floating/tiled).
pub fn focus_pane_with_id(
    pane_id: PaneId,
    should_float_if_hidden: bool,
    should_be_in_place_if_hidden: bool,
) {
    let plugin_command = match pane_id {
        PaneId::Terminal(terminal_pane_id) => PluginCommand::FocusTerminalPane(
            terminal_pane_id,
            should_float_if_hidden,
            should_be_in_place_if_hidden,
        ),
        PaneId::Plugin(plugin_pane_id) => PluginCommand::FocusPluginPane(
            plugin_pane_id,
            should_float_if_hidden,
            should_be_in_place_if_hidden,
        ),
    };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Edit the scrollback of the specified pane in the user's default `$EDITOR` (currently only works
/// for terminal panes)
pub fn edit_scrollback_for_pane_with_id(pane_id: PaneId) {
    let plugin_command = PluginCommand::EditScrollbackForPaneWithId(pane_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Retrieves the scrollback contents from the specified pane
///
/// # Arguments
/// * `pane_id` - The ID of the pane to get scrollback from
/// * `get_full_scrollback` - Whether to retrieve the full scrollback buffer (including lines above and below viewport)
///
/// # Returns
/// * `Ok(PaneContents)` - The pane contents if successful
/// * `Err(String)` - An error message if the pane was not found, timed out, or another error occurred
pub fn get_pane_scrollback(
    pane_id: PaneId,
    get_full_scrollback: bool,
) -> Result<PaneContents, String> {
    let plugin_command = PluginCommand::GetPaneScrollback {
        pane_id,
        get_full_scrollback,
    };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    // Read response from stdin
    let response_bytes =
        bytes_from_stdin().map_err(|e| format!("Failed to read response from stdin: {:?}", e))?;

    // Decode protobuf response
    let protobuf_response = ProtobufPaneScrollbackResponse::decode(response_bytes.as_slice())
        .map_err(|e| format!("Failed to decode protobuf response: {}", e))?;

    // Convert to Rust type
    let response = PaneScrollbackResponse::try_from(protobuf_response)
        .map_err(|e| format!("Failed to convert protobuf response: {}", e))?;

    // Convert Result enum to actual Result type
    match response {
        PaneScrollbackResponse::Ok(contents) => Ok(contents),
        PaneScrollbackResponse::Err(error_msg) => Err(error_msg),
    }
}

/// Write bytes to the `STDIN` of the specified pane
pub fn write_to_pane_id(bytes: Vec<u8>, pane_id: PaneId) {
    let plugin_command = PluginCommand::WriteToPaneId(bytes, pane_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Write characters to the `STDIN` of the specified pane
pub fn write_chars_to_pane_id(chars: &str, pane_id: PaneId) {
    let plugin_command = PluginCommand::WriteCharsToPaneId(chars.to_owned(), pane_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Send SIGINT to the process running inside a terminal pane identified by this PaneId
pub fn send_sigint_to_pane_id(pane_id: PaneId) {
    let plugin_command = PluginCommand::SendSigintToPaneId(pane_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Send SIGKILL to the process running inside a terminal pane identified by this PaneId
pub fn send_sigkill_to_pane_id(pane_id: PaneId) {
    let plugin_command = PluginCommand::SendSigkillToPaneId(pane_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Get the PID of the process running inside a terminal pane
pub fn get_pane_pid(pane_id: PaneId) -> Result<i32, String> {
    let plugin_command = PluginCommand::GetPanePid { pane_id };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    // Read response from stdin
    let response_bytes =
        bytes_from_stdin().map_err(|e| format!("Failed to read response from stdin: {:?}", e))?;

    // Decode protobuf response
    let protobuf_response = ProtobufGetPanePidResponse::decode(response_bytes.as_slice())
        .map_err(|e| format!("Failed to decode protobuf response: {}", e))?;

    // Convert to Rust type
    let response = GetPanePidResponse::try_from(protobuf_response)
        .map_err(|e| format!("Failed to convert protobuf response: {}", e))?;

    // Convert Result enum to actual Result type
    match response {
        GetPanePidResponse::Ok(pid) => Ok(pid),
        GetPanePidResponse::Err(error_msg) => Err(error_msg),
    }
}

/// Gets the current running command for a specific pane by its ID.
///
/// This queries the operating system for the **current** running command,
/// not the initial command used to launch the pane. The command is returned
/// as a vector of strings (argv-style), where the first element is the
/// executable and subsequent elements are arguments.
///
/// # Arguments
/// * `pane_id` - The ID of the pane to query
///
/// # Returns
/// * `Ok(Vec<String>)` - The command and arguments as separate strings
/// * `Err(String)` - Error message if:
///   - Pane is a plugin (only terminal panes have commands)
///   - Pane doesn't exist
///   - OS query failed
///
/// # Permissions Required
/// * `ReadApplicationState`
///
/// # Example
/// ```no_run
/// use zellij_tile::prelude::*;
///
/// let pane_id = PaneId::Terminal(1);
/// match get_pane_running_command(pane_id) {
///     Ok(cmd) => eprintln!("Running: {} {}", cmd[0], cmd[1..].join(" ")),
///     Err(e) => eprintln!("Error: {}", e),
/// }
/// ```
pub fn get_pane_running_command(pane_id: PaneId) -> Result<Vec<String>, String> {
    let plugin_command = PluginCommand::GetPaneRunningCommand { pane_id };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let protobuf_response =
        ProtobufGetPaneRunningCommandResponse::decode(bytes_from_stdin().unwrap().as_slice())
            .unwrap();

    match protobuf_response.result {
        Some(get_pane_running_command_response::Result::Command(cmd)) => Ok(cmd.args),
        Some(get_pane_running_command_response::Result::Error(err)) => Err(err),
        None => Err("Empty response from server".to_string()),
    }
}

/// Fetches a fresh snapshot of all live and resurrectable sessions on this machine.
///
/// # Permissions Required
/// * `ReadApplicationState`
pub fn get_session_list() -> Result<SessionListSnapshot, String> {
    let plugin_command = PluginCommand::GetSessionList;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let protobuf_response =
        ProtobufGetSessionListResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();

    match protobuf_response.result {
        Some(get_session_list_response::Result::Snapshot(snapshot)) => {
            let mut live_sessions = Vec::new();
            for manifest in snapshot.live_sessions {
                match SessionInfo::try_from(manifest) {
                    Ok(si) => live_sessions.push(si),
                    Err(e) => return Err(format!("Malformed session manifest: {}", e)),
                }
            }
            let resurrectable_sessions = snapshot
                .resurrectable_sessions
                .into_iter()
                .map(<(String, std::time::Duration)>::from)
                .collect();
            Ok(SessionListSnapshot {
                live_sessions,
                resurrectable_sessions,
            })
        },
        Some(get_session_list_response::Result::Error(err)) => Err(err),
        None => Err("Empty response from server".to_string()),
    }
}

/// Gets the current working directory for a specific pane by its ID.
///
/// This queries the operating system for the **current** working directory
/// of the process running in the pane. The CWD may change as the user
/// navigates directories within the terminal.
///
/// # Arguments
/// * `pane_id` - The ID of the pane to query
///
/// # Returns
/// * `Ok(PathBuf)` - The current working directory
/// * `Err(String)` - Error message if:
///   - Pane is a plugin (only terminal panes have CWDs)
///   - Pane doesn't exist
///   - OS query failed (process may have exited)
///   - CWD is inaccessible (permissions, deleted directory)
///
/// # Permissions Required
/// * `ReadApplicationState`
///
/// # Example
/// ```no_run
/// use zellij_tile::prelude::*;
///
/// let pane_id = PaneId::Terminal(1);
/// match get_pane_cwd(pane_id) {
///     Ok(cwd) => eprintln!("CWD: {}", cwd.display()),
///     Err(e) => eprintln!("Error: {}", e),
/// }
/// ```
pub fn get_pane_cwd(pane_id: PaneId) -> Result<PathBuf, String> {
    let plugin_command = PluginCommand::GetPaneCwd { pane_id };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let protobuf_response =
        ProtobufGetPaneCwdResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();

    match protobuf_response.result {
        Some(get_pane_cwd_response::Result::Cwd(cwd_str)) => Ok(PathBuf::from(cwd_str)),
        Some(get_pane_cwd_response::Result::Error(err)) => Err(err),
        None => Err("Empty response from server".to_string()),
    }
}

/// What Git says about a **remote** pane's folder, as its own machine sees it.
///
/// Raw and unparsed — the same two `git` invocations a caller would run itself,
/// with their bytes and exit code — so the parsing stays in one place rather
/// than being duplicated for remote panes.
///
/// Only a remote pane has an answer. A local pane returns `Err`: nothing on this
/// machine has a cached probe to give, and the pty thread will not spawn `git`
/// to make one. Branch on `PaneInfo::host` and run Git yourself for a local
/// pane. The `cwd` in the answer is the folder the probe was *run* in, which for
/// a pane that has just moved is not the folder it is in now — file the result
/// under the probe's own folder rather than the pane's.
///
/// # Permissions Required
/// * `ReadApplicationState`
pub fn get_pane_git_probe(pane_id: PaneId) -> Result<PaneGitProbe, String> {
    let plugin_command = PluginCommand::GetPaneGitProbe { pane_id };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let protobuf_response =
        ProtobufGetPaneGitProbeResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();

    match protobuf_response.result {
        Some(get_pane_git_probe_response::Result::Probe(probe)) => Ok(PaneGitProbe {
            host: probe.host,
            cwd: probe.cwd,
            exit_code: probe.exit_code,
            metadata: probe.metadata,
            config: probe.config,
        }),
        Some(get_pane_git_probe_response::Result::Error(err)) => Err(err),
        None => Err("Empty response from server".to_string()),
    }
}

/// Save a layout to the user's layout directory
///
/// # Arguments
/// * `layout_name` - Name of the layout file (without .kdl extension)
/// * `layout_kdl` - KDL string representing the layout
/// * `overwrite` - Whether to overwrite if the file already exists
///
/// # Returns
/// * `Ok(())` - Layout was successfully validated and saved
/// * `Err(String)` - Error message (parse error, I/O error, file exists, etc.)
pub fn save_layout<S: AsRef<str>>(
    layout_name: S,
    layout_kdl: S,
    overwrite: bool,
) -> Result<(), String> {
    let plugin_command = PluginCommand::SaveLayout {
        layout_name: layout_name.as_ref().to_owned(),
        layout_kdl: layout_kdl.as_ref().to_owned(),
        overwrite,
    };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    // Read response from stdin
    let response_bytes =
        bytes_from_stdin().map_err(|e| format!("Failed to read response from stdin: {:?}", e))?;

    // Decode protobuf response
    let protobuf_response = ProtobufSaveLayoutResponse::decode(response_bytes.as_slice())
        .map_err(|e| format!("Failed to decode protobuf response: {}", e))?;

    // Convert to Rust type
    let response = SaveLayoutResponse::try_from(protobuf_response)
        .map_err(|e| format!("Failed to convert protobuf response: {}", e))?;

    // Convert Result enum to actual Result type
    match response {
        SaveLayoutResponse::Ok(_) => Ok(()),
        SaveLayoutResponse::Err(error_msg) => Err(error_msg),
    }
}

/// Delete a layout from the user's layout directory.
///
/// # Arguments
///
/// * `layout_name` - The name of the layout file to delete (without the `.kdl` extension).
///   Layout names are sanitized server-side to prevent directory traversal.
///
/// # Returns
///
/// * `Ok(())` - The layout was successfully deleted
/// * `Err(String)` - An error occurred with a descriptive message (e.g., file not found, permission denied)
///
/// # Permissions
///
/// Requires the `ChangeApplicationState` permission.
///
/// # Examples
///
/// ```no_run
/// use zellij_tile::prelude::*;
///
/// // Delete a layout named "my-layout"
/// match delete_layout("my-layout") {
///     Ok(_) => eprintln!("Layout deleted successfully"),
///     Err(e) => eprintln!("Failed to delete layout: {}", e),
/// }
/// ```
pub fn delete_layout<S: AsRef<str>>(layout_name: S) -> Result<(), String> {
    let plugin_command = PluginCommand::DeleteLayout {
        layout_name: layout_name.as_ref().to_owned(),
    };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    // Read response from stdin
    let response_bytes =
        bytes_from_stdin().map_err(|e| format!("Failed to read response from stdin: {:?}", e))?;

    // Decode protobuf response
    let protobuf_response = ProtobufDeleteLayoutResponse::decode(response_bytes.as_slice())
        .map_err(|e| format!("Failed to decode protobuf response: {}", e))?;

    // Convert to Rust type
    let response = DeleteLayoutResponse::try_from(protobuf_response)
        .map_err(|e| format!("Failed to convert protobuf response: {}", e))?;

    // Convert Result enum to actual Result type
    match response {
        DeleteLayoutResponse::Ok(_) => Ok(()),
        DeleteLayoutResponse::Err(error_msg) => Err(error_msg),
    }
}

/// Rename a layout file in the user's layout directory
///
/// # Arguments
/// * `old_layout_name` - Current name of the layout (without .kdl extension)
/// * `new_layout_name` - New name for the layout (without .kdl extension)
///
/// # Returns
/// * `Ok(())` - Layout was successfully renamed
/// * `Err(String)` - Error message describing what went wrong
///
/// # Error Cases
/// * Old layout name is invalid (empty, contains path separators, etc.)
/// * New layout name is invalid
/// * Source layout file doesn't exist
/// * Target layout file already exists (no overwrite)
/// * Layout directory not found
/// * File system error during rename operation
///
/// # Permissions
///
/// Requires the `ChangeApplicationState` permission.
///
/// # Examples
///
/// ```no_run
/// use zellij_tile::prelude::*;
///
/// // Rename a layout from "old-name" to "new-name"
/// match rename_layout("old-name", "new-name") {
///     Ok(_) => eprintln!("Layout renamed successfully"),
///     Err(e) => eprintln!("Failed to rename layout: {}", e),
/// }
/// ```
pub fn rename_layout(
    old_layout_name: impl Into<String>,
    new_layout_name: impl Into<String>,
) -> Result<(), String> {
    let plugin_command = PluginCommand::RenameLayout {
        old_layout_name: old_layout_name.into(),
        new_layout_name: new_layout_name.into(),
    };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    // Read response from stdin
    let response_bytes =
        bytes_from_stdin().map_err(|e| format!("Failed to read response from stdin: {:?}", e))?;

    // Decode protobuf response
    let protobuf_response = ProtobufRenameLayoutResponse::decode(response_bytes.as_slice())
        .map_err(|e| format!("Failed to decode protobuf response: {}", e))?;

    // Convert to native response type
    let response: RenameLayoutResponse = protobuf_response
        .try_into()
        .map_err(|e| format!("Failed to convert protobuf response: {}", e))?;

    match response {
        RenameLayoutResponse::Ok(_) => Ok(()),
        RenameLayoutResponse::Err(error_msg) => Err(error_msg),
    }
}

/// Opens a layout file in the user's default `$EDITOR`
pub fn edit_layout<S: AsRef<str>>(
    layout_name: S,
    context: BTreeMap<String, String>,
) -> Result<(), String> {
    let layout_name = layout_name.as_ref().to_owned();
    let plugin_command = PluginCommand::EditLayout {
        layout_name,
        context,
    };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    // Read response from stdin
    let response_bytes =
        bytes_from_stdin().map_err(|e| format!("Failed to read response from stdin: {:?}", e))?;

    // Decode protobuf response
    let protobuf_response = ProtobufEditLayoutResponse::decode(response_bytes.as_slice())
        .map_err(|e| format!("Failed to decode protobuf response: {}", e))?;

    // Convert to Rust type
    let response = EditLayoutResponse::try_from(protobuf_response)
        .map_err(|e| format!("Failed to convert protobuf response: {}", e))?;

    // Convert Result enum to actual Result type
    match response {
        EditLayoutResponse::Ok(_) => Ok(()),
        EditLayoutResponse::Err(error_msg) => Err(error_msg),
    }
}

/// Switch the position of the pane with this id with a different pane
pub fn move_pane_with_pane_id(pane_id: PaneId) {
    let plugin_command = PluginCommand::MovePaneWithPaneId(pane_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Switch the position of the pane with this id with a different pane in the specified direction (eg. `Down`, `Up`, `Left`, `Right`).
pub fn move_pane_with_pane_id_in_direction(pane_id: PaneId, direction: Direction) {
    let plugin_command = PluginCommand::MovePaneWithPaneIdInDirection(pane_id, direction);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Clear the scroll buffer of the specified pane
pub fn clear_screen_for_pane_id(pane_id: PaneId) {
    let plugin_command = PluginCommand::ClearScreenForPaneId(pane_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Scroll the specified pane up 1 line
pub fn scroll_up_in_pane_id(pane_id: PaneId) {
    let plugin_command = PluginCommand::ScrollUpInPaneId(pane_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Scroll the specified pane down 1 line
pub fn scroll_down_in_pane_id(pane_id: PaneId) {
    let plugin_command = PluginCommand::ScrollDownInPaneId(pane_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Scroll the specified pane all the way to the top of the scrollbuffer
pub fn scroll_to_top_in_pane_id(pane_id: PaneId) {
    let plugin_command = PluginCommand::ScrollToTopInPaneId(pane_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Scroll the specified pane all the way to the bottom of the scrollbuffer
pub fn scroll_to_bottom_in_pane_id(pane_id: PaneId) {
    let plugin_command = PluginCommand::ScrollToBottomInPaneId(pane_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Scroll the specified pane up one page
pub fn page_scroll_up_in_pane_id(pane_id: PaneId) {
    let plugin_command = PluginCommand::PageScrollUpInPaneId(pane_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Scroll the specified pane down one page
pub fn page_scroll_down_in_pane_id(pane_id: PaneId) {
    let plugin_command = PluginCommand::PageScrollDownInPaneId(pane_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Toggle the specified pane to be fullscreen or normal sized
pub fn toggle_pane_id_fullscreen(pane_id: PaneId) {
    let plugin_command = PluginCommand::TogglePaneIdFullscreen(pane_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Embed the specified pane (make it stop floating) or turn it to a float pane if it is not
pub fn toggle_pane_embed_or_eject_for_pane_id(pane_id: PaneId) {
    let plugin_command = PluginCommand::TogglePaneEmbedOrEjectForPaneId(pane_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Close the focused tab
pub fn close_tab_with_index(tab_index: usize) {
    let plugin_command = PluginCommand::CloseTabWithIndex(tab_index);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Close the tab with the given stable ID.
///
/// Unlike `close_tab_with_index`, this function identifies the tab by its stable
/// `tab_id` rather than its display position, which may change as tabs are moved
/// or closed. The tab_id can be obtained from `TabInfo.tab_id`.
pub fn close_tab_with_id(tab_id: u64) {
    let plugin_command = PluginCommand::CloseTabWithId(tab_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Focus the tab with the given stable ID.
///
/// Unlike `go_to_tab`, this function identifies the tab by its stable `tab_id`
/// rather than its display position, which may change as tabs are moved or
/// closed. The tab_id can be obtained from `TabInfo.tab_id`.
pub fn go_to_tab_with_id(tab_id: u64) {
    let plugin_command = PluginCommand::GoToTabWithId(tab_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Rename the specified pane
pub fn rename_pane_with_id<S>(pane_id: PaneId, new_name: S)
where
    S: AsRef<str> + ToString,
{
    let plugin_command = match pane_id {
        PaneId::Terminal(terminal_pane_id) => {
            PluginCommand::RenameTerminalPane(terminal_pane_id, new_name.to_string())
        },
        PaneId::Plugin(plugin_pane_id) => {
            PluginCommand::RenamePluginPane(plugin_pane_id, new_name.to_string())
        },
    };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Compose a decoration in front of a pane's title, or drop it with `None`.
///
/// Not a rename. The pane's name, its own OSC title and the `title` every plugin
/// reads off the pane manifest are all untouched, so the prefix survives every
/// rename and cannot erase one — which is what makes it the place for a fact about
/// the pane rather than a name for it. It shows wherever a human looks at the pane:
/// its frame, and its row in a stack list. Terminal panes only.
pub fn set_pane_title_prefix(pane_id: PaneId, prefix: Option<&str>) {
    let plugin_command = PluginCommand::SetPaneTitlePrefix {
        pane_id,
        prefix: prefix.map(ToOwned::to_owned),
    };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Create a new tab that includes the specified pane ids
pub fn break_panes_to_new_tab(
    pane_ids: &[PaneId],
    new_tab_name: Option<String>,
    should_change_focus_to_new_tab: bool,
) -> Option<usize> {
    let plugin_command = PluginCommand::BreakPanesToNewTab(
        pane_ids.to_vec(),
        new_tab_name,
        should_change_focus_to_new_tab,
    );
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufBreakPanesToNewTabResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    BreakPanesToNewTabResponse::try_from(response).unwrap()
}

/// Move the pane ids to the tab with the specified index
pub fn break_panes_to_tab_with_index(
    pane_ids: &[PaneId],
    tab_index: usize,
    should_change_focus_to_new_tab: bool,
) -> Option<usize> {
    let plugin_command = PluginCommand::BreakPanesToTabWithIndex(
        pane_ids.to_vec(),
        tab_index,
        should_change_focus_to_new_tab,
    );
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufBreakPanesToTabWithIndexResponse::decode(bytes_from_stdin().unwrap().as_slice())
            .unwrap();
    BreakPanesToTabWithIndexResponse::try_from(response).unwrap()
}

/// Move the pane ids to the tab with the specified id
pub fn break_panes_to_tab_with_id(
    pane_ids: &[PaneId],
    tab_id: usize,
    should_change_focus_to_target_tab: bool,
) -> Option<usize> {
    let plugin_command = PluginCommand::BreakPanesToTabWithId(
        pane_ids.to_vec(),
        tab_id as u64,
        should_change_focus_to_target_tab,
    );
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let response =
        ProtobufBreakPanesToTabWithIdResponse::decode(bytes_from_stdin().unwrap().as_slice())
            .unwrap();
    BreakPanesToTabWithIdResponse::try_from(response).unwrap()
}

/// Reload an already-running in this session, optionally skipping the cache
pub fn reload_plugin_with_id(plugin_id: u32) {
    let plugin_command = PluginCommand::ReloadPlugin(plugin_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Reload an already-running in this session, optionally skipping the cache
pub fn load_new_plugin<S>(
    url: S,
    config: BTreeMap<String, String>,
    load_in_background: bool,
    skip_plugin_cache: bool,
) where
    S: AsRef<str> + ToString,
{
    let plugin_command = PluginCommand::LoadNewPlugin {
        url: url.to_string(),
        config,
        load_in_background,
        skip_plugin_cache,
    };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

// Work queues

/// Put a plugin on a named work queue, to run as a fresh headless instance when the queue has a
/// free slot. Returns the job id.
///
/// The queue is created with concurrency 1 if it does not exist; see [`configure_queue`].
/// `config` becomes the job's configuration - alongside `zellij_queue`, `zellij_queue_job_id` and
/// `zellij_queue_enqueuer_plugin_id`, which the host adds - and `context` is never given to the
/// job but echoed back in [`Event::QueueJobStarted`] and [`Event::QueueJobFinished`], which only
/// the enqueuer receives (subscribe to them). Needs `MessageAndLaunchOtherPlugins`.
pub fn enqueue_queue_job<Q, U>(
    queue: Q,
    plugin_url: U,
    config: BTreeMap<String, String>,
    context: BTreeMap<String, String>,
) -> Result<u64, String>
where
    Q: ToString,
    U: ToString,
{
    let plugin_command = PluginCommand::EnqueueQueueJob {
        queue: queue.to_string(),
        plugin_url: plugin_url.to_string(),
        config,
        context,
    };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let protobuf_response =
        ProtobufEnqueueQueueJobResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    match EnqueueQueueJobResponse::try_from(protobuf_response) {
        Ok(EnqueueQueueJobResponse::Ok(job_id)) => Ok(job_id),
        Ok(EnqueueQueueJobResponse::Err(error)) => Err(error),
        Err(e) => Err(e.to_owned()),
    }
}

/// Set how many jobs `queue` may run at once, creating it if needed. Lowering the limit below
/// the number of running jobs evicts none of them. Needs `ChangeApplicationState`.
pub fn configure_queue<Q: ToString>(queue: Q, concurrency: usize) {
    let plugin_command = PluginCommand::ConfigureQueue {
        queue: queue.to_string(),
        concurrency,
    };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Drop a pending job, or unload a running one. Its enqueuer is told `Cancelled`. Needs
/// `ChangeApplicationState`.
pub fn cancel_queue_job(job_id: u64) {
    let plugin_command = PluginCommand::CancelQueueJob { job_id };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Every queue, or just the named one, with its pending and running jobs. Needs
/// `ReadApplicationState`. To be told about changes instead of asking, subscribe to
/// [`Event::QueueChanged`].
pub fn queue_status(queue: Option<&str>) -> Vec<QueueSnapshot> {
    let plugin_command = PluginCommand::QueueStatus {
        queue: queue.map(ToOwned::to_owned),
    };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };

    let protobuf_response =
        ProtobufQueueStatusResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    QueueStatusResponse::try_from(protobuf_response).unwrap_or_default()
}

/// Called by a job to report its outcome. The host unloads this plugin afterwards, so anything
/// still to do should be done before. Ignored when the caller is not a queued job.
pub fn finish_queue_job(success: bool, payload: Option<String>) {
    let plugin_command = PluginCommand::FinishQueueJob { success, payload };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Rebind keys for the current user
pub fn rebind_keys(
    keys_to_unbind: Vec<(InputMode, KeyWithModifier)>,
    keys_to_rebind: Vec<(InputMode, KeyWithModifier, Vec<Action>)>,
    write_config_to_disk: bool,
) {
    let plugin_command = PluginCommand::RebindKeys {
        keys_to_rebind,
        keys_to_unbind,
        write_config_to_disk,
    };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn change_host_folder(new_host_folder: PathBuf) {
    let plugin_command = PluginCommand::ChangeHostFolder(new_host_folder);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn set_floating_pane_pinned(pane_id: PaneId, should_be_pinned: bool) {
    let plugin_command = PluginCommand::SetFloatingPanePinned(pane_id, should_be_pinned);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn stack_panes(pane_ids: Vec<PaneId>) {
    let plugin_command = PluginCommand::StackPanes(pane_ids);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn change_floating_panes_coordinates(
    pane_ids_and_coordinates: Vec<(PaneId, FloatingPaneCoordinates)>,
) {
    let plugin_command = PluginCommand::ChangeFloatingPanesCoordinates(pane_ids_and_coordinates);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Move or resize an overlay - a pane drawn over the whole session surface.
///
/// Its kind - dismissible or modal - was decided when it was born and is left alone here. Use
/// `change_floating_panes_coordinates` for a pane on a tab's floating layer.
pub fn change_overlay_coordinates(
    pane_ids_and_coordinates: Vec<(PaneId, FloatingPaneCoordinates)>,
) {
    change_overlay_coordinates_with(pane_ids_and_coordinates, false);
}

/// The same call, for a placement the user asked for by a key in the plugin.
///
/// Once the user has dragged or resized an overlay with the mouse, the server keeps their
/// position or size and takes only the rest from `change_overlay_coordinates` - a plugin that
/// re-places itself on every resize would otherwise undo every step of the drag. A key the user
/// pressed is as much theirs as the drag was, so this variant reclaims both: the overlay goes
/// where the coordinates say, and the plugin's later placements move it again until the next
/// drag.
pub fn change_overlay_coordinates_overriding_user_placement(
    pane_ids_and_coordinates: Vec<(PaneId, FloatingPaneCoordinates)>,
) {
    change_overlay_coordinates_with(pane_ids_and_coordinates, true);
}

fn change_overlay_coordinates_with(
    placements: Vec<(PaneId, FloatingPaneCoordinates)>,
    override_user_placement: bool,
) {
    let plugin_command = PluginCommand::ChangeOverlayCoordinates {
        placements,
        override_user_placement,
    };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Toggle the borderless state of a pane identified by pane_id
///
/// # Arguments
/// * `pane_id` - The ID of the pane to toggle (PaneId::Terminal or PaneId::Plugin)
pub fn toggle_pane_borderless(pane_id: PaneId) {
    let plugin_command = PluginCommand::TogglePaneBorderless(pane_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Set the borderless state of a pane explicitly
///
/// # Arguments
/// * `pane_id` - The ID of the pane (PaneId::Terminal or PaneId::Plugin)
/// * `borderless` - true for borderless, false for bordered
pub fn set_pane_borderless(pane_id: PaneId, borderless: bool) {
    let plugin_command = PluginCommand::SetPaneBorderless(pane_id, borderless);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Reload every running instance of the plugin at `url` in place, opening nothing; a plugin
/// that is not running is left alone. Needs `AuthorPlugins`.
pub fn reload_plugin_by_url(url: &str) {
    let plugin_command = PluginCommand::ReloadPluginByUrl(url.to_owned());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Set what agent runs in `pane_id` and what it is doing, or clear the record with `None`. The
/// record replaces the pane's whole one; every reader of the manifest sees it next report.
pub fn set_pane_agent(pane_id: PaneId, agent: Option<AgentInfo>) {
    let plugin_command = PluginCommand::SetPaneAgent { pane_id, agent };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Set the default foreground and/or background color of a pane
///
/// # Arguments
/// * `pane_id` - The ID of the pane (PaneId::Terminal or PaneId::Plugin)
/// * `fg` - Optional foreground color string (e.g. "#00e000"), None to leave unchanged
/// * `bg` - Optional background color string (e.g. "#001a3a"), None to leave unchanged
pub fn set_pane_color(pane_id: PaneId, fg: Option<String>, bg: Option<String>) {
    let plugin_command = PluginCommand::SetPaneColor(pane_id, fg, bg);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn start_web_server() {
    let plugin_command = PluginCommand::StartWebServer;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn stop_web_server() {
    let plugin_command = PluginCommand::StopWebServer;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn query_web_server_status() {
    let plugin_command = PluginCommand::QueryWebServerStatus;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn share_current_session() {
    let plugin_command = PluginCommand::ShareCurrentSession;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn stop_sharing_current_session() {
    let plugin_command = PluginCommand::StopSharingCurrentSession;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn group_and_ungroup_panes(
    pane_ids_to_group: Vec<PaneId>,
    pane_ids_to_ungroup: Vec<PaneId>,
    for_all_clients: bool,
) {
    let plugin_command = PluginCommand::GroupAndUngroupPanes(
        pane_ids_to_group,
        pane_ids_to_ungroup,
        for_all_clients,
    );
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn highlight_and_unhighlight_panes(
    pane_ids_to_highlight: Vec<PaneId>,
    pane_ids_to_unhighlight: Vec<PaneId>,
) {
    let plugin_command =
        PluginCommand::HighlightAndUnhighlightPanes(pane_ids_to_highlight, pane_ids_to_unhighlight);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Bring a pane into view for this client and highlight its frame, without
/// taking keyboard focus - unlike [`focus_pane_with_id`], which does both.
///
/// `label` is drawn in place of the pane's title for as long as the highlight is
/// up, so a pane can say what it has been singled out *for*. Clear both with
/// [`highlight_and_unhighlight_panes`], passing the pane as one to unhighlight.
///
/// How much can be revealed depends on where this client's keyboard focus is,
/// because Zellij gives a client one tab that is both the one it sees and the
/// one it types into. A suppressed or hidden floating pane is always brought
/// out; a pane in another tab is left where it stands, with the highlight applied
/// there and the client's view untouched - that is the promise, focus does not move.
pub fn reveal_pane_with_id(pane_id: PaneId, label: Option<String>) {
    let plugin_command = PluginCommand::RevealPaneWithId(pane_id, label);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn close_multiple_panes(pane_ids: Vec<PaneId>) {
    let plugin_command = PluginCommand::CloseMultiplePanes(pane_ids);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn float_multiple_panes(pane_ids: Vec<PaneId>) {
    let plugin_command = PluginCommand::FloatMultiplePanes(pane_ids);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn embed_multiple_panes(pane_ids: Vec<PaneId>) {
    let plugin_command = PluginCommand::EmbedMultiplePanes(pane_ids);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn set_self_mouse_selection_support(selection_support: bool) {
    let plugin_command = PluginCommand::SetSelfMouseSelectionSupport(selection_support);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn generate_web_login_token(
    token_label: Option<String>,
    read_only: bool,
) -> Result<String, String> {
    let plugin_command = PluginCommand::GenerateWebLoginToken(token_label, read_only);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
    let create_token_response =
        CreateTokenResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    if let Some(error) = create_token_response.error {
        Err(error)
    } else if let Some(token) = create_token_response.token {
        Ok(token)
    } else {
        Err("Received empty response".to_owned())
    }
}

pub fn revoke_web_login_token(token_label: &str) -> Result<(), String> {
    let plugin_command = PluginCommand::RevokeWebLoginToken(token_label.to_owned());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
    let revoke_token_response =
        RevokeTokenResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    if let Some(error) = revoke_token_response.error {
        Err(error)
    } else {
        Ok(())
    }
}

pub fn list_web_login_tokens() -> Result<Vec<(String, String, bool)>, String> {
    // (name, created_at, read_only)
    let plugin_command = PluginCommand::ListWebLoginTokens;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
    let list_tokens_response =
        ListTokensResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();

    if let Some(error) = list_tokens_response.error {
        Err(error)
    } else {
        let tokens_with_info = list_tokens_response
            .tokens
            .iter()
            .zip(list_tokens_response.creation_times.iter())
            .zip(list_tokens_response.read_only_flags.iter())
            .map(|((name, created_at), read_only)| (name.clone(), created_at.clone(), *read_only))
            .collect();
        Ok(tokens_with_info)
    }
}

pub fn revoke_all_web_tokens() -> Result<(), String> {
    let plugin_command = PluginCommand::RevokeAllWebLoginTokens;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
    let revoke_all_web_tokens_response =
        RevokeAllWebTokensResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    if let Some(error) = revoke_all_web_tokens_response.error {
        Err(error)
    } else {
        Ok(())
    }
}

pub fn rename_web_token(old_name: &str, new_name: &str) -> Result<(), String> {
    let plugin_command =
        PluginCommand::RenameWebLoginToken(old_name.to_owned(), new_name.to_owned());
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
    let rename_web_token_response =
        RenameWebTokenResponse::decode(bytes_from_stdin().unwrap().as_slice()).unwrap();
    if let Some(error) = rename_web_token_response.error {
        Err(error)
    } else {
        Ok(())
    }
}

pub fn intercept_key_presses() {
    let plugin_command = PluginCommand::InterceptKeyPresses;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn clear_key_presses_intercepts() {
    let plugin_command = PluginCommand::ClearKeyPressesIntercepts;
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn replace_pane_with_existing_pane(
    pane_id_to_replace: PaneId,
    existing_pane_id: PaneId,
    suppress_replaced_pane: bool,
) {
    let plugin_command = PluginCommand::ReplacePaneWithExistingPane(
        pane_id_to_replace,
        existing_pane_id,
        suppress_replaced_pane,
    );
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

// Utility Functions

#[allow(unused)]
/// Returns the `TabInfo` corresponding to the currently active tab
pub fn get_focused_tab(tab_infos: &Vec<TabInfo>) -> Option<TabInfo> {
    for tab_info in tab_infos {
        if tab_info.active {
            return Some(tab_info.clone());
        }
    }
    None
}

#[allow(unused)]
/// Returns the `PaneInfo` corresponding to the currently active pane (ignoring plugins)
pub fn get_focused_pane(tab_position: usize, pane_manifest: &PaneManifest) -> Option<PaneInfo> {
    let panes = pane_manifest.panes.get(&tab_position);
    if let Some(panes) = panes {
        for pane in panes {
            if pane.is_focused & !pane.is_plugin {
                return Some(pane.clone());
            }
        }
    }
    None
}

pub fn override_layout<L: AsRef<LayoutInfo>>(
    layout_info: L,
    retain_existing_terminal_panes: bool,
    retain_existing_plugin_panes: bool,
    apply_only_to_active_tab: bool,
    context: BTreeMap<String, String>,
) {
    let plugin_command = PluginCommand::OverrideLayout(
        layout_info.as_ref().clone(),
        retain_existing_terminal_panes,
        retain_existing_plugin_panes,
        apply_only_to_active_tab,
        context,
    );
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

// Internal Functions

#[doc(hidden)]
pub fn object_from_stdin<T: DeserializeOwned>() -> Result<T> {
    let err_context = || "failed to deserialize object from stdin".to_string();

    let mut json = String::new();
    io::stdin().read_line(&mut json).with_context(err_context)?;
    serde_json::from_str(&json).with_context(err_context)
}

#[doc(hidden)]
pub fn bytes_from_stdin() -> Result<Vec<u8>> {
    let err_context = || "failed to deserialize bytes from stdin".to_string();
    let mut json = String::new();
    io::stdin().read_line(&mut json).with_context(err_context)?;
    serde_json::from_str(&json).with_context(err_context)
}

#[doc(hidden)]
pub fn object_to_stdout(object: &impl Serialize) {
    // TODO: no crashy
    println!("{}", serde_json::to_string(object).unwrap());
}

/// Post a message to a worker of this plugin, for more information please see [Plugin Workers](https://zellij.dev/documentation/plugin-api-workers.md)
pub fn post_message_to(plugin_message: PluginMessage) {
    let plugin_command = PluginCommand::PostMessageTo(plugin_message);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Post a message to this plugin, for more information please see [Plugin Workers](https://zellij.dev/documentation/plugin-api-workers.md)
pub fn post_message_to_plugin(plugin_message: PluginMessage) {
    let plugin_command = PluginCommand::PostMessageToPlugin(plugin_message);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

pub fn run_action(action: Action, context: BTreeMap<String, String>) {
    // TODO: also accept reference
    let plugin_command = PluginCommand::RunAction(action, context);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Show all floating panes in the specified tab, or the active tab if `tab_id` is `None`.
///
/// Blocks until the server-side action is complete.
///
/// Returns `Ok(true)` if the floating panes were made visible (state changed),
/// `Ok(false)` if the floating panes were already visible (no change),
/// or `Err(String)` if the specified tab was not found.
pub fn show_floating_panes(tab_id: Option<usize>) -> Result<bool, String> {
    let plugin_command = PluginCommand::ShowFloatingPanes { tab_id };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
    let response_bytes =
        bytes_from_stdin().map_err(|e| format!("Failed to read response: {:?}", e))?;
    let response = ProtobufShowFloatingPanesResponse::decode(response_bytes.as_slice())
        .map_err(|e| format!("Failed to decode response: {}", e))?;
    match response.result {
        Some(show_floating_panes_response::Result::Success(changed)) => Ok(changed),
        Some(show_floating_panes_response::Result::Error(e)) => Err(e),
        None => Err("Empty response".to_string()),
    }
}

/// Hide all floating panes in the specified tab, or the active tab if `tab_id` is `None`.
///
/// Blocks until the server-side action is complete.
///
/// Returns `Ok(true)` if the floating panes were hidden (state changed),
/// `Ok(false)` if the floating panes were already hidden (no change),
/// or `Err(String)` if the specified tab was not found.
pub fn hide_floating_panes(tab_id: Option<usize>) -> Result<bool, String> {
    let plugin_command = PluginCommand::HideFloatingPanes { tab_id };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
    let response_bytes =
        bytes_from_stdin().map_err(|e| format!("Failed to read response: {:?}", e))?;
    let response = ProtobufHideFloatingPanesResponse::decode(response_bytes.as_slice())
        .map_err(|e| format!("Failed to decode response: {}", e))?;
    match response.result {
        Some(hide_floating_panes_response::Result::Success(changed)) => Ok(changed),
        Some(hide_floating_panes_response::Result::Error(e)) => Err(e),
        None => Err("Empty response".to_string()),
    }
}

/// Set or update regex-based content highlights for a pane.
///
/// Each entry in `highlights` is keyed by its `pattern` string. Calling this
/// function again with the same pattern updates its style; new patterns are
/// added; patterns not present in this call are kept. To remove all highlights
/// for this plugin on the pane, use `clear_pane_highlights`.
///
/// Pattern matching is performed by the server against the current viewport at
/// render time. The plugin never handles coordinates, so there is no race
/// condition between content changes and highlight application.
///
/// When `on_hover` is `true` and `tooltip_text` is `Some(...)`, the tooltip
/// text is displayed at the bottom of the pane frame (formatted as
/// `" Alt <Click> - {tooltip_text} "`) whenever the mouse cursor hovers over
/// the highlighted region.
///
/// Requires `ChangeApplicationState` permission.
pub fn set_pane_regex_highlights(pane_id: PaneId, highlights: Vec<RegexHighlight>) {
    let plugin_command = PluginCommand::SetPaneRegexHighlights(pane_id, highlights);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Answer a `Event::ContextMenuRequested` with the menu the user should see.
///
/// `request_id` comes from the event; echo it back. An answer for a menu that has already been
/// dismissed or replaced is dropped by the server, so there is no need to track whether the menu
/// is still open - and no way to get it wrong by answering late.
///
/// The items you were given carry an `id` and no actions: return one with its `id` intact and the
/// configured actions come back with it, relabelled or reordered however you like. An item you
/// build yourself has no `id`, runs no actions, and reaches you as `Event::ContextMenuItemSelected`
/// carrying its `context` - from where `run_action` can do anything an action could.
///
/// Requires `RunActionsAsUser` permission.
pub fn set_context_menu(request_id: u64, items: Vec<ContextMenuItem>) {
    let plugin_command = PluginCommand::SetContextMenu { request_id, items };
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

/// Remove all regex highlights this plugin has set on the given pane.
///
/// Other plugins' highlights on the same pane are not affected.
///
/// Requires `ChangeApplicationState` permission.
pub fn clear_pane_highlights(pane_id: PaneId) {
    let plugin_command = PluginCommand::ClearPaneHighlights(pane_id);
    let protobuf_plugin_command: ProtobufPluginCommand = plugin_command.try_into().unwrap();
    object_to_stdout(&protobuf_plugin_command.encode_to_vec());
    unsafe { host_run_plugin_command() };
}

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "zellij")]
extern "C" {
    fn host_run_plugin_command();
}

#[cfg(not(target_arch = "wasm32"))]
unsafe fn host_run_plugin_command() {}
