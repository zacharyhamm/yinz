use serde::de::DeserializeOwned;
use std::collections::{BTreeMap, HashSet};
use std::{
    io,
    path::{Path, PathBuf},
};
pub use yinz_plugin_api as plugin_api;
use yinz_plugin_api::event::ProtobufPaneScrollbackResponse;
use yinz_runtime::errors::prelude::*;
use yinz_types::agent::{AgentInfo, AgentSpawnSpec};
use yinz_types::command::{CommandToRun, GitCheckout};
use yinz_types::data::*;
pub use yinz_types::geometry::PercentOrFixed;
use yinz_types::geometry::{Direction, FloatingPaneCoordinates, Resize, ResizeStrategy};
use yinz_types::input::actions::Action;
use yinz_types::internal_ids::SupervisorId;
use yinz_types::internal_ids::TabId;
use yinz_types::internal_ids::TabNumber;
use yinz_types::internal_ids::TabPosition;
use yinz_types::internal_ids::{ClientId, SessionName};
use yinz_types::internal_ids::{ContextMenuRequestId, DataWatchId, QueueJobId};
use yinz_types::internal_ids::{PluginId, ProcessId};
use yinz_types::units::{FractionalSeconds, SignedUnixMillis};
use {
    yinz_plugin_api::generated_api::api::plugin_command::hide_floating_panes_response,
    yinz_plugin_api::generated_api::api::plugin_command::save_session_response,
    yinz_plugin_api::generated_api::api::plugin_command::show_floating_panes_response,
};
use {
    yinz_plugin_api::plugin_command::dump_layout_response,
    yinz_plugin_api::plugin_command::dump_session_layout_response,
    yinz_plugin_api::plugin_command::fs_list_response,
    yinz_plugin_api::plugin_command::fs_metadata_response,
    yinz_plugin_api::plugin_command::fs_mutate_response,
    yinz_plugin_api::plugin_command::fs_read_response,
    yinz_plugin_api::plugin_command::get_focused_pane_info_response,
    yinz_plugin_api::plugin_command::get_pane_cwd_response,
    yinz_plugin_api::plugin_command::get_pane_git_probe_response,
    yinz_plugin_api::plugin_command::get_pane_running_command_response,
    yinz_plugin_api::plugin_command::parse_layout_response,
    yinz_plugin_api::plugin_command::read_plugin_log_response,
    yinz_plugin_api::plugin_command::ProtobufBreakPanesToNewTabResponse,
    yinz_plugin_api::plugin_command::ProtobufBreakPanesToTabWithIdResponse,
    yinz_plugin_api::plugin_command::ProtobufBreakPanesToTabWithIndexResponse,
    yinz_plugin_api::plugin_command::ProtobufCurrentSessionLastSavedTimeResponse,
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
    yinz_plugin_api::plugin_command::ProtobufEnqueueQueueJobResponse,
    yinz_plugin_api::plugin_command::ProtobufFocusOrCreateTabResponse,
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
    yinz_plugin_api::plugin_command::ProtobufQueueStatusResponse,
    yinz_plugin_api::plugin_command::ProtobufReadPluginLogResponse,
    yinz_plugin_api::plugin_command::ProtobufRenameLayoutResponse,
    yinz_plugin_api::plugin_command::ProtobufSaveLayoutResponse,
    yinz_plugin_api::plugin_command::ProtobufSaveSessionResponse,
    yinz_plugin_api::plugin_command::ProtobufShowFloatingPanesResponse,
    yinz_plugin_api::plugin_command::ProtobufSpawnAgentResponse,
    yinz_plugin_api::plugin_command::ProtobufWarpPaneResponse,
};
use {
    yinz_plugin_api::plugin_ids::ProtobufPluginIds,
    yinz_plugin_api::plugin_ids::ProtobufYinzVersion,
};

pub use super::ui_components::*;
pub use prost::{self, *};

// -----------------------------------------------------------------------------------------
// Talking to the host: one extern, one `unsafe`, one place that knows the wire format.
// -----------------------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "yinz")]
extern "C" {
    fn host_run_plugin_command();
}

#[cfg(not(target_arch = "wasm32"))]
unsafe fn host_run_plugin_command() {}

/// The wire form of `command`. Rejects values the protocol cannot represent, such as
/// out-of-range tab ids and non-UTF-8 paths.
fn encode(command: PluginCommand) -> Result<Vec<u8>, String> {
    let kind = CommandType::from(&command);
    let protobuf: ProtobufPluginCommand = command
        .try_into()
        .map_err(|e| format!("Failed to encode the {kind} request: {e}"))?;
    Ok(protobuf.encode_to_vec())
}

/// Hand an encoded command to the host; returns once the host has run it.
fn dispatch(bytes: &[u8]) {
    object_to_stdout(bytes);
    unsafe { host_run_plugin_command() };
}

/// A command the host answers nothing to.
fn send(command: PluginCommand) {
    let bytes = encode(command).unwrap_or_else(|e| panic!("yinz-tile: {e}"));
    dispatch(&bytes);
}

/// A command the host answers. This is the one policy for a reply that is missing or
/// unreadable: `Err` naming the command and the reason - never a panic, never a hang. A
/// denied permission is a host-side log and no reply at all, so the read comes back empty
/// rather than blocking.
fn call<P: prost::Message + Default>(command: PluginCommand) -> Result<P, String> {
    let kind = CommandType::from(&command);
    dispatch(&encode(command)?);
    let bytes = command_response_bytes(kind)?;
    P::decode(bytes.as_slice()).map_err(|e| format!("Malformed {kind} response: {e}"))
}

/// `call`, then the conversion from the wire type to the SDK's.
fn call_as<T, P>(command: PluginCommand) -> Result<T, String>
where
    P: prost::Message + Default,
    T: TryFrom<P>,
    T::Error: std::fmt::Display,
{
    let kind = CommandType::from(&command);
    let protobuf: P = call(command)?;
    T::try_from(protobuf).map_err(|e| format!("Malformed {kind} response: {e}"))
}

/// For a function whose signature has no room for an error: the reason goes to the
/// plugin's stderr, which yinz logs under the plugin's name, and the caller gets the
/// neutral value - `None`, an empty collection, an empty string.
fn or_default<T: Default>(result: Result<T, String>) -> T {
    result.unwrap_or_else(|e| {
        eprintln!("yinz-tile: {e}");
        T::default()
    })
}

/// For the two bootstrap queries no permission gates and whose reply has no neutral
/// value: a missing reply is a fault in the host, and the plugin cannot run without it.
fn or_panic<T>(result: Result<T, String>) -> T {
    result.unwrap_or_else(|e| panic!("yinz-tile: {e}"))
}

// Subscription Handling

/// Subscribe to a list of [`Event`]s represented by their [`EventType`]s that will then trigger the `update` method
pub fn subscribe(event_types: &[EventType]) {
    let event_types: HashSet<EventType> = event_types.iter().cloned().collect();
    send(PluginCommand::Subscribe(event_types));
}

/// Unsubscribe to a list of [`Event`]s represented by their [`EventType`]s.
pub fn unsubscribe(event_types: &[EventType]) {
    let event_types: HashSet<EventType> = event_types.iter().cloned().collect();
    send(PluginCommand::Unsubscribe(event_types));
}

// Plugin Settings

/// Sets the plugin as selectable or unselectable to the user. Unselectable plugins might be desired when they do not accept user input.
pub fn set_selectable(selectable: bool) {
    send(PluginCommand::SetSelectable(selectable));
}

/// Shows the cursor at specific coordinates or hides it
///
/// # Arguments
/// * `cursor_position` - None to hide cursor, Some((x, y)) to show at coordinates
pub fn show_cursor(cursor_position: Option<(usize, usize)>) {
    #[cfg(test)]
    LAST_CURSOR_POSITION.with(|last| *last.borrow_mut() = Some(cursor_position));
    send(PluginCommand::ShowCursor(cursor_position));
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
    send(PluginCommand::RequestPluginPermissions(permissions.into()));
}

// Query Functions
/// Returns the unique Yinz pane ID for the plugin as well as the Yinz process id.
pub fn get_plugin_ids() -> PluginIds {
    let plugin_command = PluginCommand::GetPluginIds;
    or_panic(call_as::<PluginIds, ProtobufPluginIds>(plugin_command))
}

/// Returns the version of the running Yinz instance - can be useful to check plugin compatibility
pub fn get_yinz_version() -> String {
    let plugin_command = PluginCommand::GetYinzVersion;
    or_panic(call::<ProtobufYinzVersion>(plugin_command)).version
}

/// Generates a random human-readable name using Yinz's curated word lists.
/// Returns a name in the format AdjectiveNoun (e.g., "BraveRustacean", "ZippyWeasel").
///
/// This uses the same word lists as session name generation, providing
/// approximately 4,096 unique combinations.
pub fn generate_random_name() -> String {
    let plugin_command = PluginCommand::GenerateRandomName;
    let response = or_default(call::<ProtobufGenerateRandomNameResponse>(plugin_command));
    response.name
}

/// Dumps a layout by name and returns its KDL content as a String
///
/// Supports both built-in layouts (eg. "default", "compact", "welcome")
/// and custom layouts from the plugin's layout directory.
pub fn dump_layout(layout_name: &str) -> Result<String, String> {
    let plugin_command = PluginCommand::DumpLayout(layout_name.to_string());
    let protobuf_response: ProtobufDumpLayoutResponse = call(plugin_command)?;

    // Extract result from the oneof field
    match protobuf_response.result {
        Some(dump_layout_response::Result::LayoutContent(content)) => Ok(content),
        Some(dump_layout_response::Result::Error(error)) => Err(error),
        None => Err("Server returned empty response".to_string()),
    }
}

/// Returns the path to the layout directory.
///
/// This is the directory where Yinz looks for layout files. It can be:
/// - The directory specified via CLI `--layout-dir` flag
/// - The directory specified in the config file
/// - The directory specified via YINZ_LAYOUT_DIR env var
/// - The default: `~/.config/yinz/layouts`
///
/// # Returns
/// A String containing the absolute path to the layout directory.
/// Returns an empty string if the layout directory cannot be determined (rare edge case).
pub fn get_layout_dir() -> String {
    let plugin_command = PluginCommand::GetLayoutDir;
    let response = or_default(call::<ProtobufGetLayoutDirResponse>(plugin_command));
    response.layout_dir
}

/// The folder Yinz looks in for the user's own plugins.
///
/// # Returns
/// A String containing the absolute path to that folder.
/// Returns an empty string if it cannot be determined, in which case there is nowhere to write
/// a plugin and the caller should say so rather than pick a path of its own.
pub fn get_user_plugin_dir() -> String {
    let plugin_command = PluginCommand::GetUserPluginDir;
    let response = or_default(call::<ProtobufGetUserPluginDirResponse>(plugin_command));
    response.user_plugin_dir
}

