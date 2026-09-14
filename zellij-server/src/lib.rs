// test comment line 1 - this is just harmless placeholder text for a test change
// test comment line 2 - this is just harmless placeholder text for a test change
// test comment line 3 - this is just harmless placeholder text for a test change
// test comment line 4 - this is just harmless placeholder text for a test change
// test comment line 5 - this is just harmless placeholder text for a test change
// test comment line 6 - this is just harmless placeholder text for a test change
// test comment line 7 - this is just harmless placeholder text for a test change
// test comment line 8 - this is just harmless placeholder text for a test change
// test comment line 9 - this is just harmless placeholder text for a test change
// test comment line 10 - this is just harmless placeholder text for a test change
// test comment 1
// test comment 2
// test comment 3
// test comment 4
// test comment 5
pub(crate) use zellij_bus::os_input_output_unix;

#[cfg(target_os = "windows")]
compile_error!("Zellij does not support Windows");

pub use zellij_bus::os_input_output;
pub use zellij_panes::{panes, tab};
pub use zellij_process::firecracker;
pub use zellij_terminal::host_query;
pub use zellij_terminal::output;
use zellij_types::internal_ids::TabPosition;
use zellij_types::style::default_palette;

pub use zellij_bus::background_jobs;
pub(crate) use zellij_bus::global_async_runtime;
pub use zellij_bus::nested_guest;
pub mod notifications;
pub(crate) use zellij_panes::pane_groups;
pub(crate) use zellij_plugin_host as plugins;
pub use zellij_plugin_host::deno::testing as plugin_testing;
use zellij_plugin_host::get_engine;
#[doc(hidden)]
pub use zellij_plugin_host::run_compilation_worker;
mod locked_escape;
pub use zellij_plugin_host::plugin_authoring;
mod pty;
#[cfg(unix)]
pub use zellij_process::pty_host;
mod client_router;
pub(crate) use zellij_bus::pty_writer;
mod route;
mod screen;
pub(crate) use zellij_bus::server_instruction::ServerInstruction;
pub(crate) use zellij_bus::session_layout_metadata;
pub(crate) use zellij_bus::session_recorder;
mod terminal_bytes;
pub(crate) use zellij_bus::thread_bus;
pub(crate) use zellij_bus::warp;
pub(crate) use zellij_panes::ui;

use background_jobs::{background_jobs_main, BackgroundJob};
use log::info;
use pty_writer::{pty_writer_main, PtyWriteInstruction};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, RwLock,
    },
    thread,
    time::Duration,
};

/// How long shutdown waits for queued client messages to reach their sockets.
/// Generous enough for a live client to drain, short enough that a wedged one
/// cannot keep the server process alive.
const CLIENT_WRITER_FLUSH_TIMEOUT: Duration = Duration::from_millis(500);

/// How long a freshly started server waits for its first client before giving
/// up. Must exceed the client's own `CONNECT_TO_SERVER_TIMEOUT` (30s) with
/// margin, so the server never leaves before a slow client finishes connecting.
const FIRST_CLIENT_TIMEOUT: Duration = Duration::from_secs(60);

/// How long session shutdown waits for the plugin thread. It exits once every plugin is
/// unloaded, and a plugin that is looping is cancelled first (`PluginEnv::cancel`) — but a
/// guest that never calls the host cannot be, and joining it without a bound once held a dead
/// session's process open indefinitely, with every hook client that connected piling up
/// behind the join. Past this the thread is abandoned and the exit goes on.
const PLUGIN_THREAD_EXIT_TIMEOUT: Duration = Duration::from_secs(5);

/// How many client connections the server will serve at once.
///
/// Every accepted connection costs two threads — the router that reads it and
/// the writer that answers it — and nothing in the accept path could ever say
/// no: `SessionState::new_client` hands out an id unconditionally. A server
/// that leaked connections therefore leaked threads without bound, and one
/// observed here reached 65,538 of them, at which point its listen backlog was
/// permanently full and a blocking `connect` to it never returned. That hung
/// every `zellij`, `zellij ls` and `zellij attach` on the machine, for every
/// unrelated session, because they all sweep the socket directory.
///
/// Far above any real load — a session has a handful of clients, plus the
/// short-lived `zellij action` and hook invocations passing through — so
/// reaching it means something is wrong, and refusing is strictly better than
/// the alternative of taking the whole machine's zellij down with it.
const MAX_CONCURRENT_CLIENT_CONNECTIONS: usize = 512;

/// How long a freshly accepted connection may stay silent before its router
/// gives up on it.
///
/// The cap above is only a fix if a slot cannot be held forever, and a peer
/// that connects and never writes would hold one exactly that long: the
/// router blocks reading it, and every `RemoveClient` — hence every
/// `shutdown_reads` that would unblock the router — is sent by the router
/// itself. So until the first message the read is bounded by this deadline,
/// after which the connection is treated as abandoned and its slot released.
///
/// Only the first message: every real client writes the moment it connects
/// (`AttachClient`, `ConnStatus`, an `Action`), and a later read must not be
/// bounded, because a deadline expiring halfway through a large frame would
/// desynchronise a connection that was merely slow. Equal to the client's
/// `CONNECT_TO_SERVER_TIMEOUT`, so it cannot fire on a client that is only
/// slow to be scheduled.
const FIRST_CLIENT_MESSAGE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the accept loop pauses after an error before trying again, so that
/// a persistent failure (`EMFILE`, typically) does not become a spin.
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(100);

/// Holds one of the `MAX_CONCURRENT_CLIENT_CONNECTIONS` slots for as long as a
/// router thread lives.
///
/// A guard rather than a decrement at the end of the router, so that a router
/// which panics still gives its slot back — the panic hook turns a router panic
/// into a server error rather than an unwind that skips cleanup, but a leaked
/// slot would be permanent either way.
struct ConnectionSlot(Arc<AtomicUsize>);

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
use zellij_runtime::envs;
use zellij_storage::data_access::DataAccess;
use zellij_types::pane_size::Size;

use zellij_types::input::cli_assets::CliAssets;
use {
    zellij_types::input::options::PaneFrameStyle,
    zellij_types::input::options::DEFAULT_AGENT_PANE_MIN_COLUMNS,
    zellij_types::input::options::DEFAULT_AGENT_PANE_MIN_ROWS,
    zellij_types::input::options::DEFAULT_WORD_SEPARATORS,
};

use crate::{
    os_input_output::ServerOsApi,
    panes::PaneId,
    plugins::{plugin_thread_main, PluginInstruction},
    pty::{get_default_shell, pty_thread_main, Pty, PtyInstruction},
    screen::{screen_thread_main, ScreenInstruction},
    thread_bus::{Bus, ThreadSenders},
};
use route::{route_thread_main, NotificationEnd};
use zellij_runtime::channels;
use zellij_runtime::channels::ChannelWithContext;
use zellij_runtime::channels::SenderWithContext;
use zellij_runtime::errors::prelude::*;
use zellij_runtime::errors::FatalError;
use zellij_runtime::home::default_layout_dir;
use zellij_runtime::home::get_default_data_dir;
use zellij_runtime::home::user_plugin_dir_or_default;
use zellij_types::diagnostics::ContextType;
use zellij_types::keys::KeyWithModifier;
use zellij_types::style::Style;
use {
    zellij_config::config::watch_config_file_changes,
    zellij_config::config::watch_layout_dir_changes, zellij_config::config::Config,
    zellij_ipc::ipc::ClientAttributes, zellij_ipc::ipc::ExitReason,
    zellij_ipc::ipc::ServerToClientMsg, zellij_runtime::consts::DEFAULT_SCROLL_BUFFER_SIZE,
    zellij_runtime::consts::SCROLL_BUFFER_SIZE,
    zellij_runtime::consts::ZELLIJ_SEEN_RELEASE_NOTES_CACHE_FILE, zellij_types::data::InputMode,
    zellij_types::data::LayoutInfo, zellij_types::data::LayoutWithError,
    zellij_types::input::actions::Action, zellij_types::input::command::RunCommand,
    zellij_types::input::command::TerminalAction, zellij_types::input::keybinds::Keybinds,
    zellij_types::input::layout::FloatingPaneLayout, zellij_types::input::layout::Layout,
    zellij_types::input::layout::PluginAlias, zellij_types::input::layout::Run,
    zellij_types::input::layout::RunPluginOrAlias, zellij_types::input::options::Options,
    zellij_types::input::plugins::PluginAliases,
};

pub use zellij_types::internal_ids::{ClientId, PluginId, TerminalId};

#[derive(Debug, Clone, Default)]
pub(crate) struct SessionConfiguration {
    runtime_config: HashMap<ClientId, Config>, // if present, overrides the saved_config
    effective_keybinds: HashMap<ClientId, Keybinds>,
    plugin_default_keybinds: BTreeMap<PluginId, Keybinds>,
    saved_config: Config, // the config as it is on disk (not guaranteed),
                          // when changed, this resets the runtime config to
                          // be identical to it and override any previous
                          // changes
}

impl SessionConfiguration {
    pub fn change_saved_config(&mut self, new_saved_config: Config) -> Vec<(ClientId, Config)> {
        self.saved_config = new_saved_config.clone();

        let mut config_changes = vec![];
        let client_ids: Vec<_> = self.runtime_config.keys().copied().collect();
        for client_id in client_ids {
            if self.runtime_config.get(&client_id) != Some(&new_saved_config) {
                self.runtime_config
                    .insert(client_id, new_saved_config.clone());
                config_changes.push((client_id, new_saved_config.clone()))
            }
        }
        self.recompute_effective_keybinds();
        config_changes
    }
    pub fn set_saved_configuration(&mut self, config: Config) {
        self.saved_config = config;
        self.recompute_effective_keybinds();
    }
    pub fn set_client_runtime_configuration(&mut self, client_id: ClientId, client_config: Config) {
        self.runtime_config.insert(client_id, client_config);
        self.recompute_effective_keybinds();
    }
    pub fn get_client_keybinds(&self, client_id: &ClientId) -> &Keybinds {
        self.runtime_config
            .get(client_id)
            .and_then(|_| self.effective_keybinds.get(client_id))
            .unwrap_or(&self.saved_config.keybinds)
    }
    pub fn get_client_default_input_mode(&self, client_id: &ClientId) -> InputMode {
        self.runtime_config
            .get(client_id)
            .or(Some(&self.saved_config))
            .and_then(|c| c.options.default_mode)
            .unwrap_or_default()
    }
    pub fn get_client_locked_escape_timeout(&self, client_id: &ClientId) -> Duration {
        let millis = self
            .runtime_config
            .get(client_id)
            .or(Some(&self.saved_config))
            .and_then(|c| c.options.locked_escape_timeout_ms)
            .unwrap_or(zellij_types::units::ElapsedMillis::new(
                locked_escape::DEFAULT_LOCKED_ESCAPE_TIMEOUT_MS,
            ));
        Duration::from_millis((millis).get())
    }
    pub fn get_client_configuration(&self, client_id: &ClientId) -> Config {
        self.runtime_config
            .get(client_id)
            .or(Some(&self.saved_config))
            .cloned()
            .unwrap_or_default()
    }
    pub fn reconfigure_runtime_config(
        &mut self,
        client_id: &ClientId,
        stringified_config: String,
    ) -> (Option<Config>, bool) {
        // bool is whether the config changed
        let mut full_reconfigured_config = None;
        let mut config_changed = false;
        let current_client_configuration = self.get_client_configuration(client_id);
        match Config::from_kdl(
            &stringified_config,
            Some(current_client_configuration.clone()),
        ) {
            Ok(new_config) => {
                config_changed = current_client_configuration != new_config;
                self.runtime_config.insert(*client_id, new_config);
                self.recompute_effective_keybinds();
                full_reconfigured_config = Some(self.get_client_configuration(client_id));
            },
            Err(e) => {
                log::error!("Failed to reconfigure runtime config: {}", e);
            },
        }
        (full_reconfigured_config, config_changed)
    }
    pub fn rebind_keys(
        &mut self,
        client_id: &ClientId,
        keys_to_rebind: Vec<(InputMode, KeyWithModifier, Vec<Action>)>,
        keys_to_unbind: Vec<(InputMode, KeyWithModifier)>,
    ) -> (Option<Config>, bool) {
        let mut full_reconfigured_config = None;
        let mut config_changed = false;

        if !self.runtime_config.contains_key(client_id) {
            self.runtime_config
                .insert(*client_id, self.saved_config.clone());
        }
        match self.runtime_config.get_mut(client_id) {
            Some(config) => {
                for (input_mode, key_with_modifier) in keys_to_unbind {
                    let keys_in_mode = config
                        .keybinds
                        .0
                        .entry(input_mode)
                        .or_insert_with(Default::default);
                    let removed = keys_in_mode.remove(&key_with_modifier);
                    if removed.is_some() {
                        config_changed = true;
                    }
                }
                for (input_mode, key_with_modifier, actions) in keys_to_rebind {
                    let keys_in_mode = config
                        .keybinds
                        .0
                        .entry(input_mode)
                        .or_insert_with(Default::default);
                    if keys_in_mode.get(&key_with_modifier) != Some(&actions) {
                        config_changed = true;
                        keys_in_mode.insert(key_with_modifier, actions);
                    }
                }
                if config_changed {
                    full_reconfigured_config = Some(config.clone());
                }
            },
            None => {
                log::error!(
                    "Could not find runtime or saved configuration for client, cannot rebind keys"
                );
            },
        }
        if config_changed {
            self.recompute_effective_keybinds();
        }

        (full_reconfigured_config, config_changed)
    }

    pub fn register_plugin_default_keybinds(
        &mut self,
        plugin_id: PluginId,
        keybinds: Keybinds,
    ) -> Vec<(ClientId, Config)> {
        if keybinds.0.is_empty() {
            self.plugin_default_keybinds.remove(&plugin_id);
        } else {
            self.plugin_default_keybinds.insert(plugin_id, keybinds);
        }
        self.recompute_effective_keybinds();
        self.runtime_config
            .keys()
            .copied()
            .map(|client_id| (client_id, self.get_client_configuration(&client_id)))
            .collect()
    }

    fn with_plugin_defaults(&self, mut config: Config) -> Config {
        for defaults in self.plugin_default_keybinds.values() {
            for (mode, bindings) in &defaults.0 {
                let effective = config.keybinds.0.entry(*mode).or_default();
                for (key, actions) in bindings {
                    effective
                        .entry(key.clone())
                        .or_insert_with(|| actions.clone());
                }
            }
        }
        config
    }

    fn recompute_effective_keybinds(&mut self) {
        self.effective_keybinds = self
            .runtime_config
            .iter()
            .map(|(client_id, config)| {
                (
                    *client_id,
                    self.with_plugin_defaults(config.clone()).keybinds,
                )
            })
            .collect();
    }
}

pub(crate) struct SessionMetaData {
    pub senders: ThreadSenders,
    pub default_shell: Option<TerminalAction>,
    pub current_input_modes: HashMap<ClientId, InputMode>,
    pub session_configuration: SessionConfiguration,
    pub key_passthrough_clients: HashMap<ClientId, PaneId>,
    pub locked_escapes: HashMap<ClientId, locked_escape::LockedEscape>,
    screen_thread: Option<thread::JoinHandle<()>>,
    pty_thread: Option<thread::JoinHandle<()>>,
    plugin_thread: Option<thread::JoinHandle<()>>,
    pty_writer_thread: Option<thread::JoinHandle<()>>,
    background_jobs_thread: Option<thread::JoinHandle<()>>,
    session_recorder_thread: Option<thread::JoinHandle<()>>,
    config_file_path: Option<PathBuf>,
    /// The session's `--config-dir` / `ZELLIJ_CONFIG_DIR`, kept so that a config reload can
    /// re-derive the folders that hang off it. Without it, reloading a config that does not
    /// name `user_plugin_dir` explicitly would wipe the one derived at startup.
    config_dir: Option<PathBuf>,
}

impl SessionMetaData {
    fn change_input_mode(
        &mut self,
        client_id: ClientId,
        input_mode: InputMode,
        completion: Option<NotificationEnd>,
    ) -> Result<()> {
        self.locked_escapes.remove(&client_id);
        let base_mode = self
            .session_configuration
            .get_client_default_input_mode(&client_id);
        self.current_input_modes.insert(client_id, input_mode);
        self.senders
            .send_to_screen(ScreenInstruction::ChangeMode(
                input_mode,
                Some(base_mode),
                client_id,
                completion,
            ))
            .context("changing client input mode")
    }

    pub fn get_client_keybinds_and_mode(
        &self,
        client_id: &ClientId,
    ) -> Option<(&Keybinds, &InputMode, InputMode)> {
        // (keybinds, current_input_mode,
        // default_input_mode)
        let client_keybinds = self.session_configuration.get_client_keybinds(client_id);
        let default_input_mode = self
            .session_configuration
            .get_client_default_input_mode(client_id);
        match self.current_input_modes.get(client_id) {
            Some(client_input_mode) => {
                Some((client_keybinds, client_input_mode, default_input_mode))
            },
            _ => None,
        }
    }
    pub fn remove_key_passthrough_client(&mut self, client_id: ClientId) {
        self.remove_key_passthrough_client_with_notify(client_id, true);
    }
    pub fn remove_key_passthrough_client_with_notify(
        &mut self,
        client_id: ClientId,
        notify_guest: bool,
    ) {
        self.locked_escapes.remove(&client_id);
        if let Some(pane_id) = self.key_passthrough_clients.remove(&client_id) {
            let pane_still_active = self.key_passthrough_clients.values().any(|p| *p == pane_id);
            if !pane_still_active && notify_guest {
                if let PaneId::Terminal(terminal_id) = pane_id {
                    let frame = zellij_ipc::nested_session::encode_frame(
                        &zellij_ipc::nested_session::NestedSessionMessage::FocusLost,
                    );
                    let _ = self.senders.send_to_pty_writer(PtyWriteInstruction::Write(
                        frame,
                        terminal_id,
                        None,
                    ));
                }
            }
        }
    }
    pub fn change_mode_for_all_clients(&mut self, input_mode: InputMode) {
        self.locked_escapes.clear();
        let all_clients: Vec<ClientId> = self.current_input_modes.keys().copied().collect();
        for client_id in all_clients {
            self.current_input_modes.insert(client_id, input_mode);
        }
    }
    pub fn propagate_configuration_changes(
        &mut self,
        config_changes: Vec<(ClientId, Config)>,
        config_was_written_to_disk: bool,
    ) {
        let mut new_plugin_config = None;
        for (client_id, new_config) in config_changes {
            self.locked_escapes.remove(&client_id);
            let new_config = self.session_configuration.with_plugin_defaults(new_config);
            if new_plugin_config.is_none() {
                new_plugin_config = Some(new_config.plugins.clone());
            }

            self.default_shell = new_config.options.default_shell.as_ref().map(|shell| {
                TerminalAction::RunCommand(RunCommand {
                    command: shell.clone(),
                    cwd: new_config.options.default_cwd.clone(),
                    use_terminal_title: true,
                    ..Default::default()
                })
            });
            let host_theme_dark = new_config
                .options
                .theme_dark
                .as_ref()
                .and_then(|name| new_config.theme_config(Some(name)));
            let host_theme_light = new_config
                .options
                .theme_light
                .as_ref()
                .and_then(|name| new_config.theme_config(Some(name)));
            if new_config.options.theme_dark.is_some() && host_theme_dark.is_none() {
                log::warn!(
                    "theme_dark='{}' not found in themes; auto-theme switch disabled for dark.",
                    new_config.options.theme_dark.as_deref().unwrap_or("?")
                );
            }
            if new_config.options.theme_light.is_some() && host_theme_light.is_none() {
                log::warn!(
                    "theme_light='{}' not found in themes; auto-theme switch disabled for light.",
                    new_config.options.theme_light.as_deref().unwrap_or("?")
                );
            }
            let pane_frame_style = PaneFrameStyle::from_options(&new_config.options);
            self.senders
                .send_to_screen(ScreenInstruction::Reconfigure {
                    agent_double_escape: new_config.options.agent_double_escape.unwrap_or(false),
                    client_id,
                    keybinds: new_config.keybinds.clone(),
                    default_mode: new_config.options.default_mode.unwrap_or_default(),
                    theme: new_config
                        .theme_config(new_config.options.theme.as_ref())
                        .unwrap_or_else(|| default_palette().into()),
                    host_theme_dark,
                    host_theme_light,
                    explicit_theme_hue: new_config.options.explicit_theme_hue,
                    simplified_ui: new_config.options.simplified_ui.unwrap_or(false),
                    default_shell: new_config.options.default_shell,
                    default_cwd: new_config.options.default_cwd.clone(),
                    pane_frame_style,
                    copy_command: new_config.options.copy_command,
                    copy_to_clipboard: new_config.options.copy_clipboard,
                    copy_on_select: new_config.options.copy_on_select.unwrap_or(true),
                    auto_layout: new_config.options.auto_layout.unwrap_or(true),
                    rounded_corners: new_config.ui.pane_frames.rounded_corners,
                    hide_session_name: new_config.ui.pane_frames.hide_session_name,
                    stacked_resize: new_config.options.stacked_resize.unwrap_or(true),
                    agent_pane_min_size: Size {
                        cols: new_config
                            .options
                            .agent_pane_min_columns
                            .unwrap_or(DEFAULT_AGENT_PANE_MIN_COLUMNS),
                        rows: new_config
                            .options
                            .agent_pane_min_rows
                            .unwrap_or(DEFAULT_AGENT_PANE_MIN_ROWS),
                    },
                    stacked_pane_list: new_config.options.stacked_pane_list.unwrap_or(true),
                    default_editor: new_config.options.scrollback_editor.clone(),
                    advanced_mouse_actions: new_config
                        .options
                        .advanced_mouse_actions
                        .unwrap_or(true),
                    mouse_scroll_resize: new_config.options.mouse_scroll_resize.unwrap_or(true),
                    mouse_hover_effects: new_config.options.mouse_hover_effects.unwrap_or(true),
                    visual_bell: new_config.options.visual_bell.unwrap_or(true),
                    pane_animations: new_config.options.pane_animations.unwrap_or(true),
                    focus_follows_mouse: new_config.options.focus_follows_mouse.unwrap_or(false),
                    mouse_click_through: new_config.options.mouse_click_through.unwrap_or(false),
                    context_menu_on_right_click: new_config
                        .options
                        .context_menu_on_right_click
                        .unwrap_or(false),
                    context_menu: new_config.context_menu.clone(),
                    osc133_command_selection: new_config
                        .options
                        .osc133_command_selection
                        .unwrap_or(true),
                    dangerously_enable_paste_buffer_read: new_config
                        .options
                        .dangerously_enable_paste_buffer_read
                        .unwrap_or(false),
                    word_separators: new_config
                        .options
                        .word_separators
                        .clone()
                        .unwrap_or_else(|| DEFAULT_WORD_SEPARATORS.to_owned()),
                    nested_session_handling: new_config
                        .options
                        .nested_session_handling
                        .unwrap_or_default(),
                    host_notification_protocol: new_config
                        .options
                        .host_notification_protocol
                        .unwrap_or_default(),
                })
                .unwrap();
            self.senders
                .send_to_plugin(PluginInstruction::Reconfigure {
                    client_id,
                    keybinds: Some(new_config.keybinds),
                    default_mode: new_config.options.default_mode,
                    default_shell: self.default_shell.clone(),
                    layout_dir: new_config.options.layout_dir,
                    // Re-derived rather than passed through: `WasmBridge::reconfigure` assigns
                    // this verbatim, so a config that does not name the option explicitly would
                    // otherwise wipe the folder derived at session start. The server's cwd has
                    // not moved since then, so a relative folder resolves to the same place.
                    user_plugin_dir: user_plugin_dir_or_default(
                        new_config.options.user_plugin_dir,
                        self.config_dir.clone(),
                    ),
                    builtin_plugins_dir: new_config.options.builtin_plugins_dir,
                    hooks: new_config.hooks,
                    context_menu: new_config.context_menu,
                    was_written_to_disk: config_was_written_to_disk,
                })
                .unwrap();
            self.senders
                .send_to_pty(PtyInstruction::Reconfigure {
                    client_id,
                    default_editor: new_config.options.scrollback_editor,
                    post_command_discovery_hook: new_config.options.post_command_discovery_hook,
                })
                .unwrap();
        }

        // Detect and notify plugins of configuration changes
        if config_was_written_to_disk {
            if let Some(new_plugins) = new_plugin_config {
                self.senders
                    .send_to_plugin(PluginInstruction::DetectPluginConfigChanges(new_plugins))
                    .unwrap();
            }
        }
    }
}

impl Drop for SessionMetaData {
    fn drop(&mut self) {
        let _ = self.senders.send_to_pty(PtyInstruction::Exit);
        let _ = self.senders.send_to_screen(ScreenInstruction::Exit);
        let _ = self.senders.send_to_plugin(PluginInstruction::Exit);
        let _ = self.senders.send_to_pty_writer(PtyWriteInstruction::Exit);
        let _ = self.senders.send_to_background_jobs(BackgroundJob::Exit);
        if let Some(screen_thread) = self.screen_thread.take() {
            let _ = screen_thread.join();
        }
        if let Some(pty_thread) = self.pty_thread.take() {
            let _ = pty_thread.join();
        }
        if let Some(plugin_thread) = self.plugin_thread.take() {
            join_with_timeout(plugin_thread, PLUGIN_THREAD_EXIT_TIMEOUT, "plugin");
        }
        if let Some(pty_writer_thread) = self.pty_writer_thread.take() {
            let _ = pty_writer_thread.join();
        }
        if let Some(background_jobs_thread) = self.background_jobs_thread.take() {
            let _ = background_jobs_thread.join();
        }
        if let Some(recorder) = self.senders.session_recorder.take() {
            recorder.stop();
        }
        if let Some(recorder_thread) = self.session_recorder_thread.take() {
            let _ = recorder_thread.join();
        }
    }
}