pub fn get_session_environment_variables() -> BTreeMap<String, String> {
    let plugin_command = PluginCommand::GetSessionEnvironmentVariables;
    let response = or_default(call::<ProtobufGetSessionEnvironmentVariablesResponse>(
        plugin_command,
    ));

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
/// own configuration already arrives through [`YinzPlugin::load`], and
/// `SessionInfo.plugins` covers plugins that are currently loaded - but only those, so it
/// cannot answer for an alias whose plugin has never been launched in this session.
///
/// An alias nobody declared answers with an empty map: "nothing is configured under that
/// name", which is the same thing a declared-but-empty block means.
pub fn get_plugin_alias_configuration(alias: &str) -> BTreeMap<String, String> {
    let plugin_command = PluginCommand::GetPluginAliasConfiguration(alias.to_owned());
    let response = or_default(call::<ProtobufGetPluginAliasConfigurationResponse>(
        plugin_command,
    ));

    response.configuration.into_iter().collect()
}

/// Returns the focused pane ID and tab ID for the client associated with this plugin.
pub fn get_focused_pane_info() -> Result<(TabId, PaneId), String> {
    let plugin_command = PluginCommand::GetFocusedPaneInfo;
    let protobuf_response: ProtobufGetFocusedPaneInfoResponse = call(plugin_command)?;

    match protobuf_response.result {
        Some(get_focused_pane_info_response::Result::FocusedPaneInfo(info)) => {
            let tab_id = TabId::new(info.focused_tab_id as usize);
            match info.focused_pane_id {
                Some(pb_pane_id) => match pb_pane_id.try_into() {
                    Ok(pane_id) => Ok((tab_id, pane_id)),
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
/// This synchronously queries Yinz for detailed information about the pane with the given ID,
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
/// use yinz_tile::prelude::*;
///
/// // Query info for a specific pane
/// let pane_id = PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1));
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
    let protobuf_response = or_default(call::<ProtobufGetPaneInfoResponse>(plugin_command));

    protobuf_response
        .pane_info
        .and_then(|pb_pane_info| pb_pane_info.try_into().ok())
}

/// Query information about a specific tab by its tab ID.
///
/// This synchronously queries Yinz for detailed information about the tab with the given ID,
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
/// use yinz_tile::prelude::*;
///
/// // Query info for a specific tab
/// let tab_id = TabId::new(3);
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
pub fn get_tab_info(tab_id: TabId) -> Option<TabInfo> {
    let plugin_command = PluginCommand::GetTabInfo(tab_id);
    let protobuf_response = or_default(call::<ProtobufGetTabInfoResponse>(plugin_command));

    protobuf_response
        .tab_info
        .and_then(|pb_tab_info| pb_tab_info.try_into().ok())
}

/// Save the current session state to disk immediately.
///
/// This triggers an immediate write of the current session metadata and layout
/// to the session cache directory (~/.cache/yinz/cv1/session_info/<session_name>/).
///
/// # Returns
///
/// - `Ok(())` if the save request was successfully sent
/// - `Err(String)` if there was an error sending the request
///
/// # Example
///
/// ```no_run
/// use yinz_tile::prelude::*;
///
/// // Save the current session
/// match save_session() {
///     Ok(()) => println!("Session saved successfully"),
///     Err(e) => eprintln!("Failed to save session: {}", e),
/// }
/// ```
pub fn save_session() -> Result<(), String> {
    let plugin_command = PluginCommand::SaveSession;
    let protobuf_response: ProtobufSaveSessionResponse = call(plugin_command)?;

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
/// use yinz_tile::prelude::*;
///
/// if let Some(elapsed_millis) = current_session_last_saved_time() {
///     println!("Session was last saved {} ms ago", elapsed_millis);
/// } else {
///     println!("Session has not been saved yet");
/// }
/// ```
pub fn current_session_last_saved_time() -> Option<u64> {
    let plugin_command = PluginCommand::CurrentSessionLastSavedTime;
    let protobuf_response = or_default(call::<ProtobufCurrentSessionLastSavedTimeResponse>(
        plugin_command,
    ));

    protobuf_response.timestamp_millis
}

// Host Functions

/// Open a file in the user's default `$EDITOR` in a new pane
pub fn open_file(file_to_open: FileToOpen, context: BTreeMap<String, String>) -> Option<PaneId> {
    let plugin_command = PluginCommand::OpenFile(file_to_open, context);
    or_default(call_as::<OpenFileResponse, ProtobufOpenFileResponse>(
        plugin_command,
    ))
}

/// Open a file in the user's default `$EDITOR` in a new floating pane
pub fn open_file_floating(
    file_to_open: FileToOpen,
    coordinates: Option<FloatingPaneCoordinates>,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command = PluginCommand::OpenFileFloating(file_to_open, coordinates, context);
    or_default(call_as::<
        OpenFileFloatingResponse,
        ProtobufOpenFileFloatingResponse,
    >(plugin_command))
}

/// Open a file in the user's default `$EDITOR`, replacing the focused pane
pub fn open_file_in_place(
    file_to_open: FileToOpen,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command = PluginCommand::OpenFileInPlace(file_to_open, context);
    or_default(call_as::<
        OpenFileInPlaceResponse,
        ProtobufOpenFileInPlaceResponse,
    >(plugin_command))
}

/// Open a file in the user's default `$EDITOR` in a new pane near th eplugin
pub fn open_file_near_plugin(
    file_to_open: FileToOpen,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command = PluginCommand::OpenFileNearPlugin(file_to_open, context);
    or_default(call_as::<
        OpenFileNearPluginResponse,
        ProtobufOpenFileNearPluginResponse,
    >(plugin_command))
}

/// Open a file in the user's default `$EDITOR` in a new floating pane near the plugin
pub fn open_file_floating_near_plugin(
    file_to_open: FileToOpen,
    coordinates: Option<FloatingPaneCoordinates>,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command =
        PluginCommand::OpenFileFloatingNearPlugin(file_to_open, coordinates, context);
    or_default(call_as::<
        OpenFileFloatingNearPluginResponse,
        ProtobufOpenFileFloatingNearPluginResponse,
    >(plugin_command))
}

/// Open a file in the user's default `$EDITOR`, replacing the plugin pane
pub fn open_file_in_place_of_plugin(
    file_to_open: FileToOpen,
    close_plugin_after_replace: bool,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command =
        PluginCommand::OpenFileInPlaceOfPlugin(file_to_open, close_plugin_after_replace, context);
    or_default(call_as::<
        OpenFileInPlaceOfPluginResponse,
        ProtobufOpenFileInPlaceOfPluginResponse,
    >(plugin_command))
}
/// Open a new terminal pane to the specified location on the host filesystem
pub fn open_terminal<P: AsRef<Path>>(path: P) -> Option<PaneId> {
    let file_to_open = FileToOpen::new(path.as_ref());
    let plugin_command = PluginCommand::OpenTerminal(file_to_open);
    or_default(call_as::<OpenTerminalResponse, ProtobufOpenTerminalResponse>(plugin_command))
}

/// Open a new terminal pane to the specified location on the host filesystem
/// This variant is identical to open_terminal, excpet it opens it near the plugin regardless of
/// whether the user was focused on it or not
pub fn open_terminal_near_plugin<P: AsRef<Path>>(path: P) -> Option<PaneId> {
    let file_to_open = FileToOpen::new(path.as_ref());
    let plugin_command = PluginCommand::OpenTerminalNearPlugin(file_to_open);
    or_default(call_as::<
        OpenTerminalNearPluginResponse,
        ProtobufOpenTerminalNearPluginResponse,
    >(plugin_command))
}

/// Open a new floating terminal pane to the specified location on the host filesystem
pub fn open_terminal_floating<P: AsRef<Path>>(
    path: P,
    coordinates: Option<FloatingPaneCoordinates>,
) -> Option<PaneId> {
    let file_to_open = FileToOpen::new(path.as_ref());
    let plugin_command = PluginCommand::OpenTerminalFloating(file_to_open, coordinates);
    or_default(call_as::<
        OpenTerminalFloatingResponse,
        ProtobufOpenTerminalFloatingResponse,
    >(plugin_command))
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
    or_default(call_as::<
        OpenTerminalFloatingNearPluginResponse,
        ProtobufOpenTerminalFloatingNearPluginResponse,
    >(plugin_command))
}

/// Open a new terminal pane to the specified location on the host filesystem, temporarily
/// replacing the focused pane
pub fn open_terminal_in_place<P: AsRef<Path>>(path: P) -> Option<PaneId> {
    let file_to_open = FileToOpen::new(path.as_ref());
    let plugin_command = PluginCommand::OpenTerminalInPlace(file_to_open);
    or_default(call_as::<
        OpenTerminalInPlaceResponse,
        ProtobufOpenTerminalInPlaceResponse,
    >(plugin_command))
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
    or_default(call_as::<
        OpenTerminalInPlaceOfPluginResponse,
        ProtobufOpenTerminalInPlaceOfPluginResponse,
    >(plugin_command))
}

/// Open a new command pane with the specified command and args (this sort of pane allows the user to control the command, re-run it and see its exit status through the Yinz UI).
pub fn open_command_pane(
    command_to_run: CommandToRun,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command = PluginCommand::OpenCommandPane(command_to_run, context);
    or_default(call_as::<
        OpenCommandPaneResponse,
        ProtobufOpenCommandPaneResponse,
    >(plugin_command))
}

/// Open a new command pane with the specified command and args (this sort of pane allows the user to control the command, re-run it and see its exit status through the Yinz UI).
/// This variant is the same as `open_command_pane` except it opens the pane in the same tab as the
/// plugin regardless of whether the user is focused on it
pub fn open_command_pane_near_plugin(
    command_to_run: CommandToRun,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command = PluginCommand::OpenCommandPaneNearPlugin(command_to_run, context);
    or_default(call_as::<
        OpenCommandPaneNearPluginResponse,
        ProtobufOpenCommandPaneNearPluginResponse,
    >(plugin_command))
}

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
    let response: ProtobufOpenCommandPaneInGitCheckoutResponse = call(plugin_command)?;
    OpenCommandPaneInGitCheckoutResponse::try_from(response).map_err(|error| error.to_owned())?
}

pub fn agent_control(
    id: String,
    request: &yinz_types::agent::control::AgentRequest,
) -> Result<(), String> {
    let request = serde_json::to_string(request).map_err(|error| error.to_string())?;
    send(PluginCommand::AgentControl { id, request });
    Ok(())
}

pub fn spawn_agent(
    spec: AgentSpawnSpec,
    source_directory: Option<PathBuf>,
    cwd: Option<PathBuf>,
    name: Option<String>,
    context: BTreeMap<String, String>,
    start_suppressed: bool,
) -> Result<SpawnedAgent, String> {
    let plugin_command = PluginCommand::SpawnAgent {
        spec,
        source_directory,
        cwd,
        name,
        context,

        start_suppressed,
    };
    let response: ProtobufSpawnAgentResponse = call(plugin_command)?;
    SpawnAgentResponse::try_from(response).map_err(|error| error.to_owned())?
}

/// Open a new floating command pane with the specified command and args (this sort of pane allows the user to control the command, re-run it and see its exit status through the Yinz UI).
pub fn open_command_pane_floating(
    command_to_run: CommandToRun,
    coordinates: Option<FloatingPaneCoordinates>,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command =
        PluginCommand::OpenCommandPaneFloating(command_to_run, coordinates, context);
    or_default(call_as::<
        OpenCommandPaneFloatingResponse,
        ProtobufOpenCommandPaneFloatingResponse,
    >(plugin_command))
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
    or_default(call_as::<
        OpenCommandPaneOverlayResponse,
        ProtobufOpenCommandPaneOverlayResponse,
    >(plugin_command))
}

/// Open a new floating command pane with the specified command and args (this sort of pane allows the user to control the command, re-run it and see its exit status through the Yinz UI).
/// This variant is the same as `open_command_pane_floating` except it opens the pane in the same tab as the
/// plugin regardless of whether the user is focused on it
pub fn open_command_pane_floating_near_plugin(
    command_to_run: CommandToRun,
    coordinates: Option<FloatingPaneCoordinates>,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command =
        PluginCommand::OpenCommandPaneFloatingNearPlugin(command_to_run, coordinates, context);
    or_default(call_as::<
        OpenCommandPaneFloatingNearPluginResponse,
        ProtobufOpenCommandPaneFloatingNearPluginResponse,
    >(plugin_command))
}

/// Open a new in place command pane with the specified command and args (this sort of pane allows the user to control the command, re-run it and see its exit status through the Yinz UI).
pub fn open_command_pane_in_place(
    command_to_run: CommandToRun,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command = PluginCommand::OpenCommandPaneInPlace(command_to_run, context);
    or_default(call_as::<
        OpenCommandPaneInPlaceResponse,
        ProtobufOpenCommandPaneInPlaceResponse,
    >(plugin_command))
}

/// Open a new in place command pane with the specified command and args (this sort of pane allows the user to control the command, re-run it and see its exit status through the Yinz UI).
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

    or_default(call_as::<
        OpenCommandPaneInPlaceOfPluginResponse,
        ProtobufOpenCommandPaneInPlaceOfPluginResponse,
    >(plugin_command))
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

    or_default(call_as::<
        OpenCommandPaneInPlaceOfPaneIdResponse,
        ProtobufOpenCommandPaneInPlaceOfPaneIdResponse,
    >(plugin_command))
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
    or_default(call_as::<
        OpenTerminalPaneInPlaceOfPaneIdResponse,
        ProtobufOpenTerminalPaneInPlaceOfPaneIdResponse,
    >(plugin_command))
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

    or_default(call_as::<
        OpenEditPaneInPlaceOfPaneIdResponse,
        ProtobufOpenEditPaneInPlaceOfPaneIdResponse,
    >(plugin_command))
}

/// Open a new hidden (background) command pane with the specified command and args (this sort of pane allows the user to control the command, re-run it and see its exit status through the Yinz UI).
pub fn open_command_pane_background(
    command_to_run: CommandToRun,
    context: BTreeMap<String, String>,
) -> Option<PaneId> {
    let plugin_command = PluginCommand::OpenCommandPaneBackground(command_to_run, context);
    or_default(call_as::<
        OpenCommandPaneBackgroundResponse,
        ProtobufOpenCommandPaneBackgroundResponse,
    >(plugin_command))
}

/// Change the focused tab to the specified index (corresponding with the default tab names, to starting at `1`, `0` will be considered as `1`).
pub fn switch_tab_to(tab_idx: TabNumber) {
    send(PluginCommand::SwitchTabTo(tab_idx));
}

/// Set a timeout in seconds (or fractions thereof) after which the plugins [update](./plugin-api-events#update) method will be called with the [`Timer`](./plugin-api-events.md#timer) event.
pub fn set_timeout(secs: FractionalSeconds) {
    send(PluginCommand::SetTimeout(secs));
}

#[doc(hidden)]
pub fn exec_cmd(cmd: &[&str]) {
    let plugin_command =
        PluginCommand::ExecCmd(cmd.iter().cloned().map(|s| s.to_owned()).collect());
    send(plugin_command);
}

/// Run this command in the background on the host machine, optionally being notified of its output
/// if subscribed to the `RunCommandResult` Event
pub fn run_command(cmd: &[&str], context: BTreeMap<String, String>) {
    send(PluginCommand::RunCommand(
        cmd.iter().cloned().map(|s| s.to_owned()).collect(),
        BTreeMap::new(),
        PathBuf::from("."),
        context,
        None,
        None,
    ));
}

/// Run this command in the background on the host machine, providing environment variables and a
/// cwd. Optionally being notified of its output if subscribed to the `RunCommandResult` Event
pub fn run_command_with_env_variables_and_cwd(
    cmd: &[&str],
    env_variables: BTreeMap<String, String>,
    cwd: PathBuf,
    context: BTreeMap<String, String>,
) {
    send(PluginCommand::RunCommand(
        cmd.iter().cloned().map(|s| s.to_owned()).collect(),
        env_variables,
        cwd,
        context,
        None,
        None,
    ));
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
    send(PluginCommand::RunCommand(
        cmd.iter().cloned().map(|s| s.to_owned()).collect(),
        env_variables,
        cwd,
        context,
        Some(max_output_bytes),
        None,
    ));
}

/// Stream stdout/stderr as `RunCommandOutput`, then receive `RunCommandResult` after drain.
/// Subscribe to both events before calling. The completion output is capped per stream.
pub fn run_command_stream(
    cmd: &[&str],
    env_variables: BTreeMap<String, String>,
    cwd: PathBuf,
    context: BTreeMap<String, String>,
    max_output_bytes: usize,
) {
    let command = PluginCommand::RunCommand(
        cmd.iter().map(|s| (*s).to_owned()).collect(),
        env_variables,
        cwd,
        context,
        Some(max_output_bytes),
        Some(true),
    );
    send(command);
}

/// Observe terminal output matching a nonempty command context. `None` removes the
/// subscription without stopping any process. Subscribe to `PaneOutput` before dispatch.
pub fn subscribe_pane_output(context: Option<BTreeMap<String, String>>) {
    send(PluginCommand::SubscribePaneOutput(context));
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
    send(plugin_command);
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
    send(PluginCommand::WebRequest(
        url.to_string(),
        verb,
        headers,
        body,
        context,
        Some(stream_id.to_string()),
        proxy,
    ));
}

/// Cancel an in-flight [`web_request_stream`]. A final `WebRequestStreamChunk` with `done: true`
/// and `error: Some(_)` is still delivered. Cancelling an unknown or already finished stream is
/// a no-op.
pub fn cancel_web_request<S: ToString>(stream_id: S) {
    send(PluginCommand::CancelWebRequest(stream_id.to_string()));
}

/// Hide the plugin pane (suppress it) from the UI
pub fn hide_self() {
    send(PluginCommand::HideSelf);
}

/// Hide the pane (suppress it) with the specified [PaneId] from the UI
pub fn hide_pane_with_id(pane_id: PaneId) {
    send(PluginCommand::HidePaneWithId(pane_id));
}

/// Show the plugin pane (unsuppress it if it is suppressed), focus it and switch to its tab
pub fn show_self(should_float_if_hidden: bool) {
    send(PluginCommand::ShowSelf(should_float_if_hidden));
}

/// Show the pane (unsuppress it if it is suppressed) with the specified [PaneId], focus it and switch to its tab
pub fn show_pane_with_id(pane_id: PaneId, should_float_if_hidden: bool, should_focus_pane: bool) {
    let plugin_command =
        PluginCommand::ShowPaneWithId(pane_id, should_float_if_hidden, should_focus_pane);
    send(plugin_command);
}

/// Close this plugin pane
pub fn close_self() {
    send(PluginCommand::CloseSelf);
}

/// Switch to the specified Input Mode (eg. `Normal`, `Tab`, `Pane`)
pub fn switch_to_input_mode(mode: &InputMode) {
    send(PluginCommand::SwitchToMode(*mode));
}

/// Provide a stringified layout to be applied to the current session. If the layout has multiple tabs, they will all be opened.
/// Requires `ChangeApplicationState` plus the permission for every command, editor or plugin pane
/// the layout starts.
pub fn new_tabs_with_layout(layout: &str) -> Vec<TabId> {
    let plugin_command = PluginCommand::NewTabsWithLayout(layout.to_owned());
    or_default(call_as::<NewTabsResponse, ProtobufNewTabsResponse>(
        plugin_command,
    ))
}

/// Provide a LayoutInfo to be applied to the current session in a new tab. If the layout has multiple tabs, they will all be opened.
/// File layouts are confined to the layout directory and require `OpenFiles`, `RunCommands` and
/// `OpenTerminalsOrPlugins`. Stringified and built-in layouts require only the capabilities
/// they actually start.
pub fn new_tabs_with_layout_info<L: AsRef<LayoutInfo>>(layout_info: L) -> Vec<TabId> {
    let plugin_command = PluginCommand::NewTabsWithLayoutInfo(layout_info.as_ref().clone());
    or_default(call_as::<NewTabsResponse, ProtobufNewTabsResponse>(
        plugin_command,
    ))
}

/// Open a new tab with the default layout
pub fn new_tab<S>(name: Option<S>, cwd: Option<S>) -> Option<TabId>
where
    S: AsRef<str> + ToString,
{
    let name = name.map(|s| s.to_string());
    let cwd = cwd.map(|s| s.to_string());
    let plugin_command = PluginCommand::NewTab { name, cwd };
    or_default(call_as::<NewTabResponse, ProtobufNewTabResponse>(
        plugin_command,
    ))
}

pub fn new_tab_unfocused<S>(name: Option<S>, cwd: Option<S>) -> Option<TabId>
where
    S: AsRef<str> + ToString,
{
    let name = name.map(|s| s.to_string());
    let cwd = cwd.map(|s| s.to_string());
    let plugin_command = PluginCommand::NewTabUnfocused { name, cwd };
    or_default(call_as::<
        NewTabUnfocusedResponse,
        ProtobufNewTabUnfocusedResponse,
    >(plugin_command))
}

pub fn new_tiled_pane_in_tab(tab_id: TabId) -> Option<PaneId> {
    let plugin_command = PluginCommand::NewTiledPaneInTab { tab_id };
    or_default(call_as::<
        NewTiledPaneInTabResponse,
        ProtobufNewTiledPaneInTabResponse,
    >(plugin_command))
}

/// Opens a new tab with a command pane running `command_to_run`.
/// Returns `(tab_id, pane_id)` of the created tab and pane, or `None` if unavailable.
pub fn open_command_pane_in_new_tab(
    command_to_run: CommandToRun,
    context: BTreeMap<String, String>,
) -> (Option<TabId>, Option<PaneId>) {
    let plugin_command = PluginCommand::OpenCommandPaneInNewTab(command_to_run, context);
    or_default(
        call_as::<OpenPaneInNewTabResponse, ProtobufOpenPaneInNewTabResponse>(plugin_command)
            .map(|result| (result.tab_id, result.pane_id)),
    )
}

/// Opens a new tab with a plugin pane loaded from `plugin_url`.
/// `plugin_url` can be a path (`file:/path/to/plugin.wasm`) or a named alias.
/// Returns `(tab_id, pane_id)` of the created tab and pane.
pub fn open_plugin_pane_in_new_tab(
    plugin_url: impl ToString,
    configuration: BTreeMap<String, String>,
    context: BTreeMap<String, String>,
) -> (Option<TabId>, Option<PaneId>) {
    let plugin_command = PluginCommand::OpenPluginPaneInNewTab {
        plugin_url: plugin_url.to_string(),
        configuration,
        context,
    };

    or_default(
        call_as::<OpenPaneInNewTabResponse, ProtobufOpenPaneInNewTabResponse>(plugin_command)
            .map(|result| (result.tab_id, result.pane_id)),
    )
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

    or_default(call_as::<
        OpenPluginPaneFloatingResponse,
        ProtobufOpenPluginPaneFloatingResponse,
    >(plugin_command))
}

/// Opens a new tab with an editor pane for `file_to_open`.
/// Returns `(tab_id, pane_id)` of the created tab and pane.
pub fn open_editor_pane_in_new_tab(
    file_to_open: FileToOpen,
    context: BTreeMap<String, String>,
) -> (Option<TabId>, Option<PaneId>) {
    let plugin_command = PluginCommand::OpenEditorPaneInNewTab(file_to_open, context);
    or_default(
        call_as::<OpenPaneInNewTabResponse, ProtobufOpenPaneInNewTabResponse>(plugin_command)
            .map(|result| (result.tab_id, result.pane_id)),
    )
}

/// Change focus to the next tab or loop back to the first
pub fn go_to_next_tab() {
    send(PluginCommand::GoToNextTab);
}

/// Change focus to the previous tab or loop back to the last
pub fn go_to_previous_tab() {
    send(PluginCommand::GoToPreviousTab);
}

pub fn report_panic(info: &std::panic::PanicHookInfo) {
    let panic_payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
        s.to_string()
    } else {
        "<NO PAYLOAD>".to_string()
    };
    let panic_stringified = format!("{}\n\r{:#?}", panic_payload, info).replace("\n", "\r\n");
    send(PluginCommand::ReportPanic(panic_stringified));
}

/// Either Increase or Decrease the size of the focused pane
pub fn resize_focused_pane(resize: Resize) {
    send(PluginCommand::Resize(resize));
}

/// Either Increase or Decrease the size of the focused pane in a specified direction (eg. `Left`, `Right`, `Up`, `Down`).
pub fn resize_focused_pane_with_direction(resize: Resize, direction: Direction) {
    let resize_strategy = ResizeStrategy {
        resize,
        direction: Some(direction),
        invert_on_boundaries: false,
    };
    send(PluginCommand::ResizeWithDirection(resize_strategy));
}

/// Change focus tot he next pane in chronological order
pub fn focus_next_pane() {
    send(PluginCommand::FocusNextPane);
}

/// Change focus to the previous pane in chronological order
pub fn focus_previous_pane() {
    send(PluginCommand::FocusPreviousPane);
}

/// Change focus to the previously focused pane
pub fn focus_last_pane() {
    send(PluginCommand::FocusLastPane);
}

/// Change the focused pane in the specified direction
pub fn move_focus(direction: Direction) {
    send(PluginCommand::MoveFocus(direction));
}

/// Change the focused pane in the specified direction, if the pane is on the edge of the screen, the next tab is focused (next if right edge, previous if left edge).
pub fn move_focus_or_tab(direction: Direction) {
    send(PluginCommand::MoveFocusOrTab(direction));
}

/// Detach the user from the active session
pub fn detach() {
    send(PluginCommand::Detach);
}

/// Edit the scrollback of the focused pane in the user's default `$EDITOR`
pub fn edit_scrollback() {
    send(PluginCommand::EditScrollback);
}

/// Write bytes to the `STDIN` of the focused pane
pub fn write(bytes: Vec<u8>) {
    send(PluginCommand::Write(bytes));
}

/// Write characters to the `STDIN` of the focused pane
pub fn write_chars(chars: &str) {
    send(PluginCommand::WriteChars(chars.to_owned()));
}

/// Copy arbitrary text to the user's clipboard
///
/// Respects the user's configured clipboard destination (system clipboard or primary selection).
/// Requires the WriteToClipboard permission.
pub fn copy_to_clipboard(text: impl Into<String>) {
    send(PluginCommand::CopyToClipboard(text.into()));
}

/// Focused the previously focused tab (regardless of the tab position)
pub fn toggle_tab() {
    send(PluginCommand::ToggleTab);
}

/// Switch the position of the focused pane with a different pane
pub fn move_pane() {
    send(PluginCommand::MovePane);
}

/// Switch the position of the focused pane with a different pane in the specified direction (eg. `Down`, `Up`, `Left`, `Right`).
pub fn move_pane_with_direction(direction: Direction) {
    send(PluginCommand::MovePaneWithDirection(direction));
}

/// Clear the scroll buffer of the focused pane
pub fn clear_screen() {
    send(PluginCommand::ClearScreen);
}

/// Scroll the focused pane up 1 line
pub fn scroll_up() {
    send(PluginCommand::ScrollUp);
}

/// Scroll the focused pane down 1 line
pub fn scroll_down() {
    send(PluginCommand::ScrollDown);
}

/// Scroll the focused pane all the way to the top of the scrollbuffer
pub fn scroll_to_top() {
    send(PluginCommand::ScrollToTop);
}

/// Scroll the focused pane all the way to the bottom of the scrollbuffer
pub fn scroll_to_bottom() {
    send(PluginCommand::ScrollToBottom);
}

/// Scroll the focused pane up one page
pub fn page_scroll_up() {
    send(PluginCommand::PageScrollUp);
}

/// Scroll the focused pane down one page
pub fn page_scroll_down() {
    send(PluginCommand::PageScrollDown);
}

/// Toggle the focused pane to be fullscreen or normal sized
pub fn toggle_focus_fullscreen() {
    send(PluginCommand::ToggleFocusFullscreen);
}

pub fn toggle_focus_no_ui_fullscreen() {
    send(PluginCommand::ToggleFocusNoUiFullscreen);
}

pub fn focus_host_session() {
    send(PluginCommand::FocusHostSession);
}

/// Toggle the UI pane frames on or off
pub fn toggle_pane_frames() {
    send(PluginCommand::TogglePaneFrames);
}

pub fn set_pane_frame_style(pane_frame_style: PaneFrameStyle) {
    send(PluginCommand::SetPaneFrameStyle(pane_frame_style));
}

/// Embed the currently focused pane (make it stop floating) or turn it to a float pane if it is not
pub fn toggle_pane_embed_or_eject() {
    send(PluginCommand::TogglePaneEmbedOrEject);
}

pub fn undo_rename_pane() {
    send(PluginCommand::UndoRenamePane);
}

/// Close the focused pane
pub fn close_focus() {
    send(PluginCommand::CloseFocus);
}

pub fn new_pane() {
    send(PluginCommand::NewPane);
}

pub fn toggle_floating_panes(tab_id: Option<TabId>) {
    send(PluginCommand::ToggleFloatingPanes { tab_id });
}

/// Turn the `STDIN` synchronization of the current tab on or off
pub fn toggle_active_tab_sync() {
    send(PluginCommand::ToggleActiveTabSync);
}

/// Close the focused tab
pub fn close_focused_tab() {
    send(PluginCommand::CloseFocusedTab);
}

pub fn undo_rename_tab() {
    send(PluginCommand::UndoRenameTab);
}

/// Compeltely quit Yinz for this and all other connected clients
pub fn quit_yinz() {
    send(PluginCommand::QuitYinz);
}

/// Change to the previous swap layout
pub fn previous_swap_layout() {
    send(PluginCommand::PreviousSwapLayout);
}

/// Change to the next swap layout
pub fn next_swap_layout() {
    send(PluginCommand::NextSwapLayout);
}

/// Change focus to the tab with the specified name
pub fn go_to_tab_name(tab_name: &str) {
    send(PluginCommand::GoToTabName(tab_name.to_owned()));
}

/// Change focus to the tab with the specified name or create it if it does not exist
pub fn focus_or_create_tab(tab_name: &str) -> Option<TabId> {
    let plugin_command = PluginCommand::FocusOrCreateTab(tab_name.to_owned());
    or_default(call_as::<
        FocusOrCreateTabResponse,
        ProtobufFocusOrCreateTabResponse,
    >(plugin_command))
}

pub fn go_to_tab(tab_index: TabPosition) {
    send(PluginCommand::GoToTab(tab_index));
}

/// Grant `permissions` to the plugin at `plugin_url` before it asks, as if the user had answered
/// its prompt. Only a builtin plugin may call this; for anything else the host refuses and logs.
/// The grant is remembered in the permissions cache and applied to any running instance.
pub fn grant_plugin_permissions(plugin_url: &str, permissions: &[PermissionType]) {
    send(PluginCommand::GrantPluginPermissions {
        plugin_url: plugin_url.to_owned(),
        permissions: permissions.to_vec(),
    });
}

pub fn start_or_reload_plugin(url: &str) {
    send(PluginCommand::StartOrReloadPlugin(url.to_owned()));
}

/// Closes a terminal pane with the specified id
pub fn close_terminal_pane(terminal_pane_id: yinz_types::internal_ids::TerminalId) {
    send(PluginCommand::CloseTerminalPane(terminal_pane_id));
}

/// Closes a plugin pane with the specified id
pub fn close_plugin_pane(plugin_pane_id: yinz_types::internal_ids::PluginId) {
    send(PluginCommand::ClosePluginPane(plugin_pane_id));
}

/// Changes the focus to the terminal pane with the specified id, unsuppressing it if it was suppressed and switching to its tab and layer (eg. floating/tiled).
pub fn focus_terminal_pane(
    terminal_pane_id: yinz_types::internal_ids::TerminalId,
    should_float_if_hidden: bool,
    should_be_in_place_if_hidden: bool,
) {
    send(PluginCommand::FocusTerminalPane(
        terminal_pane_id,
        should_float_if_hidden,
        should_be_in_place_if_hidden,
    ));
}

/// Changes the focus to the plugin pane with the specified id, unsuppressing it if it was suppressed and switching to its tab and layer (eg. floating/tiled).
pub fn focus_plugin_pane(
    plugin_pane_id: yinz_types::internal_ids::PluginId,
    should_float_if_hidden: bool,
    should_be_in_place_if_hidden: bool,
) {
    send(PluginCommand::FocusPluginPane(
        plugin_pane_id,
        should_float_if_hidden,
        should_be_in_place_if_hidden,
    ));
}

/// Changes the name (the title that appears in the UI) of the terminal pane with the specified id.
pub fn rename_terminal_pane<S>(terminal_pane_id: yinz_types::internal_ids::TerminalId, new_name: S)
where
    S: AsRef<str> + ToString,
{
    send(PluginCommand::RenameTerminalPane(
        terminal_pane_id,
        new_name.to_string(),
    ));
}

/// Changes the name (the title that appears in the UI) of the plugin pane with the specified id.
pub fn rename_plugin_pane<S>(plugin_pane_id: yinz_types::internal_ids::PluginId, new_name: S)
where
    S: AsRef<str> + ToString,
{
    send(PluginCommand::RenamePluginPane(
        plugin_pane_id,
        new_name.to_string(),
    ));
}

/// Changes the name (the title that appears in the UI) of the tab with the specified position.
///
/// `tab_position` is the 0-based display position, matching [`go_to_tab`]. A position
/// shifts whenever a tab closes; use [`rename_tab_with_id`] to name a tab that stays
/// the same one.
pub fn rename_tab<S>(tab_position: TabPosition, new_name: S)
where
    S: AsRef<str> + ToString,
{
    send(PluginCommand::RenameTab(tab_position, new_name.to_string()));
}

/// Changes the name (the title that appears in the UI) of the tab with the specified id.
pub fn rename_tab_with_id<S>(tab_id: TabId, new_name: S)
where
    S: AsRef<str> + ToString,
{
    send(PluginCommand::RenameTabWithId(tab_id, new_name.to_string()));
}

/// Arrange existing terminals by project; requires ChangeApplicationState permission.
pub fn arrange_project_panes(groups: Vec<ProjectPaneGroup>) {
    send(PluginCommand::ArrangeProjectPanes(groups));
}

/// Replace the registered project names and pane memberships for this session.
pub fn set_project_context(projects: Vec<String>, panes: Vec<(PaneId, String)>) {
    send(PluginCommand::SetProjectContext(projects, panes));
}

/// Tells the host which project each tab is working on, keyed by tab id.
///
/// Not a rename: the name is shown in place of the tab's default "Tab #N", and a
/// tab the user named keeps that name. Two tabs on the same project are told
/// apart with a `#1`/`#2` suffix. The map is the whole truth each time it is
/// sent, so a tab left out of it goes back to its own name.
pub fn set_tab_projects(tab_projects: BTreeMap<TabId, String>) {
    send(PluginCommand::SetTabProjects(tab_projects));
}

/// Switch to a session with the given name, create one if no name is given
pub fn switch_session(name: Option<&SessionName>) {
    send(PluginCommand::SwitchSession(ConnectToSession {
        name: name.cloned(),
        ..Default::default()
    }));
}

/// Switch to a session with the given name, creating one from `layout` if needed. Layout
/// permissions follow [`new_tabs_with_layout_info`].
pub fn switch_session_with_layout(
    name: Option<&SessionName>,
    layout: LayoutInfo,
    cwd: Option<PathBuf>,
) {
    send(PluginCommand::SwitchSession(ConnectToSession {
        name: name.cloned(),
        layout: Some(layout),
        cwd,
        ..Default::default()
    }));
}

/// Switch to a session with the given name, create one if no name is given
pub fn switch_session_with_cwd(name: Option<&SessionName>, cwd: Option<PathBuf>) {
    send(PluginCommand::SwitchSession(ConnectToSession {
        name: name.cloned(),
        cwd,
        ..Default::default()
    }));
}

/// Switch to a session with the given name, focusing either the provided pane_id or the provided
/// tab position (in that order)
pub fn switch_session_with_focus(
    name: &SessionName,
    tab_position: Option<TabPosition>,
    pane_id: Option<yinz_types::pane_id::PaneId>,
) {
    send(PluginCommand::SwitchSession(ConnectToSession {
        name: Some(name.clone()),
        tab_position,
        pane_id,
        ..Default::default()
    }));
}

/// Permanently delete a resurrectable session with the given name.
///
/// Returns `Ok(())` if the session's cache directory was removed; `Err(...)`
/// if the underlying `remove_dir_all` failed (e.g. permissions, missing path).
pub fn delete_dead_session(name: &SessionName) -> Result<(), String> {
    let plugin_command = PluginCommand::DeleteDeadSessionAndReply(name.clone());
    let protobuf_response: ProtobufDeleteDeadSessionResponse = call(plugin_command)?;
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
    let protobuf_response: ProtobufDeleteAllDeadSessionsResponse = call(plugin_command)?;
    match protobuf_response.error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Rename the current session
pub fn rename_session(name: &SessionName) {
    send(PluginCommand::RenameSession(name.clone()));
}

/// Show a notification that dismisses itself after a timeout. Its corner and timeout come from the
/// `notification_position` and `notification_timeout` config options.
pub fn notify(text: &str) {
    send(PluginCommand::Notify(text.to_owned(), None, None));
}

/// Show a notification that focuses `pane_id` when clicked.
pub fn notify_with_pane_id(text: &str, pane_id: PaneId) {
    send(PluginCommand::Notify(text.to_owned(), Some(pane_id), None));
}

/// Show a notification that focuses `pane_id` when clicked *and* tells this plugin that it was,
/// as an `Event::CustomMessage(message, payload)`.
///
/// The focus alone cannot be told apart from any other way into the pane, so a plugin that has to
/// do something more particular than "be looked at" - open the conversation the toast was about,
/// say - wants this rather than [`notify_with_pane_id`]. Requires a subscription to
/// `EventType::CustomMessage`; the message arrives after the focus has landed.
pub fn notify_with_click(text: &str, pane_id: PaneId, message: &str, payload: &str) {
    send(PluginCommand::Notify(
        text.to_owned(),
        Some(pane_id),
        Some(NotifyClick {
            message: message.to_owned(),
            payload: payload.to_owned(),
        }),
    ));
}

/// Unblock the input side of a pipe, requesting the next message be sent if there is one
pub fn unblock_cli_pipe_input(pipe_name: &str) {
    send(PluginCommand::UnblockCliPipeInput(pipe_name.to_owned()));
}

/// Block the input side of a pipe, will only be released once this or another plugin unblocks it
pub fn block_cli_pipe_input(pipe_name: &str) {
    send(PluginCommand::BlockCliPipeInput(pipe_name.to_owned()));
}

/// Send output to the output side of a pipe, ths does not affect the input side of same pipe
pub fn cli_pipe_output(pipe_name: &str, output: &str) {
    send(PluginCommand::CliPipeOutput(
        pipe_name.to_owned(),
        output.to_owned(),
    ));
}

/// Send a message to a plugin, it will be launched if it is not already running
pub fn pipe_message_to_plugin(message_to_plugin: MessageToPlugin) {
    send(PluginCommand::MessageToPlugin(message_to_plugin));
}

/// Disconnect all other clients from the current session
pub fn disconnect_other_clients() {
    send(PluginCommand::DisconnectOtherClients);
}

/// Kill all Yinz sessions in the list.
///
/// Waits for each peer session to acknowledge the kill (over its own IPC
/// socket -- the server-side handler reads back the `Exit` message the peer
/// sends as part of its shutdown, or a stream-close if the peer dies first).
/// Returns once every kill has been confirmed or one has failed. A wedged
/// peer is bounded by a short server-side wedge timeout.
pub fn kill_sessions(session_names: &[SessionName]) -> Result<(), String> {
    let plugin_command = PluginCommand::KillSessionsAndReply(session_names.to_vec());
    let protobuf_response: ProtobufKillSessionsResponse = call(plugin_command)?;
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
    to_session: &SessionName,
    placement: WarpPlacement,
) -> Result<String, String> {
    warp_request(PluginCommand::WarpPaneToSession {
        pane_id,
        to_session: to_session.clone(),
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
    from_session: &SessionName,
    placement: WarpPlacement,
) -> Result<String, String> {
    warp_request(PluginCommand::WarpPaneFromSession {
        pane_id,
        from_session: from_session.clone(),
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
/// `tab_id` is the tab's **stable id**, not its display
/// position: `Screen` keys its tabs by id and looks this value up there directly.
/// Passing a position warps whichever tab happens to own that number as an id,
/// which stops being the same tab as soon as the session closes one.
///
/// # Permissions Required
/// * `ChangeApplicationState`
pub fn warp_tab_to_session(tab_id: TabId, to_session: &SessionName) -> Result<String, String> {
    warp_request(PluginCommand::WarpTabToSession {
        tab_id,
        to_session: to_session.clone(),
    })
}

/// Bring a tab from another session into this one.
///
/// The mirror of [`warp_tab_to_session`]: the source session does the work, so
/// `tab_id` is a stable tab id in *that* session's numbering, not this one's,
/// and not a display position in either.
///
/// # Permissions Required
/// * `ChangeApplicationState`
pub fn warp_tab_from_session(tab_id: TabId, from_session: &SessionName) -> Result<String, String> {
    warp_request(PluginCommand::WarpTabFromSession {
        tab_id,
        from_session: from_session.clone(),
    })
}

/// Take an orphaned pane over into this session, reconnecting it to its
/// still-running process. `host_id` comes from [`list_orphaned_panes`].
///
/// # Permissions Required
/// * `ChangeApplicationState`
pub fn adopt_orphaned_pane(
    host_id: &SupervisorId,
    placement: WarpPlacement,
) -> Result<String, String> {
    warp_request(PluginCommand::AdoptOrphanedPane {
        host_id: host_id.clone(),
        placement,
    })
}

fn warp_request(plugin_command: PluginCommand) -> Result<String, String> {
    match call_as::<WarpPaneResponse, ProtobufWarpPaneResponse>(plugin_command)? {
        WarpPaneResponse::Ok(message) => Ok(message),
        WarpPaneResponse::Err(error) => Err(error),
    }
}

/// Panes whose session died but whose process is still running, waiting to be
/// recovered with [`adopt_orphaned_pane`].
///
/// Empty unless a yinz server was killed rather than shut down. Panes that are
/// merely in another *live* session are not listed here — those move with
/// [`warp_pane_from_session`].
///
/// # Permissions Required
/// * `ReadApplicationState`
pub fn list_orphaned_panes() -> Result<Vec<OrphanedPane>, String> {
    let plugin_command = PluginCommand::ListOrphanedPanes;
    let protobuf_response: ProtobufListOrphanedPanesResponse = call(plugin_command)?;
    match ListOrphanedPanesResponse::try_from(protobuf_response) {
        Ok(ListOrphanedPanesResponse::Ok(panes)) => Ok(panes),
        Ok(ListOrphanedPanesResponse::Err(error)) => Err(error),
        Err(e) => Err(format!("Malformed ListOrphanedPanes response: {}", e)),
    }
}

// ------------------------------------------------------------------ data store

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
/// use yinz_tile::prelude::*;
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
    let response: DataDefineResponse =
        call_as::<_, ProtobufDataDefineResponse>(PluginCommand::DataDefineEntity {
            namespace: namespace.map(str::to_owned),
            def: def.clone(),
            allow_destructive,
        })?;
    match response {
        DataDefineResponse::Ok(info) => Ok(info),
        DataDefineResponse::Err(error) => Err(error),
    }
}

/// Apply a batch of writes in one transaction, all or nothing.
///
/// ```no_run
/// use yinz_tile::prelude::*;
///
/// fn main() -> Result<(), String> {
/// let cutoff = 0_i64;
/// data_apply(None, vec![
///     Mutation::put("agent").uid("a1").set("project", "yinz").build(),
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
) -> Result<ApplyReport, WriteError> {
    for mutation in &mutations {
        mutation
            .validate()
            .map_err(|e| WriteError::Failed(e.to_string()))?;
    }
    let response: DataApplyResponse =
        call_as::<_, ProtobufDataApplyResponse>(PluginCommand::DataApply {
            namespace: namespace.map(str::to_owned),
            mutations,
        })
        .map_err(WriteError::Failed)?;
    match response {
        DataApplyResponse::Ok(report) => Ok(report),
        DataApplyResponse::Conflict(message) => Err(WriteError::Conflict(message)),
        DataApplyResponse::Err(error) => Err(WriteError::Failed(error)),
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
    let response: DataFetchResponse =
        call_as::<_, ProtobufDataFetchResponse>(PluginCommand::DataFetch {
            request: request.clone(),
        })?;
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
    let response: DataCountResponse =
        call_as::<_, ProtobufDataCountResponse>(PluginCommand::DataCount {
            request: request.clone(),
        })?;
    match response {
        DataCountResponse::Ok(count) => Ok(count),
        DataCountResponse::Err(error) => Err(error),
    }
}

/// What entities exist, and what their columns are.
///
/// `None` asks for everything this plugin may see: its own namespace, plus every `shared:` one if
/// it holds `SharedPluginData`. Another plugin's private namespace is never listed, whatever the
/// permissions. `Some(namespace)` asks about one `shared:` namespace.
///
/// This is how a plugin finds data another plugin publishes without being told the schema out of
/// band — and note that reading an entity someone else declared needs no declaration of your own:
/// name the namespace and fetch.
///
/// # Permissions Required
///
/// `PermissionType::PluginData`, plus `PermissionType::SharedPluginData` for a `shared:` namespace.
pub fn data_catalog(namespace: Option<&str>) -> Result<Vec<DataEntityInfo>, String> {
    let response: DataCatalogResponse =
        call_as::<_, ProtobufDataCatalogResponse>(PluginCommand::DataCatalog {
            namespace: namespace.map(str::to_owned),
        })?;
    match response {
        DataCatalogResponse::Ok(entities) => Ok(entities),
        DataCatalogResponse::Err(error) => Err(error),
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
    let response: DataWatchResponse =
        call_as::<_, ProtobufDataWatchResponse>(PluginCommand::DataWatch {
            request: request.clone(),
            mode,
        })?;
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
pub fn data_unwatch(watch_id: DataWatchId) -> Result<(), String> {
    let response: DataUnwatchResponse =
        call_as::<_, ProtobufDataUnwatchResponse>(PluginCommand::DataUnwatch { watch_id })?;
    match response {
        DataUnwatchResponse::Ok => Ok(()),
        DataUnwatchResponse::Err(error) => Err(error),
    }
}

/// Terminal operations on a [`FetchRequest`], so that a query reads as one expression.
///
/// An extension trait rather than inherent methods because `FetchRequest` is defined in
/// `yinz-plugin-api`, which cannot reach the host-call shims here.
///
/// ```no_run
/// use yinz_tile::prelude::*;
///
/// fn main() -> Result<(), String> {
/// let rows = FetchRequest::entity("agent")
///     .filter(Pred::eq("project", "yinz"))
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
    send(PluginCommand::ScanHostFolder(
        folder_to_scan.as_ref().to_path_buf(),
    ));
}

/// Start watching the host folder for filesystem changes (Note: somewhat unstable at the time
/// being)
pub fn watch_filesystem() {
    send(PluginCommand::WatchFilesystem);
}

/// Get the serialized session layout in KDL format synchronously
/// note: this removes the requesting plugin from the dumped layout
pub fn dump_session_layout() -> Result<(String, Option<LayoutMetadata>), String> {
    dump_session_layout_impl(None)
}

/// Get the serialized layout for a specific tab in KDL format synchronously
/// note: this removes the requesting plugin from the dumped layout
pub fn dump_session_layout_for_tab(
    tab_index: TabId,
) -> Result<(String, Option<LayoutMetadata>), String> {
    dump_session_layout_impl(Some(tab_index))
}

fn dump_session_layout_impl(
    tab_index: Option<TabId>,
) -> Result<(String, Option<LayoutMetadata>), String> {
    let plugin_command = PluginCommand::DumpSessionLayout { tab_index };
    let protobuf_response: ProtobufDumpSessionLayoutResponse = call(plugin_command)?;

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

/// Seconds east of UTC where this session is running, at the instant `epoch_ms`.
///
/// The one way a plugin can draw an absolute date or time. Wasm has no timezone: WASI inherits
/// none of the session's environment and nothing that reads a zoneinfo database can be linked
/// into a plugin, so the offset has to come from the server, which is the side that has
/// `chrono`.
///
/// Asked for an instant rather than for "now", because a timestamp is usually drawn long after
/// it was taken: a run that began before a daylight-saving change wants the offset that was in
/// force then, not today's.
///
/// The offset is the *server's*, which is where the session runs rather than necessarily where
/// the person looking at it is - the same clock a plugin's own `SystemTime::now()` already
/// reads, so a session attached from another timezone reads times as the host keeps them.
///
/// `Err` where the round trip failed, which includes a host that answered nothing. A caller
/// drawing a timestamp should fall back to `0` - UTC, stated as such - rather than refuse to
/// draw one.
pub fn get_local_utc_offset(epoch_ms: SignedUnixMillis) -> Result<i32, String> {
    let plugin_command = PluginCommand::GetLocalUtcOffset { epoch_ms };
    let response: ProtobufGetLocalUtcOffsetResponse = call(plugin_command)?;
    Ok(response.offset_seconds)
}

/// Check proposed JS/TS source without executing it. Semantic checks read local imports.
pub fn check_plugin(
    name: &str,
    source: &str,
    mode: yinz_types::authoring::PluginCheckMode,
) -> Result<yinz_types::authoring::PluginCheckReport, String> {
    use yinz_plugin_api::plugin_command::{check_plugin_response, ProtobufCheckPluginResponse};
    let response: ProtobufCheckPluginResponse = call(PluginCommand::CheckPlugin {
        name: name.to_owned(),
        source: source.to_owned(),
        mode,
    })?;
    match response.result {
        Some(check_plugin_response::Result::ReportJson(json)) => {
            serde_json::from_str(&json).map_err(|e| e.to_string())
        },
        Some(check_plugin_response::Result::Error(error)) => Err(error),
        None => Err("yinz answered with nothing".into()),
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

fn command_response_bytes(kind: CommandType) -> Result<Vec<u8>, String> {
    #[cfg(test)]
    if let Some(response) = TEST_COMMAND_RESPONSE.with(|response| response.borrow_mut().take()) {
        return response.map_err(|e| format!("Failed to read the {kind} response: {e}"));
    }
    bytes_from_stdin().map_err(|e| format!("Failed to read the {kind} response: {e}"))
}

#[cfg(test)]
thread_local! {
    static TEST_COMMAND_RESPONSE: std::cell::RefCell<Option<Result<Vec<u8>, String>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn set_test_command_response(response: Result<Vec<u8>, String>) {
    TEST_COMMAND_RESPONSE.with(|slot| *slot.borrow_mut() = Some(response));
}

fn fs_mutate(plugin_command: PluginCommand) -> Result<(), String> {
    let response: ProtobufFsMutateResponse = call(plugin_command)?;
    match response.result {
        Some(fs_mutate_response::Result::Success(_)) => Ok(()),
        Some(fs_mutate_response::Result::Error(error)) => Err(error),
        None => Err("yinz answered with nothing".to_owned()),
    }
}

/// The last `lines` log entries for a plugin name, configured alias, or explicit path
/// (absolute, `./`, `../`, or `file:`). Relative paths use the calling plugin's cwd.
/// Paths and aliases require source-attributed entries; bare names also read legacy entries.
/// Empty when no entries match; an error when the log cannot be found.
pub fn read_plugin_log(name: &str, lines: usize) -> Result<String, String> {
    let response: ProtobufReadPluginLogResponse = call(PluginCommand::ReadPluginLog {
        name: name.to_owned(),
        lines,
    })?;
    match response.result {
        Some(read_plugin_log_response::Result::Lines(lines)) => Ok(lines),
        Some(read_plugin_log_response::Result::Error(error)) => Err(error),
        None => Err("yinz answered with nothing".to_owned()),
    }
}

/// The contents of `path`, up to `max_bytes` (the host's own cap otherwise).
pub fn fs_read(path: &Path, max_bytes: Option<usize>) -> Result<Vec<u8>, String> {
    fs_read_at(path, max_bytes, None)
}

/// Read up to `max_bytes` from a byte offset, even when the file exceeds the host's read cap.
/// Returns fewer bytes at EOF. Separate calls reopen the file and do not form a snapshot.
pub fn fs_read_range(path: &Path, offset: u64, max_bytes: usize) -> Result<Vec<u8>, String> {
    fs_read_at(path, Some(max_bytes), Some(offset))
}

fn fs_read_at(
    path: &Path,
    max_bytes: Option<usize>,
    offset: Option<u64>,
) -> Result<Vec<u8>, String> {
    let response: ProtobufFsReadResponse = call(PluginCommand::FsRead {
        path: path.to_path_buf(),
        max_bytes,
        offset,
    })?;
    match response.result {
        Some(fs_read_response::Result::Contents(contents)) => Ok(contents),
        Some(fs_read_response::Result::Error(error)) => Err(error),
        None => Err("yinz answered with nothing".to_owned()),
    }
}

/// The entries of the folder at `path`, one level deep.
pub fn fs_list(path: &Path) -> Result<Vec<FsEntry>, String> {
    let response: ProtobufFsListResponse = call(PluginCommand::FsList {
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
        None => Err("yinz answered with nothing".to_owned()),
    }
}

/// What is at `path`: `Ok(None)` when nothing is, which is an answer rather than an error.
pub fn fs_metadata(path: &Path) -> Result<Option<FileMetadata>, String> {
    let response: ProtobufFsMetadataResponse = call(PluginCommand::FsMetadata {
        path: path.to_path_buf(),
    })?;
    match response.result {
        Some(fs_metadata_response::Result::Metadata(metadata)) => {
            Ok(Option::<FileMetadata>::from(&metadata))
        },
        Some(fs_metadata_response::Result::NotFound(_)) => Ok(None),
        Some(fs_metadata_response::Result::Error(error)) => Err(error),
        None => Err("yinz answered with nothing".to_owned()),
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

    let protobuf_response: ProtobufParseLayoutResponse =
        call(plugin_command).map_err(|_| LayoutParsingError::SyntaxError)?;

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

/// List every toast received by the current live session, oldest first.
///
/// Requires the `ReadApplicationState` permission.
pub fn resolve_notification(
    id: yinz_types::internal_ids::NotificationId,
    decision: yinz_types::notifications::NotificationDecision,
) {
    send(PluginCommand::ResolveNotification { id, decision });
}

pub fn list_notifications() -> Result<Vec<NotificationInfo>, String> {
    let plugin_command = PluginCommand::ListNotifications;
    let response: ProtobufListNotificationsResponse = call(plugin_command)?;
    match response.error {
        Some(error) => Err(error),
        None => Ok(response.notifications.into_iter().map(Into::into).collect()),
    }
}

/// Get a list of clients, their focused pane and running command or focused plugin back as an
/// Event::ListClients (note: this event must be subscribed to)
pub fn list_clients() {
    send(PluginCommand::ListClients);
}

/// Change configuration for the current user
pub fn reconfigure(new_config: String, save_configuration_file: bool) {
    send(PluginCommand::Reconfigure(
        new_config,
        save_configuration_file,
    ));
}

/// Re-run command in pane
pub fn rerun_command_pane(terminal_pane_id: yinz_types::internal_ids::TerminalId) {
    send(PluginCommand::RerunCommandPane(terminal_pane_id));
}

/// Run a *different* command in an existing command pane, reusing its pane id. The `context`
/// round-trips on the resulting `CommandPaneExited`, so each step of a sequence can carry its
/// own correlation keys. When the pane's current command is still running, the replacement
/// starts when it exits (last write wins). `keep_scrollback` scrolls the previous command's
/// output into history instead of erasing it.
pub fn run_command_in_pane(
    pane_id: PaneId,
    command: CommandToRun,
    context: BTreeMap<String, String>,
    keep_scrollback: bool,
) {
    send(PluginCommand::RunCommandInPane {
        pane_id,
        command,
        context,
        keep_scrollback,
    });
}

/// Sugar for close_terminal_pane and close_plugin_pane
pub fn close_pane_with_id(pane_id: PaneId) {
    let plugin_command = match pane_id {
        PaneId::Terminal(terminal_pane_id) => PluginCommand::CloseTerminalPane(terminal_pane_id),
        PaneId::Plugin(plugin_pane_id) => PluginCommand::ClosePluginPane(plugin_pane_id),
    };
    send(plugin_command);
}

/// Resize the specified pane (increase/decrease) with an optional direction (left/right/up/down)
pub fn resize_pane_with_id(resize_strategy: ResizeStrategy, pane_id: PaneId) {
    send(PluginCommand::ResizePaneIdWithDirection(
        resize_strategy,
        pane_id,
    ));
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
    send(plugin_command);
}

/// Edit the scrollback of the specified pane in the user's default `$EDITOR` (currently only works
/// for terminal panes)
pub fn edit_scrollback_for_pane_with_id(pane_id: PaneId) {
    send(PluginCommand::EditScrollbackForPaneWithId(pane_id));
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
    let protobuf_response: ProtobufPaneScrollbackResponse = call(plugin_command)?;

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
    send(PluginCommand::WriteToPaneId(bytes, pane_id));
}

/// Write characters to the `STDIN` of the specified pane
pub fn write_chars_to_pane_id(chars: &str, pane_id: PaneId) {
    send(PluginCommand::WriteCharsToPaneId(chars.to_owned(), pane_id));
}

/// Send SIGINT to the process running inside a terminal pane identified by this PaneId
pub fn send_sigint_to_pane_id(pane_id: PaneId) {
    send(PluginCommand::SendSigintToPaneId(pane_id));
}

/// Send SIGKILL to the process running inside a terminal pane identified by this PaneId
pub fn send_sigkill_to_pane_id(pane_id: PaneId) {
    send(PluginCommand::SendSigkillToPaneId(pane_id));
}

/// Get the PID of the process running inside a terminal pane
pub fn get_pane_pid(pane_id: PaneId) -> Result<ProcessId, String> {
    let plugin_command = PluginCommand::GetPanePid { pane_id };
    let protobuf_response: ProtobufGetPanePidResponse = call(plugin_command)?;

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
/// use yinz_tile::prelude::*;
///
/// let pane_id = PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1));
/// match get_pane_running_command(pane_id) {
///     Ok(cmd) => eprintln!("Running: {} {}", cmd[0], cmd[1..].join(" ")),
///     Err(e) => eprintln!("Error: {}", e),
/// }
/// ```
pub fn get_pane_running_command(pane_id: PaneId) -> Result<Vec<String>, String> {
    let plugin_command = PluginCommand::GetPaneRunningCommand { pane_id };
    let protobuf_response: ProtobufGetPaneRunningCommandResponse = call(plugin_command)?;

    match protobuf_response.result {
        Some(get_pane_running_command_response::Result::Command(cmd)) => Ok(cmd.args),
        Some(get_pane_running_command_response::Result::Error(err)) => Err(err),
        None => Err("Empty response from server".to_string()),
    }
}

/// Ordinary clients that disconnect_other_clients will remove; watchers are excluded.
/// Each query has a 100 ms timeout and its own response channel.
///
/// # Permissions Required
/// * `ReadApplicationState`
pub fn get_disconnect_targets() -> Result<Vec<ClientId>, String> {
    let response: yinz_plugin_api::plugin_command::ProtobufGetDisconnectTargetsResponse =
        call(PluginCommand::GetDisconnectTargets)?;
    if let Some(error) = response.error {
        return Err(error);
    }
    response
        .client_ids
        .into_iter()
        .map(|id| {
            ClientId::try_from(id).map_err(|error| format!("Invalid disconnect client id: {error}"))
        })
        .collect()
}

/// Fetches a fresh snapshot of all live and resurrectable sessions on this machine.
///
/// # Permissions Required
/// * `ReadApplicationState`
pub fn get_session_list() -> Result<SessionListSnapshot, String> {
    let plugin_command = PluginCommand::GetSessionList;
    let protobuf_response: ProtobufGetSessionListResponse = call(plugin_command)?;

    match GetSessionListResponse::try_from(protobuf_response)
        .map_err(|e| format!("Malformed GetSessionList response: {e}"))?
    {
        GetSessionListResponse::Ok(snapshot) => Ok(snapshot),
        GetSessionListResponse::Err(error) => Err(error),
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
/// use yinz_tile::prelude::*;
///
/// let pane_id = PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1));
/// match get_pane_cwd(pane_id) {
///     Ok(cwd) => eprintln!("CWD: {}", cwd.display()),
///     Err(e) => eprintln!("Error: {}", e),
/// }
/// ```
pub fn get_pane_cwd(pane_id: PaneId) -> Result<PathBuf, String> {
    let plugin_command = PluginCommand::GetPaneCwd { pane_id };
    let protobuf_response: ProtobufGetPaneCwdResponse = call(plugin_command)?;

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
/// pane. `Ok(None)` means the requested `cwd` is pending at the broker.
/// `Ok(Some(probe))` always describes that directory; file it under its host and cwd.
///
/// # Permissions Required
/// * `ReadApplicationState`
pub fn get_pane_git_probe(pane_id: PaneId, cwd: &str) -> Result<Option<PaneGitProbe>, String> {
    get_pane_git_probe_with_refresh(pane_id, cwd, None)
}

/// Request fresh metadata with a unique token; poll with that token until ready.
pub fn get_pane_git_probe_with_refresh(
    pane_id: PaneId,
    cwd: &str,
    refresh_token: Option<String>,
) -> Result<Option<PaneGitProbe>, String> {
    let plugin_command = PluginCommand::GetPaneGitProbe {
        refresh_token,
        pane_id,
        cwd: cwd.into(),
    };
    let protobuf_response: ProtobufGetPaneGitProbeResponse = call(plugin_command)?;

    match protobuf_response.result {
        Some(get_pane_git_probe_response::Result::Probe(probe)) => Ok(Some(probe.into())),
        Some(get_pane_git_probe_response::Result::Pending(_)) => Ok(None),
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
    let protobuf_response: ProtobufSaveLayoutResponse = call(plugin_command)?;

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
/// use yinz_tile::prelude::*;
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
    let protobuf_response: ProtobufDeleteLayoutResponse = call(plugin_command)?;

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
/// use yinz_tile::prelude::*;
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
    let protobuf_response: ProtobufRenameLayoutResponse = call(plugin_command)?;

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
    let protobuf_response: ProtobufEditLayoutResponse = call(plugin_command)?;

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
    send(PluginCommand::MovePaneWithPaneId(pane_id));
}

/// Switch the position of the pane with this id with a different pane in the specified direction (eg. `Down`, `Up`, `Left`, `Right`).
pub fn move_pane_with_pane_id_in_direction(pane_id: PaneId, direction: Direction) {
    send(PluginCommand::MovePaneWithPaneIdInDirection(
        pane_id, direction,
    ));
}

/// Clear the scroll buffer of the specified pane
pub fn clear_screen_for_pane_id(pane_id: PaneId) {
    send(PluginCommand::ClearScreenForPaneId(pane_id));
}

/// Scroll the specified pane up 1 line
pub fn scroll_up_in_pane_id(pane_id: PaneId) {
    send(PluginCommand::ScrollUpInPaneId(pane_id));
}

/// Scroll the specified pane down 1 line
pub fn scroll_down_in_pane_id(pane_id: PaneId) {
    send(PluginCommand::ScrollDownInPaneId(pane_id));
}

/// Scroll the specified pane all the way to the top of the scrollbuffer
pub fn scroll_to_top_in_pane_id(pane_id: PaneId) {
    send(PluginCommand::ScrollToTopInPaneId(pane_id));
}

/// Scroll the specified pane all the way to the bottom of the scrollbuffer
pub fn scroll_to_bottom_in_pane_id(pane_id: PaneId) {
    send(PluginCommand::ScrollToBottomInPaneId(pane_id));
}

/// Scroll the specified pane up one page
pub fn page_scroll_up_in_pane_id(pane_id: PaneId) {
    send(PluginCommand::PageScrollUpInPaneId(pane_id));
}

/// Scroll the specified pane down one page
pub fn page_scroll_down_in_pane_id(pane_id: PaneId) {
    send(PluginCommand::PageScrollDownInPaneId(pane_id));
}

/// Toggle the specified pane to be fullscreen or normal sized
pub fn toggle_pane_id_fullscreen(pane_id: PaneId) {
    send(PluginCommand::TogglePaneIdFullscreen(pane_id));
}

/// Embed the specified pane (make it stop floating) or turn it to a float pane if it is not
pub fn toggle_pane_embed_or_eject_for_pane_id(pane_id: PaneId) {
    send(PluginCommand::TogglePaneEmbedOrEjectForPaneId(pane_id));
}

/// Close the focused tab
pub fn close_tab_with_index(tab_index: TabPosition) {
    send(PluginCommand::CloseTabWithIndex(tab_index));
}

/// Close the tab with the given stable ID.
///
/// Unlike `close_tab_with_index`, this function identifies the tab by its stable
/// `tab_id` rather than its display position, which may change as tabs are moved
/// or closed. The tab_id can be obtained from `TabInfo.tab_id`.
pub fn close_tab_with_id(tab_id: TabId) {
    send(PluginCommand::CloseTabWithId(tab_id));
}

/// Focus the tab with the given stable ID.
///
/// Unlike `go_to_tab`, this function identifies the tab by its stable `tab_id`
/// rather than its display position, which may change as tabs are moved or
/// closed. The tab_id can be obtained from `TabInfo.tab_id`.
pub fn go_to_tab_with_id(tab_id: TabId) {
    send(PluginCommand::GoToTabWithId(tab_id));
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
    send(plugin_command);
}

/// Compose a decoration in front of a pane's title, or drop it with `None`.
///
/// Not a rename. The pane's name, its own OSC title and the `title` every plugin
/// reads off the pane manifest are all untouched, so the prefix survives every
/// rename and cannot erase one — which is what makes it the place for a fact about
/// the pane rather than a name for it. It shows wherever a human looks at the pane:
/// its frame, and its row in a stack list. Terminal panes only.
pub fn set_pane_title_prefix(pane_id: PaneId, prefix: Option<&str>) {
    send(PluginCommand::SetPaneTitlePrefix {
        pane_id,
        prefix: prefix.map(ToOwned::to_owned),
    });
}

/// Set the reserved row below a pane title; `None` or empty text clears it.
pub fn set_pane_subtitle(pane_id: PaneId, subtitle: Option<&str>) {
    send(PluginCommand::SetPaneSubtitle {
        pane_id,
        subtitle: subtitle.map(ToOwned::to_owned),
    });
}

/// Create a new tab that includes the specified pane ids
pub fn break_panes_to_new_tab(
    pane_ids: &[PaneId],
    new_tab_name: Option<String>,
    should_change_focus_to_new_tab: bool,
) -> Option<TabId> {
    let plugin_command = PluginCommand::BreakPanesToNewTab(
        pane_ids.to_vec(),
        new_tab_name,
        should_change_focus_to_new_tab,
    );

    or_default(call_as::<
        BreakPanesToNewTabResponse,
        ProtobufBreakPanesToNewTabResponse,
    >(plugin_command))
}

/// Move the pane ids to the tab with the specified index
pub fn break_panes_to_tab_with_index(
    pane_ids: &[PaneId],
    tab_index: TabPosition,
    should_change_focus_to_new_tab: bool,
) -> Option<TabId> {
    let plugin_command = PluginCommand::BreakPanesToTabWithIndex(
        pane_ids.to_vec(),
        tab_index,
        should_change_focus_to_new_tab,
    );

    or_default(call_as::<
        BreakPanesToTabWithIndexResponse,
        ProtobufBreakPanesToTabWithIndexResponse,
    >(plugin_command))
}

/// Move the pane ids to the tab with the specified id
pub fn break_panes_to_tab_with_id(
    pane_ids: &[PaneId],
    tab_id: TabId,
    should_change_focus_to_target_tab: bool,
) -> Option<TabId> {
    let plugin_command = PluginCommand::BreakPanesToTabWithId(
        pane_ids.to_vec(),
        tab_id,
        should_change_focus_to_target_tab,
    );

    or_default(call_as::<
        BreakPanesToTabWithIdResponse,
        ProtobufBreakPanesToTabWithIdResponse,
    >(plugin_command))
}

/// Reload an already-running plugin in this session
pub fn reload_plugin_with_id(plugin_id: yinz_types::internal_ids::PluginId) {
    send(PluginCommand::ReloadPlugin(plugin_id));
}

/// Load a plugin into this session. Requires `OpenTerminalsOrPlugins`.
pub fn load_new_plugin<S>(
    url: S,
    config: BTreeMap<String, String>,
    load_in_background: bool,
    skip_plugin_cache: bool,
) where
    S: AsRef<str> + ToString,
{
    send(PluginCommand::LoadNewPlugin {
        url: url.to_string(),
        config,
        load_in_background,
        skip_plugin_cache,
    });
}

// Work queues

/// Put a plugin on a named work queue, to run as a fresh headless instance when the queue has a
/// free slot. Returns the job id.
///
/// The queue is created with concurrency 1 if it does not exist; see [`configure_queue`].
/// `config` becomes the job's configuration - alongside `yinz_queue`, `yinz_queue_job_id` and
/// `yinz_queue_enqueuer_plugin_id`, which the host adds - and `context` is never given to the
/// job but echoed back in [`Event::QueueJobStarted`] and [`Event::QueueJobFinished`], which only
/// the enqueuer receives (subscribe to them). Needs `MessageAndLaunchOtherPlugins`.
pub fn enqueue_queue_job<Q, U>(
    queue: Q,
    plugin_url: U,
    config: BTreeMap<String, String>,
    context: BTreeMap<String, String>,
) -> Result<QueueJobId, String>
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
    let protobuf_response: ProtobufEnqueueQueueJobResponse = call(plugin_command)?;
    match EnqueueQueueJobResponse::try_from(protobuf_response) {
        Ok(EnqueueQueueJobResponse::Ok(job_id)) => Ok(job_id),
        Ok(EnqueueQueueJobResponse::Err(error)) => Err(error),
        Err(e) => Err(e.to_owned()),
    }
}

/// Set how many jobs `queue` may run at once, creating it if needed. Lowering the limit below
/// the number of running jobs evicts none of them. Needs `ChangeApplicationState`.
pub fn configure_queue<Q: ToString>(queue: Q, concurrency: usize) {
    send(PluginCommand::ConfigureQueue {
        queue: queue.to_string(),
        concurrency,
    });
}

/// Drop a pending job, or unload a running one. Its enqueuer is told `Cancelled`. Needs
/// `ChangeApplicationState`.
pub fn cancel_queue_job(job_id: QueueJobId) {
    send(PluginCommand::CancelQueueJob { job_id });
}

/// Every queue, or just the named one, with its pending and running jobs. Needs
/// `ReadApplicationState`. To be told about changes instead of asking, subscribe to
/// [`Event::QueueChanged`].
pub fn queue_status(queue: Option<&str>) -> Vec<QueueSnapshot> {
    let plugin_command = PluginCommand::QueueStatus {
        queue: queue.map(ToOwned::to_owned),
    };

    or_default(call_as::<QueueStatusResponse, ProtobufQueueStatusResponse>(
        plugin_command,
    ))
}

/// Called by a job to report its outcome. The host unloads this plugin afterwards, so anything
/// still to do should be done before. Ignored when the caller is not a queued job.
pub fn finish_queue_job(success: bool, payload: Option<String>) {
    send(PluginCommand::FinishQueueJob { success, payload });
}

/// Rebind keys for the current user
pub fn rebind_keys(
    keys_to_unbind: Vec<(InputMode, KeyWithModifier)>,
    keys_to_rebind: Vec<(InputMode, KeyWithModifier, Vec<Action>)>,
    write_config_to_disk: bool,
) {
    send(PluginCommand::RebindKeys {
        keys_to_rebind,
        keys_to_unbind,
        write_config_to_disk,
    });
}

pub fn change_host_folder(new_host_folder: PathBuf) {
    send(PluginCommand::ChangeHostFolder(new_host_folder));
}

pub fn set_floating_pane_pinned(pane_id: PaneId, should_be_pinned: bool) {
    send(PluginCommand::SetFloatingPanePinned(
        pane_id,
        should_be_pinned,
    ));
}

pub fn stack_panes(pane_ids: Vec<PaneId>) {
    send(PluginCommand::StackPanes(pane_ids));
}

pub fn change_floating_panes_coordinates(
    pane_ids_and_coordinates: Vec<(PaneId, FloatingPaneCoordinates)>,
) {
    send(PluginCommand::ChangeFloatingPanesCoordinates(
        pane_ids_and_coordinates,
    ));
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
/// pressed to move it is as much theirs as the drag was, so this variant reclaims the position;
/// the independently user-stated size remains unchanged.
pub fn change_overlay_coordinates_overriding_user_placement(
    pane_ids_and_coordinates: Vec<(PaneId, FloatingPaneCoordinates)>,
) {
    change_overlay_coordinates_with(pane_ids_and_coordinates, true);
}

fn change_overlay_coordinates_with(
    placements: Vec<(PaneId, FloatingPaneCoordinates)>,
    override_user_placement: bool,
) {
    send(PluginCommand::ChangeOverlayCoordinates {
        placements,
        override_user_placement,
    });
}

/// Toggle the borderless state of a pane identified by pane_id
///
/// # Arguments
/// * `pane_id` - The ID of the pane to toggle (PaneId::Terminal or PaneId::Plugin)
pub fn toggle_pane_borderless(pane_id: PaneId) {
    send(PluginCommand::TogglePaneBorderless(pane_id));
}

/// Set the borderless state of a pane explicitly
///
/// # Arguments
/// * `pane_id` - The ID of the pane (PaneId::Terminal or PaneId::Plugin)
/// * `borderless` - true for borderless, false for bordered
pub fn set_pane_borderless(pane_id: PaneId, borderless: bool) {
    send(PluginCommand::SetPaneBorderless(pane_id, borderless));
}

/// Reload every running instance of the plugin at `url` in place, opening nothing; a plugin
/// that is not running is left alone. Needs `AuthorPlugins`.
pub fn reload_plugin_by_url(url: &str) {
    send(PluginCommand::ReloadPluginByUrl(url.to_owned()));
}

/// Find running instances using the host's canonical source matching. Requires ReadApplicationState.
pub fn get_plugin_ids_for_url(url: &str) -> Result<Vec<PluginId>, String> {
    let response: yinz_plugin_api::plugin_command::ProtobufGetPluginIdsForUrlResponse =
        call(PluginCommand::GetPluginIdsForUrl(url.to_owned()))?;
    match response.error {
        Some(error) => Err(error),
        None => Ok(response.plugin_ids.into_iter().map(PluginId::new).collect()),
    }
}

/// Read metadata supplied by loaded TypeScript agent plugins. Requires RunCommands.
pub fn get_agent_providers() -> Result<Vec<yinz_types::agent::provider::AgentProvider>, String> {
    let response: yinz_plugin_api::plugin_command::ProtobufAgentProvidersResponse =
        call(PluginCommand::GetAgentProviders)?;
    serde_json::from_str(&response.json)
        .map_err(|error| format!("Malformed agent provider reply: {error}"))?
}

/// Load provider metadata and keep the portable registry current through plugin events.
/// Call during `load` before using agent detection or launch metadata. Requires RunCommands.
pub fn init_agent_providers() {
    subscribe(&[EventType::AgentProvidersChanged]);
    if let Ok(providers) = get_agent_providers() {
        update_agent_providers(&Event::AgentProvidersChanged(providers));
    }
}

/// Refresh the portable registry before a Wasm plugin handles a provider update.
pub fn update_agent_providers(event: &Event) {
    if let Event::AgentProvidersChanged(providers) = event {
        if let Err(error) = yinz_types::agent::provider::replace_snapshot(providers.clone()) {
            eprintln!("yinz-tile: invalid agent provider snapshot: {error}");
        }
    }
}

/// Set what agent runs in `pane_id` and what it is doing, or clear the record with `None`.
/// The record replaces the pane's whole one; every manifest reader sees it next report.
pub fn set_pane_agent(pane_id: PaneId, agent: Option<AgentInfo>) {
    send(PluginCommand::SetPaneAgent { pane_id, agent });
}

/// Set the id of the run record this plugin keeps for `pane_id`'s agent, or clear it with
/// `None`.
///
/// Additive where [`set_pane_agent`] is wholesale: this touches one field and leaves the rest
/// of the record as the agent last reported it. A pane with no record has nothing for a run id
/// to name, so one sent for it is dropped.
pub fn set_pane_agent_run(pane_id: PaneId, run_id: Option<String>) {
    send(PluginCommand::SetPaneAgentRun { pane_id, run_id });
}

/// Say what this plugin wants said, and decided, when the session is about to end.
///
/// `notice` is one line the quit confirmation shows, or `None` to withdraw whatever this plugin
/// last said. `guard` is whether closing the session's *last* pane should raise that confirmation
/// instead of ending the session there and then.
///
/// The two are independent: a plugin may have something worth saying about a quit without wanting
/// to stand in the way of one. Withdraw the guard when there is nothing left outstanding - a
/// plugin that keeps one up with nothing to show for it leaves the session unable to close its
/// last pane.
pub fn set_quit_notice(notice: Option<String>, guard: bool) {
    send(PluginCommand::SetQuitNotice { notice, guard });
}

/// Set the default foreground and/or background color of a pane
///
/// # Arguments
/// * `pane_id` - The ID of the pane (PaneId::Terminal or PaneId::Plugin)
/// * `fg` - Optional foreground color string (e.g. "#00e000"), None to leave unchanged
/// * `bg` - Optional background color string (e.g. "#001a3a"), None to leave unchanged
pub fn set_pane_color(pane_id: PaneId, fg: Option<String>, bg: Option<String>) {
    send(PluginCommand::SetPaneColor(pane_id, fg, bg));
}

pub fn group_and_ungroup_panes(
    pane_ids_to_group: Vec<PaneId>,
    pane_ids_to_ungroup: Vec<PaneId>,
    for_all_clients: bool,
) {
    send(PluginCommand::GroupAndUngroupPanes(
        pane_ids_to_group,
        pane_ids_to_ungroup,
        for_all_clients,
    ));
}

pub fn highlight_and_unhighlight_panes(
    pane_ids_to_highlight: Vec<PaneId>,
    pane_ids_to_unhighlight: Vec<PaneId>,
) {
    let plugin_command =
        PluginCommand::HighlightAndUnhighlightPanes(pane_ids_to_highlight, pane_ids_to_unhighlight);
    send(plugin_command);
}

/// Mark these panes as kin of the focused one, so the session draws them with a gentle
/// tint - a hint that they belong together, not a second focus.
///
/// The call replaces the whole set: whatever was marked before and is not in `pane_ids`
/// stops being marked. Pass an empty vector to clear. A plugin therefore says what is
/// related *now* rather than tracking what the session still believes, and a reload
/// re-asserts by simply saying it again.
///
/// Unlike [`highlight_and_unhighlight_panes`], this does not touch the pane's one frame
/// override slot, so a bell flash or a failed command still shows through.
pub fn set_related_panes(pane_ids: Vec<PaneId>) {
    send(PluginCommand::SetRelatedPanes(pane_ids));
}

/// Bring a pane into view for this client and highlight its frame, without
/// taking keyboard focus - unlike [`focus_pane_with_id`], which does both.
///
/// `label` is drawn in place of the pane's title for as long as the highlight is
/// up, so a pane can say what it has been singled out *for*. Clear both with
/// [`highlight_and_unhighlight_panes`], passing the pane as one to unhighlight.
///
/// How much can be revealed depends on where this client's keyboard focus is,
/// because Yinz gives a client one tab that is both the one it sees and the
/// one it types into. A suppressed or hidden floating pane is always brought
/// out; a pane in another tab is left where it stands, with the highlight applied
/// there and the client's view untouched - that is the promise, focus does not move.
pub fn reveal_pane_with_id(pane_id: PaneId, label: Option<String>) {
    send(PluginCommand::RevealPaneWithId(pane_id, label));
}

pub fn close_multiple_panes(pane_ids: Vec<PaneId>) {
    send(PluginCommand::CloseMultiplePanes(pane_ids));
}

pub fn float_multiple_panes(pane_ids: Vec<PaneId>) {
    send(PluginCommand::FloatMultiplePanes(pane_ids));
}

pub fn embed_multiple_panes(pane_ids: Vec<PaneId>) {
    send(PluginCommand::EmbedMultiplePanes(pane_ids));
}

pub fn set_self_mouse_selection_support(selection_support: bool) {
    send(PluginCommand::SetSelfMouseSelectionSupport(
        selection_support,
    ));
}

pub fn intercept_key_presses() {
    send(PluginCommand::InterceptKeyPresses);
}

pub fn clear_key_presses_intercepts() {
    send(PluginCommand::ClearKeyPressesIntercepts);
}

pub fn replace_pane_with_existing_pane(
    pane_id_to_replace: PaneId,
    existing_pane_id: PaneId,
    suppress_replaced_pane: bool,
) {
    send(PluginCommand::ReplacePaneWithExistingPane(
        pane_id_to_replace,
        existing_pane_id,
        suppress_replaced_pane,
    ));
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
pub fn get_focused_pane(
    tab_position: TabPosition,
    pane_manifest: &PaneManifest,
) -> Option<PaneInfo> {
    let panes = pane_manifest.panes.get(&tab_position);
    if let Some(panes) = panes {
        for pane in panes {
            if pane.is_focused & !pane.is_plugin() {
                return Some(pane.clone());
            }
        }
    }
    None
}

/// Replace the current layout. Layout permissions follow [`new_tabs_with_layout_info`].
pub fn override_layout<L: AsRef<LayoutInfo>>(
    layout_info: L,
    retain_existing_terminal_panes: bool,
    retain_existing_plugin_panes: bool,
    apply_only_to_active_tab: bool,
    context: BTreeMap<String, String>,
) {
    send(PluginCommand::OverrideLayout(
        layout_info.as_ref().clone(),
        retain_existing_terminal_panes,
        retain_existing_plugin_panes,
        apply_only_to_active_tab,
        context,
    ));
}

// Internal Functions

#[doc(hidden)]
pub fn object_from_stdin<T: DeserializeOwned>() -> Result<T> {
    let err_context = || "failed to deserialize object from stdin".to_string();

    let mut json = String::new();
    io::stdin().read_line(&mut json).with_context(err_context)?;
    serde_json::from_str(&json).with_context(err_context)
}

/// The host's reply: one line of JSON-encoded protobuf bytes.
fn bytes_from_stdin() -> Result<Vec<u8>> {
    let err_context = || "failed to deserialize bytes from stdin".to_string();
    let mut json = String::new();
    io::stdin().read_line(&mut json).with_context(err_context)?;
    serde_json::from_str(&json).with_context(err_context)
}

/// One request to the host: the protobuf bytes, JSON-encoded on a line of their own.
fn object_to_stdout(bytes: &[u8]) {
    println!(
        "{}",
        serde_json::to_string(bytes).expect("a byte slice always serializes")
    );
}

/// Post a message to a worker of this plugin, for more information please see [`YinzWorker`](crate::YinzWorker)
pub fn post_message_to(plugin_message: PluginMessage) {
    send(PluginCommand::PostMessageTo(plugin_message));
}

/// Post a message to this plugin, for more information please see [`YinzWorker`](crate::YinzWorker)
pub fn post_message_to_plugin(plugin_message: PluginMessage) {
    send(PluginCommand::PostMessageToPlugin(plugin_message));
}

pub fn run_action(action: Action, context: BTreeMap<String, String>) {
    send(PluginCommand::RunAction(action, context));
}

/// Show all floating panes in the specified tab, or the active tab if `tab_id` is `None`.
///
/// Blocks until the server-side action is complete.
///
/// Returns `Ok(true)` if the floating panes were made visible (state changed),
/// `Ok(false)` if the floating panes were already visible (no change),
/// or `Err(String)` if the specified tab was not found.
pub fn show_floating_panes(tab_id: Option<TabId>) -> Result<bool, String> {
    let plugin_command = PluginCommand::ShowFloatingPanes { tab_id };
    let response: ProtobufShowFloatingPanesResponse = call(plugin_command)?;
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
pub fn hide_floating_panes(tab_id: Option<TabId>) -> Result<bool, String> {
    let plugin_command = PluginCommand::HideFloatingPanes { tab_id };
    let response: ProtobufHideFloatingPanesResponse = call(plugin_command)?;
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
    send(PluginCommand::SetPaneRegexHighlights(pane_id, highlights));
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
pub fn set_context_menu(request_id: ContextMenuRequestId, items: Vec<ContextMenuItem>) {
    send(PluginCommand::SetContextMenu { request_id, items });
}

/// Remove all regex highlights this plugin has set on the given pane.
///
/// Other plugins' highlights on the same pane are not affected.
///
/// Requires `ChangeApplicationState` permission.
pub fn clear_pane_highlights(pane_id: PaneId) {
    send(PluginCommand::ClearPaneHighlights(pane_id));
}

/// Read project workflow settings; requires ReadFiles.
pub fn project_config_read(directory: PathBuf) -> Result<yinz_project_config::Snapshot, String> {
    project_config_response(PluginCommand::ProjectConfigRead { directory })
}

/// Write previously reviewed settings; requires WriteFiles, and rejects stale source.
pub fn project_config_write(
    directory: PathBuf,
    source: String,
    expected: Option<String>,
) -> Result<yinz_project_config::Snapshot, String> {
    project_config_response(PluginCommand::ProjectConfigWrite {
        directory,
        source,
        expected,
    })
}

fn project_config_response(
    command: PluginCommand,
) -> Result<yinz_project_config::Snapshot, String> {
    let response: ProtobufFsReadResponse = call(command)?;
    match response.result {
        Some(fs_read_response::Result::Contents(contents)) => {
            serde_json::from_slice(&contents).map_err(|e| e.to_string())
        },
        Some(fs_read_response::Result::Error(error)) => Err(error),
        None => Err("Missing project configuration reply".into()),
    }
}

/// Start installed TypeScript tests. Subscribe to CustomMessage before calling; the result is
/// JSON Result<PluginTestReport, String> under `__yinz_plugin_test:<request_id>`.
pub fn test_plugin(
    name: &str,
    options: yinz_types::authoring::PluginTestOptions,
    request_id: &str,
) {
    let command = PluginCommand::TestPlugin {
        name: name.to_owned(),
        options,
        request_id: request_id.to_owned(),
    };
    send(command);
}
pub fn cancel_plugin_test(request_id: &str) {
    let command = PluginCommand::CancelPluginTest(request_id.to_owned());
    send(command);
}

/// Compose two panes into one host-owned overlay.
pub fn create_multipane(
    request: yinz_types::multipane::CreateMultipane,
) -> yinz_types::multipane::MultipaneResponse {
    multipane_response(PluginCommand::CreateMultipane(request))
}
pub fn update_multipane(
    request: yinz_types::multipane::UpdateMultipane,
) -> yinz_types::multipane::MultipaneResponse {
    multipane_response(PluginCommand::UpdateMultipane(request))
}
pub fn replace_multipane_child(
    request: yinz_types::multipane::ReplaceMultipaneChild,
) -> yinz_types::multipane::MultipaneResponse {
    multipane_response(PluginCommand::ReplaceMultipaneChild(request))
}
fn multipane_response(command: PluginCommand) -> yinz_types::multipane::MultipaneResponse {
    let response: yinz_plugin_api::plugin_command::ProtobufMultipaneResponse = call(command)?;
    serde_json::from_str(&response.json).map_err(|error| error.to_string())?
}

/// Mount a fixed plugin tab list, retaining the existing pane in its slot.
pub fn mount_pane_tabs(
    pane_id: PaneId,
    layout: yinz_types::pane_tabs::PaneTabsLayout,
    existing: usize,
) -> yinz_types::pane_tabs::PaneTabsResponse {
    pane_tabs(yinz_types::pane_tabs::PaneTabsCommand::Mount {
        pane_id,
        layout,
        existing,
    })
}
pub fn select_pane_tab(
    id: yinz_types::pane_tabs::PaneTabsId,
    index: usize,
) -> yinz_types::pane_tabs::PaneTabsResponse {
    pane_tabs(yinz_types::pane_tabs::PaneTabsCommand::Select { id, index })
}
pub fn focus_pane_tabs(
    id: yinz_types::pane_tabs::PaneTabsId,
    bar: bool,
) -> yinz_types::pane_tabs::PaneTabsResponse {
    pane_tabs(yinz_types::pane_tabs::PaneTabsCommand::Focus { id, bar })
}
pub fn pane_tabs(
    command: yinz_types::pane_tabs::PaneTabsCommand,
) -> yinz_types::pane_tabs::PaneTabsResponse {
    let response: yinz_plugin_api::plugin_command::ProtobufPaneTabsResponse =
        call(PluginCommand::PaneTabs(command))?;
    serde_json::from_str(&response.json).map_err(|error| error.to_string())?
}

/// Inspect launch provenance. Inspecting another instance requires ReadApplicationState.
pub fn get_plugin_info(
    id: Option<PluginId>,
) -> Result<Option<yinz_types::data::PluginInstanceInfo>, String> {
    let response: yinz_plugin_api::plugin_command::ProtobufGetPluginInfoResponse =
        call(PluginCommand::GetPluginInfo(id))?;
    serde_json::from_str(&response.json).map_err(|error| format!("Malformed plugin info: {error}"))
}

/// Find instances using alias defaults overlaid with the supplied settings.
pub fn get_plugin_ids_for_url_with_configuration(
    url: &str,
    configuration: BTreeMap<String, String>,
) -> Result<Vec<PluginId>, String> {
    let response: yinz_plugin_api::plugin_command::ProtobufGetPluginIdsForUrlResponse =
        call(PluginCommand::GetPluginIdsForUrlWithConfiguration {
            url: url.into(),
            configuration,
        })?;
    match response.error {
        Some(error) => Err(error),
        None => Ok(response.plugin_ids.into_iter().map(PluginId::new).collect()),
    }
}

/// Host-owned picker driver protocol; only the assigned driver can attach or complete.
pub fn pick(
    command: yinz_types::pick::PickCommand,
) -> Result<Option<yinz_types::pick::PickRequest>, String> {
    let response: yinz_plugin_api::plugin_command::ProtobufPickResponse =
        call(PluginCommand::Pick(command))?;
    serde_json::from_str(&response.json)
        .map_err(|error| format!("Malformed picker response: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn respond(message: impl prost::Message) {
        set_test_command_response(Ok(message.encode_to_vec()));
    }

    fn fail_next_read() {
        set_test_command_response(Err("stdin closed".to_owned()));
    }

    fn enqueue_test_job() -> Result<QueueJobId, String> {
        enqueue_queue_job(
            "test-queue",
            "file:/plugin.wasm",
            BTreeMap::new(),
            BTreeMap::new(),
        )
    }

    #[test]
    fn disconnect_target_query_decodes_success_and_error() {
        use yinz_plugin_api::plugin_command::ProtobufGetDisconnectTargetsResponse;
        respond(ProtobufGetDisconnectTargetsResponse {
            client_ids: vec![2, 3],
            error: None,
        });
        assert_eq!(
            get_disconnect_targets().unwrap(),
            vec![ClientId::new(2), ClientId::new(3)]
        );
        respond(ProtobufGetDisconnectTargetsResponse {
            client_ids: vec![],
            error: Some("timeout".into()),
        });
        assert_eq!(get_disconnect_targets().unwrap_err(), "timeout");
        respond(ProtobufGetDisconnectTargetsResponse {
            client_ids: vec![u32::MAX],
            error: None,
        });
        assert!(get_disconnect_targets().is_err());
        fail_next_read();
        assert!(get_disconnect_targets()
            .unwrap_err()
            .contains("stdin closed"));
    }
    #[test]
    fn plugin_queries_return_stdin_read_errors() {
        let pane_id = PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1));

        fail_next_read();
        assert_eq!(
            get_pane_running_command(pane_id).unwrap_err(),
            "Failed to read the GetPaneRunningCommand response: stdin closed"
        );
        fail_next_read();
        assert_eq!(
            get_session_list().unwrap_err(),
            "Failed to read the GetSessionList response: stdin closed"
        );
        fail_next_read();
        assert_eq!(
            get_pane_cwd(pane_id).unwrap_err(),
            "Failed to read the GetPaneCwd response: stdin closed"
        );
        fail_next_read();
        assert_eq!(
            get_pane_git_probe(pane_id, "/work/project").unwrap_err(),
            "Failed to read the GetPaneGitProbe response: stdin closed"
        );
        fail_next_read();
        assert_eq!(
            get_focused_pane_info().unwrap_err(),
            "Failed to read the GetFocusedPaneInfo response: stdin closed"
        );
        fail_next_read();
        assert_eq!(
            enqueue_test_job().unwrap_err(),
            "Failed to read the EnqueueQueueJob response: stdin closed"
        );
    }

    #[test]
    fn plugin_queries_return_truncated_and_malformed_protobuf_errors() {
        let mut truncated =
            ProtobufGetPaneRunningCommandResponse::from(GetPaneRunningCommandResponse::Ok(vec![
                "sh".to_owned(),
            ]))
            .encode_to_vec();
        truncated.pop();
        set_test_command_response(Ok(truncated));
        assert!(get_pane_running_command(PaneId::Terminal(
            yinz_types::internal_ids::TerminalId::new(1)
        ))
        .unwrap_err()
        .starts_with("Malformed GetPaneRunningCommand response:"));

        set_test_command_response(Ok(vec![0xff]));
        assert!(get_session_list()
            .unwrap_err()
            .starts_with("Malformed GetSessionList response:"));

        set_test_command_response(Ok(vec![0xff]));
        assert!(get_focused_pane_info()
            .unwrap_err()
            .starts_with("Malformed GetFocusedPaneInfo response:"));

        set_test_command_response(Ok(vec![0xff]));
        assert!(enqueue_test_job()
            .unwrap_err()
            .starts_with("Malformed EnqueueQueueJob response:"));
    }

    #[test]
    fn plugin_queries_preserve_server_errors() {
        let pane_id = PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1));
        let expected = "server refused the query".to_owned();

        respond(ProtobufGetPaneRunningCommandResponse::from(
            GetPaneRunningCommandResponse::Err(expected.clone()),
        ));
        assert_eq!(get_pane_running_command(pane_id), Err(expected.clone()));
        respond(ProtobufGetSessionListResponse::from(
            GetSessionListResponse::Err(expected.clone()),
        ));
        assert_eq!(get_session_list(), Err(expected.clone()));
        respond(ProtobufGetPaneCwdResponse::from(GetPaneCwdResponse::Err(
            expected.clone(),
        )));
        assert_eq!(get_pane_cwd(pane_id), Err(expected.clone()));
        respond(ProtobufGetPaneGitProbeResponse::from(
            GetPaneGitProbeResponse::Err(expected.clone()),
        ));
        assert_eq!(get_pane_git_probe(pane_id, "/work/project"), Err(expected));

        let focused_permission_error = "Permission denied: ReadApplicationState".to_owned();
        respond(ProtobufGetFocusedPaneInfoResponse::from(
            GetFocusedPaneInfoResponse::Err(focused_permission_error.clone()),
        ));
        assert_eq!(get_focused_pane_info(), Err(focused_permission_error));

        let enqueue_permission_error = "Permission denied: MessageAndLaunchOtherPlugins".to_owned();
        respond(ProtobufEnqueueQueueJobResponse::from(
            EnqueueQueueJobResponse::Err(enqueue_permission_error.clone()),
        ));
        assert_eq!(enqueue_test_job(), Err(enqueue_permission_error));
    }

    #[test]
    fn plugin_queries_decode_valid_responses() {
        let pane_id = PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1));

        let command = vec!["sh".to_owned(), "-l".to_owned()];
        respond(ProtobufGetPaneRunningCommandResponse::from(
            GetPaneRunningCommandResponse::Ok(command.clone()),
        ));
        assert_eq!(get_pane_running_command(pane_id), Ok(command));

        let sessions = SessionListSnapshot {
            live_sessions: vec![],
            resurrectable_sessions: vec![(
                "dead".to_owned().into(),
                std::time::Duration::from_secs(3),
            )],
        };
        respond(ProtobufGetSessionListResponse::from(
            GetSessionListResponse::Ok(sessions.clone()),
        ));
        assert_eq!(get_session_list(), Ok(sessions));

        let cwd = PathBuf::from("/tmp/work");
        respond(ProtobufGetPaneCwdResponse::from(GetPaneCwdResponse::Ok(
            cwd.clone(),
        )));
        assert_eq!(get_pane_cwd(pane_id), Ok(cwd));

        let probe = PaneGitProbe {
            host: "buildbox".to_owned(),
            cwd: "/repo".to_owned(),
            exit_code: Some(0),
            metadata: b"main\0origin/main\0".to_vec(),
            config: b"remote.origin.url\0git@example/repo\0".to_vec(),
        };
        respond(ProtobufGetPaneGitProbeResponse::from(
            GetPaneGitProbeResponse::Ok(probe.clone()),
        ));
        assert_eq!(
            get_pane_git_probe(pane_id, "/work/project"),
            Ok(Some(probe))
        );
        respond(ProtobufGetPaneGitProbeResponse::from(
            GetPaneGitProbeResponse::Pending,
        ));
        assert_eq!(get_pane_git_probe(pane_id, "/work/project"), Ok(None));

        respond(ProtobufGetFocusedPaneInfoResponse::from(
            GetFocusedPaneInfoResponse::Ok {
                tab_id: TabId::new(7),
                pane_id: PaneId::Plugin(yinz_types::internal_ids::PluginId::new(42)),
            },
        ));
        assert_eq!(
            get_focused_pane_info(),
            Ok((
                TabId::new(7),
                PaneId::Plugin(yinz_types::internal_ids::PluginId::new(42))
            ))
        );

        respond(ProtobufEnqueueQueueJobResponse::from(
            EnqueueQueueJobResponse::Ok(yinz_types::internal_ids::QueueJobId::new(99)),
        ));
        assert_eq!(enqueue_test_job(), Ok(QueueJobId::new(99)));
    }
    fn a_command() -> CommandToRun {
        CommandToRun {
            path: PathBuf::from("htop"),
            ..Default::default()
        }
    }

    #[test]
    fn a_missing_reply_is_none_not_a_panic() {
        fail_next_read();
        assert_eq!(open_command_pane(a_command(), BTreeMap::new()), None);
        fail_next_read();
        assert_eq!(new_tab(None::<&str>, None), None);
    }

    #[test]
    fn a_missing_reply_is_an_empty_collection() {
        fail_next_read();
        assert!(new_tabs_with_layout("layout {}").is_empty());
        fail_next_read();
        assert!(queue_status(None).is_empty());
        fail_next_read();
        assert_eq!(generate_random_name(), "");
    }

    #[test]
    fn a_malformed_reply_is_none() {
        set_test_command_response(Ok(vec![0xff]));
        assert_eq!(open_command_pane(a_command(), BTreeMap::new()), None);
    }

    #[test]
    fn a_valid_reply_reaches_an_option_returner() {
        let pane_id = PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(7));
        respond(ProtobufOpenCommandPaneResponse::from(Some(pane_id)));
        assert_eq!(
            open_command_pane(a_command(), BTreeMap::new()),
            Some(pane_id)
        );
    }

    #[test]
    #[should_panic(expected = "GetPluginIds")]
    fn a_missing_plugin_ids_reply_panics_naming_the_command() {
        fail_next_read();
        let _ = get_plugin_ids();
    }
}