/// Join `handle`, giving up after `timeout` with an error in the log rather than waiting on a
/// thread that may never finish. See `PLUGIN_THREAD_EXIT_TIMEOUT`.
fn join_with_timeout(handle: thread::JoinHandle<()>, timeout: Duration, name: &str) {
    let started = std::time::Instant::now();
    while !handle.is_finished() {
        if started.elapsed() >= timeout {
            log::error!(
                "the {name} thread did not exit within {timeout:?} of being told to; leaving it \
                 behind"
            );
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let _ = handle.join();
}

/// Remove a client from session state and synthesize empty
/// `ForwardedReplyFromHost` instructions for any host-query forwards
/// that had been dispatched to it. Without this, a client that goes
/// away while holding an in-flight forward leaves `Screen`'s
/// `forward_in_flight` flag stuck — the flag is only released by
/// replies, and no reply will ever come.
fn remove_client_and_flush_forwards(
    client_id: ClientId,
    os_input: &mut Box<dyn ServerOsApi>,
    session_state: &Arc<RwLock<SessionState>>,
    session_data: &Arc<RwLock<Option<SessionMetaData>>>,
) {
    let _ = os_input.remove_client(client_id);
    let stuck_tokens = session_state.write().unwrap().remove_client(client_id);
    if stuck_tokens.is_empty() {
        return;
    }
    if let Some(session) = session_data.read().unwrap().as_ref() {
        for token in stuck_tokens {
            let _ = session
                .senders
                .send_to_screen(ScreenInstruction::ForwardedReplyFromHost {
                    token,
                    reply_bytes: Vec::new(),
                });
        }
    }
}

macro_rules! remove_client {
    ($client_id:expr, $os_input:expr, $session_state:expr, $session_data:expr) => {
        $crate::remove_client_and_flush_forwards(
            $client_id,
            &mut $os_input,
            &$session_state,
            &$session_data,
        );
    };
}

macro_rules! send_to_client {
    ($client_id:expr, $os_input:expr, $msg:expr, $session_state:expr, $session_data:expr) => {
        let send_to_client_res = $os_input.send_to_client($client_id, $msg);
        if let Err(e) = send_to_client_res {
            // Try to recover the message
            let context = match e.downcast_ref::<ZellijError>() {
                Some(ZellijError::ClientTooSlow { .. }) => {
                    format!(
                        "client {} is processing server messages too slow",
                        $client_id
                    )
                },
                _ => {
                    format!("failed to route server message to client {}", $client_id)
                },
            };
            // Log it so it isn't lost
            Err::<(), _>(e).context(context).non_fatal();
            // failed to send to client, remove it
            remove_client!($client_id, $os_input, $session_state, $session_data);
        }
    };
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SessionState {
    clients: HashSet<ClientId>,
    interactive_clients: HashSet<ClientId>,
    pipes: HashMap<String, ClientId>,     // String => pipe_id
    watchers: HashSet<ClientId>,          // watcher clients (read-only observers)
    last_active_client: Option<ClientId>, // last client that sent a Key message
    /// Host-query forward tokens that have been dispatched to a
    /// specific client and are waiting for a reply. Used to clean up
    /// when that client disconnects (or when there's no client to
    /// dispatch to in the first place) — each stuck token gets an
    /// empty synthetic reply so `Screen`'s `forward_in_flight` slot
    /// releases and the queued forwards keep moving.
    forwards_in_flight: HashMap<u32, ClientId>,
}

impl SessionState {
    pub fn new() -> Self {
        SessionState {
            clients: HashSet::new(),
            interactive_clients: HashSet::new(),
            pipes: HashMap::new(),
            watchers: HashSet::new(),
            last_active_client: None,
            forwards_in_flight: HashMap::new(),
        }
    }
    pub fn new_client(&mut self) -> ClientId {
        let all_ids: HashSet<ClientId> = self
            .clients
            .iter()
            .copied()
            .chain(self.watchers.iter().copied())
            .collect();

        let mut next_client_id = 1;
        loop {
            if all_ids.contains(&ClientId::new(next_client_id)) {
                next_client_id += 1;
            } else {
                break;
            }
        }
        self.clients.insert(ClientId::new(next_client_id));
        ClientId::new(next_client_id)
    }
    pub fn associate_pipe_with_client(&mut self, pipe_id: String, client_id: ClientId) {
        self.pipes.insert(pipe_id, client_id);
    }
    /// Remove a client and return any host-query tokens that had
    /// been dispatched to this client and were still awaiting a
    /// reply. Callers must synthesize an empty reply for each token
    /// (via `ScreenInstruction::ForwardedReplyFromHost`) so
    /// `Screen`'s in-flight slot releases and any queued forwards
    /// can proceed.
    pub fn remove_client(&mut self, client_id: ClientId) -> Vec<u32> {
        self.clients.remove(&client_id);
        self.interactive_clients.remove(&client_id);
        self.pipes.retain(|_p_id, c_id| c_id != &client_id);
        self.clear_last_active_client(client_id);
        let stuck: Vec<u32> = self
            .forwards_in_flight
            .iter()
            .filter_map(|(token, owner)| (*owner == client_id).then_some(*token))
            .collect();
        for token in &stuck {
            self.forwards_in_flight.remove(token);
        }
        stuck
    }
    pub fn client_ids(&self) -> Vec<ClientId> {
        self.clients.iter().copied().collect()
    }
    fn input_unblock_clients(&self) -> Vec<ClientId> {
        self.interactive_clients.iter().copied().collect()
    }
    pub fn watcher_client_ids(&self) -> Vec<ClientId> {
        self.watchers.iter().copied().collect()
    }
    pub fn get_pipe(&self, pipe_name: &str) -> Option<ClientId> {
        self.pipes.get(pipe_name).copied()
    }
    pub fn active_clients_are_connected(&self) -> bool {
        let ids_of_pipe_clients: HashSet<ClientId> = self.pipes.values().copied().collect();
        let mut active_clients_connected = false;
        for client_id in &self.clients {
            if ids_of_pipe_clients.contains(client_id) {
                continue;
            }
            active_clients_connected = true;
        }
        active_clients_connected
    }
    pub fn convert_client_to_watcher(&mut self, client_id: ClientId) {
        self.clients.remove(&client_id);
        self.interactive_clients.remove(&client_id);
        self.watchers.insert(client_id);
    }
    pub fn is_watcher(&self, client_id: &ClientId) -> bool {
        self.watchers.contains(client_id)
    }
    pub fn remove_watcher(&mut self, client_id: ClientId) {
        self.watchers.remove(&client_id);
    }
    pub fn set_last_active_client(&mut self, client_id: ClientId) {
        self.last_active_client = Some(client_id);
    }
    pub fn get_last_active_client(&self) -> Option<ClientId> {
        self.last_active_client
    }
    pub fn clear_last_active_client(&mut self, client_id: ClientId) {
        if self.last_active_client == Some(client_id) {
            self.last_active_client = None;
        }
    }
    pub fn mark_forward_in_flight(&mut self, token: u32, client_id: ClientId) {
        self.forwards_in_flight.insert(token, client_id);
    }
    pub fn clear_forward_in_flight(&mut self, token: u32) {
        self.forwards_in_flight.remove(&token);
    }
    /// Client to route a host-query forward to. Prefers whichever
    /// client most recently interacted with the session; falls back
    /// to any currently-connected non-watcher client. Returns `None`
    /// only when no regular client is connected.
    pub fn pick_forward_target(&self) -> Option<ClientId> {
        if let Some(candidate) = self.last_active_client {
            if self.clients.contains(&candidate) {
                return Some(candidate);
            }
        }
        self.clients.iter().copied().next()
    }
}

pub fn start_server(os_input: Box<dyn ServerOsApi>, socket_path: PathBuf) {
    info!("Starting Zellij server!");

    use nix::sys::stat::{umask, Mode};
    // preserve the current umask: read current value by setting to another mode, and then restoring it
    let current_umask = umask(Mode::all());
    umask(current_umask);
    daemonize::Daemonize::new()
        .working_directory(std::env::current_dir().unwrap())
        .umask(current_umask.bits() as u32)
        .start()
        .expect("could not daemonize the server process");

    start_server_impl(os_input, socket_path, true);
}

pub fn start_server_impl(
    mut os_input: Box<dyn ServerOsApi>,
    socket_path: PathBuf,
    install_panic_hook: bool,
) {
    envs::set_zellij("0".to_string());

    let (to_server, server_receiver): ChannelWithContext<ServerInstruction> = channels::bounded(50);
    let to_server = SenderWithContext::new(to_server);
    let session_data: Arc<RwLock<Option<SessionMetaData>>> = Arc::new(RwLock::new(None));
    let session_state = Arc::new(RwLock::new(SessionState::new()));
    let mut attach_traces_waiting_for_first_render: HashMap<ClientId, std::time::Instant> =
        HashMap::new();

    if install_panic_hook {
        std::panic::set_hook({
            use zellij_runtime::errors::handle_panic;
            let to_server = to_server.clone();
            Box::new(move |info| {
                handle_panic(info, Some(&to_server));
            })
        });
    }

    let _ = thread::Builder::new()
        .name("server_listener".to_string())
        .spawn({
            use interprocess::local_socket::prelude::*;
            use zellij_ipc::consts::ipc_bind;
            use zellij_runtime::shared::set_permissions;

            let os_input = os_input.clone();
            let session_data = session_data.clone();
            let session_state = session_state.clone();
            let to_server = to_server.clone();
            let socket_path = socket_path.clone();
            let live_connections = Arc::new(AtomicUsize::new(0));
            move || {
                drop(std::fs::remove_file(&socket_path));
                let listener = ipc_bind(&socket_path).unwrap();
                // set the sticky bit to avoid the socket file being potentially cleaned up
                // https://specifications.freedesktop.org/basedir-spec/basedir-spec-latest.html states that for XDG_RUNTIME_DIR:
                // "To ensure that your files are not removed, they should have their access time timestamp modified at least once every 6 hours of monotonic time or the 'sticky' bit should be set on the file. "
                // It is not guaranteed that all platforms allow setting the sticky bit on sockets!
                drop(set_permissions(&socket_path, 0o1700));

                for stream in listener.incoming() {
                    match stream {
                        Ok(stream) => {
                            let mut os_input = os_input.clone();
                            let accepted_at = std::time::Instant::now();
                            // Claim a slot before anything is allocated for this
                            // connection. Over the cap the stream is dropped
                            // unserved, which the peer sees as an immediate EOF —
                            // the same thing it would see from a server that had
                            // gone away, and far better than being accepted into a
                            // process that is already past saving.
                            let in_flight =
                                live_connections.fetch_add(1, Ordering::SeqCst) + 1;
                            let slot = ConnectionSlot(live_connections.clone());
                            if in_flight > MAX_CONCURRENT_CLIENT_CONNECTIONS {
                                if in_flight == MAX_CONCURRENT_CLIENT_CONNECTIONS + 1 {
                                    // Once per crossing, not once per refusal: a
                                    // peer hammering the socket must not also
                                    // flood the log.
                                    log::error!(
                                        "refusing client connections: {MAX_CONCURRENT_CLIENT_CONNECTIONS} already open. \
                                         Something is opening connections without closing them."
                                    );
                                }
                                drop(slot);
                                drop(stream);
                                continue;
                            }
                            let client_id = session_state.write().unwrap().new_client();
                            log::info!(
                                "attach trace: server listener allocated client_id={client_id} after {:?}; registering IPC client",
                                accepted_at.elapsed()
                            );

                            let receiver = match os_input.new_client(client_id, stream) {
                                Ok(receiver) => receiver,
                                Err(err) => {
                                    // The same outcome as the refusal above, and
                                    // for the same reason: this used to `unwrap`,
                                    // which took the listener thread down with it
                                    // and left the session running but
                                    // unreachable. Registering the client dups
                                    // its socket, so `EMFILE` reaches here too.
                                    Err::<(), _>(err)
                                        .with_context(|| {
                                            format!("failed to register client {client_id}")
                                        })
                                        .non_fatal();
                                    // Give the id back. Nothing has been dispatched
                                    // to a client this new, so there are no
                                    // in-flight forwards to flush, and no sender
                                    // was registered for `os_input.remove_client`
                                    // to remove.
                                    let _ = session_state.write().unwrap().remove_client(client_id);
                                    drop(slot);
                                    continue;
                                },
                            };
                            log::info!(
                                "attach trace: server listener registered IPC client_id={client_id} after {:?}; spawning router",
                                accepted_at.elapsed()
                            );

                            let session_data = session_data.clone();
                            let session_state = session_state.clone();
                            let to_server = to_server.clone();
                            thread::Builder::new()
                                .name("server_router".to_string())
                                .spawn(move || {
                                    // Held for the life of the router, released
                                    // however it ends.
                                    let _slot = slot;
                                    route_thread_main(
                                        session_data,
                                        session_state,
                                        os_input,
                                        to_server,
                                        receiver,
                                        client_id,
                                        FIRST_CLIENT_MESSAGE_TIMEOUT,
                                    )
                                    .fatal()
                                })
                                .unwrap();
                            log::info!(
                                "attach trace: server listener router spawned client_id={client_id} after {:?}",
                                accepted_at.elapsed()
                            );
                        },
                        Err(err) => {
                            // Not fatal. This used to panic, which killed the
                            // listener thread outright and left the session
                            // running but unreachable, with nothing in the log
                            // to say why. The realistic cause is `EMFILE` from
                            // connections piling up, which is transient once
                            // the cap above starts refusing them.
                            log::error!("failed to accept a client connection: {err:?}");
                            thread::sleep(ACCEPT_ERROR_BACKOFF);
                        },
                    }
                }
            }
        });

    // Why the loop below stopped. Set at every break so that the reason a
    // session ended is written down: without it, a server shutdown leaves
    // nothing in the log at all, and "the session died" is indistinguishable
    // from "the session was closed" after the fact.
    let exit_reason: &str;
    let mut kill_session = false;
    let mut kill_completion_client = None;
    // Whether the socket file was already unlinked by a `break` path below. It must not be
    // unlinked twice: between the two would be the thread joins, long enough for a new server
    // to bind the same path, and the second unlink would then delete *its* socket.
    let mut socket_removed = false;
    // Until a first client has initialised the session, the wait is bounded by
    // this one deadline. The client that spawned this server gives up on it
    // after `CONNECT_TO_SERVER_TIMEOUT` (30s) and exits with no way to kill
    // what it started; a server left like that would sit forever with a bound
    // socket and no session state. A fixed deadline rather than a per-message
    // timeout, so that probes from `zellij ls` — which arrive here too, and
    // are answered without a session — cannot keep an orphan alive by turning
    // up every so often. Once there is a session the loop waits without a
    // deadline as before: a detached session is meant to outlive its clients.
    let first_client_deadline = std::time::Instant::now() + FIRST_CLIENT_TIMEOUT;
    loop {
        let recv = if session_data.read().unwrap().is_none() {
            match server_receiver.recv_deadline(first_client_deadline) {
                Ok(recv) => Ok(recv),
                Err(channels::RecvTimeoutError::Timeout) => {
                    exit_reason = "no client attached within the startup timeout";
                    break;
                },
                Err(channels::RecvTimeoutError::Disconnected) => Err(channels::RecvError),
            }
        } else {
            server_receiver.recv()
        };
        let (instruction, mut err_ctx) = recv.unwrap();
        zellij_runtime::errors::add_call(
            &mut err_ctx,
            ContextType::IPCServer((&instruction).into()),
        );
        match &instruction {
            ServerInstruction::AttachClient(_, _, _, client_id) => log::info!(
                "attach trace: server main loop received AttachClient client_id={client_id}"
            ),
            ServerInstruction::ConnStatus(client_id) => log::info!(
                "attach trace: server main loop received ConnStatus client_id={client_id}"
            ),
            _ => {},
        }
        match instruction {
            ServerInstruction::FirstClientConnected(cli_assets, client_id) => {
                let host_terminal_env = cli_assets.host_terminal_env.clone();
                let (config, layout) =
                    match zellij_config::cli_assets::load_config_and_layout(&cli_assets) {
                        Ok(config_and_layout) => config_and_layout,
                        Err(error) => {
                            let _ = os_input.send_to_client(
                                client_id,
                                ServerToClientMsg::Exit {
                                    exit_reason: ExitReason::Error(format!(
                                        "failed to load session checkpoint: {error:#}"
                                    )),
                                },
                            );
                            Err::<(), _>(error)
                                .context("failed to load session checkpoint")
                                .non_fatal();
                            remove_client!(client_id, os_input, session_state, session_data);
                            exit_reason = "failed to load session checkpoint";
                            break;
                        },
                    };
                let layout_is_welcome_screen = cli_assets.layout
                    == Some(LayoutInfo::BuiltIn("welcome".to_owned()))
                    || config.options.default_layout == Some(PathBuf::from("welcome"));

                let successfully_written_config = Config::write_config_to_disk_if_it_does_not_exist(
                    config.to_string(true),
                    &cli_assets.config_file_path,
                );
                // if we successfully wrote the config to disk, it means two things:
                // 1. It did not exist beforehand
                // 2. The config folder is writeable
                //
                // If these two are true, we should launch the setup wizard, if even one of them is
                // false, we should never launch it.
                let should_launch_setup_wizard = successfully_written_config;

                let runtime_config_options = match &cli_assets.configuration_options {
                    Some(configuration_options) => {
                        config.options.merge(configuration_options.clone())
                    },
                    None => config.options.clone(),
                };

                let client_attributes = ClientAttributes {
                    size: cli_assets.terminal_window_size,
                    style: Style {
                        colors: config
                            .theme_config(runtime_config_options.theme.as_ref())
                            .unwrap_or_else(|| default_palette().into()),
                        rounded_corners: config.ui.pane_frames.rounded_corners,
                        hide_session_name: config.ui.pane_frames.hide_session_name,
                    },
                };

                info!("FirstClientConnected: initializing session");
                let mut session = init_session(
                    os_input.clone(),
                    to_server.clone(),
                    client_attributes.clone(),
                    Box::new(runtime_config_options.clone()), // TODO: no box
                    Box::new(layout.clone()),                 // TODO: no box
                    cli_assets.clone(),
                    config.clone(),
                    config.plugins.clone(),
                    client_id,
                );
                info!("FirstClientConnected: session initialized, spawning tabs");
                let mut runtime_configuration = config.clone();
                runtime_configuration.options = runtime_config_options.clone();
                session
                    .session_configuration
                    .set_saved_configuration(config.clone());
                session
                    .session_configuration
                    .set_client_runtime_configuration(client_id, runtime_configuration);
                let default_input_mode = runtime_config_options.default_mode.unwrap_or_default();
                session
                    .current_input_modes
                    .insert(client_id, default_input_mode);
                session_state
                    .write()
                    .unwrap()
                    .interactive_clients
                    .insert(client_id);

                *session_data.write().unwrap() = Some(session);
                session_data
                    .read()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .senders
                    .send_to_screen(ScreenInstruction::RecomputeTabSize(
                        client_id,
                        client_attributes.size,
                    ))
                    .unwrap();

                let default_shell = runtime_config_options.default_shell.map(|shell| {
                    TerminalAction::RunCommand(RunCommand {
                        command: shell,
                        cwd: config.options.default_cwd.clone(),
                        use_terminal_title: true,
                        ..Default::default()
                    })
                });
                let cwd = cli_assets.cwd.or(runtime_config_options.default_cwd);

                let spawn_tabs = |tab_layout,
                                  floating_panes_layout,
                                  tab_name,
                                  swap_layouts,
                                  should_focus_tab| {
                    session_data
                        .read()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .senders
                        .send_to_screen(ScreenInstruction::NewTab(
                            cwd.clone(),
                            default_shell.clone(),
                            tab_layout,
                            floating_panes_layout,
                            tab_name,
                            swap_layouts,
                            None,  // initial_panes
                            false, // block_on_first_terminal
                            should_focus_tab,
                            client_id,
                            None,
                        ))
                        .unwrap()
                };

                if layout.has_tabs() {
                    let focused_tab_index =
                        layout.focused_tab_index().unwrap_or(TabPosition::new(0));
                    for (tab_index, (tab_name, tab_layout, floating_panes_layout)) in
                        layout.tabs().into_iter().enumerate()
                    {
                        let should_focus_tab = TabPosition::new(tab_index) == focused_tab_index;
                        spawn_tabs(
                            Some(tab_layout.clone()),
                            floating_panes_layout.clone(),
                            tab_name,
                            (
                                Some(layout.swap_tiled_layouts.clone()),
                                Some(layout.swap_floating_layouts.clone()),
                            ),
                            should_focus_tab,
                        );
                    }
                } else {
                    let mut floating_panes =
                        layout.template.map(|t| t.1).clone().unwrap_or_default();
                    if should_launch_setup_wizard {
                        // we only do this here (and only once) because otherwise it will be
                        // intrusive
                        let setup_wizard = setup_wizard_floating_pane();
                        floating_panes.push(setup_wizard);
                    } else if should_show_release_notes(
                        runtime_config_options.show_release_notes,
                        layout_is_welcome_screen,
                    ) {
                        let about = about_floating_pane();
                        floating_panes.push(about);
                    } else if should_show_startup_tip(
                        runtime_config_options.show_startup_tips,
                        layout_is_welcome_screen,
                    ) {
                        let tip = tip_floating_pane();
                        floating_panes.push(tip);
                    }
                    spawn_tabs(
                        None,
                        floating_panes,
                        None,
                        (
                            Some(layout.swap_tiled_layouts.clone()),
                            Some(layout.swap_floating_layouts.clone()),
                        ),
                        true,
                    );
                }
                {
                    let rlock = session_data.read().unwrap();
                    let session_data = rlock.as_ref().unwrap();
                    session_data
                        .senders
                        .send_to_plugin(PluginInstruction::AddClient(client_id))
                        .unwrap();
                    session_data
                        .senders
                        .send_to_screen(ScreenInstruction::SetClientHostTerminalEnv(
                            client_id,
                            host_terminal_env,
                        ))
                        .unwrap();
                }
            },
            ServerInstruction::AttachClient(
                cli_assets,
                tab_number_to_focus,
                pane_id_to_focus,
                client_id,
            ) => {
                let attach_started_at = std::time::Instant::now();
                attach_traces_waiting_for_first_render.insert(client_id, attach_started_at);
                log::info!(
                    "attach trace: server main client_id={client_id} waiting for session_data write lock"
                );
                let mut rlock = session_data.write().unwrap();
                log::info!(
                    "attach trace: server main client_id={client_id} acquired session_data write lock after {:?}",
                    attach_started_at.elapsed()
                );
                let Some(session_data) = rlock.as_mut() else {
                    // No first client ever initialised this session — the one
                    // that spawned the server gave up before its first message
                    // arrived. There is nothing to attach to; tell the client
                    // rather than unwrap and take the whole server down.
                    drop(rlock);
                    let _ = os_input.send_to_client(
                        client_id,
                        ServerToClientMsg::Exit {
                            exit_reason: ExitReason::Error(
                                "the session was never initialised, nothing to attach to"
                                    .to_owned(),
                            ),
                        },
                    );
                    remove_client!(client_id, os_input, session_state, session_data);
                    attach_traces_waiting_for_first_render.remove(&client_id);
                    continue;
                };
                let config = session_data.session_configuration.saved_config.clone();
                let host_terminal_env = cli_assets.host_terminal_env.clone();
                let runtime_config_options = match cli_assets.configuration_options {
                    Some(configuration_options) => config.options.merge(configuration_options),
                    None => config.options.clone(),
                };

                let client_attributes = ClientAttributes {
                    size: cli_assets.terminal_window_size,
                    style: Style {
                        colors: config
                            .theme_config(runtime_config_options.theme.as_ref())
                            .unwrap_or_else(|| default_palette().into()),
                        rounded_corners: config.ui.pane_frames.rounded_corners,
                        hide_session_name: config.ui.pane_frames.hide_session_name,
                    },
                };

                let mut runtime_configuration = config.clone();
                runtime_configuration.options = runtime_config_options.clone();
                session_data
                    .session_configuration
                    .set_client_runtime_configuration(client_id, runtime_configuration);

                let default_input_mode = config.options.default_mode.unwrap_or_default();
                session_data
                    .current_input_modes
                    .insert(client_id, default_input_mode);
                session_state
                    .write()
                    .unwrap()
                    .interactive_clients
                    .insert(client_id);

                log::info!(
                    "attach trace: server main client_id={client_id} waiting for session_state write lock"
                );
                log::info!(
                    "attach trace: server main client_id={client_id} session state updated after {:?}",
                    attach_started_at.elapsed()
                );

                log::info!(
                    "attach trace: server main client_id={client_id} enqueue SetClientHostTerminalEnv"
                );
                session_data
                    .senders
                    .send_to_screen(ScreenInstruction::SetClientHostTerminalEnv(
                        client_id,
                        host_terminal_env,
                    ))
                    .unwrap();
                log::info!("attach trace: server main client_id={client_id} enqueue AddClient");
                session_data
                    .senders
                    .send_to_screen(ScreenInstruction::AddClient(
                        client_id,
                        client_attributes.size,
                        tab_number_to_focus,
                        pane_id_to_focus,
                    ))
                    .unwrap();
                log::info!(
                    "attach trace: server main client_id={client_id} enqueue Plugin AddClient"
                );
                session_data
                    .senders
                    .send_to_plugin(PluginInstruction::AddClient(client_id))
                    .unwrap();
                let default_mode = config.options.default_mode.unwrap_or_default();
                // ModeUpdate broadcast is handled by the screen thread via
                // change_mode() -> update_input_modes()
                log::info!("attach trace: server main client_id={client_id} enqueue ChangeMode");
                session_data
                    .senders
                    .send_to_screen(ScreenInstruction::ChangeMode(
                        default_mode,
                        Some(default_mode),
                        client_id,
                        None,
                    ))
                    .unwrap();
                log::info!(
                    "attach trace: server main client_id={client_id} attach dispatch complete after {:?}",
                    attach_started_at.elapsed()
                );
            },
            ServerInstruction::AttachWatcherClient(client_id, terminal_size) => {
                // the client_id was inserted into clients upon ipc tunnel initialization
                // now that it identified itself as a watcher, we need to convert it

                // Convert to watcher in SessionState (needed for input filtering in route.rs)
                session_state
                    .write()
                    .unwrap()
                    .convert_client_to_watcher(client_id);

                // Also notify Screen to add this as a watcher client (for rendering) with the terminal size
                session_data
                    .write()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .senders
                    .send_to_screen(ScreenInstruction::AddWatcherClient(
                        client_id,
                        terminal_size,
                    ))
                    .unwrap();
            },
            ServerInstruction::UnblockInputThread => {
                // Pane closes and plugin reloads release interactive input only. A CLI
                // must wait for its own route/completion, otherwise it exits before its
                // reply and that reply can reach a newly allocated client with the same id.
                let client_ids = session_state.read().unwrap().input_unblock_clients();
                for client_id in client_ids {
                    send_to_client!(
                        client_id,
                        os_input,
                        ServerToClientMsg::UnblockInputThread,
                        session_state,
                        session_data
                    );
                }
            },
            ServerInstruction::UnblockCliPipeInput(pipe_name) => {
                let pipe = session_state.read().unwrap().get_pipe(&pipe_name);
                if let Some(client_id) = pipe {
                    send_to_client!(
                        client_id,
                        os_input,
                        ServerToClientMsg::UnblockCliPipeInput { pipe_name },
                        session_state,
                        session_data
                    );
                }
            },
            ServerInstruction::CliPipeOutput(pipe_name, output) => {
                // Association is queued before dispatching the pipe to the plugin. An unknown
                // pipe has disconnected; broadcasting its late output can corrupt a probe's reply.
                let pipe = session_state.read().unwrap().get_pipe(&pipe_name);
                if let Some(client_id) = pipe {
                    send_to_client!(
                        client_id,
                        os_input,
                        ServerToClientMsg::CliPipeOutput { pipe_name, output },
                        session_state,
                        session_data
                    );
                }
            },
            ServerInstruction::EventUpdateToClient(client_id, event_type, event_json) => {
                send_to_client!(
                    client_id,
                    os_input,
                    ServerToClientMsg::EventUpdate {
                        event_type: event_type.clone(),
                        event_json: event_json.clone(),
                    },
                    session_state,
                    session_data
                );
            },
            ServerInstruction::ClientExit(client_id, completion_tx) => {
                attach_traces_waiting_for_first_render.remove(&client_id);
                let _ = os_input.send_to_client(
                    client_id,
                    ServerToClientMsg::Exit {
                        exit_reason: ExitReason::Normal,
                    },
                );

                // Check if this is a watcher
                let is_watcher = session_state.read().unwrap().is_watcher(&client_id);
                if is_watcher {
                    // Remove from SessionState watchers set
                    session_state.write().unwrap().remove_watcher(client_id);

                    // Also notify Screen to remove watcher
                    if let Some(session_data) = session_data.write().unwrap().as_ref() {
                        let _ = session_data
                            .senders
                            .send_to_screen(ScreenInstruction::RemoveWatcherClient(client_id));
                    }

                    os_input.remove_client(client_id).unwrap();
                } else {
                    if let Some(session_data) = session_data.write().unwrap().as_mut() {
                        session_data.remove_key_passthrough_client(client_id);
                    }
                    // Handle regular client removal
                    remove_client!(client_id, os_input, session_state, session_data);
                    drop(completion_tx); // prevent deadlock with route thread
                    session_data
                        .write()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .senders
                        .send_to_screen(ScreenInstruction::RemoveClient(client_id))
                        .unwrap();
                    session_data
                        .write()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .senders
                        .send_to_plugin(PluginInstruction::RemoveClient(client_id))
                        .unwrap();
                    if !session_state.read().unwrap().active_clients_are_connected() {
                        // Take it out under the lock, drop it outside. Dropping
                        // `SessionMetaData` joins the screen, pty and plugin
                        // threads, and assigning `None` through the guard would
                        // run those joins while still holding the write lock —
                        // parking this loop and, with it, every route thread
                        // waiting on `session_data.read()`. A session whose
                        // screen thread is slow to exit would take the whole
                        // server's client-connection path down with it, which is
                        // what makes an unrelated `zellij attach` hang.
                        // The guard is a temporary, so it is released at the
                        // semicolon — before the `drop` below.
                        //
                        // The socket goes first, for the same reason as at the
                        // end of this function: those joins are where a
                        // shutdown that waits on a stuck plugin thread spends
                        // its time, and a client connecting meanwhile — a
                        // hook's `zellij action agent-report`, a probe — must
                        // be refused, not parked behind them.
                        drop(std::fs::remove_file(&socket_path));
                        socket_removed = true;
                        let session_metadata = session_data.write().unwrap().take();
                        drop(session_metadata);
                        let client_ids_to_cleanup: Vec<ClientId> = session_state
                            .read()
                            .unwrap()
                            .clients
                            .iter()
                            .copied()
                            .collect();
                        // these are just the pipes
                        for client_id in client_ids_to_cleanup {
                            remove_client!(client_id, os_input, session_state, session_data);
                        }

                        let watcher_client_ids: Vec<ClientId> =
                            session_state.read().unwrap().watcher_client_ids();
                        for watcher_id in watcher_client_ids {
                            let _ = os_input.send_to_client(
                                watcher_id,
                                ServerToClientMsg::Exit {
                                    exit_reason: ExitReason::Normal,
                                },
                            );
                        }

                        exit_reason = "the last client exited";
                        break;
                    }
                }
            },
            ServerInstruction::RemoveClient(client_id) => {
                attach_traces_waiting_for_first_render.remove(&client_id);
                // Check if this is a watcher
                let is_watcher = session_state.read().unwrap().is_watcher(&client_id);
                if is_watcher {
                    // Remove from SessionState watchers set
                    session_state.write().unwrap().remove_watcher(client_id);

                    // Also notify Screen to remove watcher
                    if let Some(session_data) = session_data.write().unwrap().as_ref() {
                        let _ = session_data
                            .senders
                            .send_to_screen(ScreenInstruction::RemoveWatcherClient(client_id));
                    }

                    os_input.remove_client(client_id).unwrap();
                } else {
                    if let Some(session_data) = session_data.write().unwrap().as_mut() {
                        session_data.remove_key_passthrough_client(client_id);
                    }
                    // Handle regular client removal
                    remove_client!(client_id, os_input, session_state, session_data);
                    session_data
                        .write()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .senders
                        .send_to_screen(ScreenInstruction::RemoveClient(client_id))
                        .unwrap();
                    session_data
                        .write()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .senders
                        .send_to_plugin(PluginInstruction::RemoveClient(client_id))
                        .unwrap();
                }
            },
            ServerInstruction::KillSession | ServerInstruction::KillSessionWithCompletion(_) => {
                if let ServerInstruction::KillSessionWithCompletion(client_id) = instruction {
                    kill_completion_client = Some(client_id);
                }
                let client_ids = session_state.read().unwrap().client_ids();
                for client_id in client_ids {
                    if Some(client_id) == kill_completion_client {
                        continue;
                    }
                    let _ = os_input.send_to_client(
                        client_id,
                        ServerToClientMsg::Exit {
                            exit_reason: ExitReason::Normal,
                        },
                    );
                    remove_client!(client_id, os_input, session_state, session_data);
                }
                kill_session = true;
                exit_reason = "KillSession";
                break;
            },
            ServerInstruction::DisconnectAllClientsExcept(client_id) => {
                let client_ids: Vec<ClientId> = session_state
                    .read()
                    .unwrap()
                    .client_ids()
                    .iter()
                    .copied()
                    .filter(|c| c != &client_id)
                    .collect();
                for client_id in client_ids {
                    if let Some(session_data) = session_data.write().unwrap().as_mut() {
                        session_data.remove_key_passthrough_client(client_id);
                    }
                    let _ = os_input.send_to_client(
                        client_id,
                        ServerToClientMsg::Exit {
                            exit_reason: ExitReason::KickedByHost,
                        },
                    );
                    remove_client!(client_id, os_input, session_state, session_data);
                }
            },
            ServerInstruction::DetachSession(client_ids, completion_tx) => {
                for client_id in &client_ids {
                    if let Some(session_data) = session_data.write().unwrap().as_mut() {
                        session_data.remove_key_passthrough_client(*client_id);
                    }
                    let _ = os_input.send_to_client(
                        *client_id,
                        ServerToClientMsg::Exit {
                            exit_reason: ExitReason::Normal,
                        },
                    );
                    remove_client!(*client_id, os_input, session_state, session_data);
                }
                drop(completion_tx); // we do this here explicitly to signal that the clients have
                                     // already disconnected and to prevent a deadlock below caused
                                     // by us having to wait for session_data to send cleanup
                                     // signals to the various threads
                for client_id in client_ids {
                    session_data
                        .write()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .senders
                        .send_to_screen(ScreenInstruction::RemoveClient(client_id))
                        .unwrap();
                    session_data
                        .write()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .senders
                        .send_to_plugin(PluginInstruction::RemoveClient(client_id))
                        .unwrap();
                }
            },
            ServerInstruction::Render(serialized_output) => {
                let client_ids = session_state.read().unwrap().client_ids();
                // If `Some(_)`- unwrap it and forward it to the clients to render.
                // If `None`- Send an exit instruction. This is the case when a user closes the last Tab/Pane.
                if let Some(output) = &serialized_output {
                    for (client_id, client_render_instruction) in output.iter() {
                        let attach_started_at = attach_traces_waiting_for_first_render
                            .get(client_id)
                            .copied();
                        if let Some(started_at) = attach_started_at {
                            log::info!(
                                "attach trace: server main client_id={client_id} received first Render bytes={} after {:?}; client queue begin",
                                client_render_instruction.len(),
                                started_at.elapsed()
                            );
                        }
                        send_to_client!(
                            *client_id,
                            os_input,
                            ServerToClientMsg::Render {
                                content: client_render_instruction.clone()
                            },
                            session_state,
                            session_data
                        );
                        if let Some(started_at) = attach_started_at {
                            log::info!(
                                "attach trace: server main client_id={client_id} first Render queued after {:?}",
                                started_at.elapsed()
                            );
                            attach_traces_waiting_for_first_render.remove(client_id);
                        }
                    }
                } else {
                    // Session is exiting - disconnect all regular clients
                    for client_id in client_ids {
                        let _ = os_input.send_to_client(
                            client_id,
                            ServerToClientMsg::Exit {
                                exit_reason: ExitReason::Normal,
                            },
                        );
                        remove_client!(client_id, os_input, session_state, session_data);
                    }

                    // Also disconnect all watchers
                    let watcher_ids: Vec<ClientId> = session_state
                        .read()
                        .unwrap()
                        .watchers
                        .iter()
                        .copied()
                        .collect();
                    for watcher_id in watcher_ids {
                        let _ = os_input.send_to_client(
                            watcher_id,
                            ServerToClientMsg::Exit {
                                exit_reason: ExitReason::Normal,
                            },
                        );
                        remove_client!(watcher_id, os_input, session_state, session_data);
                    }
                    exit_reason = "the last pane closed";
                    break;
                }
            },
            ServerInstruction::Error(backtrace) => {
                let client_ids = session_state.read().unwrap().client_ids();
                for client_id in client_ids {
                    let _ = os_input.send_to_client(
                        client_id,
                        ServerToClientMsg::Exit {
                            exit_reason: ExitReason::Error(backtrace.clone()),
                        },
                    );
                    remove_client!(client_id, os_input, session_state, session_data);
                }
                exit_reason = "a fatal error in the server";
                break;
            },
            ServerInstruction::ConnStatus(client_id) => {
                // A liveness probe from `assert_socket` in `sessions.rs`. It is
                // answered from this loop on purpose: the probe has a receive
                // deadline, so a loop that is parked — joining threads, stuck
                // on a lock — does not answer in time and reads as unresponsive.
                // That is the property callers rely on: a server that answers
                // here can also take an `AttachClient` next.
                let started_at = std::time::Instant::now();
                log::info!(
                    "attach trace: server main client_id={client_id} ConnStatus reply queue begin"
                );
                let result = os_input.send_to_client(
                    client_id,
                    ServerToClientMsg::Connected {
                        warp_companions: true,
                    },
                );
                log::info!(
                    "attach trace: server main client_id={client_id} ConnStatus reply queued after {:?} ({})",
                    started_at.elapsed(),
                    if result.is_ok() { "ok" } else { "error" }
                );
                // The router queues RemoveClient after ConnStatus. Leave ownership there:
                // removing twice can close a new connection that reuses this client ID.
            },
            ServerInstruction::Log(lines_to_log, client_id, completion_tx) => {
                // A timed-out route may already have released its client ID. Its late reply
                // belongs to that request, never to a new connection reusing the ID.
                if completion_tx
                    .as_ref()
                    .is_some_and(NotificationEnd::is_cancelled)
                {
                    continue;
                }
                send_to_client!(
                    client_id,
                    os_input,
                    ServerToClientMsg::Log {
                        lines: lines_to_log
                    },
                    session_state,
                    session_data
                );
            },
            ServerInstruction::LogError(lines_to_log, client_id, completion_tx) => {
                // A timed-out route may already have released its client ID. Its late reply
                // belongs to that request, never to a new connection reusing the ID.
                if completion_tx
                    .as_ref()
                    .is_some_and(NotificationEnd::is_cancelled)
                {
                    continue;
                }
                send_to_client!(
                    client_id,
                    os_input,
                    ServerToClientMsg::LogError {
                        lines: lines_to_log
                    },
                    session_state,
                    session_data
                );
            },
            ServerInstruction::SwitchSession(mut connect_to_session, client_id, completion_tx) => {
                let current_session_name = envs::get_session_name();
                if connect_to_session.name == current_session_name.ok() {
                    log::error!("Cannot attach to same session");
                } else {
                    let layout_dir = session_data
                        .read()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .session_configuration
                        .get_client_configuration(&client_id)
                        .options
                        .layout_dir
                        .or_else(default_layout_dir);
                    if let Some(layout_dir) = layout_dir {
                        connect_to_session.apply_layout_dir(&layout_dir);
                    }

                    send_to_client!(
                        client_id,
                        os_input,
                        ServerToClientMsg::SwitchSession { connect_to_session },
                        session_state,
                        session_data
                    );
                    remove_client!(client_id, os_input, session_state, session_data);
                    drop(completion_tx); // do not deadlock with route thread

                    session_data
                        .write()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .senders
                        .send_to_screen(ScreenInstruction::RemoveClient(client_id))
                        .unwrap();
                    session_data
                        .write()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .senders
                        .send_to_plugin(PluginInstruction::RemoveClient(client_id))
                        .unwrap();
                }
            },
            ServerInstruction::AssociatePipeWithClient { pipe_id, client_id } => {
                session_state
                    .write()
                    .unwrap()
                    .associate_pipe_with_client(pipe_id, client_id);
            },
            ServerInstruction::ChangeMode(client_id, input_mode, completion) => {
                let mut session_data = session_data.write().unwrap();
                let session_data = session_data.as_mut().unwrap();
                session_data
                    .change_input_mode(client_id, input_mode, completion)
                    .fatal();
            },
            ServerInstruction::ResolveLockedEscape(client_id, token) => {
                let mut guard = session_data
                    .write()
                    .to_anyhow()
                    .context("locking escape state")
                    .fatal();
                if let Some(session) = guard.as_mut() {
                    session.resolve_locked_escape(client_id, token).non_fatal();
                }
            },
            ServerInstruction::ChangeModeForAllClients(input_mode) => {
                session_data
                    .write()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .change_mode_for_all_clients(input_mode);
            },
            ServerInstruction::Reconfigure {
                client_id,
                config,
                write_config_to_disk,
            } => {
                let (new_config, runtime_config_changed) = session_data
                    .write()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .session_configuration
                    .reconfigure_runtime_config(&client_id, config.clone());
                let persistence = match write_config_to_disk {
                    true => ConfigPersistence::Fragment(config),
                    false => ConfigPersistence::None,
                };
                update_new_saved_config(
                    new_config,
                    persistence,
                    runtime_config_changed,
                    &session_data,
                    client_id,
                );
            },
            ServerInstruction::ReconfigureAllClients {
                config,
                write_config_to_disk,
                completion,
            } => {
                // Every client the session is drawing for. A CLI client is in this list too,
                // and harmlessly: it has no configuration of its own and is about to leave.
                let client_ids: Vec<ClientId> = {
                    let state = session_state.read().unwrap();
                    state
                        .client_ids()
                        .into_iter()
                        .filter(|client_id| !state.is_watcher(client_id))
                        .collect()
                };
                let mut write_pending = write_config_to_disk;
                for client_id in client_ids {
                    let (new_config, runtime_config_changed) = session_data
                        .write()
                        .unwrap()
                        .as_mut()
                        .unwrap()
                        .session_configuration
                        .reconfigure_runtime_config(&client_id, config.clone());
                    // Written once: a write propagates the saved config to every client itself.
                    let write_now = std::mem::take(&mut write_pending) && new_config.is_some();
                    let persistence = match write_now {
                        true => ConfigPersistence::Fragment(config.clone()),
                        false => ConfigPersistence::None,
                    };
                    update_new_saved_config(
                        new_config,
                        persistence,
                        runtime_config_changed,
                        &session_data,
                        client_id,
                    );
                }
                drop(completion);
            },
            ServerInstruction::ConfigWrittenToDisk(new_config) => {
                let changes = session_data
                    .write()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .session_configuration
                    .change_saved_config(new_config);
                let config_was_written_to_disk = true;
                session_data
                    .write()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .propagate_configuration_changes(changes, config_was_written_to_disk);
                let client_ids = session_state.read().unwrap().client_ids();
                for client_id in client_ids {
                    send_to_client!(
                        client_id,
                        os_input,
                        ServerToClientMsg::ConfigFileUpdated,
                        session_state,
                        session_data
                    );
                }
            },
            ServerInstruction::FailedToWriteConfigToDisk(_client_id, file_path) => {
                session_data
                    .write()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .senders
                    .send_to_plugin(PluginInstruction::FailedToWriteConfigToDisk { file_path })
                    .unwrap();
            },
            ServerInstruction::RebindKeys {
                client_id,
                keys_to_rebind,
                keys_to_unbind,
                write_config_to_disk,
                plugin_id,
            } => {
                if let Some(plugin_id) = plugin_id {
                    let mut keybinds = Keybinds::default();
                    for (mode, key, actions) in keys_to_rebind {
                        keybinds.0.entry(mode).or_default().insert(key, actions);
                    }
                    let changes = session_data
                        .write()
                        .unwrap()
                        .as_mut()
                        .unwrap()
                        .session_configuration
                        .register_plugin_default_keybinds(plugin_id, keybinds);
                    session_data
                        .write()
                        .unwrap()
                        .as_mut()
                        .unwrap()
                        .propagate_configuration_changes(changes, false);
                    continue;
                }
                let (new_config, runtime_config_changed) = session_data
                    .write()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .session_configuration
                    .rebind_keys(&client_id, keys_to_rebind, keys_to_unbind);

                update_new_saved_config(
                    new_config,
                    match write_config_to_disk {
                        true => ConfigPersistence::WholeConfig,
                        false => ConfigPersistence::None,
                    },
                    runtime_config_changed,
                    &session_data,
                    client_id,
                );
            },
            ServerInstruction::SynchronizedOutputSupport {
                client_id,
                supported,
            } => {
                // Unwrapped like its neighbours would be unsafe here: this one can arrive while
                // the session is being torn down, having come from a client socket rather than
                // from inside the server.
                if let Some(session_data) = session_data.read().unwrap().as_ref() {
                    let _ = session_data.senders.send_to_screen(
                        ScreenInstruction::SetSynchronizedOutputSupport {
                            client_id,
                            supported,
                        },
                    );
                }
            },
            ServerInstruction::ClearMouseHelpText(client_id) => {
                session_data
                    .write()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .senders
                    .send_to_screen(ScreenInstruction::ClearMouseHelpText(client_id))
                    .unwrap();
            },
            ServerInstruction::ClearCommandOutputFlash(pane_id) => {
                session_data
                    .write()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .senders
                    .send_to_screen(ScreenInstruction::ClearCommandOutputFlash(pane_id))
                    .unwrap();
            },
            ServerInstruction::ForwardQueryToHost(token, query_bytes, resolve_async) => {
                // Pick a regular (non-watcher) client to carry the
                // forward. Preference is the most recently active
                // client (whichever last sent input); falls back to
                // any connected client. When the host terminals of
                // attached clients differ, the recently-active one
                // is the best proxy for "what the user is currently
                // looking at".
                let target_client_id = {
                    let mut session = session_state.write().unwrap();
                    let picked = session.pick_forward_target();
                    if let Some(cid) = picked {
                        session.mark_forward_in_flight(token, cid);
                    }
                    picked
                };
                if let Some(client_id) = target_client_id {
                    send_to_client!(
                        client_id,
                        os_input,
                        ServerToClientMsg::ForwardQueryToHost {
                            token,
                            query_bytes,
                            resolve_async
                        },
                        session_state,
                        session_data
                    );
                } else {
                    // No client to ask — synthesize an empty reply
                    // so `Screen`'s in-flight slot releases. Without
                    // this, a detached session (apps still running)
                    // accumulates forwards in `forward_queue` with
                    // no way for them to drain until someone
                    // reattaches.
                    log::warn!(
                        "No connected client to forward host query (token={}); returning empty reply",
                        token
                    );
                    if let Some(session) = session_data.read().unwrap().as_ref() {
                        let _ = session.senders.send_to_screen(
                            ScreenInstruction::ForwardedReplyFromHost {
                                token,
                                reply_bytes: Vec::new(),
                            },
                        );
                    }
                }
            },
            ServerInstruction::KeyPassthroughChanged(
                client_id,
                _old_pane_id,
                new_pane_id,
                should_route,
                entered_from_direction,
                notify_guest,
            ) => {
                let mut session_data = session_data.write().unwrap();
                if let Some(session_data) = session_data.as_mut() {
                    session_data.locked_escapes.remove(&client_id);
                    let previous_pane = session_data
                        .key_passthrough_clients
                        .get(&client_id)
                        .copied();

                    if should_route {
                        if previous_pane != Some(new_pane_id) {
                            if let Some(previous_pane_id) = previous_pane {
                                session_data.key_passthrough_clients.remove(&client_id);
                                let pane_still_active = session_data
                                    .key_passthrough_clients
                                    .values()
                                    .any(|p| *p == previous_pane_id);
                                if !pane_still_active && notify_guest {
                                    if let crate::panes::PaneId::Terminal(terminal_id) =
                                        previous_pane_id
                                    {
                                        let frame = zellij_ipc::nested_session::encode_frame(
                                            &zellij_ipc::nested_session::NestedSessionMessage::FocusLost,
                                        );
                                        let _ = session_data.senders.send_to_pty_writer(
                                            PtyWriteInstruction::Write(frame, terminal_id, None),
                                        );
                                    }
                                }
                            }
                            let pane_already_active = session_data
                                .key_passthrough_clients
                                .values()
                                .any(|p| *p == new_pane_id);
                            session_data
                                .key_passthrough_clients
                                .insert(client_id, new_pane_id);
                            if !pane_already_active {
                                if let crate::panes::PaneId::Terminal(terminal_id) = new_pane_id {
                                    let frame = zellij_ipc::nested_session::encode_frame(
                                        &zellij_ipc::nested_session::NestedSessionMessage::FocusGained {
                                            from_direction: entered_from_direction,
                                        },
                                    );
                                    let _ = session_data.senders.send_to_pty_writer(
                                        PtyWriteInstruction::Write(frame, terminal_id, None),
                                    );
                                }
                            }
                        }
                    } else if previous_pane.is_some() {
                        session_data
                            .remove_key_passthrough_client_with_notify(client_id, notify_guest);
                    }
                }
            },
            ServerInstruction::EmitNestedSessionFrameToClient(client_id, payload_bytes) => {
                send_to_client!(
                    client_id,
                    os_input,
                    ServerToClientMsg::EmitNestedSessionFrame { payload_bytes },
                    session_state,
                    session_data
                );
            },
        }
    }

    log::info!("Server exiting: {exit_reason}");

    // Before the joins below, so that a client connecting from now on — a hook's
    // `zellij action agent-report`, a session probe — is refused at the socket rather than
    // accepted by the listener and parked on `session_state` until the joins finish, which
    // is where a shutdown that waits on a stuck plugin thread spends its time. Once only: a
    // path that already unlinked it has joined its threads since, and a new server may own
    // the path by now.
    if !socket_removed {
        drop(std::fs::remove_file(&socket_path));
    }

    // Drop cached session data before exit. This joins the pty thread, which
    // drops `Pty` and closes every pane — which is what terminates each pane's
    // supervisor, via `clear_terminal_id`.
    //
    // Taken out under the lock and dropped outside it, for the same reason as
    // the teardown in the loop above: those joins must not run with the write
    // lock held, or route threads still blocked on `session_data.read()` stay
    // blocked for the whole shutdown instead of seeing the socket close.
    let tracking = if kill_completion_client.is_some() {
        os_input.begin_confirmed_pty_host_shutdown()
    } else {
        Ok(())
    };
    os_input.begin_firecracker_shutdown();
    let session_metadata = session_data.write().unwrap().take();
    drop(session_metadata);

    // Belt and braces. A supervisor that outlived the sweep above would be a
    // leaked user process holding a live shell, invisible until `pty-host list`.
    os_input.shutdown_pty_hosts();
    // A kill by name also reaches what an earlier incarnation of this session
    // left detached when it died without shutting down. Only on a kill: a detach
    // or a last-client exit is not the user saying those panes are done.
    let owned_teardown = tracking.and_then(|_| os_input.confirm_pty_host_shutdown());
    let cleanup = if kill_session {
        envs::get_session_name()
            .context("missing session name during shutdown")
            .and_then(|name| os_input.terminate_orphaned_pty_hosts(name.as_str()))
    } else {
        Ok(())
    };
    // On every exit path, the fatal-error one included: the pty thread has been
    // joined and every pane closed by now, so nothing in this server still needs
    // its VMs. Only a server that never reaches here — killed, or crashed — leaves
    // them running, which is what `zellij firecracker list` is for.
    os_input.stop_firecracker_session().non_fatal();

    if let Some(client_id) = kill_completion_client {
        let error = owned_teardown
            .and(cleanup)
            .err()
            .map(|e| format!("shutdown incomplete: {e:#}"));
        os_input
            .send_to_client(client_id, ServerToClientMsg::KillSessionComplete { error })
            .non_fatal();
    } else {
        owned_teardown.and(cleanup).non_fatal();
    }

    // Every break above sends the clients an `Exit` first, but sending only
    // queues it — the writer threads are detached, so without this wait the
    // process can exit before the bytes are written and a client that was told
    // why the session ended sees a bare hangup instead.
    os_input.flush_client_writers(CLIENT_WRITER_FLUSH_TIMEOUT);
}

fn init_session(
    os_input: Box<dyn ServerOsApi>,
    to_server: SenderWithContext<ServerInstruction>,
    client_attributes: ClientAttributes,
    config_options: Box<Options>,
    layout: Box<Layout>,
    cli_assets: CliAssets,
    mut config: Config,
    plugin_aliases: PluginAliases,
    client_id: ClientId,
) -> SessionMetaData {
    config.options = config.options.merge(*config_options.clone());

    let _ = SCROLL_BUFFER_SIZE.set(
        config_options
            .scroll_buffer_size
            .unwrap_or(DEFAULT_SCROLL_BUFFER_SIZE),
    );

    let (to_screen, screen_receiver): ChannelWithContext<ScreenInstruction> = channels::unbounded();
    let to_screen = SenderWithContext::new(to_screen);

    let (to_screen_bounded, bounded_screen_receiver): ChannelWithContext<ScreenInstruction> =
        channels::bounded(50);
    let to_screen_bounded = SenderWithContext::new(to_screen_bounded);

    let (to_plugin, plugin_receiver): ChannelWithContext<PluginInstruction> = channels::unbounded();
    let to_plugin = SenderWithContext::new(to_plugin);
    let (to_pty, pty_receiver): ChannelWithContext<PtyInstruction> = channels::unbounded();
    let to_pty = SenderWithContext::new(to_pty);

    let (to_pty_writer, pty_writer_receiver): ChannelWithContext<PtyWriteInstruction> =
        channels::unbounded();
    let to_pty_writer = SenderWithContext::new(to_pty_writer);

    let (to_background_jobs, background_jobs_receiver): ChannelWithContext<BackgroundJob> =
        channels::unbounded();
    let to_background_jobs = SenderWithContext::new(to_background_jobs);

    // Determine and initialize the data directory
    let data_dir = cli_assets.data_dir.unwrap_or_else(get_default_data_dir);
    // Before anything can write to the plugin data store: this is the one place that holds both
    // the data dir and the options the id is resolved from, and every later reader takes what is
    // cached here. See `zellij_runtime::host_identity`.
    zellij_runtime::host_identity::init_host_id(&data_dir, config.options.host_id.as_ref());
    let data_access = Arc::new(DataAccess::configured(
        config.options.plugin_data_url.as_deref(),
        config.options.plugin_data_schema.as_deref(),
        &data_dir,
    ));

    let recording_enabled = config.options.session_recording.unwrap_or(false);
    let input_enabled = config.options.session_recording_input.unwrap_or(false);
    if input_enabled && !recording_enabled {
        log::warn!(
            "session_recording_input is true but session_recording is false; input will not be recorded. Enable session_recording to record input"
        );
        eprintln!(
            "Warning: session_recording_input is true but session_recording is false; input will not be recorded. Enable session_recording to record input"
        );
    }
    let (session_recorder_handle, session_recorder_thread) = if recording_enabled {
        let capture_input = recording_enabled && input_enabled;
        let session_name = envs::get_session_name()
            .ok()
            .or_else(|| config.options.session_name.clone())
            .unwrap_or_else(|| "default".to_string().into());
        match crate::session_recorder::spawn_session_recorder(
            data_access.clone(),
            data_dir.clone(),
            session_name,
            capture_input,
        ) {
            Some((handle, thread)) => (Some(handle), Some(thread)),
            None => {
                log::error!(
                        "Failed to start session recorder for data_dir {:?}; recording will be disabled",
                        data_dir
                    );
                eprintln!(
                        "Warning: failed to start session recording for {:?} — check permissions/data_dir is a directory; recording disabled",
                        data_dir
                    );
                (None, None)
            },
        }
    } else {
        (None, None)
    };

    let serialization_interval = config_options.serialization_interval;
    let disable_session_metadata = config_options.disable_session_metadata.unwrap_or(false);

    // Config only exists once a client has connected, long after the pty backend
    // was built, so this is the first point at which we can know. It is safe to
    // be this late: the pty thread does not exist yet, so no pane can have been
    // spawned. Session-scoped by construction — attaching later with different
    // config will not change it.
    #[cfg(unix)]
    os_input.set_pty_host_enabled(
        config
            .options
            .detachable_panes
            .unwrap_or(pty_host::DEFAULT_DETACHABLE_PANES),
    );
    os_input
        .set_firecracker_provider(
            config.options.firecracker_provider.clone(),
            config.options.firecracker_provider_timeout_secs,
            envs::get_session_name()
                .unwrap_or_else(|_| "default".to_owned().into())
                .to_string(),
        )
        .non_fatal();

    let default_shell = config_options.default_shell.clone().map(|command| {
        TerminalAction::RunCommand(RunCommand {
            command,
            use_terminal_title: true,
            ..Default::default()
        })
    });
    let path_to_default_shell = config_options
        .default_shell
        .clone()
        .unwrap_or_else(get_default_shell);

    let default_mode = config_options.default_mode.unwrap_or_default();
    let default_keybinds = config.keybinds.clone();

    let pty_thread = thread::Builder::new()
        .name("pty".to_string())
        .spawn({
            let layout = layout.clone();
            let session_recorder_handle = session_recorder_handle.clone();
            let mut bus = Bus::new(
                vec![pty_receiver],
                Some(&to_screen_bounded),
                // The pty thread sends to itself: a Firecracker pane is resolved on a
                // worker thread (booting a VM must not block this one) and hands its
                // result back as a `Firecracker*Spawned` instruction. Without this
                // sender that hand-back fails, and the pane - error pane included -
                // is dropped without a trace.
                Some(&to_pty),
                Some(&to_plugin),
                Some(&to_server),
                Some(&to_pty_writer),
                Some(&to_background_jobs),
                Some(os_input.clone()),
            );
            if let Some(handle) = session_recorder_handle {
                bus.senders.session_recorder = Some(handle);
            }
            let pty = Pty::new(
                bus,
                cli_assets.is_debug,
                config_options.scrollback_editor.clone(),
                config_options.post_command_discovery_hook.clone(),
            );

            move || pty_thread_main(pty, layout.clone()).fatal()
        })
        .unwrap();

    let screen_thread = thread::Builder::new()
        .name("screen".to_string())
        .spawn({
            let mut screen_bus = Bus::new(
                vec![screen_receiver, bounded_screen_receiver],
                Some(&to_screen), // there are certain occasions (eg. caching) where the screen
                // needs to send messages to itself
                Some(&to_pty),
                Some(&to_plugin),
                Some(&to_server),
                Some(&to_pty_writer),
                Some(&to_background_jobs),
                Some(os_input.clone()),
            );
            if let Some(handle) = session_recorder_handle.clone() {
                screen_bus.senders.session_recorder = Some(handle);
            }
            let max_panes = cli_assets.max_panes;

            let client_attributes_clone = client_attributes.clone();
            let debug = cli_assets.is_debug;
            let layout = layout.clone();
            let config = config.clone();
            let config_file_path = cli_assets.config_file_path.clone();
            let config_dir = cli_assets.config_dir.clone();
            move || {
                screen_thread_main(
                    screen_bus,
                    max_panes,
                    client_attributes_clone,
                    config,
                    debug,
                    config_file_path,
                    config_dir,
                    layout,
                )
                .fatal();
            }
        })
        .unwrap();

    let zellij_cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut session_env_vars: std::collections::BTreeMap<String, String> =
        std::env::vars().collect();
    // CLI paths are not necessarily present in the server's inherited environment.
    // Expose them to plugins so their child CLI commands use this session's config.
    for (key, path) in [
        (
            zellij_runtime::consts::ZELLIJ_CONFIG_FILE_ENV,
            cli_assets.config_file_path.as_ref(),
        ),
        (
            zellij_runtime::consts::ZELLIJ_CONFIG_DIR_ENV,
            cli_assets.config_dir.as_ref(),
        ),
    ] {
        if let Some(path) = path {
            session_env_vars.insert(key.to_owned(), zellij_cwd.join(path).display().to_string());
        }
    }

    let (available_layouts, available_layout_errors) = get_available_layouts(&config_options);

    let plugin_thread = thread::Builder::new()
        .name("wasm".to_string())
        .spawn({
            let mut plugin_bus = Bus::new(
                vec![plugin_receiver],
                Some(&to_screen_bounded),
                Some(&to_pty),
                Some(&to_plugin),
                Some(&to_server),
                Some(&to_pty_writer),
                Some(&to_background_jobs),
                None,
            );
            if let Some(handle) = session_recorder_handle.clone() {
                plugin_bus.senders.session_recorder = Some(handle);
            }
            let engine = get_engine();

            let layout = layout.clone();
            let default_shell = default_shell.clone();
            let layout_dir = config_options
                .layout_dir
                .clone()
                .or_else(default_layout_dir);
            // The session's *active* config dir, not whichever conventional one happens to exist:
            // a session started with `--config-dir` must load plugins from the folder
            // `zellij setup --check` names for it.
            let user_plugin_dir = user_plugin_dir_or_default(
                config_options.user_plugin_dir.clone(),
                cli_assets.config_dir.clone(),
            );
            let builtin_plugins_dir = config_options.builtin_plugins_dir.clone();
            let background_plugins = config.background_plugins.clone();
            let hooks = config.hooks.clone();
            let context_menu = config.context_menu.clone();
            let session_env_vars = session_env_vars.clone();
            let data_access = data_access.clone();
            move || {
                plugin_thread_main(
                    plugin_bus,
                    engine,
                    data_dir,
                    data_access,
                    layout,
                    layout_dir,
                    user_plugin_dir,
                    available_layouts,
                    available_layout_errors,
                    path_to_default_shell,
                    zellij_cwd,
                    session_env_vars,
                    default_shell,
                    plugin_aliases,
                    default_mode,
                    default_keybinds,
                    background_plugins,
                    hooks,
                    context_menu,
                    builtin_plugins_dir,
                    client_id,
                )
                .fatal()
            }
        })
        .unwrap();

    let pty_writer_thread = thread::Builder::new()
        .name("pty_writer".to_string())
        .spawn({
            let mut pty_writer_bus = Bus::new(
                vec![pty_writer_receiver],
                Some(&to_screen),
                Some(&to_pty),
                Some(&to_plugin),
                Some(&to_server),
                None,
                Some(&to_background_jobs),
                Some(os_input.clone()),
            );
            if let Some(handle) = session_recorder_handle.clone() {
                pty_writer_bus.senders.session_recorder = Some(handle);
            }
            || pty_writer_main(pty_writer_bus).fatal()
        })
        .unwrap();

    let background_jobs_thread = thread::Builder::new()
        .name("background_jobs".to_string())
        .spawn({
            let mut background_jobs_bus = Bus::new(
                vec![background_jobs_receiver],
                Some(&to_screen),
                Some(&to_pty),
                Some(&to_plugin),
                Some(&to_server),
                Some(&to_pty_writer),
                // this thread sends to itself: streamed_web_request reports back with
                // WebRequestStreamFinished so the stream terminator is emitted from the loop
                Some(&to_background_jobs),
                Some(os_input.clone()),
            );
            if let Some(handle) = session_recorder_handle.clone() {
                background_jobs_bus.senders.session_recorder = Some(handle);
            }
            move || {
                background_jobs_main(
                    background_jobs_bus,
                    serialization_interval,
                    disable_session_metadata,
                )
                .fatal()
            }
        })
        .unwrap();
    if let Some(config_file_path) = cli_assets.config_file_path.clone() {
        let layout_dir = config_options
            .layout_dir
            .clone()
            .or_else(default_layout_dir);
        let default_layout_name = config_options
            .default_layout
            .map(|l| format!("{}", l.display()));
        report_changes_in_config_file(
            config_file_path,
            cli_assets.config_dir.as_deref(),
            to_server.clone(),
        );

        // Watch layout directory for changes
        if let Some(layout_dir_path) = layout_dir {
            report_changes_in_layout_dir(
                layout_dir_path,
                default_layout_name,
                to_plugin.clone(),
                to_screen.clone(),
            );
        }
    }

    SessionMetaData {
        senders: ThreadSenders {
            to_screen: Some(to_screen),
            to_pty: Some(to_pty),
            to_plugin: Some(to_plugin),
            to_pty_writer: Some(to_pty_writer),
            to_background_jobs: Some(to_background_jobs),
            to_server: Some(to_server),
            session_recorder: session_recorder_handle.clone(),
            should_silently_fail: false,
        },
        default_shell,
        session_configuration: Default::default(),
        current_input_modes: HashMap::new(),
        screen_thread: Some(screen_thread),
        pty_thread: Some(pty_thread),
        plugin_thread: Some(plugin_thread),
        pty_writer_thread: Some(pty_writer_thread),
        background_jobs_thread: Some(background_jobs_thread),
        session_recorder_thread,
        key_passthrough_clients: HashMap::new(),
        locked_escapes: HashMap::new(),
        config_file_path: cli_assets.config_file_path,
        config_dir: cli_assets.config_dir,
    }
}

fn setup_wizard_floating_pane() -> FloatingPaneLayout {
    let mut setup_wizard_pane = FloatingPaneLayout::new();
    let configuration = BTreeMap::from_iter([("is_setup_wizard".to_owned(), "true".to_owned())]);
    setup_wizard_pane.run = Some(Run::Plugin(RunPluginOrAlias::Alias(PluginAlias::new(
        "configuration",
        &Some(configuration),
        None,
    ))));
    setup_wizard_pane
}

fn about_floating_pane() -> FloatingPaneLayout {
    let mut about_pane = FloatingPaneLayout::new();
    let configuration = BTreeMap::from_iter([("is_release_notes".to_owned(), "true".to_owned())]);
    about_pane.run = Some(Run::Plugin(RunPluginOrAlias::Alias(PluginAlias::new(
        "about",
        &Some(configuration),
        None,
    ))));
    about_pane
}

fn tip_floating_pane() -> FloatingPaneLayout {
    let mut about_pane = FloatingPaneLayout::new();
    let configuration = BTreeMap::from_iter([("is_startup_tip".to_owned(), "true".to_owned())]);
    about_pane.run = Some(Run::Plugin(RunPluginOrAlias::Alias(PluginAlias::new(
        "about",
        &Some(configuration),
        None,
    ))));
    about_pane
}

fn should_show_release_notes(
    should_show_release_notes_config: Option<bool>,
    layout_is_welcome_screen: bool,
) -> bool {
    if layout_is_welcome_screen {
        return false;
    }
    if let Some(should_show_release_notes_config) = should_show_release_notes_config {
        if !should_show_release_notes_config {
            // if we were explicitly told not to show release notes, we don't show them,
            // otherwise we make sure we only show them if they were not seen AND we know
            // we are able to write to the cache
            return false;
        }
    }
    if ZELLIJ_SEEN_RELEASE_NOTES_CACHE_FILE.exists() {
        false
    } else {
        if let Some(parent) = ZELLIJ_SEEN_RELEASE_NOTES_CACHE_FILE.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&*ZELLIJ_SEEN_RELEASE_NOTES_CACHE_FILE, []) {
            log::error!(
                "Failed to write seen release notes indication to disk: {}",
                e
            );
            return false;
        }
        true
    }
}

fn should_show_startup_tip(
    should_show_startup_tip_config: Option<bool>,
    layout_is_welcome_screen: bool,
) -> bool {
    if layout_is_welcome_screen {
        false
    } else {
        should_show_startup_tip_config.unwrap_or(true)
    }
}

fn report_changes_in_config_file(
    config_file_path: PathBuf,
    config_dir: Option<&Path>,
    to_server: SenderWithContext<ServerInstruction>,
) {
    let config_dir = config_dir.map(Path::to_path_buf);
    global_async_runtime::get_tokio_runtime().spawn(async move {
        watch_config_file_changes(config_file_path, config_dir.as_deref(), move |new_config| {
            let to_server = to_server.clone();
            async move {
                let _ = to_server.send(ServerInstruction::ConfigWrittenToDisk(new_config));
            }
        })
        .await;
    });
}

fn report_changes_in_layout_dir(
    layout_dir: PathBuf,
    default_layout_name: Option<String>,
    to_plugin: SenderWithContext<PluginInstruction>,
    to_screen: SenderWithContext<ScreenInstruction>,
) {
    std::thread::spawn(move || {
        let rt = crate::global_async_runtime::get_tokio_runtime();
        rt.block_on(async move {
            watch_layout_dir_changes(
                layout_dir,
                default_layout_name,
                move |new_layouts, layout_errors| {
                    let to_plugin = to_plugin.clone();
                    let to_screen = to_screen.clone();
                    async move {
                        let _ = to_plugin.send(PluginInstruction::LayoutListUpdate(
                            new_layouts.clone(),
                            layout_errors.clone(),
                        ));
                        let _ = to_screen.send(ScreenInstruction::UpdateAvailableLayouts(
                            new_layouts,
                            layout_errors,
                        ));
                    }
                },
            )
            .await;
        });
    });
}

/// How a configuration change is kept, beyond the running session.
enum ConfigPersistence {
    /// Applied to the session only.
    None,
    /// The whole effective config rewrites the user's config file. Only `RebindKeys` still
    /// does this - it has no fragment to write.
    WholeConfig,
    /// The requested KDL fragment goes into `config.d/zellij-managed.kdl`, whatever the
    /// user's own config is written in. See `Config::write_managed_fragment`.
    Fragment(String),
}

fn update_new_saved_config(
    new_config: Option<Config>,
    persistence: ConfigPersistence,
    runtime_config_changed: bool,
    session_data: &Arc<RwLock<Option<SessionMetaData>>>,
    client_id: ClientId,
) {
    if let Some(new_config) = new_config {
        if let ConfigPersistence::Fragment(fragment) = persistence {
            let overlay_dir = {
                let config_dir = session_data
                    .read()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .config_dir
                    .clone();
                zellij_runtime::home::get_config_overlay_dir(
                    config_dir.or_else(zellij_runtime::home::find_default_config_dir),
                )
            };
            let written = match overlay_dir {
                Some(dir) => Config::write_managed_fragment(&dir, &fragment).map_err(|e| {
                    log::error!("Failed to persist the config fragment: {}", e);
                    Some(dir.join(zellij_config::config::MANAGED_CONFIG_FILE_NAME))
                }),
                None => {
                    log::error!("No config directory to persist the config fragment in.");
                    Err(None)
                },
            };
            match written {
                Ok(_) => {
                    // The session's saved config is the running one now, and every client
                    // hears that it was written.
                    let changes = session_data
                        .write()
                        .unwrap()
                        .as_mut()
                        .unwrap()
                        .session_configuration
                        .change_saved_config(new_config);
                    session_data
                        .write()
                        .unwrap()
                        .as_mut()
                        .unwrap()
                        .propagate_configuration_changes(changes, true);
                },
                Err(file_path) => {
                    session_data
                        .read()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .senders
                        .send_to_plugin(PluginInstruction::FailedToWriteConfigToDisk { file_path })
                        .unwrap();
                    if runtime_config_changed {
                        session_data
                            .write()
                            .unwrap()
                            .as_mut()
                            .unwrap()
                            .propagate_configuration_changes(vec![(client_id, new_config)], false);
                    }
                },
            }
            return;
        }
        if matches!(persistence, ConfigPersistence::WholeConfig) {
            let clear_defaults = true;
            let config_file_path = session_data
                .read()
                .unwrap()
                .as_ref()
                .unwrap()
                .config_file_path
                .clone();

            let Some(config_file_path) = config_file_path.as_ref() else {
                log::error!("No config file path found.");
                session_data
                    .write()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .senders
                    .send_to_plugin(PluginInstruction::FailedToWriteConfigToDisk {
                        file_path: None,
                    })
                    .unwrap();
                return;
            };
            match new_config
                .write_config_to_disk(new_config.to_string(clear_defaults), config_file_path)
            {
                Ok(written_config) => {
                    let changes = session_data
                        .write()
                        .unwrap()
                        .as_mut()
                        .unwrap()
                        .session_configuration
                        .change_saved_config(written_config);
                    let config_was_written_to_disk = true;
                    session_data
                        .write()
                        .unwrap()
                        .as_mut()
                        .unwrap()
                        .propagate_configuration_changes(changes, config_was_written_to_disk);
                },
                Err(e) => {
                    let error_path = e
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(String::new);
                    log::error!("Failed to write config to disk: {}", error_path);
                    session_data
                        .write()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .senders
                        .send_to_plugin(PluginInstruction::FailedToWriteConfigToDisk {
                            file_path: e,
                        })
                        .unwrap();
                },
            }
        } else if runtime_config_changed {
            let config_was_written_to_disk = false;
            session_data
                .write()
                .unwrap()
                .as_mut()
                .unwrap()
                .propagate_configuration_changes(
                    vec![(client_id, new_config)],
                    config_was_written_to_disk,
                );
        }
    }
}

// TODO: move elsewhere
fn get_available_layouts(config_options: &Options) -> (Vec<LayoutInfo>, Vec<LayoutWithError>) {
    let layout_dir = config_options
        .layout_dir
        .clone()
        .or_else(default_layout_dir);
    let default_layout_name = config_options
        .default_layout
        .as_ref()
        .map(|l| format!("{}", l.display()));
    zellij_config::layout::list_available_layouts(layout_dir, &default_layout_name)
}

#[cfg(test)]
mod session_state_tests {
    use super::*;
    use std::str::FromStr;

    fn with_client(id: ClientId) -> SessionState {
        let mut s = SessionState::new();
        s.clients.insert(id);
        s
    }

    #[test]
    fn pick_forward_target_prefers_last_active_when_still_connected() {
        let mut s = SessionState::new();
        s.clients.insert(crate::ClientId::new(1));
        s.clients.insert(crate::ClientId::new(2));
        s.set_last_active_client(crate::ClientId::new(2));
        assert_eq!(s.pick_forward_target(), Some(ClientId::new(2)));
    }

    #[test]
    fn plugin_default_keybinds_fill_only_free_slots_and_return_after_unbind() {
        let key = KeyWithModifier::from_str("Ctrl g").unwrap();
        let mut base = Config::default();
        base.keybinds
            .0
            .entry(InputMode::Normal)
            .or_default()
            .insert(key.clone(), vec![Action::Quit]);
        let mut configuration = SessionConfiguration::default();
        configuration.set_saved_configuration(base.clone());
        configuration.set_client_runtime_configuration(crate::ClientId::new(1), base);

        let mut defaults = Keybinds::default();
        defaults.0.entry(InputMode::Normal).or_default().insert(
            key.clone(),
            vec![Action::NewPane {
                direction: None,
                pane_name: None,
                start_suppressed: false,
            }],
        );
        configuration.register_plugin_default_keybinds(crate::PluginId::new(2), defaults);
        assert_eq!(
            configuration
                .get_client_keybinds(&ClientId::new(1))
                .get_actions_for_key_in_mode(&InputMode::Normal, &key),
            Some(&vec![Action::Quit])
        );

        let (_, changed) = configuration.rebind_keys(
            &ClientId::new(1),
            vec![],
            vec![(InputMode::Normal, key.clone())],
        );
        assert!(changed);
        assert_eq!(
            configuration
                .get_client_keybinds(&ClientId::new(1))
                .get_actions_for_key_in_mode(&InputMode::Normal, &key),
            Some(&vec![Action::NewPane {
                direction: None,
                pane_name: None,
                start_suppressed: false,
            }])
        );
    }

    #[test]
    fn lower_plugin_id_wins_default_keybind_conflicts_until_it_unloads() {
        let key = KeyWithModifier::from_str("Alt x").unwrap();
        let mut configuration = SessionConfiguration::default();
        configuration.set_client_runtime_configuration(crate::ClientId::new(1), Config::default());
        for (plugin_id, action) in [(9, Action::Quit), (3, Action::NoOp)] {
            let mut defaults = Keybinds::default();
            defaults
                .0
                .entry(InputMode::Normal)
                .or_default()
                .insert(key.clone(), vec![action]);
            configuration
                .register_plugin_default_keybinds(crate::PluginId::new(plugin_id), defaults);
        }
        assert_eq!(
            configuration
                .get_client_keybinds(&ClientId::new(1))
                .get_actions_for_key_in_mode(&InputMode::Normal, &key),
            Some(&vec![Action::NoOp])
        );

        configuration
            .register_plugin_default_keybinds(crate::PluginId::new(3), Keybinds::default());
        assert_eq!(
            configuration
                .get_client_keybinds(&ClientId::new(1))
                .get_actions_for_key_in_mode(&InputMode::Normal, &key),
            Some(&vec![Action::Quit])
        );
    }

    #[test]
    fn a_late_reply_detects_that_its_requester_stopped_waiting() {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let completion = NotificationEnd::new(sender);
        assert!(!completion.is_cancelled());
        drop(receiver);
        assert!(completion.is_cancelled());
    }

    #[test]
    fn input_unblocks_exclude_cli_connections_and_reused_client_ids() {
        let mut session = SessionState::new();
        let cli = session.new_client();
        let interactive = session.new_client();
        session.interactive_clients.insert(interactive);
        assert_eq!(session.input_unblock_clients(), vec![interactive]);
        session.remove_client(interactive);
        assert_eq!(session.new_client(), interactive);
        assert!(session.input_unblock_clients().is_empty());
        assert!(session.client_ids().contains(&cli));
    }

    #[test]
    fn pick_forward_target_falls_back_when_last_active_disconnected() {
        let mut s = SessionState::new();
        s.clients.insert(crate::ClientId::new(1));
        s.clients.insert(crate::ClientId::new(2));
        // Client 3 was last active but has since disconnected — not in
        // `clients` map anymore. Must fall through to any connected
        // client rather than returning None.
        s.last_active_client = Some(crate::ClientId::new(3));
        let picked = s
            .pick_forward_target()
            .expect("some client still connected");
        assert!(picked == crate::ClientId::new(1) || picked == crate::ClientId::new(2));
    }

    #[test]
    fn pick_forward_target_none_when_no_clients() {
        let s = SessionState::new();
        assert_eq!(s.pick_forward_target(), None);
    }

    #[test]
    fn remove_client_returns_stuck_forward_tokens() {
        let mut s = with_client(crate::ClientId::new(1));
        s.mark_forward_in_flight(10, crate::ClientId::new(1));
        s.mark_forward_in_flight(11, crate::ClientId::new(1));
        // A forward dispatched to a *different* client — must not be
        // returned when we remove client 1.
        s.clients.insert(crate::ClientId::new(2));
        s.mark_forward_in_flight(12, crate::ClientId::new(2));

        let mut stuck = s.remove_client(crate::ClientId::new(1));
        stuck.sort_unstable();
        assert_eq!(stuck, vec![10, 11]);
        assert_eq!(s.forwards_in_flight.get(&12), Some(&ClientId::new(2)));
    }

    #[test]
    fn remove_client_returns_empty_when_no_forwards_in_flight() {
        let mut s = with_client(crate::ClientId::new(1));
        assert!(s.remove_client(crate::ClientId::new(1)).is_empty());
    }

    #[test]
    fn clear_forward_in_flight_removes_entry() {
        let mut s = with_client(crate::ClientId::new(1));
        s.mark_forward_in_flight(42, crate::ClientId::new(1));
        s.clear_forward_in_flight(42);
        // After clear, removing the client yields no stuck tokens.
        assert!(s.remove_client(crate::ClientId::new(1)).is_empty());
    }
}
